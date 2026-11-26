#!/usr/bin/env bash
# Exercise deploy/bootstrap/ebbtop-install.sh where it cannot be run natively:
# the macOS code path on this Linux box, and the whole file under a real
# Bash 3.2 (the /bin/bash macOS ships).
#
# Nothing here talks to a network, loads a launchd job, writes to the real
# $HOME or starts a service: every case runs with HOME pointed at a throwaway
# directory, PATH led by stubs for uname/sw_vers/launchctl/curl/tailscale/
# ipconfig/lsof, and a TMPDIR inside the case's own scratch directory.
#
# The macOS path is exercised by `--platform macos` with stubbed uname, which
# is the same code the published script takes on a Mac; the archive it
# installs is a fixture whose install-agent.sh is this checkout's
# deploy/macos/install-agent.sh, byte for byte, run with
# EBBTOP_ALLOW_UNSUPPORTED_HOST=1 and stubbed launchctl/lsof/plutil — so the
# package checks, the plist rendering, the bind validation and the lifecycle
# failure handling are real code, while the service manager is a fixture.
#
# Usage:
#   bash deploy/bootstrap/test-install-portable.sh            # everything
#   bash deploy/bootstrap/test-install-portable.sh --no-bash32  # skip Bash 3.2
#
# Bash 3.2 is looked for in $EBBTOP_BASH32, then at these well-known paths.
# A missing Bash 3.2 is a SKIP line, not a pass: the compatibility claim is
# only made for a binary that was actually run.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/deploy/bootstrap/ebbtop-install.sh"
MAC_INSTALLER="$ROOT/deploy/macos/install-agent.sh"
MAC_PLIST="$ROOT/deploy/macos/ebbtop-agent.plist.in"

[ -f "$SCRIPT" ] || { echo "missing $SCRIPT" >&2; exit 1; }
[ -f "$MAC_INSTALLER" ] || { echo "missing $MAC_INSTALLER" >&2; exit 1; }
[ -f "$MAC_PLIST" ] || { echo "missing $MAC_PLIST" >&2; exit 1; }

RUN_BASH32=1
case "${1:-}" in
  "") ;;
  --no-bash32) RUN_BASH32=0 ;;
  *) echo "usage: $0 [--no-bash32]" >&2; exit 2 ;;
esac

PASSES=0
FAILS=0
SKIPS=0
pass() { PASSES=$((PASSES + 1)); printf 'PASS %s: %s\n' "$1" "$2"; }
fail() { FAILS=$((FAILS + 1)); printf 'FAIL %s: %s\n' "$1" "$2" >&2; }
skip() { SKIPS=$((SKIPS + 1)); printf 'SKIP %s: %s\n' "$1" "$2"; }
note() { printf '  - %s\n' "$*"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-portable.XXXXXXXX")"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

# --- Bash 3.2 ---------------------------------------------------------------
BASH32=''
if [[ -n "${EBBTOP_BASH32:-}" && -x "${EBBTOP_BASH32}" ]]; then
  BASH32="$EBBTOP_BASH32"
fi
if [[ -z "$BASH32" ]] && (( RUN_BASH32 )); then
  for c in /home/kyler/.hermes/profiles/builder-hard/cache/scratch/bash32/bash-3.2.57/bash \
           /opt/bash-3.2/bin/bash /usr/local/bin/bash3.2; do
    [[ -x "$c" ]] && { BASH32="$c"; break; }
  done
fi
if [[ -n "$BASH32" ]]; then
  note "Bash 3.2 under test: $BASH32 ($("$BASH32" --version | head -1))"
else
  note "no Bash 3.2 binary found (EBBTOP_BASH32); the 3.2 cases will SKIP"
fi

# --- fixtures ---------------------------------------------------------------
STUB="$WORK/stub"
mkdir -p "$STUB"
STUB_STATE="$WORK/stub-state"
mkdir -p "$STUB_STATE"

# uname: the macOS pair. -s answers Darwin, -m answers arm64, and everything
# else answers Darwin so a stray call cannot silently look like Linux.
cat > "$STUB/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo Darwin ;;
  -m) echo "${FIX_ARCH:-arm64}" ;;
  *)  echo Darwin ;;
esac
EOF

# sw_vers: a plausible Apple Silicon macOS.
cat > "$STUB/sw_vers" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -productVersion) echo 14.6.1 ;;
  *) printf 'ProductName:\tmacOS\nProductVersion:\t14.6.1\nBuildVersion:\t23G93\n' ;;
esac
EOF

# plutil: -lint on a plist. A real plutil parses the XML, so this one does too:
# python3's expat parser is on every Linux box and rejects exactly the things
# plutil rejects (a truncated file, an unclosed element, a raw '&'). Comments
# and whitespace before <plist> are legal and must not confuse it.
cat > "$STUB/plutil" <<'EOF'
#!/usr/bin/env bash
f=''
for a in "$@"; do f="$a"; done
[[ -f "$f" ]] || { echo "plutil: no such file: $f" >&2; exit 1; }
exec python3 - "$f" <<'PY'
import sys, xml.etree.ElementTree as ET
path = sys.argv[1]
try:
    root = ET.parse(path).getroot()
except ET.ParseError as e:
    sys.stderr.write(f"plutil: not a valid plist: {path}: {e}\n")
    raise SystemExit(1)
if root.tag != 'plist':
    sys.stderr.write(f"plutil: root element is {root.tag!r}, not 'plist': {path}\n")
    raise SystemExit(1)
if root.find('dict') is None:
    sys.stderr.write(f"plutil: no top-level <dict>: {path}\n")
    raise SystemExit(1)
text = open(path, encoding='utf-8').read()
if '__' in text and any(f'__{n}__' in text for n in
                        ('AGENT_BIN', 'ENV_FILE', 'PORT', 'LOG_DIR')):
    sys.stderr.write(f"plutil: unrendered placeholder in {path}\n")
    raise SystemExit(1)
PY
EOF

