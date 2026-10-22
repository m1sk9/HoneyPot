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
mod i18n;
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

/// Installs `ring` as the process-wide rustls crypto provider.
///
/// twilight builds rustls with no provider enabled, so one must be installed
/// before the first TLS connection or rustls panics.
fn install_crypto_provider() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("install rustls crypto provider once");
}

/// The bot's presence: a custom status reading `Running v{version}`.
#[cfg(not(feature = "preview"))]
fn presence() -> twilight_model::gateway::payload::outgoing::update_presence::UpdatePresencePayload
{
    use twilight_model::gateway::payload::outgoing::update_presence::UpdatePresencePayload;
    use twilight_model::gateway::presence::{Activity, ActivityType, MinimalActivity, Status};

    let mut activity = Activity::from(MinimalActivity {
        kind: ActivityType::Custom,
        name: "Custom Status".into(),
        url: None,
    });
    // A custom status renders `state`, not `name`; without it the status is blank.
    activity.state = Some(format!("Running v{}", env!("CARGO_PKG_VERSION")));
    UpdatePresencePayload::new(vec![activity], false, None, Status::Online)
        .expect("one activity is always supplied")
}

/// Normal operation: load the guild config and connect to the gateway.
#[cfg(not(feature = "preview"))]
async fn run() -> Result<(), HoneyPotError> {
    use crate::discord::handler;
    use crate::settings::HoneyPotConfig;
    use futures_util::StreamExt;
    use std::sync::Arc;
    use twilight_gateway::{ConfigBuilder, Event, Intents, Message, Shard, ShardId, ShardState};

    install_crypto_provider();

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
    let intents = Intents::GUILDS
        | Intents::GUILD_MEMBERS
        | Intents::GUILD_MESSAGES
        | Intents::GUILD_MODERATION
        | Intents::MESSAGE_CONTENT;

    let http = Arc::new(twilight_http::Client::new(token.clone()));
    let config = ConfigBuilder::new(token, intents)
        .presence(presence())
        .build();
    // A single shard is plenty for the handful of guilds one deployment serves.
    let mut shard = Shard::with_config(ShardId::ONE, config);

    // The shard is polled as a raw message stream (not `next_event`) so each
    // event's JSON text survives for `discord::raw`, which recovers the account
    // signals twilight-model drops.
    while let Some(item) = shard.next().await {
        match item {
            Ok(Message::Text(json)) => {
                match twilight_gateway::parse(json.clone(), handler::WANTED_EVENTS) {
                    Ok(Some(event)) => {
                        let http = Arc::clone(&http);
                        // Handled on its own task: the role path sleeps between
                        // audit-log polls, which must not stall the shard loop.
                        tokio::spawn(async move {
                            handler::handle(&http, Event::from(event), &json).await;
                        });
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!(%error, "failed to parse gateway event"),
                }
            }
            Ok(Message::Close(frame)) => {
                if matches!(shard.state(), ShardState::FatallyClosed) {
                    tracing::error!(?frame, "gateway connection closed fatally");
                } else {
                    tracing::warn!(?frame, "gateway connection closed; shard will reconnect");
                }
            }
            Err(error) => tracing::warn!(%error, "error receiving gateway message"),
        }
    }

    // The stream only ends once the shard has closed fatally (e.g. an invalid
    // token or disallowed intents), so surface that as a failed run.
    Err(HoneyPotError::GatewayClosed)
}

/// Preview mode (`preview` feature): post one of each honeypot log embed to the
/// channel named by [`discord::preview::PREVIEW_CHANNEL_ENV`], then exit. Reads
/// no guild config and opens no gateway connection.
#[cfg(feature = "preview")]
async fn run() -> Result<(), HoneyPotError> {
    use crate::discord::preview;
    use twilight_model::id::Id;

    install_crypto_provider();

    let token = std::env::var(BOT_TOKEN_ENV)
        .map_err(|_| HoneyPotError::MissingEnv(BOT_TOKEN_ENV.to_string()))?;
    let channel = std::env::var(preview::PREVIEW_CHANNEL_ENV)
        .map_err(|_| HoneyPotError::MissingEnv(preview::PREVIEW_CHANNEL_ENV.to_string()))?;
    let channel_id = channel
        .parse::<u64>()
        .ok()
        .and_then(Id::new_checked)
        .ok_or_else(|| HoneyPotError::InvalidEnv(preview::PREVIEW_CHANNEL_ENV.to_string()))?;

    tracing::warn!(
        %channel_id,
        "preview feature active: posting embed previews, then exiting"
    );
    preview::run(token, channel_id).await
}
