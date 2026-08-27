#!/bin/sh
# Bridge an agent session rooted in the main checkout to a sibling worktree's files.
#
# Why: the coding agent's Edit tool can reject worktree paths with a stale snapshot
# hash ("hash #XXXX is not from this session") even after a fresh read of the same
# path — same-relative-path files in two checkouts appear to collide in its snapshot
# cache. Symlinking the worktree's paths under THIS checkout with absolute targets
# gives the edits a unique path; reads and edits through the link land in the worktree.
#
# Usage:
#   scripts/worktree-edit-link.sh <worktree-dir>       # create links, print them
#   scripts/worktree-edit-link.sh <worktree-dir> --rm  # remove links (cleanup)
#
# <worktree-dir> is the worktree root (e.g. ../grind-179). Every top-level directory
# and regular file of the worktree that is NOT tracked in this checkout gets a
# session-root link named .worktree-<name>-<entry>; files tracked here (AGENTS.md,
# justfile, ...) are left alone so this checkout stays pristine.
set -eu

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <worktree-dir> [--rm]" >&2
    exit 2
fi

WT=$(cd "$1" && pwd)
ROOT=$(pwd)
NAME=$(basename "$WT")
PREFIX=".worktree-${NAME}-"

if [ "${2:-}" = "--rm" ]; then
    for link in "$PREFIX"*; do
        [ -L "$link" ] && rm "$link" && echo "removed $link"
    done
    git checkout -- . 2>/dev/null || true
    exit 0
fi

[ -d "$WT/.git" ] || [ -f "$WT/.git" ] || { echo "not a worktree: $WT" >&2; exit 1; }

for entry in "$WT"/*; do
    base=$(basename "$entry")
    link="$PREFIX$base"
    [ -e "$ROOT/$base" ] && continue
    [ -e "$ROOT/$link" ] && continue
    if [ -d "$entry" ]; then
        ln -s "$entry" "$link"
    else
        ln -s "$entry" "$link"
    fi
    echo "linked $link -> $entry"
done
echo "edit through $ROOT/$PREFIX* paths; run '$0 $1 --rm' when the work merges"
