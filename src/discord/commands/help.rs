//! `/help` — lists every command with its description in the guild's language.

use crate::discord::commands;
use crate::i18n::Language;
use twilight_http::Client;
use twilight_model::application::interaction::Interaction;
use twilight_model::channel::message::Embed;
use twilight_model::id::{Id, marker::GuildMarker};
use twilight_util::builder::embed::EmbedBuilder;

pub(super) async fn run(http: &Client, interaction: &Interaction) {
    let language = commands::language_for(interaction.guild_id);
    commands::respond_embed(
        http,
        interaction,
        build_embed(language, interaction.guild_id),
    )
    .await;
}

/// Builds the command-list embed for `language`.
///
/// Each command is rendered as a clickable slash-command mention resolved for
/// `guild_id` (see [`commands::command_mentions`]).
pub(crate) fn build_embed(language: Language, guild_id: Option<Id<GuildMarker>>) -> Embed {
    let msg = language.messages();
    let entries = [
        (commands::HELP, msg.cmd_help_desc),
        (commands::VERSION, msg.cmd_version_desc),
        (commands::PING, msg.cmd_ping_desc),
        (commands::WHOIS, msg.cmd_whois_desc),
        (commands::DOCTOR, msg.cmd_doctor_desc),
    ];
    let names: Vec<&str> = entries.iter().map(|(name, _)| *name).collect();
    let body = commands::command_mentions(&names, guild_id)
        .into_iter()
        .zip(entries)
        .map(|(mention, (_, desc))| format!("{mention} — {desc}"))
        .collect::<Vec<_>>()
        .join("\n");

    EmbedBuilder::new()
        .title(msg.help_title)
        .description(body)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_embed_lists_every_command() {
        let embed = build_embed(Language::En, None);
        let description = embed.description.expect("description present");
        // No registration runs in tests, so mentions fall back to plain `/name`.
        for name in [
            commands::HELP,
            commands::VERSION,
            commands::PING,
            commands::WHOIS,
            commands::DOCTOR,
        ] {
            assert!(
                description.contains(&format!("/{name}")),
                "missing {name} in help body"
            );
        }
    }
}
