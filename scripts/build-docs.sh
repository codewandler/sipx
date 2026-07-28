#!/usr/bin/env bash
# Build the public documentation site, and check that it does not link into thin air.
#
# One command, because the acceptance for this asks for one: a change to the docs should be
# viewable before it ships, and a build you have to remember four steps for is a build nobody
# runs locally.
#
# The site is the customer-facing `website/` (Docusaurus); the internal contributor material in
# `docs/` is deliberately not published. Three guarantees are enforced here rather than hoped
# for: every code sample on the site is a real example file the workspace compiles; every page
# the site links to exists (the site build refuses broken links); and every relative link
# inside the *internal* docs tree still resolves, so the unpublished half rots no faster than
# the published one.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

if ! command -v node >/dev/null; then
    echo "node is not installed (>= 20 needed): https://nodejs.org" >&2
    exit 1
fi

# The guides inline real example files rather than quoting code into prose. Building them is
# what makes "every sample compiles" true rather than aspirational: a sample that has rotted is
# worse than no sample, because it is read as working code.
echo "==> building the samples the guides inline"
cargo build --workspace --examples --quiet

echo "==> checking the inlined samples match the files"
./scripts/sync-website.py --check

echo "==> checking internal docs links"
python3 - <<'PYEOF'
import pathlib
import re
import sys
import urllib.parse

root = pathlib.Path(".")
problems = []
pages = [root / "README.md", root / "AGENTS.md", *sorted((root / "docs").rglob("*.md"))]
for page in pages:
    text = page.read_text(encoding="utf-8")
    # Markdown links only; code spans and fences excluded by stripping fenced blocks first.
    text = re.sub(r"```.*?```", "", text, flags=re.DOTALL)
    for link in re.findall(r"\]\(([^)\s]+)\)", text):
        if link.startswith(("http://", "https://", "mailto:", "#")):
            continue
        target = urllib.parse.unquote(link.split("#")[0])
        if not target:
            continue
        if not (page.parent / target).exists():
            problems.append(f"{page} -> {link}")

if problems:
    print("internal docs links that go nowhere:", file=sys.stderr)
    for problem in sorted(set(problems)):
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)
print(f"links: {len(pages)} internal pages, every relative link resolves")
PYEOF

# The site build. `onBrokenLinks: 'throw'` in docusaurus.config.js is the link check for the
# published half: a page that links to something the site does not publish fails right here.
echo "==> building the site"
cd website
if [ ! -d node_modules ]; then
    if [ -f package-lock.json ]; then npm ci; else npm install; fi
fi
npm run build
cd "$HERE"

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
