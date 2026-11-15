#!/usr/bin/env bash
# EbbTop agent wizard: install, update, uninstall and inspect the agent on a
# Linux host, local or over SSH.
#
#   deploy/agent-wizard.sh                       menu
#   deploy/agent-wizard.sh install               this host (builds from this tree)
#   deploy/agent-wizard.sh uninstall [--purge]    this host
#   deploy/agent-wizard.sh status                 this host
#   deploy/agent-wizard.sh install --host user@host
#   deploy/agent-wizard.sh uninstall --host user@host [--purge]
#
# Flags:
#   --host user@host   remote target over SSH (extra options: EBBTOP_SSH_OPTS)
#   --bind <addr>      bind address, instead of detecting/prompting
#   --port <n>         agent port (default 45976, or EBBTOP_PORT)
#   --purge            uninstall also deletes the token and env files
#   --yes, -y          take every default, no prompts
#   --dry-run, -n      print the plan, change nothing
#
# The paths and the service layout are the ones deploy/install-agent.sh and
# deploy/install-agent-remote.sh already use; this script adds the guided flow
# and the uninstall path they never had. No secret is printed: the token file is
# only ever counted and hashed, never read out.
set -euo pipefail

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=wizard/common.sh
source "$SELF_DIR/wizard/common.sh"

REPO="$(cd "$SELF_DIR/.." && pwd)"
UNIT_NAME="ebbtop-agent.service"
PORT="${EBBTOP_PORT:-45976}"
SVC="$HOME/services/ebbtop-agent"
BIN="$SVC/bin/ebbtop-agent"
BAK="$SVC/bin/ebbtop-agent.bak"
ENVFILE="$SVC/agent.env"
TOKENFILE="$SVC/ebbtop.token"
UNIT="$HOME/.config/systemd/user/$UNIT_NAME"

ACTION=''
HOST=''
BIND="${EBBTOP_ADDR:-}"
PURGE=0
PROBE_SECS=5

SSH_OPTS=()
if [[ -n "${EBBTOP_SSH_OPTS:-}" ]]; then
  read -r -a SSH_OPTS <<<"$EBBTOP_SSH_OPTS"
fi

usage() {
  # Print the header comment block only (lines after the shebang, until code).
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "${BASH_SOURCE[0]}"
}

remote() { ssh "${SSH_OPTS[@]}" "$HOST" "$@"; }

agent_installed() { [[ -f "$BIN" || -f "$UNIT" ]]; }

svc_state() { systemctl --user is-active "$UNIT_NAME" 2>/dev/null || true; }
enabled_state() { systemctl --user is-enabled "$UNIT_NAME" 2>/dev/null || true; }
listen_addrs() { ss -tlnH "sport = :$PORT" 2>/dev/null | awk '{print $4}' | paste -sd' ' - || true; }
env_bind() { grep -m1 '^EBBTOP_BIND=' "$ENVFILE" 2>/dev/null | cut -d= -f2- || true; }

detect_bind() {
  local b=''
  [[ -f "$ENVFILE" ]] && b="$(env_bind)"
  if [[ -z "$b" ]] && command -v tailscale >/dev/null 2>&1; then
    b="$(tailscale ip -4 2>/dev/null | head -n 1 || true)"
  fi
  if [[ -z "$b" ]]; then
    b="$(ip -4 route get 1.1.1.1 2>/dev/null |
      awk '{for (i = 1; i <= NF; i++) if ($i == "src") { print $(i + 1); exit }}' || true)"
  fi
  printf '%s' "$b"
}

is_local_addr() {
  ip -4 -o addr show 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | grep -qx "$1"
}

probe_agent() {
  local secs="$1" probe="$REPO/target/release/ebbtop-probe" bind
  if [[ ! -x "$probe" ]]; then
    wz_note "probe not built; build it with: cargo build --release -p ebbtop-core --bin ebbtop-probe"
    return 0
  fi
  if [[ ! -f "$TOKENFILE" ]]; then
    wz_warn "no token file at $TOKENFILE, skipping the live probe"
    return 0
  fi
  bind="${BIND:-$(detect_bind)}"
  wz_info "probing ws://$bind:$PORT/ws for ${secs}s (token read from the file, never printed)"
  "$probe" --url "ws://$bind:$PORT/ws" --seconds "$secs" --token-file "$TOKENFILE" 2>&1 |
    sed -n '/^--- summary/,$p' | sed 's/^/   /'
}

