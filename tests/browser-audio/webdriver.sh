#!/usr/bin/env bash
# Start the browser driver's W3C endpoint on the loopback port the proof runner owns.
set -euo pipefail

if [[ -n ${CHROMEWEBDRIVER:-} && -x ${CHROMEWEBDRIVER%/}/chromedriver ]]; then
    executable=${CHROMEWEBDRIVER%/}/chromedriver
elif command -v chromedriver >/dev/null 2>&1; then
    executable=$(command -v chromedriver)
else
    printf 'browser-audio proof: no compatible WebDriver executable was found\n' >&2
    exit 1
fi

exec "$executable" --port=9515 --allowed-ips=127.0.0.1
