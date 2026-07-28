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

echo "==> building"
mdbook build

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