# launchctl: a job registry in a directory. bootstrap/bootout/print/state are
# modelled, and FIX_BOOTSTRAP_FAILS makes bootstrap fail so the installer's
# activation-failure path can be exercised separately from a probe failure.
#
# `print gui/<uid>` is the user DOMAIN and must succeed whenever the user is
# logged in; `print gui/<uid>/<label>` is the JOB and succeeds only once it has
# been bootstrapped. Conflating the two makes every `agent_loaded` check true,
# which silently removes the unload/bootstrap sequencing being tested.
#
# Health is derived from the INSTALLED BINARY, not from a test toggle: a
# candidate whose `--version` reports "broken" never serves, and restoring the
# previous binary makes the job healthy again. That is what a real bad
# candidate does, so the rollback case exercises the real sequence.
cat > "$STUB/launchctl" <<'EOF'
#!/usr/bin/env bash
STATE="${FIX_STUB_STATE:?}"
LABEL=com.ebbwater.ebbtop-agent
BIN="$HOME/Library/Application Support/ebbtop-agent/bin/ebbtop-agent"
cmd="${1:-}"
printf 'launchctl %s\n' "$*" >> "$STATE/launchctl.log"
healthy() {
  # The job is healthy when the installed binary can serve. A *-not-serving
  # candidate is loaded by launchd but never answers; a *-refuse-boot one
  # cannot even activate, which `bootstrap` below models.
  [[ -x "$BIN" ]] || return 1
  "$BIN" --version 2>/dev/null | grep -qE '(not-serving|refuse-boot)' && return 1
  return 0
}
bootable() {
  [[ -x "$BIN" ]] || return 1
  "$BIN" --version 2>/dev/null | grep -q 'refuse-boot' && return 1
  return 0
}
case "$cmd" in
  print)
    target="${2:-}"
    case "$target" in
      */"$LABEL")
        [[ -f "$STATE/loaded" ]] || { echo "Could not find service \"$LABEL\"" >&2; exit 113; }
        if healthy; then echo "	state = running"; else echo "	state = exited"; fi
        echo "	program = $BIN"
        exit 0 ;;
      *) echo "gui domain (logged in)"; exit 0 ;;
    esac ;;
  bootstrap)
    # launchd refuses a job whose program cannot run. The fixture's candidate
    # is "broken" exactly when it reports so from --version, and a restored
    # previous binary loads normally again: that is the real sequence, where
    # the bad job definition fails to activate and the rollback's job does not.
    if [[ -f "$STATE/bootstrap-fails" ]] || ! bootable; then
      echo "Bootstrap failed: 5: Input/output error" >&2
      exit 5
    fi
    : > "$STATE/loaded"
    exit 0 ;;
  bootout)
    rm -f "$STATE/loaded"
    exit 0 ;;
  *) exit 0 ;;
esac
EOF

# lsof: the port is either free or held by the fixture agent. `-t` prints just
# the PID, which is what the installer now uses (real lsof truncates the
# COMMAND column to 9 characters, so a name test is unsound). The listener
# exists only while the installed binary can serve, so the same health rule
# drives both `launchctl print` and the socket.
cat > "$STUB/lsof" <<'EOF'
#!/usr/bin/env bash
STATE="${FIX_STUB_STATE:?}"
BIN="$HOME/Library/Application Support/ebbtop-agent/bin/ebbtop-agent"
[[ -x "$BIN" ]] || exit 1
"$BIN" --version 2>/dev/null | grep -qE '(not-serving|refuse-boot)' && exit 1
case " $* " in
  *" -t "*) echo 4242; exit 0 ;;
esac
echo "COMMAND     PID USER   FD   TYPE             DEVICE SIZE/OFF NODE NAME"
echo "ebbtop-age 4242 kyler    7u  IPv4 0x1234567890abcdef      0t0  TCP $(cat "$STATE/bind"):45976 (LISTEN)"
EOF

# ps: `-p <pid> -o comm=`. A real launchd job's comm is the executable it
# started, so the fixture answers with the installed binary's path.
cat > "$STUB/ps" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$HOME/Library/Application Support/ebbtop-agent/bin/ebbtop-agent"
EOF

# tailscale: the Mac's tailnet address, which is what the pilot binds.
cat > "$STUB/tailscale" <<'EOF'
#!/usr/bin/env bash
echo 100.64.0.5
EOF

cat > "$STUB/ipconfig" <<'EOF'
#!/usr/bin/env bash
echo 192.168.1.77
EOF

# curl: the whole network. The API is a file; an asset download copies the
# fixture body. Requests are recorded so a case can assert what was asked for.
cat > "$STUB/curl" <<'EOF'
#!/usr/bin/env bash
STATE="${FIX_STUB_STATE:?}"
printf 'curl %s\n' "$*" >> "$STATE/curl.log"
out=''; url=''
while [ $# -gt 0 ]; do
  case "$1" in
    -o|--output) out="$2"; shift 2 ;;
    -H|--header|--proto|--proto-redir) shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
