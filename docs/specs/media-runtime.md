# Media runtime construction and ownership

**Status:** normative · **Stories:** M-32, M-35, M-36, M-37, M-45, P-15, M-71

## 1. Scope

This specification defines the boundary at which negotiated media becomes asynchronous work. It
covers session timing validation, codec construction, and conference worker ownership. RTP and RTCP
packet syntax remain defined by RFC 3550; Opus payload identity remains defined by RFC 7587.

The central invariant is transactional startup: validation and codec construction either complete
before any worker is spawned, or startup returns a typed error and leaves no worker or socket alive.

## 2. Media-session startup

`Config` is valid only when both of these conditions hold:

| Field | Valid values | Error |
|---|---|---|
| `packet_duration` | at least 1 millisecond | `SetupError::PacketDurationTooShort` |
| `rtcp_interval` | `None`, or at least 1 millisecond | `SetupError::RtcpIntervalTooShort` |

The one-millisecond floor is also the resolution used to derive samples per packet. Accepting a
positive sub-millisecond value would pass a timer check while deriving an empty audio frame.

All public session-start paths MUST perform these checks before binding a new socket or spawning a
worker. A previously bound `MediaPort` is consumed on either success or failure, so an error releases
its sockets.

After timing validation, startup constructs the negotiated codec's encoder and decoder. Both MUST
succeed before the send, receive, playback, ICE, or RTCP workers are spawned. Construction failure is
reported as `SetupError::Codec`, including the negotiated codec and whether the encoder or decoder
failed. The diagnostic MUST NOT contain media, SRTP keys, or DTLS key material.

For a dynamic payload type, the codec named by negotiation and the bytes carried under that number
are one contract. In particular, a failed Opus construction MUST NOT install a G.711 codec under the
negotiated Opus payload type. RFC 7587 §7 assigns Opus no static payload type, so substitution based
only on the number is never valid.

### 2.1 Media-session shutdown

A running `MediaSession` owns the completion handle for every worker it starts: RTP sending, RTP
receiving, queued playback, each enabled RTCP loop, and the ICE driver when ICE is active. A browser
session adds its component and ICE supervisors to the same bounded owner set. No spawn in session
construction may discard its handle.

`stop()` is the synchronous cancellation signal. `shutdown()` sends that durable signal, closes a
browser ingress when one exists, and joins every handle before it returns. A handle remains in the
owner set while its join is pending, so cancellation of `shutdown()` cannot detach it: a later call
resumes the same drain. `Drop` signals cancellation and aborts handles that were not explicitly
joined; it makes no synchronous-join claim. The same retention rule applies across reconfiguration:
the replacement session owns the stopped generation until every old handle joins, and a cancelled
`reconfigure()` is resumed by the next reconfiguration or shutdown.

An answering `Call` likewise owns the handle for its RFC 3261 §13.3.1.4 successful-final-response
retransmitter. Its stop signal is latched: stopping before the task's first poll, while it waits for
T1, or while a response handoff is pending all select the same durable cancellation state. ACK,
remote BYE, local teardown and answer-setup failure cancel and join that handle. A terminal call
path then joins its active and retired `MediaSession` generations before returning. Therefore a
load responder's joined per-dialog task is a complete barrier for the generated-media call beneath
it; zero outer tasks cannot be reported while an RTP, RTCP, ICE, playback or final-response
retransmission worker remains live.

Replacing a media session during renegotiation performs the same explicit shutdown on the old
session. The confirmed `Call` retains the old `Arc`, and in-place `MediaSession::reconfigure`
retains the old generation, before either begins an await. Merely swapping and relying on a local
destructor would let cancellation abort the shutdown future and lose the only retry handle.

### 2.2 Named telephone-event reception

RFC 4733 events enter the receiver only when their RTP payload type equals the `telephone-event`
format selected by SDP. The value is dynamic: no default number, including 101, is accepted as a
substitute for negotiation. A packet on any other payload follows the negotiated audio/unknown
payload path and cannot create a digit.

The receiver is pure state. It reads no clock and performs no I/O. Its inputs are an accepted RTP
packet or an explicit silence expiration fired by the media worker. One receive-loop generation
owns one receiver and the existing 32-place media digit queue. A completed event is offered to that
queue with `try_send`; it never waits for an application consumer. The call driver then offers the
typed `CallEvent::Dtmf` to the existing bounded call-event queue. No third digit buffer or per-digit
task exists.

An event identity is its synchronisation source, event code and initial RTP timestamp. Source is
pinned before this state machine; the receiver therefore stores only event code and timestamp.
RFC 4733 §§2.2.1, 2.2.2 and 2.3.5 give the state transitions:

| Input | State transition | Output |
|---|---|---|
| M-bit start for an unreported identity | create current event; retain the greatest observed duration | none |
| same timestamp and event code, increasing duration | update current duration | none |
| duplicate or older RTP sequence | do not move state | none |
| E-bit report for current identity | close current and retain its identity as reported | one digit and the greatest duration |
| further E-bit reports for the reported identity | absorb them, including the recommended final-report retransmissions of §2.5.1.4 | none |
| newer M-bit start while an event lacks its E bit | close the prior event at its greatest duration, then start the new event | prior event once |
| fired silence expiration while an event is current | close it at its greatest duration; this is §2.5.2.2's bounded stuck-tone recovery | current event once |
| accepted ordinary audio after an event whose E bit was lost | close the event before delivering the audio | current event once |
| media generation, source or negotiated payload replacement | discard current and reported receive state before packets from the replacement are admitted | none |

The receive loop arms the silence boundary for three negotiated packet intervals whenever an event
is current, as bounded by RFC 4733 §2.5.2.2. It is a definition of media silence and shares the
loop's cancellation-safe socket wait; it is not a new worker. The fired expiration is an input to
the RTP state, not a core clock read. A timeout reports the last duration carried on the wire rather
than adding local elapsed time. With no event current, the existing jitter-buffer flush boundary
remains four packet intervals with a 60 ms floor.

Long events keep one application identity across unmarked, contiguous timestamp segments. The
reported duration is the segment timestamp's forward distance from the initial timestamp plus the
greatest duration in that segment. A marker starts a new application event. Sequence comparison is
serial-number arithmetic, so wrap is forward progress; accepted non-event RTP packets advance the
ordering reference too, preventing a long pause between digits from looking like reordering.

### 2.3 Cancelling an application task that is using a session

§2.1 governs the workers a session owns. This governs the tasks an *application* parks in a
session's long-running entry points — `play`, `play_pcm`, `Playback::finished`, `Playback::play_out`,
`recv`, `record_at_least` and `record_until_idle` — and it exists because `JoinHandle::abort()` is
the ordinary way to stop background work in this runtime.

Two requirements, both normative.

**Dropping the future releases everything it held.** A `play` dropped mid-clip stops that playback,
its unsent packets are discarded rather than paced onto the wire, and the outstanding-clip count a
`flush` reads is decremented. No teardown path waits on anything a dropped wait leaves behind.

**Every one of those futures MUST reach a suspension point before it answers.** This is the
requirement `M-93` added, and the one that is easy to satisfy accidentally and easy to lose. Each of
these calls answers immediately once the session has stopped — `play` returns `false`, `recv`
returns `None`, the recorders return what they have — so a caller that loops without acting on the
answer stops being paced by the media path and becomes a bare busy loop. An `async fn` that can
complete without suspending is a task the scheduler never gets back: its `abort()` never lands, its
`JoinHandle` never resolves, and dropping the runtime blocks in its own destructor. **The resource
such a loop holds is a runtime worker**, and holding one is unbounded, so §2.1's joins are performed
on a runtime that has lost a thread to it.

A caller is still expected to act on the answer, and the entry points' documentation states what the
answer means. The suspension requirement is what makes ignoring it a cost rather than a deadlock.

The requirement is discharged by a scheduler hop on the paths that can otherwise complete without
one: unconditionally in `Playback::finished`, which every playback wait resolves through, and on the
closed-and-drained `None` of the application's receive queue, which is the only answer that queue can
give for ever. It is not needed on the path that hands over a frame — a queue holds only as many
frames as arrived — nor for `recv_digit` and `recv_encoded`, which resolve through channels that are
cancellation points already.

Test vectors: `crates/sipx-media/tests/play_cancellation.rs`.

## 3. Conference construction and shutdown

A conference mix interval MUST be at least 1 millisecond. `Conference::new` returns
`ConferenceError::IntervalTooShort` before spawning its mixer when the value is shorter.

A running conference owns:

- one cancellation signal shared by its mixer and all participant collectors;
- the mixer's join handle; and
- one collector join handle for each participant ID.

The state transitions are:

| Event | Member map | Collector | Mixer |
|---|---|---|---|
| `join(session)` | insert | spawn and retain handle | unchanged |
| `leave(id)` | remove | abort and remove handle | unchanged |
| `close()` | clear | signal all, abort and remove all handles | signal and abort |
| `Drop` | released with conference state | signal all and abort | signal and abort |

`close()` and `Drop` use the same idempotent shutdown operation. `Drop` initiates cancellation but
does not synchronously wait on asynchronous joins. A collector MUST be cancellable while it is
waiting for its participant to produce a frame; shutdown must not depend on another packet or on the
participant stopping itself.

Worker registration and shutdown form one serialised lifecycle transition. `join` MUST NOT expose a
spawned collector before its completion handle is registered, and MUST NOT register one after close
has drained the registry. The stop notification is durable: a waiter registers before checking the
stopped flag, so a signal between those operations cannot be lost.

`close()` is cancellation-safe at its asynchronous lock boundary. Cancellation before it owns the
member map leaves the conference running and unchanged. Once it owns the map, it marks the lifecycle
closed, aborts and drains the worker registry, and clears every participant without another await;
cancelling the subsequent completion wait therefore cannot strand a session in a closed conference.

