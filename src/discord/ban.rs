//! Shared ban execution for honeypot triggers.
//!
//! Both honeypot paths (role acquisition and channel post) funnel through
//! [`execute_ban`]: it bans the offender, then posts a log embed carrying an
//! `Unban` button to the guild's log channel. Detection is factored into pure
//! predicates ([`newly_acquired_honeypot_role`], [`is_honeypot_channel`]) so the
//! HTTP-free logic stays unit-testable.
//!
//! Because the serenity cache is disabled, `guild_member_update` fires for a
//! honeypot-role holder on *any* update (nickname, timeout, …), and an offender
//! can trip both honeypot paths at once. [`execute_ban`] therefore claims each
//! `(guild, user)` before acting so concurrent or repeated triggers ban and
//! notify only once.

use crate::error::HoneyPotError;
use serenity::all::{
    ButtonStyle, ChannelId, Colour, Context, CreateActionRow, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateMessage, GuildId, Mentionable, MessageId, RoleId, Timestamp, User,
    UserId,
};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// `(guild, user)` pairs already banned by a honeypot trigger.
///
/// Kept on success so concurrent events (e.g. a honeypot post *and* role gained
/// at once) or repeated `guild_member_update`s don't post duplicate log embeds.
/// A failed ban releases its claim (see [`execute_ban`]) so a later event can
/// retry, and [`forget_ban`] lets a future unban handler allow re-banning.
static HANDLED_BANS: LazyLock<Mutex<HashSet<(GuildId, UserId)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Claims `(guild_id, user_id)`, returning `true` only on the first claim.
///
/// Subsequent claims return `false` until [`forget_ban`] releases the pair.
fn claim_ban(guild_id: GuildId, user_id: UserId) -> bool {
    HANDLED_BANS
        .lock()
        .expect("HANDLED_BANS mutex poisoned")
        .insert((guild_id, user_id))
}

/// Releases a claim so `(guild_id, user_id)` can be banned again.
///
/// Called internally when a ban fails, and intended for the future unban
/// handler to call after lifting a ban.
pub fn forget_ban(guild_id: GuildId, user_id: UserId) {
    HANDLED_BANS
        .lock()
        .expect("HANDLED_BANS mutex poisoned")
        .remove(&(guild_id, user_id));
}

/// Prefix for the unban button `custom_id`. The full id is `uhp_unban:{user_id}`;
/// the button handler parses the suffix as a [`UserId`]. Clicking it does not
/// unban yet — it opens an ephemeral confirmation (see [`UNBAN_CONFIRM_CUSTOM_ID_PREFIX`]).
pub const UNBAN_CUSTOM_ID_PREFIX: &str = "uhp_unban";

/// Prefix for the manual-ban button `custom_id`. The full id is
/// `uhp_ban:{user_id}`. This button appears on the "untrusted bot" notice so a
/// moderator can confirm the ban that was deliberately not applied automatically.
/// Clicking it does not ban yet — it opens an ephemeral confirmation (see
/// [`BAN_CONFIRM_CUSTOM_ID_PREFIX`]).
pub const BAN_CUSTOM_ID_PREFIX: &str = "uhp_ban";

/// Prefix for the "confirm unban" button shown in the ephemeral confirmation.
/// The full id is `uhp_unban_do:{user_id}:{message_id}`, where `message_id`
/// identifies the original log message so the handler can edit it in place.
/// The `_do` suffix keeps this from matching [`UNBAN_CUSTOM_ID_PREFIX`]'s
/// `uhp_unban:` form (they diverge before the `:` separator).
pub const UNBAN_CONFIRM_CUSTOM_ID_PREFIX: &str = "uhp_unban_do";

/// Prefix for the "confirm ban" button shown in the ephemeral confirmation.
/// The full id is `uhp_ban_do:{user_id}:{message_id}`, mirroring
/// [`UNBAN_CONFIRM_CUSTOM_ID_PREFIX`].
pub const BAN_CONFIRM_CUSTOM_ID_PREFIX: &str = "uhp_ban_do";

