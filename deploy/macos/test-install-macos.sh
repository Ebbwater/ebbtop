#!/usr/bin/env bash
# macOS installer fixture suite.
#
# Every case runs with HOME pointed at a throwaway directory and with stubs for
# the platform tools (launchctl, lsof, ps, plutil, file, sw_vers), so no
# service-manager call, no write to the real ~/Library and no listener on this
# host is ever touched. The installer and the plist template under test are
# this checkout's own files, byte for byte.
#
# This suite runs on Linux as well as macOS: it sets
# EBBTOP_ALLOW_UNSUPPORTED_HOST=1 so the installer's Darwin/arm64 guard steps
# aside, and drives the launchd path through the stub on either OS. Real
# LaunchAgent behavior is verified separately in a logged-in Mac session.
#
# Usage:
#   bash deploy/macos/test-install-macos.sh
#   EBBTOP_BASH32=/path/to/bash-3.2 bash deploy/macos/test-install-macos.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
NOTICES_SRC="$ROOT/THIRD-PARTY-NOTICES"
INSTALLER="$ROOT/deploy/macos/install-agent.sh"
PLIST="$ROOT/deploy/macos/ebbtop-agent.plist.in"
PACKAGER="$ROOT/deploy/macos/package-agent.sh"
MEMBERCHECK="$ROOT/deploy/macos/member-check.sh"

[ -f "$INSTALLER" ] || { echo "missing $INSTALLER" >&2; exit 1; }
[ -f "$PLIST" ] || { echo "missing $PLIST" >&2; exit 1; }

PASSES=0; FAILS=0
pass() { PASSES=$((PASSES+1)); printf 'PASS %s: %s\n' "$1" "$2"; }
fail() { FAILS=$((FAILS+1)); printf 'FAIL %s: %s\n' "$1" "$2" >&2; }
note() { printf '  - %s\n' "$*"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-mac-fixtures.XXXXXXXX")"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

STUB="$WORK/stub"
mkdir -p "$STUB"
ST="$WORK/stub-state"
mkdir -p "$ST"
echo 100.64.0.5 > "$ST/bind"

# --- stubs ------------------------------------------------------------------
cat > "$STUB/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo Darwin ;;
  -m) echo arm64 ;;
  *)  echo Darwin ;;
esac
EOF

cat > "$STUB/sw_vers" <<'EOF'
#!/usr/bin/env bash
echo 14.6.1
EOF

# plutil -lint: a real XML parse, so a truncated or malformed render is caught
# here exactly as macOS's plutil would catch it.
cat > "$STUB/plutil" <<'EOF'
#!/usr/bin/env bash
f=''
for a in "$@"; do f="$a"; done
[[ -f "$f" ]] || { echo "plutil: no such file: $f" >&2; exit 1; }
exec python3 - "$f" <<'PY'
import sys, xml.etree.ElementTree as ET
p = sys.argv[1]
try:
    root = ET.parse(p).getroot()
except ET.ParseError as e:
    sys.stderr.write(f"plutil: {p}: {e}\n"); raise SystemExit(1)
if root.tag != 'plist' or root.find('dict') is None:
    sys.stderr.write(f"plutil: {p}: not a plist dictionary\n"); raise SystemExit(1)
text = open(p, encoding='utf-8').read()
for n in ('AGENT_BIN', 'ENV_FILE', 'PORT', 'LOG_DIR'):
    if f'__{n}__' in text:
        sys.stderr.write(f"plutil: {p}: unrendered __{n}__\n"); raise SystemExit(1)
PY
EOF

