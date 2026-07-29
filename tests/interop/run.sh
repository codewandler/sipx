#!/usr/bin/env bash
# Run the interop tests against real SIP implementations in Docker.
#
# These are the only tests in this repo that prove sipx talks to something it did not also
# write. Everything else is sipx agreeing with itself, which is exactly the kind of agreement
# that survives a wrong shared assumption — and one peer only narrows that risk, because a
# single peer's reading of a sentence is still a single reading.
#
# A peer is a directory beside this script containing a `profile.sh`. Adding one is adding a
# profile; nothing in this file names an image, a container or a configuration directory.
#
# Usage:
#   ./tests/interop/run.sh                       every peer
#   ./tests/interop/run.sh --peer asterisk       one peer
#   ./tests/interop/run.sh --list                what peers exist
#   ./tests/interop/run.sh -- some_test_name     extra arguments for `cargo test`
#
# Environment:
#   SIPX_KEEP_SERVER=1   leave the container running for poking at
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# The test list. Deliberately here and not in a profile: the list is the contract every peer is
# measured against, and a list a peer could edit measures the peer's opinion of itself. A peer
# says which *roles* it can play; it does not say which tests apply to the role.
#
#   server      registration, digest, refresh, a wrong password, OPTIONS, TLS, WebSocket
#   user-agent  a call answered, a call placed, SDP negotiated, audio, BYE
declare -A ROLE_TESTS=(
    [server]="-p sipx-ua --test interop"
    [user-agent]="-p sipx-cli --test interop_call"
)
ROLE_ORDER=(server user-agent)

