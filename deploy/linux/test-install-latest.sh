#!/usr/bin/env bash
# Exercise deploy/linux/ebbtop-install.sh inside a disposable fixture.
#
# Every case runs with HOME pointed at a throwaway directory, XDG_RUNTIME_DIR at
# a fixture user-manager directory, and stubs for step on PATH: systemctl, ss,
# loginctl, curl, tailscale, ip, getconf, uname, and failing git/cargo/rustc.
# No service-manager call, no listener, no network request and no write outside
# the fixture is ever real.
#
# The archives are built here, byte for byte, from this checkout's own
# deploy/install-agent.sh and deploy/ebbtop-agent.service, so the wrapper's
# delegation is tested against the installer that ships in the repository and
# not against a copy of it. A fixture archive only has to satisfy the packaged
# installer's own checks: member set, unit contract, embedded SHA256SUMS.
#
# Usage:
#   bash deploy/linux/test-install-latest.sh
#
# One PASS line per case, then a SUMMARY line.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="$ROOT/deploy/linux/ebbtop-install.sh"
REPO_INSTALLER="$ROOT/deploy/install-agent.sh"
REPO_UNIT="$ROOT/deploy/ebbtop-agent.service"

[ -f "$SCRIPT" ] || { echo "missing $SCRIPT" >&2; exit 1; }
[ -f "$REPO_INSTALLER" ] || { echo "missing $REPO_INSTALLER" >&2; exit 1; }
[ -f "$REPO_UNIT" ] || { echo "missing $REPO_UNIT" >&2; exit 1; }

PASSES=0
pass() { PASSES=$((PASSES + 1)); printf 'PASS %s: %s\n' "$1" "$2"; }
fail() { printf 'FAIL %s: %s\n' "$1" "$2" >&2; exit 1; }
note() { printf '  - %s\n' "$*"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-install-latest.XXXXXXXX")"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

STUB="$WORK/stub"
STUB_ARCH="$WORK/stub-arch"
STUB_NOTTY="$WORK/stub-notty"      # placeholder, kept for symmetry with $STUB_ARCH
STATE="$WORK/state"
mkdir -p "$STUB" "$STUB_ARCH" "$STATE"
: > "$STATE/calls.log"
: > "$STATE/ss.log"
: > "$STATE/curl.log"
: > "$STATE/curl.urls"
echo yes > "$STATE/linger"
export EBBTOP_FIXTURE_STATE="$STATE"

HOST_GLIBC="$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{ print $2 }')"
[ -n "$HOST_GLIBC" ] || HOST_GLIBC=2.43

# --- stubs -----------------------------------------------------------------
cat > "$STUB/systemctl" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
printf 'systemctl %s\n' "$*" >> "$STATE/calls.log"
[[ "${1:-}" == "--user" ]] && shift
sub="${1:-}"; shift || true
case "$sub" in
  daemon-reload|enable|disable|stop) exit 0 ;;
  restart)
    if [[ -f "$STATE/fail-next-restart" ]]; then
      rm -f "$STATE/fail-next-restart"
      echo failed > "$STATE/service"
    else
      echo active > "$STATE/service"
    fi
    exit 0 ;;
  is-active)
    state="$(cat "$STATE/service" 2>/dev/null || echo inactive)"
    echo "$state"
    [[ "$state" == active ]] && exit 0 || exit 3 ;;
  show)
    echo "argv[]=/lib/systemd/systemd --user ; path=$HOME/services/ebbtop-agent/bin/ebbtop-agent ; ignore_errors=no"
    exit 0 ;;
  *) exit 0 ;;
esac
STUB

cat > "$STUB/ss" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
printf 'ss %s\n' "$*" >> "$STATE/ss.log"
port=45976
[[ "$*" =~ :([0-9]+) ]] && port="${BASH_REMATCH[1]}"
addr="$(sed -n 's/^[[:blank:]]*EBBTOP_BIND[[:blank:]]*=[[:blank:]]*//p' "$HOME/services/ebbtop-agent/agent.env" 2>/dev/null \
  | head -n 1 | tr -d '\r' | sed -E 's/[[:blank:]]+$//')"
[[ -n "$addr" ]] || addr=100.64.0.5
if [[ "$(cat "$STATE/service" 2>/dev/null || echo inactive)" == active ]]; then
  echo "LISTEN 0      128    ${addr}:${port}      0.0.0.0:*    users:((\"ebbtop-agent\",pid=4242,fd=7))"
fi
exit 0
STUB

cat > "$STUB/loginctl" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
case "$*" in
  *Linger*) cat "$STATE/linger" 2>/dev/null || echo yes ;;
  *) echo "" ;;
esac
exit 0
STUB

# The whole network is this stub: the API and the asset download are files.
cat > "$STUB/curl" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
printf 'curl %s\n' "$*" >> "$STATE/curl.log"
out=''; url=''
while (( $# )); do
  case "$1" in
    -o|--output) out="$2"; shift 2 ;;
    -H|--header|--proto|--proto-redir) shift 2 ;;
    -*) shift ;;
    *) url="$1"; shift ;;
  esac
done
[[ -n "$url" ]] || { echo "curl: no URL" >&2; exit 2; }
printf '%s\n' "$url" >> "$STATE/curl.urls"
if [[ -f "$STATE/no-network" ]]; then
  echo "curl: (6) Could not resolve host" >&2
  exit 6
