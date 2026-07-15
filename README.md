# HoneyPot

[![CI](https://github.com/m1sk9/HoneyPot/actions/workflows/ci.yaml/badge.svg)](https://github.com/m1sk9/HoneyPot/actions/workflows/ci.yaml)
[![Release honeypot](https://github.com/m1sk9/HoneyPot/actions/workflows/release.yaml/badge.svg)](https://github.com/m1sk9/HoneyPot/actions/workflows/release.yaml)
[![Apache License 2.0](https://img.shields.io/github/license/m1sk9/HoneyPot?color=%239944ee)](https://github.com/m1sk9/HoneyPot/blob/main/LICENSE)
[![codecov](https://codecov.io/github/m1sk9/HoneyPot/graph/badge.svg)](https://codecov.io/github/m1sk9/HoneyPot)

A honeypot that automatically filters out spam bots.

```shell
# Latest Release
docker pull ghcr.io/m1sk9/honeypot:latest

# Minor Release
docker pull ghcr.io/m1sk9/honeypot:v0

# Specific Release
docker pull ghcr.io/m1sk9/honeypot:v0.1.0
```

[_API Support: requires Discord API v10_](https://discord.com/developers/docs/reference#api-versioning)

[HoneyPot API Documentation](https://honeypot.api.m1sk9.dev/)

## Features

- **Honeypot traps, not heuristics.** Instead of guessing whether an account is
  spam, HoneyPot bans on a deterministic signal: acquiring a designated trap
  **role** or posting in a designated trap **channel** — places a legitimate
  member has no reason to touch. Configure either or both per guild.
- **Account-type-aware policy.** A regular user who trips a trap is banned
  immediately, but bots are treated cautiously (they can only be added by an
  admin): a bot on the `trusted_bot_ids` allowlist is ignored, and any other bot
  is posted for **manual moderator review** rather than auto-banned.
- **Self-assign–aware role traps.** A role trap only fires when the member grants
  the role themselves (onboarding / self-assign) — the path a spam bot takes. A
  role granted by someone else (an admin, a reaction-role bot) or by an
  unverifiable grantor is held for **manual moderator review** instead, so a
  mistaken grant never triggers a false ban.
- **Rich, actionable log embeds.** Every action posts an embed to your log
  channel with the signals a moderator needs to judge it — account age (with a
  new-account warning), join date, default-vs-custom avatar, Discord's own spam
  flag, unusual-DM-activity flag, and, on a channel trigger, the offending
  message itself.
- **One-click moderation with a safety net.** Log embeds carry **Unban** /
  **Ban** buttons gated on the clicker's *Ban Members* permission, each guarded
  by a two-step ephemeral **Confirm / Cancel** prompt so a misclick never bans or
  unbans anyone. The original embed is rewritten in place to record who acted.
- **Per-guild localization.** Moderator-facing text (embeds, buttons, prompts)
  is available in **English** and **Japanese**, selected per guild via the
  `language` key. Translation gaps are caught at compile time, so no string ever
  ships untranslated.
- **Safe by construction.** Bans are de-duplicated per `(guild, user)` so an
  offender that trips two traps at once is banned only once, the bot never bans
  itself, and failed actions release their claim so a later event can retry.
- **Dry-run mode.** Set `HONEYPOT_DRY_RUN=1` to exercise the entire flow —
  detection, embeds, buttons, confirmations — without actually banning anyone,
  so you can validate your traps on your own account.

## Installation

HoneyPot is distributed as a Docker image.

- HoneyPot is tested on macOS and Linux (major distributions) as the recommended environment.
- Multi-architecture images are published for `linux/amd64` and `linux/arm64`.

Using Docker Compose is the recommended way to run HoneyPot. The repository
ships [`docker/compose.yaml`](./docker/compose.yaml):

```yaml
services:
  app:
    image: ghcr.io/m1sk9/honeypot:v0
    env_file:
      - ../.env
    volumes:
      - ../config/config.toml:/config/config.toml
    restart: always
```

Start the bot from the repository root:

```shell
docker compose -f docker/compose.yaml up -d
```

Paths in the compose file are relative to `docker/`, so `../` points at the
repository root where `.env` and `config/config.toml` live.

If you are using orchestration tools such as Kubernetes or Docker Swarm,
configure them according to their respective configuration files.

### Setup HoneyPot

HoneyPot is configured with numeric Discord IDs. To copy them, enable
**Developer Mode** in Discord (**User Settings** → **Advanced** → **Developer
Mode**). You can then right-click any server, role, or channel and choose **Copy
ID**. You will need:

- The **guild (server) ID**.
- The **honeypot role ID(s)** and/or **honeypot channel ID(s)** — the traps.
- The **log channel ID** — where ban notifications are posted.

> [!IMPORTANT]
> HoneyPot needs two privileged intents: **`GUILD_MEMBERS`** to observe role
> changes, and **`MESSAGE_CONTENT`** to record the offending message on a
> channel-triggered ban (so a moderator can confirm it was spam). In the
> [Discord Developer Portal](https://discord.com/developers/applications), open
> your application's **Bot** tab, enable both **Server Members Intent**
> (`GUILD_MEMBERS`) and **Message Content Intent** (`MESSAGE_CONTENT`) under
> **Privileged Gateway Intents**, and save — this is also where you copy the bot
> token for `HONEYPOT_BOT_TOKEN`. If either intent is left disabled the gateway
> refuses the connection and the bot will not start. The bot also needs the
> **Ban Members** and **View Audit Log** permissions in every guild it moderates.
> **View Audit Log** lets HoneyPot check who granted a honeypot role: without it,
> every role trigger is held for manual review instead of acting automatically
> (see below).

Create `config/config.toml` from [`config/config.example.toml`](./config/config.example.toml)
and fill in the IDs from the previous step. See [Configuration](#configuration)
for the full reference.

Create a `.env` file from [`.env.example`](./.env.example) and set
`HONEYPOT_BOT_TOKEN` to your bot token.

## Configuration

HoneyPot is configured with a TOML file. Each `[[guilds]]` block configures one
guild; declare multiple blocks to serve multiple guilds. See
[`config/config.example.toml`](./config/config.example.toml) for a complete
example.

```toml
[[guilds]]
guild_id             = 100000000000000000  # the guild (server) ID
honeypot_role_ids    = [200000000000000000] # roles that trigger a ban when acquired
honeypot_channel_ids = [300000000000000000] # channels that trigger a ban when posted in
trusted_bot_ids      = [500000000000000000] # bots exempt from the honeypot
log_channel_id       = 400000000000000000  # channel where ban notifications are sent
language             = "en"                 # moderator-facing language: "en" or "ja"
```

- `honeypot_role_ids`, `honeypot_channel_ids`, and `trusted_bot_ids` are
  optional and default to empty.
- `language` is optional and selects the language of this guild's
  moderator-facing text (log embeds and button responses): `"en"` (English) or
  `"ja"` (Japanese). It defaults to `"en"` when omitted. The Discord audit-log
  ban reason stays English regardless, so moderation logs remain consistent and
  searchable.
- IDs are TOML integers, so they must not have leading zeros. Discord snowflake
  IDs are 17–19 digit numbers and never start with `0`, so paste them as-is.

When a honeypot fires, a regular (non-bot) account is banned immediately. A bot
is handled more cautiously, since bots can only be added by an administrator:
a bot listed in `trusted_bot_ids` is ignored, and any other bot is posted to the
log channel with a **Ban** button for manual review instead of being
auto-banned. This keeps well-behaved bots — for example a link expander that
echoes a message into a honeypot channel — from being caught.

A **role** trigger has one extra safeguard. The trap is meant for a member who
acquires the role themselves — through Discord onboarding or a self-assign menu —
which is how a spam bot mechanically grabbing roles is caught. So HoneyPot checks
the audit log for who granted the role: only a **self-assign** fires the trap. If
the role was granted by someone else — an admin by hand, a reaction-role bot — or
by a grantor the audit log can't confirm (for example a missing **View Audit
Log** permission), the trap is **not** fired; instead the member is posted to the
log channel with a **Ban** button for manual review, so a mistaken grant never
causes an automatic ban. A **channel** trigger has no such check — posting is
always the offender's own act.

By default the configuration is read from `config/config.toml` (relative to the
working directory). Override the path with the `HONEYPOT_CONFIG_PATH`
environment variable.

## Environment Variables

`HONEYPOT_BOT_TOKEN` is the only variable required for startup.

| Key                    | Description                                                                     | Default              |
| ---------------------- | ------------------------------------------------------------------------------- | -------------------- |
| `HONEYPOT_BOT_TOKEN`   | Discord bot token. **Required.**                                                | —                    |
| `HONEYPOT_CONFIG_PATH` | Path to the guild configuration file.                                           | `config/config.toml` |
| `RUST_LOG`             | Log level filter (same syntax as `tracing`). Overrides the built-in default.    | `honeypot=info`      |
| `HONEYPOT_DRY_RUN`     | Simulate bans/unbans (`1`/`true`): run the full flow but skip the actual ban.   | off                  |
| `HONEYPOT_PREVIEW_CHANNEL` | Channel to post embed previews to. Only read in `--features preview` builds. | —                 |

When `HONEYPOT_DRY_RUN` is enabled, detection, log embeds, and buttons all run
normally, but no member is banned or unbanned — the log embed shows a `⚠️ DRY-RUN`
footer. This lets you debug the whole flow on your own account without needing a
throwaway account.

## Preview mode

Building with the `preview` feature turns the binary into a one-shot embed
previewer instead of the normal daemon: set `HONEYPOT_PREVIEW_CHANNEL` and run

```shell
cargo run --features preview
```

It posts one of every log embed (ban, untrusted-bot notice, unban, manual ban)
to that channel with sample data via REST — no gateway connection and no guild
config — then exits. Use it to check how the embeds render on a given client
(for instance the mobile app, which has no moderator view). The feature is off by
default, so none of the preview code or its sample data ships in the production
image.

For local development, copy [`.env.example`](./.env.example) to `.env`; it is
loaded automatically at startup. In production, supply these as real environment
variables instead.

## LICENSE

HoneyPot is published under the [Apache License 2.0](./LICENSE).

<sub>
    ® 2026 m1sk9
    <br/>
    HoneyPot is not affiliated with Discord.
</sub>
