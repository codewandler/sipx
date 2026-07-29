#!/usr/bin/env python3
"""Hold what `sipx-audio` advertises against what `sipx-audio` implements.

The crate's package description promised "G.722 … and resampling" from the commit that
scaffolded the workspace until `X-26`, and neither ever existed. Nothing caught it because
nothing connected the sentence to the code: the description is metadata, the crate documentation
is a comment, and the website's crate table is prose. `X-25` went looking for the argument behind
dropping G.722, found no story, no spec and no commit message, and found the claim still being
made in three places instead. A fourth hand correction would have left the arrangement that
produced the first three.

So the claim is now checked rather than remembered. Every place `sipx-audio` advertises itself —

    the `description` in its manifest, which is the registry listing;
    the summary paragraph of its crate documentation, which is the front page of the API
    reference;
    its row in the website's "Which crate" table —

is read for codec and capability names, and each name must be backed by code in the crate: a
codec by a module that both encodes and decodes it, a capability by a public item that provides
it. A codec named in one of those strings and implemented nowhere fails the gate.

Three things this deliberately does not do.

**It reads the crate documentation's summary paragraph, not the whole header.** The prose below
the summary is where the crate says what it does *not* do — `X-26`'s record of why G.722 and
resampling are absent lives there — and a check that could not tell a claim from a disclaimer
would forbid writing the decision down, which is the opposite of the point. The summary is what a
reader is shown before choosing to read further, so it is the string that has to be true on its
own.

**It checks one crate.** The rule generalises and the failure did not: `sipx-audio` is the crate
that *is* the codecs, so a codec name in its blurb reads as an implementation, while `sipx-call`
naming DTMF describes an API over a payload format implemented in `sipx-rtp`. A check that
demanded both mean the same thing would be switched off by whoever hit it second.

**It does not check whether a codec is any good, only that both directions exist.** A stack that
can decode a codec and not encode it cannot offer it — that argument is `sipx-audio/src/opus.rs`'s
and it is prose. What this enforces is the weaker mechanical claim, which is the one that was
false.
"""

import re
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parent.parent

#: The crate whose advertising this checks. Everything else here is derived from it.
CRATE = "sipx-audio"
CRATE_DIR = ROOT / "crates" / CRATE
MANIFEST = CRATE_DIR / "Cargo.toml"
LIB = CRATE_DIR / "src" / "lib.rs"

#: The guide whose "Which crate" table restates every crate's description in the reader's words.
#: It is the third place the G.722 claim was found, so it is the third string checked.
GUIDE = ROOT / "website" / "docs" / "guides" / "as-a-library.md"

#: Below this the module reader has stopped understanding `lib.rs` rather than found a small
#: crate. A reader that silently finds no modules backs no claim and would pass a description
#: promising everything.
_PLAUSIBLE_MODULES = 3


class Claim(NamedTuple):
    """Something a description can promise, and how a description writes it."""

    #: What to call it in the report.
    name: str
    #: How the strings spell it. Matched case-insensitively against each front door.
    written: str
    #: For a capability, the substring a public item's name must contain. Empty for a codec,
    #: whose evidence is a module that encodes and decodes it rather than a single symbol.
    symbol: str = ""


#: Codec names a telephony crate might claim. The list is the vocabulary a reader recognises as
#: "this crate does that codec"; a name outside it is not a codec claim and is not checked.
CODECS = (
    Claim("G.711", r"G\.?711"),
    Claim("G.722", r"G\.?722"),
    Claim("G.723.1", r"G\.?723(\.1)?"),
    Claim("G.726", r"G\.?726"),
    Claim("G.729", r"G\.?729"),
    Claim("Opus", r"\bOpus\b"),
    Claim("iLBC", r"\biLBC\b"),
    Claim("AMR", r"\bAMR(-WB)?\b"),
    Claim("Speex", r"\bSpeex\b"),
    Claim("GSM", r"\bGSM\b"),
    Claim("L16", r"\bL16\b"),
)

#: The rest of what the description promised. `resampling` is here because it was the other half
#: of the untruth `X-26` removed, and DTMF because RFC 4733 telephone-events are an RTP payload
#: format that lives in `sipx-rtp` — a claim this crate cannot back however true it is elsewhere.
CAPABILITIES = (
    Claim("resampling", r"resampl\w*", "resample"),
    Claim("RFC 4733 DTMF", r"\bDTMF\b|RFC ?4733", "dtmf"),
    Claim("WAV", r"\bWAV\b", "wav"),
    Claim("mixing", r"\bmix\w*", "mix"),
)

_MODULE = re.compile(
    r'(?:#\[cfg\(feature = "(?P<feature>[\w-]+)"\)\]\s*\n\s*)?pub mod (?P<name>\w+);'
)
_PUBLIC_ITEM = re.compile(r"\bpub (?:fn|struct|enum|trait|const|type) (?P<name>\w+)")


class Module(NamedTuple):
    """One module of the crate, reduced to what can back a claim."""

    name: str
    #: The feature that gates it, or empty. A codec behind a feature is off by default, so a
    #: description naming it has to say so.
    feature: str
    #: Its own `//!` header, where a codec module names its codec.
    header: str
    #: Every public item name in the file, methods included — `Encoder::encode` is an encoder.
    items: tuple[str, ...]

    def provides(self, direction: str) -> bool:
        """Whether the module has an `encode` or a `decode` path, by either spelling."""
        return any(direction in item.lower() for item in self.items)


class FrontDoor(NamedTuple):
    """One string the crate advertises itself with, and where to point when it is wrong."""

    where: str
    text: str


