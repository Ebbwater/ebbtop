#!/usr/bin/env bash
# ebbtop-install.sh: one command installs the prebuilt EbbTop agent.
#
#   curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash
#   curl -fsSL .../ebbtop-install.sh | bash -s -- --version v0.3.1
#
# The same published script serves both platforms. It selects the package from
# `uname -s` and `uname -m` BEFORE it resolves a release, so an unsupported
# host is refused with one line and no network access:
#
#   Linux  x86_64       ebbtop-agent-linux-x86_64.tar.gz    systemd user unit
#   Darwin arm64        ebbtop-agent-macos-arm64.tar.gz     per-user LaunchAgent
#
# Anything else (an Intel Mac, Windows, aarch64 Linux, any other uname) is
# refused: this release ships and tests no package for it, and inventing one
# would be a false claim about what was verified.
#
# This is a thin fetch-and-verify wrapper. It installs nothing itself: the
# install, the service definition, the token, the login/linger guidance and
# the self-verification all belong to the packaged install-agent.sh
# --prebuilt, which ships inside the release archive byte-identical to this
# repository. The wrapper only proves that the bytes it hands over are the
# bytes the release published.
#
# What it does, in this order:
#   1. preflight, one clear refusal line each, before any network access:
#        Linux: x86_64, glibc >= 2.39, a systemd user manager
#        macOS: Darwin arm64 (not a Rosetta shell), a logged-in launchd domain
#   2. resolve the release: latest via the GitHub API, --version <tag> to pin,
#      or --archive <path> for a local tarball with no network at all
#   3. download the selected archive and refuse it unless its sha256 equals
#      the digest the API reports for that asset
#   4. extract it and verify the manifest inside it -- `sha256sum --strict -c
#      SHA256SUMS` on Linux, `shasum -a 256 --strict -c SHA256SUMS` on macOS --
#      after checking every member name so nothing is written outside the
#      extraction directory
#   5. choose the bind address: an existing agent.env wins; otherwise
#      EBBTOP_ADDR, otherwise a prompt defaulting to this host's tailnet/LAN
#      address. It is passed to the packaged installer, whose own validation
#      is what decides whether that address is acceptable
#   6. run the packaged installer from the extracted directory
#   7. verify what landed and print the pairing facts: ws://<bind>:<port>/ws
#      and the token file path - never the token value
#
# Flags:
#   --version <tag>   pin a published release tag, exactly as tagged (v0.3.1)
#   --archive <path>  install from a local tarball: no API, no download
#   --yes, -y         take the default answer, no prompt (scripted installs)
#   --dry-run, -n     print the plan, change nothing
#   --platform <p>    force linux or macos (test fixtures only; otherwise the
#                     platform comes from uname)
#   --help, -h        this text
#
# Environment:
#   EBBTOP_ADDR   bind address for a fresh install, instead of prompting
#   EBBTOP_PORT   NOT used: the packaged service definition fixes the port and
#                 this script reports it, so a stray value can never misreport
#                 the listener
#   WZ_YES/WZ_DRY/WZ_NO_TTY  accepted from the wizard helpers' convention
#
# Portability: this file must run under macOS's stock Bash 3.2 as well as Linux
# Bash 5, so it avoids `mapfile`, associative arrays, `${var,,}`, and any
# `${array[@]}` that could be empty under `set -u` (an unbound-variable error
# in 3.2). deploy/bootstrap/test-install-portable.sh proves that against a
# real Bash 3.2 binary.
#
# Presentation follows deploy/wizard/common.sh (show the state, print the plan,
# confirm once, act, verify, report) but is inline: this file has to run
# standalone from a pipe, where there is no checkout to source common.sh from.
# When it is piped, stdin is the script itself, so every prompt reads /dev/tty.
set -euo pipefail

PROG=ebbtop-install.sh

REPO_SLUG='Ebbwater/ebbtop'
API_BASE="https://api.github.com/repos/$REPO_SLUG/releases"
MIN_GLIBC='2.39'
DEFAULT_PORT=45976

# Platform facts, filled in by detect_platform. This is the only place in the
# file that names a platform difference.
PLATFORM=''
ASSET=''
PKG_MEMBERS=''
SVC=''
ENVFILE=''
TOKENFILE=''
BINFILE=''
UNITFILE=''
SERVICE_KIND=''
SERVICE_KIND_LABEL=''
RUNTIME_DIR=''
PLATFORM_FORCE=''

WZ_YES="${WZ_YES:-0}"
WZ_DRY="${WZ_DRY:-0}"
WZ_NO_TTY="${WZ_NO_TTY:-0}"

# --- presentation (mirrors deploy/wizard/common.sh) -------------------------
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  WZ_B=$'\033[1m'; WZ_DIM=$'\033[2m'; WZ_GRN=$'\033[32m'
  WZ_YEL=$'\033[33m'; WZ_RED=$'\033[31m'; WZ_RST=$'\033[0m'
else
  WZ_B=''; WZ_DIM=''; WZ_GRN=''; WZ_YEL=''; WZ_RED=''; WZ_RST=''
fi
WZ_RULE='------------------------------------------------------------'

wz_banner() {
  printf '\n%s%s%s\n' "$WZ_B" "${1:-EbbTop}" "$WZ_RST"
  [[ -n "${2:-}" ]] && printf '%s%s%s\n' "$WZ_DIM" "$2" "$WZ_RST"
  printf '%s\n' "$WZ_RULE"
}
wz_step() { printf '\n%s== %s%s\n' "$WZ_B" "$*" "$WZ_RST"; }
wz_info() { printf '   %s\n' "$*"; }
wz_ok()   { printf '   %sok%s   %s\n' "$WZ_GRN" "$WZ_RST" "$*"; }
wz_warn() { printf '   %swarn%s %s\n' "$WZ_YEL" "$WZ_RST" "$*"; }
wz_err()  { printf '   %sfail%s %s\n' "$WZ_RED" "$WZ_RST" "$*" >&2; }
wz_die()  { wz_err "$*"; exit 1; }
wz_note() { printf '   %s%s%s\n' "$WZ_DIM" "$*" "$WZ_RST"; }
wz_kv()   { printf '   %-10s %s\n' "$1" "$2"; }

