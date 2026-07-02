//! Discord event handler.
//!
//! Handles the two honeypot triggers — acquiring a honeypot role
//! (`guild_member_update`) and posting in a honeypot channel (`message`) — by
//! looking up the guild's configuration and delegating to [`ban::execute_ban`].
//! The bot's own account is captured in `ready` and never banned.

use crate::discord::ban::{self, BanTrigger, is_honeypot_channel, newly_acquired_honeypot_role};
use crate::discord::interaction;
use crate::settings::HoneyPotConfig;
use serenity::all::{
    Context, EventHandler, GuildMemberUpdateEvent, Interaction, Member, Message, Ready, UserId,
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
        if is_self(event.user.id) {
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

        if let Err(error) = ban::execute_ban(
            &ctx,
            event.guild_id,
            guild.log_channel_id,
            &event.user,
            BanTrigger::Role(role),
        )
        .await
        {
            tracing::error!(
                %error,
                user_id = %event.user.id,
                "failed to ban member on honeypot role"
            );
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
        if is_self(new_message.author.id) {
            return;
        }
        let Some(guild) = HoneyPotConfig::get().guild(guild_id) else {
            return;
        };
        if !is_honeypot_channel(&guild.honeypot_channel_ids, new_message.channel_id) {
            return;
        }

        if let Err(error) = ban::execute_ban(
            &ctx,
            guild_id,
            guild.log_channel_id,
            &new_message.author,
            BanTrigger::Channel(new_message.channel_id),
        )
        .await
        {
            tracing::error!(
                %error,
                user_id = %new_message.author.id,
                "failed to ban author on honeypot channel"
            );
        }
    }
}