status_local() {
  wz_step "agent status on this host ($(hostname))"
  if ! agent_installed; then
    wz_info "not installed here (no $BIN, no $UNIT)"
    return 0
  fi
  wz_kv unit "$UNIT_NAME: $(svc_state) ($(enabled_state))"
  wz_kv port "$PORT"
  wz_kv listen "$(listen_addrs)"
  if [[ -f "$BIN" ]]; then
    wz_kv binary "$BIN"
    wz_kv sha256 "$(sha256sum "$BIN" | cut -c1-16)  built $(date -r "$BIN" '+%Y-%m-%d %H:%M')"
  else
    wz_warn "no binary at $BIN"
  fi
  if [[ -f "$ENVFILE" ]]; then
    wz_kv env "$ENVFILE (mode $(stat -c %a "$ENVFILE"), bind $(env_bind))"
  else
    wz_warn "no $ENVFILE: the unit has no EBBTOP_TOKEN and will not serve"
  fi
  if [[ -f "$TOKENFILE" ]]; then
    local n
    n="$(tr -d '\n' <"$TOKENFILE" | wc -c)"
    wz_kv token "$TOKENFILE (mode $(stat -c %a "$TOKENFILE"), $n chars, value not printed)"
  else
    wz_warn "no token file at $TOKENFILE"
  fi
  local linger
  linger="$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || true)"
  if [[ "$linger" == yes ]]; then
    wz_kv linger "enabled (agent starts at boot with no login)"
  else
    wz_warn "linger is off: the agent stops at logout and misses boots (ask an admin: loginctl enable-linger $(id -un))"
  fi
}

status_remote() {
  wz_step "agent status on $HOST"
  remote sh -s "$PORT" <<'EOFR'
set -u
PORT="$1"
SVC="$HOME/services/ebbtop-agent"
printf '   %-10s %s\n' host "$(hostname)"
printf '   %-10s %s\n' unit "$(systemctl --user is-active ebbtop-agent.service 2>/dev/null || true) ($(systemctl --user is-enabled ebbtop-agent.service 2>/dev/null || true))"
printf '   %-10s %s\n' port "$PORT"
printf '   %-10s %s\n' listen "$(ss -tlnH "sport = :$PORT" 2>/dev/null | awk '{print $4}' | paste -sd' ' -)"
if [ -f "$SVC/bin/ebbtop-agent" ]; then
  printf '   %-10s %s\n' binary "$SVC/bin/ebbtop-agent sha256:$(sha256sum "$SVC/bin/ebbtop-agent" | cut -c1-16)"
else
  printf '   %-10s %s\n' binary "missing"
fi
if [ -f "$SVC/agent.env" ]; then
  printf '   %-10s %s\n' env "$SVC/agent.env (bind $(grep -m1 '^EBBTOP_BIND=' "$SVC/agent.env" | cut -d= -f2-))"
fi
if [ -f "$SVC/ebbtop.token" ]; then
  printf '   %-10s %s\n' token "present, $(tr -d '\n' < "$SVC/ebbtop.token" | wc -c) chars, value not printed"
fi
printf '   %-10s %s\n' linger "$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || echo unknown)"
EOFR
}

verify_local() {
  wz_step "verify"
  local i st
  for i in 1 2 3 4 5 6 7 8 9 10; do
    st="$(svc_state)"
    [[ "$st" == active ]] && break
    sleep 1
  done
  if [[ "$(svc_state)" == active ]]; then
    wz_ok "unit is active (MainPID $(systemctl --user show -p MainPID --value "$UNIT_NAME"))"
  else
    wz_err "unit is $(svc_state) after 10s: journalctl --user -u $UNIT_NAME -n 40"
    return 1
  fi
  local listen
  listen="$(listen_addrs)"
  if [[ "$listen" == *":$PORT"* ]]; then
    wz_ok "listening on $listen"
  else
    wz_err "nothing listening on port $PORT"
    return 1
  fi
  if [[ -x "$REPO/target/release/ebbtop-agent" ]]; then
    local a b
    a="$(sha256sum "$BIN" | cut -d' ' -f1)"
    b="$(sha256sum "$REPO/target/release/ebbtop-agent" | cut -d' ' -f1)"
    if [[ "$a" == "$b" ]]; then
      wz_ok "installed bytes match the build (sha256 ${a:0:16})"
    else
      wz_err "installed bytes differ from the build: ${a:0:16} vs ${b:0:16}"
    fi
  fi
  probe_agent "$PROBE_SECS"
}

