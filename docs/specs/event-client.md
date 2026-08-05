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
| `Start` | resource URI, event token and optional `id`, requested expiry, Accept values, optional bounded SUBSCRIBE body, credentials, driver-supplied fresh Call-ID/From tag/initial CSeq and injected package consumer |
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
| `N` | exactly `64*T1`, armed for every initial, refresh or unsubscribe SUBSCRIBE | if no matching NOTIFY completed, terminate the attempt/subscription |
| `Expiry` | newest valid grant for the current SUBSCRIBE operation: an accepted active/pending NOTIFY dominates that operation's response even if it arrived first; until that NOTIFY, a successful SUBSCRIBE response updates the previous expiry | terminate locally and cancel refresh work |
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
decrease or repeat; gaps caused by authentication are valid.

The first accepted NOTIFY establishes the subscription dialog usage. The subscriber's route set is
the Record-Route sequence from that NOTIFY in wire order, not the sequence in the SUBSCRIBE 2xx. Its
remote target is the NOTIFY Contact. Later accepted NOTIFY requests and in-dialog SUBSCRIBE
responses may refresh the remote target from Contact because both methods are target-refresh
methods; neither replaces the route set.

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
| `Idle` + `Start` | capacity and package policy valid | allocate; emit neutral state, initial `SendSubscribe`, `ArmTimer(N)`; enter `NotifyWait` |
| `NotifyWait` + 401/407 | valid supported challenge and retry budget remains | increment CSeq; emit authenticated `SendSubscribe`; replace the transaction and N generation |
| `NotifyWait` + 423 | valid Min-Expires within host/package maximum and interval retry unused | increment CSeq; emit a retry with Min-Expires; replace the transaction and N generation |
| `NotifyWait` + 2xx | Expires present and no longer than requested | record granted expiry and response dialog candidate; stay `NotifyWait` |
| `NotifyWait` + non-2xx | no accepted NOTIFY exists | cancel N; terminate with typed response failure |
| `NotifyWait` + matching active/pending NOTIFY | first dialog candidate | validate and consume; emit 200 before delivery; establish from NOTIFY; cancel N; enter `Active` or `Pending` |
| `NotifyWait` + matching terminated NOTIFY | first dialog candidate | emit 200 and optional final delivery; cancel N; terminate using §7 |
| any establishing state + N | current generation | cancel transaction/timers, discard queued neutral state and terminate `NoInitialNotify` |

A NOTIFY may arrive before the SUBSCRIBE response. It is processed exactly as the table says; the
later transaction response still completes its transaction. If that later 2xx names another remote
tag, its dialog facts and expiry are ignored. If a non-2xx arrives after a NOTIFY established the
subscription, the client emits a typed `ConflictingSubscribeResponse` fact and retains the
NOTIFY-established usage until terminal NOTIFY, expiry or explicit unsubscribe. This deterministic
rule prevents a response from another fork from rolling back accepted state.

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
| CSeq method is NOTIFY and sequence is newer | 400 for method mismatch; 500 for stale/replayed sequence |
| exactly one parseable `Subscription-State` exists | 400 |
| body is within the configured bound | 413 |
| queue has capacity | 503 with `Retry-After: 1` |
| content type/body satisfy the injected consumer | consumer's 400, 415 or package-defined final status |

On success the client records remote CSeq, refreshes the remote target from Contact without changing
the route set, applies `Subscription-State`, invokes the consumer, and emits `RespondNotify(200)`
before `Deliver`. It does not wait for an application/user decision.

`active` and `pending` cancel the current N and replace expiry and Refresh from their `expires`
parameter when present. Without that parameter, the most recent authoritative expiry remains;
absence is never treated as zero. `reason` and `retry-after` on active/pending are ignored.
`terminated` cancels expiry, refresh and N, ignores any `expires` parameter, optionally delivers its
body, and follows §7 after its 200 output.

## 7. Termination, retry and refresh

The 401/407 and 423 retry transitions in §5 apply to every initial, refresh and unsubscribe
SUBSCRIBE operation. Each retry preserves the logical operation, increments CSeq, uses a fresh Via
branch and digest cnonce where required, and replaces both transaction and N generation. Exhausting
a retry budget follows the failure transition for that logical operation; it never starts a second
subscription or silently extends the current expiry.

| Input/result | Transition |
|---|---|
| refresh due | increment CSeq; send in-dialog SUBSCRIBE with the current desired Expires; arm N; retain old expiry until success/NOTIFY |
| refresh 2xx | validate Expires; update response-derived expiry only if this operation has not already accepted its NOTIFY; remain in current active/pending state until the required NOTIFY arrives |
| refresh 404, 405, 410, 416, 480-485, 489, 501 or 604 | terminate; any future attempt is an unrelated initial SUBSCRIBE with fresh Call-ID/tag |
| other refresh failure | keep the old subscription only until its last authoritative expiry; do not move the expiry later |
| expiry timer | terminate `LocalExpiry`; no automatic resurrection |
| `Unsubscribe` | cancel Refresh/Retry; increment CSeq; send in-dialog SUBSCRIBE with Expires 0; arm N; enter `Unsubscribing` |
| terminal NOTIFY while unsubscribing | respond 200, optionally deliver body, then terminate |
| unsubscribe 2xx without terminal NOTIFY | remain `Unsubscribing` until N; the 2xx alone does not end the usage |
| unsubscribe N/failure | terminate locally and surface whether peer confirmation was missing |

Automatic re-subscription occurs only while the original application intent remains open and always
uses a fresh Call-ID and From tag:

| Termination reason | Automatic policy |
|---|---|
| `deactivated` | retry on the next driver turn, never recursively in the same input |
| `probation` | arm Retry for `retry-after`; without it use configured probation backoff (default 60 s) |
| `timeout` | no automatic retry by default; surface `MayRetryNow` |
| `giveup` | wait for `retry-after`, or surface `MayRetryNow` when absent |
| unknown/no reason | honor `retry-after`; otherwise surface `MayRetryNow` |
| `rejected`, `noresource`, `invariant`, `badfilter` | never retry automatically; ignore `retry-after` where RFC 6665 gives it no semantics |

An application may explicitly start a new subscription after any terminal state except while client
shutdown is in progress. A terminated subscription is never changed back to active or pending.

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

The implementation is incomplete if any vector is asserted only against a helper rather than the
public event-client surface and a real SIP transaction layer.
