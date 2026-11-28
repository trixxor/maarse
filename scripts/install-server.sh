#!/usr/bin/env bash
# =============================================================================
#  Installs (or updates) maarse-server on Debian / Ubuntu.
#
#  Builds the server from the source code, asks for the settings it needs,
#  installs it as a systemd service that starts at boot, and optionally sets
#  up HTTPS for the dashboard with Caddy.
#
#  Run as root (or with sudo):
#    curl -fsSL https://raw.githubusercontent.com/trixxor/maarse/main/scripts/install-server.sh -o install-server.sh
#    sudo bash install-server.sh
#  or from a checkout:  sudo bash scripts/install-server.sh
#
#  Run it again later to update to the newest version; the settings are kept
#  unless you choose to change them. Without questions:
#    sudo bash /opt/maarse/src/scripts/install-server.sh --update
#
#  What it installs:
#    /usr/local/bin/maarse-server          the program
#    /var/lib/maarse-server/settings.txt   all settings (the dashboard edits it too)
#    /etc/systemd/system/maarse-server.service
#    /var/lib/maarse-server                data: identity, telemetry, mail, admin password
#    /var/lib/maarse-server-state          a counter that notices a restored old data folder
#                                          (do not back it up or restore it with the data)
#    /opt/maarse/src                       the source code it was built from
#    user "maarse" (system user, in the "dialout" group for a USB LoRa modem)
# =============================================================================
set -euo pipefail

REPO_DEFAULT="https://github.com/trixxor/maarse.git"
SRC=/opt/maarse/src
DATA=/var/lib/maarse-server
STATE=/var/lib/maarse-server-state
CONF=/etc/default/maarse-server
UNIT=/etc/systemd/system/maarse-server.service
BIN=/usr/local/bin/maarse-server
SVC_USER=maarse
SETTINGS=$DATA/settings.txt

# Questions are read from the terminal, also when the script is piped.
TTY=/dev/tty
bold() { printf '\033[1m%s\033[0m\n' "$*"; }
info() { printf '  %s\n' "$*"; }
warn() { printf '\033[33m! %s\033[0m\n' "$*"; }
die() { printf '\033[31mError: %s\033[0m\n' "$*" >&2; exit 1; }

ask() { # ask "Question" "default" -> REPLY
  local q="$1" def="${2:-}"
  if [ -n "$def" ]; then printf '%s [%s]: ' "$q" "$def" >"$TTY"; else printf '%s: ' "$q" >"$TTY"; fi
  IFS= read -r REPLY <"$TTY" || true
  [ -z "$REPLY" ] && REPLY="$def"
  return 0
}
ask_yn() { # ask_yn "Question" y|n -> 0 for yes
  local q="$1" def="$2" hint
  [ "$def" = y ] && hint="Y/n" || hint="y/N"
  while true; do
    printf '%s [%s]: ' "$q" "$hint" >"$TTY"
    IFS= read -r REPLY <"$TTY" || true
    REPLY="${REPLY:-$def}"
    case "$REPLY" in [Yy]*) return 0 ;; [Nn]*) return 1 ;; esac
  done
}
ask_secret() { # ask_secret "Question" -> REPLY (not echoed)
  printf '%s: ' "$1" >"$TTY"
  stty -echo <"$TTY"; IFS= read -r REPLY <"$TTY" || true; stty echo <"$TTY"; printf '\n' >"$TTY"
}

# Turns an option string ("--listen 0.0.0.0:4242 --peer a --no-relay") into one option per line.
args_to_lines() {
  local word line=""
  for word in $1; do
    case "$word" in
      --*) [ -n "$line" ] && printf '%s\n' "$line"; line="$word" ;;
      *)   line="$line $word" ;;
    esac
  done
  [ -n "$line" ] && printf '%s\n' "$line"
  return 0
}

# Options that can appear several times; every other option has one value.
is_list_option() {
  case "$1" in
    --peer|--seed-dns|--region|--trunk|--dht|--dht-allow-private|--block|--udp-peer|--bulletin-key|--alert-root|--mail-replica-id|--node) return 0 ;;
  esac
  return 1
}

