#!/usr/bin/env python3
"""Check that every relative link in the *internal* docs tree resolves — file and anchor.

`website/` is the published half, and Docusaurus checks its own link graph: `onBrokenLinks`,
`onBrokenAnchors`, `onDuplicateRoutes` and `markdown.hooks.onBrokenMarkdownLinks` are all
`throw` in `website/docusaurus.config.js`, so a dead link there fails the site build.

Nothing checked the unpublished half. `docs/` is read on GitHub, and until `X-41` this script's
predecessor — a heredoc inside `build-docs.sh` — split every link on `#` and threw the fragment
away. So a link to a *file* that does not exist failed the build, and a link to a *heading* that
does not exist was invisible: the same defect one directory over, and the reason both halves are
now checked to the same depth.

Anchors are resolved the way GitHub renders them, because GitHub is where these pages are read:
a heading becomes its text lowercased with punctuation dropped and spaces hyphenated, a repeated
heading gets `-1`, `-2`, … and an explicit `{#id}` or an `id="…"` on inline HTML wins outright.
`slug` documents the three places that rule is subtler than it sounds, each of which cost a live
link before it was found.

Two known limits, both deliberate, because each can only produce a false *negative* — a dead
anchor this accepts — and never a red gate for a link that works. No heading in the tree has an
emoji, and GitHub's own rewriting of an author-supplied `id=` is unquantified without a live
render. If either ever matters, it will matter as an anchor nobody noticed was dead, which is what
the site's own `onBrokenAnchors` catches for the published half.

Usage:
    ./scripts/check-docs-links.py            # the repository this script lives in
    ./scripts/check-docs-links.py --root DIR # a tree assembled by a test
"""

import argparse
import pathlib
import re
import sys
import urllib.parse

FENCE = re.compile(r"^\s*(```+|~~~+)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*$")
EXPLICIT_ID = re.compile(r"\{#([\w:.-]+)\}\s*$")
HTML_ID = re.compile(r"""<[^>]*\sid=["']([^"']+)["']""")
HTML_NAME = re.compile(r"""<a[^>]*\sname=["']([^"']+)["']""")
LINK = re.compile(r"\]\(([^)\s]+)\)")
CODE_SPAN = re.compile(r"`([^`]*)`")
# Underscore emphasis, only where a renderer would read it: `_` cannot open or close emphasis when
# it is flanked by word characters, which is what keeps `on_failure` and `snake_case` intact.
STRONG_UNDERSCORE = re.compile(r"(?<!\w)__([^_]+)__(?!\w)")
EM_UNDERSCORE = re.compile(r"(?<!\w)_([^_]+)_(?!\w)")

# A fragment on a link to one of these is a line range or a source-file anchor, not a heading we
# can resolve from markdown. The file's existence is still checked; the fragment is not.
ANCHORABLE_SUFFIXES = {".md", ".markdown"}


def strip_code(text: str) -> str:
    """Blank out fenced blocks, keeping line numbering intact.

    A fenced block is illustration, not navigation: `docs/` is full of sample markdown and shell
    snippets whose `](…)` would otherwise be read as a link this repository has to honour.
    """
    out, fence = [], None
    for line in text.splitlines():
        match = FENCE.match(line)
        if fence is None:
            if match:
                fence = match.group(1)[0] * 3
                out.append("")
                continue
            out.append(line)
            continue
        out.append("")
        if line.strip().startswith(fence):
            fence = None
    return "\n".join(out)


