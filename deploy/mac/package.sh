#!/usr/bin/env bash
# EbbTop.app bundle builder, run ON the Mac (not on the Linux build host).
# Builds the release binary, converts the brand-pack PNG ladder to
# EbbTop.icns with iconutil, and assembles dist/EbbTop.app so EbbTop
# launches from Finder / the Dock with no terminal.
#
# Usage (on the Mac, from the repo root):
#   bash deploy/mac/package.sh
# Result: dist/EbbTop.app — `open dist/EbbTop.app`, or drag it to
# /Applications. First launch may need right-click > Open (unsigned
# local build; the script ad-hoc signs it to reduce friction).
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
  echo "codesign skipped (first launch: right-click > Open)"
fi

echo "built: $APP (EbbTop $VERSION)"
echo "launch: open \"$APP\"   (or drag it to /Applications)"
