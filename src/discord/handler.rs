//! Discord gateway event dispatch.
//!
//! Handles the two honeypot triggers — acquiring a honeypot role
//! (`GUILD_MEMBER_UPDATE`) and posting in a honeypot channel (`MESSAGE_CREATE`)
//! — by looking up the guild's configuration and dispatching via
//! [`act_on_trigger`]. The bot's own account is captured on `READY` and never
//! banned.
//!
//! A non-bot offender is banned immediately. A bot is treated more cautiously:
//! trusted bots are ignored, and any other bot is flagged for manual review
//! rather than auto-banned, so well-behaved bots that legitimately post into a
//! honeypot channel are not caught.
//!
//! twilight keeps no cache, so a `GUILD_MEMBER_UPDATE` never carries the
//! member's previous roles: role detection always takes the intersection path
//! of [`newly_acquired_honeypot_role`]. Each event's raw JSON is passed along so
//! [`crate::discord::raw`] can recover the account signals twilight-model drops.

use crate::discord::ban::{
    self, BanTrigger, OffenderContext, is_honeypot_channel, newly_acquired_honeypot_role,
};
use crate::discord::{commands, interaction, raw};
use crate::settings::{GuildConfig, HoneyPotConfig};
use std::sync::OnceLock;
use twilight_gateway::{Event, EventTypeFlags};
use twilight_http::Client;
use twilight_model::application::interaction::InteractionType;
use twilight_model::channel::Message;
use twilight_model::gateway::payload::incoming::{MemberUpdate, Ready};
use twilight_model::id::{
    Id,
    marker::{GuildMarker, UserMarker},
};
use twilight_model::user::User;

/// The gateway events HoneyPot acts on; every other dispatch is skipped before
/// it is deserialized.
pub const WANTED_EVENTS: EventTypeFlags = EventTypeFlags::READY
    .union(EventTypeFlags::MEMBER_UPDATE)
    .union(EventTypeFlags::MESSAGE_CREATE)
    .union(EventTypeFlags::INTERACTION_CREATE);

/// The bot's own user ID, captured on `READY` to guard against self-bans.
static BOT_USER_ID: OnceLock<Id<UserMarker>> = OnceLock::new();

/// Whether `id` is this bot's own account.
///
/// Returns `false` before `READY` has populated [`BOT_USER_ID`], which is
/// harmless: no gateway events arrive before `READY`.
fn is_self(id: Id<UserMarker>) -> bool {
    BOT_USER_ID.get() == Some(&id)
}

/// This bot's own user ID, or `None` before `READY` has captured it.
///
/// Used by the `/doctor` command to locate the bot's member and compare its
/// role position against the honeypot roles.
pub(crate) fn bot_user_id() -> Option<Id<UserMarker>> {
    BOT_USER_ID.get().copied()
}

/// Dispatches one gateway event. `raw` is the event's JSON text.
pub async fn handle(http: &Client, event: Event, raw: &str) {
    match event {
        Event::Ready(ready) => on_ready(http, &ready).await,
        Event::MemberUpdate(update) => on_member_update(http, *update, raw).await,
        Event::MessageCreate(message) => on_message(http, message.0, raw).await,
        Event::InteractionCreate(event) => match event.kind {
            InteractionType::MessageComponent => interaction::handle_component(http, &event).await,
            InteractionType::ApplicationCommand => commands::dispatch(http, &event, raw).await,
            _ => {}
        },
        _ => {}
    }
}

async fn on_ready(http: &Client, ready: &Ready) {
    let _ = BOT_USER_ID.set(ready.user.id);
    tracing::info!(
        "Running v{}, {} is connected!",
        env!("CARGO_PKG_VERSION"),
        ready.user.name
    );

    commands::register(http, ready.application.id).await;
}