# Read one answer. A piped `curl | bash` has the script on stdin, so the
# terminal is the only safe source apart from an explicit WZ_NO_TTY=1, which
# the test harness uses to drive answers from stdin.
wz__input() {
  REPLY=''
  if ((WZ_NO_TTY)); then
    IFS= read -r REPLY || REPLY=''
    return 0
  fi
  { IFS= read -r REPLY < /dev/tty; } 2>/dev/null || { REPLY=''; return 1; }
  return 0
}

wz__can_prompt() {
  ((WZ_NO_TTY)) && return 0
  { : < /dev/tty; } 2>/dev/null
}

# wz_confirm <prompt> [y|n] -> 0 yes, 1 no, 2 no way to ask.
wz_confirm() {
  local prompt="$1" def="${2:-n}" ans hint
  if ((WZ_YES)); then
    printf '   %s [yes]\n' "$prompt"
    return 0
  fi
  if ((WZ_DRY)); then
    printf '   %s [%s] (dry)\n' "$prompt" "$def"
    [[ "$def" == y ]]
    return
  fi
  wz__can_prompt || return 2
  if [[ "$def" == y ]]; then hint='Y/n'; else hint='y/N'; fi
  printf '   %s [%s]: ' "$prompt" "$hint"
  wz__input || return 2
  ans="${REPLY:-$def}"
  case "$ans" in
    [Yy]*) return 0 ;;
    *) return 1 ;;
  esac
}

usage() {
  cat <<'USAGE'
ebbtop-install.sh: install the prebuilt EbbTop agent on this host.

  curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash

The package is selected from uname -s and uname -m before any download:

  Linux x86_64         ebbtop-agent-linux-x86_64.tar.gz    systemd user service
  macOS Apple Silicon  ebbtop-agent-macos-arm64.tar.gz     per-user LaunchAgent

  --version <tag>   pin a published release tag, exactly as tagged (v0.3.1)
  --archive <path>  install from a local tarball: no API, no download
  --yes, -y         take the default answer, no prompt (scripted installs)
  --dry-run, -n     print the plan, change nothing
  --help, -h        this text

Environment: EBBTOP_ADDR sets the bind address for a fresh install; EBBTOP_PORT
is not used, because the packaged service definition fixes the port and this
script reports the port it actually finds.

The install itself is the packaged install-agent.sh --prebuilt from the
release archive. No token is ever printed.
USAGE
}

# --- refusal helpers -------------------------------------------------------
refuse() { wz_err "$*"; exit 1; }

# --- versions --------------------------------------------------------------
# Prints this host's glibc version, or nothing when it is not glibc.
host_glibc() {
  local v=''
  v="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
  v="${v##* }"
  [[ "$v" =~ ^[0-9]+(\.[0-9]+)*$ ]] || v=''
  printf '%s' "$v"
}

# version_ge <have> <need>
version_ge() {
  [[ "$1" == "$2" ]] && return 0
  [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" == "$2" ]]
}

# A running systemd user manager publishes its private socket under
# XDG_RUNTIME_DIR. -e, not -S, so a fixture can stand in for the socket; on a
# real host that path is created by systemd --user (and its absence means
# `systemctl --user` cannot work, so the packaged installer would fail later).
user_manager_ok() {
  command -v systemctl >/dev/null 2>&1 || return 1
  [[ -n "$RUNTIME_DIR" && -d "$RUNTIME_DIR/systemd" ]] || return 1
  [[ -e "$RUNTIME_DIR/systemd/private" || -e "$RUNTIME_DIR/bus" ]]
}

# A per-user LaunchAgent needs a logged-in launchd user domain. On a Mac that
# is `launchctl print gui/<uid>`; it fails over a bare SSH session with no
# login, and saying so is the honest answer: this release installs into a
# logged-in session, not a pre-login context.
launchd_user_domain_ok() {
  command -v launchctl >/dev/null 2>&1 || return 1
  launchctl print "gui/$(id -u)" >/dev/null 2>&1
}

# --- platform ---------------------------------------------------------------
# `uname -m` in a Rosetta shell reports x86_64 on an Apple Silicon Mac, so the
# Mac path checks the translation flag before refusing: a Rosetta terminal is
# a mistake worth naming precisely, not a silent downgrade to a package this
# release does not build.
detect_platform() {
  local os arch translated=0
  if [[ -n "$PLATFORM_FORCE" ]]; then
    case "$PLATFORM_FORCE" in
      linux|macos) PLATFORM="$PLATFORM_FORCE" ;;
      *) refuse "--platform '$PLATFORM_FORCE' is not a platform: use linux or macos" ;;
    esac
  fi

  if [[ -z "$PLATFORM" ]]; then
    os="$(uname -s 2>/dev/null || echo unknown)"
    case "$os" in
      Linux)  PLATFORM=linux ;;
      Darwin) PLATFORM=macos ;;
      *)      refuse "unsupported operating system '$os': this installer supports Linux (x86_64) and macOS (Apple Silicon). On Windows, download EbbTop-Windows-x64.zip from the release page instead." ;;
    esac
  fi

  case "$PLATFORM" in
    linux)
      ASSET='ebbtop-agent-linux-x86_64.tar.gz'
      PKG_MEMBERS='BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh'
      SVC="$HOME/services/ebbtop-agent"
      ENVFILE="$SVC/agent.env"
      TOKENFILE="$SVC/ebbtop.token"
      BINFILE="$SVC/bin/ebbtop-agent"
      UNITFILE="$HOME/.config/systemd/user/ebbtop-agent.service"
      SERVICE_KIND=systemd
      SERVICE_KIND_LABEL=unit
      RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
      ;;
    macos)
      ASSET='ebbtop-agent-macos-arm64.tar.gz'
      PKG_MEMBERS='BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh'
      SVC="${EBBTOP_STATE_DIR:-$HOME/Library/Application Support/ebbtop-agent}"
      ENVFILE="$SVC/agent.env"
      TOKENFILE="$SVC/ebbtop.token"
      BINFILE="$SVC/bin/ebbtop-agent"
      UNITFILE="$HOME/Library/LaunchAgents/com.ebbwater.ebbtop-agent.plist"
      SERVICE_KIND=launchd
      SERVICE_KIND_LABEL=plist
      RUNTIME_DIR=''
      ;;
  esac

  arch="$(uname -m 2>/dev/null || echo unknown)"
  if [[ "$PLATFORM" == macos && "$arch" == x86_64 ]]; then
    translated="$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)"
    if [[ "$translated" == 1 ]]; then
      refuse "this shell is running under Rosetta (x86_64 on Apple Silicon): open a native Terminal (or run 'arch -arm64 bash') and re-run, so the arm64 agent is installed and its metrics are native"
    fi
  fi
}

