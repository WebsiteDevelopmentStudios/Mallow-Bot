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
- Rust stable toolchain (1.82 or newer)
- Discord bot token
- Lavalink v4 server when music playback is enabled

```sh
cd rust-bot
cargo build --release
```

## Run

From the repository root:

```sh
./start.sh
```

The startup script installs the Rust stable toolchain into the container user's home directory if Cargo is not already installed, compiles the source from GitHub, and starts the resulting Rust executable. No Python `requirements.txt` or manually uploaded executable is needed.

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

## bot-hosting.net deployment

The bot-hosting.net server pulls this repository from GitHub. It should build and run the Rust source directly; do not upload the GitHub Actions executable artifact.

Set the bot-hosting.net startup command to:

```sh
bash start.sh
```

The script:
1. Uses the Rust toolchain already installed on the container, or installs Rust stable under the container user's home directory if Cargo is missing.
2. Runs `cargo build --release` inside `rust-bot/`.
3. Starts `rust-bot/target/release/mallow-bot`.

Configure the environment variables listed above in the host panel. Set `MALLOW_DATA_DIR` to a writable persistent directory provided by bot-hosting.net. Keep the existing Lavalink URI/password if music playback should remain enabled.

The old Python startup command (`pip install ... requirements.txt && python3 ...`) must be replaced with `bash start.sh`. Rust dependencies are managed by `rust-bot/Cargo.toml`; they are fetched and compiled by Cargo, not pip. Do not start the old Python process alongside Rust using the same Discord token.

## Data persistence

The bot writes JSON state under `MALLOW_DATA_DIR`. The host must provide persistent storage if this data must survive container replacement or host restarts. The dashboard remains a separate application; the Rust bot reads settings and Premium status through its existing HTTP APIs.