/// `custom_id` of the "cancel" button in either ephemeral confirmation. Carries
/// no payload: cancelling only dismisses the ephemeral prompt, touching neither
/// the ban nor the original log message.
pub const CANCEL_CUSTOM_ID: &str = "uhp_cancel";

/// Number of days' worth of the offender's messages to delete on ban.
const DELETE_MESSAGE_DAYS: u8 = 1;

/// Builds the unban button `custom_id` for `user_id`.
fn unban_custom_id(user_id: UserId) -> String {
    format!("{UNBAN_CUSTOM_ID_PREFIX}:{user_id}")
}

/// Parses an unban button `custom_id` (`uhp_unban:{user_id}`) into its target
/// [`UserId`]. Returns `None` for any other `custom_id`.
///
/// Inverse of [`unban_custom_id`]; kept alongside it so the encoding stays in
/// sync.
pub fn parse_unban_custom_id(custom_id: &str) -> Option<UserId> {
    let suffix = custom_id
        .strip_prefix(UNBAN_CUSTOM_ID_PREFIX)?
        .strip_prefix(':')?;
    suffix.parse::<u64>().ok().map(UserId::new)
}

/// Builds the manual-ban button `custom_id` for `user_id`.
fn ban_custom_id(user_id: UserId) -> String {
    format!("{BAN_CUSTOM_ID_PREFIX}:{user_id}")
}

/// Parses a manual-ban button `custom_id` (`uhp_ban:{user_id}`) into its target
/// [`UserId`]. Returns `None` for any other `custom_id`.
///
/// The `uhp_ban` prefix is not a prefix of `uhp_unban` (they diverge at the
/// fifth byte), so this never matches an unban id and vice versa.
pub fn parse_ban_custom_id(custom_id: &str) -> Option<UserId> {
    let suffix = custom_id
        .strip_prefix(BAN_CUSTOM_ID_PREFIX)?
        .strip_prefix(':')?;
    suffix.parse::<u64>().ok().map(UserId::new)
}

/// Builds the "confirm unban" button `custom_id` for `user_id`, embedding the
/// `message_id` of the log message to edit once the unban is confirmed.
fn unban_confirm_custom_id(user_id: UserId, message_id: MessageId) -> String {
    format!("{UNBAN_CONFIRM_CUSTOM_ID_PREFIX}:{user_id}:{message_id}")
}

/// Parses a "confirm unban" `custom_id` (`uhp_unban_do:{user_id}:{message_id}`)
/// into its target user and the log message to edit. Returns `None` for any
/// other `custom_id`.
pub fn parse_unban_confirm_custom_id(custom_id: &str) -> Option<(UserId, MessageId)> {
    parse_confirm_custom_id(UNBAN_CONFIRM_CUSTOM_ID_PREFIX, custom_id)
}

/// Builds the "confirm ban" button `custom_id` for `user_id`, embedding the
/// `message_id` of the notice to edit once the ban is confirmed.
fn ban_confirm_custom_id(user_id: UserId, message_id: MessageId) -> String {
    format!("{BAN_CONFIRM_CUSTOM_ID_PREFIX}:{user_id}:{message_id}")
}

/// Parses a "confirm ban" `custom_id` (`uhp_ban_do:{user_id}:{message_id}`) into
/// its target user and the notice message to edit. Returns `None` for any other
/// `custom_id`.
pub fn parse_ban_confirm_custom_id(custom_id: &str) -> Option<(UserId, MessageId)> {
    parse_confirm_custom_id(BAN_CONFIRM_CUSTOM_ID_PREFIX, custom_id)
}

