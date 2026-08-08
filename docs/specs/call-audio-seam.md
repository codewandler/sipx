# The bounded call PCM processing seam

**Status:** normative · **Story:** `M-54` · **Epics:** `local-speech`, `call-audio-analysis` ·
**Design:** [local-speech](../designs/local-speech.md) · **Crate:** `sipx-media`
(`session::MediaSession`, `processing`)

Two epics need live call audio and neither may tap the media path itself.
[`speech-providers.md`](speech-providers.md) §1 says "Call-side audio enters and leaves through
M-54's bounded PCM processing seam. This spec does not define a second media tap; it defines what
rides the seam." [`call-audio-processing.md`](call-audio-processing.md) §9 says the same thing from
the other side and forbids a second tap by name. Both delegate to a document that did not exist.
This is that document: **the seam is one, it is per call, it is bounded, and it cannot stall the
media path.**

Where this document and an implementation disagree, this document is right until it is changed
deliberately.

## 1. Scope

The seam attaches application media processors to a running `MediaSession` and delivers each of
them owned linear PCM in a format they chose, with the metadata needed to place every sample on a
timeline: direction, format, sample time, sequence and typed discontinuity.

It defines: the two tap points, the frame, format selection and its typed refusal, the per-call
queue bound and the frame-loss policy, the discontinuity vocabulary and when the seam emits each
kind, fan-out to simultaneous consumers, and the attach/detach/cancel lifecycle.

It deliberately does not define: what a processor computes (`call-audio-processing.md` for the
deterministic analyser, `speech-providers.md` for recognition and synthesis), how observations reach
the application SDK (`M-58`/`M-59`, `A-26`/`A-27`), or any new PCM representation — the boundary is
`linear-pcm.md`'s and this document adds no second one.

## 2. Normative references

- **RFC 3550** §5 — the RTP media clock every sample time derives from. The seam sees decoded
  frames, never packets; RTP sequence numbers and timestamps stop below it.
- **RFC 3551** — the audio profile behind the linear PCM boundary.
- [linear-pcm.md](linear-pcm.md) — the owned mono PCM boundary (`Pcm`, `PcmFormat`,
  `LinearResampler`), the supported rate domain 1..=384,000 Hz and the typed
  `UnsupportedSampleRate` refusal. `M-43`. The seam **reuses** this and mints nothing parallel.
- [media-runtime.md](media-runtime.md) §2.1 and §4 — worker ownership, shutdown, and the rule that
  a discard in the media path is counted or carries its reason. The seam owns no worker (§8), so it
  does not amend §2.1; its frame loss is counted (§6.3).
- [call-audio-processing.md](call-audio-processing.md) — the deterministic processor contract. Its
  §3.1 direction vocabulary, §3.4 sequence and discontinuity rules, §5.1 `queue_capacity` domain and
  §8.3 coalescing loss accounting are inherited here, not restated differently.
- [speech-providers.md](speech-providers.md) — the recognition and synthesis contracts. Its §5
  frame and discontinuity inputs, and its §8 per-session frame-queue bound and drop-oldest policy,
  are inherited here.

No third-party implementation is referenced by this contract, its vectors or its rationale.

## 3. The two tap points, and there are exactly two

```
Direction ::= Inbound | Outbound
```

`Inbound` is audio decoded from the remote peer. Its tap is the **jitter buffer's output, after
decode and immediately before the frame reaches the application receive queue** — the same samples
`MediaSession::recv` and `PcmCapture` see, in the order playback would hear them. Tapping before
the jitter buffer would hand a processor the arrival order rather than the played order, and two
observers of one call would disagree about what happened.

`Outbound` is audio produced locally for transmission. Its tap is **the send loop, after the mute
gate and before encoding** — the samples that actually become RTP. Tapping before the gate would
report muted audio as transmitted (`M-18`), which is a privacy claim and not a timing detail.

Three consequences are normative:

- The seam carries linear PCM and only linear PCM. A **relaying** session (`MediaSession::set_relay`)
  decodes nothing, so it produces no `Inbound` frames, and a leg forwarding an encoded payload
  verbatim produces no `Outbound` ones. This is a stated absence, not a silent one: a processor
  attached to a relaying session receives frames only if and when relay is turned off. Observing a
  relayed leg means decoding it, which is a bridge's decision and not a seam that decodes behind the
  session's back.
- Telephone events (RFC 4733) are not audio and never reach the seam in either direction. DTMF keeps
  its one typed path (`media-runtime.md` §2.2).
- Frames the send loop discards before the wire — an encode failure, a stopped playback — have
  already been offered to the `Outbound` tap, because the tap reports what the session produced.
  A processor is an observer of the call's audio, not an audit of the socket.

## 4. The frame

```
PcmFrame ::= { direction:     Direction,
               pcm:           Pcm,               -- owned, in the attachment's format
               sample_time:   u64,               -- samples at the format's rate, in this epoch
               sequence:      u64,               -- strictly increasing per attachment
               discontinuity: Option<Discontinuity> }

Discontinuity ::= { kind: Loss | Overflow | Realign, frames: u64, samples: u64 }
```

**`pcm`** is owned and carries its own `PcmFormat`, so a processor never infers a rate from a buffer
length and never reinterprets a depth. Ownership is deliberate: the media loop must not wait for a
consumer, so it cannot lend it a buffer.

**`sequence`** starts at 0 for each attachment and counts every frame the seam *offered* to that
attachment, delivered or dropped. A gap in the sequence a consumer receives is therefore exactly the
frames it lost, and the gap is always flagged — an unflagged gap is a defect in this seam, which is
the reading `call-audio-processing.md` §3.4 depends on.

**`sample_time`** is the position of the frame's first sample within the attachment's current
**epoch**, counted in samples at the attachment's own rate. The epoch opens at attachment and
re-opens at every `Realign`. It advances by the delivered sample count of each frame, and across a
gap it additionally advances by the `samples` the discontinuity names — so the timeline never
compresses over loss. Wall-clock time appears nowhere.

**`discontinuity`**, when present, describes the break immediately *before* this frame.

## 5. Attachment and format selection

An attachment is requested with a direction, a `PcmFormat` and a queue capacity:

| Field | Domain | Refusal |
|---|---|---|
| `direction` | `Inbound` or `Outbound` | — |
| `format` | any `linear-pcm.md` format: rate 1..=384,000 Hz, `Unsigned8` or `Signed16` | `UnsupportedConversion`, carrying `linear-pcm.md`'s own `PcmError` |
| `queue_capacity` | 2..=4,096 frames, default 32 | `QueueCapacity` naming the requested value |

The rate domain and its refusal type are **reused, not re-minted** — the refusal a caller sees for
rate 0 or 384,001 is `linear-pcm.md`'s `UnsupportedSampleRate`, the same type its PCM-4 vector
names. The capacity domain is `call-audio-processing.md` §5.1's `queue_capacity` domain; the default
of 32 frames is `speech-providers.md` §8's default recognition input bound. Neither number is
invented here.

Conversion from the session's media clock to the attachment's format is
`linear-pcm.md`'s: one `LinearResampler` per attachment, retaining interpolation history across
frames so packet boundaries create no discontinuity or drift, followed by that boundary's depth
conversion. The seam performs **no other audio decision** — no gain, no channel, no filtering.
A conversion the boundary cannot express is refused at attach time, before any queue is allocated;
the session is never distorted and never dropped to satisfy a processor.

Attachments per session are bounded at **8**. Each one is a queue whose worst case is
`queue_capacity` frames, so an unbounded attachment count would make per-call memory unbounded while
every individual bound held. Eight is both epics' two consumers per direction with headroom. The
ninth is refused with `TooManyProcessors`.

