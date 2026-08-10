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
the struct rule runs behind one boundary, stated where it is defined: `MEDIA_SURFACE` names the
crates. `M-80` had a second, a type boundary holding only a struct the crate already published a
`new` for; `M-92` removed it, because a struct with public fields and no constructor breaks a
downstream literal exactly as hard and the boundary excused twenty-nine of them. What is not yet
held is counted and printed on every run, the way the enum rollout's remainder is. See
`struct_problems` for why `#[non_exhaustive]` is the side of this decision that stays reversible,
and so the one a pre-1.0 release should take.

The hazard that type boundary named is real and survives as the rule's second obligation:
`#[non_exhaustive]` on a struct with no constructor leaves a caller no way to build one at all. A
type in that position states which it is — it publishes a constructor, or it says beside itself
that it is a value this crate hands out and nothing outside builds. Both are answers; being
outside the rule was not one.

**Diagnostics.** Three more rules, over what a public type's `Debug` *renders* rather than over what
its crate promises or what its shape breaks. `M-61` found a derived `Debug` printing 65,536 samples
of a call into a refusal record and `M-107` found the same derive on two more types, so a type
holding an `i16` or `f32` buffer now implements `Debug` or argues its samples are not somebody's
conversation — see `sample_buffer_problems`. `M-107`'s reader read a public type's *own* fields,
which `M-68` then found to be one hop short: `sipx_media::DspGraph` rendered the frame in flight
through four private types and this check passed the tree that held it. So the rule follows a
public type's fields into the crate's private ones, `_CARRIER_HOPS` deep, and cuts a chain at the
first type that writes its own rendering — see `carrier_chain`. `M-107` also concluded that the
*encoded* half could
not be checked, because a payload is `Bytes` and so is a `Call-ID`. `M-110` found the narrowing
that makes it checkable: not a different element type but a different **scope**. On the crates the
call's own bytes reach, every byte buffer is decidable and every one is decided — see
`byte_buffer_problems`, `RELAY_PATH` and `AUDIO_AT_REST`. The third rule is not audio's at all:
a fixed-size `[u8; N]` in this workspace is a key rather than a message, so `key_problems` holds
every one of them in every crate, after `M-110` found `sipx_ua::Authenticator` deriving a `Debug`
over the secret its nonces are `MACed` with.

Each rule has its own escape phrase, and the separations are load-bearing rather than tidy. "Not
call audio" is *true* of an RFC 3550 §6.5 `CNAME`, so excusing personal data truthfully is the one
thing the byte rule cannot afford; "not the call" is *true* of a signing key, which is the same
mistake one level up and why the key rule asks its own question.

Both extensibility rules read the attribute as an attribute and each rationale as the opening of a
doc line (`M-97`). `preamble` hands them a type's attributes and its documentation as one string,
and a substring test over that string let a sentence *about* `#[non_exhaustive]` stand in for
carrying it: `M-83` wrote three such sentences in one diff, and `sipx-media`'s `ProviderKind` had
passed on a sentence saying it is deliberately **not** marked since `M-74`. A form of the exception
nobody chose is the one thing a check built on two greppable forms cannot afford. See `marked`.

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

`GUARDED_SURFACE` was the one boundary in this file and it is gone (`M-83`). It named crates and
never an item, so no enum was ever excused individually; correcting the extensibility rule's
selector turned up more than a hundred reachable enums at once, which is a breaking change across
eleven crates rather than a review anybody can do, so `M-74` paid down the media path behind the
boundary and `M-83` paid down the rest. What decided the timing is that the choice is only
reversible in one direction: `#[non_exhaustive]` can be *removed* in a minor release and added only
in a major one, so a type left unmarked when `1.0.0` freezes the API is decided for the life of the
major version. The enum rule now runs over every published crate, and the one crate outside it is
outside for a stated reason rather than for a rollout's convenience — see `CLOSED_VOCABULARY`. What
that exclusion holds is still counted and printed on every run, because an exclusion that reported
nothing would be a suppression list with a better name.
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

#: The guard itself, read as an attribute rather than as a substring of the preamble: its own line,
#: at whatever indentation the item sits at. `preamble` hands the rules a type's attributes and its
#: doc comment as one string, so a substring test cannot tell `#[non_exhaustive]` from a sentence
#: naming it — and a type that argued its way past the rule in prose is a type nobody reviewed under
#: the rule. `M-97`; see `marked`.
_MARKED = re.compile(r"(?m)^[ \t]*#\[non_exhaustive\][ \t]*$")

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

#: A public associated function, up to its name. The parameter list is scanned rather than matched
#: (`constructs_self`), because a parameter type can hold parentheses of its own —
#: `impl Fn(RtcpQualitySample)` is one this workspace writes — and a pattern that stopped at the
#: first `)` would read half a signature and then guess at the return type.
_PUBLIC_FN = re.compile(r"(?m)^[ \t]*pub (?:const )?(?:async )?fn \w+")

#: A `self` receiver, in every spelling a method can open with. A function that takes one is not a
#: constructor: it needs a value of the type before it can be called at all.
_RECEIVER = re.compile(r"^\s*(?:&\s*(?:'\w+\s*)?)?(?:mut\s+)?self\b")

#: A derived `Default`, read from what is written immediately above the struct.
_DERIVES_DEFAULT = re.compile(r"#\[derive\([^)]*\bDefault\b")

#: A hand-written `Default`, for the types whose defaults are values rather than zeroes —
#: `ice::Timers` is the RFCs' recommended durations and cannot be derived.
_IMPLEMENTS_DEFAULT = r"(?m)^impl(?:<[^>]*>)?\s+Default\s+for\s+{name}\b"

#: A re-export. The body runs to the semicolon and may span lines, because that is how a crate
#: root writes a long one. What it re-exports is read out of the body by `reexports` below.
_REEXPORT = re.compile(r"(?m)^[ \t]*pub use\s+(?P<body>[^;]+);")

#: A file's test module, read as the attribute at the file's own top level. See `code`: at any
#: indentation this is a test-only item inside a real one, and cutting there loses the rest of the
#: file.
_TEST_MODULE = re.compile(r"(?m)^#\[cfg\(test\)\]")

#: Any struct or enum a crate declares, at whatever visibility, and the pattern the carrier chase
#: walks into (`M-121`). `_PUBLIC_STRUCT` and `_PUBLIC_ENUM` answer "can a caller outside write
#: this type's name"; this answers "is there a declaration here to follow a field into", which is a
#: different question with a different answer for precisely the types this rule exists to reach.
#: Every hop of `M-68`'s chain is one nothing outside `sipx-media` can name.
#:
#: The offset a caller wants is the **keyword's** and not the line's, which is why `kind` is a
#: group rather than an alternation. `type_body` scans forward from the offset for the first `{`,
#: `(` or `;`; handed the start of `pub(crate) struct SlotRef` it reads the parenthesis in the
#: visibility as a tuple body and returns `crate`, so half the workspace's private types looked
#: like a one-field tuple struct holding nothing.
_DECLARED_TYPE = re.compile(
    r"(?m)^[ \t]*(?:pub(?:\([\w:]+\))?[ \t]+)?(?P<kind>struct|enum)[ \t]+(?P<name>\w+)"
)

#: A generic constructor in a field's type, read as the name immediately before its `<`.
_CONSTRUCTOR = re.compile(r"(\w+)\s*<")

#: An identifier, for reading the type names out of a field's type expression.
_IDENTIFIER = re.compile(r"\b[A-Za-z_]\w*\b")

#: The four bracket pairs a Rust type expression nests with, for splitting a body into fields.
_OPENERS, _CLOSERS = "<([{", ">)]}"