/// Shared parser for the `{prefix}:{user_id}:{message_id}` confirmation ids.
fn parse_confirm_custom_id(prefix: &str, custom_id: &str) -> Option<(UserId, MessageId)> {
    let suffix = custom_id.strip_prefix(prefix)?.strip_prefix(':')?;
    let (user, message) = suffix.split_once(':')?;
    let user_id = UserId::new(user.parse::<u64>().ok()?);
    let message_id = MessageId::new(message.parse::<u64>().ok()?);
    Some((user_id, message_id))
}

/// Builds the action row carrying the `Unban` button for `user_id`.
pub fn unban_action_row(user_id: UserId) -> CreateActionRow {
    let button = CreateButton::new(unban_custom_id(user_id))
        .style(ButtonStyle::Danger)
        .label("Unban");
    CreateActionRow::Buttons(vec![button])
}

/// Builds the cancel button shared by both ephemeral confirmations.
fn cancel_button() -> CreateButton {
    CreateButton::new(CANCEL_CUSTOM_ID)
        .style(ButtonStyle::Secondary)
        .label("Cancel")
}

/// Builds the confirmation action row for an unban: a danger `Confirm unban`
/// button (carrying `message_id`) alongside `Cancel`.
pub fn confirm_unban_action_row(user_id: UserId, message_id: MessageId) -> CreateActionRow {
    let confirm = CreateButton::new(unban_confirm_custom_id(user_id, message_id))
        .style(ButtonStyle::Danger)
        .label("Confirm unban");
    CreateActionRow::Buttons(vec![confirm, cancel_button()])
}

/// Builds the confirmation action row for a manual ban: a danger `Confirm ban`
/// button (carrying `message_id`) alongside `Cancel`.
pub fn confirm_ban_action_row(user_id: UserId, message_id: MessageId) -> CreateActionRow {
    let confirm = CreateButton::new(ban_confirm_custom_id(user_id, message_id))
        .style(ButtonStyle::Danger)
        .label("Confirm ban");
    CreateActionRow::Buttons(vec![confirm, cancel_button()])
}

/// Which honeypot fired, carried into the log embed.
pub enum BanTrigger {
    /// The offender acquired this honeypot role.
    Role(RoleId),
    /// The offender posted in this honeypot channel.
    Channel {
        /// The honeypot channel posted in.
        channel_id: ChannelId,
        /// The offending message's content, shown in the log embed so a
        /// moderator can confirm it was spam. `None` when it wasn't captured
        /// (e.g. an empty message); role triggers never carry content.
        content: Option<String>,
    },
}

impl BanTrigger {
    /// Human-readable trigger kind for the embed field.
    fn kind(&self) -> &'static str {
        match self {
            BanTrigger::Role(_) => "role",
            BanTrigger::Channel { .. } => "channel",
        }
    }

    /// Mention of the specific role/channel that fired.
    fn detail(&self) -> String {
        match self {
            BanTrigger::Role(id) => id.mention().to_string(),
            BanTrigger::Channel { channel_id, .. } => channel_id.mention().to_string(),
        }
    }

    /// The offending message's content, if captured. Only channel triggers
    /// carry one; a role trigger always returns `None`.
    fn message_content(&self) -> Option<&str> {
        match self {
            BanTrigger::Role(_) => None,
            BanTrigger::Channel { content, .. } => content.as_deref(),
        }
    }
}

/// Returns the first honeypot role the member newly holds, if any.
///
/// When `old_roles` is `Some`, only roles in `new − old` (newly acquired) are
/// considered. When `None` (the normal case with the serenity cache disabled),
/// falls back to the intersection of the full new role set with the honeypots.
pub fn newly_acquired_honeypot_role(
    honeypot_role_ids: &[RoleId],
    new_roles: &[RoleId],
    old_roles: Option<&[RoleId]>,
) -> Option<RoleId> {
    new_roles
        .iter()
        .filter(|role| match old_roles {
            Some(old) => !old.contains(role),
            None => true,
        })
        .find(|role| honeypot_role_ids.contains(role))
        .copied()
}

