//! Slash-command registration and dispatch.
//!
//! HoneyPot uses twilight's application-command API directly rather than a
//! command framework: only a handful of read-only informational commands are
//! needed, and the bot already owns a hand-written gateway event loop, so a
//! framework that wants to own the event loop would cost more than it saves.
//!
//! [`register`] is called from `READY` and publishes the [`definitions`] either
//! globally or per configured guild, chosen by [`crate::settings::command_scope`]
//! (per-guild propagates instantly, which is convenient in development).
//! [`dispatch`] routes an incoming application-command [`Interaction`] to the
//! matching command module by name. Each command builds its own reply; like the
//! gateway handlers, failures are logged and swallowed.
//!
//! Command *descriptions* are localized to Discord's client locale (`ja`) at
//! registration, while command *replies* follow the guild's configured language
//! (see [`language_for`]) — the same per-guild `Language` the ban embeds use.
//! Command names stay English (lowercase, Discord's canonical form).

// `pub(crate)` so the embed preview (`crate::discord::preview`) can call each
// command's `build_embed` and render it without a live interaction; the `run`
// entry points stay `pub(super)`, private to this module.
pub(crate) mod doctor;
pub(crate) mod help;
pub(crate) mod ping;
pub(crate) mod version;
pub(crate) mod whois;

use crate::discord::interaction;
use crate::i18n::{JA, Language};
use crate::settings::{CommandScope, HoneyPotConfig};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use twilight_http::Client;
use twilight_model::application::command::{Command, CommandType};
use twilight_model::application::interaction::{
    Interaction, InteractionContextType, InteractionData,
};
use twilight_model::channel::message::{Embed, MessageFlags};
use twilight_model::guild::Permissions;
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, CommandMarker, GuildMarker},
};
use twilight_util::builder::InteractionResponseDataBuilder;
use twilight_util::builder::command::{CommandBuilder, UserBuilder};

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
/// the invoking guild to the global set, and [`command_mentions`] falls back to
/// plain text when no id is known (e.g. the embed preview, which never registers).
static COMMAND_IDS: LazyLock<Mutex<CommandIdRegistry>> =
    LazyLock::new(|| Mutex::new(CommandIdRegistry::default()));

#[derive(Default)]
struct CommandIdRegistry {
    global: HashMap<String, Id<CommandMarker>>,
    per_guild: HashMap<Id<GuildMarker>, HashMap<String, Id<CommandMarker>>>,
}

/// Builds the full set of slash commands.
///
/// English (the default catalog) supplies the base description; the `ja` catalog
/// supplies the localized description shown to Japanese-locale clients. `/whois`
/// defaults to requiring Ban Members (moderator-level) and `/doctor` to Manage
/// Server (admin-level); both surface privileged detail, so both are hidden from
/// direct messages.
pub(crate) fn definitions() -> Vec<Command> {
    let en = Language::En.messages();
    vec![
        localized(HELP, en.cmd_help_desc, JA.cmd_help_desc).build(),
        localized(VERSION, en.cmd_version_desc, JA.cmd_version_desc).build(),
        localized(PING, en.cmd_ping_desc, JA.cmd_ping_desc).build(),
        localized(WHOIS, en.cmd_whois_desc, JA.cmd_whois_desc)
            .option(
                UserBuilder::new(whois::USER_OPTION, en.cmd_whois_user_desc)
                    .description_localizations([(JA_LOCALE, JA.cmd_whois_user_desc)])
                    .required(true),
            )
            .default_member_permissions(Permissions::BAN_MEMBERS)
            .contexts([InteractionContextType::Guild])
            .build(),
        localized(DOCTOR, en.cmd_doctor_desc, JA.cmd_doctor_desc)
            .default_member_permissions(Permissions::MANAGE_GUILD)
            .contexts([InteractionContextType::Guild])
            .build(),
    ]
}

/// Builds a command with an English description and a `ja` localization.
fn localized(name: &str, en_desc: &str, ja_desc: &str) -> CommandBuilder {
    CommandBuilder::new(name, en_desc, CommandType::ChatInput)
        .description_localizations([(JA_LOCALE, ja_desc)])
}

