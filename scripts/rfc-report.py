#!/usr/bin/env python3
"""Render docs/compliance.md from docs/rfc/registry.toml, and check that it is not lying.

A compliance table nobody can verify is marketing. This does two things a hand-written table
cannot: it regenerates the document so it cannot drift from its source, and with `--check` it
holds the source against the code — every header and method an entry names must actually be
known to the parser, and every file it cites must exist.

What it deliberately does *not* claim to verify is behaviour. No script can read
`crates/sipx-sip/src/transaction/client.rs` and decide whether Timer A is right; the tests do
that. What it can do is stop an entry claiming syntax support for a header the code has never
heard of, which is the failure mode a table like this actually has.

The second failure mode, and the one that recurred: a capability implemented and tested inside
one crate that nothing above it can select, reported as shipped because every check above passes
for it. `unreachable_role_claims` is the check for that, scoped to the media layer for reasons
measured in docs/designs/rfc-registry-grain.md.
"""

import argparse
import os.path
import pathlib
import re
import sys
import tomllib
from collections import Counter, defaultdict

ROOT = pathlib.Path(__file__).resolve().parent.parent
REGISTRY = ROOT / "docs" / "rfc" / "registry.toml"
REPORT = ROOT / "docs" / "compliance.md"
NAMES = ROOT / "crates" / "sipx-sip" / "src" / "name.rs"
METHODS = ROOT / "crates" / "sipx-sip" / "src" / "message.rs"

STATUS_ORDER = ["implemented", "partial", "syntax", "none", "n/a"]
STATUS_LABEL = {
    "implemented": "✅ implemented",
    "partial": "🟡 partial",
    "syntax": "🔤 syntax only",
    "none": "⬜ not started",
    "n/a": "— superseded",
}
LAYER_ORDER = ["wire", "transport", "core", "security", "media", "services"]
LAYER_TITLE = {
    "wire": "Wire — can the bytes be represented at all?",
    "transport": "Transport — how a message travels, and how the far end is found",
    "core": "Core — transactions, dialogs, negotiation",
    "security": "Security — transport security, authentication, identity",
    "media": "Media — describing it, carrying it, encoding it",
    "services": "Services — what is built on top",
}


def known_headers() -> set[str]:
    """Header variants the parser knows, read from the name table."""
    return set(re.findall(r"^\s+(\w+)\s+=> \"", NAMES.read_text(), re.M))


def known_methods() -> set[str]:
    """Method tokens the parser knows, read from the wire spellings."""
    return {m.upper() for m in re.findall(r'b"([A-Z]+)" =>', METHODS.read_text())}


# The registry's grain is one row per RFC, decided in docs/designs/rfc-registry-grain.md and
# promised to downstream registries in docs/rfc/README.md. These two sets are that decision in
# executable form.
#
# The guard exists because the alternative failure is silent. `tomllib` accepts any key, and a
# checker that only reads the keys it knows walks past the rest — so a finer-grained
# `[[rfc.requirement]]` row, or a `role` typed for `roles`, lands in the source, never reaches
# the generated table, and nobody is told. A registry whose claims can go missing between the
# source and the published table is the exact failure this file was written to prevent.
REQUIRED_KEYS = {"number", "title", "layer", "status", "evidence", "note"}
OPTIONAL_KEYS = {"roles", "headers", "methods", "spec"}
LIST_KEYS = {"evidence", "roles", "headers", "methods"}
# `spec` is the normative document for the RFC — the registry's half of AGENTS.md
# non-negotiable 4. One path, not a list: a subsystem has one spec, and a row that named two
# would be describing a subsystem boundary that has not been decided.
STRING_KEYS = {"spec"}


# Where an application asks for a call. A media capability the call layer cannot select is one
# no UA role can perform, however well the crate below implements and tests it.
CALL_CRATE = "sipx-call"
CRATES = ROOT / "crates"

# The rule is scoped to one layer. That is a *choice*, not something the workspace forced, and
# the reason is that the media layer is the one place where the crate serving a role and the
# crate implementing the capability come apart. A media row claims `uac`/`uas` — placing and
# answering calls — which an application does through `sipx-call`, while the capability lives in
# `sipx-media` or `sipx-sdp`. Nothing makes `sipx-call` select it, and twice it did not: ICE and
# DTLS-SRTP were built, tested and claimed for both roles with no call able to ask for either.
# Elsewhere that gap does not exist. Transport, core and security capabilities sit on the path
# every call already takes, so "can a call reach the transaction layer" has no false answer; and
# a services row like RFC 3856 claims `uas` for a surface `sipx-ua` itself serves, so the check
# would only be asking whether a crate's public API reaches its own module.
#
# Measured before adoption, the unscoped rule rejects 22 of the 29 role-claiming rows, and only
# 7 of those rejections are real. docs/designs/rfc-registry-grain.md carries the full count, the
# argument, and what would widen this — including the two ways the scope can be worked around.
ROLE_REACHABILITY_LAYERS = {"media"}


