#!/usr/bin/env bash
# Build the public documentation site, and check that it does not link into thin air.
#
# One command, because the acceptance for this asks for one: a change to the docs should be
# viewable before it ships, and a build you have to remember four steps for is a build nobody
# runs locally.
#
# The site is the customer-facing `website/` (Docusaurus); the internal contributor material in
# `docs/` is deliberately not published.
#
# -- the contract of the `docs site` gate step (X-41) -----------------------------------------
#
# This is one step in `./scripts/gate.py`, and a gate step is read by its exit code. So the rule
# here is narrower than "run the build": **no check in this file may print a defect and exit 0.**
#
# It was broken. `onBrokenAnchors` was unset in `website/docusaurus.config.js`, Docusaurus
# defaults it to `warn`, and the step printed
#
#     [WARNING] Docusaurus found broken anchors! … -> linking to /sipx/docs#sipx
#     [SUCCESS] Generated static files in "build".
#
# and exited 0. The gate reported every step green with a dead link in the published site, and
# the only reason anyone found out is that `S-30` read the step's output instead of trusting its
# result. That is the `X-36` failure: it looks like coverage and is not.
#
# What is checked, and how each one fails:
#
#   1. the samples the guides inline compile        `cargo build --examples` exits non-zero
#   2. the inlined text matches those files         `sync-website.py --check` exits non-zero
#   3. every link in the internal `docs/` tree      `check-docs-links.py` exits non-zero —
#      resolves, file *and* anchor                  including anchors, which its predecessor
#                                                   split off and threw away
#   4. the site has no dead link, dead anchor,      all four reporting handlers in
#      dead relative markdown link or duplicate     `docusaurus.config.js` are `throw`, each
#      route                                        stated there with its reason
#   5. the site build printed no warning at all     WARNING_EXCEPTIONS below, empty on purpose:
#                                                   a handler we never audited cannot ride green
#   6. check 4 is actually armed                    the guard self-check calls the installed link
#                                                   checker with a link to an id no page emits
#   7. every public item is documented and every    `RUSTDOCFLAGS=-D warnings cargo doc`
#      intra-doc link resolves
#
# `set -euo pipefail` below is load-bearing for all of it: without `pipefail`, piping the site
# build into a log to inspect its output would discard the build's own exit code, which is how a
# step grows this defect back by accident.
#
# Check 6 used to launch a second full site build around a temporary Markdown page. That duplicated
# compilation and worker startup just to ask the already-installed link checker one question. Under
# full-workspace load the second Node process sometimes left its top-level await unsettled, an
# infrastructure exit unrelated to the anchor policy. The probe now invokes the same checker with
# the real config and a synthetic collected-link graph. The real site is still built once above.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

# Docusaurus warnings that are deliberately not failures. Empty, and it should stay that way:
# anything added here needs a reason on the line and a story against it, because every entry is
# a defect this step is choosing to print and exit 0 about. Matched as fixed substrings.
WARNING_EXCEPTIONS=()

if ! command -v node >/dev/null; then
    echo "node is not installed (>= 20 needed): https://nodejs.org" >&2
    exit 1
fi

# The guides inline real example files rather than quoting code into prose. Building them is
# what makes "every sample compiles" true rather than aspirational: a sample that has rotted is
# worse than no sample, because it is read as working code.
echo "==> building the samples the guides inline"
cargo build --workspace --examples --quiet

echo "==> testing the public-doc generators and guards"
python3 scripts/test-sync-website.py

echo "==> checking the inlined samples match the files"
./scripts/sync-website.py --check

# The unpublished half. Docusaurus never sees `docs/`, so its link graph is checked here or
# nowhere — and anchors are part of the link, not decoration on it.
echo "==> checking internal docs links and anchors"
./scripts/check-docs-links.py

# The site build. The four reporting handlers in docusaurus.config.js are the link check for the
# published half; a page that links to something the site does not publish, or to an anchor no
# page emits, fails right here.
echo "==> building the site"
site_log="$(mktemp)"
trap 'rm -f "$site_log"' EXIT
cd website
if [ ! -d node_modules ]; then
    if [ -f package-lock.json ]; then npm ci; else npm install; fi
fi
# The exit code is the build's, not tee's — see `pipefail` above.
npm run build 2>&1 | tee "$site_log"
cd "$HERE"

# Check 5. A handler nobody audited, or one Docusaurus adds in a later version, would otherwise
# print here and exit 0 all over again — this time under a heading we have not read yet.
warnings="$(grep -F '[WARNING]' "$site_log" || true)"
for allowed in ${WARNING_EXCEPTIONS[@]+"${WARNING_EXCEPTIONS[@]}"}; do
    warnings="$(printf '%s\n' "$warnings" | grep -Fv -- "$allowed" || true)"
done
if [ -n "$(printf '%s' "$warnings" | tr -d '[:space:]')" ]; then
    echo >&2
    echo "the site build printed a warning, which this step treats as a defect:" >&2
    printf '%s\n' "$warnings" >&2
    echo >&2
    echo "either fix it, or add it to WARNING_EXCEPTIONS in this script with a reason and a" >&2
    echo "story — a warning that exits 0 is the defect X-41 was filed for." >&2
    exit 1
fi

# Check 6. "The config says throw" and "the installed checker rejects the link" are distinct
# claims. Exercise the handler a site build calls with the real config and a synthetic page linking
# to an id the target page does not emit. This is deterministic and does not start a second compiler
# and worker lifecycle merely to reach the checker again (`X-60`).
echo "==> checking a dead anchor still fails the build"
node scripts/check-docs-anchor-guard.mjs

# The API reference, published into the site rather than beside it. `-D warnings` is the point:
# a missing doc on a public item or an intra-doc link that resolves nowhere fails here rather
# than shipping as a 404 on a page nobody re-reads.
echo "==> building the API reference"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --quiet
rm -rf website/build/api
cp -r target/doc website/build/api
# rustdoc writes no index at the root of a multi-crate build, so a link to /api/ would 404.
# Send it to the crate a reader most likely wants first.
cat > website/build/api/index.html <<'HTMLEOF'
<!doctype html>
<meta charset="utf-8">
<title>sipx API reference</title>
<meta http-equiv="refresh" content="0; url=sipx_call/index.html">
<p><a href="sipx_call/index.html">sipx API reference</a></p>
HTMLEOF

echo
echo "built: website/build/index.html"
