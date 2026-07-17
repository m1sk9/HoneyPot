//! `/ping` — reports the bot's REST round-trip latency.
//!
//! The latency is measured as the wall-clock time to acknowledge the interaction
//! (the deferred-response REST call), then reported by editing that reply. This
//! needs no cache and no shard-latency plumbing, so it works with serenity's
//! cache disabled.

use crate::discord::commands;
use crate::i18n::Language;
use serenity::all::{
    CommandInteraction, Context, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, EditInteractionResponse,
};
use std::time::Instant;

pub(super) async fn run(ctx: &Context, command: &CommandInteraction) {
    let language = commands::language_for(command.guild_id);

    let started = Instant::now();
    let defer =
        CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true));
    if let Err(error) = command.create_response(&ctx.http, defer).await {
        tracing::error!(%error, "failed to acknowledge ping");
        return;
    }
    let latency = started.elapsed().as_millis();

    let embed = build_embed(language, latency);
    if let Err(error) = command
        .edit_response(&ctx.http, EditInteractionResponse::new().embed(embed))
        .await
    {
        tracing::error!(%error, "failed to report ping latency");
    }
}

/// Builds the latency embed for `language`, reporting `latency_ms`.
pub(crate) fn build_embed(language: Language, latency_ms: u128) -> CreateEmbed {
    let msg = language.messages();
    CreateEmbed::new()
        .title(msg.ping_title)
        .description(msg.pong.replace("{}", &latency_ms.to_string()))
}
