#!/bin/sh
# What a resumed attempt does to the stage's own transcript: it appends. The newest existing
# `.jsonl` under `--session-dir` is continued rather than replaced, and only the appended frames
# reach stdout — so the fan-out slice is right only if the pre-run line count was right.
FILE=""
for candidate in "$SESSION_DIR"/*.jsonl; do
  [ -f "$candidate" ] && FILE="$candidate"
done
if [ -z "$FILE" ]; then
  printf 'resume_append found no session file to continue under %s\n' "$SESSION_DIR" >&2
  exit 66
fi
APPENDED=$(cat <<'FRAMES'
{"type":"turn_start","n":3}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"r-1","name":"task","arguments":{"description":"second wave"}}],"stopReason":"toolUse","usage":{"input":100,"output":50,"cost":{"total":0.02}}}}
{"type":"turn_end"}
{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"resumed and still working"}]}]}
FRAMES
)
printf '%s\n' "$APPENDED" >> "$FILE"
printf '%s\n' "$APPENDED"
exit 0
