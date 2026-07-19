//! `/doctor` — inspects the invoking guild's honeypot configuration.
//!
//! Reports whether the configured role/channel IDs actually exist in the guild,
//! whether the bot can ban, and — the subtle one — whether every honeypot role
//! sits below the bot's highest role. A honeypot role positioned at or above the
//! bot cannot have its holders banned, silently defeating the trap, so it is
//! called out explicitly.
//!
//! The pure checks ([`roles_at_or_above`], [`missing_ids`], [`effective_can_ban`],
//! [`highest_position`]) take plain data so they stay unit-testable without HTTP.

use crate::discord::commands;
use crate::discord::handler;
use crate::i18n::{Language, Messages};
use crate::settings::{GuildConfig, HoneyPotConfig};
use serenity::all::{
    ChannelId, CommandInteraction, Context, CreateEmbed, Mentionable, PartialGuild, Permissions,
    RoleId, UserId,
};

pub(super) async fn run(ctx: &Context, command: &CommandInteraction) {
    let language = commands::language_for(command.guild_id);
    let msg = language.messages();

    if !commands::has_manage_guild(command) {
        commands::respond_text(ctx, command, msg.cmd_perm_needed_manage).await;
        return;
    }

    let Some(guild_id) = command.guild_id else {
        commands::respond_text(ctx, command, msg.cmd_guild_only).await;
        return;
    };
    let Some(config) = HoneyPotConfig::get().guild(guild_id) else {
        let embed = CreateEmbed::new()
            .title(msg.doctor_title)
            .description(msg.doctor_not_configured);
        commands::respond_embed(ctx, command, embed).await;
        return;
    };
    let Some(bot_id) = handler::bot_user_id() else {
        tracing::warn!("doctor invoked before the bot user id was captured");
        return;
    };

    let guild = match guild_id.to_partial_guild(&ctx.http).await {
        Ok(guild) => guild,
        Err(error) => {
            tracing::error!(%error, %guild_id, "doctor: failed to fetch guild");
            return;
        }
    };
    let bot_roles = match guild_id.member(&ctx.http, bot_id).await {
        Ok(member) => member.roles,
        Err(error) => {
            tracing::error!(%error, %guild_id, "doctor: failed to fetch bot member");
            return;
        }
    };
    let channels = match guild_id.channels(&ctx.http).await {
        Ok(channels) => channels.into_keys().collect::<Vec<_>>(),
        Err(error) => {
            tracing::error!(%error, %guild_id, "doctor: failed to fetch channels");
            return;
        }
    };

    let findings = collect_findings(config, &guild, &bot_roles, &channels, bot_id);
    commands::respond_embed(ctx, command, build_embed(&findings, language)).await;
}

/// The outcome of every configuration check, extracted from the guild data.
///
/// Separating extraction from rendering keeps [`report`] pure and lets the embed
/// preview fabricate findings without a live guild (see [`crate::discord::preview`]).
pub(crate) struct Findings {
    /// Whether the bot can ban members in the guild.
    ban_ok: bool,
    /// Configured honeypot roles (shown as a count and mention list).
    honeypot_roles: Vec<RoleId>,
    /// Configured role IDs absent from the guild.
    missing_roles: Vec<RoleId>,
    /// Configured honeypot channels (shown as a count and mention list).
    honeypot_channels: Vec<ChannelId>,
    /// Configured channel IDs absent from the guild.
    missing_channels: Vec<ChannelId>,
    /// The configured log channel.
    log_channel: ChannelId,
    /// Whether the log channel exists in the guild.
    log_present: bool,
    /// Ordinary roles at or above the bot's role — these break the honeypot.
    offending_blocking: Vec<RoleId>,
    /// Privileged or bot roles at or above the bot's role — benign but noted.
    offending_privileged: Vec<RoleId>,
}