port_owner() { ss -tlnpH "sport = :$PORT" 2>/dev/null || true; }

install_local() {
  wz_step "preflight"
  wz_need cargo "the Rust toolchain builds the agent on this host; or run the install from a host that has it with --host" ||
    wz_die "cannot build the agent here"
  wz_need systemctl "a systemd user session is required" || wz_die "no systemd user session"
  [[ -f "$REPO/deploy/install-agent.sh" ]] || wz_die "missing $REPO/deploy/install-agent.sh"
  local holder
  holder="$(port_owner)"
  if [[ -n "$holder" && "$holder" != *ebbtop-agent* ]]; then
    wz_die "port $PORT is held by another process:$holder"
  fi
  wz_ok "cargo, systemd, repo and port $PORT"

  wz_step "bind address"
  if [[ -z "$BIND" ]]; then
    BIND="$(detect_bind)"
    wz_ask BIND "address the agent binds (tailnet/LAN IP of this host)" "$BIND"
  fi
  [[ -n "$BIND" ]] || wz_die "a bind address is required for a fresh install"
  wz_info "bind: $BIND"
  if is_local_addr "$BIND"; then
    wz_ok "$BIND is an address on this host"
  else
    wz_warn "$BIND is not on this host right now; the unit keeps retrying until it appears"
  fi

  wz_step "plan"
  wz_info "1. build ebbtop-agent from $REPO (release, gpu)"
  wz_info "2. back up any current binary to $(basename "$BAK")"
  wz_info "3. install to $BIN"
  wz_info "4. write/keep $ENVFILE (a fresh install generates a token, mode 600)"
  wz_info "5. install $UNIT, enable it, restart the service"
  wz_info "6. verify: unit active, port listening, bytes match, ${PROBE_SECS}s probe"
  if ! wz_confirm "install or update the agent on this host?" y; then
    wz_info "cancelled, nothing changed"
    return 0
  fi

  wz_step "install"
  if ((WZ_DRY)); then
    wz_run env EBBTOP_ADDR="$BIND" EBBTOP_PORT="$PORT" "$REPO/deploy/install-agent.sh"
    return 0
  fi
  EBBTOP_ADDR="$BIND" EBBTOP_PORT="$PORT" bash "$REPO/deploy/install-agent.sh"
  verify_local
  wz_step "next"
  wz_info "client URL: ws://$BIND:$PORT/ws"
  wz_info "token:      ssh $(id -un)@$(hostname) cat services/ebbtop-agent/ebbtop.token | pbcopy"
  wz_info "or read it from $TOKENFILE (mode 600) and paste it into EbbTop onboarding"
}