#: The containers whose `Debug` renders the container and never what is inside it, so a buffer
#: behind one cannot reach a log through the type holding it (`M-121`).
#:
#: `sipx_media::MediaSession` holds an `mpsc::Sender<Frame>` and `Frame::Audio` holds the packet's
#: samples, but a channel's rendering is the channel: no chain runs through it, and a rule that
#: reported one would be reporting a leak that cannot happen. `Arc`, `Mutex`, `Option`, `Vec` and
#: `VecDeque` are the opposite — every one of them renders what it holds, which is how the four
#: hops between `DspGraph` and the frame in flight rendered raw audio.
#:
#: **A deny list and not an allow list, for the reason every other narrowing here is written the
#: way it is.** An allow list fails by *not* recognising a container: a wrapper nobody thought of
#: would end the chase silently, at exit 0, which is the one failure this file cannot afford. This
#: fails the other way — a container missing from it is followed, and what that costs is a report
#: at a type whose `Debug` was never going to print the buffer. That is a reviewer's minute and a
#: line in this set, against a leak nobody hears about.
OPAQUE_CONTAINERS = frozenset(
    {
        "Sender",
        "SyncSender",
        "UnboundedSender",
        "Receiver",
        "UnboundedReceiver",
        "JoinHandle",
        "Weak",
        "PhantomData",
    }
)

#: How many hops through a crate's private types the carrier chase follows before it stops.
#:
#: **The depth is the defect's and not the workspace's.** `M-68` found `sipx_media::DspGraph`
#: rendering the call's own audio at two distances: the frame in flight is four hops down —
#: `DspGraph → SlotRef → Slot → Live → Buffers` — and a supervised stage's pool of up to nine more
#: frames is six, through `Live → Stage → Running → Supervised`. Six is the deeper of the two, so
#: the whole of the one finding this rule was filed for is inside it rather than half of it.
#:
#: What the workspace says about that number is a check on it rather than the reason for it: the
#: chase's report saturates at four hops and its carrier population at six, and seven and eight
#: select nothing further. So six is chosen from the defect and confirmed to cover everything this
#: workspace currently writes.
#:
#: **What a carrier past it still costs a reviewer, stated rather than discovered.** A chain of
#: seven private hops from a public type to a PCM buffer is not reported, and there is nothing in
#: a red gate that would say so — the rule is silent about it exactly as `M-107`'s was silent about
#: `DspGraph`. What stands in for that is the count this run prints: a workspace that grows a
#: seventh hop grows its carrier population first, and the number moving is the prompt to re-read
#: this constant. A rule that narrows has to say where it stopped looking, and this is where.
_CARRIER_HOPS = 6

#: A raw buffer of linear PCM samples, in every shape a field can be written in: `Vec<i16>`, a
#: borrowed or boxed `[i16]`, and a fixed `[i16; N]`. `f32` joins `i16` because a float sample
#: buffer is the same audio at a different depth, and matching it now costs nothing.
#:
#: **The element type is the whole of what makes this checkable across every crate at once**
#: (`M-107`). A `Vec<i16>` is call audio and nothing else in this workspace; a `Vec<u8>` or a
#: `Bytes` is a G.711 payload *or* a `Call-ID` *or* a SIP body *or* a STUN attribute, and 58
#: reachable public types hold one — so widening this to `u8` would report the whole SIP surface,
#: where rendering the bytes is what a protocol log is *for*, and the check would be switched off
#: by whoever hit it second.
#:
#: What `M-107` concluded from that is that encoded audio is a reviewer's question. `M-110` found
#: the narrowing that was missing: not a different element type but a different **scope**. See
#: `RELAY_PATH` and `_BYTE_BUFFER` — the sixty are spread across eleven crates, and on the two the
#: call's own bytes pass through, every one of them can be decided.
_SAMPLE_BUFFER = re.compile(r"\bVec<\s*(?:i16|f32)\s*>|\[\s*(?:i16|f32)\s*[];]")

#: A hand-written `Debug`, at whatever indentation and in whatever spelling of the path the file
#: imported — `std::fmt::Debug`, `core::fmt::Debug` or a bare `fmt::Debug`.
_IMPLEMENTS_DEBUG = r"(?m)^[ \t]*impl(?:\s*<[^>]*>)?\s+(?:\w+::)*fmt::Debug\s+for\s+{name}\b"

#: The phrase that classifies a sample-typed buffer which is not call audio — a coefficient table,
#: a window, a fixture. A fourth phrase rather than a reuse of the extensibility rules', because it
#: answers a different question again: "these samples are not somebody's conversation".
NOT_AUDIO_REASON = "/// Not call audio:"

#: Below this the sample-buffer reader has stopped recognising a buffer rather than found a
#: workspace that holds none.
#:
#: This is `_PLAUSIBLE_ITEMS`'s argument for the other rule that reads Rust rather than prose, and
#: it is the answer to the one direction in which narrowing a *selector* is quiet. Over-narrowing
#: the exception side of `sample_buffer_problems` is loud by construction — a reader that stopped
#: recognising a hand-written `Debug` reports every carrier in the workspace. Over-narrowing
#: `_SAMPLE_BUFFER` is the opposite: it finds nothing, holds nothing, and passes. So the population
#: is counted and a run that recognises almost none of it fails instead.
#:
#: Twenty-eight public types across two crates hold or reach one today, which is `M-107`'s ten
#: plus the eighteen `M-121` widened the reader to reach. The floor stays at four rather than
#: following the number up: what it is set against is a *selector* that has stopped selecting, and
#: `_SAMPLE_BUFFER` going blind takes the count to zero however far the chase reaches. Four is low
#: enough that removing a crate's worth of carriers is not a red gate and high enough that a reader
#: which has gone blind is, and neither half of that sentence is about how many hops the chase now
#: follows.
_PLAUSIBLE_CARRIERS = 4

#: A buffer of raw octets, in every shape a field can be written in. The byte-buffer rule's
#: selector, and unlike `_SAMPLE_BUFFER` it says nothing at all about what the octets are — which
#: is why it runs only over `RELAY_PATH` and why it needs its own escape phrase.
_BYTE_BUFFER = re.compile(r"\bBytes(?:Mut)?\b|\bVec<\s*u8\s*>|\[\s*u8\s*[];]")

#: The phrase that classifies a byte buffer on the relay path as neither the call nor a
#: participant — a STUN attribute, an ICE datagram, a key.
#:
#: A fifth phrase rather than a reuse of `NOT_AUDIO_REASON`, and the difference is the whole of
#: `M-110`'s finding. "Not call audio" is *true* of an RFC 3550 §6.5 `SdesItem`: its bytes are a
#: `CNAME`, a `NAME` or an `EMAIL`, which is a person rather than a conversation. A rule whose
#: escape was the audio phrase would therefore have been answered truthfully by the one type in
#: this crate carrying personal data, and a redaction check that can be switched off by a true
#: sentence checks nothing. This phrase asks the question the rule actually has: **neither.**
NOT_THE_CALL_REASON = "/// Not the call:"

#: Below this the byte-buffer reader has stopped recognising a buffer rather than found a relay
#: path that carries none. `_PLAUSIBLE_CARRIERS`' argument, for the other selector that fails by
#: selecting nothing: eight reachable public types across the two crates hold one today, and four
#: is low enough that removing a crate's worth is not a red gate and high enough that a reader
#: which has gone blind is.
_PLAUSIBLE_RELAY_CARRIERS = 4

#: A byte buffer whose length is fixed at compile time — `[u8; 32]`, `[u8; TAG_LEN]`. The key
#: rule's selector, and the one selector in this file that needs no scope at all.
#:
#: A `Vec<u8>` is a message and a `Bytes` is a message; both are as long as whatever arrived, which
#: is why deciding them takes a crate list. A `[u8; N]` is the opposite shape: its length is a
#: constant the type chose, and in this workspace that means a key, a tag or a fingerprint — never
#: a `Call-ID` and never a body. So the population is small enough to hold everywhere and nothing
#: has to be excused for being a protocol header. See `key_problems`.
_KEY_ARRAY = re.compile(r"\[\s*u8\s*;")

