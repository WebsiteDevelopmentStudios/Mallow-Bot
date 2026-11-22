# Mallow Bot — Rust migration

This crate is the Rust replacement runtime. It deliberately does not use Cloudflare D1. The existing dashboard remains a separate Pages application and the bot reads its authenticated runtime configuration through the existing `/api/runtime-config` HTTP contract. Premium ban checks use the existing `/api/premium-check` contract.

## Current migration status

This is the first executable migration slice, not feature parity. It currently includes:
- Rust Discord gateway startup and slash-command registration for initial smoke tests
- HTTP health endpoint on `PORT` / `SERVER_PORT`
- Cached, bounded-time dashboard runtime-config client
- Cached, bounded-time premium-ban client
- Graceful fallback to defaults when dashboard/access services are unavailable

The production Python entry point is intentionally unchanged until the complete parity checklist in `MIGRATION.md` is implemented and validated.

## Build and run

```sh
cd rust-bot
cargo build --release
DISCORD_TOKEN=... MALLOW_DASHBOARD_CONFIG_SECRET=... cargo run --release
```

The dashboard secret must match the existing Cloudflare Pages `RUNTIME_CONFIG_SECRET`. No D1 credentials or D1 database ID are required by this Rust bot runtime.
