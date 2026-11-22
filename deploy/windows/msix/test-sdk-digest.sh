#!/usr/bin/env bash
# Offline, deterministic proof of the MSIX SDK source-integrity gate.
#
#   bash deploy/windows/msix/test-sdk-digest.sh
#   bash deploy/windows/msix/test-sdk-digest.sh --expect-tamper-refusal
#   bash deploy/windows/msix/test-sdk-digest.sh --pin <64-hex>
#
# Builds a small fixture tree from fixed bytes, then proves with no network
# that the gate in bootstrap-makemsix.sh:
#   * accepts a tree whose bytes match the expected digest,
#   * refuses a one-byte mutation with exit 1 and both digests printed,
#   * fails for the digest comparison itself, not for a missing file, a
#     missing tool or a usage error (the negative control prints the reason),
#   * produces the same digest under LC_ALL=C, LC_ALL=C.UTF-8,
#     LANG=en_US.UTF-8 and a stripped environment,
#   * refuses a wrong pinned value, an override without a commit bump, an
#     empty override, and the retired EBBTOP_MSIX_SHA256 variable,
#   * is not overridden by the hosted workflow.
#
# The expected fixture digest is a literal computed with an independent
# implementation (textbook sha256 over the byte-ordered `sha256sum` lines, in
# Python), not with the shell recipe under test.
#
# --expect-tamper-refusal runs only the negative control and exits 0 when the
#   refusal is the digest comparison (prints the reason and both digests).
# --pin <64-hex> replaces the expected fixture digest. With a wrong value the
#   gate must refuse it, so the script prints the refusal and exits nonzero.
set -uo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
GATE="$HERE/bootstrap-makemsix.sh"
REPO_ROOT="$(cd -- "$HERE/../../.." && pwd)"

PIN_PINNED="5b850191232ff0034df1eaf8fc09642651ca38535d0113138e70e914cffc93b6"
FIXTURE_DIGEST="ae56c20388e281b35adf4789a5d1cdc4fd04a80692223d70eec43986050bed15"
TAMPER_DIGEST="c446651e2274dd1a406ecb7dd059d6d4d34a7283a679558ebbf08ca2404daffb"
FIXTURE_COMMIT="ebbtop-msix-digest-test"
# Real-directory tamper fixture, so the reviewer can re-run the gate against
# it directly (see the card's done-when list).
TAMPER_DIR="${EBBTOP_MSIX_TAMPER_FIXTURE:-/tmp/ebbtop-tamper-fixture}"

MODE="all"
PIN="$FIXTURE_DIGEST"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --expect-tamper-refusal) MODE="tamper"; shift ;;
    --pin)
      [[ $# -ge 2 ]] || { echo "error: --pin needs a 64-hex digest" >&2; exit 2; }
      PIN="$2"; shift 2 ;;
    -h|--help)
      sed -n '2,28p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *)
      echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ -f "$GATE" ]] || { echo "error: gate not found: $GATE" >&2; exit 2; }
command -v sha256sum >/dev/null || { echo "error: sha256sum is required" >&2; exit 2; }
command -v sort >/dev/null || { echo "error: sort is required" >&2; exit 2; }
command -v cmp >/dev/null || { echo "error: cmp is required" >&2; exit 2; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/ebbtop-msix-digest-test.XXXXXX")"
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT
PRISTINE="$WORK/pristine"

make_fixture() {
  local dest="$1"
  mkdir -p "$dest/MsixCore" "$dest/cmake" "$dest/lib/catch2" "$dest/lib/openssl" "$dest/src"
  printf '*.txt text eol=lf\n'                                          > "$dest/.gitattributes"
  printf 'cmake_minimum_required(VERSION 3.29)\nproject(fixture NONE)\n' > "$dest/CMakeLists.txt"
  printf 'MIT\n'                                                        > "$dest/LICENSE"
  printf '# fixture\n'                                                   > "$dest/README.md"
  printf 'int main(){return 0;}\n'                                      > "$dest/MsixCore/msixmgr.cpp"
  printf 'set(FIXTURE 1)\n'                                             > "$dest/cmake/linux.cmake"
  printf '#pragma once\n'                                               > "$dest/lib/catch2/catch.hpp"
  printf 'openssl fixture\n'                                            > "$dest/lib/openssl/README"
  printf 'SECTION-ONE\nSECTION-TWO\n'                                   > "$dest/src/payload.bin"
}

make_fixture "$PRISTINE"

# One-byte mutation: byte 23 of src/payload.bin, 'O' (0x4f) -> 'X' (0x58).
# Same length, same file list, one byte different.
case "$TAMPER_DIR" in
  /?*) : ;;
  *) echo "error: EBBTOP_MSIX_TAMPER_FIXTURE must be an absolute path" >&2; exit 2 ;;