# Older versions kept the options in $CONF (MAARSE_ARGS="..."). All settings live in
# $SETTINGS now, which the dashboard edits too. This moves the options there, after
# the lines already in the file, which is what they overrode before: an option with
# one value replaces its earlier line. Safe to repeat: $CONF is renamed afterwards.
migrate_args() {
  [ -f "$CONF" ] || return 0
  local args line flag
  args=$(sed -n 's/^MAARSE_ARGS="\(.*\)"[[:space:]]*$/\1/p' "$CONF" | tail -n1)
  # The server is installed (it has a $CONF), so the settings file exists from now on.
  mkdir -p "$DATA"
  touch "$SETTINGS"
  if [ -n "$args" ]; then
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      flag="${line%% *}"
      is_list_option "$flag" || sed -i "/^$flag /d;/^$flag\$/d" "$SETTINGS"
      printf '%s\n' "$line" >>"$SETTINGS"
    done < <(args_to_lines "$args")
    chown "$SVC_USER:$SVC_USER" "$SETTINGS" 2>/dev/null || true
    chmod 0600 "$SETTINGS"
    info "moved the options from $CONF into $SETTINGS (the dashboard edits that file)"
  fi
  mv "$CONF" "$CONF.migrated"
}

UPDATE=0
[ "${1:-}" = "--update" ] && UPDATE=1
[ "$(id -u)" -eq 0 ] || die "run as root, e.g.: sudo bash $0"
command -v apt-get >/dev/null || die "this script is for Debian or Ubuntu (apt-get not found)"
command -v systemctl >/dev/null || die "systemd is needed"

bold "Må:rse server installer"
info "Builds maarse-server from source, asks for its settings and runs it as a service."
echo

# ---------------------------------------------------------------- packages
bold "1/6  System packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq ca-certificates curl git build-essential pkg-config >/dev/null
info "ok: git, compiler, curl"

# ---------------------------------------------------------------- rust
bold "2/6  Rust (the compiler, installed for root under /root/.cargo)"
export CARGO_HOME=/root/.cargo RUSTUP_HOME=/root/.rustup PATH="/root/.cargo/bin:$PATH"
if ! command -v cargo >/dev/null; then
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable >/dev/null
  info "installed $(rustc --version)"
else
  rustup update stable >/dev/null 2>&1 || true
  info "ok: $(rustc --version)"
fi

# ---------------------------------------------------------------- source
bold "3/6  Source code"
if [ -d "$SRC/.git" ]; then
  info "updating $SRC"
  git -C "$SRC" pull --ff-only || die "git pull failed in $SRC (local changes?)"
else
  ask "Git repository" "$REPO_DEFAULT"; REPO="$REPLY"
  mkdir -p "$(dirname "$SRC")"
  git clone "$REPO" "$SRC" </dev/tty || die "git clone failed"
fi

# Building Rust needs memory: a small VPS without swap runs out.
MEM_MB=$(awk '/MemTotal/ {print int($2/1024)}' /proc/meminfo)
SWAP_MB=$(awk '/SwapTotal/ {print int($2/1024)}' /proc/meminfo)
if [ "$MEM_MB" -lt 1500 ] && [ "$SWAP_MB" -lt 1000 ]; then
  warn "only ${MEM_MB} MB of memory and no swap: the build may run out of memory"
  if [ "$UPDATE" = 1 ] || ask_yn "Create a 2 GB swap file (/swapfile)?" y; then
    fallocate -l 2G /swapfile 2>/dev/null || dd if=/dev/zero of=/swapfile bs=1M count=2048 status=none
    chmod 600 /swapfile && mkswap /swapfile >/dev/null && swapon /swapfile
    grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' >>/etc/fstab
    info "swap on"
  fi
fi

bold "4/6  Building maarse-server (the first time takes 5-15 minutes)"
( cd "$SRC" && cargo build --release -p maarse-host --bin maarse-server )
install -m 0755 "$SRC/target/release/maarse-server" "$BIN"
info "installed $BIN"

