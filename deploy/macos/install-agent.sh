#!/usr/bin/env bash
# Install, upgrade, roll back or uninstall the EbbTop agent on macOS as a
# per-user launchd LaunchAgent. Idempotent. Keeps an existing token and bind
# address. Prints no secret.
#
#   ./install-agent.sh --prebuilt            install/upgrade from this package
#   ./install-agent.sh --rollback            restore the previous binary
#   ./install-agent.sh --uninstall           stop and remove it; keeps state
#   ./install-agent.sh --uninstall --purge   also delete token and pairing state
#   ./install-agent.sh --status              what is installed, and readiness
#
# This script only ever runs from an extracted
# ebbtop-agent-macos-arm64.tar.gz: the binary, plist template and manifest
# are read from this script's own directory. It requires no root: the agent
# is a per-user LaunchAgent in the invoking user's own launchd session.
#
# macOS ships Bash 3.2 (a 2007 build, /bin/bash). This file therefore uses
# only 3.2 syntax: no associative arrays, no `mapfile`, no `${var,,}`, no
# `readarray`, no `local -n`, no `**` globs. deploy/macos/test-install-macos.sh
# runs this script under a real Bash 3.2 to prove that.
#
# Required env (fresh install only):
#   EBBTOP_ADDR   address the agent binds: 127.0.0.1, a private/LAN IPv4
#                 address, or the Mac's Tailscale 100.64/10 address
# Optional env:
#   EBBTOP_PORT         port (default 45976)
#   EBBTOP_STATE_DIR    state directory (default
#                       ~/Library/Application Support/ebbtop-agent)
#   EBBTOP_ALLOW_UNSUPPORTED_HOST=1  run on a host that is not Darwin/arm64
#                       (used by the Linux test harness with a stubbed
#                       launchctl; never needed on a real Mac)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SVC="${EBBTOP_STATE_DIR:-$HOME/Library/Application Support/ebbtop-agent}"
LABEL="com.ebbwater.ebbtop-agent"
LA_DIR="$HOME/Library/LaunchAgents"
PLIST="$LA_DIR/$LABEL.plist"
LOG_DIR="$HOME/Library/Logs/ebbtop-agent"
BIN="$SVC/bin/ebbtop-agent"
BAK="$SVC/bin/ebbtop-agent.bak"
ENVFILE="$SVC/agent.env"
TOKENFILE="$SVC/ebbtop.token"
PORT="${EBBTOP_PORT:-45976}"
ADDR="${EBBTOP_ADDR:-}"

# Filled in by install_prebuilt, read by rollback_previous. Kept as plain
# globals because the rollback runs from several failure points, some of them
# after a step that could have disturbed a `local`.
PREV_SHA=''          # sha256 of the binary that was installed before this run
ROLLBACK_ADDR=''     # the bind address to probe when verifying a rollback
PLIST_BAK="$LA_DIR/.$LABEL.plist.bak"

# Exactly the members the archive carries, and the ones SHA256SUMS covers.
PKG_MEMBERS="BUILD SHA256SUMS ebbtop-agent ebbtop-agent.plist install-agent.sh THIRD-PARTY-NOTICES"
PKG_SUMMED="BUILD ebbtop-agent ebbtop-agent.plist THIRD-PARTY-NOTICES"

usage() {
  cat >&2 <<'USAGE'
usage: install-agent.sh [--prebuilt | --rollback | --status | --uninstall [--purge]]

  --prebuilt           install or upgrade from the package in this script's
                       directory (ebbtop-agent, ebbtop-agent.plist, BUILD,
                       SHA256SUMS); needs no Rust toolchain and no root
  --rollback           restore the previous binary saved as ebbtop-agent.bak
  --status             report what is installed and whether an authenticated
                       probe of the running agent succeeds
  --uninstall          stop and unload the LaunchAgent, remove the plist and
                       the binary; KEEP the state directory (token, bind
                       address and pairing state)
  --purge              with --uninstall: also delete the state directory,
                       which removes the token and the pairing state;
                       enrolled clients lose access
USAGE
}

die() { echo "install-agent.sh: $*" >&2; exit 1; }
warn() { echo "install-agent.sh: $*" >&2; }