# --- JSON, without jq ------------------------------------------------------
# The release JSON is walked byte by byte with string state and brace depth, so
# the answer does not depend on key order, on pretty printing, or on which
# asset comes first. Only the "assets" array is entered, and inside an asset
# object only a top-level string value is read.
#
# json_asset_field <asset-name> <field>   (JSON on stdin)
json_asset_field() {
  awk -v want="$1" -v field="$2" '
    { doc = doc $0 "\n" }
    END {
      n = length(doc)
      # 1. find the "assets" key at the top level and the "[" that follows it
      pos = 0; p = 1
      while ((q = index(substr(doc, p), "\"assets\"")) > 0) {
        cand = p + q - 1
        j = cand + 8
        while (j <= n && substr(doc, j, 1) ~ /[ \t\r\n]/) j++
        if (substr(doc, j, 1) == ":") {
          j++
          while (j <= n && substr(doc, j, 1) ~ /[ \t\r\n]/) j++
          if (substr(doc, j, 1) == "[") { pos = j; break }
        }
        p = cand + 8
      }
      if (!pos) exit 0
      # 2. walk that array, collecting each depth-1 object
      i = pos + 1; bdepth = 0; ins = 0; start = 0
      while (i <= n) {
        c = substr(doc, i, 1)
        if (ins) {
          if (c == "\\") { i += 2; continue }
          if (c == "\"") ins = 0
          i++; continue
        }
        if (c == "\"") { ins = 1; i++; continue }
        if (c == "{") { if (bdepth == 0) start = i; bdepth++; i++; continue }
        if (c == "}") {
          bdepth--
          if (bdepth == 0 && start) {
            obj = substr(doc, start, i - start + 1)
            if (json_str(obj, "name") == want) { print json_value(obj, field); exit }
            start = 0
          }
          i++; continue
        }
        if (c == "]" && bdepth == 0) break
        i++
      }
    }
    # the first string value of a top-level key in obj
    function json_str(obj, key,    re, s, out, i, c) {
      re = "\"" key "\"[ \t\r\n]*:[ \t\r\n]*\""
      if (!match(obj, re)) return ""
      s = substr(obj, RSTART + RLENGTH)
      out = ""
      for (i = 1; i <= length(s); i++) {
        c = substr(s, i, 1)
        if (c == "\\") { out = out c substr(s, i + 1, 1); i++; continue }
        if (c == "\"") break
        out = out c
      }
      return out
    }
    function json_value(obj, key,    re, s) {
      s = json_str(obj, key)
      if (s != "") return s
      re = "\"" key "\"[ \t\r\n]*:[ \t\r\n]*[0-9]+"
      if (match(obj, re)) {
        s = substr(obj, RSTART, RLENGTH)
        sub(/^.*:[ \t\r\n]*/, "", s)
        return s
      }
      return ""
    }
  '
}

# json_string <key>   (JSON on stdin) - one top-level string value anywhere
json_string() {
  awk -v key="$1" '
    { doc = doc $0 "\n" }
    END {
      re = "\"" key "\"[ \t\r\n]*:[ \t\r\n]*\""
      if (!match(doc, re)) exit 0
      s = substr(doc, RSTART + RLENGTH)
      out = ""
      for (i = 1; i <= length(s); i++) {
        c = substr(s, i, 1)
        if (c == "\\") { out = out c substr(s, i + 1, 1); i++; continue }
        if (c == "\"") break
        out = out c
      }
      print out
    }
  '
}

# --- platform-native tools --------------------------------------------------
# Linux ships sha256sum; macOS ships shasum. One helper per use so nothing
# below has to know which tool exists. The two produce identical digests, and
# both support --strict so a malformed manifest line is a failure rather than
# a skipped line.
sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# The manifest check, in the wording each platform's tool uses, and reported
# on the line the Linux harness has always asserted.
manifest_verify() {
  local dir="$1" tool n
  if command -v sha256sum >/dev/null 2>&1; then
    tool='sha256sum --strict -c SHA256SUMS'
    ( cd "$dir" && sha256sum --strict -c SHA256SUMS ) || return 1
  else
    tool='shasum -a 256 --strict -c SHA256SUMS'
    ( cd "$dir" && shasum -a 256 --strict -c SHA256SUMS ) || return 1
  fi
  n="$(grep -c . "$dir/SHA256SUMS" 2>/dev/null || echo 0)"
  printf '%s: %s files verified\n' "$tool" "$n"
}

file_size() {
  if stat -c %s "$1" >/dev/null 2>&1; then stat -c %s "$1"; else stat -f %z "$1"; fi
}

file_mode() {
  if stat -c %a "$1" >/dev/null 2>&1; then stat -c %a "$1"; else stat -f %Lp "$1"; fi
}

# --- state (read-only) ------------------------------------------------------
state_bind() {
  local b=''
  if [[ -f "$ENVFILE" ]]; then
    b="$(grep -m1 '^EBBTOP_BIND=' "$ENVFILE" 2>/dev/null | cut -d= -f2- || true)"
  fi
  printf '%s' "$b"
}

