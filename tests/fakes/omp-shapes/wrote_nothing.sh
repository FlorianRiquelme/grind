#!/bin/sh
# A child that streamed frames and allocated no session transcript anywhere. Both the primary
# arm and the fallback come back empty, which must be loud in the Attempt (`WroteNone`, an
# unobservable fan-out) and never a silent zero.
cat <<'FRAMES'
{"type":"turn_start","n":1}
{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"nothing was persisted"}],"stopReason":"endTurn","usage":{"input":10,"output":20,"cost":{"total":0.005}}}}
{"type":"turn_end"}
{"type":"agent_end","messages":[{"role":"assistant","content":[{"type":"text","text":"nothing was persisted"}]}]}
FRAMES
exit 0