## 4. Discard counters

**[sipx]** Media discard counters are a parallel, media-owned snapshot. They do not join
`sipx_transport::Counters` in a shared crate. The two layers have independent lifetimes — an
endpoint can carry many media sessions and a media session can be constructed without an endpoint
— and neither crate can observe the other's losses. Moving only their value types below both would
therefore leave the atomics and their increment sites separate while adding a crate whose sole job
was to erase an honest ownership boundary. The application or call layer already depends on both
and is the right place to join snapshots when it needs one view.

`MediaSession::discard_counts` returns `MediaDiscardCounts`, a synchronous plain snapshot over
atomics owned by that session. `MediaPort` creates the meters before candidate gathering; the same
meters follow the port through gathering, the ICE driver, and every RTP, RTCP, codec, DTMF, and
playback worker. A discard during gathering therefore remains visible after the port becomes a
session rather than being reset to zero at the ownership transition.

The snapshot counts each consequence separately: codec encode and decode failures; SRTP and SRTCP
unprotect failures; caller-supplied header extensions that
could not be written onto the packet they were handed with; packets from a foreign SSRC; completed
DTMF digits refused by the
application queue; unknown RTP payload types; playback completion reports with no listener; ICE
driver datagrams and data-sent notes refused by its queue; renegotiation replies with no listener;
ICE outputs failing to send; redundant server-reflexive candidates;
non-STUN-server datagrams consumed while gathering; frames lost by an attached PCM processor
under the bounded-queue policy of [call-audio-seam.md](call-audio-seam.md) §6; decoded frames shed
from the application's inbound queue under §4.3; and the three jitter-buffer consequences of §4.2.

An ICE output naming no bound socket has no counter: it is structurally unreachable because every
base the agent can name was created from the exact socket vector the driver owns. The site carries
that reason instead of a field permanently stuck at zero.

**Neither protect-error branch has a counter, and since `M-85` the reason is the same one.** Both
`protect` and `protect_rtcp` fail on a header the transform cannot read, and neither encoder in
front of them can build one. `Rtcp::encode_compound` builds every octet of a report out of this
session's own state — SSRC, CNAME, counters — and always emits at least the eight octets
`protect_rtcp` requires. `Packet::encode` always emits the twelve-octet fixed header, caps the CSRC
count at the fifteen its nibble can name, and writes an extension only when its length word agrees
with the bytes behind it (§4.4). Each site carries that reason rather than a field permanently
stuck at zero. Both still drop the packet — sending it in the clear would defeat the encryption the
far end negotiated. Authentication failures on *unprotect* are reachable from network input and are
counted separately.

**That is `M-90`'s decision, and it reverses `M-81`'s.** `M-79` made `Encoded::extension` public
and nothing validated it: an extension whose embedded length word claimed more 32-bit words than it
carried was written verbatim with the X bit set, the header SRTP computed was longer than the whole
packet, `protect` refused it, and the packet was dropped with nothing counting it. `M-81` published
`srtp_protect_failures` for that drop and was right to: it was reachable from a caller and it cost
the call a packet. `M-85` then closed the route at `Packet::encode`, one boundary earlier, where
the extension can be dropped and the payload still sent (§4.4). What was left was a published field
nothing could move — the shape the paragraphs above and this one reject — so the field and its
increment were removed before `1.0.0` froze them.

**What an operator reads instead is `malformed_extensions_dropped`**: the same caller mistake, at
the boundary that keeps the media. Zero there means no caller on this session built an extension
that disagreed with its own length word; there is no second number for the same mistake, and never
a number for the transform refusing a packet. If the log line `dropping a packet SRTP could not
protect` ever appears, it is a defect inside this stack rather than anything a peer or an
application did, and it should be filed as one.

`protect` was re-examined as a function rather than as a branch before the field went, because
"unreachable" had been a claim about the packet layer and not about the transform. It fails four
ways and a session can produce none of them. Two are the header refusals above. The third is key
material of the wrong length, guarded inside `keystream` and `aead_seal` because a length check in
another function is not a guarantee those may rely on — but `SrtpContext::new` measures the master
key and salt against the profile and derives the session key and salt at the profile's own lengths,
so the guard has nothing to catch. The fourth is an AEAD plaintext over RFC 7714 §10's `P_MAX` of
2^36 - 32 octets, four orders of magnitude past any datagram. There is no key to exhaust on the way
either: RFC 3711 §9.2's master-key packet lifetime is not enforced here and `SrtpError` carries no
variant for it, so a long call reaches no limit that would make this branch fire. Any of the four
firing would be a defect in this stack, not an event on the call.

