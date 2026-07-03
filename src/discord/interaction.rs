//! Component interaction handling.
//!
//! Handles the buttons attached to honeypot log embeds (see
//! [`crate::discord::ban`]): the `Unban` button on a ban notice, and the `Ban`
//! button on an untrusted-bot notice. Both require the clicker to hold the
//! `BAN_MEMBERS` permission; unauthorized clicks are rejected with an ephemeral
//! message. A successful unban releases the offender's claim (via
//! [`ban::forget_ban`]) so they can be re-banned if they trip a honeypot again.
//! Each action rewrites the log embed to record who took it.

use crate::discord::ban;
use serenity::all::{
    Colour, ComponentInteraction, Context, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, Mentionable, Permissions, UserId,
};

/// Handles a component interaction, dispatching the honeypot log buttons.
///
/// Components other than the `Unban`/`Ban` buttons are ignored. Any Discord
/// error while responding is logged and swallowed, matching the fire-and-forget
/// style of the gateway event handlers.
pub async fn handle_component(ctx: &Context, component: &ComponentInteraction) {
    if let Some(target_id) = ban::parse_unban_custom_id(&component.data.custom_id) {
        handle_unban(ctx, component, target_id).await;
    } else if let Some(target_id) = ban::parse_ban_custom_id(&component.data.custom_id) {
        handle_manual_ban(ctx, component, target_id).await;
    }
}

/// Lifts the ban on `target_id` in response to the `Unban` button.
async fn handle_unban(ctx: &Context, component: &ComponentInteraction, target_id: UserId) {
    let Some(guild_id) = component.guild_id else {
        return;
    };

    if !has_ban_permission(component) {
        respond_ephemeral(
            ctx,
            component,
            "You need the Ban Members permission to unban.",
        )
        .await;
        return;
    }

    if let Err(error) = guild_id.unban(&ctx.http, target_id).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "failed to unban member from unban button"
        );
        respond_ephemeral(
            ctx,
            component,
            "Failed to unban the user. Please try again.",
        )
        .await;
        return;
    }

    // Allow the offender to be banned again if they re-trip a honeypot.
    ban::forget_ban(guild_id, target_id);

    tracing::info!(
        guild_id = %guild_id,
        user_id = %target_id,
        moderator_id = %component.user.id,
        "unbanned member via unban button"
    );

    let embed = resolved_embed(component, target_id);
    let response = CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(vec![]),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "unbanned user but failed to update log embed"
        );
    }
}

/// Bans `target_id` in response to the `Ban` button on an untrusted-bot notice.
///
/// Swaps the pending notice's `Ban` button for an `Unban` button so a mistaken
/// confirmation can be reversed, matching the auto-ban log embeds.
async fn handle_manual_ban(ctx: &Context, component: &ComponentInteraction, target_id: UserId) {
    let Some(guild_id) = component.guild_id else {
        return;
    };

    if !has_ban_permission(component) {
        respond_ephemeral(
            ctx,
            component,
            "You need the Ban Members permission to ban.",
        )
        .await;
        return;
    }

    if let Err(error) = ban::confirm_bot_ban(ctx, guild_id, target_id).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "failed to ban bot from manual ban button"
        );
        respond_ephemeral(ctx, component, "Failed to ban the bot. Please try again.").await;
        return;
    }

    tracing::info!(
        guild_id = %guild_id,
        user_id = %target_id,
        moderator_id = %component.user.id,
        "banned bot via manual ban button"
    );

    let embed = manually_banned_embed(component, target_id);
    let response = CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new()
            .embed(embed)
            .components(vec![ban::unban_action_row(target_id)]),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "banned bot but failed to update log embed"
        );
    }
}

/// Whether the clicking member holds the `BAN_MEMBERS` permission.
///
/// Discord populates `member.permissions` with the interacting member's
/// channel-computed permissions, so no cache lookup is needed.
fn has_ban_permission(component: &ComponentInteraction) -> bool {
    component
        .member
        .as_ref()
        .and_then(|member| member.permissions)
        .is_some_and(Permissions::ban_members)
}

/// Builds the "ban lifted" embed shown after a successful unban.
///
/// Reuses the original log embed (preserving the trigger details) when present,
/// recoloring it, retitling it, and appending who lifted the ban. Falls back to
/// a minimal embed if the original message carried none.
fn resolved_embed(component: &ComponentInteraction, target_id: UserId) -> CreateEmbed {
    let base = component
        .message
        .embeds
        .first()
        .cloned()
        .map(CreateEmbed::from)
        .unwrap_or_else(|| {
            CreateEmbed::new().field("User", target_id.mention().to_string(), false)
        });

    base.title("🍯 Honeypot ban lifted")
        .color(Colour::DARK_GREEN)
        .field("Unbanned by", component.user.mention().to_string(), false)
}

/// Builds the "bot banned" embed shown after a manual ban confirmation.
///
/// Reuses the pending notice embed (preserving the trigger details) when
/// present, recoloring it, retitling it, replacing the pending description, and
/// appending who confirmed the ban.
fn manually_banned_embed(component: &ComponentInteraction, target_id: UserId) -> CreateEmbed {
    let base = component
        .message
        .embeds
        .first()
        .cloned()
        .map(CreateEmbed::from)
        .unwrap_or_else(|| {
            CreateEmbed::new().field("User", target_id.mention().to_string(), false)
        });

    base.title("🍯 Honeypot triggered — bot banned")
        .description("Banned after manual review.")
        .color(Colour::RED)
        .field("Banned by", component.user.mention().to_string(), false)
}

/// Sends an ephemeral text response, logging any failure.
async fn respond_ephemeral(ctx: &Context, component: &ComponentInteraction, content: &str) {
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .ephemeral(true)
            .content(content),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(%error, "failed to send ephemeral interaction response");
    }
}