# --- platform ---------------------------------------------------------------
# Everything below assumes Darwin/arm64 launchd semantics. The harness sets
# EBBTOP_ALLOW_UNSUPPORTED_HOST=1 with stub launchctl/plutil/sw_vers on PATH,
# which is the only way this file runs off a Mac (and it is what proves the
# Bash 3.2 compatibility, the package checks and the lifecycle failures
# without a Mac).
require_platform() {
  if [[ "${EBBTOP_ALLOW_UNSUPPORTED_HOST:-0}" == 1 ]]; then
    return 0
  fi
  [[ "$(uname -s)" == Darwin ]] \
    || die "this package installs a macOS LaunchAgent: uname -s reports '$(uname -s)' (use deploy/install-agent.sh on Linux)"
  [[ "$(uname -m)" == arm64 ]] \
    || die "this package is Apple Silicon only: uname -m reports '$(uname -m)' (Intel Macs are not supported in this release)"
}

# --- bind address -----------------------------------------------------------
# A bind address must be an IP literal this agent is allowed to serve on. The
# agent itself enforces the same rule at startup (it fails closed); this
# check exists so the installer refuses *before* it writes a file, a plist or
# a token, with a message that says what to do.
#
# Accepted, and both must be chosen explicitly (this installer never guesses
# an address): loopback (127.0.0.1, ::1) for a local-only install, and a
# private/LAN, CGNAT/Tailscale or link-local address for a Mac the desktop
# client reaches over the tailnet or the LAN.
#
# Refused: the wildcard (0.0.0.0, ::) because it listens on every interface
# including whatever Wi-Fi the Mac joins next, and any public address, which
# would put a metrics listener on the open internet.
validate_bind() {
  local addr="$1" octet rest
  case "$addr" in
    "")
      die "bind address is empty" ;;
    0.0.0.0|::|::0|0:0:0:0:0:0:0:0|::ffff:0.0.0.0)
      die "refusing bind address '$addr': it listens on every interface, so the agent would answer on any network this Mac joins. Bind 127.0.0.1 (local only) or this Mac's Tailscale/LAN address." ;;
    127.0.0.1|::1|0:0:0:0:0:0:0:1)
      # Allowed, and recorded: only this Mac can reach the agent, so the
      # desktop client has to run on this Mac too. The install report says so.
      LOOPBACK_BIND=1
      return 0 ;;
  esac
  if [[ "$addr" == *:* ]]; then
    # IPv6 literal: only a private/unique-local or link-local range, and only
    # the plain hex-and-colon form (no zone id, no compression tricks the
    # agent would parse differently).
    [[ "$addr" =~ ^[0-9A-Fa-f:]+$ ]] || die "invalid bind address '$addr': not an IPv6 literal"
    case "$addr" in
      fd*|fc*|FD*|FC*|fe80*|FE80*) return 0 ;;
      *) die "refusing bind address '$addr': not a private (fc00::/7) or link-local (fe80::/10) IPv6 address" ;;
    esac
  fi
  [[ "$addr" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})$ ]] \
    || die "invalid bind address '$addr': not an IP literal"

  # Octet-wise range check without `set --` (which would clobber this
  # function's own arguments) and without `IFS=.` word splitting.
  rest="$addr"
  while [[ -n "$rest" ]]; do
    case "$rest" in
      *.*) octet="${rest%%.*}"; rest="${rest#*.}" ;;
      *)   octet="$rest"; rest="" ;;
    esac
    (( 10#$octet <= 255 )) || die "invalid bind address '$addr': octet $octet out of range"
  done

  # Private, CGNAT/Tailscale, or link-local only. A public address would put
  # a metrics listener on the open internet.
  case "$addr" in
    10.*|192.168.*|169.254.*) return 0 ;;
    172.*)
      octet="${addr#172.}"; octet="${octet%%.*}"
      (( 10#$octet >= 16 && 10#$octet <= 31 )) && return 0
      die "refusing bind address '$addr': 172.16.0.0-172.31.255.255 is private, $addr is not" ;;
    100.*)
      octet="${addr#100.}"; octet="${octet%%.*}"
      (( 10#$octet >= 64 && 10#$octet <= 127 )) && return 0
      die "refusing bind address '$addr': only 100.64.0.0/10 (Tailscale/CGNAT) is accepted in 100/8" ;;
    *)
      die "refusing bind address '$addr': not a private, CGNAT/Tailscale or link-local address. Bind 10/8, 172.16/12, 192.168/16, 100.64/10 or 127.0.0.1." ;;
  esac
}

# --- package verification ---------------------------------------------------
# The archive's own manifest is the authority, and it must cover exactly the
# payload by bare name so a verification can never be pointed outside the
# package. Member names are checked by the bootstrap before extraction; this
# check re-runs it after extraction, because that is what is about to be
# installed.
verify_package() {
  local dir="$1" member line name count=0 found
  for member in $PKG_MEMBERS; do
    [[ -f "$dir/$member" ]] || die "package is missing member '$member' in $dir"
    [[ -L "$dir/$member" ]] && die "package member '$member' is a symlink, not a regular file"
  done
  # No extras: anything else in the directory means a partial or repacked
  # archive, and it is refused before any write to this host.
  for member in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
    [[ -e "$member" || -L "$member" ]] || continue
    name="${member##*/}"
    found=0
    for line in $PKG_MEMBERS; do
      [[ "$name" == "$line" ]] && found=1
    done
    (( found )) || die "package carries an unexpected member '$name': expected exactly: $PKG_MEMBERS"
  done

  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" ]] && continue
    [[ "$line" =~ ^[0-9a-f]{64}[[:space:]][[:space:]]([A-Za-z0-9._-]+)$ ]] \
      || die "malformed SHA256SUMS line: '$line'"
    name="${BASH_REMATCH[1]}"
    case "$name" in
      BUILD|ebbtop-agent|ebbtop-agent.plist|THIRD-PARTY-NOTICES) ;;
      *) die "SHA256SUMS names an unexpected member: '$name'" ;;
    esac
    count=$((count + 1))
  done < "$dir/SHA256SUMS"
  # PKG_SUMMED is a plain string, not an array: this installer runs under
  # Bash 3.2 (macOS stock bash), so count it by word splitting.
  expected=0
  for m in $PKG_SUMMED; do expected=$((expected + 1)); done
  (( count == expected )) || die "SHA256SUMS must cover exactly $expected files ($PKG_SUMMED), found $count"
  for member in $PKG_SUMMED; do
    grep -q "  $member\$" "$dir/SHA256SUMS" || die "SHA256SUMS does not cover '$member'"
  done

  ( cd "$dir" && shasum -a 256 --strict --quiet -c SHA256SUMS ) \
    || die "package checksum verification failed in $dir (tampered or incomplete archive)"
  echo "verified: SHA256SUMS ok for $PKG_SUMMED"

  # The plist template must not carry a credential or an address, and must
  # use the placeholders this script renders. A template that already named a
  # token would put that token in a user-readable LaunchAgent. Test what
  # would actually leak — an assignment or a credential-shaped value in a
  # plist string — not the explanatory comments.
  grep -q '__AGENT_BIN__' "$dir/ebbtop-agent.plist" \
    || die "package plist does not carry the __AGENT_BIN__ placeholder"
  grep -q '__ENV_FILE__' "$dir/ebbtop-agent.plist" \
    || die "package plist does not carry the __ENV_FILE__ placeholder"
  if grep -Eq 'EBBTOP_TOKEN=|EBBTOP_BIND=|SOCKTOP_TOKEN=' "$dir/ebbtop-agent.plist"; then
    die "package plist assigns a credential or a bind address; refusing to install it"
  fi
  if grep -Eq '<string>[0-9a-fA-F]{32,}</string>' "$dir/ebbtop-agent.plist"; then
    die "package plist carries a credential-shaped value; refusing to install it"
  fi

  # A Mach-O arm64 binary, verified with the platform's own tools.
  [[ -x "$dir/ebbtop-agent" ]] || die "package binary is not executable"
  if command -v file >/dev/null 2>&1; then
    case "$(file -b "$dir/ebbtop-agent")" in
      *arm64*|*Mach-O*) ;;
      *) die "package binary is not a Mach-O executable: $(file -b "$dir/ebbtop-agent")" ;;
    esac
  fi
}

