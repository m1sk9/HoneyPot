//! `/whois <user>` — shows a user's basic info, public badges, and, when
//! Discord itself has flagged the account (spammer mark or unusual DM activity),
//! a warnings field for those flags.
//!
//! The user (and their partial member, when in a guild) is resolved from the
//! interaction, which carries the full [`User`] (including `public_flags`), so no
//! extra HTTP fetch is needed. The two flags twilight-model drops are read from
//! the raw interaction payload (see [`crate::discord::raw`]). Gated on Ban
//! Members both at registration and here, matching the button handlers.

use crate::discord::raw::{self, AccountSignals};
use crate::discord::{ban, commands};
use crate::i18n::{Language, Messages};
use twilight_http::Client;
use twilight_model::application::interaction::application_command::{
    CommandData, CommandOptionValue,
};
use twilight_model::application::interaction::{Interaction, InteractionMember};
use twilight_model::channel::message::Embed;
use twilight_model::user::{User, UserFlags};
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder};

/// The `user` option name shared with the command definition.
pub(super) const USER_OPTION: &str = "user";

pub(super) async fn run(http: &Client, interaction: &Interaction, data: &CommandData, raw: &str) {
    let language = commands::language_for(interaction.guild_id);
    let msg = language.messages();

    if !commands::has_ban_permission(interaction) {
        commands::respond_text(http, interaction, msg.cmd_perm_needed).await;
        return;
    }

    let Some((user, member)) = resolved_target(data) else {
        // The option is `required`, so Discord rejects a missing argument before
        // it reaches us; this only guards against a malformed interaction.
        tracing::warn!("whois invoked without a resolved user option");
        return;
    };

    let signals = raw::from_resolved_user(raw, user.id);
    commands::respond_embed(
        http,
        interaction,
        build_embed(user, member, signals, language),
    )
    .await;
}

/// Extracts the resolved `user` argument and its partial member, if present.
fn resolved_target(data: &CommandData) -> Option<(&User, Option<&InteractionMember>)> {
    let user_id = data.options.iter().find_map(|option| match option.value {
        CommandOptionValue::User(id) if option.name == USER_OPTION => Some(id),
        _ => None,
    })?;
    let resolved = data.resolved.as_ref()?;
    Some((
        resolved.users.get(&user_id)?,
        resolved.members.get(&user_id),
    ))
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
    member: Option<&InteractionMember>,
    signals: AccountSignals,
    language: Language,
) -> Embed {
    let msg = language.messages();
    let offender = ban::OffenderContext {
        joined_at: member.and_then(|member| member.joined_at),
        unusual_dm_activity_until: signals.unusual_dm_activity_until,
        spammer: signals.spammer,
    };

    let badges = badge_names(user.public_flags.unwrap_or_else(UserFlags::empty));
    let badges_value = if badges.is_empty() {
        msg.badges_none.to_string()
    } else {
        badges.join(", ")
    };

    let mut embed = EmbedBuilder::new()
        .title(msg.whois_title)
        .field(EmbedFieldBuilder::new(
            msg.field_user,
            ban::target_field(user, msg),
        ))
        .field(
            EmbedFieldBuilder::new(msg.field_account, ban::account_type_field(user, msg)).inline(),
        )
        .field(
            EmbedFieldBuilder::new(
                msg.field_account_created,
                ban::timestamp_field(ban::created_at(user)),
            )
            .inline(),
        )
        .field(EmbedFieldBuilder::new(msg.field_joined, ban::joined_field(&offender, msg)).inline())
        .field(EmbedFieldBuilder::new(msg.field_badges, badges_value));

    if let Some(warnings) = flag_warnings(&offender, msg) {
        embed = embed.field(EmbedFieldBuilder::new(msg.field_warnings, warnings));
    }
    embed.build()
}

