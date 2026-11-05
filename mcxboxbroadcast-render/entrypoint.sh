#!/bin/sh
set -eu

cd /opt/mcxboxbroadcast

: "${MCXB_SERVER_ADDRESS:=bliss1.atbp.fun}"
: "${MCXB_SERVER_PORT:=19205}"
: "${PORT:=10000}"

# The free Render filesystem is ephemeral. Keep the working directory in the
# container and preserve auth/cache for as long as this instance stays alive.
mkdir -p /opt/mcxboxbroadcast/cache

# Generate a minimal standalone config on first boot. MCXboxBroadcast can
# populate any additional defaults when it loads this file.
if [ ! -f config.yml ]; then
  cat > config.yml <<EOF
session:
  remote-address: "${MCXB_SERVER_ADDRESS}"
  remote-port: ${MCXB_SERVER_PORT}
  update-interval: 30
EOF
fi

python3 /opt/mcxboxbroadcast/health_server.py &
HEALTH_PID=$!

cleanup() {
  kill "$HEALTH_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

exec java -Djava.awt.headless=true -jar MCXboxBroadcastStandalone.jar
