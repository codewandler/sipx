# Sans-I/O SIP event client

This specification defines the reusable subscriber half of SIP-specific event notification. It is
the contract for `S-38`; it does not implement an event package, open a socket, read a clock, or own
an async task. A driver supplies received messages and fired timers as inputs and performs the
ordered outputs.

The words **MUST**, **MUST NOT**, **SHOULD**, **SHOULD NOT**, and **MAY** are normative.

## 1. Normative references and precedence

- RFC 3261 §§8, 12, 17, 22 and 24 define requests, dialogs, transactions, CSeq ordering and digest
  authentication.
- RFC 3265 §§3.3.4 and 4.4.9 document the original event framework, including the initial-NOTIFY
  race and package-defined forking policy.
- RFC 6665 obsoletes RFC 3265. Its §§3.1, 4.1, 4.4.1, 5.4 and 8.2 define the subscriber behavior in
  this document. Where RFC 3265 and RFC 6665 differ, RFC 6665 controls.
- [`sip-auth.md`](sip-auth.md) defines supported digest challenges, selection, credential secrecy and
  fresh client nonces; this client does not define a second authentication policy.

The generic client implements the framework only. The selected event-package specification still
controls package names, default and acceptable durations, accepted media types, neutral state,
payload meaning and whether a body is complete or partial.

## 2. Boundary and vocabulary

One `EventClient` contains independent `Subscription` values. A subscription is identified locally
by an opaque `SubscriptionId`; wire matching uses the initial SUBSCRIBE's Call-ID, local From tag,
exact Event package token and exact Event `id` parameter. Other Event parameters do not participate
in matching, per RFC 6665 §8.2.1.

The core receives only these classes of input:

| Input | Data |
|---|---|
| `Start` | resource URI, event token and optional `id`, requested expiry, Accept values, optional bounded SUBSCRIBE body, credentials, driver-supplied fresh Call-ID/From tag/initial CSeq, selected SUBSCRIBE target, injected NOTIFY trust policy and package consumer |
| `Response` | transaction identity and one parsed SIP response or transaction failure |
| `Notify` | server-transaction identity, parsed request, source target and transport facts |
| `TimerFired` | subscription identity, timer kind and generation |
| `Unsubscribe` | subscription identity |
| `Shutdown` | one global shutdown deadline |
| `ConsumerDrained` | subscription identity and number of queued application notifications removed |

It produces ordered outputs only:

| Output | Meaning |
|---|---|
| `SendSubscribe` | transaction-independent SUBSCRIBE plan plus target; the driver supplies fresh branch/cnonce material, renders the complete request and creates the client transaction |
| `RespondNotify` | final response on the supplied NOTIFY server transaction |
| `Deliver` | typed framework metadata and the package consumer's value |
| `ArmTimer` / `CancelTimer` | timer identity, generation and relative duration |
| `StateChanged` | typed lifecycle and termination/retry facts for the application |
| `Stopped` | no subscription, timer, queued notification or request remains owned |

Every input is atomic. Outputs are applied in order. In particular, `RespondNotify(200)` precedes
`Deliver`; a slow application receiver can never hold a NOTIFY transaction open.

The I/O-facing driver obtains entropy: it supplies fresh Call-ID and From-tag values on `Start`, then
fresh branch and digest-cnonce material while applying each `SendSubscribe`. The core owns their
protocol placement and retry invariants but never reads operating-system entropy.

### 2.1 Injected package consumer

`Start` supplies one package consumer selected by the host. The consumer declares:

- the exact Event token and optional `id` it accepts;
- allowed NOTIFY media types and whether an empty terminal body is valid;
- a neutral state emitted until the first accepted NOTIFY;
- a synchronous, sans-I/O `consume(content_type, body)` operation returning either an owned value or
  a typed rejection status.

The callback MUST be finite, MUST NOT perform I/O, MUST NOT read time and MUST NOT retain borrowed
message bytes. The generic client imports no registration, discovery, presence, call or UI type. It
understands dialog and `Subscription-State` metadata, never package payload semantics.

### 2.2 Injected NOTIFY trust policy

Matching SIP dialog fields proves correlation, not authority: they are visible on the wire. `Start`
therefore supplies a synchronous, sans-I/O trust policy. Before accepting a NOTIFY dialog, CSeq,
Contact, state or body, the core calls it with the resource URI, selected SUBSCRIBE target, received
source/transport/connection identity and parsed request. It returns `Accept` or `Reject`; rejection
produces 403 and changes no state. The callback has the same finite/no-I/O/no-clock restrictions as
the package consumer.

