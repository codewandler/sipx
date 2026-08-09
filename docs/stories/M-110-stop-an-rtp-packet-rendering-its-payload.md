---
id: M-110
title: Stop an RTP packet rendering its payload
pillar: Media
status: done
priority: 6
design:
epic: media
areas: [sipx-rtp]
predicate:
announcement:
note: after M-107 · Packet, Rtcp and SdesItem derive Debug over Bytes; M-107 redacted Encoded at the other end of the same relay path
---

# Stop an RTP packet rendering its payload

## Goal

Give `sipx_rtp::Packet` — and the RTCP types beside it that hold `Bytes` — a `Debug` that carries
what a protocol log needs and not the call it is carrying.

## Why

`M-107` closed this on `sipx_media::Encoded`, which is the *same payload* one layer up: its
derived `Debug` rendered every octet, and for G.711 an octet is a sample. `sipx_rtp::Packet` still
does. So does `Rtcp`, and so does `SdesItem` — whose `Bytes` is a CNAME or an end-user's name and
email address (RFC 3550 §6.5), which is not audio but is personal data with the same problem.

Fixing one end of a relay path and leaving the other is the mistake this pair has already made
once in the other direction: `M-80`'s note records marking `Encoded` and `Packet`
`#[non_exhaustive]` **together**, "because they are the two ends of one relay path and marking one
would have left the other free to break the same caller in the same way for the same reason". The
same sentence applies here and `M-107` could not act on it, because `sipx-rtp` was outside the
story's crates.

**It is not enforceable the way `M-107`'s half is, and that is the point of a separate story.**
`scripts/check-audio-claims.py` recognises a sample buffer by its element type — `Vec<i16>`,
`[i16; N]` — and holds every published crate to it. A payload is `Bytes`, which in this workspace
is equally a `Call-ID`, a SIP body, a URI and a STUN attribute: around sixty reachable public types
hold one, and for nearly all of them rendering the bytes is what a protocol log is *for*. So this
is decided type by type with an argument each time, which is a review and not a rule.

**That paragraph is half right, and the wrong half is the delivery.** It is true that no *element
type* can decide a `Bytes`, and the sixty are real — 58 reachable public types hold a byte buffer.
What it missed is that they are spread across eleven crates. Narrowing by **scope** instead, to the
two crates the call's own bytes pass through as bytes, leaves eight carriers, every one of which a
person can decide once and a checker can hold afterwards. See `## Progress`.

## Acceptance

- [x] A failing-first test renders a `Packet` carrying a distinctive payload and asserts no payload
      octet appears in the output, in decimal or in hex.
- [x] `Packet`'s `Debug` carries payload type, sequence, timestamp, SSRC, marker and the payload's
      **length**, and its length is bounded independently of the packet's.
- [x] `Rtcp` and `SdesItem` are decided on the same terms and each is either fixed or carries the
      reason its rendering is safe. `SdesItem` is the one to argue rather than assume: a CNAME is
      an identifier a log legitimately correlates on, and an RFC 3550 §6.5 `NAME`/`EMAIL` item is
      not.
- [x] Whether the header extension's bytes belong in a record is answered either way — they are
      RFC 8285 metadata rather than media, and `M-107` rendered only their length at `Encoded`.
- [x] `./scripts/gate.py` green.

## Progress

**2026-08-09 — implemented on `impl/M-110`.**

*Failing-first, at merge base `413f19c`.* `crates/sipx-rtp/tests/payload_diagnostics.rs`, six tests,
five red:

```
$ cargo test -p sipx-rtp --test payload_diagnostics --all-features
a payload octet survived as `a7` in: Packet { marker: true, payload_type: 96, sequence: 1000,
  timestamp: 987654, ssrc: 3735928559, csrc: [286335522, 858997828],
  extension: Some(b"\xbe\xde\0\x01\xc3\xc3\xc3\xc3"), payload: b"\xa7\xa7\xa7…" (160 octets) }
an RFC 3550 §6.5 item's value is personal data and stayed in the record:
  SdesItem { kind: 1, value: b"alice.smith@host.invalid" }
a participant's identity reached a record through the packet: Sdes(Sdes { chunks: [SdesChunk {
  ssrc: 286335522, items: [SdesItem { kind: 1, value: b"alice.smith@host.invalid" }, … ] }] })
an unmodelled packet's body octet survived as `~~~~` in: Other { packet_type: 203, count: 1,
  padding: false, payload: b"~~~~~~~~…" }
a record whose length is the packet's is an unbounded diagnostic: 670138 octets
test result: FAILED. 1 passed; 5 failed
```

The sixth, `a_receiver_report_still_renders_its_numbers`, passed at the base by design: it pins the
decision *not* to redact report blocks, so that somebody generalising the redaction has to argue
with a test rather than with a comment.

