#!/usr/bin/env sh
# ======================================================================
#  Starts maarse-server: the dedicated host for a server or VPS
#  (backbone, relay for hosts behind NAT, post office copies and the
#  telemetry dashboard with admin login). See docs/GUIDE.md,
#  "Dedicated server (maarse-server)". As a service instead:
#  packaging/systemd/maarse-server.service.
#
#  Fill in the settings below and run the file. Extra options on the
#  command line are passed on, e.g.:  ./start-server.sh --min-peers 8
# ======================================================================

# ---- Required ---------------------------------------------------------
# The address and port others reach this server on (public IP or DNS name).
PUBLIC_ADDR="server.example.org:4242"

# ---- Network ----------------------------------------------------------
LISTEN="0.0.0.0:4242"     # must match the port in PUBLIC_ADDR
PEERS=""                  # other servers/hosts, separated by spaces
SEED_DNS=""               # a DNS name with several servers behind it
MIN_PEERS=6               # keep at least this many backbone links

# ---- Regions (empty = forward everything) -----------------------------
REGIONS=""                # e.g. "dk nordjylland"
STRICT="no"               # yes: without regions only messages without a region

# ---- Dashboard --------------------------------------------------------
HTTP="0.0.0.0:8080"       # 127.0.0.1:8080 = only this machine / SSH tunnel
ADMIN_PASSWORD=""         # set a new admin password at start (empty = keep)

# ---- Optional: a LoRa modem (empty = none, as on a VPS) ---------------
SERIAL=""                 # e.g. /dev/ttyACM0

# ---- Data folder ------------------------------------------------------
DATA="maarse-server-data"
# ======================================================================

set -e
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"
if [ -x "$here/maarse-server" ]; then
  EXE="$here/maarse-server"
elif [ -x "$here/../target/release/maarse-server" ]; then
  EXE="$here/../target/release/maarse-server"
else
  if ! command -v cargo >/dev/null 2>&1; then
    echo "maarse-server was not found next to this file, and Rust is missing to build it."
    echo "Install Rust from https://rustup.rs, or put maarse-server next to this file."
    exit 1
  fi
  echo "Building maarse-server (first time only) ..."
  (cd "$here/.." && cargo build --release -p maarse-host --bin maarse-server)
  EXE="$here/../target/release/maarse-server"
fi

if [ "$PUBLIC_ADDR" = "server.example.org:4242" ]; then
  echo "Fill in PUBLIC_ADDR at the top of $(basename "$0") first: the address others reach this server on."
  exit 1
fi

if [ -n "$ADMIN_PASSWORD" ]; then
  "$EXE" --set-admin-password "$ADMIN_PASSWORD" --data "$DATA"
fi

set -- --data "$DATA" --public-addr "$PUBLIC_ADDR" --listen "$LISTEN" --http "$HTTP" --min-peers "$MIN_PEERS" "$@"
for p in $PEERS; do set -- "$@" --peer "$p"; done
for s in $SEED_DNS; do set -- "$@" --seed-dns "$s"; done
for r in $REGIONS; do set -- "$@" --region "$r"; done
[ "$STRICT" = "yes" ] && set -- "$@" --strict-region
[ -n "$SERIAL" ] && set -- "$@" --serial "$SERIAL"

echo "Starting: $EXE $*"
exec "$EXE" "$@"
