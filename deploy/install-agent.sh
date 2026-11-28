#!/usr/bin/env bash
# Install or update the ebbtop agent on the monitored host as a systemd
# user service. Idempotent. Keeps an existing token. Prints no secret.
#   ~/ebbtop/deploy/install-agent.sh            install/update + start
#   ~/ebbtop/deploy/install-agent.sh --rollback restore previous binary
#   ./install-agent.sh --prebuilt               install a packaged release
#                                               (no git, cargo or rustc needed)
#   ./install-agent.sh --uninstall [--purge]    stop and remove it again;
#                                               --purge also deletes the token
#                                               and the pairing state
#
# --prebuilt is only valid from an extracted ebbtop-agent-linux-x86_64.tar.gz:
# the binary, unit and manifest are read from this script's own directory.
#
# Required env (fresh install only):
#   EBBTOP_ADDR   address the agent binds (tailnet or LAN IP of this host)
# Optional env:
#   EBBTOP_PORT   port (default 45976)
#   EBBTOP_STATE_DIR  agent state directory (default ~/services/ebbtop-agent);
#                 --uninstall --purge deletes pairing.json/clients.json here
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
usage: install-agent.sh [--prebuilt | --rollback | --uninstall [--purge]]

  (no argument)  build from source in this checkout and install/update it
  --prebuilt     install the package in this script's directory
                 (ebbtop-agent, ebbtop-agent.service, BUILD, SHA256SUMS);
                 needs no git, cargo or rustc
  --rollback     restore the previous binary saved as ebbtop-agent.bak
  --uninstall    stop and disable the unit, remove the unit file and the
                 binary; keep ~/services/ebbtop-agent (token and settings)
  --purge        with --uninstall: also delete that directory, which removes
                 the token and the pairing state (pairing.json, clients.json);
                 enrolled clients lose access
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
PKG_MEMBERS=(BUILD SHA256SUMS ebbtop-agent ebbtop-agent.service install-agent.sh THIRD-PARTY-NOTICES)
# SHA256SUMS covers the payload: build metadata, binary and unit.
PKG_SUMMED=(BUILD ebbtop-agent ebbtop-agent.service THIRD-PARTY-NOTICES)

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
      BUILD|ebbtop-agent|ebbtop-agent.service|THIRD-PARTY-NOTICES) ;;
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

