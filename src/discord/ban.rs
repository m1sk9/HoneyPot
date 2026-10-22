//! Shared ban execution for honeypot triggers.
//!
//! Both honeypot paths (role acquisition and channel post) funnel through
//! [`execute_ban`]: it bans the offender, then posts a log embed carrying an
//! `Unban` button to the guild's log channel. Detection is factored into pure
//! predicates ([`newly_acquired_honeypot_role`], [`is_honeypot_channel`]) so the
//! HTTP-free logic stays unit-testable.
//!
//! Because twilight keeps no cache, `GUILD_MEMBER_UPDATE` fires for a
//! honeypot-role holder on *any* update (nickname, timeout, …), and an offender
//! can trip both honeypot paths at once. [`execute_ban`] therefore claims each
//! `(guild, user)` before acting so concurrent or repeated triggers ban and
//! notify only once.

use crate::error::HoneyPotError;
use crate::i18n::{Language, Messages};
use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use twilight_http::Client;
use twilight_http::request::AuditLogReason;
use twilight_mention::Mention;
use twilight_model::channel::message::component::{ActionRow, Button, ButtonStyle};
use twilight_model::channel::message::embed::EmbedFooter;
use twilight_model::channel::message::{Component, Embed};
use twilight_model::guild::audit_log::{AuditLogChange, AuditLogEntry, AuditLogEventType};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, MessageMarker, RoleMarker, UserMarker},
};
use twilight_model::user::User;
use twilight_model::util::Timestamp;
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder};
use twilight_util::snowflake::Snowflake;

/// Embed colour of a ban notice (Discord's "red").
pub(crate) const COLOUR_RED: u32 = 0xE7_4C_3C;

/// Embed colour of a notice awaiting manual review (Discord's "gold").
pub(crate) const COLOUR_GOLD: u32 = 0xF1_C4_0F;

/// Embed colour of a lifted ban (Discord's "dark green").
pub(crate) const COLOUR_DARK_GREEN: u32 = 0x1F_8B_4C;

/// The current time as a [`Timestamp`], at whole-second precision.
pub(crate) fn now() -> Timestamp {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before the UNIX epoch")
        .as_secs();
    Timestamp::from_secs(secs as i64).expect("current time is a valid timestamp")
}

/// When `user`'s account was created, decoded from its snowflake id.
pub(crate) fn created_at(user: &User) -> Timestamp {
    Timestamp::from_secs(user.id.timestamp() / 1000).expect("snowflake time is a valid timestamp")
}

/// The user's tag: the bare username for a migrated (pomelo) account, whose
/// discriminator is `0`, or `name#0001` for a legacy one.
pub(crate) fn user_tag(user: &User) -> String {
    if user.discriminator == 0 {
        user.name.clone()
    } else {
        format!("{}#{}", user.name, user.discriminator())
    }
}

/// A clickable button carrying `custom_id`.
pub(crate) fn button(custom_id: impl Into<String>, label: &str, style: ButtonStyle) -> Component {
    Component::Button(Button {
        id: None,
        custom_id: Some(custom_id.into()),
        disabled: false,
        emoji: None,
        label: Some(label.to_string()),
        style,
        url: None,
        sku_id: None,
    })
}

/// An action row holding `components`.
pub(crate) fn buttons_row(components: Vec<Component>) -> Component {
    Component::ActionRow(ActionRow {
        id: None,
        components,
    })
}

/// A set of `(guild, user)` pairs shared across event tasks.
type GuildUserSet = LazyLock<Mutex<HashSet<(Id<GuildMarker>, Id<UserMarker>)>>>;

/// `(guild, user)` pairs already banned by a honeypot trigger.
///
/// Kept on success so concurrent events (e.g. a honeypot post *and* role gained
/// at once) or repeated `GUILD_MEMBER_UPDATE`s don't post duplicate log embeds.
/// A failed ban releases its claim (see [`execute_ban`]) so a later event can
/// retry, and [`forget_ban`] lets a future unban handler allow re-banning.
static HANDLED_BANS: GuildUserSet = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Claims `(guild_id, user_id)`, returning `true` only on the first claim.
///
/// Subsequent claims return `false` until [`forget_ban`] releases the pair.
fn claim_ban(guild_id: Id<GuildMarker>, user_id: Id<UserMarker>) -> bool {
    HANDLED_BANS
        .lock()
        .expect("HANDLED_BANS mutex poisoned")
        .insert((guild_id, user_id))
}

/// Releases a claim so `(guild_id, user_id)` can be banned again.
///
/// Called internally when a ban fails, and intended for the future unban
/// handler to call after lifting a ban.
pub fn forget_ban(guild_id: Id<GuildMarker>, user_id: Id<UserMarker>) {
    HANDLED_BANS
        .lock()
        .expect("HANDLED_BANS mutex poisoned")
        .remove(&(guild_id, user_id));
}

/// `(guild, user)` pairs already posted as a third-party role-grant review notice.
///
/// Deliberately separate from [`HANDLED_BANS`]: a third-party grant usually lands
/// on a *legitimate* member (a mistaken admin grant), so its notice must dedupe
/// repeat `GUILD_MEMBER_UPDATE`s **without** marking the member as handled. Were
/// it to reuse `HANDLED_BANS`, a later genuine trigger — a honeypot channel post,
/// a self-assign — would be silently suppressed by the lingering claim. Released
/// by [`forget_grant_notice`] when the member is unbanned.
static NOTIFIED_GRANTS: GuildUserSet = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Claims a third-party grant notice, returning `true` only on the first claim
/// so repeat triggers don't post duplicate notices.
fn claim_grant_notice(guild_id: Id<GuildMarker>, user_id: Id<UserMarker>) -> bool {
    NOTIFIED_GRANTS
        .lock()
        .expect("NOTIFIED_GRANTS mutex poisoned")
        .insert((guild_id, user_id))
}

/// Releases a third-party grant notice claim so a future grant can notify again.
pub fn forget_grant_notice(guild_id: Id<GuildMarker>, user_id: Id<UserMarker>) {
    NOTIFIED_GRANTS
        .lock()
        .expect("NOTIFIED_GRANTS mutex poisoned")
        .remove(&(guild_id, user_id));
}

/// Prefix for the unban button `custom_id`. The full id is `uhp_unban:{user_id}`;
/// the button handler parses the suffix as a user id. Clicking it does not
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

/// Seconds' worth of the offender's messages to delete on ban (one day).
const DELETE_MESSAGE_SECONDS: u32 = 86_400;

/// Builds the unban button `custom_id` for `user_id`.
fn unban_custom_id(user_id: Id<UserMarker>) -> String {
    format!("{UNBAN_CUSTOM_ID_PREFIX}:{user_id}")
}

/// Parses a non-zero snowflake id; `Id::new` panics on `0`, which a forged
/// `custom_id` could otherwise carry.
fn parse_id<M>(value: &str) -> Option<Id<M>> {
    value.parse::<u64>().ok().and_then(Id::new_checked)
}