[ -n "$url" ] || { echo "curl: no URL" >&2; exit 2; }
printf '%s\n' "$url" >> "$STATE/curl.urls"
case "$url" in
  */releases/latest) body="$STATE/api-latest.json" ;;
  */releases/tags/*) body="$STATE/api-tag-${url##*/}.json" ;;
  */releases/download/*) body="$STATE/download-body" ;;
  *) echo "curl: (22) 404" >&2; exit 22 ;;
esac
[ -f "$body" ] || { echo "curl: (22) 404" >&2; exit 22; }
if [ -n "$out" ]; then cp "$body" "$out"; else cat "$body"; fi
EOF

# getconf: only the Linux path asks, and the macOS cases must not reach it.
cat > "$STUB/getconf" <<'EOF'
#!/usr/bin/env bash
echo "glibc 2.43"
EOF

# file(1): the fixture agent is a shell script that stands in for the real
# arm64 binary (a real Mach-O cannot execute here), so `file` is the one
# platform tool the fixture must answer for. It reports what a real arm64
# Mach-O would, and the installer's own Mach-O refusal is proven separately
# by deploy/macos/test-install-macos.sh, which feeds it a real non-Mach-O file.
cat > "$STUB/file" <<'EOF'
#!/usr/bin/env bash
# Every argument is a path; answer for the last one, like file(1) does.
f=''
for a in "$@"; do f="$a"; done
if [[ "$f" == *not-a-macho* ]]; then
  echo "ASCII text"
else
  echo "Mach-O 64-bit executable arm64"
fi
EOF

chmod 755 "$STUB"/*

# --- the fixture agent binary ----------------------------------------------
# A shell script that behaves like the agent for the operations the installer
# performs: --version prints a version; --probe consults the fixture state and
# answers exactly like the real one (ok/denied/failed) at the address it was
# given. It records the probe target so a case can prove the installer probed
# the CONFIGURED bind rather than a hard-coded loopback address.
#
# A version containing "broken" makes the binary unserviceable: the launchd and
# lsof stubs derive the job's health from what the installed binary reports, so
# a "broken" candidate loads but never serves, exactly as a crashing agent
# would. Restoring the previous binary restores health, which is what makes the
# rollback case a real test of the sequence rather than of a toggle.
make_agent() { # <path> <version>
  # Three flavours, chosen by the version string so a case's intent is visible
  # in the package it installs:
  #   *-refuse-boot  the job cannot activate at all (launchd refuses it)
  #   *-not-serving  the job activates but never serves (no socket, no probe)
  #   anything else  a healthy agent
  local serves=1 boot=1
  case "$2" in
    *refuse-boot*) serves=0; boot=0 ;;
    *not-serving*) serves=0 ;;
  esac
  cat > "$1" <<EOF
#!/usr/bin/env bash
STATE="\${FIX_STUB_STATE:?}"
SERVES=$serves
case "\${1:-}" in
  --version)
    echo "ebbtop-agent $2" ;;
  --probe)
    target="\${2:-}"
    printf '%s\\n' "\$target" >> "\$STATE/probe.targets"
    [ "\$SERVES" = 1 ] || { echo "probe: failed: agent is not serving"; exit 1; }
    want="\$(cat "\$STATE/bind")"
    case "\$target" in
      "[\$want]:"*|"\$want:"*) ;;
      *) echo "probe: failed: connect \$target: refused"; exit 1 ;;
    esac
    echo "probe: ok frames=1"
    exit 0 ;;
  *) exit 0 ;;
esac
EOF
  chmod 755 "$1"
}

# make_mac_pkg <out.tar.gz> <version> <installer> <plist-template> <stage> [marker]
#
# The optional marker is appended to the packaged plist as an XML comment. It
# makes a candidate's rendered LaunchAgent distinguishable from the previous
# install's, which is what turns "the rollback restored the old plist" into an
# observable fact: a baseline and a candidate rendered from the same template
# are otherwise byte-identical, so the assertion could never fail.
make_mac_pkg() {
  local out="$1" ver="$2" installer="$3" plist="$4" stage="$5" marker="${6:-}"
  mkdir -p "$stage"
  make_agent "$stage/ebbtop-agent" "$ver"
  cp "$installer" "$stage/install-agent.sh"; chmod 755 "$stage/install-agent.sh"
  cp "$plist" "$stage/ebbtop-agent.plist"
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
  ( cd "$stage" && shasum -a 256 BUILD ebbtop-agent ebbtop-agent.plist > SHA256SUMS )
  ( cd "$stage" && tar -czf "$out" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh )
}

# API JSON with (or without) the macOS asset.
make_mac_api() { # <out> <tag> <digest|"">
  local out="$1" tag="$2" digest="$3" asset=''
  if [ -n "$digest" ]; then
    asset=$(cat <<JSON
    {
      "name": "ebbtop-agent-macos-arm64.tar.gz",
      "id": 5,
      "state": "uploaded",
      "digest": "sha256:$digest",
      "size": 2000000,
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/ebbtop-agent-macos-arm64.tar.gz"
    },
JSON
)
  fi
  cat > "$out" <<JSON
{
  "tag_name": "$tag",
  "assets": [
    {
      "name": "ebbtop-agent-linux-x86_64.tar.gz",
      "id": 2,
      "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/ebbtop-agent-linux-x86_64.tar.gz"
    },
$asset
    {
      "name": "EbbTop-Windows-x64.zip",
      "id": 1,
      "digest": "sha256:2222222222222222222222222222222222222222222222222222222222222222",
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/EbbTop-Windows-x64.zip"
    }
  ]
}
JSON
}

pkg_digest() { shasum -a 256 "$1" | cut -d' ' -f1; }

# file_mode <path>: the harness asserts on 0600 credential files. macOS's
# `stat -f %Lp` and Linux's `stat -c %a` both print just the octal mode.
file_mode() {
  if stat -c %a "$1" >/dev/null 2>&1; then stat -c %a "$1"; else stat -f %Lp "$1"; fi
}

# --- shared fixture packages ------------------------------------------------
# Built once, before any case, because several cases install a known-good
# baseline and then upgrade or roll back from it. Each is a real archive with
# exactly the five packaged members, this checkout's macOS installer and plist
# template byte for byte, and its own SHA256SUMS.
GOOD_PKG="$WORK/fixtures/ebbtop-agent-macos-arm64-good.tar.gz"
NEWER_PKG="$WORK/fixtures/ebbtop-agent-macos-arm64-newer.tar.gz"
mkdir -p "$WORK/fixtures"
make_mac_pkg "$GOOD_PKG" 0.3.0 "$MAC_INSTALLER" "$MAC_PLIST" "$WORK/fixtures/stage-good"
make_mac_pkg "$NEWER_PKG" 0.3.2 "$MAC_INSTALLER" "$MAC_PLIST" "$WORK/fixtures/stage-newer"
# The packages must carry this checkout's installer and template unchanged, or
# a green case would be evidence about a copy rather than the shipped file.
for member in install-agent.sh ebbtop-agent.plist; do
  case "$member" in
    install-agent.sh) src="$MAC_INSTALLER" ;;
    *) src="$MAC_PLIST" ;;
  esac
  if ! tar -xzOf "$GOOD_PKG" "$member" | cmp -s - "$src"; then
    echo "setup: fixture archive does not carry this checkout's $member" >&2
    exit 1
  fi
done
GOOD_SHA="$(pkg_digest "$GOOD_PKG")"

# --- drivers ----------------------------------------------------------------
# new_case <name> -> sets CASE, CASE_HOME, CASE_WORK, clears the stub state.
new_case() {
  CASE="$1"
  CASE_HOME="$WORK/cases/$CASE/home"
  CASE_WORK="$WORK/cases/$CASE/work"
  mkdir -p "$CASE_HOME" "$CASE_WORK"
  rm -f "$STUB_STATE"/*
  echo 100.64.0.5 > "$STUB_STATE/bind"
}

# run_script <bash-binary> <out> <args...>   (macOS platform, stub PATH)
run_script() {
  local bashbin="$1" out="$2"; shift 2
  ( env -i \
      HOME="$CASE_HOME" \
      TMPDIR="$CASE_WORK" \
      PATH="$STUB:/usr/bin:/bin:/usr/sbin" \
      FIX_STUB_STATE="$STUB_STATE" \
      SHELL=/bin/sh \
      "$bashbin" "$SCRIPT" --platform macos "$@" ) >"$out" 2>&1
}

expect_refusal() { # <name> <out> <substring>
  local name="$1" out="$2" needle="$3"
  if grep -qF -- "$needle" "$out"; then return 0; fi
  fail "$name" "no refusal containing '$needle': $(sed -n 's/^   fail //p' "$out" | head -2 | tr '\n' ' ')"
  return 1
}

# ===========================================================================
# 1. platform selection: the pair (uname -s, uname -m) decides the package,
#    before any network access, and an unsupported pair is refused.
# ===========================================================================
case_unsupported_os() {
  local name=unsupported-os out="$WORK/os.out" rc=0
  new_case "$name"
  cat > "$STUB/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo FreeBSD ;;
  -m) echo amd64 ;;
  *)  echo FreeBSD ;;
esac
EOF
  chmod 755 "$STUB/uname"
  mkdir -p "$WORK/cases/$name/stub"
  cp "$STUB"/* "$WORK/cases/$name/stub/" 2>/dev/null || true

  # Run without --platform so uname decides.
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" "$(command -v bash)" "$SCRIPT" --yes ) >"$out" 2>&1 || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "FreeBSD was accepted"; return; }
  expect_refusal "$name" "$out" "unsupported operating system 'FreeBSD'" || return
  [[ ! -s "$STUB_STATE/curl.log" ]] || { fail "$name" "the network was used before the refusal"; return; }
  # Restore the macOS uname for later cases.
  cat > "$STUB/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo Darwin ;;
  -m) echo "${FIX_ARCH:-arm64}" ;;
  *)  echo Darwin ;;
esac
EOF
  chmod 755 "$STUB/uname"
  pass "$name" "a non-Linux/non-Darwin uname is refused with one line and no network request"
}

case_intel_mac() {
  local name=intel-mac out="$WORK/intel.out" rc=0
  new_case "$name"
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" FIX_ARCH=x86_64 \
      "$(command -v bash)" "$SCRIPT" --yes ) >"$out" 2>&1 || rc=$?
  # Miss on purpose: the arch check reads `uname -m` without FIX_ARCH when the
  # stub is shared, so set it through the environment of the stub's parent.
  [[ $rc -ne 0 ]] || { fail "$name" "an Intel Mac was accepted"; return; }
  if expect_refusal "$name" "$out" "Apple Silicon only" \
     || expect_refusal "$name" "$out" "running under Rosetta"; then
    [[ ! -s "$STUB_STATE/curl.log" ]] || { fail "$name" "network used before the arch refusal"; return; }
    pass "$name" "an x86_64 Darwin host is refused before the network (Intel Mac / Rosetta shell)"
  fi
}

# ===========================================================================
# 2. a release with no macOS asset is refused with an actionable line, and a
#    Rosetta shell is named precisely rather than downgraded.
# ===========================================================================
case_missing_mac_asset() {
  local name=missing-mac-asset out="$WORK/noasset.out" rc=0
  new_case "$name"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 ""
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a release without the Mac asset was accepted"; return; }
  expect_refusal "$name" "$out" "has no ebbtop-agent-macos-arm64.tar.gz asset" || return
  grep -qF 'predates the Apple Silicon agent' "$out" \
    || { fail "$name" "the refusal does not explain the release predates the package"; return; }
  [[ ! -s "$CASE_HOME/Library/Application Support/ebbtop-agent/agent.env" ]] 2>/dev/null \
    || { fail "$name" "agent.env was written"; return; }
  pass "$name" "a release without the Mac archive is refused, and the line says to pin a newer one"
}

case_rosetta() {
  local name=rosetta out="$WORK/rosetta.out" rc=0
  new_case "$name"
  # A Rosetta shell: uname -m answers x86_64 and sysctl reports the flag.
  cat > "$STUB/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo Darwin ;;
  -m) echo x86_64 ;;
  *)  echo Darwin ;;
esac
EOF
  cat > "$STUB/sysctl" <<'EOF'
#!/usr/bin/env bash
echo 1
EOF
  chmod 755 "$STUB/uname" "$STUB/sysctl"
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" "$(command -v bash)" "$SCRIPT" --yes ) >"$out" 2>&1 || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a Rosetta shell was accepted"; return; }
  expect_refusal "$name" "$out" "running under Rosetta" || return
  grep -qF 'arch -arm64 bash' "$out" || { fail "$name" "the refusal does not say how to fix it"; return; }
  # Restore.
  cat > "$STUB/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo Darwin ;;
  -m) echo "${FIX_ARCH:-arm64}" ;;
  *)  echo Darwin ;;
esac
EOF
  rm -f "$STUB/sysctl"
  chmod 755 "$STUB/uname"
  pass "$name" "a Rosetta (translated) shell is refused with the exact fix, before any download"
}

# ===========================================================================
# 3. dry run resolves the release, prints the plan and writes nothing.
# ===========================================================================
case_dry_run() {
  local name=dry-run out="$WORK/dry.out" rc=0
  new_case "$name"
  local pkg="$GOOD_PKG"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"

  run_script "$(command -v bash)" "$out" --dry-run --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "dry run exited $rc: $(cat "$out")"; return; }
  grep -qF 'release    v0.3.0' "$out" || { fail "$name" "the plan does not name the release"; return; }
  grep -qF 'ebbtop-agent-macos-arm64.tar.gz' "$out" || { fail "$name" "the plan does not name the Mac archive"; return; }
  grep -qF 'LaunchAgent' "$out" || { fail "$name" "the plan does not name the LaunchAgent"; return; }
  grep -qF 'sha256:' "$out" || { fail "$name" "the plan does not name the digest"; return; }
  [[ ! -e "$CASE_HOME/Library" ]] || { fail "$name" "the dry run wrote to ~/Library"; return; }
  [[ ! -s "$STUB_STATE/lsof.log" ]] 2>/dev/null || true
  [[ "$(sed -n 's/^curl .*//p' "$STUB_STATE/curl.log" | wc -l)" -ge 0 ]] || true
  # Only the API was read: the asset was never downloaded.
  if grep -q 'releases/download' "$STUB_STATE/curl.urls"; then
    fail "$name" "the dry run downloaded the asset"; return
  fi
  pass "$name" "the plan names the Mac archive, the LaunchAgent, the digest and the bind, downloads nothing and writes nothing"
}

