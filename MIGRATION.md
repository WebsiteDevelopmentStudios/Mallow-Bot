# Rust migration status

Target: one Rust Mallow Bot runtime hosted on bot-hosting.net. Production `main` remains unchanged until the Rust executable is validated and ready for cutover.

## Implemented on `migration/full-rust-bot`

- [x] Isolated migration branch.
- [x] Rust executable using Serenity for Discord and Axum for HTTP.
- [x] Authenticated HTTP interaction bridge for the Vercel and Cloudflare forwarders.
- [x] Owner guild-list and leave endpoints using the existing Bot-token authorization contract.
- [x] Dashboard runtime configuration and Premium/ban checks over HTTP.
- [x] Lavalink v4 WebSocket client, Discord voice-state handoff, per-guild queues, playback controls, Spotify metadata lookup, and synchronized LRCLIB lyrics.
- [x] MCXboxBroadcast and Lavalink keep-alive tasks.
- [x] Rotating custom Discord presence.
- [x] XP, levels, leaderboards, configurable XP cooldown/range, Mallow Coins, economy games, transfers, and daily rewards.
- [x] Fun commands and randomized direct-mention replies.
- [x] Swear counter with Unicode/obfuscation normalization, non-overlapping phrase counting, developer adjustments, and leaderboards.
- [x] Prefix music and community command dispatch.
- [x] Discord invite protection and configurable timeouts.
- [x] Dashboard-driven welcome/goodbye messages and 700×400 rendered member cards with avatar, display name, and member number.
- [x] Bliss SMP welcome, announcements, and events.
- [x] Automatic roles, reaction roles, and rules verification.
- [x] Support ticket creation, persistent ticket panel buttons, duplicate-ticket prevention, and owner/support close permissions.
- [x] Premium gating, vanity taglines, custom embeds, custom commands, snipe capture, sticky reposts, server layout export, DM forwarding, purge, update log, YouTube enrichment, ToS, uptime, command list, and command synchronization.
- [x] Persistent Halloween countdown messages with daily general Halloween facts.
- [x] Vercel interaction verification and Cloudflare Worker forwarding.
- [x] Remove the legacy Python bot runtime and its Python-only dependency manifest.
- [x] Remove the obsolete dashboard feature controls and API settings for retired functionality.

## Remaining validation before production cutover

- [ ] Pass Rust CI on the latest commit: formatting, `cargo check`, `cargo test`, release build, production Docker image build, and bot-hosting.net Linux binary artifact.
- [ ] Download and upload the Linux x86_64 executable artifact to bot-hosting.net; verify the server's Linux architecture and runtime compatibility.
- [ ] Configure the bot-hosting.net startup command and environment variables without changing the active Python startup prematurely.
- [ ] Confirm a persistent writable data directory for `MALLOW_DATA_DIR`.
- [ ] Run live Discord tests for gateway startup, Vercel/Worker forwarded slash commands, command options and permissions, reactions, member join/leave cards, ticket buttons, and owner endpoints.
- [ ] Run live Lavalink tests for voice join, search/play, queue advance, pause/resume, skip/stop/volume, Spotify links, and synchronized lyrics.
- [ ] Validate dashboard runtime-config and Premium API contracts against the deployed dashboard.
- [ ] Verify Vercel and Cloudflare Worker secrets and endpoint URLs.
- [ ] Review Discord intents and permissions in the Developer Portal and bot role.
- [ ] Review deployment variables and perform a rollback rehearsal.
- [ ] Only after these checks pass, replace the production startup with the Rust executable and merge to `main`.

## Architecture

- `rust-bot/src/main.rs`: Discord gateway, interaction dispatch, HTTP bridge, owner API, and member/message events.
- `rust-bot/src/community.rs`: community commands, moderation, tickets, reaction roles, and persistent JSON state.
- `rust-bot/src/music.rs`: Lavalink v4, Discord voice session synchronization, queue, Spotify metadata, and synchronized lyrics.
- `rust-bot/src/member_cards.rs`: 700×400 welcome/goodbye image rendering.
- `rust-bot/src/halloween.rs`: persistent countdown messages and daily facts.
- `rust-bot/src/dashboard.rs`: cached dashboard runtime-config HTTP contract.
- `rust-bot/src/premium.rs`: Premium and ban checks.
- `rust-bot/src/health.rs`: health/readiness endpoint.
- `rust-bot/src/tasks.rs`: keep-alives and rotating status.
- `api/index.js`: Vercel Discord signature verification and interaction forwarding.
- `worker/src/index.js`: Cloudflare Worker Discord signature verification and interaction forwarding.

## Data boundary

The bot reads dashboard settings and Premium state through the existing HTTP APIs. It does not access the dashboard database directly. Local community state is stored as JSON under `MALLOW_DATA_DIR`.
