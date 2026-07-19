//! `/whois <user>` — shows a user's basic info, public badges, and, when
//! Discord itself has flagged the account (spammer mark or unusual DM activity),
//! a warnings field for those flags.
//!
//! The user (and their partial member, when in a guild) is resolved from the
//! interaction, which carries the full [`User`] (including `public_flags`), so no
//! extra HTTP fetch is needed. Gated on Ban Members both at registration and
//! here, matching the button handlers.

use crate::discord::{ban, commands};
use crate::i18n::{Language, Messages};
use serenity::all::{
    CommandInteraction, Context, CreateEmbed, PartialMember, ResolvedValue, Timestamp, User,
    UserPublicFlags,
};

/// The `user` option name shared with the command definition.
const USER_OPTION: &str = "user";

pub(super) async fn run(ctx: &Context, command: &CommandInteraction) {
    let language = commands::language_for(command.guild_id);
    let msg = language.messages();

    if !commands::has_ban_permission(command) {
        commands::respond_text(ctx, command, msg.cmd_perm_needed).await;
        return;
    }

    let Some((user, member)) = resolved_target(command) else {
        // The option is `required`, so Discord rejects a missing argument before
        // it reaches us; this only guards against a malformed interaction.
        tracing::warn!("whois invoked without a resolved user option");
        return;
    };

    commands::respond_embed(ctx, command, build_embed(user, member, language)).await;
}

/// Extracts the resolved `user` argument and its partial member, if present.
fn resolved_target(command: &CommandInteraction) -> Option<(&User, Option<&PartialMember>)> {
    command
        .data
        .options()
        .into_iter()
        .find_map(|option| match (option.name, option.value) {
            (USER_OPTION, ResolvedValue::User(user, member)) => Some((user, member)),
            _ => None,
        })
}

/// Builds the user-information embed.
///
/// Always shows the basic fields (user, account type, creation and join dates,
/// badges). A warnings field is appended only for Discord's own account *flags*
/// — the spammer mark and an active unusual-DM-activity flag. The ban embed's
/// broader heuristics (new account, default avatar) are deliberately *not*
/// reused here: they fire for many legitimate members and would make a routine
/// lookup read like a threat report.
pub(crate) fn build_embed(
    user: &User,
    member: Option<&PartialMember>,
    language: Language,
) -> CreateEmbed {
    let msg = language.messages();
    let offender = ban::OffenderContext {
        joined_at: member.and_then(|member| member.joined_at),
        unusual_dm_activity_until: member.and_then(|member| member.unusual_dm_activity_until),
    };

    let badges = badge_names(user.public_flags.unwrap_or_else(UserPublicFlags::empty));
    let badges_value = if badges.is_empty() {
        msg.badges_none.to_string()
    } else {
        badges.join(", ")
    };

    let mut embed = CreateEmbed::new()
        .title(msg.whois_title)
        .field(msg.field_user, ban::target_field(user, msg), false)
        .field(msg.field_account, ban::account_type_field(user, msg), true)
        .field(
            msg.field_account_created,
            ban::timestamp_field(user.created_at()),
            true,
        )
        .field(msg.field_joined, ban::joined_field(&offender, msg), true)
        .field(msg.field_badges, badges_value, false);

    if let Some(warnings) = flag_warnings(user, &offender, msg) {
        embed = embed.field(msg.field_warnings, warnings, false);
    }
    embed
}

