# Endpoint event notifier

**Status:** normative · **Story:** S-35 · **RFCs:** 3261, 3680, 3856, 4235, 6665

## 1. Scope and ownership

This specification connects the existing pure notifier state machine in
`sipx_ua::subscribe::Subscriptions` to an endpoint. It defines the server half only: receiving
SUBSCRIBE, answering it, sending NOTIFY and expiring the resulting subscription. Issuing
SUBSCRIBE and consuming NOTIFY belong to `docs/specs/event-client.md` and are out of scope.

The call dispatcher owns method routing and delegates every SUBSCRIBE to one configured notifier.
The notifier and its public observation handle hold the same `Arc<Mutex<Subscriptions>>`; a socket
path MUST NOT copy, mirror or reconstruct the store. With no notifier configured, the dispatcher's
existing unsupported-method answer remains in force.

One notifier has these finite resources:

- one subscription store with a configured non-zero `capacity`;
- at most one package-state object and one expiry task per accepted subscription; and
- a fixed three-entry package renderer table: `dialog`, `reg`, and `presence`.

Admission uses those three exact, case-insensitive package tokens (before Event parameters). The
template relationship understood by the general `Packages` registry is not runtime support:
`dialog.winfo`, `reg.winfo`, `presence.winfo`, and every other derived token receive 489. Adding a
template requires its own renderer and state contract before it may be admitted here.

An application may inspect the shared store and the runtime counters through the notifier handle.
It does not route requests or mutate notifier dialog bookkeeping.

## 2. Identity and dialog matching

RFC 6665 §4.4.1 identifies a subscription by dialog and Event. `Id` contains the Call-ID,
subscriber (`From`) tag and a normalized Event identity: the event-type token followed only by its
optional `id` parameter. RFC 6665 §8.2.1 compares the event-type and `id` values byte-for-byte,
treats the `id` parameter name case-insensitively, and ignores every other parameter. Consequently
parameter order and irrelevant parameter changes do not create a new subscription, while changing
the case of the event-type or opaque `id` value does. A duplicate `id` parameter is malformed
rather than a first-value-wins identity.

The socket driver adds the notifier's local (`To`) tag, remote target, route set and local CSeq;
these are wire state and do not form a second protocol store. Dialog tags are opaque RFC 3261
tokens and compare byte-for-byte: changing tag case does not select the recorded dialog.

An initial SUBSCRIBE has no `To` tag. Acceptance mints one local tag and records it with the
subscription. A refresh or unsubscribe has a `To` tag and MUST match the recorded local tag for the
same `Id`; otherwise it is answered `481 Call/Transaction Does Not Exist` and does not enter the
pure state machine. Thus a stale dialog cannot accidentally create a new subscription.

An untagged request whose `Id` is already owned by a live subscription is also answered 481. It is
not treated as a refresh and MUST NOT replace the existing task, tag, target, package document or
store row.

Each store row records the CSeq of the last accepted remote SUBSCRIBE. A tagged refresh or
unsubscribe MUST have a strictly greater sequence number. An equal or lower sequence is answered
`500 Server Internal Error` per RFC 3261 §12.2.2 and MUST NOT change expiry, state, task deadline,
target or counters. The sequence advances only after all request validation has succeeded.

The initial remote target is the request's Contact URI. Missing, malformed, duplicate or
conflicting Call-ID, From, To, Event, Contact, CSeq or Expires is `400 Bad Request` before any
first-value parsing or store lookup. The route set is the request's Record-Route values in received
order. NOTIFY uses that target and route set rather than resolving the resource URI again.

## 3. Request decision table

All time values below use one monotonic origin owned by the runtime driver. `now` in the pure store
is elapsed whole seconds from that origin.

| Input | Store action | Response | Notification/task action |
|---|---|---|---|
| Initial, served Event, capacity available | `Established` | `200`, tagged `To`, granted `Expires`, `Allow-Events` | send initial active NOTIFY immediately; arm one expiry task |
| Matching refresh, positive Expires | `Refreshed` | `200`, same `To` tag, granted `Expires` | re-arm the owned expiry task; no mandatory state-change NOTIFY |
| Matching unsubscribe, `Expires: 0` | `Unsubscribed` | `200`, same `To` tag, `Expires: 0` | cancel expiry task; send one terminating NOTIFY; remove wire state after the send attempt |
| Initial while at capacity | no mutation | `503`, `Retry-After: 5` | increment `shed`; no task or package state |
| Unserved or template-derived Event | no mutation | `489 Bad Event`, `Allow-Events` | none |
| In-dialog request with unknown identity/tag | no mutation | `481` | none |
| Untagged request colliding with a live identity | no mutation | `481` | none |
| Tagged refresh/unsubscribe with equal or lower CSeq | no mutation | `500` | retain the existing deadline/task |
| Missing, malformed or duplicate CSeq; CSeq method other than SUBSCRIBE | `Malformed` | `400` | none |
| Malformed or duplicate Expires | `Malformed` | `400` | none |
| Missing, malformed, duplicate or conflicting Call-ID, From, To, Event or Contact | `Malformed` | `400` | none |
| Expiry task fires | `terminate(id, timeout)` | none | send one terminating NOTIFY; remove wire state after the send attempt |