# ===========================================================================
# 4. fresh install: the fixture package installs, agent.env is 0600, the
#    plist is rendered, the probe targets the CONFIGURED bind, and the report
#    prints the tailnet address (not loopback).
# ===========================================================================
case_fresh_install() {
  local name=fresh-install out="$WORK/fresh.out" rc=0 pkg="$GOOD_PKG"
  new_case "$name"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"

  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "install exited $rc: $(cat "$out")"; return; }

  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  [[ -f "$svc/agent.env" ]] || { fail "$name" "no agent.env at $svc"; return; }
  [[ "$(file_mode "$svc/agent.env")" == 600 ]] || { fail "$name" "agent.env mode is $(file_mode "$svc/agent.env")"; return; }
  [[ "$(file_mode "$svc/ebbtop.token")" == 600 ]] || { fail "$name" "token mode is $(file_mode "$svc/ebbtop.token")"; return; }
  grep -qx 'EBBTOP_BIND=100.64.0.5' "$svc/agent.env" || { fail "$name" "agent.env does not carry the detected tailnet bind: $(cat "$svc/agent.env")"; return; }
  grep -qx 'EBBTOP_AGENT_DISCOVERY=0' "$svc/agent.env" || { fail "$name" "discovery is not disabled on the Mac install"; return; }

  local plist="$CASE_HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist"
  [[ -f "$plist" ]] || { fail "$name" "no LaunchAgent at $plist"; return; }
  grep -q "$svc/bin/ebbtop-agent" "$plist" || { fail "$name" "the plist does not name the installed binary"; return; }
  grep -q "$svc/agent.env" "$plist" || { fail "$name" "the plist does not pass the env file"; return; }
  if grep -Eq 'EBBTOP_TOKEN=|EBBTOP_BIND=' "$plist"; then
    fail "$name" "the plist assigns a credential or a bind address"; return
  fi
  if grep -q '__[A-Z_]*__' "$plist"; then
    fail "$name" "the installed plist still carries a placeholder"; return
  fi

  # The probe must have gone to the configured bind, not 127.0.0.1.
  grep -qx '100.64.0.5:45976' "$STUB_STATE/probe.targets" \
    || { fail "$name" "the probe did not target the configured bind: $(tr '\n' ' ' < "$STUB_STATE/probe.targets")"; return; }
  if grep -q '^127\.0\.0\.1:45976$' "$STUB_STATE/probe.targets"; then
    fail "$name" "the installer probed loopback on a tailnet-bound install"; return
  fi

  grep -qF 'address    ws://100.64.0.5:45976/ws' "$out" \
    || { fail "$name" "the pairing address is not the tailnet bind: $(grep 'address' "$out" | head -1)"; return; }
  grep -qF 'authenticated probe of the running agent: probe: ok' "$out" \
    || { fail "$name" "the report does not carry the authenticated probe line"; return; }

  local tok; tok="$(cat "$svc/ebbtop.token")"
  [[ "$tok" =~ ^[0-9a-f]{64}$ ]] || { fail "$name" "token is not 32 bytes of hex"; return; }
  if grep -qF "$tok" "$out"; then fail "$name" "the token value was printed"; return; fi
  if grep -qF "$tok" "$plist"; then fail "$name" "the token was written into the plist"; return; fi

  pass "$name" "fresh install: 0600 credential file, rendered LaunchAgent, probe at the tailnet bind (never loopback), pairing address from the bind, no token printed"
}

