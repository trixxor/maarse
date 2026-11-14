#!/usr/bin/env sh
# Bygger (første gang) og starter maarse-host. Ekstra valg gives videre, fx:
#   ./scripts/start-host.sh --peer 192.168.1.20:4242
set -e
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust mangler. Installér det fra https://rustup.rs og kør scriptet igen."
  exit 1
fi
cargo run --release -p maarse-host -- "$@"