/// Registers the slash commands according to [`crate::settings::command_scope`].
///
/// Called from `READY` with the application ID it carries. Registration failures
/// are logged rather than propagated: a bot that fails to register commands
/// should still moderate. Per-guild registration overwrites each configured
/// guild's command set so stale commands from a previous run are cleared.
pub(crate) async fn register(http: &Client, application_id: Id<ApplicationMarker>) {
    let client = http.interaction(application_id);
    let commands = definitions();
    match crate::settings::command_scope() {
        CommandScope::Global => {
            match decode_commands(client.set_global_commands(&commands).await).await {
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
                match decode_commands(client.set_guild_commands(guild_id, &commands).await).await {
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

/// Decodes a command-registration response into the registered commands.
async fn decode_commands(
    response: Result<
        twilight_http::Response<twilight_http::response::marker::ListBody<Command>>,
        twilight_http::Error,
    >,
) -> Result<Vec<Command>, crate::error::HoneyPotError> {
    Ok(response?.models().await?)
}

/// Maps registered commands by name to their assigned id. Discord always
/// returns ids for registered commands; one without is skipped.
fn name_to_id(commands: &[Command]) -> HashMap<String, Id<CommandMarker>> {
    commands
        .iter()
        .filter_map(|command| Some((command.name.clone(), command.id?)))
        .collect()
}

/// Renders clickable slash-command mentions (`</name:id>`) for `names` under a
/// single registry lock, falling back to plain `/name` text for any command
/// whose id is unknown (e.g. the embed preview, which never registers).
pub(crate) fn command_mentions(names: &[&str], guild_id: Option<Id<GuildMarker>>) -> Vec<String> {
    let registry = COMMAND_IDS.lock().expect("command id registry poisoned");
    names
        .iter()
        .map(|name| render_mention(&registry, name, guild_id))
        .collect()
}

/// Renders one mention from an already-locked registry. The id is resolved from
/// the invoking guild's registration first, then the global set.
fn render_mention(
    registry: &CommandIdRegistry,
    name: &str,
    guild_id: Option<Id<GuildMarker>>,
) -> String {
    let id = guild_id
        .and_then(|guild| registry.per_guild.get(&guild))
        .and_then(|ids| ids.get(name))
        .or_else(|| registry.global.get(name));
    match id {
        Some(id) => format!("</{name}:{id}>"),
        None => format!("/{name}"),
    }
}

/// Routes an incoming command interaction to its handler by name. `raw` is the
/// interaction's gateway JSON, which `/whois` reads account signals from.
///
/// An unrecognized name is logged and ignored (e.g. a command left registered
/// from an older version).
pub(crate) async fn dispatch(http: &Client, interaction: &Interaction, raw: &str) {
    let Some(InteractionData::ApplicationCommand(data)) = &interaction.data else {
        return;
    };
    match data.name.as_str() {
        HELP => help::run(http, interaction).await,
        VERSION => version::run(http, interaction).await,
        PING => ping::run(http, interaction).await,
        WHOIS => whois::run(http, interaction, data, raw).await,
        DOCTOR => doctor::run(http, interaction).await,
        other => tracing::warn!(command = other, "received unknown slash command"),
    }
}

/// Resolves the reply language for an interaction (a command or a log-embed
/// button) from the guild's configuration, defaulting to English outside a guild
/// or for an unconfigured guild.
pub(crate) fn language_for(guild_id: Option<Id<GuildMarker>>) -> Language {
    guild_id
        .and_then(|id| HoneyPotConfig::get().guild(id).map(|guild| guild.language))
        .unwrap_or_default()
}

/// The invoking member's channel-computed permissions, if in a guild.
fn member_permissions(interaction: &Interaction) -> Option<Permissions> {
    interaction
        .member
        .as_ref()
        .and_then(|member| member.permissions)
}

/// Whether the invoking member holds the Ban Members permission.
///
/// Discord populates the interaction member's channel-computed permissions, so
/// no cache lookup is needed. Shared by the slash commands and the log-embed
/// buttons (see [`crate::discord::interaction`]).
pub(crate) fn has_ban_permission(interaction: &Interaction) -> bool {
    member_permissions(interaction).is_some_and(|perms| perms.contains(Permissions::BAN_MEMBERS))
}

/// Whether the invoking member can manage the server.
///
/// Discord's computed permissions already fold Administrator into every bit, but
/// the explicit Administrator check keeps the intent legible.
pub(crate) fn has_manage_guild(interaction: &Interaction) -> bool {
    member_permissions(interaction).is_some_and(|perms| {
        perms.contains(Permissions::MANAGE_GUILD) || perms.contains(Permissions::ADMINISTRATOR)
    })
}

/// The command name for log lines, or empty for a non-command interaction.
fn command_name(interaction: &Interaction) -> &str {
    match &interaction.data {
        Some(InteractionData::ApplicationCommand(data)) => data.name.as_str(),
        _ => "",
    }
}

/// An ephemeral initial response built from `data`.
pub(crate) fn ephemeral(data: InteractionResponseDataBuilder) -> InteractionResponse {
    InteractionResponse {
        kind: InteractionResponseType::ChannelMessageWithSource,
        data: Some(data.flags(MessageFlags::EPHEMERAL).build()),
    }
}

/// Sends an ephemeral embed reply, logging any failure.
///
/// Command replies are ephemeral: they are informational and would otherwise
/// clutter the invoking channel.
pub(crate) async fn respond_embed(http: &Client, interaction: &Interaction, embed: Embed) {
    let response = ephemeral(InteractionResponseDataBuilder::new().embeds([embed]));
    if let Err(error) = interaction::create_response(http, interaction, &response).await {
        tracing::error!(%error, command = command_name(interaction), "failed to respond to command");
    }
}

/// Sends an ephemeral text reply, logging any failure.
pub(crate) async fn respond_text(http: &Client, interaction: &Interaction, content: &str) {
    let response = ephemeral(InteractionResponseDataBuilder::new().content(content));
    if let Err(error) = interaction::create_response(http, interaction, &response).await {
        tracing::error!(%error, command = command_name(interaction), "failed to respond to command");
    }
}

/// Acknowledges a command with an ephemeral "thinking…" placeholder, to be
/// replaced later via [`edit_response_embed`] / [`edit_response_text`].
pub(crate) async fn defer_ephemeral(
    http: &Client,
    interaction: &Interaction,
) -> Result<(), crate::error::HoneyPotError> {
    let response = InteractionResponse {
        kind: InteractionResponseType::DeferredChannelMessageWithSource,
        data: Some(
            InteractionResponseDataBuilder::new()
                .flags(MessageFlags::EPHEMERAL)
                .build(),
        ),
    };
    interaction::create_response(http, interaction, &response).await
}

/// Replaces a deferred response with `embed`.
pub(crate) async fn edit_response_embed(
    http: &Client,
    interaction: &Interaction,
    embed: Embed,
) -> Result<(), crate::error::HoneyPotError> {
    let embeds = [embed];
    http.interaction(interaction.application_id)
        .update_response(&interaction.token)
        .embeds(Some(&embeds))
        .await?;
    Ok(())
}

/// Replaces a deferred response with a text line.
pub(crate) async fn edit_response_text(
    http: &Client,
    interaction: &Interaction,
    content: &str,
) -> Result<(), crate::error::HoneyPotError> {
    http.interaction(interaction.application_id)
        .update_response(&interaction.token)
        .content(Some(content))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use twilight_model::application::command::CommandOptionType;

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

    fn definition(name: &str) -> Command {
        definitions()
            .into_iter()
            .find(|command| command.name == name)
            .expect("command is defined")
    }

    #[test]
    fn whois_definition_requires_user_option_and_guild_context() {
        let whois = definition(WHOIS);
        assert_eq!(whois.options.len(), 1);
        let option = &whois.options[0];
        assert_eq!(option.name, whois::USER_OPTION);
        assert_eq!(option.kind, CommandOptionType::User);
        assert_eq!(option.required, Some(true));
        assert_eq!(
            whois.default_member_permissions,
            Some(Permissions::BAN_MEMBERS)
        );
        assert_eq!(whois.contexts, Some(vec![InteractionContextType::Guild]));
    }

    #[test]
    fn doctor_definition_requires_manage_guild() {
        let doctor = definition(DOCTOR);
        assert_eq!(
            doctor.default_member_permissions,
            Some(Permissions::MANAGE_GUILD)
        );
        assert_eq!(doctor.contexts, Some(vec![InteractionContextType::Guild]));
    }

    #[test]
    fn definitions_carry_ja_description_localizations() {
        for command in definitions() {
            let localizations = command
                .description_localizations
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no localizations", command.name));
            assert!(
                localizations.contains_key(JA_LOCALE),
                "{} lacks a ja description",
                command.name
            );
        }
        let whois = definition(WHOIS);
        assert_eq!(
            whois.options[0]
                .description_localizations
                .as_ref()
                .and_then(|l| l.get(JA_LOCALE))
                .map(String::as_str),
            Some(JA.cmd_whois_user_desc)
        );
    }

    #[test]
    fn command_mentions_fall_back_to_plain_text_when_unregistered() {
        // A name no registration inserts, so the id lookup always misses.
        assert_eq!(
            command_mentions(&["unregistered_zzz"], None),
            vec!["/unregistered_zzz".to_string()]
        );
    }

    #[test]
    fn command_mentions_render_a_clickable_mention_for_a_known_id() {
        // A unique fake name keeps this independent of the shared registry.
        COMMAND_IDS
            .lock()
            .expect("command id registry poisoned")
            .global
            .insert("fakeglobal_zzz".to_string(), Id::new(42));
        assert_eq!(
            command_mentions(&["fakeglobal_zzz"], None),
            vec!["</fakeglobal_zzz:42>".to_string()]
        );
    }

    #[test]
    fn language_for_defaults_to_english_outside_a_guild() {
        assert_eq!(language_for(None), Language::En);
    }
}
