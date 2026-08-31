#!/bin/sh
# The fallback arm, and the only shape that covers it: `--session-dir` is ignored and the
# session is bucketed under the encoded cwd inside `~/.omp/agent/sessions/` instead. ADR-0017's
# own amendment records this as *usually honored flat, sometimes ignored for the encoded-cwd
# bucket* — the harness is inconsistent, so `strayed_after` exists, so something has to
# reproduce it.
BUCKET="$HOME/.omp/agent/sessions/$(pwd -P | sed 's|[^A-Za-z0-9]|-|g')"
mkdir -p "$BUCKET"
FILE="$BUCKET/2026-01-02T03-04-05-000Z_99999999-8888-4777-8666-555555555555.jsonl"
cat > "$FILE" <<'FRAMES'
{"type":"session","version":3,"id":"99999999-8888-4777-8666-555555555555"}
{"type":"turn_start","n":1}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"s-1","name":"task","arguments":{"description":"strayed scout"}}],"stopReason":"toolUse","usage":{"input":10,"output":20,"cost":{"total":0.01}}}}
{"type":"turn_end"}
{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"bucketed elsewhere"}]}]}
FRAMES
cat "$FILE"
exit 0
