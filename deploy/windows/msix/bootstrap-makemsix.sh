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
# Pinned to the PRISTINE checkout of the pinned commit. The original pin
# (fe9f94d3...) was taken over a worktree that also carried unfetchable
# vendored-openssl test fixtures, so no clean environment could reproduce
# the build; this one is exactly what the fetch above produces.
MSIX_SDK_SHA256="${EBBTOP_MSIX_SDK_SHA256:-49c286124d14307acfc643b1a0163fe0df625767de14b5a2ce364b07ddb02a82}"
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
# The pinned commit carries four vendored-openssl test gitlinks (boringssl,
# krb5, pyca-cryptography, wycheproof) with no .gitmodules entries for them,
# so a plain `submodule update` dies with "No url found". They are OpenSSL's
# own test fixtures and makemsix does not use them: drop the stale gitlinks
# before updating submodules. `git rm --cached` touches only the index, so
# the worktree digest below is unaffected.
git -C "$WORK/sdk" rm -q --cached \
  lib/openssl/boringssl lib/openssl/krb5 \
  lib/openssl/pyca-cryptography lib/openssl/wycheproof 2>/dev/null || true
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
