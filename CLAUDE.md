# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

HoneyPot is a Discord bot (Rust, `serenity`) that automatically bans spam bots.
It watches for "honeypot" traps — configured roles that trigger a ban when
acquired, and channels that trigger a ban when posted in — and acts on offenders.
It is distributed as a Docker image; there is no CLI beyond the running daemon.

## Commands

```shell
cargo build                                   # debug build
cargo build --release --bin honeypot          # release binary (as Docker builds it)
cargo run                                      # run locally (loads .env, reads config/config.toml)
cargo test                                     # all tests
cargo test --verbose deserialize_single_guild # a single test by name
cargo fmt --all -- --check                     # rustfmt check (CI gate)
cargo clippy --all-targets --all-features      # clippy (CI gate)
cargo llvm-cov --all-features --workspace      # coverage (matches CI; needs cargo-llvm-cov)
```

CI (`.github/workflows/ci.yaml`) gates on rustfmt, clippy, `cargo test`, and
coverage, then cross-builds for Linux (gnu/musl), macOS (x86_64/aarch64), and
Windows. The toolchain is pinned to stable via `rust-toolchain.toml`; the crate
uses **edition 2024**.

## Running locally

Requires a real Discord bot token and a live gateway connection.

1. Copy `.env.example` to `.env` and set `HONEYPOT_BOT_TOKEN`.
2. Copy `config/config.example.toml` to `config/config.toml` and fill in guild IDs.
3. `cargo run`.

The bot needs the **`GUILD_MEMBERS`** privileged intent (enabled in the Discord
Developer Portal) and the **Ban Members** permission in every moderated guild.
`MESSAGE_CONTENT` is intentionally *not* requested — only the fact that a message
was posted matters, never its content. Do not add it.

## Architecture

Data flows in one direction: TOML file → raw config → runtime settings → event
handlers → ban module → Discord.

- **`src/config.rs`** — raw `config.toml` representation. IDs are bare `u64`
  here (`GuildConfigEntry`). Owns file loading/parsing only.
- **`src/settings.rs`** — converts raw entries into `serenity` ID types and
  stores them in a global `OnceLock<HoneyPotConfig>` keyed by `GuildId` for O(1)
  lookup. `HoneyPotConfig::init()` is called once from `main`; `::get()` panics
  if called before init. Config path defaults to `config/config.toml`, overridable
  via `HONEYPOT_CONFIG_PATH`.
- **`src/main.rs`** — bootstraps `.env` (missing file is not an error), JSON
  `tracing` logging (`RUST_LOG`, default `honeypot=info`), config, and the
  serenity client with the four gateway intents.
- **`src/discord/handler.rs`** — the `EventHandler`. Captures the bot's own
  user ID on `ready` (guards against self-bans via `is_self`). `guild_member_update`
  and `message` detect triggers and funnel through `act_on_trigger`, which routes
  by account type (see below). `interaction_create` dispatches button clicks.
- **`src/discord/ban.rs`** — shared ban execution and the pure, unit-tested
  detection predicates (`newly_acquired_honeypot_role`, `is_honeypot_channel`).
  Builds all log embeds and encodes/parses button `custom_id`s.
- **`src/discord/interaction.rs`** — handles the log-embed buttons (`Unban`,
  and `Ban` for manual bot confirmation), gated on the clicker's `BAN_MEMBERS`
  permission. Each button opens a two-step ephemeral confirmation
  (`Confirm`/`Cancel`) — only the `Confirm` click actually bans/unbans and
  rewrites the original log embed.
- **`src/error.rs`** — `HoneyPotError`, the single error type and `main`'s return.

### Account-type policy (the core rule)

When a honeypot fires, `act_on_trigger` branches on the offender:

- **Non-bot user** → banned immediately (`execute_ban`).
- **Bot in `trusted_bot_ids`** → ignored entirely (e.g. a link-expander that
  legitimately echoes into a honeypot channel).
- **Any other bot** → *not* auto-banned. A notice with a `Ban` button is posted
  to the log channel for manual moderator review (`execute_suspicious_bot_notice`
  → `confirm_bot_ban`). The rationale: bots can only be added by an admin, so
  err on the side of caution.

### Two invariants worth knowing

- **The serenity cache is disabled.** So `guild_member_update` fires for a
  honeypot-role holder on *any* member update (nickname, timeout, …), and the
  "old roles" set is usually unavailable. `newly_acquired_honeypot_role` handles
  both: it diffs against old roles when present, otherwise falls back to
  intersecting the full new role set with the honeypots. When editing role
  detection, keep both paths correct.
- **`HANDLED_BANS` deduplicates by `(guild, user)`.** An offender can trip both
  honeypot paths at once, or `guild_member_update` can fire repeatedly. Every ban
  path *claims* the pair first (`claim_ban`) and acts only on the first claim. A
  *failed* ban/notice releases the claim (`forget_ban`) so a later event retries;
  a successful unban also releases it so the user can be re-banned. Preserve this
  claim/release discipline in any new ban path.

## Conventions

- **Fire-and-forget event handlers.** Gateway handlers log errors via `tracing`
  and swallow them rather than propagating — a failed log post must not undo a
  successful ban. Match this style.
- **Detection logic stays pure and HTTP-free** so it remains unit-testable; tests
  live in `#[cfg(test)] mod tests` at the bottom of each module. Add tests there
  when touching predicates or `custom_id` encoding.
- **Button `custom_id`s** are `uhp_unban:{user_id}` / `uhp_ban:{user_id}` (the
  log-embed buttons), `uhp_unban_do:{user_id}:{message_id}` /
  `uhp_ban_do:{user_id}:{message_id}` (the ephemeral `Confirm` buttons, which
  carry the log message id so it can be edited in place), and `uhp_cancel`. The
  prefixes deliberately do not collide; there are tests asserting this.
- **User-controlled text** (e.g. usernames in embeds) is sanitized — see
  `target_field`, which neutralizes backticks to prevent embed-layout spoofing.

## Releases

Automated by **release-please** (`release-please-config.json`, release-type
`rust`). Commits must follow **Conventional Commits**; `feat`/`fix`/`chore`/`ci`
map to changelog sections. Merging the release PR bumps `Cargo.toml`, updates
`CHANGELOG.md`, tags, and triggers the Docker image publish to
`ghcr.io/m1sk9/honeypot`. Do not hand-edit the version or changelog.

Rustdoc for `main` is published to GitHub Pages (`.github/workflows/docs.yaml`,
built with nightly `--document-private-items`) — module-level `//!` docs are the
primary architecture reference and are kept thorough.
