# Spec: the browser signalling transport

**Status:** normative for the binding · **Epic:** `browser-sdk` · **Story:** `T-33` ·
**Parent contract:** [browser-sdk.md](browser-sdk.md) · **Implementation:** `browser/src/` ·
**Evidence:** `browser/test/`, `scripts/check-browser-binding.sh`

[browser-sdk.md](browser-sdk.md) is the parent and it wins. This document specifies only what its
§11 assigns to `T-33` — "§4.7 entropy feed, §4.5 timers, WSS socket ownership, §8.6 enforcement" —
and adds the bounds a transport needs and an ABI does not have: how many messages may wait, how
many times a connection may be attempted, and what happens at each edge. It restates none of the
parent's boundaries and defines no new command or event vocabulary; a convenience that cannot be
expressed over the parent's §4 and §5 does not belong here either.

[sip-tls.md](sip-tls.md) §4 is normative for SIP over WebSocket and is not restated: the `sip`
subprotocol, the frame boundary being the message boundary, and the message limit doubling as the
decoder limit are its rules, applied here at the browser's edge.

## 1. What owns what

The binding is the only part of the browser SDK that touches the network. It owns one `WebSocket`
at a time, the queues in front of and behind it, every host timer the kernel asked for, the draw on
the platform CSPRNG, and the decision to stop trying. It owns no protocol state: it does not parse
SIP, retransmit, retry a request, or interpret any kernel event beyond the two it is obliged to act
on (§6).

The kernel owns everything else. The consequence worth stating: **the binding is not a second
parser.** Every byte arriving from the peer is attacker-controlled (parent §8.1), and a host that
looked for message boundaries would be a second decoder on the same hostile input, in the language
with the weaker guarantees. It offers each WebSocket message to the kernel exactly as it arrived.

## 2. The URL, and the security of the transport

The URL is built from the `BSDK-CFG` `transport` block and nothing else.

| Rule | Behaviour |
|---|---|
| `wss` | the default and the contract |
| `ws` without `"insecure":"allow-development"` | refused, `insecure-scheme` |
| `ws` with the policy, to a non-loopback host | refused, `cleartext-not-loopback` |
| `ws` with the policy, to loopback | permitted |
| a scheme that is neither | refused, `unsupported-scheme` |
| userinfo in `host` | refused, `credentials-in-url` |
| a query or fragment in `resource` | refused, `resource-carries-query` |
| `resource` neither empty nor beginning with `/` | refused, `invalid-resource` |
| a host the URL parser rewrites | refused, `invalid-host` |

Every refusal is thrown from the constructor, before anything is allocated: parent §6.2 does not
allow a half-started client, and a transport that discovered its own configuration was refused
halfway through a registration would have no honest way to finish.

**Loopback is part of the cleartext rule, not an extra.** Parent §8.6 states the flag exists "for
local development against loopback" and hands `T-33` the socket-side enforcement. A development
flag that also reaches a routable host is a production deployment with a typo in it — and by then
the kernel has already stopped answering digest challenges (parent §8.6), so the page fails to
register anyway, later and with a worse error. Loopback is `localhost` (RFC 6761 §6.3),
`127.0.0.0/8` (RFC 5735) and `[::1]`, and nothing else whatever a hosts file says.

**Credentials cannot enter the URL by any route.** Userinfo is refused, and so is a query string —
which is where a bearer token would otherwise be written. No thrown error, log line or typed event
quotes the configuration, carries an authorization header, a digest response or an entropy octet
(parent §8.3). The refusal messages are fixed strings.

The resource is RFC 7118's and is not fixed: [sip-tls.md](sip-tls.md) §4 makes both `/` and `/ws`
conformant, so a non-root path is ordinary. An empty resource means the root.

## 3. Bounds

Every number is derived, and `browser/test/transport.test.mjs` holds the derived ones to the
kernel's own constants in `crates/sipx-wasm/src/bounds.rs` so the two cannot drift apart silently.

