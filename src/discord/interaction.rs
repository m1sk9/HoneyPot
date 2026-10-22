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
use crate::discord::commands::{self, has_ban_permission, language_for};
use crate::error::HoneyPotError;
use crate::i18n::{Language, Messages};
use twilight_http::Client;
use twilight_mention::Mention;
use twilight_model::application::interaction::{Interaction, InteractionData};
use twilight_model::channel::Message;
use twilight_model::channel::message::{Component, Embed};
use twilight_model::http::interaction::{InteractionResponse, InteractionResponseType};
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, GuildMarker, MessageMarker, UserMarker},
};
use twilight_model::user::User;
use twilight_util::builder::InteractionResponseDataBuilder;
use twilight_util::builder::embed::{EmbedBuilder, EmbedFieldBuilder};

/// Handles a component interaction, dispatching the honeypot log buttons.
///
/// Components other than the honeypot buttons are ignored. Any Discord error
/// while responding is logged and swallowed, matching the fire-and-forget style
/// of the gateway event handlers.
///
/// The plain-button ids (`uhp_unban:` / `uhp_ban:`) and the confirmation ids
/// (`uhp_unban_do:` / `uhp_ban_do:`) never overlap (see [`ban`]), so the parse
/// order here is not load-bearing.
pub async fn handle_component(http: &Client, interaction: &Interaction) {
    let Some(InteractionData::MessageComponent(data)) = &interaction.data else {
        return;
    };
    let custom_id = data.custom_id.as_str();
    if let Some(target_id) = ban::parse_unban_custom_id(custom_id) {
        prompt_unban(http, interaction, target_id).await;
    } else if let Some(target_id) = ban::parse_ban_custom_id(custom_id) {
        prompt_ban(http, interaction, target_id).await;
    } else if let Some((target_id, message_id)) = ban::parse_unban_confirm_custom_id(custom_id) {
        perform_unban(http, interaction, target_id, message_id).await;
    } else if let Some((target_id, message_id)) = ban::parse_ban_confirm_custom_id(custom_id) {
        perform_ban(http, interaction, target_id, message_id).await;
    } else if custom_id == ban::CANCEL_CUSTOM_ID {
        handle_cancel(http, interaction).await;
    }
}

/// Opens the ephemeral "confirm unban" prompt in response to the `Unban` button.
///
/// Nothing is unbanned here; the actual unban waits for the `Confirm` click
/// (see [`perform_unban`]). The permission is checked up front so an
/// unauthorized user never even sees the prompt. The current message id is
/// carried into the confirm button so [`perform_unban`] can edit this log embed.
async fn prompt_unban(http: &Client, interaction: &Interaction, target_id: Id<UserMarker>) {
    let (Some(guild_id), Some(message)) = (interaction.guild_id, interaction.message.as_ref())
    else {
        return;
    };
    let language = language_for(Some(guild_id));
    let msg = language.messages();

    if !has_ban_permission(interaction) {
        respond_ephemeral(http, interaction, msg.perm_needed_unban).await;
        return;
    }

    let row = ban::confirm_unban_action_row(target_id, message.id, language);
    let content = msg
        .confirm_unban_prompt
        .replace("{}", &target_id.mention().to_string());
    if let Err(error) = create_response(http, interaction, &ephemeral(content, vec![row])).await {
        tracing::error!(%error, user_id = %target_id, "failed to open unban confirmation");
    }
}

/// Opens the ephemeral "confirm ban" prompt in response to the `Ban` button on
/// an untrusted-bot notice. The actual ban waits for the `Confirm` click (see
/// [`perform_ban`]).
async fn prompt_ban(http: &Client, interaction: &Interaction, target_id: Id<UserMarker>) {
    let (Some(guild_id), Some(message)) = (interaction.guild_id, interaction.message.as_ref())
    else {
        return;
    };
    let language = language_for(Some(guild_id));
    let msg = language.messages();

    if !has_ban_permission(interaction) {
        respond_ephemeral(http, interaction, msg.perm_needed_ban).await;
        return;
    }

    let row = ban::confirm_ban_action_row(target_id, message.id, language);
    let content = msg
        .confirm_ban_prompt
        .replace("{}", &target_id.mention().to_string());
    if let Err(error) = create_response(http, interaction, &ephemeral(content, vec![row])).await {
        tracing::error!(%error, user_id = %target_id, "failed to open ban confirmation");
    }
}