def call_layer_crates() -> set[str]:
    """`sipx-call` and every workspace crate that can reach it.

    Read from the manifests rather than listed here. A list would be one more hand-copied fact
    about the workspace, and the way that fails is silent: a new crate above `sipx-call` would
    not count as reachable, so a row citing it would be rejected for citing the right file.
    """
    dependencies = {}
    for manifest in sorted(CRATES.glob("*/Cargo.toml")):
        parsed = tomllib.loads(manifest.read_text())
        named = set(parsed.get("dependencies", {})) | set(parsed.get("dev-dependencies", {}))
        dependencies[manifest.parent.name] = {n for n in named if n.startswith("sipx-")}

    reachable = {CALL_CRATE}
    growing = True
    while growing:
        growing = False
        for crate, on in dependencies.items():
            if crate not in reachable and on & reachable:
                reachable.add(crate)
                growing = True
    return reachable


def reaches_the_call_layer(path: str, crates: set[str]) -> bool:
    """Whether an evidence path is a source file in a crate at or above the call layer.

    Only `crates/<name>/…` counts. The repository-root `tests/` tree is the interop harness —
    shell scripts and peer configuration, not Rust — and its Rust half lives in
    `crates/sipx-cli/tests/`, which this already accepts. Admitting the root tree wholesale would
    have made `tests/interop/README.md` proof that a role is reachable, since `evidence` may
    legitimately cite markdown (RFC 5922 cites a spec). No row relied on it.
    """
    parts = pathlib.PurePosixPath(path).parts
    return len(parts) > 1 and parts[0] == "crates" and parts[1] in crates


def unreachable_role_claims(entry, crates: set[str]) -> list[str]:
    """A claimed role that no cited file shows a call can reach.

    This is the one thing the other checks cannot see. A header must be in the parser's table and
    a file must exist, but "implemented in a crate" and "reachable from a call" are different
    facts, and until this check they were reported as the same one — five times in two days.
    Unreachable code is untested code with better paperwork.
    """
    if entry.get("layer") not in ROLE_REACHABILITY_LAYERS:
        return []
    roles = entry.get("roles")
    if not isinstance(roles, list) or not roles:
        return []
    if any(reaches_the_call_layer(p, crates) for p in entry.get("evidence", [])):
        return []
    return [
        f"RFC {entry.get('number', '?')} claims {', '.join(roles)} but cites nothing a call can"
        f" reach — no evidence at or above {CALL_CRATE}. Either cite the call-layer code or test"
        f" that selects it, or drop the roles and say in the note what is missing"
    ]


def schema_problems(entry) -> list[str]:
    """Ways an entry departs from the per-RFC schema.

    Kept separate from `check` because this asks a different question. `check` asks whether a
    claim is true; this asks whether the entry is shaped like a claim at all — and an entry that
    is not cannot be checked, only ignored.
    """
    where = f"RFC {entry.get('number', '?')}"
    problems = []

    for key in sorted(REQUIRED_KEYS - entry.keys()):
        problems.append(f"{where} is missing the required key {key!r}")

    for key in sorted(entry.keys() - REQUIRED_KEYS - OPTIONAL_KEYS):
        hint = ""
        if key == "requirement":
            # Named explicitly, because this is the one somebody adds on purpose.
            hint = (
                " — the registry's grain is one row per RFC; see"
                " docs/designs/rfc-registry-grain.md before changing that"
            )
        problems.append(f"{where} carries the unknown key {key!r}{hint}")

    if "number" in entry and not isinstance(entry["number"], int):
        problems.append(f"{where} has a non-integer number, which cannot be referenced")
    for key in sorted(LIST_KEYS & entry.keys()):
        value = entry[key]
        if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
            problems.append(f"{where} has {key!r}, which must be a list of strings")
    for key in sorted(STRING_KEYS & entry.keys()):
        if not isinstance(entry[key], str):
            problems.append(f"{where} has {key!r}, which must be a single path")

    return problems