/// Whether `channel_id` is one of the guild's honeypot channels.
pub fn is_honeypot_channel(honeypot_channel_ids: &[ChannelId], channel_id: ChannelId) -> bool {
    honeypot_channel_ids.contains(&channel_id)
}

/// Formats the target user field: mention, tag, and raw ID.
///
/// The tag embeds a user-controlled username inside an inline code span, so
/// backticks are neutralized to keep the span from being broken (spoofing the
/// log embed's layout).
fn target_field(target: &User) -> String {
    let tag = target.tag().replace('`', "'");
    format!("{} (`{}`)\nID: {}", target.mention(), tag, target.id)
}

/// Maximum characters of message content shown in the log embed.
///
/// Discord caps an embed field value at 1024 characters; this leaves headroom
/// for the surrounding code fence and the truncation marker.
const MAX_MESSAGE_LEN: usize = 1000;

/// Formats the offending message's content for the log embed.
///
/// The content is user-controlled, so backticks are neutralized (they would
/// otherwise break out of the surrounding code fence and let the spammer spoof
/// the embed) and the text is truncated to [`MAX_MESSAGE_LEN`] characters to
/// stay within Discord's field limit. Wrapping it in a code fence also renders
/// any links and mentions inert.
fn message_field(content: &str) -> String {
    let sanitized = content.replace('`', "'");
    let mut chars = sanitized.chars();
    let truncated: String = chars.by_ref().take(MAX_MESSAGE_LEN).collect();
    let body = if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        truncated
    };
    format!("```\n{body}\n```")
}

/// The formatted "Message" field value for `trigger`, or `None` when there is
/// nothing to show (a role trigger, or a channel trigger whose content wasn't
/// captured or was empty).
///
/// Kept pure and separate from [`with_message_field`] so the "show it or not"
/// decision stays unit-testable without constructing an embed.
fn message_field_value(trigger: &BanTrigger) -> Option<String> {
    match trigger.message_content() {
        Some(content) if !content.is_empty() => Some(message_field(content)),
        _ => None,
    }
}

/// Appends a "Message" field carrying the offending message's content when
/// [`message_field_value`] has something to show. A no-op otherwise.
fn with_message_field(embed: CreateEmbed, trigger: &BanTrigger) -> CreateEmbed {
    match message_field_value(trigger) {
        Some(value) => embed.field("Message", value, false),
        None => embed,
    }
}

/// Appends a footer marking the embed as a dry-run when [`crate::settings::dry_run`]
/// is enabled, so simulated bans/unbans can't be mistaken for real ones in the
/// log channel. A no-op in normal operation.
pub(crate) fn apply_dry_run_marker(embed: CreateEmbed) -> CreateEmbed {
    if crate::settings::dry_run() {
        embed.footer(CreateEmbedFooter::new(
            "⚠ DRY-RUN — no ban/unban was executed",
        ))
    } else {
        embed
    }
}

/// Builds the ban notification embed.
fn build_ban_embed(target: &User, trigger: &BanTrigger) -> CreateEmbed {
    let embed = CreateEmbed::new()
        .title("🍯 Honeypot triggered — user banned")
        .color(Colour::RED)
        .field("User", target_field(target), false)
        .field(
            "Trigger",
            format!("{} {}", trigger.kind(), trigger.detail()),
            true,
        )
        .field("Bot", if target.bot { "Yes" } else { "No" }, true)
        .timestamp(Timestamp::now());
    apply_dry_run_marker(with_message_field(embed, trigger))
}

/// Builds the ban notification message: the embed plus an `Unban` button.
fn build_ban_message(target: &User, trigger: &BanTrigger) -> CreateMessage {
    CreateMessage::new()
        .embed(build_ban_embed(target, trigger))
        .components(vec![unban_action_row(target.id)])
}