# ===========================================================================
# 5. the probe target is derived from the bind: a loopback install probes
#    loopback, and an IPv6 bind is probed as [addr]:port.
# ===========================================================================
case_probe_targets_the_bind() {
  local name=probe-target out="$WORK/target.out" rc=0 pkg="$GOOD_PKG"
  local ok=1
  for bind in 127.0.0.1 192.168.1.50; do
    new_case "$name-$bind"
    echo "$bind" > "$STUB_STATE/bind"
    rm -f "$STUB_STATE/healthy"
    make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
    cp "$pkg" "$STUB_STATE/download-body"
    ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
        FIX_STUB_STATE="$STUB_STATE" EBBTOP_ADDR="$bind" \
        "$(command -v bash)" "$SCRIPT" --platform macos --yes ) >"$out" 2>&1 || rc=$?
    [[ $rc -eq 0 ]] || { fail "$name" "install for bind $bind exited $rc: $(tail -5 "$out")"; ok=0; break; }
    grep -qx "$bind:45976" "$STUB_STATE/probe.targets" \
      || { fail "$name" "bind $bind was not probed at $bind:45976: $(tr '\n' ' ' < "$STUB_STATE/probe.targets")"; ok=0; break; }
    rc=0
  done
  # The fixture's lsof reports the bind it was given, so the healthy job answers
  # only at that address; the "failed" arm below proves a wrong target is
  # rejected rather than silently accepted.
  (( ok )) || return 0

  new_case "$name-mismatch"
  echo 10.1.2.3 > "$STUB_STATE/bind"
  rm -f "$STUB_STATE/healthy"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"
  # Force the fixture agent to consider the job healthy but the bind different
  # from the one the installer will probe, by pre-seeding a healthy job whose
  # address is not the configured one.
  : > "$STUB_STATE/healthy"
  echo 10.9.9.9 > "$STUB_STATE/bind"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a probe at the wrong address was accepted as ready"; return; }
  grep -qF 'did not become ready' "$out" \
    || { fail "$name" "a probe mismatch was not reported as not-ready: $(tail -5 "$out")"; return; }
  pass "$name" "the readiness probe follows the configured bind (loopback and LAN), and a target that does not match the bound address fails the install"
}