esac
rm -rf -- "$TAMPER_DIR"
make_fixture "$TAMPER_DIR"
printf 'SECTION-ONE\nSECTION-TWX\n' > "$TAMPER_DIR/src/payload.bin"

PASSED=0
FAILED=0
TOTAL=13

pass() { PASSED=$((PASSED + 1)); printf 'PASS %02d/%02d %s\n' "$PASSED" "$TOTAL" "$1"; }
fail() { FAILED=$((FAILED + 1)); printf 'FAIL %s\n     %s\n' "$1" "$2"; }

# run_gate <dir> [env assignments...]
# UNSET_VARS lists variables removed from the child environment before the run.
UNSET_VARS=()
run_gate() {
  local dir="$1"; shift
  GATE_OUT="$(
    unset ${UNSET_VARS[@]+"${UNSET_VARS[@]}"}
    env "$@" bash "$GATE" --digest-only "$dir" 2>&1
  )"
  GATE_RC=$?
}

digests_in() { printf '%s\n' "$1" | grep -oE '[0-9a-f]{64}' | sort -u; }

echo "gate:      $GATE"
echo "fixture:   $FIXTURE_DIGEST (9 files)"
echo "tampered:  $TAMPER_DIGEST (one byte changed)"
echo "pin:       $PIN_PINNED (real SDK worktree)"
echo "sort:      $(sort --version | head -1)"
echo

# ---- negative control mode -------------------------------------------------
if [[ "$MODE" == "tamper" ]]; then
  run_gate "$TAMPER_DIR" \
    EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256="$PIN"
  echo "--- gate output ------------------------------------------------"
  printf '%s\n' "$GATE_OUT"
  echo "----------------------------------------------------------------"
  echo "exit: $GATE_RC"
  if [[ "$GATE_RC" -ne 1 ]]; then
    echo "FAIL negative control: expected exit 1 from the digest comparison, got $GATE_RC"
    exit 1
  fi
  if ! printf '%s' "$GATE_OUT" | grep -q 'does not match the pinned value'; then
    echo "FAIL negative control: refusal is not the digest comparison"
    exit 1
  fi
  for want in "$PIN" "$TAMPER_DIGEST"; do
    if ! printf '%s' "$GATE_OUT" | grep -q "$want"; then
      echo "FAIL negative control: digest $want not printed"
      exit 1
    fi
  done
  echo "PASS negative control: refused by the digest comparison (exit 1), both digests printed"
  exit 0
fi

# ---- --pin mode: prove the gate refuses a caller-supplied wrong value ------
if [[ "$PIN" != "$FIXTURE_DIGEST" ]]; then
  echo "--pin $PIN supplied: the pristine fixture must be refused, not re-pinned"
  run_gate "$PRISTINE" \
    EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256="$PIN"
  printf '%s\n' "$GATE_OUT"
  echo "exit: $GATE_RC"
  if [[ "$GATE_RC" -ne 0 ]] && printf '%s' "$GATE_OUT" | grep -q 'does not match the pinned value'; then
    echo "RESULT wrong pinned value refused with exit $GATE_RC (the gate does not replace it)"
    exit 1
  fi
  echo "RESULT FAIL: pinned value $PIN was not refused (exit $GATE_RC)"
  exit 1