uninstall_local() {
  wz_step "what is installed here"
  if ! agent_installed; then
    wz_ok "nothing to remove on this host (no $BIN, no $UNIT)"
    return 0
  fi
  wz_kv unit "$UNIT_NAME: $(svc_state) ($(enabled_state))"
  [[ -f "$BIN" ]] && wz_kv binary "$BIN"
  [[ -f "$BAK" ]] && wz_kv backup "$BAK"
  [[ -f "$ENVFILE" ]] && wz_kv env "$ENVFILE"
  [[ -f "$TOKENFILE" ]] && wz_kv token "$TOKENFILE (value not printed)"

  wz_step "plan"
  wz_info "1. stop and disable $UNIT_NAME"
  wz_info "2. remove $UNIT and reload systemd"
  wz_info "3. remove the binary and its .bak"
  if ((PURGE)); then
    wz_warn "4. remove the whole $SVC directory: token and env included (clients lose access)"
  else
    wz_info "4. keep $SVC (token and env are reused by the next install)"
  fi
  wz_info "nothing else on the host is touched: no packages, no linger change, no user files"
  if ! wz_confirm "uninstall the agent from this host?" n; then
    wz_info "cancelled, nothing changed"
    return 0
  fi

  wz_step "remove"
  if ((WZ_DRY)); then
    wz_run systemctl --user stop "$UNIT_NAME"
    wz_run systemctl --user disable "$UNIT_NAME"
    wz_run rm -f "$UNIT"
    wz_run systemctl --user daemon-reload
    wz_run rm -f "$BIN" "$BAK"
    ((PURGE)) && wz_run rm -rf "$SVC"
    return 0
  fi
  systemctl --user stop "$UNIT_NAME" 2>/dev/null || true
  systemctl --user disable "$UNIT_NAME" 2>/dev/null || true
  rm -f "$UNIT"
  systemctl --user daemon-reload 2>/dev/null || true
  rm -f "$BIN" "$BAK"
  if ((PURGE)); then
    rm -rf "$SVC"
  fi

  wz_step "verify"
  local st en listen
  st="$(svc_state)"
  en="$(enabled_state)"
  if [[ "$st" != active && "$en" != enabled ]]; then
    wz_ok "unit $UNIT_NAME is $st (${en:-not-found})"
  else
    wz_err "unit still $st/$en: check systemctl --user status $UNIT_NAME"
  fi
  [[ -e "$UNIT" ]] && wz_err "unit file still present: $UNIT" || wz_ok "unit file removed"
  listen="$(listen_addrs)"
  if [[ -z "$listen" ]]; then
    wz_ok "port $PORT is free"
  else
    wz_err "port $PORT still listening on $listen"
  fi
  [[ -f "$BIN" ]] && wz_err "binary still present: $BIN" || wz_ok "binary removed"
  if ((PURGE)); then
    [[ -e "$SVC" ]] && wz_warn "$SVC still exists" || wz_ok "$SVC removed (token gone)"
  elif [[ -f "$TOKENFILE" ]]; then
    wz_note "kept: $SVC (token and env). Delete it with: deploy/agent-wizard.sh uninstall --purge"
  fi
  wz_note "linger for $(id -un) is unchanged: loginctl disable-linger $(id -un) (admin) if you want it off"
}

remote_uninstall_payload() {
  remote sh -s "$PORT" "$((PURGE ? 1 : 0))" <<'EOFR'
set -u
PORT="$1"; PURGE="$2"
SVC="$HOME/services/ebbtop-agent"
UNIT="$HOME/.config/systemd/user/ebbtop-agent.service"
systemctl --user stop ebbtop-agent.service 2>/dev/null || true
systemctl --user disable ebbtop-agent.service 2>/dev/null || true
rm -f "$UNIT"
systemctl --user daemon-reload 2>/dev/null || true
rm -f "$SVC/bin/ebbtop-agent" "$SVC/bin/ebbtop-agent.bak"
if [ "$PURGE" = 1 ]; then rm -rf "$SVC"; fi
printf '   %-10s %s\n' unit "$(systemctl --user is-active ebbtop-agent.service 2>/dev/null || true) ($(systemctl --user is-enabled ebbtop-agent.service 2>/dev/null || true))"
printf '   %-10s %s\n' binary "$( [ -f "$SVC/bin/ebbtop-agent" ] && echo present || echo removed)"
printf '   %-10s %s\n' port "$(ss -tlnH "sport = :$PORT" 2>/dev/null | awk '{print $4}' | paste -sd' ' -)"
if [ "$PURGE" = 0 ] && [ -f "$SVC/ebbtop.token" ]; then
  printf '   %-10s %s\n' token "kept in $SVC (use --purge to delete)"
fi
EOFR
}

install_remote() {
  [[ -n "$HOST" ]] || wz_die "--host user@host is required for a remote install"
  wz_step "preflight for $HOST"
  wz_need ssh "remote installs use ssh" || wz_die "ssh not available"
  [[ -f "$REPO/deploy/install-agent-remote.sh" ]] || wz_die "missing deploy/install-agent-remote.sh"
  local arch
  arch="$(remote -o BatchMode=yes 'uname -m' 2>/dev/null || true)"
  [[ "$arch" == x86_64 ]] ||
    wz_die "remote arch is '${arch:-unknown}': this script ships an x86_64 binary, build on the host instead"
  wz_ok "reachable over ssh, arch $arch"

  wz_step "bind address"
  if [[ -z "$BIND" ]]; then
    BIND="$(remote 'tailscale ip -4 2>/dev/null | head -n 1' || true)"
    wz_ask BIND "address the agent binds on the remote (its tailnet/LAN IP)" "$BIND"
  fi
  [[ -n "$BIND" ]] || wz_die "a bind address is required (pass --bind <addr>)"
  wz_info "bind: $BIND, port $PORT"

  wz_step "plan"
  wz_info "1. build ebbtop-agent here and copy it to $HOST"
  wz_info "2. install it as a user service on $HOST, keep or create the token"
  wz_info "3. enable, start and probe over the network"
  if ! wz_confirm "install or update the agent on $HOST?" y; then
    wz_info "cancelled, nothing changed"
    return 0
  fi

  wz_step "install"
  if ((WZ_DRY)); then
    wz_run bash "$REPO/deploy/install-agent-remote.sh" "$HOST" "$BIND" "$PORT"
    status_remote
    return 0
  fi
  bash "$REPO/deploy/install-agent-remote.sh" "$HOST" "$BIND" "$PORT"
  status_remote
  wz_step "next"
  wz_info "client URL: ws://$BIND:$PORT/ws"
  wz_info "token:      ssh $HOST cat services/ebbtop-agent/ebbtop.token | pbcopy"
}

