#!/usr/bin/env bash
# ebbtop-install.sh: one command installs the prebuilt EbbTop Linux agent.
#
#   curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash
#   curl -fsSL .../ebbtop-install.sh | bash -s -- --version v0.2.9
#
# This is a thin fetch-and-verify wrapper. It installs nothing itself: the
# install, the unit, the token, linger guidance and the self-verification all
# belong to the packaged deploy/install-agent.sh --prebuilt, which ships inside
# the release archive byte-identical to this repository. The wrapper only
# proves that the bytes it hands over are the bytes the release published.
#
# What it does, in this order:
#   1. preflight: x86_64, glibc >= 2.39 and a systemd user manager - one clear
#      refusal line each, before any network access
#   2. resolve the release: latest via the GitHub API, --version <tag> to pin,
#      or --archive <path> for a local tarball with no network at all
#   3. download ebbtop-agent-linux-x86_64.tar.gz and refuse it unless its
#      sha256 equals the digest the API reports for that asset
#   4. extract it and run `sha256sum --strict -c SHA256SUMS` on the manifest
#      inside it (member names are checked first so nothing is written outside
#      the extraction directory)
#   5. choose the bind address: an existing agent.env wins; otherwise
#      EBBTOP_ADDR, otherwise a prompt defaulting to this host's tailnet/LAN
#      address. It is passed to the packaged installer, whose own validation
#      refuses loopback and wildcard addresses
#   6. run the packaged installer from the extracted directory
#   7. verify what landed and print the pairing facts: ws://<bind>:<port>/ws
#      and the token file path - never the token value
#
# Flags:
#   --version <tag>   pin a published release tag, exactly as tagged (v0.2.9)
#   --archive <path>  install from a local tarball: no API, no download
#   --yes, -y         take the default answer, no prompt (scripted installs)
#   --dry-run, -n     print the plan, change nothing
#   --help, -h        this text
#
# Environment:
#   EBBTOP_ADDR   bind address for a fresh install, instead of prompting
#   EBBTOP_PORT   NOT used: the packaged unit fixes the port and this script
#                 reports it, so a stray value can never misreport the listener
#   WZ_YES/WZ_DRY/WZ_NO_TTY  accepted from the wizard helpers' convention
#
# Presentation follows deploy/wizard/common.sh (show the state, print the plan,
# confirm once, act, verify, report) but is inline: this file has to run
# standalone from a pipe, where there is no checkout to source common.sh from.
# When it is piped, stdin is the script itself, so every prompt reads /dev/tty.
set -euo pipefail

PROG=ebbtop-install.sh

REPO_SLUG='Ebbwater/ebbtop'
ASSET='ebbtop-agent-linux-x86_64.tar.gz'
API_BASE="https://api.github.com/repos/$REPO_SLUG/releases"
MIN_GLIBC='2.39'
DEFAULT_PORT=45976
PKG_MEMBERS=(BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh)

# The packaged installer's own layout. Nothing here is ever written by this
# wrapper; it is only read to show state and to verify the result.
SVC="$HOME/services/ebbtop-agent"
ENVFILE="$SVC/agent.env"
TOKENFILE="$SVC/ebbtop.token"
BINFILE="$SVC/bin/ebbtop-agent"
UNITFILE="$HOME/.config/systemd/user/ebbtop-agent.service"

RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

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
ebbtop-install.sh: install the prebuilt EbbTop Linux agent on this host.

  curl -fsSL https://github.com/Ebbwater/ebbtop/releases/latest/download/ebbtop-install.sh | bash

  --version <tag>   pin a published release tag, exactly as tagged (v0.2.9)
  --archive <path>  install from a local tarball: no API, no download
  --yes, -y         take the default answer, no prompt (scripted installs)
  --dry-run, -n     print the plan, change nothing
  --help, -h        this text

Environment: EBBTOP_ADDR sets the bind address for a fresh install; EBBTOP_PORT
is not used, because the packaged unit fixes the port and this script reports
the port it actually finds in the unit.

The install itself is the packaged deploy/install-agent.sh --prebuilt from the
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
  if [[ -f "$UNITFILE" ]]; then wz_kv unit "$UNITFILE"; else wz_kv unit "not installed"; fi
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
    tok="$(stat -c %a "$TOKENFILE" 2>/dev/null || echo '?')"
    wz_kv token "$TOKENFILE (mode $tok; not read)"
  else
    wz_kv token "absent (the installer generates one)"
  fi
}

