#!/bin/sh
# The wall: the stream is cut mid-frame, the prose lands on the child's own stderr file and the
# exit code is not zero. `classify`'s stderr needle path is reached by literals today and by no
# process; this is the process.
FILE="$SESSION_DIR/2026-01-02T03-04-05-000Z_44444444-3333-4222-8111-000000000000.jsonl"
printf '%s\n' '{"type":"session","version":3,"id":"44444444-3333-4222-8111-000000000000"}' > "$FILE"
cat "$FILE"
printf '{"type":"turn_start","n":1'
printf 'You have reached your usage limit. Try again later.\n' >&2
exit 1
