# Full Rust migration status

Target: one Rust Mallow Bot runtime. All movie and media-hosting functionality is excluded and has been removed from the migration branch.

## Completed on `migration/full-rust-bot`

- [x] Isolated migration branch; production `main` remains unchanged.
- [x] Rust executable, Serenity gateway, Axum health server, dashboard config client, and Premium client.
- [x] Authenticated HTTP interaction bridge for the Cloudflare/Vercel interaction forwarders.
- [x] Owner guild-list and leave endpoints with the existing Bot-token authorization contract.
- [x] Lavalink v4 WebSocket connection, Discord voice-state handoff, per-guild queue, playback controls, Spotify track metadata lookup, and LRCLIB synchronized lyrics.
- [x] MCXboxBroadcast and Lavalink keep-alive tasks.
- [x] Rotating custom Discord presence.
- [x] XP/levels/leaderboard, configurable XP cooldown/range, economy balances/rewards/games/transfers, fun commands, swear counter, adjustments, and swear leaderboard.
- [x] Prefix music and community command dispatch.
- [x] Invite-link deletion and configurable timeouts.
- [x] Dashboard-driven welcome/goodbye messages and 700×400 rendered member cards with avatar, display name, and member number.
- [x] Bliss SMP welcome, announcements, and events.
- [x] Auto-role, reaction roles, and rules verification.
- [x] Ticket channel creation/close, ticket ownership/support permission checks, and duplicate-ticket prevention.
- [x] Premium gating, vanity taglines, custom embeds, custom commands, snipe capture, sticky reposts, channel/role backup export, DM forwarding, purge, update log, YouTube enrichment, ToS, uptime, and command list.
- [x] Remove Python bot runtime modules and Python requirements from the migration branch.
- [x] Remove movie commands, movie search/hosting/proxy code, movie directory, FFmpeg/FFprobe/HLS dependencies, and movie-specific Would You Rather content by removing the legacy Python bot runtime.
- [x] Remove the unrelated D1 storage-usage endpoint and developer-dashboard panel.
- [x] Add Rust Render Docker deployment and Node.js Vercel interaction verification.

## Still required before production cutover

- [ ] Resolve all current Rust CI/build issues and pass `cargo check`, `cargo test`, release build, and production Docker build on the latest commit.
- [ ] Run a live Discord test for gateway startup, HTTP-forwarded slash commands, command options/permissions, reactions, member join/leave cards, tickets, and owner endpoints.
- [ ] Run a live Lavalink test for voice join, track search/play, queue advance, pause/resume, skip/stop/volume, Spotify links, and synchronized lyrics.
- [ ] Validate the dashboard runtime-config and Premium API contracts against the production Cloudflare Pages deployment.
- [ ] Verify Vercel and Cloudflare Worker forwarding secrets and endpoint URLs.
- [ ] Validate persistent storage behavior on the chosen host; Render's free filesystem is ephemeral.
- [ ] Finish parity gaps such as ticket-panel component buttons, full server backup fidelity, and any dashboard/owner controls not covered by the live test.
- [ ] Confirm all required Discord intents and permissions in the Developer Portal and bot role.
- [ ] Review deployment environment variables and perform a rollback rehearsal.
- [ ] Only after all checks pass, replace the production startup command with the Rust executable and merge to `main`.

## Explicit exclusion

No movie commands, movie search, movie channel linking, movie file library, movie downloads, movie web routes, movie proxy, FFmpeg/FFprobe, HLS playback/hosting, or movie cleanup tasks belong in Mallow Bot.

## Architecture

- `rust-bot/src/main.rs`: Discord gateway, interaction dispatch, HTTP bridge, owner API, and member/message events.
- `rust-bot/src/community.rs`: community commands, moderation, tickets, reaction roles, and persistent JSON state.
- `rust-bot/src/music.rs`: Lavalink v4, Discord voice session synchronization, queue, Spotify metadata, and synchronized lyrics.
- `rust-bot/src/member_cards.rs`: 700×400 welcome/goodbye image rendering.
- `rust-bot/src/dashboard.rs`: cached dashboard runtime-config HTTP contract.
- `rust-bot/src/premium.rs`: Premium and ban checks.
- `rust-bot/src/health.rs`: health/readiness endpoint.
- `rust-bot/src/tasks.rs`: keep-alives and rotating status.
- `api/index.js`: Discord signature verification and forwarding to the Rust interaction endpoint.
- `worker/src/index.js`: Cloudflare Worker interaction forwarding.

## Data boundary

The Rust bot does not connect directly to Cloudflare D1. Dashboard settings and Premium state continue through the existing HTTP APIs. The D1 storage-usage reporting feature remains removed.