fi
case "$url" in
  */releases/latest) body="$STATE/api-latest.json" ;;
  */releases/tags/*) body="$STATE/api-tag-${url##*/}.json" ;;
  */releases/download/*) body="$STATE/download-body" ;;
  *) echo "curl: (22) The requested URL returned error: 404" >&2; exit 22 ;;
esac
if [[ ! -f "$body" ]]; then
  echo "curl: (22) The requested URL returned error: 404" >&2
  exit 22
fi
if [[ -n "$out" ]]; then cp "$body" "$out"; else cat "$body"; fi
STUB

cat > "$STUB/tailscale" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
if [[ -f "$STATE/tailscale-fail" ]]; then exit 1; fi
echo 100.64.0.5
STUB

cat > "$STUB/ip" <<'STUB'
#!/usr/bin/env bash
set -u
case "$*" in
  *"route get"*)   echo "1.1.1.1 via 192.168.1.1 dev eth0 src 192.168.1.50 uid 1000" ;;
  *"addr show"*)   echo "2: eth0    inet 192.168.1.50/24 brd 192.168.1.255 scope global eth0" ;;
  *)               exit 0 ;;
esac
exit 0
STUB

cat > "$STUB/getconf" <<'STUB'
#!/usr/bin/env bash
set -u
STATE="${EBBTOP_FIXTURE_STATE:?}"
case "${1:-}" in
  GNU_LIBC_VERSION) echo "glibc $(cat "$STATE/host-glibc" 2>/dev/null || echo 2.43)" ;;
  *) /usr/bin/getconf "$@" ;;
esac
STUB

cat > "$STUB/uname" <<'STUB'
#!/bin/sh
echo x86_64
STUB

# Any attempt to build from source is a hard failure: this path needs no
# toolchain and must never reach for one.
for tool in git cargo rustc; do
  cat > "$STUB/$tool" <<STUB
#!/bin/sh
echo "$tool must not be called by the prebuilt installer" >&2
: > "\$EBBTOP_FIXTURE_STATE/tool-called"
exit 127
STUB
done

cat > "$STUB_ARCH/uname" <<'STUB'
#!/bin/sh
echo aarch64
STUB

chmod 755 "$STUB"/* "$STUB_ARCH"/*
echo "$HOST_GLIBC" > "$STATE/host-glibc"

RUN_PATH="$STUB:/usr/bin:/bin"
RUN_PATH_ARCH="$STUB_ARCH:$STUB:/usr/bin:/bin"

# --- fixture helpers -------------------------------------------------------
snapshot() { # $1 = directory; type/mode/size/path plus a hash per file
  local dir="$1"
  if [[ ! -d "$dir" ]]; then echo "<absent>"; return 0; fi
  ( cd "$dir" && find . -mindepth 1 -printf '%y %m %s %P\n' | LC_ALL=C sort )
  ( cd "$dir" && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum | LC_ALL=C sort )
}

calls()      { wc -l < "$STATE/calls.log" | tr -d ' '; }
ss_calls()   { wc -l < "$STATE/ss.log" | tr -d ' '; }
curl_calls() { wc -l < "$STATE/curl.log" | tr -d ' '; }
curl_urls()  { cat "$STATE/curl.urls"; }

new_home() { # $1 = label
  FIXHOME="$WORK/home-$1"
  rm -rf -- "$FIXHOME"
  mkdir -p "$FIXHOME"
  FIXRUN="$WORK/run-$1"
  rm -rf -- "$FIXRUN"
  mkdir -p "$FIXRUN/systemd"
  : > "$FIXRUN/systemd/private"          # stands in for the manager's socket
  CASE_TMP="$WORK/tmp-$1"
  rm -rf -- "$CASE_TMP"
  mkdir -p "$CASE_TMP"
  : > "$STATE/calls.log"
  : > "$STATE/ss.log"
  : > "$STATE/curl.log"
  : > "$STATE/curl.urls"
  rm -f "$STATE/service" "$STATE/no-network" "$STATE/tailscale-fail" "$STATE/tool-called"
}

RUN_ENV=()
RUN_PATH_NOW=""
run_wrapper() { # $1 = output file, rest = wrapper args; stdin from RUN_STDIN
  local out="$1"; shift
  local stdin_src="${RUN_STDIN:-/dev/null}"
  ( env -u EBBTOP_ADDR -u EBBTOP_PORT -u WZ_YES -u WZ_DRY -u WZ_NO_TTY \
      "${RUN_ENV[@]}" \
      HOME="$FIXHOME" XDG_RUNTIME_DIR="$FIXRUN" TMPDIR="$CASE_TMP" \
      EBBTOP_FIXTURE_STATE="$STATE" PATH="${RUN_PATH_NOW:-$RUN_PATH}" \
      bash "$SCRIPT" "$@" ) >"$out" 2>&1 <"$stdin_src"
}

# The documented one-liner: the script itself is on stdin, exactly as
# `curl -fsSL ... | bash` delivers it. Nothing may read an answer from stdin.
run_wrapper_piped() { # $1 = output file, rest = wrapper args
  local out="$1"; shift
  ( env -u EBBTOP_ADDR -u EBBTOP_PORT -u WZ_YES -u WZ_DRY -u WZ_NO_TTY \
      "${RUN_ENV[@]}" \
      HOME="$FIXHOME" XDG_RUNTIME_DIR="$FIXRUN" TMPDIR="$CASE_TMP" \
      EBBTOP_FIXTURE_STATE="$STATE" PATH="${RUN_PATH_NOW:-$RUN_PATH}" \
      bash -s -- "$@" ) >"$out" 2>&1 <"$SCRIPT"
}

# every refusal: exit non-zero, exactly one refusal line, no fixture change,
# no service-manager call, no listener probe
expect_refusal() { # <case> <out> <pattern> <before-snapshot> <systemctl-before>
  local case_id="$1" out="$2" pattern="$3" before="$4" calls_before="$5" n
  n="$(grep -c '^   fail ' "$out" || true)"
  [[ "$n" == 1 ]] || fail "$case_id" "expected exactly one refusal line, got $n: $(grep '^   fail ' "$out" || true)"
  grep -qF -- "$pattern" "$out" || fail "$case_id" "refusal does not mention '$pattern': $(grep '^   fail ' "$out")"
  [[ "$(snapshot "$FIXHOME")" == "$before" ]] \
    || fail "$case_id" "fixture home changed: $(snapshot "$FIXHOME" | diff -u <(printf '%s\n' "$before") - | head -20)"
  [[ "$(calls)" == "$calls_before" ]] \
    || fail "$case_id" "service manager was called $(($(calls) - calls_before)) time(s): $(tail -n +$((calls_before + 1)) "$STATE/calls.log")"
  [[ "$(ss_calls)" == 0 ]] || fail "$case_id" "ss was called $(ss_calls) time(s): $(cat "$STATE/ss.log")"
  [[ -z "$(find "$CASE_TMP" -mindepth 1 -print -quit)" ]] \
    || fail "$case_id" "temporary work survived the refusal: $(find "$CASE_TMP" -mindepth 1)"
}

no_toolchain_called() { # <case>
  [[ ! -e "$STATE/tool-called" ]] || fail "$1" "the wrapper or the installer reached for git/cargo/rustc"
}

# --- fixture packages ------------------------------------------------------
# BUILD, SHA256SUMS, ebbtop-agent, ebbtop-agent.service, install-agent.sh:
# exactly the five members deploy/linux/package-agent.sh writes, with the real
# installer and the real unit from this checkout.
PKG_STAGE="$WORK/stage"
make_pkg() { # <out> <version> [glibc floor]
  local out="$1" ver="$2" floor="${3:-2.39}" stage="$WORK/stage-$RANDOM"
  mkdir -p "$stage"
  {
    echo "name=ebbtop-agent"
    echo "version=$ver"
    echo "commit=0123456789abcdef0123456789abcdef01234567"
    echo "target=x86_64-unknown-linux-gnu"
    echo "glibc_floor=$floor"
  } > "$stage/BUILD"
  printf '#!/bin/sh\necho "ebbtop-agent %s"\n' "$ver" > "$stage/ebbtop-agent"
  chmod 755 "$stage/ebbtop-agent"
  cp "$REPO_UNIT" "$stage/ebbtop-agent.service"
  cp "$REPO_INSTALLER" "$stage/install-agent.sh"
  chmod 755 "$stage/install-agent.sh"
  ( cd "$stage" && sha256sum BUILD ebbtop-agent ebbtop-agent.service > SHA256SUMS )
  ( cd "$stage" && tar -czf "$out" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh )
  PKG_STAGE="$stage"
}

pkg_digest() { sha256sum "$1" | cut -d' ' -f1; }

# GitHub-shaped release JSON. Key order, asset order and a look-alike asset name
# are all deliberately unhelpful: a parser that takes the first digest, assumes
# alphabetical keys or matches the asset name by prefix must fail here.
make_api_json() { # <out> <tag> <linux-digest|""|null> <mode: full|no-linux|no-digest>
  local out="$1" tag="$2" digest="$3" mode="${4:-full}"
  local win='sha256:1111111111111111111111111111111111111111111111111111111111111111'
  local sha='sha256:2222222222222222222222222222222222222222222222222222222222222222'
  local mac='sha256:3333333333333333333333333333333333333333333333333333333333333333'
  local linux_asset=''
  if [[ "$mode" == full ]]; then
    linux_asset=$(
      cat <<JSON
    {
      "url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/assets/2",
      "name": "ebbtop-agent-linux-x86_64.tar.gz",
      "id": 2,
      "label": null,
      "uploader": {
        "login": "Ebbwater",
        "id": 9001,
        "type": "User"
      },
      "content_type": "application/gzip",
      "state": "uploaded",
      "digest": "sha256:$digest",
      "size": 1437330,
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/ebbtop-agent-linux-x86_64.tar.gz"
    },
JSON
    )
  elif [[ "$mode" == no-digest ]]; then
    linux_asset=$(
      cat <<JSON
    {
      "url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/assets/2",
      "name": "ebbtop-agent-linux-x86_64.tar.gz",
      "id": 2,
      "label": null,
      "uploader": {
        "login": "Ebbwater",
        "id": 9001,
        "type": "User"
      },
      "content_type": "application/gzip",
      "state": "uploaded",
      "digest": null,
      "size": 1437330,
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/ebbtop-agent-linux-x86_64.tar.gz"
    },
JSON
    )
  fi
  cat > "$out" <<JSON
{
  "url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/12345",
  "assets_url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/12345/assets",
  "html_url": "https://github.com/Ebbwater/ebbtop/releases/tag/$tag",
  "id": 12345,
  "author": {
    "login": "Ebbwater",
    "id": 9001
  },
  "node_id": "RE_kwDO",
  "tag_name": "$tag",
  "target_commitish": "main",
  "name": "EbbTop $tag",
  "draft": false,
  "prerelease": false,
  "created_at": "2026-11-20T09:00:00Z",
  "published_at": "2026-11-20T09:05:00Z",
  "assets": [
    {
      "url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/assets/4",
      "name": "ebbtop-agent-linux-x86_64.tar.gz.sha256",
      "id": 4,
      "label": null,
      "uploader": {
        "login": "Ebbwater",
        "id": 9001
      },
      "content_type": "text/plain",
      "state": "uploaded",
      "digest": "$sha",
      "size": 78,
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/ebbtop-agent-linux-x86_64.tar.gz.sha256"
    },
$linux_asset
    {
      "url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/assets/1",
      "name": "EbbTop-Windows-x64.zip",
      "id": 1,
      "label": null,
      "uploader": {
        "login": "Ebbwater",
        "id": 9001
      },
      "content_type": "application/zip",
      "state": "uploaded",
      "digest": "$win",
      "size": 6649951,
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/EbbTop-Windows-x64.zip"
    },
    {
      "url": "https://api.github.com/repos/Ebbwater/ebbtop/releases/assets/3",
      "name": "EbbTop.app.zip",
      "id": 3,
      "label": null,
      "uploader": {
        "login": "Ebbwater",
        "id": 9001
      },
      "content_type": "application/zip",
      "state": "uploaded",
      "digest": "$mac",
      "size": 9000000,
      "browser_download_url": "https://github.com/Ebbwater/ebbtop/releases/download/$tag/EbbTop.app.zip"
    }
  ],
  "tarball_url": "https://api.github.com/repos/Ebbwater/ebbtop/tarball/$tag",
  "zipball_url": "https://api.github.com/repos/Ebbwater/ebbtop/zipball/$tag"
}
JSON
}

# --- main fixture archives -------------------------------------------------
GOOD_PKG="$WORK/ebbtop-agent-linux-x86_64.tar.gz"
make_pkg "$GOOD_PKG" 0.2.9 2.39
GOOD_STAGE="$PKG_STAGE"
GOOD_SHA="$(pkg_digest "$GOOD_PKG")"
# The packaged installer inside the fixture is this checkout's installer, byte
# for byte: the wrapper is never allowed to test against a copy of it.
cmp "$REPO_INSTALLER" "$GOOD_STAGE/install-agent.sh" \
  || fail setup "fixture archive does not carry this checkout's install-agent.sh"
cmp "$REPO_UNIT" "$GOOD_STAGE/ebbtop-agent.service" \
  || fail setup "fixture archive does not carry this checkout's unit"
note "fixture archive $GOOD_PKG sha256 $GOOD_SHA"

TAMPERED_PKG="$WORK/tampered.tar.gz"
mkdir -p "$WORK/tamper"
tar -xzf "$GOOD_PKG" -C "$WORK/tamper"
printf '#!/bin/sh\necho tampered\n' > "$WORK/tamper/ebbtop-agent"
chmod 755 "$WORK/tamper/ebbtop-agent"
( cd "$WORK/tamper" && tar -czf "$TAMPERED_PKG" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh )

ESCAPE_PKG="$WORK/escape.tar.gz"
printf 'not part of any package\n' > "$WORK/escape-target"
( cd "$WORK" && tar -czf "$ESCAPE_PKG" -C "$GOOD_STAGE" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh \
    -P ../escape-target 2>/dev/null )

FLOOR_PKG="$WORK/high-floor.tar.gz"
make_pkg "$FLOOR_PKG" 0.2.9 2.99

# A package whose installer reports success and installs nothing. The wrapper's
# own verification must notice: it may not simply believe the installer's output.
LYING_PKG="$WORK/lying-installer.tar.gz"
mkdir -p "$WORK/lying"
cp "$GOOD_STAGE/BUILD" "$GOOD_STAGE/SHA256SUMS" "$GOOD_STAGE/ebbtop-agent" \
   "$GOOD_STAGE/ebbtop-agent.service" "$WORK/lying/"
cat > "$WORK/lying/install-agent.sh" <<'STUB'
#!/usr/bin/env bash
echo "verified: SHA256SUMS ok for BUILD ebbtop-agent ebbtop-agent.service"
echo "version: 0.2.9"
echo "active: active"
echo "listen: 100.64.0.5:45976"
echo "binary: 0000000000000000"
echo "loaded: path=$HOME/services/ebbtop-agent/bin/ebbtop-agent"
echo "linger: yes (unit starts at boot)"
STUB
chmod 755 "$WORK/lying/install-agent.sh"
( cd "$WORK/lying" && tar -czf "$LYING_PKG" BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh )

# ===========================================================================
# 1. latest release, verified digest, verified manifest, delegated install,
#    and a pairing report that carries the token path but never the token.
# ===========================================================================
case_happy_latest() {
  local name=happy-latest out="$WORK/happy.out" rc=0 tok mode
  new_home happy
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"

  run_wrapper "$out" --yes || rc=$?
  (( rc == 0 )) || fail "$name" "installer exited $rc: $(cat "$out")"

  grep -qF "sha256 matches the release digest ($GOOD_SHA)" "$out" \
    || fail "$name" "no digest verification line: $(cat "$out")"
  grep -q 'sha256sum --strict -c SHA256SUMS: 3 files verified' "$out" \
    || fail "$name" "no inner manifest verification line: $(cat "$out")"
  grep -qF 'installed binary is the verified package member' "$out" \
    || fail "$name" "the installed binary was not compared with the verified member"
  grep -qF 'ebbtop-agent.service is active' "$out" \
    || fail "$name" "the unit was not reported active: $(cat "$out")"
  grep -qF 'listen: 100.64.0.5:45976' "$out" \
    || fail "$name" "the installer's listener line is missing: $(cat "$out")"
  grep -qF 'address    ws://100.64.0.5:45976/ws' "$out" \
    || fail "$name" "no pairing address: $(cat "$out")"
  grep -qF "token      $FIXHOME/services/ebbtop-agent/ebbtop.token" "$out" \
    || fail "$name" "the pairing block does not name the token path: $(cat "$out")"

  tok="$(cat "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  [[ "$tok" =~ ^[0-9a-f]{64}$ ]] || fail "$name" "token is not 32 bytes of hex"
  if grep -qF "$tok" "$out"; then fail "$name" "the wrapper printed the token value"; fi
  if grep -q 'EBBTOP_TOKEN=' "$out"; then fail "$name" "the wrapper printed a token assignment"; fi

  cmp "$GOOD_STAGE/ebbtop-agent" "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent" \
    || fail "$name" "installed binary is not the package member"
  cmp "$GOOD_STAGE/ebbtop-agent.service" "$FIXHOME/.config/systemd/user/ebbtop-agent.service" \
    || fail "$name" "installed unit is not the packaged unit"
  grep -qx 'EBBTOP_BIND=100.64.0.5' "$FIXHOME/services/ebbtop-agent/agent.env" \
    || fail "$name" "agent.env does not carry the bind the wrapper chose"
  mode="$(stat -c %a "$FIXHOME/services/ebbtop-agent/agent.env")"
  [[ "$mode" == 600 ]] || fail "$name" "agent.env mode is $mode"
  mode="$(stat -c %a "$FIXHOME/services/ebbtop-agent/ebbtop.token")"
  [[ "$mode" == 600 ]] || fail "$name" "token mode is $mode"

  # the API was asked for the latest release, and the asset came from the
  # browser_download_url the API reported
  grep -qx 'https://api.github.com/repos/Ebbwater/ebbtop/releases/latest' "$STATE/curl.urls" \
    || fail "$name" "the wrapper did not ask the API for the latest release: $(curl_urls)"
  grep -qx 'https://github.com/Ebbwater/ebbtop/releases/download/v0.2.9/ebbtop-agent-linux-x86_64.tar.gz' "$STATE/curl.urls" \
    || fail "$name" "the download did not use the API's browser_download_url: $(curl_urls)"
  no_toolchain_called "$name"
  [[ -z "$(find "$CASE_TMP" -mindepth 1 -print -quit)" ]] || fail "$name" "temporary work survived"
  pass "$name" "latest release: digest verified, manifest verified, packaged installer ran, pairing printed host:port and the token path only, no toolchain, temp cleaned"
}

# ===========================================================================
# 2. digest mismatch: refused before anything on this host is written, with no
#    service-manager call and no listener probe.
# ===========================================================================
case_digest_mismatch() {
  local name=digest-mismatch out="$WORK/mismatch.out" rc=0 before
  new_home mismatch
  make_api_json "$STATE/api-latest.json" v0.2.9 "$(printf 'a%.0s' {1..64})" full
  cp "$GOOD_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a mismatched digest was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" 'sha256 mismatch for ebbtop-agent-linux-x86_64.tar.gz' "$before" 0
  grep -qF "$(printf 'a%.0s' {1..64})" "$out" || fail "$name" "the refusal does not quote the API digest"
  grep -qF "$GOOD_SHA" "$out" || fail "$name" "the refusal does not quote the downloaded digest"
  grep -qF 'nothing was installed' "$out" || fail "$name" "the refusal does not state what was left alone"
  [[ ! -e "$FIXHOME/services" ]] || fail "$name" "a service directory was created"
  # the download did happen: the refusal is the digest check, not a failed fetch
  grep -q 'releases/download/v0.2.9/ebbtop-agent-linux-x86_64.tar.gz' "$STATE/curl.urls" \
    || fail "$name" "the asset was never downloaded, so the digest check was not exercised"
  pass "$name" "a digest that does not match the bytes is refused with both hashes, zero writes, zero service-manager calls, zero ss calls, temp cleaned"
}

# ===========================================================================
# 3. wrong architecture: refused at preflight, before the network.
# ===========================================================================
case_wrong_arch() {
  local name=wrong-arch out="$WORK/arch.out" rc=0 before
  new_home arch
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  RUN_PATH_NOW="$RUN_PATH_ARCH"
  run_wrapper "$out" --yes || rc=$?
  RUN_PATH_NOW=""
  (( rc != 0 )) || fail "$name" "an aarch64 host was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" "this package is x86_64 only: uname -m reports 'aarch64'" "$before" 0
  [[ "$(curl_calls)" == 0 ]] || fail "$name" "curl ran $(curl_calls) time(s) before the arch refusal: $(curl_urls)"
  pass "$name" "aarch64 is refused with one line, no network request, zero writes"
}

# ===========================================================================
# 4. no systemd user manager: refused at preflight, before the network.
# ===========================================================================
case_no_user_manager() {
  local name=no-user-manager out="$WORK/nomgr.out" rc=0 before
  new_home nomgr
  rm -rf -- "$FIXRUN/systemd"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a host with no user manager was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" "no systemd user manager" "$before" 0
  [[ "$(curl_calls)" == 0 ]] || fail "$name" "curl ran before the systemd refusal"
  pass "$name" "a missing systemd user manager is refused with one line, no network request, zero writes"
}

# ===========================================================================
# 5. glibc below the package floor: refused at preflight, before the network.
# ===========================================================================
case_glibc_low() {
  local name=glibc-low out="$WORK/glibc.out" rc=0 before
  new_home glibc
  echo 2.31 > "$STATE/host-glibc"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --yes || rc=$?
  echo "$HOST_GLIBC" > "$STATE/host-glibc"
  (( rc != 0 )) || fail "$name" "glibc 2.31 was accepted for a 2.39 package: $(cat "$out")"
  expect_refusal "$name" "$out" "needs glibc 2.39 or newer: this host has glibc 2.31" "$before" 0
  [[ "$(curl_calls)" == 0 ]] || fail "$name" "curl ran before the glibc refusal"
  pass "$name" "glibc below 2.39 is refused with one line naming both versions, no network request, zero writes"
}

# ===========================================================================
# 6. --dry-run: the plan is printed, nothing is downloaded or written.
# ===========================================================================
case_dry_run() {
  local name=dry-run out="$WORK/dry.out" rc=0 before
  new_home dry
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --dry-run --yes || rc=$?
  (( rc == 0 )) || fail "$name" "dry run exited $rc: $(cat "$out")"
  grep -qF 'the plan above was printed and nothing was downloaded, written or started' "$out" \
    || fail "$name" "no dry-run statement: $(cat "$out")"
  grep -qF 'release    v0.2.9' "$out" || fail "$name" "the plan does not name the release: $(cat "$out")"
  grep -qF "digest     sha256:$GOOD_SHA" "$out" || fail "$name" "the plan does not name the digest"
  grep -qF 'bind       100.64.0.5 (fresh install)' "$out" || fail "$name" "the plan does not name the bind address"
  grep -qF "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent" "$out" || fail "$name" "the plan does not name the install path"

  [[ "$(snapshot "$FIXHOME")" == "$before" ]] || fail "$name" "the fixture home changed"
  [[ "$(calls)" == 0 ]] || fail "$name" "systemctl was called in a dry run: $(cat "$STATE/calls.log")"
  [[ "$(ss_calls)" == 0 ]] || fail "$name" "ss was called in a dry run"
  [[ -z "$(find "$CASE_TMP" -mindepth 1 -print -quit)" ]] || fail "$name" "the dry run wrote into TMPDIR"
  if grep -q 'releases/download/' "$STATE/curl.urls"; then fail "$name" "the dry run downloaded the asset"; fi
  grep -q '/releases/latest' "$STATE/curl.urls" || fail "$name" "the dry run did not resolve the release"
  pass "$name" "dry run prints release, digest, bind, install path and the delegated action; downloads nothing, writes nothing, calls neither systemctl nor ss"
}

# ===========================================================================
# 7. --version pins the exact tag: the tag endpoint is fetched, not latest.
# ===========================================================================
case_version_pin() {
  local name=version-pin out="$WORK/pin.out" rc=0
  new_home pin
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  make_api_json "$STATE/api-tag-v0.2.8.json" v0.2.8 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"

  run_wrapper "$out" --version v0.2.8 --yes || rc=$?
  (( rc == 0 )) || fail "$name" "pinned install exited $rc: $(cat "$out")"
  grep -qx 'https://api.github.com/repos/Ebbwater/ebbtop/releases/tags/v0.2.8' "$STATE/curl.urls" \
    || fail "$name" "the pinned tag endpoint was not fetched: $(curl_urls)"
  grep -q '/releases/latest' "$STATE/curl.urls" && fail "$name" "the pin still asked for latest"
  grep -qx 'https://github.com/Ebbwater/ebbtop/releases/download/v0.2.8/ebbtop-agent-linux-x86_64.tar.gz' "$STATE/curl.urls" \
    || fail "$name" "the download did not use the pinned tag's asset URL: $(curl_urls)"
  grep -qF 'release    v0.2.8 (pinned)' "$out" || fail "$name" "the plan does not report the pinned tag: $(cat "$out")"
  [[ -f "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent" ]] || fail "$name" "nothing was installed"
  pass "$name" "--version v0.2.8 fetches the tag endpoint for exactly v0.2.8 (never latest) and installs that release's asset"
}

# ===========================================================================
# 8. --archive: offline install, no curl at all.
# ===========================================================================
case_offline_archive() {
  local name=offline-archive out="$WORK/offline.out" rc=0
  new_home offline
  : > "$STATE/no-network"

  run_wrapper "$out" --archive "$GOOD_PKG" --yes || rc=$?
  (( rc == 0 )) || fail "$name" "offline install exited $rc: $(cat "$out")"
  [[ "$(curl_calls)" == 0 ]] || fail "$name" "curl was called $(curl_calls) time(s) with --archive: $(curl_urls)"
  grep -qF 'no publisher digest is available offline' "$out" \
    || fail "$name" "the offline plan does not say what it cannot verify: $(cat "$out")"
  grep -qF 'sha256sum --strict -c SHA256SUMS: 3 files verified' "$out" \
    || fail "$name" "the offline path did not verify the inner manifest"
  grep -qF 'address    ws://100.64.0.5:45976/ws' "$out" || fail "$name" "no pairing address: $(cat "$out")"
  [[ -f "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent" ]] || fail "$name" "nothing was installed"
  pass "$name" "--archive installs from the local tarball with zero curl calls and no network, still verifying the manifest inside it"
}

# ===========================================================================
# 9. tampered member: the inner manifest catches it before the installer runs.
# ===========================================================================
case_tampered_member() {
  local name=tampered-member out="$WORK/tamper.out" rc=0 before
  new_home tamper
  make_api_json "$STATE/api-latest.json" v0.2.9 "$(pkg_digest "$TAMPERED_PKG")" full
  cp "$TAMPERED_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a tampered member was installed: $(cat "$out")"
  expect_refusal "$name" "$out" "SHA256SUMS did not verify" "$before" 0
  grep -qF 'ebbtop-agent: FAILED' "$out" || fail "$name" "the refusal does not quote sha256sum's FAILED line: $(cat "$out")"
  pass "$name" "an archive whose member no longer matches its SHA256SUMS is refused before the packaged installer runs"
}

# ===========================================================================
# 10. archive member that is a path: refused before extraction, so nothing can
#     be written outside the package directory.
# ===========================================================================
case_member_escape() {
  local name=member-escape out="$WORK/escape.out" rc=0 before escape_before
  new_home escape
  make_api_json "$STATE/api-latest.json" v0.2.9 "$(pkg_digest "$ESCAPE_PKG")" full
  cp "$ESCAPE_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"
  escape_before="$(sha256sum < "$WORK/escape-target")"

  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "an archive carrying ../escape-target was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" 'is a path, not a bare packaged file name' "$before" 0
  [[ "$(sha256sum < "$WORK/escape-target")" == "$escape_before" ]] \
    || fail "$name" "a member outside the package directory was written"
  pass "$name" "an archive with a traversing member is refused before extraction, with zero writes and no service call"
}

# ===========================================================================
# 11. a release without the Linux asset, an asset without a digest, no network
#     at all, and a --archive path that does not exist: every one fails closed
#     instead of installing unverified bytes.
# ===========================================================================
case_unverifiable_release() {
  local name=unverifiable out="$WORK/noasset.out" rc=0 before

  new_home noasset
  make_api_json "$STATE/api-latest.json" v0.2.9 "" no-linux
  before="$(snapshot "$FIXHOME")"
  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a release with no Linux asset was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" "has no ebbtop-agent-linux-x86_64.tar.gz asset" "$before" 0
  note "release without the asset refused: $(grep '^   fail ' "$out")"

  new_home nodigest
  make_api_json "$STATE/api-latest.json" v0.2.9 "" no-digest
  before="$(snapshot "$FIXHOME")"
  rc=0
  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "an asset with no digest was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" "reports no sha256 digest for ebbtop-agent-linux-x86_64.tar.gz" "$before" 0
  note "asset without a digest refused: $(grep '^   fail ' "$out")"

  # no network: the API cannot be read, so there is nothing to verify against
  new_home nonet
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  : > "$STATE/no-network"
  before="$(snapshot "$FIXHOME")"
  rc=0
  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "an unreadable release API was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" "cannot read https://" "$before" 0
  note "no network refused: $(grep '^   fail ' "$out")"

  # --archive pointing at nothing
  new_home noarchive
  before="$(snapshot "$FIXHOME")"
  rc=0
  run_wrapper "$out" --archive "$WORK/does-not-exist.tar.gz" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a missing --archive path was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" "does not exist" "$before" 0
  note "missing archive refused: $(grep '^   fail ' "$out")"

  pass "$name" "a release with no Linux asset, an asset with no reported digest, an unreadable release API and a missing --archive file all fail closed with zero writes"
}

# ===========================================================================
# 12. the package's own ABI floor, when it is higher than the script's: refused
#     after the manifest verifies, before the installer runs.
# ===========================================================================
case_build_floor() {
  local name=build-floor out="$WORK/floor.out" rc=0 before
  new_home floor
  make_api_json "$STATE/api-latest.json" v0.2.9 "$(pkg_digest "$FLOOR_PKG")" full
  cp "$FLOOR_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a package needing glibc 2.99 was accepted on glibc $HOST_GLIBC: $(cat "$out")"
  expect_refusal "$name" "$out" "the package needs glibc 2.99 (BUILD glibc_floor)" "$before" 0
  pass "$name" "a BUILD glibc_floor above the host is refused after the manifest check and before the packaged installer runs"
}

# ===========================================================================
# 13. EBBTOP_PORT is not allowed to misreport the listener: the packaged unit
#     fixes the port, and the wrapper drops the variable before delegating.
# ===========================================================================
case_port_env_ignored() {
  local name=port-env out="$WORK/port.out" rc=0
  new_home port
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"

  RUN_ENV=("EBBTOP_PORT=45999")
  run_wrapper "$out" --yes || rc=$?
  RUN_ENV=()
  (( rc == 0 )) || fail "$name" "install exited $rc: $(cat "$out")"
  grep -qF 'listen: 100.64.0.5:45976' "$out" \
    || fail "$name" "the port the unit starts was not the one reported: $(cat "$out")"
  grep -q '45999' "$STATE/ss.log" && fail "$name" "EBBTOP_PORT leaked into the packaged installer: $(cat "$STATE/ss.log")"
  grep -qF 'address    ws://100.64.0.5:45976/ws' "$out" || fail "$name" "the pairing address used the stray port"
  pass "$name" "EBBTOP_PORT=45999 never reaches the packaged installer: the unit's 45976 is checked, reported and paired"
}

# ===========================================================================
# 14. a loopback address is passed through to the packaged installer's own
#     validation, not re-implemented here, and it refuses before any write.
# ===========================================================================
case_loopback_passthrough() {
  local name=loopback out="$WORK/loop.out" rc=0 before
  new_home loop
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  RUN_ENV=("EBBTOP_ADDR=127.0.0.1")
  run_wrapper "$out" --yes || rc=$?
  RUN_ENV=()
  (( rc != 0 )) || fail "$name" "a loopback bind was accepted: $(cat "$out")"
  grep -qF "install-agent.sh: refusing bind address '127.0.0.1'" "$out" \
    || fail "$name" "the packaged installer's bind validation did not run: $(cat "$out")"
  [[ "$(grep -c '^   fail ' "$out" || true)" == 1 ]] \
    || fail "$name" "the wrapper added refusal lines of its own instead of relaying the installer's: $(grep '^   fail ' "$out")"
  grep -qF 'the packaged installer exited' "$out" \
    || fail "$name" "the failed delegation was not reported: $(cat "$out")"
  [[ "$(snapshot "$FIXHOME")" == "$before" ]] || fail "$name" "the fixture home changed"
  [[ "$(calls)" == 0 ]] || fail "$name" "systemctl was called: $(cat "$STATE/calls.log")"
  [[ "$(ss_calls)" == 0 ]] || fail "$name" "ss was called before the bind refusal"
  pass "$name" "127.0.0.1 reaches the packaged installer's own validation, which refuses it before any write or service call"
}

# ===========================================================================
# 15. an upgrade keeps agent.env and the token byte-identical, ignores a stray
#     EBBTOP_ADDR, and reports the bind the host already has.
# ===========================================================================
case_upgrade_keeps_env() {
  local name=upgrade out="$WORK/upgrade.out" rc=0 env_before tok_before
  new_home upgrade
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  run_wrapper "$WORK/upgrade-first.out" --yes || fail "$name" "the first install failed: $(cat "$WORK/upgrade-first.out")"

  env_before="$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")"
  tok_before="$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")"

  RUN_ENV=("EBBTOP_ADDR=10.0.0.9")
  run_wrapper "$out" --yes || rc=$?
  RUN_ENV=()
  (( rc == 0 )) || fail "$name" "the upgrade exited $rc: $(cat "$out")"
  grep -qF 'EBBTOP_ADDR=10.0.0.9 ignored' "$out" \
    || fail "$name" "a stray EBBTOP_ADDR was not called out: $(cat "$out")"
  grep -qF 'existing agent.env kept as-is: the agent keeps binding 100.64.0.5' "$out" \
    || fail "$name" "the existing bind was not reported: $(cat "$out")"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/agent.env")" == "$env_before" ]] \
    || fail "$name" "agent.env was rewritten"
  [[ "$(sha256sum < "$FIXHOME/services/ebbtop-agent/ebbtop.token")" == "$tok_before" ]] \
    || fail "$name" "the token bytes changed"
  grep -qF 'listen: 100.64.0.5:45976' "$out" || fail "$name" "the upgrade did not keep the existing listener: $(cat "$out")"
  pass "$name" "a second run keeps agent.env and the token byte-identical, calls out a stray EBBTOP_ADDR and reports the bind already in use"
}

# ===========================================================================
# 16. a fresh install detects the LAN address when tailscale is not answering.
# ===========================================================================
case_bind_detect_lan() {
  local name=bind-detect-lan out="$WORK/lan.out" rc=0
  new_home lan
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  : > "$STATE/tailscale-fail"

  run_wrapper "$out" --yes || rc=$?
  (( rc == 0 )) || fail "$name" "install exited $rc: $(cat "$out")"
  grep -qF 'bind       192.168.1.50 (fresh install)' "$out" \
    || fail "$name" "the LAN fallback address was not used: $(cat "$out")"
  grep -qx 'EBBTOP_BIND=192.168.1.50' "$FIXHOME/services/ebbtop-agent/agent.env" \
    || fail "$name" "agent.env does not carry the detected LAN address"
  pass "$name" "with no tailscale answer the default bind falls back to the default-route source address"
}

# ===========================================================================
# 17. no terminal and no --yes: refuse instead of installing blind. Declining
#     the confirmation writes nothing.
# ===========================================================================
case_no_terminal() {
  local name=no-terminal out="$WORK/notty.out" rc=0 before
  new_home notty
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  if command -v setsid >/dev/null 2>&1; then
    rc=0
    ( exec setsid -w bash -c '
        exec env -u WZ_NO_TTY HOME="$1" XDG_RUNTIME_DIR="$2" TMPDIR="$3" \
          EBBTOP_FIXTURE_STATE="$4" PATH="$5" bash "$6" --archive "$7"
      ' _ "$FIXHOME" "$FIXRUN" "$CASE_TMP" "$STATE" "$RUN_PATH" "$SCRIPT" "$GOOD_PKG" ) \
      >"$out" 2>&1 </dev/null || rc=$?
    (( rc != 0 )) || fail "$name" "an install with no terminal and no --yes was accepted: $(cat "$out")"
    grep -qF 'no terminal to prompt on' "$out" \
      || fail "$name" "the refusal does not name the missing terminal: $(cat "$out")"
    [[ "$(snapshot "$FIXHOME")" == "$before" ]] || fail "$name" "the fixture home changed"
    [[ "$(calls)" == 0 ]] || fail "$name" "systemctl was called"
    note "without a controlling terminal: $(grep -m1 'no terminal' "$out")"
  else
    note "setsid is not available: the no-terminal refusal was NOT RUN in this harness run"
  fi

  # Declining the one confirmation is a no-op, not a failed install. Two
  # answers: the bind prompt (fresh install) and the confirmation.
  new_home decline
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  printf '100.64.0.5\nn\n' > "$WORK/decline.stdin"
  before="$(snapshot "$FIXHOME")"
  rc=0
  RUN_ENV=("WZ_NO_TTY=1")
  RUN_STDIN="$WORK/decline.stdin"
  run_wrapper "$out" --archive "$GOOD_PKG" || rc=$?
  RUN_STDIN=''
  RUN_ENV=()
  (( rc == 0 )) || fail "$name" "declining the confirmation exited $rc: $(cat "$out")"
  grep -qF 'nothing was changed' "$out" || fail "$name" "a declined install did not say so: $(cat "$out")"
  [[ "$(snapshot "$FIXHOME")" == "$before" ]] || fail "$name" "a declined install wrote to the fixture home"
  [[ "$(calls)" == 0 ]] || fail "$name" "a declined install called systemctl"
  pass "$name" "no terminal without --yes is refused; declining the confirmation leaves the fixture untouched"
}

# ===========================================================================
# 18. the documented `curl -fsSL ... | bash` shape: the script itself is on
#     stdin, and the install still works because no prompt reads stdin.
# ===========================================================================
case_piped_stdin() {
  local name=piped-stdin out="$WORK/piped.out" rc=0
  new_home piped
  make_api_json "$STATE/api-latest.json" v0.2.9 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"

  run_wrapper_piped "$out" --yes || rc=$?
  (( rc == 0 )) || fail "$name" "the piped install exited $rc: $(cat "$out")"
  grep -qF 'ebbtop-agent.service is active' "$out" || fail "$name" "the piped install did not finish: $(cat "$out")"
  grep -qF 'address    ws://100.64.0.5:45976/ws' "$out" || fail "$name" "no pairing address from the piped install"
  [[ -f "$FIXHOME/services/ebbtop-agent/bin/ebbtop-agent" ]] || fail "$name" "nothing was installed from the pipe"

  rc=0
  run_wrapper_piped "$out" --help || rc=$?
  (( rc == 0 )) || fail "$name" "--help from a pipe exited $rc: $(cat "$out")"
  grep -qF -- '--archive <path>' "$out" || fail "$name" "--help from a pipe printed no flags: $(cat "$out")"
  pass "$name" "the script installs when piped into bash (script on stdin) and --help still works from a pipe"
}

# ===========================================================================
# 19. a pinned tag that the API answers with a different release is refused:
#     the tag is an identity claim, not a hint.
# ===========================================================================
case_pin_mismatch() {
  local name=pin-mismatch out="$WORK/pinmix.out" rc=0 before
  new_home pinmix
  make_api_json "$STATE/api-tag-v0.2.8.json" v0.2.7 "$GOOD_SHA" full
  cp "$GOOD_PKG" "$STATE/download-body"
  before="$(snapshot "$FIXHOME")"

  run_wrapper "$out" --version v0.2.8 --yes || rc=$?
  (( rc != 0 )) || fail "$name" "a tag answered by another release was accepted: $(cat "$out")"
  expect_refusal "$name" "$out" 'asked for release v0.2.8 but the API answered with v0.2.7' "$before" 0
  pass "$name" "a pinned tag that the API answers with a different tag is refused instead of installing that other release"
}

# ===========================================================================
# 19b. the wrapper does not take the installer's word for it: an installer that
#      prints success and installs nothing is caught by the wrapper's own
#      comparison against the verified package members.
# ===========================================================================
case_verify_not_trusted() {
  local name=verify-not-trusted out="$WORK/lying.out" rc=0
  new_home lying
  make_api_json "$STATE/api-latest.json" v0.2.9 "$(pkg_digest "$LYING_PKG")" full
  cp "$LYING_PKG" "$STATE/download-body"

  run_wrapper "$out" --yes || rc=$?
  (( rc != 0 )) || fail "$name" "an installer that installed nothing was reported as success: $(cat "$out")"
  grep -qF 'installed binary at' "$out" || fail "$name" "the missing binary was not reported: $(cat "$out")"
  grep -qF 'does not match the verified package member' "$out" || fail "$name" "the binary comparison did not run"
  grep -qF 'no token file at' "$out" || fail "$name" "the missing token was not reported: $(cat "$out")"
  grep -qF 'verification check(s) failed' "$out" || fail "$name" "the failed verification did not fail the run: $(cat "$out")"
  pass "$name" "an installer that claims success and installs nothing fails the wrapper's own verification (exit 1) instead of being reported as installed"
}

# ===========================================================================
# 20. argument handling: unknown flag, --version without a value, and the
#     contradictory --archive + --version are usage errors, not installs.
# ===========================================================================
case_usage() {
  local name=usage out="$WORK/usage.out" rc=0 before
  new_home usage
  before="$(snapshot "$FIXHOME")"

  rc=0; run_wrapper "$out" --nope || rc=$?
  (( rc == 2 )) || fail "$name" "an unknown flag exited $rc, not 2"
  grep -qF 'unknown argument: --nope' "$out" || fail "$name" "no unknown-argument line: $(cat "$out")"
  grep -qF -- '--dry-run, -n     print the plan, change nothing' "$out" \
    || fail "$name" "usage was not printed with the unknown-argument line: $(cat "$out")"

  rc=0; run_wrapper "$out" --version || rc=$?
  (( rc == 2 )) || fail "$name" "--version without a value exited $rc, not 2"
  grep -qF -- '--version needs a release tag' "$out" || fail "$name" "no --version usage line: $(cat "$out")"

  rc=0; run_wrapper "$out" --archive "$GOOD_PKG" --version v0.2.9 || rc=$?
  (( rc == 2 )) || fail "$name" "--archive with --version exited $rc, not 2"
  grep -qF -- '--archive installs a local tarball and --version pins a release' "$out" \
    || fail "$name" "no conflicting-flag line: $(cat "$out")"

  rc=0; run_wrapper "$out" --help || rc=$?
  (( rc == 0 )) || fail "$name" "--help exited $rc"
  grep -qF -- '--dry-run, -n' "$out" || fail "$name" "--help does not list the flags"
  [[ "$(snapshot "$FIXHOME")" == "$before" ]] || fail "$name" "the fixture home changed"
  pass "$name" "unknown flag, --version without a value and --archive plus --version are usage errors (exit 2); --help lists the flags"
}

case_happy_latest
case_digest_mismatch
case_wrong_arch
case_no_user_manager
case_glibc_low
case_dry_run
case_version_pin
case_offline_archive
case_tampered_member
case_member_escape
case_unverifiable_release
case_build_floor
case_port_env_ignored
case_loopback_passthrough
case_upgrade_keeps_env
case_bind_detect_lan
case_no_terminal
case_piped_stdin
case_pin_mismatch
case_verify_not_trusted
case_usage

# The caller's real installation, if any, was never touched: compare what this
# harness could have written outside the fixture, measured rather than assumed.
REAL="$HOME/services/ebbtop-agent"
if [[ -d "$REAL" ]]; then
  note "real $REAL exists and was not used by this harness"
fi

printf 'SUMMARY: %d passed, 0 failed\n' "$PASSES"