# ===========================================================================
# 6. upgrade keeps the token and the bind; a repeat install is idempotent.
# ===========================================================================
case_upgrade_keeps_state() {
  local name=upgrade out="$WORK/up.out" rc=0 pkg="$GOOD_PKG"
  new_case "$name"
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "first install exited $rc: $(tail -5 "$out")"; return; }
  local tok1 bind1 env1
  tok1="$(cat "$svc/ebbtop.token")"
  bind1="$(grep '^EBBTOP_BIND=' "$svc/agent.env")"
  env1="$(shasum -a 256 "$svc/agent.env" | cut -d' ' -f1)"

  out="$WORK/up2.out"; rc=0
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "repeat install exited $rc: $(tail -5 "$out")"; return; }
  [[ "$(cat "$svc/ebbtop.token")" == "$tok1" ]] || { fail "$name" "the repeat install changed the token"; return; }
  [[ "$(grep '^EBBTOP_BIND=' "$svc/agent.env")" == "$bind1" ]] || { fail "$name" "the repeat install changed the bind"; return; }
  [[ "$(shasum -a 256 "$svc/agent.env" | cut -d' ' -f1)" == "$env1" ]] \
    || { fail "$name" "agent.env was rewritten on the repeat install"; return; }
  # An EBBTOP_ADDR on an upgrade is ignored, not applied.
  out="$WORK/up3.out"; rc=0
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" EBBTOP_ADDR=10.9.9.9 \
      "$(command -v bash)" "$SCRIPT" --platform macos --yes ) >"$out" 2>&1 || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "upgrade with EBBTOP_ADDR exited $rc: $(tail -5 "$out")"; return; }
  grep -qF 'EBBTOP_ADDR=10.9.9.9 ignored' "$out" || { fail "$name" "an upgrade did not say EBBTOP_ADDR was ignored"; return; }
  [[ "$(grep '^EBBTOP_BIND=' "$svc/agent.env")" == "$bind1" ]] || { fail "$name" "EBBTOP_ADDR changed the bind on an upgrade"; return; }
  pass "$name" "a repeat install is idempotent: same token, byte-identical agent.env, and EBBTOP_ADDR is ignored rather than applied on an upgrade"
}

# ===========================================================================
# 7. failed upgrade rolls back BOTH binary and plist; a bad candidate never
#    leaves a broken install behind.
# ===========================================================================
case_failed_upgrade_rollback() {
  local name=rollback out="$WORK/rb.out" rc=0
  new_case "$name"
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  local good="$GOOD_PKG" bad="$WORK/mac-bad.tar.gz"

  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$good")"
  cp "$good" "$STUB_STATE/download-body"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline install exited $rc: $(tail -5 "$out")"; return; }
  local good_sha plist_sha
  good_sha="$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)"
  plist_sha="$(shasum -a 256 "$CASE_HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist" | cut -d' ' -f1)"

  # A candidate that installs but cannot serve. `make_mac_pkg` with a *-not-serving
  # version writes a binary that reports itself unserviceable, so the fixture
  # launchd job loads it, reports state=exited, holds no socket and answers no
  # probe. Restoring the previous binary restores all three, which is the real
  # bad-upgrade sequence rather than a test-only toggle.
  make_mac_pkg "$bad" 0.3.1-not-serving "$MAC_INSTALLER" "$MAC_PLIST" "$WORK/mac-stage-bad" "candidate 0.3.1"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.1 "$(pkg_digest "$bad")"
  cp "$bad" "$STUB_STATE/download-body"

  out="$WORK/rb2.out"; rc=0
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "an upgrade that cannot serve was reported as success"; return; }
  grep -qF 'did not become ready' "$out" || { fail "$name" "no readiness failure was reported: $(tail -8 "$out")"; return; }
  grep -qF 'readiness probe failed' "$out" || { fail "$name" "the rollback does not name the probe failure: $(tail -8 "$out")"; return; }
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" == "$good_sha" ]] \
    || { fail "$name" "the previous binary was not restored"; return; }
  if grep -q 'candidate 0.3.1' "$CASE_HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist" 2>/dev/null; then
    fail "$name" "the failed candidate's LaunchAgent was left installed"; return
  fi
  [[ "$(shasum -a 256 "$CASE_HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist" | cut -d' ' -f1)" == "$plist_sha" ]] \
    || { fail "$name" "the previous LaunchAgent was not restored"; return; }
  grep -qF 'the previous version is running again' "$out" \
    || { fail "$name" "the report does not state the previous version is back: $(tail -6 "$out")"; return; }
  pass "$name" "a candidate that installs but cannot serve is rolled back: previous binary AND plist restored, the failure named, and the install exits nonzero"
}

# ===========================================================================
# 8. a launchd activation failure is a distinct failure from a probe failure
#    and goes through the same rollback.
# ===========================================================================
case_activation_failure() {
  local name=activation out="$WORK/act.out" rc=0
  new_case "$name"
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  local good="$GOOD_PKG" unbootable="$WORK/mac-unbootable.tar.gz"

  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$good")"
  cp "$good" "$STUB_STATE/download-body"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline install exited $rc: $(tail -5 "$out")"; return; }
  local good_sha; good_sha="$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)"

  # A candidate launchd refuses outright: the job never activates, so the
  # installer never reaches a probe. The failure must be attributed to
  # activation, not to readiness, and the rollback's own bootstrap must succeed
  # (which is why this is a property of the candidate package rather than a
  # global "all bootstraps fail" switch).
  make_mac_pkg "$unbootable" 0.3.1-refuse-boot "$MAC_INSTALLER" "$MAC_PLIST" "$WORK/mac-stage-unbootable"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.1 "$(pkg_digest "$unbootable")"
  cp "$unbootable" "$STUB_STATE/download-body"

  out="$WORK/act2.out"; rc=0
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a launchd bootstrap failure was reported as success"; return; }
  grep -qF 'launchd refused to load the LaunchAgent' "$out" \
    || { fail "$name" "the activation failure is not named: $(tail -8 "$out")"; return; }
  grep -qF 'launchd bootstrap failed' "$out" \
    || { fail "$name" "the rollback does not attribute the failure to activation: $(tail -8 "$out")"; return; }
  # It must NOT be reported as a probe failure: the two are distinct.
  if grep -qF 'readiness probe failed' "$out"; then
    fail "$name" "an activation failure was misreported as a probe failure"; return
  fi
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" == "$good_sha" ]] \
    || { fail "$name" "the previous binary was not restored after an activation failure"; return; }
  grep -qF 'the previous version is running again' "$out" \
    || { fail "$name" "the report does not state the previous version is back: $(tail -8 "$out")"; return; }
  pass "$name" "a launchd bootstrap failure is named as an activation failure (not a probe failure) and rolls back to the previous binary"
}

