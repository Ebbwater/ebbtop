#!/usr/bin/env bash
# EbbTop Mac wizard: install or uninstall the EbbTop.app client.
#
#   bash deploy/mac/setup.sh                     menu
#   bash deploy/mac/setup.sh install             install or replace EbbTop.app
#   bash deploy/mac/setup.sh install --from ~/Downloads/EbbTop.app.zip
#   bash deploy/mac/setup.sh uninstall [--purge]  remove the app
#   bash deploy/mac/setup.sh status              what is installed
#
# Flags:
#   --from <zip|app>   install from this bundle or zip (default: dist/, then
#                      ~/Downloads; offers a local build when both are missing)
#   --app-dir <dir>    install location (default /Applications, or
#                      ~/Applications when /Applications is not writable)
#   --purge            uninstall also deletes config, log and the stored tokens
#   --no-open          do not launch EbbTop after installing
#   --yes, -y          take every default, no prompts
#   --dry-run, -n      print the plan, change nothing
#
# Reads no secret. The tokens live in the login keychain under service
# "ebbtop" (account = source name); they are only counted, and deleted only on
# an explicit --purge, by account name.
set -euo pipefail

SELF_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../wizard/common.sh
source "$SELF_DIR/../wizard/common.sh"

REPO="$(cd "$SELF_DIR/../.." && pwd)"
APP_NAME="EbbTop.app"
KEYCHAIN_SERVICE="ebbtop"
SUPPORT="$HOME/Library/Application Support/ebbtop"

ACTION=''
FROM=''
APP_DIR="${EBBTOP_APP_DIR:-}"
PURGE=0
OPEN_APP=1

usage() {
  # Print the header comment block only (lines after the shebang, until code).
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "${BASH_SOURCE[0]}"
}

app_bundle() { printf '%s/%s' "$APP_DIR" "$APP_NAME"; }

resolve_app_dir() {
  if [[ -n "$APP_DIR" ]]; then return 0; fi
  if [[ -w /Applications ]]; then APP_DIR=/Applications; else APP_DIR="$HOME/Applications"; fi
}

app_version() {
  local plist="$1/Contents/Info.plist"
  [[ -f "$plist" ]] || { printf '?'; return 0; }
  if [[ -x /usr/libexec/PlistBuddy ]]; then
    local v
    v="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$plist" 2>/dev/null || true)"
    [[ -n "$v" ]] && { printf '%s' "$v"; return 0; }
  fi
  if command -v python3 >/dev/null 2>&1; then
    local v
    v="$(python3 -c 'import plistlib,sys;print(plistlib.load(open(sys.argv[1],"rb")).get("CFBundleShortVersionString","?"))' "$plist" 2>/dev/null || true)"
    [[ -n "$v" ]] && { printf '%s' "$v"; return 0; }
  fi
  printf '?'
}

sign_state() {
  local app="$1" out
  if ! command -v codesign >/dev/null 2>&1; then
    printf 'codesign unavailable'
    return 0
  fi
  if codesign -dv "$app" >/dev/null 2>&1; then
    out="$(codesign -dv "$app" 2>&1 || true)"
    if grep -q 'Signature=adhoc' <<<"$out"; then
      printf 'ad-hoc signed (Gatekeeper: first launch may need System Settings > Privacy & Security > Open Anyway)'
    else
      printf 'signed (%s)' "$(sed -n 's/^Authority=//p' <<<"$out" | head -n 1)"
    fi
    return 0
  fi
  printf 'not signed (Gatekeeper: first launch may need System Settings > Privacy & Security > Open Anyway)'
}

source_names() {
  local cfg="$SUPPORT/config.json"
  [[ -f "$cfg" ]] || return 0
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$cfg" <<'EOF' 2>/dev/null || true
import json, sys
try:
    cfg = json.load(open(sys.argv[1]))
except Exception:
    raise SystemExit(0)
for s in cfg.get("sources", []):
    n = s.get("name")
    if n:
        print(n)
EOF
  else
    sed -n 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$cfg"
  fi
}

keychain_present() { # $1 account (source name)
  command -v security >/dev/null 2>&1 || return 1
  security find-generic-password -s "$KEYCHAIN_SERVICE" -a "$1" >/dev/null 2>&1
}

keychain_count() {
  local n=0 s
  while IFS= read -r s; do
    [[ -n "$s" ]] && keychain_present "$s" && n=$((n + 1))
  done < <(source_names)
  printf '%s' "$n"
}