/// Extracts the check results from the fetched guild data (no HTTP, no I/O).
fn collect_findings(
    config: &GuildConfig,
    guild: &PartialGuild,
    bot_roles: &[RoleId],
    channels: &[ChannelId],
    bot_id: UserId,
) -> Findings {
    let everyone = RoleId::new(guild.id.get());
    let everyone_perms = role_permissions(guild, everyone);
    let bot_perms: Vec<Permissions> = bot_roles
        .iter()
        .map(|id| role_permissions(guild, *id))
        .collect();

    let present_roles: Vec<RoleId> = guild.roles.keys().copied().collect();

    let bot_highest = highest_position(&role_positions(guild, bot_roles));
    let honeypot_positions: Vec<(RoleId, u16)> = config
        .honeypot_role_ids
        .iter()
        .filter_map(|id| guild.roles.get(id).map(|role| (*id, role.position)))
        .collect();

    // Roles above the bot are split: a privileged/bot role sitting high is
    // expected (and can't be a real spam target), so it is a benign notice rather
    // than a honeypot-breaking warning.
    let (offending_privileged, offending_blocking): (Vec<RoleId>, Vec<RoleId>) =
        roles_at_or_above(bot_highest, &honeypot_positions)
            .into_iter()
            .partition(|id| {
                guild
                    .roles
                    .get(id)
                    .is_some_and(|role| is_privileged_role(role.permissions, role.managed))
            });

    Findings {
        ban_ok: effective_can_ban(everyone_perms, &bot_perms, guild.owner_id == bot_id),
        honeypot_roles: config.honeypot_role_ids.clone(),
        missing_roles: missing_ids(&config.honeypot_role_ids, &present_roles),
        honeypot_channels: config.honeypot_channel_ids.clone(),
        missing_channels: missing_ids(&config.honeypot_channel_ids, channels),
        log_channel: config.log_channel_id,
        log_present: channels.contains(&config.log_channel_id),
        offending_blocking,
        offending_privileged,
    }
}

/// A representative set of findings for the embed preview: a healthy config with
/// one honeypot role positioned above the bot, so both the passing checks and the
/// role-order warning are exercised.
#[cfg(feature = "preview")]
pub(crate) fn preview_findings() -> Findings {
    Findings {
        ban_ok: true,
        honeypot_roles: vec![
            RoleId::new(1_234_567_890_123_456_789),
            RoleId::new(1_234_567_890_123_456_790),
        ],
        missing_roles: Vec::new(),
        honeypot_channels: vec![ChannelId::new(2_234_567_890_123_456_789)],
        missing_channels: Vec::new(),
        log_channel: ChannelId::new(2_234_567_890_123_456_789),
        log_present: true,
        // One ordinary role (a real problem) and one privileged role (benign), so
        // both role-order notices render.
        offending_blocking: vec![RoleId::new(1_234_567_890_123_456_789)],
        offending_privileged: vec![RoleId::new(1_234_567_890_123_456_790)],
    }
}

/// Builds the `/doctor` embed for `language` from `findings`.
pub(crate) fn build_embed(findings: &Findings, language: Language) -> CreateEmbed {
    let msg = language.messages();
    CreateEmbed::new()
        .title(msg.doctor_title)
        .description(report(findings, msg))
}

/// Renders the multi-line check report (pure).
fn report(findings: &Findings, msg: &Messages) -> String {
    let mut lines = Vec::new();

    // Ban permission.
    if findings.ban_ok {
        lines.push(mark_line(msg.ok_mark, msg.ban_perm_ok));
    } else {
        lines.push(mark_line(msg.warn_mark, msg.ban_perm_missing));
    }

    // Honeypot roles: count and mentions, then any missing ids.
    lines.push(mark_line(
        msg.ok_mark,
        &labeled_ids(msg.check_roles, &findings.honeypot_roles),
    ));
    if !findings.missing_roles.is_empty() {
        lines.push(mark_line(
            msg.warn_mark,
            &msg.roles_missing
                .replace("{}", &join_ids(&findings.missing_roles)),
        ));
    }

    // Honeypot channels: count and mentions, then any missing ids.
    lines.push(mark_line(
        msg.ok_mark,
        &labeled_ids(msg.check_channels, &findings.honeypot_channels),
    ));
    if !findings.missing_channels.is_empty() {
        lines.push(mark_line(
            msg.warn_mark,
            &msg.channels_missing
                .replace("{}", &join_ids(&findings.missing_channels)),
        ));
    }

    // Log channel.
    let log_mention = findings.log_channel.mention().to_string();
    if findings.log_present {
        lines.push(mark_line(
            msg.ok_mark,
            &msg.log_channel_ok.replace("{}", &log_mention),
        ));
    } else {
        lines.push(mark_line(
            msg.warn_mark,
            &msg.log_channel_missing.replace("{}", &log_mention),
        ));
    }

    // Role order.
    if findings.offending_blocking.is_empty() && findings.offending_privileged.is_empty() {
        lines.push(mark_line(msg.ok_mark, msg.role_order_ok));
    }
    if !findings.offending_blocking.is_empty() {
        lines.push(mark_line(
            msg.warn_mark,
            &msg.role_order_blocking
                .replace("{}", &id_mentions(&findings.offending_blocking)),
        ));
    }
    if !findings.offending_privileged.is_empty() {
        lines.push(mark_line(
            msg.info_mark,
            &msg.role_order_privileged
                .replace("{}", &id_mentions(&findings.offending_privileged)),
        ));
    }

    lines.join("\n")
}

