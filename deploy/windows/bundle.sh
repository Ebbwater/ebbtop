#!/usr/bin/env bash
# Bundle verified Windows release assets for first-time installation.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "${1:?usage: bundle.sh <dist directory>}"
sha256sum --strict -c SHA256SUMS

archive=EbbTop-Windows-x64.zip
tmp="$(mktemp -d .windows-bundle.XXXXXXXX)"
trap 'rm -rf -- "$tmp"' EXIT
zip -q -MM -X -j "$tmp/$archive" ebbtop.exe ebbtop-probe.exe install.ps1 setup.ps1 \
    LICENSE-OFL.txt THIRD-PARTY-NOTICES SHA256SUMS BUILD "$SCRIPT_DIR/Install EbbTop.cmd"
unzip -tq "$tmp/$archive"
mv -f -- "$tmp/$archive" "$archive"
