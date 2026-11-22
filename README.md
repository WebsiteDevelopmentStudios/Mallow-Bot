# Mallow Bot

Mallow Bot is being migrated to a Rust Discord runtime. Movie and media-hosting functionality is intentionally excluded and is being removed from the project.

## Runtime

- Rust 2021 crate: `rust-bot/`
- Serenity Discord gateway
- Axum HTTP service for health checks, authenticated interaction forwarding, and owner endpoints
- Dashboard runtime configuration from `https://mallowbot.kz.ci/api/runtime-config`
- Premium status from `https://mallowbot.kz.ci/api/premium-check`
- Lavalink v4 for voice playback, per-server queues, Spotify track metadata lookup, and synchronized LRCLIB lyrics
- Persistent local JSON state for XP, economy, swear counts, tickets, reaction roles, sticky messages, and moderation settings

## Retained features

- Music: play, join, leave, pause, resume, skip, stop, queue, volume, and synchronized lyrics
- Community: XP, levels, leaderboard, economy games, coin transfers, fun commands, swear counter, and swear leaderboard
- Server tools: welcome/goodbye messages, auto-role, reaction roles, rules verification, Bliss SMP announcements/events, tickets, purge, custom commands, vanity taglines, sticky messages, snipe, and server layout exports
- Moderation: Discord invite removal and configurable member timeouts
- Utility: ToS, uptime, updates, command list, YouTube metadata lookup, say, and sync
- Integrations: dashboard configuration, Premium checks, MCXboxBroadcast keep-alive, and owner guild management

The migration is still being validated. The production entry point will not be switched until Rust command/event parity and deployment checks are complete.

## Explicitly excluded

Mallow Bot no longer has a movie library or movie-hosting target. No movie search, movie channel linking, media downloads, FFmpeg/FFprobe, HLS playback, movie proxy routes, or movie commands are part of the Rust runtime.

## Build locally

Requirements:
- Rust stable toolchain
- Discord bot token
- A Lavalink v4 server if music playback is enabled

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
| `WAKE_SECRET` | Shared secret for authenticated interaction and owner bridge requests |
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

## Discord application and Cloudflare Worker

Enable the required privileged intents in the Discord Developer Portal:
- Server Members Intent
- Message Content Intent

The bot also needs permissions appropriate to the enabled features, including Manage Messages, Manage Roles, Manage Channels, Moderate Members, and voice connect/speak permissions.

For the Cloudflare interaction Worker, set:
- `DISCORD_PUBLIC_KEY`
- `WAKE_SECRET`
- `BOT_INTERACTION_URL` to the Rust service's `/discord/interactions` endpoint

The Rust service also exposes:
- `GET /health`
- `GET /owner/guilds` (requires `Authorization: Bot <DISCORD_TOKEN>`)
- `POST /owner/leave` (requires `Authorization: Bot <DISCORD_TOKEN>`)

## Render deployment

The repository's `render.yaml` uses `bot.Dockerfile` to build and run the Rust executable. The root `Dockerfile` and `lavalink/` directory are retained for the separate Lavalink service configuration.

## Data and dashboard boundary

The bot does not connect directly to Cloudflare D1. The dashboard remains a separate application; Rust reads its runtime settings and Premium status over HTTP. The unrelated D1 storage-usage reporting panel and API route were removed from the migration branch.

Local JSON state is written under `MALLOW_DATA_DIR`. Hosts with ephemeral filesystems will lose that state on restart unless a persistent disk or external storage is configured.
