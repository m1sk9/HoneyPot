# HoneyPot

[![CI](https://github.com/m1sk9/HoneyPot/actions/workflows/ci.yaml/badge.svg)](https://github.com/m1sk9/HoneyPot/actions/workflows/ci.yaml)
[![Release honeypot](https://github.com/m1sk9/HoneyPot/actions/workflows/release.yaml/badge.svg)](https://github.com/m1sk9/HoneyPot/actions/workflows/release.yaml)
[![Apache License 2.0](https://img.shields.io/github/license/m1sk9/HoneyPot?color=%239944ee)](https://github.com/m1sk9/HoneyPot/blob/main/LICENSE)
[![codecov](https://codecov.io/github/m1sk9/HoneyPot/graph/badge.svg)](https://codecov.io/github/m1sk9/HoneyPot)

**HoneyPot** is a lightweight, fast Discord bot that automatically bans spam bots.

Users (and bots) that step into a configured *honeypot* — acquiring a honeypot
role or posting in a honeypot channel — are banned automatically, and a log
notification carrying an **Unban** button is posted to a log channel so a
moderator can reverse a false positive with one click.

```shell
# Latest Release
docker pull ghcr.io/m1sk9/honeypot:latest

# Minor Release
docker pull ghcr.io/m1sk9/honeypot:v0

# Specific Release
docker pull ghcr.io/m1sk9/honeypot:v0.1.0
```

[_API Support: requires Discord API v10_](https://discord.com/developers/docs/reference#api-versioning)

## Features

- **Honeypot roles**: acquiring a configured role triggers an immediate ban.
- **Honeypot channels**: posting in a configured channel triggers an immediate ban.
- **One-click unban**: every ban posts an embed with an `Unban` button to the log channel.
- **Multi-guild**: a single deployment serves any number of guilds via `[[guilds]]` blocks.
- **Lightweight**: built on [distroless](https://github.com/GoogleContainerTools/distroless) for a very small image.
- **Fast**: written in Rust.
- **OSS**: open-source under the Apache License 2.0; self-hostable.

On ban, up to one day of the offender's messages are deleted, and the bot never
bans its own account.

## Setup

HoneyPot is distributed as a Docker image. The steps below take you from an
empty Discord application to a running bot.

- HoneyPot is tested on macOS and Linux (major distributions) as the recommended environment.
- Multi-architecture images are published for `linux/amd64` and `linux/arm64`.

### 1. Create a Discord application and bot

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and click **New Application**.
2. Go to the **Bot** tab and click **Reset Token** to reveal the bot token. Copy it — this is your `HONEYPOT_BOT_TOKEN`. Keep it secret.

### 2. Enable the privileged intent

HoneyPot requires the **`GUILD_MEMBERS`** privileged intent to observe role
changes. Enable it before starting the bot:

1. In the **Bot** tab, scroll to **Privileged Gateway Intents**.
2. Enable **Server Members Intent** (`GUILD_MEMBERS`), then save.

The bot uses the following gateway intents: `GUILDS`, `GUILD_MEMBERS`,
`GUILD_MESSAGES`, and `GUILD_MODERATION`. `MESSAGE_CONTENT` is intentionally
**not** requested — only the fact that a message was posted matters, not its
content.

### 3. Invite the bot to your server

Generate an invite URL with the `bot` scope and the permissions HoneyPot needs:
**Ban Members** (to ban offenders), **View Channel** (to receive messages in
honeypot channels), and **Send Messages** + **Embed Links** (to post the log
notification).

Replace `YOUR_CLIENT_ID` with your application's **Client ID** (found on the
**OAuth2** tab). `permissions=19460` bundles the four permissions above.

```
https://discord.com/oauth2/authorize?client_id=YOUR_CLIENT_ID&permissions=19460&scope=bot
```

Open the URL, pick your server, and authorize.

### 4. Collect the guild, role, and channel IDs

HoneyPot is configured with numeric Discord IDs. To copy them, enable
**Developer Mode** in Discord (**User Settings** → **Advanced** → **Developer
Mode**). You can then right-click any server, role, or channel and choose **Copy
ID**. You will need:

- The **guild (server) ID**.
- The **honeypot role ID(s)** and/or **honeypot channel ID(s)** — the traps.
- The **log channel ID** — where ban notifications are posted.

### 5. Configure HoneyPot

Create `config/config.toml` from [`config/config.example.toml`](./config/config.example.toml)
and fill in the IDs from the previous step. See [Configuration](#configuration)
for the full reference.

Create a `.env` file from [`.env.example`](./.env.example) and set
`HONEYPOT_BOT_TOKEN` to the token from step 1.

### 6. Run with Docker Compose

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
log_channel_id       = 400000000000000000  # channel where ban notifications are sent
```

- `honeypot_role_ids` and `honeypot_channel_ids` are optional and default to empty.
- IDs are TOML integers, so they must not have leading zeros. Discord snowflake
  IDs are 17–19 digit numbers and never start with `0`, so paste them as-is.

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
