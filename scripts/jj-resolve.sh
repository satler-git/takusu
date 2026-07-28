#!/usr/bin/env bash
# jj-resolve.sh — inspect and resolve Jujutsu merge conflicts.
#
# Usage:
#   jj-resolve.sh list              List conflicted files in the working copy.
#   jj-resolve.sh show [<file>]     Show conflict markers for one file (or all).
#   jj-resolve.sh edit <file>       Open $EDITOR on the file. After saving,
#                                   run `mark` to verify resolution.
#   jj-resolve.sh mark <file>       Verify a file is resolved (jj auto-detects
#                                   resolution once conflict markers are gone;
#                                   there is no explicit "mark" command).
#   jj-resolve.sh merge <file>      Launch a 3-way merge tool via `jj resolve`.
#   jj-resolve.sh abort             Abort the in-progress merge/rebase.
#   jj-resolve.sh status            One-line summary: N files conflicted.
#
# This wraps `jj resolve --list` and friends so the agent has a stable,
# documented interface. It does NOT auto-pick a side — conflict resolution
# requires a human or the agent to edit the file deliberately.
#
# Requires: jj.
#
# Conflict marker style: this repo sets `ui.conflict-marker-style = "git"` in
# `.jj/repo/config.toml`, so conflicts use the git format:
#   <<<<<<<  (side label)
#   ...ours...
#   |||||||  (base, optional)
#   ...base...
#   =======
#   ...theirs...
#   >>>>>>>  (side label)
# We match these four markers (7+ chars of <, >, |, =).

set -euo pipefail

die() { echo "jj-resolve: $*" >&2; exit 1; }

# Grep pattern matching git-style conflict markers (7+ chars of <, >, |, =).
CONFLICT_MARKER_RE='^<{7}|^>{7}|^\|{7}|^={7}'

usage() {
  sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
  exit "${1:-0}"
}

# `jj resolve --list` outputs lines like:
#   path/to/file    2-sided conflict
# We extract just the path (first whitespace-delimited token).
conflicted_files() {
  jj resolve --list 2>/dev/null | awk '{print $1}' || true
}

cmd_list() {
  local files
  files="$(conflicted_files)"
  if [ -z "$files" ]; then
    echo "no conflicts"
    return 0
  fi
  echo "$files"
}

cmd_status() {
  local n
  n="$(conflicted_files | wc -l | tr -d ' ')"
  echo "$n conflicted file(s)"
}

cmd_show() {
  local file="${1:-}"
  if [ -n "$file" ]; then
    [ -f "$file" ] || die "show: file not found: $file"
    grep -nE "$CONFLICT_MARKER_RE" "$file" || echo "(no conflict markers in $file)"
    return
  fi
  local files
  files="$(conflicted_files)"
  if [ -z "$files" ]; then
    echo "no conflicts"
    return
  fi
  while IFS= read -r f; do
    echo "=== $f ==="
    grep -nE "$CONFLICT_MARKER_RE" "$f" 2>/dev/null || echo "(no markers)"
  done <<< "$files"
}

cmd_edit() {
  [ $# -ge 1 ] || die "edit: missing file"
  local file="$1"; shift
  [ -f "$file" ] || die "edit: file not found: $file"
  ${EDITOR:-vi} "$file"
  echo "after saving, run: jj-resolve.sh mark $file"
}

cmd_mark() {
  [ $# -ge 1 ] || die "mark: missing file"
  local file="$1"; shift
  [ -f "$file" ] || die "mark: file not found: $file"
  # jj has no explicit "mark resolved" command. A file is considered resolved
  # once it no longer appears in `jj resolve --list`. This happens automatically
  # when all conflict markers are removed from the file. Here we verify that:
  # 1. The file has no remaining conflict markers.
  # 2. The file is no longer listed as conflicted by jj.
  if grep -qE "$CONFLICT_MARKER_RE" "$file" 2>/dev/null; then
    die "mark: $file still has conflict markers — edit it first to remove them"
  fi
  local still_conflicted
  still_conflicted="$({ jj resolve --list 2>/dev/null || true; } | awk -v f="$file" '$1 == f {print $1; exit}')"
  if [ -n "$still_conflicted" ]; then
    die "mark: $file still listed as conflicted by jj (markers gone but jj hasn't re-scanned — run 'jj st' to trigger a re-scan)"
  fi
  echo "resolved: $file"
  local remaining
  remaining="$(conflicted_files | wc -l | tr -d ' ')"
  echo "$remaining file(s) still conflicted"
  if [ "$remaining" = "0" ]; then
    echo "all conflicts resolved — you can now continue (e.g. jj rebase --continue or jj describe)"
  fi
}

cmd_merge() {
  [ $# -ge 1 ] || die "merge: missing file"
  local file="$1"; shift
  [ -f "$file" ] || die "merge: file not found: $file"
  # `jj resolve <file>` launches the configured 3-way merge tool. The built-in
  # tools `:ours` and `:theirs` pick side #1 and side #2 respectively.
  jj resolve "$file"
  echo "merge tool exited for $file"
  cmd_mark "$file"
}

cmd_abort() {
  # Jujutsu does not have a single "abort" verb for all operations. The
  # closest is abandoning the current change if it was created by a merge.
  # We print guidance instead of guessing.
  local op
  op="$(jj op log --no-graph -T 'description.first_line()' -r ..@ -l 1 2>/dev/null || echo '?')"
  echo "last op: $op"
  echo "jj does not have a universal abort. Common recovery:"
  echo "  jj abandon @            # discard the conflicted working copy change"
  echo "  jj op restore <op-id>   # undo the last operation"
  echo "  jj undo                 # undo the most recent operation"
}

[ $# -ge 1 ] || usage 1
sub="$1"; shift
case "$sub" in
  list)   cmd_list "$@" ;;
  status) cmd_status "$@" ;;
  show)   cmd_show "$@" ;;
  edit)   cmd_edit "$@" ;;
  mark)   cmd_mark "$@" ;;
  merge)  cmd_merge "$@" ;;
  abort)  cmd_abort "$@" ;;
  -h|--help) usage 0 ;;
  *)      die "unknown subcommand: $sub (expected: list|status|show|edit|mark|merge|abort)" ;;
esac
