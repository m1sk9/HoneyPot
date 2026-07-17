//! Localization catalog for HoneyPot's moderator-facing text.
//!
//! Every user-visible string in the log embeds and the button interactions is
//! held here as a field on [`Messages`], with one filled-in catalog per
//! [`Language`] ([`EN`] / [`JA`]). Because both catalogs are `const Messages`
//! values, adding a field forces *both* languages to define it or the crate
//! fails to compile — translation gaps are caught at build time rather than
//! surfacing as an untranslated string in production.
//!
//! The language is chosen per guild (see [`crate::settings::GuildConfig`]) and
//! threaded into the embed/response builders; detection logic stays language-free.
//!
//! Strings carrying a single `{}` are format templates: the call site fills the
//! placeholder with [`str::replace`] (`format!` requires a literal format
//! string, so the templates can't be `format!`-ed directly). Only single-slot
//! substitution is used, so `replace` is unambiguous. Emoji and `⚠️` markers are
//! baked into the strings so translators can place them per language.
//!
//! The Discord audit-log ban reason is deliberately *not* localized (it stays
//! English for consistent, searchable moderation logs), so it has no entry here.

use serde::Deserialize;

/// The display language for a guild's moderator-facing text.
///
/// Deserialized from the `language` key of a `[[guilds]]` entry (`"en"` / `"ja"`,
/// lowercase). Defaults to [`Language::En`] when the key is omitted, so existing
/// configuration files keep their current English output.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English (default).
    #[default]
    En,
    /// Japanese.
    Ja,
}

impl Language {
    /// Returns the message catalog for this language.
    pub fn messages(self) -> &'static Messages {
        match self {
            Language::En => &EN,
            Language::Ja => &JA,
        }
    }
}

/// The full set of moderator-facing strings, one instance per language.
///
/// Fields carrying a `{}` are templates (see the module docs); the rest are
/// literal labels. See [`EN`] for the canonical wording of each field.
pub struct Messages {
    /// Title of the auto-ban log embed.
    pub ban_title: &'static str,
    /// Title of the untrusted-bot notice embed.
    pub pending_title: &'static str,
    /// Description of the untrusted-bot notice embed.
    pub pending_desc: &'static str,
    /// Title of the embed after a ban is lifted.
    pub lifted_title: &'static str,
    /// Title of the embed after a bot is manually banned.
    pub bot_banned_title: &'static str,
    /// Description of the embed after a bot is manually banned.
    pub bot_banned_desc: &'static str,
    /// Title of the third-party role grant review notice embed.
    pub manual_grant_title: &'static str,
    /// Description of the third-party role grant review notice embed.
    pub manual_grant_desc: &'static str,

    /// "User" field name.
    pub field_user: &'static str,
    /// "Trigger" field name.
    pub field_trigger: &'static str,
    /// "Account" field name.
    pub field_account: &'static str,
    /// "Account created" field name.
    pub field_account_created: &'static str,
    /// "Joined server" field name.
    pub field_joined: &'static str,
    /// "Warnings" field name — the single field aggregating every risk signal.
    pub field_warnings: &'static str,
    /// "Unbanned by" field name.
    pub field_unbanned_by: &'static str,
    /// "Banned by" field name.
    pub field_banned_by: &'static str,

    // Trigger-kind labels shown in the "Trigger" field value. Distinct from the
    // audit-log `BanTrigger::kind` tokens, which stay English.
    /// Label for a role trigger.
    pub trigger_role: &'static str,
    /// Label for a channel trigger.
    pub trigger_channel: &'static str,

    /// "Display" label inside the "User" field.
    pub display_label: &'static str,
    /// "ID" label inside the "User" field.
    pub id_label: &'static str,

    /// New-account warning line; `{}` is the age threshold in days.
    pub new_account: &'static str,
    /// Warning line shown when the account has no custom avatar.
    pub avatar_default: &'static str,
    /// Warning line shown when Discord has flagged the account as a spammer.
    pub spammer: &'static str,
    /// Warning line for an active unusual-DM-activity flag; `{}` is its expiry.
    pub unusual_dm_flagged: &'static str,

