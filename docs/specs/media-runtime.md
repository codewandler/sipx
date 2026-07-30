# Media runtime construction and ownership

**Status:** normative · **Stories:** M-35, M-36, M-37

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

## 4. Test vectors

| Vector | Input | Required result |
|---|---|---|
| T1 | `packet_duration = 0` | typed setup error; requested bind address remains available |
| T2 | `rtcp_interval = Some(0)` | typed setup error; requested bind address remains available |
| T3 | packet and RTCP intervals of 1 ms | session starts and remains able to send |
| T4 | conference interval of 0 | typed conference error; no mixer starts |
| T5 | conference interval of 1 ms | conference starts and can be closed repeatedly |
| C1 | drop a conference whose collector is blocked in `recv()` | collector is cancelled and its session `Arc` is released within a bounded deadline |
| C2 | leave, close twice, then drop | no retained participant and no panic |
| C3 | race `join` against `close` while the participant is quiet | either join is refused or its registered collector is drained; no retained session |
| C4 | cancel `close` while it waits for the member map | conference remains open and retryable; a later close releases every session |
| O1 | Opus encoder construction is refused | typed encoder setup error; no direct G.711 encoder exists in the resulting state |
| O2 | Opus decoder construction is refused | typed decoder setup error; no direct G.711 decoder exists in the resulting state |
| O3 | successful Opus on dynamic payload type 96 | emitted RTP names 96 and carries Opus bytes |