#: The phrase that classifies a fixed-size byte array as something no reader could misuse — an
#: address, a magic number, a published fingerprint.
#:
#: A sixth phrase, and it exists for `NOT_THE_CALL_REASON`'s argument applied one level up. "Not the
#: call" is *true* of `Authenticator::secret`: the key is neither the conversation nor a
#: participant, it is the thing that makes every nonce forgeable. A rule whose escape were the
#: byte-buffer phrase would have been answered truthfully by the one type it exists for, which is
#: precisely the hole `M-110` refused to leave between the audio phrase and the call phrase.
NOT_A_SECRET_REASON = "/// Not a secret:"

#: Below this the key reader has stopped recognising a fixed-size byte array. See `unread_keys`.
#:
#: **The weakest floor in this file, and it is set to the population rather than below it.** The
#: other two sit under theirs so a crate's worth of carriers can leave without a red gate; this
#: population is one type in one crate, so there is no room between "a crate went" and "the reader
#: went blind" to put a threshold in. One is what it can be: it catches the realistic failure, which
#: is somebody editing `_KEY_ARRAY` and matching nothing, and it goes red if `sipx_ua::Authenticator`
#: stops keeping its secret in an array. That second case is a false accusation in its wording and
#: the right interrupt anyway — a workspace where this rule selects nothing is one where a green run
#: implies a coverage that does not exist, and somebody should retire the rule out loud rather than
#: let it pass quietly forever.
_PLAUSIBLE_KEY_CARRIERS = 1

#: The one phrase that classifies an intentionally exhaustive enum. Like the fixed-sleep guard's
#: classifications, the reason lives at the site it excuses rather than in a list here.
EXHAUSTIVE_REASON = "/// Exhaustive by design:"

#: The same, for a struct whose public fields are deliberately the whole record. A separate phrase
#: rather than the enum's, because they answer different questions — "these variants are the
#: domain" and "these fields are the record" — and a reader who writes one at the other's type has
#: not made the argument the rule asked for.
COMPLETE_REASON = "/// Complete by design:"

#: The phrase that answers the second obligation `M-92` added: a `#[non_exhaustive]` struct with no
#: constructor cannot be built from outside the crate at all, and a type in that position has to
#: say that it is meant to be read rather than built. A third phrase rather than a reuse of
#: `COMPLETE_REASON`, because the two are opposite claims — "these fields are the whole record" and
#: "this value comes from inside this crate" — and a type that made the wrong one would be arguing
#: for the attribute it does not carry.
UNBUILT_REASON = "/// Built by this crate only:"

#: The crates whose reachable public-field structs are held to the guard today (`M-80`).
#:
#: This is the last **rollout boundary** in this file, and the difference between one of those and
#: a suppression list is mechanical rather than a promise. It names crates and never a struct, so
#: no type inside a crate in scope can be excused one at a time — which is the shape a suppression
#: list takes and the shape this check has always refused. Widening it is a reviewable diff, and
#: until it is widened the run prints how many reachable public-field structs the rule does not
#: hold, so the debt is reported at every gate run rather than kept somewhere nobody reads.
#:
#: The struct rule reaches the two crates the relay path runs through, which is where both
#: breakages happened and where the argument for the rule was made. `M-83` retired the enum rule's
#: equivalent boundary by paying it down rather than by widening it in one step, which is the
#: sequence this one is expected to follow.
MEDIA_SURFACE = ("sipx-media", "sipx-rtp")

#: The crates the call's own bytes pass through **as bytes**, and the scope of the byte-buffer rule
#: (`M-110`). See `byte_buffer_problems`.
#:
#: It holds the same two crates as `MEDIA_SURFACE` and is deliberately a separate constant, because
#: the two mean opposite things about their own future. `MEDIA_SURFACE` is a **rollout boundary**:
#: it is expected to be widened, and `M-83` is the sequence it is meant to follow. This is a
#: **stated scope**, and widening it is a category error rather than progress — a `Bytes` in
#: `sipx-sip` is a `Call-ID`, a URI or a body, and holding those to a redaction would report forty
#: types where printing the bytes is the entire purpose of the log. Whoever widens the rollout
#: boundary must not widen this one with it, which one constant used twice could not have said.
#:
#: Why these two and not three. `sipx-audio` handles the same conversation, but there it is `i16`
#: samples and `_SAMPLE_BUFFER` already holds every type that touches them; adding it here would
#: hold `DspCapability`'s compile-time channel list to a rule aimed at a call. The bytes only
#: become bytes at `sipx_media::Encoded` and stay bytes down to `sipx_rtp::Packet`, and those two
#: types are the ones `M-80` marked `#[non_exhaustive]` together as "the two ends of one relay
#: path".
#:
#: What it holds out is not assumed empty: `outstanding_byte_buffers` counts every reachable public
#: type outside both scopes that holds a byte buffer, and the run prints the number. Reading the
#: fifty found four that are not protocol headers, and `M-117` answered all four — three of them by
#: adding the scope below, and `sipx_ua::Authenticator` by `key_problems`, which is not a scope at
#: all because a signing key is not audio.
RELAY_PATH = ("sipx-media", "sipx-rtp")

#: The crates where the call's own bytes come to **rest**, and the byte-buffer rule's second scope
#: (`M-117`). See `byte_buffer_problems`.
#:
#: A second constant rather than three more crates in `RELAY_PATH`, because that name means "the
#: call in flight" and these two crates are the opposite half of the same sentence.
#: `sipx_media::Encoded` holds one payload for as long as it takes to send it;
#: `sipx_app_protocol::Source::Inline` holds a prompt inside a document a host may have logged
#: whole on arrival, and `sipx_testkit::Record` holds every uplink byte of a call for the life of
#: the test. Same defect, opposite lifetime — and a rule whose scope constant claimed these were on
#: the relay path would be wrong about where they are.
#:
#: **Why these two crates and not the other nine.** `M-110`'s finding was that the fifty carriers
#: outside its scope are spread across eleven crates and that a crate is the unit on which they can
#: be decided one by one. That is true here and stays false for the rest: `sipx-sip`'s twenty-eight
#: are a `Call-ID`, a `Via`, a URI and a body, where rendering the octets is the entire purpose of
#: the log. These two hold six between them, every one of them was read, and each is either
#: redacted by hand or carries the reason its bytes are neither the call nor a participant.
#:
#: A test-fixture crate is in scope on purpose. `sipx-testkit`'s realtime peer stands in for the far
#: end of a real call — the bridge tests drive real media through it — and its record is formatted
#: into an assertion message by this workspace's own tests, so a failing run wrote a call's audio to
#: a CI log. Latent elsewhere, live here.
AUDIO_AT_REST = ("sipx-app-protocol", "sipx-testkit")

#: `sipx-app-protocol` owns a closed, versioned application vocabulary and documents its own
#: exceptions, so `A-9` explicitly leaves it out — a decision that outlives any particular
#: rollout boundary, which is why it survived `M-83` retiring the one the enum rule had.
#:
#: It is a stated exclusion and not a boundary of convenience, and the two are told apart the same
#: mechanical way: this names one crate and never an item, and what it holds out of the enum rule
#: is counted and printed on every run rather than disappearing.
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

    **The cut is the test *module*, and the anchor is what says so** (`M-121`). This read the first
    `#[cfg(test)]` anywhere in the file, and a test-only *item* carries the same attribute at an
    inner indentation: `sipx-media`'s DSP graph declares a `#[cfg(test)]` constructor 1,357 lines
    above the end of the file, so every declaration after it — `Stage`, `Buffers`, `Live`, `Slot`
    and `SlotRef`, which is the whole of `M-68`'s chain — was invisible to every rule here. Twelve
    files across five crates were cut short the same way. A test module in this workspace is
    written at the file's top level, and every one of those twenty-four inner attributes is a
    helper function, a field or a statement, so anchoring the cut at column zero cuts exactly what
    this docstring always claimed it did.
    """
    found = _TEST_MODULE.search(text)
    return text if found is None else text[: found.start()]


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

    `rfind` reports two different facts through one integer: a blank line found at some index, and
    no blank line at all. Adding 2 to both read the second as a match just before the file, started
    the slice at byte 1 and dropped the file's first character — enough to turn an opening
    `/// Exhaustive by design:` into `// Exhaustive by design:`, and an opening `#[non_exhaustive]`
    into `[non_exhaustive]`, so the item's guard was reported missing while it sat one byte above
    the slice. `M-96`. The two facts are separated here rather than special-cased at offset 0: a
    real match at index 0 is a blank line that still bounds the preamble.
    """
    above = source.rfind("\n\n", 0, offset)
    return source[0 if above < 0 else above + 2 : offset]


