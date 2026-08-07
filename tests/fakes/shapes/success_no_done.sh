#!/bin/sh
# Exits clean, valid JSON, is_error:false — but no <promise>DONE</promise>. This is attempt
# 4 of snapper-21: it succeeded as a Claude Code invocation while the pipeline itself was
# still mid-flight, so the supervisor must re-enter again rather than call it done.
cat >/dev/null
cat <<'EOF'
{"is_error":false,"subtype":"success","stop_reason":"end_turn","total_cost_usd":11.74,"result":"Made progress but the pipeline has not reached an open PR yet."}
EOF
exit 0