The default is fail-closed `SamePeer`: the source socket address and transport MUST equal the target
used for the current SUBSCRIBE transaction; on a connection-oriented transport the connection
generation MUST also match. Deployments that receive NOTIFY through a trusted proxy must inject an
explicit allow-list or authenticated-identity policy. There is no accept-any default.

### 2.3 Complete transport targets

`Peer` is a sans-I/O description, but it MUST retain every fact the endpoint driver needs to
reconstruct the selected transport target: socket address, transport, optional stream generation,
optional TLS certificate identity and optional WebSocket resource. Identity and resource are
bounded owned strings supplied on `Start`; the core neither resolves nor verifies them.

Every `SendSubscribe` before a target refresh carries those facts unchanged. The live driver MUST
rebuild `Target` with the certificate identity and WebSocket resource rather than using address and
transport alone. A target refresh from a NOTIFY Contact may change the address but inherits the
selected secure identity/resource unless an injected routing policy replaces them. Losing either is
a security failure: an address without its DNS-derived identity cannot select the intended
certificate, and a WebSocket connection for `/` is not authority for `/sip`.

The endpoint reports the exact stream generation selected for each outbound SUBSCRIBE. The driver
records it before accepting a NOTIFY, and inbound requests carry the generation of the stream that
delivered them. `SamePeer` therefore compares two endpoint-issued generation identifiers rather
than an inferred or stale pool value. UDP has no generation. For an established dialog the first
route-set URI is the transport next hop even when strict routing rewrites the request URI; without a
route set, the current remote target is the next hop.

Route-hop transport selection is the pure URI rule shared with RFC 3263 resolution. An explicit
port always wins; otherwise `sip;transport=tcp` uses TCP/5060, `sip;transport=ws` uses WS/80,
`sips;transport=tcp|tls` uses TLS/5061, and `sips;transport=ws|wss` uses WSS/443. A `sips` route
requesting UDP is a typed `UnsupportedRouteTransport` termination and MUST NOT emit a downgraded
request. TLS/WSS verification authority is the selected route URI's host, never the Contact host or
the address inherited from the previous target. A Contact-only target refresh changes the remote
address and explicit port while retaining V14's selected transport, certificate identity and
WebSocket resource. A stream generation is retained only while all resulting next-hop selectors
still match the target for which the endpoint issued it.

## 3. Resource and timer bounds

The public configuration exposes the limits below and refuses zero values. The defaults are part of
the `S-38` contract; lowering them is supported, raising them is an explicit host decision.

| Resource | Default maximum | Behavior at the bound |
|---|---:|---|
| owned subscription intents (establishing, live, unsubscribing or retry-waiting) | 1,024 per client | `Start` returns `CapacityExceeded`; no request or timer is produced |
| queued delivered notifications | 32 per subscription | a new valid NOTIFY receives `503 Service Unavailable` with `Retry-After: 1`; it is not delivered |
| retained NOTIFY body | 65,536 octets | `413 Content Too Large`; the consumer is not called |
| SUBSCRIBE body | 65,536 octets | typed local error before any output |
| in-flight SUBSCRIBE operations | one per subscription | refresh, auth retry and unsubscribe are serialized, never queued without a bound |
| authentication retries | two per SUBSCRIBE operation | the next challenge terminates that operation with `AuthenticationExhausted` |
| interval retry after 423 | one per SUBSCRIBE operation | a second 423 terminates that operation with `IntervalRejected` |
| accepted fork dialogs | one per `Start` | every competing NOTIFY receives 481 |

At most one live N, Expiry, Refresh and Retry timer exists per subscription, plus one global
Shutdown timer:

| Timer | Duration/source | Fired behavior |
|---|---|---|
| `N` | exactly `64*T1`, armed for every initial, refresh or unsubscribe SUBSCRIBE | initial: terminate `NoInitialNotify`; refresh: end only the refresh attempt as `RefreshUnconfirmed` and retain the usage until its unchanged authoritative Expiry; unsubscribe: terminate locally unconfirmed |
| `Expiry` | requested expiry is armed at `Start` as a provisional upper bound; a valid 2xx grant or active/pending NOTIFY `expires` replaces it according to §5; every replacement is measured from the input that supplied it | terminate locally and cancel refresh work |
| `Refresh` | four fifths of the authoritative expiry, clamped to at least 1 second and at most 1 second before expiry when expiry is at least 2 seconds; a 1-second expiry becomes one immediate input, not a loop | issue one in-dialog refresh if no operation is in flight |
| `Retry` | reason table in §7 | start an unrelated subscription only while application intent remains active |
| `Shutdown` | driver-supplied global deadline | abandon remaining transactions, clear every timer/queue and emit `Stopped` |