| Bound | Value | Where it comes from |
|---|---:|---|
| one received message | 64 KiB | parent §4.9's inbound bound, enforced before the bytes are copied so an oversize frame buys no allocation |
| send queue | 256 records **or** 256 KiB | parent §4.9's `MAX_QUEUED_RECORDS`/`MAX_QUEUED_BYTES` |
| platform outbound buffer | 256 KiB | the same pair, applied to `bufferedAmount` |
| receive queue (the inbox) | 256 entries **or** 256 KiB | the same pair |
| connection attempts, per binding | 7 | §4 |
| reconnect delays | 500, 1000, 2000, 4000, 8000, 16000 ms | §4 |
| one connection attempt | 32 000 ms | 64 × T1 |
| entropy refill | 960 octets | parent §4.7's capacity (1024) minus its low-water mark (64) |

The queue bounds are the kernel's own pair on purpose. That pair is the kernel's statement of how
much in-flight signalling is a defect rather than a burst, and the host has no better information
about where that line falls. It is also exactly tight for the send side: one drain can hand the
binding no more than the kernel was willing to queue.

The entropy refill is the headroom above the low-water mark, which makes it the largest feed that
can never overflow the pool — the kernel asks only when the pool is *below* the mark, so the
binding never needs to read the pool level to size a feed and parent §4.7's `E_BOUNDS` is
unreachable from this path.

**The inbox is one queue, not three.** A received message, a fired timer and a submitted command
that arrive in the same turn must reach the kernel in a defined order, because the kernel's output
order is reproducible only if its input order is total; "the order they arrived" is the only order
that needs no arbitration. Fired timers are additionally bounded by parent §4.9's 128 pending
timers, so the count above is reached by inbound traffic or by commands.

**Every overflow fails closed.** A queue at its bound emits `overflow` and then `closed`, with
`send-overflow` or `receive-overflow`. The binding never grows a queue past its bound, never drops
a message silently to stay under one, and never blocks.

## 4. Connecting, and the end of trying

One attempt: construct the socket, ask for the `sip` subprotocol, and wait at most 32 000 ms —
64 × T1, because a handshake that has not completed by then is of no use to any transaction that
was waiting for it, and the platform's `WebSocket` has no connect timeout of its own.

The attempt budget is **7 for the lifetime of one binding**, and it is deliberately a lifetime
budget rather than a per-episode one. Replenishing it on a successful open turns a peer that
accepts and immediately drops into an unbounded retry loop, which is exactly what this story
forbids. Retry policy past the budget belongs to the application (parent §5.3), whose move is to
construct a new binding — a decision it takes deliberately, in response to the typed `closed`
event this one ends with.

The delays before attempts 2 through 7 double from RFC 3261's T1: 500, 1000, 2000, 4000, 8000,
16 000 ms. They sum to 31 500 ms, which is 63 × T1, so the last attempt begins strictly inside
RFC 3261's 64 × T1 give-up horizon (Timer B and Timer F,
[sip-transaction.md](sip-transaction.md)). A transport that reconnected later than that could not
rescue any transaction the kernel still holds open.

Two endings are not retried at all, because retrying a deterministic refusal proves nothing:

- **the subprotocol was not selected** — the same peer will refuse it on the next socket;
- **the peer framed wrongly** (§5).

**Offline.** While the browser reports itself offline the binding attempts nothing, emits `offline`
once, and waits. Coming back emits `online` and resumes — with the budget it had, never a fresh
one. An offline period buys patience, never attempts.

**Every message queued for a socket dies with that socket.** A SIP message serialised for a
connection that no longer exists is precisely the message the kernel's transaction layer will
produce again if it still wants it, and replaying one onto a fresh socket would put a
credential-bearing request on the wire that nothing asked for. This is the close-during-
authentication case: the answer to a `401` is discarded with its connection, typed as `discarded`,
and the kernel — not the binding — decides whether anything replaces it.

## 5. Framing

[sip-tls.md](sip-tls.md) §4 is normative and unambiguous: over WebSocket the frame boundary is the
message boundary, a message split across frames is malformed, two messages in one frame likewise,
and **both close the connection rather than being patched up**.

The binding applies that rule without being able to evaluate it, which is the whole design. It
offers each frame to the kernel whole and once, holds no cross-message buffer to join a fragment
with and no boundary search to split a coalesced frame, and then asks the kernel — through the
parent §4.11 snapshot's `parse_errors` — whether the message parsed. A raised count closes the
connection with `framing`. The snapshot is read only when the input produced no records at all,
which a parse failure always does and a parsed message that had nothing to say sometimes does, so
a healthy connection pays for the read rarely.

