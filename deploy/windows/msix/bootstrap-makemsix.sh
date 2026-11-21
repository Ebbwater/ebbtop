#!/usr/bin/env bash
# Build Microsoft's cross-platform MSIX SDK (`makemsix`) on Linux.
#
#   bash deploy/windows/msix/bootstrap-makemsix.sh [install-dir]
#
# The Store package is packed with Microsoft's own tooling rather than a
# home-grown zip writer, so the block map, content types and manifest
# validation come from the same code the ecosystem uses. The SDK is built
# from a pinned upstream commit; the archive digest below pins the bytes we
# compiled, so a hostile or accidental upstream rewrite cannot slip into a
# package unnoticed.
#
# Needs: git, cmake >= 3.29, a C/C++ toolchain, make. All present on a
# GitHub-hosted ubuntu-latest image. Prints the makemsix path on success.
set -euo pipefail

MSIX_SDK_COMMIT="25a65f5c1690930813bcc10cdf1d59fa865f2bb1"
# Digest of the source worktree compiled here, outside .git:
#   find . -path ./.git -prune -o -type f -print0 | sort -z \
#     | xargs -0 sha256sum | sha256sum
# Override only when deliberately bumping MSIX_SDK_COMMIT.
MSIX_SDK_SHA256="${EBBTOP_MSIX_SDK_SHA256:-fe9f94d3058f5a4993d2f0e889ae04efc44fe93066216866cf33b9b3de3123ec}"
INSTALL_DIR="${1:-${RUNNER_TEMP:-/tmp}/ebbtop-msix-sdk}"

command -v cmake >/dev/null || { echo "cmake is required (>= 3.29)" >&2; exit 1; }
command -v git >/dev/null || { echo "git is required" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf -- "$WORK"' EXIT

echo "== fetching msix-packaging $MSIX_SDK_COMMIT"
git -C "$WORK" init -q sdk
git -C "$WORK/sdk" remote add origin https://github.com/microsoft/msix-packaging.git
git -C "$WORK/sdk" fetch -q --depth 1 origin "$MSIX_SDK_COMMIT"
git -C "$WORK/sdk" checkout -q FETCH_HEAD
git -C "$WORK/sdk" submodule update --init --recursive --depth 1 -q

if [[ -n "$MSIX_SDK_SHA256" ]]; then
  # Hash the exact worktree bytes we compile, so the digest is checkable.
  actual="$(cd "$WORK/sdk" && find . -path ./.git -prune -o -type f -print0 \
    | sort -z | xargs -0 sha256sum | sha256sum | cut -d' ' -f1)"
  [[ "$actual" == "$MSIX_SDK_SHA256" ]] || {
    echo "msix-packaging worktree digest $actual != pinned $MSIX_SDK_SHA256" >&2
    exit 1
  }
fi

echo "== configuring (pack support on, samples/tests off)"
# The upstream makelinux.sh hardcodes clang via cmake/linux.cmake. The hosted
# image has clang, so use the upstream script's own toolchain file there;
# fall back to the default compiler when clang is absent (documented in
# docs/windows-store.md, not a supported release path).
TOOLCHAIN=()
CLANG_ENV=()
if command -v clang >/dev/null && command -v clang++ >/dev/null; then
  TOOLCHAIN=(-DCMAKE_TOOLCHAIN_FILE="$WORK/sdk/cmake/linux.cmake")
else
  CLANG_ENV=(CC=gcc CXX=g++)
fi
mkdir -p "$WORK/build"
(
  cd "$WORK/build"
  env "${CLANG_ENV[@]}" cmake -DCMAKE_BUILD_TYPE=Release \
    "${TOOLCHAIN[@]}" \
    -DSKIP_BUNDLES=on -DUSE_VALIDATION_PARSER=on \
    -DMSIX_PACK=on -DMSIX_SAMPLES=off -DMSIX_TESTS=off -DLINUX=on \
    "$WORK/sdk" >/dev/null
  make -j"$(nproc)" makemsix >/dev/null
)

mkdir -p "$INSTALL_DIR/bin" "$INSTALL_DIR/lib"
cp "$WORK/build/bin/makemsix" "$INSTALL_DIR/bin/"
cp "$WORK/build/lib/libmsix.so" "$INSTALL_DIR/lib/"
echo "makemsix: $INSTALL_DIR/bin/makemsix"
LD_LIBRARY_PATH="$INSTALL_DIR/lib" "$INSTALL_DIR/bin/makemsix" -? | head -3