/// The permissions of role `id` in `guild`, or none when the role is absent.
fn role_permissions(guild: &PartialGuild, id: RoleId) -> Permissions {
    guild
        .roles
        .get(&id)
        .map(|role| role.permissions)
        .unwrap_or_else(Permissions::empty)
}

/// The positions of `role_ids` that exist in `guild`.
fn role_positions(guild: &PartialGuild, role_ids: &[RoleId]) -> Vec<u16> {
    role_ids
        .iter()
        .filter_map(|id| guild.roles.get(id).map(|role| role.position))
        .collect()
}

/// A line prefixed with `mark`.
fn mark_line(mark: &str, label: &str) -> String {
    format!("{mark} {label}")
}

/// Renders `label: count (mentions)`, or `label: 0` when there are none.
fn labeled_ids<T: Mentionable>(label: &str, ids: &[T]) -> String {
    if ids.is_empty() {
        format!("{label}: 0")
    } else {
        format!("{label}: {} ({})", ids.len(), id_mentions(ids))
    }
}

/// Renders ids as a comma-separated list of Discord mentions.
fn id_mentions<T: Mentionable>(ids: &[T]) -> String {
    ids.iter()
        .map(|id| id.mention().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Renders ids as a comma-separated, backtick-wrapped list of raw values, for
/// ids that no longer resolve to a mention (a role/channel that was deleted).
fn join_ids<T: std::fmt::Display>(ids: &[T]) -> String {
    ids.iter()
        .map(|id| format!("`{id}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether a role above the bot is a benign case: it holds elevated permissions
/// (Administrator or Manage Server) or is a managed (bot/integration) role, so
/// sitting high is expected rather than a honeypot misconfiguration.
fn is_privileged_role(permissions: Permissions, managed: bool) -> bool {
    permissions.administrator() || permissions.manage_guild() || managed
}

/// The highest role position among `positions`, or 0 (the `@everyone` floor)
/// when empty.
fn highest_position(positions: &[u16]) -> u16 {
    positions.iter().copied().max().unwrap_or(0)
}

/// The honeypot roles positioned at or above `bot_highest`.
///
/// A holder of such a role outranks the bot, so the bot cannot ban them — the
/// trap would fire but the ban would fail. Equal positions are treated as a
/// conflict because Discord's tie-break (by id) is not something to rely on.
fn roles_at_or_above(bot_highest: u16, honeypot: &[(RoleId, u16)]) -> Vec<RoleId> {
    honeypot
        .iter()
        .filter(|(_, position)| *position >= bot_highest)
        .map(|(id, _)| *id)
        .collect()
}

/// The configured ids absent from `present`.
fn missing_ids<T: PartialEq + Copy>(configured: &[T], present: &[T]) -> Vec<T> {
    configured
        .iter()
        .copied()
        .filter(|id| !present.contains(id))
        .collect()
}

/// Whether the bot can ban, from its effective permissions.
///
/// Guild owners and administrators can always ban; otherwise the union of the
/// `@everyone` permissions and the bot's role permissions must include Ban
/// Members. This mirrors Discord's guild-level permission resolution (channel
/// overwrites aside, which do not gate the guild-wide ban action).
fn effective_can_ban(
    everyone: Permissions,
    role_permissions: &[Permissions],
    is_owner: bool,
) -> bool {
    if is_owner {
        return true;
    }
    let effective = role_permissions
        .iter()
        .fold(everyone, |acc, perms| acc | *perms);
    effective.administrator() || effective.ban_members()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: u64, position: u16) -> (RoleId, u16) {
        (RoleId::new(id), position)
    }

    #[test]
    fn highest_position_defaults_to_zero_when_empty() {
        assert_eq!(highest_position(&[]), 0);
        assert_eq!(highest_position(&[3, 7, 1]), 7);
    }

    #[test]
    fn roles_at_or_above_flags_ties_and_higher() {
        let honeypot = [role(1, 4), role(2, 5), role(3, 6)];
        // Bot's highest is 5: role 1 (below) is fine; roles 2 (tie) and 3 (above)
        // are conflicts.
        let offending = roles_at_or_above(5, &honeypot);
        assert_eq!(offending, vec![RoleId::new(2), RoleId::new(3)]);
    }

    #[test]
    fn roles_at_or_above_empty_when_all_below() {
        let honeypot = [role(1, 1), role(2, 2)];
        assert!(roles_at_or_above(5, &honeypot).is_empty());
    }

    #[test]
    fn missing_ids_reports_only_absent() {
        let configured = [RoleId::new(1), RoleId::new(2), RoleId::new(3)];
        let present = [RoleId::new(2)];
        assert_eq!(
            missing_ids(&configured, &present),
            vec![RoleId::new(1), RoleId::new(3)]
        );
    }

    #[test]
    fn effective_can_ban_true_for_owner_regardless_of_permissions() {
        assert!(effective_can_ban(Permissions::empty(), &[], true));
    }

    #[test]
    fn effective_can_ban_true_via_administrator() {
        assert!(effective_can_ban(
            Permissions::empty(),
            &[Permissions::ADMINISTRATOR],
            false
        ));
    }

    #[test]
    fn effective_can_ban_true_via_ban_members_on_any_role() {
        assert!(effective_can_ban(
            Permissions::empty(),
            &[Permissions::empty(), Permissions::BAN_MEMBERS],
            false
        ));
    }

    #[test]
    fn effective_can_ban_false_without_the_permission() {
        assert!(!effective_can_ban(
            Permissions::SEND_MESSAGES,
            &[Permissions::KICK_MEMBERS],
            false
        ));
    }

    fn healthy_findings() -> Findings {
        Findings {
            ban_ok: true,
            honeypot_roles: vec![RoleId::new(1)],
            missing_roles: Vec::new(),
            honeypot_channels: vec![ChannelId::new(2)],
            missing_channels: Vec::new(),
            log_channel: ChannelId::new(3),
            log_present: true,
            offending_blocking: Vec::new(),
            offending_privileged: Vec::new(),
        }
    }

    #[test]
    fn report_has_no_warning_marker_when_all_checks_pass() {
        let msg = Language::En.messages();
        assert!(!report(&healthy_findings(), msg).contains(msg.warn_mark));
    }

    #[test]
    fn report_warns_on_a_blocking_role_above_the_bot() {
        let msg = Language::En.messages();
        let findings = Findings {
            offending_blocking: vec![RoleId::new(42)],
            ..healthy_findings()
        };
        let text = report(&findings, msg);
        assert!(text.contains(msg.warn_mark));
        assert!(text.contains("<@&42>"));
    }

    #[test]
    fn report_treats_a_privileged_role_above_the_bot_as_a_benign_notice() {
        let msg = Language::En.messages();
        let findings = Findings {
            offending_privileged: vec![RoleId::new(7)],
            ..healthy_findings()
        };
        let text = report(&findings, msg);
        assert!(text.contains(msg.info_mark));
        assert!(
            !text.contains(msg.warn_mark),
            "privileged role is not a warning"
        );
        assert!(text.contains("<@&7>"));
    }

    #[test]
    fn is_privileged_role_recognizes_permissions_and_managed_roles() {
        assert!(is_privileged_role(Permissions::ADMINISTRATOR, false));
        assert!(is_privileged_role(Permissions::MANAGE_GUILD, false));
        assert!(is_privileged_role(Permissions::empty(), true));
        assert!(!is_privileged_role(Permissions::SEND_MESSAGES, false));
    }

    #[test]
    fn report_warns_when_ban_permission_is_missing() {
        let msg = Language::En.messages();
        let findings = Findings {
            ban_ok: false,
            ..healthy_findings()
        };
        let text = report(&findings, msg);
        assert!(text.contains(msg.ban_perm_missing));
        assert!(text.contains(msg.warn_mark));
    }

    #[test]
    fn report_lists_missing_role_and_channel_ids() {
        let msg = Language::En.messages();
        let findings = Findings {
            missing_roles: vec![RoleId::new(9)],
            missing_channels: vec![ChannelId::new(8)],
            ..healthy_findings()
        };
        let text = report(&findings, msg);
        assert!(text.contains("`9`"));
        assert!(text.contains("`8`"));
        assert!(text.contains(msg.warn_mark));
    }

    #[test]
    fn report_warns_when_log_channel_is_absent() {
        let msg = Language::En.messages();
        let findings = Findings {
            log_present: false,
            ..healthy_findings()
        };
        // healthy_findings sets the log channel to id 3.
        assert!(report(&findings, msg).contains("<#3>"));
    }

    #[test]
    fn labeled_ids_renders_count_and_mentions_or_zero() {
        assert_eq!(labeled_ids("Roles", &[RoleId::new(1)]), "Roles: 1 (<@&1>)");
        assert_eq!(labeled_ids::<RoleId>("Roles", &[]), "Roles: 0");
    }

    #[test]
    fn join_ids_backtick_wraps_each_id() {
        assert_eq!(join_ids(&[RoleId::new(1), RoleId::new(2)]), "`1`, `2`");
    }

    #[test]
    fn build_embed_titles_and_describes_the_report() {
        let msg = Language::En.messages();
        let embed = build_embed(&healthy_findings(), Language::En);
        let value = serenity::json::to_value(embed).expect("embed serializes");
        assert_eq!(value["title"].as_str().unwrap(), msg.doctor_title);
        assert!(
            value["description"]
                .as_str()
                .unwrap()
                .contains(msg.ban_perm_ok)
        );
    }
}
