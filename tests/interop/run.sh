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
#   server          registration, digest, refresh, a wrong password, OPTIONS, TLS, WebSocket
#   user-agent      a call answered, a call placed, SDP negotiated, audio, BYE
#   media-security  a call whose media is encrypted, keyed the way the peer declares below
#   opus-audio      Opus-only calls in both offer/answer roles with decoded signal evidence
declare -A ROLE_TESTS=(
    [server]="-p sipx-ua --test interop"
    [user-agent]="-p sipx-cli --test interop_call"
    [media-security]="-p sipx-cli --features dtls --test interop_srtp"
    [opus-audio]="-p sipx-cli --features opus --test interop_opus"
)
ROLE_ORDER=(server user-agent media-security opus-audio)

# ---------------------------------------------------------------------------- the keyings ----
# SRTP can be keyed two ways and they share no code path: SDES carries the master key in the SDP
# (RFC 4568), DTLS-SRTP handshakes on the media path and the SDP carries only a fingerprint
# (RFC 5764). A peer that does one need not do the other, so the `media-security` role is run
# per keying and a peer declares which it can do in `PEER_KEYINGS`.
#
# The map lives here and not in a profile for the reason the role list does: it is the contract
# peers are measured against. A peer says which keyings it *supports*; it does not get to say
# which test that means.
declare -A KEYING_TESTS=(
    [sdes]=a_real_peer_accepts_media_sipx_encrypted_with_sdes
    [dtls]=a_real_peer_accepts_media_sipx_encrypted_with_dtls_srtp
)
KEYING_ORDER=(sdes dtls)

declare -A KEYING_UNIMPLEMENTED=()

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

# ------------------------------------------------------------------- one run at a time ----
# Everything a run reserves is machine-global: the peer runs on the host network under one fixed
# container name, on fixed ports, and the call tests bind a fixed port of their own because the
# peer's contact is written into its configuration before any test starts. Start-up removes that
# container by name and cleanup removes every container carrying the label above — neither asks
# whether another run is using it.
#
# So two runs on one machine are not two runs. `X-23` measured what the second one does to the
# first: its start-up deletes the peer out from under a call in progress, and both call tests
# then fail on their twenty-second timeout with nothing on the far end. Nine times in ten when
# the two overlap. That is the flake that was reported as one run in five, and nothing about it
# is a race inside a test.
#
# A run therefore holds an exclusive lock for its whole life, and a second run waits its turn
# instead of destroying the first. The lock is machine-global because what it guards is.
LOCK_FILE="${SIPX_INTEROP_LOCK:-${TMPDIR:-/tmp}/sipx-interop.lock}"
if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    if ! flock -n 9; then
        echo "==> another interop run holds the peer; waiting for it to finish"
        flock 9
    fi
else
    # Refusing here would make the harness unrunnable where `flock` is absent; saying nothing
    # would restore the silent collision. So it runs, and says what is not being guarded.
    echo "!! flock is unavailable: a concurrent interop run on this machine would collide" >&2
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

        if ! peer_check "$(logs)"; then
            # `run_peer` is called as an `if` condition below, so Bash disables `errexit` in this
            # whole function. Make a failed capability guard terminal explicitly: continuing
            # would let later tests turn "the required peer feature is absent" into a green run.
            return 1
        fi

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

        # Which keyings this peer can be measured on, said out loud. Three different things can
        # stop a keying's test from running and a summary that showed none of them would be the
        # same blindness this role exists to end: the peer cannot do it, sipx cannot offer it,
        # or it ran. Each is printed by name.
        if [[ " $PEER_ROLES " == *" media-security "* ]]; then
            local keying
            for keying in "${KEYING_ORDER[@]}"; do
                if [[ " ${PEER_KEYINGS[*]-} " != *" $keying "* ]]; then
                    echo "==> keying $keying: not supported by $peer; ${KEYING_TESTS[$keying]} is not run"
                    skips+=(--skip "${KEYING_TESTS[$keying]}")
                elif [[ -n "${KEYING_UNIMPLEMENTED[$keying]-}" ]]; then
                    echo "==> keying $keying: $peer supports it, sipx does not offer it — ${KEYING_UNIMPLEMENTED[$keying]}"
                    skips+=(--skip "${KEYING_TESTS[$keying]}")
                else
                    echo "==> keying $keying: supported by $peer, exercised by ${KEYING_TESTS[$keying]}"
                fi
            done
        fi

        local role
        for role in "${ROLE_ORDER[@]}"; do
            [[ " $PEER_ROLES " == *" $role "* ]] || continue
            echo "==> running the $role tests against $peer"
            # shellcheck disable=SC2086  # the role's target words are meant to split
            if ! cargo test ${ROLE_TESTS[$role]} -- --ignored --test-threads=1 \
                ${skips+"${skips[@]}"} ${cargo_args+"${cargo_args[@]}"}; then
                # `run_peer` is called as an `if` condition below. Bash disables `errexit` inside
                # a function used that way, so relying on `set -e` lets a failed Cargo test fall
                # through the loop and become the status of a later successful command (`X-62`).
                return 1
            fi
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
