#!/usr/bin/env bash
# Install or update the ebbtop agent on a REMOTE Linux host over SSH.
# Same result as install-agent.sh, but the binary ships from here instead
# of building on the target (no Rust needed there). Idempotent. Keeps an
# existing token. Prints no secret.
#   ~/ebbtop/deploy/install-agent-remote.sh user@host [bind-addr] [port]
#   ~/ebbtop/deploy/install-agent-remote.sh user@host --rollback
#
# bind-addr defaults to the remote's Tailscale IPv4 (`tailscale ip -4`);
# pass it explicitly for LAN-only hosts without Tailscale.
# Extra ssh options via EBBTOP_SSH_OPTS (e.g. "-i ~/.ssh/x1-ebbtop").
set -euo pipefail

TARGET="${1:?usage: install-agent-remote.sh user@host [bind-addr] [port] [--rollback]}"
if [[ "${2:-}" == "--rollback" || "${3:-}" == "--rollback" ]]; then
  # shellcheck disable=SC2086
  # shellcheck disable=SC2029
  ssh ${EBBTOP_SSH_OPTS:-} "$TARGET" sh -s <<'EOF'
set -e
if [ -f ~/services/ebbtop-agent/bin/ebbtop-agent.bak ]; then
  mv ~/services/ebbtop-agent/bin/ebbtop-agent.bak ~/services/ebbtop-agent/bin/ebbtop-agent
else
  echo "no .bak to roll back to"; exit 1
fi
systemctl --user restart ebbtop-agent.service
echo "rolled back"
EOF
  exit 0
fi
BIND="${2:-}"
PORT="${3:-45976}"

REPO="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC2086
SSH="ssh ${EBBTOP_SSH_OPTS:-}"

# The remote must be x86_64 Linux: this is the binary we ship.
arch="$($SSH -o BatchMode=yes "$TARGET" 'uname -m' 2>/dev/null || true)"
if [[ "$arch" != "x86_64" ]]; then
  echo "remote arch is '$arch', this script ships the x86_64 Linux binary; build the agent on the host instead"
  exit 1
fi
if [[ -z "$BIND" ]]; then
  # shellcheck disable=SC2029
  BIND="$($SSH "$TARGET" 'tailscale ip -4 2>/dev/null | head -n 1' || true)"
  [[ -n "$BIND" ]] || { echo "no Tailscale IP found; pass bind-addr explicitly"; exit 1; }
fi
echo "target: $TARGET arch=$arch bind=$BIND port=$PORT"

# Fresh release binary from this tree (same flags as install-agent.sh).
(cd "$REPO" && cargo build --release --locked -p ebbtop-agent --no-default-features --features gpu)

BIN="$REPO/target/release/ebbtop-agent"
scp ${EBBTOP_SSH_OPTS:-} "$BIN" "$TARGET:ebbtop-agent-new"
scp ${EBBTOP_SSH_OPTS:-} "$REPO/deploy/ebbtop-agent.service" "$TARGET:.config/systemd/user/ebbtop-agent.service.new"

# shellcheck disable=SC2029
$SSH "$TARGET" sh -s "$BIND" "$PORT" <<'EOF'
set -euo pipefail
BIND="$1"; PORT="$2"
SVC="$HOME/services/ebbtop-agent"
mkdir -p "$SVC/bin" "$HOME/.config/systemd/user"
[ -f "$SVC/bin/ebbtop-agent" ] && cp -p "$SVC/bin/ebbtop-agent" "$SVC/bin/ebbtop-agent.bak"
mv ~/ebbtop-agent-new "$SVC/bin/ebbtop-agent"
chmod 755 "$SVC/bin/ebbtop-agent"
mv ~/.config/systemd/user/ebbtop-agent.service.new ~/.config/systemd/user/ebbtop-agent.service
( umask 077
  if [ ! -f "$SVC/agent.env" ]; then
    tok="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    printf 'EBBTOP_BIND=%s\nEBBTOP_AGENT_PROCESSES_TTL_MS=500\nEBBTOP_AGENT_METRICS_TTL_MS=100\nEBBTOP_TOKEN=%s\n' "$BIND" "$tok" > "$SVC/agent.env"
    printf '%s\n' "$tok" > "$SVC/ebbtop.token"
    echo "new token written to $SVC/ebbtop.token (mode 600)"
  fi )
chmod 600 "$SVC/agent.env" "$SVC/ebbtop.token" 2>/dev/null || true
grep -q '^EBBTOP_AGENT_METRICS_TTL_MS=' "$SVC/agent.env" || echo 'EBBTOP_AGENT_METRICS_TTL_MS=100' >> "$SVC/agent.env"
if [ "$(loginctl show-user "$USER" -p Linger --value 2>/dev/null)" != "yes" ]; then
  echo "note: linger is off; ask an admin for: loginctl enable-linger $USER"
  echo "without it the agent stops at logout and misses boots"
fi
systemctl --user daemon-reload
systemctl --user enable ebbtop-agent.service >/dev/null
systemctl --user restart ebbtop-agent.service
sleep 2
echo "active: $(systemctl --user is-active ebbtop-agent.service)"
echo "listen: $(ss -tlnH "sport = :$PORT" | awk '{print $4}' | paste -sd' ')"
echo "binary: $(sha256sum "$SVC/bin/ebbtop-agent" | cut -c1-16)"
EOF

echo "== probe over the network (token from a one-shot env, never printed)"
EBBTOP_TOKEN="$($SSH "$TARGET" 'cat ~/services/ebbtop-agent/ebbtop.token')" \
  "$REPO/target/release/ebbtop-probe" --url "ws://$BIND:$PORT/ws" --seconds 10 2>&1 | tail -n 8
echo "== client setup"
echo "fetch the token (never leaves your clipboard):"
echo "  ssh $TARGET cat services/ebbtop-agent/ebbtop.token | pbcopy   # macOS"
echo "then Settings/add source in EbbTop: ws://$BIND:$PORT/ws + token (one token per host)"
