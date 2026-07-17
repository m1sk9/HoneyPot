//! Slash-command registration and dispatch.
//!
//! HoneyPot uses serenity's native application-command API directly rather than
//! a command framework: only a handful of read-only informational commands are
//! needed, and the bot already owns a hand-written [`EventHandler`], so a
//! framework that wants to own the event loop would cost more than it saves.
//!
//! [`register`] is called from `ready` and publishes the [`definitions`] either
//! globally or per configured guild, chosen by [`crate::settings::command_scope`]
//! (per-guild propagates instantly, which is convenient in development).
//! [`dispatch`] routes an incoming [`CommandInteraction`] to the matching command
//! module by name. Each command builds its own reply; like the gateway handlers,
//! failures are logged and swallowed.
//!
//! Command *descriptions* are localized to Discord's client locale (`ja`) at
//! registration, while command *replies* follow the guild's configured language
//! (see [`language_for`]) — the same per-guild `Language` the ban embeds use.
//! Command names stay English (lowercase, Discord's canonical form).
//!
//! [`EventHandler`]: serenity::all::EventHandler

// `pub(crate)` so the embed preview (`crate::discord::preview`) can call each
// command's `build_embed` and render it without a live interaction; the `run`
// entry points stay `pub(super)`, private to this module.
pub(crate) mod doctor;
pub(crate) mod help;
pub(crate) mod ping;
pub(crate) mod version;
pub(crate) mod whois;

use crate::i18n::{JA, Language};
use crate::settings::{CommandScope, HoneyPotConfig};
use serenity::all::{
    CommandId, CommandInteraction, CommandOptionType, Context, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateInteractionResponse, CreateInteractionResponseMessage, GuildId,
    InteractionContext, Permissions,
};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// `/help` command name.
pub(crate) const HELP: &str = "help";
/// `/version` command name.
pub(crate) const VERSION: &str = "version";
/// `/ping` command name.
pub(crate) const PING: &str = "ping";
/// `/whois` command name.
pub(crate) const WHOIS: &str = "whois";
/// `/doctor` command name.
pub(crate) const DOCTOR: &str = "doctor";

/// The Discord client locale that the `ja` catalog localizes command metadata to.
const JA_LOCALE: &str = "ja";

/// Registered command IDs, captured after registration so `/help` can render
/// clickable command mentions (`</name:id>`).
///
/// Guild-scoped registration mints a distinct id per guild, so those are keyed by
/// guild; global registration uses one id per command. A lookup falls back from
/// the invoking guild to the global set, and [`command_mention`] falls back to
/// plain text when no id is known (e.g. the embed preview, which never registers).
static COMMAND_IDS: LazyLock<Mutex<CommandIdRegistry>> =
    LazyLock::new(|| Mutex::new(CommandIdRegistry::default()));

#[derive(Default)]
struct CommandIdRegistry {
    global: HashMap<String, CommandId>,
    per_guild: HashMap<GuildId, HashMap<String, CommandId>>,
}

/// Builds the full set of slash commands.
///
/// English (the default catalog) supplies the base description; the `ja` catalog
/// supplies the localized description shown to Japanese-locale clients. `/whois`
/// defaults to requiring Ban Members (moderator-level) and `/doctor` to Manage
/// Server (admin-level); both surface privileged detail, so both are hidden from
/// direct messages.
pub(crate) fn definitions() -> Vec<CreateCommand> {
    let en = Language::En.messages();
    vec![
        localized(HELP, en.cmd_help_desc, JA.cmd_help_desc),
        localized(VERSION, en.cmd_version_desc, JA.cmd_version_desc),
        localized(PING, en.cmd_ping_desc, JA.cmd_ping_desc),
        localized(WHOIS, en.cmd_whois_desc, JA.cmd_whois_desc)
            .add_option(
                CreateCommandOption::new(CommandOptionType::User, "user", en.cmd_whois_user_desc)
                    .description_localized(JA_LOCALE, JA.cmd_whois_user_desc)
                    .required(true),
            )
            .default_member_permissions(Permissions::BAN_MEMBERS)
            .contexts(vec![InteractionContext::Guild]),
        localized(DOCTOR, en.cmd_doctor_desc, JA.cmd_doctor_desc)
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .contexts(vec![InteractionContext::Guild]),
    ]
}

/// Builds a command with an English description and a `ja` localization.
fn localized(name: &str, en_desc: &str, ja_desc: &str) -> CreateCommand {
    CreateCommand::new(name)
        .description(en_desc)
        .description_localized(JA_LOCALE, ja_desc)
}