Attaching to a stopped session is refused with `SessionStopped` rather than returning a handle that
can never produce a frame.

## 6. Bounds and the frame-loss policy

### 6.1 The policy

Every queue is per attachment, and attachments are per call: one call's slow consumer cannot consume
another call's budget and no queue is shared mutable state.

Offering a frame to an attachment **never blocks and never allocates beyond the frame**. When the
queue is at capacity:

1. the **oldest** queued frame is dropped — `speech-providers.md` §8's stated behaviour at the
   input-frame bound, "drop oldest", and the right end for audio: the newest frame is the one the
   consumer has the best chance of still being able to use;
2. the loss is **coalesced** into the attachment's pending discontinuity rather than blocking or
   growing the queue — `call-audio-processing.md` §8.3's accounting, applied at the head instead of
   the tail because this queue drops from the head;
3. the frame that is now oldest is marked with that pending discontinuity, so the next frame the
   consumer receives is the frame that follows the gap and it names the gap.

The result is `speech-providers.md` §5's obligation exactly: "the oldest queued frame is dropped and
the driver MUST deliver one `Discontinuity` input with kind `Overflow` naming the accumulated lost
span before the next `Frame`". One discontinuity per gap, however many frames the gap swallowed.

### 6.2 What cannot happen

The offer is a bounded, synchronous, non-awaiting operation. Therefore RTP decode, RTP encode,
playback and capture are never blocked by a processor, however slow or however stopped it is. This
is the guarantee `speech-providers.md` §5 restates as a driver obligation and attributes here.

### 6.3 Accounting

Dropped frames are counted twice, in two views of one fact: per attachment, as a monotonic count the
consumer can read, and per session, as `MediaDiscardCounts::processor_frames_lost`
(`media-runtime.md` §4). Neither is a substitute for the discontinuity on the frame — a count says
how much was lost and the flag says where.

## 7. Discontinuity kinds

| Kind | The seam emits it when | `frames` | `samples` |
|---|---|---|---|
| `Loss` | upstream audio never became a frame: the decoder refused a packet | frames lost | their span in the attachment's rate |
| `Overflow` | §6.1's bounded-queue policy dropped queued frames | frames dropped | their span in the attachment's rate |
| `Realign` | the seam re-anchored the timeline: the session's workers were replaced by renegotiation, or its media clock changed | frames discarded at the re-anchor | 0 — the epoch restarts rather than skipping a measurable span |

The vocabulary is closed and shared: it is `call-audio-processing.md` §3.3's, and
`speech-providers.md` §9 marks it extended compatibly, so a consumer writes a wildcard arm.

**A lost span in the attachment's rate.** A span of `n` samples at the session's media clock is
`n · target_rate / source_rate`, in `u64`, truncated. `sample_time` advances by exactly that, so a
consumer that adds spans and delivered lengths reconstructs the epoch position of every frame.

**Coalescing.** At most one discontinuity is pending per attachment. When several causes accumulate
before the next frame is delivered, their `frames` and `samples` add and the `kind` names the most
disruptive cause present, in the order `Realign` > `Overflow` > `Loss`: `Realign` subsumes a span
because it restarts the epoch, and the seam's own loss is named ahead of upstream loss because it is
the one the consumer can act on.

**`Realign` discards the queue.** Frames queued under a media generation that no longer exists would
land in the new epoch's timeline as old audio at a new position. They are dropped, counted, and
named by the `Realign` that opens the new epoch.

## 8. Fan-out and lifecycle

**Fan-out is by independent copy.** Each attachment has its own queue, its own resampler, its own
sequence and epoch counters and its own loss accounting. Two simultaneous consumers — a speech
provider and a deterministic analyser — therefore observe the same audio without observing each
other: one falling behind loses its own frames and no one else's, and one requesting 16 kHz signed
16-bit alongside one requesting 8 kHz unsigned 8-bit costs each of them only their own conversion.
No state is shared between two attachments, between two directions, or between two calls.