**The lesson is about the shape of the excuse rather than about SRTP, and it cuts both ways.**
"Unreachable because of what the caller can be" is a claim with an expiry date: publishing one
field made `M-81`'s version of it false without either site changing a line. So the reason a branch
carries no counter is a **test** here and not a paragraph —
`crates/sipx-rtp/tests/srtp_protect_header.rs` protects every packet `Packet::encode` can be made
to produce, over the adversarial extensions and over-long CSRC lists a caller can reach it with,
and fails the day one of them is refused. Whoever makes the branch reachable owns restoring its
counter, and `MediaDiscardCounts` is exhaustive, so restoring one after `1.0.0` costs a major
release. That asymmetry is real and was weighed against keeping the field as insurance; it lost,
because the same insurance argument would keep a field for every branch that might one day become
reachable, and a snapshot whose fields cannot be read as facts about the call is worth less than
one field's worth of foresight. It is also why this was settled before the freeze rather than
after it.

Every discard site MUST either increment exactly one counter or carry a `// discard: <reason>` on
the site explaining why no counter can truthfully reach it. A source-enumeration test enforces that
rule. A log line is not a counter.

### 4.1 What the numbers do not promise

Each field is individually monotonic and incremented with relaxed atomic ordering. A snapshot is
not an instant: workers can increment different fields between their individual loads, so arithmetic
relationships across fields are exact only while the session is quiet.

Codec callbacks and socket workers run on different tasks from the caller reading the snapshot. A
read racing a discard can observe the value immediately before or after that discard; it cannot lose
or double-count the increment. Tests that cause asynchronous loss MUST wait for the named counter to
rise with a bounded deadline. A fixed sleep followed by an assertion is not evidence that the count
is honest under load.

### 4.2 The jitter buffer's play-out, and what it discards

The receive loop drains the buffer on arrival: it pushes each accepted packet and then releases
everything the buffer is willing to release. Depth is therefore latency, directly — a buffer of
`depth` packets holds a packet for at most `depth` packetisation intervals before playing it — and
the ceiling on depth is the ceiling on that latency. `M-45` measured it: on ten synthetic traces of
1500 G.711 packets, including 300 ms spikes on a third of them, a 3 s straggler and a 1 s stall,
the longest any packet was held was 515 ms and the depth returned to its floor after every
disturbance. The buffer is bounded and it does not ratchet.

Three consequences follow from a release, and each MUST increment exactly one counter.

| Consequence | Counter | Meaning |
|---|---|---|
| the buffer refused a packet whose slot had been played | `jitter_late` | audio arrived and will not be heard; the depth is too shallow for this network |
| the buffer refused a sequence it was already holding | `jitter_duplicates` | the network delivered the same packet twice |
| a slot was filled because its packet never arrived | `jitter_concealed` | a gap in the timeline, filled |

`push_at` returns whether the packet was kept, and the answer is `#[must_use]`: a caller in the
media path cannot drop it silently, because a refusal that increments nothing is exactly the
invisible shedding §4 exists to prevent.

**Concealment.** A slot whose packet never arrived is filled with one packetisation interval of
silence before the packet behind it is delivered. Silence rather than an estimated waveform: it is
codec-independent and cannot invent speech that was not sent. Filling the slot is what keeps the
played timeline the length the far end sent — without it the packets either side of a gap are
delivered back to back, which is both a discontinuity in the waveform and a permanent 20 ms of
drift per lost packet.

Concealment is bounded at **200 ms** of consecutive missing packets, in time rather than in
packets, because a packet is 10 ms of audio in one codec and 60 in another. A longer run is a
stream discontinuity rather than loss — a far end that stopped, was partitioned, or restarted —
and MUST NOT be filled: doing so would inject the whole outage into the timeline as silence. The
unfilled remainder is not counted here. RFC 3550 cumulative loss already carries it and
`MediaSession::quality` reports it, and publishing the same span under two names would make the
snapshot's totals lie.

A relaying session conceals nothing. It hands payloads on in the codec they arrived in, silence
would have to be encoded to join them, and the far leg's own buffer conceals its own gaps.

The seam of [call-audio-seam.md](call-audio-seam.md) §7 is told a concealed slot was `Loss` and is
not offered the silence as audio. A processor that adds spans to delivered lengths therefore still
reconstructs the timeline, and a speech provider is not handed invented quiet it would read as the
caller pausing.

### 4.3 The application's queue, and what it sheds

The jitter buffer of §4.2 is not the last place inbound audio waits. After it, decoded frames wait
in a queue the application reads with `MediaSession::recv` — and *that* is where an application's
own reading speed turns into delay. `M-45` measured the buffer across ten 1 500-packet traces and
cleared it: bounded, no ratchet, worst measured hold 515 ms under a 300 ms spike on every third
packet. The queue behind it had no bound in time, no counter and no shed policy, and a 256-frame
channel delivered with a blocking send is **5.12 seconds** of audio at the universal 20 ms
packetisation. An application reading slightly slower than real time filled it once and stayed at
the far end of it for the rest of the call.

**The bound is a duration.** `Config::inbound_queue` states how much audio may be waiting, and
defaults to **200 ms**. It is expressed in time and enforced against the queued *sample count* at
the session's audio rate, not against a number of frames, for two reasons. A frame is 10 ms of
audio in one codec and 60 in another, so a frame depth means a different delay in every session —
the same sizing inconsistency §4.2's packet-counted depth still carries. And a far end may change
its packetisation mid-call, which this side cannot refuse; a bound counted in frames would silently
become a different bound, while a bound counted in audio does not move.

