#!/usr/bin/env bash
# EbbTop wizard helpers. Sourced by the install/uninstall wizards, never run
# directly. Presentation and prompts only: nothing here reads, echoes or writes
# a secret.
#
# Non-interactive use:
#   WZ_YES=1     take the default answer everywhere
#   WZ_DRY=1     print the plan, change nothing
#   WZ_NO_TTY=1  read answers from stdin even when a terminal is attached

WZ_YES="${WZ_YES:-0}"
WZ_DRY="${WZ_DRY:-0}"
WZ_NO_TTY="${WZ_NO_TTY:-0}"

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  WZ_B=$'\033[1m'; WZ_DIM=$'\033[2m'; WZ_GRN=$'\033[32m'
  WZ_YEL=$'\033[33m'; WZ_RED=$'\033[31m'; WZ_RST=$'\033[0m'
else
  WZ_B=''; WZ_DIM=''; WZ_GRN=''; WZ_YEL=''; WZ_RED=''; WZ_RST=''
fi

WZ_RULE='------------------------------------------------------------'

# wz_banner <title> [subtitle]
wz_banner() {
  printf '\n%s%s%s\n' "$WZ_B" "${1:-EbbTop}" "$WZ_RST"
  [[ -n "${2:-}" ]] && printf '%s%s%s\n' "$WZ_DIM" "$2" "$WZ_RST"
  printf '%s\n' "$WZ_RULE"
}

# wz_step <text>: numbered or plain stage heading
wz_step() { printf '\n%s== %s%s\n' "$WZ_B" "$*" "$WZ_RST"; }
wz_info() { printf '   %s\n' "$*"; }
wz_ok()   { printf '   %sok%s   %s\n' "$WZ_GRN" "$WZ_RST" "$*"; }
wz_warn() { printf '   %swarn%s %s\n' "$WZ_YEL" "$WZ_RST" "$*"; }
wz_err()  { printf '   %sfail%s %s\n' "$WZ_RED" "$WZ_RST" "$*" >&2; }
wz_die()  { wz_err "$*"; exit 1; }
wz_note() { printf '   %s%s%s\n' "$WZ_DIM" "$*" "$WZ_RST"; }

# wz_kv <key> <value>: aligned two-column line
wz_kv() { printf '   %-10s %s\n' "$1" "$2"; }

# wz_run <cmd...>: run, or print what would run in dry mode
wz_run() {
  if ((WZ_DRY)); then
    printf '   %swould:%s %s\n' "$WZ_DIM" "$WZ_RST" "$*"
    return 0
  fi
  "$@"
}

wz__input() {
  REPLY=''
  if ((WZ_NO_TTY == 0)) && [[ -r /dev/tty ]]; then
    IFS= read -r REPLY < /dev/tty || REPLY=''
  else
    IFS= read -r REPLY || REPLY=''
  fi
}

# wz_ask <VAR> <prompt> [default]
wz_ask() {
  local __var="$1" __prompt="$2" __default="${3:-}"
  if ((WZ_YES || WZ_DRY)); then
    printf '   %s: %s\n' "$__prompt" "${__default:-(empty)}"
    printf -v "$__var" '%s' "$__default"
    return 0
  fi
  printf '   %s' "$__prompt"
  [[ -n "$__default" ]] && printf ' [%s]' "$__default"
  printf ': '
  wz__input
  [[ -z "$REPLY" ]] && REPLY="$__default"
  printf -v "$__var" '%s' "$REPLY"
}

# wz_confirm <prompt> [y|n] -> 0 yes, 1 no. Default is n.
# WZ_YES=1 answers yes to every confirmation (that is what --yes is for);
# WZ_DRY=1 reports the default without asking.
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
  if [[ "$def" == y ]]; then hint='Y/n'; else hint='y/N'; fi
  printf '   %s [%s]: ' "$prompt" "$hint"
  wz__input
  ans="${REPLY:-$def}"
  case "$ans" in
    [Yy]*) return 0 ;;
    *) return 1 ;;
  esac
}

# wz_menu <VAR> <prompt> <option...> : sets VAR to the chosen 1-based index
wz_menu() {
  local __var="$1" __prompt="$2" i pick
  shift 2
  local opts=("$@")
  printf '\n   %s\n' "$__prompt"
  for i in "${!opts[@]}"; do
    printf '     %d) %s\n' "$((i + 1))" "${opts[$i]}"
  done
  if ((WZ_YES || WZ_DRY)); then
    printf '     -> 1\n'
    printf -v "$__var" '%s' 1
    return 0
  fi
  while :; do
    printf '   choice [1]: '
    wz__input
    pick="${REPLY:-1}"
    if [[ "$pick" =~ ^[0-9]+$ ]] && ((pick >= 1 && pick <= ${#opts[@]})); then
      printf -v "$__var" '%s' "$pick"
      return 0
    fi
    printf '   enter a number from 1 to %d\n' "${#opts[@]}"
  done
}

wz_pause() {
  if ((WZ_YES || WZ_DRY)); then return 0; fi
  printf '\n   press Enter to continue: '
  wz__input
}

# wz_need <command> <hint>
wz_need() {
  command -v "$1" >/dev/null 2>&1 && return 0
  wz_warn "missing: $1${2:+ ($2)}"
  return 1
}