# --- state (read-only) ------------------------------------------------------
env_bind() {
  local b=""
  if [[ -f "$ENVFILE" ]]; then
    b="$(grep -m1 '^EBBTOP_BIND=' "$ENVFILE" 2>/dev/null | cut -d= -f2- || true)"
  fi
  printf '%s' "$b"
}

installed_version() {
  local v=""
  if [[ -x "$BIN" ]]; then
    v="$("$BIN" --version 2>/dev/null | awk 'NR == 1 { print $2 }' || true)"
  fi
  printf '%s' "$v"
}

file_mode() {
  stat -f %Lp "$1" 2>/dev/null || echo '?'
}

show_state() {
  local v b
  echo "state on this host:"
  v="$(installed_version)"
  if [[ -n "$v" ]]; then echo "  agent     $v at $BIN"; else echo "  agent     not installed"; fi
  if [[ -f "$PLIST" ]]; then echo "  plist     $PLIST"; else echo "  plist     not installed"; fi
  if [[ -f "$ENVFILE" ]]; then
    b="$(env_bind)"
    echo "  env       $ENVFILE (mode $(file_mode "$ENVFILE"); bind ${b:-unset})"
  else
    echo "  env       absent (a fresh install writes it)"
  fi
  if [[ -f "$TOKENFILE" ]]; then
    echo "  token     $TOKENFILE (mode $(file_mode "$TOKENFILE"); value never read)"
  else
    echo "  token     absent (the installer generates one)"
  fi
}

