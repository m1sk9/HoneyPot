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
    CreateMessage, GuildId, Mentionable, RoleId, Timestamp, User, UserId,
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
/// the button handler (separate issue) parses the suffix as a [`UserId`].
pub const UNBAN_CUSTOM_ID_PREFIX: &str = "uhp_unban";

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

/// Which honeypot fired, carried into the log embed.
pub enum BanTrigger {
    /// The offender acquired this honeypot role.
    Role(RoleId),
    /// The offender posted in this honeypot channel.
    Channel(ChannelId),
}

impl BanTrigger {
    /// Human-readable trigger kind for the embed field.
    fn kind(&self) -> &'static str {
        match self {
            BanTrigger::Role(_) => "role",
            BanTrigger::Channel(_) => "channel",
        }
    }

    /// Mention of the specific role/channel that fired.
    fn detail(&self) -> String {
        match self {
            BanTrigger::Role(id) => id.mention().to_string(),
            BanTrigger::Channel(id) => id.mention().to_string(),
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

/// Builds the ban notification embed.
fn build_ban_embed(target: &User, trigger: &BanTrigger) -> CreateEmbed {
    CreateEmbed::new()
        .title("🍯 Honeypot triggered — user banned")
        .color(Colour::RED)
        .field("User", target_field(target), false)
        .field(
            "Trigger",
            format!("{} {}", trigger.kind(), trigger.detail()),
            true,
        )
        .field("Bot", if target.bot { "Yes" } else { "No" }, true)
        .timestamp(Timestamp::now())
}

/// Builds the ban notification message: the embed plus an `Unban` button.
fn build_ban_message(target: &User, trigger: &BanTrigger) -> CreateMessage {
    let button = CreateButton::new(unban_custom_id(target.id))
        .style(ButtonStyle::Danger)
        .label("Unban");
    let row = CreateActionRow::Buttons(vec![button]);
    CreateMessage::new()
        .embed(build_ban_embed(target, trigger))
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
    if let Err(error) = guild_id
        .ban_with_reason(&ctx.http, target.id, DELETE_MESSAGE_DAYS, &reason)
        .await
    {
        forget_ban(guild_id, target.id);
        return Err(error.into());
    }

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
    fn ban_trigger_kind_and_detail() {
        let role = BanTrigger::Role(RoleId::new(42));
        assert_eq!(role.kind(), "role");
        assert_eq!(role.detail(), "<@&42>");

        let channel = BanTrigger::Channel(ChannelId::new(84));
        assert_eq!(channel.kind(), "channel");
        assert_eq!(channel.detail(), "<#84>");
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
