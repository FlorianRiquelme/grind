#!/bin/sh
# Dies mid-response: emits half a JSON object, then exits non-zero.
# Real shape: "API Error: Connection closed mid-response." (attempt 1 of the snapper-21 run).
cat >/dev/null # drain stdin (the prompt), like a real child would
printf '{"is_error":true,"subtype":"success","stop_reason":"stop_sequence","total_cost_usd":23.5,"result":"API Error: Connection closed mid-resp'
exit 1
