//! `/ping` — reports the bot's REST round-trip latency.
//!
//! The latency is measured as the wall-clock time to acknowledge the interaction
//! (the deferred-response REST call), then reported by editing that reply. This
//! needs no cache and no shard-latency plumbing.

use crate::discord::commands;
use crate::i18n::Language;
use std::time::Instant;
use twilight_http::Client;
use twilight_model::application::interaction::Interaction;
use twilight_model::channel::message::Embed;
use twilight_util::builder::embed::EmbedBuilder;

pub(super) async fn run(http: &Client, interaction: &Interaction) {
    let language = commands::language_for(interaction.guild_id);

    let started = Instant::now();
    if let Err(error) = commands::defer_ephemeral(http, interaction).await {
        tracing::error!(%error, "failed to acknowledge ping");
        return;
    }
    let latency = started.elapsed().as_millis();

    let embed = build_embed(language, latency);
    if let Err(error) = commands::edit_response_embed(http, interaction, embed).await {
        tracing::error!(%error, "failed to report ping latency");
    }
}

/// Builds the latency embed for `language`, reporting `latency_ms`.
pub(crate) fn build_embed(language: Language, latency_ms: u128) -> Embed {
    let msg = language.messages();
    EmbedBuilder::new()
        .title(msg.ping_title)
        .description(msg.pong.replace("{}", &latency_ms.to_string()))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_embed_reports_the_latency() {
        let embed = build_embed(Language::En, 42);
        assert!(embed.description.unwrap().contains("42"));
    }
}