# --- readiness --------------------------------------------------------------
# The agent binds exactly ONE address — the one in agent.env — so the probe
# must target that address, not a convenient constant. Probing 127.0.0.1 on a
# tailnet-bound install would fail a healthy agent (worst case: a good upgrade
# rolled back), and probing the bind on a loopback install would fail too.
# `probe_target` therefore renders the configured bind, bracketing an IPv6
# literal so `host:port` parses (fd00::1 -> [fd00::1]:45976).
#
# `ebbtop-agent --probe` opens one authenticated WebSocket connection and
# prints `probe: ok ...`, `probe: denied` or `probe: failed: ...`. The token
# is passed by file, never on the command line, so it cannot appear in `ps`.
probe_target() {
  local addr="$1" port="$2"
  case "$addr" in
    \[*\]) printf '%s:%s' "$addr" "$port" ;;   # already bracketed
    *:*)   printf '[%s]:%s' "$addr" "$port" ;; # bare IPv6 literal
    *)     printf '%s:%s' "$addr" "$port" ;;
  esac
}

probe_agent() {
  local addr="${1:-}" out rc=0 target
  [[ -x "$BIN" ]] || { echo "probe: failed: no installed binary"; return 1; }
  [[ -n "$addr" ]] || { echo "probe: failed: no bind address to probe"; return 1; }
  target="$(probe_target "$addr" "$PORT")"
  out="$("$BIN" --probe "$target" --token-file "$TOKENFILE" --timeout-ms 5000 2>&1)" || rc=$?
  echo "$out"
  return "$rc"
}

# --- launchd ----------------------------------------------------------------
# launchctl's user domain. As a LaunchAgent, `launchctl bootstrap gui/$UID`
# is the modern equivalent of `load`; both are accepted by current macOS.
launch_domain() { printf 'gui/%s' "$(id -u)"; }

agent_loaded() {
  launchctl print "$(launch_domain)/$LABEL" >/dev/null 2>&1
}

unload_agent() {
  if agent_loaded; then
    launchctl bootout "$(launch_domain)/$LABEL" 2>/dev/null || true
    # bootout is asynchronous; wait for the job to actually go away so the
    # next bootstrap cannot race it.
    local i=0
    while agent_loaded && (( i < 20 )); do sleep 0.25; i=$((i + 1)); done
  fi
}

load_agent() {
  launchctl bootstrap "$(launch_domain)" "$PLIST" 2>&1 || {
    # A stale registration from a previous install can make bootstrap fail
    # with "already bootstrapped"; booting it out and retrying once is the
    # documented recovery.
    launchctl bootout "$(launch_domain)/$LABEL" 2>/dev/null || true
    sleep 1
    launchctl bootstrap "$(launch_domain)" "$PLIST" 2>&1
  }
}

# The plist is a template shipped in the package; render the three values the
# agent needs a path or a port for. Everything else (token, bind, tuning) is
# read by the agent from agent.env at startup.
render_plist() {
  local src="$1" dst="$2"
  sed -e "s|__AGENT_BIN__|$BIN|g" \
      -e "s|__ENV_FILE__|$ENVFILE|g" \
      -e "s|__PORT__|$PORT|g" \
      -e "s|__LOG_DIR__|$LOG_DIR|g" "$src" > "$dst"
  # A template value that still contains a placeholder means a new packaged
  # key this script does not render; refuse rather than install a job that
  # cannot start.
  if grep -Eq '__[A-Z_]+__' "$dst"; then
    die "rendered LaunchAgent still contains a placeholder: $(grep -Eo '__[A-Z_]+__' "$dst" | sort -u | tr '\n' ' ')"
  fi
  if command -v plutil >/dev/null 2>&1; then
    plutil -lint "$dst" >/dev/null 2>&1 || die "rendered LaunchAgent is not a valid plist: $dst"
  fi
}

