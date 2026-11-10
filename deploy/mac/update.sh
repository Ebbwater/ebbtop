#!/usr/bin/env bash
# EbbTop Mac update: one command, no fat. Run ON the Mac from the repo root.
#   bash deploy/mac/update.sh
# Does: fast-forward pull, ONE release build, bundle refresh. No cargo check,
# no test suite, no double build. Config + Keychain token are untouched.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
git pull --ff-only
cargo build --release --locked -p ebbtop-gui
bash deploy/mac/package.sh --skip-build
echo "updated: dist/EbbTop.app (quit EbbTop, open it or copy to /Applications)"
