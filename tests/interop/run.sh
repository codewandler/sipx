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

# The certificate comes from the same fixture authority the unit tests use, issued for the name
# the registrar is known by. Generated per run rather than committed: a certificate in the
# repository is a private key in the repository, and one with a fixed expiry is a test that
# starts failing on a date nobody chose.
echo "==> issuing the interop certificate"
rm -rf "$HERE/kamailio/tls"
cargo run --quiet -p sipx-testkit --example issue-certs -- "$HERE/kamailio/tls" sipx.test

echo "==> starting $IMAGE"
docker rm -f "$NAME" >/dev/null 2>&1 || true
generate_dbtext "$HERE/kamailio/dbtext"
docker run -d --name "$NAME" --network host \
    -v "$HERE/kamailio/kamailio.cfg:/etc/kamailio/kamailio.cfg:ro" \
    -v "$HERE/kamailio/dbtext:/etc/kamailio/dbtext:ro" \
    -v "$HERE/kamailio/tls:/etc/kamailio/tls:ro" \
    "$IMAGE" >/dev/null

# Read the log once into a variable rather than piping `docker logs` into each check.
#
# Not a style preference. Under `set -o pipefail`, `docker logs | grep -q pattern` returns
# *failure on a match*: `grep -q` exits the instant it finds one, `docker logs` takes SIGPIPE,
# and pipefail reports the pipeline by that. Every guard below would then fire exactly when the
# thing it looks for is present, which is the most misleading way for a check to be wrong.
logs() { docker logs "$NAME" 2>&1 || true; }

echo "==> waiting for it to listen"
for _ in $(seq 1 30); do
    if grep -q "io_listen_loop" <<<"$(logs)"; then break; fi
    sleep 0.5
done

startup="$(logs)"

# A table that failed to load makes every authentication fail, and the tests would blame the
# client. Fail loudly here instead.
if grep -q "does not exist" <<<"$startup"; then
    echo "!! the subscriber table did not load; interop results would be meaningless" >&2
    grep -i error <<<"$startup" >&2 || true
    exit 1
fi

# Same reasoning for TLS: a server that failed to load its certificate still answers on UDP,
# and the TLS tests would then read as a sipx handshake bug.
if ! grep -q "private_key='/etc/kamailio/tls/server.key'" <<<"$startup"; then
    echo "!! TLS did not load its key; the TLS results would be meaningless" >&2
    tail -40 <<<"$startup" >&2
    exit 1
fi

echo "==> running interop tests"
# The tests need the fixture authority to trust, which is the only thing that makes the TLS
# result mean anything: without it every certificate would be refused and the negative tests
# would pass for the wrong reason.
export SIPX_INTEROP_CA="$HERE/kamailio/tls/ca.pem"
cargo test -p sipx-ua --test interop -- --ignored --test-threads=1 "$@"