# --- rollback ---------------------------------------------------------------
# One rollback path for every failure after the new binary and plist are in
# place: a launchd bootstrap that fails, a job that loads but never binds, or
# a probe that is refused. It restores BOTH the previous binary and the
# previous plist, because they are one unit: a new plist pointing at a
# restored binary is the same install, and a new plist left behind after a
# failed upgrade is a broken install waiting to be loaded at next login.
#
# `$1` says what failed, so the operator's log says which of the two distinct
# failures happened. Returns 0 when the previous install is serving again.
rollback_previous() {
  local why="$1" restored_sha state now
  if [[ ! -f "$BAK" ]]; then
    echo "no previous binary to roll back to; the new install is in place but not serving"
    echo "the LaunchAgent is left loaded so its log keeps the failure: $LOG_DIR/agent.log"
    return 1
  fi
  unload_agent
  mv -f "$BAK" "$BIN"
  if [[ -f "$PLIST_BAK" ]]; then
    mv -f "$PLIST_BAK" "$PLIST"
  else
    rm -f -- "$PLIST"
  fi
  restored_sha="$(shasum -a 256 "$BIN" | cut -d' ' -f1)"
  if [[ -n "$PREV_SHA" && "$restored_sha" != "$PREV_SHA" ]]; then
    echo "restored binary sha256 $restored_sha does not match the previous $PREV_SHA"
    return 1
  fi
  if [[ -f "$PLIST" ]]; then
    load_agent || { echo "the previous LaunchAgent could not be loaded again"; return 1; }
    sleep 2
  fi
  state="$(launchd_state)"
  if probe_agent "$ROLLBACK_ADDR" >/dev/null 2>&1; then
    echo "restored previous binary: ${restored_sha:0:16} ($BIN)"
    [[ -f "$PLIST" ]] && echo "restored previous LaunchAgent: $PLIST"
    echo "install failed ($why); the previous version is running again"
    return 0
  fi
  echo "restore could not be verified: the previous files are back but the agent is NOT serving at $(probe_target "$ROLLBACK_ADDR" "$PORT") (launchd state: ${state:-unknown})"
  return 1
}

launchd_state() {
  launchctl print "$(launch_domain)/$LABEL" 2>/dev/null \
    | awk -F' = ' '/^[[:space:]]*state = /{print $2; exit}' || true
}

