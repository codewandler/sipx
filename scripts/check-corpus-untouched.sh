#!/usr/bin/env bash
# Fail if the committed fuzz corpus differs from the tree — edited or *added to*.
#
# `git diff --exit-code -- <dir>` is the wrong tool for a corpus: it is blind to untracked files, so
# a seed added by hand passed CI and the campaign ran against a set nothing had reviewed (`X-31`).
# The RFC 4475 equivalent this is shaped after uses `diff -r --brief` for the same reason
# (scripts/import-rfc4475-corpus.sh).
set -euo pipefail
cd "$(dirname "$0")/.."

untouched() {
    local path="$1" name="$2"
    local problems=0
    if ! git diff --exit-code --quiet -- "$path"; then
        echo "$name has uncommitted modifications:" >&2
        git diff --name-only -- "$path" >&2
        problems=1
    fi
    local added
    added=$(git ls-files --others --exclude-standard -- "$path")
    if [[ -n "$added" ]]; then
        echo "$name has uncommitted *additions* a plain git diff cannot see:" >&2
        echo "$added" >&2
        problems=1
    fi
    return $problems
}

failed=0
untouched crates/sipx-testkit/corpus/transaction-sequences "the transaction-sequence corpus" || failed=1
if [[ $failed -ne 0 ]]; then
    echo >&2
    echo "A fuzz corpus is the campaign's input set. It changes deliberately, by a commit that says" >&2
    echo "why — or the campaign starts exploring a program nobody wrote down." >&2
    exit 1
fi
echo "corpus is untouched (modifications and additions both checked)"
