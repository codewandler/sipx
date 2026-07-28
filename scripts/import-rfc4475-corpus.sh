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

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/sipx-testkit/corpus/rfc4475"
url="https://www.rfc-editor.org/rfc/rfc4475.txt"

check_only=0
[[ "${1:-}" == "--check" ]] && check_only=1

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "fetching $url"
curl -fsSL "$url" -o "$work/rfc4475.txt"

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
