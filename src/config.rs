//! Configuration file loading for HoneyPot.
//!
//! This module owns the raw representation of `config.toml` as it appears on
//! disk. IDs are stored as bare integers (`u64`) here; they are converted into
//! serenity ID types in [`crate::settings`].

use crate::error::HoneyPotError;
use crate::i18n::Language;
use serde::Deserialize;

/// The top-level structure of `config.toml`.
///
/// Guilds are declared as a `[[guilds]]` array, allowing a single deployment
/// to serve multiple guilds.
#[derive(Deserialize, Debug, Default)]
pub struct HoneyPotConfigFile {
    /// Per-guild honeypot configuration.
    #[serde(default)]
    pub guilds: Vec<GuildConfigEntry>,
}

/// Honeypot configuration for a single guild.
#[derive(Deserialize, Debug, Clone)]
pub struct GuildConfigEntry {
    /// The Discord guild (server) ID.
    pub guild_id: u64,
    /// Roles that trigger a ban when acquired.
    #[serde(default)]
    pub honeypot_role_ids: Vec<u64>,
    /// Channels that trigger a ban when a message is posted.
    #[serde(default)]
    pub honeypot_channel_ids: Vec<u64>,
    /// Bot user IDs exempt from the honeypot.
    ///
    /// A bot that trips a honeypot is normally not auto-banned but flagged for
    /// manual review; bots listed here are ignored entirely, so well-behaved
    /// bots (e.g. link expanders that echo into a honeypot channel) are never
    /// flagged.
    #[serde(default)]
    pub trusted_bot_ids: Vec<u64>,
    /// Channel where ban notifications are sent.
    pub log_channel_id: u64,
    /// Language for this guild's moderator-facing text (log embeds and button
    /// responses). Defaults to English when omitted.
    #[serde(default)]
    pub language: Language,
}

/// Reads and parses the configuration file at `path`.
pub fn load(path: &str) -> Result<HoneyPotConfigFile, HoneyPotError> {
    let buffer = std::fs::read_to_string(path)?;
    let config: HoneyPotConfigFile = toml::from_str(&buffer)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_empty_config() {
        let config: HoneyPotConfigFile = toml::from_str("").unwrap();
        assert!(config.guilds.is_empty());
    }

    #[test]
    fn deserialize_single_guild() {
        let toml_str = r#"
            [[guilds]]
            guild_id             = 100000000000000000
            honeypot_role_ids    = [200000000000000000]
            honeypot_channel_ids = [300000000000000000]
            log_channel_id       = 400000000000000000
        "#;
        let config: HoneyPotConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.guilds.len(), 1);
        let guild = &config.guilds[0];
        assert_eq!(guild.guild_id, 100000000000000000);
        assert_eq!(guild.honeypot_role_ids, vec![200000000000000000]);
        assert_eq!(guild.honeypot_channel_ids, vec![300000000000000000]);
        assert_eq!(guild.log_channel_id, 400000000000000000);
    }

    #[test]
    fn deserialize_multiple_guilds() {
        let toml_str = r#"
            [[guilds]]
            guild_id             = 100000000000000000
            honeypot_role_ids    = [200000000000000000]
            honeypot_channel_ids = [300000000000000000]
            log_channel_id       = 400000000000000000

            [[guilds]]
            guild_id             = 111111111111111111
            honeypot_role_ids    = [222222222222222222]
            honeypot_channel_ids = []
            log_channel_id       = 444444444444444444
        "#;
        let config: HoneyPotConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.guilds.len(), 2);
        assert!(config.guilds[1].honeypot_channel_ids.is_empty());
    }

    #[test]
    fn deserialize_defaults_optional_arrays() {
        let toml_str = r#"
            [[guilds]]
            guild_id       = 100000000000000000
            log_channel_id = 400000000000000000
        "#;
        let config: HoneyPotConfigFile = toml::from_str(toml_str).unwrap();
        assert!(config.guilds[0].honeypot_role_ids.is_empty());
        assert!(config.guilds[0].honeypot_channel_ids.is_empty());
        assert!(config.guilds[0].trusted_bot_ids.is_empty());
    }

    #[test]
    fn deserialize_language_defaults_to_english() {
        let toml_str = r#"
            [[guilds]]
            guild_id       = 100000000000000000
            log_channel_id = 400000000000000000
        "#;
        let config: HoneyPotConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.guilds[0].language, Language::En);
    }

    #[test]
    fn deserialize_explicit_language() {
        let toml_str = r#"
            [[guilds]]
            guild_id       = 100000000000000000
            log_channel_id = 400000000000000000
            language       = "ja"
        "#;
        let config: HoneyPotConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.guilds[0].language, Language::Ja);
    }

    #[test]
    fn deserialize_trusted_bot_ids() {
        let toml_str = r#"
            [[guilds]]
            guild_id        = 100000000000000000
            trusted_bot_ids = [500000000000000000, 600000000000000000]
            log_channel_id  = 400000000000000000
        "#;
        let config: HoneyPotConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.guilds[0].trusted_bot_ids,
            vec![500000000000000000, 600000000000000000]
        );
    }
}