RFC 6665 §4.2.1.1 permits a notifier to shorten an expiry and forbids lengthening it. Every 200
therefore carries the value returned by `granted_expiry(requested, policy_maximum)`. A missing
Expires requests the policy maximum. The initial NOTIFY is queued only after the 200 has been handed
to the server transaction, but without waiting for another inbound request or application poll.

## 4. NOTIFY construction

For an accepted subscription, NOTIFY is an in-dialog request:

- request URI: remote Contact URI;
- `Call-ID`: copied from SUBSCRIBE;
- `From`: SUBSCRIBE `To` address plus the notifier's local tag;
- `To`: SUBSCRIBE `From`, retaining the subscriber tag;
- `CSeq`: starts at `1 NOTIFY` and increases for that subscription;
- `Event`: byte-equivalent semantic value from SUBSCRIBE;
- `Subscription-State`: `active;expires=N`, or `terminated;reason=timeout` for expiry;
- `Contact`: the endpoint's advertised address for the received transport;
- `Content-Type`: the package MIME type; and
- body: the package's current full document.

The first `dialog` and `reg` bodies are their RFC-defined `state="full"`, version-zero documents.
The first `presence` body is a PIDF document. Package state is private to one accepted subscription,
so version counters cannot leak between watchers, and the number of package-state objects is bounded
by the subscription capacity.

Vector (irrelevant generated Via branch and Contact authority omitted):

```text
SUBSCRIBE sip:alice@example.test SIP/2.0
To: <sip:alice@example.test>
From: <sip:watcher@example.net>;tag=remote
Call-ID: watch-1
CSeq: 1 SUBSCRIBE
Contact: <sip:watcher@192.0.2.10:5060>
Event: dialog
Expires: 900

SIP/2.0 200 OK
To: <sip:alice@example.test>;tag=local
From: <sip:watcher@example.net>;tag=remote
Call-ID: watch-1
CSeq: 1 SUBSCRIBE
Expires: 300
Allow-Events: dialog, reg, presence

NOTIFY sip:watcher@192.0.2.10:5060 SIP/2.0
To: <sip:watcher@example.net>;tag=remote
From: <sip:alice@example.test>;tag=local
Call-ID: watch-1
CSeq: 1 NOTIFY
Event: dialog
Subscription-State: active;expires=300
Content-Type: application/dialog-info+xml
```

## 5. Bounds and lifecycle observability

`capacity` bounds live subscriptions, package state and expiry tasks together. An in-dialog refresh
or unsubscribe is still admitted while the capacity is full because it cannot increase any of the
three. A refused initial request increments a monotonic `shed` counter and carries a retryable 503.

The handle exposes current `active_tasks`, cumulative `started_tasks`, `finished_tasks`, and `shed`.
Every expiry-task exit path, including cancellation and notifier drop, decrements `active_tasks` and
increments `finished_tasks`. A refresh re-arms that subscription's existing task rather than
creating a second one. Termination is proven by those counters reaching zero; removing a store row
alone is not evidence that scheduled work stopped.

Dropping the runtime owner aborts all owned tasks. No task holds the runtime owner alive. A NOTIFY
transaction awaits its final response for at most two seconds. This is a failure bound, not a
protocol timer: timeout, transport failure and every final status are observed completion of the
send attempt and do not decide whether the subscription exists. The expiry deadline is fixed before
the initial send, so a silent peer cannot lengthen the granted lifetime. After the application wait
ends, the endpoint still owns the RFC 3261 transaction until its protocol timer completes; that
bounded residue is included in `Handle::outstanding()` rather than hidden by the notifier.

## 6. Required tests

1. Send an initial SUBSCRIBE through two loopback endpoints; observe it through the exact shared
   `Subscriptions` allocation and receive the immediate initial NOTIFY.
2. Assert 489 for an unserved package, 481 for an unknown tagged dialog, and that both the 200 and
   initial NOTIFY carry the shortened expiry.
3. Fill the capacity, assert one further initial request receives 503/Retry-After, `shed` increases,
   and task/package-state counts never exceed the cap.
4. Unsubscribe and separately expire under paused time; observe active tasks fall to zero and
   finished tasks rise after the terminating NOTIFY attempt.
5. Assert no public method issues SUBSCRIBE or consumes NOTIFY in this story.
6. For each exact built-in package, receive and answer the initial NOTIFY, and assert its MIME type
   and full-document marker. Refuse a template-derived package with 489.
7. Send malformed Expires and CSeq values over a socket and observe 400 without store mutation;
   reject an untagged live-identity collision with 481 without replacing its task.
8. Establish a tagged subscription, then send equal and lower CSeq refreshes and an unsubscribe;
   each receives 500 and leaves the accepted expiry/state/task unchanged. A greater CSeq still
   refreshes or terminates it.
9. Prove Event matching ignores irrelevant parameter order and spelling, treats the `id` parameter
   name case-insensitively, and compares the event-type and opaque `id` values byte-for-byte.
10. Send duplicate Call-ID, From, To, Event and Contact fields and observe 400 before any store or
    task mutation; prove that a case-changed local dialog tag receives 481.
