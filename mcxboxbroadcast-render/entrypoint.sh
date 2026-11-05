#!/bin/sh
set -eu
cd /opt/mcxboxbroadcast
: "${MCXB_SERVER_ADDRESS:=bliss1.atbp.fun}"
: "${MCXB_SERVER_PORT:=19205}"
: "${PORT:=10000}"
mkdir -p /opt/mcxboxbroadcast/cache
if [ ! -f config.yml ]; then
  cat > config.yml <<EOF
session:
  remote-address: "${MCXB_SERVER_ADDRESS}"
  remote-port: ${MCXB_SERVER_PORT}
  update-interval: 30
EOF
fi
python3 /opt/mcxboxbroadcast/backup.py --restore
python3 /opt/mcxboxbroadcast/health_server.py &
HEALTH_PID=$!
python3 /opt/mcxboxbroadcast/backup.py &
BACKUP_PID=$!
cleanup() { kill "$HEALTH_PID" "$BACKUP_PID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
java -Djava.awt.headless=true -jar MCXboxBroadcastStandalone.jar &
JAVA_PID=$!
wait "$JAVA_PID"
