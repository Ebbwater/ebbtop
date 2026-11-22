#!/usr/bin/env bash
# Build Microsoft's cross-platform MSIX SDK (`makemsix`) on Linux.
#
#   bash deploy/windows/msix/bootstrap-makemsix.sh [install-dir]
#   bash deploy/windows/msix/bootstrap-makemsix.sh --digest-only <source-tree>
#
# The Store package is packed with Microsoft's own tooling rather than a
# home-grown zip writer, so the block map, content types and manifest
# validation come from the same code the ecosystem uses. The SDK is built
# from a pinned upstream commit; the worktree digest below pins the bytes we
# compiled, so a hostile or accidental upstream rewrite cannot slip into a
# package unnoticed.
#
# `--digest-only` hashes an already-fetched tree, prints the digest, compares
# it with the pinned value and stops. It touches nothing: no fetch, no build,
# no cmake requirement. Exit 0 means the tree matches the pin, exit 1 means
# the digest differs (both values are printed), exit 2 is a usage or tool
# error.
#
# Why the digest is computed under `LC_ALL=C`
# -------------------------------------------
# `sort` does not order bytes: it collates according to the locale in the
# environment. Under byte collation (the `C` locale) every uppercase name
# sorts before every lowercase name. Under the dictionary collation of
# `en_US.UTF-8`, case is a lower-order difference, so `./LICENSE` sorts among
# the lowercase `l` paths and the whole `./MsixCore/...` block sorts among
# `m`. The recipe hashes the *lines* produced by `sha256sum` in that order and
# hashes the result again, so the same files in a different order are a
# different digest.
#
# That is exactly the failure this script used to have. On
# https://github.com/Ebbwater/ebbtop/actions/runs/45068232751 (job
# 137349109443) the hosted job computed `5b850191…` (byte order, the value now
# pinned) while the script still pinned `49c28612…` (dictionary order), and
# the job stopped before `makemsix` ran. Reproduced from a pristine worktree
# of 25a65f5c… (21117 files, the four vendored-openssl gitlinks dropped):
# requesting `LC_ALL=en_US.UTF-8` moves 21112 of those 21117 paths to a
# different index and yields `49c28612…`, while `LC_ALL=C`, `LC_ALL=C.UTF-8`
# and a stripped environment all yield `5b850191…`. The collation is therefore
# forced for the whole pipeline below, so the caller's `LANG`, `LC_ALL`,
# `LC_COLLATE`, an unset locale or a locale that is not installed cannot
# change the value.
#
# Recipe (exactly what this script runs):
#   ( cd <tree> && export LC_ALL=C LANG=C LC_COLLATE=C
#     find . -path ./.git -prune -o -type f -print0 | sort -z \
#       | xargs -0 sha256sum | sha256sum | cut -d' ' -f1 )
#
# What is *not* the cause, stated so nobody re-derives it as fact:
#   - Filesystem readdir order cannot leak: `sort -z` orders the complete file
#     list before anything is hashed.
#   - The four `git rm --cached` deletions touch only the index, so they leave
#     the hashed worktree bytes unchanged (measured: same digest with and
#     without them).
#   - Submodule checkout differences are ruled out for this commit: it has no
#     `.gitmodules` at the worktree root, so `git submodule update --init
#     --recursive` is a no-op and cannot vary the file list.
#   - Coreutils implementation differences are ruled out: this host's `sort`
#     (uutils coreutils 0.10.0) and the hosted runner's GNU coreutils agree on
#     `5b850191…` under `C`.
#   - Left open: where the old `49c28612…` pin was first computed. A host whose
#     `sort` ran under an installed en_US-style collation is consistent with
#     the value, but that host's log was not available, so it is a hypothesis,
#     not a finding. It does not matter any more: only the forced-`C` value is
#     accepted below.
#
# Needs for a full build: git, cmake >= 3.29, a C/C++ toolchain, make. All
# present on a GitHub-hosted ubuntu-latest image. Prints the makemsix path on
# success.
set -euo pipefail

MSIX_SDK_COMMIT_DEFAULT="25a65f5c1690930813bcc10cdf1d59fa865f2bb1"
# Digest of the source worktree compiled here, outside .git, under forced `C`
# collation (recipe and evidence above). This value was reproduced from a
# fresh fetch of the pinned commit, not copied from a log.
MSIX_SDK_SHA256_PINNED="5b850191232ff0034df1eaf8fc09642651ca38535d0113138e70e914cffc93b6"
# Override only when deliberately bumping the commit: a digest override is
# accepted only together with a non-default EBBTOP_MSIX_SDK_COMMIT.
MSIX_SDK_COMMIT="${EBBTOP_MSIX_SDK_COMMIT:-$MSIX_SDK_COMMIT_DEFAULT}"

usage() {
  cat >&2 <<'USAGE'
usage: bash deploy/windows/msix/bootstrap-makemsix.sh [install-dir]
       bash deploy/windows/msix/bootstrap-makemsix.sh --digest-only <source-tree>

Fetches and builds the pinned MSIX SDK, checking the source worktree digest
before compiling. --digest-only only hashes an existing tree and compares it
with the pin (read-only).

Environment (deliberate commit bumps only):
  EBBTOP_MSIX_SDK_COMMIT  upstream commit to build instead of the pinned one
  EBBTOP_MSIX_SDK_SHA256  expected digest; refused while the pinned commit is
                          in use, and refused when set but empty
USAGE
}