Timers carry generations. Replacing or cancelling a timer increments its generation; a fired input
with an old generation is ignored. Time enters nowhere else. A large time jump is processed by the
driver in deadline order, one fired timer input at a time.

The number of refresh timers and outstanding refresh transactions is therefore never greater than
the number of live subscriptions. Shutdown creates no unbounded fan-out: it emits at most one
unsubscribe per live subscription and waits under the one global deadline.

## 4. Request and dialog invariants

The client, not the package consumer, owns Request-URI, Via, Route, To, From, Call-ID, Contact, CSeq,
Max-Forwards, Event, Accept, Expires, Content-Length and authentication headers.

An initial SUBSCRIBE uses a fresh Call-ID, cryptographically fresh From tag, non-zero initial local
CSeq and the resource URI. Every retry after 401, 407 or 423 increments local CSeq. Every refresh and
unsubscribe is an in-dialog target-refresh request and increments local CSeq again. Numbers never
decrease or repeat; gaps caused by authentication are valid. Local CSeq is a `u32` and never wraps:
before an operation would increment `u32::MAX`, it emits no request and follows the typed
`LocalCSeqExhausted` transition in §7.

The first accepted NOTIFY establishes the subscription dialog usage. The subscriber's route set is
the Record-Route sequence from that NOTIFY in wire order, not the sequence in the SUBSCRIBE 2xx. Its
remote target is the NOTIFY Contact. Every dialog-creating or target-refresh NOTIFY MUST contain
exactly one parseable Contact. Later accepted NOTIFY requests and in-dialog SUBSCRIBE responses may
refresh the remote target from Contact because both methods are target-refresh methods; neither
replaces the route set. A missing, malformed or duplicate Contact changes neither candidate nor
established target.

For an established dialog, a NOTIFY MUST match Call-ID, local/remote tags and Event token/`id`.
Transaction-layer retransmissions receive the cached response without entering the client again.
The first delivered NOTIFY records remote CSeq. A later CSeq greater than the recorded value is
accepted and becomes the new value, including gaps. A lower or equal value that escapes transaction
deduplication receives `500 Server Internal Error`, is not delivered and changes no state.

## 5. Establishment and the initial-NOTIFY race

Establishment tracks two facts independently: the final SUBSCRIBE response and a matching initial
NOTIFY. Timer N remains armed until a matching NOTIFY transaction completes, even if a 2xx arrived.

| State/input | Guard | State/output |
|---|---|---|
| `Idle` + `Start` | capacity/package/trust policy valid; requested expiry is positive and representable | allocate; emit neutral state, initial `SendSubscribe`, `ArmTimer(N)` and provisional `ArmTimer(Expiry, requested)`; enter `NotifyWait` |
| `NotifyWait` + 401/407 | valid supported challenge, retry budget remains and CSeq can increment | increment CSeq; emit authenticated `SendSubscribe`; replace the transaction and N generation |
| `NotifyWait` + 423 | exactly one valid Min-Expires is greater than the attempted interval, within host/package maximum, retry unused and CSeq can increment | increment CSeq; emit retry with Min-Expires; replace transaction, N and provisional Expiry generations |
| `NotifyWait` + 2xx | exactly one valid Expires is positive and no longer than the attempted interval | record granted expiry and response dialog candidate; replace provisional Expiry; stay `NotifyWait` |
| `NotifyWait` + non-2xx | no accepted NOTIFY exists | cancel N; terminate with typed response failure |
| `NotifyWait` + matching active/pending NOTIFY | first trusted dialog candidate | validate and consume; emit 200 before delivery; establish from NOTIFY; cancel N; replace Expiry when state carries `expires`, otherwise retain the finite provisional/response bound; enter `Active` or `Pending` |
| `NotifyWait` + matching terminated NOTIFY | first dialog candidate | emit 200 and optional final delivery; cancel N; terminate using §7 |
| any establishing state + N | current generation | cancel transaction/timers, discard queued neutral state and terminate `NoInitialNotify` |

