# Design: Media

**Status:** accepted — this describes the media stack as delivered, not a plan for one ·
**Pillar:** Media · **Epics:** `media`, `depth`, `conformance`, `ice` (and `app-sdk` for `M-17`
and `M-18`) · **Stories:** `M-1` … `M-24`, of which `M-1` … `M-15` and `M-17` … `M-20` are done

## What this document is, and which one you probably want

This is a **design record**. It says *why* the media stack is shaped the way it is: the decisions
that were made, the alternatives that were rejected, and the reasons. It is an argument, and it is
not normative — no implementation is required to satisfy a sentence in here.

[`docs/specs/ice.md`](../specs/ice.md) is a **spec**. It says *what*, normatively: RFC citations,
types, state tables, timer values, a STUN attribute profile, an SDP grammar and byte-level test
vectors. Tests are derived from its vectors ([AGENTS.md](../../AGENTS.md), non-negotiable 4), and
when the spec and the code disagree the spec is right until somebody changes it deliberately.

Twenty stories name this file in their `design:` frontmatter — `M-1` … `M-13`, `M-16` and
`M-19` … `M-24` — so most readers arrive here from a story. If you are implementing one, you
almost certainly want a different document as well:

| You want to know | Read |
|---|---|
| Why the media stack is built this way; what was tried and rejected | this record |
| What ICE must do, exactly — priority formula, pair states, role conflict, timers, vectors | [`specs/ice.md`](../specs/ice.md) |
| What SRTP and its two keyings must do, exactly — the transform, SDES, DTLS-SRTP, which keying wins, vectors | [`specs/srtp.md`](../specs/srtp.md) |
| What playback, mute and the bridge owe an application, and why they behave as they do | [`designs/app-sdk.md`](app-sdk.md) — `M-17` and `M-18` are recorded there, not here |
| Why the protocol core does no I/O at all | [`vision.md`](../vision.md) principle 1, [`designs/sip-core.md`](sip-core.md) |
| What a `Call` is, and where dialogs live | [`designs/call.md`](call.md) |

**SRTP, SDES and DTLS-SRTP have a spec now** — [`specs/srtp.md`](../specs/srtp.md), written by
`M-25`. It was written *after* `M-14` and `M-15` built them, which is the departure from
non-negotiable 4 that `X-25` recorded here as a gap. The gap is closed and the departure is not
retracted: writing the spec found five places where the code and the RFCs disagree — two fixed by
`M-25`, three left open with an owner — and the first of them, a session authentication key derived
to §B.3's example length rather than to `n_a`, was fatal to interoperating with anything that is not
sipx and undetectable by any test in which both ends are. `specs/srtp.md` §12 is the list.
Spec-first would have caught it before two releases shipped; that is the argument for the rule, made
backwards.

## Why

Signalling that cannot carry audio is a curiosity. The media layer is also where the sans-IO
discipline pays off a second time: offer/answer is a pure function, RTP packet handling is pure,
and the only part that needs a socket is the session that binds them.

That sentence was written before `M-1`. It survived contact, but only because the boundary it
describes turned out to be a *crate* boundary rather than a style, and the rest of this record is
largely about where that boundary actually fell.

## The shape

Four crates, and the split between them is the design:

| Crate | Holds | I/O |
|---|---|---|
| [`sipx-sdp`](../../crates/sipx-sdp/src/lib.rs) | SDP (RFC 8866), offer/answer (RFC 3264), the `a=crypto`, `a=fingerprint` and RFC 8839 ICE grammars | none — forbidden by [AGENTS.md](../../AGENTS.md) non-negotiable 2 |
| [`sipx-audio`](../../crates/sipx-audio/src/lib.rs) | G.711 µ-law and A-law, PCM mixing, WAV, Opus behind a feature | none |
| [`sipx-rtp`](../../crates/sipx-rtp/src/lib.rs) | RTP and RTCP packets, the jitter buffer, DTMF events, SRTP, quality arithmetic | one clock read (below) |
| [`sipx-media`](../../crates/sipx-media/src/lib.rs) | The session, the sockets, the pacing clock, the bridge, the conference, DTLS-SRTP, ICE | all of it |

Three of the four have no `tokio`, no socket and no runtime in their manifests. `sipx-media` is
unashamedly the driver: `tokio` with `net`, `rt`, `sync`, `time` and `macros`, one task per
direction, one interval timer.

`sipx-call` sits above all four and owns dialogs; it is [`designs/call.md`](call.md)'s subject, not
this one's. The media stack's only opinion about it is that a `Call` **owns** its pipeline outright
— [vision](../vision.md) principle 3 — which is why the bridge moves frames over channels rather
than sharing a session behind a lock.

## Why the media state machines are sans-IO, with a driver over them

