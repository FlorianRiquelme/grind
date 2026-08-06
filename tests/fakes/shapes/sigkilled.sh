#!/bin/sh
# Writes a partial response, then SIGKILLs itself mid-write — no chance to close stdout
# cleanly or set an exit code the parent chose. Proves the supervisor still captures
# whatever bytes made it down the pipe before the kill landed.
cat >/dev/null
printf '{"is_error":false,"subtype":"success","result":"writing a long response and then the process just'
kill -9 $$
