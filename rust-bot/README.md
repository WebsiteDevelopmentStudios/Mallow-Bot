# Mallow Bot Rust Runtime

This crate is the Rust replacement for the Mallow Bot Discord runtime.

## Build

```sh
cargo build --release
```

## Required configuration

- `DISCORD_TOKEN`
- `WAKE_SECRET` for the authenticated Cloudflare interaction and owner bridge
- `MALLOW_DASHBOARD_CONFIG_SECRET` for dashboard runtime configuration

Optional:
- `MALLOW_DASHBOARD_CONFIG_URL`
- `PREMIUM_CHECK_URL`
- `ENABLE_LAVALINK`, `LAVALINK_URI`, and `LAVALINK_PASSWORD`
- `MCXBROADCAST_URL` and `MCXBROADCAST_PING_INTERVAL`
- `MALLOW_DATA_DIR`
- `PORT` or `SERVER_PORT`
- `RUST_LOG`

## HTTP routes

- `GET /health`
- `POST /discord/interactions` (requires `X-Mallow-Bot-Secret` or `X-Mochi-Bot-Secret` matching `WAKE_SECRET`)
- `GET /owner/guilds` (requires `Authorization: Bot <DISCORD_TOKEN>`)
- `POST /owner/leave` (requires `Authorization: Bot <DISCORD_TOKEN>`)

## Runtime modules

- `community.rs`: XP, economy, fun, swear counters, moderation settings, server tools, and persistent community state
- `music.rs`: Lavalink v4 WebSocket, voice-state synchronization, queues, Spotify metadata lookup, playback controls, and LRCLIB synchronized lyrics
- `dashboard.rs`: cached dashboard runtime configuration
- `premium.rs`: Premium and ban checks
- `tasks.rs`: MCXboxBroadcast/Lavalink keep-alives and rotating Discord status
- `halloween.rs`: persistent Halloween countdown messages and daily facts
- `health.rs`: HTTP health endpoint

The Rust migration must pass command/event parity checks and deployment validation before it replaces the production runtime.
