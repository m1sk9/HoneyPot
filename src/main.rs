//! HoneyPot - a Discord bot that automatically bans spam bots.
//!
//! Bots (and users) that step into a configured honeypot — acquiring a
//! honeypot role or posting in a honeypot channel — are banned. This binary
//! bootstraps configuration, logging, and the Discord client.
//!
//! Built with the `preview` feature, [`run`] instead posts a sample of every
//! log embed to a channel and exits (see [`discord::preview`]); the default
//! build compiles only the normal gateway path.

// Under the `preview` feature the normal gateway path (config loading, the event
// handler, …) is compiled but unused, since `run` posts previews and exits.
// Silence dead-code warnings for that debug build only; the default (production)
// build keeps full dead-code checking.
#![cfg_attr(feature = "preview", allow(dead_code))]

mod config;
mod discord;
mod error;
mod settings;

use crate::error::HoneyPotError;
use tracing_subscriber::EnvFilter;

/// Environment variable holding the Discord bot token.
const BOT_TOKEN_ENV: &str = "HONEYPOT_BOT_TOKEN";

#[tokio::main]
async fn main() -> Result<(), HoneyPotError> {
    // Load a local `.env` if present so `RUST_LOG`, the bot token, and other
    // settings can be supplied without exporting them. A missing file is not an
    // error (production supplies real environment variables); other parse errors
    // are surfaced. Runs before anything reads the environment.
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(HoneyPotError::Dotenv(error));
    }

    // `RUST_LOG` takes precedence; otherwise default this crate to `info`.
    // The tracing target root is the crate name, so derive it from
    // `CARGO_CRATE_NAME` to stay correct across renames.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(concat!(env!("CARGO_CRATE_NAME"), "=info")));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .init();

    run().await
}

/// Normal operation: load the guild config and connect to the gateway.
#[cfg(not(feature = "preview"))]
async fn run() -> Result<(), HoneyPotError> {
    use crate::discord::handler::HoneyPotEventHandler;
    use crate::settings::HoneyPotConfig;
    use serenity::Client;
    use serenity::all::GatewayIntents;

    HoneyPotConfig::init()?;
    tracing::debug!("Config: {:?}", HoneyPotConfig::get());

    // Priming the cached flag here also surfaces the mode in the startup logs.
    if settings::dry_run() {
        tracing::warn!("HONEYPOT_DRY_RUN enabled: bans and unbans are simulated, not executed");
    }

    let token = std::env::var(BOT_TOKEN_ENV)
        .map_err(|_| HoneyPotError::MissingEnv(BOT_TOKEN_ENV.to_string()))?;

    // `GUILD_MEMBERS` and `MESSAGE_CONTENT` are privileged intents and must be
    // enabled in the Discord Developer Portal, or the gateway rejects the
    // connection. `MESSAGE_CONTENT` is required so a message-triggered ban can
    // record the offending message in its log embed, letting a moderator verify
    // it really was spam (and catch a mistaken ban).
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MODERATION
        | GatewayIntents::MESSAGE_CONTENT;

    let mut client = Client::builder(&token, intents)
        .event_handler(HoneyPotEventHandler)
        .await?;

    client.start().await?;

    Ok(())
}

/// Preview mode (`preview` feature): post one of each honeypot log embed to the
/// channel named by [`discord::preview::PREVIEW_CHANNEL_ENV`], then exit. Reads
/// no guild config and opens no gateway connection.
#[cfg(feature = "preview")]
async fn run() -> Result<(), HoneyPotError> {
    use crate::discord::preview;
    use serenity::all::ChannelId;

    let token = std::env::var(BOT_TOKEN_ENV)
        .map_err(|_| HoneyPotError::MissingEnv(BOT_TOKEN_ENV.to_string()))?;
    let channel = std::env::var(preview::PREVIEW_CHANNEL_ENV)
        .map_err(|_| HoneyPotError::MissingEnv(preview::PREVIEW_CHANNEL_ENV.to_string()))?;
    let channel_id = channel
        .parse::<u64>()
        .map_err(|_| HoneyPotError::InvalidEnv(preview::PREVIEW_CHANNEL_ENV.to_string()))?;

    tracing::warn!(
        channel_id,
        "preview feature active: posting embed previews, then exiting"
    );
    preview::run(&token, ChannelId::new(channel_id)).await
}