The default comes from the delay budget rather than from the queue. ITU-T G.114 puts the one-way
mouth-to-ear target at 150 ms and the limit of acceptable interactive conversation at 400 ms, and
this queue is one contributor among the network, the jitter buffer's ceiling of `jitter_max_depth`
packets, and the application itself. `Duration::ZERO` is legal and means one frame — the shallowest
queue there is, because delivering nothing is not a smaller delay — so a bound below one
packetisation interval degrades rather than refusing the call.

**The policy is shed oldest**, and it is chosen against the two alternatives rather than by
default:

| Policy | Why not |
|---|---|
| backpressure (what this replaced) | the receive loop stops draining the socket, so the delay moves into the kernel's buffer where it is neither bounded nor counted, and the call loses packets with no counter naming the reason |
| shed newest | bounds the delay identically in the steady state, but leaves the application listening to the *oldest* audio it could still be holding — after a stall it hears the beginning of the stall and the recent speech is gone |
| **shed oldest** | bounds the delay and keeps the audio the consumer has the best chance of still being able to use |

This is the policy [call-audio-seam.md](call-audio-seam.md) §6.1 already states at the processor
boundary and [speech-providers.md](speech-providers.md) §8 states at the input-frame bound, for the
same reason, so the three application-facing queues in this stack shed the same end.

Offering a frame to this queue **never blocks**. RTP decode, statistics and RTCP are therefore never
held up by a slow or stopped reader, which is what stops an application's reading speed reaching
the socket at all.

One consequence follows from that and is worth stating rather than discovering. A concealment run
at §4.2's cap is 200 ms of audio produced in a single burst with nothing awaited between the
frames, so at the default bound it can fill this queue by itself and shed whatever the application
had not yet read. That is the bound behaving as specified — 200 ms may be waiting, and a maximal
concealment run is 200 ms — but a caller that needs a long gap and the audio either side of it in
one recording should say so with `inbound_queue` rather than assume the default covers both.

**Every shed frame increments `inbound_frames_shed`**, per §4's rule. It is the one counter in the
snapshot that describes the *application* rather than the network or a codec, and it is what to
read before concluding that added delay came from the media path: a rising `inbound_frames_shed` is
the local reader falling behind, not the far end or the network. A concealed slot that is then shed
appears under both `jitter_concealed` and `inbound_frames_shed`, which are two different facts — a
packet never arrived, and the reader was too far behind to be handed the silence that stood in for
it.

Stopping a session closes the queue without discarding it: frames already accepted are still
delivered, and `recv` reports the end only once they are drained.

### 4.4 A header extension that disagrees with itself

An RTP header extension is a two-octet profile field, a two-octet length counting 32-bit words,
then exactly that many words (RFC 3550 §5.3.1). sipx interprets none of the content — RFC 8285's
element forms are a profile's business — but that length word is not content. **It is where every
reader of the packet finds the payload**, this side's transform included, so an extension that
disagrees with it is not a malformed value that can be passed along: it is a wrong answer to the
only question the packet layer asks about it.

`M-79` made `Encoded::extension` public and nothing validated it. `M-81` handled the half that
fails loudly, where the declared length runs past the whole packet. The other half stays inside the
packet and was refused by nothing: an extension declaring nine words and carrying eight octets, on
a 160-octet payload, makes a header the transform computes as ending at octet 52 of a 180-octet
packet. Every bounds check passes, and 32 octets of media are treated as header — authenticated but
not encrypted under counter mode, Associated Data under the AEAD profiles (RFC 7714 §8.2). Measured
under each profile before the fix: a 32-octet run of µ-law in the clear, 17 windows of 16 identical
octets in a 196-octet datagram, with no drop, no counter and no log.

**The boundary is `Packet::encode`, and it drops the extension rather than the packet.** An
extension whose length word does not agree with its byte count MUST NOT be written, and the X bit
MUST NOT be set over bytes that do not add up. This generalises the four-octet filter `encode`
already applied — too short to hold a length word was only the total case of the same
disagreement — and it is a property of the extension alone, published as
`sipx_rtp::extension_is_self_consistent`, so the same question has one answer everywhere it is
asked.

Two alternatives were weighed. A typed refusal at `send_encoded` would change a published signature
and reach the application layer's realtime sink trait, and would still leave every other caller of
the packet layer writing the bad bytes — including a plain leg, which is where the mistake costs
media rather than secrecy. Rewriting the length word to match the bytes would be a guess: a word
that is wrong says nothing about which of the two numbers the caller meant, and the guess would be
made on the wire under the caller's name.

