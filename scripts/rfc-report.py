#!/usr/bin/env python3
"""Render docs/compliance.md from docs/rfc/registry.toml, and check that it is not lying.

A compliance table nobody can verify is marketing. This does two things a hand-written table
cannot: it regenerates the document so it cannot drift from its source, and with `--check` it
holds the source against the code — every header and method an entry names must actually be
known to the parser, every file it cites must exist, and a row claiming behaviour must cite code
rather than only prose (`prose_only_claims`, which RFC 8996 was the one row to fail).

What it deliberately does *not* claim to verify is behaviour. No script can read
`crates/sipx-sip/src/transaction/client.rs` and decide whether Timer A is right; the tests do
that. What it can do is stop an entry claiming syntax support for a header the code has never
heard of, which is the failure mode a table like this actually has.

The second failure mode, and the one that recurred: a capability implemented and tested inside
one crate that nothing above it can select, reported as shipped because every check above passes
for it. `unreachable_claims` is the check for that. It is scoped to the layers where a capability
must be *selected* before a call can use it — media and security — by choice and not by necessity,
and it asks the question of both kinds of claim a row makes there: the roles it lists, and, at the
media layer, the status it states. The argument, the measurement each scope rests on and the ways
they can still be walked around are in docs/designs/rfc-registry-grain.md.
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


# The statuses that claim behaviour, and so must point at code. `syntax` is not one of them: it
# claims the parser represents something, which `known_headers` and `known_methods` check against
# the name table itself rather than against a path. `none` and `n/a` claim nothing and cite nothing.
CODE_BACKED_STATUSES = {"implemented", "partial"}

# Where an application asks for a call. A media capability the call layer cannot select is one
# no UA role can perform, however well the crate below implements and tests it.
CALL_CRATE = "sipx-call"
CRATES = ROOT / "crates"

# The rule is scoped to the layers where a capability must be **selected**. That is a *choice*,
# not something the workspace forced.
#
# The property: a capability at a selection layer is carried only because something asked for it —
# `Capabilities::with_srtp`, `with_dtls_srtp`, `start_with_ice`, `Config::with_credentials`, a
# `Target` built with a secure `TransportKind` — and asking for nothing is both the default and
# silent. The call still connects, unencrypted and unauthenticated, and every test in the crate
# below still passes. That is how ICE and DTLS-SRTP came to be built, tested and claimed for both
# roles with no call able to select either.
#
# The other layers have no such gap because their capabilities are not selected at all: there is no
# `with_transactions` and no `with_dns`, so "can a call reach the transaction layer" is a question
# that cannot come out `no`. And a services row like RFC 3856 claims `uas` for a surface `sipx-ua`
# *itself* serves — `sipx-call` does not depend on `sipx-ua` and no crate above it must select
# anything.
#
# `security` was added by X-33. It has both halves of the property: `Config::with_credentials` is
# the opt-in for digest and its only caller above the call layer sits inside an
# `if let Some(password)` (crates/sipx-cli/src/register.rs), so a REGISTER with no password still
# succeeds; and a secure transport is chosen per `Target`, with UDP the default. Measured, the four
# security rows claiming a role all failed the rule, and three needed one honest citation each —
# which is what X-30's first false justification ("those rows cannot satisfy it at any price")
# denied was possible.
#
# `transport` was measured and left out: the layer mixes capabilities that *are* selected (RFC 7118
# WebSocket, 5626 outbound, 8599 push) with plumbing on the path of every call (3263 DNS, 3581
# `rport`), and an evidence-path check cannot tell them apart. Widening to it would reject 3263 and
# 3581, which no citation could honestly fix. That is the proxy failing, and it is the case for the
# successor check in docs/designs/rfc-registry-grain.md rather than for a longer layer list.
#
# `layer` is a proxy, used because this check reads evidence paths and nothing else; deciding
# selection means resolving callers across crates, which is a different check on a different input.
# The proxy's cost is that `layer` is author-set — `misdeclared_layer` below closes that for the
# rows that would want the dodge.
#
# Measured before adoption on 57857c6, the unscoped rule rejects 22 of the 29 role-claiming rows.
# Only 7 of those rejections point at anything true of the row and only 3 rows were over-claiming
# at all, so on the question this check exists to answer the unscoped rule is wrong 19 times out of
# 22. docs/designs/rfc-registry-grain.md carries the full count, the argument, the false
# justifications this scope has been given, and what would widen it.
ROLE_REACHABILITY_LAYERS = {"media", "security"}

# The same rule applied to `status = "implemented"` rather than to `roles`, and scoped tighter.
#
# X-30 recorded that the check keyed on `roles` alone, so a row could say `implemented` about
# something no call can reach and never be interrogated — which is what RFC 6716 and 7587 did for
# Opus. A row with no `roles` claims nothing about a role; it still makes a claim, in the same
# generated table, under a heading that reads "What sipx implements".
#
# Media only, and the reason is measured rather than preferred. Every media row claiming
# `implemented` names a capability a call either carries or does not: SDP, RTP, DTMF and G.711 are
# carried by every call and cite the call layer; Opus is carried by none. At the security layer
# three of the seven `implemented` rows — 6125, 8446, 8996 — state *policies* of the TLS stack (a
# non-matching SAN is refused rather than falling back to the CN; 1.3 preferred; 1.2 is the floor
# and not configurable downward). A policy holds on every connection and is proved by the absence
# of an API, so "which call reaches it" is not the question those rows answer, and asking it would
# reject three honest rows. Held by
# `test_the_status_gate_is_media_only_and_the_reason_is_measured`.
#
# `partial` is deliberately not held to this. It is the *demotion*, not an exception: it changes
# what the published table says, from "✅ implemented" to "🟡 partial" with the note naming the gap,
# which is the form RFC 5763, 5764, 8122, 8445 and 8839 already use for exactly this fact. There is
# no suppression list — a row that cannot be made true is demoted, and every reader of the
# generated table sees the demotion.
STATUS_REACHABILITY_LAYERS = {"media"}

# Crates whose only subject is media. A row citing one of them is a media row whatever its `layer`
# says, which closes the dodge X-30 recorded as the strongest argument against scoping by layer at
# all: nothing validated `layer` beyond membership of `LAYER_TITLE`, so relabelling an ICE row
# `security` left the check entirely.
#
# It is not a general layer classifier and does not try to be. `sipx-sdp` is cited by RFC 3264,
# which is legitimately `core`, and `sipx-transport` is cited by rows at three layers — so those
# crates cannot pin anything. These three can, they are exactly the crates an unreachable media
# capability lives in, and to leave the check an author would now have to stop citing their own
# implementation. `test_no_row_declares_a_layer_its_evidence_contradicts` holds the registry to it.
MEDIA_ONLY_CRATES = {"sipx-media", "sipx-rtp", "sipx-audio"}


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


def cites_workspace_code(path: str) -> bool:
    """Whether an evidence path is Rust source in a workspace crate.

    The weakest useful thing a citation can be: something that compiles, that tests run over, and
    that can therefore stop being true. `crates/<name>/….rs` and nothing else — a spec, a README or
    a manifest is prose about the code rather than the code.
    """
    parts = pathlib.PurePosixPath(path).parts
    return len(parts) > 1 and parts[0] == "crates" and parts[-1].endswith(".rs")


def reaches_the_call_layer(path: str, crates: set[str]) -> bool:
    """Whether an evidence path is a Rust source file in a crate at or above the call layer.

    Only `crates/<name>/….rs` counts, and both conditions were holes somebody had to walk into
    before they were closed. The repository-root `tests/` tree is the interop harness — shell
    scripts and peer configuration — and its Rust half lives in `crates/sipx-cli/tests/`, which
    this accepts; admitting the root tree wholesale would have made `tests/interop/README.md` proof
    that a role is reachable, since `evidence` may legitimately cite markdown. `X-30` closed that
    and recorded the same hole one directory in: `crates/sipx-call/README.md` would have satisfied
    a rule that only asked which crate the path was in.

    The `.rs` condition closes it, and costs nothing measurable: exactly one evidence path in the
    registry is not a `.rs` file — `docs/specs/sip-tls.md`, cited by RFC 5922 — and it is outside
    `crates/` anyway. (X-33 recorded two, RFC 8996's citation of the same spec being the other;
    X-43 replaced that one with code, which is what `prose_only_claims` below now requires of every
    behaviour claim.) It remains a path test and not a code test, so a row can still satisfy it by
    citing a call-layer file containing a dead branch; that is the limit the successor check in
    docs/designs/rfc-registry-grain.md exists to remove.
    """
    return cites_workspace_code(path) and pathlib.PurePosixPath(path).parts[1] in crates


def unreachable_claims(entry, crates: set[str]) -> list[str]:
    """A claim at a selection layer that no cited file shows a call can reach.

    This is the one thing the other checks cannot see. A header must be in the parser's table and
    a file must exist, but "implemented in a crate" and "reachable from a call" are different
    facts, and until this check they were reported as the same one — five times in two days.
    Unreachable code is untested code with better paperwork.

    Two claims trigger it, and the remedy differs, so the message says which one applies. A `roles`
    list is a claim about what a *user agent does*; `status = "implemented"` is a claim about the
    code, and at the media layer those come to the same thing because a media capability is either
    carried by a call or by nothing at all.
    """
    layer = entry.get("layer")
    roles = entry.get("roles")
    roles = roles if isinstance(roles, list) else []

    claiming_roles = bool(roles) and layer in ROLE_REACHABILITY_LAYERS
    claiming_status = (
        entry.get("status") == "implemented" and layer in STATUS_REACHABILITY_LAYERS
    )
    if not (claiming_roles or claiming_status):
        return []
    if any(reaches_the_call_layer(p, crates) for p in entry.get("evidence", [])):
        return []

    if claiming_roles:
        claim = ", ".join(roles)
        remedy = "or drop the roles and say in the note what is missing"
    else:
        claim = "implemented"
        remedy = (
            "or lower the status to `partial` and say in the note that no call can select it"
        )
    return [
        f"RFC {entry.get('number', '?')} claims {claim} but cites nothing a call can reach — no"
        f" Rust source at or above {CALL_CRATE}. Either cite the call-layer code or test that"
        f" selects it, {remedy}"
    ]


def misdeclared_layer(entry) -> list[str]:
    """A row citing a media-only crate while declaring some other layer.

    `layer` decides whether the reachability rule looks at a row at all, and nothing else validated
    it beyond membership of `LAYER_TITLE` — so relabelling an ICE row `security` was the way out of
    the check, recorded by `X-30` as the strongest argument against scoping by layer. This does not
    classify layers in general; it makes the one dodge that matters unavailable, because the row
    that would want it cites its own implementation and that implementation is in `sipx-media`,
    `sipx-rtp` or `sipx-audio`.
    """
    layer = entry.get("layer")
    if layer == "media":
        return []
    cited = set()
    for path in entry.get("evidence", []):
        parts = pathlib.PurePosixPath(path).parts
        if len(parts) > 1 and parts[0] == "crates":
            cited.add(parts[1])
    offending = sorted(cited & MEDIA_ONLY_CRATES)
    if not offending:
        return []
    return [
        f"RFC {entry.get('number', '?')} declares layer {layer!r} but cites"
        f" {', '.join(offending)}, which implement nothing but media — a media row relabelled is a"
        f" media row that has left the reachability check"
    ]


def prose_only_claims(entry) -> list[str]:
    """A behaviour claim every one of whose citations is prose.

    The existing rule is that an `implemented` or `partial` row must cite *something*, and a
    document satisfies it. RFC 8996 was the row that showed why that is not enough: it claimed
    `implemented` against `docs/specs/sip-tls.md`, our own sentence saying 1.2 is the floor. The
    sentence cannot fail. Nothing connected it to a handshake, so the claim would have survived the
    floor being lowered, and the gate would have gone on reporting "every claim backed".

    **Decided by X-43, and measured before adopting**: of the registry's 70 rows, 8996 was the only
    one of 32 `implemented` and 22 `partial` rows citing no `crates/….rs` path at all. So the rule
    costs one row — the row that prompted it, which X-43 fixed — and the next such row is caught on
    the way in rather than by auditing all 70 again. The six `syntax` rows would also pass it today,
    but they are held to the same set the evidence rule already uses rather than a wider one
    invented here: a `syntax` claim is about the parser's name table, which `known_headers` checks
    directly and far better than a path could.

    It is deliberately weaker than `reaches_the_call_layer`: any Rust file in any workspace crate
    counts. "Is this claim about code at all" and "can a call reach that code" are different
    questions, and the second one has a measured false-positive rate that is the reason it is scoped
    to two layers. This one has none to scope around — a behaviour claim that points at no code is
    not a claim anything can check.

    Rows with no evidence at all are left to the existing message, which names the remedy.
    """
    status = entry.get("status")
    evidence = entry.get("evidence", [])
    if status not in CODE_BACKED_STATUSES or not evidence:
        return []
    if any(cites_workspace_code(p) for p in evidence):
        return []
    return [
        f"RFC {entry.get('number', '?')} claims {status} but every path it cites is prose — no Rust"
        " source under crates/. Cite the code, and the test that would fail if the behaviour"
        " changed; a document cannot stop being true on its own"
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
        if entry.get("status") in CODE_BACKED_STATUSES and not entry.get("evidence"):
            problems.append(f"{where} claims {entry.get('status')} with no evidence cited")

        problems.extend(prose_only_claims(entry))
        problems.extend(misdeclared_layer(entry))
        problems.extend(unreachable_claims(entry, reachable))

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