def slug(heading: str) -> str:
    """The id a renderer emits for a heading's text.

    Every divergence from the renderer costs the same thing in the same direction: a live link a
    human can follow, reported dead, reddening the gate with no defect in the tree. So the three
    that have been found are written down here rather than left to be rediscovered.

    1. **Spaces map to hyphens one for one, not run for one.** Dropping a character between two
       spaces leaves both spaces behind, so the `Application SDK` heading in `docs/roadmap.md`
       anchors as `application-sdk--app-sdk`, with two hyphens where its em dash was. Collapsing
       runs reports its two live links as dead.
    2. **Emphasis is markup, and `_underscores_` are emphasis too.** `_(six stories, M10)_` in that
       same file contributes `six-stories-m10`, not `_six-stories-m10_` — the underscores are
       consumed, and `_` survives the punctuation strip so leaving them in changes the id. Only
       where a renderer would read emphasis: `on_failure` inside a word is not, which is why the
       rule is flanked rather than greedy.
    3. **A code span's content is literal, so markup inside it is not markup.** `<name>` inside
       ``[listener.<name>]`` is four characters, not an HTML tag — and this used to delete it,
       because backticks were stripped before tags were, leaving `42-listener` where the renderer
       gives `42-listenername`. Four headings in `docs/specs/host-config.md` have that shape. The
       fix is not an ordering tweak: code spans are held out of every markup rule and put back
       just before the punctuation strip, which is the only way "literal" stays true whatever
       rules are added above.
    """
    held: list[str] = []

    def hold(match: re.Match) -> str:
        held.append(match.group(1))
        return f"\x00{len(held) - 1}\x00"

    # Rule 3. `\x00` cannot appear in a heading and matches none of the patterns below, so a code
    # span is inert for the rest of this function.
    text = CODE_SPAN.sub(hold, heading)

    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)  # links and images, label kept
    text = re.sub(r"\*\*([^*]*)\*\*", r"\1", text)
    text = STRONG_UNDERSCORE.sub(r"\1", text)
    text = re.sub(r"\*([^*]*)\*", r"\1", text)
    text = EM_UNDERSCORE.sub(r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)

    text = re.sub(r"\x00(\d+)\x00", lambda m: held[int(m.group(1))], text)

    text = re.sub(r"\s", " ", text.lower().strip())
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


def anchors(text: str) -> set[str]:
    """Every id a page emits: headings, explicit `{#id}`, and ids on inline HTML."""
    text = strip_code(text)
    found: set[str] = set()
    seen: dict[str, int] = {}
    for line in text.splitlines():
        match = HEADING.match(line)
        if not match:
            continue
        title = match.group(2)
        explicit = EXPLICIT_ID.search(title)
        if explicit:
            found.add(explicit.group(1))
            title = title[: explicit.start()]
        name = slug(title)
        if not name:
            continue
        count = seen.get(name, 0)
        seen[name] = count + 1
        found.add(name if count == 0 else f"{name}-{count}")
    found.update(HTML_ID.findall(text))
    found.update(HTML_NAME.findall(text))
    return found


def pages_of(root: pathlib.Path) -> list[pathlib.Path]:
    """The internal tree: the two top-level pages a contributor starts from, plus `docs/`."""
    top = [root / "README.md", root / "AGENTS.md"]
    return [*(p for p in top if p.is_file()), *sorted((root / "docs").rglob("*.md"))]


class Report:
    """What the check found, so the caller prints one summary and the tests read the parts."""

    def __init__(self) -> None:
        self.pages = 0
        self.links = 0
        self.anchor_links = 0
        self.problems: list[str] = []


def check(root: pathlib.Path) -> Report:
    report = Report()
    cache: dict[pathlib.Path, set[str]] = {}
    for page in pages_of(root):
        report.pages += 1
        text = strip_code(page.read_text(encoding="utf-8"))
        for link in LINK.findall(text):
            if link.startswith(("http://", "https://", "mailto:", "tel:")):
                continue
            target_part, _, fragment = link.partition("#")
            target_part = urllib.parse.unquote(target_part)
            fragment = urllib.parse.unquote(fragment)
            where = page.relative_to(root)

            if target_part:
                report.links += 1
                target = page.parent / target_part
                if not target.exists():
                    report.problems.append(f"{where} -> {link}  (no such file)")
                    continue
            else:
                if not fragment:
                    continue
                target = page

            if not fragment:
                continue
            report.anchor_links += 1
            if target.is_dir() or target.suffix.lower() not in ANCHORABLE_SUFFIXES:
                # A `#L42` on a source file, or a fragment on a directory listing. The file is
                # there; the fragment is not ours to resolve.
                continue
            resolved = target.resolve()
            if resolved not in cache:
                cache[resolved] = anchors(resolved.read_text(encoding="utf-8"))
            if fragment not in cache[resolved]:
                report.problems.append(f"{where} -> {link}  (no such anchor in {target_part or where})")
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", default=None, help="tree to check (default: this repository)")
    args = parser.parse_args()
    root = pathlib.Path(args.root) if args.root else pathlib.Path(__file__).resolve().parent.parent

    report = check(root)
    if report.problems:
        print("internal docs links that go nowhere:", file=sys.stderr)
        for problem in sorted(set(report.problems)):
            print(f"  {problem}", file=sys.stderr)
        return 1
    print(
        f"links: {report.pages} internal pages, {report.links} relative links and "
        f"{report.anchor_links} anchors all resolve"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
