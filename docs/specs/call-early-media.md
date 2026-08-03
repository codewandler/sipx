# Early media on an INVITE dialog

**Status:** normative · **Story:** `C-2` · **RFCs:** 3261, 3262, 3264, 3960

This specification defines when the call layer starts media before an INVITE receives a final
response, who owns that session, what the application observes, and how the same session becomes
the confirmed call's media. It extends the reliable-provisional machinery specified by RFC 3262;
it does not add another offer/answer exchange.

## 1. Scope

sipx implements the **gateway model** of RFC 3960 section 3: one regular SDP offer/answer exchange
on the INVITE's own early dialog. The INVITE carries the offer. A reliable provisional response
carries the answer. The negotiated bidirectional session may then carry an announcement, network
ringing, or caller audio before the final response.

The application-server model of RFC 3960 section 4 and RFC 3959's `early-session` disposition are
out of scope. They require a second offer/answer axis. Selecting among simultaneous media from
several forked early dialogs is application policy; sipx never mixes branches or silently changes
which dialog a handle names.

## 2. Normative requirements

1. An SDP answer before the final response MUST travel in a reliable provisional response, as
   required by RFC 3262 section 5. An unreliable provisional body does not complete negotiation
   and MUST NOT start media.
2. The UAS MUST NOT send a 2xx final response while the reliable provisional carrying the answer
   remains unacknowledged (RFC 3262 sections 3 and 5).
3. Once the answer has been sent by the UAS or accepted by the UAC, that role MUST start its media
   session immediately. Starting means the RTP/RTCP workers are running and the application can
   send and receive through the session; it does not wait for the INVITE's 2xx.
4. The UAC MUST expose an `EarlyMediaStarted` event when it accepts the provisional answer. The
   event is the RFC 3960 section 3.2 signal that tells an application to stop locally generated
   ringing and render the remote stream. It is emitted once per early session, after the session
   has started, never for a bodiless provisional, and before `Answered`.
5. `Ringing` and `Dialing` own the running early `MediaSession`. Their media accessors borrow it;
   no application-facing clone shares ownership.
6. A 2xx confirming the same early dialog MUST move that exact running `MediaSession` into `Call`.
   Confirmation MUST NOT bind a second socket, start replacement workers, re-derive SRTP keys, or
   stop and restart the session. `Answered` is a signalling transition, not a media transition.
7. A final refusal, local cancellation, remote CANCEL, a losing fork, or dropping the owning early
   handle MUST drop or stop that branch's `MediaSession`. `MediaSession::Drop` is the terminal
   cleanup: it closes the queues, stops every worker and releases the sockets once their bounded
   loops observe the stop signal.
8. A reliable provisional retransmission MUST NOT start a second session or emit a second
   `EarlyMediaStarted` event. Its `RSeq` is acknowledged as usual.

## 3. Types and ownership

`Ringing` is the UAS owner before confirmation. `ring_early*` completes negotiation, starts the
session, sends the reliable provisional, and returns `Ringing`. `Ringing::media` exposes the
session for an announcement or for receiving caller audio. `answer_early` consumes the early
session out of `Ringing` and places it in `Call` after enforcing the PRACK barrier.

`Dialing` is the UAC owner before confirmation. When it accepts an answer from a reliable
provisional it turns the already-bound `MediaPort` into a `MediaSession`. `Dialing::media` exposes
that session. When `dial_early` returned on a bodiless provisional,
`Dialing::wait_for_early_media` drives later provisionals without consuming the handle and returns
when the session starts or a final response wins. `Dialing::events` exposes the event stream which
continues on the confirmed `Call`; the receiver is handed out once, exactly as `Call::events` is.
`Dialing::answered` moves the session and event sink into `Call`.

The following ownership chain is exhaustive:

| Phase | UAS owner | UAC owner | Media value |
|---|---|---|---|
| INVITE offer open | none | `Dialing` | bound `MediaPort` |
| reliable answer sent/accepted | `Ringing` | `Dialing` | running `MediaSession` |
| same dialog confirmed | `Call` | `Call` | the same running `MediaSession` |
| branch ended | none | none | stopped and dropped |

