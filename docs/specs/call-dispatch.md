# Call dispatch — one endpoint, many calls

**Status:** implemented (`C-4`) · **Crate:** `sipx-call` (`crate::dispatch`) ·
**Design:** [app-sdk](../designs/app-sdk.md)

Normative references: RFC 3261 §8.2 (UAS behaviour and the 405), §8.2.2.2 (merged requests),
§8.2.6.2 (the `To` tag on a response), §11.2 (OPTIONS), §12.2.2 (in-dialog requests and the 481),
§17.1.1.3 (an ACK for a 2xx is a transaction of its own), RFC 3311 §4 (the `Allow` contract).

## 1. What this is for

An endpoint hands out exactly one `Receiver<Incoming>`. Every request that arrives on it — for
any call, in any dialog — comes out of that one stream. Before this, an application holding more
than one call had to write its own demultiplexer, and the framework's one-call convenience
(`serve`) *dropped* whatever the call it was driving did not claim. Both are ways to lose an ACK,
which is the loss that leaks calls: nothing retransmits it after Timer H and no timer reaps the
dialog it would have completed.

The dispatcher is that demultiplexer, written once: it owns the endpoint's receiver, routes each
request to the call it belongs to, and gives every request that belongs to no call a defined
answer instead of silence.

## 2. The route key