# The MeshCore radio worker: the server starts it when the MeshCore gateway is on.
# A failure here must not stop the server update, so it only warns.
if [ -d "$SRC/tools/meshcore-host" ]; then
  MAARSE_NO_RESTART=1 bash "$SRC/scripts/install-meshcore-worker.sh" || warn "the MeshCore radio worker was not built; run $SRC/scripts/install-meshcore-worker.sh to see why"
fi

# ---------------------------------------------------------------- user
if ! id "$SVC_USER" >/dev/null 2>&1; then
  useradd --system --home-dir "$DATA" --no-create-home --shell /usr/sbin/nologin "$SVC_USER"
fi
usermod -aG dialout "$SVC_USER" 2>/dev/null || true
mkdir -p "$DATA" "$STATE"
chown -R "$SVC_USER:$SVC_USER" "$DATA" "$STATE"
chmod 0700 "$DATA" "$STATE"

# ---------------------------------------------------------------- settings
bold "5/6  Settings"
CHANGE=y
migrate_args
if [ "$UPDATE" = 1 ]; then
  [ -f "$SETTINGS" ] || die "--update: not installed yet (no $SETTINGS); run without --update first"
  CHANGE=n
elif [ -f "$SETTINGS" ]; then
  info "Current settings ($SETTINGS):"
  grep -v '^#' "$SETTINGS" | sed 's/^\(--modem-key\) .*/\1 ****/; s/^/    /'
  ask_yn "Keep the current settings?" y && CHANGE=n
fi

HTTPS_DOMAIN=""
if [ "$CHANGE" = y ]; then
  echo
  info "Public address: the name (or IP) and port other servers and hosts reach this server on."
  info "Give each server its own name, e.g. backbone01.example.org; a shared name with all"
  info "servers behind it (e.g. seed.example.org) is for --seed-dns on the others."
  ask "Public address (host:port)" "$(hostname -f 2>/dev/null || hostname):4242"; PUBLIC_ADDR="$REPLY"
  PORT="${PUBLIC_ADDR##*:}"; [ "$PORT" = "$PUBLIC_ADDR" ] && PORT=4242 && PUBLIC_ADDR="$PUBLIC_ADDR:4242"
  ask "Backbone port to listen on" "$PORT"; PORT="$REPLY"

  echo
  info "Other servers to connect to (host:port, separated by spaces). Empty for the first server."
  ask "Peers" ""; PEERS="$REPLY"
  ask "DNS seed name with several servers behind it (empty = none)" ""; SEED_DNS="$REPLY"
  ask "Keep at least this many backbone links" "6"; MIN_PEERS="$REPLY"

  echo
  info "Regions this server forwards channel messages for, e.g. 'dk nordjylland' (empty = all)."
  ask "Regions" ""; REGIONS="$REPLY"

  echo
  POST_ARGS=""
  if ask_yn "Keep mail for offline nodes (be a post office)?" y; then
    ask "Most mail to keep, MB (0 = no limit)" "500"; [ "$REPLY" != 0 ] && POST_ARGS="--mail-quota-mb $REPLY"
  else
    POST_ARGS="--no-post-office"
  fi
  RELAY_ARGS=""
  ask_yn "Relay for hosts behind NAT (recommended for a public server)?" y || RELAY_ARGS="--no-relay"

  echo
  MODEM=""
  PORTS=$(ls /dev/ttyACM* /dev/ttyUSB* 2>/dev/null || true)
  if [ -n "$PORTS" ]; then
    info "Serial ports found: $(echo $PORTS)"
    ask "USB LoRa modem port (empty = none)" ""; MODEM="$REPLY"
  else
    info "No USB serial ports found (a VPS has none); no LoRa modem."
  fi

  echo
  info "The admin dashboard shows telemetry and settings, behind a login."
  if ask_yn "Serve the dashboard with HTTPS through Caddy (needs a domain pointing here and ports 80/443)?" n; then
    ask "Domain for the dashboard" "${PUBLIC_ADDR%:*}"; HTTPS_DOMAIN="$REPLY"
    HTTP="127.0.0.1:8080"
  else
    warn "Without HTTPS the password crosses the network in plain text."
    if ask_yn "Reach the dashboard only through an SSH tunnel (safer)?" y; then
      HTTP="127.0.0.1:8080"
      info "Later: ssh -L 8080:127.0.0.1:8080 <this server>, then open http://127.0.0.1:8080/"
    else
      ask "Dashboard address" "0.0.0.0:8080"; HTTP="$REPLY"
    fi
  fi

  ARGS="--public-addr $PUBLIC_ADDR --listen 0.0.0.0:$PORT --http $HTTP --require-key --min-peers $MIN_PEERS"
  for p in $PEERS; do ARGS="$ARGS --peer $p"; done
  [ -n "$SEED_DNS" ] && ARGS="$ARGS --seed-dns $SEED_DNS"
  for r in $REGIONS; do ARGS="$ARGS --region $r"; done
  [ -n "$POST_ARGS" ] && ARGS="$ARGS $POST_ARGS"
  [ -n "$RELAY_ARGS" ] && ARGS="$ARGS $RELAY_ARGS"
  [ -n "$MODEM" ] && ARGS="$ARGS --serial $MODEM"

  # Check the options with the program itself before saving them.
  if ! "$BIN" --help >/dev/null 2>&1; then die "the installed program does not run"; fi
  [ -f "$SETTINGS" ] && cp "$SETTINGS" "$SETTINGS.bak"
  {
    echo "# maarse-server settings. Edit here or in the dashboard (Settings tab), then restart the server."
    echo "# One option per line, as on the command line (maarse-server --help). Written by install-server.sh."
    args_to_lines "$ARGS"
  } >"$SETTINGS"
  chown "$SVC_USER:$SVC_USER" "$SETTINGS" 2>/dev/null || true
  chmod 0600 "$SETTINGS"
  info "saved $SETTINGS"
