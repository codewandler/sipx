#!/usr/bin/env python3
"""Keep the website's inlined code samples identical to the compiled example files.

The public site inlines example programs so a reader sees real code — and those files are
built by `cargo build --workspace --examples`, so a sample that stops compiling fails CI.
The site cannot include files at build time the way the old book did, so the inclusion is a
generated region:

    <!-- BEGIN generated:example crates/sipx-call/examples/place_a_call.rs -->
    ```rust
    …the file, verbatim…
    ```
    <!-- END generated:example -->

`sync-website.py --check` (the gate) fails when any region differs from the file it names or
names a file that does not exist. `sync-website.py --update` rewrites the regions in place.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "website" / "docs"

REGION = re.compile(
    r"<!-- BEGIN generated:example (?P<path>\S+) -->\n(?P<body>.*?)<!-- END generated:example -->",
    re.DOTALL,
)


def render(source_path: str) -> str:
    source = ROOT / source_path
    if not source.is_file():
        raise FileNotFoundError(source_path)
    code = source.read_text(encoding="utf-8").rstrip("\n")
    return (
        f"<!-- BEGIN generated:example {source_path} -->\n"
        f"```rust\n{code}\n```\n"
        f"<!-- END generated:example -->"
    )


def process(update: bool) -> int:
    failures = []
    regions = 0
    for page in sorted(DOCS.rglob("*.md")):
        text = page.read_text(encoding="utf-8")

        def replace(match: re.Match) -> str:
            nonlocal regions
            regions += 1
            try:
                return render(match.group("path"))
            except FileNotFoundError:
                failures.append(f"{page.relative_to(ROOT)}: no such file {match.group('path')}")
                return match.group(0)

        rewritten = REGION.sub(replace, text)
        if rewritten != text:
            if update:
                page.write_text(rewritten, encoding="utf-8")
                print(f"updated {page.relative_to(ROOT)}")
            else:
                failures.append(
                    f"{page.relative_to(ROOT)}: generated region is stale "
                    f"(run scripts/sync-website.py --update)"
                )

    if regions == 0:
        failures.append("no generated regions found under website/docs — the guard is not guarding")
    for failure in failures:
        print(f"sync-website: {failure}", file=sys.stderr)
    if not failures and not update:
        print(f"sync-website: {regions} regions in sync")
    return 1 if failures else 0


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ("--check", "--update"):
        print("usage: sync-website.py --check | --update", file=sys.stderr)
        return 2
    return process(update=sys.argv[1] == "--update")


if __name__ == "__main__":
    raise SystemExit(main())
