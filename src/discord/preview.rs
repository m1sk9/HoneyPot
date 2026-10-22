//! Embed preview mode (compiled only under the `preview` feature).
//!
//! When the crate is built with `--features preview`, `main` posts one message
//! per honeypot log-embed variant *and* per slash-command reply embed — in
//! *every* [`Language`] — to the channel named by [`PREVIEW_CHANNEL_ENV`], then
//! exits without connecting to the gateway or reading the guild config. This lets
//! the embed layouts (and each localization) be reviewed on any Discord client —
//! including mobile, where the moderator view is unavailable — without tripping a
//! real honeypot or invoking a command.
//!
//! The samples are built with the *real* embed builders ([`ban::build_ban_embed`],
//! [`ban::build_pending_embed`], [`interaction::resolved_embed`],
//! [`interaction::manually_banned_embed`], and each command module's
//! `build_embed`), so a preview cannot drift from what the bot actually posts. The
//! feature is off by default, so neither this module nor its fabricated sample
//! data is compiled into the production binary.

use crate::discord::raw::AccountSignals;
use crate::discord::{ban, commands, fixtures, interaction};
use crate::error::HoneyPotError;
use crate::i18n::Language;
use std::time::{SystemTime, UNIX_EPOCH};
use twilight_http::Client;
use twilight_model::channel::message::component::ButtonStyle;
use twilight_model::channel::message::{Component, Embed};
use twilight_model::id::{Id, marker::ChannelMarker};
use twilight_model::user::UserFlags;
use twilight_model::util::Timestamp;

/// Env var naming the channel the embed previews are posted to.
pub const PREVIEW_CHANNEL_ENV: &str = "HONEYPOT_PREVIEW_CHANNEL";

/// Discord's epoch (2015-01-01) in milliseconds, for fabricating snowflake ids
/// with a chosen creation time.
const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;

/// Milliseconds in an hour, for readable sample timestamps.
const HOUR_MS: u64 = 3_600_000;

/// Milliseconds in a day, for readable sample timestamps.
const DAY_MS: u64 = 86_400_000;

/// One preview post: a caption line, a single embed, and its buttons.
struct PreviewMessage {
    caption: String,
    embed: Embed,
    components: Vec<Component>,
}

