#!/usr/bin/env bash
# Run the interop tests against a real SIP server in Docker.
#
# These are the only tests in this repo that prove sipx talks to something it did not also
# write. Everything else is sipx agreeing with itself, which is exactly the kind of agreement
# that survives a wrong shared assumption.
set -euo pipefail

IMAGE="${SIPX_KAMAILIO_IMAGE:-kamailio/kamailio-ci:5.5.2-alpine}"
NAME="sipx-kamailio"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

cleanup() {
    if [[ "${SIPX_KEEP_SERVER:-0}" != "1" ]]; then
        docker rm -f "$NAME" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

# ha1 and ha1b are generated rather than committed: db_text rejects a row with a null column,
# and a table it cannot load authenticates nothing while still answering — which looks exactly
# like a digest bug in the client.
generate_dbtext() {
    local dir="$1" ha1 ha1b
    mkdir -p "$dir"
    ha1=$(printf 'alice:sipx.test:Circle Of Life' | md5sum | cut -d' ' -f1)
    ha1b=$(printf 'alice@sipx.test:sipx.test:Circle Of Life' | md5sum | cut -d' ' -f1)
    printf 'username(str) domain(str) password(str) ha1(str) ha1b(str)\n' > "$dir/subscriber"
    printf 'alice:sipx.test:Circle Of Life:%s:%s\n' "$ha1" "$ha1b" >> "$dir/subscriber"
    printf 'table_name(str) table_version(int)\nsubscriber:7\n' > "$dir/version"
}

echo "==> starting $IMAGE"
docker rm -f "$NAME" >/dev/null 2>&1 || true
generate_dbtext "$HERE/kamailio/dbtext"
docker run -d --name "$NAME" --network host \
    -v "$HERE/kamailio/kamailio.cfg:/etc/kamailio/kamailio.cfg:ro" \
    -v "$HERE/kamailio/dbtext:/etc/kamailio/dbtext:ro" \
    "$IMAGE" >/dev/null

echo "==> waiting for it to listen"
for _ in $(seq 1 30); do
    if docker logs "$NAME" 2>&1 | grep -q "io_listen_loop"; then break; fi
    sleep 0.5
done

# A table that failed to load makes every authentication fail, and the tests would blame the
# client. Fail loudly here instead.
if docker logs "$NAME" 2>&1 | grep -q "does not exist"; then
    echo "!! the subscriber table did not load; interop results would be meaningless" >&2
    docker logs "$NAME" 2>&1 | grep -i error >&2
    exit 1
fi

echo "==> running interop tests"
cargo test -p sipx-ua --test interop -- --ignored --test-threads=1 "$@"
