#!/bin/sh
# A working Attempt that records a permission denial and adds no commits — the shape a Run
# takes when it keeps reaching for the one operation the barrier refuses. Run 2's single denial
# was exactly this invocation, on the Attempt that opened its PR; repeated with no progress it
# is an obstacle only a human can clear.
cat >/dev/null
cat <<'EOF'
{"is_error":true,"subtype":"success","stop_reason":"stop_sequence","terminal_reason":"api_error","total_cost_usd":4.10,"num_turns":31,"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"git push --force-with-lease origin HEAD"}}],"result":"The push was refused at the tool layer and I cannot proceed without it."}
EOF
exit 1
