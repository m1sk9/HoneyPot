//! Global runtime settings for HoneyPot.
//!
//! The raw configuration loaded by [`crate::config`] is converted into
//! serenity ID types and expanded into a `HashMap` keyed by [`GuildId`], so
//! event handlers can look up a guild's honeypot configuration in O(1).

use crate::config::{self, GuildConfigEntry};
use crate::error::HoneyPotError;
use serenity::all::{ChannelId, GuildId, RoleId};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Global configuration instance.
pub static SETTINGS: OnceLock<HoneyPotConfig> = OnceLock::new();

/// Environment variable that overrides the configuration file path.
const CONFIG_PATH_ENV: &str = "HONEYPOT_CONFIG_PATH";

/// Default configuration file path used when [`CONFIG_PATH_ENV`] is unset.
const DEFAULT_CONFIG_PATH: &str = "config.toml";

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
    /// Channel where ban notifications are sent.
    pub log_channel_id: ChannelId,
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
            log_channel_id: ChannelId::new(entry.log_channel_id),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(guild_id: u64) -> GuildConfigEntry {
        GuildConfigEntry {
            guild_id,
            honeypot_role_ids: vec![200000000000000000],
            honeypot_channel_ids: vec![300000000000000000],
            log_channel_id: 400000000000000000,
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
}