def marked(above: str) -> bool:
    """Whether a preamble *carries* `#[non_exhaustive]`, as against naming it.

    An attribute is syntax and a doc comment about one is prose, and `preamble` returns both as a
    single string, so the substring test this replaces read the second as the first. `M-83` wrote
    three arguments that explained a decision by naming the attribute — "`OverloadAlgorithm` is
    `#[non_exhaustive]` because its token set is a registry" among them — and every one of them was
    reclassified from *argued* to *marked* by a rule that could not tell the difference. It counted
    `sipx-media`'s `ProviderKind` as marked from `M-74` until `M-97`, on a sentence saying the type
    is deliberately **not** marked.

    The failure was quiet in the direction that matters. This whole check is built on the exception
    being written in one of two forms a reader can grep for; a third form nobody chose is the one
    thing it cannot afford. Over-narrowing here is loud instead: a reader that stopped recognising
    the attribute reports every marked type in the workspace, which is a red gate.
    """
    return _MARKED.search(above) is not None


def argued(above: str, phrase: str) -> bool:
    """Whether a preamble opens one of its doc lines with a rationale phrase.

    The phrases are `///` phrases and so genuinely prose, unlike the attribute — but the line
    anchor closes the same hole one step down: a doc comment that *quotes* a phrase mid-sentence
    while explaining a different type used to classify this one. What no rule can close is a phrase
    written at the start of a line about somebody else's type, which stays a reviewer's question
    (`M-97`).
    """
    return re.search(rf"(?m)^[ \t]*{re.escape(phrase)}", above) is not None


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


def constructs_self(body: str) -> bool:
    """Whether an `impl` body publishes an associated function that returns the type.

    Any name and not `new`, which is what `M-92` widened. `Sdes::cname`, `SrtpKeys::from_answer`
    and `RemoteCandidate::signalled` are the constructors their types publish, each named after the
    one thing it can honestly build; a rule that only accepted `new` would have asked for a second
    and worse-named function beside every one of them, which is the mechanical constructor the
    story that added this rule was told not to write.
    """
    for found in _PUBLIC_FN.finditer(body):
        opening = body.find("(", found.end())
        if opening < 0:
            continue
        depth, scan = 0, opening
        while scan < len(body):
            if body[scan] == "(":
                depth += 1
            elif body[scan] == ")":
                depth -= 1
                if depth == 0:
                    break
            scan += 1
        end = body.find("{", scan)
        if end < 0 or _RECEIVER.match(body[opening + 1 : scan]):
            continue
        # `-> Self`, `-> Option<Self>` and `-> Result<Self, E>` all build one; which of them a
        # fallible constructor uses is the type's business and not this rule's.
        if "Self" in body[scan + 1 : end]:
            return True
    return False


def publishes_a_constructor(source: str, name: str, above: str) -> bool:
    """Whether anything outside the crate has a way to build this type.

    The question this answers is whether `#[non_exhaustive]` would strand a caller, so it counts
    every way the crate offers rather than looking for one spelling.

    A derived or hand-written `Default` counts, and that is the part worth arguing. `T::default()`
    followed by assigning the public fields reaches every value a struct literal could reach — the
    same fields, one statement later — so a counter snapshot like `MediaDiscardCounts` is fully
    constructible without a twenty-argument `new` that no caller would want to read. What
    `#[non_exhaustive]` takes away from such a type is the literal and the functional-update
    shorthand, not the ability to build a value.
    """
    if _DERIVES_DEFAULT.search(above) or re.search(
        _IMPLEMENTS_DEFAULT.format(name=re.escape(name)), source
    ):
        return True
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
        if constructs_self(source[opening.end() : scan]):
            return True
    return False


def breakable_structs(crate: str) -> list[tuple[Path, str, str, int]]:
    """Reachable public structs a downstream literal can name, and so a new field can break.

    One boundary now stands between the rule in `struct_problems` and what runs, and it is
    `MEDIA_SURFACE`'s crate boundary. `M-80` added a second, a type boundary that held only a
    struct the crate already published a `new` for, and `M-92` removed it: it was sound about the
    hazard it named — `#[non_exhaustive]` on a struct with no constructor leaves a caller no way to
    build one — and wrong to answer that hazard by dropping the type out of the rule. Twenty-nine
    types on this surface sat outside the contract on the strength of it.

    What replaced it is an obligation rather than an exemption, and it is the second half of
    `struct_problems`: a marked struct must leave a way to build it, or say why nothing outside
    builds one. The hazard is now something the type answers instead of something it escapes.
    """
    found = []
    for path, module, name, offset in reachable_structs(crate):
        source = code(path.read_text(encoding="utf-8"))
        shape, body = declaration(source, offset)
        if has_a_public_field(shape, body):
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

    Two obligations since `M-92`, because the attribute has a cost as well as a benefit and a rule
    that charged only the benefit would be answered by marking everything. A struct that carries
    `#[non_exhaustive]` and publishes no constructor cannot be built from outside the crate at all;
    that is a real thing to publish — a snapshot, a report a worker hands out — and it is also the
    accident that happens when the attribute is applied by list. So the second obligation asks the
    type to say which it is: publish a constructor, or write `UNBUILT_REASON` beside it. That is
    the type boundary `M-80` used as an exemption, rewritten as something the type answers.
    """
    problems = []
    for crate in crates:
        for path, _module, name, offset in breakable_structs(crate):
            source = code(path.read_text(encoding="utf-8"))
            above = preamble(source, offset)
            line = source.count("\n", 0, offset) + 1
            try:
                where = path.relative_to(ROOT)
            except ValueError:
                where = path
            if not marked(above):
                if not argued(above, COMPLETE_REASON):
                    problems.append(
                        f"{where}:{line} `{name}` is reachable from the crate root and has public "
                        f"fields; add `#[non_exhaustive]` or an adjacent `{COMPLETE_REASON}` "
                        f"rationale"
                    )
                continue
            if publishes_a_constructor(source, name, above) or argued(above, UNBUILT_REASON):
                continue
            problems.append(
                f"{where}:{line} `{name}` is `#[non_exhaustive]` and publishes no constructor, so "
                f"nothing outside the crate can build one; publish a constructor or add an "
                f"adjacent `{UNBUILT_REASON}` rationale"
            )
    return problems


def type_body(source: str, offset: int) -> str:
    """The braced or parenthesised body of the item declared at `offset`, code only.

    `declaration` is the struct rule's reader and is deliberately not reused here. It counts brace
    depth over the raw text, which its own comment justifies — "a struct body holds field types,
    and no field type contains an unbalanced brace" — and that is true of a struct and false of an
    enum. `sipx-testkit`'s `Malformed` documents its first variant as ``/// `not json{` ``, and a
    depth count that reads a doc comment as code never closes that body at all.

    Comments and string literals are dropped rather than skipped over, because this reader's
    caller matches a *type* against the result: a doc line naming `Vec<i16>` while explaining a
    field, or an `#[error("…")]` message quoting one, is prose about a buffer and not a buffer.

    Returns the empty string for a unit struct, which has no body to read.
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
        return ""
    opener = source[index]
    closer = "}" if opener == "{" else ")"
    depth, scan, kept = 0, index, []
    while scan < end:
        character = source[scan]
        if character == "/" and source[scan + 1 : scan + 2] == "/":
            newline = source.find("\n", scan)
            scan = end if newline < 0 else newline
            continue
        if character == '"':
            scan += 1
            while scan < end and source[scan] != '"':
                scan += 2 if source[scan] == "\\" else 1
            scan += 1
            continue
        if character == opener:
            depth += 1
            if depth == 1:
                scan += 1
                continue
        elif character == closer:
            depth -= 1
            if depth == 0:
                return "".join(kept)
        kept.append(character)
        scan += 1
    raise ValueError(f"an item at offset {offset} has no closing `{closer}`")


