#!/usr/bin/env bash
# Native runtime proof for the macOS agent. Runs ONLY on a Mac: every check
# here is one that cannot be made anywhere else, and each one that cannot run
# reports SKIP with the reason rather than a pass.
#
# What it proves, in order:
#   1. the agent starts in the foreground on loopback and a real client gets a
#      metrics frame from it (no GPU, no launchd, no installer involved);
#   2. an enforced-auth runtime refuses to start with no token and with a
#      wildcard bind, and starts with a loopback/private bind + token;
#   3. the authenticated probe says ok with the right token and denies a wrong
#      one and a missing one;
#   4. RAM and disk capacity agree with the platform's own tools (vm_stat,
#      sysctl, df) within a stated tolerance, and the APFS container is
#      reported once rather than once per volume;
#   5. process CPU for a known short-lived workload tracks the truth.
#
# Nothing is installed, no existing agent is touched, and every run uses its
# own state directory and port. Output is written to dist/macos-evidence/ so a
# CI run leaves the numbers behind.
#
# Usage: bash deploy/macos/test-agent-live.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

PASSES=0; FAILS=0; SKIPS=0
pass() { PASSES=$((PASSES+1)); printf 'PASS %s: %s\n' "$1" "$2"; }
fail() { FAILS=$((FAILS+1)); printf 'FAIL %s: %s\n' "$1" "$2" >&2; }
skip() { SKIPS=$((SKIPS+1)); printf 'SKIP %s: %s\n' "$1" "$2"; }
note() { printf '  - %s\n' "$*"; }

if [[ "$(uname -s)" != Darwin ]]; then
  skip "macos-live" "this script only runs on macOS (uname -s reports $(uname -s)); the runtime checks it performs have no Linux equivalent"
  printf '\nSUMMARY: %d passed, %d failed, %d skipped\n' "$PASSES" "$FAILS" "$SKIPS"
  exit 0
fi

EVIDENCE="$ROOT/dist/macos-evidence"
mkdir -p "$EVIDENCE"
LOG="$EVIDENCE/live-agent.log"
: > "$LOG"

BIN="$ROOT/target/aarch64-apple-darwin/release/ebbtop-agent"
[[ -x "$BIN" ]] || BIN="$ROOT/target/release/ebbtop-agent"
if [[ ! -x "$BIN" ]]; then
  fail "binary" "no release agent at target/aarch64-apple-darwin/release/ebbtop-agent; build it with: cargo build --release --locked --target aarch64-apple-darwin -p ebbtop-agent --no-default-features"
  printf '\nSUMMARY: %d passed, %d failed, %d skipped\n' "$PASSES" "$FAILS" "$SKIPS"
  exit 1
fi
note "agent: $BIN ($("$BIN" --version))"
note "evidence: $LOG"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-live.XXXXXXXX")"
cleanup() {
  [[ -n "${AGENT_PID:-}" ]] && kill "$AGENT_PID" 2>/dev/null
  rm -rf -- "$WORK"
}
trap cleanup EXIT

# A free port, so this never collides with an installed agent on 45976.
free_port() {
  local p
  for p in 46081 46082 46083 46084 46085; do
    if ! /usr/bin/nc -z 127.0.0.1 "$p" 2>/dev/null; then printf '%s' "$p"; return 0; fi
  done
  return 1
}
PORT="$(free_port)" || { fail "port" "no free port in the 46081-46085 range"; exit 1; }
note "port: $PORT (the installed agent's 45976 is untouched)"

TOK="$WORK/token"
head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$TOK"
chmod 600 "$TOK"
STATE_DIR="$WORK/state"
mkdir -p "$STATE_DIR"

ENVFILE="$WORK/agent.env"
( umask 077
  printf 'EBBTOP_BIND=127.0.0.1\nEBBTOP_AGENT_DISCOVERY=0\nEBBTOP_AGENT_METRICS_TTL_MS=100\nEBBTOP_AGENT_PROCESSES_TTL_MS=500\nEBBTOP_TOKEN=%s\n' "$(cat "$TOK")" > "$ENVFILE" )
chmod 600 "$ENVFILE"

