#!/usr/bin/env bash
# Regression proof for the Xerces configuration bootstrap-makemsix.sh uses.
#
#   bash deploy/windows/msix/test-sdk-config.sh
#   bash deploy/windows/msix/test-sdk-config.sh --negative-old-invocation
#   EBBTOP_MSIX_SDK_TREE=/tmp/msix-sdk bash deploy/windows/msix/test-sdk-config.sh
#
# bootstrap-makemsix.sh pins two Xerces cache options, -Dtranscoder=gnuiconv
# and -Dmessage-loader=inmemory. This script proves from machine output that
# the command line really selects that configuration and that the option it
# replaces is the one that breaks the C++14 build:
#
#   P1 the bootstrap's configure invocation still carries every option it had
#      (pack support, validation parser, skip bundles, Linux) and the two
#      repaired cache entries by name;
#   P2 the pinned tree's own detection logic is quoted with file:line, so the
#      rule the options exploit is readable in the tree, not assumed;
#   P3 negative control: with an ICU installation visible to CMake, the
#      pre-repair invocation selects transcoder=icu and RangeToken.cpp fails to
#      compile under C++14 on the C++17 facilities in the ICU header;
#   P4 the repaired invocation, in that same ICU-visible environment, selects
#      transcoder=gnuiconv, leaves XERCES_USE_TRANSCODER_ICU undefined and
#      compiles the same file under C++14;
#   P5 a -DCMAKE_CXX_STANDARD=17 cache entry does not change the compiler
#      standard (the tree sets CMAKE_CXX_STANDARD as a normal variable), which
#      is why the repair is a transcoder switch and not a blind standard bump;
#   P6 cmake accepted every -D option: no "Manually-specified variables were
#      not used" warning in the repaired configure;
#   P7 what this host cannot prove is reported as a GAP, never as a PASS.
#
# P1 covers done-when property (a), P2+P3 cover (b), P4 covers (c), P7 covers
# (d). Every PASS line is backed by a file:line citation, a CMakeCache.txt /
# config.h / flags.make entry, or a compiler exit status printed next to it.
#
# Evidence scope, stated up front so nothing here reads as more than it is:
#   * The ICU installation used for the P3/P4 differential is SYNTHETIC (real
#     ICU development headers are not installed on this host). The fixture is
#     generated under a temporary directory: a prefix with unicode/utypes.h,
#     unicode/uvernum.h, unicode/uchar.h and a unicode/char16ptr.h that uses
#     std::is_same_v and std::u16string_view, plus two stub shared libraries so
#     the tree's find_package(ICU) finds it. It exists so the pinned tree's own
#     detection and selection code can be exercised deterministically on any
#     host; it is not a reproduction of Ubuntu's ICU 74 headers.
#   * Everything else is real: the pinned upstream tree, its CMake logic, the
#     configure invocation extracted from bootstrap-makemsix.sh, the compiler
#     the bootstrap would use here, and the C++14 standard the tree selects.
#   * The hosted compile that this repair targets is GitHub run 45076924984,
#     job 137376128159 (source-integrity gate passed, Xerces RangeToken.cpp
#     failed against the image's installed ICU header). That run remains the
#     only evidence for the real ICU bytes and is re-run by the Captain, not
#     here.
#
# Options:
#   --negative-old-invocation  run only the citations (P2) and the old-invocation
#                              negative control (P3), printing the full evidence;
#                              exits 0 when the old invocation selects ICU and
#                              fails to compile, so a reviewer can see the two
#                              configurations differ in the direction claimed.
#   --sdk-tree DIR             use an already-fetched pinned tree (a .git
#                              checkout of MSIX_SDK_COMMIT). The worktree digest
#                              is still verified with the bootstrap's own
#                              --digest-only gate before anything is configured.
#
# Environment:
#   EBBTOP_MSIX_SDK_TREE       same as --sdk-tree.
#
# Without a tree the pinned commit is fetched exactly as the bootstrap fetches
# it (network needed only in that case) and its digest is checked with the
# bootstrap's gate. Nothing outside a temporary directory is written; the SDK
# tree is never modified.
set -uo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GATE="$HERE/bootstrap-makemsix.sh"
MSIX_SDK_COMMIT="25a65f5c1690930813bcc10cdf1d59fa865f2bb1"
HOSTED_RUN="https://github.com/Ebbwater/ebbtop/actions/runs/45076924984 job 137376128159"