## 4. State tables

### 4.1 UAS

| State | Input | Action | Next state |
|---|---|---|---|
| offered | `ring_early*` and caller offered `100rel` | settle SDP, bind and start media, send answer with `Require: 100rel` and `RSeq` | early-running/unacknowledged |
| early-running/unacknowledged | matching PRACK | answer PRACK 2xx, stop provisional retransmission | early-running/acknowledged |
| early-running/unacknowledged | `answer_early` | return `UnacknowledgedProvisional`; leave owner and media unchanged | early-running/unacknowledged |
| early-running/acknowledged | `answer_early` | send bodiless 2xx; move session into `Call` | confirmed-running |
| either early-running state | CANCEL, refusal, or owner drop | stop session and provisional retransmission | ended |

### 4.2 UAC

| State | Input | Action | Next state |
|---|---|---|---|
| offered | unreliable provisional, with or without SDP | record ringing only; do not accept SDP or start media | offered |
| offered | reliable provisional without SDP | PRACK; record ringing | offered |
| offered | reliable provisional with usable SDP answer | accept answer, start session, PRACK, emit `EarlyMediaStarted` | early-running |
| early-running | retransmission or later provisional | PRACK as required; do not restart or re-emit | early-running |
| early-running | same-dialog 2xx | ACK; move session into `Call`; emit `Answered` | confirmed-running |
| offered | 2xx with answer | settle and start once, ACK, emit `Answered` only | confirmed-running |
| either pre-final state | final refusal, cancel, timeout, or owner drop | stop/drop port or session | ended |

## 5. Timers and cancellation

C-2 adds no clock. Reliable provisionals retain RFC 3262's T1-doubling retransmission schedule and
64*T1 bound. The INVITE retains its transaction and caller-configured deadline. Media workers use
the bounded packet and RTCP intervals already specified by the media runtime. Early-media cleanup
is cancellation-safe because it uses the session's existing stop token and `Drop`; no detached
cleanup task and no fixed sleep is introduced.

## 6. Byte-level vectors

The examples show only the headers and bodies relevant to this contract. `V1` is the positive
gateway exchange used by `a_caller_receives_early_media_before_the_call_is_answered`.

### V1 — answer in a reliable 183

```text
INVITE sip:callee.example SIP/2.0\r\n
Supported: 100rel\r\n
Content-Type: application/sdp\r\n
Content-Length: ...\r\n
\r\n
v=0\r\n...m=audio 40000 RTP/AVP 0\r\n

SIP/2.0 183 Session Progress\r\n
Require: 100rel\r\n
RSeq: 1\r\n
Content-Type: application/sdp\r\n
Content-Length: ...\r\n
\r\n
v=0\r\n...m=audio 40002 RTP/AVP 0\r\n

PRACK sip:callee.example SIP/2.0\r\n
RAck: 1 1 INVITE\r\n
Content-Length: 0\r\n
\r\n
```

Expected after the 183 is accepted: both early handles expose running media; the UAC event order
is `Ringing { reliable: true }`, `EarlyMediaStarted`; audio sent by the UAS is receivable before
any final response. After the PRACK is answered, a bodiless same-tag 200 to the INVITE moves both
sessions into their calls and adds `Answered` without changing either local RTP address.

### V2 — unreliable SDP is not an early answer

```text
SIP/2.0 183 Session Progress\r\n
Content-Type: application/sdp\r\n
Content-Length: ...\r\n
\r\n
v=0\r\n...m=audio 40002 RTP/AVP 0\r\n
```

Expected: no PRACK, no media session, and no `EarlyMediaStarted`. A later 2xx still has to carry a
usable answer to the INVITE offer.

### V3 — the 2xx barrier

Given V1 before its PRACK, `answer_early` returns `UnacknowledgedProvisional`, sends no 2xx, and
leaves the early session running under `Ringing`. After the matching PRACK, the same call succeeds.
