#!/usr/bin/env bash
# Install or update the ebbtop agent on the monitored host as a systemd
# user service. Idempotent. Keeps an existing token. Prints no secret.
#   ~/ebbtop/deploy/install-agent.sh            install/update + start
#   ~/ebbtop/deploy/install-agent.sh --rollback restore previous binary
#
# Required env:
#   EBBTOP_ADDR   address the agent binds (tailnet or LAN IP of this host)
# Optional env:
#   EBBTOP_PORT   port (default 45976)
set -euo pipefail
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SVC="$HOME/services/ebbtop-agent"
OLD_SVC="$HOME/services/kmon-agent"
OLD_UNIT="$HOME/.config/systemd/user/kmon-agent.service"
UNIT="$HOME/.config/systemd/user/ebbtop-agent.service"
BIN="$REPO/target/release/ebbtop-agent"
PORT="${EBBTOP_PORT:-45976}"
# Only needed for a fresh install (bind address for agent.env).
ADDR="${EBBTOP_ADDR:-}"

migrate_from_kmon() {
  # One-time move from the pre-rename layout. Keeps token and env.
  if [[ ! -e "$SVC" && -e "$OLD_SVC" ]]; then
    mv "$OLD_SVC" "$SVC"
    echo "migrated $OLD_SVC -> $SVC"
  fi
  if [[ -f "$SVC/kmon.token" && ! -f "$SVC/ebbtop.token" ]]; then
    mv "$SVC/kmon.token" "$SVC/ebbtop.token"
    echo "migrated kmon.token -> ebbtop.token"
  fi
  if [[ -f "$OLD_UNIT" ]]; then
    systemctl --user stop kmon-agent.service 2>/dev/null || true
    systemctl --user disable kmon-agent.service 2>/dev/null || true
    rm -f "$OLD_UNIT"
    echo "retired kmon-agent.service"
  fi
}

if [[ "${1:-}" == "--rollback" ]]; then
  if [[ -f "$SVC/bin/ebbtop-agent.bak" ]]; then
    mv "$SVC/bin/ebbtop-agent.bak" "$SVC/bin/ebbtop-agent"
  else
    echo "no .bak to roll back to"
    exit 1
  fi
  systemctl --user restart ebbtop-agent.service
  echo "rolled back"; exit 0
fi

migrate_from_kmon

# Port must be free or already ours.
holder="$(ss -tlnpH "sport = :$PORT" 2>/dev/null || true)"
if [[ -n "$holder" && "$holder" != *ebbtop-agent* ]]; then
  echo "port $PORT held by something else: $holder"; exit 1
fi

# GPU on: gfxinfo is built without its AMD/libdrm path (needs libdrm headers
# the build host may lack); NVIDIA plus the NVML fallback cover that host.
(cd "$REPO" && cargo build --release --locked -p ebbtop-agent --no-default-features --features gpu)

mkdir -p "$SVC/bin" "$(dirname "$UNIT")"
[[ -f "$SVC/bin/ebbtop-agent" ]] && cp -p "$SVC/bin/ebbtop-agent" "$SVC/bin/ebbtop-agent.bak"
install -m 755 "$BIN" "$SVC/bin/ebbtop-agent"

( umask 077
  if [[ ! -f "$SVC/agent.env" ]]; then
    [[ -n "$ADDR" ]] || { echo "fresh install needs EBBTOP_ADDR=this host's tailnet/LAN IP"; exit 1; }
    tok="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    printf 'EBBTOP_BIND=%s\nEBBTOP_AGENT_PROCESSES_TTL_MS=500\nEBBTOP_AGENT_METRICS_TTL_MS=100\nEBBTOP_TOKEN=%s\n' "$ADDR" "$tok" > "$SVC/agent.env"
    printf '%s\n' "$tok" > "$SVC/ebbtop.token"
    echo "new token written to $SVC/ebbtop.token (mode 600)"
  fi )
chmod 600 "$SVC/agent.env" "$SVC/ebbtop.token" 2>/dev/null || true
# Settings added after first install; appended once, token untouched.
grep -q '^EBBTOP_AGENT_METRICS_TTL_MS=' "$SVC/agent.env" || echo 'EBBTOP_AGENT_METRICS_TTL_MS=100' >> "$SVC/agent.env"

install -m 644 "$REPO/deploy/ebbtop-agent.service" "$UNIT"
systemctl --user daemon-reload
systemctl --user enable ebbtop-agent.service
systemctl --user restart ebbtop-agent.service
sleep 2

echo "active: $(systemctl --user is-active ebbtop-agent.service)"
echo "listen: $(ss -tlnH "sport = :$PORT" | awk '{print $4}' | paste -sd' ')"
echo "binary: $(sha256sum "$SVC/bin/ebbtop-agent" | cut -c1-16)"
echo "loaded: $(systemctl --user show -p ExecStart --value ebbtop-agent.service | grep -o 'path=[^ ;]*')"