    /// "Bot" label in the "Account" field value.
    pub bot_label: &'static str,
    /// "System" label in the "Account" field value.
    pub system_label: &'static str,
    /// Affirmative value ("Yes").
    pub yes: &'static str,
    /// Negative value ("No").
    pub no: &'static str,
    /// "Joined server" value when the join date is unknown.
    pub joined_unknown: &'static str,

    /// "Unban" button label.
    pub btn_unban: &'static str,
    /// "Ban" button label.
    pub btn_ban: &'static str,
    /// "Cancel" button label.
    pub btn_cancel: &'static str,
    /// "Confirm unban" button label.
    pub btn_confirm_unban: &'static str,
    /// "Confirm ban" button label.
    pub btn_confirm_ban: &'static str,

    /// Rejection shown when the clicker lacks Ban Members for an unban.
    pub perm_needed_unban: &'static str,
    /// Rejection shown when the clicker lacks Ban Members for a ban.
    pub perm_needed_ban: &'static str,
    /// Unban confirmation prompt; `{}` is the target mention.
    pub confirm_unban_prompt: &'static str,
    /// Ban confirmation prompt; `{}` is the target mention.
    pub confirm_ban_prompt: &'static str,
    /// Unban acknowledgement; `{}` is the target mention.
    pub unbanned_ack: &'static str,
    /// Ban acknowledgement; `{}` is the target mention.
    pub banned_ack: &'static str,
    /// Acknowledgement shown when a confirmation is cancelled.
    pub cancelled: &'static str,
    /// Error shown when an unban HTTP call fails.
    pub unban_failed: &'static str,
    /// Error shown when a ban HTTP call fails.
    pub ban_failed: &'static str,

    /// Footer marking a dry-run embed.
    pub dry_run_footer: &'static str,

    // Slash-command descriptions, shown in the Discord command picker. Also used
    // as the `ja` command-description localization at registration.
    /// `/help` command description.
    pub cmd_help_desc: &'static str,
    /// `/version` command description.
    pub cmd_version_desc: &'static str,
    /// `/ping` command description.
    pub cmd_ping_desc: &'static str,
    /// `/whois` command description.
    pub cmd_whois_desc: &'static str,
    /// `/whois` `user` option description.
    pub cmd_whois_user_desc: &'static str,
    /// `/doctor` command description.
    pub cmd_doctor_desc: &'static str,

    /// Rejection shown when a command caller lacks Ban Members.
    pub cmd_perm_needed: &'static str,
    /// Rejection shown when a command caller lacks Manage Server.
    pub cmd_perm_needed_manage: &'static str,
    /// Rejection shown when a guild-only command is used outside a guild.
    pub cmd_guild_only: &'static str,

    /// `/help` embed title.
    pub help_title: &'static str,

    /// `/version` embed title.
    pub version_title: &'static str,
    /// "Version" field name.
    pub version_label: &'static str,
    /// "Build" field name (holds the commit hash).
    pub build_label: &'static str,

    /// `/ping` embed title.
    pub ping_title: &'static str,
    /// Latency line; `{}` is the round-trip time in milliseconds.
    pub pong: &'static str,

    /// `/whois` embed title.
    pub whois_title: &'static str,
    /// "Badges" field name.
    pub field_badges: &'static str,
    /// "Badges" value shown when the account carries none.
    pub badges_none: &'static str,

    /// `/doctor` embed title.
    pub doctor_title: &'static str,
    /// Shown when `/doctor` runs in a guild absent from the configuration.
    pub doctor_not_configured: &'static str,
    /// Ban-permission check, passing.
    pub ban_perm_ok: &'static str,
    /// Ban-permission check, failing.
    pub ban_perm_missing: &'static str,
    /// "Honeypot roles" check label (the count and mentions are appended).
    pub check_roles: &'static str,
    /// "Honeypot channels" check label (the count and mentions are appended).
    pub check_channels: &'static str,
    /// Log-channel check, passing; `{}` is the channel mention.
    pub log_channel_ok: &'static str,
    /// Log-channel check, failing; `{}` is the channel mention.
    pub log_channel_missing: &'static str,
    /// Warning that one or more configured role IDs are absent from the guild;
    /// `{}` lists the missing IDs.
    pub roles_missing: &'static str,
    /// Warning that one or more configured channel IDs are absent from the guild;
    /// `{}` lists the missing IDs.
    pub channels_missing: &'static str,
    /// Passing role-order result: every honeypot role sits below the bot.
    pub role_order_ok: &'static str,
    /// Role-order result for ordinary roles above the bot; `{}` lists them.
    pub role_order_blocking: &'static str,
    /// Role-order result for privileged/bot roles above the bot (benign); `{}`
    /// lists them.
    pub role_order_privileged: &'static str,
    /// Passing marker (leads a check line).
    pub ok_mark: &'static str,
    /// Warning marker (leads a check line).
    pub warn_mark: &'static str,
    /// Informational marker (leads a benign notice line).
    pub info_mark: &'static str,
}

