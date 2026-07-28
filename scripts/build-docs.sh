#!/usr/bin/env bash
# Build the documentation site, and check that it does not link into thin air.
#
# One command, because the acceptance for this asks for one: a change to the docs should be
# viewable before it ships, and a build you have to remember four steps for is a build nobody
# runs locally.
#
# The link check is the part that earns its place. The site is built from `docs/`, which also
# holds the internal documentation — stories, specs, designs — that is deliberately *not*
# published. Every cross-reference from a published page into one of those is a 404 that only
# appears once the site is live.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

if ! command -v mdbook >/dev/null; then
    echo "mdbook is not installed: cargo install mdbook --locked" >&2
    exit 1
fi

# The guides include real example files rather than quoting code into prose. Building them is
# what makes "every sample compiles" true rather than aspirational: a sample that has rotted is
# worse than no sample, because it is read as working code.
echo "==> building the samples the guides include"
cargo build --workspace --examples --quiet

echo "==> checking every included sample exists"
python3 - <<'PYEOF'
import pathlib
import re
import sys

root = pathlib.Path(".")
missing = []
for page in sorted((root / "docs").rglob("*.md")):
    for include in re.findall(r"\{\{#include ([^}]+)\}\}", page.read_text()):
        target = (page.parent / include.split(":")[0]).resolve()
        if not target.exists():
            missing.append(f"{page} includes {include}")

if missing:
    print("guides including files that do not exist:", file=sys.stderr)
    for problem in missing:
        print(f"  {problem}", file=sys.stderr)
    sys.exit(1)
print("samples: every include resolves")
PYEOF

echo "==> building"
mdbook build

# The API reference, published into the site rather than beside it. `-D warnings` is the point:
# a missing doc on a public item or an intra-doc link that resolves nowhere fails here rather
# than shipping as a 404 on a page nobody re-reads.
echo "==> building the API reference"
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps --quiet
rm -rf target/book/api
cp -r target/doc target/book/api
# rustdoc writes no index at the root of a multi-crate build, so a link to /api/ would 404.
# Send it to the crate a reader most likely wants first.
cat > target/book/api/index.html <<'HTMLEOF'
<!doctype html>
<meta charset="utf-8">
<title>sipx API reference</title>
<meta http-equiv="refresh" content="0; url=sipx_call/index.html">
<p><a href="sipx_call/index.html">sipx API reference</a></p>
HTMLEOF

# mdBook creates a directory for every subdirectory of the source, whether or not anything in it
# was published. Empty ones are cruft in the deployed artefact.
find target/book -type d -empty -delete

echo "==> checking links"
python3 - <<'PYEOF'
import pathlib
import re
import sys

root = pathlib.Path("target/book")
problems = []

for page in sorted(root.rglob("*.html")):
    html = page.read_text(encoding="utf-8", errors="replace")
    for link in re.findall(r'href="([^"#?]+)', html):
        if link.startswith(("http://", "https://", "mailto:", "//", "data:")):
            continue
        # Absolute paths are resolved against the deployed site root, not the build directory.
        # `site-url` makes them correct in production and unresolvable here.
        if link.startswith("/"):
            continue
        # rustdoc's output is checked by rustdoc, with `-D warnings` on broken intra-doc
        # links. Re-walking tens of thousands of generated pages here would add a minute to
        # every docs build to re-answer a question already answered.
        if "/api/" in str(page) or page.parts[:1] == ("api",):
            continue
        target = (page.parent / link).resolve()
        if not target.exists():
            problems.append(f"{page.relative_to(root)} -> {link}")

if problems:
    print("links that go nowhere on the built site:", file=sys.stderr)
    for problem in sorted(set(problems)):
        print(f"  {problem}", file=sys.stderr)
    print(
        "\nA page under docs/ that links to a story, spec or design is linking to something "
        "the site does not publish. Either publish it in SUMMARY.md or point at GitHub.",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"links: {len(list(root.rglob('*.html')))} pages, every internal link resolves")
PYEOF

echo
echo "built: target/book/index.html"