fi

# ---- property 1: pristine fixture accepted ---------------------------------
run_gate "$PRISTINE" \
  EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256="$PIN"
if [[ "$GATE_RC" -eq 0 ]] && printf '%s' "$GATE_OUT" | grep -q "matches the pinned value: $PIN"; then
  pass "pristine fixture accepted (exit 0, digest matches the expected value)"
else
  fail "pristine fixture accepted" "exit=$GATE_RC output: $GATE_OUT"
fi

# ---- properties 2 and 3: one-byte mutation refused -------------------------
run_gate "$TAMPER_DIR" \
  EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256="$PIN"
TAMPER_OUT="$GATE_OUT"
TAMPER_RC="$GATE_RC"
FOUND="$(digests_in "$TAMPER_OUT" | tr '\n' ' ')"
if [[ "$TAMPER_RC" -eq 1 ]] \
   && printf '%s' "$TAMPER_OUT" | grep -q 'does not match the pinned value' \
   && printf '%s' "$TAMPER_OUT" | grep -q "$TAMPER_DIGEST" \
   && printf '%s' "$TAMPER_OUT" | grep -q "$PIN"; then
  pass "one-byte mutation refused (exit 1, actual $TAMPER_DIGEST and pinned $PIN printed)"
else
  fail "one-byte mutation refused" "exit=$TAMPER_RC digests=[$FOUND] output: $TAMPER_OUT"
fi

if [[ "$TAMPER_RC" -eq 1 ]] \
   && ! printf '%s' "$TAMPER_OUT" | grep -qE 'not a directory|cmake is required|unknown option|needs a source tree' \
   && [[ "$(digests_in "$TAMPER_OUT" | wc -l)" -ge 2 ]] \
   && printf '%s' "$TAMPER_OUT" | grep -q "tree:"; then
  pass "refusal is the digest comparison, not a usage/tool error (exit 1, two digests, tree named)"
else
  fail "refusal is the digest comparison" "exit=$TAMPER_RC digests=[$FOUND] output: $TAMPER_OUT"
fi

# ---- properties 4-7: locale matrix ----------------------------------------
locale_case() {
  local label="$1"; shift
  local unset_list="$1"; shift
  UNSET_VARS=()
  [[ -z "$unset_list" ]] || read -r -a UNSET_VARS <<< "$unset_list"
  run_gate "$PRISTINE" \
    EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256="$PIN" "$@"
  UNSET_VARS=()
  if [[ "$GATE_RC" -eq 0 ]] && printf '%s' "$GATE_OUT" | grep -q "matches the pinned value: $PIN"; then
    pass "digest identical under $label"
  else
    fail "digest identical under $label" "exit=$GATE_RC output: $GATE_OUT"
  fi
}
locale_case "LC_ALL=C" "" LC_ALL=C LANG=C
locale_case "LC_ALL=C.UTF-8" "" LC_ALL=C.UTF-8 LANG=C.UTF-8
locale_case "LANG=en_US.UTF-8" "LC_ALL LC_COLLATE LANGUAGE" LANG=en_US.UTF-8
locale_case "locale unset" "LC_ALL LANG LC_COLLATE LANGUAGE"

# ---- property 8: wrong pin refused ----------------------------------------
ZERO_PIN="0000000000000000000000000000000000000000000000000000000000000000"
run_gate "$PRISTINE" \
  EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256="$ZERO_PIN"
if [[ "$GATE_RC" -eq 1 ]] \
   && printf '%s' "$GATE_OUT" | grep -q 'does not match the pinned value' \
   && printf '%s' "$GATE_OUT" | grep -q "$ZERO_PIN"; then
  pass "wrong pinned value refused (exit 1, zero pin echoed as the pinned value)"
