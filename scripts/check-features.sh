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
    "udp,tcp,tls,quic"
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

# `sipx-media` has its own optional layers, and the same trap: everything RFC 5764 decides is
# compiled whatever the features say, so the crate has to build with the handshake absent as
# well as present. A `dtls`-only path that referred to something behind `opus` would pass
# `--all-features` and fail for everyone who wanted encrypted media without the codec.
media_combinations=(
    ""
    "dtls"
    "opus"
    "dtls,opus"
)

for features in "${media_combinations[@]}"; do
    label="sipx-media ${features:-<none>}"
    printf '  %-24s ' "$label"
    if cargo check --quiet -p sipx-media --no-default-features \
        ${features:+--features "$features"} 2>/tmp/sipx-features.$$; then
        echo "ok"
    else
        echo "FAILED"
        cat /tmp/sipx-features.$$
        status=1
    fi
    rm -f /tmp/sipx-features.$$
done

# `sipx-call` re-exports `sipx-media`'s codec choice as a feature of its own (`M-30`), and the
# variant it adds is `#[cfg]`-gated: `Codecs::Opus` does not exist with the feature off. Every arm
# that names it, and every test that selects it, therefore has to be gated to match — which is a
# thing you get wrong silently, because `--all-features` compiles all of it and CI builds nothing
# else.
#
# `--all-targets`, unlike the checks above, because this crate's exposure is in its *tests*: the
# codec table in `call.rs`'s test module and all of `tests/opus.rs` are conditional, and a `cfg`
# that disagrees with the feature is invisible to a check that only builds the library.
call_combinations=(
    ""
    "opus"
)

for features in "${call_combinations[@]}"; do
    label="sipx-call ${features:-<none>}"
    printf '  %-24s ' "$label"
    if cargo check --quiet -p sipx-call --all-targets --no-default-features \
        ${features:+--features "$features"} 2>/tmp/sipx-features.$$; then
        echo "ok"
    else
        echo "FAILED"
        cat /tmp/sipx-features.$$
        status=1
    fi
    rm -f /tmp/sipx-features.$$
done

# `sipx-media` reuses `sipx_transport::stun` for RFC 5389's header, and takes it with
# `default-features = false` — twenty bytes of header layout must not drag rustls, a WebSocket
# stack and a DNS client behind them into every crate that plays audio. Nothing about the build
# notices if that flag is dropped: everything still compiles, and the cost lands on downstream
# users. So the assertion is on the resolved graph, the same shape as the `sipx-ua` one below.
printf '  %-24s ' "sipx-media stun only"
if cargo tree --quiet -p sipx-media --no-default-features --edges normal --prefix none \
    2>/dev/null | grep -qE '^(tokio-rustls|tokio-tungstenite|hickory-resolver|rustls-native-certs) '; then
    echo "FAILED"
    echo "    sipx-media resolves a SIP transport it only wants a STUN header from:"
    cargo tree -p sipx-media --no-default-features --edges normal --prefix none \
        2>/dev/null | grep -E '^(tokio-rustls|tokio-tungstenite|hickory-resolver|rustls-native-certs) ' \
        | sed 's/^/      /'
    status=1
else
    echo "ok"
fi

# `sipx-ua` carries the digest primitives, and a caller with no async runtime has to be able to
# take them. Only `agent`, `flows` and `error` need one; `auth`, `challenge`, `outbound` and
# `registrar` are hashing and header text.
ua_combinations=(
    ""
    "runtime"
)

for features in "${ua_combinations[@]}"; do
    label="sipx-ua ${features:-<none>}"
    printf '  %-24s ' "$label"
    if cargo check --quiet -p sipx-ua --no-default-features \
        ${features:+--features "$features"} 2>/tmp/sipx-features.$$; then
        echo "ok"
    else
        echo "FAILED"
        cat /tmp/sipx-features.$$
        status=1
    fi
    rm -f /tmp/sipx-features.$$
done

# **That it builds is not the claim.** A runtime-free `sipx-ua` that still resolved `tokio` would
# compile perfectly and be useless for the thing the feature exists for, so the assertion is on the
# resolved graph rather than on the exit code of a build.
printf '  %-24s ' "sipx-ua no runtime dep"
if cargo tree --quiet -p sipx-ua --no-default-features --edges normal --prefix none \
    2>/dev/null | grep -qE '^(tokio|sipx-transport) '; then
    echo "FAILED"
    echo "    a runtime-free sipx-ua still resolves a runtime:"
    cargo tree -p sipx-ua --no-default-features --edges normal --prefix none \
        2>/dev/null | grep -E '^(tokio|sipx-transport) ' | sed 's/^/      /'
    status=1
else
    echo "ok"
fi

if [ "$status" -eq 0 ]; then
    echo "features: every combination builds"
fi
exit "$status"
