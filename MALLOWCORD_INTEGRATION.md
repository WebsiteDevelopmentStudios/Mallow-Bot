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
