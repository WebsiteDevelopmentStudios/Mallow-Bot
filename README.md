# Mallow Bot

Mallow Bot is a Rust-based Discord bot for music, community engagement, moderation, and server utilities.

## Runtime

- Rust 2021 application in `rust-bot/`, exposed through a root Cargo workspace
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

From the repository root:

```sh
cargo build --release
```

The root `Cargo.toml` is a workspace manifest whose default member is `rust-bot/`. This lets hosting panels run Cargo from the repository root without needing a custom working directory.

## Run

From the repository root:

```sh
cargo run --release
```

Or use:

```sh
bash start.sh
```

No Python `requirements.txt`, Python entry file, or manually uploaded executable is needed.

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

The bot-hosting.net server pulls this repository from GitHub. Select the **Rust** runtime in the panel. Its provided startup command detects the root `Cargo.toml` and runs `cargo run --release`; the root workspace automatically selects `rust-bot/` as the default member.

The host's Cargo command also generates a root `Cargo.lock` when one is not present. Keeping the workspace manifest at the repository root allows the host's dependency detection to inspect the Rust project's lockfile and install any required system packages from `Aptfile`.

If entering a custom start command, use:

```sh
cargo run --release
```

Configure the environment variables listed above in the host panel. Set `MALLOW_DATA_DIR` to a writable persistent directory provided by bot-hosting.net. Keep the existing Lavalink URI/password if music playback should remain enabled.

Do not use the old Python startup command (`pip install ... requirements.txt && python3 ...`) or run the old Python process alongside Rust using the same Discord token. Rust dependencies are managed by Cargo, not pip.

## Data persistence

The bot writes JSON state under `MALLOW_DATA_DIR`. The host must provide persistent storage if this data must survive container replacement or host restarts. The dashboard remains a separate application; the Rust bot reads settings and Premium status through its existing HTTP APIs.
