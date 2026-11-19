#!/usr/bin/env bash
# Install or update the ebbtop agent on the monitored host as a systemd
# user service. Idempotent. Keeps an existing token. Prints no secret.
#   ~/ebbtop/deploy/install-agent.sh            install/update + start
#   ~/ebbtop/deploy/install-agent.sh --rollback restore previous binary
#   ./install-agent.sh --prebuilt               install a packaged release
#                                               (no git, cargo or rustc needed)
#
# --prebuilt is only valid from an extracted ebbtop-agent-linux-x86_64.tar.gz:
# the binary, unit and manifest are read from this script's own directory.
#
# Required env (fresh install only):
#   EBBTOP_ADDR   address the agent binds (tailnet or LAN IP of this host)
# Optional env:
#   EBBTOP_PORT   port (default 45976)
set -euo pipefail
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$SCRIPT_DIR/.." && pwd)"
SVC="$HOME/services/ebbtop-agent"
OLD_SVC="$HOME/services/kmon-agent"
OLD_UNIT="$HOME/.config/systemd/user/kmon-agent.service"
UNIT="$HOME/.config/systemd/user/ebbtop-agent.service"
BIN="$REPO/target/release/ebbtop-agent"
PORT="${EBBTOP_PORT:-45976}"
# Only needed for a fresh install (bind address for agent.env).
ADDR="${EBBTOP_ADDR:-}"

usage() {
  cat >&2 <<'USAGE'
usage: install-agent.sh [--prebuilt | --rollback]

  (no argument)  build from source in this checkout and install/update it
  --prebuilt     install the package in this script's directory
                 (ebbtop-agent, ebbtop-agent.service, BUILD, SHA256SUMS);
                 needs no git, cargo or rustc
  --rollback     restore the previous binary saved as ebbtop-agent.bak
USAGE
}

die() { echo "install-agent.sh: $*" >&2; exit 1; }

migrate_from_kmon() {
  # One-time move from the pre-rename layout. Keeps token and env.
  if [[ ! -e "$SVC" && -e "$OLD_SVC" ]]; then
    mv "$OLD_SVC" "$SVC"
    echo "migrated $OLD_SVC -> $SVC"
  fi
  if [[ -f "$SVC/kmon.token" && ! -f "$SVC/ebbtop.token" ]]; then
    mv "$SVC/kmon.token" "$SVC/ebbtop.token"
    echo "migrated kmon.token -> ebbtop.token"
  fi
  if [[ -f "$OLD_UNIT" ]]; then
    systemctl --user stop kmon-agent.service 2>/dev/null || true
    systemctl --user disable kmon-agent.service 2>/dev/null || true
    rm -f "$OLD_UNIT"
    echo "retired kmon-agent.service"
  fi
}

# --- prebuilt package path -------------------------------------------------
# The archive carries exactly these five members at its top level.
PKG_MEMBERS=(BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh)
# SHA256SUMS covers the payload: build metadata, binary and unit.
PKG_SUMMED=(BUILD ebbtop-agent ebbtop-agent.service)