[`specs/ice.md`](../specs/ice.md) §2 declares the agent sans-IO "in the shape the transaction
machines already use" and then gets on with the protocol. It never argues the point. This is the
argument, because `M-21` and `M-22` are the stories that pay for it.

**The inherited argument.** [`designs/sip-core.md`](sip-core.md) rejected "async throughout, a task
per transaction" on one ground: *it makes timing behaviour untestable without a clock, and pushes
every retransmission bug into flaky integration tests* — which get retried rather than fixed.
[`specs/sip-transaction.md`](../specs/sip-transaction.md) §2 fixes the resulting interface as
`Input`/`Output` with the outputs ordered — `Send` before `SetTimer`, always, so a retransmission
timer never starts before the thing it retransmits has gone out.

**It carries to ICE, and more strongly than it carried to transactions.** RFC 8445 is a protocol
with retransmitting timers (Ta pacing, RTO, Rc, Rm — spec §9), so the transaction argument applies
unchanged. But ICE has two properties the transaction machines do not, and both are reasons to keep
the socket out:

- **The interesting states are combinatorial, not enumerable.** A transaction has four FSMs with
  tables you can walk row by row. An ICE agent's state is a product of N local candidates × M
  remote ones, and which of those pairs wins is decided by the §5.1.2.1 priority formula — the
  formula, `M-16` insists, *not an approximation of it, because the priority ordering is what makes
  two independent implementations agree on which pair wins*. Asserting that ordering means
  constructing candidate sets; over a socket that means constructing a NAT topology, and over a
  function it means a table. Spec §14's vectors 6, 7 and 8 are exactly that table, and `M-21`'s
  Acceptance asserts them pair by pair.
- **The failure that matters is reachable only by a race.** Role conflict (§7.3.1.1) is, in
  `M-16`'s words, *the failure mode that only appears when both ends run the same stack* — two
  agents that both believe they control never converge. Reaching it with real sockets needs two
  live agents and a timing coincidence, and the resulting test either passes for the wrong reason
  or is the flaky one somebody retries. Sans-IO it is spec §7.3's seven rows, one assertion each,
  including the `T = V` row that decides whether two identical stacks converge at all.

There is a third reason specific to this port. The connectivity-check parser eats unauthenticated
datagrams from anyone who can reach the media socket — `M-20`'s Acceptance says so in as many
words. Keeping the codec and the machine free of I/O is what lets that parser be fuzzed and
property-tested without a network, which is the only honest way to satisfy
[vision](../vision.md)'s north star for a surface that hostile.

**Where the argument does not carry, and is not applied.** This is the part a reader coming from
`specs/ice.md` §2 will otherwise get wrong: the media stack as a whole is *not* sans-IO, and was
never meant to be.

- **The media session is a driver and nothing else.** It has a socket, an interval timer and two
  tasks. There is no `MediaSession` state machine to test without I/O, because there is no protocol
  in it — what it does is bind, pace, encode and hand off.
- **The jitter buffer is pure but not event-shaped.** It takes packets in and releases them on
  `pop`; time is entirely the caller's cadence. It needs no `TimerFired` because it sets no timer,
  and giving it an `Input`/`Output` vocabulary would add a layer carrying no decision. What makes
  it testable is
  [`crates/sipx-rtp/tests/jitter_traces.rs`](../../crates/sipx-rtp/tests/jitter_traces.rs) driving
  two buffers over identical synthetic traces on the same playout clock (`M-9`).
- **Codecs are pure functions of a frame — except Opus, which is not.** `M-13` records the
  consequence: Opus carries encoder and decoder state, so the state lives one each in the send and
  receive loops. That is the ownership principle, not the sans-IO one, and it is what keeps a
  stateful codec out of a lock.
- **One clock read survives in a pure crate.**
  [`sipx_rtp::quality::ntp_now`](../../crates/sipx-rtp/src/quality.rs) reads the wall clock, because
  RFC 3550 §6.4.1's sender-report NTP field *is* a wall-clock value and has no meaning as a fired
  timer. Its own documentation bounds the damage: the round-trip calculation works on differences,
  so a constant offset cancels, and a clock that steps mid-call is why round-trip time is reported
  as a most recent sample rather than accumulated into an average.

## The path a packet takes

**Out.** The application hands frames to the session. One `tokio::time::interval` at the
packetisation interval — 20 ms by default — takes one frame per tick, gates it for mute, encodes
it, stamps it with the stream's own sequence number and timestamp, protects it if SRTP is keyed,
and sends. Pacing on the timer rather than on channel readiness is the crate's second stated
decision, and its module documentation gives the reason: sending on readiness makes the packet rate
depend on how fast the application produces samples, *which is how a call ends up sending 200
packets per second to a jitter buffer expecting 50.*