uninstall_remote() {
  [[ -n "$HOST" ]] || wz_die "--host user@host is required for a remote uninstall"
  wz_step "what is installed on $HOST"
  status_remote
  wz_step "plan"
  wz_info "1. stop and disable ebbtop-agent.service on $HOST"
  wz_info "2. remove the unit, reload systemd, remove the binary and its .bak"
  if ((PURGE)); then
    wz_warn "3. remove ~/services/ebbtop-agent on $HOST: token and env included"
  else
    wz_info "3. keep ~/services/ebbtop-agent on $HOST (token and env reused next time)"
  fi
  if ! wz_confirm "uninstall the agent from $HOST?" n; then
    wz_info "cancelled, nothing changed"
    return 0
  fi
  wz_step "remove"
  if ((WZ_DRY)); then
    wz_note "would run the uninstall payload on $HOST over ssh"
    return 0
  fi
  remote_uninstall_payload
}

choose_target() { # sets HOST ('' = this host); returns 1 on cancel
  if [[ -n "$HOST" ]]; then return 0; fi
  local pick
  wz_menu pick "which host?" "this host" "a remote host over SSH"
  if [[ "$pick" == 2 ]]; then
    wz_ask HOST "remote target (user@host)" ""
    [[ -n "$HOST" ]] || return 1
  fi
  return 0
}

menu() {
  local choice
  while :; do
    wz_banner "EbbTop agent wizard" "$(hostname)   repo: $REPO"
    if agent_installed; then
      wz_info "state: installed, unit $(svc_state)"
    else
      wz_info "state: not installed on this host"
    fi
    wz_menu choice "what do you want to do?" \
      "install or update the agent" \
      "uninstall the agent" \
      "show status" \
      "quit"
    case "$choice" in
      1) choose_target || { continue; }; if [[ -n "$HOST" ]]; then install_remote || true; else install_local || true; fi ;;
      2) choose_target || { continue; }; if [[ -n "$HOST" ]]; then uninstall_remote || true; else uninstall_local || true; fi ;;
      3) if [[ -n "$HOST" ]]; then status_remote || true; else status_local; fi ;;
      4) exit 0 ;;
    esac
    wz_pause
  done
}

while (($#)); do
  case "$1" in
    install | uninstall | status | menu) ACTION="$1"; shift ;;
    --host) HOST="${2:?--host needs user@host}"; shift 2 ;;
    --host=*) HOST="${1#*=}"; shift ;;
    --bind) BIND="${2:?--bind needs an address}"; shift 2 ;;
    --bind=*) BIND="${1#*=}"; shift ;;
    --port) PORT="${2:?--port needs a number}"; shift 2 ;;
    --port=*) PORT="${1#*=}"; shift ;;
    --purge) PURGE=1; shift ;;
    --yes | -y) WZ_YES=1; shift ;;
    --dry-run | -n) WZ_DRY=1; shift ;;
    --help | -h) usage; exit 0 ;;
    *) wz_die "unknown argument: $1 (try --help)" ;;
  esac
done

if [[ -z "$ACTION" ]]; then
  if [[ -t 0 && -t 1 ]]; then
    menu
    exit 0
  fi
  ACTION=status
fi

case "$ACTION" in
  install) [[ -n "$HOST" ]] && install_remote || install_local ;;
  uninstall) [[ -n "$HOST" ]] && uninstall_remote || uninstall_local ;;
  status) [[ -n "$HOST" ]] && status_remote || status_local ;;
  menu) menu ;;
esac