# A bind address must be an IP literal on this host's tailnet/LAN. Wildcard
# and loopback-only addresses are refused: the agent must never listen beyond
# the address the operator configured, and a loopback-only agent cannot be
# reached by the client that monitors this host.
validate_bind() {
  local addr="$1" octet
  case "$addr" in
    "")        die "bind address is empty" ;;
    0.0.0.0|::|::0|0:0:0:0:0:0:0:0|::ffff:0.0.0.0)
               die "refusing bind address '$addr': it listens beyond the configured address" ;;
    127.*|::1|0:0:0:0:0:0:0:1)
               die "refusing bind address '$addr': loopback-only, the client cannot reach it" ;;
  esac
  if [[ "$addr" == *:* ]]; then
    [[ "$addr" =~ ^[0-9A-Fa-f:]+$ ]] || die "invalid bind address '$addr': not an IP literal"
    return 0
  fi
  [[ "$addr" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})$ ]] \
    || die "invalid bind address '$addr': not an IP literal"
  for octet in "${BASH_REMATCH[@]:1}"; do
    (( 10#$octet <= 255 )) || die "invalid bind address '$addr': octet $octet out of range"
  done
  return 0
}

verify_package() {
  local dir="$1" entry member line name count=0
  local -a entries=()
  local -A seen=()

  # 1. Member set: exactly the five names above, no extra files, no links,
  #    no directories. A partial extraction or a repacked archive is refused
  #    before anything on the host is touched.
  for entry in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do
    [[ -e "$entry" || -L "$entry" ]] || continue
    entries+=("${entry##*/}")
  done
  if (( ${#entries[@]} != ${#PKG_MEMBERS[@]} )); then
    die "package member set mismatch in $dir: expected ${#PKG_MEMBERS[@]} members (${PKG_MEMBERS[*]}), found ${#entries[@]} (${entries[*]:-none})"
  fi
  for entry in "${entries[@]}"; do
    seen["$entry"]=1
  done
  for member in "${PKG_MEMBERS[@]}"; do
    [[ -n "${seen[$member]:-}" ]] || die "package is missing member '$member' in $dir"
    [[ -f "$dir/$member" && ! -L "$dir/$member" ]] || die "package member '$member' is not a regular file"
  done

  # 2. SHA256SUMS must name exactly the payload files by bare name, so
  #    sha256sum -c can never be pointed at a path outside the package.
  while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -z "$line" ]] && continue
    [[ "$line" =~ ^[0-9a-f]{64}[[:space:]][[:space:]]([A-Za-z0-9._-]+)$ ]] \
      || die "malformed SHA256SUMS line: '$line'"
    name="${BASH_REMATCH[1]}"
    case "$name" in
      BUILD|ebbtop-agent|ebbtop-agent.service) ;;
      *) die "SHA256SUMS names an unexpected member: '$name'" ;;
    esac
    seen["sum:$name"]=1
    count=$((count + 1))
  done < "$dir/SHA256SUMS"
  (( count == ${#PKG_SUMMED[@]} )) \
    || die "SHA256SUMS must cover exactly ${#PKG_SUMMED[@]} files, found $count"
  for member in "${PKG_SUMMED[@]}"; do
    [[ -n "${seen[sum:$member]:-}" ]] || die "SHA256SUMS does not cover '$member'"
  done
  ( cd "$dir" && sha256sum --strict --quiet -c SHA256SUMS ) \
    || die "package checksum verification failed in $dir (tampered or incomplete archive)"
  echo "verified: SHA256SUMS ok for ${PKG_SUMMED[*]}"

  # 3. The unit must keep the documented contract: agent.env as the only
  #    EnvironmentFile and the installed binary as ExecStart.
  grep -qx 'EnvironmentFile=%h/services/ebbtop-agent/agent.env' "$dir/ebbtop-agent.service" \
    || die "package unit does not carry the expected EnvironmentFile contract"
  grep -q '^ExecStart=%h/services/ebbtop-agent/bin/ebbtop-agent --port [0-9][0-9]*$' "$dir/ebbtop-agent.service" \
    || die "package unit does not start the installed agent binary"
}

prebuilt_bind() {
  local addr=""
  if [[ -f "$SVC/agent.env" ]]; then
    addr="$(sed -n 's/^EBBTOP_BIND=//p' "$SVC/agent.env" | head -n 1)"
    [[ -n "$addr" ]] || die "existing $SVC/agent.env has no EBBTOP_BIND; the agent would bind 0.0.0.0. Add EBBTOP_BIND=<this host's tailnet/LAN IP> and re-run."
  else
    [[ -n "$ADDR" ]] || die "fresh install needs EBBTOP_ADDR=this host's tailnet/LAN IP"
    addr="$ADDR"
  fi
  validate_bind "$addr"
  printf '%s\n' "$addr"
}

install_prebuilt() {
  local dir="$SCRIPT_DIR" arch addr holder prev_sha="" state running restored
  local listen linger tok

  # Refuse anything we cannot install on this host before touching the service.
  arch="$(uname -m)"
  [[ "$arch" == "x86_64" ]] \
    || die "unsupported architecture '$arch': this package is x86_64 only"
  verify_package "$dir"
  addr="$(prebuilt_bind)"

  migrate_from_kmon

  # Port must be free or already ours.
  holder="$(ss -tlnpH "sport = :$PORT" 2>/dev/null || true)"
  if [[ -n "$holder" && "$holder" != *ebbtop-agent* ]]; then
    echo "port $PORT held by something else: $holder"; exit 1
  fi

  mkdir -p "$SVC/bin" "$(dirname "$UNIT")"
  if [[ -f "$SVC/bin/ebbtop-agent" ]]; then
    cp -p "$SVC/bin/ebbtop-agent" "$SVC/bin/ebbtop-agent.bak"
    prev_sha="$(sha256sum "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)"
  fi
  install -m 755 "$dir/ebbtop-agent" "$SVC/bin/ebbtop-agent"

  # An existing agent.env is never rewritten: bind address, token and any
  # settings the operator appended after first install stay byte-identical.
  if [[ ! -f "$SVC/agent.env" ]]; then
    ( umask 077
      tok="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
      printf 'EBBTOP_BIND=%s\nEBBTOP_AGENT_PROCESSES_TTL_MS=500\nEBBTOP_AGENT_METRICS_TTL_MS=100\nEBBTOP_TOKEN=%s\n' "$addr" "$tok" > "$SVC/agent.env"
      printf '%s\n' "$tok" > "$SVC/ebbtop.token"
      echo "new token written to $SVC/ebbtop.token (mode 600)" )
    chmod 600 "$SVC/agent.env" "$SVC/ebbtop.token"
  fi

  install -m 644 "$dir/ebbtop-agent.service" "$UNIT"
  systemctl --user daemon-reload
  systemctl --user enable ebbtop-agent.service
  systemctl --user restart ebbtop-agent.service
  sleep 2

  state="$(systemctl --user is-active ebbtop-agent.service || true)"
  if [[ "$state" != "active" ]]; then
    echo "ebbtop-agent failed to start after install (is-active: ${state:-unknown})"
    if [[ -n "$prev_sha" ]]; then
      mv -f "$SVC/bin/ebbtop-agent.bak" "$SVC/bin/ebbtop-agent"
      systemctl --user restart ebbtop-agent.service
      sleep 2
      restored="$(sha256sum "$SVC/bin/ebbtop-agent" | cut -d' ' -f1)"
      state="$(systemctl --user is-active ebbtop-agent.service || true)"
      running="$(systemctl --user show -p ExecStart --value ebbtop-agent.service | grep -o 'path=[^ ;]*' | cut -d= -f2)"
      [[ "$restored" == "$prev_sha" ]] \
        || echo "restored binary sha256 $restored does not match the previous $prev_sha"
      [[ "$running" == "$SVC/bin/ebbtop-agent" ]] \
        || echo "unit ExecStart path '$running' is not the restored binary"
      if [[ "$restored" == "$prev_sha" && "$running" == "$SVC/bin/ebbtop-agent" && "$state" == "active" ]]; then
        echo "restored previous binary: ${prev_sha:0:16} ($SVC/bin/ebbtop-agent)"
        echo "install failed; the previous version is running again"
      else
        echo "restore could not be verified (is-active: ${state:-unknown})"
      fi
      exit 1
    fi
    echo "fresh install failed to start; no previous binary to restore"
    exit 1
  fi

  echo "version: $(sed -n 's/^version=//p' "$dir/BUILD")"
  echo "active: $state"
  listen="$(ss -tlnH "sport = :$PORT" | awk '{print $4}' | paste -sd' ')"
  echo "listen: ${listen:-none (nothing listening on port $PORT)}"
  echo "binary: $(sha256sum "$SVC/bin/ebbtop-agent" | cut -c1-16)"
  echo "loaded: $(systemctl --user show -p ExecStart --value ebbtop-agent.service | grep -o 'path=[^ ;]*')"
  linger=""
  if command -v loginctl >/dev/null 2>&1; then
    linger="$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || true)"
  fi
  if [[ "$linger" == "yes" ]]; then
    echo "linger: yes (unit starts at boot)"
  else
    echo "linger: ${linger:-unknown} - this unit will NOT start at boot; enable it with: sudo loginctl enable-linger $(id -un)"
  fi
}

# --- dispatch --------------------------------------------------------------
if (( $# > 1 )); then
  echo "unknown argument: $2" >&2
  usage
  exit 2
fi

case "${1:-}" in
  "")          MODE=source ;;
  --prebuilt)  MODE=prebuilt ;;
  --rollback)  MODE=rollback ;;
  *)           echo "unknown option: $1" >&2; usage; exit 2 ;;
