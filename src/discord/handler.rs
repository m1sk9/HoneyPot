//! Discord event handler.
//!
//! Currently only the `ready` event is handled. Honeypot logic (ban on role
//! acquisition / channel post, log embeds, and the unban button) is added in
//! follow-up work.

use serenity::all::{Context, EventHandler, Ready};

/// Event handler for HoneyPot.
pub struct HoneyPotEventHandler;

#[serenity::async_trait]
impl EventHandler for HoneyPotEventHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        tracing::info!("Running {}, {} is connected!", version, ready.user.name);
    }
}
