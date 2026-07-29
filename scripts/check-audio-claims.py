#!/usr/bin/env python3
"""Hold what every published crate advertises against what it implements.

`sipx-audio`'s package description promised "G.722 … and resampling" from the commit that
scaffolded the workspace until `X-26`, and neither ever existed. Nothing caught it because
nothing connected the sentence to the code: the description is metadata, the crate documentation
is a comment, and the crate tables are prose. `X-25` went looking for the argument behind
dropping G.722, found no story, no spec and no commit message, and found the claim still being
made in three places instead. A fourth hand correction would have left the arrangement that
produced the first three.

It did. `X-26` removed the RFC 4733 DTMF claim from the crate and it survived in `README.md`'s
crate table, because the first version of this script read three strings and the README was not
one of them — so `X-35` found the same untruth in the fourth front door, still passing the gate at
exit 0. That is the argument the paragraph above makes, run once more. The check therefore no
longer knows about one crate: every published crate has **four front doors**, and the check reads
all four for every one of them —

    the `description` in its manifest, which is the registry listing;
    the summary paragraph of its `lib.rs` or `main.rs`, which is the front page of the API
    reference;
    its row in `README.md`'s crate table, which is what a reader sees first;
    its row in the website's "Which crate" table —

and the set of crates those two tables name must be exactly the set of crates that publish. A
crate the tables omit is a crate nobody's sentence describes; a table row for a crate that does
not publish points at something a reader cannot depend on.

Three rules run over the doors.

**Membership.** The set of crates the two tables name is the set of crates that publish. Nothing
else is a row and nothing published is missing.

**Restatement.** The manifest description is the crate's canonical sentence about itself — the
string the registry shows. The other three doors restate it: for a reader who wants the short
version, for a reader who opened the API reference, for a reader choosing a crate. A restatement
may say *less* than the description, because compressing is what it is for; it may not claim a
capability the description does not, because then the crate's own listing is not the authority on
the crate. And the two tables, being the same kind of string written for the same reader, must
claim the same set as each other. That pair of rules is what would have caught the DTMF row:
`README.md` named RFC 4733 DTMF, the manifest did not, and neither did the website's table.

Set *equality* across all four doors would be the wrong instrument, and is not what runs here. A
crate summary is written at a different altitude — `Sans-IO SIP core.` is a good first line and a
bad capability list — and a rule that demanded it enumerate every capability would turn every
crate's front page into keywords. Containment is the honest version of "one crate, one answer".

**Backing.** A capability named in any door must be backed by an item of that crate whose name
contains the capability's word. `sipx-media` may claim bridging because it has `Bridge`;
`sipx-call` may not, because it has nothing named bridge — which is `X-35`'s finding and `C-6`'s
gap.

Three things this deliberately does not do.

**It reads each crate documentation's summary paragraph, not the whole header.** The prose below
the summary is where a crate says what it does *not* do — `X-26`'s record of why G.722 and
resampling are absent lives there — and a check that could not tell a claim from a disclaimer
would forbid writing the decision down, which is the opposite of the point. The summary is what a
reader is shown before choosing to read further, so it is the string that has to be true on its
own.

**It checks codecs in one crate only.** The backing rule generalises across crates and the codec
rule does not: `sipx-audio` is the crate that *is* the codecs, so a codec name in its blurb reads
as an implementation and is held to a module that both encodes and decodes it. Elsewhere a codec
name describes a payload type being carried, not an implementation, and a check that demanded
both mean the same thing would be switched off by whoever hit it second.

**It does not check whether a codec is any good, only that both directions exist.** A stack that
can decode a codec and not encode it cannot offer it — that argument is `sipx-audio/src/opus.rs`'s
and it is prose. What this enforces is the weaker mechanical claim, which is the one that was
false.

There is no suppression list, under any name. A claim this check reports is either true and
missing from the other doors, or false and has to go; a third option is what let the first three
corrections be hand corrections.
"""