# --- install / upgrade ------------------------------------------------------
install_prebuilt() {
  local dir="$SCRIPT_DIR" addr state probe_out probe_rc=0 rendered env_token file_token

  require_platform
  verify_package "$dir"

  # --- everything that can refuse runs before a single byte is written ------
  # An existing agent.env is never rewritten: bind address, token and any
  # settings the operator appended stay byte-identical across upgrades.
  if [[ -f "$ENVFILE" ]]; then
    addr="$(env_bind)"
    [[ -n "$addr" ]] || die "existing $ENVFILE has no EBBTOP_BIND line; add EBBTOP_BIND=<this Mac's tailnet/LAN address> and re-run"
    if [[ -n "$ADDR" && "$ADDR" != "$addr" ]]; then
      warn "EBBTOP_ADDR=$ADDR ignored: an existing $ENVFILE is kept as-is (it carries bind $addr)"
    fi
  else
    [[ -n "$ADDR" ]] || die "a fresh install needs EBBTOP_ADDR=this Mac's tailnet/LAN address (or 127.0.0.1 for a local-only install)"
    addr="$ADDR"
  fi
  validate_bind "$addr"

  # The credential pair must be intact BEFORE the current install is touched:
  # a missing or empty token file cannot be verified, so an upgrade that went
  # ahead would swap a working agent for one nobody can authenticate against.
  if [[ -f "$ENVFILE" ]]; then
    if [[ ! -f "$TOKENFILE" ]]; then
      die "existing $ENVFILE has no $TOKENFILE, so the token cannot be verified: refusing before any change to the current install (restore the token file, or move $ENVFILE aside for a fresh install)"
    fi
    [[ -s "$TOKENFILE" ]] \
      || die "existing $TOKENFILE is empty, so the token cannot be verified: refusing before any change to the current install"
    # The token in agent.env and the one in ebbtop.token must be the same
    # credential; a mismatch means the files disagree about what the agent
    # will accept, and the probe would fail after the swap.
    env_token="$(sed -n 's/^EBBTOP_TOKEN=//p' "$ENVFILE")"
    file_token="$(cat "$TOKENFILE")"
    if [[ -z "$file_token" || "$env_token" != "$file_token" ]]; then
      die "existing $ENVFILE and $TOKENFILE carry different tokens: refusing before any change to the current install (one of the two was edited by hand; fix it or move $ENVFILE aside)"
    fi
    unset env_token file_token
    # An agent.env written by an earlier build of THIS installer has no
    # EBBTOP_STATE_DIR line, so its agent falls back to its own default. On a
    # stock layout that default IS this directory, so the upgrade is safe; it is
    # only unsafe when the operator installed under a custom EBBTOP_STATE_DIR,
    # where the fallback would not match $SVC. Refuse before touching the
    # current install rather than leave pairing state outside the purge target.
    if ! grep -q '^EBBTOP_STATE_DIR=' "$ENVFILE"; then
      [[ "$SVC" == "$HOME/Library/Application Support/ebbtop-agent" ]] \
        || die "existing $ENVFILE predates the state-directory pin and $SVC is not the default location, so pairing state would land outside the directory --uninstall --purge deletes: add EBBTOP_STATE_DIR=$SVC to $ENVFILE and re-run"
    fi
  fi

  # A port already held by something that is not this agent would make the new
  # job fail after it replaced the old one; check first.
  #
  # This must identify the listener by PID and full executable path, NOT by the
  # COMMAND column: lsof truncates that column to 9 characters, so the agent
  # itself shows up as "ebbtop-age". A name-prefix comparison therefore refuses
  # every legitimate upgrade of a running agent with "port in use".
  if command -v lsof >/dev/null 2>&1; then
    local pids pid cmd holder=''
    pids="$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null || true)"
    for pid in $pids; do
      cmd="$(ps -p "$pid" -o comm= 2>/dev/null || true)"
      case "$cmd" in
        '' ) continue ;;
        "$BIN"|*/ebbtop-agent) continue ;;   # this agent already serving here
        * ) holder="${holder}${cmd}(pid ${pid}) " ;;
      esac
    done
    if [[ -n "$holder" ]]; then
      die "port $PORT is already used by: $holder(stop it, or set EBBTOP_PORT)"
    fi
  fi

  # The rendered LaunchAgent is validated in a staging file beside the live
  # one and only then moved into place, so a template this script cannot
  # render never replaces a working plist.
  mkdir -p "$SVC/bin" "$LA_DIR" "$LOG_DIR"
  chmod 700 "$SVC" 2>/dev/null || true
  rendered="$LA_DIR/.$LABEL.plist.new.$$"
  trap 'rm -f -- "$rendered"' EXIT
  render_plist "$dir/ebbtop-agent.plist" "$rendered"
  chmod 644 "$rendered"

  # --- now change the host -------------------------------------------------
  # Keep the previous binary and plist so a bad candidate can be undone.
  PREV_SHA=""
  if [[ -f "$BIN" ]]; then
    cp -p "$BIN" "$BAK"
    PREV_SHA="$(shasum -a 256 "$BIN" | cut -d' ' -f1)"
    if [[ -f "$PLIST" ]]; then
      cp -p "$PLIST" "$PLIST_BAK"
    fi
  fi
  install -m 755 "$dir/ebbtop-agent" "$BIN"

  # First install writes the credential file once, with a fresh token. It is
  # 0600 from the first byte (umask 077 covers the window before chmod) and
  # discovery is off: the Mac pilot has no broadcast responder.
  #
  # EBBTOP_STATE_DIR pins the agent's pairing state to THIS directory, the one
  # `--uninstall --purge` deletes. Without the pin the agent falls back to its
  # own default, which is right for the stock layout but would drift out of the
  # purge target whenever the operator installed with a custom EBBTOP_STATE_DIR;
  # `--purge` would then report success having deleted the token and left live
  # read-only client tokens behind.
  if [[ ! -f "$ENVFILE" ]]; then
    ( umask 077
      printf 'EBBTOP_BIND=%s\nEBBTOP_AGENT_DISCOVERY=0\nEBBTOP_AGENT_METRICS_TTL_MS=100\nEBBTOP_AGENT_PROCESSES_TTL_MS=500\nEBBTOP_STATE_DIR=%s\nEBBTOP_TOKEN=%s\n' \
        "$addr" "$SVC" "$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')" > "$ENVFILE" )
    awk -F= '$1 == "EBBTOP_TOKEN" { print $2 }' "$ENVFILE" > "$TOKENFILE"
    chmod 600 "$ENVFILE" "$TOKENFILE"
    echo "new token written to $TOKENFILE (mode 600; value never printed)"
  else
    chmod 600 "$ENVFILE" 2>/dev/null || true
    chmod 600 "$TOKENFILE" 2>/dev/null || true
  fi

  mv -f "$rendered" "$PLIST"
  trap - EXIT

  # Both the configured bind and the address of the previous install are
  # legitimate probe targets during an upgrade: agent.env is never rewritten,
  # so they are the same address. ROLLBACK_ADDR records it for the rollback
  # path, which runs after a failure and must not depend on variables that a
  # failed step could have disturbed.
  ROLLBACK_ADDR="$addr"

  unload_agent
  # An activation failure is a distinct failure from a probe failure: launchd
  # can refuse the job outright (a malformed job, a domain that went away, a
  # stale registration) before the agent ever binds. Both go through the same
  # rollback, and both are named in the log.
  if ! load_agent; then
    echo "launchd refused to load the LaunchAgent ($(launch_domain)/$LABEL)"
    if rollback_previous "launchd bootstrap failed"; then exit 1; fi
    exit 1
  fi
  # launchd reports a job as loaded as soon as bootstrap succeeds; the agent
  # then needs a moment to bind. The authenticated probe below is the real
  # readiness check.
  sleep 2

  state="$(launchd_state)"
  probe_out="$(probe_agent "$addr" 2>&1)" || probe_rc=$?

  if [[ "$probe_rc" -ne 0 ]]; then
    echo "ebbtop-agent did not become ready after install (launchd state: ${state:-unknown}) at $(probe_target "$addr" "$PORT")"
    echo "$probe_out"
    if rollback_previous "readiness probe failed"; then exit 1; fi
    exit 1
  fi

  echo "version: $(sed -n 's/^version=//p' "$dir/BUILD")"
  echo "state: ${state:-running}"
  # probe_out is already the CLI's own line (`probe: ok frames=1`); printing
  # another prefix would produce `probe: probe: ok`, which no caller parses.
  echo "$probe_out"
  echo "binary: $(shasum -a 256 "$BIN" | cut -c1-16)"
  echo "listen: $(host_port)"
  echo "plist: $PLIST"
  echo "log: $LOG_DIR/agent.log"
  echo "starts at login: yes (LaunchAgent RunAtLoad; no sudo, no launch daemon)"
  if [[ "${LOOPBACK_BIND:-0}" == 1 ]]; then
    echo "reachable from: this Mac only (bound to loopback; run the desktop client here, or reinstall with this Mac's tailnet/LAN address)"
  fi
  echo "pair: mint a pairing code with: $BIN --pair"
}