**The seam owns no task.** Delivery is the media loop's existing work and consumption is the
caller's; nothing is spawned, so `media-runtime.md` §2.1's owner set is unchanged and there is no
new handle for shutdown to join.

| Transition | Effect |
|---|---|
| attach | registers the queue; returns the consumer handle; the epoch opens at sample time 0 |
| detach, explicitly or by dropping the handle | deregisters the queue and releases its frames at once; the session is otherwise unchanged |
| session `stop()`, `shutdown()` or drop | closes every attachment; each drains what it already holds and then completes |
| processor failure — a consumer that stops consuming and never detaches | §6.1's policy; the attachment loses frames and the session is unaffected. Its buffering stays at its stated capacity forever |
| renegotiation (`reconfigure`) | attachments **survive** the generation change and are re-anchored (§7, `Realign`); a consumer does not have to re-attach across a re-INVITE |

**Completion is observable, and it is an event.** A closed, drained attachment's receive completes
with "no more frames" rather than idling, so a caller that has drained one knows the seam is done.
Nothing in this contract may be observed by waiting a fixed duration.

## 9. Vectors

Unless a vector says otherwise it runs on `S8`: one `MediaSession` at G.711 µ-law, media clock
8,000 Hz, 20 ms packets (160 samples), one `Inbound` attachment at 8,000 Hz `Signed16` with the
default capacity of 32.

| ID | Input | Expected |
|---|---|---|
| SEAM-1 | one 160-sample packet arrives; drain the attachment | one frame: `direction Inbound`, format 8,000 Hz `Signed16`, `sequence 0`, `sample_time 0`, no discontinuity, 160 samples |
| SEAM-2 | `S8` with an `Outbound` attachment; send one 160-sample frame | one frame: `direction Outbound`, `sequence 0`, `sample_time 0`, 160 samples equal to what was sent |
| SEAM-3 | `S8` with the attachment at 16,000 Hz `Signed16`; two adjacent 160-sample packets | the two frames together carry the same samples one combined `LinearResampler` push would, and the second frame's `sample_time` equals the first's delivered length |
| SEAM-4 | attach at rate 0, then at 384,001 | both refused `UnsupportedConversion` carrying `UnsupportedSampleRate`; no queue is allocated and the session keeps running |
| SEAM-5 | attach with `queue_capacity` 1, then 4,097 | both refused `QueueCapacity` naming the value |
| SEAM-6 | `S8` with `queue_capacity` 2; offer 5 frames without draining; then drain | exactly 2 frames; the first carries `Overflow { frames: 3, samples: 480 }` and `sequence 3`; the second `sequence 4` and no discontinuity |
| SEAM-7 | SEAM-6, then read the per-attachment loss count and `discard_counts()` | both report 3 lost frames |
| SEAM-8 | one attachment at 8,000 Hz and one at 16,000 Hz on the same direction; one packet | each receives one frame in its own format; neither's sequence, epoch or loss is affected by the other |
| SEAM-9 | two attachments, capacity 2 on one and 32 on the other; offer 5 frames; drain both | the small one reports `Overflow`; the large one reports none and has all 5 |
| SEAM-10 | attach, then drop the handle, then offer frames | the session accepts and delivers audio as before; no frame is retained for the dropped attachment |
| SEAM-11 | attach, offer 2 frames, `stop()` the session, then drain | the 2 frames are delivered, then the receive completes with no more frames |
| SEAM-12 | attach, `reconfigure()` the session to a different codec, then offer a frame | the attachment is still registered; the next frame carries `Realign` and `sample_time 0` |
| SEAM-13 | attach 8 times, then a ninth | the ninth is refused `TooManyProcessors`; the eight remain live |
| SEAM-14 | attach to a stopped session | refused `SessionStopped` |
| SEAM-15 | `S8` relaying; a packet arrives | no `Inbound` frame; the encoded path is unchanged |