import re
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

#: The crate that *is* the codecs, and so the only one whose codec names are held to an
#: implementation. See the module docstring.
CODEC_CRATE = "sipx-audio"

#: The two hand-maintained crate tables, each as the file and the heading its table sits under.
#: Anchoring on the heading rather than on "a row mentioning a crate" matters: both files carry a
#: second table whose cells are crate names, and a reader that swept up both would compare a
#: capability list against a list of rustdoc links.
README_TABLE = (ROOT / "README.md", "## Crates")
GUIDE_TABLE = (ROOT / "website" / "docs" / "guides" / "as-a-library.md", "## Which crate")

#: Below this the reader has stopped understanding a crate rather than found a small one. A
#: reader that silently finds no public items backs no claim and would pass a description
#: promising everything.
_PLAUSIBLE_ITEMS = 5


class Claim(NamedTuple):
    """Something a front door can promise, and how a front door writes it."""

    #: What to call it in the report.
    name: str
    #: How the strings spell it. Matched case-insensitively against each front door.
    written: str
    #: The words an item name may contain to back it, any one of which counts — matched against
    #: the identifier split into words, so `IceAgent` backs `ice` and `Service` does not. Empty
    #: for a codec, whose evidence is a module that encodes and decodes it rather than a name.
    #:
    #: More than one word, because a crate is entitled to name a capability after what it does
    #: rather than after the RFC: `sipx-call` provides RFC 4733 DTMF through `send_digits` and
    #: `recv_digit`, and a rule that only accepted `dtmf` would have called that an over-claim.
    #: The synonyms are the capability's other true names, never a way past the rule — nothing in
    #: `sipx-call` is called `couple` either, so bridging still has nothing behind it there.
    backing: tuple[str, ...] = ()


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

#: The capabilities the front page's own "Can it do what I need?" table sells — which is the
#: vocabulary this check exists for, since that table is where `X-35` found Opus, bridging and a
#: DTLS-SRTP workaround being advertised past the code. Deliberately not here: `dialogs`,
#: `transactions`, `parser`, `registration`. Those are how sipx is built rather than a capability
#: a reader shops for, they cannot be present or absent independently of the crate that has them,
#: and a vocabulary that included them would compare architecture instead of promises.
CAPABILITIES = (
    Claim("resampling", r"resampl\w*", ("resample",)),
    Claim("RFC 4733 DTMF", r"\bDTMF\b|RFC ?4733", ("dtmf", "digit")),
    Claim("WAV", r"\bWAV\b", ("wav",)),
    Claim("mixing", r"\bmix\w*", ("mix",)),
    Claim("bridging", r"\bbridg\w*", ("bridge",)),
    Claim("conferencing", r"\bconferenc\w*", ("conference",)),
    Claim("transfer", r"\btransfer\w*", ("transfer", "refer")),
    Claim("playback", r"\bplay\w*", ("play",)),
    Claim("recording", r"\brecord\w*", ("record",)),
    Claim("hold and resume", r"\bhold and resume\b", ("hold",)),
    Claim("SRTP", r"\bSRTP\b", ("srtp",)),
    Claim("DTLS-SRTP", r"\bDTLS\b", ("dtls",)),
    Claim("ICE", r"\bICE\b", ("ice",)),
    Claim("TLS", r"\bTLS\b", ("tls",)),
    Claim("WebSocket", r"\bWebSockets?\b|\bWSS?\b", ("ws", "websocket")),
    Claim("jitter buffer", r"\bjitter\b", ("jitter",)),
    Claim("quality statistics", r"\bMOS\b|quality statistics?", ("quality", "mos")),
    Claim("session timers", r"session timers?|RFC ?4028", ("timer",)),
    Claim("Outbound", r"\bOutbound\b|RFC ?5626", ("outbound", "flow")),
    Claim("GRUU", r"\bGRUU\b", ("gruu",)),
    Claim("push", r"\bpush\b", ("push",)),
)

