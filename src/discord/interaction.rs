//! Component interaction handling.
//!
//! Handles the buttons attached to honeypot log embeds (see
//! [`crate::discord::ban`]): the `Unban` button on a ban notice, and the `Ban`
//! button on an untrusted-bot notice. Both are guarded by a two-step
//! confirmation — clicking one opens an ephemeral prompt with `Confirm`/`Cancel`
//! buttons, and only the `Confirm` click actually bans or unbans, so a stray
//! click cannot flip a member's ban state.
//!
//! Every step requires the clicker to hold the `BAN_MEMBERS` permission;
//! unauthorized clicks are rejected with an ephemeral message. A successful
//! unban releases the offender's claim (via [`ban::forget_ban`]) so they can be
//! re-banned if they trip a honeypot again. The confirming action rewrites the
//! original log embed to record who took it.

use crate::discord::ban;
use crate::i18n::{Language, Messages};
use crate::settings::HoneyPotConfig;
use serenity::all::{
    ChannelId, Colour, ComponentInteraction, Context, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, EditMessage, Embed, GuildId, Mentionable, MessageId,
    Permissions, User, UserId,
};

/// Resolves the display language for `guild_id` from the global config, falling
/// back to the default when the guild isn't configured.
fn language_for(guild_id: GuildId) -> Language {
    HoneyPotConfig::get()
        .guild(guild_id)
        .map(|guild| guild.language)
        .unwrap_or_default()
}

/// Handles a component interaction, dispatching the honeypot log buttons.
///
/// Components other than the honeypot buttons are ignored. Any Discord error
/// while responding is logged and swallowed, matching the fire-and-forget style
/// of the gateway event handlers.
///
/// The plain-button ids (`uhp_unban:` / `uhp_ban:`) and the confirmation ids
/// (`uhp_unban_do:` / `uhp_ban_do:`) never overlap (see [`ban`]), so the parse
/// order here is not load-bearing.
pub async fn handle_component(ctx: &Context, component: &ComponentInteraction) {
    let custom_id = component.data.custom_id.as_str();
    if let Some(target_id) = ban::parse_unban_custom_id(custom_id) {
        prompt_unban(ctx, component, target_id).await;
    } else if let Some(target_id) = ban::parse_ban_custom_id(custom_id) {
        prompt_ban(ctx, component, target_id).await;
    } else if let Some((target_id, message_id)) = ban::parse_unban_confirm_custom_id(custom_id) {
        perform_unban(ctx, component, target_id, message_id).await;
    } else if let Some((target_id, message_id)) = ban::parse_ban_confirm_custom_id(custom_id) {
        perform_ban(ctx, component, target_id, message_id).await;
    } else if custom_id == ban::CANCEL_CUSTOM_ID {
        handle_cancel(ctx, component).await;
    }
}

/// Opens the ephemeral "confirm unban" prompt in response to the `Unban` button.
///
/// Nothing is unbanned here; the actual unban waits for the `Confirm` click
/// (see [`perform_unban`]). The permission is checked up front so an
/// unauthorized user never even sees the prompt. The current message id is
/// carried into the confirm button so [`perform_unban`] can edit this log embed.
async fn prompt_unban(ctx: &Context, component: &ComponentInteraction, target_id: UserId) {
    let Some(guild_id) = component.guild_id else {
        return;
    };
    let language = language_for(guild_id);
    let msg = language.messages();

    if !has_ban_permission(component) {
        respond_ephemeral(ctx, component, msg.perm_needed_unban).await;
        return;
    }

    let row = ban::confirm_unban_action_row(target_id, component.message.id, language);
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .ephemeral(true)
            .content(
                msg.confirm_unban_prompt
                    .replace("{}", &target_id.mention().to_string()),
            )
            .components(vec![row]),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(%error, user_id = %target_id, "failed to open unban confirmation");
    }
}

/// Opens the ephemeral "confirm ban" prompt in response to the `Ban` button on
/// an untrusted-bot notice. The actual ban waits for the `Confirm` click (see
/// [`perform_ban`]).
async fn prompt_ban(ctx: &Context, component: &ComponentInteraction, target_id: UserId) {
    let Some(guild_id) = component.guild_id else {
        return;
    };
    let language = language_for(guild_id);
    let msg = language.messages();

    if !has_ban_permission(component) {
        respond_ephemeral(ctx, component, msg.perm_needed_ban).await;
        return;
    }

    let row = ban::confirm_ban_action_row(target_id, component.message.id, language);
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .ephemeral(true)
            .content(
                msg.confirm_ban_prompt
                    .replace("{}", &target_id.mention().to_string()),
            )
            .components(vec![row]),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(%error, user_id = %target_id, "failed to open ban confirmation");
    }
}

