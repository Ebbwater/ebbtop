#!/usr/bin/env bash
# Cross-build the Windows executables and installer ZIP on Linux, into dist/.
# The packaging step uses Info-ZIP zip and unzip.
# Toolchain: llvm-mingw 20261020 ucrt (sha256 55064b60...e8ae335), unpacked in
# ~/.local/opt/llvm-mingw. Static CRT so the exe needs only system DLLs.
# EBBTOP_LLVM_MINGW_BIN overrides that bin/ path; the hosted release workflow
# (.github/workflows/windows-build.yml) downloads the same pinned tarball and
# runs THIS script, so k-server and GitHub-hosted builds cannot drift.
# The .exe icon and the VERSIONINFO block (crates/*/build.rs, winresource)
# need the rc step: llvm-windres/llvm-ar. winresource shells out to them for
# *-gnullvm targets, so PATH with $LM first is load-bearing, and WINDRES/AR are
# pinned to llvm-mingw below so the resources embed even if a stale windres/ar
# shadows PATH. EBBTOP_REQUIRE_VERSIONINFO=1 turns a failed rc step into a hard
# build error, and the readback gate below re-parses the built PE: a packaged
# Windows build requires ProductName/ProductVersion on every binary, so a
# metadata-less exe must never be packaged. If llvm-windres ever stops accepting winresource's .rc (or the .o
# it emits), build fails here; retry with `llvm-windres --version` +
# `cargo build -v` in this environment and read the cargo:warning line.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
LM="${EBBTOP_LLVM_MINGW_BIN:-$HOME/.local/opt/llvm-mingw/llvm-mingw-20261020-ucrt-ubuntu-22.04-x86_64/bin}"
T=x86_64-pc-windows-gnullvm
[[ -x "$LM/llvm-windres" ]] || {
  echo "llvm-mingw binaries not found in $LM (expected llvm-mingw 20261020 ucrt; set EBBTOP_LLVM_MINGW_BIN to its bin/)" >&2
  exit 1
}
VERSION="$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
export PATH="$LM:$PATH"
# rc step for crates/ebbtop-gui/build.rs and crates/ebbtop-core/build.rs
# (winresource: *.ico + VERSIONINFO -> .rsrc). winresource honours WINDRES/AR
# over its own prefix inference; pin both to llvm-mingw so `cargo build` below
# embeds the resources deterministically.
export WINDRES="$LM/llvm-windres"
export AR="$LM/llvm-ar"
# Release builds must not silently ship an exe without VERSIONINFO.
export EBBTOP_REQUIRE_VERSIONINFO=1
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C target-feature=+crt-static"
# Optional feature passthrough: package-msix.sh sets EBBTOP_CARGO_FEATURES=store
# for the Microsoft Store build (updater compiled out). Empty means the regular
# release build with default features.
FEATURES="${EBBTOP_CARGO_FEATURES:-}"
cd "$REPO"
rustup target add "$T" >/dev/null
cargo build --release --locked --target "$T" -p ebbtop-gui -p ebbtop-core ${FEATURES:+--features "$FEATURES"}
mkdir -p dist
cp "target/$T/release/ebbtop.exe" "target/$T/release/ebbtop-probe.exe" dist/
cp deploy/windows/install.ps1 dist/
cp deploy/windows/setup.ps1 dist/
# The desktop redistributes the bundled Noto Sans CJK faces under the SIL
# OFL 1.1, so the exact checked-in license text ships beside the exe in
# every non-Store artifact (loose assets and the ZIP), and the checksums
# cover it.
cp crates/ebbtop-gui/assets/fonts/LICENSE-OFL.txt dist/
# License attribution for the crates compiled into the executables
# (cargo-about; regenerate in the same commit as any dependency change).
cp THIRD-PARTY-NOTICES dist/
# Refuse to ship an exe that needs a non-system DLL. bcrypt.dll is inbox
# since Vista (like the bcryptprimitives.dll already listed): rustls/ring
# uses BCryptGenRandom on Windows, so the updater's verified-download stack
# added it to the import table in 0.2.1 (v0.2.0 predates reqwest).
for f in dist/ebbtop.exe dist/ebbtop-probe.exe; do
  extra=$("$LM/llvm-objdump" -p "$f" | awk '/DLL Name/{print $3}' | sort -u |
    grep -viE '^(api-ms-win-|advapi32|bcrypt|bcryptprimitives|combase|dwmapi|gdi32|imm32|kernel32|ntdll|ole32|oleaut32|opengl32|shell32|uiautomationcore|user32|uxtheme|ws2_32)' || true)
  [[ -z "$extra" ]] || { echo "$f imports non-system DLLs: $extra"; exit 1; }
done
# VERSIONINFO readback: parse the PE resource directory of each built exe and
# require product name, product+file version, author and a per-binary
# description/original filename. Fails the build before any checksum is taken.
python3 deploy/windows/versioninfo.py \
  --require-product-name EbbTop \
  --require-version "$VERSION" \
  --require-company Ebbwater \
  --require-file-description "EbbTop desktop client" \
  dist/ebbtop.exe
python3 deploy/windows/versioninfo.py \
  --require-product-name EbbTop \
  --require-version "$VERSION" \
  --require-company Ebbwater \
  --require-file-description "EbbTop connection probe" \
  dist/ebbtop-probe.exe
# Checksums cover the bytes as built, so they follow the metadata gate.
(cd dist && sha256sum ebbtop.exe ebbtop-probe.exe install.ps1 setup.ps1 LICENSE-OFL.txt THIRD-PARTY-NOTICES > SHA256SUMS)
{
  echo "version $VERSION"
  echo "commit $(git rev-parse --short HEAD)$(git diff --quiet || echo -dirty)"
} > dist/BUILD
bash deploy/windows/bundle.sh dist
cat dist/SHA256SUMS dist/BUILD
