#!/bin/sh
# The primary harvest arm: the session file lands flat inside `--session-dir`, exactly the P0
# observation (`<ISO-ts>_<uuid>.jsonl`). Two turns, per-message usage on the assistant
# `message_end` frames, one `task` fan-out that completed, and an `agent_end` closing over
# assistant work — the healthy shape, so a failure here is the argv or the harvest and never
# the classifier.
FILE="$SESSION_DIR/2026-01-02T03-04-05-000Z_11111111-2222-4333-8444-555555555555.jsonl"
cat > "$FILE" <<'FRAMES'
{"type":"session","version":3,"id":"11111111-2222-4333-8444-555555555555"}
{"type":"agent_start","agent":"main"}
{"type":"turn_start","n":1}
{"type":"message_end","message":{"role":"user","content":"ping"}}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"toolCall","id":"t-1","name":"task","arguments":{"description":"scout the module"}}],"stopReason":"toolUse","usage":{"input":1000,"output":200,"cost":{"total":0.03}}}}
{"type":"tool_execution_end","callId":"t-1"}
{"type":"turn_end"}
{"type":"turn_start","n":2}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"all green <promise>DONE</promise>"}],"stopReason":"endTurn","usage":{"input":2000,"output":400,"cost":{"total":0.04}}}}
{"type":"turn_end"}
{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"all green"}]}]}
FRAMES
cat "$FILE"
exit 0
