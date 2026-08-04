#!/usr/bin/env bash
#
# Provenance gate.
#
# This repository must be publishable with no reference to any third-party prior-art
# project that informed its design. Nothing is copied from any such project, and nothing
# committed here may name one — not in code, comments, specs, docs, story files, fixture
# names, commit messages or package metadata.
#
# One exception, added by X-71 and scoped by COMPARISON_SCOPE below: the comparison
# registry and the documents generated from it may name a comparison subject, because a
# comparison whose subjects are anonymous cannot cite evidence that anyone can check.
# Commit messages are NOT covered by that exception.
#
# The denylist itself is deliberately NOT stored in this repository, so that the terms we
# refuse to mention are not mentioned by the check that refuses them. It is supplied by:
#
#   SIPX_DENYLIST       — newline- or comma-separated terms, or
#   SIPX_DENYLIST_FILE  — path to a file with one term per line (# comments allowed)
#
# and falls back to ~/notes/sipx-research/denylist.txt for local runs.
#
# Usage:  scripts/check-provenance.sh [--history]
#         --history   also scan the full commit log (slower; run in CI and before release)
#
# Exit codes: 0 clean · 1 hit found · 2 misconfigured (no denylist available in CI)

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

scan_history=0
[[ "${1:-}" == "--history" ]] && scan_history=1

# ---------------------------------------------------------------- load the denylist ----
terms=()

# `read` drops a trailing line that has no newline, so every loop below uses the
# `|| [[ -n "$line" ]]` form. Losing the last term would make this gate pass silently.
add_terms() {
    local line term
    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%%#*}"
        while IFS= read -r term || [[ -n "$term" ]]; do
            term="${term#"${term%%[![:space:]]*}"}"
            term="${term%"${term##*[![:space:]]}"}"
            [[ -n "$term" ]] && terms+=("$term")
        done < <(printf '%s\n' "$line" | tr ',' '\n')
    done
}

if [[ -n "${SIPX_DENYLIST:-}" ]]; then
    add_terms <<<"$SIPX_DENYLIST"
fi

denylist_file="${SIPX_DENYLIST_FILE:-$HOME/notes/sipx-research/denylist.txt}"
if [[ ${#terms[@]} -eq 0 && -r "$denylist_file" ]]; then
    add_terms <"$denylist_file"
fi

if [[ ${#terms[@]} -eq 0 ]]; then
    if [[ -n "${CI:-}" ]]; then
        echo "provenance: FAIL — no denylist available." >&2
        echo "  Set the SIPX_DENYLIST secret in the CI environment. An unconfigured gate" >&2
        echo "  that passes is worse than no gate at all." >&2
        exit 2
    fi
    echo "provenance: skipped — no denylist configured locally."
    echo "  Set SIPX_DENYLIST, or create $denylist_file, to enable this check."
    exit 0
fi

# ------------------------------------------------------- the comparison exception ----
# X-71: a comparison subject may be named in the comparison registry and in the documents
# generated from it. Nowhere else, and never as design rationale — `docs/vision.md`
# principle 5 is unchanged, so a spec or a design doc citing another implementation is
# still a failure here.
#
# This array IS the boundary. Widening it is an edit to this file with a diff somebody
# reviews, which is the point: the previous rule was a sentence, and a sentence can be
# reinterpreted by whoever is in a hurry.
#
# The history scan below is deliberately NOT subject to it. A commit message naming a
# subject still fails, so `--history` stays clean and the one failure mode with no cheap
# remedy — "history must be rewritten before this repository is published" — stays
# unreachable. Name subjects in the data; never in the message that lands it.
COMPARISON_SCOPE=(
    ':!docs/comparison'
    ':!docs/comparison.md'
    ':!website/docs/reference/comparison.md'
)

# ------------------------------------------------------------------------- scanning ----
# Build one alternation pattern so each corpus is scanned in a single pass.
pattern="$(printf '%s\n' "${terms[@]}" | paste -sd '|' -)"

status=0

# Tracked files only: untracked scratch is ignored by design, and .gitignore already
# excludes the local research directories.
if git rev-parse --git-dir >/dev/null 2>&1 && [[ -n "$(git ls-files)" ]]; then
    files_hit="$(git grep -I -n -i -E "$pattern" -- . ':!scripts/check-provenance.sh' \
        "${COMPARISON_SCOPE[@]}" || true)"
    if [[ -n "$files_hit" ]]; then
        echo "provenance: FAIL — prior-art reference in tracked files:" >&2
        printf '%s\n' "$files_hit" >&2
        status=1
    fi
else
    # No git: a coarser approximation of COMPARISON_SCOPE, since plain grep excludes by
    # basename rather than by path. It over-excludes a directory called `comparison`
    # anywhere in the tree. Acceptable only because this branch runs outside a checkout;
    # every gate and CI run takes the git path above.
    files_hit="$(grep -rInE "$pattern" . \
        --exclude-dir=.git --exclude-dir=target --exclude-dir=notes \
        --exclude-dir=comparison --exclude='comparison.md' \
        --exclude='check-provenance.sh' || true)"
    if [[ -n "$files_hit" ]]; then
        echo "provenance: FAIL — prior-art reference found:" >&2
        printf '%s\n' "$files_hit" >&2
        status=1
    fi
fi

if [[ $scan_history -eq 1 ]] && git rev-parse HEAD >/dev/null 2>&1; then
    log_hit="$(git log --format='%H%n%an%n%ae%n%s%n%b' | grep -inE "$pattern" || true)"
    if [[ -n "$log_hit" ]]; then
        echo "provenance: FAIL — prior-art reference in commit history:" >&2
        printf '%s\n' "$log_hit" >&2
        echo "  History must be rewritten before this repository is published." >&2
        status=1
    fi
fi

if [[ $status -eq 0 ]]; then
    echo "provenance: clean (${#terms[@]} terms checked$([[ $scan_history -eq 1 ]] && echo ', including history'))"
fi

exit $status
