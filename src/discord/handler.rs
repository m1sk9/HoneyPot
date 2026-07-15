//! Discord event handler.
//!
//! Handles the two honeypot triggers — acquiring a honeypot role
//! (`guild_member_update`) and posting in a honeypot channel (`message`) — by
//! looking up the guild's configuration and dispatching via [`act_on_trigger`].
//! The bot's own account is captured in `ready` and never banned.
//!
//! A non-bot offender is banned immediately. A bot is treated more cautiously:
//! trusted bots are ignored, and any other bot is flagged for manual review
//! rather than auto-banned, so well-behaved bots that legitimately post into a
//! honeypot channel are not caught.

use crate::discord::ban::{
    self, BanTrigger, OffenderContext, is_honeypot_channel, newly_acquired_honeypot_role,
};
use crate::discord::interaction;
use crate::settings::{GuildConfig, HoneyPotConfig};
use serenity::all::{
    Context, EventHandler, GuildId, GuildMemberUpdateEvent, Interaction, Member, Message, Ready,
    User, UserId,
};
use std::sync::OnceLock;

/// The bot's own user ID, captured on `ready` to guard against self-bans.
static BOT_USER_ID: OnceLock<UserId> = OnceLock::new();

/// Whether `id` is this bot's own account.
///
/// Returns `false` before `ready` has populated [`BOT_USER_ID`], which is
/// harmless: no gateway events arrive before `ready`.
fn is_self(id: UserId) -> bool {
    BOT_USER_ID.get() == Some(&id)
}

/// Acts on a confirmed honeypot trigger for `user` in `guild`.
///
/// A non-bot offender is banned immediately. A bot in the guild's trusted list
/// is ignored; any other bot is flagged for manual review via
/// [`ban::execute_suspicious_bot_notice`] instead of being auto-banned.
async fn act_on_trigger(
    ctx: &Context,
    guild_id: GuildId,
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
            ctx,
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
            ctx,
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

/// Event handler for HoneyPot.
pub struct HoneyPotEventHandler;

#[serenity::async_trait]
impl EventHandler for HoneyPotEventHandler {
    async fn ready(&self, _ctx: Context, ready: Ready) {
        let _ = BOT_USER_ID.set(ready.user.id);
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        tracing::info!("Running {}, {} is connected!", version, ready.user.name);
    }

    async fn guild_member_update(
        &self,
        ctx: Context,
        old_if_available: Option<Member>,
        _new: Option<Member>,
        event: GuildMemberUpdateEvent,
    ) {
        let user = event.user;

        if is_self(user.id) {
            return;
        }
        let Some(guild) = HoneyPotConfig::get().guild(event.guild_id) else {
            return;
        };
        let old = old_if_available
            .as_ref()
            .map(|member| member.roles.as_slice());
        let Some(role) = newly_acquired_honeypot_role(&guild.honeypot_role_ids, &event.roles, old)
        else {
            return;
        };

        let offender = OffenderContext {
            joined_at: Some(event.joined_at),
            unusual_dm_activity_until: event.unusual_dm_activity_until,
        };

        // A honeypot role only marks an offender when the member self-assigned it
        // (onboarding / self-serve) — the trap's intended path. A third-party
        // grant (an admin by hand, a reaction-role bot) or an unresolvable grantor
        // is held for manual review instead of auto-banned, so a mistaken grant
        // can't cause a false ban. Only the role trigger needs this: a channel
        // post is always the offender's own act.
        match ban::resolve_role_grant_source(&ctx, event.guild_id, user.id, role).await {
            ban::RoleGrantSource::SelfAssigned => {
                act_on_trigger(
                    &ctx,
                    event.guild_id,
                    guild,
                    &user,
                    BanTrigger::Role(role),
                    &offender,
                )
                .await;
            }
            ban::RoleGrantSource::ThirdParty | ban::RoleGrantSource::Unknown => {
                if let Err(error) = ban::execute_third_party_grant_notice(
                    &ctx,
                    event.guild_id,
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

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Component(component) = interaction {
            interaction::handle_component(&ctx, &component).await;
        }
    }

    async fn message(&self, ctx: Context, new_message: Message) {
        let Some(guild_id) = new_message.guild_id else {
            return;
        };

        let user = new_message.author;
        if is_self(user.id) {
            return;
        }
        let Some(guild) = HoneyPotConfig::get().guild(guild_id) else {
            return;
        };
        if !is_honeypot_channel(&guild.honeypot_channel_ids, new_message.channel_id) {
            return;
        }

        // The partial member Discord attaches to a guild message carries the
        // join date and DM-activity flag; a channel trigger has no other source.
        let offender = OffenderContext {
            joined_at: new_message
                .member
                .as_ref()
                .and_then(|member| member.joined_at),
            unusual_dm_activity_until: new_message
                .member
                .as_ref()
                .and_then(|member| member.unusual_dm_activity_until),
        };
        act_on_trigger(
            &ctx,
            guild_id,
            guild,
            &user,
            BanTrigger::Channel {
                channel_id: new_message.channel_id,
                content: Some(new_message.content),
            },
            &offender,
        )
        .await;
    }
}