installed_version() {
  local v=''
  if [[ -x "$BINFILE" ]]; then
    v="$("$BINFILE" --version 2>/dev/null | awk 'NR == 1 { print $2 }' || true)"
  fi
  printf '%s' "$v"
}

show_state() {
  local v bind tok
  wz_step "state found on this host"
  v="$(installed_version)"
  if [[ -n "$v" ]]; then wz_kv agent "$v installed at $BINFILE"; else wz_kv agent "not installed"; fi
  if [[ -f "$UNITFILE" ]]; then wz_kv "$SERVICE_KIND_LABEL" "$UNITFILE"; else wz_kv "$SERVICE_KIND_LABEL" "not installed"; fi
  if [[ -f "$ENVFILE" ]]; then
    bind="$(state_bind)"
    if [[ -n "$bind" ]]; then
      wz_kv env "$ENVFILE (kept as-is; bind $bind)"
    else
      wz_kv env "$ENVFILE (kept as-is; no plain EBBTOP_BIND line)"
    fi
  else
    wz_kv env "absent (a fresh install writes it with the bind address)"
  fi
  if [[ -f "$TOKENFILE" ]]; then
    tok="$(file_mode "$TOKENFILE")"
    wz_kv token "$TOKENFILE (mode $tok; not read)"
  else
    wz_kv token "absent (the installer generates one)"
  fi
}

# --- preflight --------------------------------------------------------------
preflight_linux() {
  local arch glib
  arch="$(uname -m)"
  if [[ "$arch" != x86_64 ]]; then
    refuse "this package is x86_64 only: uname -m reports '$arch'"
  fi
  wz_ok "x86_64"

  glib="$(host_glibc)"
  if [[ -z "$glib" ]]; then
    refuse "this package needs glibc $MIN_GLIBC or newer: this host's C library is not glibc (musl or unknown)"
  fi
  if ! version_ge "$glib" "$MIN_GLIBC"; then
    refuse "this package needs glibc $MIN_GLIBC or newer: this host has glibc $glib (Ubuntu 24.04+, Debian trixie+, Fedora 40+)"
  fi
  wz_ok "glibc $glib (package floor $MIN_GLIBC)"

  if ! user_manager_ok; then
    refuse "no systemd user manager: $RUNTIME_DIR/systemd is missing, so 'systemctl --user' cannot run (enable a user session or run this on a host with systemd)"
  fi
  wz_ok "systemd user manager"
}

preflight_macos() {
  local arch ver
  arch="$(uname -m)"
  if [[ "$arch" != arm64 ]]; then
    refuse "this package is Apple Silicon only: uname -m reports '$arch' (Intel Macs are not supported in this release)"
  fi
  wz_ok "Apple Silicon (arm64)"

  ver="$(sw_vers -productVersion 2>/dev/null || echo unknown)"
  wz_ok "macOS $ver"

  if ! launchd_user_domain_ok; then
    refuse "no launchd user domain: 'launchctl print gui/$(id -u)' failed, so a per-user LaunchAgent cannot be loaded (log in at the console and re-run; this release does not install a pre-login launch daemon)"
  fi
  wz_ok "launchd user domain (gui/$(id -u))"
}

preflight() {
  wz_step "preflight"
  case "$PLATFORM" in
    linux) preflight_linux ;;
    macos) preflight_macos ;;
  esac
}

# --- release resolution -----------------------------------------------------
# Sets: TAG DOWNLOAD_URL EXPECT_SHA256 ASSET_SIZE ARCHIVE SOURCE_KIND
resolve_release() {
  local json url tag size digest
  if [[ -n "$ARCHIVE_PATH" ]]; then
    [[ -f "$ARCHIVE_PATH" ]] || refuse "--archive $ARCHIVE_PATH does not exist"
    [[ -r "$ARCHIVE_PATH" ]] || refuse "--archive $ARCHIVE_PATH is not readable"
    ARCHIVE="$ARCHIVE_PATH"
    SOURCE_KIND=archive
    TAG=''
    DOWNLOAD_URL=''
    EXPECT_SHA256=''
    ASSET_SIZE="$(file_size "$ARCHIVE_PATH" 2>/dev/null || echo 0)"
    return 0
  fi

  SOURCE_KIND=release
  if [[ -n "$PIN" ]]; then
    url="$API_BASE/tags/$PIN"
  else
    url="$API_BASE/latest"
  fi
  json="$(curl -fsSL --proto '=https' \
            -H 'Accept: application/vnd.github+json' \
            -H 'X-GitHub-Api-Version: 2022-11-28' \
            "$url")" \
    || refuse "cannot read $url: no network, or that release tag does not exist"
  [[ -n "$json" ]] || refuse "cannot read $url: the release API returned an empty response"

  tag="$(printf '%s\n' "$json" | json_string tag_name)"
  [[ -n "$tag" ]] || refuse "cannot read $url: the response has no tag_name"
  if [[ -n "$PIN" && "$tag" != "$PIN" ]]; then
    refuse "asked for release $PIN but the API answered with $tag; refusing"
  fi

  DOWNLOAD_URL="$(printf '%s\n' "$json" | json_asset_field "$ASSET" browser_download_url)"
  digest="$(printf '%s\n' "$json" | json_asset_field "$ASSET" digest)"
  size="$(printf '%s\n' "$json" | json_asset_field "$ASSET" size)"

  if [[ -z "$DOWNLOAD_URL" ]]; then
    if [[ "$PLATFORM" == macos ]]; then
      refuse "release $tag has no $ASSET asset: that release predates the Apple Silicon agent, so there is nothing to install on this Mac. Pin a release that includes it with --version <tag>, or build one from a checkout with deploy/macos/package-agent.sh"
    fi
    refuse "release $tag has no $ASSET asset: the Linux package is not attached to that release"
  fi
  case "$DOWNLOAD_URL" in
    https://github.com/"$REPO_SLUG"/releases/download/*) ;;
    *) refuse "release $tag points $ASSET at '$DOWNLOAD_URL', which is not a https://github.com/$REPO_SLUG/releases/download/ URL; refusing" ;;
  esac
  case "$digest" in
    sha256:*) EXPECT_SHA256="${digest#sha256:}" ;;
    *) refuse "release $tag reports no sha256 digest for $ASSET: nothing to verify the download against, refusing (use --archive with a tarball you verified)" ;;
  esac
  [[ "$EXPECT_SHA256" =~ ^[0-9a-f]{64}$ ]] \
    || refuse "release $tag reports a malformed digest for $ASSET ('$digest'); refusing"
  ASSET_SIZE="${size:-0}"
  TAG="$tag"
  ARCHIVE=''
}

