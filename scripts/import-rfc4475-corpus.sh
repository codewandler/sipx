#!/usr/bin/env bash
#
# Import the SIP torture-test corpus from RFC 4475.
#
# RFC 4475 Appendix A embeds a base64-encoded, gzip-compressed tar archive of every test
# message in the document, precisely so implementers get the messages bit-exactly rather than
# by retyping them out of the rendered text — several cases hinge on octets that do not
# survive transcription (NUL escapes, UTF-8 in display names, trailing whitespace).
#
# This script recovers that archive from the RFC and writes the messages to
# crates/sipx-testkit/corpus/rfc4475/. It is committed so the corpus's provenance is
# reproducible: run it again and the files must be identical.
#
# Usage: scripts/import-rfc4475-corpus.sh [--check]
#          --check   verify the committed corpus matches the RFC; do not write
#
# Exit codes: 0 the corpus matches (or was written), 1 it differs from the RFC, 64 the arguments
# were not understood, 75 the RFC editor could not be reached — see `check_only` and the fetch
# guard below.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/sipx-testkit/corpus/rfc4475"
url="https://www.rfc-editor.org/rfc/rfc4475.txt"

# `EX_USAGE` and `EX_TEMPFAIL` from sysexits(3). The second one is the contract with the gate:
# `scripts/gate.py` reads it as "this step made no claim about the corpus" instead of putting the
# step in the failed tally (`X-58`).
readonly EX_USAGE=64
readonly EX_UNREACHABLE=75

refuse_argument() {
    echo "unknown argument: $1" >&2
    echo "usage: ${BASH_SOURCE[0]##*/} [--check]" >&2
    exit "$EX_USAGE"
}

# X-58: this used to be `[[ "${1:-}" == "--check" ]] && check_only=1`, which made `--check=1`,
# `-check` and every typo select the *other* branch — the one that overwrites the corpus with the
# RFC's own bytes and exits 0. That is a green step that erases the hand edit the check exists to
# catch, and it matters most here: the `fuzz` job's invocation is what proves a campaign deposited
# none of its generated inputs in the seed corpus, and the write path would launder exactly that.
#
# Dispatched on `$#` rather than on `"${1:-}"`, because those two disagree on exactly one input:
# `$1` present and empty. `case "${1:-}" in "")` reads an empty argument as "no argument given"
# and falls through to the write path — so `"$flag"` with `flag` unset silently rewrites the
# corpus and exits 0, which is the failure this whole item is about.
check_only=0
if [[ $# -gt 1 ]]; then
    refuse_argument "$2"
elif [[ $# -eq 1 ]]; then
    case "$1" in
        --check) check_only=1 ;;
        *) refuse_argument "$1" ;;
    esac
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "fetching $url"
# The guard is here to say two things curl does not, and to exit a code that means them.
#
# curl at these flags is not silent — `-S` in `-fsSL` is *show errors*, so it prints e.g.
# `curl: (6) Could not resolve host: www.rfc-editor.org` on stderr. What it cannot say is which
# corpus was being checked, or that the fifty committed files are not what failed. And its exit
# code is about curl: 6, 7, 22, 28 all land in the gate's failed tally as `exit N` beside the
# steps that really did find something wrong with the tree.
#
# So: one sentence naming the corpus and the host, and `EX_TEMPFAIL`, which `scripts/gate.py`
# reads as a step disclaiming its own run rather than reporting on the corpus (`X-58`). It is
# still not a skip — a provenance check that *passes* when it could not reach the RFC is the MSRV
# hole in a second place.
#
# A machine with no `curl` at all lands here too (127), and is deliberately reported the same way:
# it is equally true that the RFC could not be read, and equally false that the corpus drifted.
# The message names the tool as well as the network so the sentence is accurate either way.
if ! curl -fsSL "$url" -o "$work/rfc4475.txt"; then
    echo "could not fetch $url — this says nothing about the committed RFC 4475 corpus, only" >&2
    echo "that the RFC could not be read to compare against it. Check the network and that" >&2
    echo "curl is installed, then run this again; nothing here is a finding about the fifty" >&2
    echo "committed messages." >&2
    exit "$EX_UNREACHABLE"
fi

# Appendix A wraps the archive in "-- BEGIN/END MESSAGE ARCHIVE --". Everything between the
# markers that is a single whitespace-delimited token is base64; page furniture (headers,
# footers, figure captions) has embedded spaces and is dropped by the same rule.
awk '/-- BEGIN MESSAGE ARCHIVE --/,/-- END MESSAGE ARCHIVE --/' "$work/rfc4475.txt" \
    | grep -E '^[[:space:]]*[^[:space:]]+[[:space:]]*$' \
    | grep -vE 'MESSAGE ARCHIVE|^[[:space:]]*(Sparks|RFC|Figure)' \
    | tr -d ' \t' > "$work/archive.b64"

base64 -d "$work/archive.b64" > "$work/archive.tgz"
mkdir -p "$work/out"
tar xzf "$work/archive.tgz" -C "$work/out"

count="$(find "$work/out" -type f -name '*.dat' | wc -l)"
if [[ "$count" -ne 50 ]]; then
    echo "expected 50 messages in the archive, got $count" >&2
    exit 1
fi

if [[ $check_only -eq 1 ]]; then
    if diff -r --brief "$work/out" "$dest" \
        --exclude=README.md >/dev/null; then
        echo "corpus matches RFC 4475 ($count messages)"
    else
        echo "corpus differs from RFC 4475:" >&2
        diff -r --brief "$work/out" "$dest" --exclude=README.md >&2 || true
        exit 1
    fi
    exit 0
fi

mkdir -p "$dest"
cp "$work/out"/*.dat "$dest/"
echo "wrote $count messages to ${dest#"$repo_root"/}"
