#!/usr/bin/env bash
# Runs a command; on failure puts the end of its output into a GitHub annotation.
set -uo pipefail
log="$(mktemp)"
"$@" 2>&1 | tee "$log"
code=$?
if [[ $code -ne 0 ]]; then
  grep -v 'rerun-if\|Compiling\|= None' "$log" | tail -n 40 | cut -c1-300 | sed 's/%/%25/g' | awk 'BEGIN { printf "::error::" } { printf "%s%%0A", $0 } END { print "" }'
fi
exit $code