else
  fail "wrong pinned value refused" "exit=$GATE_RC output: $GATE_OUT"
fi

# ---- properties 9-11: no bypass -------------------------------------------
run_gate "$PRISTINE" EBBTOP_MSIX_SDK_SHA256="$PIN"
if [[ "$GATE_RC" -eq 2 ]] && printf '%s' "$GATE_OUT" | grep -q 'refusing EBBTOP_MSIX_SDK_SHA256'; then
  pass "digest override refused while the built-in pinned commit is in use"
else
  fail "digest override refused without a commit bump" "exit=$GATE_RC output: $GATE_OUT"
fi

run_gate "$PRISTINE" EBBTOP_MSIX_SDK_COMMIT="$FIXTURE_COMMIT" EBBTOP_MSIX_SDK_SHA256=
if [[ "$GATE_RC" -eq 2 ]] && printf '%s' "$GATE_OUT" | grep -q 'set but empty'; then
  pass "empty override refused (an empty value cannot disable the check)"
else
  fail "empty override refused" "exit=$GATE_RC output: $GATE_OUT"
fi

run_gate "$PRISTINE" EBBTOP_MSIX_SHA256=
if [[ "$GATE_RC" -eq 2 ]] && printf '%s' "$GATE_OUT" | grep -q 'not supported'; then
  pass "retired EBBTOP_MSIX_SHA256 refused (no silent skip path)"
else
  fail "retired EBBTOP_MSIX_SHA256 refused" "exit=$GATE_RC output: $GATE_OUT"
fi

# ---- property 12: the hosted workflow sets no override ---------------------
WORKFLOW="$REPO_ROOT/.github/workflows/windows-build.yml"
if [[ -f "$WORKFLOW" ]] && ! grep -q 'EBBTOP_MSIX_SDK_SHA256\|EBBTOP_MSIX_SHA256' "$WORKFLOW"; then
  pass "hosted workflow sets no digest override ($WORKFLOW)"
else
  fail "hosted workflow sets no digest override" "missing file or override present: $WORKFLOW"
fi

# ---- property 13: real-directory tamper fixture ---------------------------
DIFF_BYTES="$(cmp -l "$PRISTINE/src/payload.bin" "$TAMPER_DIR/src/payload.bin" | wc -l | tr -d ' ')"
if [[ -d "$TAMPER_DIR" && "$DIFF_BYTES" == "1" ]]; then
  pass "one-byte tamper fixture materialised at $TAMPER_DIR"
else
  fail "tamper fixture materialised" "dir=$TAMPER_DIR differing byte count=$DIFF_BYTES"
fi

# ---- informational: was the fixture collation-sensitive at all? -----------
raw_digest() {
  ( cd "$PRISTINE" && env "$@" bash -c 'find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | cut -d" " -f1' )
}
RAW_C="$(raw_digest LC_ALL=C)"
RAW_EN="$(raw_digest LC_ALL=en_US.UTF-8)"
echo
echo "INFO raw (not locale-forced) fixture digest: LC_ALL=C=$RAW_C"
echo "INFO raw (not locale-forced) fixture digest: LC_ALL=en_US.UTF-8=$RAW_EN"
if [[ "$RAW_C" != "$RAW_EN" ]]; then
  echo "INFO the fixture is collation-sensitive: a locale leak would change the digest."
else
  echo "INFO en_US.UTF-8 is unavailable here and this sort fell back to byte order, so the"
  echo "INFO locale cases above exercise the unavailable-locale path, not an en_US collation."
fi
echo "INFO installed locales: $(locale -a 2>/dev/null | tr '\n' ' ')"

# ---- summary --------------------------------------------------------------
echo
echo "== $PASSED/$TOTAL properties passed, $FAILED failed"

[[ "$FAILED" -eq 0 ]] || exit 1
echo "RESULT all properties proved offline; the pinned SDK value was not re-fetched here"
exit 0