# ------------------------------------------------------------------------ the peer list ----
available_peers() {
    local profile
    for profile in "$HERE"/*/profile.sh; do
        [[ -e "$profile" ]] || continue
        basename "$(dirname "$profile")"
    done | sort
}

# ---------------------------------------------------------------------------- arguments ----
peers=()
cargo_args=()
while [[ $# -gt 0 ]]; do
    case "$1" in
    --peer)
        peers+=("${2:?--peer needs a name}")
        shift 2
        ;;
    --peer=*)
        peers+=("${1#*=}")
        shift
        ;;
    --list)
        available_peers
        exit 0
        ;;
    --)
        shift
        cargo_args+=("$@")
        break
        ;;
    *)
        # Bare arguments are test-name filters, as they were before this script grew peers.
        cargo_args+=("$1")
        shift
        ;;
    esac
done

if [[ ${#peers[@]} -eq 0 ]]; then
    mapfile -t peers < <(available_peers)
fi
if [[ ${#peers[@]} -eq 0 ]]; then
    echo "no peer profiles found under $HERE" >&2
    exit 1
fi

# ----------------------------------------------------------------------- one peer's turn ----
# Containers are torn down by label rather than by name. The name lives in the profile, and a
# cleanup path that has to reach into the profile to find out what to remove is a cleanup path
# that leaks a container the day a profile is written slightly differently.
LABEL="sipx-interop=1"
cleanup() {
    [[ "${SIPX_KEEP_SERVER:-0}" == "1" ]] && return 0
    local stale
    stale="$(docker ps -aq --filter "label=$LABEL" 2>/dev/null || true)"
    [[ -n "$stale" ]] && docker rm -f $stale >/dev/null 2>&1
    return 0
}
trap cleanup EXIT

# Read the log once into a variable rather than piping `docker logs` into each check.
#
# Not a style preference. Under `set -o pipefail`, `docker logs | grep -q pattern` returns
# *failure on a match*: `grep -q` exits the instant it finds one, `docker logs` takes SIGPIPE,
# and pipefail reports the pipeline by that. Every guard below would then fire exactly when the
# thing it looks for is present, which is the most misleading way for a check to be wrong.
logs() { docker logs "$PEER_CONTAINER" 2>&1 || true; }

run_peer() {
    local peer="$1"
    # Each profile is sourced in a fresh subshell, so one peer's variables and functions cannot
    # leak into the next one's run.
    (
        set -euo pipefail

        PEER_DIR="$HERE/$peer"
        PEER_ROLES="server"
        PEER_ENV=()
        PEER_DIVERGES_ON=()
        # shellcheck disable=SC1090  # the profile is chosen at runtime, by design
        source "$PEER_DIR/profile.sh"

        echo
        echo "======================================================================"
        echo "==> peer: $peer — ${PEER_TITLE:-no description}"
        echo "======================================================================"

        # The certificate comes from the same fixture authority the unit tests use, issued for
        # the name every peer is known by. Generated per run rather than committed: a
        # certificate in the repository is a private key in the repository, and one with a fixed
        # expiry is a test that starts failing on a date nobody chose.
        echo "==> issuing the interop certificate"
        rm -rf "$PEER_DIR/tls"
        cargo run --quiet -p sipx-testkit --example issue-certs -- "$PEER_DIR/tls" sipx.test

        peer_prepare

        echo "==> starting $PEER_IMAGE as $PEER_CONTAINER"
        docker rm -f "$PEER_CONTAINER" >/dev/null 2>&1 || true
        local mounts=() mount
        while IFS= read -r mount; do mounts+=(-v "$mount"); done < <(peer_mounts)
        docker run -d --name "$PEER_CONTAINER" --label "$LABEL" --network host \
            "${mounts[@]}" "$PEER_IMAGE" >/dev/null

        echo "==> waiting for it to listen"
        local ready=0 _
        for _ in $(seq 1 60); do
            if grep -q "$PEER_READY_MARKER" <<<"$(logs)"; then
                ready=1
                break
            fi
            sleep 0.5
        done
        if [[ $ready -eq 0 ]]; then
            echo "!! $peer never reported '$PEER_READY_MARKER'; it is not listening" >&2
            tail -40 <<<"$(logs)" >&2
            return 1
        fi

        peer_check "$(logs)"

        # The tests need the fixture authority to trust, which is the only thing that makes the
        # TLS result mean anything: without it every certificate would be refused and the
        # negative tests would pass for the wrong reason.
        export SIPX_INTEROP_CA="$PEER_DIR/tls/ca.pem"
        export SIPX_INTEROP_PEER="$peer"
        export SIPX_INTEROP_CONTAINER="$PEER_CONTAINER"
        local assignment
        for assignment in ${PEER_ENV+"${PEER_ENV[@]}"}; do export "${assignment?}"; done

        # A divergence is skipped by name and announced, never reworded. A test softened until
        # every peer passes it measures the intersection of the peers, which is the one thing
        # an interop suite must not do.
        local skips=() divergence
        for divergence in ${PEER_DIVERGES_ON+"${PEER_DIVERGES_ON[@]}"}; do
            skips+=(--skip "${divergence%%:*}")
            echo "==> known divergence: ${divergence%%:*} — see ${divergence##*:}"
        done

        local role
        for role in "${ROLE_ORDER[@]}"; do
            [[ " $PEER_ROLES " == *" $role "* ]] || continue
            echo "==> running the $role tests against $peer"
            # shellcheck disable=SC2086  # the role's target words are meant to split
            cargo test ${ROLE_TESTS[$role]} -- --ignored --test-threads=1 \
                ${skips+"${skips[@]}"} ${cargo_args+"${cargo_args[@]}"}
        done
    )
}

# ------------------------------------------------------------------------------- the run ----
failed=()
for peer in "${peers[@]}"; do
    if [[ ! -r "$HERE/$peer/profile.sh" ]]; then
        echo "no such peer: $peer (have: $(available_peers | paste -sd ' ' -))" >&2
        exit 1
    fi
    if run_peer "$peer"; then
        echo "==> $peer: ok"
    else
        echo "==> $peer: FAILED" >&2
        failed+=("$peer")
    fi
    cleanup
done

echo
if [[ ${#failed[@]} -gt 0 ]]; then
    echo "interop: FAILED against ${failed[*]}" >&2
    exit 1
fi
echo "interop: every peer agreed (${peers[*]})"