human_size() {
  local n="${1:-0}"
  if [[ "$n" =~ ^[0-9]+$ ]] && (( n > 0 )); then
    awk -v b="$n" 'BEGIN { printf "%.1f MB", b / 1048576 }'
  else
    printf 'unknown size'
  fi
}

# --- bind address -----------------------------------------------------------
detect_bind_linux() {
  local b=''
  if command -v tailscale >/dev/null 2>&1; then
    b="$(tailscale ip -4 2>/dev/null | head -n 1 || true)"
  fi
  if [[ -z "$b" ]] && command -v ip >/dev/null 2>&1; then
    b="$(ip -4 route get 1.1.1.1 2>/dev/null |
      awk '{ for (i = 1; i <= NF; i++) if ($i == "src") { print $(i + 1); exit } }' || true)"
  fi
  if [[ -z "$b" ]] && command -v ip >/dev/null 2>&1; then
    b="$(ip -4 -o addr show scope global 2>/dev/null |
      awk '{ print $4 }' | cut -d/ -f1 | grep -v '^127\.' | head -n 1 || true)"
  fi
  printf '%s' "$b"
}

# macOS has no `ip`(8). The tailnet address comes from the tailscale CLI and
# the LAN address from ipconfig: en0 is Wi-Fi on every current Apple Silicon
# laptop and en1+ are adapters/docks. No candidate is invented -- an empty
# answer leaves it to the prompt or the refusal to say what to set.
detect_bind_macos() {
  local b='' i
  if command -v tailscale >/dev/null 2>&1; then
    b="$(tailscale ip -4 2>/dev/null | head -n 1 || true)"
  fi
  if [[ -z "$b" ]] && command -v ipconfig >/dev/null 2>&1; then
    for i in en0 en1 en2; do
      b="$(ipconfig getifaddr "$i" 2>/dev/null || true)"
      [[ -n "$b" ]] && break
    done
  fi
  printf '%s' "$b"
}

detect_bind() {
  case "$PLATFORM" in
    linux) detect_bind_linux ;;
    macos) detect_bind_macos ;;
  esac
}

# Only a fresh install needs an address; an existing agent.env is what the
# packaged installer reads, so the prompt would be a lie on an upgrade. The
# bind shown here is only for display: the packaged installer's own check is
# what decides whether that file is usable, and its refusal is the one the
# operator needs to read.
choose_bind() {
  local answer hint
  wz_step "bind address"
  if [[ "$PLATFORM" == macos ]]; then
    hint="this Mac's Tailscale/LAN IP, or 127.0.0.1 for a local-only install"
  else
    hint="this host's tailnet/LAN IP"
  fi
  if [[ -f "$ENVFILE" ]]; then
    BIND="$(state_bind)"
    if [[ -n "${EBBTOP_ADDR:-}" && "${EBBTOP_ADDR}" != "$BIND" ]]; then
      wz_warn "EBBTOP_ADDR=$EBBTOP_ADDR ignored: an existing agent.env is kept as-is (it carries bind ${BIND:-unknown})"
    fi
    if [[ -z "$BIND" ]]; then
      wz_warn "the existing agent.env has no plain EBBTOP_BIND line; the packaged installer decides whether it is usable and refuses it if not"
    else
      wz_info "existing agent.env kept as-is: the agent keeps binding $BIND"
    fi
    FRESH=0
    return 0
  fi

  FRESH=1
  if [[ -n "${EBBTOP_ADDR:-}" ]]; then
    BIND="$EBBTOP_ADDR"
    return 0
  fi
  answer="$(detect_bind)"
  if ((WZ_YES || WZ_DRY)); then
    [[ -n "$answer" ]] || refuse "no tailnet/LAN address detected: set EBBTOP_ADDR=<${hint}> and re-run"
    BIND="$answer"
    return 0
  fi
  wz__can_prompt \
    || refuse "no terminal to prompt on: re-run with --yes, or set EBBTOP_ADDR=<${hint}>${answer:+ (detected $answer)}"
  printf '   %s [%s]: ' "address the agent binds ($hint)" "${answer:-none detected}"
  wz__input || refuse "no answer for the bind address: set EBBTOP_ADDR=<${hint}> and re-run"
  BIND="${REPLY:-$answer}"
  [[ -n "$BIND" ]] || refuse "the bind address is empty: set EBBTOP_ADDR=<${hint}> and re-run"
}

# --- plan / confirm ---------------------------------------------------------
print_plan() {
  wz_step "plan"
  if [[ "$SOURCE_KIND" == archive ]]; then
    wz_kv release "local archive (offline: no API, no download)"
    wz_kv asset "$ARCHIVE"
    wz_kv sha256 "$(sha256_of "$ARCHIVE" | cut -d' ' -f1)"
    wz_note "no publisher digest is available offline; the manifest inside the archive is verified"
  else
    wz_kv release "$TAG${PIN:+ (pinned)}"
    wz_kv asset "$ASSET ($(human_size "$ASSET_SIZE"))"
    wz_kv digest "sha256:$EXPECT_SHA256"
    wz_note "the download is refused unless its sha256 equals that digest"
  fi
  if [[ "$FRESH" == 1 ]]; then
    wz_kv bind "$BIND (fresh install)"
  else
    wz_kv bind "${BIND:-unknown} (kept from the existing agent.env; the packaged installer validates it)"
  fi
  if [[ "$PLATFORM" == macos ]]; then
    wz_kv port "$DEFAULT_PORT (the packaged LaunchAgent fixes the port; re-read before pairing)"
    wz_kv install "$SVC/bin/ebbtop-agent"
    wz_kv plist "$UNITFILE"
    wz_kv action "load the per-user LaunchAgent, then verify it with an authenticated probe"
  else
    wz_kv port "$DEFAULT_PORT (the packaged unit fixes the port; re-read before pairing)"
    wz_kv install "$SVC/bin/ebbtop-agent"
    wz_kv unit "$UNITFILE"
    wz_kv action "enable and start ebbtop-agent.service, then verify it"
  fi
  wz_note "the install itself is the packaged install-agent.sh --prebuilt from the archive"
}