/// Posts one of each honeypot log embed, in every supported language, to
/// `channel_id`, then returns.
///
/// Sends over REST via a fresh [`Client`]; no gateway connection or privileged
/// intent is needed. A send failure aborts the run and surfaces as an error so
/// the exit code reflects it (e.g. the bot lacking access to the channel).
pub async fn run(token: String, channel_id: Id<ChannelMarker>) -> Result<(), HoneyPotError> {
    let http = Client::new(token);
    let samples = sample_messages(channel_id);
    let total = samples.len();
    for (index, message) in samples.into_iter().enumerate() {
        let embeds = [message.embed];
        http.create_message(channel_id)
            .content(&message.caption)
            .embeds(&embeds)
            .components(&message.components)
            .await?;
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
fn sample_messages(channel_id: Id<ChannelMarker>) -> Vec<PreviewMessage> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is set before the UNIX epoch")
        .as_millis() as u64;
    let ts =
        |ms: u64| Timestamp::from_micros(ms as i64 * 1000).expect("fabricated timestamp is valid");

    let mut offender = fixtures::user(fabricated_id(now_ms - DAY_MS), "spammy");
    offender.global_name = Some("Free Nitro Giveaway".to_string());

    let mut sus_bot = offender.clone();
    sus_bot.bot = true;
    sus_bot.name = "sus-bot".to_string();
    sus_bot.global_name = Some("Suspicious Bot".to_string());

    let mut moderator = fixtures::user(100_000_000_000_000_000, "mod");
    moderator.global_name = Some("A Moderator".to_string());

    // An account with a couple of profile badges *and* the spammer flag, so the
    // `/whois` preview shows both a populated badges field and the warnings field.
    let mut whois_target = fixtures::user(fabricated_id(now_ms - 400 * DAY_MS), "member");
    whois_target.global_name = Some("A Member".to_string());
    whois_target.public_flags =
        Some(UserFlags::ACTIVE_DEVELOPER | UserFlags::PREMIUM_EARLY_SUPPORTER);
    let whois_signals = AccountSignals {
        spammer: true,
        unusual_dm_activity_until: None,
    };
    let doctor_findings = commands::doctor::preview_findings();

    // The spammer mark belongs to the offender's account, so every offender
    // context carries it while the bot's does not.
    let flagged = ban::OffenderContext {
        joined_at: Some(ts(now_ms - HOUR_MS)),
        unusual_dm_activity_until: Some(ts(now_ms + HOUR_MS)),
        spammer: true,
    };
    let flagged_bot = ban::OffenderContext {
        spammer: false,
        ..flagged
    };
    let established = ban::OffenderContext {
        joined_at: Some(ts(now_ms - 30 * DAY_MS)),
        unusual_dm_activity_until: None,
        spammer: true,
    };

    let channel_trigger = ban::BanTrigger::Channel {
        channel_id,
        content: Some("@everyone FREE NITRO 🎁 claim now: http://not-a-scam.example".to_string()),
    };
    let role_trigger = ban::BanTrigger::Role(Id::new(1_234_567_890_123_456_789));

    let mut messages = Vec::new();
    for language in [Language::En, Language::Ja] {
        let tag = language_tag(language);

        // A non-functional stand-in for the review notices' Ban button; the
        // preview run never handles interactions.
        let ban_notice_button = || {
            ban::buttons_row(vec![ban::button(
                "preview_ban_noop",
                language.messages().btn_ban,
                ButtonStyle::Danger,
            )])
        };

        messages.push(captioned(
            format!("**[{tag}] Preview 1/6** — user banned (channel trigger)"),
            ban::build_ban_embed(&offender, &channel_trigger, &flagged, language),
            vec![ban::unban_action_row(offender.id, language)],
        ));
        messages.push(captioned(
            format!("**[{tag}] Preview 2/6** — user banned (role trigger, established account)"),
            ban::build_ban_embed(&offender, &role_trigger, &established, language),
            vec![ban::unban_action_row(offender.id, language)],
        ));
        messages.push(captioned(
            format!("**[{tag}] Preview 3/6** — untrusted bot notice (awaiting manual review)"),
            ban::build_pending_embed(&sus_bot, &channel_trigger, &flagged_bot, language),
            vec![ban_notice_button()],
        ));
        messages.push(captioned(
            format!("**[{tag}] Preview 4/6** — ban lifted (after unban confirmation)"),
            interaction::resolved_embed(
                Some(ban::build_ban_embed(
                    &offender,
                    &channel_trigger,
                    &flagged,
                    language,
                )),
                offender.id,
                &moderator,
                language,
            ),
            Vec::new(),
        ));
        messages.push(captioned(
            format!("**[{tag}] Preview 5/6** — bot banned (after manual confirmation)"),
            interaction::manually_banned_embed(
                Some(ban::build_pending_embed(
                    &sus_bot,
                    &channel_trigger,
                    &flagged_bot,
                    language,
                )),
                sus_bot.id,
                &moderator,
                language,
            ),
            vec![ban::unban_action_row(sus_bot.id, language)],
        ));
        messages.push(captioned(
            format!(
                "**[{tag}] Preview 6/6** — third-party role grant notice (awaiting manual review)"
            ),
            ban::build_third_party_grant_embed(&offender, &role_trigger, &established, language),
            vec![ban_notice_button()],
        ));

        // Slash-command reply embeds, built with the real command builders so
        // these previews cannot drift from what the commands post.
        messages.push(captioned(
            format!("**[{tag}] Command 1/5** — /help"),
            commands::help::build_embed(language, None),
            Vec::new(),
        ));
        messages.push(captioned(
            format!("**[{tag}] Command 2/5** — /version"),
            commands::version::build_embed(language),
            Vec::new(),
        ));
        messages.push(captioned(
            format!("**[{tag}] Command 3/5** — /ping"),
            commands::ping::build_embed(language, 42),
            Vec::new(),
        ));
        messages.push(captioned(
            format!("**[{tag}] Command 4/5** — /whois"),
            commands::whois::build_embed(&whois_target, None, whois_signals, language),
            Vec::new(),
        ));
        messages.push(captioned(
            format!("**[{tag}] Command 5/5** — /doctor"),
            commands::doctor::build_embed(&doctor_findings, language),
            Vec::new(),
        ));
    }
    messages
}

/// Short uppercase tag for a preview caption, e.g. `EN`.
fn language_tag(language: Language) -> &'static str {
    match language {
        Language::En => "EN",
        Language::Ja => "JA",
    }
}

/// A user snowflake that encodes `created_ms` as its creation time, so
/// [`ban::created_at`] reports it in the preview.
fn fabricated_id(created_ms: u64) -> u64 {
    (created_ms - DISCORD_EPOCH_MS) << 22
}

/// A preview post with a caption line, a single embed, and its buttons.
fn captioned(caption: String, embed: Embed, components: Vec<Component>) -> PreviewMessage {
    PreviewMessage {
        caption,
        embed,
        components,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_messages_covers_every_variant_in_both_languages() {
        // 6 log-embed variants + 5 command embeds, per each supported language.
        let messages = sample_messages(Id::new(1));
        assert_eq!(messages.len(), (6 + 5) * 2);
    }
}