class Declaration(NamedTuple):
    """Where one struct or enum of a crate is written, however private it is."""

    #: The file it is declared in.
    path: Path
    #: The offset of its `struct` or `enum` keyword. See `_DECLARED_TYPE` for why not the line's.
    offset: int


class TypeIndex(NamedTuple):
    """A crate's type declarations, and the source of every module they were read out of."""

    #: Every name a field can be written in, to the declarations that carry it. A name is not
    #: unique in a crate — `sipx-audio` declares three private `Band`s — so this is a tuple.
    declarations: dict[str, tuple[Declaration, ...]]
    #: Each module's file, already cut at its test module, so the chase reads each one once.
    sources: dict[Path, str]


def type_index(crate: str) -> TypeIndex:
    """Every struct and enum a crate declares, by name, with the sources they were read from.

    The same modules `reachable` walks, read for the third question this file asks of them: not
    what backs a claim and not what a caller can name, but what a field of a public type points at.
    """
    declarations: dict[str, list[Declaration]] = {}
    sources: dict[Path, str] = {}
    for reach in module_graph(crate, entry_point(crate)).values():
        text = code(reach.path.read_text(encoding="utf-8"))
        sources[reach.path] = text
        for found in _DECLARED_TYPE.finditer(text):
            declarations.setdefault(found.group("name"), []).append(
                Declaration(path=reach.path, offset=found.start("kind"))
            )
    return TypeIndex(
        declarations={name: tuple(sites) for name, sites in declarations.items()},
        sources=sources,
    )


def _parts(text: str) -> list[str]:
    """`text` split on the commas that are not inside a bracket of any of the four kinds."""
    parts: list[str] = []
    depth, current = 0, ""
    for character in text:
        if character in _OPENERS:
            depth += 1
        elif character in _CLOSERS:
            depth -= 1
        if character == "," and depth == 0:
            parts.append(current)
            current = ""
        else:
            current += character
    parts.append(current)
    return [part.strip() for part in parts if part.strip()]


def _outermost(text: str, character: str) -> int:
    """Where `character` first appears outside every bracket, or -1."""
    depth = 0
    for index, found in enumerate(text):
        if found in _OPENERS:
            depth += 1
        elif found in _CLOSERS:
            depth -= 1
        elif found == character and depth == 0:
            return index
    return -1


def field_types(body: str) -> list[str]:
    """The type expression of every field in a struct or enum body, field names dropped.

    Per field rather than over the whole body, because whether a chain runs through a field is a
    property of *that* field's type: `MediaSession` holds both an `mpsc::Sender<Frame>`, which
    renders nothing it carries, and an `Arc<InboundQueue>`, which renders everything. A reader that
    decided for a whole type would have to choose between missing the second and inventing the
    first.

    An enum is unwrapped rather than read flat: a variant's own name is not a type, and
    `RecognitionInput::Frame(RecognitionFrame)` resolved by name would chase every `Frame` in the
    crate on the strength of a variant label. Both variant forms are unwrapped, and a unit variant
    is left as it is written — it names no type, and no declaration will answer to it.

    `->` is spelled away first: a `>` in a function type is not a closing bracket, and one counted
    as one leaves every field after it at the wrong depth.
    """
    types: list[str] = []
    for part in _parts(body.replace("->", "  ")):
        colon = _outermost(part, ":")
        if colon >= 0:
            types.append(part[colon + 1 :].strip())
        elif "{" in part:
            types += field_types(part[part.index("{") + 1 : part.rfind("}")])
        elif "(" in part:
            types += _parts(part[part.index("(") + 1 : part.rfind(")")])
        else:
            types.append(part)
    return types


def followed_types(body: str) -> list[str]:
    """The names a rendering of this body would render, which is what the chase follows.

    Every identifier of a field's type and not just its head, because a buffer reached through
    `Arc<Mutex<Slot>>` is reached through `Slot`. Names that are not this crate's declarations —
    `Arc`, `u64`, a lifetime, a const generic — resolve to nothing and fall away in `resolve`.
    """
    names: list[str] = []
    for field in field_types(body):
        if any(name in OPAQUE_CONTAINERS for name in _CONSTRUCTOR.findall(field)):
            continue
        names += _IDENTIFIER.findall(field)
    return list(dict.fromkeys(names))


def resolve(index: TypeIndex, path: Path, name: str) -> tuple[Declaration, ...]:
    """Where a name written in `path` is declared, preferring a declaration in that same file.

    A crate is not a flat namespace and this reader does not parse `use` lines, so a name is
    resolved by the one rule that is right whenever it applies and conservative when it does not:
    a type declared beside the field that names it *is* the one meant, and otherwise every
    declaration of that name is followed.

    Both halves were measured. `sipx-audio` declares three private `Band`s — the peaking filter's
    two one-pole sections, the sub-band suppressor's three integers, and G.722's twenty-four-sample
    delay line — and resolving flatly reported the first two for a buffer in a codec neither has
    heard of. The fall-back is what reaches `SlotRef` from `dsp/mod.rs`, four hops of `M-68`'s
    chain being in a file the type naming them is not, and it errs toward reporting: a name that
    resolves to two declarations is chased into both, and the cost of guessing wrong is a report
    rather than a silence.
    """
    sites = index.declarations.get(name, ())
    beside = tuple(site for site in sites if site.path == path)
    return beside or sites


def answers_the_rule(source: str, name: str, offset: int) -> bool:
    """Whether a type decides its own rendering, which is what stops the chase at it.

    The same two answers `sample_buffer_problems` has always accepted, asked at every hop rather
    than only at the public type. A derived `Debug` above a hand-written one renders the
    hand-written one, so a redaction halfway down a chain cuts everything above it — which is
    exactly where `M-68` wrote four of its five fixes.
    """
    if re.search(_IMPLEMENTS_DEBUG.format(name=re.escape(name)), source):
        return True
    return argued(preamble(source, offset), NOT_AUDIO_REASON)


def carrier_chain(
    index: TypeIndex, path: Path, name: str, offset: int, *, redacted: bool
) -> tuple[str, ...]:
    """The types from the one at `offset` to the PCM buffer its `Debug` reaches, or empty.

    `redacted` is the difference between the two questions this rule asks of the same walk. With it
    the chase stops at any type that decides its own rendering, and what comes back is a leak; with
    it off nothing stops the chase but the depth, and what comes back is the *population* — every
    reachable public type that can reach a buffer at all, whether or not somebody has already
    redacted it. `sample_buffer_carriers` counts the second so the printed number describes the
    reader's reach rather than the workspace's remaining debt.

    Depth-first and first-answer-wins: one chain is enough to report a type, and the shortest is
    not more true than another. A type is entered once per chain, so the cycles a real type graph
    has — `sipx_media::MediaSession` holds a `Mutex<Vec<MediaSession>>` of retired generations —
    terminate rather than recur.
    """

    def walk(
        path: Path, name: str, offset: int, seen: frozenset[tuple[Path, str]], trail: tuple[str, ...]
    ) -> tuple[str, ...]:
        if (path, name) in seen:
            return ()
        source = index.sources[path]
        if redacted and answers_the_rule(source, name, offset):
            return ()
        trail = (*trail, name)
        body = type_body(source, offset)
        if _SAMPLE_BUFFER.search(body):
            return trail
        if len(trail) > _CARRIER_HOPS:
            return ()
        seen = seen | {(path, name)}
        for word in followed_types(body):
            for site in resolve(index, path, word):
                found = walk(site.path, word, site.offset, seen, trail)
                if found:
                    return found
        return ()

    return walk(path, name, offset, frozenset(), ())


