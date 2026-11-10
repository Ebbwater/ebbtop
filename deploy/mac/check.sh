#!/usr/bin/env bash
# Mac client check + build, run ON the Mac (Apple Silicon, aarch64).
# Same workspace as Windows/Linux: no core-logic change, no Windows-only APIs
# in ebbtop-core (tokio/tungstenite, prost, serde, flate2 — all portable). The
# only OS surface is `keyring` v4 (macOS Keychain / Windows Credential Manager
# / Linux Secret Service behind one API), `dirs` paths, and eframe/winit
# backends; `#![cfg_attr(all(windows, ...), windows_subsystem = "windows")]`
# in ebbtop-gui is a no-op on macOS.
#
# Usage (on the Mac):
#   git clone <your-fork-or-mirror-url>
#   cd ebbtop
#   bash deploy/mac/check.sh            # check + test + release build
#   bash deploy/mac/check.sh --check-only
#
# Builds no .dmg by design. For day-to-day runs use ./target/release/ebbtop
# directly; for a double-clickable bundle: bash deploy/mac/package.sh
# (assembles dist/EbbTop.app with the brand-pack icon).
set -euo pipefail

CHECK_ONLY=false
if [[ "${1:-}" == "--check-only" ]]; then CHECK_ONLY=true; fi

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"

echo "== host"
uname -sm
if [[ "$(uname -m)" == "arm64" ]]; then
  TARGET=aarch64-apple-darwin
else
  TARGET=x86_64-apple-darwin
fi
echo "target: $TARGET"

echo "== rustup (install if missing)"
if ! command -v cargo >/dev/null; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1091
  export PATH="$HOME/.cargo/bin:$PATH"
fi
rustup toolchain install stable --profile minimal
rustup target add "$TARGET"
cargo --version

echo "== cargo check (type-check for $TARGET, no link)"
cargo check --locked --target "$TARGET" --workspace

echo "== cargo test --workspace (native; expect no-op pass)"
cargo test --locked --workspace

if [[ "$CHECK_ONLY" == true ]]; then
  echo "check-only: done"
  exit 0
fi

echo "== release build (ebbtop + ebbtop-probe)"
cargo build --release --locked -p ebbtop-gui -p ebbtop-core
echo "built: target/release/ebbtop"
echo "probe: target/release/ebbtop-probe --seconds 60 --token-file <file>"

echo "== first run (once)"
echo "1. fetch the token over the tailnet and copy it (never written to disk):"
echo "   ssh <user>@<agent-host> cat services/ebbtop-agent/ebbtop.token | pbcopy"
echo "2. launch the GUI:  ./target/release/ebbtop"
echo "3. the onboarding wizard takes the host URL + token; the token is stored"
echo "   in the macOS Keychain (service 'ebbtop', one entry per host). Config"
echo "   + log land in ~/Library/Application Support/ebbtop/ (config.json,"
echo "   ebbtop.log)."
echo "4. verify headless first if you like:"
echo "   pbpaste > ~/ebbtop.token && chmod 600 ~/ebbtop.token"
echo "   ./target/release/ebbtop-probe --url ws://<agent-host>:45976/ws --seconds 60 --token-file ~/ebbtop.token"
echo "   rm ~/ebbtop.token"