# --- act --------------------------------------------------------------------
# Member names are checked before extraction: a traversing or unexpected member
# must never be written, and the packaged installer's own member-set check runs
# after extraction where it is already too late for a path escape.
#
# Written without arrays: macOS's Bash 3.2 makes an empty `${a[@]}` under
# `set -u` an unbound-variable error, and this line has to run there.
check_members() {
  local archive="$1" listing member name expected found found_list expected_list
  listing="$(tar -tzf "$archive" 2>/dev/null || true)"
  [[ -n "$listing" ]] || refuse "cannot list the archive $archive: it is not a readable gzip tarball"
  found_list=''
  while IFS= read -r member; do
    [[ -z "$member" ]] && continue
    case "$member" in
      */../*|../*|/*|*/) refuse "archive member '$member' is a path, not a bare packaged file name: refusing before extraction" ;;
    esac
    name="${member#./}"
    found=0
    for expected in $PKG_MEMBERS; do
      [[ "$name" == "$expected" ]] && found=1
    done
    (( found )) || refuse "archive carries an unexpected member '$member': expected exactly $PKG_MEMBERS"
    found_list="${found_list}${name}
"
  done <<MEMBERS
$listing
MEMBERS
  found_list="$(printf '%s' "$found_list" | LC_ALL=C sort)"
  expected_list="$(printf '%s\n' $PKG_MEMBERS | LC_ALL=C sort)"
  [[ "$found_list" == "$expected_list" ]] \
    || refuse "archive member set mismatch: found $(printf '%s' "$found_list" | tr '\n' ' '); expected $PKG_MEMBERS"
}

fetch_and_verify() {
  local got
  wz_step "fetch and verify"
  if [[ "$SOURCE_KIND" == archive ]]; then
    ARCHIVE="$ARCHIVE_PATH"
    wz_ok "local archive $ARCHIVE"
  else
    ARCHIVE="$WORK/$ASSET"
    wz_info "GET $DOWNLOAD_URL"
    curl -fSL --proto '=https' -o "$ARCHIVE" "$DOWNLOAD_URL" \
      || refuse "download failed: $DOWNLOAD_URL"
    got="$(sha256_of "$ARCHIVE")"
    if [[ "$got" != "$EXPECT_SHA256" ]]; then
      refuse "sha256 mismatch for $ASSET: the release reports $EXPECT_SHA256, the downloaded bytes are $got - refusing, nothing was installed and no unit or token was written"
    fi
    wz_ok "sha256 matches the release digest ($EXPECT_SHA256)"
  fi

  check_members "$ARCHIVE"
  mkdir -p "$PKG"
  tar -xzf "$ARCHIVE" -C "$PKG" || refuse "cannot extract $ARCHIVE"
  if ! manifest_verify "$PKG" > "$WORK/manifest.log" 2>&1; then
    local detail
    detail="$(grep -m1 -E 'FAILED|No such|WARNING|FAILED open' "$WORK/manifest.log" | tr -s ' ' || true)"
    refuse "the archive's own SHA256SUMS did not verify: ${detail:-the checksum tool reported no detail}"
  fi
  grep -q 'files verified' "$WORK/manifest.log" \
    && wz_ok "$(tail -n 1 "$WORK/manifest.log")"

  # The packaged BUILD states the real ABI floor; the earlier preflight used
  # the floor this script ships with. A package that needs more than that is
  # refused here, before the packaged installer touches anything. macOS has no
  # glibc, so its BUILD carries no glibc_floor and only version/target are
  # reported.
  local floor host
  floor="$(sed -n 's/^glibc_floor=//p' "$PKG/BUILD" | head -n 1)"
  if [[ -n "$floor" ]]; then
    host="$(host_glibc)"
    if ! version_ge "$host" "$floor"; then
      refuse "the package needs glibc $floor (BUILD glibc_floor) but this host has glibc ${host:-unknown}"
    fi
    wz_ok "package BUILD: glibc_floor=$floor, version=$(sed -n 's/^version=//p' "$PKG/BUILD" | head -n 1)"
  else
    wz_ok "package BUILD: version=$(sed -n 's/^version=//p' "$PKG/BUILD" | head -n 1), target=$(sed -n 's/^target=//p' "$PKG/BUILD" | head -n 1)"
  fi
}

run_packaged_installer() {
  local rc=0
  wz_step "install"
  if [[ "$FRESH" == 1 ]]; then
    wz_info "EBBTOP_ADDR=$BIND bash install-agent.sh --prebuilt"
  else
    wz_info "bash install-agent.sh --prebuilt (existing agent.env kept)"
  fi
  # EBBTOP_PORT is dropped so the packaged installer's own port checks and its
  # "listen:" line describe the port the service starts, not a stray env value.
  # The fresh-install address is passed the same way on both platforms; the
  # brands differ only in how the variable reaches the child.
  if [[ "$FRESH" == 1 ]]; then
    ( cd "$PKG" && env -u EBBTOP_PORT -u EBBTOP_ADDR "EBBTOP_ADDR=$BIND" \
        bash ./install-agent.sh --prebuilt ) 2>&1 | tee "$WORK/install.log" || rc=$?
  else
    ( cd "$PKG" && env -u EBBTOP_PORT -u EBBTOP_ADDR \
        bash ./install-agent.sh --prebuilt ) 2>&1 | tee "$WORK/install.log" || rc=$?
  fi
  (( rc == 0 )) || { wz_err "the packaged installer exited $rc; its output is above (it restores the previous binary when a new one does not start)"; exit "$rc"; }
}

