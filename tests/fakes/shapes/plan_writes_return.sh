#!/bin/sh
# The Plan stage completing honestly: writes its own `plan-facts.json` and its return under the
# stages directory the context block named (unit C's ladder walk reads returns off disk, never
# off a session's claim), then answers like an ordinary successful session.
F="${HOME:?}/.fake"
STAGES=$(sed -n 's/^Stages directory:  *//p' "$F/last-prompt.txt" | sed -n '1p')
[ -n "$STAGES" ] || exit 1
mkdir -p "$STAGES/plan"
cat >"$STAGES/plan/plan-facts.json" <<'EOF'
{"step_count":2,"forecast_paths":["src/lib.rs"],"new_module_count":0}
EOF
cat >"$STAGES/plan.return.json" <<'EOF'
{"status":"complete"}
EOF
cat <<'EOF'
{"is_error":false,"subtype":"success","stop_reason":"end_turn","total_cost_usd":1.5,"result":"The anchor plan is written."}
EOF
exit 0
