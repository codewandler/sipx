#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$root"
proof_root="${RUNNER_TEMP:?RUNNER_TEMP is required}/sipx-browser-sdk"
chrome="${CHROME_BIN:-$(command -v google-chrome)}"
if [[ -n ${CHROMEWEBDRIVER:-} ]]; then
    driver="${CHROMEWEBDRIVER%/}/chromedriver"
else
    driver="$(command -v chromedriver)"
fi
cargo build -p sipx-call --example browser_sdk_proof --features dtls,opus
python3 scripts/pack-browser.py --output "$proof_root/package"
python3 tests/browser-sdk/prove.py \
    --archive "$proof_root/package/sipx-browser-0.1.0.tgz" \
    --output "$proof_root/proof" --chrome "$chrome" --driver "$driver"