quit_app() {
  if pgrep -x ebbtop >/dev/null 2>&1; then
    wz_info "quitting the running EbbTop"
    if command -v osascript >/dev/null 2>&1; then
      osascript -e 'quit app "EbbTop"' >/dev/null 2>&1 || true
    fi
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      pgrep -x ebbtop >/dev/null 2>&1 || return 0
      sleep 0.4
    done
    pkill -x ebbtop 2>/dev/null || true
    wz_warn "EbbTop had to be forced to quit"
  fi
}

status() {
  resolve_app_dir
  local app found_any=0 alt
  app="$(app_bundle)"
  wz_step "EbbTop.app status on this Mac"
  if [[ -d "$app" ]]; then
    wz_kv bundle "$app"
    wz_kv version "$(app_version "$app")"
    wz_kv signing "$(sign_state "$app")"
    wz_kv size "$(du -sh "$app" 2>/dev/null | awk '{print $1}')"
    found_any=1
  else
    for alt in /Applications "$HOME/Applications"; do
      if [[ -d "$alt/$APP_NAME" ]]; then
        wz_warn "not at $app, but found $alt/$APP_NAME v$(app_version "$alt/$APP_NAME")"
        found_any=1
      fi
    done
    ((found_any == 0)) && wz_info "not installed (no $app)"
  fi
  if pgrep -x ebbtop >/dev/null 2>&1; then
    wz_kv running "yes (pid $(pgrep -x ebbtop | head -n 1))"
  else
    wz_kv running "no"
  fi
  if [[ -d "$SUPPORT" ]]; then
    local size="?"
    [[ -f "$SUPPORT/ebbtop.log" ]] && size="$(du -h "$SUPPORT/ebbtop.log" | awk '{print $1}')"
    wz_kv state "$SUPPORT (config $([[ -f "$SUPPORT/config.json" ]] && echo yes || echo no), log $size)"
    wz_kv sources "$(source_names | paste -sd, - | sed 's/^$/none/')"
    wz_kv keychain "$(keychain_count) stored token(s) for those sources (values not read)"
  else
    wz_kv state "$SUPPORT (missing: first launch creates it)"
  fi
}

prepare_source() { # sets SRC (a .app path) or SRC_ZIP; may build
  local cand
  if [[ -n "$FROM" ]]; then
    [[ -e "$FROM" ]] || wz_die "no such file: $FROM"
    SRC="$FROM"
    return 0
  fi
  for cand in "$REPO/dist/EbbTop.app.zip" "$REPO/dist/EbbTop.app" \
    "$HOME/Downloads/EbbTop.app.zip" "$HOME/Downloads/EbbTop.app"; do
    if [[ -e "$cand" ]]; then
      SRC="$cand"
      wz_note "using $cand (pass --from to choose another)"
      return 0
    fi
  done
  wz_warn "no EbbTop.app or EbbTop.app.zip in dist/ or ~/Downloads"
  [[ -f "$REPO/deploy/mac/package.sh" ]] || wz_die "nothing to install from"
  if ! wz_confirm "build the bundle now with deploy/mac/package.sh? (needs cargo)" y; then
    wz_die "nothing to install"
  fi
  if ((WZ_DRY)); then
    SRC="$REPO/dist/EbbTop.app"
    return 0
  fi
  bash "$REPO/deploy/mac/package.sh"
  SRC="$REPO/dist/EbbTop.app"
  [[ -e "$SRC" ]] || wz_die "package.sh finished but produced no $SRC"
}

unpack_zip() { # $1 zip -> prints the .app path inside a temp dir
  local zip="$1" tmp app
  tmp="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-install.XXXXXX")"
  if command -v ditto >/dev/null 2>&1; then
    ditto -x -k "$zip" "$tmp"
  else
    unzip -q "$zip" -d "$tmp"
  fi
  app="$(find "$tmp" -maxdepth 2 -name "$APP_NAME" -type d | head -n 1)"
  [[ -n "$app" ]] || wz_die "no $APP_NAME inside $zip"
  printf '%s' "$app"
}

