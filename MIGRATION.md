# Full Rust migration checklist

Target: replace the Python Discord bot runtime with Rust while retaining the current dashboard, HTTP contracts, Discord behavior, configuration semantics, and every enabled integration.

## Production safety

- [x] Create isolated `migration/full-rust-bot` branch.
- [x] Keep the Python production entry point and current hosting configuration untouched during parity work.
- [x] Remove the unrelated Cloudflare D1 storage-usage endpoint and developer-dashboard panel.
- [x] Establish a Rust executable, Discord gateway, health endpoint, dashboard config client, and premium-ban client.
- [ ] Port and test all commands, message handlers, member events, reactions, component interactions, scheduled jobs, and background tasks.
- [ ] Port the movie HTTP host and its FFmpeg/HLS lifecycle.
- [ ] Port Lavalink lifecycle, Wavelink queue behavior, synchronized lyrics, Spotify metadata resolution, and voice controls.
- [ ] Port MCXboxBroadcast keep-alive and all owner/developer HTTP endpoints.
- [ ] Port every dashboard feature contract, including welcome/leave cards, moderation, swear counter, XP, tickets, economy, premium, and guild settings.
- [ ] Verify exact command names, option types, permission checks, guild restrictions, ephemeral/public response behavior, and error handling.
- [ ] Add integration tests and run a complete command/event parity suite.
- [ ] Switch the production start command only after all checks pass.

## Existing Python feature inventory

The source of truth for parity is the current `main.py` and `community_features.py`. The existing bot includes:

- Movie channel linking, movie listing/search/link sharing, media hosting, FFmpeg/FFprobe HLS lifecycle, Cloudflare tunnel lifecycle, and cleanup.
- Lavalink music playback, queue, join/leave, pause/resume, skip/stop, volume, Spotify metadata lookup, lyrics synchronization, and startup/shutdown management.
- Utility commands: ToS, ping/uptime, updates, commands list, say, sync, and prefix equivalents.
- Community commands and events: XP/levels/leaderboard, invite moderation/timeouts, welcome/goodbye cards, autorole, reaction roles, rules verification, announcements/events for Bliss SMP, swear counting/leaderboards, economy games and balances, fun commands, premium commands, custom embeds, snipe/sticky, server backup, vanity, DM, YouTube enrichment, tickets, purge, and owner/developer controls.
- Background services: dashboard config cache, premium-ban checks, MCXboxBroadcast keep-alive, member/message event processing, sticky message maintenance, scheduled updates, Lavalink process management, lyrics tasks, and hosted movie expiration.

## Architecture

- `rust-bot/src/main.rs`: Discord gateway and interaction dispatch.
- `rust-bot/src/dashboard.rs`: existing dashboard runtime-config HTTP contract with bounded timeouts and caching.
- `rust-bot/src/premium.rs`: existing premium-ban HTTP contract with bounded timeouts and caching.
- `rust-bot/src/health.rs`: hosting health endpoint.
- Future modules should be separated by domain: `commands`, `events`, `community`, `music`, `movies`, `dashboard_api`, `storage`, and `tasks`.

## Data boundary

The Rust bot does not connect to Cloudflare D1. Dashboard configuration and premium checks remain HTTP integrations with the existing dashboard API. The unrelated D1 storage-usage reporting feature has been removed from the developer page and Pages API route.