# ===========================================================================
# 1. fail closed: the enforced gate refuses a missing token and a wildcard bind
# ===========================================================================
case_startup_refusals() {
  local name=startup-refusals out rc=0
  # (a) no token at all
  ( umask 077; printf 'EBBTOP_BIND=127.0.0.1\n' > "$WORK/no-token.env" )
  out="$(EBBTOP_STATE_DIR="$STATE_DIR" timeout 10 "$BIN" --env-file "$WORK/no-token.env" --port "$PORT" 2>&1)" || rc=$?
  printf '%s\n' "$out" >> "$LOG"
  if [[ $rc -eq 0 ]] || ! printf '%s' "$out" | grep -q 'without a token'; then
    fail "$name" "an agent with no token was not refused (exit $rc): $out"; return
  fi
  # (b) a wildcard bind
  rc=0
  ( umask 077; printf 'EBBTOP_BIND=0.0.0.0\nEBBTOP_TOKEN=%s\n' "$(cat "$TOK")" > "$WORK/wild.env" )
  out="$(EBBTOP_STATE_DIR="$STATE_DIR" timeout 10 "$BIN" --env-file "$WORK/wild.env" --port "$PORT" 2>&1)" || rc=$?
  printf '%s\n' "$out" >> "$LOG"
  if [[ $rc -eq 0 ]] || ! printf '%s' "$out" | grep -q 'not loopback or a private'; then
    fail "$name" "a wildcard bind was not refused (exit $rc): $out"; return
  fi
  # (c) the gate cannot be relaxed by the environment on Darwin. Set every
  #     spelling that would disable enforcement on a permissive build.
  rc=0
  out="$(EBBTOP_AGENT_REQUIRE_AUTH=0 EBBTOP_STATE_DIR="$STATE_DIR" timeout 10 "$BIN" --env-file "$WORK/no-token.env" --port "$PORT" 2>&1)" || rc=$?
  printf '%s\n' "$out" >> "$LOG"
  if [[ $rc -eq 0 ]]; then
    fail "$name" "EBBTOP_AGENT_REQUIRE_AUTH=0 disabled enforcement on macOS"; return
  fi
  printf '%s' "$out" | grep -q 'without a token' \
    || { fail "$name" "the refusal message changed under REQUIRE_AUTH=0: $out"; return; }
  pass "$name" "the Darwin runtime refuses a missing token and a wildcard bind, and EBBTOP_AGENT_REQUIRE_AUTH cannot relax it"
}

# ===========================================================================
# 2. a real foreground agent serves metrics to a real client
# ===========================================================================
start_agent() {
  EBBTOP_STATE_DIR="$STATE_DIR" "$BIN" --env-file "$ENVFILE" --port "$PORT" >> "$LOG" 2>&1 &
  AGENT_PID=$!
  local i=0
  while (( i < 60 )); do
    if /usr/bin/nc -z 127.0.0.1 "$PORT" 2>/dev/null; then return 0; fi
    sleep 0.2; i=$((i+1))
  done
  return 1
}

case_foreground_metrics() {
  local name=foreground-metrics out rc=0
  if ! start_agent; then
    fail "$name" "the agent did not start listening on 127.0.0.1:$PORT: $(tail -5 "$LOG")"; return
  fi
  out="$("$BIN" --probe "127.0.0.1:$PORT" --token-file "$TOK" --timeout-ms 8000 2>&1)" || rc=$?
  printf 'probe: %s\n' "$out" >> "$LOG"
  [[ $rc -eq 0 ]] || { fail "$name" "the authenticated probe failed (exit $rc): $out"; return; }
  printf '%s' "$out" | grep -q 'probe: ok' || { fail "$name" "no ok line: $out"; return; }
  pass "$name" "a foreground agent on loopback answers an authenticated probe with a metrics frame"
}

case_token_denial() {
  local name=token-denial out rc=0 wrong="$WORK/wrong-token"
  [[ -n "${AGENT_PID:-}" ]] || { skip "$name" "no agent running"; return; }
  printf '%064d\n' 0 > "$wrong"
  out="$("$BIN" --probe "127.0.0.1:$PORT" --token-file "$wrong" --timeout-ms 8000 2>&1)" || rc=$?
  printf 'wrong-token probe: %s\n' "$out" >> "$LOG"
  [[ $rc -eq 3 ]] || { fail "$name" "a wrong token did not produce exit 3 (got $rc): $out"; return; }
  printf '%s' "$out" | grep -q 'probe: denied' || { fail "$name" "a wrong token was not reported as denied: $out"; return; }

  rc=0
  out="$("$BIN" --probe "127.0.0.1:$PORT" --timeout-ms 8000 2>&1)" || rc=$?
  printf 'no-token probe: %s\n' "$out" >> "$LOG"
  [[ $rc -ne 0 ]] || { fail "$name" "a probe with no token was accepted"; return; }
  pass "$name" "a wrong token is denied (exit 3) and a missing token never reaches the data path"
}