/// Parses an unban button `custom_id` (`uhp_unban:{user_id}`) into its target
/// user id. Returns `None` for any other `custom_id`.
///
/// Inverse of [`unban_custom_id`]; kept alongside it so the encoding stays in
/// sync.
pub fn parse_unban_custom_id(custom_id: &str) -> Option<Id<UserMarker>> {
    let suffix = custom_id
        .strip_prefix(UNBAN_CUSTOM_ID_PREFIX)?
        .strip_prefix(':')?;
    parse_id(suffix)
}

/// Builds the manual-ban button `custom_id` for `user_id`.
fn ban_custom_id(user_id: Id<UserMarker>) -> String {
    format!("{BAN_CUSTOM_ID_PREFIX}:{user_id}")
}

/// Parses a manual-ban button `custom_id` (`uhp_ban:{user_id}`) into its target
/// user id. Returns `None` for any other `custom_id`.
///
/// The `uhp_ban` prefix is not a prefix of `uhp_unban` (they diverge at the
/// fifth byte), so this never matches an unban id and vice versa.
pub fn parse_ban_custom_id(custom_id: &str) -> Option<Id<UserMarker>> {
    let suffix = custom_id
        .strip_prefix(BAN_CUSTOM_ID_PREFIX)?
        .strip_prefix(':')?;
    parse_id(suffix)
}

/// Builds the "confirm unban" button `custom_id` for `user_id`, embedding the
/// `message_id` of the log message to edit once the unban is confirmed.
fn unban_confirm_custom_id(user_id: Id<UserMarker>, message_id: Id<MessageMarker>) -> String {
    format!("{UNBAN_CONFIRM_CUSTOM_ID_PREFIX}:{user_id}:{message_id}")
}

/// Parses a "confirm unban" `custom_id` (`uhp_unban_do:{user_id}:{message_id}`)
/// into its target user and the log message to edit. Returns `None` for any
/// other `custom_id`.
pub fn parse_unban_confirm_custom_id(
    custom_id: &str,
) -> Option<(Id<UserMarker>, Id<MessageMarker>)> {
    parse_confirm_custom_id(UNBAN_CONFIRM_CUSTOM_ID_PREFIX, custom_id)
}

/// Builds the "confirm ban" button `custom_id` for `user_id`, embedding the
/// `message_id` of the notice to edit once the ban is confirmed.
fn ban_confirm_custom_id(user_id: Id<UserMarker>, message_id: Id<MessageMarker>) -> String {
    format!("{BAN_CONFIRM_CUSTOM_ID_PREFIX}:{user_id}:{message_id}")
}

/// Parses a "confirm ban" `custom_id` (`uhp_ban_do:{user_id}:{message_id}`) into
/// its target user and the notice message to edit. Returns `None` for any other
/// `custom_id`.
pub fn parse_ban_confirm_custom_id(custom_id: &str) -> Option<(Id<UserMarker>, Id<MessageMarker>)> {
    parse_confirm_custom_id(BAN_CONFIRM_CUSTOM_ID_PREFIX, custom_id)
}

/// Shared parser for the `{prefix}:{user_id}:{message_id}` confirmation ids.
fn parse_confirm_custom_id(
    prefix: &str,
    custom_id: &str,
) -> Option<(Id<UserMarker>, Id<MessageMarker>)> {
    let suffix = custom_id.strip_prefix(prefix)?.strip_prefix(':')?;
    let (user, message) = suffix.split_once(':')?;
    Some((parse_id(user)?, parse_id(message)?))
}

/// Builds the action row carrying the `Unban` button for `user_id`.
pub fn unban_action_row(user_id: Id<UserMarker>, language: Language) -> Component {
    buttons_row(vec![button(
        unban_custom_id(user_id),
        language.messages().btn_unban,
        ButtonStyle::Danger,
    )])
}

/// Builds the action row carrying the `Ban` button of a review notice.
fn review_action_row(user_id: Id<UserMarker>, language: Language) -> Component {
    buttons_row(vec![button(
        ban_custom_id(user_id),
        language.messages().btn_ban,
        ButtonStyle::Danger,
    )])
}

/// Builds the cancel button shared by both ephemeral confirmations.
fn cancel_button(msg: &Messages) -> Component {
    button(CANCEL_CUSTOM_ID, msg.btn_cancel, ButtonStyle::Secondary)
}

/// Builds the confirmation action row for an unban: a danger `Confirm unban`
/// button (carrying `message_id`) alongside `Cancel`.
pub fn confirm_unban_action_row(
    user_id: Id<UserMarker>,
    message_id: Id<MessageMarker>,
    language: Language,
) -> Component {
    let msg = language.messages();
    let confirm = button(
        unban_confirm_custom_id(user_id, message_id),
        msg.btn_confirm_unban,
        ButtonStyle::Danger,
    );
    buttons_row(vec![confirm, cancel_button(msg)])
}

/// Builds the confirmation action row for a manual ban: a danger `Confirm ban`
/// button (carrying `message_id`) alongside `Cancel`.
pub fn confirm_ban_action_row(
    user_id: Id<UserMarker>,
    message_id: Id<MessageMarker>,
    language: Language,
) -> Component {
    let msg = language.messages();
    let confirm = button(
        ban_confirm_custom_id(user_id, message_id),
        msg.btn_confirm_ban,
        ButtonStyle::Danger,
    );
    buttons_row(vec![confirm, cancel_button(msg)])
}

/// Which honeypot fired, carried into the log embed.
pub enum BanTrigger {
    /// The offender acquired this honeypot role.
    Role(Id<RoleMarker>),
    /// The offender posted in this honeypot channel.
    Channel {
        /// The honeypot channel posted in.
        channel_id: Id<ChannelMarker>,
        /// The offending message's content, shown in the log embed so a
        /// moderator can confirm it was spam. `None` when it wasn't captured
        /// (e.g. an empty message); role triggers never carry content.
        content: Option<String>,
    },
}