# --- verify and report ------------------------------------------------------
verify_and_report_linux() {
  local state listen linger ver commit floor pair_addr problems=0
  ver="$(sed -n 's/^version=//p' "$PKG/BUILD" | head -n 1)"
  commit="$(sed -n 's/^commit=//p' "$PKG/BUILD" | head -n 1)"
  floor="$(sed -n 's/^glibc_floor=//p' "$PKG/BUILD" | head -n 1)"

  if [[ -f "$BINFILE" ]] && cmp -s "$PKG/ebbtop-agent" "$BINFILE"; then
    wz_ok "installed binary is the verified package member ($(sha256_of "$BINFILE" | cut -c1-16))"
  else
    wz_err "installed binary at $BINFILE does not match the verified package member"
    problems=$((problems + 1))
  fi
  if [[ -f "$UNITFILE" ]] && cmp -s "$PKG/ebbtop-agent.service" "$UNITFILE"; then
    wz_ok "unit is the packaged unit ($UNITFILE)"
  else
    wz_err "unit at $UNITFILE does not match the packaged unit"
    problems=$((problems + 1))
  fi

  state="$(sed -n 's/^active: //p' "$WORK/install.log" | tail -n 1)"
  listen="$(sed -n 's/^listen: //p' "$WORK/install.log" | tail -n 1)"
  linger="$(sed -n 's/^linger: //p' "$WORK/install.log" | tail -n 1)"
  [[ -n "$state" ]] || state="unknown"
  if [[ "$state" == active ]]; then
    wz_ok "ebbtop-agent.service is $state"
  else
    wz_err "ebbtop-agent.service is $state"
    problems=$((problems + 1))
  fi
  if [[ -n "$listen" ]]; then wz_kv listen "$listen"; fi
  if [[ -n "$linger" ]]; then wz_kv linger "$linger"; fi
  wz_kv package "${TAG:-local archive} version $ver${commit:+, commit ${commit:0:8}}${floor:+, glibc floor $floor}"

  if [[ -f "$TOKENFILE" ]]; then
    wz_ok "token file present, mode $(file_mode "$TOKENFILE") (not read)"
  else
    wz_err "no token file at $TOKENFILE"
    problems=$((problems + 1))
  fi

  wz_step "pair it with the client"
  # The installer's own "listen:" line is the authority: it is what the kernel
  # reports for the unit's port. Fall back to the address this script handled
  # only when nothing is listening to quote.
  pair_addr="$(printf '%s' "$listen" | awk '{ print $1 }')"
  case "$pair_addr" in
    ''|none*) pair_addr='' ;;
  esac
  if [[ -z "$pair_addr" && -n "$BIND" ]]; then
    pair_addr="$BIND:$PORT"
  fi
  if [[ -n "$pair_addr" ]]; then
    wz_kv address "ws://$pair_addr/ws"
  else
    wz_kv address "the packaged installer reported no listener; check its output above"
  fi
  wz_kv token "$TOKENFILE"
  wz_note "the token value is never printed here: read it on this host with 'cat $TOKENFILE' and paste it into the client's source"
  wz_note "same network, no public exposure: the agent has no TLS listener, the token is the gate"

  if (( problems )); then
    wz_err "$problems verification check(s) failed; the install output above is the installer's own account"
    exit 1
  fi
}

# The macOS report. The packaged installer prints `probe: ok ...` for an
# authenticated exchange with the address in agent.env, `state:` for the
# launchd job and `listen:` for the bound socket; this wrapper checks those
# against the package and adds the pairing block. The pairing address comes
# from the bind on file, bracketed when it is an IPv6 literal.
verify_and_report_macos() {
  local ver commit minos probe_line listen problems=0 addr
  ver="$(sed -n 's/^version=//p' "$PKG/BUILD" | head -n 1)"
  commit="$(sed -n 's/^commit=//p' "$PKG/BUILD" | head -n 1)"
  minos="$(sed -n 's/^min_macos=//p' "$PKG/BUILD" | head -n 1)"

  if [[ -f "$BINFILE" ]] && cmp -s "$PKG/ebbtop-agent" "$BINFILE"; then
    wz_ok "installed binary is the verified package member ($(sha256_of "$BINFILE" | cut -c1-16))"
  else
    wz_err "installed binary at $BINFILE does not match the verified package member"
    problems=$((problems + 1))
  fi
  # The installed LaunchAgent is the package template RENDERED for this home,
  # so it is never byte-identical to the package member. What must hold is
  # that it is a valid plist, names the installed binary and the agent's env
  # file, and carries no unrendered placeholder: that is the same set of facts
  # the installer's own lint checks, verified here on what is actually on disk.
  if [[ ! -f "$UNITFILE" ]]; then
    wz_err "no LaunchAgent at $UNITFILE"
    problems=$((problems + 1))
  else
    local plist_ok=1 plist_problem=''
    if ! grep -q '<plist' "$UNITFILE" || ! grep -q '</plist>' "$UNITFILE"; then
      plist_ok=0; plist_problem='it is not a plist document'
    elif ! grep -qF "$BINFILE" "$UNITFILE"; then
      plist_ok=0; plist_problem="it does not name the installed binary $BINFILE"
    elif ! grep -qF "$ENVFILE" "$UNITFILE"; then
      plist_ok=0; plist_problem="it does not pass the agent env file $ENVFILE"
    elif grep -q '__[A-Z_]*__' "$UNITFILE"; then
      plist_ok=0; plist_problem='it still carries an unrendered placeholder'
    elif grep -Eq 'EBBTOP_TOKEN=|EBBTOP_BIND=' "$UNITFILE"; then
      plist_ok=0; plist_problem='it assigns a credential or a bind address'
    fi
    if (( plist_ok )); then
      wz_ok "LaunchAgent is the packaged template rendered for this home ($UNITFILE)"
    else
      wz_err "LaunchAgent at $UNITFILE is not a usable render of the packaged template: $plist_problem"
      problems=$((problems + 1))
    fi
  fi

  probe_line="$(sed -n 's/^probe: //p' "$WORK/install.log" | tail -n 1)"
  if [[ "$probe_line" == ok* ]]; then
    wz_ok "authenticated probe of the running agent: probe: $probe_line"
  else
    wz_err "no authenticated probe succeeded (probe: ${probe_line:-none reported})"
    problems=$((problems + 1))
  fi
  listen="$(sed -n 's/^listen: //p' "$WORK/install.log" | tail -n 1)"
  if [[ -n "$listen" ]]; then wz_kv listen "$listen"; fi
  wz_kv package "${TAG:-local archive} version $ver${commit:+, commit ${commit:0:8}}${minos:+, min macOS $minos}"

  if [[ -f "$TOKENFILE" ]]; then
    wz_ok "token file present, mode $(file_mode "$TOKENFILE") (not read)"
  else
    wz_err "no token file at $TOKENFILE"
    problems=$((problems + 1))
  fi

  wz_step "pair it with the client"
  addr="$(state_bind)"
  case "$addr" in
    '')  wz_kv address "the agent has no bind address on file; check the install output above" ;;
    \[*) wz_kv address "ws://$addr:$PORT/ws" ;;
    *:*) wz_kv address "ws://[$addr]:$PORT/ws" ;;
    *)   wz_kv address "ws://$addr:$PORT/ws" ;;
  esac
  wz_kv token "$TOKENFILE"
  wz_note "the token value is never printed here: read it on this Mac with 'cat \"$TOKENFILE\"' and paste it into the client's source"
  wz_note "tailnet or LAN only: the agent binds one private address and has no TLS listener, the token is the gate"
  wz_note "this LaunchAgent starts at login and after a reboot once you log in; it is not a pre-login service"

  if (( problems )); then
    wz_err "$problems verification check(s) failed; the install output above is the installer's own account"
    exit 1
  fi
}