*Two sensitivities, kept apart.* `Packet::payload` is the call still encoded — for G.711 one octet
per sample — so its `Debug` renders payload type, sequence, timestamp, SSRC, marker and lengths.
`SdesItem::value` is not audio at all: RFC 3550 §6.5 makes it a `CNAME`, `NAME`, `EMAIL`, `PHONE`
or `LOC`, which is personal data about a participant. The CNAME exception was argued and rejected,
for three reasons recorded at the implementation. §6.5.1's recommended form is `user@host` derived
from a login name, so "an identifier rather than personal data" is false of the shape the RFC asks
for. The `kind` octet that would select the exception is chosen by the far end. And the value is
uncapped `Bytes` in memory, so a kind-dependent rendering would need its own truncation — a
redaction bounded except for the one kind we print is the half-fix this story exists to avoid.
Correlating on a CNAME stays a field access away.

*`Rtcp`.* Hand-written only because of `Other`, whose body is a packet this crate does not model —
§6.6's free-text leaving reason, §6.7's application data — so its length is rendered and not its
bytes. `Sender` and `Receiver` delegate unchanged: a report block is seven integer counters and is
the entire reason RTCP exists. `Sdes` is safe by composition once `SdesItem` redacts.

*Header extension.* Rendered as a length, matching `Encoded`. Two reasons beyond consistency: this
crate never interprets an extension, and RFC 7941 puts SDES items — `CNAME` included — in exactly
that place, so an extension can carry the personal data `SdesItem` redacts.

*Bounded.* Every variable-length member of `Packet` is a count, the contributing-source list
included: the wire caps it at fifteen through the `CC` nibble and the `Vec` does not.
`a_packet_record_is_bounded_independently_of_the_packet` builds a 100,000-octet payload, a
40,004-octet extension and 10,000 CSRCs and holds the record under 200 octets — it was 670,138 at
the base.

*Enforced rather than reviewed — the fourth question, answered yes.* `M-107` concluded no checker
could decide a `Bytes`. That is right about element types and wrong about scopes.
`scripts/check-audio-claims.py` gains `byte_buffer_problems` over `RELAY_PATH` — `sipx-media` and
`sipx-rtp` — where a reachable public type holding a byte buffer implements `Debug` or carries an
adjacent `/// Not the call:` rationale. Run against the merge base it reports six types: `Packet`,
`Rtcp`, `SdesItem`, and ICE's `Input`/`Output` and STUN's `Attribute`, which now carry the argument
`M-107` had recorded only in a commit message. Population 8; the 50 carriers outside the scope are
counted and printed on every run.

The doc-vocabulary rule the dispatch suggested — select on a field whose documentation says "media"
or names an identifier — was evaluated and not built. It has to separate `Packet::payload` ("The
media.") from `ContentType::media_type` and `Address::display_name`, so it either reports the SIP
surface or is quiet; and it is switched off by rewording, which is the one narrowing that fails
silently. The crate scope needs no vocabulary at all.

*Loudness.* `_IMPLEMENTS_DEBUG` is shared with the sample rule, so a reader blind to a hand-written
`Debug` reports all eight carriers. `_BYTE_BUFFER` is the selector that fails by selecting nothing,
so `unread_relay_path` holds the population to a floor of four. `NOT_THE_CALL_REASON` is a **fifth**
phrase rather than a reuse of `/// Not call audio:` — that phrase is *true* of an `SdesItem`, and a
redaction check a true sentence can switch off checks nothing. Both directions are tested.
`RELAY_PATH` is a separate constant from `MEDIA_SURFACE` although the two name the same crates
today: one is a rollout boundary meant to be widened, the other a stated scope whose widening would
report forty SIP headers, and a test asserts that widening the first does not widen the second.

*Filed.* `M-117`, for four carriers outside the scope the count was hiding, found by reading the
fifty: `sipx_app_protocol::Source::Inline` ("PCM carried in the document itself"),
`sipx_testkit::Record` and `ClientEvent` (call audio), and `sipx_ua::Authenticator`, whose derived
`Debug` prints the `[u8; 32]` key every self-describing nonce is MACed with.

*Not done, deliberately.* No spec clause. `call-audio-seam.md` §2 says "the seam sees decoded
frames, never packets", so a normative sentence about `sipx_rtp::Packet` there would contradict the
document's own scope, and no spec owns this crate's public API. The rule and its argument live at
`check-audio-claims.py` instead, which is where the dispatch asked for them.

*Owed `CHANGELOG.md` sentence*, for whoever integrates this — the file is fenced from this branch:

> `sipx_rtp::Packet`, `Rtcp` and `SdesItem` no longer render their bytes in a `Debug` record. A
> packet reports its header and its payload's length, and an RFC 3550 §6.5 source-description item
> reports its type and its value's length rather than the `CNAME`, `NAME` or `EMAIL` it carries.
> `scripts/check-audio-claims.py` enforces this over `sipx-media` and `sipx-rtp` (`M-110`).

*Board.* `status` is `in-progress` and `M-117` is new, so `docs/stories/README.md` needs
regenerating at integration; it is fenced from this branch.

- 2026-08-10: closed at the `1.0.0-rc.18` boundary, against the wave gate run on this tree.