def sample_buffer_carriers(crates: list[str]) -> list[tuple[Path, str, int]]:
    """Every reachable public type of these crates whose `Debug` can reach a buffer of PCM samples.

    Structs and enums together, because the two leak identically: `PcmSamples` is the enum that
    holds every owned buffer in this workspace, and a rule that read only structs would have missed
    the one type all the others are made of.

    Its own field or a private type's, up to `_CARRIER_HOPS` away (`M-121`). Counting only the
    first would print a number about the reader this one replaced.
    """
    found = []
    for crate in crates:
        index = type_index(crate)
        for pattern in (_PUBLIC_STRUCT, _PUBLIC_ENUM):
            for path, _module, name, offset in reachable(crate, pattern):
                if carrier_chain(index, path, name, offset, redacted=False):
                    found.append((path, name, offset))
    return found


def sample_buffer_problems(crates: list[str]) -> list[str]:
    """Reachable public types whose `Debug` would render the call's own audio.

    `M-61` found `sipx_audio::AnalysisFrame`'s derived `Debug` rendering all 65,536 samples it
    borrowed, reachable from a refusal record `sipx-call` already wrote; `M-107` found the same
    derive on `sipx_media::PcmFrame`, latent only because nothing logged it yet. Both are one
    defect written twice: raw call audio in the place an operator copies into a ticket, in a record
    whose length is the audio's rather than the format's. A type that holds samples therefore
    **implements** `Debug` — identity, position and a count — or says beside itself why its buffer
    is not somebody's conversation.

    The rule asks for the implementation rather than forbidding the derive, and that is the whole
    reason it is safe to narrow. Reading "does not derive `Debug`" would be quiet when it went
    wrong: a reader that stopped recognising `#[derive(Debug)]` would excuse every carrier in the
    workspace at exit 0. Reading "implements `Debug`" fails the other way — a reader that stopped
    recognising the implementation reports every carrier, which is a red gate. `M-97` made the same
    correction to `marked`, and `X-131` and `X-132` to the CLI-reference readers.

    **What it cannot check, stated rather than discovered.** It reads a field's element type, so it
    holds `i16` and `f32` buffers and nothing else. Encoded audio is `Bytes` or `Vec<u8>`, which in
    this workspace is equally a `Call-ID`, a SIP body and a STUN attribute — sixty-odd reachable
    public types, nearly all of them protocol bytes whose `Debug` should be their bytes. So
    `sipx_media::Encoded` and `sipx_rtp::Packet` carry the call in a shape no checker here can tell
    from a header, and they stay a reviewer's question: `M-107` redacted the first by hand and
    `M-110` is the second. It also cannot read what an implementation *prints*; it enforces that
    somebody wrote one, and the tests beside each type enforce what it says.

    **What `M-121` added, and where it still stops.** The reader above read a public type's *own*
    fields, and `M-68` then found the same defect one indirection past that: `sipx_media::DspGraph`
    holds no buffer, it holds a `SlotRef`, which holds an `Arc<Mutex<Slot>>`, which holds the live
    generation, which holds the frame in flight — and this check passed the tree that contained it.
    So a public type's fields are now followed into the crate's private types, `_CARRIER_HOPS` deep,
    and a chain is cut at the first type that decides its own rendering. That constant states the
    depth, why it is what it is, and what a carrier past it still costs; `OPAQUE_CONTAINERS` states
    the one kind of field the chase deliberately does not follow, and which way each of the two
    fails. Running it over the workspace found `MediaSession` and `PcmProcessor` rendering the whole
    of a bounded audio queue, three hops down in both cases — the same defect a third and fourth
    time.
    """
    problems = []
    for crate in crates:
        index = type_index(crate)
        for pattern in (_PUBLIC_STRUCT, _PUBLIC_ENUM):
            for path, _module, name, offset in reachable(crate, pattern):
                chain = carrier_chain(index, path, name, offset, redacted=True)
                if not chain:
                    continue
                source = index.sources[path]
                line = source.count("\n", 0, offset) + 1
                try:
                    where = path.relative_to(ROOT)
                except ValueError:
                    where = path
                if len(chain) == 1:
                    problems.append(
                        f"{where}:{line} `{name}` is reachable from the crate root and holds a "
                        f"buffer of PCM samples, so a derived `Debug` renders the call's own "
                        f"audio; implement `Debug` with a sample count, or add an adjacent "
                        f"`{NOT_AUDIO_REASON}` rationale"
                    )
                else:
                    # The chain, because the type to redact is not the type reported: a reviewer
                    # sent to `MediaSession` for a buffer three private hops away has nothing to
                    # fix in the file the report names.
                    problems.append(
                        f"{where}:{line} `{name}` is reachable from the crate root and its "
                        f"derived `Debug` reaches a buffer of PCM samples through "
                        f"{' -> '.join(chain)}, so rendering it renders the call's own audio; "
                        f"implement `Debug` on `{chain[-1]}` with a sample count, or add an "
                        f"adjacent `{NOT_AUDIO_REASON}` rationale"
                    )
    return problems


def unreadable_surface(carriers: list[tuple[Path, str, int]]) -> list[str]:
    """The one problem a sample-buffer reader that has gone blind reports about itself.

    See `_PLAUSIBLE_CARRIERS`. Every other narrowing in this file fails loudly by reporting types;
    a selector fails by reporting none, so the population it selected is held to a floor and a run
    that finds almost nothing says so instead of passing.
    """
    if len(carriers) >= _PLAUSIBLE_CARRIERS:
        return []
    return [
        f"the sample-buffer reader recognised {len(carriers)} public types that hold or reach PCM "
        f"samples, below the {_PLAUSIBLE_CARRIERS} this workspace's audio path is built from; the "
        f"reader has narrowed rather than the workspace changed"
    ]


def byte_buffer_carriers(crates: list[str]) -> list[tuple[Path, str, int]]:
    """Every reachable public type of these crates that holds a buffer of raw octets.

    Structs and enums together, for `sample_buffer_carriers`' reason and one more: `Rtcp` is an
    enum whose `Other` variant holds the body of a packet the crate does not model, and a rule that
    read only structs would have held both ends of the relay path and missed the one in the middle.
    """
    found = []
    for crate in crates:
        for pattern in (_PUBLIC_STRUCT, _PUBLIC_ENUM):
            for path, _module, name, offset in reachable(crate, pattern):
                source = code(path.read_text(encoding="utf-8"))
                if _BYTE_BUFFER.search(type_body(source, offset)):
                    found.append((path, name, offset))
    return found