else
  ARGS=$(grep -v '^#' "$SETTINGS" | tr '\n' ' ')
  MODEM=$(printf '%s' "$ARGS" | sed -n 's/.*--serial \([^ ]*\).*/\1/p')
  PORT=$(printf '%s' "$ARGS" | sed -n 's/.*--listen [^ ]*:\([0-9]*\).*/\1/p'); PORT="${PORT:-4242}"
  HTTP=$(printf '%s' "$ARGS" | sed -n 's/.*--http \([^ ]*\).*/\1/p')
fi

# ---------------------------------------------------------------- admin password
if [ ! -f "$DATA/admin.hash" ] || { [ "$UPDATE" = 0 ] && [ "$CHANGE" = y ] && ask_yn "Set a new admin password for the dashboard?" n; }; then
  if ask_yn "Choose the admin password yourself (no = make a random one)?" n; then
    while true; do
      ask_secret "Admin password (at least 10 characters)"; PW1="$REPLY"
      ask_secret "Again"; PW2="$REPLY"
      [ "$PW1" = "$PW2" ] && [ "${#PW1}" -ge 10 ] && break
      warn "the passwords differ or are shorter than 10 characters"
    done
  else
    PW1=$(LC_ALL=C tr -dc 'abcdefghjkmnpqrstuvwxyzACDEFGHJKLMNPQRSTUVWXYZ23456789' </dev/urandom 2>/dev/null | head -c 18 || true)
    GENERATED=1
  fi
  printf '%s\n' "$PW1" | runuser -u "$SVC_USER" -- "$BIN" --set-admin-password-stdin --data "$DATA" >/dev/null
  rm -f "$DATA/admin-password.txt"
fi

# ---------------------------------------------------------------- service
bold "6/6  Service"
DEVICES=""
if [ -n "${MODEM:-}" ]; then
  DEVICES="SupplementaryGroups=dialout
