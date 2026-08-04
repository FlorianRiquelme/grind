#!/bin/sh
# Rate-limit-shaped error: is_error:true, spread across api_error_status/terminal_reason.
cat >/dev/null
cat <<'EOF'
{"is_error":true,"subtype":"error_max_turns","stop_reason":null,"api_error_status":"429","terminal_reason":"Usage limit reached, resets at 2026-08-05T00:00:00Z","result":"You have hit the usage limit for this plan."}
EOF
exit 1
