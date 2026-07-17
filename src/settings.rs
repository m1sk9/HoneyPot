//! Global runtime settings for HoneyPot.
//!
//! The raw configuration loaded by [`crate::config`] is converted into
//! serenity ID types and expanded into a `HashMap` keyed by [`GuildId`], so
//! event handlers can look up a guild's honeypot configuration in O(1).

use crate::config::{self, GuildConfigEntry};
use crate::error::HoneyPotError;
use crate::i18n::Language;
use serenity::all::{ChannelId, GuildId, RoleId, UserId};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Global configuration instance.
pub static SETTINGS: OnceLock<HoneyPotConfig> = OnceLock::new();

/// Environment variable that overrides the configuration file path.
const CONFIG_PATH_ENV: &str = "HONEYPOT_CONFIG_PATH";

/// Default configuration file path used when [`CONFIG_PATH_ENV`] is unset.
const DEFAULT_CONFIG_PATH: &str = "config/config.toml";

/// Environment variable that enables dry-run mode (bans/unbans are simulated).
const DRY_RUN_ENV: &str = "HONEYPOT_DRY_RUN";

/// Cached dry-run flag, read once from the environment.
static DRY_RUN: OnceLock<bool> = OnceLock::new();

/// Environment variable that selects the slash-command registration scope.
const COMMAND_SCOPE_ENV: &str = "HONEYPOT_COMMAND_SCOPE";

/// Cached command scope, read once from the environment.
static COMMAND_SCOPE: OnceLock<CommandScope> = OnceLock::new();

/// Where slash commands are registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandScope {
    /// Registered once for every guild in the configuration. Propagates
    /// instantly, so it is convenient during development.
    Guild,
    /// Registered globally across every guild the bot is in. Discord can take up
    /// to an hour to propagate these, so it is the production default.
    Global,
}

/// Parses a command-scope env value: `guild` (case-insensitive, trimmed) selects
/// per-guild registration; anything else selects global.
fn parse_command_scope(value: &str) -> CommandScope {
    match value.trim().to_ascii_lowercase().as_str() {
        "guild" => CommandScope::Guild,
        _ => CommandScope::Global,
    }
}

/// The slash-command registration scope.
///
/// Reads [`COMMAND_SCOPE_ENV`] once and caches it. `guild` selects per-guild
/// registration (instant, for development); anything else — including the unset
/// default — selects global registration.
pub fn command_scope() -> CommandScope {
    *COMMAND_SCOPE.get_or_init(|| {
        std::env::var(COMMAND_SCOPE_ENV)
            .map(|value| parse_command_scope(&value))
            .unwrap_or(CommandScope::Global)
    })
}

/// Parses a dry-run env value: enabled on `1`/`true` (case-insensitive, trimmed).
fn parse_dry_run(value: &str) -> bool {
    matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true")
}

/// Whether dry-run mode is enabled.
///
/// In dry-run, the ban/unban HTTP calls are skipped (treated as success) but
/// every other path — detection, log embeds, buttons, and the [`crate::discord::ban`]
/// claim tracking — runs for real, so the full flow can be exercised on a normal
/// account without anyone actually being banned. Read once from [`DRY_RUN_ENV`]
/// and cached; the value is fixed for the lifetime of the process.
pub fn dry_run() -> bool {
    *DRY_RUN.get_or_init(|| {
        std::env::var(DRY_RUN_ENV)
            .map(|value| parse_dry_run(&value))
            .unwrap_or(false)
    })
}

/// Runtime configuration for all guilds served by this deployment.
#[derive(Debug)]
pub struct HoneyPotConfig {
    guilds: HashMap<GuildId, GuildConfig>,
}

/// Honeypot configuration for a single guild, using serenity ID types.
#[derive(Debug)]
pub struct GuildConfig {
    /// Roles that trigger a ban when acquired.
    pub honeypot_role_ids: Vec<RoleId>,
    /// Channels that trigger a ban when a message is posted.
    pub honeypot_channel_ids: Vec<ChannelId>,
    /// Bot user IDs exempt from the honeypot (never flagged or banned).
    pub trusted_bot_ids: Vec<UserId>,
    /// Channel where ban notifications are sent.
    pub log_channel_id: ChannelId,
    /// Language for this guild's moderator-facing text.
    pub language: Language,
}