/// The account's active Discord flags as warning lines, or `None` when it has
/// none. Limited to the spammer mark and an unexpired unusual-DM-activity flag
/// — signals Discord itself set, not the ban embed's heuristics.
fn flag_warnings(offender: &ban::OffenderContext, msg: &Messages) -> Option<String> {
    let mut lines = Vec::new();
    if offender.spammer {
        lines.push(msg.spammer.to_string());
    }
    if let Some(until) = offender.unusual_dm_activity_until
        && until.as_secs() > ban::now().as_secs()
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
fn badge_names(flags: UserFlags) -> Vec<&'static str> {
    [
        (UserFlags::STAFF, "Discord Staff"),
        (UserFlags::PARTNER, "Partner"),
        (UserFlags::HYPESQUAD, "HypeSquad Events"),
        (UserFlags::HYPESQUAD_ONLINE_HOUSE_1, "HypeSquad Bravery"),
        (UserFlags::HYPESQUAD_ONLINE_HOUSE_2, "HypeSquad Brilliance"),
        (UserFlags::HYPESQUAD_ONLINE_HOUSE_3, "HypeSquad Balance"),
        (UserFlags::BUG_HUNTER_LEVEL_1, "Bug Hunter"),
        (UserFlags::BUG_HUNTER_LEVEL_2, "Bug Hunter Level 2"),
        (UserFlags::PREMIUM_EARLY_SUPPORTER, "Early Supporter"),
        (
            UserFlags::VERIFIED_DEVELOPER,
            "Early Verified Bot Developer",
        ),
        // Bit 18: twilight names it after Discord's later rename of the program.
        (UserFlags::MODERATOR_PROGRAMS_ALUMNI, "Certified Moderator"),
        (UserFlags::VERIFIED_BOT, "Verified Bot"),
        (UserFlags::ACTIVE_DEVELOPER, "Active Developer"),
    ]
    .into_iter()
    .filter(|(flag, _)| flags.contains(*flag))
    .map(|(_, name)| name)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::fixtures;
    use twilight_model::util::Timestamp;

    fn field_names(embed: &Embed) -> Vec<&str> {
        embed
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect()
    }

    #[test]
    fn badge_names_lists_only_set_public_badges() {
        let flags = UserFlags::ACTIVE_DEVELOPER
            | UserFlags::PREMIUM_EARLY_SUPPORTER
            | UserFlags::TEAM_PSEUDO_USER; // internal: must not appear
        let badges = badge_names(flags);
        assert!(badges.contains(&"Active Developer"));
        assert!(badges.contains(&"Early Supporter"));
        assert_eq!(badges.len(), 2, "internal flags must be excluded");
    }

    #[test]
    fn badge_names_empty_when_no_flags() {
        assert!(badge_names(UserFlags::empty()).is_empty());
    }

    #[test]
    fn build_embed_shows_badges_and_a_warning_when_flagged() {
        let mut user = fixtures::user(1, "spammy");
        user.public_flags = Some(UserFlags::ACTIVE_DEVELOPER);
        let signals = AccountSignals {
            spammer: true,
            ..AccountSignals::default()
        };

        let embed = build_embed(&user, None, signals, Language::En);
        let names = field_names(&embed);

        let msg = Language::En.messages();
        assert!(names.contains(&msg.field_badges));
        // The spammer flag drives the warnings field.
        assert!(names.contains(&msg.field_warnings));
    }

    #[test]
    fn build_embed_omits_warnings_for_an_unflagged_account() {
        // A default account with only a badge and no spammer/DM flag: no warnings.
        let mut user = fixtures::user(1, "member");
        user.public_flags = Some(UserFlags::ACTIVE_DEVELOPER);

        let embed = build_embed(&user, None, AccountSignals::default(), Language::En);
        assert!(!field_names(&embed).contains(&Language::En.messages().field_warnings));
    }

    #[test]
    fn build_embed_warns_on_unusual_dm_flag_from_signals() {
        let user = fixtures::user(1, "member");
        let until = Timestamp::from_secs(ban::now().as_secs() + 3600).expect("valid timestamp");
        let signals = AccountSignals {
            spammer: false,
            unusual_dm_activity_until: Some(until),
        };

        let embed = build_embed(&user, None, signals, Language::En);
        let msg = Language::En.messages();
        let warnings = embed
            .fields
            .iter()
            .find(|field| field.name == msg.field_warnings)
            .expect("warnings field present");
        assert!(warnings.value.contains(&ban::timestamp_field(until)));
        assert!(!warnings.value.contains(msg.spammer));
    }
}