# A job registry, with health derived from the INSTALLED BINARY: a *-not-serving
# candidate activates but never answers; a *-refuse-boot one cannot activate.
cat > "$STUB/launchctl" <<'EOF'
#!/usr/bin/env bash
STATE="${FIX_STUB_STATE:?}"
LABEL=com.ebbwater.ebbtop-agent
BIN="$HOME/Library/Application Support/ebbtop-agent/bin/ebbtop-agent"
printf 'launchctl %s\n' "$*" >> "$STATE/launchctl.log"
v=''; [[ -x "$BIN" ]] && v="$("$BIN" --version 2>/dev/null || true)"
serving=0
case "$v" in *not-serving*|*refuse-boot*) serving=0 ;; '' ) serving=0 ;; *) serving=1 ;; esac
bootable=1
case "$v" in *refuse-boot*) bootable=0 ;; esac
case "${1:-}" in
  print)
    case "${2:-}" in
      */"$LABEL")
        [[ -f "$STATE/loaded" ]] || { echo "Could not find service \"$LABEL\"" >&2; exit 113; }
        [[ $serving -eq 1 ]] && echo "	state = running" || echo "	state = exited"
        echo "	program = $BIN"
        exit 0 ;;
      *) echo "gui domain (logged in)"; exit 0 ;;
    esac ;;
  bootstrap)
    if [[ -f "$STATE/bootstrap-fails" ]] || [[ $bootable -eq 0 ]]; then
      echo "Bootstrap failed: 5: Input/output error" >&2; exit 5
    fi
    : > "$STATE/loaded"; exit 0 ;;
  bootout) rm -f "$STATE/loaded"; exit 0 ;;
  *) exit 0 ;;
esac
EOF

cat > "$STUB/lsof" <<'EOF'
#!/usr/bin/env bash
STATE="${FIX_STUB_STATE:?}"
BIN="$HOME/Library/Application Support/ebbtop-agent/bin/ebbtop-agent"
# A foreign holder is independent of whether this agent is installed, so it is
# checked first: that is the case where a fresh install must refuse.
if [[ -f "$STATE/foreign-holder" ]]; then
  case " $* " in *" -t "*) echo 9999 ;; *) echo "COMMAND   PID USER   FD TYPE DEVICE SIZE/OFF NODE NAME"
                                   echo "postgres 9999 kyler 7u IPv4 0x1 0t0 TCP *:45976 (LISTEN)" ;; esac
  exit 0
fi
[[ -x "$BIN" ]] || exit 1
"$BIN" --version 2>/dev/null | grep -qE '(not-serving|refuse-boot)' && exit 1
case " $* " in
  *" -t "*) echo 4242; exit 0 ;;
esac
echo "COMMAND     PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME"
echo "ebbtop-age 4242 kyler 7u IPv4 0x1 0t0 TCP $(sed -n 's/^EBBTOP_BIND=//p' "$HOME/Library/Application Support/ebbtop-agent/agent.env" 2>/dev/null | head -n 1):45976 (LISTEN)"
EOF

cat > "$STUB/ps" <<'EOF'
#!/usr/bin/env bash
if [[ -f "${FIX_STUB_STATE:?}/foreign-holder" ]]; then
  echo /usr/lib/postgresql/16/bin/postgres
else
  echo "$HOME/Library/Application Support/ebbtop-agent/bin/ebbtop-agent"
fi
EOF

# file(1): the fixture agent is a shell script standing in for the arm64
# binary; report what a real Mach-O would. A file whose name contains
# not-a-macho is reported honestly so the refusal path can be exercised.
cat > "$STUB/file" <<'EOF'
#!/usr/bin/env bash
f=''
for a in "$@"; do f="$a"; done
case "$f" in
  *not-a-macho*) echo "ASCII text" ;;
  *) echo "Mach-O 64-bit executable arm64" ;;
esac
EOF

