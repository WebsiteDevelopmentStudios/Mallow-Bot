# MCXboxBroadcast on Render

This folder contains the standalone MCXboxBroadcast service used by Render.

## Render service settings

- Runtime: Docker
- Dockerfile: `mcxboxbroadcast-render/Dockerfile`
- Docker context: repository root (`.`)
- Plan: Free
- Branch: `main`
- Environment:
  - `MCXB_SERVER_ADDRESS=bliss1.atbp.fun`
  - `MCXB_SERVER_PORT=19205`

The service exposes `/health` on Render's assigned `PORT`. Mallow-Bot pings
that endpoint every five minutes when its `MCXBROADCAST_URL` environment
variable is set.

## First login

After the first deployment, open the Render service logs. MCXboxBroadcast
prints a Microsoft device-login URL and code. Open
https://www.microsoft.com/link, enter the code, and complete sign-in. Add the
broadcast account as an Xbox friend on the account you use in Minecraft.

## Important free-plan limitation

Render Free does not provide a persistent disk. The Microsoft/Xbox auth cache
and generated config are stored on the container filesystem and can be lost
when Render replaces or restarts the instance. A restart may therefore require
Microsoft device login again. The bot's periodic HTTP pings help prevent idle
spin-down, but cannot prevent platform maintenance, deploys, crashes, or all
restarts.

The server address and port can be changed with the two environment variables
above. If you change them, redeploy the service.
