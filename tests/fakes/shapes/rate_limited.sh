#!/bin/sh
# Run 2's real rate-limited response, byte-for-byte from
# tests/fixtures/run2/rate-limited.stdout.json — the only recorded copy of the session-limit
# shape. Its prose matches none of the script's phrases; only the 429 classifies it.
cat >/dev/null
cat "${HOME:?}/.fake/rate-limited.stdout.json"
exit 1
