# The first interop peer: a proxy/registrar.
#
# Sourced by `run.sh`. Everything peer-specific lives here; `run.sh` owns the lifecycle and the
# test list, and the test list is the same for every peer by design.

PEER_TITLE="a proxy/registrar, C, descended from a SIP router lineage"
PEER_IMAGE="${SIPX_KAMAILIO_IMAGE:-kamailio/kamailio-ci:5.5.2-alpine}"
PEER_CONTAINER="sipx-kamailio"

# What this peer can be asked to do. `server` is the registrar/OPTIONS list; `user-agent` adds
# the call list, which needs something that answers an INVITE with SDP and carries audio.
# A proxy has no dialplan and cannot answer, so it declares only the first.
PEER_ROLES="server"

# The line that proves the process is listening, not merely started.
PEER_READY_MARKER="io_listen_loop"

# Generated rather than committed. `db_text` rejects a row with a null column, and a table it
# cannot load authenticates nothing while still answering — which looks exactly like a digest
# bug in the client.
peer_prepare() {
    local dir="$PEER_DIR/dbtext" ha1 ha1b
    mkdir -p "$dir"
    ha1=$(printf 'alice:sipx.test:Circle Of Life' | md5sum | cut -d' ' -f1)
    ha1b=$(printf 'alice@sipx.test:sipx.test:Circle Of Life' | md5sum | cut -d' ' -f1)
    printf 'username(str) domain(str) password(str) ha1(str) ha1b(str)\n' >"$dir/subscriber"
    printf 'alice:sipx.test:Circle Of Life:%s:%s\n' "$ha1" "$ha1b" >>"$dir/subscriber"
    printf 'table_name(str) table_version(int)\nsubscriber:7\n' >"$dir/version"
}

peer_mounts() {
    printf '%s\n' \
        "$PEER_DIR/kamailio.cfg:/etc/kamailio/kamailio.cfg:ro" \
        "$PEER_DIR/dbtext:/etc/kamailio/dbtext:ro" \
        "$PEER_DIR/tls:/etc/kamailio/tls:ro"
}

# Guards against a peer that started but cannot do the thing under test. Both of these failed
# for real once, and both looked like a bug in sipx.
peer_check() {
    local startup="$1"

    # A table that failed to load makes every authentication fail, and the tests would blame
    # the client.
    if grep -q "does not exist" <<<"$startup"; then
        echo "!! the subscriber table did not load; interop results would be meaningless" >&2
        grep -i error <<<"$startup" >&2 || true
        return 1
    fi

    # Same reasoning for TLS: a server that failed to load its certificate still answers on
    # UDP, and the TLS tests would then read as a sipx handshake bug.
    if ! grep -q "private_key='/etc/kamailio/tls/server.key'" <<<"$startup"; then
        echo "!! TLS did not load its key; the TLS results would be meaningless" >&2
        tail -40 <<<"$startup" >&2
        return 1
    fi
}