A route is keyed on **(`Call-ID`, the peer's tag)**, where the peer's tag is the `From` tag of an
arriving request and `Dialog::id.remote_tag` of a registered call. The local tag is deliberately
not part of the key.

That is what lets a route be **reserved from the INVITE alone**, before the application has
decided how to answer and therefore before a local tag exists. The window it closes is real: the
ACK to our own 2xx can arrive before the application has finished answering, and a route that
could only be installed after `answer()` returned would have nowhere to put it.

The key is still unique per call in both directions:

- **Inbound.** One INVITE creates at most one dialog here; a second dialog on the same `Call-ID`
  and `From` tag would be a merged request, which §4 refuses.
- **Outbound.** A forked INVITE creates one dialog per *remote* tag, and `dial` returns one call
  per dialog, so the remote tag separates them.

A request that is routed anyway still meets `Dialog::matches` inside `Call::handle`, which
compares all three components. The key decides *where* a request goes; the dialog decides whether
it belongs.

## 3. The decision table

`Dispatcher::next` consumes requests until one has to be surfaced to the application. For each
request, in this order:

| # | Condition | Outcome |
|---|---|---|
| 1 | No `Call-ID`, or no `From` tag | `400 Bad Request` (RFC 3261 §8.1.1 makes both mandatory) |
| 2 | INVITE with no `To` tag, key not routed | **surfaced** as `Dispatched::Invitation`, route reserved |
| 3 | INVITE with no `To` tag, key already routed | `482 Loop Detected` (§8.2.2.2) |
| 4 | The key is routed | delivered to that call's inbox — see §5 |
| 5 | ACK | counted, logged; **no response** (§17.1.1.3: there is none to send) |
| 6 | Has a `To` tag, or the method exists only inside a dialog | `481 Call/Transaction Does Not Exist` (§12.2.2) |
| 7 | The method is one `sipx_sip::update::ALLOW` advertises | **surfaced** as `Dispatched::OutOfDialog` |
| 8 | Otherwise | `405 Method Not Allowed` with `Allow` (§8.2.1) |

The methods row 6 calls dialog-only are BYE, UPDATE, PRACK, REFER, NOTIFY and INFO: each is
defined only inside a dialog, so one arriving without a `To` tag is an orphan of a dialog that is
gone, not a new transaction to offer the application.

**Row 8's `Allow` is `sipx_sip::update::ALLOW`, the one constant the rest of the stack
advertises.** §8.2.1 requires the 405 to list what this UAS supports, and a second copy of that
list is a peer that is told the wrong thing on a path no test looks at (RFC 3311 §4 makes the
header the peer's *only* permission to send an UPDATE). Because the list is that constant, row 7
exists: OPTIONS and CANCEL are on it and the dispatcher does not place them itself, so they are
handed to the application rather than refused with a status contradicting the list.

Row 4 precedes rows 5–8, so a CANCEL for an invitation that *is* routed goes to that call's inbox
rather than to row 7 — it belongs to that transaction, and an application that means to honour one
reads the inbox while it decides how to answer. Answering the INVITE 487 and the CANCEL 200 is not
something sipx does as a UAS at all, here or anywhere else; that gap predates this spec.

## 4. Nothing is dropped silently

Every outcome above is a response, a surfaced event, or a counter. `Dispatcher::counts` returns
a `DispatchCounts` — the same shape and the same reasoning as `Handle::shed` (`T-19`):

| Field | What it counts |
|---|---|
| `shed` | requests refused `503` because the call they belong to was not reading |
| `acks` | ACKs that could not be delivered and cannot be refused |
| `unmatched` | in-dialog requests answered `481` |
| `unsupported` | out-of-dialog requests answered `405` |

`acks` is counted apart for `T-19`'s reason, which is unchanged by moving one layer up: an ACK
cannot be refused, nothing retransmits it once Timer H expires, and the dialog it would have
completed is not reaped unless RFC 4028 session timers happen to be running. It is logged at
`error` where the others are `warn`.

## 5. One stalled call does not stall its siblings

Per-call delivery is a **bounded** `tokio::sync::mpsc` channel (`DEFAULT_QUEUE`, overridable with
`Dispatcher::with_queue`). The dispatcher never awaits room on it, so a call whose task has
stopped reading cannot stop the loop that serves every other call.

The defined consequence of a full inbox is **for that call only**:

- an ordinary request is answered `503 Service Unavailable` with a `Retry-After`, and `shed` moves;
- an ACK is counted in `acks` and logged at `error`, because there is nothing to answer;
- every other call's inbox is untouched, and the dispatcher goes straight on to the next request.

Refusing rather than blocking is the vision's principle 3 seen from the dispatch side: a peer
that is told to back off behaves better than one that is ignored, and one slow application task
must not become a stack that drops the established calls of every other task.

When the inbox is *closed* — the application dropped the receiver, which is what ending a call
does — the route is removed and the request gets the answer an unknown dialog gets (§3 row 6).
Routes are also swept on registration, so a long-lived dispatcher does not accumulate the dead.

## 6. `serve` is the one-call convenience

`serve(&mut call, &mut requests)` is unchanged in shape and now sits at both ends of this: over
the endpoint's own receiver it is the one-call program it always was, and over an inbox the
dispatcher handed out it is one call of many. That is why `Dispatcher` hands out a plain
`Receiver<Incoming>` rather than a wrapper — the one-call and many-call cases are the same loop.

What did change is the drop the story's notes name: a request `Call::handle` does not claim is no
longer discarded. `serve` answers it —

- ACK: nothing (§17.1.1.3);
- not this dialog: `481` (§12.2.2);
- OPTIONS: `200 OK` with `Allow` and `Accept` (§11.2), which `Call::handle` now answers because
  the `Allow` the 405 below carries names OPTIONS and an advertisement has to be true;
- anything else: `405` with `Allow` (§8.2.1).

## 7. Test vectors

Each row of §3 and §5 has a test in `crates/sipx-call/tests/dispatch.rs`; the named ones are
`two_calls_served_concurrently_from_one_endpoint`,
`an_in_dialog_request_for_no_live_call_is_answered_481`,
`an_unsupported_method_outside_a_dialog_is_refused_405` and
`a_full_call_queue_sheds_for_that_call_only`.

## 8. RFC 3311 §5.2 through a concurrent dispatcher

`S-19` recorded that two of §5.2's three refusals had no end-to-end path, because sipx dispatches
in-dialog requests through `&mut self` and a call is therefore never mid-exchange when the next
request arrives. **The dispatcher does not change that**, and the finding is worth writing down
rather than leaving as an expectation: routing to N calls concurrently still serialises the
requests *of one call*, because handling one needs `&mut Call`.

What does leave a `Negotiation` non-idle across a `handle` boundary is an **abandoned exchange** —
a `Call::update` or a `Call::handle` future dropped part-way, which is what
`timeout(d, serve(..))` does to whatever was in flight. The dispatcher makes that reachable in
the way that matters: the peer's next request is read off the wire and queued in the call's inbox
while this side is still mid-exchange, so it is waiting to be handled the moment the call is
non-idle, instead of sitting unread behind an endpoint receiver nobody is polling.

Both rules are covered end to end on that path
(`an_update_arriving_while_our_own_offer_is_outstanding_is_refused_491`,
`an_update_arriving_while_another_is_in_progress_is_refused_500`), and rule 3 keeps the wire test
`S-19` gave it.
