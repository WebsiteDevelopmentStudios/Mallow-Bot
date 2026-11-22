# Mallow Bot service architecture

Mallow Bot runs as a Rust Discord gateway application hosted on Render. The Cloudflare Worker verifies Discord interaction signatures, acknowledges interactions, and forwards signed interaction payloads to the authenticated Rust HTTP bridge.

## Flow

Discord -> Cloudflare Worker -> Render Rust service -> Discord API

The Rust service maintains the Discord Gateway connection, handles slash commands and message events, and exposes the health, interaction, and owner-management endpoints.

## Secrets

Cloudflare Worker:
- DISCORD_PUBLIC_KEY
- BOT_INTERACTION_URL
- WAKE_SECRET

Render:
- DISCORD_TOKEN
- WAKE_SECRET
- MALLOW_DASHBOARD_CONFIG_SECRET
- LAVALINK_URI and LAVALINK_PASSWORD when music is enabled

WAKE_SECRET must match on both sides. The dashboard remains a separate application; the Rust bot reads per-server settings and Premium status through its HTTP API.

## Runtime

The Rust service is the production bot runtime. Lavalink remains an external service for voice playback. MCXboxBroadcast keep-alive is an optional independent integration. The bot uses its configured persistent data directory for community state.