/// Registers the slash commands according to [`crate::settings::command_scope`].
///
/// Called from `ready`. Registration failures are logged rather than propagated:
/// a bot that fails to register commands should still moderate. Per-guild
/// registration overwrites each configured guild's command set so stale commands
/// from a previous run are cleared.
pub(crate) async fn register(ctx: &Context) {
    let commands = definitions();
    match crate::settings::command_scope() {
        CommandScope::Global => {
            match serenity::all::Command::set_global_commands(&ctx.http, commands).await {
                Ok(registered) => {
                    tracing::info!(count = registered.len(), "registered global slash commands");
                    let ids = name_to_id(&registered);
                    COMMAND_IDS
                        .lock()
                        .expect("command id registry poisoned")
                        .global = ids;
                }
                Err(error) => tracing::error!(%error, "failed to register global slash commands"),
            }
        }
        CommandScope::Guild => {
            for guild_id in HoneyPotConfig::get().guild_ids() {
                match guild_id.set_commands(&ctx.http, commands.clone()).await {
                    Ok(registered) => {
                        tracing::info!(%guild_id, "registered guild slash commands");
                        let ids = name_to_id(&registered);
                        COMMAND_IDS
                            .lock()
                            .expect("command id registry poisoned")
                            .per_guild
                            .insert(guild_id, ids);
                    }
                    Err(error) => {
                        tracing::error!(%error, %guild_id, "failed to register guild slash commands");
                    }
                }
            }
        }
    }
}

/// Maps registered commands by name to their assigned id.
fn name_to_id(commands: &[serenity::all::Command]) -> HashMap<String, CommandId> {
    commands
        .iter()
        .map(|command| (command.name.clone(), command.id))
        .collect()
}

/// Renders a clickable slash-command mention (`</name:id>`) for `name`, falling
/// back to plain `/name` text when the command's id is unknown.
///
/// The id is resolved from the invoking guild's registration first, then the
/// global set (see [`COMMAND_IDS`]).
pub(crate) fn command_mention(name: &str, guild_id: Option<GuildId>) -> String {
    let registry = COMMAND_IDS.lock().expect("command id registry poisoned");
    let id = guild_id
        .and_then(|guild| registry.per_guild.get(&guild))
        .and_then(|ids| ids.get(name))
        .or_else(|| registry.global.get(name));
    match id {
        Some(id) => format!("</{name}:{id}>"),
        None => format!("/{name}"),
    }
}

/// Routes an incoming command interaction to its handler by name.
///
/// An unrecognized name is logged and ignored (e.g. a command left registered
/// from an older version).
pub(crate) async fn dispatch(ctx: &Context, command: &CommandInteraction) {
    match command.data.name.as_str() {
        HELP => help::run(ctx, command).await,
        VERSION => version::run(ctx, command).await,
        PING => ping::run(ctx, command).await,
        WHOIS => whois::run(ctx, command).await,
        DOCTOR => doctor::run(ctx, command).await,
        other => tracing::warn!(command = other, "received unknown slash command"),
    }
}

/// Resolves the reply language for a command from the guild's configuration,
/// defaulting to English outside a guild or for an unconfigured guild.
pub(crate) fn language_for(guild_id: Option<GuildId>) -> Language {
    guild_id
        .and_then(|id| HoneyPotConfig::get().guild(id).map(|guild| guild.language))
        .unwrap_or_default()
}

/// Whether the invoking member holds the Ban Members permission.
///
/// Discord populates the interaction member's channel-computed permissions, so
/// no cache lookup is needed — the same source the button handlers use.
pub(crate) fn has_ban_permission(command: &CommandInteraction) -> bool {
    command
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(Permissions::ban_members)
}

/// Whether the invoking member can manage the server.
///
/// Discord's computed permissions already fold Administrator into every bit, but
/// the explicit `administrator()` check keeps the intent legible.
pub(crate) fn has_manage_guild(command: &CommandInteraction) -> bool {
    command
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(|perms| perms.manage_guild() || perms.administrator())
}

/// Sends an ephemeral embed reply, logging any failure.
///
/// Command replies are ephemeral: they are informational and would otherwise
/// clutter the invoking channel.
pub(crate) async fn respond_embed(ctx: &Context, command: &CommandInteraction, embed: CreateEmbed) {
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .ephemeral(true)
            .embed(embed),
    );
    if let Err(error) = command.create_response(&ctx.http, response).await {
        tracing::error!(%error, command = %command.data.name, "failed to respond to command");
    }
}

/// Sends an ephemeral text reply, logging any failure.
pub(crate) async fn respond_text(ctx: &Context, command: &CommandInteraction, content: &str) {
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .ephemeral(true)
            .content(content),
    );
    if let Err(error) = command.create_response(&ctx.http, response).await {
        tracing::error!(%error, command = %command.data.name, "failed to respond to command");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_are_lowercase_and_unique() {
        let names = [HELP, VERSION, PING, WHOIS, DOCTOR];
        for name in names {
            assert_eq!(name, name.to_lowercase(), "command name must be lowercase");
        }
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "command names must be unique");
    }

    #[test]
    fn definitions_cover_every_command() {
        assert_eq!(definitions().len(), 5);
    }
}