impl From<GuildConfigEntry> for GuildConfig {
    fn from(entry: GuildConfigEntry) -> Self {
        Self {
            honeypot_role_ids: entry
                .honeypot_role_ids
                .into_iter()
                .map(RoleId::new)
                .collect(),
            honeypot_channel_ids: entry
                .honeypot_channel_ids
                .into_iter()
                .map(ChannelId::new)
                .collect(),
            trusted_bot_ids: entry.trusted_bot_ids.into_iter().map(UserId::new).collect(),
            log_channel_id: ChannelId::new(entry.log_channel_id),
            language: entry.language,
        }
    }
}

impl HoneyPotConfig {
    /// Builds a [`HoneyPotConfig`] from raw configuration entries.
    fn from_entries(entries: Vec<GuildConfigEntry>) -> Self {
        let guilds = entries
            .into_iter()
            .map(|entry| (GuildId::new(entry.guild_id), GuildConfig::from(entry)))
            .collect();
        Self { guilds }
    }

    /// Initializes the global configuration.
    ///
    /// Reads the file at `HONEYPOT_CONFIG_PATH` (defaulting to `config.toml`),
    /// converts it into serenity ID types, and stores it in [`SETTINGS`].
    pub fn init() -> Result<(), HoneyPotError> {
        let path =
            std::env::var(CONFIG_PATH_ENV).unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_string());
        let file = config::load(&path)?;
        let config = HoneyPotConfig::from_entries(file.guilds);
        SETTINGS
            .set(config)
            .map_err(|_| HoneyPotError::AlreadyInitialized)
    }

    /// Returns a reference to the global configuration.
    ///
    /// # Panics
    ///
    /// Panics if [`HoneyPotConfig::init`] has not been called.
    pub fn get() -> &'static HoneyPotConfig {
        SETTINGS
            .get()
            .expect("Configuration has not been initialized.")
    }

    /// Returns the honeypot configuration for `guild_id`, if any.
    pub fn guild(&self, guild_id: GuildId) -> Option<&GuildConfig> {
        self.guilds.get(&guild_id)
    }

    /// Returns every configured guild's ID, for per-guild command registration.
    pub fn guild_ids(&self) -> impl Iterator<Item = GuildId> + '_ {
        self.guilds.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(guild_id: u64) -> GuildConfigEntry {
        GuildConfigEntry {
            guild_id,
            honeypot_role_ids: vec![200000000000000000],
            honeypot_channel_ids: vec![300000000000000000],
            trusted_bot_ids: vec![500000000000000000],
            log_channel_id: 400000000000000000,
            language: Language::En,
        }
    }

    #[test]
    fn lookup_registered_guild() {
        let config = HoneyPotConfig::from_entries(vec![entry(100000000000000000)]);
        let guild = config.guild(GuildId::new(100000000000000000)).unwrap();
        assert_eq!(
            guild.honeypot_role_ids,
            vec![RoleId::new(200000000000000000)]
        );
        assert_eq!(
            guild.honeypot_channel_ids,
            vec![ChannelId::new(300000000000000000)]
        );
        assert_eq!(guild.trusted_bot_ids, vec![UserId::new(500000000000000000)]);
        assert_eq!(guild.log_channel_id, ChannelId::new(400000000000000000));
    }

    #[test]
    fn lookup_unregistered_guild_returns_none() {
        let config = HoneyPotConfig::from_entries(vec![entry(100000000000000000)]);
        assert!(config.guild(GuildId::new(999999999999999999)).is_none());
    }

    #[test]
    fn from_entries_expands_all_guilds() {
        let config = HoneyPotConfig::from_entries(vec![entry(1), entry(2)]);
        assert!(config.guild(GuildId::new(1)).is_some());
        assert!(config.guild(GuildId::new(2)).is_some());
    }

    #[test]
    fn parse_dry_run_enabled_values() {
        for value in ["1", "true", "TRUE", " true ", "True"] {
            assert!(parse_dry_run(value), "expected {value:?} to enable dry-run");
        }
    }

    #[test]
    fn parse_dry_run_disabled_values() {
        for value in ["0", "false", "", "yes", "on", "off"] {
            assert!(
                !parse_dry_run(value),
                "expected {value:?} to leave dry-run disabled"
            );
        }
    }

    #[test]
    fn parse_command_scope_selects_guild_only_for_guild_value() {
        for value in ["guild", "GUILD", " guild ", "Guild"] {
            assert_eq!(parse_command_scope(value), CommandScope::Guild);
        }
    }

    #[test]
    fn parse_command_scope_defaults_to_global() {
        for value in ["global", "", "prod", "GLOBAL", "gild"] {
            assert_eq!(parse_command_scope(value), CommandScope::Global);
        }
    }
}
