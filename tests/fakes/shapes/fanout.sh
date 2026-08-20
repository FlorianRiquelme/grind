#!/bin/sh
# A working Attempt that **fans out to two subagents and sees both return**, appending to the
# Run's one transcript exactly as a real session does — the session id is fixed at dispatch and
# every later Attempt resumes it, so this file grows and is never replaced.
#
# The shape exists to make *fan-out recorded per Attempt* (R51) observable across more than one
# Attempt of one Run, which is the case that made the cumulative count invisible.
cat >/dev/null
F="${HOME:?}/.fake"
T=$(cat "$F/transcript" 2>/dev/null)
N=$(cat "$F/counter" 2>/dev/null || echo 0)
if [ -n "$T" ]; then
  mkdir -p "$(dirname "$T")"
  for i in 1 2; do
    printf '%s\n' "{\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"toolu_${N}_${i}\",\"name\":\"Task\",\"input\":{\"description\":\"a subagent\"}}]}}" >> "$T"
    printf '%s\n' "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"toolu_${N}_${i}\",\"content\":\"done\"}]}}" >> "$T"
  done
fi
cat <<'EOF'
{"is_error":false,"subtype":"success","total_cost_usd":2.50,"num_turns":24,"result":"Made progress on the plan."}
EOF
