#!/usr/bin/env bash
#
# Import the AES-GCM test vectors from RFC 7714.
#
# RFC 7714 §16 and §17 publish worked examples of AEAD_AES_128_GCM and AEAD_AES_256_GCM over
# SRTP and SRTCP: the key, the salt, the formed IV, the associated data, the plaintext, the
# ciphertext and the tag, for encryption, decryption, tagging-only and tag verification. They are
# what tells an AES-GCM SRTP transform that is *self-consistently* wrong — one whose IV formation
# or associated-data boundary disagrees with the RFC — from one that interoperates. Two endpoints
# running the same wrong code protect and unprotect each other's packets perfectly.
#
# Unlike RFC 4475 there is no embedded archive to recover, so what is imported is the RFC's own
# text: this script slices §16 and §17 out of the document, drops the running page headers and
# footers, and writes the result to crates/sipx-testkit/corpus/rfc7714/. Nothing is retyped and
# nothing is reformatted — every hex digit a test asserts against is a digit the RFC editor
# serves. `--check` re-slices and diffs, which is what proves a fixture was not quietly adjusted
# to match an implementation that disagreed with it.
#
# Usage: scripts/import-rfc7714-corpus.sh [--check]
#          --check   verify the committed vectors match the RFC; do not write
#
# Exit codes: 0 the vectors match (or were written), 1 they differ from the RFC, 64 the arguments
# were not understood, 75 the RFC editor could not be reached.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/sipx-testkit/corpus/rfc7714"
url="https://www.rfc-editor.org/rfc/rfc7714.txt"

# `EX_USAGE` and `EX_TEMPFAIL` from sysexits(3). The second one is the contract with the gate:
# `scripts/gate.py` reads it as "this step made no claim about the vectors" instead of putting the
# step in the failed tally (`X-58`).
readonly EX_USAGE=64
readonly EX_UNREACHABLE=75

refuse_argument() {
    echo "unknown argument: $1" >&2
    echo "usage: ${BASH_SOURCE[0]##*/} [--check]" >&2
    exit "$EX_USAGE"
}

# Dispatched on `$#` rather than on `"${1:-}"`, for the reason spelled out in
# import-rfc4475-corpus.sh: those two disagree on exactly one input — `$1` present and empty —
# and the branch they disagree about is the one that overwrites the fixtures and exits 0.
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
# The same guard, and the same exit code, as the RFC 4475 importer: one sentence naming what could
# not be read, and `EX_TEMPFAIL`, which the gate reads as a step disclaiming its own run rather
# than as a finding about the committed vectors. A machine with no `curl` lands here too and is
# reported the same way, because it is equally true that the RFC could not be read.
if ! curl -fsSL "$url" -o "$work/rfc7714.txt"; then
    echo "could not fetch $url — this says nothing about the committed RFC 7714 vectors, only" >&2
    echo "that the RFC could not be read to compare against them. Check the network and that" >&2
    echo "curl is installed, then run this again; nothing here is a finding about the two" >&2
    echo "committed vector files." >&2
    exit "$EX_UNREACHABLE"
fi

# One section per file, sliced between its own heading and the next one. The three filters after
# the slice are the page furniture and nothing else: the running footer, the running header, and
# the form feed between them. `cat -s` then collapses the blank run a page break leaves behind —
# no line inside a vector block is ever blank twice over, so this cannot merge two of them.
slice() {
    local from="$1" to="$2" out="$3"
    awk -v from="$from" -v to="$to" '$0 == from {f = 1} $0 == to {f = 0} f' "$work/rfc7714.txt" \
        | grep -vE '^(McGrew & Igoe|RFC 7714)' \
        | tr -d '\f' \
        | cat -s > "$out"
}

mkdir -p "$work/out"
slice "16.  Some RTP Test Vectors" "17.  RTCP Test Vectors" "$work/out/rtp-vectors.txt"
slice "17.  RTCP Test Vectors" "18.  References" "$work/out/rtcp-vectors.txt"

# A slice that found nothing is an empty file, which would diff clean against an empty committed
# one and against nothing else. Both files are checked for the last line of their own section, so
# a truncated fetch or a renumbered document is a failure here rather than a green step over a
# corpus that no longer says anything.
for file in rtp-vectors.txt rtcp-vectors.txt; do
    if ! grep -q 'Received tag verified\.' "$work/out/$file"; then
        echo "$file does not end in a completed vector; the slice found no section 16/17" >&2
        exit 1
    fi
done
for expected in "16.2.4.  SRTP AEAD_AES_256_GCM Tag Verification:rtp-vectors.txt" \
                "17.4.  SRTCP AEAD_AES_256_GCM Tag Verification:rtcp-vectors.txt"; do
    heading="${expected%:*}"
    file="${expected##*:}"
    if ! grep -qF "$heading" "$work/out/$file"; then
        echo "$file is missing '$heading'" >&2
        exit 1
    fi
done

if [[ $check_only -eq 1 ]]; then
    if diff -r --brief "$work/out" "$dest" --exclude=README.md >/dev/null; then
        echo "vectors match RFC 7714 (sections 16 and 17)"
    else
        echo "vectors differ from RFC 7714:" >&2
        diff -r --brief "$work/out" "$dest" --exclude=README.md >&2 || true
        exit 1
    fi
    exit 0
fi

mkdir -p "$dest"
cp "$work/out"/*.txt "$dest/"
echo "wrote the section 16 and 17 vectors to ${dest#"$repo_root"/}"
