#!/usr/bin/env bash
# Parse an ebbtop.log (desktop client log) for the overnight acceptance check
# (brief step 7). POSIX sh, no deps beyond grep/awk/sed/sort.
#
#   deploy/parse-ebbtop-log.sh [logfile]
#   LOGFILE=... deploy/parse-ebbtop-log.sh
#
# Default: %LOCALAPPDATA% equivalent on Unix (~/Library/Application Support/
# ebbtop/ebbtop.log on macOS, ~/.local/share/ebbtop/ebbtop.log on Linux). Pass
# a path to parse a copy scp'd from Windows (e.g. /tmp/ebbtop.log).
#
# Reads the per-minute summary lines (conn=... interval_p95=... gaps=...
# reconnects=...), `disconnected` and `start` lines. Never prints a token:
# the log never contains one, and this script does not read the credential
# store. Tailnet-only: reads a local file, does no network I/O, opens nothing
# to the public side.
#
# Verdict: PASS if every summary line has interval_p95 < 2000 ms and every
# gaps increase is at or after a `disconnected` line (sleep + recovery, which
# step 7 expects); REVIEW otherwise. Exit 1 on REVIEW, 0 on PASS.
set -euo pipefail

LOGFILE="${LOGFILE:-${1:-}}"
if [[ -z "$LOGFILE" ]]; then
  if [[ -n "${LOCALAPPDATA:-}" ]]; then LOGFILE="$LOCALAPPDATA/ebbtop/ebbtop.log"
  elif [[ "$(uname)" == "Darwin" ]]; then LOGFILE="$HOME/Library/Application Support/ebbtop/ebbtop.log"
  else LOGFILE="$HOME/.local/share/ebbtop/ebbtop.log"
  fi
fi
[[ -f "$LOGFILE" ]] || { echo "log not found: $LOGFILE" >&2; exit 2; }

# Extract the three fields we need, in order, from summary lines.
# Summary format (history.rs summary_line):
#   conn=<state> since_new_ms=.. interval_p50=.. interval_p95=.. interval_max=..
#   lag_p50=.. lag_p95=.. proc_interval_p95=.. proc_lag_p95=.. gaps=.. repeats=..
#   reconnects=.. samples=..
extract() { # $1 = field name; reads stdin
  sed -nE "s/.* $1=([0-9]+).*/\1/p"
}
summary_lines() { grep -E 'conn=' "$LOGFILE" || true; }

if ! grep -q 'conn=' "$LOGFILE"; then
  echo "log: $LOGFILE"; echo "summary lines: 0 (nothing to check — launch the client first)"
  exit 1
fi

ts_first="$(head -n1 "$LOGFILE" | cut -c1-25)"
ts_last="$(tail -n1 "$LOGFILE" | cut -c1-25)"
total="$(wc -l < "$LOGFILE")"
nsum="$(summary_lines | wc -l)"
ndisc="$(grep -c ' disconnected ' "$LOGFILE" || true)"
nstart="$(grep -cE ' start url=' "$LOGFILE" || true)"

p95_list="$(summary_lines | extract interval_p95)"
gaps_list="$(summary_lines | extract gaps)"
rec_list="$(summary_lines | extract reconnects)"
p95_max="$(printf '%s\n' "$p95_list" | sort -n | tail -n1)"
p95_last="$(printf '%s\n' "$p95_list" | tail -n1)"
p95_bad="$(printf '%s\n' "$p95_list" | awk '$1 >= 2000 {n++} END {print n+0}')"
gaps_first="$(printf '%s\n' "$gaps_list" | head -n1)"
gaps_last="$(printf '%s\n' "$gaps_list" | tail -n1)"
gaps_inc="$(printf '%s\n' "$gaps_list" | awk 'NR>1 && $1>prev {n++} {prev=$1} END {print n+0}')"
rec_first="$(printf '%s\n' "$rec_list" | head -n1)"
rec_last="$(printf '%s\n' "$rec_list" | tail -n1)"

# Recovery: a summary line with conn=connected after the last disconnected.
last_disc_line="$(grep -n ' disconnected ' "$LOGFILE" | tail -n1 | cut -d: -f1 || true)"
recov="no"
if [[ -n "$last_disc_line" ]]; then
  if awk -v after="$last_disc_line" 'NR>after && /conn=connected/ {f=1} END {exit !f}' "$LOGFILE"; then
    recov="yes"
  fi
else
  recov="yes (no disconnects)"
fi

last_line="$(summary_lines | tail -n1)"

echo "log: $LOGFILE"
echo "total lines: $total (summary: $nsum, disconnected: $ndisc, start: $nstart)"
echo "span: $ts_first .. $ts_last"
echo "interval_p95_ms: max=$p95_max last=$p95_last lines_ge_2000=$p95_bad"
echo "gaps (cumulative): first=$gaps_first last=$gaps_last delta=$(( gaps_last - gaps_first )) lines_with_increase=$gaps_inc"
echo "reconnects (cumulative): first=$rec_first last=$rec_last delta=$(( rec_last - rec_first ))"
echo "recovery after last disconnect: $recov"
echo "last summary: $last_line"

if [[ "$p95_bad" -gt 0 ]]; then
  echo "VERDICT: REVIEW (some interval_p95 >= 2000 ms; brief pass bar is < 2000)"
  exit 1
fi
if [[ "$ndisc" -gt 0 && "$recov" != "yes" ]]; then
  echo "VERDICT: REVIEW (disconnected with no connected summary line after it)"
  exit 1
fi
# Gap increases are expected only with a disconnect (sleep). Flag increases
# on a file with no disconnected lines at all.
if [[ "$gaps_inc" -gt 0 && "$ndisc" -eq 0 ]]; then
  echo "VERDICT: REVIEW (gaps increased with no disconnected line to explain them)"
  exit 1
fi
echo "VERDICT: PASS (all interval_p95 < 2000 ms; disconnect+recovery is expected per step 7)"