esac

if [[ "$MODE" == "rollback" ]]; then
  if [[ -f "$SVC/bin/ebbtop-agent.bak" ]]; then
    mv "$SVC/bin/ebbtop-agent.bak" "$SVC/bin/ebbtop-agent"
  else
    echo "no .bak to roll back to"
    exit 1
  fi
  systemctl --user restart ebbtop-agent.service
  echo "rolled back"; exit 0
fi

if [[ "$MODE" == "prebuilt" ]]; then
  install_prebuilt
  exit 0
fi

migrate_from_kmon

# Port must be free or already ours.
holder="$(ss -tlnpH "sport = :$PORT" 2>/dev/null || true)"
if [[ -n "$holder" && "$holder" != *ebbtop-agent* ]]; then
  echo "port $PORT held by something else: $holder"; exit 1
fi

# GPU on: gfxinfo is built without its AMD/libdrm path (needs libdrm headers
# the build host may lack); NVIDIA plus the NVML fallback cover that host.
(cd "$REPO" && cargo build --release --locked -p ebbtop-agent --no-default-features --features gpu)

mkdir -p "$SVC/bin" "$(dirname "$UNIT")"
[[ -f "$SVC/bin/ebbtop-agent" ]] && cp -p "$SVC/bin/ebbtop-agent" "$SVC/bin/ebbtop-agent.bak"
install -m 755 "$BIN" "$SVC/bin/ebbtop-agent"

