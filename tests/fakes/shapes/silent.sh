#!/bin/sh
# Writes nothing at all to stdout — the "zero output" death (attempt 3 of snapper-21 had
# num_turns:0 and effectively no usage).
cat >/dev/null
exit 1
