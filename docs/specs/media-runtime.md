# Media runtime construction and ownership

**Status:** normative · **Stories:** M-32, M-35, M-36, M-37, P-15

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
unprotect failures; packets from a foreign SSRC; completed DTMF digits refused by the
application queue; unknown RTP payload types; playback completion reports with no listener; ICE
driver datagrams and data-sent notes refused by its queue; renegotiation replies with no listener;
ICE outputs failing to send; redundant server-reflexive candidates; and
non-STUN-server datagrams consumed while gathering.

An ICE output naming no bound socket has no counter: it is structurally unreachable because every
base the agent can name was created from the exact socket vector the driver owns. The site carries
that reason instead of a field permanently stuck at zero.

The SRTP and SRTCP protect-error branches likewise have no counters. Their only errors are short
headers, while those branches receive bytes from `Packet::encode` and `Rtcp::encode_compound`, which
always make complete headers. Authentication failures on unprotect are reachable from network input
and are counted. This distinction avoids publishing protect counters structurally stuck at zero.

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

## 5. Test vectors

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
