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
longer knows about one crate: every published crate has **five front doors**, and the check reads
all five for every one of them —

    the `description` in its manifest, which is the registry listing;
    the summary paragraph of its `lib.rs` or `main.rs`, which is the front page of the API
    reference;
    the summary paragraph of its package `README`, which is the crates.io landing page;
    its row in `README.md`'s crate table, which is what a reader sees first;
    its row in the website's "Which crate" table —

and the set of crates those two tables name must be exactly the set of crates that publish. A
crate the tables omit is a crate nobody's sentence describes; a table row for a crate that does
not publish points at something a reader cannot depend on.

Three rules run over the doors.

**Membership.** The set of crates the two tables name is the set of crates that publish. Nothing
else is a row and nothing published is missing.

**Restatement.** The manifest description is the crate's canonical sentence about itself — the
string the registry shows. The other four doors restate it: for a reader who wants the short
version, for a reader who opened the API reference, for a reader choosing a crate. A restatement
may say *less* than the description, because compressing is what it is for; it may not claim a
capability the description does not, because then the crate's own listing is not the authority on
the crate. And the two tables, being the same kind of string written for the same reader, must
claim the same set as each other. That pair of rules is what would have caught the DTMF row:
`README.md` named RFC 4733 DTMF, the manifest did not, and neither did the website's table.

Set *equality* across all five doors would be the wrong instrument, and is not what runs here. A
crate summary is written at a different altitude — `Sans-IO SIP core.` is a good first line and a
bad capability list — and a rule that demanded it enumerate every capability would turn every
crate's front page into keywords. Containment is the honest version of "one crate, one answer".

**Backing.** A capability named in any door must be backed by an item of that crate whose name
contains the capability's word. `sipx-media` may claim bridging because it has `Bridge`;
`sipx-call` may not, because it has nothing named bridge — which is `X-35`'s finding and `C-6`'s
gap.

A fourth rule runs over the API rather than over the prose, because `A-9` froze what a published
crate may add and an additive change that breaks a caller who did nothing wrong is the one thing a
published crate may not do quietly. Two shapes of item can do it.

**Extensibility.** A public enum either carries `#[non_exhaustive]` or argues beside itself why its
variants are the complete domain. Which enums that covers is decided by **reachability from the
crate root**: a `pub enum` is public API when its module path is writable from outside — every
`mod` on the way is a bare `pub` — or when the crate re-exports it out of a private module, which
is how `sipx-call` publishes `MediaProfile` from a private `media_policy`. A `pub enum` in a
private module that nothing re-exports is not public API at all, and reporting it would be noise
dressed as contract. `M-78` replaced the rule this succeeds, which selected by a name ending in
`Error` — a spelling convention standing in for a visibility question, so `MediaProfile`,
`IcePolicy` and `Keying` were unguarded for the life of the project while an internal
`ParseError` was held to the contract.

**Extensibility, for structs** (`M-80`). The same rule and the same reachability, over a public
struct that has **public fields**: adding a field breaks every downstream struct literal exactly as
adding a variant breaks every downstream exhaustive `match`. This check said for most of its life
that structs were out of scope "because a struct can add a private field without breaking a
caller". That is true of a struct whose fields are private and false of every struct on the media
surface: `M-75` added `Packet::extension`, `M-79` added `Encoded::extension`, and each additive
change broke in-tree literals — twice in two stories, which is a rate rather than a hypothetical.

Because the corrected selector turns up around two hundred reachable public-field structs at once,
the struct rule runs behind two boundaries and both are stated where they are defined:
`MEDIA_SURFACE` names the crates, and `breakable_structs` names the property — a struct the crate
*already publishes a `new` for*, which is the crate having said construction is the constructor's
job while leaving the literal legal. What is not yet held is counted and printed on every run, the
way the enum rollout's remainder is. See `struct_problems` for why `#[non_exhaustive]` is the side
of this decision that stays reversible, and so the one a pre-1.0 release should take.

Three things this deliberately does not do.

