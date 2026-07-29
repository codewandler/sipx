#!/usr/bin/env python3
"""Hold the specs' account of the connection pool key against the type that defines it.

`docs/specs/sip-transport.md` said the pool key was `(transport, remote address)` for long
enough to be wrong twice: once when the verified identity joined `ConnectionKey`, and again when
the WebSocket resource did in `T-23`. Both times the other specs describing the key were
corrected and this one was not, because nothing connected the sentence to the type. A third
hand-written correction would have left the arrangement that produced the first two.

So the enumeration is no longer written by hand. `docs/specs/sip-transport.md` §8 carries a
generated region — the arrangement `sync-website.py` uses for inlined samples and
`rfc-report.py` for the compliance table — rendered from `ConnectionKey`'s fields and their doc
comments:

    <!-- BEGIN generated:pool-key crates/sipx-transport/src/target.rs -->
    | Field | What it is |
    …
    <!-- END generated:pool-key -->

`--check` is the gate step and fails when the region has drifted from the type; `--update`
rewrites it. Adding a field to `ConnectionKey` now fails the gate until the spec is regenerated,
which is the property the sentence never had.

Three things this deliberately does not do.

It does not check *why* a field is in the key. That argument is prose, it lives in `sip-tls.md`
§5, and no script can verify it — but a field appearing in the generated table with no paragraph
behind it is visible to a reviewer in a way a missing field never was.

It does not check `sip-quic.md`, which specifies a transport sipx has not implemented: there is
no `ConnectionKey` for it to disagree with yet, and a check that demanded one would make "spec
before code" impossible.

It does not try to tell prose that re-enumerates the key from prose that explains it. Instead it
requires that every spec speaking of the pool key names where the key is defined, because a
description with no link to its source is exactly the one that goes quietly stale — which is the
literal history of the sentence this script replaces.
"""

import re
import sys
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parent.parent
SPECS = ROOT / "docs" / "specs"

#: The type that defines the key. Everything else here is derived from it.
TYPE = "ConnectionKey"
SOURCE = Path("crates") / "sipx-transport" / "src" / "target.rs"

#: The one spec that enumerates the key. Every other spec cites it.
DEFINITION = SPECS / "sip-transport.md"
DEFINITION_SECTION = "§8"

REGION = re.compile(
    r"<!-- BEGIN generated:pool-key (?P<source>\S+) -->\n(?P<body>.*?)<!-- END generated:pool-key -->",
    re.DOTALL,
)

#: What counts as speaking of the connection pool key, kept narrow on purpose: `docs/specs`
#: also pools isolates and rejects shared pools of applications, and neither is this key.
SPEAKS_OF_THE_KEY = re.compile(r"pool keys?|pooled by|connection pool", re.I)

#: Below this the struct reader has stopped understanding `target.rs` rather than found a small
#: type. A reader that silently finds one field would render a table nobody would notice was
#: wrong, which is the failure mode this whole script exists to remove.
_PLAUSIBLE_FIELDS = 2


class Field(NamedTuple):
    """One field of the key, as the spec table renders it."""

    name: str
    #: Its doc comment, which is the spec's description of it. The two are one sentence, and
    #: this is the mechanism that keeps them one sentence.
    doc: str


def fields(text: str) -> list[Field]:
    """`ConnectionKey`'s public fields, in declaration order, with their doc comments."""
    struct = re.search(
        rf"^pub struct {TYPE} \{{\n(?P<body>.*?)^\}}", text, re.DOTALL | re.MULTILINE
    )
    if struct is None:
        raise ValueError(f"{SOURCE} declares no `pub struct {TYPE}`")

    found: list[Field] = []
    doc: list[str] = []
    for line in struct.group("body").splitlines():
        stripped = line.strip()
        if stripped.startswith("///"):
            doc.append(stripped[3:].strip())
            continue
        declaration = re.match(r"pub (?P<name>\w+):", stripped)
        if declaration:
            found.append(Field(declaration.group("name"), " ".join(part for part in doc if part)))
        doc = []

    if len(found) < _PLAUSIBLE_FIELDS:
        raise ValueError(
            f"read only {len(found)} fields from {TYPE}; the reader has drifted from the shape of "
            f"{SOURCE} and would report a key that agrees with nothing"
        )
    return found