def byte_buffer_problems(crates: list[str]) -> list[str]:
    """Reachable public types on the relay path whose `Debug` would render the call, or a person.

    **This is the rule `M-107` concluded could not exist, narrowed until it could** (`M-110`). That
    conclusion was about `Bytes` in general and it is correct in general: 58 reachable public types
    in this workspace hold a byte buffer, and for nearly all of them — a `Call-ID`, a URI, a SIP
    body, a STUN attribute — rendering the octets is the entire point of the log. What the
    conclusion missed is that the sixty are spread across eleven crates. On the two the call's own
    bytes pass through, every one of them can be decided by a person once and held by a checker
    afterwards, which is the difference between a review and a rule. See `RELAY_PATH`.

    So a reachable public type in those crates that holds a byte buffer **implements** `Debug` — a
    payload type, a count, a length — or says beside itself that its bytes are neither the call nor
    a participant. The population is eight and every one of them has an answer today: `Encoded`,
    `Packet`, `Rtcp` and `SdesItem` redact, `SrtpKeys` already did, and the ICE and STUN carriers
    argue.

    **Two sensitivities, and the escape phrase is where the difference is enforced.** A payload is
    the conversation still encoded, and for G.711 an octet is a sample. An RFC 3550 §6.5 `SdesItem`
    is not audio at all — it is a login name, a real name, an email address — and the reason it is
    redacted is not the payload's reason. `NOT_THE_CALL_REASON` exists because the audio phrase
    would have excused it *truthfully*; see that constant.

    **Where it is narrowed, and which way each narrowing fails.** Asking for the implementation
    rather than forbidding the derive is `sample_buffer_problems`' argument unchanged, and it fails
    loudly: a reader blind to a hand-written `Debug` reports all eight carriers. `_BYTE_BUFFER` is
    the selector, which fails by selecting nothing, so its population is held to a floor by
    `unread_relay_path`. The escape is a line-anchored phrase, so a reader that stopped recognising
    it reports the types it excuses.

    **A second scope, on the same terms** (`M-117`). `AUDIO_AT_REST` names the two crates where the
    same call comes to rest rather than passes through: a prompt carried inside a document, and
    every uplink byte of a call kept in a test peer's record. The rule is not widened, duplicated or
    weakened for them — it is the same rule over a second list of crates, and its six carriers were
    read the same way `M-110` read its eight. Two scopes rather than one list because the names mean
    different things about their own futures; see both constants.

    **What it still cannot do.** It cannot read what an implementation *prints* — the tests beside
    each type do that: `crates/sipx-rtp/tests/payload_diagnostics.rs` for `M-110`'s,
    `crates/sipx-app-protocol/tests/document_diagnostics.rs` and
    `crates/sipx-testkit/tests/realtime_peer_diagnostics.rs` for `M-117`'s. It cannot see a *field*,
    only a type, so a caller who formats `Record::appended_audio` itself still gets the bytes; that
    field is public because ORB-3 asserts on those octets, and the type's own documentation says so.
    And it still reaches no further than the four crates it names — forty-four carriers stay
    outside, `outstanding_byte_buffers` counts them on every run, and they are protocol headers a
    log exists to print rather than a debt.
    """
    problems = []
    for path, name, offset in byte_buffer_carriers(crates):
        source = code(path.read_text(encoding="utf-8"))
        above = preamble(source, offset)
        if re.search(_IMPLEMENTS_DEBUG.format(name=re.escape(name)), source):
            continue
        if argued(above, NOT_THE_CALL_REASON):
            continue
        line = source.count("\n", 0, offset) + 1
        try:
            where = path.relative_to(ROOT)
        except ValueError:
            where = path
        problems.append(
            f"{where}:{line} `{name}` is reachable from the crate root of a crate the call's own "
            f"octets reach and holds a buffer of them, so a derived `Debug` renders whatever the "
            f"call put in it; implement `Debug` with a length, or add an adjacent "
            f"`{NOT_THE_CALL_REASON}` rationale"
        )
    return problems


def unread_relay_path(carriers: list[tuple[Path, str, int]]) -> list[str]:
    """The one problem a byte-buffer reader that has gone blind reports about itself.

    `unreadable_surface`, for the other selector. See `_PLAUSIBLE_RELAY_CARRIERS`: narrowing
    `_BYTE_BUFFER` is quiet by construction, so the population it selected is held to a floor.
    """
    if len(carriers) >= _PLAUSIBLE_RELAY_CARRIERS:
        return []
    return [
        f"the byte-buffer reader recognised {len(carriers)} public types holding octets across "
        f"{' and '.join(RELAY_PATH)}, below the {_PLAUSIBLE_RELAY_CARRIERS} the relay path is "
        f"built from; the reader has narrowed rather than the workspace changed"
    ]


def on_the_relay_path(crates: list[str]) -> list[str]:
    """The crates the byte-buffer rule holds because the call passes through them. `RELAY_PATH`."""
    return [crate for crate in crates if crate in RELAY_PATH]


def where_the_call_rests(crates: list[str]) -> list[str]:
    """The crates it holds because the call stops in them. `AUDIO_AT_REST`."""
    return [crate for crate in crates if crate in AUDIO_AT_REST]


def key_carriers(crates: list[str]) -> list[tuple[Path, str, int]]:
    """Every reachable public type of these crates that keeps a fixed-size array of octets.

    Structs and enums together, for `sample_buffer_carriers`' reason: a key is as easily a variant's
    payload as a field, and `SrtpKeys` is the shape that made this workspace's first one.
    """
    found = []
    for crate in crates:
        for pattern in (_PUBLIC_STRUCT, _PUBLIC_ENUM):
            for path, _module, name, offset in reachable(crate, pattern):
                source = code(path.read_text(encoding="utf-8"))
                if _KEY_ARRAY.search(type_body(source, offset)):
                    found.append((path, name, offset))
    return found


def key_problems(crates: list[str]) -> list[str]:
    """Reachable public types whose `Debug` would render a key.

    `M-110`'s implementor found `sipx_ua::Authenticator` deriving a `Debug` over `secret: [u8; 32]`
    while counting byte buffers for a story about audio, and fixed it out of band because it is not
    the same kind of finding. A record carrying that array does not leak a credential somebody could
    replay: it is the key every self-describing nonce is `MACed` with, so whoever reads the record
    can **mint nonces this authenticator accepts as its own**, which is the whole of the replay
    protection. Nothing logged one, which is what made it latent rather than live — exactly the
    state `PcmFrame` was in for one release before `M-107`.

    **This is a rule and not a hand fix, which is the decision `M-117` was filed to take.** The
    alternative was to redact the one type and write a note; the objection to that is `M-107`'s own
    history, where a hand fix at one type left the same derive at two more for three stories. What
    a rule buys is the *next* key — the type nobody has written yet meets the question at the moment
    it is written rather than at the review that happens to notice.

    **No scope, and that is the point of choosing this selector.** The byte-buffer rule needs a
    crate list because a `Vec<u8>` is a payload in one crate and a `Call-ID` in another. A `[u8; N]`
    is a length the *type* chose rather than one the wire chose, and this workspace has never used
    that shape for a message: see `_KEY_ARRAY`. So the rule runs over every published crate and
    excuses nothing for being protocol.

    **It overlaps `byte_buffer_problems` on the four crates those two scopes name, deliberately.**
    `_BYTE_BUFFER` already matches `[u8; N]`, so a key on the relay path is selected twice — and the
    two rules ask different questions, so it must answer both. A type that says its bytes are not
    the call has said nothing about whether they are a secret, which is `NOT_A_SECRET_REASON`'s
    whole argument.

    **What it cannot do.** It cannot read what an implementation *prints* —
    `crates/sipx-ua/tests/authenticator_diagnostics.rs` checks all four spellings the key could
    survive as. And a key kept in a `Vec<u8>` is outside this selector and inside the byte-buffer
    rule only if its crate is in scope: `sipx_app::SessionApp` holds one that way and redacts it by
    hand, which no rule here required of it.
    """
    problems = []
    for path, name, offset in key_carriers(crates):
        source = code(path.read_text(encoding="utf-8"))
        above = preamble(source, offset)
        if re.search(_IMPLEMENTS_DEBUG.format(name=re.escape(name)), source):
            continue
        if argued(above, NOT_A_SECRET_REASON):
            continue
        line = source.count("\n", 0, offset) + 1
        try:
            where = path.relative_to(ROOT)
        except ValueError:
            where = path
        problems.append(
            f"{where}:{line} `{name}` is reachable from the crate root and keeps octets in a "
            f"fixed-size array, which in this workspace is a key rather than a message, so a "
            f"derived `Debug` prints it; implement `Debug` with the array redacted, or add an "
            f"adjacent `{NOT_A_SECRET_REASON}` rationale"
        )
    return problems


def unread_keys(carriers: list[tuple[Path, str, int]]) -> list[str]:
    """The one problem a key reader that has gone blind reports about itself.

    `unreadable_surface` and `unread_relay_path`, for the third selector — and the weakest of the
    three, for the reason `_PLAUSIBLE_KEY_CARRIERS` states in full.
    """
    if len(carriers) >= _PLAUSIBLE_KEY_CARRIERS:
        return []
    return [
        f"the key reader recognised {len(carriers)} public types keeping octets in a fixed-size "
        f"array, below the {_PLAUSIBLE_KEY_CARRIERS} this workspace holds; either the reader has "
        f"narrowed, or this rule now protects nothing and should be retired rather than left green"
    ]


