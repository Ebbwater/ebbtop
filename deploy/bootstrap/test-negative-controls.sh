#!/usr/bin/env bash
# Negative controls for the portable harness: each case injects a specific
# regression into a COPY of the tree and asserts the harness reports a FAILURE.
# A suite that passes against a broken installer proves nothing, so these run
# before the suite is trusted.
set -uo pipefail
# The tree under test is THIS checkout, derived from the script's own location.
# It used to be an absolute path into a producer workspace, which meant the
# controls silently validated whatever bytes happened to sit there instead of
# the release candidate beside them.
R="$(cd "$(dirname "$0")/../.." && pwd)"
B32="${EBBTOP_BASH32:-/home/kyler/.hermes/profiles/builder-hard/cache/scratch/bash32/bash-3.2.57/bash}"
WORK=$(mktemp -d)
FAILS=0
pass() { printf 'PASS %s: %s\n' "$1" "$2"; }
fail() { FAILS=$((FAILS+1)); printf 'FAIL %s: %s\n' "$1" "$2" >&2; }

run_harness() { # <tree> -> prints PASS/FAIL/SUMMARY lines
  ( cd "$1" && env -u PYTHONPATH EBBTOP_BASH32="$B32" bash deploy/bootstrap/test-install-portable.sh 2>&1 )
}

# 1. Hard-coding the probe target back to loopback must be caught.
control_loopback_probe() {
  local name=neg-loopback-probe tree="$WORK/t1"
  cp -a "$R" "$tree" 2>/dev/null || { fail "$name" "could not copy tree"; return; }
  # Reinstate the exact bug Captain found: a fixed 127.0.0.1 probe.
  sed -i 's|out="$("$BIN" --probe "$target" --token-file "$TOKENFILE" --timeout-ms 5000 2>&1)"|out="$("$BIN" --probe "127.0.0.1:$PORT" --token-file "$TOKENFILE" --timeout-ms 5000 2>&1)"|' \
    "$tree/deploy/macos/install-agent.sh"
  if ! grep -q 'probe "127.0.0.1:' "$tree/deploy/macos/install-agent.sh"; then
    fail "$name" "the injected loopback regression did not apply"; return
  fi
  local out; out="$(run_harness "$tree")"
  if printf '%s' "$out" | grep -qE '^FAIL .*(fresh-install|probe-target)'; then
    pass "$name" "a loopback-hardcoded probe is caught by $(printf '%s' "$out" | grep -cE '^FAIL') case(s)"
  else
    fail "$name" "a loopback-hardcoded probe was NOT caught"
  fi
}

# 2. Removing the plist rollback must be caught.
control_no_plist_rollback() {
  local name=neg-plist-rollback tree="$WORK/t2"
  cp -a "$R" "$tree" 2>/dev/null || { fail "$name" "could not copy tree"; return; }
  python3 - "$tree/deploy/macos/install-agent.sh" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace('''  if [[ -f "$PLIST_BAK" ]]; then
    mv -f "$PLIST_BAK" "$PLIST"
  else
    rm -f -- "$PLIST"
  fi''', '''  # regression: the plist is left as the new candidate wrote it''')
p.write_text(s)
PY
  local out; out="$(run_harness "$tree")"
  if printf '%s' "$out" | grep -qE '^FAIL .*rollback'; then
    pass "$name" "dropping the plist rollback is caught"
  else
    fail "$name" "dropping the plist rollback was NOT caught"
  fi
}

# 3. Making the activation failure exit before rollback must be caught.
control_no_activation_rollback() {
  local name=neg-activation-rollback tree="$WORK/t3"
  cp -a "$R" "$tree" 2>/dev/null || { fail "$name" "could not copy tree"; return; }
  python3 - "$tree/deploy/macos/install-agent.sh" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace('''  if ! load_agent; then
    echo "launchd refused to load the LaunchAgent ($(launch_domain)/$LABEL)"
    if rollback_previous "launchd bootstrap failed"; then exit 1; fi
    exit 1
  fi''', '''  if ! load_agent; then
    echo "launchd refused to load the LaunchAgent ($(launch_domain)/$LABEL)"
    exit 1
  fi''')
p.write_text(s)
PY
  local out; out="$(run_harness "$tree")"
  if printf '%s' "$out" | grep -qE '^FAIL .*activation'; then
    pass "$name" "an activation failure that skips rollback is caught"
  else
    fail "$name" "an activation failure that skips rollback was NOT caught"
  fi
}

