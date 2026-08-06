#!/bin/sh
# Valid JSON, exit 1 — but subtype says "success". This is the subtle shape from the real
# run: attempts 1-3 of snapper-21 all had is_error:true, exit_code:1, subtype:"success".
cat >/dev/null
cat <<'EOF'
{"is_error":true,"subtype":"success","stop_reason":"stop_sequence","api_error_status":null,"terminal_reason":"api_error","total_cost_usd":2.35,"result":"API Error: Connection closed mid-response. The response above may be incomplete."}
EOF
exit 1