/// English catalog (the default, and the canonical source wording).
pub const EN: Messages = Messages {
    ban_title: "🍯 Honeypot triggered — user banned",
    pending_title: "🍯 Honeypot triggered — untrusted bot",
    pending_desc: "This bot is not in the trusted list, so it was not auto-banned.\nPress **Ban** to remove it.",
    lifted_title: "🍯 Ban lifted",
    bot_banned_title: "🍯 Honeypot triggered — bot banned",
    bot_banned_desc: "Banned after manual review.",
    manual_grant_title: "🍯 Honeypot role granted by a third party",
    manual_grant_desc: "This honeypot role was not self-assigned — it was granted by someone else (an admin, a reaction role) or by an unresolved source, so the trap was not triggered and no ban was applied.\nReview and press **Ban** if this is spam.",

    field_user: "User",
    field_trigger: "Trigger",
    field_account: "Account",
    field_account_created: "Account created",
    field_joined: "Joined server",
    field_warnings: "Warnings",
    field_unbanned_by: "Unbanned by",
    field_banned_by: "Banned by",

    trigger_role: "role",
    trigger_channel: "channel",

    display_label: "Display",
    id_label: "ID",
    new_account: "⚠️ New account, created less than {} days ago",
    avatar_default: "⚠️ No custom avatar set, which is common for spam accounts",
    spammer: "⚠️ Marked by Discord as a likely spammer",
    unusual_dm_flagged: "⚠️ Flagged for unusual DM activity until {} — this account has recently sent suspicious DMs to members",
    bot_label: "Bot",
    system_label: "System",
    yes: "Yes",
    no: "No",
    joined_unknown: "Unknown",

    btn_unban: "Unban",
    btn_ban: "Ban",
    btn_cancel: "Cancel",
    btn_confirm_unban: "Confirm unban",
    btn_confirm_ban: "Confirm ban",

    perm_needed_unban: "You need the Ban Members permission to unban.",
    perm_needed_ban: "You need the Ban Members permission to ban.",
    confirm_unban_prompt: "Unban {}?",
    confirm_ban_prompt: "Ban {}?",
    unbanned_ack: "Unbanned {}.",
    banned_ack: "Banned {}.",
    cancelled: "Cancelled.",
    unban_failed: "Failed to unban the user. Please try again.",
    ban_failed: "Failed to ban the user. Please try again.",

    dry_run_footer: "⚠️ DRY-RUN — no ban/unban was executed",

    cmd_help_desc: "Show the list of commands",
    cmd_version_desc: "Show the bot version and build hash",
    cmd_ping_desc: "Show the bot's latency",
    cmd_whois_desc: "Show information about a user",
    cmd_whois_user_desc: "The user to look up",
    cmd_doctor_desc: "Check that the bot is correctly configured for this server",

    cmd_perm_needed: "You need the Ban Members permission to use this command.",
    cmd_perm_needed_manage: "You need the Manage Server permission to use this command.",
    cmd_guild_only: "This command can only be used in a server.",

    help_title: "🍯 HoneyPot commands",

    version_title: "🍯 HoneyPot version",
    version_label: "Version",
    build_label: "Build",

    ping_title: "🏓 Pong!",
    pong: "Round-trip latency: {} ms",

    whois_title: "👤 User information",
    field_badges: "Badges",
    badges_none: "None",

    doctor_title: "🩺 Configuration check",
    doctor_not_configured: "This server is not present in the configuration, so there is nothing to check.",
    ban_perm_ok: "Required permission granted: `Ban Members`",
    ban_perm_missing: "Required permission missing: `Ban Members`",
    check_roles: "Honeypot roles",
    check_channels: "Honeypot channels",
    log_channel_ok: "Log channel configured: {}",
    log_channel_missing: "Log channel not found in this server: {}",
    roles_missing: "These configured role IDs were not found in this server: {}",
    channels_missing: "These configured channel IDs were not found in this server: {}",
    role_order_ok: "Every honeypot role sits below the bot's own role.",
    role_order_blocking: "These roles sit at or above the bot's own role, so the honeypot may not fire correctly: {}",
    role_order_privileged: "These roles sit at or above the bot's own role. That is fine since they hold elevated permissions, but adjust their position if it was not intended: {}",
    ok_mark: "✅",
    warn_mark: "⚠️",
    info_mark: "ℹ️",
};

