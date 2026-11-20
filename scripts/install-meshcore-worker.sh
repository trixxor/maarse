#!/usr/bin/env bash
# =============================================================================
#  Builds and installs the MeshCore radio worker (meshcore-host) on a maarse-server
#  (Debian / Ubuntu).
#
#  You normally do not run this yourself: install-server.sh (also with --update)
#  runs it. maarse-server starts the worker and keeps it running when the
#  MeshCore gateway is enabled, so there is no separate service.
#
#    sudo bash /opt/maarse/src/scripts/install-meshcore-worker.sh
#
#  The backbone's MeshCore gateway (the Heltec V4 that connects over Wi-Fi) is only
#  a radio. meshcore-host runs the MeshCore identities of your users and talks to
#  that radio through the server. It builds from tools/meshcore-host: the official
#  MeshCore code at the revision in firmware/meshcore-gateway/MESHCORE_REVISION.
#  It is installed as /usr/local/bin/meshcore-host, where the server finds it.
# =============================================================================
set -euo pipefail

SRC=/opt/maarse/src
BIN=/usr/local/bin/meshcore-host

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
die() { printf '\033[31mError: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root, e.g.: sudo bash $0"
command -v apt-get >/dev/null || die "this script is for Debian or Ubuntu (apt-get not found)"
[ -d "$SRC/tools/meshcore-host" ] || die "$SRC/tools/meshcore-host not found: install or update the server first with install-server.sh"

bold "MeshCore radio worker: packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates git cmake g++ make >/dev/null

bold "MeshCore radio worker: building (a minute or two)"
BUILD_OUT="$SRC/tools/meshcore-host/build/meshcore-host"
( cd "$SRC" && tools/meshcore-host/build.sh "$BUILD_OUT" ) || die "the worker build failed (see above)"
install -m 0755 "$BUILD_OUT" "$BIN"
info "installed $BIN"

# An earlier version of this script ran the worker as its own service. The server
# starts it now, so two workers would fight over the same socket: remove that service.
if [ -f /etc/systemd/system/maarse-meshcore-worker.service ]; then
  systemctl disable --now maarse-meshcore-worker >/dev/null 2>&1 || true
  rm -f /etc/systemd/system/maarse-meshcore-worker.service
  systemctl daemon-reload
  info "removed the old maarse-meshcore-worker service"
fi

# A running server starts its worker at start-up: restart it to pick up a new one.
# (install-server.sh restarts the server itself, so it sets MAARSE_NO_RESTART.)
if [ -z "${MAARSE_NO_RESTART:-}" ] && systemctl is-active --quiet maarse-server 2>/dev/null; then
  systemctl restart maarse-server
  info "restarted maarse-server so that it starts the new worker"
fi
info "The dashboard (Connections -> MeshCore gateway) shows 'radio worker connected' when a gateway is connected."