# Address the agent is listening on, for the report. `lsof` when present, else
# a statement that the port cannot be shown (the authenticated probe already
# proved the listener works).
host_port() {
  command -v lsof >/dev/null 2>&1 || { printf '%s' "not shown (lsof is unavailable; the probe above proved the listener)"; return 0; }
  local out
  out="$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN 2>/dev/null | awk 'NR > 1 { print $9 }' | sort -u | tr '\n' ' ')"
  printf '%s' "${out:-none (nothing listening on port $PORT)}"
}

# `launchctl print` for this job, filtered to the executable path, so the
# report says which binary the running job is actually using.
loaded_program() {
  local plist="$1"
  echo "loaded: $(awk '/<string>.*ebbtop-agent<\/string>/{ getline; print "program " $0; exit }' "$plist" 2>/dev/null | sed 's/<[^>]*>//g' || true)"
}

# --- status -----------------------------------------------------------------
status_agent() {
  local addr="" st
  show_state
  addr="$(env_bind)"
  echo "launchd:"
  if agent_loaded; then
    echo "  loaded    yes ($(launch_domain)/$LABEL)"
    echo "  state     $(launchd_state)"
    st="$(loaded_program "$PLIST" 2>/dev/null | sed 's/^program //')"
    [[ -n "$st" ]] && echo "  program   $st"
  else
    echo "  loaded    no"
  fi
  echo "readiness:"
  if [[ -f "$TOKENFILE" && -x "$BIN" && -n "$addr" ]]; then
    echo "  target    $(probe_target "$addr" "$PORT") (the configured bind)"
    probe_agent "$addr" | sed 's/^/  /' || true
  elif [[ -f "$TOKENFILE" && -x "$BIN" ]]; then
    echo "  no bind address in $ENVFILE, so there is nothing to probe"
  else
    echo "  not installed"
  fi
}