/// Japanese catalog.
pub const JA: Messages = Messages {
    ban_title: "🍯 Honeypot 作動 — ユーザーを BAN しました",
    pending_title: "🍯 Honeypot 作動 — 信頼されていない Bot",
    pending_desc: "この Bot は信頼リストに存在しないため、自動 BAN は行われません\n削除するには **BAN** を押してください。",
    lifted_title: "🍯 BAN を解除しました",
    bot_banned_title: "🍯 Honeypot 作動 — Bot を BAN しました",
    bot_banned_desc: "手動レビューの結果 BAN しました",
    manual_grant_title: "🍯 Honeypot ロールが第三者により付与",
    manual_grant_desc: "この Honeypot ロールは本人による付与ではなく、第三者（管理者・リアクションロール）または不明な経路で付与されたため、トリガーは発動せず BAN も行っていません。\nスパムであれば内容を確認して **BAN** を押してください。",

    field_user: "ユーザー",
    field_trigger: "トリガー",
    field_account: "アカウント",
    field_account_created: "アカウント作成日",
    field_joined: "サーバー参加日",
    field_warnings: "警告",
    field_unbanned_by: "解除者",
    field_banned_by: "実行者",

    trigger_role: "ロール",
    trigger_channel: "チャンネル",

    display_label: "表示名",
    id_label: "ID",
    new_account: "⚠️ 作成から {} 日未満の新規アカウントです",
    avatar_default: "⚠️ アカウント画像が未設定のため、スパムの可能性が高いです",
    spammer: "⚠️ Discord によりスパムの可能性が高いとマークされています",
    unusual_dm_flagged: "⚠️ 異常な DM アクティビティのフラグが {} まで有効です。過去このユーザはメンバーに対して不審な DM を送信しています",
    bot_label: "Bot",
    system_label: "システム",
    yes: "はい",
    no: "いいえ",
    joined_unknown: "不明",

    btn_unban: "BAN 解除",
    btn_ban: "BAN",
    btn_cancel: "キャンセル",
    btn_confirm_unban: "BAN 解除を確定",
    btn_confirm_ban: "BAN を確定",

    perm_needed_unban: "BAN 解除には Ban Members 権限が必要です。",
    perm_needed_ban: "BAN には Ban Members 権限が必要です。",
    confirm_unban_prompt: "{} の BAN を解除しますか？",
    confirm_ban_prompt: "{} を BAN しますか？",
    unbanned_ack: "{} の BAN を解除しました。",
    banned_ack: "{} を BAN しました。",
    cancelled: "キャンセルしました。",
    unban_failed: "BAN 解除に失敗しました。もう一度お試しください。",
    ban_failed: "BAN に失敗しました。もう一度お試しください。",

    dry_run_footer: "⚠️ DRY-RUN — BAN／BAN 解除は実行されていません",

    cmd_help_desc: "コマンド一覧を表示します",
    cmd_version_desc: "Bot のバージョンとビルドハッシュを表示します",
    cmd_ping_desc: "Bot のレイテンシを表示します",
    cmd_whois_desc: "ユーザーの情報を表示します",
    cmd_whois_user_desc: "対象のユーザー",
    cmd_doctor_desc: "このサーバーに対して Bot の設定が正しいか検査します",

    cmd_perm_needed: "このコマンドの実行には Ban Members 権限が必要です。",
    cmd_perm_needed_manage: "このコマンドの実行には サーバー管理 権限が必要です。",
    cmd_guild_only: "このコマンドはサーバー内でのみ使用できます。",

    help_title: "🍯 HoneyPot コマンド一覧",

    version_title: "🍯 HoneyPot バージョン",
    version_label: "バージョン",
    build_label: "ビルド",

    ping_title: "🏓 Pong!",
    pong: "ラウンドトリップ遅延: {} ms",

    whois_title: "👤 ユーザー情報",
    field_badges: "バッジ",
    badges_none: "なし",

    doctor_title: "🩺 設定の検査",
    doctor_not_configured: "このサーバーは設定に登録されていないため、検査する項目がありません。",
    ban_perm_ok: "必要な権限が付与されています: `Ban Members`",
    ban_perm_missing: "必要な権限が付与されていません: `Ban Members`",
    check_roles: "Honeypot に指定されているロール",
    check_channels: "Honeypot に指定されているチャンネル",
    log_channel_ok: "ログチャンネルが設定されています: {}",
    log_channel_missing: "ログチャンネルがサーバーに存在しません: {}",
    roles_missing: "次のロール ID がサーバーに存在しません: {}",
    channels_missing: "次のチャンネル ID がサーバーに存在しません: {}",
    role_order_ok: "すべての Honeypot ロールが Bot の固有ロールより下位にあります。",
    role_order_blocking: "次のロールが Bot の固有ロール以上の位置にあるため、ハニーポットが正しく発動しない可能性があります: {}",
    role_order_privileged: "次のロールが Bot の固有ロール以上の位置にあります。権限所有者のため問題はありませんが、想定されていない場合はロールの位置を調整してください: {}",
    ok_mark: "✅",
    warn_mark: "⚠️",
    info_mark: "ℹ️",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_language_is_english() {
        assert_eq!(Language::default(), Language::En);
    }

    #[test]
    fn deserialize_language_variants() {
        #[derive(Deserialize)]
        struct Wrapper {
            language: Language,
        }
        let en: Wrapper = toml::from_str(r#"language = "en""#).unwrap();
        assert_eq!(en.language, Language::En);
        let ja: Wrapper = toml::from_str(r#"language = "ja""#).unwrap();
        assert_eq!(ja.language, Language::Ja);
    }

    #[test]
    fn deserialize_rejects_unknown_language() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[allow(dead_code)]
            language: Language,
        }
        assert!(toml::from_str::<Wrapper>(r#"language = "fr""#).is_err());
    }

    #[test]
    fn catalogs_differ_between_languages() {
        // A representative spot check: the two catalogs must actually diverge,
        // guarding against a JA field accidentally left as its English copy.
        assert_ne!(
            Language::En.messages().ban_title,
            Language::Ja.messages().ban_title
        );
        assert_ne!(
            Language::En.messages().btn_unban,
            Language::Ja.messages().btn_unban
        );
        assert_ne!(
            Language::En.messages().cmd_help_desc,
            Language::Ja.messages().cmd_help_desc
        );
    }

    #[test]
    fn templates_carry_a_placeholder() {
        for msg in [&EN, &JA] {
            assert!(msg.new_account.contains("{}"));
            assert!(msg.unusual_dm_flagged.contains("{}"));
            assert!(msg.confirm_unban_prompt.contains("{}"));
            assert!(msg.confirm_ban_prompt.contains("{}"));
            assert!(msg.unbanned_ack.contains("{}"));
            assert!(msg.banned_ack.contains("{}"));
            assert!(msg.pong.contains("{}"));
            assert!(msg.log_channel_ok.contains("{}"));
            assert!(msg.log_channel_missing.contains("{}"));
            assert!(msg.roles_missing.contains("{}"));
            assert!(msg.channels_missing.contains("{}"));
            assert!(msg.role_order_blocking.contains("{}"));
            assert!(msg.role_order_privileged.contains("{}"));
        }
    }
}