install() {
  resolve_app_dir
  wz_step "preflight"
  local present=() missing=() t
  for t in ditto codesign xattr security; do
    if command -v "$t" >/dev/null 2>&1; then present+=("$t"); else missing+=("$t"); fi
  done
  wz_ok "tools: ${present[*]:-none}"
  ((${#missing[@]})) && wz_warn "missing: ${missing[*]} (those steps are skipped or reported as unknown)"

  wz_step "source"
  prepare_source
  local src="$SRC"
  if [[ -d "$src" && "$src" == *.app ]]; then
    wz_ok "bundle: $src $( [[ -n "$(app_version "$src" | grep -v '?')" ]] && echo "v$(app_version "$src")" )"
  elif [[ -f "$src" && "$src" == *.zip ]]; then
    if ((WZ_DRY)); then
      wz_ok "zip: $src (would unpack and inspect)"
    else
      src="$(unpack_zip "$src")"
      wz_ok "bundle from zip: $src v$(app_version "$src")"
    fi
  else
    wz_die "$src is neither a .app bundle nor a .zip"
  fi

  wz_step "destination"
  if [[ -z "$APP_DIR" ]]; then APP_DIR="$( [[ -w /Applications ]] && echo /Applications || echo "$HOME/Applications" )"; fi
  wz_ask APP_DIR "install location" "$APP_DIR"
  local app="$APP_DIR/$APP_NAME"
  wz_info "target: $app"
  if [[ -d "$app" ]]; then
    wz_info "replaces the installed copy (v$(app_version "$app"))"
  fi

  wz_step "plan"
  wz_info "1. quit EbbTop if it is running"
  wz_info "2. copy the bundle to $app (old copy removed after the new one is written)"
  wz_info "3. clear the quarantine flag so it opens without a warning"
  wz_info "4. verify signature, version and executable bit"
  ((OPEN_APP)) && wz_info "5. launch EbbTop"
  if ! wz_confirm "install EbbTop.app to $APP_DIR?" y; then
    wz_info "cancelled, nothing changed"
    return 0
  fi
  if ((WZ_DRY)); then
    wz_info "dry run: nothing copied"
    return 0
  fi

  quit_app
  mkdir -p "$APP_DIR"
  rm -rf "$app.new"
  if command -v ditto >/dev/null 2>&1; then
    ditto "$src" "$app.new"
  else
    cp -R "$src" "$app.new"
  fi
  rm -rf "$app"
  mv "$app.new" "$app"
  if command -v xattr >/dev/null 2>&1; then
    xattr -dr com.apple.quarantine "$app" 2>/dev/null || true
  fi

  wz_step "verify"
  [[ -d "$app" ]] && wz_ok "bundle present: $app" || { wz_err "bundle missing after copy"; return 1; }
  [[ -x "$app/Contents/MacOS/ebbtop" ]] && wz_ok "executable present" || wz_err "no Contents/MacOS/ebbtop"
  wz_kv version "$(app_version "$app")"
  wz_kv signing "$(sign_state "$app")"
  if command -v codesign >/dev/null 2>&1; then
    codesign --verify --deep --strict "$app" >/dev/null 2>&1 &&
      wz_ok "codesign verifies" ||
      wz_warn "codesign does not verify: first launch may need System Settings > Privacy & Security > Open Anyway"
  fi
  if ((OPEN_APP)); then
    wz_step "launch"
    open "$app" && wz_ok "launched" || wz_warn "could not launch: open $app"
    wz_info "onboarding asks for the host URL (ws://<host>:45976/ws) and that host's token"
  fi
}

uninstall() {
  resolve_app_dir
  local app="$APP_DIR/$APP_NAME" names_list names
  # The app may sit in the other Applications folder (the standard one when it
  # is writable, the user one otherwise): uninstall wherever it actually is.
  if [[ ! -d "$app" ]]; then
    for alt in /Applications "$HOME/Applications"; do
      if [[ -d "$alt/$APP_NAME" ]]; then
        wz_note "found it at $alt/$APP_NAME instead of $app"
        app="$alt/$APP_NAME"
        break
      fi
    done
  fi
  # Read the source list before anything is deleted: the keychain account name
  # for each stored token comes from config.json.
  names_list="$(source_names || true)"
  wz_step "what is installed"
  local found=0
  if [[ -d "$app" ]]; then found=1; wz_kv bundle "$app v$(app_version "$app")"; fi
  [[ -d "$SUPPORT" ]] && { found=1; wz_kv state "$SUPPORT"; }
  if [[ ! -d "$app" && ! -d "$SUPPORT" ]]; then
    wz_ok "nothing to remove (no $app, no $SUPPORT)"
    return 0
  fi
  if ((!found)); then wz_info "no EbbTop.app found at $app"; fi
  names="$(printf '%s' "$names_list" | paste -sd, - || true)"
  [[ -n "$names" ]] && wz_kv sources "$names"
  wz_info "stored tokens: $(keychain_count) in the login keychain (service $KEYCHAIN_SERVICE)"

  wz_step "plan"
  wz_info "1. quit EbbTop if it is running"
  wz_info "2. remove $app"
  if ((PURGE)); then
    wz_warn "3. remove $SUPPORT: config, log and display settings"
    wz_warn "4. delete the stored token of every source from the login keychain"
  else
    wz_info "3. keep $SUPPORT (config, log and tokens are reused by the next install)"
  fi
  if ! wz_confirm "uninstall EbbTop.app from this Mac?" n; then
    wz_info "cancelled, nothing changed"
    return 0
  fi
  if ((WZ_DRY)); then
    wz_info "dry run: nothing removed"
    return 0
  fi

  quit_app
  wz_step "remove"
  if [[ -d "$app" ]]; then
    rm -rf "$app" || wz_warn "could not remove $app (needs admin: sudo rm -rf \"$app\")"
  fi
  if ((PURGE)); then
    [[ -d "$SUPPORT" ]] && rm -rf "$SUPPORT"
    if command -v security >/dev/null 2>&1; then
      if [[ -z "$names_list" ]]; then
        wz_warn "no source names to work from: delete the '$KEYCHAIN_SERVICE' items in Keychain Access by hand"
      fi
      while IFS= read -r n; do
        [[ -z "$n" ]] && continue
        if keychain_present "$n"; then
          security delete-generic-password -s "$KEYCHAIN_SERVICE" -a "$n" >/dev/null 2>&1 &&
            wz_ok "deleted keychain token for source '$n'" ||
            wz_warn "could not delete the keychain item for '$n' (Keychain Access)"
        fi
      done <<<"$names_list"
    else
      wz_warn "security tool missing: delete the '$KEYCHAIN_SERVICE' items in Keychain Access"
    fi
  fi

  wz_step "verify"
  [[ -e "$app" ]] && wz_err "bundle still present: $app" || wz_ok "bundle removed"
  if pgrep -x ebbtop >/dev/null 2>&1; then
    wz_err "EbbTop is still running (pid $(pgrep -x ebbtop | head -n 1))"
  else
    wz_ok "no EbbTop process"
  fi
  if ((PURGE)); then
    [[ -e "$SUPPORT" ]] && wz_warn "$SUPPORT still present" || wz_ok "$SUPPORT removed"
    wz_kv keychain "$(keychain_count) token(s) still found"
  elif [[ -d "$SUPPORT" ]]; then
    wz_note "kept: $SUPPORT (config, log, tokens). Remove it with: bash deploy/mac/setup.sh uninstall --purge"
  fi
}

menu() {
  local choice
  while :; do
    resolve_app_dir
    wz_banner "EbbTop Mac wizard" "$(sw_vers -productVersion 2>/dev/null || echo macOS)   $(uname -m)"
    if [[ -d "$APP_DIR/$APP_NAME" ]]; then
      wz_info "state: installed at $APP_DIR/$APP_NAME v$(app_version "$APP_DIR/$APP_NAME")"
    else
      wz_info "state: not installed"
    fi
    wz_menu choice "what do you want to do?" \
      "install or update EbbTop.app" \
      "uninstall EbbTop.app" \
      "show status" \
      "quit"
    case "$choice" in
      1) install || true ;;
      2) uninstall || true ;;
      3) status ;;
      4) exit 0 ;;
    esac
    wz_pause
  done
}