/// The account's active Discord flags as warning lines, or `None` when it has
/// none. Limited to the spammer mark and an unexpired unusual-DM-activity flag
/// — signals Discord itself set, not the ban embed's heuristics.
fn flag_warnings(user: &User, offender: &ban::OffenderContext, msg: &Messages) -> Option<String> {
    let mut lines = Vec::new();
    if user
        .public_flags
        .is_some_and(|flags| flags.contains(UserPublicFlags::SPAMMER))
    {
        lines.push(msg.spammer.to_string());
    }
    if let Some(until) = offender.unusual_dm_activity_until
        && until.unix_timestamp() > Timestamp::now().unix_timestamp()
    {
        lines.push(
            msg.unusual_dm_flagged
                .replace("{}", &ban::timestamp_field(until)),
        );
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Maps the account's public flags to their badge names.
///
/// Only user-facing profile badges are listed; internal flags (team/system
/// accounts, HTTP-interaction bots, the spammer mark) are omitted. Badge names
/// are Discord proper nouns, kept English like the audit-log ban reason.
fn badge_names(flags: UserPublicFlags) -> Vec<&'static str> {
    [
        (UserPublicFlags::DISCORD_EMPLOYEE, "Discord Staff"),
        (UserPublicFlags::PARTNERED_SERVER_OWNER, "Partner"),
        (UserPublicFlags::HYPESQUAD_EVENTS, "HypeSquad Events"),
        (UserPublicFlags::HOUSE_BRAVERY, "HypeSquad Bravery"),
        (UserPublicFlags::HOUSE_BRILLIANCE, "HypeSquad Brilliance"),
        (UserPublicFlags::HOUSE_BALANCE, "HypeSquad Balance"),
        (UserPublicFlags::BUG_HUNTER_LEVEL_1, "Bug Hunter"),
        (UserPublicFlags::BUG_HUNTER_LEVEL_2, "Bug Hunter Level 2"),
        (UserPublicFlags::EARLY_SUPPORTER, "Early Supporter"),
        (
            UserPublicFlags::EARLY_VERIFIED_BOT_DEVELOPER,
            "Early Verified Bot Developer",
        ),
        (
            UserPublicFlags::DISCORD_CERTIFIED_MODERATOR,
            "Certified Moderator",
        ),
        (UserPublicFlags::VERIFIED_BOT, "Verified Bot"),
        (UserPublicFlags::ACTIVE_DEVELOPER, "Active Developer"),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .map(|(_, name)| name)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_names_lists_only_set_public_badges() {
        let flags = UserPublicFlags::ACTIVE_DEVELOPER
            | UserPublicFlags::EARLY_SUPPORTER
            | UserPublicFlags::TEAM_USER; // internal: must not appear
        let badges = badge_names(flags);
        assert!(badges.contains(&"Active Developer"));
        assert!(badges.contains(&"Early Supporter"));
        assert_eq!(badges.len(), 2, "internal flags must be excluded");
    }

    #[test]
    fn badge_names_empty_when_no_flags() {
        assert!(badge_names(UserPublicFlags::empty()).is_empty());
    }

    #[test]
    fn build_embed_shows_badges_and_a_warning_when_flagged() {
        let mut user = User::default();
        user.name = "spammy".to_string();
        user.discriminator = None;
        user.public_flags = Some(UserPublicFlags::ACTIVE_DEVELOPER | UserPublicFlags::SPAMMER);

        let embed = build_embed(&user, None, Language::En);
        let value = serenity::json::to_value(embed).expect("embed serializes");
        let names: Vec<&str> = value["fields"]
            .as_array()
            .expect("fields present")
            .iter()
            .map(|field| field["name"].as_str().unwrap())
            .collect();

        let msg = Language::En.messages();
        assert!(names.contains(&msg.field_badges));
        // The spammer flag drives the warnings field.
        assert!(names.contains(&msg.field_warnings));
    }

    #[test]
    fn build_embed_omits_warnings_for_an_unflagged_account() {
        // A default account with only a badge and no spammer/DM flag: no warnings.
        let mut user = User::default();
        user.name = "member".to_string();
        user.discriminator = None;
        user.public_flags = Some(UserPublicFlags::ACTIVE_DEVELOPER);

        let embed = build_embed(&user, None, Language::En);
        let value = serenity::json::to_value(embed).expect("embed serializes");
        let names: Vec<&str> = value["fields"]
            .as_array()
            .expect("fields present")
            .iter()
            .map(|field| field["name"].as_str().unwrap())
            .collect();
        assert!(!names.contains(&Language::En.messages().field_warnings));
    }
}
