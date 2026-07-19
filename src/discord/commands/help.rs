//! `/help` — lists every command with its description in the guild's language.

use crate::discord::commands;
use crate::i18n::Language;
use serenity::all::{CommandInteraction, Context, CreateEmbed, GuildId};

pub(super) async fn run(ctx: &Context, command: &CommandInteraction) {
    let language = commands::language_for(command.guild_id);
    commands::respond_embed(ctx, command, build_embed(language, command.guild_id)).await;
}

/// Builds the command-list embed for `language`.
///
/// Each command is rendered as a clickable slash-command mention resolved for
/// `guild_id` (see [`commands::command_mention`]).
pub(crate) fn build_embed(language: Language, guild_id: Option<GuildId>) -> CreateEmbed {
    let msg = language.messages();
    let body = [
        (commands::HELP, msg.cmd_help_desc),
        (commands::VERSION, msg.cmd_version_desc),
        (commands::PING, msg.cmd_ping_desc),
        (commands::WHOIS, msg.cmd_whois_desc),
        (commands::DOCTOR, msg.cmd_doctor_desc),
    ]
    .into_iter()
    .map(|(name, desc)| format!("{} — {desc}", commands::command_mention(name, guild_id)))
    .collect::<Vec<_>>()
    .join("\n");

    CreateEmbed::new().title(msg.help_title).description(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_embed_lists_every_command() {
        let embed = build_embed(Language::En, None);
        let value = serenity::json::to_value(embed).expect("embed serializes");
        let description = value["description"].as_str().expect("description present");
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
