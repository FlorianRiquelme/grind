#!/bin/sh
# The terminal-good shape: valid JSON, is_error:false, result carries the DONE promise.
cat >/dev/null
cat <<'EOF'
{"is_error":false,"subtype":"success","stop_reason":"end_turn","total_cost_usd":3.18,"result":"PR is open, stopping here. <promise>DONE</promise>"}
EOF
exit 0