The consequence, stated because it is a real limit and not a rounding error: **the binding refuses
exactly what the kernel refuses.** A frame the kernel accepts is accepted here. Both halves of the
RFC 7118 §5 rule are the kernel's to reach — parent [browser-sdk.md](browser-sdk.md) §4.3.1 makes a
coalesced frame, and a frame with any trailing octets, count into `parse_errors` exactly as a
fragment does, with nothing acted on from the part that parsed.

A frame whose bytes are not available synchronously — a `Blob` — is refused rather than read.
Awaiting one read per message lets a peer that sends two in the same turn have them delivered in
either order, and a transaction state machine cannot survive its inputs being permuted. The socket
is put in `arraybuffer` mode before the handshake completes, so this is unreachable in practice and
is a guard rather than a path.

## 6. Timers, entropy and re-entry

**A timer re-enters only as a fired-timer input.** A `TIMER_SET` record becomes one host timer at
its absolute deadline; a `TIMER_CANCEL` clears the one it names. When the host timer comes due, its
callback appends to the inbox and returns — it does not call the kernel. The kernel is entered from
exactly one place, in a deferred task, which is what parent §4.5 means. A callback for a timer the
binding no longer owns is counted and dropped: `clearTimeout` cannot unring a callback the platform
has already taken off its queue, so that race is survivable rather than assumed away.

`now_ms` is read when an input is driven rather than when it arrived. Since the inbox is FIFO and
the clock is monotonic, parent §4.5's non-decreasing requirement then holds by construction. It is
truncated per parent §4.2 and clamped against the last value handed over.

**Entropy is seeded before the first command and refilled on demand.** A fresh kernel's pool is
empty and parent §4.7 fails an identifier derived from an insufficient pool whole, so a binding
that waited to be asked would earn that failure on its first command. Bytes come from
`crypto.getRandomValues` and from nothing else; there is no fallback path in this package and
`Math.random` appears in none of its files (parent §8.4). Two guards keep the refill from becoming
a loop: a demand raised while a refill is already queued is the same demand, and a demand raised
*by* a refill is impossible for a kernel keeping §4.7 and is treated as the kernel defect it is.

## 7. Cancellation

One path, synchronous and idempotent, in parent §6.5's order: clear every host timer, drop both
queues, release the connectivity subscription, **free the kernel (step 4), then close the socket
(step 5)**, then deliver `closed` as the final event. Nothing follows it.

Synchronous by necessity: `pagehide` gives a page no task in which to finish anything.

A callback that fires after cancellation — a platform timer already dispatched when `clearTimeout`
ran, or a socket event dispatched before `removeEventListener` took effect — reaches neither the
kernel nor a listener, and is counted rather than ignored.

## 8. Events

Every event carries a `type`; no event carries credentials, SIP bytes, an authorization header or
an entropy octet. `connecting`, `open`, `disconnected`, `discarded`, `reconnect-scheduled`,
`offline`, `online`, `overflow`, `kernel` and `closed`. `closed` is terminal. The reason codes are
listed in `browser/src/transport.mjs`; both sets are open for appending and closed for
redefinition, matching the rule parent §4.10 sets for the ABI's codes.

Delivery is always a separate task: no listener runs inside an SDK method call, a socket handler
or a timer handler (parent §6.4 rule 2). A throwing listener is counted and stops neither its
siblings nor the next event.

## 9. Deliberately not here

- **Reconnection past the budget, and any retry of a SIP request.** Both belong to the application
  and to the kernel respectively (parent §5.3).
- **A `SipxClient`, promises or `AbortSignal`.** Parent §6 is `A-17`'s handwritten layer.
- **The generated ABI glue.** Parent §7.2 makes it generated from a checked Rust source; the hand
  written port in `browser/test/kernel.test.mjs` is a test fixture and is expected to be deleted.
- **Anything to do with media.** `RTCPeerConnection` is `M-52`'s.
- **A keep-alive.** [sip-tls.md](sip-tls.md) §4's 25-second Ping is a native-transport facility;
  the browser's `WebSocket` exposes no ping, and a SIP-level keep-alive would be kernel vocabulary.
  A registration whose connection has silently died is still detected — by the refresh that fails.