( umask 077
  if [[ ! -f "$SVC/agent.env" ]]; then
    [[ -n "$ADDR" ]] || { echo "fresh install needs EBBTOP_ADDR=this host's tailnet/LAN IP"; exit 1; }
    tok="$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')"
    printf 'EBBTOP_BIND=%s\nEBBTOP_AGENT_PROCESSES_TTL_MS=500\nEBBTOP_AGENT_METRICS_TTL_MS=100\nEBBTOP_TOKEN=%s\n' "$ADDR" "$tok" > "$SVC/agent.env"
    printf '%s\n' "$tok" > "$SVC/ebbtop.token"
    echo "new token written to $SVC/ebbtop.token (mode 600)"
  fi )
chmod 600 "$SVC/agent.env" "$SVC/ebbtop.token" 2>/dev/null || true
# Settings added after first install; appended once, token untouched.
grep -q '^EBBTOP_AGENT_METRICS_TTL_MS=' "$SVC/agent.env" || echo 'EBBTOP_AGENT_METRICS_TTL_MS=100' >> "$SVC/agent.env"

install -m 644 "$REPO/deploy/ebbtop-agent.service" "$UNIT"
systemctl --user daemon-reload
systemctl --user enable ebbtop-agent.service
systemctl --user restart ebbtop-agent.service
sleep 2

echo "active: $(systemctl --user is-active ebbtop-agent.service)"
echo "listen: $(ss -tlnH "sport = :$PORT" | awk '{print $4}' | paste -sd' ')"
echo "binary: $(sha256sum "$SVC/bin/ebbtop-agent" | cut -c1-16)"
echo "loaded: $(systemctl --user show -p ExecStart --value ebbtop-agent.service | grep -o 'path=[^ ;]*')"