MODE="all"
SDK_TREE="${EBBTOP_MSIX_SDK_TREE:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --negative-old-invocation) MODE="negative"; shift ;;
    --sdk-tree)
      [[ $# -ge 2 ]] || { echo "error: --sdk-tree needs a directory" >&2; exit 2; }
      SDK_TREE="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,70p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ -f "$GATE" ]] || { echo "error: bootstrap not found: $GATE" >&2; exit 2; }
for tool in cmake make git; do
  command -v "$tool" >/dev/null || { echo "error: $tool is required" >&2; exit 2; }
done

PASSED=0
FAILED=0
GAPS=0
pass() { PASSED=$((PASSED + 1)); printf '\nPASS %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '\nFAIL %s\n     %s\n' "$1" "$2"; }
gap()  { GAPS=$((GAPS + 1));   printf '\nGAP  %s\n' "$1"; }
note() { printf '     %s\n' "$1"; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-msix-config-test.XXXXXX")"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT

TREE=""
if [[ -n "$SDK_TREE" ]]; then
  [[ -d "$SDK_TREE" ]] || { echo "error: --sdk-tree is not a directory: $SDK_TREE" >&2; exit 2; }
  TREE="$(cd -- "$SDK_TREE" && pwd)"
  echo "== sdk tree: $TREE (supplied)"
else
  TREE="$WORK/sdk"
  echo "== fetching msix-packaging $MSIX_SDK_COMMIT into $TREE"
  git -C "$WORK" init -q sdk
  git -C "$TREE" remote add origin https://github.com/microsoft/msix-packaging.git
  git -C "$TREE" fetch -q --depth 1 origin "$MSIX_SDK_COMMIT"
  git -C "$TREE" checkout -q FETCH_HEAD
  # Same two steps as the bootstrap: drop the stale vendored-openssl test
  # gitlinks (index only) and update submodules.
  git -C "$TREE" rm -q --cached \
    lib/openssl/boringssl lib/openssl/krb5 \
    lib/openssl/pyca-cryptography lib/openssl/wycheproof 2>/dev/null || true
  git -C "$TREE" submodule update --init --recursive --depth 1 -q
fi

HEAD_SHA="$(git -C "$TREE" rev-parse HEAD 2>/dev/null || echo unknown)"
echo "== sdk commit: $HEAD_SHA"
echo "== verifying the tree with the bootstrap's own digest gate"
if DIGEST_OUT="$(bash "$GATE" --digest-only "$TREE" 2>&1)"; then
  note "$DIGEST_OUT"
else
  printf '%s\n' "$DIGEST_OUT" >&2
  echo "error: the supplied tree is not the pinned source; configuration evidence from it" >&2
  echo "       would not describe the bytes the bootstrap compiles. Fetch the pin instead." >&2
  exit 2
fi

# ---- the invocation under test ---------------------------------------------
# Configuring the pinned tree writes into the tree itself: vendored zlib renames
# lib/zlib/zconf.h to zconf.h.included (lib/zlib/CMakeLists.txt:71-76) and the
# MSIX resource generator writes src/inc/internal/MSIXResource.hpp and
# src/msix/common/MSIXResource.cpp into the source dir. Both are covered by
# .gitignore, yet both change the worktree digest the bootstrap's gate checks
# *before* it configures. So every configure below runs against a copy: the
# pinned tree is read for citations and verified for its digest, never written.
CONFIG_TREE="$WORK/sdk-config-copy"
echo "== copying the pinned tree to $CONFIG_TREE for the configure runs"
mkdir -p "$CONFIG_TREE"
if ! tar -C "$TREE" --exclude=./.git -cf - . | tar -C "$CONFIG_TREE" -xf -; then
  echo "error: could not copy the pinned tree" >&2
  exit 2
fi
note "copied $(find "$CONFIG_TREE" -type f | wc -l | tr -d ' ') files; the pinned tree is left untouched"

# Extract the -D options from the bootstrap's cmake invocation itself, so the
# properties below describe the shipped command line and not a copy of it.
extract_configure_flags() {
  awk '
    /cmake -DCMAKE_BUILD_TYPE=Release/ { inblock = 1 }
    inblock { print }
    inblock && /"\$WORK\/sdk"/ { exit }
  ' "$GATE" | grep -oE -- '-D[A-Za-z0-9_-]+=[^ \\]*' | sort -u
}

mapfile -t ALL_FLAGS < <(extract_configure_flags)
REPAIR_FLAGS=()
BASE_FLAGS=()
for flag in "${ALL_FLAGS[@]}"; do
  case "$flag" in
    -Dtranscoder=*|-Dmessage-loader=*) REPAIR_FLAGS+=("$flag") ;;
    *) BASE_FLAGS+=("$flag") ;;
  esac
done

echo
echo "== configure invocation extracted from bootstrap-makemsix.sh"
for flag in "${ALL_FLAGS[@]}"; do note "$flag"; done

# ---- compiler selection, mirroring the bootstrap ----------------------------
CONFIG_ENV=()
TOOLCHAIN=()
HAVE_CLANG=0
if command -v clang >/dev/null 2>&1 && command -v clang++ >/dev/null 2>&1; then
  HAVE_CLANG=1
  TOOLCHAIN=(-DCMAKE_TOOLCHAIN_FILE="$CONFIG_TREE/cmake/linux.cmake")
  COMPILER_DESC="clang/clang++ via cmake/linux.cmake (the hosted path)"
else
  CONFIG_ENV=(CC=gcc CXX=g++)
  COMPILER_DESC="gcc/g++ fallback (clang is absent here, so the bootstrap would take the same fallback)"
fi

# ---- citations from the pinned tree ----------------------------------------
P2_OK=1
cite() { # cite <relative path> <exact line fragment>
  local rel="$1" needle="$2" hit
  hit="$(grep -n -F -- "$needle" "$TREE/$rel" 2>/dev/null | head -1)"
  if [[ -n "$hit" ]]; then
    note "$rel:$hit"
  else
    note "MISSING: $rel does not contain: $needle"
    P2_OK=0
  fi
}

citations() {
  echo
  echo "== pinned-tree detection logic (P2)"
  cite "CMakeLists.txt" 'set(CMAKE_CXX_STANDARD 14)'
  cite "lib/xerces/CMakeLists.txt" 'set(CMAKE_CXX_STANDARD 14)'
  cite "lib/xerces/CMakeLists.txt" 'include(XercesICU)'
  cite "lib/xerces/CMakeLists.txt" 'include(XercesTranscoderSelection)'
  cite "lib/xerces/cmake/XercesICU.cmake" 'find_package(ICU COMPONENTS uc data)'
  cite "lib/xerces/cmake/XercesTranscoderSelection.cmake" 'if(ICU_FOUND)'
  cite "lib/xerces/cmake/XercesTranscoderSelection.cmake" 'list(APPEND transcoders icu)'
  cite "lib/xerces/cmake/XercesTranscoderSelection.cmake" 'list(GET transcoders 0 xerces_transcoder_default)'
  cite "lib/xerces/cmake/XercesTranscoderSelection.cmake" 'set(transcoder "${xerces_transcoder_default}" CACHE STRING'
  cite "lib/xerces/cmake/XercesTranscoderSelection.cmake" 'set(XERCES_USE_TRANSCODER_ICU 1)'
  cite "lib/xerces/cmake/XercesMsgLoaderSelection.cmake" 'set(message-loader "${xerces_msgloader_default}" CACHE STRING'
  cite "lib/xerces/src/CMakeLists.txt" 'if(XERCES_USE_TRANSCODER_ICU)'
  cite "lib/xerces/src/CMakeLists.txt" 'list(APPEND libxerces_c_DEPS ICU::uc ICU::data)'
  cite "lib/xerces/src/xercesc/util/regx/RangeToken.cpp" '#if XERCES_USE_TRANSCODER_ICU'
  cite "lib/xerces/src/xercesc/util/regx/RangeToken.cpp" '#include <unicode/uchar.h>'
  cite "lib/CMakeLists.txt" 'if(XML_PARSER MATCHES xerces)'
  cite "lib/CMakeLists.txt" 'add_subdirectory(xerces)'
}

# ---- synthetic ICU installation --------------------------------------------
# See the header: a fixture, not a reproduction of a real ICU installation. It
# is what makes the P3/P4 differential deterministic on a host without ICU
# development headers.
make_synthetic_icu() {
  local prefix="$1" cc="${CC:-cc}"
  local banner='/* SYNTHETIC ICU-shaped header fixture generated by
   deploy/windows/msix/test-sdk-config.sh. This is NOT an ICU installation: it
   exists so the pinned tree'"'"'s own find_package(ICU) detection and transcoder
   selection can be exercised on a host without ICU development headers. */'
  mkdir -p "$prefix/include/unicode" "$prefix/lib"
  {
    printf '%s\n' "$banner"
    printf '#define U_ICU_VERSION "74.2"\n'
    printf '#define U_ICU_VERSION_MAJOR_NUM 74\n'
    printf '#define U_ICU_VERSION_MINOR_NUM 2\n'
  } > "$prefix/include/unicode/uvernum.h"
  {
    printf '%s\n' "$banner"
    printf '#ifndef FIXTURE_UTYPES_H\n#define FIXTURE_UTYPES_H\n'
    printf '#include "unicode/uvernum.h"\n'
    printf 'typedef char16_t UChar;\n#endif\n'
  } > "$prefix/include/unicode/utypes.h"
  {
    printf '%s\n' "$banner"
    printf '/* Mirrors the property of an installed modern ICU char16ptr.h that\n'
    printf '   this C++14 tree cannot compile: C++17 library facilities. */\n'
    printf '#ifndef FIXTURE_CHAR16PTR_H\n#define FIXTURE_CHAR16PTR_H\n'
    printf '#include <string_view>\n#include <type_traits>\n'
    printf '#include "unicode/utypes.h"\n'
    printf 'namespace icu_fixture {\n'
    printf 'template <typename T, typename U>\nconstexpr bool same = std::is_same_v<T, U>;\n'
    printf 'inline std::u16string_view view(const UChar* p, std::size_t n) { return std::u16string_view(p, n); }\n'
    printf '}\n#endif\n'
  } > "$prefix/include/unicode/char16ptr.h"
  {
    printf '%s\n' "$banner"
    printf '#ifndef FIXTURE_UCHAR_H\n#define FIXTURE_UCHAR_H\n'
    printf '#include "unicode/utypes.h"\n#include "unicode/char16ptr.h"\n#include "unicode/uvernum.h"\n'
    printf '#endif\n'
  } > "$prefix/include/unicode/uchar.h"
  {
    printf '%s\n' "$banner"
    printf '#ifndef FIXTURE_USET_H\n#define FIXTURE_USET_H\n'
    printf '#include "unicode/utypes.h"\n#endif\n'
  } > "$prefix/include/unicode/uset.h"
  printf 'void icu_fixture_uc(void) {}\n' > "$prefix/uc.c"
  printf 'void icu_fixture_data(void) {}\n' > "$prefix/dt.c"
  "$cc" -shared -fPIC -o "$prefix/lib/libicuuc.so" "$prefix/uc.c" || return 1
  "$cc" -shared -fPIC -o "$prefix/lib/libicudata.so" "$prefix/dt.c" || return 1
}

# ---- configure / inspect helpers -------------------------------------------
# configure <build dir> <label> <extra flags...>  (uses BASE_FLAGS, plus the
# extra flags given; the caller decides whether REPAIR_FLAGS are included).
configure() {
  local build="$1" label="$2"; shift 2
  mkdir -p "$build"
  echo
  echo "== configuring [$label] with: ${CONFIG_ENV[*]:-} ${TOOLCHAIN[*]:-} ${BASE_FLAGS[*]} $*"
  (
    cd "$build" || exit 1
    # shellcheck disable=SC2086
    env "${CONFIG_ENV[@]}" cmake "${BASE_FLAGS[@]}" "${TOOLCHAIN[@]}" \
      -DCMAKE_EXPORT_COMPILE_COMMANDS=ON "$@" "$CONFIG_TREE"
  ) > "$build/configure.log" 2>&1
  CFG_RC=$?
  note "cmake exit=$CFG_RC log=$build/configure.log"
  grep -E 'Transcoder:|Message Loader:' "$build/configure.log" | sed 's/^-- */     /'
  return $CFG_RC
}

cache_field() { # cache_field <build> <key>  -> "<type>=<value>"
  grep -E "^$2:" "$1/CMakeCache.txt" 2>/dev/null | head -1 | sed "s/^$2://"
}

cache_help() { # cache_help <build> <key>  -> the // help comment above the entry
  awk -v key="$2" 'index($0, "//") == 1 { help = $0 }
                   $0 ~ "^" key ":" { print help; exit }' "$1/CMakeCache.txt" 2>/dev/null
}

config_macro() { # config_macro <build> <MACRO>  -> "defined" / "undefined" / "absent"
  local file="$1/lib/xerces/config.h"
  if grep -qE "^#define $2 " "$file" 2>/dev/null; then echo defined
  elif grep -qE "^/\* #undef $2 \*/" "$file" 2>/dev/null; then echo undefined
  else echo absent; fi
}

compiler_std() { # compiler_std <build> -> the -std= value in the xerces flags
  local flags="$1/lib/xerces/src/CMakeFiles/xerces-c.dir/flags.make"
  grep -E '^CXX_FLAGS =' "$flags" 2>/dev/null | grep -oE '\-std=[A-Za-z0-9+]+' | tail -1
}

compile_rangetoken() { # compile_rangetoken <build> -> exit status; log beside the build
  local build="$1" log="$1/rangetoken.log" entry
  entry="$(grep -F 'util/regx/RangeToken.cpp' "$build/compile_commands.json" 2>/dev/null | head -1)"
  if [[ -z "$entry" ]]; then
    printf 'no RangeToken.cpp entry in %s/compile_commands.json\n' "$build" > "$log"
    return 2
  fi
  printf 'compile_commands entry: %s\n' "$entry" > "$log"
  (
    cd "$build/lib/xerces/src" || exit 1
    make xercesc/util/regx/RangeToken.cpp.o
  ) >> "$log" 2>&1
  return $?
}

# ---- P1: the shipped invocation --------------------------------------------
property_p1() {
  local have_transcoder="" have_msgloader="" missing=()
  for flag in "${ALL_FLAGS[@]}"; do
    case "$flag" in
      -Dtranscoder=gnuiconv) have_transcoder="$flag" ;;
      -Dmessage-loader=inmemory) have_msgloader="$flag" ;;
    esac
  done
  for want in -DUSE_VALIDATION_PARSER=on -DMSIX_PACK=on -DSKIP_BUNDLES=on -DLINUX=on; do
    local seen=0
    for flag in "${BASE_FLAGS[@]}"; do [[ "$flag" == "$want" ]] && seen=1; done
    [[ "$seen" -eq 1 ]] || missing+=("$want")
  done
  if [[ -n "$have_transcoder" && -n "$have_msgloader" && ${#missing[@]} -eq 0 ]]; then
    pass "P1 (done-when a): the bootstrap's cmake invocation carries $have_transcoder $have_msgloader with pack support, validation parser, skip bundles and Linux still on"
    note "the two repaired entries are cache options the pinned tree reads:"
    note "  transcoder is set at lib/xerces/cmake/XercesTranscoderSelection.cmake:93 (CACHE STRING)"
    note "  message-loader is set at lib/xerces/cmake/XercesMsgLoaderSelection.cmake:44 (CACHE STRING)"
    note "both are read while lib/xerces is configured, i.e. before add_subdirectory(xerces) in lib/CMakeLists.txt"
  else
    fail "P1 (done-when a): configure line carries the repaired cache options" \
         "transcoder=[$have_transcoder] message-loader=[$have_msgloader] missing=${missing[*]:-none} flags=${ALL_FLAGS[*]}"
  fi
}

# ---- P2: citations ---------------------------------------------------------
property_p2() {
  citations
  if [[ "$P2_OK" -eq 1 ]]; then
    pass "P2 (done-when b, source half): the quoted lines are the pinned tree's own logic - ICU is listed first when detected, becomes the default transcoder, and RangeToken.cpp includes <unicode/uchar.h> only under XERCES_USE_TRANSCODER_ICU"
  else
    fail "P2 (done-when b, source half): pinned-tree citations" "one or more expected lines were not found in $TREE"
  fi
}

# ---- P3 / P4: the differential -----------------------------------------------
property_p3_p4() {
  local fake="$WORK/synthetic-icu"
  if ! make_synthetic_icu "$fake"; then
    fail "P3/P4 differential: synthetic ICU fixture" "could not build the fixture under $fake"
    return
  fi
  note "synthetic ICU prefix (fixture, not a real ICU installation): $fake"
  note "  $(grep -c . "$fake/include/unicode/char16ptr.h") lines in unicode/char16ptr.h, C++17 facilities in use"
  local fake_flag="-DICU_ROOT=$fake"

  configure "$WORK/build-old" "old invocation, ICU visible" "$fake_flag"
  local old_rc=$?
  local old_transcoder old_icu_macro old_std
  old_transcoder="$(cache_field "$WORK/build-old" transcoder)"
  old_icu_macro="$(config_macro "$WORK/build-old" XERCES_USE_TRANSCODER_ICU)"
  old_std="$(compiler_std "$WORK/build-old")"
  compile_rangetoken "$WORK/build-old"
  local old_compile_rc=$?
  note "transcoder cache entry: $old_transcoder   help: $(cache_help "$WORK/build-old" transcoder)"
  note "config.h XERCES_USE_TRANSCODER_ICU: $old_icu_macro   compiler standard: ${old_std:-none}"
  note "RangeToken.cpp compile exit: $old_compile_rc (log $WORK/build-old/rangetoken.log)"
  grep -m2 -E 'error:.*(is_same_v|u16string_view)' "$WORK/build-old/rangetoken.log" | sed 's/^/     /'

  if [[ "$old_rc" -eq 0 \
        && "$old_transcoder" == "STRING=icu" \
        && "$old_icu_macro" == "defined" \
        && "$old_std" == "-std=c++14" \
        && "$old_compile_rc" -ne 0 ]] \
     && grep -qE 'error:.*is_same_v' "$WORK/build-old/rangetoken.log"; then
    pass "P3 (done-when b, negative control): with ICU visible, the pre-repair invocation selects transcoder=icu, defines XERCES_USE_TRANSCODER_ICU, and RangeToken.cpp fails under -std=c++14 on the C++17 facilities in the ICU header"
  else
    fail "P3 (done-when b, negative control): pre-repair invocation selects the ICU path and fails" \
         "configure_rc=$old_rc transcoder=[$old_transcoder] macro=[$old_icu_macro] std=[$old_std] compile_rc=$old_compile_rc"
  fi

  if [[ ${#REPAIR_FLAGS[@]} -eq 0 ]]; then
    fail "P4 (done-when c): repaired invocation selects gnuiconv" "no repaired cache options were extracted from $GATE"
    return
  fi

  configure "$WORK/build-new" "repaired invocation, ICU visible" "$fake_flag" "${REPAIR_FLAGS[@]}"
  local new_rc=$?
  local new_transcoder new_msgloader new_icu_macro new_gnu_macro new_std
  new_transcoder="$(cache_field "$WORK/build-new" transcoder)"
  new_msgloader="$(cache_field "$WORK/build-new" message-loader)"
  new_icu_macro="$(config_macro "$WORK/build-new" XERCES_USE_TRANSCODER_ICU)"
  new_gnu_macro="$(config_macro "$WORK/build-new" XERCES_USE_TRANSCODER_GNUICONV)"
  new_std="$(compiler_std "$WORK/build-new")"
  compile_rangetoken "$WORK/build-new"
  local new_compile_rc=$?
  note "transcoder cache entry: $new_transcoder   help: $(cache_help "$WORK/build-new" transcoder)"
  note "message-loader cache entry: $new_msgloader   help: $(cache_help "$WORK/build-new" message-loader)"
  note "config.h XERCES_USE_TRANSCODER_ICU: $new_icu_macro   XERCES_USE_TRANSCODER_GNUICONV: $new_gnu_macro"
  note "compiler standard: ${new_std:-none}"
  note "RangeToken.cpp compile exit: $new_compile_rc (log $WORK/build-new/rangetoken.log)"

  if [[ "$new_rc" -eq 0 \
        && "$new_transcoder" == "STRING=gnuiconv" \
        && "$new_msgloader" == "STRING=inmemory" \
        && "$new_icu_macro" == "undefined" \
        && "$new_gnu_macro" == "defined" \
        && "$new_std" == "-std=c++14" \
        && "$new_compile_rc" -eq 0 ]]; then
    pass "P4 (done-when c): the repaired invocation, in the same ICU-visible environment, selects transcoder=gnuiconv and message-loader=inmemory, leaves XERCES_USE_TRANSCODER_ICU undefined, and compiles RangeToken.cpp under -std=c++14"
    note "the two configurations differ in exactly the direction claimed: same tree, same ICU visibility, same compiler, same standard"
  else
    fail "P4 (done-when c): repaired invocation selects gnuiconv and compiles" \
         "configure_rc=$new_rc transcoder=[$new_transcoder] message-loader=[$new_msgloader] icu_macro=[$new_icu_macro] gnu_macro=[$new_gnu_macro] std=[$new_std] compile_rc=$new_compile_rc"
  fi

  # P6 rides on the same configure: every -D option must have been consumed.
  if ! grep -qi 'Manually-specified variables were not used' "$WORK/build-new/configure.log"; then
    pass "P6: cmake consumed every -D option in the repaired invocation (no 'Manually-specified variables were not used' warning)"
  else
    fail "P6: cmake accepted every -D option" \
         "$(grep -i -A3 'Manually-specified variables were not used' "$WORK/build-new/configure.log" | tr '\n' ' ')"
  fi
}

# ---- P5: the standard bump is inert ----------------------------------------
property_p5() {
  configure "$WORK/build-std17" "repaired invocation + -DCMAKE_CXX_STANDARD=17 probe" \
    "-DICU_ROOT=$WORK/synthetic-icu" "${REPAIR_FLAGS[@]}" "-DCMAKE_CXX_STANDARD=17"
  local rc=$? std cache_entry
  std="$(compiler_std "$WORK/build-std17")"
  cache_entry="$(cache_field "$WORK/build-std17" CMAKE_CXX_STANDARD)"
  note "CMAKE_CXX_STANDARD cache entry: ${cache_entry:-absent}   compiler standard in flags.make: ${std:-none}"
  if [[ "$rc" -eq 0 && -n "$cache_entry" && "$std" == "-std=c++14" ]]; then
    pass "P5: a -DCMAKE_CXX_STANDARD=17 cache entry is inert here (cache=$cache_entry, compiler flag=$std) because the root and Xerces CMakeLists set CMAKE_CXX_STANDARD 14 as a normal variable, so bumping the standard from the command line cannot be the repair"
  else
    fail "P5: standard bump via cache is inert" "configure_rc=$rc cache=[$cache_entry] std=[$std]"
  fi
}

# ---- P7: honest gaps -------------------------------------------------------
property_p7() {
  gap "P7 (done-when d): no byte-identical hosted environment here - the real hosted compile is not reproduced:"
  if [[ "$HAVE_CLANG" -eq 1 ]]; then
    note "clang is present on this host: $COMPILER_DESC"
  else
    note "no clang/clang++ on PATH, so the hosted toolchain (cmake/linux.cmake selects clang++) was not used; configures used $COMPILER_DESC"
  fi
  local icu_header=""
  for candidate in /usr/include/unicode/char16ptr.h /usr/local/include/unicode/char16ptr.h; do
    [[ -f "$candidate" ]] && icu_header="$candidate"
  done
  if [[ -n "$icu_header" ]]; then
    note "real ICU development header present: $icu_header"
  else
    note "no real ICU development headers on this host (no unicode/char16ptr.h under /usr/include or /usr/local/include): the ICU-visible runs above use the synthetic fixture, so P3 shows the failure mechanism, not Ubuntu's ICU 74 bytes"
  fi
  note "the only evidence for the real ICU bytes is the hosted run: $HOSTED_RUN"
  note "still the Captain's checks, not passes: hosted store-package job end to end, downloaded MSIX, verify-msix.py, native Add-AppxPackage/Get-AppxPackage readback, wack-report.xml"
  note "what P3/P4 do establish: the pinned tree's detection logic, the shipped flags, and the effective CMakeCache.txt/config.h/compiler-standard values for both invocations"
}

echo
echo "== compiler this host would use: $COMPILER_DESC"
echo "== hosted evidence under repair: $HOSTED_RUN"

if [[ "$MODE" == "negative" ]]; then
  property_p2
  local_fake_dir="$WORK/synthetic-icu"
  if ! make_synthetic_icu "$local_fake_dir"; then
    echo "error: could not build the synthetic ICU fixture" >&2
    exit 2
  fi
  configure "$WORK/build-old" "old invocation, ICU visible" "-DICU_ROOT=$local_fake_dir"
  old_rc=$?
  echo "     transcoder: $(cache_field "$WORK/build-old" transcoder)   help: $(cache_help "$WORK/build-old" transcoder)"
  echo "     config.h XERCES_USE_TRANSCODER_ICU: $(config_macro "$WORK/build-old" XERCES_USE_TRANSCODER_ICU)"
  echo "     compiler standard: $(compiler_std "$WORK/build-old")"
  compile_rangetoken "$WORK/build-old"
  old_compile_rc=$?
  echo "     RangeToken.cpp compile exit: $old_compile_rc"
  grep -m3 -E 'error:' "$WORK/build-old/rangetoken.log" | sed 's/^/     /'
  echo
  echo "== old invocation summary (property P3 evidence, printed for review)"
  echo "   transcoder=$(cache_field "$WORK/build-old" transcoder) icu_macro=$(config_macro "$WORK/build-old" XERCES_USE_TRANSCODER_ICU) std=$(compiler_std "$WORK/build-old") compile_exit=$old_compile_rc"
  if [[ "$old_rc" -eq 0 && "$(cache_field "$WORK/build-old" transcoder)" == "STRING=icu" && "$old_compile_rc" -ne 0 ]]; then
    echo "RESULT old invocation selects the ICU transcoder and fails to compile; the repaired invocation (see the default run) does not"
    exit 0
  fi
  echo "RESULT FAIL: the old invocation did not select ICU and fail as expected" >&2
  exit 1
fi

property_p1
property_p2
property_p3_p4
property_p5
property_p7

echo
if [[ "$FAILED" -eq 0 ]]; then
  echo "== $PASSED passed, 0 failed, $GAPS reported as a capability gap"
  echo "RESULT all configuration properties proved from machine output; hosted compile, packaging and WACK remain the Captain's checks"
  exit 0
fi
echo "== $PASSED passed, $FAILED failed, $GAPS reported as a capability gap"
echo "RESULT FAIL: the configuration evidence does not support the repair"
exit 1