/// The guild, log channel, and clicking moderator of a confirmation click, or
/// `None` outside a guild (component interactions always carry all three there).
fn confirmation_context(
    interaction: &Interaction,
) -> Option<(Id<GuildMarker>, Id<ChannelMarker>, &User)> {
    Some((
        interaction.guild_id?,
        interaction.channel.as_ref()?.id,
        interaction.author()?,
    ))
}

/// Lifts the ban on `target_id` after the moderator confirms, then rewrites the
/// original log message (`message_id`) to record the unban.
///
/// The interaction is acknowledged (the ephemeral prompt is replaced with a
/// short result) right after the unban HTTP call, before the best-effort log
/// edit, to stay inside Discord's interaction response window.
async fn perform_unban(
    http: &Client,
    interaction: &Interaction,
    target_id: Id<UserMarker>,
    message_id: Id<MessageMarker>,
) {
    let Some((guild_id, channel_id, moderator)) = confirmation_context(interaction) else {
        return;
    };
    let language = language_for(Some(guild_id));
    let msg = language.messages();

    if !has_ban_permission(interaction) {
        respond_ephemeral(http, interaction, msg.perm_needed_unban).await;
        return;
    }

    if crate::settings::dry_run() {
        tracing::warn!(
            guild_id = %guild_id,
            user_id = %target_id,
            "dry-run: skipping unban"
        );
    } else if let Err(error) = http.delete_ban(guild_id, target_id).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "failed to unban member from unban button"
        );
        respond_ephemeral(http, interaction, msg.unban_failed).await;
        return;
    }

    ban::forget_ban(guild_id, target_id);
    // Also clear any third-party grant-notice claim, so a future grant to this
    // member can post a fresh notice.
    ban::forget_grant_notice(guild_id, target_id);

    tracing::info!(
        guild_id = %guild_id,
        user_id = %target_id,
        moderator_id = %moderator.id,
        "unbanned member via unban button"
    );

    ack_confirmation(
        http,
        interaction,
        &msg.unbanned_ack
            .replace("{}", &target_id.mention().to_string()),
    )
    .await;

    let base = fetch_base_embed(http, channel_id, message_id).await;
    let embed = resolved_embed(base, target_id, moderator, language);
    edit_log_message(http, channel_id, message_id, embed, vec![], target_id).await;
}

/// Bans `target_id` after the moderator confirms an untrusted-bot notice, then
/// rewrites the notice (`message_id`) to record the ban.
///
/// The rewritten embed carries an `Unban` button so a mistaken confirmation can
/// be reversed, matching the auto-ban log embeds.
async fn perform_ban(
    http: &Client,
    interaction: &Interaction,
    target_id: Id<UserMarker>,
    message_id: Id<MessageMarker>,
) {
    let Some((guild_id, channel_id, moderator)) = confirmation_context(interaction) else {
        return;
    };
    let language = language_for(Some(guild_id));
    let msg = language.messages();

    if !has_ban_permission(interaction) {
        respond_ephemeral(http, interaction, msg.perm_needed_ban).await;
        return;
    }

    if let Err(error) = ban::confirm_bot_ban(http, guild_id, target_id).await {
        tracing::error!(
            %error,
            user_id = %target_id,
            "failed to ban bot from manual ban button"
        );
        respond_ephemeral(http, interaction, msg.ban_failed).await;
        return;
    }

    tracing::info!(
        guild_id = %guild_id,
        user_id = %target_id,
        moderator_id = %moderator.id,
        "banned bot via manual ban button"
    );

    ack_confirmation(
        http,
        interaction,
        &msg.banned_ack
            .replace("{}", &target_id.mention().to_string()),
    )
    .await;

    let base = fetch_base_embed(http, channel_id, message_id).await;
    let embed = manually_banned_embed(base, target_id, moderator, language);
    edit_log_message(
        http,
        channel_id,
        message_id,
        embed,
        vec![ban::unban_action_row(target_id, language)],
        target_id,
    )
    .await;
}

