# Mallow Bot — Rust runtime migration

This crate is the Rust replacement runtime for Mallow Bot. Movie/media-hosting features are explicitly excluded and will not be ported. The runtime does not connect to Cloudflare D1. The existing dashboard remains a separate Pages application; the bot reads its runtime configuration through `/api/runtime-config` and checks premium bans through `/api/premium-check`.

## Current status

This is an initial executable migration slice, not feature parity. It includes:
- Rust Discord gateway startup and initial smoke-test slash commands
- HTTP health endpoint on `PORT` / `SERVER_PORT`
- Cached dashboard runtime-config client with bounded request timeouts
- Cached premium-ban client with bounded request timeouts
- Graceful fallback when dashboard/access services are unavailable

The production Python entry point remains unchanged until every retained feature has been ported and tested. See the repository-root `MIGRATION.md` for the full parity checklist.

## Build

```sh
cd rust-bot
cargo build --release
```

Required environment:
- `DISCORD_TOKEN`
- `MALLOW_DASHBOARD_CONFIG_SECRET` (same value as Cloudflare Pages `RUNTIME_CONFIG_SECRET`)

Optional:
- `MALLOW_DASHBOARD_CONFIG_URL` (defaults to `https://mallowbot.kz.ci/api/runtime-config`)
- `PREMIUM_CHECK_URL` (defaults to `https://mallowbot.kz.ci/api/premium-check`)
- `PORT` or `SERVER_PORT` (defaults to 25572)
- `RUST_LOG`