impl BanTrigger {
    /// Trigger kind token for the audit-log ban reason, kept English (and thus
    /// stable/searchable) regardless of the guild's display language.
    fn kind(&self) -> &'static str {
        match self {
            BanTrigger::Role(_) => "role",
            BanTrigger::Channel { .. } => "channel",
        }
    }

    /// Localized trigger-kind label for the embed's "Trigger" field.
    fn label(&self, msg: &Messages) -> &'static str {
        match self {
            BanTrigger::Role(_) => msg.trigger_role,
            BanTrigger::Channel { .. } => msg.trigger_channel,
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
/// considered. When `None`, falls back to the intersection of the full new role
/// set with the honeypots. twilight keeps no cache and `MEMBER_UPDATE` carries
/// no previous roles, so the handler always passes `None` today; the diff path
/// is kept (and tested) so a future cache can supply old roles without rework.
pub fn newly_acquired_honeypot_role(
    honeypot_role_ids: &[Id<RoleMarker>],
    new_roles: &[Id<RoleMarker>],
    old_roles: Option<&[Id<RoleMarker>]>,
) -> Option<Id<RoleMarker>> {
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
pub fn is_honeypot_channel(
    honeypot_channel_ids: &[Id<ChannelMarker>],
    channel_id: Id<ChannelMarker>,
) -> bool {
    honeypot_channel_ids.contains(&channel_id)
}

/// How many recent `MEMBER_ROLE_UPDATE` audit-log entries to scan when resolving
/// who granted a honeypot role. Grants are rare, so a modest window is plenty
/// even when several members' roles change around the same time.
const AUDIT_LOG_SCAN_LIMIT: u16 = 50;

/// How many times to poll the audit log before giving up. The entry can lag the
/// gateway event, so a self-assign (which should fire) isn't misread as unknown.
const AUDIT_LOG_LOOKUP_ATTEMPTS: usize = 3;

/// Delay between audit-log poll attempts.
const AUDIT_LOG_RETRY_DELAY: Duration = Duration::from_millis(400);

/// Who granted a honeypot role, as far as the audit log can tell.
///
/// Only a self-assign fires the trap; every other outcome is held for manual
/// review, erring against a false ban when the grantor can't be trusted.
#[derive(Debug, PartialEq, Eq)]
pub enum RoleGrantSource {
    /// The member granted the role to themselves — onboarding or a self-assign
    /// menu. This is the trap's intended trigger, so it fires.
    SelfAssigned,
    /// A third party granted it: an admin by hand, or a reaction-role bot. Not
    /// the member's own doing, so it's held for review rather than auto-banned.
    ThirdParty,
    /// The grantor couldn't be determined — a missing or lagging audit-log entry,
    /// or a missing `VIEW_AUDIT_LOG` permission. Held for review so an unverifiable
    /// grant never triggers an automatic ban.
    Unknown,
}

/// Classifies a role grant from its resolved executor relative to the `target`.
///
/// `Some(target)` is a self-assign; any other executor is a third party; `None`
/// (executor unresolved) is unknown.
pub fn classify_role_grant(
    executor: Option<Id<UserMarker>>,
    target: Id<UserMarker>,
) -> RoleGrantSource {
    match executor {
        Some(id) if id == target => RoleGrantSource::SelfAssigned,
        Some(_) => RoleGrantSource::ThirdParty,
        None => RoleGrantSource::Unknown,
    }
}

/// Whether `entry` records `role` being *added* (a `$add` change containing it).
fn entry_added_role(entry: &AuditLogEntry, role: Id<RoleMarker>) -> bool {
    entry.changes.iter().any(|change| match change {
        AuditLogChange::RoleAdded { new, .. } => new.iter().any(|added| added.id == role),
        _ => false,
    })
}

/// Finds who added `role` to `target` among audit-log entries.
///
/// `entries` are Discord's `MEMBER_ROLE_UPDATE` entries, newest first; returns
/// the executor (`user_id`) of the most recent entry that added `role` to
/// `target`, or `None` if no such entry is present.
pub fn find_role_grant_executor(
    entries: &[AuditLogEntry],
    target: Id<UserMarker>,
    role: Id<RoleMarker>,
) -> Option<Id<UserMarker>> {
    entries
        .iter()
        .find(|entry| {
            entry.target_id.map(Id::get) == Some(target.get()) && entry_added_role(entry, role)
        })
        .and_then(|entry| entry.user_id)
}

/// Resolves who granted `role` to `target` by consulting the guild audit log.
///
/// The `MEMBER_ROLE_UPDATE` entry can lag the gateway event, so the lookup is
/// retried a few times before giving up. Returns [`RoleGrantSource::Unknown`]
/// when the executor can't be determined — a missing/lagging entry, or a missing
/// `VIEW_AUDIT_LOG` permission (which surfaces as an HTTP error and is not
/// retried, since retrying can't grant the permission).
pub async fn resolve_role_grant_source(
    http: &Client,
    guild_id: Id<GuildMarker>,
    target: Id<UserMarker>,
    role: Id<RoleMarker>,
) -> RoleGrantSource {
    for attempt in 0..AUDIT_LOG_LOOKUP_ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(AUDIT_LOG_RETRY_DELAY).await;
        }
        match fetch_role_update_log(http, guild_id).await {
            Ok(logs) => {
                if let Some(executor) = find_role_grant_executor(&logs.entries, target, role) {
                    return classify_role_grant(Some(executor), target);
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    guild_id = %guild_id,
                    user_id = %target,
                    "failed to read audit log for role grant; holding for manual review"
                );
                return RoleGrantSource::Unknown;
            }
        }
    }

    tracing::warn!(
        guild_id = %guild_id,
        user_id = %target,
        "no audit-log entry found for honeypot role grant; holding for manual review"
    );
    RoleGrantSource::Unknown
}

/// Fetches the guild's most recent `MEMBER_ROLE_UPDATE` audit-log entries.
async fn fetch_role_update_log(
    http: &Client,
    guild_id: Id<GuildMarker>,
) -> Result<twilight_model::guild::audit_log::AuditLog, HoneyPotError> {
    let response = http
        .audit_log(guild_id)
        .action_type(AuditLogEventType::MemberRoleUpdate)
        .limit(AUDIT_LOG_SCAN_LIMIT)
        .await?;
    Ok(response.model().await?)
}

/// Guild-membership details about an offender, captured at trigger time and
/// shown in the log embed as moderator decision aids.
///
/// Sourced from the triggering event — the member on a `GUILD_MEMBER_UPDATE`, or
/// the message's partial member on a channel post — so no extra HTTP fetch is
/// needed. `joined_at` is optional because a channel trigger's partial member
/// may omit it. `unusual_dm_activity_until` and `spammer` come from the raw
/// payload (see [`crate::discord::raw`]), since twilight-model drops both.
pub struct OffenderContext {
    /// When the offender joined the guild, if known.
    pub joined_at: Option<Timestamp>,
    /// When set to a future time, Discord has flagged the account for unusual
    /// DM activity (excessive DMs to non-friends). `None` or a past time means
    /// the account is not currently flagged.
    pub unusual_dm_activity_until: Option<Timestamp>,
    /// Discord has marked the account as a likely spammer.
    pub spammer: bool,
}

/// Accounts younger than this many days at trigger time are flagged as new in
/// the log embed — a throwaway account created just to spam is the common case.
const NEW_ACCOUNT_WARN_DAYS: i64 = 7;

/// Seconds in a day, for the [`NEW_ACCOUNT_WARN_DAYS`] age comparison.
const SECONDS_PER_DAY: i64 = 86_400;