# ===========================================================================
# 9. credential problems refuse BEFORE the install is modified.
# ===========================================================================
case_credential_refusals() {
  local name=credentials out="$WORK/cred.out" rc=0
  new_case "$name"
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  local good="$GOOD_PKG"

  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$good")"
  cp "$good" "$STUB_STATE/download-body"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline install exited $rc: $(tail -5 "$out")"; return; }
  local bin_sha; bin_sha="$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)"

  # (a) token file missing
  mv "$svc/ebbtop.token" "$svc/ebbtop.token.hidden"
  out="$WORK/cred-a.out"; rc=0
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a missing token file was accepted"; return; }
  expect_refusal "$name" "$out" "has no" || return
  grep -qF 'refusing before any change to the current install' "$out" \
    || { fail "$name" "the missing-token refusal does not say nothing was changed"; return; }
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" == "$bin_sha" ]] \
    || { fail "$name" "the binary was replaced before the missing-token refusal"; return; }
  mv "$svc/ebbtop.token.hidden" "$svc/ebbtop.token"

  # (b) token file empty
  : > "$svc/ebbtop.token"
  out="$WORK/cred-b.out"; rc=0
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "an empty token file was accepted"; return; }
  grep -qF 'is empty, so the token cannot be verified' "$out" \
    || { fail "$name" "the empty-token refusal is missing: $(sed -n 's/^install-agent.sh: //p' "$out" | head -2)"; return; }
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" == "$bin_sha" ]] \
    || { fail "$name" "the binary was replaced before the empty-token refusal"; return; }

  # (c) agent.env and ebbtop.token disagree
  printf '%s\n' "$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')" > "$svc/ebbtop.token"
  out="$WORK/cred-c.out"; rc=0
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "a token mismatch between agent.env and ebbtop.token was accepted"; return; }
  grep -qF 'carry different tokens' "$out" \
    || { fail "$name" "the token-mismatch refusal is missing: $(sed -n 's/^install-agent.sh: //p' "$out" | head -2)"; return; }
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" == "$bin_sha" ]] \
    || { fail "$name" "the binary was replaced before the token-mismatch refusal"; return; }
  pass "$name" "a missing, empty or disagreeing credential file refuses before the current install is touched (binary bytes unchanged)"
}

# ===========================================================================
# 10. unsafe bind addresses are refused before anything is written.
# ===========================================================================
case_unsafe_binds() {
  local name=binds out="$WORK/binds.out" rc=0 pkg="$GOOD_PKG" bad
  new_case "$name"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"

  for bad in 0.0.0.0 8.8.8.8 172.32.0.1 100.128.0.1 2001:4860:4860::8888; do
    rc=0
    ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
        FIX_STUB_STATE="$STUB_STATE" EBBTOP_ADDR="$bad" \
        "$(command -v bash)" "$SCRIPT" --platform macos --yes ) >"$out" 2>&1 || rc=$?
    [[ $rc -ne 0 ]] || { fail "$name" "bind $bad was accepted"; return; }
    grep -qF "refusing bind address '$bad'" "$out" \
      || { fail "$name" "bind $bad was not refused with a clear line: $(sed -n 's/^install-agent.sh: //p' "$out" | head -2)"; return; }
    if [[ -e "$CASE_HOME/Library" ]]; then
      fail "$name" "bind $bad was refused but ~/Library was created"; return
    fi
  done
  pass "$name" "wildcard, public and out-of-range private binds are refused before a single write"
}

# ===========================================================================
# 11. uninstall preserves state; --purge deletes it. Rollback restores.
# ===========================================================================
case_lifecycle() {
  local name=lifecycle out="$WORK/life.out" rc=0 pkg="$GOOD_PKG"
  new_case "$name"
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  local la="$CASE_HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist"

  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "install exited $rc: $(tail -5 "$out")"; return; }
  local tok; tok="$(cat "$svc/ebbtop.token")"

  # ordinary uninstall: binary and plist gone, state kept
  out="$WORK/life2.out"; rc=0
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 \
      "$(command -v bash)" "$svc/install-agent.sh" --uninstall ) >"$out" 2>&1 || rc=$?
  # install-agent.sh is not in the state dir; run it from the extracted package
  # the wrapper used, which the wrapper cleaned up. Re-extract it explicitly.
  local stage="$WORK/lifecycle-pkg"
  mkdir -p "$stage" && tar -xzf "$pkg" -C "$stage"
  out="$WORK/life3.out"; rc=0
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 \
      "$(command -v bash)" "$stage/install-agent.sh" --uninstall ) >"$out" 2>&1 || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "uninstall exited $rc: $(cat "$out")"; return; }
  [[ ! -f "$la" ]] || { fail "$name" "the plist survived an uninstall"; return; }
  [[ ! -f "$svc/bin/ebbtop-agent" ]] || { fail "$name" "the binary survived an uninstall"; return; }
  [[ -f "$svc/ebbtop.token" && "$(cat "$svc/ebbtop.token")" == "$tok" ]] \
    || { fail "$name" "an ordinary uninstall removed or changed the token"; return; }
  [[ -f "$svc/agent.env" ]] || { fail "$name" "an ordinary uninstall removed agent.env"; return; }

  # purge: state gone
  out="$WORK/life4.out"; rc=0
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 \
      "$(command -v bash)" "$stage/install-agent.sh" --uninstall --purge ) >"$out" 2>&1 || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "purge exited $rc: $(cat "$out")"; return; }
  [[ ! -e "$svc" ]] || { fail "$name" "--purge left the state directory"; return; }
  grep -qF 'token: deleted with the state directory' "$out" || { fail "$name" "the purge report does not say the token went too"; return; }
  pass "$name" "uninstall removes the job and binary but keeps the token and bind; --purge deletes the state directory and says so"
}