/// Acts on a confirmed honeypot trigger for `user` in `guild`.
///
/// A non-bot offender is banned immediately. A bot in the guild's trusted list
/// is ignored; any other bot is flagged for manual review via
/// [`ban::execute_suspicious_bot_notice`] instead of being auto-banned.
async fn act_on_trigger(
    http: &Client,
    guild_id: Id<GuildMarker>,
    guild: &GuildConfig,
    user: &User,
    trigger: BanTrigger,
    offender: &OffenderContext,
) {
    let result = if user.bot {
        if guild.trusted_bot_ids.contains(&user.id) {
            tracing::debug!(
                user_id = %user.id,
                "ignoring trusted bot that tripped honeypot"
            );
            return;
        }
        ban::execute_suspicious_bot_notice(
            http,
            guild_id,
            guild.log_channel_id,
            user,
            trigger,
            offender,
            guild.language,
        )
        .await
    } else {
        ban::execute_ban(
            http,
            guild_id,
            guild.log_channel_id,
            user,
            trigger,
            offender,
            guild.language,
        )
        .await
    };

    if let Err(error) = result {
        tracing::error!(
            %error,
            user_id = %user.id,
            "failed to handle honeypot trigger"
        );
    }
}

async fn on_member_update(http: &Client, update: MemberUpdate, raw: &str) {
    let user = update.user;

    if is_self(user.id) {
        return;
    }
    let Some(guild) = HoneyPotConfig::get().guild(update.guild_id) else {
        return;
    };
    let Some(role) = newly_acquired_honeypot_role(&guild.honeypot_role_ids, &update.roles, None)
    else {
        return;
    };

    let signals = raw::from_member_update(raw);
    let offender = OffenderContext {
        joined_at: update.joined_at,
        unusual_dm_activity_until: signals.unusual_dm_activity_until,
        spammer: signals.spammer,
    };

    // A honeypot role only marks an offender when the member self-assigned it
    // (onboarding / self-serve) — the trap's intended path. A third-party
    // grant (an admin by hand, a reaction-role bot) or an unresolvable grantor
    // is held for manual review instead of auto-banned, so a mistaken grant
    // can't cause a false ban. Only the role trigger needs this: a channel
    // post is always the offender's own act.
    match ban::resolve_role_grant_source(http, update.guild_id, user.id, role).await {
        ban::RoleGrantSource::SelfAssigned => {
            act_on_trigger(
                http,
                update.guild_id,
                guild,
                &user,
                BanTrigger::Role(role),
                &offender,
            )
            .await;
        }
        ban::RoleGrantSource::ThirdParty | ban::RoleGrantSource::Unknown => {
            // A trusted bot stays exempt even when a third party grants it a
            // honeypot role — an admin assigning a bot's role is the usual way
            // a bot acquires one, so without this the allowlist would be lost
            // on every non-self-assign path (mirrors act_on_trigger).
            if user.bot && guild.trusted_bot_ids.contains(&user.id) {
                tracing::debug!(
                    user_id = %user.id,
                    "ignoring trusted bot granted a honeypot role by a third party"
                );
                return;
            }
            if let Err(error) = ban::execute_third_party_grant_notice(
                http,
                update.guild_id,
                guild.log_channel_id,
                &user,
                BanTrigger::Role(role),
                &offender,
                guild.language,
            )
            .await
            {
                tracing::error!(
                    %error,
                    user_id = %user.id,
                    "failed to post third-party role grant notice"
                );
            }
        }
    }
}

async fn on_message(http: &Client, message: Message, raw: &str) {
    let Some(guild_id) = message.guild_id else {
        return;
    };

    let user = message.author;
    if is_self(user.id) {
        return;
    }
    let Some(guild) = HoneyPotConfig::get().guild(guild_id) else {
        return;
    };
    if !is_honeypot_channel(&guild.honeypot_channel_ids, message.channel_id) {
        return;
    }

    // The partial member Discord attaches to a guild message carries the join
    // date (and, in the raw payload, the DM-activity flag); a channel trigger
    // has no other source.
    let signals = raw::from_message(raw);
    let offender = OffenderContext {
        joined_at: message.member.as_ref().and_then(|member| member.joined_at),
        unusual_dm_activity_until: signals.unusual_dm_activity_until,
        spammer: signals.spammer,
    };
    act_on_trigger(
        http,
        guild_id,
        guild,
        &user,
        BanTrigger::Channel {
            channel_id: message.channel_id,
            content: Some(message.content),
        },
        &offender,
    )
    .await;
}
