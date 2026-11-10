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