# An existing agent.env is read the way systemd reads an EnvironmentFile, and
# only a conservative subset of that grammar is supported. systemd v259
# (src/basic/env-file.c) parses the file as bytes: the whitespace it skips is
# exactly space, TAB, LF and CR; VT (0x0b) and FF (0x0c) are ordinary KEY
# characters; it stops at the first NUL byte; '#' and ';' start a comment; the
# key ends at the first '=' with trailing whitespace removed; an unquoted value
# runs to the end of the line with surrounding whitespace stripped; and a
# repeated key is replaced, so the LAST assignment wins.
#
# bash cannot be used to decide what systemd sees: `read` silently drops NUL
# and `[[:space:]]` is locale-dependent and wider than systemd's class. The
# file is therefore checked as raw bytes first, and only this plain ASCII
# subset is accepted:
#   * TAB, LF, CR and printable ASCII; no NUL, no other control byte and no
#     non-ASCII byte (an NBSP indent is not whitespace to systemd, so it is
#     not whitespace here either);
#   * CR ends a line exactly as systemd treats it;
#   * exactly one plain, unquoted EBBTOP_BIND=<IP literal> assignment, or, for
#     a host installed before the EBBTOP_* rename, exactly one plain, unquoted
#     legacy SOCKTOP_BIND=<IP literal> and no EBBTOP_BIND. The agent reads
#     EBBTOP_BIND first and falls back to SOCKTOP_BIND, so a file with the
#     legacy key alone yields the address the agent will actually bind; the
#     value is validated under the same grammar and the same IP-literal rules.
#     A file carrying BOTH keys is refused as ambiguous: the agent would take
#     EBBTOP_BIND and silently ignore the other, so the two lines could
#     disagree without anyone noticing;
#   * every other line is a '#'/';' comment or a plain KEY=VALUE line whose
#     key matches [A-Za-z_][A-Za-z0-9_]*.
# Duplicates and every form this installer cannot read exactly as systemd
# reads it are refused after package verification and before any write or
# systemctl/ss call, so the address validated here is the address the unit
# loads. agent.env is never rewritten or renamed: accepting the legacy key
# only means the existing bytes are read, not migrated.
env_file_bind() {
  local file="$1" line key raw value lineno=0 bad src_key src_line
  local -a bind_lines=() bind_values=() legacy_lines=() legacy_values=()

  # Raw-byte checks, before any shell parsing. NUL ends systemd's parse, so an
  # EBBTOP_BIND after it would be ignored and the agent would fall back to
  # 0.0.0.0; VT/FF would become part of a key, so systemd would not see
  # EBBTOP_BIND at all.
  if [[ "$(LC_ALL=C tr -dc '\000' < "$file" | wc -c)" -ne 0 ]]; then
    die "unsupported $file: it contains a NUL byte; systemd stops reading an EnvironmentFile there, so EBBTOP_BIND would be ignored and the agent would listen on 0.0.0.0. Remove the NUL byte and keep exactly one plain EBBTOP_BIND=<this host's tailnet/LAN IP> line."
  fi
  bad="$(LC_ALL=C tr -d '\011\012\015\040-\176' < "$file" | od -An -tx1 | tr -d ' \n' | cut -c1-2)"
  if [[ -n "$bad" ]]; then
    die "unsupported $file: the byte 0x$bad is not supported; only printable ASCII, TAB, CR and LF are allowed. systemd treats VT, FF and non-ASCII bytes as part of a key or value, so keep exactly one plain EBBTOP_BIND=<this host's tailnet/LAN IP> line and drop other control or non-ASCII bytes."
  fi

  # CR terminates a line in systemd's parser; read the file the same way.
  while IFS= read -r line || [[ -n "$line" ]]; do
    lineno=$((lineno + 1))
    key="${line#"${line%%[![:blank:]]*}"}"          # leading space/TAB
    [[ -z "$key" ]] && continue                     # empty line
    case "$key" in [\#\;]*) continue ;; esac        # '#' and ';' comments
    [[ "$key" == *=* ]] \
      || die "unsupported line $lineno in $file: a non-comment line without '=' (write KEY=VALUE)"
    raw="${key#*=}"
    key="${key%%=*}"
    key="${key%"${key##*[![:blank:]]}"}"            # trailing space/TAB on the key
    [[ "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] \
      || die "unsupported line $lineno in $file: '$key' is not a plain KEY=VALUE assignment"
    case "$key" in
      EBBTOP_BIND)  bind_lines+=("$lineno");   bind_values+=("$raw") ;;
      SOCKTOP_BIND) legacy_lines+=("$lineno"); legacy_values+=("$raw") ;;
    esac
  done < <(LC_ALL=C tr '\r' '\n' < "$file")

  # Both keys at once is ambiguous: the agent prefers EBBTOP_BIND, so the
  # legacy line would be dead weight that can disagree with the live address.
  if (( ${#bind_lines[@]} > 0 && ${#legacy_lines[@]} > 0 )); then
    die "ambiguous $file: it carries both EBBTOP_BIND (line ${bind_lines[*]}) and the legacy SOCKTOP_BIND (line ${legacy_lines[*]}); the agent would use EBBTOP_BIND and ignore the other, so the two lines could disagree. Remove one and re-run."
  fi
  if (( ${#bind_lines[@]} > 1 )); then
    die "ambiguous $file: ${#bind_lines[@]} EBBTOP_BIND assignments (lines ${bind_lines[*]}); systemd applies the last one, so the address the agent binds would depend on line order. Keep exactly one EBBTOP_BIND=<this host's tailnet/LAN IP>."
  fi
  if (( ${#legacy_lines[@]} > 1 )); then
    die "ambiguous $file: ${#legacy_lines[@]} legacy SOCKTOP_BIND assignments (lines ${legacy_lines[*]}); systemd applies the last one, so the address the agent binds would depend on line order. Keep exactly one SOCKTOP_BIND=<this host's tailnet/LAN IP>, or rename it to EBBTOP_BIND."
  fi
  if (( ${#bind_lines[@]} == 0 && ${#legacy_lines[@]} == 0 )); then
    die "existing $file has no EBBTOP_BIND (and no legacy SOCKTOP_BIND); the agent would bind 0.0.0.0. Add EBBTOP_BIND=<this host's tailnet/LAN IP> and re-run."
  fi

  if (( ${#bind_lines[@]} == 1 )); then
    src_key=EBBTOP_BIND
    src_line="${bind_lines[0]}"
    value="${bind_values[0]}"
  else
    # Host installed before the EBBTOP_* rename: same grammar, same rules.
    src_key=SOCKTOP_BIND
    src_line="${legacy_lines[0]}"
    value="${legacy_values[0]}"
  fi

  case "$value" in
    *\"*|*\'*|*\\*)
      die "unsupported $src_key on line $src_line of $file: quoted or escaped values are not supported (systemd would unquote it). Write $src_key=<this host's tailnet/LAN IP>." ;;
  esac
  value="${value#"${value%%[![:blank:]]*}"}"   # systemd skips space/TAB after '='
  value="${value%"${value##*[![:blank:]]}"}"   # and chomps trailing space/TAB
  printf '%s\n' "$value"
}

prebuilt_bind() {
  local addr="" envfile="$SVC/agent.env"
  if [[ -f "$envfile" ]]; then
    # Explicit propagation: bash ignores errexit for an assignment whose
    # command substitution fails, so a refusal there must exit here.
    addr="$(env_file_bind "$envfile")" || exit 1
  else
    [[ -n "$ADDR" ]] || die "fresh install needs EBBTOP_ADDR=this host's tailnet/LAN IP"
    addr="$ADDR"
  fi
  validate_bind "$addr"
  # Reached only once the address is accepted. The file has been proven to hold
  # exactly one of the two bind keys, so "no EBBTOP_BIND line" means the legacy
  # key is what supplied the address. Printed to stderr: stdout carries the
  # address. Informational only - nothing is migrated or rewritten.
  if [[ -f "$envfile" ]] && ! grep -q '^[[:blank:]]*EBBTOP_BIND[[:blank:]]*=' "$envfile"; then
    echo "note: $envfile carries the legacy SOCKTOP_BIND; using it as the bind address ($addr), file left byte-identical" >&2
  fi
  printf '%s\n' "$addr"
}

# Pairing (P3a) is an opt-in surface of the installed binary. Capability is read
# from the installed bytes rather than probed by running the agent: a binary
# from before pairing does not handle --help either, it ignores the unknown flag
# and starts serving, so a --help probe is not inert. The binary that implements
# `--pair` carries that literal as its argument name.
supports_pairing() {
  LC_ALL=C grep -qaF -e '--pair' "$1" 2>/dev/null
}

install_prebuilt() {
  local dir="$SCRIPT_DIR" arch addr holder prev_sha="" state running restored
  local listen linger tok

  # Refuse anything we cannot install on this host before touching the service.
  arch="$(uname -m)"
  [[ "$arch" == "x86_64" ]] \
    || die "unsupported architecture '$arch': this package is x86_64 only"
  verify_package "$dir"
  addr="$(prebuilt_bind)" || exit 1

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

  # P3a: one hint line, and only when the installed binary can mint a code.
  # The installer never mints a code and never prints a code or token value;
  # `--pair` prints the code itself, on this host's terminal.
  if supports_pairing "$SVC/bin/ebbtop-agent"; then
    echo "pair: mint a pairing code on this host with: $SVC/bin/ebbtop-agent --pair"
  fi
}

# --- uninstall -------------------------------------------------------------
# Token files and pairing state are counted, never read out. Without --purge
# they stay where they are: the next install reuses the token and the clients
# already enrolled. With --purge the service directory is removed, and the
# pairing state is removed from $EBBTOP_STATE_DIR when that points elsewhere
# (the agent's `--pair` and `/pair` share that directory).
token_files() {
  local f
  for f in "$SVC/ebbtop.token" "$SVC/kmon.token"; do
    if [[ -f "$f" ]]; then printf '%s\n' "$f"; fi
  done
}

pairing_files() {
  local dir="$SVC" f
  [[ -n "${EBBTOP_STATE_DIR:-}" ]] && dir="$EBBTOP_STATE_DIR"
  for f in pairing.json clients.json; do
    if [[ -f "$dir/$f" ]]; then printf '%s\n' "$dir/$f"; fi
  done
}

uninstall_agent() {
  local purge="$1" st en listen
  local -a toks=() pairs=()

  mapfile -t toks < <(token_files)
  mapfile -t pairs < <(pairing_files)

  if [[ ! -e "$UNIT" && ! -e "$OLD_UNIT" && ! -e "$SVC/bin/ebbtop-agent" && ${#toks[@]} -eq 0 && ${#pairs[@]} -eq 0 ]]; then
    echo "nothing to remove on this host (no $UNIT, no $OLD_UNIT, no $SVC/bin/ebbtop-agent, no token or pairing state)"
    return 0
  fi

  echo "unit: $UNIT"
  echo "binary: $SVC/bin/ebbtop-agent"
  echo "tokens: ${#toks[@]} file(s) present"
  echo "pairing: ${#pairs[@]} state file(s) present"
  if (( purge )); then
    echo "plan: stop and disable the unit, remove the unit file and the binary, then delete $SVC and the pairing state (the token goes with it; clients lose access)"
  else
    echo "plan: stop and disable the unit, remove the unit file and the binary; keep $SVC (token, settings and pairing state)"
  fi

  systemctl --user stop ebbtop-agent.service 2>/dev/null || true
  systemctl --user disable ebbtop-agent.service 2>/dev/null || true
  rm -f -- "$UNIT" "$OLD_UNIT"
  systemctl --user daemon-reload 2>/dev/null || true
  rm -f -- "$SVC/bin/ebbtop-agent" "$SVC/bin/ebbtop-agent.bak"
  if (( purge )); then
    if (( ${#pairs[@]} )); then rm -f -- "${pairs[@]}"; fi
    rm -rf -- "$SVC"
  fi

  st="$(systemctl --user is-active ebbtop-agent.service 2>/dev/null || true)"
  en="$(systemctl --user is-enabled ebbtop-agent.service 2>/dev/null || true)"
  listen="$(ss -tlnH "sport = :$PORT" 2>/dev/null | awk '{print $4}' | paste -sd' ' || true)"
  echo "unit: ${st:-unknown} (${en:-not-found})"
  if [[ -e "$UNIT" ]]; then echo "unit file: still present at $UNIT"; else echo "unit file: removed"; fi
  if [[ -e "$SVC/bin/ebbtop-agent" ]]; then echo "binary: still present at $SVC/bin/ebbtop-agent"; else echo "binary: removed"; fi
  echo "listen: ${listen:-none (nothing listening on port $PORT)}"
  if (( purge )); then
    echo "tokens: ${#toks[@]} file(s) deleted"
    echo "pairing: ${#pairs[@]} state file(s) deleted"
    if [[ -e "$SVC" ]]; then echo "state: $SVC still exists"; else echo "state: $SVC removed"; fi
  else
    echo "tokens: ${#toks[@]} file(s) kept in $SVC (--purge deletes them)"
    echo "pairing: ${#pairs[@]} state file(s) kept (--purge deletes them)"
  fi
}

# --- dispatch --------------------------------------------------------------
MODE=""
PURGE=0
case "${1:-}" in
  "")          MODE=source ;;
  --prebuilt)  MODE=prebuilt ;;
  --rollback)  MODE=rollback ;;
  --uninstall) MODE=uninstall ;;
  --purge)     echo "--purge only makes sense with --uninstall" >&2; usage; exit 2 ;;
  *)           echo "unknown option: $1" >&2; usage; exit 2 ;;
esac

if (( $# > 1 )); then
  if [[ "$MODE" == "uninstall" && "$2" == "--purge" && $# -eq 2 ]]; then
    PURGE=1
  else
    echo "unknown argument: $2" >&2
    usage
    exit 2
  fi
fi

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

if [[ "$MODE" == "uninstall" ]]; then
  uninstall_agent "$PURGE"
  exit 0
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