/// Builds the "untrusted bot" notice embed.
///
/// Unlike [`build_ban_embed`], no ban has happened yet: the bot tripped a
/// honeypot but is not in the guild's trusted list, so a moderator must decide.
fn build_pending_embed(target: &User, trigger: &BanTrigger) -> CreateEmbed {
    let embed = CreateEmbed::new()
        .title("🍯 Honeypot triggered — untrusted bot")
        .description("This bot is not in the trusted list. Press **Ban** to remove it.")
        .color(Colour::GOLD)
        .field("User", target_field(target), false)
        .field(
            "Trigger",
            format!("{} {}", trigger.kind(), trigger.detail()),
            true,
        )
        .field("Bot", "Yes", true)
        .timestamp(Timestamp::now());
    apply_dry_run_marker(with_message_field(embed, trigger))
}

/// Builds the untrusted-bot notice message: the pending embed plus a `Ban` button.
fn build_pending_message(target: &User, trigger: &BanTrigger) -> CreateMessage {
    let button = CreateButton::new(ban_custom_id(target.id))
        .style(ButtonStyle::Danger)
        .label("Ban");
    let row = CreateActionRow::Buttons(vec![button]);
    CreateMessage::new()
        .embed(build_pending_embed(target, trigger))
        .components(vec![row])
}

/// Bans `target` from `guild_id` and posts a log embed to `log_channel_id`.
///
/// Idempotent per `(guild, user)`: a duplicate trigger while the offender is
/// already claimed returns early without re-banning or re-notifying. Bans first
/// (deleting [`DELETE_MESSAGE_DAYS`] of messages), then notifies; a failed
/// notification is logged but does not undo or mask the successful ban. A failed
/// ban releases the claim so a later event can retry.
pub async fn execute_ban(
    ctx: &Context,
    guild_id: GuildId,
    log_channel_id: ChannelId,
    target: &User,
    trigger: BanTrigger,
) -> Result<(), HoneyPotError> {
    if !claim_ban(guild_id, target.id) {
        tracing::debug!(
            user_id = %target.id,
            "skipping duplicate honeypot ban for already-handled user"
        );
        return Ok(());
    }

    let reason = format!("Honeypot triggered ({})", trigger.kind());
    if crate::settings::dry_run() {
        tracing::warn!(
            guild_id = %guild_id,
            user_id = %target.id,
            "dry-run: skipping ban_with_reason"
        );
    } else if let Err(error) = guild_id
        .ban_with_reason(&ctx.http, target.id, DELETE_MESSAGE_DAYS, &reason)
        .await
    {
        forget_ban(guild_id, target.id);
        return Err(error.into());
    }

    tracing::info!(
        guild_id = %guild_id,
        user_id = %target.id,
        trigger = trigger.kind(),
        "banned member on honeypot trigger"
    );

    if let Err(error) = log_channel_id
        .send_message(&ctx.http, build_ban_message(target, &trigger))
        .await
    {
        tracing::error!(
            %error,
            user_id = %target.id,
            "banned user but failed to post log notification"
        );
    }

    Ok(())
}

/// Posts an "untrusted bot" notice to `log_channel_id` without banning.
///
/// Used when a *bot* (not in the guild's trusted list) trips a honeypot: rather
/// than auto-banning — which would catch well-behaved bots that legitimately
/// echo into a honeypot channel — the decision is deferred to a moderator via
/// the `Ban` button on the notice (handled by [`confirm_bot_ban`]).
///
/// Shares [`HANDLED_BANS`] with [`execute_ban`] to dedupe: a bot claimed here is
/// not notified again on repeat triggers, and a failed post releases the claim
/// so a later trigger can retry.
pub async fn execute_suspicious_bot_notice(
    ctx: &Context,
    guild_id: GuildId,
    log_channel_id: ChannelId,
    target: &User,
    trigger: BanTrigger,
) -> Result<(), HoneyPotError> {
    if !claim_ban(guild_id, target.id) {
        tracing::debug!(
            user_id = %target.id,
            "skipping duplicate suspicious-bot notice for already-handled user"
        );
        return Ok(());
    }

    tracing::warn!(
        guild_id = %guild_id,
        user_id = %target.id,
        trigger = trigger.kind(),
        "untrusted bot tripped honeypot; awaiting manual ban"
    );

    if let Err(error) = log_channel_id
        .send_message(&ctx.http, build_pending_message(target, &trigger))
        .await
    {
        forget_ban(guild_id, target.id);
        return Err(error.into());
    }

    Ok(())
}