def check(entries) -> list[str]:
    """Every claim that the code does not back up."""
    headers, methods = known_headers(), known_methods()
    reachable = call_layer_crates()
    problems = []

    for entry in entries:
        # `.get` throughout: an entry can be malformed, and a checker that crashes on one reports
        # nothing about the other sixty-eight.
        where = f"RFC {entry.get('number', '?')}"

        problems.extend(schema_problems(entry))

        if entry.get("status") not in STATUS_LABEL:
            problems.append(f"{where}: unknown status {entry.get('status')!r}")
        if entry.get("layer") not in LAYER_TITLE:
            problems.append(f"{where}: unknown layer {entry.get('layer')!r}")

        for header in entry.get("headers", []):
            if header not in headers:
                problems.append(
                    f"{where} names header {header!r}, which the parser does not know"
                )
        for method in entry.get("methods", []):
            if method not in methods:
                problems.append(
                    f"{where} names method {method!r}, which the parser does not know"
                )
        for path in entry.get("evidence", []):
            if not (ROOT / path).exists():
                problems.append(f"{where} cites {path}, which does not exist")

        # Held to the same standard as evidence. A `spec` that has been moved or renamed leaves
        # the table reading as though the subsystem is specified and the link going nowhere,
        # which is worse than an empty cell.
        spec = entry.get("spec")
        if isinstance(spec, str) and not (ROOT / spec).exists():
            problems.append(f"{where} names the spec {spec}, which does not exist")

        # An entry that claims to be implemented and points at nothing is an assertion.
        if entry.get("status") in {"implemented", "partial"} and not entry.get("evidence"):
            problems.append(f"{where} claims {entry.get('status')} with no evidence cited")

        problems.extend(unreachable_role_claims(entry, reachable))

    numbers = [e["number"] for e in entries if "number" in e]
    for number, count in Counter(numbers).items():
        if count > 1:
            problems.append(f"RFC {number} appears {count} times")

    problems.extend(stale_counts(len(entries)))
    return problems


# Prose that states the number of tracked RFCs. Every one of these is a copy of a fact whose
# source is the registry, so every one of them drifts the moment an RFC is added — which is
# exactly what happened when the QUIC entries landed and four documents kept saying 61.
COUNTED_IN = [
    "README.md",
    "website/docs/guides/does-this-fit.md",
    "website/docs/reference/compliance.md",
    # Not generated, and it opens by counting the gaps — so it drifts the moment an RFC is
    # added. It did, between one story and the next, which is why it is on this list.
    "docs/rfc-roadmap.md",
]
COUNT_PATTERNS = [
    # "63 RFCs", and "63 tracked RFCs" — the second phrasing is how prose usually says it, and
    # a checker that only knew the first one let a stale count sit in the RFC roadmap.
    re.compile(r"(?<!\d)(\d{2,3}) (?:tracked )?RFCs"),
    re.compile(r"RFCs%20tracked-(\d{2,3})-"),
]


def stale_counts(tracked: int) -> list[str]:
    """Prose stating an RFC count that the registry no longer agrees with.

    A generated table cannot drift, but a sentence in the README that says "61 RFCs" is a
    hand-copied fact and drifts silently. Checking it here is cheaper than noticing later that
    the front page understates the work by two.
    """
    problems = []
    for name in COUNTED_IN:
        path = ROOT / name
        if not path.exists():
            problems.append(f"{name} is listed as stating the RFC count but does not exist")
            continue
        text = path.read_text(encoding="utf-8")
        found = {m.group(1) for pattern in COUNT_PATTERNS for m in pattern.finditer(text)}
        for stated in sorted(found):
            if int(stated) != tracked:
                problems.append(
                    f"{name} says {stated} RFCs; the registry has {tracked}"
                )
    return problems


def spec_link(entry) -> str:
    """The `Spec` cell: a link to the normative document, relative to the report.

    The registry stores a repository-relative path so that `--check` can test it for existence
    from anywhere; the table needs one relative to `docs/compliance.md`, which is where a reader
    clicks it. The two are not the same string, and writing the repository-relative one into the
    table produces a link that is dead everywhere except the repository root.
    """
    spec = entry.get("spec")
    if not isinstance(spec, str) or not spec:
        return "—"
    href = os.path.relpath(ROOT / spec, REPORT.parent).replace(os.path.sep, "/")
    return f"[{pathlib.PurePosixPath(spec).stem}]({href})"