A NOTIFY may arrive before the SUBSCRIBE response. It is processed exactly as the table says; the
later transaction response still completes its transaction. If that later 2xx names another remote
tag, its dialog facts and expiry are ignored. For the selected tag, a NOTIFY `expires` is
authoritative; otherwise one valid response Expires replaces the provisional requested bound and,
when the usage is already active/pending, arms Refresh from that response-derived interval. A
missing, malformed, duplicate, zero or over-attempt Expires is an `InvalidExpiry` operation failure
and cannot remove the finite bound. If a non-2xx, transaction failure or invalid 2xx arrives after a
NOTIFY established the subscription, the client emits a typed `ConflictingSubscribeResponse` fact
and retains the usage only until the NOTIFY expiry when supplied, otherwise the provisional
requested bound. This deterministic rule prevents a response from another fork from rolling back
accepted state or an incomplete response from creating an immortal usage.

### 5.1 Deliberate refusal of forking

The generic client supports exactly one subscription dialog per `Start`, independent of package.
The first potential dialog-establishing NOTIFY wins. A later corresponding NOTIFY with a different
remote tag receives 481, is never delivered and consumes no slot. A late 2xx naming another dialog
only completes its SUBSCRIBE transaction. There is no merge callback and no hidden second refresh
loop. An application that needs fork aggregation must originate separately named subscriptions.

This is a deliberate narrowing of RFC 6665 §4.1.4's package-selectable behavior: none of sipx's
current consumers requires fork merging, and admitting it without a package-defined merge contract
would make ordering and resource bounds ambiguous.

## 6. NOTIFY processing

The client performs the following checks in order. The first failure produces the listed final
response and no later step runs.

| Check | Failure |
|---|---|
| request method is NOTIFY and required dialog headers parse | 400 |
| request matches an establishing attempt or live subscription | 481 |
| Event token and `id` exactly match the selected consumer | 489 |
| dialog is the selected non-forked dialog | 481 |
| injected trust policy accepts the received peer/connection and request | 403 |
| CSeq method is NOTIFY and sequence is newer | 400 for method mismatch; 500 for stale/replayed sequence |
| exactly one parseable Contact exists | 400 |
| exactly one parseable `Subscription-State` exists | 400 |
| body is within the configured bound | 413 |
| queue has capacity | 503 with `Retry-After: 1` |
| content type/body satisfy the injected consumer | consumer's 400, 415 or package-defined final status |

No rejecting step records a candidate dialog, remote CSeq, Contact, state or body. On success the
client records remote CSeq, refreshes the remote target from Contact without changing the route set,
applies `Subscription-State`, invokes the consumer, and emits `RespondNotify(200)` before `Deliver`.
It does not wait for an application/user decision.

`active` and `pending` cancel the current N and replace expiry and Refresh from their `expires`
parameter when present. Without that parameter, the most recent response-derived expiry remains;
before a valid response, the requested provisional expiry remains. Absence is never treated as zero
or infinity. `reason` and `retry-after` on active/pending are ignored.
`terminated` cancels expiry, refresh and N, ignores any `expires` parameter, optionally delivers its
body, and follows §7 after its 200 output.

## 7. Termination, retry and refresh

The 401/407 retry transitions in §5 apply to initial, refresh and unsubscribe SUBSCRIBE operations;
423 retry applies only to positive initial/refresh intervals. Each retry preserves the logical
operation, increments CSeq, uses a fresh Via branch and digest cnonce where required, and replaces
both transaction and N generation. Exhausting a retry budget follows the failure transition for
that logical operation; it never starts a second subscription or silently extends the current
expiry.

Response interval parsing is fail-closed and precedes every state or timer change:

| Response | Valid interval | Invalid interval |
|---|---|---|
| initial/refresh 2xx | exactly one parseable Expires in `1..=attempted` | absent, malformed, duplicate, zero, numerically unrepresentable or greater than attempted is `InvalidExpiry` |
| unsubscribe 2xx | exactly one parseable `Expires: 0` | every other value, absence, malformed, duplicate or unrepresentable is `InvalidExpiry` |
| initial/refresh 423 | exactly one parseable Min-Expires, positive, greater than the attempted interval and no greater than both package and host maxima | absent, malformed, duplicate, unrepresentable, non-increasing or over-policy is `IntervalRejected`; no retry |
| unsubscribe 423 | none; raising Expires would reverse the requested operation | always `IntervalRejected`, terminate locally unconfirmed, no retry |