while (($#)); do
  case "$1" in
    install | uninstall | status | menu) ACTION="$1"; shift ;;
    --from) FROM="${2:?--from needs a path}"; shift 2 ;;
    --from=*) FROM="${1#*=}"; shift ;;
    --app-dir) APP_DIR="${2:?--app-dir needs a path}"; shift 2 ;;
    --app-dir=*) APP_DIR="${1#*=}"; shift ;;
    --purge) PURGE=1; shift ;;
    --no-open) OPEN_APP=0; shift ;;
    --yes | -y) WZ_YES=1; shift ;;
    --dry-run | -n) WZ_DRY=1; shift ;;
    --help | -h) usage; exit 0 ;;
    *) wz_die "unknown argument: $1 (try --help)" ;;
  esac
done

# Platform guard after argument parsing so --help works anywhere.
if [[ "$(uname -s)" != Darwin ]]; then
  wz_die "this wizard runs on macOS (it needs ditto, codesign and the keychain). On Linux use deploy/agent-wizard.sh; the Windows client uses deploy/windows/setup.ps1."
fi

if [[ -z "$ACTION" ]]; then
  if [[ -t 0 && -t 1 ]]; then
    menu
    exit 0
  fi
  ACTION=status
fi

case "$ACTION" in
  install) install ;;
  uninstall) uninstall ;;
  status) status ;;
  menu) menu ;;
esac