chmod 755 "$STUB"/*

# --- the fixture agent -------------------------------------------------------
# Three flavours by version string: plain (serves), *-not-serving (activates but
# never answers), *-refuse-boot (cannot activate at all).
make_agent() { # <path> <version>
  local serves=1
  case "$2" in *not-serving*|*refuse-boot*) serves=0 ;; esac
  cat > "$1" <<EOF
#!/usr/bin/env bash
STATE="\${FIX_STUB_STATE:?}"
SERVES=$serves
case "\${1:-}" in
  --version) echo "ebbtop-agent $2" ;;
  --probe)
    target="\${2:-}"
    printf '%s' "\$target" >> "\$STATE/probe.targets"
    [ "\$SERVES" = 1 ] || { echo "probe: failed: agent is not serving"; exit 1; }
    # The address the agent actually serves on is the one in ITS OWN env file,
    # read exactly as the agent reads it. That makes the fixture a real check of
    # "did the installer probe the address it configured" for every bind,
    # including loopback, IPv4 LAN and IPv6.
    want="\$(sed -n 's/^EBBTOP_BIND=//p' "\$HOME/Library/Application Support/ebbtop-agent/agent.env" 2>/dev/null | head -n 1)"
    [ -n "\$want" ] || { echo "probe: failed: no bind address configured"; exit 1; }
    case "\$target" in
      "[\$want]:"*|"\$want:"*) ;;
      *) echo "probe: failed: connect \$target: refused"; exit 1 ;;
    esac
    echo "probe: ok frames=1" ;;
  *) exit 0 ;;
esac
EOF
  chmod 755 "$1"
}

make_pkg() { # <out> <version> <stage> [plist-marker]
  local out="$1" ver="$2" stage="$3" marker="${4:-}"
  mkdir -p "$stage"
  make_agent "$stage/ebbtop-agent" "$ver"
  cp "$INSTALLER" "$stage/install-agent.sh"; chmod 755 "$stage/install-agent.sh"
  cp "$PLIST" "$stage/ebbtop-agent.plist"
  # A marker makes this candidate's rendered LaunchAgent distinguishable from
  # the previous install's, so "the rollback restored the old plist" is an
  # observable fact rather than a comparison of two identical files. The
  # template itself may not contain '--' inside a comment, so the marker is a
  # plain XML comment line.
  if [[ -n "$marker" ]]; then
    printf '<!-- %s -->\n' "$marker" >> "$stage/ebbtop-agent.plist"
  fi
  chmod 644 "$stage/ebbtop-agent.plist"
  {
    echo "name=ebbtop-agent"
    echo "version=$ver"
    echo "commit=4eb33ee1fa2d6afb376e09ea23fa87cba041544d"
    echo "target=aarch64-apple-darwin"
    echo "min_macos=13.0"
  } > "$stage/BUILD"
  cp "$NOTICES_SRC" "$stage/THIRD-PARTY-NOTICES"
  ( cd "$stage" && shasum -a 256 BUILD ebbtop-agent ebbtop-agent.plist THIRD-PARTY-NOTICES > SHA256SUMS )
}

new_home() { # <name> -> FIXHOME
  FIXHOME="$WORK/homes/$1"
  mkdir -p "$FIXHOME"
  rm -f "$ST"/*; echo 100.64.0.5 > "$ST/bind"
  : > "$ST/probe.targets"; : > "$ST/launchctl.log"
  SVC="$FIXHOME/Library/Application Support/ebbtop-agent"
  LA="$FIXHOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist"
}

run_installer() { # <pkg dir> <args...> ; extra env via RUN_ENV
  local dir="$1"; shift
  ( env -i HOME="$FIXHOME" TMPDIR="$WORK/tmp" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$ST" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 \
      ${RUN_ENV:+$RUN_ENV} \
      "$(command -v bash)" "$dir/install-agent.sh" "$@" ) 2>&1
}

GOOD="$WORK/pkg-good"
make_pkg "$WORK/good.tar.gz" 0.3.0 "$GOOD"
mkdir -p "$WORK/tmp"

# ===========================================================================
# 1. a clean install: verified package, 0600 credential file, rendered plist,
#    authenticated probe at the configured bind.
# ===========================================================================
case_fresh() {
  local name=fresh out rc=0
  new_home "$name"
  out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "install exited $rc: $out"; return; }
  [[ -f "$SVC/agent.env" ]] || { fail "$name" "no agent.env"; return; }
  [[ "$(stat -c %a "$SVC/agent.env" 2>/dev/null || stat -f %Lp "$SVC/agent.env")" == 600 ]] \
    || { fail "$name" "agent.env mode is not 600"; return; }
  grep -qx 'EBBTOP_BIND=100.64.0.5' "$SVC/agent.env" || { fail "$name" "agent.env bind is wrong"; return; }
  grep -qx 'EBBTOP_AGENT_DISCOVERY=0' "$SVC/agent.env" || { fail "$name" "discovery is not off"; return; }
  # The agent's pairing state must land in the directory --purge deletes: the
  # installer pins EBBTOP_STATE_DIR so a --pair from a plain shell (and the
  # LaunchAgent) cannot fall back to a different default.
  grep -qx "EBBTOP_STATE_DIR=$SVC" "$SVC/agent.env" \
    || { fail "$name" "agent.env does not pin EBBTOP_STATE_DIR=$SVC (pairing state would escape --purge)"; return; }
  [[ -f "$LA" ]] || { fail "$name" "no LaunchAgent"; return; }
  grep -q "$SVC/bin/ebbtop-agent" "$LA" || { fail "$name" "plist does not name the binary"; return; }
  if grep -Eq 'EBBTOP_TOKEN=|EBBTOP_BIND=' "$LA"; then fail "$name" "plist carries a credential or address"; return; fi
  grep -qx '100.64.0.5:45976' "$ST/probe.targets" || { fail "$name" "probe target: $(tr '\n' ' ' < "$ST/probe.targets")"; return; }
  printf '%s' "$out" | grep -q 'probe: ok frames=1' || { fail "$name" "no probe ok line: $out"; return; }
  printf '%s' "$out" | grep -q 'starts at login: yes' || { fail "$name" "no login-startup statement"; return; }
  local tok; tok="$(cat "$SVC/ebbtop.token")"
  printf '%s' "$out" | grep -qF "$tok" && { fail "$name" "the token value was printed"; return; }
  pass "$name" "clean install: verified package, 0600 credentials, rendered plist with no secret, probe at the tailnet bind, login startup stated, token never printed"
}

# ===========================================================================
# 2. the package itself: a wrong digest, an extra member, a leaked credential
#    in the plist, and a non-Mach-O binary are all refused before a write.
# ===========================================================================
case_package_refusals() {
  local name=package-refusals out rc bad

  # (a) a member that no longer matches SHA256SUMS
  new_home pkg-a
  bad="$WORK/bad-digest"
  cp -a "$GOOD" "$bad"
  printf '#!/usr/bin/env bash\necho tampered\n' > "$bad/ebbtop-agent"; chmod 755 "$bad/ebbtop-agent"
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$bad" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a tampered binary was installed"; return; }
  printf '%s' "$out" | grep -q 'checksum verification failed' || { fail "$name" "no checksum refusal: $out"; return; }
  [[ ! -e "$SVC/agent.env" ]] || { fail "$name" "agent.env was written despite the refusal"; return; }

  # (b) an unexpected extra member
  new_home pkg-b
  bad="$WORK/bad-extra"
  cp -a "$GOOD" "$bad"
  echo extra > "$bad/extra-file"
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$bad" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "an extra member was accepted"; return; }
  printf '%s' "$out" | grep -q "unexpected member 'extra-file'" || { fail "$name" "no extra-member refusal: $out"; return; }

  # (c) a plist that assigns a credential
  new_home pkg-c
  bad="$WORK/bad-plist-secret"
  cp -a "$GOOD" "$bad"
  printf 'EBBTOP_TOKEN=%064d\n' 0 >> "$bad/ebbtop-agent.plist"
  ( cd "$bad" && shasum -a 256 BUILD ebbtop-agent ebbtop-agent.plist THIRD-PARTY-NOTICES > SHA256SUMS )
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$bad" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a plist with a token assignment was accepted"; return; }
  printf '%s' "$out" | grep -q 'assigns a credential or a bind address' || { fail "$name" "no plist-secret refusal: $out"; return; }

  # (d) a binary that is not a Mach-O
  new_home pkg-d
  bad="$WORK/bad-not-a-macho"
  cp -a "$GOOD" "$bad"
  install -m 755 "$INSTALLER" "$bad/ebbtop-agent"   # a shell script, not a Mach-O
  ( cd "$bad" && shasum -a 256 BUILD ebbtop-agent ebbtop-agent.plist THIRD-PARTY-NOTICES > SHA256SUMS )
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$bad" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a non-Mach-O binary was accepted"; return; }
  printf '%s' "$out" | grep -q 'not a Mach-O executable' || { fail "$name" "no Mach-O refusal: $out"; return; }

  pass "$name" "a tampered member, an extra member, a plist assigning a credential and a non-Mach-O binary are each refused before anything is written"
}

# ===========================================================================
# 3. an upgrade keeps the token and bind; a failed upgrade restores the binary
#    AND the plist.
# ===========================================================================
case_upgrade_and_rollback() {
  local name=upgrade out rc=0
  new_home "$name"
  out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline exited $rc: $out"; return; }
  local tok bin_sha plist_sha
  tok="$(cat "$SVC/ebbtop.token")"
  bin_sha="$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)"
  plist_sha="$(shasum -a 256 "$LA" | cut -d' ' -f1)"

  # a good newer candidate
  local newer="$WORK/pkg-newer"
  make_pkg "$WORK/newer.tar.gz" 0.3.2 "$newer" "candidate 0.3.2"
  out="$(run_installer "$newer" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "the newer install exited $rc: $out"; return; }
  [[ "$(cat "$SVC/ebbtop.token")" == "$tok" ]] || { fail "$name" "the upgrade changed the token"; return; }
  [[ "$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)" != "$bin_sha" ]] \
    || { fail "$name" "the newer candidate did not replace the binary"; return; }

  # a candidate that installs but never serves
  local bad="$WORK/pkg-bad"
  local newer_sha newer_plist
  newer_sha="$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)"
  newer_plist="$(shasum -a 256 "$LA" | cut -d' ' -f1)"
  make_pkg "$WORK/bad.tar.gz" 0.3.3-not-serving "$bad" "candidate 0.3.3"
  rc=0; out="$(run_installer "$bad" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a candidate that cannot serve was accepted: $out"; return; }
  # The candidate's plist must NOT be the one left on disk: the marker is what
  # makes "the old plist was restored" observable at all (a baseline and a
  # candidate rendered from the same template are otherwise byte-identical).
  if grep -q 'candidate 0.3.3' "$LA" 2>/dev/null; then
    fail "$name" "the failed candidate's plist was left installed"; return
  fi
  # The rollback restores the install that was there BEFORE this attempt, i.e.
  # the 0.3.2 candidate, not the original 0.3.0. Restoring anything else would
  # discard a good upgrade.
  [[ "$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)" == "$newer_sha" ]] \
    || { fail "$name" "rollback did not restore the pre-attempt (0.3.2) binary"; return; }
  [[ "$(shasum -a 256 "$LA" | cut -d' ' -f1)" == "$newer_plist" ]] \
    || { fail "$name" "rollback did not restore the pre-attempt plist"; return; }
  printf '%s' "$out" | grep -q 'the previous version is running again' \
    || { fail "$name" "the rollback was not reported: $out"; return; }

  # an activation failure is attributed to launchd, not to readiness
  local unbootable="$WORK/pkg-unbootable"
  make_pkg "$WORK/unbootable.tar.gz" 0.3.4-refuse-boot "$unbootable" "candidate 0.3.4"
  rc=0; out="$(run_installer "$unbootable" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "an unbootable candidate was accepted"; return; }
  if grep -q 'candidate 0.3.4' "$LA" 2>/dev/null; then
    fail "$name" "the unbootable candidate's plist was left installed"; return
  fi
  printf '%s' "$out" | grep -q 'launchd bootstrap failed' \
    || { fail "$name" "the activation failure was not attributed to launchd: $out"; return; }
  if printf '%s' "$out" | grep -q 'readiness probe failed'; then
    fail "$name" "an activation failure was reported as a probe failure"; return
  fi
  [[ "$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)" == "$newer_sha" ]] \
    || { fail "$name" "an activation failure did not restore the binary"; return; }
  pass "$name" "an upgrade keeps the token; a bad candidate restores binary and plist; an unbootable candidate is reported as an activation failure and restores the binary"
}

# ===========================================================================
# 4. credential files that cannot be verified refuse before any change.
# ===========================================================================
case_credentials() {
  local name=credentials out rc=0
  new_home "$name"
  out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline exited $rc: $out"; return; }
  local sha; sha="$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)"

  mv "$SVC/ebbtop.token" "$SVC/tok.hidden"
  rc=0; out="$(run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a missing token file was accepted"; return; }
  printf '%s' "$out" | grep -q 'refusing before any change' || { fail "$name" "no pre-change refusal: $out"; return; }
  [[ "$(shasum -a 256 "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)" == "$sha" ]] \
    || { fail "$name" "the binary was replaced despite the missing token"; return; }
  mv "$SVC/tok.hidden" "$SVC/ebbtop.token"

  : > "$SVC/ebbtop.token"
  rc=0; out="$(run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "an empty token file was accepted"; return; }
  printf '%s' "$out" | grep -q 'is empty, so the token cannot be verified' || { fail "$name" "no empty-token refusal: $out"; return; }

  printf '%064d\n' 0 > "$SVC/ebbtop.token"
  rc=0; out="$(run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a token mismatch was accepted"; return; }
  printf '%s' "$out" | grep -q 'carry different tokens' || { fail "$name" "no mismatch refusal: $out"; return; }
  printf '%s\n' '.*' > "$SVC/ebbtop.token"
  rc=0; out="$(run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -ne 0 ]] && printf '%s' "$out" | grep -q 'carry different tokens' \
    || { fail "$name" "the token file was interpreted as a pattern"; return; }
  pass "$name" "a missing, empty or disagreeing token file refuses before the current install is touched"
}

# ===========================================================================
# 5. bind validation, and a foreign process on the port.
# ===========================================================================
case_binds_and_port() {
  local name=binds out rc bad
  for bad in 0.0.0.0 :: 8.8.8.8 172.32.1.1 100.128.0.1 2001:4860:4860::8888; do
    new_home "binds-$(printf '%s' "$bad" | tr -c 'a-z0-9' '-')"
    rc=0; out="$(RUN_ENV="EBBTOP_ADDR=$bad" run_installer "$GOOD" --prebuilt)" || rc=$?
    [[ $rc -ne 0 ]] || { fail "$name" "bind $bad was accepted"; return; }
    printf '%s' "$out" | grep -q "refusing bind address '$bad'" \
      || { fail "$name" "bind $bad was not refused clearly: $out"; return; }
    [[ ! -e "$FIXHOME/Library" ]] || { fail "$name" "bind $bad was refused but ~/Library exists"; return; }
  done
  # loopback is allowed and reported
  new_home binds-loopback
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=127.0.0.1" run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "a loopback bind was refused: $out"; return; }
  printf '%s' "$out" | grep -q 'reachable from: this Mac only' \
    || { fail "$name" "a loopback install did not say it is local-only: $out"; return; }
  # a foreign process holding the port
  new_home binds-foreign
  : > "$ST/foreign-holder"
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$GOOD" --prebuilt)" || rc=$?
  rm -f "$ST/foreign-holder"
  [[ $rc -ne 0 ]] || { fail "$name" "a port held by postgres was accepted"; return; }
  printf '%s' "$out" | grep -q 'already used by: */postgres' \
    || { printf '%s' "$out" | grep -q 'already used by' || { fail "$name" "no port-holder refusal: $out"; return; }; }
  # and the agent's OWN listener is not treated as foreign (the lsof
  # truncation bug: COMMAND is capped at 9 chars, so a name test refuses a
  # legitimate upgrade)
  new_home binds-own
  rc=0; out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline exited $rc: $out"; return; }
  rc=0; out="$(run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "an upgrade was refused because the agent's own truncated lsof COMMAND was treated as foreign: $out"; return; }
  pass "$name" "wildcard, public and out-of-range binds are refused with no writes; loopback is allowed and called local-only; a foreign listener is refused while the agent's own is not"
}