def render(found: list[Field]) -> str:
    """The generated region, markers included, exactly as it appears in the spec."""
    rows = "\n".join(f"| `{f.name}` | {f.doc.replace('|', chr(92) + '|')} |" for f in found)
    return (
        f"<!-- BEGIN generated:pool-key {SOURCE.as_posix()} -->\n"
        f"| Field | What it is |\n"
        f"|---|---|\n"
        f"{rows}\n"
        f"<!-- END generated:pool-key -->"
    )


def sections(text: str) -> list[tuple[str, str]]:
    """A page split at its `##` headings, each with the heading it falls under.

    The section rather than the file, because a link in an unrelated section is not a citation a
    reader following the pool key would ever see. The section rather than the paragraph, because
    a spec that argues about the key over four paragraphs would then have to link four times,
    and a rule that makes correct prose ugly gets deleted.
    """
    found: list[tuple[str, str]] = []
    heading = "(preamble)"
    body: list[str] = []
    for line in text.splitlines():
        if line.startswith("## "):
            found.append((heading, "\n".join(body)))
            heading, body = line[3:].strip(), []
            continue
        body.append(line)
    found.append((heading, "\n".join(body)))
    return found


def citation_problems() -> list[str]:
    """Specs that describe the pool key without saying where it is defined."""
    problems = []
    for page in sorted(SPECS.glob("*.md")):
        if page == DEFINITION:
            continue
        for heading, body in sections(page.read_text(encoding="utf-8")):
            if not SPEAKS_OF_THE_KEY.search(body):
                continue
            if DEFINITION.name not in body:
                problems.append(
                    f"{page.relative_to(ROOT)} §'{heading}' describes the connection pool key and "
                    f"does not link to {DEFINITION.relative_to(ROOT)} {DEFINITION_SECTION}, where "
                    f"it is defined; a description with no link to its source is the one that "
                    f"goes stale"
                )
    return problems


def region_problems(update: bool) -> list[str]:
    """Whether the spec's table still is what the type says, and rewrite it if asked."""
    expected = render(fields((ROOT / SOURCE).read_text(encoding="utf-8")))
    text = DEFINITION.read_text(encoding="utf-8")
    regions = REGION.findall(text)
    if len(regions) != 1:
        return [
            f"{DEFINITION.relative_to(ROOT)} has {len(regions)} generated:pool-key regions and "
            f"needs exactly one; without it nothing holds the key against {TYPE}"
        ]

    rewritten = REGION.sub(lambda _: expected, text)
    if rewritten == text:
        return []
    if update:
        DEFINITION.write_text(rewritten, encoding="utf-8")
        print(f"updated {DEFINITION.relative_to(ROOT)}")
        return []
    return [
        f"{DEFINITION.relative_to(ROOT)} {DEFINITION_SECTION} no longer matches {TYPE} in "
        f"{SOURCE}; run scripts/check-pool-key.py --update",
        f"  the spec says:\n"
        + "\n".join(f"    {line}" for line in REGION.search(text).group(0).splitlines()),
        f"  the type says:\n" + "\n".join(f"    {line}" for line in expected.splitlines()),
    ]


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ("--check", "--update"):
        print("usage: check-pool-key.py --check | --update", file=sys.stderr)
        return 2
    update = sys.argv[1] == "--update"

    problems = region_problems(update) + citation_problems()
    if problems:
        print("The specs and the connection pool key have drifted apart:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    if not update:
        count = len(fields((ROOT / SOURCE).read_text(encoding="utf-8")))
        print(f"pool key: {count} fields, one spec defines them, every other one cites it")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
