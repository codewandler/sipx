# Design: video admission

**Status:** resolved — video is **not admitted**; the [vision](../vision.md) is unchanged ·
**Pillar:** Media · **Epic:** `video` · **Stories:** `M-40`

**Decision:** not admitted. sipx carries one audio media section and no video codec, SDP profile,
packetizer, runtime or public support claim. The `video` epic closes at maturity level **0
(proposed)**. No video implementation story may be filed `ready`, and no public surface may claim
video, until this record is replaced by an admitted outcome that also amends `docs/vision.md`.

## Why this record exists

`M-40` is an admission gate, not an implementation story. The vision names video a non-goal, so the
cost of reversing that had to be measured before any video code could enter the workspace — and the
measurement had to be recorded either way, so that a later reader can tell a decision from an
omission.

Three questions decide it: is there demand, what would a second media type cost, and does the
project's own release gate permit taking that cost now. All three answer the same way.

## Demand: measured at or near zero

The project owns one demand instrument, the survey recorded in [`demand.md`](demand.md). It puts
video in the negative-signal list (`docs/designs/demand.md:26-30`):

> G.729, AMR, iLBC, T.38, **video**, answering-machine detection, voice activity detection, CDR and
> OpenTelemetry are all at or near zero.

Everything the survey found demand *for* is audio and is already scheduled — address advertisement
and RTP latching (`M-42`), an unopinionated linear-PCM boundary (`M-43`), G.722 (`M-44`), a jitter
buffer (`M-45`). The later direct product requests (`demand.md:89-98`) are three more audio epics:
local speech, call-audio analysis, custom call DSP.

Corroborating searches over the whole tree found no request from any other channel:

| Channel | Result |
|---|---|
| Five reports in `docs/reviews/` | One mention, endorsing the exclusion as an honest boundary. No reviewer names video as a gap. The 2026-08-04 competitive review lists its genuine capability gaps — a relayed ICE candidate and the AEAD SRTP profiles — and video is not among them. |
| `docs/comparison/` | No video dimension exists, so no subject is compared on it. sipx's own generated cell says it "is not a general browser, TURN or video stack". |
| Downstream consumers (`sipx-app`, the app contract, the realtime bridge, 17 example binaries) | No video-shaped requirement. The realtime session contract is `"output_modalities": ["audio"]`. |
| The browser SDK contract (`docs/specs/browser-sdk.md:797`) | Video is refused *by contract* — an INVITE whose offer carries a video section receives an automatic 488. The SDP half of that refusal is already a running test: `crates/sipx-sdp/tests/browser_audio_profile.rs:378` offers a video section alongside the audio one and requires a typed error. |

The survey's own caveat (`demand.md:79-82`) is that it measures another user base, so "at or near
zero" is a borrowed, lagging number. That weakens it as proof of *absence* — but the project has no
evidence of *presence* from any source at all, and a capability with no requester on either side of
the ledger does not clear an admission gate.

## What one bounded send-and-receive profile would cost

Measured against the tree at `1.0.0-rc.3`. The workspace is 12 crates and 196,488 lines of Rust with
2,342 test functions; the local gate is 39 steps.

### Reused from the `webrtc-audio` epic

Real, and larger than it looks. [`webrtc-audio.md`](webrtc-audio.md) already composes the transport
half of any browser-facing video path, and none of it is audio-specific:

- SIP over secure WebSocket, the ICE agent and nomination, the DTLS-SRTP handshake with fingerprint
  verification, SRTP/SRTCP protection, and RTP/RTCP multiplexing (RFC 8834's profile requirements).
- The SDP AST is already media-type agnostic: `MediaDescription.media` is a `String` documented as
  "`audio`, `video`, `application`…" (`crates/sipx-sdp/src/session.rs:223`), and `rtpmap.rs` parses
  `H264/90000` today.
- The answer engine already honours RFC 3264 §6's correspondence rule — same number of `m=` lines,
  same order, a refused stream answered with port 0 — and there is a passing test that offers a
  video section and asserts it is declined in place (`crates/sipx-sdp/src/answer.rs:7-10` and
  `:517-540`).

So the *signalling shell* is largely pre-paid. Nothing below it is.

### Video-specific state, per seam

| Seam | What exists today | What video adds |
|---|---|---|
| SDP policy | Three lines decline everything that is not audio (`crates/sipx-sdp/src/answer.rs:240-244`). `Capabilities` is scalar: one `audio_port`, one `audio_formats` list. | `Capabilities` becomes a per-section list; `a=rtcp-fb`, `a=fmtp` (RFC 6184 §8.1 `profile-level-id` and `packetization-mode`; RFC 7741's descriptor parameters), `a=imageattr`, `a=group:BUNDLE` and `a=mid` all become load-bearing. RFC 9429's offer/answer rules become normative for a browser-facing profile. |
| Browser profile | Normatively **one** section: "An offer with a second media section — including a rejected or bundled section — is outside this profile" (`docs/specs/webrtc-audio.md:88-93`), enforced by a length-one slice pattern (`crates/sipx-sdp/src/browser_audio.rs:236-238`). | Video does not extend the delivered profile; it invalidates it. A second, separately specified and separately proved profile is required, plus real BUNDLE handling the current spec explicitly ignores. |
| Offer/answer state | `Negotiated` is a `Copy` scalar — one remote, one codec, one payload type (`crates/sipx-call/src/call/offer_answer.rs:146-173`). Eleven `media == "audio"` filters in crate sources, plus positional `media.first()` reads in the ICE path (`crates/sipx-call/src/call/ice.rs:107,214`). | Every one becomes index-correlated against RFC 3264 §6 ordering. A positional read that silently picks section 0 is a call where video arrives on the audio port — the exact failure `answer.rs` was written to prevent. |
| Call binding | One `MediaSession` and one `MediaPort` per call. | A collection keyed by m-line index; `Call::media()` cannot stay singular. Hold, re-INVITE and UPDATE become per-section. |
| RTP packetization | 4,504 lines of `sipx-rtp` with **no MTU constant, no fragmentation and no aggregation**; header extensions are parsed on receive and dropped on re-encode. | RFC 6184 §5.7/§5.8 (STAP-A, FU-A) or RFC 7741 §4.2 (payload descriptor, partition boundaries), plus reassembly with frame-completeness and marker-bit semantics. New subsystem. |
| Frame timing | "**The clock lives in one place.** Audio is paced by one interval timer at the packetisation interval" (`crates/sipx-media/src/lib.rs:9-12`); the send loop is one dequeued frame → one packet → one 20 ms tick (`session.rs:2969-2970`). Codec clock rates are 8 000–48 000 Hz sample clocks (`session.rs:195-203`). | Inverts that invariant: one video frame is *N* packets sharing one 90 kHz timestamp, at a variable frame rate, with bursts an order of magnitude above the mean. The pacer, the `Frame` enum (`session.rs:557-592`, whose variants are `Audio { samples: Vec<i16> }`, `Dtmf`, `Encoded`) and `samples_per_packet` arithmetic are all the wrong shape. |
| Buffering | Jitter depth is counted in **packets** — default 3, max 12 (`session.rs:465-466`) — and `SHRINK_AFTER = 250` is documented as "five seconds at the usual 20 ms" (`crates/sipx-rtp/src/jitter.rs:33`). | Frame-complete assembly keyed by timestamp, with a discard policy. A three-packet buffer discards every keyframe. |
| RTCP feedback | Four packet types exist: 200–203 (`crates/sipx-rtp/src/rtcp.rs:22-29`). No PLI, FIR, NACK or `rtcp-fb` anywhere. The report interval is a flat 5 s, justified because RFC 3550 §6.2's arithmetic always lands at the minimum for a two-party call (`session.rs:467`). | RFC 4585's feedback modes and AVPF scheduling, its payload-specific messages (PLI, SLI, RPSI), RFC 5104's codec-control commands (FIR, TMMBR), and a keyframe-request path from depacketizer to encoder. A PLI delivered on a five-second timer is not a PLI, so the timing rules must be implemented, not approximated. |
| Congestion response | None. Audio is constant-rate at 64 kbit/s and needs none. | RFC 8834 requires congestion control for WebRTC media. A rate controller, a bandwidth estimate and an encoder that can be told to slow down are all new, and would exist for video alone. |
| Codec integration | `Codec` is a closed five-variant enum behind private closed matches — not a trait, not pluggable (`crates/sipx-media/src/session.rs:69-96`). `sipx-audio` is 2,308 lines of telephony primitives. | A new crate. And the codec cannot follow the existing pattern: "Codecs are pure Rust by default. Opus lives behind the `opus` feature because it binds to C" (`crates/sipx-audio/src/lib.rs:4-5`). See the boundary section below. |
| Security | SRTP/SRTCP reuse cleanly, but the context is per-stream and single-owner by design (`session.rs:2962-2964`). | A second SSRC, context and rollover counter per call. More seriously, a video decoder introduces decompression and resource-exhaustion classes the project has never carried: attacker-chosen resolution, frame rate and reference structure all allocate. |
| Packaging | `Codec`, `MediaProfile`, `Capabilities`, `Codecs`'s fixed `[Option<CodecPreference>; 5]` and `Call::media()` are all exhaustive or scalar. `A-9`'s guard covers public error enums only. | Each is a breaking change to a published crate, unguarded by the gate. |
| Independent-peer proof | 1,647 lines of native-browser harness typed to audio — `kind === "audio"` stats and `totalAudioEnergy` for non-silence (`tests/browser-audio/peer.js:265-266,387`) — plus its own adversarial self-test. Container interop has two peers, only one of which can answer a call. | A second harness with a different liveness metric entirely (decoded/keyframe counts, frame dimensions, freeze counts), a deterministic non-camera source, per-frame identity checking, and negatives for keyframe request, midstream resolution change and decoder limits. One peer that can play the role is one reading of the RFCs, which `tests/interop/README.md` already says is not a consensus. |

Summarised: the transport shell is reused, and **every layer that makes video *video* is new** —
packetization, feedback, congestion, buffering, codec and proof. That is not an increment on the
media crate; it is a second media stack beside it.

## The codec and profile boundary

The story requires this be resolved without assuming an encoder or decoder is free to ship merely
because an RTP payload format is specified. It is not, and this is the decisive technical finding.

- RFC 6184 specifies how to carry H.264 in RTP. RFC 7741 specifies how to carry VP8. Neither
  supplies a codec; both reference separate bitstream specifications.
- RFC 7742 §5 requires a WebRTC browser to implement **both** VP8 and H.264 constrained baseline, so
  a browser-facing profile cannot reduce its obligation by picking one. A SIP-only profile could
  pick one, and H.264 with `packetization-mode=1` would be the choice — but that only narrows the
  packetization work, not the codec problem.
- The codec problem is a workspace invariant, not a preference. `unsafe_code` is forbidden
  workspace-wide and the north star is that no network peer can cause a panic. Every practical
  video encoder/decoder is either a C library reached through FFI — which is `unsafe`, and which the
  Opus precedent already shows must then live behind a feature — or a large decoder whose hostile
  input surface dwarfs everything the project currently fuzzes. Admitting video means admitting that
  surface, and a decoder is the single most attacked component in any media stack.

So the resolved boundary is: **there is no initial codec.** A profile cannot be scoped until the
project decides how it will own a decoder under its own no-panic, no-`unsafe` rules, and that
decision is larger than `M-40`.

## Budgets

No video code exists, so these are **admission thresholds**: the numbers a future video profile must
meet *before* it is written, not results. They are anchored on the audio envelope measured from the
repository's own bounded workloads.

Measured 2026-08-08 on x86_64 Linux 6.6, unoptimized profile, from committed tests that use real
sockets, real G.711 encode/decode and the real 20 ms pacer:

| Workload | Wall | CPU | Peak RSS |
|---|---|---|---|
| `cargo test -p sipx-media --test conference` (9 tests, multi-leg mixing) | 3.28 s | 0.17 s (5% of one core) | 11.8 MiB |
| `cargo test -p sipx-media --test bridge` (7 tests, four sessions each) | 2.52 s | 0.04 s (2% of one core) | 11.8 MiB |

These are envelopes for the whole process including startup, not steady-state per-call rates; they
are cited for order of magnitude. Derived per-call audio, from the constants above: G.711 at 20 ms is
50 packets/s and 8,600 B/s including the RTP header — 68.8 kbit/s per direction, constant, with a
320-byte queued frame and 60 ms of default jitter latency.

| Budget | Audio today | Video admission ceiling |
|---|---|---|
| Bitrate, per direction | 68.8 kbit/s, constant | ≤ 512 kbit/s, rate-controlled |
| Packet rate | 50/s, constant | ≤ 60/s mean; peak within one frame interval declared and bounded |
| CPU, per call per direction | whole bounded suite at 2–5% of one core | ≤ 25% of one core at the ceiling, measured the same way |
| Memory, video state per call | whole bounded suite at 11.8 MiB peak RSS | ≤ 16 MiB, including frame assembly, reference frames and decoder |
| Resolution | n/a | ≤ 640×360; anything larger is a typed refusal at negotiation, never a runtime discovery |
| Frame rate | n/a | ≤ 15 fps nominal, 30 fps hard cap |
| Receive queue | 3 packets default, 12 max | ≤ 2 frames or 256 packets, whichever is smaller; overflow discards, never grows |
| Feedback latency | one RTCP report per 5 s | PLI emitted within 100 ms of loss detection; keyframe restored within 500 ms under 5% loss |
| Recovery | n/a | a decode failure stalls at most one frame interval, allocates nothing, and never terminates the audio section |

Every one of those ceilings is a step change against the audio envelope, and the CPU and memory rows
are the reason the vision's phrase "latency and simplicity budget" is not rhetorical.

## Evidence standard, if this is ever reversed

"A picture appeared" is not evidence, and neither is a decoded frame count on its own. An admitted
profile's first public claim requires, in both offer and answer roles, against an independently
implemented peer:

- decoded-frame **identity** against a deterministic synthetic source — per-frame hashes, not a
  camera — and presentation **timing** within a declared tolerance;
- the same run under impaired transport: loss, reordering and delay variation, with the recovery
  budgets above asserted rather than observed;
- negative cases that fail closed: malformed payload, oversized or unsupported codec parameters,
  resolution above the ceiling, a midstream resolution change, a keyframe request, and cancellation
  mid-frame;
- no regression in the audio path measured by the same bounded workloads used above.

Browser compatibility stays unclaimed until the combined audio/video session the profile advertises
is independently proved, on top of `M-38`.

## Release-gate reasons, which are independent of cost

Even if the cost were acceptable, the project's own `1.0.0` predicates say not now
(`docs/roadmap.md:685-690`):

- Predicate 3 — "**The public API has been used from outside this repository**, by at least one
  application nobody here wrote" — is not met. Adding a second media type would reshape `Codec`,
  `MediaProfile`, `Capabilities`, `Codecs` and `Call::media()` for a capability that no outside user
  has asked for, in an API that no outside user has yet exercised.
- Predicate 4 requires "at least one instance of a change being shaped by the contract rather than
  the contract being edited to fit the change." Refusing video is that instance, recorded here.

The vision's own tie-breakers agree: "Maximum feature count" is a declared non-goal, and "a smaller
stack whose every path is tested beats a larger one whose edges are guesswork."

## The vision is unchanged

`docs/vision.md:52-53` stands as written:

> - **Video.** The media layer is built for telephony audio. Video would compromise the latency and
>   simplicity budget without serving the north star.

Recorded so a future admission has a pre-identified target rather than a silent edit: **that is the
sentence an admitted outcome must change**, together with the "A WebRTC stack" non-goal above it if
the admitted profile is browser-facing. Any admission must edit those lines explicitly, in the same
change that files the normative spec — never by a story quietly shipping a codec.

## What would reverse this

The decision is falsifiable. Re-open `M-40` when **any two** of the following hold, or at the first
`1.0.0` promotion review, whichever comes first. None of them is "someone thinks it would be nice."

1. **Named demand from sipx's own users.** At least three independent, named, still-open requests for
   SIP video, recorded in [`demand.md`](demand.md) at the same grain as its existing entries — not a
   borrowed survey, and not one requester counted three times.
2. **v1 predicate 3 is met and the report names video.** An application outside this repository uses
   the public API and records video as a blocker (`docs/roadmap.md:685`).
3. **The decoder problem has an answer.** A video decoder for an RFC-specified payload format can be
   integrated without `unsafe` and without FFI, *or* the project explicitly amends the workspace's
   no-`unsafe` non-negotiable with the same deliberation this record used.
4. **Congestion control exists for another reason.** A rate controller and bandwidth estimate are in
   the tree for an audio purpose, so RFC 8834's congestion requirement is no longer a video-only
   cost.
5. **RTCP feedback exists for another reason.** RFC 4585's scheduling and feedback messages are
   implemented for audio (for example generic NACK), so RFC 5104's codec control is an increment
   rather than a new subsystem.
6. **Multi-section state is pre-paid.** The one-section rule in `docs/specs/webrtc-audio.md:88-93` is
   relaxed for another purpose — a second audio section, or real BUNDLE — so per-section
   `Capabilities`, indexed `Negotiated` state and per-section call binding already exist.

Conversely, this record should be re-read and *strengthened* if the audio backlog the demand survey
did find — `M-42` through `M-45`, the speech and analysis epics — ships and produces its own users
without any of them asking for video.

## Consequences

- The `video` epic stays at maturity level **0 (proposed)** in [`roadmap.md`](../roadmap.md). The
  four levels above it are not backlog debt.
- No RFC registry rows change. This record cites RFC 4585, RFC 5104, RFC 6184, RFC 7741, RFC 7742 and
  RFC 9429 as the requirements a video profile *would* have to meet; none is implemented, and citing
  them here is not a support claim.
- The public boundary statements that already exclude video — the README, the release notes, the fit
  guide, the browser SDK contract — remain correct and need no change.