# 4. Dropping the credential pre-check must be caught.
control_no_credential_precheck() {
  local name=neg-credential-precheck tree="$WORK/t4"
  cp -a "$R" "$tree" 2>/dev/null || { fail "$name" "could not copy tree"; return; }
  python3 - "$tree/deploy/macos/install-agent.sh" <<'PY'
import sys, pathlib, re
p = pathlib.Path(sys.argv[1]); s = p.read_text()
start = s.index('  # The credential pair must be intact BEFORE')
end = s.index('  # A port already held by something')
s = s[:start] + s[end:]
p.write_text(s)
PY
  local out; out="$(run_harness "$tree")"
  if printf '%s' "$out" | grep -qE '^FAIL .*credentials'; then
    pass "$name" "dropping the credential pre-check is caught"
  else
    fail "$name" "dropping the credential pre-check was NOT caught"
  fi
}

# 5. Making the bootstrap refuse an existing agent.env (a break of upgrade
#    idempotence) must be caught.
control_upgrade_breaks_token() {
  local name=neg-upgrade-token tree="$WORK/t5"
  cp -a "$R" "$tree" 2>/dev/null || { fail "$name" "could not copy tree"; return; }
  python3 - "$tree/deploy/macos/install-agent.sh" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
# Rotate the token on every install, including upgrades.
s = s.replace('''  else
    chmod 600 "$ENVFILE" 2>/dev/null || true
    chmod 600 "$TOKENFILE" 2>/dev/null || true
  fi''', '''  else
    # regression: rewrite the credential file on every install
    ( umask 077
      printf 'EBBTOP_BIND=%s\\nEBBTOP_TOKEN=%s\\n' "$addr" \\
        "$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \\n')" > "$ENVFILE" )
    awk -F= '$1 == "EBBTOP_TOKEN" { print $2 }' "$ENVFILE" > "$TOKENFILE"
    chmod 600 "$ENVFILE" "$TOKENFILE"
  fi''')
p.write_text(s)
PY
  local out; out="$(run_harness "$tree")"
  if printf '%s' "$out" | grep -qE '^FAIL .*upgrade'; then
    pass "$name" "an upgrade that rotates the token is caught"
  else
    fail "$name" "an upgrade that rotates the token was NOT caught"
  fi
}

# 6. Installer that exits 0 while doing nothing (the classic "reports success,
#    installed nothing") must be caught by the outer verify step.
control_silent_noop_installer() {
  local name=neg-silent-noop tree="$WORK/t6"
  cp -a "$R" "$tree" 2>/dev/null || { fail "$name" "could not copy tree"; return; }
  # Make install-agent.sh claim success without doing anything.
  python3 - "$tree/deploy/macos/install-agent.sh" <<'PY'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
s = s.replace('''install_prebuilt
''', '''# regression: claim success, install nothing
echo "verified: SHA256SUMS ok for BUILD ebbtop-agent ebbtop-agent.plist"
echo "state: running"
echo "probe: ok frames=1"
echo "version: 0.3.0"
exit 0
''')
p.write_text(s)
PY
  local out; out="$(run_harness "$tree")"
  if printf '%s' "$out" | grep -qE '^FAIL .*fresh-install'; then
    pass "$name" "an installer that claims success and installs nothing is caught by the outer verification"
  else
    fail "$name" "a silent no-op installer was NOT caught"
  fi
}

control_loopback_probe
control_no_plist_rollback
control_no_activation_rollback
control_no_credential_precheck
control_upgrade_breaks_token
control_silent_noop_installer

printf '\nNEGATIVE CONTROLS: %d failed\n' "$FAILS"
rm -rf -- "$WORK"
(( FAILS == 0 )) || exit 1
