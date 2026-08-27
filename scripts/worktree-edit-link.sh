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
# <worktree-dir> is the worktree root (e.g. ../grind-179). Every top-level entry of
# the worktree except `.git` gets a session-root link named .worktree-<name>-<entry>,
# unless that name is already taken in this checkout (tracked files like AGENTS.md
# are left alone so this checkout stays pristine). Cleanup only ever touches links
# whose target is the worktree named on the command line.
set -eu

if [ "$#" -lt 1 ]; then
    echo "usage: $0 <worktree-dir> [--rm]" >&2
    exit 2
fi

if [ "${2:-}" = "--rm" ]; then
    for link in .worktree-*; do
        [ -L "$link" ] || continue
        target=$(readlink "$link")
        case "$target" in
            "$1"/*|"$1") rm "$link" && echo "removed $link" ;;
        esac
    done
    exit 0
fi
WT=$(cd "$1" && pwd)
NAME=$(basename "$WT")
PREFIX=".worktree-${NAME}-"
ROOT=$(pwd)

for entry in "$WT"/* "$WT"/.[!.]* "$WT"/..?*; do
    [ -e "$entry" ] || continue
    base=$(basename "$entry")
    [ "$base" = ".git" ] && continue
    link="$PREFIX$base"
    [ -e "$ROOT/$base" ] || [ -L "$ROOT/$base" ] && continue
    [ -e "$ROOT/$link" ] || [ -L "$ROOT/$link" ] && continue
    ln -s "$entry" "$link"
    echo "linked $link -> $entry"
done
echo "edit through $ROOT/$PREFIX* paths; run '$0 $1 --rm' when the work merges"
