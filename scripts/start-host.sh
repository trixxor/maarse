#!/usr/bin/env sh
# Builds (the first time) and starts maarse-host. Extra options are passed on, e.g.:
#   ./scripts/start-host.sh --peer 192.168.1.20:4242
set -e
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is missing. Install it from https://rustup.rs and run the script again."
  exit 1
fi
cargo run --release -p maarse-host --bin maarse-host -- "$@"