def outstanding_byte_buffers(crates: list[str]) -> list[str]:
    """Every reachable public type outside both scopes that holds a byte buffer.

    A stated scope whose size nobody prints is a suppression list with a better name — the argument
    `outside_the_rule` and `outstanding_structs` each make, applied to the scopes `M-110` and
    `M-117` chose. The number is what makes re-reading `RELAY_PATH` and `AUDIO_AT_REST` a decision
    somebody takes rather than one that decays, and it is the number that says how much of the
    workspace is still a reviewer's question.

    **The count is what found `M-117`.** Fifty carriers were printed on every run for one story's
    length, somebody read the fifty rather than the number, and four of them turned out not to be
    protocol headers at all. That is the count working exactly as intended and it is also the
    warning attached to it: what makes a number like this worth printing is that it is read back,
    and forty-four is not smaller than fifty in any way that means the remainder was checked.

    It counts *carriers* rather than *problems*, unlike `outstanding_structs`. Nearly every one of
    them is a header a log exists to print, so reporting them as unfixed work would be a debt line
    that misrepresents its own contents; what the count measures is how far the rule does not
    reach.
    """
    held = set(RELAY_PATH) | set(AUDIO_AT_REST)
    return [
        f"{path}:{name}"
        for path, name, _offset in byte_buffer_carriers(
            [crate for crate in crates if crate not in held]
        )
    ]


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
            if marked(above) or argued(above, EXHAUSTIVE_REASON):
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
    """The crates whose reachable enums this run holds to the guard: every one that publishes.

    `M-83` retired the rollout boundary this used to read, so the answer is now derived from the
    workspace the way `published` is — a crate added later joins the enum rule by existing rather
    than by somebody remembering to name it. The one subtraction is `CLOSED_VOCABULARY`, which is a
    stated decision about a crate rather than a rollout's convenience.
    """
    return [crate for crate in crates if crate != CLOSED_VOCABULARY]


def outside_the_rule(crates: list[str]) -> list[str]:
    """The crates the enum rule does not hold, whose count the summary line reports.

    Exactly `CLOSED_VOCABULARY` since `M-83`, and it is still counted rather than assumed empty:
    an exclusion whose size nobody prints is indistinguishable from a suppression list, and the
    number is what makes re-reading `A-9` a decision somebody takes rather than one that decays.
    """
    return [crate for crate in crates if crate == CLOSED_VOCABULARY]


def guarded_structs(crates: list[str]) -> list[str]:
    """The crates whose reachable structs this run holds to the guard. See `MEDIA_SURFACE`."""
    return [crate for crate in crates if crate in MEDIA_SURFACE and crate != CLOSED_VOCABULARY]


def outstanding_structs(crates: list[str]) -> list[str]:
    """Every reachable public-field struct in the workspace the struct rule does not hold yet.

    `M-80` counted these over all published crates including the guarded ones, because its type
    boundary left work inside `MEDIA_SURFACE` too and a debt line that reported only the crates
    outside would have said the media surface was finished. `M-92` removed that boundary and paid
    the work down, so inside `MEDIA_SURFACE` there is nothing left for this to find — a struct
    there is either held or a failure in `struct_problems`, and reporting it twice under two names
    would double-count it. What remains is what it has always meant: a reachable public-field
    struct whose field set a downstream literal still depends on.

    The printed number went 155 to 147 across that change and it did not fall by eight. `M-80`
    computed its `held` set for *every* crate, so a struct outside `MEDIA_SURFACE` that happened to
    publish a `new` was subtracted from the debt as well — in a crate where no rule holds it,
    which made it neither guarded nor counted. 155 was 29 inside plus 126 outside; 147 is those
    same 126 plus the 21 that subtraction was hiding. Paying down the 29 and un-hiding the 21 is
    the whole of the difference.
    """
    outstanding = []
    for crate in crates:
        if crate == CLOSED_VOCABULARY or crate in MEDIA_SURFACE:
            continue
        for path, _module, name, offset in breakable_structs(crate):
            source = code(path.read_text(encoding="utf-8"))
            above = preamble(source, offset)
            if marked(above) or argued(above, COMPLETE_REASON):
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
    carriers = sample_buffer_carriers(crates)
    relay = byte_buffer_carriers(on_the_relay_path(crates))
    at_rest = byte_buffer_carriers(where_the_call_rests(crates))
    keys = key_carriers(crates)
    problems += enum_problems(guarded(crates)) + struct_problems(guarded_structs(crates))
    problems += sample_buffer_problems(crates) + unreadable_surface(carriers)
    problems += byte_buffer_problems(on_the_relay_path(crates)) + unread_relay_path(relay)
    # The second scope needs no floor of its own: it shares `_BYTE_BUFFER` with the rule above, and
    # `unread_relay_path` already fails if that selector goes blind. A second floor over the same
    # regex would check the same thing twice and would fire on a crate being *removed* rather than
    # on a reader stopping reading, which is an accusation this file is careful not to make.
    problems += byte_buffer_problems(where_the_call_rests(crates))
    problems += key_problems(crates) + unread_keys(keys)
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
    # What the one stated exclusion holds out of the enum rule, counted on every run. See
    # `CLOSED_VOCABULARY`: an exclusion that reported nothing would be a suppression list with a
    # better name, and that is as true of a decision as it was of the rollout boundary `M-83`
    # retired.
    excused = enum_problems(outside_the_rule(crates))
    print(
        f"{len(guarded(crates))} crates hold every reachable public enum non-exhaustive or argued; "
        f"{len(excused)} reachable enums in {CLOSED_VOCABULARY} stay exhaustive, which A-9 decided "
        f"because that crate's vocabulary is closed and versioned"
    )
    # The struct rule's debt, on the same terms and for the same reason (`M-80`).
    structs = outstanding_structs(crates)
    print(
        f"{len(MEDIA_SURFACE)} crates hold every reachable public-field struct non-exhaustive or "
        f"argued, and every marked one buildable; {len(structs)} reachable public-field structs "
        f"outside that boundary can still be broken by a new field"
    )
    # The sample-buffer rule's population, printed for the reason `_PLAUSIBLE_CARRIERS` gives: a
    # selector that quietly stopped selecting is the one narrowing in this file that a red gate
    # would not report by itself (`M-107`).
    print(
        f"{len(carriers)} reachable public types hold a buffer of PCM samples or reach one within "
        f"{_CARRIER_HOPS} private hops, and every one of them implements `Debug`, argues it is not "
        f"call audio, or is cut off from the buffer by a type that does; encoded audio in `Bytes` "
        f"is outside what an element type can decide and is held by scope instead"
    )
    # The byte-buffer rule's population and the scope's remainder, on the terms every other
    # boundary in this file is held to (`M-110`). Both numbers, because they answer different
    # questions: the first is whether the reader still reads, the second is how much of the
    # workspace this rule deliberately does not reach.
    outside = outstanding_byte_buffers(crates)
    print(
        f"{len(relay)} reachable public types across {' and '.join(RELAY_PATH)} hold a buffer of "
        f"octets where the call passes through, and {len(at_rest)} across "
        f"{' and '.join(AUDIO_AT_REST)} where it comes to rest; every one of them implements "
        f"`Debug` or argues its bytes are not the call, and the {len(outside)} carriers outside "
        f"both scopes are protocol headers a log exists to print"
    )
    # The key rule's population, on the same terms and for the weakest of the three reasons — see
    # `_PLAUSIBLE_KEY_CARRIERS`, which is the only floor here set *at* its population.
    kept = "type keeps" if len(keys) == 1 else "types keep"
    print(
        f"{len(keys)} reachable public {kept} octets in a fixed-size array, which in this "
        f"workspace is a key rather than a message, and every one of them implements `Debug` or "
        f"argues it is not a secret"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
