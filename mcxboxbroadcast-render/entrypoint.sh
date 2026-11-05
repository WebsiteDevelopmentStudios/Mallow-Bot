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
  remote-port: "${MCXB_SERVER_PORT}"
  update-interval: 30
EOF
fi
python3 /opt/mcxboxbroadcast/backup.py --restore
python3 - <<'PY'
from pathlib import Path
import os
import re

p = Path("/opt/mcxboxbroadcast/config.yml")
s = p.read_text()
# Restrict edits to the session block so unrelated settings remain untouched.
m = re.search(r"(?ms)^session:\s*\n(.*?)(?=^[^ \t#][^\n]*:|\Z)", s)
if not m:
    raise SystemExit("config.yml has no session block")
block = m.group(1)
for key, value in (("remote-address", os.environ.get("MCXB_SERVER_ADDRESS", "bliss1.atbp.fun")),
                   ("remote-port", os.environ.get("MCXB_SERVER_PORT", "19205"))):
    line = re.compile(rf"(?m)^(  {re.escape(key)}:\s*).*$")
    replacement = rf'\g<1>"{value}"' if key == "remote-address" else rf"\g<1>{value}"
    if line.search(block):
        block = line.sub(replacement, block, count=1)
    else:
        block += f'  {key}: "{value}"\n' if key == "remote-address" else f"  {key}: {value}\n"
s = s[:m.start(1)] + block + s[m.end(1):]
p.write_text(s)
print("Effective session configuration:")
print(block)
PY
# A restored config can contain the standalone defaults (127.0.0.1:19132).
# Always apply the Render service's configured backend after restoring state.
python3 - <<'PY'
import os
import re
from pathlib import Path

path = Path("/opt/mcxboxbroadcast/config.yml")
content = path.read_text(encoding="utf-8") if path.exists() else "session:\n"
values = {
    "remote-address": os.environ.get("MCXB_SERVER_ADDRESS", "bliss1.atbp.fun"),
    "remote-port": os.environ.get("MCXB_SERVER_PORT", "19205"),
}
for key, value in values.items():
    pattern = re.compile(rf"(?m)^(\s*{re.escape(key)}:\s*).*$")
    replacement = rf'\g<1>"{value}"' if key == "remote-address" else rf"\g<1>{value}"
    if pattern.search(content):
        content = pattern.sub(replacement, content, count=1)
    else:
        content += (f'  {key}: "{value}"\n' if key == "remote-address" else f"  {key}: {value}\n")
path.write_text(content, encoding="utf-8")
print(f"Configured backend: {values['remote-address']}:{values['remote-port']}")
PY
python3 /opt/mcxboxbroadcast/health_server.py &
HEALTH_PID=$!
python3 /opt/mcxboxbroadcast/backup.py &
BACKUP_PID=$!
cleanup() { kill "$HEALTH_PID" "$BACKUP_PID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
java -Djava.awt.headless=true -jar MCXboxBroadcastStandalone.jar &
JAVA_PID=$!
wait "$JAVA_PID"