# ===========================================================================
# 6. uninstall keeps state; --purge deletes it.
# ===========================================================================
case_uninstall() {
  local name=uninstall out rc=0
  new_home "$name"
  out="$(RUN_ENV="EBBTOP_ADDR=100.64.0.5" run_installer "$GOOD" --prebuilt)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline exited $rc: $out"; return; }
  out="$(run_installer "$GOOD" --uninstall)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "uninstall exited $rc: $out"; return; }
  [[ ! -f "$LA" ]] || { fail "$name" "the plist survived"; return; }
  [[ ! -f "$SVC/bin/ebbtop-agent" ]] || { fail "$name" "the binary survived"; return; }
  [[ -f "$SVC/ebbtop.token" && -f "$SVC/agent.env" ]] || { fail "$name" "an ordinary uninstall removed state"; return; }
  out="$(run_installer "$GOOD" --uninstall --purge)" || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "--purge exited $rc: $out"; return; }
  [[ ! -e "$SVC" ]] || { fail "$name" "--purge left the state directory"; return; }
  printf '%s' "$out" | grep -q 'token: deleted with the state directory' \
    || { fail "$name" "--purge did not say the token went"; return; }
  # --purge without --uninstall is a usage error
  rc=0; out="$(run_installer "$GOOD" --purge)" || rc=$?
  [[ $rc -eq 2 ]] || { fail "$name" "--purge alone exited $rc, expected 2"; return; }
  pass "$name" "uninstall keeps the token and bind, --purge deletes the state directory and says so, --purge alone is a usage error"
}