# --- preflight --------------------------------------------------------------
preflight() {
  local arch glib
  wz_step "preflight"
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
    ASSET_SIZE="$(stat -c %s "$ARCHIVE_PATH" 2>/dev/null || echo 0)"
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

  [[ -n "$DOWNLOAD_URL" ]] \
    || refuse "release $tag has no $ASSET asset: the Linux package is not attached to that release"
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
detect_bind() {
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

# Only a fresh install needs an address; an existing agent.env is what the
# packaged installer reads, so the prompt would be a lie on an upgrade. The
# bind shown here is only for display: the packaged installer's own check is
# what decides whether that file is usable, and its refusal is the one the
# operator needs to read.
choose_bind() {
  local answer
  wz_step "bind address"
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
    [[ -n "$answer" ]] || refuse "no tailnet/LAN address detected: set EBBTOP_ADDR=<this host's tailnet/LAN IP> and re-run"
    BIND="$answer"
    return 0
  fi
  wz__can_prompt \
    || refuse "no terminal to prompt on: re-run with --yes, or set EBBTOP_ADDR=<this host's tailnet/LAN IP>${answer:+ (detected $answer)}"
  printf '   %s [%s]: ' "address the agent binds (this host's tailnet/LAN IP)" "${answer:-none detected}"
  wz__input || refuse "no answer for the bind address: set EBBTOP_ADDR=<this host's tailnet/LAN IP> and re-run"
  BIND="${REPLY:-$answer}"
  [[ -n "$BIND" ]] || refuse "the bind address is empty: set EBBTOP_ADDR=<this host's tailnet/LAN IP> and re-run"
}

# --- plan / confirm ---------------------------------------------------------
print_plan() {
  wz_step "plan"
  if [[ "$SOURCE_KIND" == archive ]]; then
    wz_kv release "local archive (offline: no API, no download)"
    wz_kv asset "$ARCHIVE"
    wz_kv sha256 "$(sha256sum "$ARCHIVE" | cut -d' ' -f1)"
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
  wz_kv port "$DEFAULT_PORT (the packaged unit fixes the port; re-read before pairing)"
  wz_kv install "$SVC/bin/ebbtop-agent"
  wz_kv unit "$UNITFILE"
  wz_kv action "enable and start ebbtop-agent.service, then verify it"
  wz_note "the install itself is the packaged deploy/install-agent.sh --prebuilt from the archive"
}

# --- act --------------------------------------------------------------------
# Member names are checked before extraction: a traversing or unexpected member
# must never be written, and the packaged installer's own member-set check runs
# after extraction where it is already too late for a path escape.
check_members() {
  local archive="$1" m name
  local -a members=() expected=()
  mapfile -t members < <(tar -tzf "$archive" 2>/dev/null || true)
  (( ${#members[@]} > 0 )) || refuse "cannot list the archive $archive: it is not a readable gzip tarball"
  for m in "${members[@]}"; do
    case "$m" in
      */../*|../*|/*|*/) refuse "archive member '$m' is a path, not a bare packaged file name: refusing before extraction" ;;
    esac
    name="${m#./}"
    case "$name" in
      BUILD|SHA256SUMS|ebbtop-agent|ebbtop-agent.service|install-agent.sh) ;;
      *) refuse "archive carries an unexpected member '$m': expected exactly ${PKG_MEMBERS[*]}" ;;
    esac
  done
  mapfile -t expected < <(printf '%s\n' "${PKG_MEMBERS[@]}" | LC_ALL=C sort)
  mapfile -t members < <(printf '%s\n' "${members[@]}" | sed 's|^\./||' | LC_ALL=C sort)
  [[ "${members[*]}" == "${expected[*]}" ]] \
    || refuse "archive member set mismatch: found ${members[*]:-none}; expected ${PKG_MEMBERS[*]}"
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
    got="$(sha256sum "$ARCHIVE" | cut -d' ' -f1)"
    if [[ "$got" != "$EXPECT_SHA256" ]]; then
      refuse "sha256 mismatch for $ASSET: the release reports $EXPECT_SHA256, the downloaded bytes are $got - refusing, nothing was installed and no unit or token was written"
    fi
    wz_ok "sha256 matches the release digest ($EXPECT_SHA256)"
  fi

  check_members "$ARCHIVE"
  mkdir -p "$PKG"
  tar -xzf "$ARCHIVE" -C "$PKG" || refuse "cannot extract $ARCHIVE"
  if ! ( cd "$PKG" && sha256sum --strict -c SHA256SUMS ) > "$WORK/manifest.log" 2>&1; then
    local detail
    detail="$(grep -m1 -E 'FAILED|No such|WARNING' "$WORK/manifest.log" | tr -s ' ' || true)"
    refuse "the archive's own SHA256SUMS did not verify: ${detail:-sha256sum reported no detail}"
  fi
  wz_ok "sha256sum --strict -c SHA256SUMS: $(grep -c . "$WORK/manifest.log") files verified"

  # The packaged BUILD states the real ABI floor; the earlier preflight used
  # the floor this script ships with. A package that needs more than that is
  # refused here, before the packaged installer touches anything.
  local floor host
  floor="$(sed -n 's/^glibc_floor=//p' "$PKG/BUILD" | head -n 1)"
  host="$(host_glibc)"
  if [[ -n "$floor" ]] && ! version_ge "$host" "$floor"; then
    refuse "the package needs glibc $floor (BUILD glibc_floor) but this host has glibc ${host:-unknown}"
  fi
  if [[ -n "$floor" ]]; then
    wz_ok "package BUILD: glibc_floor=$floor, version=$(sed -n 's/^version=//p' "$PKG/BUILD" | head -n 1)"
  fi
}

run_packaged_installer() {
  local rc=0
  local -a env_args=()
  # EBBTOP_PORT is dropped so the packaged installer's own port checks and its
  # "listen:" line describe the port the unit starts, not a stray env value.
  if [[ "$FRESH" == 1 ]]; then
    env_args=("EBBTOP_ADDR=$BIND")
  fi
  wz_step "install"
  if [[ "$FRESH" == 1 ]]; then
    wz_info "EBBTOP_ADDR=$BIND bash install-agent.sh --prebuilt"
  else
    wz_info "bash install-agent.sh --prebuilt (existing agent.env kept)"
  fi
  ( cd "$PKG" && env -u EBBTOP_PORT -u EBBTOP_ADDR "${env_args[@]}" \
      bash ./install-agent.sh --prebuilt ) 2>&1 | tee "$WORK/install.log" || rc=$?
  (( rc == 0 )) || { wz_err "the packaged installer exited $rc; its output is above (it restores the previous binary when a new one does not start)"; exit "$rc"; }
}

# --- verify and report ------------------------------------------------------
verify_and_report() {
  local state listen linger ver commit floor pair_addr problems=0
  wz_step "verify"

  ver="$(sed -n 's/^version=//p' "$PKG/BUILD" | head -n 1)"
  commit="$(sed -n 's/^commit=//p' "$PKG/BUILD" | head -n 1)"
  floor="$(sed -n 's/^glibc_floor=//p' "$PKG/BUILD" | head -n 1)"

  if [[ -f "$BINFILE" ]] && cmp -s "$PKG/ebbtop-agent" "$BINFILE"; then
    wz_ok "installed binary is the verified package member ($(sha256sum "$BINFILE" | cut -c1-16))"
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
    wz_ok "token file present, mode $(stat -c %a "$TOKENFILE") (not read)"
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
      [[ $# -ge 2 && -n "$2" ]] || { echo "$PROG: --version needs a release tag, for example v0.2.9" >&2; exit 2; }
      PIN="$2"; shift 2 ;;
    --version=*)
      PIN="${1#*=}"; shift ;;
    --archive)
      [[ $# -ge 2 && -n "$2" ]] || { echo "$PROG: --archive needs a path to a local tarball" >&2; exit 2; }
      ARCHIVE_PATH="$2"; shift 2 ;;
    --archive=*)
      ARCHIVE_PATH="${1#*=}"; shift ;;
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

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-install.XXXXXXXX")"
PKG="$WORK/pkg"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

wz_banner "EbbTop agent installer" "prebuilt Linux agent (x86_64, systemd user service)"

show_state
preflight
resolve_release
choose_bind
print_plan

confirm_rc=0
wz_confirm "install the agent as a systemd user service on this host?" y || confirm_rc=$?
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
PORT="$(sed -n 's/^ExecStart=.*--port \([0-9][0-9]*\)$/\1/p' "$PKG/ebbtop-agent.service" | head -n 1)"
PORT="${PORT:-$DEFAULT_PORT}"

run_packaged_installer
verify_and_report
