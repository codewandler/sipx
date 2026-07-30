#!/usr/bin/env bash
#
# Import the IPv6 torture-test corpus from RFC 5118.
#
# RFC 5118 Appendix A embeds "an encoded, gzip compressed TAR archive of files that represent
# each of the example messages discussed in Section 4", precisely so implementers get the
# messages bit-exactly rather than by retyping them out of the rendered text. For an IPv6
# corpus that matters more than usual, not less: every case turns on the exact placement of
# ':', '[' and ']', and the RFC's own body text wraps two of the messages across lines with an
# <allOneLine> convention that a transcriber has to unwrap by hand. The archived files are
# already unwrapped.
#
# This script recovers that archive from the RFC and writes the messages to
# crates/sipx-testkit/corpus/rfc5118/. It is committed so the corpus's provenance is
# reproducible: run it again and the files must be identical.
#
# The file names are the archive's own — no extension is added. RFC 5118 refers to each message
# by that name ("Message Details: ipv6-good"), so keeping them verbatim is what lets a reader
# match a fixture to the section that describes it.
#
# Usage: scripts/import-rfc5118-corpus.sh [--check]
#          --check   verify the committed corpus matches the RFC; do not write

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/sipx-testkit/corpus/rfc5118"
url="https://www.rfc-editor.org/rfc/rfc5118.txt"

check_only=0
[[ "${1:-}" == "--check" ]] && check_only=1

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "fetching $url"
curl -fsSL "$url" -o "$work/rfc5118.txt"

# Appendix A wraps the archive in "-- BEGIN/END MESSAGE ARCHIVE --" and documents the recovery
# rule itself: of the lines between the markers, keep those that are a single whitespace-
# delimited token. Page furniture (running headers, footers) has embedded spaces and is dropped
# by the same rule, as are the marker lines themselves.
awk '/-- BEGIN MESSAGE ARCHIVE --/,/-- END MESSAGE ARCHIVE --/' "$work/rfc5118.txt" \
    | grep -E '^[[:space:]]*[^[:space:]]+[[:space:]]*$' \
    | tr -d ' \t' > "$work/archive.b64"

base64 -d "$work/archive.b64" > "$work/archive.tgz"
mkdir -p "$work/out"
tar xzf "$work/archive.tgz" -C "$work/out"

# Section 4 runs 4.1 through 4.10, but two of those sections carry two messages each — 4.5
# contrasts a "received" parameter with and without the "[" "]" delimiters, and 4.10 contrasts
# the buggy three-colon IPv6 reference with the correct two-colon one. Twelve files, ten
# sections; a count of ten here would mean the archive lost a contrast pair.
count="$(find "$work/out" -type f | wc -l)"
if [[ "$count" -ne 12 ]]; then
    echo "expected 12 messages in the archive, got $count" >&2
    exit 1
fi

if [[ $check_only -eq 1 ]]; then
    if diff -r --brief "$work/out" "$dest" --exclude=README.md >/dev/null; then
        echo "corpus matches RFC 5118 ($count messages)"
    else
        echo "corpus differs from RFC 5118:" >&2
        diff -r --brief "$work/out" "$dest" --exclude=README.md >&2 || true
        exit 1
    fi
    exit 0
fi

mkdir -p "$dest"
find "$work/out" -type f -exec cp {} "$dest/" \;
echo "wrote $count messages to ${dest#"$repo_root"/}"
