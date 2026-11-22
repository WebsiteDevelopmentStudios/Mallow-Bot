# Mallow Bot

Mallow Bot is a Rust-based Discord bot for music, community engagement, moderation, and server utilities.

## Runtime

- Rust 2021 application in `rust-bot/`
- Serenity Discord Gateway client
- Axum HTTP service for health checks, signed interaction forwarding, and owner endpoints
- Dashboard runtime configuration from `https://mallowbot.kz.ci/api/runtime-config`
- Premium status from `https://mallowbot.kz.ci/api/premium-check`
- Lavalink v4 for voice playback, per-server queues, Spotify track metadata, and synchronized LRCLIB lyrics
- Persistent JSON state for XP, economy, swear counts, tickets, reaction roles, sticky messages, and moderation settings

## Features

- Music: play, join, leave, pause, resume, skip, stop, queue, volume, and synchronized lyrics
- Community: XP, levels, leaderboards, Mallow Coins, economy games, transfers, fun commands, swear counting, and swear leaderboards
- Server management: welcome and goodbye cards, automatic roles, reaction roles, rules verification, tickets, purge, backups, custom commands, vanity taglines, sticky messages, and snipe
- Moderation: Discord invite protection and configurable timeouts
- Utilities: ToS, uptime, updates, command list, YouTube metadata, custom embeds, announcements, events, and countdowns
- Integrations: dashboard configuration, Premium checks, MCXboxBroadcast keep-alive, and owner guild management

## Build

Requirements:
- Rust stable toolchain
- Discord bot token
- Lavalink v4 server when music playback is enabled

```sh
cd rust-bot
cargo build --release
```

Run:

```sh
DISCORD_TOKEN=your-token cargo run --release
```

The service listens on `PORT` or `SERVER_PORT` (default `25572`).

## Environment

| Variable | Purpose |
|---|---|
| `DISCORD_TOKEN` | Discord bot token |
| `WAKE_SECRET` | Shared secret for authenticated interaction forwarding |
| `MALLOW_DASHBOARD_CONFIG_SECRET` | Same secret configured as Cloudflare Pages `RUNTIME_CONFIG_SECRET` |
| `MALLOW_DASHBOARD_CONFIG_URL` | Optional dashboard runtime-config URL |
| `PREMIUM_CHECK_URL` | Optional Premium-check URL |
| `ENABLE_LAVALINK` | Set `true` to enable music playback |
| `LAVALINK_URI` | Lavalink base URL |
| `LAVALINK_PASSWORD` | Lavalink authorization password |
| `MCXBROADCAST_URL` | Optional MCXboxBroadcast service URL for keep-alive |
| `MCXBROADCAST_PING_INTERVAL` | Keep-alive interval in seconds (minimum 60) |
| `MALLOW_DATA_DIR` | Directory for local JSON state |
| `PORT` | HTTP service port supplied by the host |
| `RUST_LOG` | Rust tracing filter |

## Discord and interaction forwarding

Enable these privileged intents in the Discord Developer Portal:
- Server Members Intent
- Message Content Intent

Grant the bot the permissions required by enabled features, including Manage Messages, Manage Roles, Manage Channels, Moderate Members, and voice connect/speak permissions.

For the Cloudflare interaction Worker, configure:
- `DISCORD_PUBLIC_KEY`
- `WAKE_SECRET`
- `BOT_INTERACTION_URL` pointing to the Rust service's `/discord/interactions` endpoint

The Rust service exposes:
- `GET /health`
- `GET /owner/guilds` (requires `Authorization: Bot <DISCORD_TOKEN>`)
- `POST /owner/leave` (requires `Authorization: Bot <DISCORD_TOKEN>`)

## Deployment

The Render service uses `bot.Dockerfile` to build and run the Rust executable. The root `Dockerfile` and `application.yml` are for the separate Lavalink service.

## Data persistence

The bot writes JSON state under `MALLOW_DATA_DIR`. The host must provide persistent storage if this data must survive container replacement or host restarts. The dashboard remains a separate application; the Rust bot reads settings and Premium status through its existing HTTP APIs.
