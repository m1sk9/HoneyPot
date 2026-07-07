//! Embed preview mode (compiled only under the `preview` feature).
//!
//! When the crate is built with `--features preview`, `main` posts one message
//! per honeypot log-embed variant — in *every* [`Language`] — to the channel
//! named by [`PREVIEW_CHANNEL_ENV`], then exits without connecting to the
//! gateway or reading the guild config. This lets the embed layouts (and each
//! localization) be reviewed on any Discord client — including mobile, where the
//! moderator view is unavailable — without tripping a real honeypot.
//!
//! The samples are built with the *real* embed builders ([`ban::build_ban_embed`],
//! [`ban::build_pending_embed`], [`interaction::resolved_embed`],
//! [`interaction::manually_banned_embed`]), so a preview cannot drift from what
//! the bot actually posts. The feature is off by default, so neither this module
//! nor its fabricated sample data is compiled into the production binary.

use crate::discord::{ban, interaction};
use crate::error::HoneyPotError;
use crate::i18n::Language;
use serenity::all::{
    ButtonStyle, ChannelId, CreateActionRow, CreateButton, CreateEmbed, CreateMessage, Embed, Http,
    RoleId, Timestamp, User, UserId, UserPublicFlags,
};
use std::time::{SystemTime, UNIX_EPOCH};

/// Env var naming the channel the embed previews are posted to.
pub const PREVIEW_CHANNEL_ENV: &str = "HONEYPOT_PREVIEW_CHANNEL";

/// Discord's epoch (2015-01-01) in milliseconds, for fabricating snowflake ids
/// with a chosen creation time.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// Milliseconds in an hour, for readable sample timestamps.
const HOUR_MS: u64 = 3_600_000;

/// Milliseconds in a day, for readable sample timestamps.
const DAY_MS: u64 = 86_400_000;

/// Posts one of each honeypot log embed, in every supported language, to
/// `channel_id`, then returns.
///
/// Sends over REST via a fresh [`Http`]; no gateway connection or privileged
/// intent is needed. A send failure aborts the run and surfaces as an error so
/// the exit code reflects it (e.g. the bot lacking access to the channel).
pub async fn run(token: &str, channel_id: ChannelId) -> Result<(), HoneyPotError> {
    let http = Http::new(token);
    let samples = sample_messages(channel_id);
    let total = samples.len();
    for (index, message) in samples.into_iter().enumerate() {
        channel_id.send_message(&http, message).await?;
        tracing::info!(step = index + 1, total, "posted embed preview");
    }
    tracing::info!(channel_id = %channel_id, total, "embed preview complete");
    Ok(())
}

