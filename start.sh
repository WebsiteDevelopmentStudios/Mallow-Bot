#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$ROOT_DIR"

# The root Cargo workspace selects rust-bot as its default member.
# This works with bot-hosting.net's native Rust runtime and Cargo startup.
exec cargo run --release