VOCABULARY = CODECS + CAPABILITIES

#: A module declaration, with the feature that gates it if there is one. The visibility is
#: optional: a library writes `pub mod`, and a binary — where there is no public API to write —
#: writes bare `mod`, so requiring `pub` here read `sipx-cli` as a crate with no code in it.
_MODULE = re.compile(
    r'(?:#\[cfg\(feature = "(?P<feature>[\w-]+)"\)\]\s*\n\s*)?'
    r"(?:pub(?:\([\w:]+\))? )?mod (?P<name>\w+);"
)
#: An item's declaration keywords, in the order Rust writes them after the visibility.
_DECLARES = r"(?:(?:async|const|unsafe|extern)\s+)*(?:fn|struct|enum|trait|const|type|union)"

#: A public item. Anchored at the start of a line — after indentation and before anything else —
#: so a doc comment quoting `pub fn play` is prose and not an item. `async` and `const` sit
#: between `pub` and the keyword, and an earlier version of this pattern did not allow them, so
#: `pub async fn play` backed nothing and a crate could advertise playback with the whole feature
#: written in `async fn`s.
_PUBLIC_ITEM = re.compile(rf"(?m)^[ \t]*pub {_DECLARES} (?P<name>\w+)")

#: The same, with the visibility optional. A binary crate has no public API — everything in
#: `sipx-cli` is `pub(crate)` or private — so what backs its front doors is its own items. Reading
#: only `pub` there would find nothing and fail every claim the binary makes. The line anchor is
#: what makes this safe: without it, the words "the type name" in a sentence are an item called
#: `name`, and the crate's vocabulary fills up with English.
_ANY_ITEM = re.compile(rf"(?m)^[ \t]*(?:pub(?:\([\w:]+\))? )?{_DECLARES} (?P<name>\w+)")
_WORD = re.compile(r"[A-Z]+(?![a-z])|[A-Z][a-z0-9]*|[a-z0-9]+")

#: A table cell that is nothing but a backticked crate name, which is how both tables name the
#: crate a row is about.
_CRATE_CELL = re.compile(r"`sipx(?:-[a-z0-9]+)*`")


def words(identifier: str) -> set[str]:
    """An identifier split into its words, lower-cased.

    `IceAgent` and `gather_ice` both contain the word `ice`; `Service` and `choice` do not. A
    plain substring test cannot tell those apart, and a capability backed by `Service` would be
    backed by nothing.
    """
    return {word.lower() for word in _WORD.findall(identifier)}


class Module(NamedTuple):
    """One module of a crate, reduced to what can back a claim."""

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
    """One string a crate advertises itself with, and where to point when it is wrong."""

    crate: str
    where: str
    text: str


class Crate(NamedTuple):
    """A published crate, reduced to what it says about itself and what backs it."""

    name: str
    #: Its four front doors, in the order the docstring lists them.
    doors: tuple[FrontDoor, ...]
    #: Its modules, each with its feature gate, its header and its public items.
    modules: tuple[Module, ...]
    #: Every word of every public item name anywhere in the crate.
    vocabulary: frozenset[str]


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


def published() -> list[str]:
    """Every crate in the workspace that `cargo publish` would upload.

    Derived from the manifests rather than listed here, so a crate added to the workspace joins
    this check by existing instead of by somebody remembering.
    """
    found = []
    for directory in sorted(CRATES.iterdir()):
        manifest = directory / "Cargo.toml"
        if not manifest.is_dir() and manifest.exists():
            package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
            if package.get("publish", True) is not False:
                found.append(directory.name)
    if not found:
        raise ValueError(f"read no publishable crates under {CRATES.relative_to(ROOT)}")
    return found