# --- uninstall --------------------------------------------------------------
uninstall_agent() {
  local purge="$1" tok_present=0 pair_present=0

  [[ -f "$TOKENFILE" ]] && tok_present=1
  [[ -f "$SVC/pairing.json" || -f "$SVC/clients.json" ]] && pair_present=1

  if [[ ! -f "$PLIST" && ! -f "$BIN" && "$tok_present" -eq 0 ]]; then
    echo "nothing to remove on this host (no $PLIST, no $BIN, no token)"
    return 0
  fi

  echo "plist: $PLIST"
  echo "binary: $BIN"
  echo "token: $tok_present file(s) present"
  echo "pairing: $pair_present state file(s) present"
  if (( purge )); then
    echo "plan: bootout the LaunchAgent, remove the plist and the binary, then delete $SVC (the token and pairing state go with it; enrolled clients lose access)"
  else
    echo "plan: bootout the LaunchAgent, remove the plist and the binary; keep $SVC (token, bind address and pairing state)"
  fi

  unload_agent
  rm -f -- "$PLIST" "$BIN" "$BAK"
  if (( purge )); then
    rm -rf -- "$SVC"
  fi

  if [[ -f "$PLIST" ]]; then echo "plist: still present at $PLIST"; else echo "plist: removed"; fi
  if [[ -f "$BIN" ]]; then echo "binary: still present at $BIN"; else echo "binary: removed"; fi
  if agent_loaded; then echo "launchd: still loaded"; else echo "launchd: not loaded"; fi
  if (( purge )); then
    if [[ -e "$SVC" ]]; then echo "state: $SVC still exists"; else echo "state: $SVC removed"; fi
    echo "token: deleted with the state directory"
  else
    if [[ -f "$TOKENFILE" ]]; then echo "token: kept ($TOKENFILE)"; else echo "token: none"; fi
    echo "state: kept at $SVC (--purge deletes it)"
  fi
}

# --- dispatch ---------------------------------------------------------------
MODE=""
PURGE=0
case "${1:-}" in
  "")          echo "install-agent.sh: this package script needs an action" >&2; usage; exit 2 ;;
  --prebuilt)  MODE=prebuilt ;;
  --rollback)  MODE=rollback ;;
  --status)    MODE=status ;;
  --uninstall) MODE=uninstall ;;
  --purge)     echo "--purge only makes sense with --uninstall" >&2; usage; exit 2 ;;
  *)           echo "unknown option: $1" >&2; usage; exit 2 ;;
esac
if (( $# > 1 )); then
  if [[ "$MODE" == uninstall && "$2" == --purge && $# -eq 2 ]]; then
    PURGE=1
  else
    echo "unknown argument: $2" >&2; usage; exit 2
  fi
fi

if [[ "$MODE" == rollback ]]; then
  require_platform
  [[ -f "$BAK" ]] || die "no .bak to roll back to"
  rb_addr="$(env_bind)"
  [[ -n "$rb_addr" ]] || die "cannot roll back without a bind address: $ENVFILE has no EBBTOP_BIND line"
  mv -f "$BAK" "$BIN"
  unload_agent
  load_agent
  sleep 2
  probe_agent "$rb_addr" || die "the previous binary is back on disk but did not become ready at $(probe_target "$rb_addr" "$PORT")"
  echo "rolled back"
  exit 0
fi

if [[ "$MODE" == status ]]; then
  require_platform
  status_agent
  exit 0
fi

if [[ "$MODE" == uninstall ]]; then
  require_platform
  uninstall_agent "$PURGE"
  exit 0
fi

install_prebuilt
