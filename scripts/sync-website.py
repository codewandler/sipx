#!/usr/bin/env python3
"""Synchronize generated public documentation and reject internal-only content.

The public docs inline compiled examples and facts whose canonical sources live elsewhere in
the repository. Generated regions keep those copies mechanical:

    <!-- BEGIN generated:example crates/sipx-call/examples/place_a_call.rs -->
    ...
    <!-- END generated:example -->

Scalar regions (`workspace-version`, `msrv`, `release-tag`, and `rfc-count`) can sit inline.
`release-heading`, `crate-map`, `compliance`, and `badges` render complete Markdown blocks. The
check also rejects work-item IDs and links into internal story/design records from public content.
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
from urllib.parse import quote


ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "website" / "docs"
README = ROOT / "README.md"
CHANGELOG = ROOT / "CHANGELOG.md"
CARGO = ROOT / "Cargo.toml"
REGISTRY = ROOT / "docs" / "rfc" / "registry.toml"
COMPLIANCE = ROOT / "docs" / "compliance.md"
COMPARISON = ROOT / "docs" / "comparison.md"
COMPARISON_REPORT = ROOT / "scripts" / "comparison-report.py"

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
CARGO_INSTALL_VERSION = re.compile(
    r"\bcargo\s+install\b[^\n]*--version\s+(?P<exact>=)?"
    r"(?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\b",
    re.IGNORECASE,
)
SIPX_DEPENDENCY_VERSION = re.compile(
    r"^\s*sipx-[a-z0-9-]+\s*=.*?[\"'](?P<exact>=)?"
    r"(?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)[\"']",
    re.IGNORECASE,
)
PUBLIC_RELEASE_HEADING = re.compile(
    r"^## (?P<version>\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?) — \d{4}-\d{2}-\d{2}$"
)
DOC_COMMENT = re.compile(r"^\s*(?://!|///)\s?(?P<body>.*)$")
RUST_DOC_LANGUAGES = {"", "rust", "no_run", "ignore", "should_panic"}

# These entry points jointly define whether somebody can adopt the current public beta
# without first reading the repository's internal roadmap. Unlike a copied capability table, the
# guard asks only for the release boundary and stability contract that must be present on each
# public front door. Detailed capability truth remains in the fit and CLI references.
ADOPTION_REQUIREMENTS = {
    "README.md": (
        ("published public-beta status", re.compile(r"current public-beta\s+release", re.I)),
        ("pre-1.0 non-frozen policy", re.compile(r"Public APIs are not frozen", re.I)),
        ("Supported migration policy", re.compile(r"Supported APIs receive migration", re.I)),
        ("Experimental change policy", re.compile(r"Experimental APIs may change", re.I)),
    ),
    "website/docs/intro.md": (
        ("published public-beta status", re.compile(r"current public-beta\s+release", re.I)),
        ("pre-1.0 non-frozen policy", re.compile(r"Public APIs are not frozen", re.I)),
    ),
    "website/docs/whats-new.md": (
        ("published beta heading", re.compile(r"first public beta is published", re.I)),
        ("registry honesty", re.compile(r"published as exact crates\.io packages", re.I)),
        ("Rust-crates-first adoption", re.compile(r"adoption surface leads with the modular Rust crates", re.I)),
        ("intentional omissions", re.compile(r"release intentionally does not provide", re.I)),
        (
            "post-beta endpoint changes",
            re.compile(r"Long-lived endpoints gained explicit operational seams", re.I),
        ),
    ),
    "website/docs/sdk/overview.md": (
        ("implemented webhook binding", re.compile(r"\| Webhook \|.*\| Implemented \|", re.I)),
        ("implemented session binding", re.compile(r"\| Session \|.*\| Implemented \|", re.I)),
        ("implemented realtime binding", re.compile(r"\| Realtime \|.*\| Implemented \|", re.I)),
        (
            "live-endpoint proof boundary",
            re.compile(
                r"credentialed live-endpoint interoperability proof has not yet been recorded",
                re.I,
            ),
        ),
        ("absent embedded binding", re.compile(r"\| Embedded handler \|.*\| Not implemented \|", re.I)),
    ),
}

# These were once truthful alpha statements and remained on current-main pages after the
# corresponding paths shipped. Historical release notes are deliberately excluded.
CURRENT_SURFACE_PAGES = (
    "README.md",
    "website/docs/intro.md",
    "website/docs/getting-started.md",
    "website/docs/guides/as-a-library.md",
    "website/docs/guides/does-this-fit.md",
    "website/docs/guides/integrate-existing-system.md",
    "website/docs/guides/troubleshooting.md",
    # X-74: the page states what sipx can do today relative to seven other stacks, so a capability
    # sentence that goes stale here is wrong twice — about sipx, and about the comparison.
    "website/docs/reference/comparison.md",
    "website/docs/sdk/overview.md",
    "website/docs/sdk/contract.md",
)
STALE_CURRENT_CLAIMS = (
    re.compile(r"\bno ICE\b", re.I),
    re.compile(r"\b(?:never|does not) opens? a microphone", re.I),
    re.compile(r"\b(?:select|use)s? UDP or TCP only\b", re.I),
    re.compile(r"\bbut not TLS\b", re.I),
    re.compile(r"\bcomplete ICE path (?:is|and).*not available\b", re.I),
    re.compile(r"\bcannot yet invoke a webhook\b", re.I),
    re.compile(r"\bcallback bindings? (?:are|is) not implemented\b", re.I),
    re.compile(r"\ball three binding adapters are unavailable\b", re.I),
    re.compile(r"\bDTLS components cannot yet key\b", re.I),
    re.compile(r"\bMESSAGE\b.*\bno user-agent behavior\b", re.I),
)

AUDIO_CLAIMS = ROOT / "scripts" / "check-audio-claims.py"
#: The codec sentence `check-audio-claims.py` prints when it passes. Parsed rather than
#: recomputed: a badge that counted codecs its own way could disagree with the check that fails
#: the build, and a badge disagreeing with the gate is worse than no badge.
CODEC_SUMMARY = re.compile(r"(?P<count>\d+) codecs claimed \((?P<names>[^)]*)\)")


class Facts(NamedTuple):
    workspace_version: str
    msrv: str
    release_tag: str
    release_date: str
    rfc_count: int
    rfc_implemented: int
    license: str
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
    license_name = workspace_package["license"]

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
    rows = registry.get("rfc", [])
    return Facts(
        workspace_version=workspace_version,
        msrv=msrv,
        release_tag=f"v{release_version}",
        release_date=match.group("date"),
        rfc_count=len(rows),
        # Only `implemented` is counted. `partial` is deliberately not folded in as a fraction —
        # `docs/maturity.md` refuses the same arithmetic, because one number would call a fully
        # implemented row and a partial one the same thing.
        rfc_implemented=sum(1 for row in rows if row.get("status") == "implemented"),
        license=license_name,
        packages=workspace_packages(),
    )


@lru_cache(maxsize=1)
def claimed_codecs() -> tuple[str, ...]:
    """The codecs `check-audio-claims.py` holds `sipx-audio` to, read from that check's own output.

    A red check raises rather than rendering: the badge would otherwise publish a codec list that
    nothing is currently asserting, which is precisely the unbacked public claim this whole
    generated-region mechanism exists to make impossible.
    """
    done = subprocess.run(
        [sys.executable, str(AUDIO_CLAIMS), "--check"],
        capture_output=True,
        text=True,
        cwd=ROOT,
        check=False,
    )
    if done.returncode != 0:
        raise ValueError("check-audio-claims is red; refusing to render a codec badge from it")
    found = CODEC_SUMMARY.search(done.stdout)
    if found is None:
        raise ValueError("check-audio-claims printed no codec summary to read")
    names = tuple(
        name.strip()
        for name in found.group("names").split(",")
        if name.strip() and name.strip() != "none"
    )
    if len(names) != int(found.group("count")):
        raise ValueError("check-audio-claims' codec count and codec list disagree")
    return names


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


#: Where an internal evidence path resolves for a public reader. The canonical report links a file
#: relative to `docs/`, which is dead everywhere except a checkout.
BLOB = "https://github.com/codewandler/sipx/blob/main"


def public_comparison() -> str:
    """Render the canonical comparison for the site, with its evidence links made absolute.

    A red checker raises rather than rendering, the same rule `claimed_codecs` follows: the whole
    value of this page is that every claim is currently evidenced and none of it has aged out, and
    publishing it from a red check would assert exactly what the check is refusing to.
    """
    done = subprocess.run(
        [sys.executable, str(COMPARISON_REPORT), "--check"],
        capture_output=True,
        text=True,
        cwd=ROOT,
        check=False,
    )
    if done.returncode != 0:
        raise ValueError(
            "comparison-report is red; refusing to publish a comparison it will not stand behind"
        )

    text = COMPARISON.read_text(encoding="utf-8")
    text = re.sub(r"^# Stack comparison\n\n", "", text, count=1)
    text = re.sub(
        r"^<!-- Generated by scripts/comparison-report\.py.*?-->\n\n",
        "",
        text,
        count=1,
        flags=re.DOTALL,
    )

    # Evidence cites files two ways — `../x` for the repository root and `x` for something under
    # `docs/` — and both are relative to a directory the published page does not sit in. Rewrite
    # the root-relative form first, so the second pattern only ever sees a `docs/` path.
    text = re.sub(r"\]\(\.\./([^)]*)\)", rf"]({BLOB}/\1)", text)
    text = re.sub(r"\]\((?!https?://|#)([^)]*)\)", rf"]({BLOB}/docs/\1)", text)

    text = INTERNAL_MARKDOWN_LINK.sub(r"\1", text)

    def without_tracking_sentences(value: str) -> str:
        sentences = re.split(r"(?<=[.!?]) (?=(?:\*\*|`|[A-Z]))", value)
        return " ".join(sentence for sentence in sentences if not STORY_ID.search(sentence))

    text = "\n".join(without_tracking_sentences(line) for line in text.splitlines())
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
    if kind == "comparison":
        return f"\n{public_comparison()}\n"
    if kind == "badges":
        return render_badges(facts)
    raise ValueError(f"unknown generated region kind {kind!r}")


#: Shields' query form rather than the `label-message-colour` path form, because the path form
#: escapes a hyphen by doubling it — which would render the version as `1.0.0--alpha.1` and put a
#: string in the README that is not the version anyone can install.
BADGE = "https://img.shields.io/static/v1?label={label}&message={message}&color={color}"


def badge(label: str, message: str, color: str, href: str) -> str:
    """One linked badge, as HTML so it can sit inside a centred header block."""
    source = BADGE.format(
        label=quote(label, safe=""), message=quote(message, safe=""), color=color
    )
    return f'<a href="{href}"><img alt="{label}: {message}" src="{source}"></a>'


def render_badges(facts: Facts) -> str:
    """The README's badge row, every number read from the source that already governs it.

    Nothing here is typed by hand. The RFC figures come from the registry the compliance report is
    generated from, the codecs from the check that fails the build when a codec claim is unbacked,
    and the version, MSRV and licence from the workspace manifest. A badge is the most-read and
    least-maintained line in a README, which is exactly the shape of claim `X-47` made generated.
    """
    codecs = claimed_codecs()
    badges = [
        badge("docs", "codewandler.github.io/sipx", "blue", "https://codewandler.github.io/sipx/"),
        badge("release", facts.workspace_version, "blue", "CHANGELOG.md"),
        badge("MSRV", f"rustc {facts.msrv}", "blue", "#try-the-cli"),
        badge(
            "RFCs",
            f"{facts.rfc_implemented} implemented of {facts.rfc_count}",
            "blue",
            "docs/compliance.md",
        ),
        badge("codecs", " · ".join(codecs), "blue", "docs/compliance.md"),
        badge("license", facts.license, "blue", "#license"),
    ]
    return "\n" + "\n".join(badges) + "\n"


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


#: The one public surface that quotes other projects' version numbers, and it quotes a lot of
#: them: every comparison row names the tag its findings were taken at.
COMPARISON_PAGE = "website/docs/reference/comparison.md"


def foreign_stack_row(source: str, line: str) -> bool:
    """A comparison table row whose subject is some stack other than this one.

    The version guards exist to stop a *sipx* version going stale in public prose. On the
    comparison page they would instead fire on every subject's pinned tag, so they are held off
    here — on a table row about another stack, and on nothing else. sipx's own rows stay checked,
    which is what keeps the guard working on the page most likely to carry a version literal.
    """
    if source != COMPARISON_PAGE or not line.startswith("| "):
        return False
    columns = line.split("|")
    return len(columns) > 2 and columns[1].strip() not in ("sipx", "Stack", "Tier", "---")


def public_fact_problems(text: str, source: str) -> list[str]:
    """Reject copied public facts that disagree with their canonical sources."""
    facts = canonical_facts()
    problems = []
    historical_release = False
    for line_number, line in enumerate(text.splitlines(), start=1):
        if foreign_stack_row(source, line):
            continue
        if source == "website/docs/whats-new.md":
            release_heading = PUBLIC_RELEASE_HEADING.fullmatch(line)
            if release_heading is not None:
                historical_release = release_heading.group("version") != facts.workspace_version
            elif line.startswith("## "):
                historical_release = False

        if not historical_release:
            dependency = SIPX_DEPENDENCY_VERSION.search(line)
            for match in TAGGED_VERSION.finditer(line):
                if match.group("version") != facts.workspace_version:
                    problems.append(
                        f"{source}:{line_number}: version {match.group(0)!r} differs from "
                        f"workspace version {facts.workspace_version!r}"
                    )
            if dependency is None:
                for match in CONTEXT_VERSION.finditer(line):
                    if match.group("version") != facts.workspace_version:
                        problems.append(
                            f"{source}:{line_number}: release version "
                            f"{match.group('version')!r} differs from workspace version "
                            f"{facts.workspace_version!r}"
                        )
            for match in CARGO_INSTALL_VERSION.finditer(line):
                if match.group("version") != facts.workspace_version:
                    problems.append(
                        f"{source}:{line_number}: install version {match.group('version')!r} "
                        f"differs from workspace version {facts.workspace_version!r}"
                    )
                elif match.group("exact") is None:
                    problems.append(
                        f"{source}:{line_number}: current release install must use an exact "
                        "--version requirement"
                    )
            if dependency is not None:
                if dependency.group("version") != facts.workspace_version:
                    problems.append(
                        f"{source}:{line_number}: dependency version "
                        f"{dependency.group('version')!r} differs from workspace version "
                        f"{facts.workspace_version!r}"
                    )
                elif dependency.group("exact") is None:
                    problems.append(
                        f"{source}:{line_number}: current sipx dependency must use an exact "
                        "version requirement"
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


def public_adoption_problems(contents: dict[str, str]) -> list[str]:
    """Hold the public beta entry points to their adoption and stability boundary."""
    problems = []
    for source, requirements in ADOPTION_REQUIREMENTS.items():
        text = contents.get(source, "")
        for label, pattern in requirements:
            if not pattern.search(text):
                problems.append(f"{source}: missing {label}")
    for source in CURRENT_SURFACE_PAGES:
        text = contents.get(source, "")
        for line_number, line in enumerate(text.splitlines(), start=1):
            for pattern in STALE_CURRENT_CLAIMS:
                if pattern.search(line):
                    problems.append(
                        f"{source}:{line_number}: stale current-main capability claim"
                    )
    return problems


def rust_doc_example_problems(text: str, source: str) -> list[str]:
    """Reject panic-prone access and detached work in fenced Rust doc examples."""

    problems = []
    in_fence = False
    rust_fence = False
    for line_number, line in enumerate(text.splitlines(), start=1):
        match = DOC_COMMENT.match(line)
        if match is None:
            in_fence = False
            rust_fence = False
            continue
        body = match.group("body")
        if body.startswith("```"):
            if in_fence:
                in_fence = False
                rust_fence = False
            else:
                language = body.removeprefix("```").strip().split(",", 1)[0]
                in_fence = True
                rust_fence = language in RUST_DOC_LANGUAGES
            continue
        if not (in_fence and rust_fence):
            continue
        if re.search(r"\.(?:unwrap|expect)\s*\(|\bpanic!\s*\(", body):
            problems.append(f"{source}:{line_number}: panic-prone Rust doc example")
        if re.search(r"\b[A-Za-z_]\w*(?:\.\w+)*\s*\[[^]]+\]", body):
            problems.append(f"{source}:{line_number}: raw indexing in Rust doc example")
        if "tokio::spawn" in body and re.search(r"(?:let\s+\w+\s*=|=\s*)tokio::spawn", body) is None:
            problems.append(f"{source}:{line_number}: detached task in Rust doc example")
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

    public_contents = {}
    for page in public_files():
        text = page.read_text(encoding="utf-8")
        source = str(page.relative_to(ROOT))
        public_contents[source] = text
        failures.extend(public_content_problems(text, source))
        if page.suffix == ".md":
            failures.extend(public_fact_problems(text, source))
    failures.extend(public_adoption_problems(public_contents))
    for source in sorted((ROOT / "crates").rglob("*.rs")):
        failures.extend(
            rust_doc_example_problems(
                source.read_text(encoding="utf-8"), str(source.relative_to(ROOT))
            )
        )

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