def entry_point(crate: str) -> Path:
    """A crate's `lib.rs`, or its `main.rs` when it is only a binary."""
    for name in ("lib.rs", "main.rs"):
        candidate = CRATES / crate / "src" / name
        if candidate.exists():
            return candidate
    raise ValueError(f"crates/{crate}/src has neither a lib.rs nor a main.rs to read")


def item_pattern(entry: Path) -> re.Pattern[str]:
    """How to read items out of this crate: public ones for a library, all of them for a binary."""
    return _PUBLIC_ITEM if entry.name == "lib.rs" else _ANY_ITEM


def code(text: str) -> str:
    """A source file with its test module cut off.

    This project names its tests as whole sentences —
    `an_unmeasurable_round_trip_is_absent_rather_than_zero` — so a reader that counted test
    functions as items would find `record`, `quality` and most of English behind every crate. A
    capability backed by the name of a test is not implemented, which is the one thing this whole
    check exists to say.
    """
    return text.partition("#[cfg(test)]")[0]


def modules(crate: str, entry: Path) -> list[Module]:
    """The crate's modules, each with its feature gate, its header and its items.

    Walked from the entry point in declaration order, depth first, so a module inherits the
    feature that gates the `pub mod` line reaching it. A module declared but absent is an error
    rather than a silent nothing: it is the shape a reader takes when the file has moved under it,
    and a reader that finds nothing backs nothing.
    """
    source_root = CRATES / crate / "src"
    pattern = item_pattern(entry)
    found: list[Module] = []
    seen: set[Path] = set()

    def walk(path: Path, prefix: str, inherited: str) -> None:
        seen.add(path)
        text = path.read_text(encoding="utf-8")
        if prefix:
            found.append(
                Module(
                    name=prefix,
                    feature=inherited,
                    header=header(text),
                    items=tuple(item.group("name") for item in pattern.finditer(code(text))),
                )
            )
        for match in _MODULE.finditer(code(text)):
            name = match.group("name")
            gate = match.group("feature") or inherited
            flat = path.parent / f"{name}.rs"
            nested = path.parent / name / "mod.rs"
            child = flat if flat.exists() else nested
            if not child.exists():
                raise ValueError(
                    f"{path.relative_to(ROOT)} declares `pub mod {name}` and neither "
                    f"{flat.relative_to(source_root)} nor {nested.relative_to(source_root)} "
                    f"is there"
                )
            if child not in seen:
                walk(child, f"{prefix}::{name}" if prefix else name, gate)

    walk(entry, "", "")
    return found


def crate_vocabulary(entry: Path, found: list[Module]) -> frozenset[str]:
    """Every word of every item name in the crate, the entry point and the module names included.

    A module is a thing the crate is called after: `sipx-media` has a `bridge` module and
    `sipx-call` has none, which is the difference the backing rule is looking for. Excluding
    module names would have made `sipx-cli` — a binary whose commands *are* its modules — unable
    to back anything it says about itself.
    """
    pattern = item_pattern(entry)
    entry_code = code(entry.read_text(encoding="utf-8"))
    names = [item.group("name") for item in pattern.finditer(entry_code)]
    names += [item for module in found for item in module.items]
    names += [part for module in found for part in module.name.split("::")]
    if len(names) < _PLAUSIBLE_ITEMS:
        raise ValueError(
            f"read only {len(names)} items from {entry.parent.relative_to(ROOT)}; the reader has "
            f"drifted from the crate's shape and would back no claim it makes"
        )
    return frozenset(word for name in names for word in words(name))