DeviceAllow=$MODEM rw"
fi
cat >"$UNIT" <<EOF
# maarse-server: Må:rse backbone server. Written by install-server.sh.
[Unit]
Description=Maarse server (backbone, post office, telemetry dashboard)
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
User=$SVC_USER
Group=$SVC_USER
ExecStart=$BIN --service --data $DATA
Restart=on-failure
RestartSec=3
StartLimitIntervalSec=0
WorkingDirectory=$DATA
Environment=MAARSE_STATE_DIR=$STATE
$DEVICES

NoNewPrivileges=yes
ProtectSystem=strict
ReadWritePaths=$DATA $STATE
ProtectHome=yes
PrivateTmp=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
LockPersonality=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK
CapabilityBoundingSet=

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
systemctl enable maarse-server >/dev/null 2>&1
systemctl restart maarse-server
sleep 3
if systemctl is-active --quiet maarse-server; then
  info "maarse-server is running and starts at boot"
else
  warn "maarse-server did not start; see: journalctl -u maarse-server -n 50"
fi

# ---------------------------------------------------------------- firewall
UFW_STATUS=$(ufw status 2>/dev/null || true)
if command -v ufw >/dev/null && [ "${UFW_STATUS#Status: active}" != "$UFW_STATUS" ]; then
  ufw allow "$PORT/tcp" >/dev/null && info "ufw: TCP $PORT (backbone) allowed"
  case "${HTTP:-}" in 0.0.0.0:*) ufw allow "${HTTP##*:}/tcp" >/dev/null && info "ufw: TCP ${HTTP##*:} (dashboard) allowed" ;; esac
fi

# ---------------------------------------------------------------- https
if [ -n "$HTTPS_DOMAIN" ]; then
  apt-get install -y -qq caddy >/dev/null || die "could not install caddy"
  mkdir -p /etc/caddy
  if ! grep -q "maarse-server" /etc/caddy/Caddyfile 2>/dev/null; then
    [ -f /etc/caddy/Caddyfile ] && cp /etc/caddy/Caddyfile /etc/caddy/Caddyfile.before-maarse
    cat >/etc/caddy/Caddyfile <<EOF
# maarse-server dashboard (written by install-server.sh)
$HTTPS_DOMAIN {
	tls {
		issuer acme https://acme-v02.api.letsencrypt.org/directory
	}
	reverse_proxy 127.0.0.1:8080 {
		header_down Set-Cookie "(.*)" "\$1; Secure"
	}
}
EOF
  fi
  systemctl reload caddy 2>/dev/null || systemctl restart caddy
  if command -v ufw >/dev/null && [ "${UFW_STATUS#Status: active}" != "$UFW_STATUS" ]; then
    ufw allow 80/tcp >/dev/null; ufw allow 443/tcp >/dev/null
  fi
  info "Caddy serves https://$HTTPS_DOMAIN/ (the certificate comes from Let's Encrypt within a minute)"
fi

# ---------------------------------------------------------------- summary
echo
bold "Done."
info "Version:     $(git -C "$SRC" log -1 --format='%h %ad %s' --date=short 2>/dev/null)"
info "Status:      systemctl status maarse-server"
info "Log:         journalctl -u maarse-server -f"
info "Settings:    the dashboard's Settings tab, or $SETTINGS and then: systemctl restart maarse-server"
info "Update:      sudo bash $SRC/scripts/install-server.sh"
if [ -n "$HTTPS_DOMAIN" ]; then
  info "Dashboard:   https://$HTTPS_DOMAIN/"
else
  case "${HTTP:-}" in
    127.0.0.1:*) info "Dashboard:   ssh -L 8080:127.0.0.1:8080 <this server>, then http://127.0.0.1:8080/" ;;
    *) info "Dashboard:   http://<this server>:${HTTP##*:}/" ;;
  esac
fi
if [ "${GENERATED:-0}" = 1 ]; then
  bold "Admin password (shown only now, write it down): $PW1"
fi
info "Make sure TCP $PORT is open in any firewall in front of this machine."
