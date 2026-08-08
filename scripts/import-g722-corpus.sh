#!/usr/bin/env bash
#
# Import the ITU-T G.722 Appendix II digital test sequences from the ITU archive.
#
# The recommendation distributes its conformance vectors electronically, precisely so
# implementers verify against the official bytes rather than against each other. This recovers
# the 16-bit big-endian binary form and writes the seventeen sequence files to
# crates/sipx-audio/corpus/g722/. The corpus is committed so its provenance is reproducible:
# run this again and the files must be identical.
#
# Usage: scripts/import-g722-corpus.sh [--check]
#          --check   verify the committed corpus matches the archive; do not write
#
# Exit codes: 0 the corpus matches (or was written), 1 it differs from the archive, 64 the
# arguments were not understood, 75 the ITU archive could not be reached — the same contract
# as the RFC corpus imports (`X-58`): 75 disclaims the run rather than reporting on the corpus.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$repo_root/crates/sipx-audio/corpus/g722"
url="https://www.itu.int/wftp3/Public/t/testsignal/SpeAudio/G722/v2012_09/G.722-201209-TestVectors.zip"
subdir="G.722_MB-testvectors/g722-ts-be/bin"
expected_count=17

readonly EX_USAGE=64
readonly EX_UNREACHABLE=75

refuse_argument() {
    echo "unknown argument: $1" >&2
    echo "usage: ${BASH_SOURCE[0]##*/} [--check]" >&2
    exit "$EX_USAGE"
}

# Dispatched on `$#` rather than on `"${1:-}"`, for the reason X-58 recorded on the RFC
# imports: `$1` present-and-empty must not silently select the overwrite path.
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
# One sentence naming the corpus and the host, and EX_TEMPFAIL, so an unreachable archive is a
# disclaimed run rather than a finding about the seventeen committed sequences. A machine with
# no curl or no unzip lands here too, and is reported the same way.
if ! curl -fsSL "$url" -o "$work/vectors.zip"; then
    echo "could not fetch $url — this says nothing about the committed G.722 corpus, only" >&2
    echo "that the ITU archive could not be read to compare against it. Check the network and" >&2
    echo "that curl is installed, then run this again." >&2
    exit "$EX_UNREACHABLE"
fi

if ! unzip -q "$work/vectors.zip" "$subdir/*" -d "$work" 2>/dev/null; then
    echo "could not extract $subdir from the fetched archive; the download may be damaged." >&2
    echo "Nothing here is a finding about the committed corpus." >&2
    exit "$EX_UNREACHABLE"
fi

mkdir -p "$work/out"
# The archive ships a per-directory readme beside the vectors; only the sequence files are the
# corpus.
find "$work/$subdir" -type f \
    \( -name '*.xmt' -o -name '*.cod' -o -name '*.rc0' -o -name '*.rc1' -o -name '*.rc2' -o -name '*.rc3' \) \
    -exec cp {} "$work/out/" \;

count="$(find "$work/out" -type f | wc -l)"
if [[ "$count" -ne "$expected_count" ]]; then
    echo "expected $expected_count sequence files in the archive, got $count" >&2
    exit 1
fi

if [[ $check_only -eq 1 ]]; then
    if diff -r --brief "$work/out" "$dest" --exclude=README.md >/dev/null; then
        echo "corpus matches the ITU G.722 archive ($count sequences)"
    else
        echo "corpus differs from the ITU G.722 archive:" >&2
        diff -r --brief "$work/out" "$dest" --exclude=README.md >&2 || true
        exit 1
    fi
    exit 0
fi

mkdir -p "$dest"
cp "$work/out"/* "$dest/"
echo "wrote $count sequences to ${dest#"$repo_root"/}"