def table(path: Path, heading: str) -> dict[str, str]:
    """The crate table under a heading, as crate name to the prose describing it.

    The cell that is exactly a backticked crate name is the key whichever column it sits in —
    `README.md` names the crate first and the guide names it second, and a reader that assumed
    one would compare a capability list against a crate name.

    A row is keyed on the *shape* of a crate name and not on the set of crates that publish. Had
    it filtered by that set, a row for a crate that does not publish would be skipped rather than
    reported, and the membership rule below could never fire in the direction `X-35` found it
    wrong in — `README.md` listing `sipx-testkit`, which is `publish = false`.
    """
    lines = path.read_text(encoding="utf-8").splitlines()
    try:
        start = lines.index(heading)
    except ValueError as absent:
        raise ValueError(
            f"{path.relative_to(ROOT)} has no `{heading}` heading, so its crate table cannot be "
            f"found; a table this check cannot find is a table that can promise anything"
        ) from absent

    rows: dict[str, str] = {}
    for line in lines[start + 1 :]:
        if line.startswith("#"):
            break
        if not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        named = [cell for cell in cells if _CRATE_CELL.fullmatch(cell)]
        if len(named) != 1:
            continue
        crate = named[0].strip("`")
        prose = " ".join(cell for cell in cells if cell != named[0])
        if crate in rows:
            raise ValueError(
                f"{path.relative_to(ROOT)} names `{crate}` in two rows under `{heading}`; one "
                f"crate, one row, or the check compares a crate against half of itself"
            )
        rows[crate] = prose
    return rows


def membership_problems(tables: dict[Path, dict[str, str]], crates: list[str]) -> list[str]:
    """Both crate tables must name exactly the crates that publish.

    A published crate the tables omit is a crate no sentence in the repository describes, so
    nothing here can be wrong about it and nothing here can be right. A row for a crate that does
    not publish points a reader at a dependency they cannot take.
    """
    problems = []
    for path, rows in tables.items():
        where = path.relative_to(ROOT)
        missing = sorted(set(crates) - set(rows))
        extra = sorted(set(rows) - set(crates))
        if missing:
            problems.append(
                f"{where}'s crate table omits {', '.join(missing)}, which publish; a crate no "
                f"table describes has no front door to check"
            )
        if extra:
            problems.append(
                f"{where}'s crate table names {', '.join(extra)}, which do not publish; a reader "
                f"cannot depend on them"
            )
    return problems


def front_doors(crate: str, tables: dict[Path, dict[str, str]]) -> list[FrontDoor]:
    """Every string in the repository that tells a reader what this crate is."""
    manifest = CRATES / crate / "Cargo.toml"
    description = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"].get(
        "description", ""
    )
    if not description:
        raise ValueError(f"{manifest.relative_to(ROOT)} has no package description")

    entry = entry_point(crate)
    lead = summary(entry.read_text(encoding="utf-8"))
    if not lead:
        raise ValueError(f"{entry.relative_to(ROOT)} opens with no `//!` summary")

    doors = [
        FrontDoor(crate, f"{manifest.relative_to(ROOT)} description", description),
        FrontDoor(crate, f"{entry.relative_to(ROOT)} summary", lead),
    ]
    for path, rows in tables.items():
        doors.append(
            FrontDoor(crate, f"{path.relative_to(ROOT)} crate table", rows.get(crate, ""))
        )
    return doors


def read(crate: str, tables: dict[Path, dict[str, str]]) -> Crate:
    entry = entry_point(crate)
    found = modules(crate, entry)
    return Crate(
        name=crate,
        doors=tuple(front_doors(crate, tables)),
        modules=tuple(found),
        vocabulary=crate_vocabulary(entry, found),
    )


def claimed(door: FrontDoor, vocabulary: tuple[Claim, ...]) -> list[Claim]:
    return [claim for claim in vocabulary if re.search(claim.written, door.text, re.I)]


def implements(claim: Claim, found: tuple[Module, ...]) -> Module | None:
    """The module that backs a codec claim: it names the codec and goes both ways.

    An ungated module wins over a gated one when both name the codec, because the feature warning
    below asks "is this codec off by default", and it is not if anything unconditional implements
    it. `opus.rs` names G.711 while explaining what Opus is for, and picking the first match would
    have reported G.711 as optional.
    """
    written = re.compile(claim.written, re.I)
    backing = [
        module
        for module in found
        if written.search(module.header) and module.provides("encode") and module.provides("decode")
    ]
    return min(backing, key=lambda module: bool(module.feature), default=None)


