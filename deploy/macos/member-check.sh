#!/usr/bin/env bash
# Member-set proof for the macOS release archive.
#
# Sourced by deploy/macos/package-agent.sh, and executed directly by
# deploy/macos/test-install-macos.sh under a real Bash 3.2 so the compatibility
# claim is proven by RUNNING this code rather than by parsing a bigger script.
#
# Why a separate file: `bash -n` on the packager only proves the syntax parses.
# The archive member check is the one step that silently did not run on macOS
# stock Bash (it used `mapfile`, a Bash 4 builtin), so it gets its own sourceable
# unit that a fixture can execute on any host with a Bash 3.2 binary present.
#
# Bash 3.2 only: no mapfile, no readarray, no `local -n`, no associative arrays,
# no `${var,,}`. `tar`, `sort` and `tr` are the only external tools.
#
# Usage:
#   . member-check.sh
#   member_set_matches <archive> <name>...   # 0 = exact set, 1 = mismatch
#
# On mismatch it prints the archive's actual member list on stdout (space
# separated) so the caller can put it in its error message; on success it prints
# nothing.

# The archive's member names, LC_ALL=C sorted, one per line.
archive_member_list() { # <archive>
  tar -tzf "$1" | LC_ALL=C sort
}

# Compare the archive's member set to an exact expected list.
#
# Both sides are built into a newline-terminated string and the trailing newline
# is stripped from the archive side, because a command substitution drops it:
# comparing a substitution result against a loop-built string fails on a correct
# archive. Ordering is irrelevant (both sides are sorted C-locale).
member_set_matches() { # <archive> <expected name>...
  local archive="$1"
  shift
  local actual_l='' expected_l='' m
  while IFS= read -r m; do
    [ -n "$m" ] || continue
    actual_l="${actual_l}${m}"$'\n'
  done < <(archive_member_list "$archive")
  actual_l="${actual_l%$'\n'}"
  for m in "$@"; do
    expected_l="${expected_l}${m}"$'\n'
  done
  expected_l="${expected_l%$'\n'}"
  expected_l="$(printf '%s' "$expected_l" | LC_ALL=C sort)"
  if [ "$actual_l" = "$expected_l" ]; then
    return 0
  fi
  printf '%s' "$actual_l" | tr '\n' ' '
  return 1
}