/// Dismisses the ephemeral confirmation prompt without acting, in response to
/// the `Cancel` button.
async fn handle_cancel(http: &Client, interaction: &Interaction) {
    let msg = language_for(interaction.guild_id).messages();
    if let Err(error) = create_response(http, interaction, &replace_prompt(msg.cancelled)).await {
        tracing::error!(%error, "failed to dismiss cancelled confirmation");
    }
}

/// Acknowledges a confirmation click by replacing the ephemeral prompt with a
/// short result line and dropping its buttons.
async fn ack_confirmation(http: &Client, interaction: &Interaction, content: &str) {
    if let Err(error) = create_response(http, interaction, &replace_prompt(content)).await {
        tracing::error!(%error, "failed to acknowledge confirmation");
    }
}

/// An ephemeral reply carrying `content` and `components`.
fn ephemeral(content: impl Into<String>, components: Vec<Component>) -> InteractionResponse {
    commands::ephemeral(
        InteractionResponseDataBuilder::new()
            .content(content)
            .components(components),
    )
}

/// Rewrites the clicked ephemeral prompt to `content`, dropping its buttons.
fn replace_prompt(content: &str) -> InteractionResponse {
    InteractionResponse {
        kind: InteractionResponseType::UpdateMessage,
        data: Some(
            InteractionResponseDataBuilder::new()
                .content(content)
                .components(Vec::new())
                .build(),
        ),
    }
}

/// Sends `response` as the initial response to `interaction`.
pub(crate) async fn create_response(
    http: &Client,
    interaction: &Interaction,
    response: &InteractionResponse,
) -> Result<(), HoneyPotError> {
    http.interaction(interaction.application_id)
        .create_response(interaction.id, &interaction.token, response)
        .await?;
    Ok(())
}