/// Lifts the ban on `target_id` after the moderator confirms, then rewrites the
/// original log message (`message_id`) to record the unban.
///
/// The interaction is acknowledged (the ephemeral prompt is replaced with a
/// short result) right after the unban HTTP call, before the best-effort log
/// edit, to stay inside Discord's interaction response window.
async fn perform_unban(
    ctx: &Context,
    component: &ComponentInteraction,
    target_id: UserId,
    message_id: MessageId,
) {
    let Some(guild_id) = component.guild_id else {
        return;
    };
    let language = language_for(guild_id);
    let msg = language.messages();

    if !has_ban_permission(component) {
        respond_ephemeral(ctx, component, msg.perm_needed_unban).await;
        return;
    }

    if crate::settings::dry_run() {
        tracing::warn!(
            guild_id = %guild_id,
            user_id = %target_id,
            "dry-run: skipping unban"
        );
    } else if let Err(error) = guild_id.unban(&ctx.http, target_id).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "failed to unban member from unban button"
        );
        respond_ephemeral(ctx, component, msg.unban_failed).await;
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

    ack_confirmation(
        ctx,
        component,
        &msg.unbanned_ack
            .replace("{}", &target_id.mention().to_string()),
    )
    .await;

    let base = fetch_base_embed(ctx, component.channel_id, message_id).await;
    let embed = resolved_embed(base, target_id, &component.user, language);
    edit_log_message(
        ctx,
        component.channel_id,
        message_id,
        embed,
        vec![],
        target_id,
    )
    .await;
}

/// Bans `target_id` after the moderator confirms an untrusted-bot notice, then
/// rewrites the notice (`message_id`) to record the ban.
///
/// The rewritten embed carries an `Unban` button so a mistaken confirmation can
/// be reversed, matching the auto-ban log embeds.
async fn perform_ban(
    ctx: &Context,
    component: &ComponentInteraction,
    target_id: UserId,
    message_id: MessageId,
) {
    let Some(guild_id) = component.guild_id else {
        return;
    };
    let language = language_for(guild_id);
    let msg = language.messages();

    if !has_ban_permission(component) {
        respond_ephemeral(ctx, component, msg.perm_needed_ban).await;
        return;
    }

    if let Err(error) = ban::confirm_bot_ban(ctx, guild_id, target_id).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "failed to ban bot from manual ban button"
        );
        respond_ephemeral(ctx, component, msg.ban_failed).await;
        return;
    }

    tracing::info!(
        guild_id = %guild_id,
        user_id = %target_id,
        moderator_id = %component.user.id,
        "banned bot via manual ban button"
    );

    ack_confirmation(
        ctx,
        component,
        &msg.banned_ack
            .replace("{}", &target_id.mention().to_string()),
    )
    .await;

    let base = fetch_base_embed(ctx, component.channel_id, message_id).await;
    let embed = manually_banned_embed(base, target_id, &component.user, language);
    edit_log_message(
        ctx,
        component.channel_id,
        message_id,
        embed,
        vec![ban::unban_action_row(target_id, language)],
        target_id,
    )
    .await;
}

/// Dismisses the ephemeral confirmation prompt without acting, in response to
/// the `Cancel` button.
async fn handle_cancel(ctx: &Context, component: &ComponentInteraction) {
    let msg = component
        .guild_id
        .map(language_for)
        .unwrap_or_default()
        .messages();
    let response = CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new()
            .content(msg.cancelled)
            .components(vec![]),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(%error, "failed to dismiss cancelled confirmation");
    }
}

/// Acknowledges a confirmation click by replacing the ephemeral prompt with a
/// short result line and dropping its buttons.
async fn ack_confirmation(ctx: &Context, component: &ComponentInteraction, content: &str) {
    let response = CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new()
            .content(content)
            .components(vec![]),
    );
    if let Err(error) = component.create_response(&ctx.http, response).await {
        tracing::error!(%error, "failed to acknowledge confirmation");
    }
}

/// Fetches the original log message's first embed as a [`CreateEmbed`] base, so
/// the rewritten embed can preserve the trigger details.
///
/// Returns `None` (rebuild from scratch) if the message can't be fetched or has
/// no embed; the confirmation prompt lives in the same channel, so `channel_id`
/// always points at the log channel.
///
/// The raw [`Embed`] is returned (not a `CreateEmbed`) so the rewrite builders
/// can inspect and drop individual fields — see [`without_warnings_field`].
async fn fetch_base_embed(
    ctx: &Context,
    channel_id: ChannelId,
    message_id: MessageId,
) -> Option<Embed> {
    match channel_id.message(&ctx.http, message_id).await {
        Ok(message) => message.embeds.into_iter().next(),
        Err(error) => {
            tracing::warn!(
                %error,
                %message_id,
                "failed to fetch original log message; rebuilding embed without trigger detail"
            );
            None
        }
    }
}