# ===========================================================================
# 7. the installer and template parse under Bash 3.2 (macOS's /bin/bash).
# ===========================================================================
case_bash32() {
  local name=bash32
  local b32="${EBBTOP_BASH32:-}"
  if [[ -z "$b32" ]]; then
    for c in /bin/bash /home/kyler/.hermes/profiles/builder-hard/cache/scratch/bash32/bash-3.2.57/bash; do
      if [[ -x "$c" ]] && "$c" --version 2>/dev/null | grep -q 'version 3\.2'; then b32="$c"; break; fi
    done
  fi
  if [[ -z "$b32" ]] || ! "$b32" --version 2>/dev/null | grep -q 'version 3\.2'; then
    printf 'SKIP %s: no Bash 3.2 binary; the compatibility claim is not made for this run\n' "$name"
    return
  fi
  local f
  for f in "$INSTALLER" "$PACKAGER" "$MEMBERCHECK"; do
    "$b32" -n "$f" || { fail "$name" "Bash 3.2 -n rejected $f"; return; }
  done
  # Parsing is not enough: the member proof used to be a Bash 4 builtin that
  # parsed fine and died at runtime on macOS stock Bash. EXECUTE it under 3.2
  # against a correct archive and against an extra-member archive.
  local good_pkg="$WORK/b32-members-good.tgz" bad_pkg="$WORK/b32-members-bad.tgz" mrc=0
  local badstage="$WORK/b32-members-stage"
  ( cd "$GOOD" && tar -czf "$good_pkg" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES )
  # A copy, never $GOOD itself: the installer must not see a polluted package.
  cp -a "$GOOD" "$badstage"
  ( cd "$badstage" && echo extra > sneaky.env \
      && tar -czf "$bad_pkg" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES sneaky.env )
  "$b32" -c '. "$1"; member_set_matches "$2" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES' \
      _ "$MEMBERCHECK" "$good_pkg" > "$WORK/b32.members.out" 2>&1 || mrc=$?
  [[ $mrc -eq 0 ]] || { fail "$name" "the member proof failed under Bash 3.2 on a CORRECT archive (exit $mrc): $(cat "$WORK/b32.members.out")"; return; }
  mrc=0
  "$b32" -c '. "$1"; member_set_matches "$2" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES' \
      _ "$MEMBERCHECK" "$bad_pkg" > "$WORK/b32.members.bad.out" 2>&1 || mrc=$?
  [[ $mrc -eq 1 ]] || { fail "$name" "the member proof did not refuse an extra member under Bash 3.2 (exit $mrc): $(cat "$WORK/b32.members.bad.out")"; return; }
  grep -qF 'sneaky.env' "$WORK/b32.members.bad.out" || { fail "$name" "the 3.2 refusal did not name the extra member: $(cat "$WORK/b32.members.bad.out")"; return; }
  # and a real install runs under it
  new_home bash32-run
  local out rc=0
  ( env -i HOME="$FIXHOME" TMPDIR="$WORK/tmp" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$ST" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 EBBTOP_ADDR=100.64.0.5 \
      "$b32" "$GOOD/install-agent.sh" --prebuilt ) > "$WORK/b32.out" 2>&1 || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "a real install failed under Bash 3.2 (exit $rc): $(cat "$WORK/b32.out")"; return; }
  [[ -f "$SVC/agent.env" && -f "$LA" ]] || { fail "$name" "the Bash 3.2 install left no state"; return; }
  pass "$name" "the installer, packager and member proof parse under $("$b32" --version | head -1); the member proof EXECUTES correctly (accepts the real set, refuses an extra member) and a full install completes under it"
}

case_fresh
case_package_refusals
case_upgrade_and_rollback
case_credentials
case_binds_and_port
case_uninstall
case_bash32

printf '\nSUMMARY: %d passed, %d failed\n' "$PASSES" "$FAILS"
(( FAILS == 0 )) || exit 1
exit 0
