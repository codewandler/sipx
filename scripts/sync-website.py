#!/usr/bin/env python3
"""Synchronize generated public documentation and reject internal-only content.

The public docs inline compiled examples and facts whose canonical sources live elsewhere in
the repository. Generated regions keep those copies mechanical:

    <!-- BEGIN generated:example crates/sipx-call/examples/place_a_call.rs -->
    ...
    <!-- END generated:example -->

Scalar regions (`workspace-version`, `msrv`, `release-tag`, and `rfc-count`) can sit inline.
`release-heading`, `crate-map`, and `compliance` render complete Markdown blocks. The check also
rejects work-item IDs and links into internal story/design records from public content.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from functools import lru_cache
from pathlib import Path
from typing import NamedTuple


ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "website" / "docs"
README = ROOT / "README.md"
CHANGELOG = ROOT / "CHANGELOG.md"
CARGO = ROOT / "Cargo.toml"
REGISTRY = ROOT / "docs" / "rfc" / "registry.toml"
COMPLIANCE = ROOT / "docs" / "compliance.md"

REGION = re.compile(
    r"<!-- BEGIN generated:(?P<kind>[a-z-]+)(?: (?P<arg>\S+))? -->"
    r"(?P<body>.*?)"
    r"<!-- END generated:(?P=kind) -->",
    re.DOTALL,
)
RELEASE = re.compile(r"^## \[(?P<version>[^]]+)\] — (?P<date>\d{4}-\d{2}-\d{2})$", re.MULTILINE)
# Story prefixes are a closed project convention. Keeping the prefix set explicit prevents
# cryptographic names such as SHA-256 and AES-128 from looking like work-item identifiers.
STORY_ID = re.compile(r"(?<![A-Za-z0-9])[STUMCPAX]-\d+\b")
INTERNAL_PUBLIC_LINK = re.compile(
    r"\]\([^)]*(?:(?:^|/)docs/|(?:\.\./)+)(?:stories|designs)/[^)]*\)",
    re.IGNORECASE,
)
INTERNAL_MARKDOWN_LINK = re.compile(
    r"\[([^]]+)\]\([^)]*(?:(?:^|/)docs/|(?:\.\./)+)(?:stories|designs)/[^)]*\)",
    re.IGNORECASE,
)
TAGGED_VERSION = re.compile(r"\bv(?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\b")
CONTEXT_VERSION = re.compile(
    r"\b(?:status|release(?:\s+is)?|sipx)\D{0,12}"
    r"(?<!v)(?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\b",
    re.IGNORECASE,
)
RUST_VERSION = re.compile(r"\bRust\s+(?P<version>\d+\.\d+)\b", re.IGNORECASE)
RFC_COUNT = re.compile(r"\*\*(?P<count>\d+) RFCs tracked\.\*\*")


class Facts(NamedTuple):
    workspace_version: str
    msrv: str
    release_tag: str
    release_date: str
    rfc_count: int
    packages: tuple[tuple[str, str], ...]


def workspace_packages() -> tuple[tuple[str, str], ...]:
    """Return publishable workspace package names/descriptions from Cargo metadata."""
    result = subprocess.run(
        [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--locked",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(result.stdout)
    packages = []
    for package in metadata["packages"]:
        # Cargo serializes `publish = false` as an empty registry allow-list.
        if package.get("publish") == []:
            continue
        name = package["name"]
        description = package.get("description")
        if not description:
            raise ValueError(f"{name}: publishable package has no description")
        packages.append((name, description))
    return tuple(sorted(packages))


@lru_cache(maxsize=1)
def canonical_facts() -> Facts:
    """Read release, toolchain, package, and RFC facts from their canonical sources."""
    manifest = tomllib.loads(CARGO.read_text(encoding="utf-8"))
    workspace_package = manifest["workspace"]["package"]
    workspace_version = workspace_package["version"]
    msrv = workspace_package["rust-version"]

    match = RELEASE.search(CHANGELOG.read_text(encoding="utf-8"))
    if match is None:
        raise ValueError("CHANGELOG.md has no dated release heading")
    release_version = match.group("version")
    if release_version != workspace_version:
        raise ValueError(
            "workspace version and newest changelog release differ: "
            f"{workspace_version!r} != {release_version!r}"
        )

    registry = tomllib.loads(REGISTRY.read_text(encoding="utf-8"))
    return Facts(
        workspace_version=workspace_version,
        msrv=msrv,
        release_tag=f"v{release_version}",
        release_date=match.group("date"),
        rfc_count=len(registry.get("rfc", [])),
        packages=workspace_packages(),
    )


def render_example(source_path: str) -> str:
    source = ROOT / source_path
    if not source.is_file():
        raise FileNotFoundError(source_path)
    code = source.read_text(encoding="utf-8").rstrip("\n")
    return f"\n```rust\n{code}\n```\n"


def public_compliance() -> str:
    """Render the canonical report for the site without internal tracking history."""
    text = COMPLIANCE.read_text(encoding="utf-8")
    text = re.sub(r"^# RFC compliance\n\n", "", text, count=1)
    text = re.sub(
        r"^<!-- Generated by scripts/rfc-report\.py.*?-->\n\n", "", text, count=1, flags=re.DOTALL
    )
    text = text.replace(
        "](rfc-roadmap.md)",
        "](https://github.com/codewandler/sipx/blob/main/docs/rfc-roadmap.md)",
    )
    text = re.sub(
        r"\]\(specs/([^)]*)\)",
        r"](https://github.com/codewandler/sipx/blob/main/docs/specs/\1)",
        text,
    )

    # The canonical engineering report records why claims changed. Public readers need the
    # resulting evidence and gaps, not internal work-item identifiers or design-record links.
    text = INTERNAL_MARKDOWN_LINK.sub(r"\1", text)
    text = re.sub(
        r"`docs/(?:stories|designs)/[^`]+`",
        "the internal engineering record",
        text,
    )
    def without_tracking_sentences(value: str) -> str:
        sentences = re.split(r"(?<=[.!?]) (?=(?:\*\*|`|[A-Z]))", value)
        return " ".join(sentence for sentence in sentences if not STORY_ID.search(sentence))

    cleaned_lines = []
    for line in text.splitlines():
        if line.startswith("| ["):
            # Preserve the RFC identity/status columns even when the first sentence of a note
            # discusses internal history. Markdown tables have six fields plus their edge bars.
            columns = line.split("|", 7)
            if len(columns) == 8:
                columns[6] = without_tracking_sentences(columns[6])
                line = "|".join(columns)
        else:
            line = without_tracking_sentences(line)
        cleaned_lines.append(line)
    text = "\n".join(cleaned_lines)

    # Interoperability evidence belongs in the registry, but public rationale stays grounded in
    # RFCs and our own specs. Match the sentence by its role rather than encoding a product name.
    text = re.sub(r"Verified against [^.]+ module\.\s*", "", text)
    return text.rstrip("\n")


def render_generated(kind: str, arg: str | None) -> str:
    """Render only the body of one generated region."""
    if kind == "example":
        if arg is None:
            raise ValueError("generated:example requires a source path")
        return render_example(arg)
    if arg is not None:
        raise ValueError(f"generated:{kind} does not accept an argument")

    facts = canonical_facts()
    scalar = {
        "workspace-version": facts.workspace_version,
        "msrv": facts.msrv,
        "release-tag": facts.release_tag,
        "rfc-count": str(facts.rfc_count),
    }
    if kind in scalar:
        return scalar[kind]
    if kind == "release-heading":
        return f"\n## {facts.workspace_version} — {facts.release_date}\n"
    if kind == "crate-map":
        rows = ["\n| Crate | What it does |", "|---|---|"]
        rows.extend(f"| `{name}` | {description} |" for name, description in facts.packages)
        return "\n".join(rows) + "\n"
    if kind == "compliance":
        return f"\n{public_compliance()}\n"
    raise ValueError(f"unknown generated region kind {kind!r}")


def render_region(match: re.Match[str]) -> str:
    kind = match.group("kind")
    arg = match.group("arg")
    suffix = f" {arg}" if arg is not None else ""
    return (
        f"<!-- BEGIN generated:{kind}{suffix} -->"
        f"{render_generated(kind, arg)}"
        f"<!-- END generated:{kind} -->"
    )


def public_content_problems(text: str, source: str) -> list[str]:
    """Find internal tracking references that do not belong in public docs."""
    problems = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        if STORY_ID.search(line):
            problems.append(f"{source}:{line_number}: internal work-item ID in public content")
        if INTERNAL_PUBLIC_LINK.search(line):
            problems.append(
                f"{source}:{line_number}: public content links to an internal story/design"
            )
    return problems


def public_fact_problems(text: str, source: str) -> list[str]:
    """Reject copied public facts that disagree with their canonical sources."""
    facts = canonical_facts()
    problems = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        for match in TAGGED_VERSION.finditer(line):
            if match.group("version") != facts.workspace_version:
                problems.append(
                    f"{source}:{line_number}: version {match.group(0)!r} differs from "
                    f"workspace version {facts.workspace_version!r}"
                )
        for match in CONTEXT_VERSION.finditer(line):
            if match.group("version") != facts.workspace_version:
                problems.append(
                    f"{source}:{line_number}: release version {match.group('version')!r} differs "
                    f"from workspace version {facts.workspace_version!r}"
                )
        for match in RUST_VERSION.finditer(line):
            if match.group("version") != facts.msrv:
                problems.append(
                    f"{source}:{line_number}: Rust version {match.group('version')!r} differs "
                    f"from workspace MSRV {facts.msrv!r}"
                )
        for match in RFC_COUNT.finditer(line):
            if int(match.group("count")) != facts.rfc_count:
                problems.append(
                    f"{source}:{line_number}: RFC count {match.group('count')} differs from "
                    f"registry count {facts.rfc_count}"
                )
    return problems


def public_files() -> list[Path]:
    files = [README]
    files.extend(sorted(DOCS.rglob("*.md")))
    pages = ROOT / "website" / "src" / "pages"
    files.extend(sorted(pages.rglob("*.js")))
    files.extend(sorted(pages.rglob("*.jsx")))
    return files


def process(update: bool) -> int:
    failures = []
    regions = 0
    pages = [README, *sorted(DOCS.rglob("*.md"))]
    for page in pages:
        text = page.read_text(encoding="utf-8")

        def replace(match: re.Match[str]) -> str:
            nonlocal regions
            regions += 1
            try:
                return render_region(match)
            except (FileNotFoundError, ValueError, subprocess.CalledProcessError) as error:
                failures.append(f"{page.relative_to(ROOT)}: {error}")
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
        failures.append("no generated regions found in public docs — the guard is not guarding")

    for page in public_files():
        text = page.read_text(encoding="utf-8")
        source = str(page.relative_to(ROOT))
        failures.extend(public_content_problems(text, source))
        if page.suffix == ".md":
            failures.extend(public_fact_problems(text, source))

    for failure in failures:
        print(f"sync-website: {failure}", file=sys.stderr)
    if not failures and not update:
        print(f"sync-website: {regions} generated regions in sync; public content clean")
    return 1 if failures else 0


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in ("--check", "--update"):
        print("usage: sync-website.py --check | --update", file=sys.stderr)
        return 2
    return process(update=sys.argv[1] == "--update")


if __name__ == "__main__":
    raise SystemExit(main())
