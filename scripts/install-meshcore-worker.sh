#!/usr/bin/env bash
# =============================================================================
#  Installs (or updates) the MeshCore radio worker on a maarse-server (Debian / Ubuntu).
#
#    sudo bash /opt/maarse/src/scripts/install-meshcore-worker.sh
#
#  The backbone's MeshCore gateway (the Heltec V4 that connects over Wi-Fi) is only
#  a radio. meshcore-host is the program that runs the MeshCore identities of your
#  users and talks to that radio through the server. It must run on the same
#  machine as maarse-server: it connects to the server's loopback socket
#  (127.0.0.1:5057, --meshcore-worker) and shares the folder meshcore-nodes in the
#  server's data folder.
#
#  What it does:
#    - installs git, cmake and a C++ compiler
#    - builds meshcore-host from tools/meshcore-host (the official MeshCore code at
#      the revision in firmware/meshcore-gateway/MESHCORE_REVISION)
#    - installs /usr/local/bin/meshcore-host
#    - installs and starts the service maarse-meshcore-worker
#
#  Run it again after "install-server.sh --update" to update the worker.
# =============================================================================
set -euo pipefail

SRC=/opt/maarse/src
DATA=/var/lib/maarse-server
NODES="$DATA/meshcore-nodes"
BIN=/usr/local/bin/meshcore-host
UNIT=/etc/systemd/system/maarse-meshcore-worker.service
SVC_USER=maarse
WORKER_SOCKET=127.0.0.1:5057

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
die() { printf '\033[31mError: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(id -u)" -eq 0 ] || die "run as root, e.g.: sudo bash $0"
command -v apt-get >/dev/null || die "this script is for Debian or Ubuntu (apt-get not found)"
command -v systemctl >/dev/null || die "systemd is needed"
[ -d "$SRC/tools/meshcore-host" ] || die "$SRC/tools/meshcore-host not found: install the server first with install-server.sh, then update it"
[ -d "$DATA" ] || die "$DATA not found: install the server first with install-server.sh"
id "$SVC_USER" >/dev/null 2>&1 || die "the user $SVC_USER does not exist: install the server first with install-server.sh"

bold "1/4  System packages"
apt-get update -qq
apt-get install -y -qq ca-certificates git cmake g++ make >/dev/null

bold "2/4  Building meshcore-host"
BUILD_OUT="$SRC/tools/meshcore-host/build/meshcore-host"
( cd "$SRC" && tools/meshcore-host/build.sh "$BUILD_OUT" ) || die "the build failed (see above)"
install -m 0755 "$BUILD_OUT" "$BIN"

bold "3/4  Folders"
install -d -o "$SVC_USER" -g "$SVC_USER" -m 0700 "$NODES"

bold "4/4  Service"
cat >"$UNIT" <<EOF
# written by install-meshcore-worker.sh
[Unit]
Description=Maarse MeshCore radio worker
After=maarse-server.service
Wants=maarse-server.service

[Service]
Type=simple
User=$SVC_USER
Group=$SVC_USER
ExecStart=$BIN --service --tcp $WORKER_SOCKET --data $NODES
# The worker exits when the server is not listening yet: try again until it is.
Restart=always
RestartSec=3
StartLimitIntervalSec=0

NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictNamespaces=yes
RestrictRealtime=yes
LockPersonality=yes
SystemCallArchitectures=native
RestrictAddressFamilies=AF_INET AF_UNIX
CapabilityBoundingSet=
ReadWritePaths=$NODES

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable maarse-meshcore-worker >/dev/null 2>&1
systemctl restart maarse-meshcore-worker
sleep 2
if systemctl is-active --quiet maarse-meshcore-worker; then
  echo
  bold "The MeshCore radio worker is running."
  info "Status:   systemctl status maarse-meshcore-worker"
  info "Log:      journalctl -u maarse-meshcore-worker -f"
  info "The dashboard (Connections -> MeshCore gateway) shows 'radio worker connected: yes' when a gateway is connected."
else
  die "the worker did not start: journalctl -u maarse-meshcore-worker -n 50"
fi