For an initial operation with no accepted NOTIFY, either typed interval failure terminates the
intent. After a NOTIFY already established it, the failure is surfaced as
`ConflictingSubscribeResponse` and the existing finite Expiry is retained. For refresh, failure
retains the previous authoritative expiry and issues no replacement timer. For unsubscribe,
failure terminates locally as `UnsubscribeUnconfirmed` and never restores Refresh. Authentication
or interval retry exhaustion uses the same operation-specific failure transition.

Before any retry, refresh or unsubscribe increments CSeq, the core checks the current value. At
`u32::MAX` it MUST NOT wrap, repeat, emit `SendSubscribe`, arm N or consume a retry budget. An
establishing intent terminates `LocalCSeqExhausted`; an active/pending usage cancels Refresh and
Expiry and terminates `LocalCSeqExhausted`; an explicit unsubscribe or shutdown drain terminates
locally as `UnsubscribeUnconfirmed(LocalCSeqExhausted)`. This transition releases the subscription
on the same input.

| Input/result | Transition |
|---|---|
| refresh due | increment CSeq; send in-dialog SUBSCRIBE with the current desired Expires; arm N; retain old expiry until success/NOTIFY |
| refresh 2xx | validate Expires by the table above; update response-derived expiry only if this operation has not already accepted an `expires` in its NOTIFY; remain in current active/pending state until the required NOTIFY arrives |
| refresh 404, 405, 410, 416, 480-485, 489, 501 or 604 | terminate; any future attempt is an unrelated initial SUBSCRIBE with fresh Call-ID/tag |
| other refresh failure | keep the old subscription only until its last authoritative expiry; do not move the expiry later |
| refresh N | clear the in-flight refresh, emit `RefreshUnconfirmed`, arm no replacement Refresh, and keep active/pending state only until the unchanged authoritative Expiry |
| expiry timer | terminate `LocalExpiry`; no automatic resurrection |
| `Unsubscribe` | cancel Refresh/Retry; increment CSeq; send in-dialog SUBSCRIBE with Expires 0; arm N; enter `Unsubscribing` |
| terminal NOTIFY while unsubscribing | respond 200, optionally deliver body, then terminate |
| unsubscribe 2xx without terminal NOTIFY | remain `Unsubscribing` until N; the 2xx alone does not end the usage |
| unsubscribe N/failure | terminate locally and surface whether peer confirmation was missing |

The core never resurrects a terminated dialog. Instead it applies the following bounded delay and
surfaces `MayRetryNow` when the application may create a new subscription with a fresh Call-ID and
From tag. Once the application has requested unsubscribe or shutdown has begun, every terminal
NOTIFY terminates without retry eligibility regardless of its reason:

| Termination reason | Retry-eligibility policy |
|---|---|
| `deactivated` | surface eligibility on the next driver turn, never recursively in the same input |
| `probation` | arm Retry for `retry-after`; without it use configured probation backoff (default 60 s) |
| `timeout` | no automatic retry by default; surface `MayRetryNow` |
| `giveup` | wait for `retry-after`, or surface `MayRetryNow` when absent |
| unknown/no reason | honor `retry-after`; otherwise surface `MayRetryNow` |
| `rejected`, `noresource`, `invariant`, `badfilter` | never retry automatically; ignore `retry-after` where RFC 6665 gives it no semantics |

An application may explicitly start a new subscription after any terminal state except while client
shutdown is in progress. Delayed eligibility retains only the typed terminal reason and one bounded
Retry timer; it retains no dialog operation. A terminated subscription is never changed back to
active or pending.

## 8. Shutdown

`Shutdown(deadline)` closes admission and disables every retry. For a subscription with no
SUBSCRIBE transaction in flight, the client cancels Refresh and sends exactly one Expires 0
unsubscribe. If an initial/refresh/auth operation is already in flight, shutdown marks it stopping;
when that transaction completes or Timer N fires, it either sends one unsubscribe for the established
usage or releases the failed attempt. It never sends CANCEL for SUBSCRIBE.

A Refresh timer racing shutdown is decided by serialized input order. Once Shutdown is accepted,
all Refresh/Retry firings, including already queued stale generations, are ignored. Terminal NOTIFYs
remain answerable during drain. At the global Shutdown timer, the client cancels every remaining
timer, drops every queued delivery and transaction handle, emits a termination fact for each owned
subscription, then emits one `Stopped`. No background task or timer survives `Stopped`.

