#!/usr/bin/env bash
# Cross-build ebbtop.exe and ebbtop-probe.exe for Windows on Linux, into dist/.
# Toolchain: llvm-mingw 20261020 ucrt (sha256 55064b60...e8ae335), unpacked in
# ~/.local/opt/llvm-mingw. Static CRT so the exe needs only system DLLs.
# The .exe icon (crates/ebbtop-gui/build.rs, winresource) needs the rc step:
# llvm-windres/llvm-ar. winresource shells out to them for *-gnullvm targets,
# so PATH with $LM first is load-bearing, and WINDRES/AR are pinned to
# llvm-mingw below so the icon embeds even if a stale windres/ar shadows
# PATH. If llvm-windres ever stops accepting winresource's .rc (or the .o it
# emits), build.rs warns and ships without the exe icon instead of breaking
# the build: the window icon (main.rs, assets/ebbtop.rgba) still applies.
# Retry then: run `llvm-windres --version` + `cargo build -v` in this
# environment and check the windres error in the cargo:warning line.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
LM="$HOME/.local/opt/llvm-mingw/llvm-mingw-20261020-ucrt-ubuntu-22.04-x86_64/bin"
T=x86_64-pc-windows-gnullvm
export PATH="$LM:$PATH"
# rc step for crates/ebbtop-gui/build.rs (winresource, *.ico -> .rsrc).
# winresource honours WINDRES/AR over its own prefix inference; pin both
# to llvm-mingw so `cargo build` below embeds the icon deterministically.
export WINDRES="$LM/llvm-windres"
export AR="$LM/llvm-ar"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER=x86_64-w64-mingw32-clang
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C target-feature=+crt-static"
cd "$REPO"
rustup target add "$T" >/dev/null
cargo build --release --locked --target "$T" -p ebbtop-gui -p ebbtop-core
mkdir -p dist
cp "target/$T/release/ebbtop.exe" "target/$T/release/ebbtop-probe.exe" dist/
cp deploy/windows/install.ps1 dist/
# Refuse to ship an exe that needs a non-system DLL.
for f in dist/ebbtop.exe dist/ebbtop-probe.exe; do
  extra=$("$LM/llvm-objdump" -p "$f" | awk '/DLL Name/{print $3}' | sort -u |
    grep -viE '^(api-ms-win-|advapi32|bcryptprimitives|combase|dwmapi|gdi32|imm32|kernel32|ntdll|ole32|oleaut32|opengl32|shell32|uiautomationcore|user32|uxtheme|ws2_32)' || true)
  [[ -z "$extra" ]] || { echo "$f imports non-system DLLs: $extra"; exit 1; }
done
(cd dist && sha256sum ebbtop.exe ebbtop-probe.exe install.ps1 > SHA256SUMS)
{
  echo "version $(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)"
  echo "commit $(git rev-parse --short HEAD)$(git diff --quiet || echo -dirty)"
} > dist/BUILD
cat dist/SHA256SUMS dist/BUILD