def render(entries) -> str:
    by_layer = defaultdict(list)
    for entry in entries:
        by_layer[entry["layer"]].append(entry)

    counts = Counter(e["status"] for e in entries)
    tracked = len(entries)

    out = [
        "# RFC compliance",
        "",
        "<!-- Generated by scripts/rfc-report.py from docs/rfc/registry.toml. Do not edit. -->",
        "",
        "What sipx implements, what it only parses, and what it has not started — measured",
        "rather than asserted. `scripts/rfc-report.py --check` runs in CI and fails the build if",
        "an entry names a header the parser does not know, or cites a file that does not exist.",
        "",
        "It cannot check that behaviour is *correct*; the tests do that, and each entry points at",
        "them. What it can do is stop this table drifting away from the code, which is the way a",
        "compliance document usually becomes untrue.",
        "",
        "## Where it stands",
        "",
        f"**{tracked} RFCs tracked.**",
        "",
        "| | Meaning | Count |",
        "|---|---|---|",
    ]
    meanings = {
        "implemented": "Behaviour present and tested for the roles listed",
        "partial": "Some of the normative behaviour; the note says which part is missing",
        "syntax": "The parser represents it; nothing acts on it",
        "none": "Tracked as a target, not started",
        "n/a": "Obsoleted by a later RFC that is tracked instead",
    }
    for status in STATUS_ORDER:
        out.append(f"| {STATUS_LABEL[status]} | {meanings[status]} | {counts.get(status, 0)} |")

    out += [
        "",
        "The list is bounded on purpose: it is what sipx already touches plus what it has decided",
        "to aim at. It is not every SIP-related RFC and it never will be — some update or obsolete",
        "others, some define alternatives to each other, and some belong to trust domains sipx does",
        "not operate in. The order to add them in is in the",
        "[RFC roadmap](rfc-roadmap.md).",
        "",
        "**Roles.** sipx is a user agent. Where an RFC defines proxy or registrar behaviour, that",
        "behaviour is not implemented even when the UA half is — the `Roles` column says which.",
        "",
    ]

    for layer in LAYER_ORDER:
        rows = by_layer.get(layer, [])
        if not rows:
            continue
        rows.sort(key=lambda e: (STATUS_ORDER.index(e["status"]), e["number"]))
        out += [
            f"## {LAYER_TITLE[layer]}",
            "",
            "| RFC | Title | Status | Roles | Spec | Notes |",
            "|---|---|---|---|---|---|",
        ]
        for entry in rows:
            roles = ", ".join(entry.get("roles", [])) or "—"
            link = f"[{entry['number']}](https://www.rfc-editor.org/rfc/rfc{entry['number']})"
            note = entry.get("note", "").replace("|", "\\|")
            out.append(
                f"| {link} | {entry['title']} | {STATUS_LABEL[entry['status']]} | {roles} |"
                f" {spec_link(entry)} | {note} |"
            )
        out.append("")

    out += [
        "## How to read a status",
        "",
        "**Syntax only is not a half-measure, it is a different thing.** sipx parses `RAck` and",
        "`RSeq` and does nothing with them: a message carrying them survives the wire and is",
        "forwarded intact, and no PRACK is ever sent. That is worth recording separately, because",
        "\"we support RFC 3262\" would be false and \"we reject it\" would also be false.",
        "",
        "**Partial always says what is missing.** An entry that cannot name the gap should be",
        "`none` until somebody works out what it is.",
        "",
        "**The Spec column is the normative contract, not the evidence.** Where it is filled in,",
        "that document says what sipx must do about the RFC — RFC citations, types, state, and the",
        "byte-level vectors the tests are derived from — and the entry's status is measured against",
        "it. An em dash means the subsystem has no spec yet, which for a non-trivial one is a gap",
        "worth a story rather than a fact about the table.",
        "",
    ]
    return "\n".join(out) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify claims and that the report is current")
    args = parser.parse_args()

    entries = tomllib.loads(REGISTRY.read_text())["rfc"]

    # Shape before substance. `render` indexes entries directly, so a malformed one would crash
    # it — and a traceback in place of "RFC 3261 carries the unknown key 'requirement'" tells
    # whoever added the row nothing about what to do next.
    malformed = [p for entry in entries for p in schema_problems(entry)]
    if malformed:
        print("The RFC registry does not match its schema:", file=sys.stderr)
        for problem in malformed:
            print(f"  {problem}", file=sys.stderr)
        return 1

    problems = check(entries)
    rendered = render(entries)

    if args.check:
        if REPORT.exists() and REPORT.read_text() != rendered:
            problems.append(
                f"{REPORT.relative_to(ROOT)} is out of date; run scripts/rfc-report.py"
            )
        if problems:
            print("RFC compliance claims that the code does not back up:", file=sys.stderr)
            for problem in problems:
                print(f"  {problem}", file=sys.stderr)
            return 1
        print(f"rfc compliance: {len(entries)} RFCs, every claim backed")
        return 0

    if problems:
        for problem in problems:
            print(f"warning: {problem}", file=sys.stderr)
    REPORT.write_text(rendered)
    print(f"wrote {REPORT.relative_to(ROOT)}: {len(entries)} RFCs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
