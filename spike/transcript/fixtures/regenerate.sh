#!/usr/bin/env bash
# Rebuild the uncommitted real fixtures from this host's own transcripts.
# PROTOTYPE helper for wayfinder #33. Picks whatever is available rather than fixed sessions.
set -euo pipefail
cd "$(dirname "$0")"
P="$HOME/.claude/projects"

big=$(ls -S "$P"/*/*.jsonl 2>/dev/null | head -1)
cp "$big" real-parent-heterogeneous.jsonl
mapfile -t small < <(ls -S "$P"/*/*.jsonl | tail -40 | head -2)
cp "${small[0]}" real-small-1.jsonl
cp "${small[1]}" real-small-2.jsonl

# a truncated copy: the shape a killed process leaves behind
head -c $(( $(wc -c < real-small-1.jsonl) - 400 )) real-small-1.jsonl > truncated.jsonl
# a not-JSON copy: a line of prose where a record belongs
{ head -20 real-small-1.jsonl; echo "this line is not json at all"; } > not-json.jsonl

# a real fan-out session: parent .jsonl plus its subagents/ dir
sub=$(find "$P" -type d -name subagents | head -1)
if [ -n "$sub" ]; then
  sess=$(dirname "$sub"); dir=$(dirname "$sess"); id=$(basename "$sess")
  rm -rf real-fanout-session && mkdir -p real-fanout-session
  cp "$dir/$id.jsonl" real-fanout-session/ 2>/dev/null || true
  mkdir -p "real-fanout-session/$id/subagents"
  cp "$sub"/* "real-fanout-session/$id/subagents/" 2>/dev/null || true
fi
echo "regenerated; $(ls -1 | wc -l) entries"