def header(text: str) -> str:
    """The leading `//!` block of a Rust file, comment markers stripped."""
    lines = []
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("//!"):
            break
        lines.append(stripped[3:].strip())
    return "\n".join(lines)


def summary(text: str) -> str:
    """The first paragraph of a `//!` header — what a reader is shown before reading on.

    Everything after the first blank comment line is the crate arguing with itself, including
    what it deliberately does not implement. See the module docstring.
    """
    first, _, _ = header(text).partition("\n\n")
    return first


def modules(lib: str) -> list[Module]:
    """The crate's modules, each with its feature gate, its header and its public items."""
    found: list[Module] = []
    for match in _MODULE.finditer(lib):
        name = match.group("name")
        source = CRATE_DIR / "src" / f"{name}.rs"
        if not source.exists():
            raise ValueError(f"{LIB.relative_to(ROOT)} declares `pub mod {name}` and {source} is not there")
        text = source.read_text(encoding="utf-8")
        found.append(
            Module(
                name=name,
                feature=match.group("feature") or "",
                header=header(text),
                items=tuple(item.group("name") for item in _PUBLIC_ITEM.finditer(text)),
            )
        )

    if len(found) < _PLAUSIBLE_MODULES:
        raise ValueError(
            f"read only {len(found)} modules from {LIB.relative_to(ROOT)}; the reader has drifted "
            f"from the file's shape and would back no claim the crate makes"
        )
    return found


def implements(claim: Claim, found: list[Module]) -> Module | None:
    """The module that backs a codec claim: it names the codec and goes both ways."""
    written = re.compile(claim.written, re.I)
    for module in found:
        if written.search(module.header) and module.provides("encode") and module.provides("decode"):
            return module
    return None


def front_doors() -> list[FrontDoor]:
    """Every string in the repository that tells a reader what `sipx-audio` is."""
    manifest = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    description = manifest["package"].get("description", "")
    if not description:
        raise ValueError(f"{MANIFEST.relative_to(ROOT)} has no package description")

    lead = summary(LIB.read_text(encoding="utf-8"))
    if not lead:
        raise ValueError(f"{LIB.relative_to(ROOT)} opens with no `//!` summary")

    rows = [
        line
        for line in GUIDE.read_text(encoding="utf-8").splitlines()
        if line.startswith("|") and f"`{CRATE}`" in line
    ]
    if len(rows) != 1:
        raise ValueError(
            f"{GUIDE.relative_to(ROOT)} has {len(rows)} rows naming `{CRATE}` and needs exactly "
            f"one; without it the guide can promise a codec and nothing notices"
        )
    cells = [cell.strip() for cell in rows[0].strip("|").split("|")]

    return [
        FrontDoor(f"{MANIFEST.relative_to(ROOT)} description", description),
        FrontDoor(f"{LIB.relative_to(ROOT)} summary", lead),
        FrontDoor(f"{GUIDE.relative_to(ROOT)} crate table", cells[0]),
    ]


def claimed(door: FrontDoor, vocabulary: tuple[Claim, ...]) -> list[Claim]:
    return [claim for claim in vocabulary if re.search(claim.written, door.text, re.I)]


def claim_problems(doors: list[FrontDoor], found: list[Module]) -> list[str]:
    """Every promise in a front door that the crate cannot keep."""
    problems: list[str] = []
    for door in doors:
        for claim in claimed(door, CODECS):
            module = implements(claim, found)
            if module is None:
                problems.append(
                    f"{door.where} names {claim.name} and no module of {CRATE} both encodes and "
                    f"decodes it; implement it or stop advertising it"
                )
                continue
            if module.feature and not names_the_feature(door.text, module.feature):
                problems.append(
                    f"{door.where} names {claim.name}, which is behind the `{module.feature}` "
                    f"feature and off by default; say so, or a reader takes it for granted"
                )
        for claim in claimed(door, CAPABILITIES):
            if not any(claim.symbol in item.lower() for module in found for item in module.items):
                problems.append(
                    f"{door.where} names {claim.name} and {CRATE} exposes no `{claim.symbol}`; "
                    f"implement it or stop advertising it"
                )
    return problems


def names_the_feature(text: str, feature: str) -> bool:
    """Whether a description says a codec is optional.

    The feature name is matched case-sensitively and the word "feature" is required with it:
    `opus` the feature and `Opus` the codec differ by one letter, and a rule satisfied by the
    codec's own name would be no rule at all.
    """
    return feature in text and re.search(r"\bfeatures?\b", text, re.I) is not None


def agreement_problems(doors: list[FrontDoor]) -> list[str]:
    """The three strings must promise the same codecs, since they describe the same crate."""
    sets = {door.where: {claim.name for claim in claimed(door, CODECS)} for door in doors}
    first, *rest = doors
    problems = []
    for door in rest:
        if sets[door.where] != sets[first.where]:
            problems.append(
                f"{door.where} claims {sorted(sets[door.where]) or 'no codec'} and {first.where} "
                f"claims {sorted(sets[first.where]) or 'no codec'}; one crate, one answer"
            )
    return problems


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] != "--check":
        print("usage: check-audio-claims.py --check", file=sys.stderr)
        return 2

    found = modules(LIB.read_text(encoding="utf-8"))
    doors = front_doors()
    problems = claim_problems(doors, found) + agreement_problems(doors)
    if problems:
        print(f"{CRATE} advertises what it does not implement:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    codecs = sorted({claim.name for door in doors for claim in claimed(door, CODECS)})
    print(
        f"{CRATE}: {len(doors)} front doors, {len(codecs)} codecs claimed "
        f"({', '.join(codecs) or 'none'}), every one of them implemented"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