/// Drops the aggregated "Warnings" field from a reused log embed.
///
/// The lifted-ban embed omits it — the risk signals were decision aids for the
/// ban, not for the resolved state — while the manual-ban rewrite keeps them.
fn without_warnings_field(mut embed: Embed, msg: &Messages) -> Embed {
    embed
        .fields
        .retain(|field| field.name != msg.field_warnings);
    embed
}

/// Edits the original log message in place with a new embed and button set.
async fn edit_log_message(
    ctx: &Context,
    channel_id: ChannelId,
    message_id: MessageId,
    embed: CreateEmbed,
    components: Vec<serenity::all::CreateActionRow>,
    target_id: UserId,
) {
    let edit = EditMessage::new().embed(embed).components(components);
    if let Err(error) = channel_id.edit_message(&ctx.http, message_id, edit).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "acted on member but failed to update log embed"
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
/// Reuses the original log embed (`base`, preserving the trigger details) when
/// present, recoloring it, retitling it, and appending who lifted the ban. The
/// aggregated "Warnings" field is dropped — the risk signals justified the ban,
/// not the resolved state. Falls back to a minimal embed if the message carried none.
pub(crate) fn resolved_embed(
    base: Option<Embed>,
    target_id: UserId,
    moderator: &User,
    language: Language,
) -> CreateEmbed {
    let msg = language.messages();
    let base = base
        .map(|embed| CreateEmbed::from(without_warnings_field(embed, msg)))
        .unwrap_or_else(|| {
            CreateEmbed::new().field(msg.field_user, target_id.mention().to_string(), false)
        });

    let embed = base
        .title(msg.lifted_title)
        .color(Colour::DARK_GREEN)
        .field(
            msg.field_unbanned_by,
            moderator.mention().to_string(),
            false,
        );
    ban::apply_dry_run_marker(embed, msg)
}

/// Builds the "bot banned" embed shown after a manual ban confirmation.
///
/// Reuses the pending notice embed (`base`, preserving the trigger details) when
/// present, recoloring it, retitling it, replacing the pending description, and
/// appending who confirmed the ban.
pub(crate) fn manually_banned_embed(
    base: Option<Embed>,
    target_id: UserId,
    moderator: &User,
    language: Language,
) -> CreateEmbed {
    let msg = language.messages();
    let base = base.map(CreateEmbed::from).unwrap_or_else(|| {
        CreateEmbed::new().field(msg.field_user, target_id.mention().to_string(), false)
    });

    let embed = base
        .title(msg.bot_banned_title)
        .description(msg.bot_banned_desc)
        .color(Colour::RED)
        .field(msg.field_banned_by, moderator.mention().to_string(), false);
    ban::apply_dry_run_marker(embed, msg)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serenity::all::EmbedField;

    /// Builds an embed carrying the given field names (values are placeholders).
    fn embed_with_fields(names: &[&str]) -> Embed {
        let mut embed = Embed::default();
        embed.fields = names
            .iter()
            .map(|name| EmbedField::new(name.to_string(), "value", false))
            .collect();
        embed
    }

    #[test]
    fn without_warnings_field_drops_only_the_warnings_field() {
        let msg = Language::En.messages();
        let embed = embed_with_fields(&[msg.field_user, msg.field_warnings, msg.field_trigger]);
        let stripped = without_warnings_field(embed, msg);
        let names: Vec<&str> = stripped.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec![msg.field_user, msg.field_trigger]);
    }

    #[test]
    fn without_warnings_field_is_noop_when_absent() {
        let msg = Language::En.messages();
        let embed = embed_with_fields(&[msg.field_user, msg.field_trigger]);
        let stripped = without_warnings_field(embed, msg);
        assert_eq!(stripped.fields.len(), 2);
    }

    #[test]
    fn without_warnings_field_matches_the_active_language() {
        // A JA warnings field must not be stripped when the EN label is used, and
        // vice versa — the label is language-specific.
        let ja = Language::Ja.messages();
        let embed = embed_with_fields(&[ja.field_warnings]);
        assert_eq!(without_warnings_field(embed, ja).fields.len(), 0);

        let embed = embed_with_fields(&[ja.field_warnings]);
        let en = Language::En.messages();
        assert_eq!(without_warnings_field(embed, en).fields.len(), 1);
    }
}