/// Fetches the original log message's first embed, so the rewritten embed can
/// preserve the trigger details.
///
/// Returns `None` (rebuild from scratch) if the message can't be fetched or has
/// no embed; the confirmation prompt lives in the same channel, so `channel_id`
/// always points at the log channel.
///
/// The embed is returned whole so the rewrite builders can inspect and drop
/// individual fields — see [`without_warnings_field`].
async fn fetch_base_embed(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
) -> Option<Embed> {
    match fetch_message(http, channel_id, message_id).await {
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

/// Fetches and decodes a single message.
async fn fetch_message(
    http: &Client,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
) -> Result<Message, HoneyPotError> {
    Ok(http.message(channel_id, message_id).await?.model().await?)
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
    http: &Client,
    channel_id: Id<ChannelMarker>,
    message_id: Id<MessageMarker>,
    embed: Embed,
    components: Vec<Component>,
    target_id: Id<UserMarker>,
) {
    let embeds = [embed];
    if let Err(error) = http
        .update_message(channel_id, message_id)
        .embeds(Some(&embeds))
        .components(Some(&components))
        .await
    {
        tracing::error!(
            %error,
            user_id = %target_id,
            "acted on member but failed to update log embed"
        );
    }
}

/// The base of a rewritten log embed: the original embed when present, or a
/// minimal embed carrying only the target user.
fn rewrite_base(base: Option<Embed>, target_id: Id<UserMarker>, msg: &Messages) -> EmbedBuilder {
    base.map(EmbedBuilder::from).unwrap_or_else(|| {
        EmbedBuilder::new().field(EmbedFieldBuilder::new(
            msg.field_user,
            target_id.mention().to_string(),
        ))
    })
}

/// Builds the "ban lifted" embed shown after a successful unban.
///
/// Reuses the original log embed (`base`, preserving the trigger details) when
/// present, recoloring it, retitling it, and appending who lifted the ban. The
/// aggregated "Warnings" field is dropped — the risk signals justified the ban,
/// not the resolved state. Falls back to a minimal embed if the message carried none.
pub(crate) fn resolved_embed(
    base: Option<Embed>,
    target_id: Id<UserMarker>,
    moderator: &User,
    language: Language,
) -> Embed {
    let msg = language.messages();
    let base = base.map(|embed| without_warnings_field(embed, msg));
    let embed = rewrite_base(base, target_id, msg)
        .title(msg.lifted_title)
        .color(ban::COLOUR_DARK_GREEN)
        .field(EmbedFieldBuilder::new(
            msg.field_unbanned_by,
            moderator.id.mention().to_string(),
        ))
        .build();
    ban::apply_dry_run_marker(embed, msg)
}

/// Builds the "bot banned" embed shown after a manual ban confirmation.
///
/// Reuses the pending notice embed (`base`, preserving the trigger details) when
/// present, recoloring it, retitling it, replacing the pending description, and
/// appending who confirmed the ban.
pub(crate) fn manually_banned_embed(
    base: Option<Embed>,
    target_id: Id<UserMarker>,
    moderator: &User,
    language: Language,
) -> Embed {
    let msg = language.messages();
    let embed = rewrite_base(base, target_id, msg)
        .title(msg.bot_banned_title)
        .description(msg.bot_banned_desc)
        .color(ban::COLOUR_RED)
        .field(EmbedFieldBuilder::new(
            msg.field_banned_by,
            moderator.id.mention().to_string(),
        ))
        .build();
    ban::apply_dry_run_marker(embed, msg)
}

/// Sends an ephemeral text response, logging any failure.
async fn respond_ephemeral(http: &Client, interaction: &Interaction, content: &str) {
    if let Err(error) = create_response(http, interaction, &ephemeral(content, Vec::new())).await {
        tracing::error!(%error, "failed to send ephemeral interaction response");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::fixtures;
    use twilight_model::channel::message::embed::EmbedField;

    /// Builds an embed carrying the given field names (values are placeholders).
    fn embed_with_fields(names: &[&str]) -> Embed {
        let mut embed = fixtures::embed();
        embed.fields = names
            .iter()
            .map(|name| EmbedField {
                inline: false,
                name: name.to_string(),
                value: "value".to_string(),
            })
            .collect();
        embed
    }

    fn field_names(embed: &Embed) -> Vec<&str> {
        embed.fields.iter().map(|f| f.name.as_str()).collect()
    }

    #[test]
    fn without_warnings_field_drops_only_the_warnings_field() {
        let msg = Language::En.messages();
        let embed = embed_with_fields(&[msg.field_user, msg.field_warnings, msg.field_trigger]);
        let stripped = without_warnings_field(embed, msg);
        assert_eq!(
            field_names(&stripped),
            vec![msg.field_user, msg.field_trigger]
        );
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

    #[test]
    fn resolved_embed_drops_warnings_and_appends_unbanned_by() {
        let msg = Language::En.messages();
        let base = embed_with_fields(&[msg.field_user, msg.field_warnings]);
        let moderator = fixtures::user(7, "mod");
        let embed = resolved_embed(Some(base), Id::new(1), &moderator, Language::En);

        assert_eq!(embed.title.as_deref(), Some(msg.lifted_title));
        assert_eq!(embed.color, Some(ban::COLOUR_DARK_GREEN));
        assert_eq!(
            field_names(&embed),
            vec![msg.field_user, msg.field_unbanned_by]
        );
        assert_eq!(embed.fields[1].value, "<@7>");
    }

    #[test]
    fn manually_banned_embed_keeps_warnings_and_sets_description() {
        let msg = Language::En.messages();
        let base = embed_with_fields(&[msg.field_user, msg.field_warnings]);
        let moderator = fixtures::user(7, "mod");
        let embed = manually_banned_embed(Some(base), Id::new(1), &moderator, Language::En);

        assert_eq!(embed.title.as_deref(), Some(msg.bot_banned_title));
        assert_eq!(embed.description.as_deref(), Some(msg.bot_banned_desc));
        assert_eq!(embed.color, Some(ban::COLOUR_RED));
        assert_eq!(
            field_names(&embed),
            vec![msg.field_user, msg.field_warnings, msg.field_banned_by]
        );
    }

    #[test]
    fn resolved_embed_falls_back_to_minimal_embed_without_base() {
        let msg = Language::En.messages();
        let moderator = fixtures::user(7, "mod");
        let embed = resolved_embed(None, Id::new(1), &moderator, Language::En);

        assert_eq!(
            field_names(&embed),
            vec![msg.field_user, msg.field_unbanned_by]
        );
        assert_eq!(embed.fields[0].value, "<@1>");
    }
}