**What a caller is promised.** The payload is sent whole; the extension is left off; the drop
increments `malformed_extensions_dropped`. The media is never lost with the metadata, because the
payload boundary was never in doubt — `Encoded::payload` says where the media is, and only the
extension's own length word was wrong. What the far end sees is a packet with no extension, which
is exactly what it sees whenever this side has no metadata to attach, so a receiver relying on an
element is handed none rather than a wrong one. A caller that needs the far end to receive the
extension MUST build one that agrees with itself, and can ask the same predicate first.

**The plain leg gets the same answer, and needed it for a different reason.** There is nothing to
leak where nothing is encrypted; what a plain leg loses is media. The far end reads the payload's
start from the same length word this side wrote, so before this change it accepted the packet and
played 128 octets of a 160-octet payload — not a rejection, and nothing it could log, because the
packet is well formed by every check a receiver can apply and only the sender ever knew where the
boundary was meant to be. `M-85`'s story text says the far end rejects such a packet; it does not,
and this measured behaviour is the record. One refusal at `Packet::encode` covers both legs, which
is the main reason it sits there rather than in the SRTP branch.

`malformed_extensions_dropped` is the only field in the snapshot that counts **metadata** rather
than media, so a rise in it — and in `total()` — is not evidence that any audio was lost. It
describes the application on this side: an extension that arrived over the network was
bounds-checked when its packet was decoded, so only an `Encoded` built by hand can move it. Since
`M-90` it is also the only field that moves for this mistake — the SRTP protect failure `M-81`
counted one boundary later is unreachable from here and no longer published (§4).

## 5. A forwarded header extension, and whose numbering it is in

`M-79` made a pass-through bridge forward the RTP header extension a packet arrived with, verbatim.
`M-82` settled what that means when neither leg negotiated one, because the identifiers inside an
RFC 8285 extension are scoped to a session and a bridge joins two.

**The decision. sipx negotiates no `a=extmap` in any profile, maps no element identifier between the
legs of a bridge, and forwards the extension byte for byte.** An offer carrying `a=extmap:` is
answered by omitting it — [webrtc-audio.md](webrtc-audio.md) §4.1 states the browser profile's half
of that. This is a decision and not a deferral; §5.4 names the one fact that would reopen it and
what must happen in the same change.

### 5.1 Why the identifier is the question at all

RFC 3550 §5.3.1 gives the extension a 16-bit profile field whose format is defined by the profile
the implementations are operating under. RFC 8285 defines two such fields — `0xBEDE` for the
one-byte form and `0x100x` for the two-byte — and inside them a run of elements, each introduced by
a **local identifier**: 1 through 14 in the one-byte form (§4.2), 1 through 255 in the two-byte form
(§4.3).

Those identifiers are local in the strict sense. RFC 8285 §5 requires that the mapping from an
identifier to the extension's URI "MUST be performed out of band — for example, as part of an SDP
Offer/Answer [RFC3264]", and §7 requires each to be used only once per media section. There is
nothing in the packet that says whose numbering an identifier is in. So on a bridge the same octet
can mean an audio level on one leg and something else entirely on the other, and **the bridge cannot
detect the disagreement** — which is what makes this something to argue rather than something to
fix.

The case is real rather than hypothetical. RFC 8285 §7 permits a peer to put elements on the wire
that the answer did not agree to: "Either party MAY include extensions in the stream other than
those negotiated ... (for example, for the benefit of intermediate nodes). Only extensions that
appeared with an identifier in the valid range in SDP originated by the sender can be sent." A
browser that offered `a=extmap:1 …` through `:4 …` and got an answer with none may still send
elements 1 to 4, conformantly. [webrtc-audio.md](webrtc-audio.md) §9.4's captured offer is exactly
such a description.

### 5.2 Why forwarding one is safe, in four steps

**sipx originates no identifier, so a forwarded one has nothing to collide with.** By the same
sentence of RFC 8285 §7, a sender may only send identifiers that appeared in SDP *it* originated —
and sipx originates none, in the generic answer path or the browser profile. Its own numbering table
is empty on every leg, in both directions. A relayed element is therefore the only element on the
outgoing packet, and no receiver of sipx's is ever put in the position of having two meanings for
one octet. **This is the whole safety argument's precondition**, and it is what §5.4 watches.

**A far end that negotiated nothing reads nothing out of what it is handed.** It negotiated nothing
because sipx offered and answered nothing. RFC 8285 §5 makes the out-of-band mapping the definitive
indication that a header extension is present at all, so a conformant far end has no identifier
bound to any URI for this session and no element to act on.

**A far end that does not implement RFC 8285 skips the extension whole.** This is RFC 3550 §5.3.1's
own design goal, stated in the section the story cites: the mechanism "is designed so that the
header extension may be ignored by other interoperating implementations that have not been
extended." The skip is driven by the four-octet header's length word, and §4.4 is what makes that
word trustworthy on everything sipx encodes — so the far end finds the payload at the right offset
whether it looks at the extension or not. RFC 8285 §4.1 makes the same property normative for the
extensions themselves: they "MUST NOT be used to extend RTP itself in a manner that is backward
incompatible with non-extended implementations", and metadata may ride along only "provided the RTP
layer can function if that metadata is missing". An element the far end must act on for the call to
work is already outside RFC 8285.