/// Formats the target user field: mention, display name, tag, and raw ID.
///
/// The tag and display name embed user-controlled text inside inline code spans,
/// so backticks are neutralized to keep the spans from being broken (spoofing
/// the log embed's layout).
pub(crate) fn target_field(target: &User, msg: &Messages) -> String {
    let tag = user_tag(target).replace('`', "'");
    let mut field = format!("{} (`{}`)", target.id.mention(), tag);
    if let Some(global) = &target.global_name {
        let display = global.replace('`', "'");
        field.push_str(&format!("\n{}: `{display}`", msg.display_label));
    }
    field.push_str(&format!("\n{}: {}", msg.id_label, target.id));
    field
}

/// Renders a timestamp as Discord's absolute + relative markdown, e.g.
/// `<t:1700000000:F> (<t:1700000000:R>)`.
///
/// The client localizes both forms — including the mobile app, where the
/// moderator view that would otherwise surface this detail is unavailable.
pub(crate) fn timestamp_field(ts: Timestamp) -> String {
    let secs = ts.as_secs();
    format!("<t:{secs}:F> (<t:{secs}:R>)")
}

/// Whether `created` is within [`NEW_ACCOUNT_WARN_DAYS`] of `now`.
///
/// Takes `now` explicitly so the age check stays pure and unit-testable.
fn is_new_account(created: Timestamp, now: Timestamp) -> bool {
    now.as_secs() - created.as_secs() < NEW_ACCOUNT_WARN_DAYS * SECONDS_PER_DAY
}

/// The "Account created" field value: the account's creation date only. The
/// new-account warning is surfaced separately in the warnings field.
fn created_field(target: &User) -> String {
    timestamp_field(created_at(target))
}

/// The "Account" field value: whether the offender is a bot and/or a Discord
/// system account.
pub(crate) fn account_type_field(target: &User, msg: &Messages) -> String {
    let yes_no = |flag: bool| if flag { msg.yes } else { msg.no };
    format!(
        "{}: {}\n{}: {}",
        msg.bot_label,
        yes_no(target.bot),
        msg.system_label,
        yes_no(target.system.unwrap_or(false))
    )
}

/// The "Joined server" field value, or `Unknown` when the trigger did not carry
/// a join date (a channel trigger's partial member may omit it).
pub(crate) fn joined_field(offender: &OffenderContext, msg: &Messages) -> String {
    match offender.joined_at {
        Some(ts) => timestamp_field(ts),
        None => msg.joined_unknown.to_string(),
    }
}