The endpoint runtime uses its driver-registry mutex as the admission linearization point. Start
checks the one-way shutdown token while holding that mutex and holds it through task spawn and
registry insertion. Shutdown holds the same mutex while cancelling admission and draining every
JoinHandle, then awaits the drained handles. A driver never removes its own final JoinHandle; a
later admission may reap it only after `is_finished`. Thus a start racing shutdown either enters the
drained set or returns typed `ShuttingDown`, and a start after the barrier always returns that error.

## 9. Byte-level vectors

All displayed lines end in CRLF and every message ends with the displayed empty line. `<branch>` and
Digest `response` values are deterministic fixture substitutions, not wildcards in assertions.

### S37-V1 — authenticated establishment

Input request:

```text
SUBSCRIBE sip:resource@example.test SIP/2.0
Via: SIP/2.0/UDP 192.0.2.10:5060;branch=z9hG4bK-v1a
Max-Forwards: 70
To: <sip:resource@example.test>
From: <sip:client@example.test>;tag=sub-a
Call-ID: sub-a@example.test
CSeq: 1 SUBSCRIBE
Contact: <sip:client@192.0.2.10:5060>
Event: test-state;id=alpha
Accept: application/test-state
Expires: 3600
Content-Length: 0

```

The 401 contains `WWW-Authenticate: Digest realm="example.test", nonce="n1", qop="auth",
algorithm=SHA-256`. The retry MUST preserve Call-ID, From tag, Event, Accept and requested Expires;
its branch is `z9hG4bK-v1b`, CSeq is `2 SUBSCRIBE`, and it adds:

```text
Authorization: Digest username="client", realm="example.test", nonce="n1", uri="sip:resource@example.test", response="fixture-v1", algorithm=SHA-256, qop=auth, nc=00000001, cnonce="c1"
```

The success response is:

```text
SIP/2.0 200 OK
Via: SIP/2.0/UDP 192.0.2.10:5060;branch=z9hG4bK-v1b
To: <sip:resource@example.test>;tag=notifier-a
From: <sip:client@example.test>;tag=sub-a
Call-ID: sub-a@example.test
CSeq: 2 SUBSCRIBE
Contact: <sip:notifier@192.0.2.20:5060>
Expires: 1800
Content-Length: 0

```

The matching initial NOTIFY is:

```text
NOTIFY sip:client@192.0.2.10:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bK-v1n
Max-Forwards: 70
From: <sip:resource@example.test>;tag=notifier-a
To: <sip:client@example.test>;tag=sub-a
Call-ID: sub-a@example.test
CSeq: 40 NOTIFY
Contact: <sip:notifier@192.0.2.20:5060>
Event: test-state;id=alpha
Subscription-State: active;expires=1800
Content-Type: application/test-state
Content-Length: 9

state=one
```

Expected outputs: 200 to NOTIFY, then delivery of `state=one`; active dialog with remote CSeq 40,
remote target `sip:notifier@192.0.2.20:5060`, N cancelled, expiry at +1800 s and refresh at +1440 s.

### S37-V2 — initial NOTIFY wins the response race and the fork

Before any response to V1's initial SUBSCRIBE, receive the V1 NOTIFY with remote tag `notifier-a`.
It establishes the dialog and is answered 200. A later corresponding NOTIFY with From tag
`notifier-b`, Contact `sip:fork@192.0.2.30:5060`, and CSeq `1 NOTIFY` receives 481. A later SUBSCRIBE
2xx carrying To tag `notifier-b` completes the transaction but changes no dialog field. Exactly one
subscription slot, refresh timer and delivery exist.

### S37-V3 — refresh and authoritative expiry

At V1's refresh timer the emitted request uses the established target and has `CSeq: 3 SUBSCRIBE`,
`Event: test-state;id=alpha`, and `Expires: 3600`. A 200 with `Expires: 1200` followed by:

```text
NOTIFY sip:client@192.0.2.10:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bK-v3n
From: <sip:resource@example.test>;tag=notifier-a
To: <sip:client@example.test>;tag=sub-a
Call-ID: sub-a@example.test
CSeq: 41 NOTIFY
Contact: <sip:notifier@192.0.2.20:5060>
Event: test-state;id=alpha
Subscription-State: active;expires=900
Content-Length: 0

```

is answered 200 and makes 900 seconds authoritative. Refresh is rearmed for +720 s, not from the
request's 3600 or response's 1200.

### S37-V4 — local expiry

