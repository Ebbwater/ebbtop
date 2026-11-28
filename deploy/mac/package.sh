#!/usr/bin/env bash
# EbbTop.app bundle builder, run ON the Mac (not on the Linux build host).
# Builds the release binary, converts the brand-pack PNG ladder to
# EbbTop.icns with iconutil, and assembles dist/EbbTop.app so EbbTop
# launches from Finder / the Dock with no terminal. It then zips the bundle
# to dist/EbbTop.app.zip — the exact asset name the desktop updater on
# macOS downloads and verifies against the release's SHA256SUMS.
#
# Usage (on the Mac, from the repo root):
#   bash deploy/mac/package.sh
# Result: dist/EbbTop.app and dist/EbbTop.app.zip (+ .sha256). `open
# dist/EbbTop.app`, or drag it to /Applications. First launch may need
# System Settings > Privacy & Security > Open Anyway (the bundle is
# ad-hoc signed; macOS Sequoia removed the right-click > Open bypass).
#
# No network, no secret: reads only the repo's brand PNGs and the
# locally built binary. Never touches the credential store.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "package.sh runs on macOS (needs iconutil). On Linux, use deploy/cross-build-windows.sh for Windows." >&2
  exit 1
fi
for tool in cargo iconutil; do
  command -v "$tool" >/dev/null || { echo "missing: $tool" >&2; exit 1; }
done

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO"
VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')"
[[ -n "$VERSION" ]] || { echo "could not read version from Cargo.toml" >&2; exit 1; }

SKIP_BUILD=false
if [[ "${1:-}" == "--skip-build" ]]; then SKIP_BUILD=true; fi
if [[ "$SKIP_BUILD" == true ]]; then
  echo "== reuse existing release binary (skip build)"
else
  echo "== release build (ebbtop $VERSION)"
  cargo build --release --locked -p ebbtop-gui
fi
BIN="target/release/ebbtop"
# Unique build number per packaging run: IconServices keys cached Dock icons
# by bundle id + version, so a fresh build number busts the stale-icon cache
# (lsregister + `killall Dock` alone do not).
BUILD="$(git rev-list --count HEAD 2>/dev/null || date +%s)"
echo "build number: $BUILD"

PNG="crates/ebbtop-gui/assets/brand/png"
for s in 16 32 64 128 256 512 1024; do
  [[ -f "$PNG/ebbtop-app-icon-$s.png" ]] || { echo "missing $PNG/ebbtop-app-icon-$s.png" >&2; exit 1; }
done

APP="dist/EbbTop.app"
CONTENTS="$APP/Contents"
rm -rf "$APP"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Resources"

echo "== binary"
cp "$BIN" "$CONTENTS/MacOS/ebbtop"
chmod +x "$CONTENTS/MacOS/ebbtop"

echo "== icon (.icns from the hinted PNG ladder)"
ICONSET="$(mktemp -d)/EbbTop.iconset"
mkdir -p "$ICONSET"
cp "$PNG/ebbtop-app-icon-16.png"   "$ICONSET/icon_16x16.png"
cp "$PNG/ebbtop-app-icon-32.png"   "$ICONSET/icon_16x16@2x.png"
cp "$PNG/ebbtop-app-icon-32.png"   "$ICONSET/icon_32x32.png"
cp "$PNG/ebbtop-app-icon-64.png"   "$ICONSET/icon_32x32@2x.png"
cp "$PNG/ebbtop-app-icon-128.png"  "$ICONSET/icon_128x128.png"
cp "$PNG/ebbtop-app-icon-256.png"  "$ICONSET/icon_128x128@2x.png"
cp "$PNG/ebbtop-app-icon-256.png"  "$ICONSET/icon_256x256.png"
cp "$PNG/ebbtop-app-icon-512.png"  "$ICONSET/icon_256x256@2x.png"
cp "$PNG/ebbtop-app-icon-512.png"  "$ICONSET/icon_512x512.png"
cp "$PNG/ebbtop-app-icon-1024.png" "$ICONSET/icon_512x512@2x.png"
echo "== rounded badge mask (mac-only; the vendored pack stays square)"
python3 "$REPO/deploy/mac/round_icon.py" "$ICONSET"/icon_*.png
iconutil -c icns "$ICONSET" -o "$CONTENTS/Resources/EbbTop.icns"
rm -rf "$(dirname "$ICONSET")"

echo "== Info.plist"
cat > "$CONTENTS/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>EbbTop</string>
  <key>CFBundleDisplayName</key>
  <string>EbbTop</string>
  <key>CFBundleIdentifier</key>
  <string>net.ebbwater.ebbtop</string>
  <key>CFBundleVersion</key>
  <string>$BUILD</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleExecutable</key>
  <string>ebbtop</string>
  <key>CFBundleIconFile</key>
  <string>EbbTop</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
EOF

if codesign -s - --deep --force "$APP" 2>/dev/null; then
  echo "ad-hoc signed"
else
  echo "codesign skipped (first launch: System Settings > Privacy & Security > Open Anyway)"
fi

# --- the release artifact ----------------------------------------------------
# The updater on macOS downloads exactly "EbbTop.app.zip" from the release and
# verifies it against SHA256SUMS, so the name is part of the contract. ditto,
# not zip: it preserves the bundle's resource forks and symlinks and keeps the
# top-level directory, so the unzipped result is EbbTop.app/, ready to move to
# /Applications.
ZIP="dist/EbbTop.app.zip"
echo "== release zip"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"

# Prove the artifact here, where it was built; assemble.sh re-checks the same
# rules against the hosted run's bytes before anything is published.
unzip -tq "$ZIP" > /dev/null || { echo "the zip does not unzip cleanly" >&2; exit 1; }
first="$(unzip -Z1 "$ZIP" | head -n 1)"
[[ "$first" == "EbbTop.app/"* ]] || { echo "the zip's first member is $first, not EbbTop.app/" >&2; exit 1; }
for part in "EbbTop.app/Contents/Info.plist" "EbbTop.app/Contents/MacOS/ebbtop" "EbbTop.app/Contents/Resources/EbbTop.icns"; do
  unzip -Z1 "$ZIP" | grep -qx "$part" || { echo "the zip is missing $part" >&2; exit 1; }
done
if unzip -Z1 "$ZIP" | grep -Eq '(\.env$|\.token$|\.pem$|\.key$)'; then
  echo "the zip carries a secret-shaped member" >&2
  exit 1
fi
magic="$(unzip -p "$ZIP" "EbbTop.app/Contents/MacOS/ebbtop" | head -c 4 | od -An -tx1 | tr -d ' \n')"
[[ "$magic" == "cffaedfe" ]] || { echo "the packaged binary is not an arm64 Mach-O (magic $magic)" >&2; exit 1; }
short="$(plutil -extract CFBundleShortVersionString raw -o - "$CONTENTS/Info.plist")"
[[ "$short" == "$VERSION" ]] || { echo "Info.plist CFBundleShortVersionString is $short, not $VERSION" >&2; exit 1; }

( cd dist && shasum -a 256 "$(basename "$ZIP")" > "$(basename "$ZIP").sha256" )
echo "built: $APP (EbbTop $VERSION)"
echo "artifact: $ZIP"
cat "$ZIP.sha256"
echo "launch: open \"$APP\"   (or drag it to /Applications)"