DIGEST_ONLY=0
DIGEST_DIR=""
INSTALL_DIR="${RUNNER_TEMP:-/tmp}/ebbtop-msix-sdk"
INSTALL_DIR_SET=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --digest-only)
      [[ $# -ge 2 ]] || { echo "error: --digest-only needs a source tree" >&2; usage; exit 2; }
      DIGEST_ONLY=1
      DIGEST_DIR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "error: unknown option: $1" >&2
      usage
      exit 2
      ;;
    *)
      [[ "$INSTALL_DIR_SET" -eq 0 ]] || { echo "error: more than one install dir" >&2; usage; exit 2; }
      INSTALL_DIR="$1"
      INSTALL_DIR_SET=1
      shift
      ;;
  esac
done
if [[ $# -gt 0 ]]; then
  echo "error: unexpected argument: $1" >&2
  usage
  exit 2
fi

# Resolve the expected digest. There is no path that skips the comparison:
# the only override is a deliberate commit bump, and an empty value is an
# error rather than a disabled check.
if [[ -n "${EBBTOP_MSIX_SHA256+x}" ]]; then
  echo "error: EBBTOP_MSIX_SHA256 is not supported; it cannot disable the digest check" >&2
  echo "       use EBBTOP_MSIX_SDK_COMMIT + EBBTOP_MSIX_SDK_SHA256 for a deliberate bump" >&2
  exit 2
fi

MSIX_SDK_SHA256="$MSIX_SDK_SHA256_PINNED"
if [[ -n "${EBBTOP_MSIX_SDK_SHA256+x}" ]]; then
  if [[ -z "$EBBTOP_MSIX_SDK_SHA256" ]]; then
    echo "error: EBBTOP_MSIX_SDK_SHA256 is set but empty; an empty override cannot disable the digest check" >&2
    exit 2
  fi
  if [[ "$MSIX_SDK_COMMIT" == "$MSIX_SDK_COMMIT_DEFAULT" ]]; then
    echo "error: refusing EBBTOP_MSIX_SDK_SHA256 while building the built-in pinned commit" >&2
    echo "       $MSIX_SDK_COMMIT_DEFAULT" >&2
    echo "       a digest override is only valid together with a deliberate EBBTOP_MSIX_SDK_COMMIT bump" >&2
    exit 2
  fi
  MSIX_SDK_SHA256="$EBBTOP_MSIX_SDK_SHA256"
fi

# Digest of one worktree, under forced C collation for the whole pipeline.
msix_sdk_worktree_digest() {
  local root="$1"
  (
    cd "$root"
    export LC_ALL=C LANG=C LC_COLLATE=C LANGUAGE=
    find . -path ./.git -prune -o -type f -print0 \
      | sort -z \
      | xargs -0 sha256sum \
      | sha256sum \
      | cut -d' ' -f1
  )
}

# Print and compare. Exit status 0 on a match, 1 on a mismatch with both
# digests on stderr. Never returns 0 for a mismatch.
msix_sdk_check_digest() {
  local root="$1" actual="$2"
  if [[ "$actual" == "$MSIX_SDK_SHA256" ]]; then
    echo "msix-packaging worktree digest matches the pinned value: $actual"
    return 0
  fi
  {
    echo "error: msix-packaging worktree digest does not match the pinned value"
    echo "  tree:          $root"
    echo "  actual digest: $actual"
    echo "  pinned digest: $MSIX_SDK_SHA256"
  } >&2
  return 1
}

if [[ "$DIGEST_ONLY" -eq 1 ]]; then
  [[ -n "$DIGEST_DIR" ]] || { echo "error: --digest-only needs a source tree" >&2; usage; exit 2; }
  [[ -d "$DIGEST_DIR" ]] || { echo "error: not a directory: $DIGEST_DIR" >&2; exit 2; }
  actual="$(msix_sdk_worktree_digest "$DIGEST_DIR")"
  if ! msix_sdk_check_digest "$DIGEST_DIR" "$actual"; then
    exit 1
  fi
  exit 0
fi

command -v cmake >/dev/null || { echo "cmake is required (>= 3.29)" >&2; exit 2; }
command -v git >/dev/null || { echo "git is required" >&2; exit 2; }

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
# the worktree digest below is unaffected. (This is the *earlier* history: the
# pin before 49c28612… was taken over a worktree that still carried
# unfetchable vendored-openssl fixtures, which is why no clean environment
# could reproduce it. 49c28612… was a collation artefact instead; see the
# header.)
git -C "$WORK/sdk" rm -q --cached \
  lib/openssl/boringssl lib/openssl/krb5 \
  lib/openssl/pyca-cryptography lib/openssl/wycheproof 2>/dev/null || true
git -C "$WORK/sdk" submodule update --init --recursive --depth 1 -q

# Hash the exact worktree bytes we compile, so the digest is checkable. This
# runs before cmake/make: a mismatch aborts the build.
echo "== checking msix-packaging worktree digest"
actual="$(msix_sdk_worktree_digest "$WORK/sdk")"
if ! msix_sdk_check_digest "$WORK/sdk" "$actual"; then
  exit 1
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
