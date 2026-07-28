#!/usr/bin/env bash
# Build every feature combination that a downstream user might actually select.
#
# `--all-features` is not enough, and the gap is not theoretical: `tokio::select!` cannot
# compile a branch out behind a `#[cfg]`, so an optional transport's branch happily referred to
# a field that only existed with that feature on. Everything built, every test passed, and the
# crate did not compile for anyone who turned TLS off.
#
# The combinations here are the ones that mean something — a transport on its own, each
# optional layer added — rather than the full power set, which would be slow and mostly
# duplicated work.
set -euo pipefail

# The same flags CI builds with. Without this the script is a weaker check than the job that
# runs it: an unused import behind a disabled feature is a warning here and an error there, so
# a release can pass locally and fail on push — which is exactly how it went the first time.
export RUSTFLAGS="${RUSTFLAGS:--D warnings}"

combinations=(
    ""
    "udp"
    "tcp"
    "udp,tcp"
    "dns"
    "udp,tcp,dns"
    "udp,tcp,tls"
    "udp,tcp,ws"
    "udp,tcp,tls,ws,wss"
)

status=0
for features in "${combinations[@]}"; do
    label="${features:-<none>}"
    printf '  %-24s ' "$label"
    if cargo check --quiet -p sipx-transport --no-default-features \
        ${features:+--features "$features"} 2>/tmp/sipx-features.$$; then
        echo "ok"
    else
        echo "FAILED"
        cat /tmp/sipx-features.$$
        status=1
    fi
    rm -f /tmp/sipx-features.$$
done

if [ "$status" -eq 0 ]; then
    echo "features: every combination builds"
fi
exit "$status"