/// Bans `user_id` from `guild_id` after a moderator confirms an untrusted-bot
/// notice, deleting [`DELETE_MESSAGE_DAYS`] of messages.
///
/// The `(guild, user)` pair is already claimed by [`execute_suspicious_bot_notice`];
/// the claim is left in place so repeat triggers stay suppressed until an unban
/// releases it. Unlike [`execute_ban`], this does not post a new log message —
/// the interaction handler edits the existing notice in place.
pub async fn confirm_bot_ban(
    ctx: &Context,
    guild_id: GuildId,
    user_id: UserId,
) -> Result<(), HoneyPotError> {
    let reason = "Honeypot triggered (bot, manually confirmed)";
    if crate::settings::dry_run() {
        tracing::warn!(
            guild_id = %guild_id,
            user_id = %user_id,
            "dry-run: skipping ban_with_reason"
        );
    } else {
        guild_id
            .ban_with_reason(&ctx.http, user_id, DELETE_MESSAGE_DAYS, reason)
            .await?;
    }

    tracing::info!(
        guild_id = %guild_id,
        user_id = %user_id,
        "banned bot after manual confirmation"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(ids: &[u64]) -> Vec<RoleId> {
        ids.iter().copied().map(RoleId::new).collect()
    }

    #[test]
    fn detects_honeypot_role_via_intersection_when_old_unknown() {
        let honeypot = roles(&[10]);
        let new = roles(&[1, 10, 2]);
        assert_eq!(
            newly_acquired_honeypot_role(&honeypot, &new, None),
            Some(RoleId::new(10))
        );
    }

    #[test]
    fn intersection_misses_when_no_honeypot_role_held() {
        let honeypot = roles(&[10]);
        let new = roles(&[1, 2, 3]);
        assert_eq!(newly_acquired_honeypot_role(&honeypot, &new, None), None);
    }

    #[test]
    fn detects_honeypot_role_via_diff_when_old_known() {
        let honeypot = roles(&[10]);
        let old = roles(&[1, 2]);
        let new = roles(&[1, 2, 10]);
        assert_eq!(
            newly_acquired_honeypot_role(&honeypot, &new, Some(&old)),
            Some(RoleId::new(10))
        );
    }

    #[test]
    fn diff_ignores_honeypot_role_already_held() {
        let honeypot = roles(&[10]);
        let old = roles(&[10, 1]);
        let new = roles(&[10, 1, 2]);
        assert_eq!(
            newly_acquired_honeypot_role(&honeypot, &new, Some(&old)),
            None
        );
    }

    #[test]
    fn empty_honeypot_list_never_matches() {
        let new = roles(&[1, 2, 3]);
        assert_eq!(newly_acquired_honeypot_role(&[], &new, None), None);
    }

    #[test]
    fn honeypot_channel_hit_and_miss() {
        let honeypot = vec![ChannelId::new(100), ChannelId::new(200)];
        assert!(is_honeypot_channel(&honeypot, ChannelId::new(100)));
        assert!(!is_honeypot_channel(&honeypot, ChannelId::new(300)));
        assert!(!is_honeypot_channel(&[], ChannelId::new(100)));
    }

    #[test]
    fn unban_custom_id_format() {
        assert_eq!(unban_custom_id(UserId::new(123)), "uhp_unban:123");
    }

    #[test]
    fn parse_unban_custom_id_roundtrips() {
        let id = UserId::new(123456789012345678);
        assert_eq!(parse_unban_custom_id(&unban_custom_id(id)), Some(id));
    }

    #[test]
    fn parse_unban_custom_id_rejects_non_matching() {
        assert_eq!(parse_unban_custom_id("other_button:123"), None);
        assert_eq!(parse_unban_custom_id("uhp_unban"), None);
        assert_eq!(parse_unban_custom_id("uhp_unban:"), None);
        assert_eq!(parse_unban_custom_id("uhp_unban:not_a_number"), None);
    }

    #[test]
    fn ban_custom_id_format() {
        assert_eq!(ban_custom_id(UserId::new(123)), "uhp_ban:123");
    }

    #[test]
    fn parse_ban_custom_id_roundtrips() {
        let id = UserId::new(123456789012345678);
        assert_eq!(parse_ban_custom_id(&ban_custom_id(id)), Some(id));
    }

    #[test]
    fn ban_and_unban_custom_ids_do_not_collide() {
        let id = UserId::new(123456789012345678);
        // Each parser must reject the other's id so the interaction dispatcher
        // can tell a manual-ban click from an unban click.
        assert_eq!(parse_unban_custom_id(&ban_custom_id(id)), None);
        assert_eq!(parse_ban_custom_id(&unban_custom_id(id)), None);
    }

    #[test]
    fn parse_ban_custom_id_rejects_non_matching() {
        assert_eq!(parse_ban_custom_id("other_button:123"), None);
        assert_eq!(parse_ban_custom_id("uhp_ban"), None);
        assert_eq!(parse_ban_custom_id("uhp_ban:"), None);
        assert_eq!(parse_ban_custom_id("uhp_ban:not_a_number"), None);
    }

    #[test]
    fn parse_unban_confirm_custom_id_roundtrips() {
        let user = UserId::new(123456789012345678);
        let message = MessageId::new(987654321098765432);
        assert_eq!(
            parse_unban_confirm_custom_id(&unban_confirm_custom_id(user, message)),
            Some((user, message))
        );
    }

    #[test]
    fn parse_ban_confirm_custom_id_roundtrips() {
        let user = UserId::new(123456789012345678);
        let message = MessageId::new(987654321098765432);
        assert_eq!(
            parse_ban_confirm_custom_id(&ban_confirm_custom_id(user, message)),
            Some((user, message))
        );
    }

    #[test]
    fn confirm_parsers_reject_non_matching() {
        // Missing message segment, wrong prefix, and non-numeric parts.
        assert_eq!(parse_unban_confirm_custom_id("uhp_unban_do:123"), None);
        assert_eq!(parse_unban_confirm_custom_id("uhp_unban_do:123:x"), None);
        assert_eq!(parse_ban_confirm_custom_id("uhp_ban_do"), None);
        assert_eq!(parse_ban_confirm_custom_id("uhp_ban_do:"), None);
    }

    #[test]
    fn plain_and_confirm_custom_ids_do_not_collide() {
        let user = UserId::new(123456789012345678);
        let message = MessageId::new(987654321098765432);
        // A confirm id must not be read as a plain button click, and a plain
        // button id must not be read as a confirmation, in either direction.
        assert_eq!(
            parse_unban_custom_id(&unban_confirm_custom_id(user, message)),
            None
        );
        assert_eq!(
            parse_ban_custom_id(&ban_confirm_custom_id(user, message)),
            None
        );
        assert_eq!(parse_unban_confirm_custom_id(&unban_custom_id(user)), None);
        assert_eq!(parse_ban_confirm_custom_id(&ban_custom_id(user)), None);
        // The two confirm prefixes are mutually exclusive as well.
        assert_eq!(
            parse_unban_confirm_custom_id(&ban_confirm_custom_id(user, message)),
            None
        );
        assert_eq!(
            parse_ban_confirm_custom_id(&unban_confirm_custom_id(user, message)),
            None
        );
    }

    #[test]
    fn ban_trigger_kind_and_detail() {
        let role = BanTrigger::Role(RoleId::new(42));
        assert_eq!(role.kind(), "role");
        assert_eq!(role.detail(), "<@&42>");

        let channel = BanTrigger::Channel {
            channel_id: ChannelId::new(84),
            content: None,
        };
        assert_eq!(channel.kind(), "channel");
        assert_eq!(channel.detail(), "<#84>");
    }

    #[test]
    fn message_content_only_for_channel_trigger() {
        assert_eq!(BanTrigger::Role(RoleId::new(1)).message_content(), None);
        assert_eq!(
            BanTrigger::Channel {
                channel_id: ChannelId::new(1),
                content: None,
            }
            .message_content(),
            None
        );
        assert_eq!(
            BanTrigger::Channel {
                channel_id: ChannelId::new(1),
                content: Some("spam".to_string()),
            }
            .message_content(),
            Some("spam")
        );
    }

    #[test]
    fn message_field_value_shown_only_for_non_empty_channel_content() {
        // Role trigger: never a message.
        assert_eq!(message_field_value(&BanTrigger::Role(RoleId::new(1))), None);
        // Channel trigger without captured content.
        assert_eq!(
            message_field_value(&BanTrigger::Channel {
                channel_id: ChannelId::new(1),
                content: None,
            }),
            None
        );
        // Channel trigger with empty content is suppressed.
        assert_eq!(
            message_field_value(&BanTrigger::Channel {
                channel_id: ChannelId::new(1),
                content: Some(String::new()),
            }),
            None
        );
        // Channel trigger with real content is formatted.
        assert_eq!(
            message_field_value(&BanTrigger::Channel {
                channel_id: ChannelId::new(1),
                content: Some("spam".to_string()),
            }),
            Some("```\nspam\n```".to_string())
        );
    }

    #[test]
    fn message_field_wraps_in_code_fence() {
        assert_eq!(message_field("hello"), "```\nhello\n```");
    }

    #[test]
    fn message_field_neutralizes_backticks() {
        // Only the fence's own backticks may remain, or a spammer could break
        // out of it: the body must carry none of the input's backticks.
        let rendered = message_field("evil ``` content");
        let body = rendered
            .trim_start_matches("```\n")
            .trim_end_matches("\n```");
        assert!(!body.contains('`'));
        assert_eq!(body, "evil ''' content");
    }

    #[test]
    fn message_field_truncates_overlong_content() {
        let long = "a".repeat(MAX_MESSAGE_LEN + 50);
        let rendered = message_field(&long);
        assert!(rendered.ends_with("…\n```"));
        let shown = rendered
            .trim_start_matches("```\n")
            .trim_end_matches("\n```")
            .trim_end_matches('…');
        assert_eq!(shown.chars().count(), MAX_MESSAGE_LEN);
    }

    #[test]
    fn message_field_keeps_content_at_the_limit_untruncated() {
        let exact = "a".repeat(MAX_MESSAGE_LEN);
        let rendered = message_field(&exact);
        assert!(!rendered.contains('…'));
    }

    #[test]
    fn claim_is_idempotent_until_forgotten() {
        // Unique ids keep this test independent of the shared global set.
        let guild = GuildId::new(9_000_000_000_000_001);
        let user = UserId::new(9_000_000_000_000_002);

        assert!(claim_ban(guild, user), "first claim should succeed");
        assert!(!claim_ban(guild, user), "second claim should be skipped");

        forget_ban(guild, user);
        assert!(claim_ban(guild, user), "claim allowed again after forget");

        forget_ban(guild, user);
    }

    #[test]
    fn target_tag_backticks_are_neutralized() {
        let mut user = User::default();
        user.name = "ev`il".to_string();
        user.discriminator = None;
        assert!(!target_field(&user).contains("ev`il"));
    }
}
