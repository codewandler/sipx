# Spec: media format identity, and the one rule that decides it

**Status:** normative. · **Crates:** `sipx-sdp` (the rule and the answer), `sipx-call` (which
formats correspond to a codec it can run) · **Stories:**
[M-31](../stories/M-31-the-answer-and-the-negotiated-codec-can-disagree.md), with
[M-1](../stories/M-1-sdp-and-offer-answer.md) and
[M-30](../stories/M-30-a-call-cannot-select-opus.md) as the
predecessors it corrects · **Design:** [media](../designs/media.md)

Where this document and the code disagree, this document is right until somebody changes it
deliberately.

This is a narrow spec on purpose. It covers one question — *do two `a=rtpmap` values name the same
media format?* — because that question was answered in two places, the two answers differed, and
the difference was inaudible in the test suite and audible on the call.

## 1. Normative references

- **RFC 8866** — SDP. §6.6 (`a=rtpmap`: the attribute, its fields, and the clock rate and encoding
  parameters as part of a format's identity), §9 (the ABNF, including the `integer` production the
  clock rate and channel count are instances of).
- **RFC 4566** — the same grammar, superseded by 8866. Cited because peers and other specs still
  refer to it; sipx implements 8866 and the two do not differ in §6.6's shape.
- **RFC 3264** — offer/answer. §6.1 (the answer lists the formats both sides support, in the
  *offerer's* order).
- **RFC 3551** — the static payload type assignments, which are what a format number means when no
  `a=rtpmap` remaps it. §4.5.14 (G.711 as mandatory-to-implement).
- **RFC 7587** — RTP payload format for Opus. §7 (the RTP clock is 48000 and the rtpmap channel
  count is 2, whatever the audio actually is).
- **RFC 4733** — `telephone-event`. §2.5.1.2 (the `fmtp` event list is per-direction).

## 2. The rule, and where it lives

**§2.1 — Format identity.** Two `a=rtpmap` values name the same media format when all three hold:

1. the encoding names are equal, compared **case-insensitively**;
2. the clock rates are equal **by value**;
3. the channel counts are equal **by value**, where an omitted encoding parameter is one channel
   (RFC 8866 §6.6).

The clock rate and the channel count are part of the format's identity, not decoration: the same
codec at two rates is two formats, and a stack that ignores the rate agrees to a format it cannot
decode.

**§2.2 — By value, not by spelling.** The clock rate and channel count are *numbers*. The identity
of a number is numeric equality. A textual comparison answers a different question — whether the
two are spelled the same — and answering that question instead is the defect this document exists to
close: `08000` and `8000` are numerically equal and textually different, so a text comparison and a
parsing one disagree on every such spelling. This generalises past leading zeros to whitespace, a
sign, a digit separator, and anything else numerically equal and textually different, which is why
the rule is stated in terms of *value* rather than as a list of spellings to normalise.

**§2.3 — There is exactly one implementation, and it is in the lower crate.**
`sipx_sdp::rtpmap::same_format` is the rule. Two callers ask it, and neither has a copy:

| Caller | The question it is asking | What it does with the answer |
| --- | --- | --- |
| `sipx_sdp::answer::supports` | is this offered format one this side supports? | it goes in the answer's `m=` format list |
| `sipx_call::call::codec_named` | which codec, if any, does this offered format name? | the media session is built with it |

The direction is forced, and the argument is worth writing down because the other direction looks
equally available until you try it. `sipx-sdp` is the lower crate; `sipx-call` depends on it, not
the reverse. `sipx_sdp::answer` builds the answer that goes on the wire, so it must be able to ask
the question, so the rule cannot live above it. The only arrangement in which one implementation
serves both is the lower crate holding it.

What comes down with the rule is nothing that belongs above it. `sipx-sdp` learns no codec-set
concept, no selection policy and no preference order — only the grammar and what makes two values
equal. `sipx-call` keeps both halves that are its own: **which** rtpmaps it has a codec for
(`offered_rtpmap`, whose exhaustive match over `Codec` forces a decision when a codec is added) and
**which** the application selected (`Codecs::carries`). This is why the split is at format identity
and not somewhere more convenient: identity is the largest piece of the question that carries none
of the layer above with it.

**§2.4 — Preference order is the offerer's, and is applied above the rule.** The rule is a
predicate on a pair of values and has no opinion about order. RFC 3264 §6.1 gives the order to the
offerer, and it is applied by walking the offer's `m=` format list in order and taking the first
format that both names a codec sipx can run and is in the selected set. The set membership test is
*part of the search*, not a filter on its result: applying it afterwards stops at the offerer's
first choice and refuses the whole stream when that one format is outside the set, while the answer
this side builds happily names a format further down the same list — which is the `M-30` defect, and
`negotiation_does_not_settle_outside_the_selected_set` is its regression test.

## 3. The grammar, and what is refused

The value this rule reads is the part of the attribute after the payload type and its space:
`PCMU/8000`, not `0 PCMU/8000`.

    rtpmap-value    = encoding-name "/" clock-rate [ "/" encoding-params ]

**§3.1 — Accepted.** An encoding name of at least one character; a clock rate that is a non-empty
string of ASCII digits fitting in a `u32`; optionally an encoding parameter of the same shape.

**§3.2 — Leading zeros are tolerated, and nothing else is.** RFC 8866 §9's `integer` production
begins at a non-zero digit, so `08000` is strictly ungrammatical. sipx reads it as 8000 anyway.
The reason is that it is unambiguously eight thousand, it is what a zero-padded field in a
configuration generator produces, and declining it would refuse a format the peer plainly named —
so the tolerance fails in the interoperating direction. It is safe to tolerate *because* there is
now one reader: two readers could tolerate it differently, and that is precisely what went wrong.

**§3.3 — Refused, as a typed error.** Every one of these is `RtpmapError` and therefore a
**non-match** at both callers — never a panic, never a raw index, never a wrapped number
([AGENTS.md](../../AGENTS.md) non-negotiable 3). An `a=rtpmap` is network input from a peer that
may be hostile or merely broken.

| Value | Refused because |
| --- | --- |
| `PCMU` | no clock rate; a value without one identifies nothing |
| `/8000` | no encoding name |
| `PCMU/` | the clock rate is empty, which is not zero and not 8000 |
| `PCMU/+8000`, `PCMU/-8000` | a sign is not a decimal digit string |
| `PCMU/ 8000` | surrounding whitespace is not part of the number |
| `PCMU/8_000`, `PCMU/eight` | not digits |
| `PCMU/99999999999999` | larger than a `u32` holds |
| `PCMU/8000/two`, `PCMU/8000/` | the encoding parameter is not a decimal digit string |
| `PCMU/8000/1/9` | a fourth field is outside the grammar. It stays with the encoding parameter, so the value is refused rather than accepted with a field nobody read |

**§3.4 — A value that identifies nothing matches nothing**, including another value that identifies
nothing. `PCMU` and `G729` are not the same format merely because neither carries a clock rate.

## 4. Test vectors

`crates/sipx-sdp/src/rtpmap.rs`'s tests are derived from §4.1–§4.3; `sipx-call`'s
`the_answer_and_the_negotiated_codec_agree` from §4.4.

**§4.1 — Well-formed.** `PCMU/8000` → (`PCMU`, 8000, 1). `opus/48000/2` → (`opus`, 48000, 2).
`X/4294967295` → the largest rate the type holds, read rather than refused.

**§4.2 — Refused.** Every row of §3.3's table, each asserted to be the named error *and* a
non-match from both sides of the comparison.

**§4.3 — Identity.** Matching: `PCMU/8000` ≡ `pcmu/8000` (case), `PCMU/8000` ≡ `PCMU/8000/1`
(default channel count), `PCMU/08000` ≡ `PCMU/8000`, `PCMU/8000/01` ≡ `PCMU/8000`,
`PCMU/0008000/0001` ≡ `PCMU/8000/1`, `opus/048000/2` ≡ `opus/48000/2`. Not matching:
`PCMU/8000` ≢ `PCMA/8000` (name), `PCMU/16000` ≢ `PCMU/8000` (rate), `PCMU/8000/2` ≢ `PCMU/8000`
(channels).

**§4.4 — The agreement.** The property that ties §2.3's two callers together, and the only place
their disagreement is observable end to end. For an offer and a selected codec set:

- if negotiation settles on a codec, the answer must not have rejected the stream, and the payload
  type the session will send on must appear in the answer's `m=` format list;
- if negotiation refuses the stream, the answer must have rejected it (port 0).

A biconditional rather than a one-way check, because both halves are reachable defects. A codec the
answer never named is a session the far end cannot place — and worse in the receive direction, where
decoding the peer's A-law through a µ-law session produces audible garbage rather than silence with
nothing reporting an error. A stream the answer accepted while negotiation refused it is a call that
fails after the 200 OK has gone out.

The payload type compared is the one that goes **on the wire** — the number the description
assigned, or the codec's own static number when nothing remapped it. `Some(0)` and `None` are two
descriptions of the same PCMU and the same byte.

The offers are a table, not an example, because the failure being closed is a rule fitted to the
one input it was tested on. The rows: the leading-zero clock rate; a leading zero in the channel
count; a codec sipx does not carry placed ahead of one it does; a dynamic number carrying a codec
sipx has; a bare static type with no rtpmap at all; mono spelled where it could be implied; stereo;
each refused spelling from §3.3 in a position where the format after it is playable; an Opus-first
offer reaching a call that selected G.711; a dynamic number with no rtpmap; a stream offering only
`telephone-event`; an offer of nothing sipx carries; and — behind the `opus` feature — Opus on the
set that carries it, Opus with a leading zero on its own clock rate, and Opus at a rate RFC 7587 §7
does not assign.

**Three of those rows were live disagreements** when the table was first run, and the fourth is
behind the `opus` feature — which is the argument for a table stated as a measurement rather than a
hope. They were: the leading-zero clock rate (the reported witness), a leading zero in the channel
count, a **signed** clock rate, and with `opus` a leading zero on Opus's own rate. The signed one
was not predicted: `u32::from_str` accepts a leading `+`, so the parsing rule read `+8000` as eight
thousand while the textual rule did not — the same split as a leading zero, arrived at from the
other side. It is why §3.1 checks the digits rather than delegating to `from_str`.

Those four also show why the fix is *one rule* and not a normalisation pass. §3.2 resolves a leading
zero by accepting it, and §3.3 resolves a sign by refusing it — opposite directions. Both callers
follow the rule either way, so the leading-zero offers settle on µ-law at the number the answer
names, and the signed offer settles on A-law further down the list with the answer naming exactly
that. Agreement is a property of there being one reader, not of any particular verdict it reaches.

## 5. What is deliberately *not* unified

**§5.1 — The answer names formats negotiation does not settle on.** The agreement in §4.4 is
one-directional in exactly one respect: `telephone-event` appears in the answer, with its
per-direction `fmtp` event list (RFC 4733 §2.5.1.2), and is never a codec to build a session with.
This is not a disagreement — it is the answer carrying a format that is not a codec. A stream
offering *only* `telephone-event` is rejected by both sides, since DTMF alone is not a call.

**§5.2 — Reading an encoding name is not the same question.** Two places take the text before the
first `/` to recognise `telephone-event`: `sipx_sdp::answer::encoding_of` and
`sipx_call::call::telephone_event_payload_type`. They are not instances of the split this document
closes — no numeric field is compared, and the two agree — so they are left as they are.
Routing them through §3's grammar would also *change behaviour*, by declining a rate-less
`a=rtpmap:101 telephone-event` that today still yields working DTMF. That is a decision with an
interoperability question attached and belongs to a story that argues for it, not to this one.

**§5.3 — A bare static payload type is matched by number, and only then.** With no `a=rtpmap` at
all there is no value for §2.1 to read, and RFC 3551's assignment is what the number means. A
*dynamic* number (96–127) with no rtpmap is uninterpretable whatever the number and is refused —
guessing a codec from a number nobody defined is how a stack decodes somebody else's G.729 as Opus.