Starting from V3, fire the current Expiry generation before any later NOTIFY. Expected outputs are
Cancel Refresh, `StateChanged(Terminated(LocalExpiry))`, and release of all subscription state. A
subsequent otherwise matching NOTIFY receives 481 and is not delivered.

### S37-V5 — unsubscribe completes on terminal NOTIFY

From V1 active state, `Unsubscribe` emits an in-dialog SUBSCRIBE with `CSeq: 3 SUBSCRIBE` and
`Expires: 0`, and arms N. Its 200 does not terminate the usage. This request does:

```text
NOTIFY sip:client@192.0.2.10:5060 SIP/2.0
Via: SIP/2.0/UDP 192.0.2.20:5060;branch=z9hG4bK-v5n
From: <sip:resource@example.test>;tag=notifier-a
To: <sip:client@example.test>;tag=sub-a
Call-ID: sub-a@example.test
CSeq: 41 NOTIFY
Contact: <sip:notifier@192.0.2.20:5060>
Event: test-state;id=alpha
Subscription-State: terminated;reason=timeout
Content-Length: 0

```

Expected outputs: 200, cancel N/Expiry, then `Terminated(timeout)` and release. An empty terminal body
does not invoke a consumer that declared it optional.

### S37-V6 — stale NOTIFY is not a duplicate delivery

After accepting CSeq 41, a new transaction carrying the same dialog/Event and `CSeq: 40 NOTIFY`
receives 500. Remote CSeq remains 41; expiry, queue and consumer invocation count do not change.

### S37-V7 — unsupported package

While V1 is live, a NOTIFY with `Event: other-state;id=alpha` receives `489 Bad Event`. It cannot
create a dialog, refresh expiry, consume queue capacity or invoke the V1 consumer.

### S37-V8 — shutdown wins a due refresh

With V1 active and its Refresh timer due but not yet delivered, input `Shutdown(+30s)` first. Outputs
cancel Refresh, emit exactly one in-dialog SUBSCRIBE with `CSeq: 3 SUBSCRIBE` and `Expires: 0`, and
arm N. Delivering the old Refresh generation produces no output. If no terminal NOTIFY arrives, the
Shutdown timer clears N, expiry and the subscription, then emits `Stopped`; no refresh request is
sent before or after it.

### S37-V9 — an expiry-less initial NOTIFY cannot create an immortal usage

Start a fresh subscription requesting 300 seconds. Before its SUBSCRIBE response, the trusted peer
sends a V1-shaped NOTIFY whose relevant fields are:

```text
CSeq: 1 NOTIFY
Contact: <sip:notifier@192.0.2.20:5060>
Event: test-state;id=alpha
Subscription-State: active
Content-Length: 0

```

It receives 200, cancels N and becomes active, but retains the provisional Expiry at +300 seconds.
A later SUBSCRIBE transaction failure emits `ConflictingSubscribeResponse` and neither cancels nor
moves that timer. Firing the current Expiry generation terminates `LocalExpiry`, releases the
subscription and leaves no Refresh timer. The same rule applies to `pending`.

### S37-V10 — local CSeq exhaustion is terminal, never wrapping

Given an active subscription whose last local request used:

```text
CSeq: 4294967295 SUBSCRIBE
```

fire its current Refresh timer. Expected outputs are Cancel Expiry, Cancel Refresh and
`StateChanged(Terminated(LocalCSeqExhausted))`. There is no `SendSubscribe`, N timer or retry-budget
change, and no request with CSeq 0 or repeated CSeq 4294967295. Applying `Unsubscribe` in the same
starting state instead yields `UnsubscribeUnconfirmed(LocalCSeqExhausted)` and the same no-send
property.

### S37-V11 — response intervals fail closed

For independent fresh attempts that requested `Expires: 300`, each of these response fragments is
invalid:

```text
SIP/2.0 200 OK
Content-Length: 0

SIP/2.0 200 OK
Expires: 4294967296
Content-Length: 0

SIP/2.0 200 OK
Expires: 301
Content-Length: 0

SIP/2.0 423 Interval Too Brief
Min-Expires: 301
Content-Length: 0

```

The first three terminate `InvalidExpiry`; the over-policy 423 terminates `IntervalRejected` when
the configured host maximum is 300. No fragment changes Expiry or emits a retry. Equivalent
malformed or duplicate Expires/Min-Expires cases have the same result. For an unsubscribe, only one
parseable `Expires: 0` in a 2xx is valid; an absent/non-zero value or any 423 terminates
`UnsubscribeUnconfirmed` without restoring Refresh.