**And verbatim is the only rule that is also right for the other kind of extension.** Not every
header extension is an RFC 8285 one. RFC 3550 §5.3.1's profile field is scoped by the *profile* both
legs are operating under — RFC 3551 for an ordinary G.711 call — and not by anything either session
numbered. For such an extension the identifier means the same thing on both legs by definition, so
dropping it would destroy meaning that was intact and translating it would be a category error.
Forwarding is right for that case and harmless for the RFC 8285 case; **no other single rule is
right for both**, which is the strongest reason this is the answer rather than a convenient one.

### 5.3 What the argument does not cover, stated as limits

- **It is about meaning, not about disclosure.** A bridge is two dialogs, and an element that
  crosses it — a level measurement, an `sdes:mid`, an absolute send time — is a fact about one
  call's endpoint delivered to the other call's. Ignorable on receipt is not the same as not
  disclosed. sipx accepts that exposure: it is bounded by what the forwarded audio already conveys,
  and refusing it would take `M-79`'s case with it. Recorded here so that a deployment joining two
  trust domains can weigh it rather than discover it.
- **It is about conformant receivers.** A far end that acts on an element it never negotiated is
  already reading data no one agreed to send it, and nothing on the packet distinguishes such a peer
  from a compliant one. sipx cannot detect that peer and does not claim to.
- **It says nothing about a peer that numbers *identically* by coincidence.** Two legs that both
  used identifier 1 for different extensions produce a packet that is correct on both by every check
  either can apply. That is the residual case, it is unobservable from the middle, and it is
  precisely why sipx does not *invent* a translation: a renumbering built on a guessed reading of
  the elements would turn an unobservable coincidence into a wire effect sipx authored.
- **It is not a claim about a conference.** A mixer's outbound packet is one this endpoint authored,
  so it carries no contributor's extension at all; that decision and its reason live in
  `crates/sipx-media/src/conference.rs`.
- **It is not a claim about the off-media role.** When sipx relays descriptions and stays off the
  media path ([call-coupling.md](call-coupling.md) §6.1), the two endpoints negotiate `extmap` with
  *each other* — one session, one numbering — and no translation is needed or possible. The question
  in this section arises only where sipx terminates media on both legs.

### 5.4 What would reopen this, and the alternative that was weighed

**The trigger is sipx originating an `a=extmap` on either leg.** The moment it does, its own
numbering table stops being empty, §5.2's first step fails, and a forwarded identifier can collide
with one sipx itself agreed to. Whoever teaches sipx to negotiate `extmap` — for RFC 6464 audio
level in a mixer, for `sdes:mid`, for anything — MUST in the same change teach `Bridge`'s relay to
map identifiers between the legs, or refuse to bridge legs whose mappings disagree. Splitting those
two across two changes leaves a window in which sipx renumbers nothing while claiming a numbering,
which is worse than either end state.