**It reads each crate documentation and package README's summary paragraph, not the whole file.**
The prose below the summary is where a crate qualifies scope — `X-26`'s record of why G.722 is
absent and `M-43`'s explicit resampling boundary live there — and a check that could not tell a claim from a
disclaimer would forbid writing the decision down, which is the opposite of the point. The summary
is what a reader is shown before choosing to read further, so it is the string that has to be true
on its own.

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

`GUARDED_SURFACE` is the one boundary in this file and it is deliberately not that. It names
crates and never an item, so no enum can be excused individually; correcting the extensibility
rule's selector turned up more than a hundred reachable enums at once, which is a breaking change
across eleven crates rather than a review anybody can do, and the run prints how many are still
outstanding beyond it. A suppression list makes a finding disappear. This one makes it a number
printed on every run.
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

#: A module declaration, with the feature that gates it if there is one and the visibility it
#: carries. The visibility is optional: a library writes `pub mod`, and a binary — where there is
#: no public API to write — writes bare `mod`, so requiring `pub` here read `sipx-cli` as a crate
#: with no code in it. `exported` captures a bare `pub` and `restriction` captures the parenthesised
#: form, because the two are opposites for reachability: `pub mod` widens the crate's surface and
#: `pub(crate) mod` is as private as no visibility at all.
_MODULE = re.compile(
    r'(?:#\[cfg\(feature = "(?P<feature>[\w-]+)"\)\]\s*\n\s*)?'
    r"(?:(?P<exported>pub) |pub\((?P<restriction>[\w: ]+)\) )?mod (?P<name>\w+);"
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

#: A public enum. Line anchored for the reason the item patterns above are: a doc comment quoting
#: `pub enum Foo` is prose.
_PUBLIC_ENUM = re.compile(r"(?m)^[ \t]*pub enum (?P<name>\w+)")

#: A public struct, read the same way and for the same question. This pattern's comment used to
#: say enums and never structs, "because a struct can add a private field without breaking a
#: caller" — which is true of a struct whose fields are private and false of one whose fields are
#: `pub`. `M-75` added `Packet::extension` and `M-79` added `Encoded::extension`, and each
#: additive change broke every struct literal that named the type. See `struct_problems`.
_PUBLIC_STRUCT = re.compile(r"(?m)^[ \t]*pub struct (?P<name>\w+)")

#: A public field of a braced struct, line anchored inside the struct's own body. `pub(crate)` and
#: `pub(super)` are deliberately not matched: a caller outside the crate cannot name those fields,
#: so it cannot write the literal that a new field would break.
_PUBLIC_FIELD = re.compile(r"(?m)^[ \t]*pub +\w+ *:")

#: A public field of a tuple struct, where there is no name to anchor on.
_PUBLIC_TUPLE_FIELD = re.compile(r"(?<![\w:])pub(?![\w(])")

#: An inherent `impl` block for a named type, up to its opening brace. Trait implementations are
#: excluded by the pattern itself: `impl Default for Packet` puts the trait name where this
#: expects the type, so it does not match.
_INHERENT_IMPL = r"(?m)^impl(?:<[^>]*>)? {name}\b(?![\w:])[^{{]*\{{"

#: A published constructor. `const fn new` counts; a `pub fn new` returning something else does
#: not need distinguishing, because a type that publishes `new` at all has named the way it is
#: meant to be built.
_CONSTRUCTOR = re.compile(r"(?m)^[ \t]*pub (?:const )?fn new\b")

#: A re-export. The body runs to the semicolon and may span lines, because that is how a crate
#: root writes a long one. What it re-exports is read out of the body by `reexports` below.
_REEXPORT = re.compile(r"(?m)^[ \t]*pub use\s+(?P<body>[^;]+);")

#: The one phrase that classifies an intentionally exhaustive enum. Like the fixed-sleep guard's
#: classifications, the reason lives at the site it excuses rather than in a list here.
EXHAUSTIVE_REASON = "/// Exhaustive by design:"

#: The same, for a struct whose public fields are deliberately the whole record. A separate phrase
#: rather than the enum's, because they answer different questions — "these variants are the
#: domain" and "these fields are the record" — and a reader who writes one at the other's type has
#: not made the argument the rule asked for.
COMPLETE_REASON = "/// Complete by design:"

#: The crates whose reachable public enums are held to the guard today.
#:
#: This is a **rollout boundary and not a suppression list**, and the difference is mechanical
#: rather than a promise. It names crates and never enums, so nothing inside a crate that is in
#: scope can be excused one item at a time — which is the shape a suppression list takes and the
#: shape this check has always refused. Widening it is a reviewable diff, and until it is widened
#: the run prints how many reachable enums outside it are still unguarded, so the debt is reported
#: at every gate run rather than kept somewhere nobody reads.
#:
#: Why a boundary exists at all: `M-78` replaced a rule that keyed on a name ending in `Error`,
#: and correcting the selector turns up well over a hundred reachable enums across the workspace.
#: Marking those is a breaking change for every downstream `match` arm in eleven crates at once,
#: which is not one reviewable change; `M-74` paid down the media path, which is the surface the
#: argument was made for. The remainder is recorded in `M-78`'s progress note.
GUARDED_SURFACE = ("sipx-audio", "sipx-call", "sipx-media", "sipx-rtp", "sipx-sdp")

#: The crates whose reachable public-field structs are held to the guard today (`M-80`).
#:
#: A second boundary rather than a second entry in the one above, because the struct rule is a
#: release behind the enum rule and saying so in a diff is cheaper than a comment claiming both are
#: at the same place. The struct rule reaches the two crates the relay path runs through, which is
#: where both breakages happened and where the argument for the rule was made.
#:
#: The same mechanical property as `GUARDED_SURFACE`: it names crates and never a struct, so no
#: type inside a crate in scope can be excused one at a time, and the run prints how many reachable
#: public-field structs the rule does not hold. That number is large — see `struct_problems` for
#: why the rule is additionally narrowed by a property of the type rather than by widening this.
MEDIA_SURFACE = ("sipx-media", "sipx-rtp")

#: `sipx-app-protocol` owns a closed, versioned application vocabulary and documents its own
#: exceptions, so `A-9` explicitly leaves it out — a decision that outlives any particular
#: rollout boundary and so is written down separately from one.
CLOSED_VOCABULARY = "sipx-app-protocol"


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
    #: Its five front doors, in the order the docstring lists them.
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


def markdown_summary(text: str) -> str:
    """The first prose paragraph after a Markdown document's H1.

    A package README needs room below its lead to state deliberate absences. Reading that whole
    file as a positive claim would turn "does not implement G.722" back into an advertisement for
    G.722, the false-positive direction this guard has always refused.
    """
    lines = text.splitlines()
    try:
        heading = next(index for index, line in enumerate(lines) if line.startswith("# "))
    except StopIteration:
        return ""
    paragraph = []
    for line in lines[heading + 1 :]:
        stripped = line.strip()
        if not stripped:
            if paragraph:
                break
            continue
        if stripped.startswith("#"):
            break
        paragraph.append(stripped)
    return " ".join(paragraph)


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


def package_readme(crate: str) -> Path | None:
    """The README Cargo assigns to a package, including its conventional inference.

    Cargo infers a file named `README.md`, `README.txt` or `README` beside the manifest when the
    package does not set `readme`. Keeping that rule here means ordinary `README.md` files do not
    need eleven redundant manifest keys, while an explicitly named file is still followed.
    """
    directory = CRATES / crate
    manifest = directory / "Cargo.toml"
    package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
    configured = package.get("readme")
    if configured is False:
        return None
    if isinstance(configured, str):
        return directory / configured
    for name in ("README.md", "README.txt", "README"):
        candidate = directory / name
        if candidate.exists():
            return candidate
    return None


def readme_problems(crates: list[str]) -> list[str]:
    """Published packages whose crates.io landing page has no file to ship."""
    problems = []
    for crate in crates:
        readme = package_readme(crate)
        if readme is None:
            problems.append(
                f"crates/{crate}/Cargo.toml sets no package README and no conventional README "
                "sits beside it"
            )
        elif not readme.is_file():
            problems.append(
                f"crates/{crate}/Cargo.toml names {readme.relative_to(ROOT)}, which is not a file"
            )
    return problems


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


class Reach(NamedTuple):
    """One module of a crate, reduced to whether anybody outside the crate can write its path."""

    #: Its path from the crate root — `ice::agent` — or empty for the root itself.
    module: str
    #: The file it is written in.
    path: Path
    #: Whether every `mod` on the way here carries a bare `pub`. `pub(crate) mod` and a bare `mod`
    #: are the same thing to a downstream crate: it cannot name what is inside either.
    exported: bool


def module_graph(crate: str, entry: Path) -> dict[str, Reach]:
    """Every module of a crate, keyed by its path, with whether it is exported.

    The same declarations `modules` walks, read for a different question: `modules` collects what
    can back a claim, this collects what a `match` in somebody else's crate can see. A module
    declared and absent is skipped here rather than raised on — `modules` walks the same lines and
    reports it, and one missing file described by two readers in two sentences is worse than one.
    """
    found: dict[str, Reach] = {}

    def walk(path: Path, module: str, exported: bool) -> None:
        found[module] = Reach(module=module, path=path, exported=exported)
        for match in _MODULE.finditer(code(path.read_text(encoding="utf-8"))):
            name = match.group("name")
            child_module = f"{module}::{name}" if module else name
            if child_module in found:
                continue
            flat = path.parent / f"{name}.rs"
            nested = path.parent / name / "mod.rs"
            child = flat if flat.exists() else nested
            if child.exists():
                walk(child, child_module, exported and match.group("exported") is not None)

    walk(entry, "", True)
    return found


def use_targets(module: str, prefix: str) -> list[str]:
    """The module paths a `use` prefix could name, read from inside `module`.

    `crate::`, `self::` and `super::` say where they start from and resolve to one answer. A bare
    first segment does not: under Rust 2018's uniform paths it is a sibling module, an item of the
    crate root, or another crate entirely. The third names no module of this crate and falls away
    on its own, so offering the first two costs nothing and misses neither.
    """
    segments = [segment for segment in prefix.split("::") if segment]
    anchored = False
    while segments and segments[0] in ("crate", "self", "super"):
        head = segments.pop(0)
        anchored = True
        if head == "crate":
            module = ""
        elif head == "super":
            module = module.rpartition("::")[0]
    if anchored:
        return ["::".join(filter(None, (module, *segments)))]
    return ["::".join(filter(None, (module, *segments))), "::".join(segments)]


def _leaves(items: str) -> list[str]:
    """The names inside a `use` brace list, split on the commas that are not nested."""
    leaves: list[str] = []
    depth = 0
    current = ""
    for character in items:
        if character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
        if character == "," and depth == 0:
            leaves.append(current)
            current = ""
        else:
            current += character
    leaves.append(current)
    return [leaf.strip() for leaf in leaves if leaf.strip()]


def reexports(module: str, text: str) -> tuple[set[tuple[str, str]], set[str]]:
    """What a module's `pub use` lines publish, as `(module, name)` pairs and glob targets.

    A name is recorded under the module it is re-exported *from*, because that is where the type
    is declared and where the guard has to look. `as` renames are read by their original name for
    the same reason: the rename changes what a caller writes, not which type grows a variant.
    """
    named: set[tuple[str, str]] = set()
    globs: set[str] = set()
    for found in _REEXPORT.finditer(text):
        body = " ".join(found.group("body").split())
        prefix, brace, rest = body.partition("{")
        leaves = _leaves(rest.rpartition("}")[0]) if brace else [body.rpartition("::")[2]]
        if not brace:
            prefix = body.rpartition("::")[0]
        for target in use_targets(module, prefix):
            for leaf in leaves:
                leaf = leaf.partition(" as ")[0].strip()
                if leaf == "*":
                    globs.add(target)
                elif leaf.isidentifier():
                    named.add((target, leaf))
    return named, globs


def reachable(crate: str, pattern: re.Pattern[str]) -> list[tuple[Path, str, str, int]]:
    """Every item of a crate matching `pattern` that a downstream crate can name.

    Two ways to be nameable, and the second is why a name-based or a module-visibility-only rule
    both get this wrong. A `pub` item in a module reached only through `pub mod` declarations is
    public because its path is writable. A `pub` item in a *private* module is public when the
    crate re-exports it — `sipx-call` keeps `MediaProfile` in a private `media_policy` and
    publishes it from the crate root, and a reader that stopped at module visibility would call
    the type it publishes an implementation detail.

    The converse is the half `M-78` was filed for: a `pub` item in a private module that nothing
    re-exports is not public API, however `pub` it is written, and marking it would be noise
    dressed as contract.

    The walk is the same question for an enum and for a struct — "can somebody outside write this
    type's name" — so `M-80` made the item pattern the parameter rather than copying the walk.

    A binary crate returns nothing: it has no public API for anybody to name.
    """
    entry = entry_point(crate)
    if entry.name != "lib.rs":
        return []
    graph = module_graph(crate, entry)
    sources = {
        module: code(reach.path.read_text(encoding="utf-8")) for module, reach in graph.items()
    }

    exported = {module for module, reach in graph.items() if reach.exported}
    published_names: set[tuple[str, str]] = set()
    modules_to_read = list(exported)
    names_to_chase: list[tuple[str, str]] = []

    def publish(target: str, leaf: str) -> None:
        whole = f"{target}::{leaf}" if target else leaf
        if whole in graph:
            # The re-export names a module rather than an item, which publishes all of it.
            if whole not in exported:
                exported.add(whole)
                modules_to_read.append(whole)
        elif (target, leaf) not in published_names:
            published_names.add((target, leaf))
            names_to_chase.append((target, leaf))

    # Both worklists only ever add to sets, so this settles. Names are chased as well as modules
    # because a private module often just forwards: `sipx-sip` publishes its transaction types out
    # of `transaction/mod.rs`, which re-exports them from a private `transaction::client`, and
    # recording only the first hop would leave the declaration site unguarded while the name is on
    # the crate's surface.
    while modules_to_read or names_to_chase:
        while modules_to_read:
            module = modules_to_read.pop()
            named, globs = reexports(module, sources[module])
            for target in globs:
                if target in graph and target not in exported:
                    exported.add(target)
                    modules_to_read.append(target)
            for target, leaf in named:
                if target in graph:
                    publish(target, leaf)
        while names_to_chase:
            module, leaf = names_to_chase.pop()
            for target, forwarded in reexports(module, sources[module])[0]:
                if forwarded == leaf and target in graph:
                    publish(target, leaf)

    found = []
    for module, reach in sorted(graph.items()):
        for match in pattern.finditer(sources[module]):
            name = match.group("name")
            if module in exported or (module, name) in published_names:
                found.append((reach.path, module, name, match.start()))
    return found


def reachable_enums(crate: str) -> list[tuple[Path, str, str, int]]:
    """Every public enum of a crate a downstream `match` can name."""
    return reachable(crate, _PUBLIC_ENUM)


def reachable_structs(crate: str) -> list[tuple[Path, str, str, int]]:
    """Every public struct of a crate a downstream literal can name."""
    return reachable(crate, _PUBLIC_STRUCT)


def preamble(source: str, offset: int) -> str:
    """What is written immediately above an item: its attributes and its doc comment.

    Bounded at the blank line above, so a rationale written over a different item does not classify
    this one.
    """
    return source[source.rfind("\n\n", 0, offset) + 2 : offset]


def declaration(source: str, offset: int) -> tuple[str, str]:
    """A struct's shape and its field list, read from the `pub struct` at `offset`.

    Three shapes and they break differently. A unit struct has no fields to add to. A tuple struct
    and a braced struct both do, and both spell a public field differently, so the shape has to
    come back with the body rather than be guessed from it.

    Angle brackets are skipped before the body opens, because a generic parameter list sits between
    the name and the body and a `where` clause can put a `{` nowhere else. Inside the body only the
    opening delimiter is counted: a struct body holds field types, and no field type contains an
    unbalanced brace.
    """
    index, angle, end = offset, 0, len(source)
    while index < end:
        character = source[index]
        if character == "<":
            angle += 1
        elif character == ">":
            angle -= 1
        elif angle == 0 and character in "{(;":
            break
        index += 1
    if index >= end or source[index] == ";":
        return ("unit", "")
    opener = source[index]
    shape, closer = ("braced", "}") if opener == "{" else ("tuple", ")")
    depth, scan = 0, index
    while scan < end:
        if source[scan] == opener:
            depth += 1
        elif source[scan] == closer:
            depth -= 1
            if depth == 0:
                return (shape, source[index + 1 : scan])
        scan += 1
    raise ValueError(f"a `pub struct` at offset {offset} has no closing `{closer}`")


def has_a_public_field(shape: str, body: str) -> bool:
    """Whether a caller in another crate can name a field of this struct, and so write a literal."""
    if shape == "unit":
        return False
    pattern = _PUBLIC_FIELD if shape == "braced" else _PUBLIC_TUPLE_FIELD
    return pattern.search(body) is not None


def publishes_a_constructor(source: str, name: str) -> bool:
    """Whether the crate offers a `pub fn new` for this type in an inherent `impl`."""
    for opening in re.finditer(_INHERENT_IMPL.format(name=re.escape(name)), source):
        depth, scan = 0, opening.end() - 1
        while scan < len(source):
            if source[scan] == "{":
                depth += 1
            elif source[scan] == "}":
                depth -= 1
                if depth == 0:
                    break
            scan += 1
        if _CONSTRUCTOR.search(source[opening.end() : scan]):
            return True
    return False


def breakable_structs(crate: str) -> list[tuple[Path, str, str, int]]:
    """Reachable public structs whose field set is part of the contract and already has a `new`.

    The rule this narrows is general and stated in `struct_problems`: any reachable struct with a
    public field breaks downstream literals when a field is added. Two narrowings stand between
    that rule and what runs, and both are boundaries rather than excuses.

    `MEDIA_SURFACE` is the crate boundary. This one is the type boundary, and it is a property
    rather than a list: a struct is held when the crate *already publishes a constructor for it*.
    That is not a proxy for importance. A crate that ships `T::new(..)` and leaves every field
    `pub` has published two ways to build one value and can only evolve one of them — which is
    exactly how `M-75` and `M-79` turned two additive changes into two breaking ones. The
    attribute is how the crate's existing intent becomes something a compiler checks.

    A public-field struct with no constructor is a different piece of work rather than the same
    work deferred: `#[non_exhaustive]` on one leaves a downstream caller with no way to build it at
    all, so the constructor has to be designed first. `M-92` carries those.

    The boundary also widens by itself in the right direction. The day a type here gains a `new`,
    the guard picks it up without anybody remembering to edit a list.
    """
    found = []
    for path, module, name, offset in reachable_structs(crate):
        source = code(path.read_text(encoding="utf-8"))
        shape, body = declaration(source, offset)
        if has_a_public_field(shape, body) and publishes_a_constructor(source, name):
            found.append((path, module, name, offset))
    return found


def struct_problems(crates: list[str]) -> list[str]:
    """Reachable public structs that promise their current field set can never grow.

    A public struct with a public field is as breakable by an additive change as a public enum is,
    and the two breakages this rule was filed for are the evidence: `M-75` added
    `Packet::extension` and `M-79` added `Encoded::extension`, and each broke every struct literal
    naming the type. The check treated structs as safe because a struct *can* add a private field
    without breaking a caller — true of a struct whose fields are private, and false of every
    struct on this surface.

    `#[non_exhaustive]` is the only side of this that stays available. Adding it after `1.0.0`
    freezes the API is itself a breaking change and so needs a major release; removing it never is.
    A type marked now can be unmarked in any later minor release, and a type left unmarked at v1 is
    decided for the life of the major version — which is why `M-80` settled it at `1.0.0-rc.10`
    rather than leaving it to the release that would have foreclosed it.

    The argument for an exception lives beside the type — never in a list here — so a reader who
    has to add a field meets it at the moment the question arises.
    """
    problems = []
    for crate in crates:
        for path, _module, name, offset in breakable_structs(crate):
            source = code(path.read_text(encoding="utf-8"))
            above = preamble(source, offset)
            if "#[non_exhaustive]" in above or COMPLETE_REASON in above:
                continue
            line = source.count("\n", 0, offset) + 1
            try:
                where = path.relative_to(ROOT)
            except ValueError:
                where = path
            problems.append(
                f"{where}:{line} `{name}` is reachable from the crate root, has public fields and "
                f"publishes a constructor; add `#[non_exhaustive]` or an adjacent "
                f"`{COMPLETE_REASON}` rationale"
            )
    return problems


def enum_problems(crates: list[str]) -> list[str]:
    """Reachable public enums that promise their current variant set can never grow.

    An enum is extensible unless the type itself argues why its variants are the complete domain.
    The argument lives beside the type — never in a list here — so a reader who has to add a
    variant meets it at the moment the question arises.
    """
    problems = []
    for crate in crates:
        for path, _module, name, offset in reachable_enums(crate):
            source = code(path.read_text(encoding="utf-8"))
            above = preamble(source, offset)
            if "#[non_exhaustive]" in above or EXHAUSTIVE_REASON in above:
                continue
            line = source.count("\n", 0, offset) + 1
            try:
                where = path.relative_to(ROOT)
            except ValueError:
                where = path
            problems.append(
                f"{where}:{line} `{name}` is reachable from the crate root and exhaustive; add "
                f"`#[non_exhaustive]` or an adjacent `{EXHAUSTIVE_REASON}` rationale"
            )
    return problems


def guarded(crates: list[str]) -> list[str]:
    """The crates whose reachable enums this run holds to the guard. See `GUARDED_SURFACE`."""
    return [crate for crate in crates if crate in GUARDED_SURFACE and crate != CLOSED_VOCABULARY]


def outside_the_boundary(crates: list[str]) -> list[str]:
    """The crates the rollout has not reached, whose debt the summary line reports."""
    return [
        crate
        for crate in crates
        if crate not in GUARDED_SURFACE and crate != CLOSED_VOCABULARY
    ]


def guarded_structs(crates: list[str]) -> list[str]:
    """The crates whose reachable structs this run holds to the guard. See `MEDIA_SURFACE`."""
    return [crate for crate in crates if crate in MEDIA_SURFACE and crate != CLOSED_VOCABULARY]


def outstanding_structs(crates: list[str]) -> list[str]:
    """Every reachable public-field struct in the workspace the struct rule does not hold yet.

    Counted over all published crates and not only the ones outside `MEDIA_SURFACE`, because the
    type boundary in `breakable_structs` leaves work inside the guarded crates too. A debt line
    that reported only the crates outside would have said the media surface was finished.
    """
    outstanding = []
    for crate in crates:
        if crate == CLOSED_VOCABULARY:
            continue
        # Keyed on where the type is written and not on its name: `sipx-media` declares a `Config`
        # in `session` and another in `ice::agent`, and a name-keyed set would have reported the
        # second one as held because the first is.
        held = {(module, offset) for _path, module, _name, offset in breakable_structs(crate)}
        for path, module, name, offset in reachable_structs(crate):
            source = code(path.read_text(encoding="utf-8"))
            shape, body = declaration(source, offset)
            if not has_a_public_field(shape, body) or (module, offset) in held:
                continue
            above = preamble(source, offset)
            if "#[non_exhaustive]" in above or COMPLETE_REASON in above:
                continue
            outstanding.append(f"{path}:{name}")
    return outstanding


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

    readme = package_readme(crate)
    if readme is None or not readme.is_file():
        raise ValueError(
            f"{manifest.relative_to(ROOT)} has no package README to use as its crates.io front door"
        )
    readme_lead = markdown_summary(readme.read_text(encoding="utf-8"))
    if not readme_lead:
        raise ValueError(f"{readme.relative_to(ROOT)} has no summary paragraph after its H1")

    doors = [
        FrontDoor(crate, f"{manifest.relative_to(ROOT)} description", description),
        FrontDoor(crate, f"{entry.relative_to(ROOT)} summary", lead),
        FrontDoor(crate, f"{readme.relative_to(ROOT)} summary", readme_lead),
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
    equality across all five doors.
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


#: The two words a crate may classify its surface with, and the heading they must sit under. Both
#: words, not one: "Supported" alone could be satisfied by a crate that never mentions the parts of
#: itself nothing can reach, which is the omission `A-8` was filed to close.
STABILITY_HEADING = "# Stability"
STABILITY_WORDS = ("Supported", "Experimental")


def stability_problems(crate: Crate) -> list[str]:
    """Whether a crate says what it guarantees, at all.

    `A-8`, and alpha predicate 5: `1.0.0` freezes what "stable" means, so the line between supported
    and experimental has to exist before it can be frozen. This checks only that the declaration is
    *present* — no script can judge whether the classification is honest, and pretending otherwise
    would be the same over-claim one level up.

    The reason it is worth checking mechanically anyway: `missing_docs` already guarantees every public
    item has *a* doc comment, so a crate can be fully documented and still never tell a reader whether
    any of it can be depended on. That was true of ten of the eleven published crates.
    """
    entry = entry_point(crate.name)
    text = entry.read_text()
    doc = "\n".join(line for line in text.splitlines() if line.strip().startswith("//!"))
    # Relative when it can be, so a message names `crates/sipx-sip/src/lib.rs` rather than an absolute
    # path; but a path outside the tree must not crash the checker.
    try:
        where = entry.relative_to(ROOT)
    except ValueError:
        where = entry
    problems = []
    if STABILITY_HEADING not in doc:
        problems.append(
            f"{where} has no `{STABILITY_HEADING}` section in its crate documentation; a reader "
            f"cannot tell whether any of {crate.name} may be depended on"
        )
        return problems
    if not any(word in doc for word in STABILITY_WORDS):
        problems.append(
            f"{where}'s `{STABILITY_HEADING}` section names neither "
            f"{' nor '.join(STABILITY_WORDS)}, so it classifies nothing"
        )
    return problems


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] != "--check":
        print("usage: check-audio-claims.py --check", file=sys.stderr)
        return 2

    crates = published()
    tables = {path: table(path, heading) for path, heading in (README_TABLE, GUIDE_TABLE)}
    problems = membership_problems(tables, crates) + readme_problems(crates)
    if problems:
        print("the published crates do not all have the front doors they require:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1

    read_crates = [read(name, tables) for name in crates]
    problems += enum_problems(guarded(crates)) + struct_problems(guarded_structs(crates))
    for crate in read_crates:
        problems += (
            claim_problems(crate) + agreement_problems(crate) + stability_problems(crate)
        )
    if problems:
        print(
            "the crate front doors advertise what the crates do not implement, or do not say what "
            "they guarantee:",
            file=sys.stderr,
        )
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
    # The rollout boundary's debt, counted on every run. See `GUARDED_SURFACE`: a boundary that
    # reported nothing would be a suppression list with a better name.
    debt = enum_problems(outside_the_boundary(crates))
    print(
        f"{len(GUARDED_SURFACE)} crates hold every reachable public enum non-exhaustive or argued; "
        f"{len(debt)} reachable enums outside that boundary are still exhaustive"
    )
    # The struct rule's debt, on the same terms and for the same reason (`M-80`).
    structs = outstanding_structs(crates)
    print(
        f"{len(MEDIA_SURFACE)} crates hold every reachable public struct that has public fields "
        f"and a constructor non-exhaustive or argued; {len(structs)} reachable public-field "
        f"structs elsewhere can still be broken by a new field"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