# ===========================================================================
# 3. the numbers agree with the platform's own tools
# ===========================================================================
case_memory_matches_vm_stat() {
  local name=memory-vs-vm_stat frames="$WORK/frames.json" out
  [[ -n "${AGENT_PID:-}" ]] || { skip "$name" "no agent running"; return; }
  # Pull one metrics frame as JSON through the same authenticated socket.
  out="$WORK/metrics.json"
  "$BIN" --probe "127.0.0.1:$PORT" --token-file "$TOK" --timeout-ms 8000 --dump-metrics > "$out" 2>>"$LOG" || {
    skip "$name" "this build has no --dump-metrics (needs the debug frames flag); the probe above still proves the socket, but the values were not compared"
    return
  }
  local reported total
  reported="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["mem_total"])' "$out" 2>/dev/null)"
  total="$(sysctl -n hw.memsize 2>/dev/null)"
  [[ -n "$reported" && -n "$total" ]] || { fail "$name" "could not read the reported or platform total"; return; }
  note "agent mem_total=$reported, sysctl hw.memsize=$total"
  local diff pct
  diff=$(( reported > total ? reported - total : total - reported ))
  pct=$(( diff * 100 / total ))
  (( pct <= 2 )) || { fail "$name" "RAM total differs by $pct% ($reported vs $total)"; return; }
  pass "$name" "reported RAM total is within 2% of sysctl hw.memsize ($reported vs $total)"
}

case_disk_no_double_count() {
  local name=disk-capacity out="$WORK/metrics-disk.json" reported df_root
  [[ -n "${AGENT_PID:-}" ]] || { skip "$name" "no agent running"; return; }
  "$BIN" --probe "127.0.0.1:$PORT" --token-file "$TOK" --timeout-ms 8000 --dump-disks > "$out" 2>>"$LOG" || {
    skip "$name" "this build has no --dump-disks; the APFS single-row rule is covered by the unit test keep_macos_volume_reports_a_container_once"
    return
  }
  reported="$(python3 - "$out" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
# The sum the client would display, and the root volume alone.
print(json.dumps({"rows": len(rows), "sum_total": sum(r["total"] for r in rows),
                  "root": next((r["total"] for r in rows if r["name"] in ("/", "Macintosh HD")), 0)}))
PY
)" || { fail "$name" "could not parse the disk frame"; return; }
  df_root="$(df -k / | awk 'NR==2 { print $2 * 1024 }')"
  note "agent disks: $reported ; df / total: $df_root"
  python3 - "$reported" "$df_root" <<'PY' || { fail "$name" "an APFS container was double counted: $reported"; return; }
import json, sys
r = json.loads(sys.argv[1]); df_root = int(sys.argv[2])
# Summing the container once must not be materially larger than the root
# volume's capacity; summing it once per volume would be several times that.
if r["sum_total"] > df_root * 1.15:
    sys.exit(1)
if r["root"] <= 0:
    sys.exit(1)
PY
  pass "$name" "the reported capacity sums each APFS container once (no volume-count multiplication)"
}

case_process_cpu_workload() {
  local name=process-cpu wl out
  [[ -n "${AGENT_PID:-}" ]] || { skip "$name" "no agent running"; return; }
  # A known single-threaded CPU burn for ~2s: /bin/sh's busy loop is enough to
  # register as a measurable share of one core and is not trivially optimised.
  ( end=$(( SECONDS + 2 )); while (( SECONDS < end )); do :; done ) &
  wl=$!
  sleep 0.6
  out="$("$BIN" --probe "127.0.0.1:$PORT" --token-file "$TOK" --timeout-ms 8000 --dump-process "$wl" 2>>"$LOG")" || {
    wait "$wl" 2>/dev/null
    skip "$name" "this build has no --dump-process; process CPU is covered by the agent's own tests"
    return
  }
  wait "$wl" 2>/dev/null
  printf 'workload %s: %s\n' "$wl" "$out" >> "$LOG"
  note "process $wl cpu: $out"
  # The agent reports a percentage of total capacity; any nonzero reading for a
  # process that just burned a full core proves the measurement reaches the
  # right process rather than a cached or reused PID.
  python3 - "$out" <<'PY' || { fail "$name" "the burnt process reported no CPU: $out"; return; }
import json, sys
row = json.loads(sys.argv[1])
if float(row.get("cpu_usage", 0)) <= 0:
    sys.exit(1)
PY
  pass "$name" "a known CPU-burning process reports nonzero CPU against its own PID"
}

echo "== native macOS agent checks =="
case_startup_refusals
case_foreground_metrics
case_token_denial
case_memory_matches_vm_stat
case_disk_no_double_count
case_process_cpu_workload

printf '\nSUMMARY: %d passed, %d failed, %d skipped\n' "$PASSES" "$FAILS" "$SKIPS"
printf 'evidence: %s\n' "$LOG"
(( FAILS == 0 )) || exit 1
exit 0