Negotiating `extmap` now was the alternative. It was declined because negotiating an identifier is a
claim to implement the extension it names, and sipx implements none of them: the answer would agree
to carry metadata nothing produces or consumes, which RFC 8285 §7 already tells an answerer not to
do ("An answerer that has no desire to receive the extension or does not understand the extension
SHOULD remove it from the SDP answer"). It would also buy nothing for a bridge until *both* legs
negotiate, and a leg that negotiates none is the common case on the telephone network this stack
exists for.

## 6. Test vectors

| Vector | Input | Required result |
|---|---|---|
| T1 | `packet_duration = 0` | typed setup error; requested bind address remains available |
| T2 | `rtcp_interval = Some(0)` | typed setup error; requested bind address remains available |
| T3 | packet and RTCP intervals of 1 ms | session starts and remains able to send |
| T4 | conference interval of 0 | typed conference error; no mixer starts |
| T5 | conference interval of 1 ms | conference starts and can be closed repeatedly |
| S1 | start an ordinary separate-RTCP session, then call `shutdown()` | every retained RTP, playback and RTCP handle is joined; the owner set is empty |
| S2 | cancel one `shutdown()` while it is joining, then call it again | the in-flight handle remains owned and the second call drains it |
| S3 | answer a call, ACK it, then end it | the successful-response retransmitter and every media worker are joined before the terminal call operation returns |
| S4 | cancel `reconfigure()` while the old generation is joining, then retry | the replacement retains the old generation; retry drains it and no old socket worker remains |
| S5 | stop a successful-response retransmitter before its first poll and during a pending handoff | both stops are observed and joined without waiting for T1 or another response |
| S6 | media setup fails after a successful final response was sent | the latched stop is set and the retransmitter is joined before setup returns the typed error |
| S7 | abort a task parked inside `play`, then flush, stop and `shutdown()` the session | the task is reaped and the whole teardown completes within a bound far below one clip |
| S8 | stop a session under a task looping over `play`, then abort that task | the task is reaped; a task that could not suspend would hold a runtime worker for ever |
| S9 | S8 for a task looping over `recv` | the task is reaped |
| C1 | drop a conference whose collector is blocked in `recv()` | collector is cancelled and its session `Arc` is released within a bounded deadline |
| C2 | leave, close twice, then drop | no retained participant and no panic |
| C3 | race `join` against `close` while the participant is quiet | either join is refused or its registered collector is drained; no retained session |
| C4 | cancel `close` while it waits for the member map | conference remains open and retryable; a later close releases every session |
| O1 | Opus encoder construction is refused | typed encoder setup error; no direct G.711 encoder exists in the resulting state |
| O2 | Opus decoder construction is refused | typed decoder setup error; no direct G.711 decoder exists in the resulting state |
| O3 | successful Opus on dynamic payload type 96 | emitted RTP names 96 and carries Opus bytes |
| D1 | 33 completed DTMF digits offered while the 32-place application queue is unread | the 33rd is absent from the queue and `dtmf_delivery_failures = 1` |
| D2 | one RTP packet using neither the negotiated nor a known static payload type | no audio is delivered and `unknown_payload_type = 1` |
| D3 | one packet after a different SSRC has established the stream | no stream state moves and `foreign_ssrc = 1` |
| D4 | a source discard is added without a nearby counter increment or `// discard:` reason | the media discard enumeration test fails with its file and line |
| D16 | `send_encoded` on each SRTP profile with an extension declaring nine words over eight octets, on a 160-octet payload | no window of 16 identical payload octets anywhere in the datagram; the far end decrypts the full 160-octet payload and no extension; the whole snapshot is `malformed_extensions_dropped = 1` and every other published field zero |
| D17 | D16 on a plain leg | the packet arrives with its full payload and no extension; `malformed_extensions_dropped = 1` |
| D18 | D16 with the length word declaring 255 words instead — `M-81`'s fixture, which runs past the packet | the same result as D16: the packet is no longer lost, and the transform is never handed it |
| D19 | every packet `Packet::encode` produces from an adversarial extension or a CSRC list longer than the count nibble, protected under each SRTP profile | `protect` accepts all of them; a refusal means the send loop's protect-error branch is reachable from a caller again and §4 owes it a counter |
| D20 | a hand-assembled packet whose extension length word runs past its end, and packets of 0 to 11 octets, protected under each profile | `SrtpError::TooShort` in every case — the refusal `Packet::encode` keeps out of reach still exists |
| D21 | a `MediaDiscardCounts` with one distinct bit set in each published field | `total()` is the saturated pattern over exactly those bits: every field summed once, none omitted and none summed twice |
| D10 | G.711 at 20 ms, depth 1; sequences 1, 2 and 4 arrive | four packets of audio are delivered, the third being silence, and `jitter_concealed = 1` |
| D11 | G.711 at 20 ms, depth 1; sequences 1 and 400 arrive | twelve packets of audio — two carried, ten concealed — and `jitter_concealed = 10`; the rest of the gap is reported only as RFC 3550 loss |
| D12 | depth 2; sequences 1, 2, 3, then 1 again and 3 again | `jitter_late = 1` and `jitter_duplicates = 1`; neither copy is played |
| D13 | G.711 at 20 ms; 200 packets arrive with nothing reading the session | the application is handed at most `inbound_queue` of audio, not the four seconds that arrived |
| D14 | D13 repeated at a 60 ms packetisation | the same bound in milliseconds, over a different number of frames |
| D15 | D13 with a distinct marker near the burst's start and another near its end | the late marker is delivered and the early one is not; `inbound_frames_shed` is the number of frames that did not fit |
| D5 | negotiated PT 96, `80e003e8000003e8decafbad010a00a0`, then PT 96 packets `806003e9000003e8decafbad010a0140` and `806003ea000003e8decafbad018a01e0` | one digit `1`, duration 480 timestamp units |
| D6 | D5 followed by two newer copies of the final `018a01e0` payload, a duplicate sequence, and a late continuation | still one digit `1`; state does not move backwards |
| D7 | PT 96 start `80e007d000000bb8decafbad030a00a0`, continuation `806007d100000bb8decafbad030a0140`, then the explicit silence expiration | one digit `3`, duration 320 timestamp units; a later end report cannot emit it again |
| D8 | D7's start followed by receiver reset; the old worker is stopped before a replacement is admitted | no digit; a fresh marked event in the replacement generation is independent and old-worker packets cannot cross the boundary |
| D9 | D5 bytes carried on PT 101 when SDP selected PT 96 | no receiver input and no digit |
| E1 | a pass-through bridge whose two legs send the same one-byte element under identifier 1 and identifier 2 respectively, at the same time | each far end receives the identifier its own sender wrote, byte for byte, in both directions; §5's decision, and the extension is the only one on the packet |
| E2 | a browser-audio offer carrying `a=extmap:` lines and `a=extmap-allow-mixed` | the offer validates and is answerable; the generated answer contains no `a=extmap`, and neither does an offer sipx generates — §5.2's precondition |