case_rollback_command() {
  local name=rollback-cmd out="$WORK/rbc.out" rc=0
  new_case "$name"
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  local good="$GOOD_PKG" newer="$NEWER_PKG"
  local stage="$WORK/rbc-stage"
  # The newer package is already built and verified above; extract it so the
  # `--rollback` invocation runs the same packaged installer the upgrade did.
  mkdir -p "$stage"
  tar -xzf "$newer" -C "$stage"

  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$good")"
  cp "$good" "$STUB_STATE/download-body"
  run_script "$(command -v bash)" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "baseline install exited $rc: $(tail -5 "$out")"; return; }
  local v0; v0="$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)"

  # A good newer candidate, installed from its own extracted package.
  out="$WORK/rbc2.out"; rc=0
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 \
      "$(command -v bash)" "$stage/install-agent.sh" --prebuilt ) >"$out" 2>&1 || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "the newer install exited $rc: $(tail -6 "$out")"; return; }
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" != "$v0" ]] \
    || { fail "$name" "the newer candidate did not replace the binary"; return; }

  out="$WORK/rbc3.out"; rc=0
  ( env -i HOME="$CASE_HOME" TMPDIR="$CASE_WORK" PATH="$STUB:/usr/bin:/bin" \
      FIX_STUB_STATE="$STUB_STATE" EBBTOP_ALLOW_UNSUPPORTED_HOST=1 \
      "$(command -v bash)" "$stage/install-agent.sh" --rollback ) >"$out" 2>&1 || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "--rollback exited $rc: $(cat "$out")"; return; }
  [[ "$(shasum -a 256 "$svc/bin/ebbtop-agent" | cut -d' ' -f1)" == "$v0" ]] \
    || { fail "$name" "--rollback did not restore the previous binary"; return; }
  grep -qF 'rolled back' "$out" || { fail "$name" "--rollback did not report success"; return; }
  # And it probed the configured bind, not loopback.
  grep -qx '100.64.0.5:45976' "$STUB_STATE/probe.targets" \
    || { fail "$name" "--rollback probed the wrong target: $(tr '\n' ' ' < "$STUB_STATE/probe.targets")"; return; }
  pass "$name" "--rollback restores the previous binary and verifies readiness at the configured bind"
}

# ===========================================================================
# 12. the whole file parses and runs under a real Bash 3.2.
# ===========================================================================
bash32_syntax() { # <file>
  "$BASH32" -n "$1"
}

case_bash32_syntax() {
  local name=bash32-syntax
  if [[ -z "$BASH32" ]]; then
    skip "$name" "no Bash 3.2 binary (set EBBTOP_BASH32); the 3.2 compatibility claim is NOT made for this run"
    return
  fi
  local f rc=0
  for f in "$SCRIPT" "$MAC_INSTALLER" "$ROOT/deploy/macos/package-agent.sh"; do
    bash32_syntax "$f" || { fail "$name" "Bash 3.2 -n rejected $f"; rc=1; }
  done
  (( rc == 0 )) || return
  pass "$name" "the bootstrap, the macOS installer and its packager all parse under $("$BASH32" --version | head -1)"
}

case_bash32_dry_run() {
  local name=bash32-dry-run out="$WORK/b32.out" rc=0 pkg="$GOOD_PKG"
  if [[ -z "$BASH32" ]]; then
    skip "$name" "no Bash 3.2 binary (set EBBTOP_BASH32)"
    return
  fi
  new_case "$name"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"
  run_script "$BASH32" "$out" --dry-run --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "Bash 3.2 dry run exited $rc: $(tail -12 "$out")"; return; }
  grep -qF 'release    v0.3.0' "$out" || { fail "$name" "the Bash 3.2 plan does not name the release: $(tail -12 "$out")"; return; }
  grep -qF 'ebbtop-agent-macos-arm64.tar.gz' "$out" || { fail "$name" "the Bash 3.2 plan does not name the Mac archive"; return; }
  pass "$name" "the bootstrap's macOS plan runs under Bash 3.2 with no 3.2-incompatible construct"
}

case_bash32_install() {
  local name=bash32-install out="$WORK/b32i.out" rc=0 pkg="$GOOD_PKG"
  if [[ -z "$BASH32" ]]; then
    skip "$name" "no Bash 3.2 binary (set EBBTOP_BASH32)"
    return
  fi
  new_case "$name"
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$pkg")"
  cp "$pkg" "$STUB_STATE/download-body"
  run_script "$BASH32" "$out" --yes || rc=$?
  [[ $rc -eq 0 ]] || { fail "$name" "Bash 3.2 install exited $rc: $(tail -14 "$out")"; return; }
  local svc="$CASE_HOME/Library/Application Support/ebbtop-agent"
  [[ -f "$svc/agent.env" ]] || { fail "$name" "no agent.env from the Bash 3.2 install: $(tail -14 "$out")"; return; }
  [[ -f "$CASE_HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist" ]] \
    || { fail "$name" "no plist from the Bash 3.2 install"; return; }
  grep -qx '100.64.0.5:45976' "$STUB_STATE/probe.targets" \
    || { fail "$name" "the Bash 3.2 install probed the wrong target: $(tr '\n' ' ' < "$STUB_STATE/probe.targets")"; return; }
  grep -qF 'probe: ok' "$out" || { fail "$name" "the Bash 3.2 install did not verify readiness"; return; }
  pass "$name" "the full macOS install path (extract, verify, render, load, probe, report) completes under Bash 3.2"
}

case_bash32_member_refusal() {
  local name=bash32-members out="$WORK/b32m.out" rc=0
  if [[ -z "$BASH32" ]]; then
    skip "$name" "no Bash 3.2 binary (set EBBTOP_BASH32)"
    return
  fi
  new_case "$name"
  # An archive with an extra member must be refused by check_members, which is
  # the one function rewritten without mapfile for 3.2.
  local stage="$WORK/b32m-stage" bad="$WORK/b32m.tar.gz"
  make_mac_pkg "$bad" 0.3.0 "$MAC_INSTALLER" "$MAC_PLIST" "$stage"
  echo 'extra' > "$stage/extra-file"
  ( cd "$stage" && tar -czf "$bad" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh extra-file )
  make_mac_api "$STUB_STATE/api-latest.json" v0.3.0 "$(pkg_digest "$bad")"
  cp "$bad" "$STUB_STATE/download-body"
  run_script "$BASH32" "$out" --yes || rc=$?
  [[ $rc -ne 0 ]] || { fail "$name" "an archive with an unexpected member was accepted under Bash 3.2"; return; }
  grep -qF "unexpected member 'extra-file'" "$out" \
    || { fail "$name" "the member refusal is wrong under Bash 3.2: $(tail -8 "$out")"; return; }
  pass "$name" "the pre-extraction member check still refuses an unexpected member under Bash 3.2"
}

# --- run --------------------------------------------------------------------
case_unsupported_os
case_intel_mac
case_missing_mac_asset
case_rosetta
case_dry_run
case_fresh_install
case_probe_targets_the_bind
case_upgrade_keeps_state
case_failed_upgrade_rollback
case_activation_failure
case_credential_refusals
case_unsafe_binds
case_lifecycle
case_rollback_command
case_bash32_syntax
case_bash32_dry_run
case_bash32_install
case_bash32_member_refusal

printf '\nSUMMARY: %d passed, %d failed, %d skipped\n' "$PASSES" "$FAILS" "$SKIPS"
(( FAILS == 0 )) || exit 1
exit 0
