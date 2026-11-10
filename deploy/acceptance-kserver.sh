#!/usr/bin/env bash
# Acceptance helper, run ON the monitored host (no SSH from the desktop needed).
# 1) probes the live agent over its tailnet/LAN address for N seconds
# 2) prints top's view of the busiest processes in the same second, as
#    ground truth for the "top process name/PID match" check.
# Prints no secret. Usage: deploy/acceptance-kserver.sh [seconds]
set -euo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
SECS="${1:-60}"
TOKEN_FILE="${EBBTOP_TOKEN_FILE:-$HOME/services/ebbtop-agent/ebbtop.token}"
ADDR="${EBBTOP_ADDR:-127.0.0.1}"
cd "$REPO"
cargo build -q --release -p ebbtop-core --bin ebbtop-probe
echo "== service"
systemctl --user is-active ebbtop-agent.service || true
ss -tlnH 'sport = :45976' | awk '{print "listen", $4}'
echo "== probe ${SECS}s"
./target/release/ebbtop-probe --url ws://$ADDR:45976/ws --seconds "$SECS" --token-file "$TOKEN_FILE" \
  | sed -n '/^--- summary/,$p'
echo "== top ground truth (second iteration, 1 s delta), $(date +%H:%M:%S)"
top -b -n 2 -d 1 -w 200 | awk '/^top -/{n++} n==2' | sed -n '1,5p;7,13p'
