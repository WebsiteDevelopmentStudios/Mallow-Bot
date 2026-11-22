# Full Rust migration checklist

Target: replace the Python Discord bot runtime with Rust. Movie/media-hosting functionality is intentionally excluded and is being removed, not ported.

## Production safety
- [x] Create isolated migration branch.
- [x] Keep the Python production entry point untouched while Rust parity work is incomplete.
- [x] Remove the unrelated Cloudflare D1 storage-usage endpoint and developer-dashboard panel.
- [x] Establish Rust executable, Discord gateway, health endpoint, dashboard config client, and premium-ban client.
- [ ] Port all retained slash commands and prefix commands.
- [ ] Port message/member/reaction/component event handlers and permission checks.
- [ ] Port Lavalink music playback, per-guild queues, Spotify metadata resolution, lyrics synchronization, and voice controls.
- [ ] Port MCXboxBroadcast keep-alive and all owner/developer HTTP endpoints.
- [ ] Port all retained dashboard integrations: welcome/leave cards, moderation, swear counter, XP, tickets, economy, premium, and guild settings.
- [ ] Port scheduled/background tasks and graceful startup/shutdown.
- [ ] Add parity tests for command names, options, permissions, guild restrictions, response visibility, and error handling.
- [ ] Verify Rust build and integration tests.
- [ ] Remove Python runtime and movie/media-hosting code only after retained functionality has Rust parity.
- [ ] Switch production hosting to the Rust executable after parity checks pass.

## Explicitly excluded
- Movie commands, movie search, movie channel linking, media file library, FFmpeg/FFprobe, HLS hosting/player, movie web routes, movie downloads, and movie expiration/cleanup tasks.
- No movie feature is to be ported into Rust.

## Retained functionality to migrate
- Music: Lavalink playback, queue, join/leave, pause/resume, skip/stop, volume, Spotify metadata lookup, lyrics synchronization, and Lavalink lifecycle.
- Utility: ToS, ping/uptime, updates, command list, say, sync, and prefix equivalents.
- Community: XP/levels/leaderboard, invite moderation/timeouts, welcome/goodbye cards, autorole, reaction roles, rules verification, Bliss SMP announcements/events, swear counting/leaderboards, economy games/balances, fun commands, premium commands, custom embeds, snipe/sticky, server backup, vanity, DM, YouTube enrichment, tickets, purge, and owner/developer controls.
- Background services: dashboard config cache, premium-ban checks, MCXboxBroadcast keep-alive, member/message processing, sticky maintenance, scheduled updates, Lavalink management, and lyrics tasks.

## Architecture
- `rust-bot/src/main.rs`: Discord gateway and interaction dispatch.
- `rust-bot/src/dashboard.rs`: existing dashboard runtime-config HTTP contract.
- `rust-bot/src/premium.rs`: existing premium-ban HTTP contract.
- `rust-bot/src/health.rs`: hosting health endpoint.
- Planned modules: `commands`, `events`, `community`, `music`, `dashboard_api`, `integrations`, and `tasks`.

## Data boundary
The Rust bot does not connect to Cloudflare D1. The existing dashboard remains hosted separately and configuration/premium checks continue through its HTTP API. The unrelated D1 storage-usage reporting feature is removed from the developer page and Pages API route.
