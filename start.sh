#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

# bot-hosting.net may provide only the Python runtime by default.
# Install the Rust toolchain into the container user's home if Cargo is absent.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

if ! command -v cargo >/dev/null 2>&1; then
  if ! command -v curl >/dev/null 2>&1; then
    echo "ERROR: curl is required to install the Rust toolchain." >&2
    exit 1
  fi
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
fi

# Make Cargo available in this startup shell.
if [ -f "$CARGO_HOME/env" ]; then
  # shellcheck disable=SC1091
  . "$CARGO_HOME/env"
fi

cd "$ROOT_DIR/rust-bot"
cargo build --release

# Replace the bootstrap shell with the Rust bot process.
exec "$ROOT_DIR/rust-bot/target/release/mallow-bot"