/// Collects every risk signal into a single newline-separated list for the
/// "Warnings" field, or `None` when the account trips none of them (so the field
/// is omitted). Each line carries its own `⚠️`. `now` is passed so the age and
/// flag-expiry checks stay pure and unit-testable.
///
/// The signals, in order: a new account, a default (no custom) avatar, Discord's
/// spammer flag, and an active unusual-DM-activity flag. The account's other
/// badges are deliberately not shown — they argue for legitimacy and are
/// near-useless (and mostly absent) on the spam accounts this bot exists to
/// catch.
///
/// The spammer flag is read from [`OffenderContext::spammer`], not
/// `public_flags`: twilight's `UserFlags` truncates that bit away (see
/// [`crate::discord::raw`]).
fn warnings_field(
    target: &User,
    offender: &OffenderContext,
    now: Timestamp,
    msg: &Messages,
) -> Option<String> {
    let mut lines = Vec::new();

    if is_new_account(created_at(target), now) {
        lines.push(
            msg.new_account
                .replace("{}", &NEW_ACCOUNT_WARN_DAYS.to_string()),
        );
    }
    if target.avatar.is_none() {
        lines.push(msg.avatar_default.to_string());
    }
    if offender.spammer {
        lines.push(msg.spammer.to_string());
    }
    if let Some(until) = offender.unusual_dm_activity_until
        && until.as_secs() > now.as_secs()
    {
        lines.push(
            msg.unusual_dm_flagged
                .replace("{}", &timestamp_field(until)),
        );
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Appends the shared offender-detail fields — creation date, join date, and (when
/// any risk signal fires) a single aggregated "Warnings" field — to a log embed.
///
/// Common to the ban embed and the untrusted-bot notice so both carry the same
/// decision aids. `now` is sampled once so the age/flag checks agree.
fn with_offender_fields(
    embed: EmbedBuilder,
    target: &User,
    offender: &OffenderContext,
    msg: &Messages,
) -> EmbedBuilder {
    let now = now();
    let mut embed = embed
        .field(EmbedFieldBuilder::new(
            msg.field_account_created,
            created_field(target),
        ))
        .field(EmbedFieldBuilder::new(
            msg.field_joined,
            joined_field(offender, msg),
        ));
    if let Some(warnings) = warnings_field(target, offender, now, msg) {
        embed = embed.field(EmbedFieldBuilder::new(msg.field_warnings, warnings));
    }
    embed
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

/// The formatted offending-message body for `trigger`, shown in the embed
/// description, or `None` when there is nothing to show (a role trigger, or a
/// channel trigger whose content wasn't captured or was empty).
///
/// Kept pure so the "show it or not" decision stays unit-testable without
/// constructing an embed.
fn message_field_value(trigger: &BanTrigger) -> Option<String> {
    match trigger.message_content() {
        Some(content) if !content.is_empty() => Some(message_field(content)),
        _ => None,
    }
}

/// Appends a footer marking the embed as a dry-run when [`crate::settings::dry_run`]
/// is enabled, so simulated bans/unbans can't be mistaken for real ones in the
/// log channel. A no-op in normal operation.
pub(crate) fn apply_dry_run_marker(embed: Embed, msg: &Messages) -> Embed {
    mark_dry_run(embed, msg, crate::settings::dry_run())
}

fn mark_dry_run(mut embed: Embed, msg: &Messages, dry_run: bool) -> Embed {
    if dry_run {
        embed.footer = Some(EmbedFooter {
            icon_url: None,
            proxy_icon_url: None,
            text: msg.dry_run_footer.to_string(),
        });
    }
    embed
}

/// The User / Trigger / Account fields shared by every log embed.
fn with_trigger_fields(
    embed: EmbedBuilder,
    target: &User,
    trigger: &BanTrigger,
    msg: &Messages,
) -> EmbedBuilder {
    embed
        .field(EmbedFieldBuilder::new(
            msg.field_user,
            target_field(target, msg),
        ))
        .field(
            EmbedFieldBuilder::new(
                msg.field_trigger,
                format!("{} {}", trigger.label(msg), trigger.detail()),
            )
            .inline(),
        )
        .field(EmbedFieldBuilder::new(msg.field_account, account_type_field(target, msg)).inline())
}

/// Builds the ban notification embed.
pub(crate) fn build_ban_embed(
    target: &User,
    trigger: &BanTrigger,
    offender: &OffenderContext,
    language: Language,
) -> Embed {
    let msg = language.messages();
    let mut embed = with_trigger_fields(
        EmbedBuilder::new().title(msg.ban_title).color(COLOUR_RED),
        target,
        trigger,
        msg,
    )
    .timestamp(now());
    if let Some(body) = message_field_value(trigger) {
        embed = embed.description(body);
    }
    let embed = with_offender_fields(embed, target, offender, msg).build();
    apply_dry_run_marker(embed, msg)
}

/// Builds a GOLD "awaiting manual review" embed with the given `title` and
/// `desc_body`, plus the shared User / Trigger / Account and offender fields.
///
/// Backs both review notices — the untrusted-bot notice and the third-party role
/// grant notice — which differ only in wording; sharing one builder keeps the two
/// from drifting apart.
fn build_review_embed(
    title: &'static str,
    desc_body: &'static str,
    target: &User,
    trigger: &BanTrigger,
    offender: &OffenderContext,
    language: Language,
) -> Embed {
    let msg = language.messages();
    let description = match message_field_value(trigger) {
        Some(body) => format!("{desc_body}\n\n{body}"),
        None => desc_body.to_string(),
    };
    let embed = with_trigger_fields(
        EmbedBuilder::new()
            .title(title)
            .description(description)
            .color(COLOUR_GOLD),
        target,
        trigger,
        msg,
    )
    .timestamp(now());
    let embed = with_offender_fields(embed, target, offender, msg).build();
    apply_dry_run_marker(embed, msg)
}

/// Builds the "untrusted bot" notice embed.
///
/// Unlike [`build_ban_embed`], no ban has happened yet: the bot tripped a
/// honeypot but is not in the guild's trusted list, so a moderator must decide.
pub(crate) fn build_pending_embed(
    target: &User,
    trigger: &BanTrigger,
    offender: &OffenderContext,
    language: Language,
) -> Embed {
    let msg = language.messages();
    build_review_embed(
        msg.pending_title,
        msg.pending_desc,
        target,
        trigger,
        offender,
        language,
    )
}

/// Builds the "third-party role grant" notice embed.
///
/// A honeypot role was granted by someone other than the member (an admin, a
/// reaction-role bot) or by an unresolved grantor, so the trap was not fired and
/// no ban applied; a moderator decides via the `Ban` button.
pub(crate) fn build_third_party_grant_embed(
    target: &User,
    trigger: &BanTrigger,
    offender: &OffenderContext,
    language: Language,
) -> Embed {
    let msg = language.messages();
    build_review_embed(
        msg.manual_grant_title,
        msg.manual_grant_desc,
        target,
        trigger,
        offender,
        language,
    )
}

/// Posts `embed` with a single action row (`row`) to `channel_id`.
///
/// Shared by the ban log and both review notices; a review notice's `Ban` button
/// funnels into the same manual-confirmation flow ([`confirm_bot_ban`]).
async fn post_log_message(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    embed: Embed,
    row: Component,
) -> Result<(), HoneyPotError> {
    let embeds = [embed];
    let components = [row];
    http.create_message(channel_id)
        .embeds(&embeds)
        .components(&components)
        .await?;
    Ok(())
}

/// Bans `target` from `guild_id` and posts a log embed to `log_channel_id`.
///
/// Idempotent per `(guild, user)`: a duplicate trigger while the offender is
/// already claimed returns early without re-banning or re-notifying. Bans first
/// (deleting [`DELETE_MESSAGE_SECONDS`] of messages), then notifies; a failed
/// notification is logged but does not undo or mask the successful ban. A failed
/// ban releases the claim so a later event can retry.
pub async fn execute_ban(
    http: &Client,
    guild_id: Id<GuildMarker>,
    log_channel_id: Id<ChannelMarker>,
    target: &User,
    trigger: BanTrigger,
    offender: &OffenderContext,
    language: Language,
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
            "dry-run: skipping create_ban"
        );
    } else if let Err(error) = http
        .create_ban(guild_id, target.id)
        .delete_message_seconds(DELETE_MESSAGE_SECONDS)
        .reason(&reason)
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

    if let Err(error) = post_log_message(
        http,
        log_channel_id,
        build_ban_embed(target, &trigger, offender, language),
        unban_action_row(target.id, language),
    )
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
    http: &Client,
    guild_id: Id<GuildMarker>,
    log_channel_id: Id<ChannelMarker>,
    target: &User,
    trigger: BanTrigger,
    offender: &OffenderContext,
    language: Language,
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

    if let Err(error) = post_log_message(
        http,
        log_channel_id,
        build_pending_embed(target, &trigger, offender, language),
        review_action_row(target.id, language),
    )
    .await
    {
        forget_ban(guild_id, target.id);
        return Err(error);
    }

    Ok(())
}

/// Posts a "third-party role grant" review notice to `log_channel_id` without
/// banning.
///
/// Used when a honeypot *role* was granted by someone other than the member (an
/// admin by hand, a reaction-role bot) or by a grantor the audit log couldn't
/// resolve: rather than auto-banning — which would misfire on a mistaken manual
/// grant — the decision is deferred to a moderator via the `Ban` button (handled
/// by [`confirm_bot_ban`]).
///
/// Dedupes on [`NOTIFIED_GRANTS`] — not [`HANDLED_BANS`] — so repeat triggers
/// don't post duplicate notices, while leaving the (usually legitimate) member
/// free to be caught by a later genuine trigger. A failed post releases the claim
/// so a later trigger can retry.
pub async fn execute_third_party_grant_notice(
    http: &Client,
    guild_id: Id<GuildMarker>,
    log_channel_id: Id<ChannelMarker>,
    target: &User,
    trigger: BanTrigger,
    offender: &OffenderContext,
    language: Language,
) -> Result<(), HoneyPotError> {
    if !claim_grant_notice(guild_id, target.id) {
        tracing::debug!(
            user_id = %target.id,
            "skipping duplicate third-party-grant notice for already-notified user"
        );
        return Ok(());
    }

    tracing::warn!(
        guild_id = %guild_id,
        user_id = %target.id,
        "honeypot role granted by a third party or unknown grantor; awaiting manual review"
    );

    if let Err(error) = post_log_message(
        http,
        log_channel_id,
        build_third_party_grant_embed(target, &trigger, offender, language),
        review_action_row(target.id, language),
    )
    .await
    {
        forget_grant_notice(guild_id, target.id);
        return Err(error);
    }

    Ok(())
}

/// Bans `user_id` from `guild_id` after a moderator confirms a review notice,
/// deleting [`DELETE_MESSAGE_SECONDS`] of messages.
///
/// Claims `(guild, user)` in [`HANDLED_BANS`] so the banned member isn't
/// re-processed and a later unban ([`forget_ban`]) can release it. This is what
/// records the ban for the third-party-grant path, which deduped on
/// [`NOTIFIED_GRANTS`] and never touched `HANDLED_BANS`; the untrusted-bot path
/// already claimed the pair, so the claim here is idempotent. Unlike
/// [`execute_ban`], this does not post a new log message — the interaction
/// handler edits the existing notice in place.
pub async fn confirm_bot_ban(
    http: &Client,
    guild_id: Id<GuildMarker>,
    user_id: Id<UserMarker>,
) -> Result<(), HoneyPotError> {
    let reason = "Honeypot triggered (bot, manually confirmed)";
    if crate::settings::dry_run() {
        tracing::warn!(
            guild_id = %guild_id,
            user_id = %user_id,
            "dry-run: skipping create_ban"
        );
    } else {
        http.create_ban(guild_id, user_id)
            .delete_message_seconds(DELETE_MESSAGE_SECONDS)
            .reason(reason)
            .await?;
    }

    claim_ban(guild_id, user_id);

    tracing::info!(
        guild_id = %guild_id,
        user_id = %user_id,
        "banned member after manual confirmation"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::fixtures;
    use twilight_model::guild::audit_log::AffectedRole;
    use twilight_model::util::ImageHash;

    fn roles(ids: &[u64]) -> Vec<Id<RoleMarker>> {
        ids.iter().copied().map(Id::new).collect()
    }

    /// The English catalog, used by the field-helper tests that assert on the
    /// default wording.
    fn en() -> &'static Messages {
        Language::En.messages()
    }

    #[test]
    fn detects_honeypot_role_via_intersection_when_old_unknown() {
        let honeypot = roles(&[10]);
        let new = roles(&[1, 10, 2]);
        assert_eq!(
            newly_acquired_honeypot_role(&honeypot, &new, None),
            Some(Id::new(10))
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
            Some(Id::new(10))
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
        let honeypot = vec![Id::new(100), Id::new(200)];
        assert!(is_honeypot_channel(&honeypot, Id::new(100)));
        assert!(!is_honeypot_channel(&honeypot, Id::new(300)));
        assert!(!is_honeypot_channel(&[], Id::new(100)));
    }

    #[test]
    fn classify_role_grant_distinguishes_self_third_party_and_unknown() {
        let target = Id::new(100);
        assert_eq!(
            classify_role_grant(Some(target), target),
            RoleGrantSource::SelfAssigned
        );
        assert_eq!(
            classify_role_grant(Some(Id::new(200)), target),
            RoleGrantSource::ThirdParty
        );
        assert_eq!(classify_role_grant(None, target), RoleGrantSource::Unknown);
    }

    /// A `MEMBER_ROLE_UPDATE` entry by `executor` on `target` carrying `change`.
    fn role_update_entry(
        entry_id: u64,
        target: u64,
        executor: u64,
        change: AuditLogChange,
    ) -> AuditLogEntry {
        AuditLogEntry {
            action_type: AuditLogEventType::MemberRoleUpdate,
            changes: vec![change],
            guild_id: None,
            id: Id::new(entry_id),
            options: None,
            reason: None,
            target_id: Some(Id::new(target)),
            user_id: Some(Id::new(executor)),
        }
    }

    /// The honeypot role `role` as it appears in an audit-log role change.
    fn affected(role: u64) -> Vec<AffectedRole> {
        vec![AffectedRole {
            id: Id::new(role),
            name: "Honeypot".to_string(),
        }]
    }

    /// One `$add` entry: `executor` added `role` to `target`, entry id `entry_id`.
    fn role_add_entry(entry_id: u64, target: u64, executor: u64, role: u64) -> AuditLogEntry {
        role_update_entry(
            entry_id,
            target,
            executor,
            AuditLogChange::RoleAdded {
                new: affected(role),
                old: Vec::new(),
            },
        )
    }

    #[test]
    fn find_role_grant_executor_returns_matching_grantor() {
        let entries = [role_add_entry(1, 100, 200, 10)];
        assert_eq!(
            find_role_grant_executor(&entries, Id::new(100), Id::new(10)),
            Some(Id::new(200))
        );
    }

    #[test]
    fn find_role_grant_executor_prefers_the_newest_entry() {
        // Entries arrive newest-first; the first matching entry wins.
        let entries = [
            role_add_entry(2, 100, 999, 10),
            role_add_entry(1, 100, 200, 10),
        ];
        assert_eq!(
            find_role_grant_executor(&entries, Id::new(100), Id::new(10)),
            Some(Id::new(999))
        );
    }

    #[test]
    fn find_role_grant_executor_ignores_other_targets_and_roles() {
        let entries = [
            role_add_entry(1, 555, 200, 10),
            role_add_entry(2, 100, 200, 77),
        ];
        assert_eq!(
            find_role_grant_executor(&entries, Id::new(100), Id::new(10)),
            None
        );
    }

    #[test]
    fn find_role_grant_executor_ignores_role_removals() {
        // A `$remove` of the honeypot role is not a grant.
        let entries = [role_update_entry(
            1,
            100,
            200,
            AuditLogChange::RoleRemoved {
                new: affected(10),
                old: Vec::new(),
            },
        )];
        assert_eq!(
            find_role_grant_executor(&entries, Id::new(100), Id::new(10)),
            None
        );
    }

    #[test]
    fn unban_custom_id_format() {
        assert_eq!(unban_custom_id(Id::new(123)), "uhp_unban:123");
    }

    #[test]
    fn parse_unban_custom_id_roundtrips() {
        let id = Id::new(123456789012345678);
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
    fn parsers_reject_a_zero_snowflake() {
        assert_eq!(parse_unban_custom_id("uhp_unban:0"), None);
        assert_eq!(parse_ban_custom_id("uhp_ban:0"), None);
        assert_eq!(parse_unban_confirm_custom_id("uhp_unban_do:0:1"), None);
        assert_eq!(parse_unban_confirm_custom_id("uhp_unban_do:1:0"), None);
        assert_eq!(parse_ban_confirm_custom_id("uhp_ban_do:0:1"), None);
        assert_eq!(parse_ban_confirm_custom_id("uhp_ban_do:1:0"), None);
    }

    #[test]
    fn ban_custom_id_format() {
        assert_eq!(ban_custom_id(Id::new(123)), "uhp_ban:123");
    }

    #[test]
    fn parse_ban_custom_id_roundtrips() {
        let id = Id::new(123456789012345678);
        assert_eq!(parse_ban_custom_id(&ban_custom_id(id)), Some(id));
    }

    #[test]
    fn ban_and_unban_custom_ids_do_not_collide() {
        let id = Id::new(123456789012345678);
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
        let user = Id::new(123456789012345678);
        let message = Id::new(987654321098765432);
        assert_eq!(
            parse_unban_confirm_custom_id(&unban_confirm_custom_id(user, message)),
            Some((user, message))
        );
    }

    #[test]
    fn parse_ban_confirm_custom_id_roundtrips() {
        let user = Id::new(123456789012345678);
        let message = Id::new(987654321098765432);
        assert_eq!(
            parse_ban_confirm_custom_id(&ban_confirm_custom_id(user, message)),
            Some((user, message))
        );
    }

    #[test]
    fn confirm_parsers_reject_non_matching() {
        assert_eq!(parse_unban_confirm_custom_id("uhp_unban_do:123"), None);
        assert_eq!(parse_unban_confirm_custom_id("uhp_unban_do:123:x"), None);
        assert_eq!(parse_ban_confirm_custom_id("uhp_ban_do"), None);
        assert_eq!(parse_ban_confirm_custom_id("uhp_ban_do:"), None);
    }

    #[test]
    fn plain_and_confirm_custom_ids_do_not_collide() {
        let user = Id::new(123456789012345678);
        let message = Id::new(987654321098765432);
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
        let role = BanTrigger::Role(Id::new(42));
        assert_eq!(role.kind(), "role");
        assert_eq!(role.detail(), "<@&42>");

        let channel = BanTrigger::Channel {
            channel_id: Id::new(84),
            content: None,
        };
        assert_eq!(channel.kind(), "channel");
        assert_eq!(channel.detail(), "<#84>");
    }

    #[test]
    fn message_content_only_for_channel_trigger() {
        assert_eq!(BanTrigger::Role(Id::new(1)).message_content(), None);
        assert_eq!(
            BanTrigger::Channel {
                channel_id: Id::new(1),
                content: None,
            }
            .message_content(),
            None
        );
        assert_eq!(
            BanTrigger::Channel {
                channel_id: Id::new(1),
                content: Some("spam".to_string()),
            }
            .message_content(),
            Some("spam")
        );
    }

    #[test]
    fn message_field_value_shown_only_for_non_empty_channel_content() {
        assert_eq!(message_field_value(&BanTrigger::Role(Id::new(1))), None);
        assert_eq!(
            message_field_value(&BanTrigger::Channel {
                channel_id: Id::new(1),
                content: None,
            }),
            None
        );
        assert_eq!(
            message_field_value(&BanTrigger::Channel {
                channel_id: Id::new(1),
                content: Some(String::new()),
            }),
            None
        );
        assert_eq!(
            message_field_value(&BanTrigger::Channel {
                channel_id: Id::new(1),
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
        let guild = Id::new(9_000_000_000_000_001);
        let user = Id::new(9_000_000_000_000_002);

        assert!(claim_ban(guild, user), "first claim should succeed");
        assert!(!claim_ban(guild, user), "second claim should be skipped");

        forget_ban(guild, user);
        assert!(claim_ban(guild, user), "claim allowed again after forget");

        forget_ban(guild, user);
    }

    #[test]
    fn grant_notice_claim_is_independent_of_ban_claim() {
        // Unique ids keep this test independent of the shared global sets.
        let guild = Id::new(9_000_000_000_000_101);
        let user = Id::new(9_000_000_000_000_102);

        // A posted grant notice dedupes itself on repeat triggers...
        assert!(claim_grant_notice(guild, user), "first notice should post");
        assert!(
            !claim_grant_notice(guild, user),
            "duplicate notice should be skipped"
        );

        // ...but must NOT mark the member as handled, so a later genuine trigger
        // can still ban them.
        assert!(
            claim_ban(guild, user),
            "grant notice must not suppress a later ban"
        );

        forget_ban(guild, user);
        forget_grant_notice(guild, user);
        assert!(
            claim_grant_notice(guild, user),
            "notice allowed again after forget"
        );
        forget_grant_notice(guild, user);
    }

    #[test]
    fn target_tag_backticks_are_neutralized() {
        let user = fixtures::user(1, "ev`il");
        assert!(!target_field(&user, en()).contains("ev`il"));
    }

    #[test]
    fn user_tag_omits_zero_discriminator() {
        assert_eq!(user_tag(&fixtures::user(1, "pomelo")), "pomelo");
    }

    #[test]
    fn user_tag_keeps_legacy_discriminator() {
        let mut user = fixtures::user(1, "legacy");
        user.discriminator = 7;
        assert_eq!(user_tag(&user), "legacy#0007");
    }

    /// A UTC timestamp from a UNIX seconds value.
    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_secs(secs).expect("valid timestamp")
    }

    /// A user whose account creation date is `secs` (derived from the snowflake
    /// id, as [`created_at`] reads it).
    fn user_created_at(secs: i64) -> User {
        const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
        let millis = (secs as u64) * 1000;
        fixtures::user((millis - DISCORD_EPOCH_MS) << 22, "user")
    }

    #[test]
    fn created_at_decodes_snowflake() {
        assert_eq!(
            created_at(&user_created_at(1_700_000_000)),
            ts(1_700_000_000)
        );
    }

    #[test]
    fn target_field_includes_sanitized_display_name() {
        let mut user = fixtures::user(1, "spammer");
        user.global_name = Some("dis`play".to_string());
        let field = target_field(&user, en());
        assert!(field.contains("Display: `dis'play`"));
        assert!(!field.contains("dis`play"));
    }

    #[test]
    fn target_field_omits_display_when_absent() {
        let user = fixtures::user(1, "spammer");
        assert!(!target_field(&user, en()).contains("Display:"));
    }

    #[test]
    fn timestamp_field_renders_absolute_and_relative() {
        assert_eq!(
            timestamp_field(ts(1_700_000_000)),
            "<t:1700000000:F> (<t:1700000000:R>)"
        );
    }

    #[test]
    fn is_new_account_flags_only_recent_creation() {
        let now = ts(1_700_000_000);
        assert!(is_new_account(ts(1_700_000_000 - 3600), now));
        assert!(!is_new_account(
            ts(1_700_000_000 - 30 * SECONDS_PER_DAY),
            now
        ));
    }

    #[test]
    fn created_field_is_pure_timestamp() {
        let recent = user_created_at(1_700_000_000 - 3600);
        assert_eq!(created_field(&recent), timestamp_field(created_at(&recent)));
        assert!(!created_field(&recent).contains("New account"));
        assert!(!created_field(&recent).contains('⚠'));
    }

    #[test]
    fn account_type_field_reports_bot_and_system() {
        let mut user = fixtures::user(1, "bot");
        user.bot = true;
        user.system = Some(false);
        assert_eq!(account_type_field(&user, en()), "Bot: Yes\nSystem: No");
    }

    #[test]
    fn account_type_field_localizes_to_japanese() {
        let mut user = fixtures::user(1, "bot");
        user.bot = true;
        user.system = Some(false);
        assert_eq!(
            account_type_field(&user, Language::Ja.messages()),
            "Bot: はい\nシステム: いいえ"
        );
    }

    #[test]
    fn joined_field_handles_known_and_unknown() {
        let known = OffenderContext {
            joined_at: Some(ts(1_700_000_000)),
            unusual_dm_activity_until: None,
            spammer: false,
        };
        assert_eq!(
            joined_field(&known, en()),
            timestamp_field(ts(1_700_000_000))
        );

        let unknown = OffenderContext {
            joined_at: None,
            unusual_dm_activity_until: None,
            spammer: false,
        };
        assert_eq!(joined_field(&unknown, en()), "Unknown");
    }

    /// A user with a custom avatar and no flags, created long before `now` — so
    /// it trips none of the warning signals on its own.
    fn clean_user(now: Timestamp) -> User {
        let mut user = user_created_at(now.as_secs() - 60 * SECONDS_PER_DAY);
        user.avatar =
            Some(ImageHash::parse(b"0123456789abcdef0123456789abcdef").expect("valid image hash"));
        user
    }

    #[test]
    fn warnings_field_absent_when_no_signals() {
        let now = ts(1_700_000_000);
        let user = clean_user(now);
        let context = OffenderContext {
            joined_at: Some(now),
            unusual_dm_activity_until: None,
            spammer: false,
        };
        assert_eq!(warnings_field(&user, &context, now, en()), None);
    }

    #[test]
    fn warnings_field_lists_each_active_signal() {
        let now = ts(1_700_000_000);
        let user = user_created_at(now.as_secs() - 3600);
        let context = OffenderContext {
            joined_at: Some(now),
            unusual_dm_activity_until: Some(ts(now.as_secs() + 3600)),
            spammer: true,
        };
        let field = warnings_field(&user, &context, now, en()).expect("signals present");

        let lines: Vec<&str> = field.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("New account"));
        assert!(lines[1].contains("custom avatar"));
        assert!(lines[2].contains("likely spammer"));
        assert!(lines[3].contains("unusual DM activity"));
        assert!(lines.iter().all(|line| line.contains('⚠')));
    }

    #[test]
    fn warnings_field_omits_expired_dm_flag() {
        let now = ts(1_700_000_000);
        let user = clean_user(now);
        let context = OffenderContext {
            joined_at: Some(now),
            unusual_dm_activity_until: Some(ts(now.as_secs() - 3600)),
            spammer: false,
        };
        assert_eq!(warnings_field(&user, &context, now, en()), None);
    }

    #[test]
    fn warnings_field_localizes_to_japanese() {
        let now = ts(1_700_000_000);
        let user = user_created_at(now.as_secs() - 3600);
        let context = OffenderContext {
            joined_at: Some(now),
            unusual_dm_activity_until: None,
            spammer: false,
        };
        let field =
            warnings_field(&user, &context, now, Language::Ja.messages()).expect("signals present");
        assert!(field.contains("新規アカウント"));
    }

    /// The `(custom_id, label, style)` of each button in an action row.
    fn row_buttons(row: &Component) -> Vec<(Option<&str>, Option<&str>, ButtonStyle)> {
        let Component::ActionRow(row) = row else {
            panic!("expected an action row, got {row:?}");
        };
        row.components
            .iter()
            .map(|component| match component {
                Component::Button(button) => (
                    button.custom_id.as_deref(),
                    button.label.as_deref(),
                    button.style,
                ),
                other => panic!("expected a button, got {other:?}"),
            })
            .collect()
    }

    #[test]
    fn unban_action_row_carries_danger_button_with_unban_id() {
        let row = unban_action_row(Id::new(123), Language::En);
        assert_eq!(
            row_buttons(&row),
            vec![(
                Some("uhp_unban:123"),
                Some(en().btn_unban),
                ButtonStyle::Danger
            )]
        );
    }

    #[test]
    fn confirm_rows_carry_confirm_and_cancel() {
        let msg = en();
        assert_eq!(
            row_buttons(&confirm_unban_action_row(
                Id::new(1),
                Id::new(2),
                Language::En
            )),
            vec![
                (
                    Some("uhp_unban_do:1:2"),
                    Some(msg.btn_confirm_unban),
                    ButtonStyle::Danger
                ),
                (
                    Some(CANCEL_CUSTOM_ID),
                    Some(msg.btn_cancel),
                    ButtonStyle::Secondary
                ),
            ]
        );
        assert_eq!(
            row_buttons(&confirm_ban_action_row(
                Id::new(1),
                Id::new(2),
                Language::En
            )),
            vec![
                (
                    Some("uhp_ban_do:1:2"),
                    Some(msg.btn_confirm_ban),
                    ButtonStyle::Danger
                ),
                (
                    Some(CANCEL_CUSTOM_ID),
                    Some(msg.btn_cancel),
                    ButtonStyle::Secondary
                ),
            ]
        );
    }

    /// `(name, inline)` of each embed field, in order.
    fn field_layout(embed: &Embed) -> Vec<(&str, bool)> {
        embed
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.inline))
            .collect()
    }

    #[test]
    fn build_ban_embed_has_expected_title_colour_and_fields() {
        let msg = en();
        let offender = OffenderContext {
            joined_at: None,
            unusual_dm_activity_until: None,
            spammer: false,
        };
        // The fixture user has no avatar, so the warnings field is present.
        let embed = build_ban_embed(
            &fixtures::user(1, "spammer"),
            &BanTrigger::Role(Id::new(42)),
            &offender,
            Language::En,
        );
        assert_eq!(embed.title.as_deref(), Some(msg.ban_title));
        assert_eq!(embed.color, Some(COLOUR_RED));
        assert_eq!(embed.description, None);
        assert!(embed.timestamp.is_some());
        assert_eq!(
            field_layout(&embed),
            vec![
                (msg.field_user, false),
                (msg.field_trigger, true),
                (msg.field_account, true),
                (msg.field_account_created, false),
                (msg.field_joined, false),
                (msg.field_warnings, false),
            ]
        );
        assert_eq!(
            embed.fields[1].value,
            format!("{} <@&42>", msg.trigger_role)
        );
    }

    #[test]
    fn build_review_embed_prepends_message_body_to_description() {
        let msg = en();
        let offender = OffenderContext {
            joined_at: None,
            unusual_dm_activity_until: None,
            spammer: false,
        };
        let embed = build_pending_embed(
            &fixtures::user(1, "bot"),
            &BanTrigger::Channel {
                channel_id: Id::new(84),
                content: Some("spam".to_string()),
            },
            &offender,
            Language::En,
        );
        assert_eq!(embed.title.as_deref(), Some(msg.pending_title));
        assert_eq!(embed.color, Some(COLOUR_GOLD));
        assert_eq!(
            embed.description,
            Some(format!("{}\n\n```\nspam\n```", msg.pending_desc))
        );
    }

    #[test]
    fn mark_dry_run_is_noop_when_disabled() {
        let embed = mark_dry_run(fixtures::embed(), en(), false);
        assert_eq!(embed, fixtures::embed());
    }

    #[test]
    fn mark_dry_run_adds_localized_footer_when_enabled() {
        let embed = mark_dry_run(fixtures::embed(), en(), true);
        assert_eq!(
            embed.footer.map(|footer| footer.text),
            Some(en().dry_run_footer.to_string())
        );
    }
}