Audio and DTMF share that one queue and that one clock. `M-7` chose that deliberately: they share a
sequence number space, so a separate path would have to interleave them anyway and would get the
numbering wrong the first time both were busy.

**In.** Datagrams are classified before anything parses them —
[`dtls::classify`](../../crates/sipx-media/src/dtls/mod.rs) implements RFC 5764 §5.1.2, which tells
STUN from DTLS from RTP on the first byte alone. RTP goes to the jitter buffer; a connectivity check
must never reach it, and an RTP packet must never reach the ICE agent (`M-22`'s Acceptance). The
buffer reorders, absorbs jitter, counts loss and releases on the receive loop's cadence; then
decode, then the application.

**The jitter buffer is the one place latency is deliberately spent**, and `M-2` and `M-9` between
them record why it is spent adaptively. The asymmetry is the whole policy: *being too shallow is
audible, being too deep is not.* A packet that misses its slot is a gap in the audio; a buffer
holding one packet more than it needs is 20 ms nobody notices. So the depth grows at the first sign
of trouble and shrinks only on sustained evidence — 250 packets, five seconds at 20 ms.

Two decisions inside it are worth carrying forward. The fixed buffer was **kept, not replaced**:
`M-9` argues that an adaptive buffer which cannot be shown to beat a constant is a constant with
extra ways to go wrong, so `JitterBuffer::new(depth)` stays as the control the comparison tests
measure against. And **shrinking is free at this layer** — lowering the depth releases the next
packet one slot sooner, nothing is dropped and nothing is played faster. Time-scale modification,
which is what makes shrinking hard elsewhere, would belong in the media layer and does not exist.

Sequence numbers are extended from 16 to 64 bits on the way in (`M-2`), after which ordinary
comparison is correct again. The 16-bit counter wraps every twenty-odd minutes at speech packet
rates, so the wrap is an ordinary event rather than an edge case, and reordering *across* the wrap
is tested directly because that is the case a naive comparison gets wrong.

**Reporting.** RTCP goes on the odd port of the bound pair (RFC 3550 §11). `M-6` fixed the interval
at RFC 3550 §6.2's five-second minimum rather than implementing §6.2's arithmetic, on the ground
that for a two-party call the calculation always lands there — a computation that can only return
one answer is a place for a bug and not a feature. Interarrival jitter is the RFC's own recurrence
`J += (|D| − J)/16` and not a variance, because *a variance produces numbers that look plausible,
move in the right direction and are wrong by a factor that depends on the traffic*, and somebody
will tune a jitter buffer with them.

`M-10` then split the two audiences. The **per-interval** loss fraction is the right number to send
and the wrong one to show, so `MediaSession::quality()` reports loss over the whole call while the
report block reports the interval; an application sampling the per-interval figure sees whichever
interval it happened to catch. Round-trip time is computed from the RTCP round trip or **omitted**,
never reported as zero. The MOS is rendered to two decimal places on purpose — the E-model behind
it has simplified impairment terms, and eight digits would invite comparing two calls on the last
one.

**Who closes a reporting interval.** `M-33` settled it: **a report being sent**, never a read.
RFC 3550 §6.4.1 defines `fraction_lost` as loss since the previous SR or RR *packet*, so the
boundary is a transmission and looking at the numbers is not one. `StreamStats` says that in its
signatures — `pending_report_block(&self)` to read, `report_block(&mut self)` for the RTCP loop that
actually sends — and `MediaSession::stats()` is therefore documented as safe to poll. Before that,
one function did both under two comments that disagreed about which it was, and the observable
consequence was a defect and not a documentation gap: an application polling `stats()` for a
dashboard closed the window the next report was going to describe, so the peer was told a lossy
interval was clean. `M-10` had found and fixed the same trap in `quality()` and left `stats()`
holding it, which is why the fix here is a signature the trap cannot survive rather than a
corrected sentence.

Asserting that on the wire needed a technique, because the obvious test cannot be written safely:
"lose two of ten, wait for a report, lose five of twenty, assert the second report says five of
twenty" assumes the report timer fires *between* the two batches, and no wall-clock interval buys
that — under a 6x CPU oversubscription the paced injection stretches past any margin and the report
describes half a batch. Measured, not supposed: the arranged version failed 14 of 20 runs there.
Which makes it an instance of the rule below, so the fix is to stop needing a boundary. **A report
names the window it covers** — `extended_highest_sequence` is the last sequence number in it, and the
previous report's is where it opened — so a test that knows which sequence numbers it withheld can
compute what §6.4.1 requires of *whatever* window the timer drew, and assert it on every report.
Wherever the boundaries land, each report has to be right about its own interval, and one quoting
the whole call cannot be.

## Waiting for audio: two questions, two verbs

Everything in this record is tested over real sockets against a real clock, so how a test *waits*
is a design decision and not a detail. `X-28` found the media suites had one verb doing two jobs,
and a flaky bridge test was the symptom.

**`record_until_idle(idle)` answers "has the far end stopped talking".** It is a stream-end
detector, and it is the right verb where the far end's length is genuinely unknown — the CLI's
`dial` and `answer` record a human. Its duration is a definition of silence.

**`record_at_least(samples, within)` answers "did all of it arrive".** It is the right verb for a
caller that played the clip itself, which is nearly every test. Its duration is a **bound on
failure** — how long before we conclude the audio is not coming — not a window to measure in, so
it is set an order of magnitude above the honest answer rather than close to it.

Confusing the two is what made `audio_played_into_one_call_is_heard_on_the_other` fail on a loaded
machine while proving nothing about the diff under test. A caller that knows the count and waits
on an idle window is racing a fixed wall clock against a pipeline that is merely slow — and this
pipeline is slow in exactly the place the first window covers, because **the first packet is the
one it delays the most**: two 20 ms send pacers in series, and behind each an adaptive jitter
buffer entitled to grow to 240 ms precisely *because* arrivals have become jittery. The failure is
not a degraded recording. It is an empty one, because once the first frame lands the rest follow
at the packet rate; the recording is all-or-nothing by construction.

The general rule this leaves for any future test in this crate: **a fixed wall-clock duration may
bound a failure, or define silence. It may not stand in for a happens-before.** Where a count
exists, wait for the count. Widening the window instead moves the cliff rather than removing it,
and leaves a test everyone re-runs instead of reading.

### The same two questions, for digits

`M-34` found the rule broken one call along, in production rather than in a test.
`collect_digits(idle)` spent one window on both "how long to wait for the **first** digit" and "how
long a gap means the digits **ended**", which is `X-40`'s defect exactly, and it fails the same way:
not a short sequence but an **empty** one, because the loop ends before its first iteration. Measured
in `sipx-media` with the arrival time as the only variable — digits at once, `"1234"`; the same
digits 2 s later against the same 1 s window, `""`.

`collect_digits(within, gap)` now takes the two durations separately, and each lands in one of the
two roles the rule permits. `within` **bounds a failure**: it is how long this side waits for
dialling that may never start, so `sipx answer` passes the call's own duration and a caller cannot
be slower than the call. `gap` **defines silence**, which is the only question it could ever answer.
The cap is enforced inside, so a collection cut short keeps its digits — the `timeout(…)
.unwrap_or_default()` around the old call was `X-40`'s second defect, discarding every digit
collected at the moment the cap fired.

**Mirroring `record_at_least` — a count wait — was considered and is the wrong verb here**, which is
worth recording because the audio path's answer does not transfer. `assert_eq!(collected, "1234#")`
after a counted wait for five digits cannot see a sixth, so a keypress reported twice would stop
failing the test that exists to catch it; and the production caller has no count at all, because a
keypad's length is not known in advance. Where no count exists, `X-28`'s own remedy applies instead:
keep the wall clock for the question it can answer and set it past any scheduling delay.

**What "the digits ended" infers, and why it is safe to infer.** RFC 4733 carries keypresses, not a
completion signal, so there is no event meaning *the caller is done* and the gap is the whole of the
inference. What makes it sound is that its input is exact. A digit is delivered **once**, when the
first packet carrying that tone's end bit arrives; the tone is identified by its own RTP timestamp,
constant across every packet of the tone, so the end retransmissions of RFC 4733 §2.5.1.3 are
absorbed rather than counted again, and `44` is told from one long `4` by the timestamp changing. An
elapsed gap therefore means *no keypress completed in it* — never that a packet went missing
mid-tone. A digit arriving a millisecond late is not lost either: it stays queued and opens the next
collection. It is in the wrong collection, and no window can fix that, which is why an application
that knows how many digits it wants should stop at that count with `recv_digit` rather than wait for
a silence at all.

## Codecs

**G.711 µ-law and A-law, in pure Rust, checked against the ITU-T reference tables and not against a
round trip.** `M-3`'s reasoning is the general rule for this stack: round-tripping proves only that
the two halves agree with each other, *and two halves wrong in mirrored ways agree perfectly while
interoperating with nothing.*

**Opus behind an off-by-default feature** (`M-13`), so the stack stays pure Rust unless somebody
asks for the codec. Three consequences reached up out of the codec and changed types above it: the
RTP clock is 48000 whatever the audio rate (RFC 7587 §7), so `Codec::clock_rate` is no longer the
sample rate; Opus has no static payload type, so the negotiated number is carried on `Config` and
`Codec::from_payload_type` deliberately never returns Opus, since guessing it from 111 would decode
somebody else's G.729 as Opus; and the codec is stateful, which put the encoder and decoder in the
loops rather than behind a lock.

**Dynamic payload types are matched by encoding name, never by number** (`M-1`). 96 is one codec at
one end and another at the other; the numbers agreeing means nothing, and agreeing on that basis is
how a stack accepts a codec it cannot decode. `M-13` is the case that rule was written for, and
`opus_is_matched_even_when_the_far_end_numbers_it_differently` is its test.

**A stream offering only `telephone-event` is rejected** (`M-1`): DTMF alone is not a call, and
accepting it would establish a session that can never carry speech.

**G.722 is not implemented and is not planned** (`X-26`). The outline this record replaces listed
it, and so did [`sipx-audio`](../../crates/sipx-audio/src/lib.rs)'s crate documentation, its
package description and the website's crate table — along with "resampling", which also does not
exist ([`sipx-cli`](../../crates/sipx-cli/src/dial.rs) tells the user to resample before
dialling). When this record first went looking (`X-25`) it could find no story that cut G.722 and
no decision to drop it, only the claim being repeated; `X-26` is where the decision was finally
taken, so it is written here rather than left in the gaps.

The argument, for whoever wants to reopen it. G.722's value is wideband audio over a static
payload type, and that slot is Opus's here (`M-13`), which is wideband, better, and already
negotiated by name. Nothing in the stack was ever built expecting G.722:
`Codec::from_payload_type(9)` returns `None`, `sipx-sdp` answers an offer of it with port 0, and
`sipx-call` refuses a call that offers nothing else — three tests assert exactly that, so the
codec's absence is a specified behaviour rather than an omission. Resampling is likewise
deliberate: `sipx-cli` rejects a clip that is not 8 kHz rather than resampling it quietly,
because audio resampled by accident is recognisably wrong rather than obviously broken. Either
one is welcome back as a story that argues for it; neither is owed by a package blurb.

The claim cannot come back by itself. `scripts/check-audio-claims.py` reads the three strings
that advertise `sipx-audio` — the manifest description, the crate documentation's summary
paragraph and the website's crate table — and fails the gate on a codec named in any of them that
no module both encodes and decodes.

## Addressing: symmetric RTP, and where ICE now sits

**Symmetric RTP is the delivered NAT strategy** (`M-4`): send to where packets arrive from, not to
the address the SDP advertised, because behind a NAT the advertised address is private and the only
path back is the pinhole the far end opened by sending.

The security decision inside it is the one to carry forward: the observed source is latched **only
after the packet has parsed**. Latching on any datagram would let anyone who can guess the port
redirect a call's media with a single byte, which is a hijack rather than NAT traversal.

`MediaPort` exists for an ordering constraint rather than an aesthetic one: an offer must name the
port audio will arrive on, but the codec and the remote address are unknown until the answer, and
binding twice fails with `AddrInUse`. So the socket pair is bound first, its port goes into the
offer, and the session starts once there is something to start it with. `M-10` later made it bind
the *pair* rather than one socket: a session that sends RTCP and cannot receive any is half a
control protocol, able to say what it heard and never to learn what the far end heard.

**ICE is where symmetric RTP runs out**, and it is the epic in flight:

| Piece | Story | State |
|---|---|---|
| RFC 8839 §5 attribute grammar in `sipx-sdp` | [`M-19`](../stories/M-19-ice-sdp-attributes.md) | done |
| STUN-for-ICE codec in `sipx-media` | [`M-20`](../stories/M-20-ice-stun-checks.md) | done |
| The sans-IO agent | [`M-21`](../stories/M-21-ice-agent.md) | ready |
| Driving it on the media port | [`M-22`](../stories/M-22-ice-on-media-port.md) | backlog |
| ICE restart | [`M-23`](../stories/M-23-ice-restart.md) | backlog |
| Relayed candidates (TURN, RFC 8656) | [`M-24`](../stories/M-24-ice-relayed-candidate.md) | backlog |

[`M-16`](../stories/M-16-ice.md) is the tracker and stays open until the six children are. It is
also the record of why the split falls where it does — twelve Acceptance items over two RFCs, with
a third (RFC 8656, a TURN client) hiding inside one bullet of the first.

Three ICE choices belong in a design record rather than a spec section, because they are choices
rather than requirements:

- **Symmetric RTP is not replaced; it is the floor.** `M-22`'s Acceptance is explicit that a peer
  offering no `a=candidate` gets exactly today's behaviour — nothing offered, no checks, no timers
  — and that *a stack that requires ICE to place a call has regressed*. The regression proof is the
  existing media suite passing unchanged. A selected pair replaces address learning only for a
  stream that has one.
- **ICE-lite is deferred, with the reason recorded** (spec §12): the lite role is for an agent
  already on a public address that never gathers and never checks, which is the opposite of sipx's
  deployment, and it is an endpoint-wide property, so supporting it means a second nomination path
  alive in the same binary. *Interoperating* with a lite peer is not deferred — `a=ice-lite` in a
  remote description makes sipx controlling unconditionally.
- **Trickle ICE is out of scope**, because half of it is worse than none: an agent that accepts
  trickled candidates and never sends them advertises a capability it does not have.

**The STUN codec's crate placement was a real fork in the road**, decided in `M-20`. `sipx-rtp`
already had `hmac`, `sha1` and `subtle` and would have cost no manifest line — and was rejected,
because a connectivity check is not a media packet, every downstream user wanting only to parse RTP
would inherit a STUN codec in that crate's public API, and the codec would sit a crate *below* its
only caller. It went to `sipx-media` for three dependency lines, on the decisive observation that
all three crates were already in `sipx-media`'s transitive graph, so naming them directly adds
nothing to anybody's build. The `sipx-transport` edge carries `default-features = false` for the
same class of reason: what is borrowed is RFC 5389's twenty-byte header, not a SIP transport, and
with defaults on, every user of `sipx-media` would inherit rustls, a WebSocket stack and a DNS
client.

## Security: two keyings, and why both

**`M-14` delivered SRTP (RFC 3711) with SDES keying (RFC 4568)**, and **`M-15` delivered DTLS-SRTP
(RFC 5764)**. Both, not one, and the reason is where the key travels. SDES puts the master key in
the SDP, so every proxy and every session border controller on the signalling path has held it.
DTLS-SRTP handshakes on the *media* path and the signalling carries only a hash of the certificate
that will appear there. SDES landed first because it is smaller and already useful; DTLS-SRTP is
what a browser will insist on.

The decisions worth keeping:

- **A key is never offered over a path that cannot protect it**, and the rule is enforced by a
  signature rather than a comment: `Crypto::offer` takes whether the signalling is secure and
  returns `None` when it is not (RFC 4568 §7.1), so a key cannot be published by somebody
  forgetting a check.
- **Both halves or neither.** A stream keyed at one end only connects and carries silence, which is
  worse than one that fails to connect.
- **A session expecting SRTP refuses plain RTP**, because accepting it would let an attacker
  downgrade the call with one unencrypted packet.
- **Checked against the RFCs' own vectors, not against sipx's arithmetic.** `M-14` states the reason
  and it is the strongest version of `M-3`'s rule: a key derivation that is wrong but
  self-consistent gives two endpoints that interoperate perfectly with each other and with nothing
  else in the world, and every round-trip test still passes. `M-15`'s surviving mutation is the
  proof — a key/salt split read in the wrong order passed the entire suite, including a real
  two-socket handshake, *because sipx was talking to sipx*.
- **The fingerprint check happens where the TLS stack cannot see it.** RFC 5763 §5 expects a
  self-signed certificate, so there is no chain to validate; what authenticates it arrived in the
  signalling. `establish` performs §6.2's check before returning any keys, and a peer that sent no
  fingerprint is refused *before* the handshake runs — an unverified handshake authenticates
  nobody, and discovering that afterwards means having established a channel to an unknown party.
- **The C dependency is not load-bearing.** Everything RFC 5764 *decides* — the
  `a=fingerprint`/`a=setup` negotiation, §5.1.2's demultiplexing, §4.2's key derivation, §6.2's
  check — compiles always, behind a `Handshake` trait; only the handshake itself is behind the
  off-by-default `dtls` feature. A pure-Rust DTLS was considered and rejected: there is none with
  comparable scrutiny, and a hand-rolled handshake for a security-critical protocol is the liability
  this project declines elsewhere — the same reasoning that has SRTP's AES come from RustCrypto.

## More than one call

**A bridge forwards; a conference is a clock.** That distinction is `M-11` and `M-12`'s shared
finding, and it is why they are two implementations rather than one generalised mixer. A bridge can
forward each packet as it arrives because there is exactly one place to send it. A mixer has to
decide *when* a frame is complete while waiting on N participants who will not arrive together — so
one task ticks at the packet interval, and a participant who has said nothing contributes silence.
Waiting for everyone would make the whole conference as late as its worst connection.

**A bridge passes bytes through when the codecs match**, on a raw path added to the session:
`set_relay` makes a session hand packets on still encoded, and `send_encoded` puts them back on the
wire on the other leg's own sequence and timestamp. **The obvious argument for this is false for the
codec sipx ships today**, and `M-11` corrected it rather than leaving it: G.711's decode is exactly
invertible over all 256 codes, so pass-through saves CPU and nothing else. The generational-loss
argument is real for Opus, and building the path now means Opus arrives into a bridge that already
does the right thing. Differing codecs are transcoded and the fact is *reported*, because somebody
looking at a call that sounds worse than it should is entitled to find out why from the software
rather than by reasoning about it.

**A conference cannot pass bytes through at all** — mixing happens on samples, so every leg is
decoded in and encoded out. `M-12` is explicit that this is not an optimisation left for later:
adding two µ-law codes is not adding two amplitudes. Mixing saturates rather than wraps, because
wrapping turns a loud moment into a loud click, and every participant's mix excludes their own
audio, because hearing yourself delayed is the single most disorienting artefact in conferencing.

Two testing rules fell out and are worth repeating in any story that touches these: **a bridge is
tested with four sessions, not two** — a bridge is between *calls*, and with two, a bridge that
mixed its legs up still passes — and **a conference is tested with three parties, not two**, because
with two, "everyone else" and "the other one" are the same set, so an implementation that simply
echoed would pass.

Ownership is enforced by `Drop`. A dropped bridge aborts both directions, because otherwise two
tasks go on forwarding audio between calls nobody holds a handle to — and the tasks keep the
sessions alive through their `Arc`s, so the sockets never close.

## Control surface: playback, mute, hold

`M-17` (playback control) and `M-18` (mute) are media stories delivered under the `app-sdk` epic,
and **their decisions are recorded in [`designs/app-sdk.md`](app-sdk.md), not here.** In summary, so
that a reader arriving from this record knows whether to go and look:

- **Clips queue, they do not replace.** Replacement would make stopping an implicit side effect of
  starting; the two verbs stay separate and composable.
- **Mute substitutes encoded silence; it does not stop the stream.** Suppressing packets closes the
  NAT pinhole, leaves the far end's jitter buffer to restart on unmute so the first word after it is
  clipped, and makes "muted" indistinguishable on the wire from "the far end has gone away".
- **RFC 3550 §6 fixes where the gate goes, not just what it does.** A sender report's packet and
  octet counts (§6.4.1) describe what actually went on the wire, so the mute gate sits *before the
  packet is built*. One step later — build the packet, drop the datagram — would overstate this
  side's own reports *and* manufacture apparent loss at the far end out of a caller who was merely
  quiet. This is the rule any future media gate has to be checked against.
- **DTMF is not gated.** A telephone event is generated by this endpoint on purpose, the way a
  keypad tone is on a handset, so a muted caller can still answer an IVR.

**Mute is not hold, and the two must not be confused at any layer.** Hold is a state two parties
agree on: an `a=sendonly`/`a=inactive` re-INVITE the far end sees, can refuse, and answers by
playing hold music. Mute is a decision one party makes about its own microphone — no re-INVITE, SDP
direction unchanged, far end's hold state untouched. `M-8` implemented hold as a *direction* rather
than a separate state, so `sendonly`, `inactive` and the way back all fall out of one code path.

`M-8` also fixed the rule for renegotiation, which is the same rule in a different dress: **a
renegotiation that fails leaves the call running.** A re-INVITE tries to change something that
already works, so an unusable offer gets 488 and the existing session continues — tearing the call
down would lose a call that was fine a moment ago. And the media session is rebuilt only when the
address or codec actually changed: some peers send a re-INVITE every thirty seconds as a keep-alive,
and restarting an unchanged session would drop packets each time for nothing.

## Alternatives considered

- **Depend on an existing Rust RTP/SDP crate ecosystem.** Rejected before `M-1`: it couples the
  stack to another project's API and pulls a large dependency tree oriented around browser media,
  for code sipx can own in a few hundred lines. Still the position — the stack is four crates and
  three of them have no runtime.
- **Async throughout, a task per protocol machine.** Rejected for the transaction layer in
  [`designs/sip-core.md`](sip-core.md), and rejected again for the ICE agent for the reasons argued
  above. *Not* rejected for the media session, which has no protocol machine in it.
- **Put the STUN-for-ICE codec in `sipx-rtp`**, where its crypto dependencies already were.
  Rejected in `M-20`; the reasoning is above.
- **A pure-Rust DTLS implementation.** Rejected in `M-15`: none with comparable scrutiny exists, and
  a hand-rolled handshake for a security-critical protocol is exactly the liability this project
  declines elsewhere.
- **Ship no Opus until a maintained pure-Rust binding exists.** `M-13` records this as *entirely
  defensible* and did not take it: the pure-Rust crates decode and do not encode, and a codec sipx
  can decode but not encode is one it cannot offer. What was done instead is bounded — the advisory
  on the FFI layer is excepted narrowly with the reasoning written into `deny.toml`, CI installs
  `libopus-dev` so the CMake pin the advisory names is never reached, and the codec is behind a
  non-default feature. `M-13` asks for a second opinion on this and has not had one.
- **Replace the fixed jitter buffer with the adaptive one.** Rejected in `M-9`: the fixed buffer is
  the control the adaptive one is measured against, and without it "adaptive" is an unfalsifiable
  claim.
- **Implement RFC 3550 §6.2's full RTCP interval arithmetic.** Rejected in `M-6`: for a two-party
  call it always returns the five-second minimum, so the calculation would be a place for a bug and
  not a feature. It becomes wrong the moment sipx reports for a session with more than two members.
- **Suppress packets while muted**, and **replace the playback queue on a second `play`.** Both
  rejected; see [`designs/app-sdk.md`](app-sdk.md).
- **Aggressive ICE nomination.** Not an option and will not become one: RFC 8445 §4 says it "has
  been deprecated in this specification", and `M-16`'s Acceptance forbids implementing it *even as
  an option*.

## Risks & open questions

- **Drift and pacing over a long call.** The send loop uses one `tokio::time::interval` with
  `MissedTickBehavior::Delay`, which prevents a burst of catch-up packets after a stall but does not
  correct accumulated drift. No story records that choice; see the gaps.
- **The sans-IO boundary is enforced for two crates, not four.** [AGENTS.md](../../AGENTS.md)
  non-negotiable 2 names `sipx-sip` and `sipx-sdp`. `sipx-rtp` and `sipx-audio` are pure as a matter
  of fact rather than of rule, and nothing in the gate would notice a `tokio` line added to either.
  `sipx-rtp` has already accreted one clock read, for a good reason, and that reason is not written
  anywhere the next person would look before adding a second.
- **A bridge's receiver falling behind.** The outline this record replaces asked what to do —
  dropping audio is correct, but only if it is measured and reported. The conference bounds each
  participant's buffer at half a second of audio; whether a bridge reports its drops is not
  recorded.
- **Interop is the untested axis for the security stories.** `M-15`'s surviving mutation was
  invisible because sipx was talking to sipx; `X-17` exists to find that class of bug against a
  foreign implementation.
- **`M-13`'s dependency exception is a judgement call**, flagged as wanting a second opinion by the
  story that made it, and not yet given one.

## Decisions this record could not find

An invented rationale in a design record is worse than an admitted gap, because the next reader
believes it. These are outcomes with no recorded reason found in a story, a spec, a commit message
or the code:

1. **Why the jitter buffer lives in `sipx-rtp`.** The outline asked *whether the jitter buffer is in
   the media session or the call layer — it affects who owns playout timing*, and the delivered
   answer is neither: the buffer is in `sipx-rtp` and the session owns the pop cadence. No story
   argues the placement. The nearest thing to a rationale is `M-9`'s remark that time-scale
   modification "belongs in the media layer", which draws the boundary without justifying it.
2. **Why `MissedTickBehavior::Delay`.** Set in the commit that introduced media sessions, never
   commented on, and untouched by the two later commits that moved the surrounding lines.
3. **Why `M-14` and `M-15` shipped without a spec** when `M-16` wrote one first for a comparable
   subsystem. Both stories are unusually thorough about *what* they decided; neither says why the
   spec-before-code rule was not applied to them.
4. **Whether a bridge reports dropped frames when a leg falls behind.** The outline asked; nothing
   answers.

*Why G.722 was dropped* was the third entry here when `X-25` wrote this list. It is no longer a
gap: `X-26` took the decision rather than looking for one that was never made, and it is recorded
under the codecs above.

*SRTP and DTLS-SRTP having no spec* was a gap in the list above until `M-25` wrote
[`specs/srtp.md`](../specs/srtp.md). Entry 3 here is not closed by it: `M-25` wrote the spec, it did
not discover why the two stories skipped it, and `X-25` had already looked. What `M-25` did add is
the cost of the omission rather than the reason for it — five code/RFC disagreements found on the
first careful reading, listed in that spec's §12 — which is the more useful half of the answer if
the question is ever asked again about a different subsystem.

## Acceptance / done

The `media` epic's own bar was met by `M-5`: two sipx endpoints exchange G.711 audio that passes a
bit-exactness check, with RTCP statistics reported and a jitter buffer that survives injected loss
and reordering. That end-to-end test also asserts the recording is loud enough to be the tone, so a
test that recorded silence of the right length cannot pass.

What this **record** owes its reader is different, and is the standard to hold it to when it is next
edited: every decision above is traceable to a story, a spec section, a commit or the code, and
every decision that is not appears in the list of gaps rather than in the prose.