### S37-V12 — trust and Contact rejection precede dialog mutation

Send the V1 initial NOTIFY from `192.0.2.99:5060` while the default policy's selected SUBSCRIBE
target is `192.0.2.20:5060`. It receives 403. Then send it from the expected peer first with Contact
absent and then with two Contact fields; each receives 400. No case records a remote tag, CSeq,
target, route set, state or delivery. Finally the unchanged V1 NOTIFY from the expected peer with
its single parseable Contact receives 200 and establishes normally. For a live subscription, the
same malformed/duplicate Contact cases preserve the previously accepted remote target.

### S37-V13 — refresh Timer N cannot extend or prematurely erase the usage

Starting from V1, let Refresh fire and emit its in-dialog SUBSCRIBE, then deliver no matching NOTIFY
and fire that operation's current N generation. Expected output is
`StateChanged(RefreshUnconfirmed)` and release of the in-flight operation. The active usage and its
already-authoritative Expiry remain; their generation and deadline do not change. No new Refresh or
SUBSCRIBE is emitted. Firing that retained Expiry then terminates `LocalExpiry` and releases all
state. A late refresh response may complete its transport transaction but changes no client state.

### S37-V14 — secure target identity and resource survive the core/driver boundary

Start independent TLS and WSS subscriptions whose peer descriptions carry certificate identity
`registrar.example.test`; the WSS peer additionally carries `/sip-events`. Initial, authenticated
retry and in-dialog refresh outputs preserve the exact values. The runtime maps them to a transport
target whose connection key still contains that identity and path. A target-refresh Contact may
replace the socket address while retaining both secure selectors. No secure request is sent through
an address-only target.

### S37-V15 — route hop selects transport, port, authority and generation

Establish independent dialogs whose first Record-Route URI names `sip;transport=tcp`,
`sip;transport=ws`, `sips;transport=tcp`, `sips;transport=tls`, `sips;transport=ws` and
`sips;transport=wss`, first without and then with an explicit port. Refresh targets use the mapping
in §2.3, and every explicit port wins. Secure targets verify the route host. A
`sips;transport=udp` route emits no refresh and terminates `UnsupportedRouteTransport`. A named
clear-WS route carries its host as HTTP authority without claiming TLS authentication. After the
driver records a selected connection generation, a matching next NOTIFY passes `SamePeer`; changing
any route selector clears that generation. A dialog with no Record-Route retains V14's identity and
WebSocket resource across a Contact-only refresh.

### S37-V16 — shutdown and admission have one linearization point

Hold the driver registry lock while a second thread attempts `subscribe`, close admission under
that lock, and release it. The attempt and a second post-shutdown attempt both return
`ShuttingDown`; no task enters the registry. In the opposite ordering, a task inserted before
shutdown is present in the drained JoinHandle set and the shutdown barrier waits for it. No test
uses a wall-clock delay to choose the winner.

## 10. S-38 conformance mapping

`S-38` MUST derive failing-first tests from the vectors, without copying their expected behavior into
a second prose contract:

| Test | Required vectors |
|---|---|
| `authenticated_subscription_establishes_from_notify` | V1 |
| `notify_before_response_selects_one_dialog` | V2 |
| `notify_expiry_overrides_refresh_response` | V3 |
| `local_expiry_releases_everything` | V4 |
| `unsubscribe_waits_for_terminal_notify` | V5 |
| `stale_notify_is_refused_without_delivery` | V6 |
| `unsupported_event_is_489` | V7 |
| `shutdown_cancels_a_due_refresh_and_drains` | V8 |
| `expiryless_notify_retains_a_finite_provisional_bound` | V9 |
| `local_cseq_exhaustion_terminates_without_a_send` | V10 |
| `response_intervals_fail_closed_for_every_operation` | V11 |
| `notify_trust_and_contact_rejections_do_not_mutate` | V12 |
| `refresh_timer_n_preserves_only_the_authoritative_expiry` | V13 |
| `secure_target_identity_and_resource_survive_every_send` | V14 |
| `record_route_selects_transport_port_authority_and_generation` | V15 |
| `secure_datagram_route_is_a_typed_refusal_without_a_send` | V15 |
| `racing_shutdown_closes_admission_before_any_spawn` | V16 |

The implementation is incomplete if any vector is asserted only against a helper rather than the
public event-client surface and a real SIP transaction layer.