verify_and_report() {
  wz_step "verify"
  case "$PLATFORM" in
    linux) verify_and_report_linux ;;
    macos) verify_and_report_macos ;;
  esac
}

# --- main -------------------------------------------------------------------
ARCHIVE_PATH=''
PIN=''
BIND=''
FRESH=0
TAG=''
DOWNLOAD_URL=''
EXPECT_SHA256=''
ASSET_SIZE=0
ARCHIVE=''
SOURCE_KIND=''
PORT="$DEFAULT_PORT"

while (( $# )); do
  case "$1" in
    --version)
      [[ $# -ge 2 && -n "$2" ]] || { echo "$PROG: --version needs a release tag, for example v0.3.1" >&2; exit 2; }
      PIN="$2"; shift 2 ;;
    --version=*)
      PIN="${1#*=}"; shift ;;
    --archive)
      [[ $# -ge 2 && -n "$2" ]] || { echo "$PROG: --archive needs a path to a local tarball" >&2; exit 2; }
      ARCHIVE_PATH="$2"; shift 2 ;;
    --archive=*)
      ARCHIVE_PATH="${1#*=}"; shift ;;
    --platform)
      [[ $# -ge 2 && -n "$2" ]] || { echo "$PROG: --platform needs linux or macos" >&2; exit 2; }
      PLATFORM_FORCE="$2"; shift 2 ;;
    --platform=*)
      PLATFORM_FORCE="${1#*=}"; shift ;;
    --yes|-y) WZ_YES=1; shift ;;
    --dry-run|-n) WZ_DRY=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "$PROG: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ -z "$PIN" || "$PIN" =~ ^[A-Za-z0-9._-]+$ ]] \
  || { echo "$PROG: --version '$PIN' is not a plain release tag" >&2; exit 2; }
[[ -z "$ARCHIVE_PATH" || -z "$PIN" ]] \
  || { echo "$PROG: --archive installs a local tarball and --version pins a release on the network; use one" >&2; exit 2; }

detect_platform

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-install.XXXXXXXX")"
PKG="$WORK/pkg"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

if [[ "$PLATFORM" == macos ]]; then
  wz_banner "EbbTop agent installer" "prebuilt macOS agent (Apple Silicon, per-user LaunchAgent)"
else
  wz_banner "EbbTop agent installer" "prebuilt Linux agent (x86_64, systemd user service)"
fi

show_state
preflight
resolve_release
choose_bind
print_plan

confirm_rc=0
if [[ "$PLATFORM" == macos ]]; then
  wz_confirm "install the agent as a LaunchAgent in your login session?" y || confirm_rc=$?
else
  wz_confirm "install the agent as a systemd user service on this host?" y || confirm_rc=$?
fi
case "$confirm_rc" in
  0) ;;
  1) wz_info "nothing was changed"; exit 0 ;;
  *) refuse "no terminal to confirm on: re-run with --yes to install without prompting" ;;
esac

if ((WZ_DRY)); then
  wz_step "dry run"
  wz_info "the plan above was printed and nothing was downloaded, written or started"
  exit 0
fi

fetch_and_verify
if [[ "$PLATFORM" == macos ]]; then
  # The packaged port lives in the plist's --port argument, between the
  # <string> tags; read it so the report quotes the port the job starts.
  PORT="$(sed -n 's/^[[:space:]]*<string>\([0-9][0-9]*\)<\/string>$/\1/p' "$PKG/ebbtop-agent.plist" | tail -n 1)"
  PORT="${PORT:-$DEFAULT_PORT}"
else
  PORT="$(sed -n 's/^ExecStart=.*--port \([0-9][0-9]*\)$/\1/p' "$PKG/ebbtop-agent.service" | head -n 1)"
  PORT="${PORT:-$DEFAULT_PORT}"
fi

run_packaged_installer
verify_and_report
