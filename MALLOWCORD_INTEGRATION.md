# MallowCord owner panel integration

The MallowCord owner page is at /mallowcord.html and is linked from the existing developer panel. It intentionally calls same-origin Mallow-Bot backend routes rather than calling Cloudflare directly, so the Cloudflare service key never reaches the browser.

The Mallow-Bot server-side API must implement these owner-only routes:

- GET /api/mallowcord/users
- POST /api/mallowcord/users with { username, password }
- POST /api/mallowcord/users/{id}/timeout with { seconds }
- POST /api/mallowcord/users/{id}/ban
- POST /api/mallowcord/users/{id}/unban
- POST /api/mallowcord/users/{id}/delete

Each route must first reuse the existing Mallow-Bot Discord session and developer authorization check, and require is_owner=true. Then call the MallowCordAdminClient in mallowcord_admin.py. Never trust an owner ID supplied by the browser, and never expose MALLOWCORD_SERVICE_KEY to the page.

Render environment variables:
- MALLOWCORD_API_URL=https://YOUR-MALLOWCORD-DOMAIN/api
- MALLOWCORD_SERVICE_KEY=the same secret configured on the MallowCord Cloudflare Worker

This repository contains the dashboard page and server-side API client. The existing dashboard API handler that implements /api/me and /api/developers is hosted outside the tracked Mallow-Bot source tree, so these proxy routes must be added to that API host before the MallowCord management page can perform live operations. Do not weaken owner checks to make the page work.


## Render admin bridge
The dedicated Render bridge is deployed as the mochi-bot service at:
https://mochi-bot-57j2.onrender.com

It runs in MALLOWCORD_ADMIN_ONLY mode (no Discord bot token is needed for this isolated management bridge). Its protected routes are:
- GET/POST /owner/mallowcord/users
- POST /owner/mallowcord/users/{user_id}/{action}

The dashboard API host must validate the current Discord session and confirm the authenticated Discord ID is the configured Mallow-Bot owner before forwarding. Forward the request to the Render URL above and add the X-MallowCord-Admin-Secret header using the MALLOWCORD_ADMIN_SECRET Render environment value. Map the existing browser paths /api/mallowcord/users and /api/mallowcord/users/{id}/{action} to those Render routes. Do not expose the admin secret to browser JavaScript.

## Discord snapshot preload and D1 write avoidance

Mallow-Bot keeps a process-local preload of the configured Discord guild using discord.py's gateway cache. Gateway events remain the fast path for messages, member changes, and presence. In addition, the bridge performs a bounded recovery scan every five minutes by default:

- `MALLOWCORD_SNAPSHOT_SCAN_SECONDS` controls the scan interval (minimum 60 seconds; default 300).
- Channel/role/emoji/permission snapshots and stable 100-member batches are fingerprinted in Mallow-Bot before any sync request is sent.
- An unchanged fingerprint is skipped in the bot process, so that scan makes no request to the D1 sync Worker and causes zero D1 reads or writes for those unchanged snapshots.
- Member presence is sent through its own changed-status batch. Presence is excluded from member-directory fingerprints so one status change does not rewrite a complete member batch.
- A changed batch is sent and its fingerprint is remembered only after the API acknowledges it. Failed batches are retried on the next scan.
- After a Mallow-Bot restart, its in-memory fingerprints are empty; the first scan deliberately re-syncs the snapshot to recover from missed events while the bot was offline.

This is an optimization for unchanged Discord snapshots, not a claim that all MallowCord features are D1-free. User settings, messages, permissions, cold-start recovery, and changed Discord records still use their configured persistence path. The preload is process-local; it is not shared across multiple Mallow-Bot instances.

