#!/usr/bin/env bash
# Build and pack the Microsoft Store MSIX for EbbTop.
#
#   bash deploy/windows/msix/package-msix.sh
#
# Output: dist/msix/EbbTop-<version>-x64.msix (+ .sha256 and BUILD).
# Runs on Linux (the hosted ubuntu-latest job) and uses only the pinned
# llvm-mingw cross-build plus Microsoft's cross-platform MSIX SDK
# (`makemsix`, from microsoft/msix-packaging, built by bootstrap-makemsix.sh).
#
# The package is deliberately UNSIGNED. The Microsoft Store re-signs every
# submitted MSIX with a Microsoft certificate during publishing, so this
# repository needs no CA certificate. A self-signed certificate is only used
# inside the WACK gate to side-load the package on a throwaway runner.
#
# The manifest is deploy/windows/msix/AppxManifest.xml, with the Partner
# Center identity (Name Ebbwater.EbbTop, Publisher CN=32700780-...). It is
# used verbatim; the only check is that its four-part Identity/Version agrees
# with Cargo.toml, so a version bump is an explicit reviewable edit.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO"
HERE="$REPO/deploy/windows/msix"
OUT="$REPO/dist/msix"
STAGE="$OUT/stage"

VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
# MSIX Identity/Version is a four-part version: x.y.z.0 here.
case "$VERSION" in
  *.*.*) MSIX_VERSION="$VERSION.0" ;;
  *.*)   MSIX_VERSION="$VERSION.0.0" ;;
  *)     MSIX_VERSION="$VERSION.0.0.0" ;;
esac

MAKEMSIX="${EBBTOP_MAKEMSIX:-$(command -v makemsix || true)}"
[[ -n "$MAKEMSIX" && -x "$MAKEMSIX" ]] || {
  echo "makemsix not found; run deploy/windows/msix/bootstrap-makemsix.sh or set EBBTOP_MAKEMSIX" >&2
  exit 1
}
# makemsix links libmsix.so; it lives beside the binary in the SDK build.
MAKEMSIX_DIR="$(cd "$(dirname "$MAKEMSIX")" && pwd)"
export LD_LIBRARY_PATH="$MAKEMSIX_DIR/../lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

# 0. The manifest is the source of truth for identity and version. Refuse to
#    pack when it disagrees with Cargo.toml: a package whose Identity/Version
#    drifted from the binary in it cannot be published.
MANIFEST_VERSION="$(python3 - "$HERE/AppxManifest.xml" <<'PY'
import sys
import xml.etree.ElementTree as ET
ns = {"f": "http://schemas.microsoft.com/appx/manifest/foundation/windows10"}
ident = ET.parse(sys.argv[1]).getroot().find("f:Identity", ns)
print(ident.get("Version"))
PY
)"
[[ "$MANIFEST_VERSION" == "$MSIX_VERSION" ]] || {
  echo "AppxManifest.xml Identity/Version $MANIFEST_VERSION != workspace $MSIX_VERSION" >&2
  echo "bump Identity/Version in $HERE/AppxManifest.xml with the release" >&2
  exit 1
}

# 1. Build the GUI with the `store` feature through the existing pinned
#    llvm-mingw recipe. EBBTOP_CARGO_FEATURES is the only knob that differs
#    from a normal release build; the feature-off bytes are untouched.
echo "== building ebbtop.exe with --features store (llvm-mingw cross-build)"
EBBTOP_CARGO_FEATURES=store bash deploy/cross-build-windows.sh >/dev/null

# 2. Stage the package contents.
echo "== staging $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/Assets"
cp dist/ebbtop.exe "$STAGE/ebbtop.exe"
cp "$HERE/AppxManifest.xml" "$STAGE/AppxManifest.xml"
python3 "$HERE/make-assets.py" "$STAGE" >/dev/null

# 3. Cross-check the staged exe against the manifest: it must be the store
#    build, and its VERSIONINFO product name/version must match what the
#    manifest claims. A mismatch here would be a bad submission.
grep -q 'Executable="ebbtop.exe"' "$STAGE/AppxManifest.xml"
python3 deploy/windows/versioninfo.py \
  --require-product-name EbbTop \
  --require-version "$VERSION" \
  "$STAGE/ebbtop.exe" >/dev/null
if [[ -n "$(strings -a "$STAGE/ebbtop.exe" | grep -F 'install.ps1')" ]]; then
  echo "store build still references install.ps1 (updater not compiled out)" >&2
  exit 1
fi

# 4. Pack. makemsix writes AppxBlockMap.xml and [Content_Types].xml itself.
MSIX="$OUT/EbbTop-$VERSION-x64.msix"
echo "== packing $MSIX"
rm -f "$MSIX"
"$MAKEMSIX" pack -d "$STAGE" -p "$MSIX" >/dev/null

# 5. Verify the package the way the Store and Windows will read it, then
#    record its identity.
python3 "$HERE/verify-msix.py" "$MSIX" --expect-version "$MSIX_VERSION"

(cd "$OUT" && sha256sum "$(basename "$MSIX")" > "$(basename "$MSIX").sha256")
{
  echo "version $VERSION"
  echo "msix $MSIX_VERSION"
  echo "identity Ebbwater.EbbTop"
  echo "publisher CN=32700780-5AAE-4D15-A591-B49543B86C16"
  echo "feature store"
  echo "commit $(git rev-parse --short HEAD)$(git diff --quiet || echo -dirty)"
} > "$OUT/BUILD"
sha256sum "$MSIX"
