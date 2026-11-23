#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

# Build Rust on GitHub's runner, not inside the memory-limited game-hosting
# container. The workflow replaces this release asset whenever main changes.
BINARY_URL="https://github.com/WebsiteDevelopmentStudios/Mallow-Bot/releases/download/bot-hosting-latest/mallow-bot-linux-x86_64"
BINARY_PATH="$ROOT_DIR/mallow-bot"

if ! command -v curl >/dev/null 2>&1; then
  echo "[mallow] curl is required to download the precompiled bot executable."
  exit 1
fi

echo "[mallow] Downloading the precompiled Linux bot..."
if ! curl --fail --location --silent --show-error --retry 5 --retry-delay 10 "$BINARY_URL" --output "$BINARY_PATH"; then
  echo "[mallow] The precompiled executable is not available yet."
  echo "[mallow] Check the GitHub Actions workflow 'Build bot-hosting binary' and retry after it succeeds."
  exit 1
fi

chmod +x "$BINARY_PATH"
echo "[mallow] Starting Mallow Bot..."
exec "$BINARY_PATH"