def names_the_feature(text: str, feature: str) -> bool:
    """Whether a description says a codec is optional.

    The feature name is matched case-sensitively and the word "feature" is required with it:
    `opus` the feature and `Opus` the codec differ by one letter, and a rule satisfied by the
    codec's own name would be no rule at all.
    """
    return feature in text and re.search(r"\bfeatures?\b", text, re.I) is not None


def claim_problems(crate: Crate) -> list[str]:
    """Every promise in a front door of this crate that the crate cannot keep."""
    problems: list[str] = []
    for door in crate.doors:
        if crate.name == CODEC_CRATE:
            for claim in claimed(door, CODECS):
                module = implements(claim, crate.modules)
                if module is None:
                    problems.append(
                        f"{door.where} names {claim.name} and no module of {crate.name} both "
                        f"encodes and decodes it; implement it or stop advertising it"
                    )
                    continue
                if module.feature and not names_the_feature(door.text, module.feature):
                    problems.append(
                        f"{door.where} names {claim.name}, which is behind the "
                        f"`{module.feature}` feature and off by default; say so, or a reader "
                        f"takes it for granted"
                    )
        for claim in claimed(door, CAPABILITIES):
            if not crate.vocabulary & set(claim.backing):
                problems.append(
                    f"{door.where} names {claim.name} and nothing in {crate.name} is called "
                    f"{' or '.join(f'`{word}`' for word in claim.backing)}; implement it or stop "
                    f"advertising it"
                )
    return problems


def agreement_problems(crate: Crate) -> list[str]:
    """No door may out-promise the manifest, and the two tables must promise the same thing.

    See the module docstring for why this is containment against the description rather than
    equality across all four doors.
    """
    canonical, *restatements = crate.doors
    promised = {claim.name for claim in claimed(canonical, VOCABULARY)}
    problems = []
    for door in restatements:
        beyond = sorted({claim.name for claim in claimed(door, VOCABULARY)} - promised)
        if beyond:
            problems.append(
                f"{door.where} claims {', '.join(beyond)} and {canonical.where} does not; a "
                f"restatement may say less than the crate's own listing and not more"
            )

    tables = [door for door in restatements if door.where.endswith("crate table")]
    if len(tables) != 2:
        raise ValueError(
            f"{crate.name} has {len(tables)} crate-table front doors and needs exactly two; "
            f"without both, one table can promise what the other denies"
        )
    first, second = tables
    sets = [{claim.name for claim in claimed(door, VOCABULARY)} for door in tables]
    if sets[0] != sets[1]:
        problems.append(
            f"{first.where} claims {sorted(sets[0]) or 'nothing'} for {crate.name} and "
            f"{second.where} claims {sorted(sets[1]) or 'nothing'}; one crate, one answer"
        )
    return problems


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] != "--check":
        print("usage: check-audio-claims.py --check", file=sys.stderr)
        return 2

    crates = published()
    tables = {path: table(path, heading) for path, heading in (README_TABLE, GUIDE_TABLE)}
    problems = membership_problems(tables, crates)
    if problems:
        print("the crate tables do not name the crates that publish:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    read_crates = [read(name, tables) for name in crates]
    for crate in read_crates:
        problems += claim_problems(crate) + agreement_problems(crate)
    if problems:
        print("the crate front doors advertise what the crates do not implement:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    doors = sum(len(crate.doors) for crate in read_crates)
    codecs = sorted(
        {
            claim.name
            for crate in read_crates
            if crate.name == CODEC_CRATE
            for door in crate.doors
            for claim in claimed(door, CODECS)
        }
    )
    print(
        f"{len(read_crates)} published crates, {doors} front doors, {len(codecs)} codecs claimed "
        f"({', '.join(codecs) or 'none'}), every claim backed and every door agreeing"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
