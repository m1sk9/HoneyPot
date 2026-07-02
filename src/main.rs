//! HoneyPot - a Discord bot that automatically bans spam bots.
//!
//! Bots (and users) that step into a configured honeypot — acquiring a
//! honeypot role or posting in a honeypot channel — are banned. This binary
//! bootstraps configuration, logging, and the Discord client.

mod config;
mod discord;
mod error;
mod settings;

use crate::discord::handler::HoneyPotEventHandler;
use crate::error::HoneyPotError;
use crate::settings::HoneyPotConfig;
use serenity::Client;
use serenity::all::GatewayIntents;
use tracing_subscriber::EnvFilter;

/// Environment variable holding the Discord bot token.
const BOT_TOKEN_ENV: &str = "HONEYPOT_BOT_TOKEN";

#[tokio::main]
async fn main() -> Result<(), HoneyPotError> {
    // `RUST_LOG` takes precedence; otherwise default this crate to `info`.
    // The tracing target root is the crate name, so derive it from
    // `CARGO_CRATE_NAME` to stay correct across renames.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(concat!(env!("CARGO_CRATE_NAME"), "=info")));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    HoneyPotConfig::init()?;
    tracing::debug!("Config: {:?}", HoneyPotConfig::get());

    let token = std::env::var(BOT_TOKEN_ENV)
        .map_err(|_| HoneyPotError::MissingEnv(BOT_TOKEN_ENV.to_string()))?;

    // `GUILD_MEMBERS` is a privileged intent and must be enabled in the
    // Discord Developer Portal. `MESSAGE_CONTENT` is intentionally omitted:
    // only the fact that a message was posted matters, not its content.
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MODERATION;

    let mut client = Client::builder(&token, intents)
        .event_handler(HoneyPotEventHandler)
        .await?;

    client.start().await?;

    Ok(())
}