/// Builds the sample messages: one per log-embed variant in every supported
/// language, each captioned with its language tag and variant so they can be
/// told apart in the channel. The fabricated offenders, moderator, contexts, and
/// triggers are language-independent, so they are built once and reused by
/// reference across languages; only the builders' `language` argument differs.
/// Rendering both languages also doubles as a side-by-side check that the
/// `en`/`ja` catalogs read correctly.
fn sample_messages(channel_id: ChannelId) -> Vec<CreateMessage> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before the UNIX epoch")
        .as_millis() as u64;
    let ts = |ms: u64| Timestamp::from_millis(ms as i64).expect("fabricated timestamp is valid");

    // A brand-new, default-avatar account that Discord has flagged as a spammer.
    let mut offender = User::default();
    offender.id = fabricated_id(now_ms - DAY_MS);
    offender.name = "spammy".to_string();
    offender.discriminator = None;
    offender.global_name = Some("Free Nitro Giveaway".to_string());
    offender.public_flags = Some(UserPublicFlags::SPAMMER);

    let mut sus_bot = offender.clone();
    sus_bot.bot = true;
    sus_bot.name = "sus-bot".to_string();
    sus_bot.global_name = Some("Suspicious Bot".to_string());
    sus_bot.public_flags = None;

    let mut moderator = User::default();
    moderator.id = UserId::new(100_000_000_000_000_000);
    moderator.name = "mod".to_string();
    moderator.discriminator = None;
    moderator.global_name = Some("A Moderator".to_string());

    // Flagged: joined an hour ago and currently flagged for unusual DM activity.
    let flagged = ban::OffenderContext {
        joined_at: Some(ts(now_ms - HOUR_MS)),
        unusual_dm_activity_until: Some(ts(now_ms + HOUR_MS)),
    };
    // Established: joined a month ago, not flagged.
    let established = ban::OffenderContext {
        joined_at: Some(ts(now_ms - 30 * DAY_MS)),
        unusual_dm_activity_until: None,
    };

    let channel_trigger = ban::BanTrigger::Channel {
        channel_id,
        content: Some("@everyone FREE NITRO 🎁 claim now: http://not-a-scam.example".to_string()),
    };
    let role_trigger = ban::BanTrigger::Role(RoleId::new(1_234_567_890_123_456_789));

    // Post the full set once per supported language, tagging each caption with
    // the language so both renderings can be compared in the channel.
    let mut messages = Vec::new();
    for language in [Language::En, Language::Ja] {
        let tag = language_tag(language);

        // A non-functional stand-in for the untrusted-bot notice's Ban button;
        // the preview run never handles interactions.
        let ban_notice_button = CreateActionRow::Buttons(vec![
            CreateButton::new("preview_ban_noop")
                .style(ButtonStyle::Danger)
                .label(language.messages().btn_ban),
        ]);

        messages.push(
            captioned(
                &format!("**[{tag}] Preview 1/5** — user banned (channel trigger)"),
                ban::build_ban_embed(&offender, &channel_trigger, &flagged, language),
            )
            .components(vec![ban::unban_action_row(offender.id, language)]),
        );
        messages.push(
            captioned(
                &format!(
                    "**[{tag}] Preview 2/5** — user banned (role trigger, established account)"
                ),
                ban::build_ban_embed(&offender, &role_trigger, &established, language),
            )
            .components(vec![ban::unban_action_row(offender.id, language)]),
        );
        messages.push(
            captioned(
                &format!("**[{tag}] Preview 3/5** — untrusted bot notice (awaiting manual review)"),
                ban::build_pending_embed(&sus_bot, &channel_trigger, &flagged, language),
            )
            .components(vec![ban_notice_button]),
        );
        messages.push(captioned(
            &format!("**[{tag}] Preview 4/5** — ban lifted (after unban confirmation)"),
            interaction::resolved_embed(
                Some(as_received(ban::build_ban_embed(
                    &offender,
                    &channel_trigger,
                    &flagged,
                    language,
                ))),
                offender.id,
                &moderator,
                language,
            ),
        ));
        messages.push(
            captioned(
                &format!("**[{tag}] Preview 5/5** — bot banned (after manual confirmation)"),
                interaction::manually_banned_embed(
                    Some(as_received(ban::build_pending_embed(
                        &sus_bot,
                        &channel_trigger,
                        &flagged,
                        language,
                    ))),
                    sus_bot.id,
                    &moderator,
                    language,
                ),
            )
            .components(vec![ban::unban_action_row(sus_bot.id, language)]),
        );
    }
    messages
}

/// Round-trips a freshly built embed into the received-message [`Embed`] form the
/// rewrite builders consume in production. This lets the previews exercise the
/// real rewrite path — including the lifted-ban embed dropping its warnings field
/// — rather than a preview-only shortcut. `CreateEmbed` serializes transparently
/// as its inner `Embed`, so the round-trip is lossless.
fn as_received(embed: CreateEmbed) -> Embed {
    let value = serenity::json::to_value(embed).expect("preview embed serializes");
    serenity::json::from_value(value).expect("preview embed round-trips")
}

/// Short uppercase tag for a preview caption, e.g. `EN`.
fn language_tag(language: Language) -> &'static str {
    match language {
        Language::En => "EN",
        Language::Ja => "JA",
    }
}

/// A [`UserId`] whose snowflake encodes `created_ms` as its creation time, so
/// [`User::created_at`] reports it in the preview.
fn fabricated_id(created_ms: u64) -> UserId {
    UserId::new((created_ms - DISCORD_EPOCH_MS) << 22)
}

/// A [`CreateMessage`] with a caption line and a single embed.
fn captioned(caption: &str, embed: CreateEmbed) -> CreateMessage {
    CreateMessage::new().content(caption).embed(embed)
}
