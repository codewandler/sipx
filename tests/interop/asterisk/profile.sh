# The second interop peer: a user agent.
#
# Sourced by `run.sh`. Everything peer-specific lives here; `run.sh` owns the lifecycle and the
# test list, and the test list is the same for every peer by design.
#
# Why this peer and not another is argued in `../README.md`. The short version is that it shares
# no code and no reading of the RFCs with the first, and that it answers calls — which the first
# peer, being a proxy, cannot.

PEER_TITLE="a PBX and back-to-back user agent, C, on an independent SIP library"
PEER_IMAGE="${SIPX_ASTERISK_IMAGE:-andrius/asterisk:20.20.1-alpine-3.24}"
PEER_CONTAINER="sipx-asterisk"

# This one answers calls, so it runs the call list as well as the server list.
PEER_ROLES="server user-agent"

# Printed once the modules are loaded and the transports are bound. Anything earlier is a
# process that exists, which is not the same as a peer that will answer.
PEER_READY_MARKER="Asterisk Ready"

# Where this peer can be reached for a call, and where it will place one.
PEER_ENV=(
    "SIPX_INTEROP_ECHO_URI=sip:echo@127.0.0.1:5060"
    "SIPX_INTEROP_UA_PORT=5080"
    "SIPX_INTEROP_ORIGINATE=PJSIP/sipx-ua"
)

# What this peer and sipx disagree about, with the story that settles it. Recorded here rather
# than by rewording a test: a test that is softened until every peer passes it measures the
# peers' intersection, which is the one thing interop testing must not do.
#
# `T-23` — sipx's WebSocket client requests the path `/`. This peer serves SIP over WebSocket
# from its HTTP server at `/ws`, on that server's own port. RFC 7118 §5 does not fix a path, so
# both readings are legal and sipx simply cannot express the other one yet.
PEER_DIVERGES_ON=(
    "registers_against_a_real_server_over_websocket:T-23"
)

peer_prepare() {
    : # every configuration file this peer needs is committed; nothing to generate
}

peer_mounts() {
    printf '%s\n' \
        "$PEER_DIR/pjsip.conf:/etc/asterisk/pjsip.conf:ro" \
        "$PEER_DIR/extensions.conf:/etc/asterisk/extensions.conf:ro" \
        "$PEER_DIR/rtp.conf:/etc/asterisk/rtp.conf:ro" \
        "$PEER_DIR/http.conf:/etc/asterisk/http.conf:ro" \
        "$PEER_DIR/tls:/etc/asterisk/tls:ro"
}

# The same shape of guard the other peer needs, for the same reason: this peer answers on UDP
# whether or not its TLS transport came up, and whether or not its subscriber exists. Either
# failure would read as a bug in sipx.
peer_check() {
    local startup="$1"

    if grep -qE "Failed to (load|create) .*transport-tls|Unable to create SIP channel" <<<"$startup"; then
        echo "!! the TLS transport did not start; the TLS results would be meaningless" >&2
        grep -iE "error|tls" <<<"$startup" >&2 || true
        return 1
    fi

    # `pjsip show endpoint` rather than the log: a configuration object that failed to build is
    # not always an error line, and an endpoint that does not exist authenticates nothing.
    local endpoints
    endpoints="$(docker exec "$PEER_CONTAINER" asterisk -rx "pjsip show endpoints" 2>&1 || true)"
    if ! grep -q "Endpoint:  alice" <<<"$endpoints"; then
        echo "!! the alice endpoint did not load; interop results would be meaningless" >&2
        printf '%s\n' "$endpoints" >&2
        return 1
    fi

    local transports
    transports="$(docker exec "$PEER_CONTAINER" asterisk -rx "pjsip show transports" 2>&1 || true)"
    if ! grep -q "transport-tls" <<<"$transports"; then
        echo "!! the TLS transport is not configured; the TLS results would be meaningless" >&2
        printf '%s\n' "$transports" >&2
        return 1
    fi
}
