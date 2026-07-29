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
| 2 | INVITE with no `To` tag, **merged** (see below) | `482 Loop Detected` (§8.2.2.2) |
| 3 | INVITE with no `To` tag, not merged | **surfaced** as `Dispatched::Invitation`, route reserved |
| 4 | The key is routed | delivered to that call's inbox — see §5 |
| 5 | ACK | counted, logged; **no response** (§17.1.1.3: there is none to send) |
| 6 | Has a `To` tag, or the method exists only inside a dialog | `481 Call/Transaction Does Not Exist` (§12.2.2) |
| 7 | The method is one `sipx_sip::update::ALLOW` advertises | **surfaced** as `Dispatched::OutOfDialog` |
| 8 | Otherwise | `405 Method Not Allowed` with `Allow` (§8.2.1) |

**Row 2 needs all three of §8.2.2.2's terms, and getting it wrong breaks every challenged call.**
An INVITE is merged when its `Call-ID`, its `From` tag **and its `CSeq`** all match a request this
dispatcher has already accepted on a live route. The `CSeq` is not decoration: §8.1.3.5's retry —
the ordinary answer to a 401, 407, 413, 415, 420 or 484, and to RFC 4028 §7.3's 422 — keeps the
`Call-ID` and the `From` tag and increments the `CSeq`. A check on the first two terms alone
refuses that retry `482`, so a call that is challenged can never be placed at all; sipx's own UAC
retries in exactly that shape, so sipx dialling a sipx dispatcher that answers 422 would be
refused by its own stack. A *retransmission* never reaches this table — the server transaction
absorbs it — so a match here is always a second copy that arrived by a different path, which is
what §8.2.2.2 is about.

A route whose inbox has been **dropped** is not a match either, whatever its `CSeq`. Refusing an
invitation is exactly dropping its inbox, so there is no accepted request left for a later one to
be a copy of; treating the dead route as live would let one refused invitation poison its key for
every later attempt by that peer. Row 3 then reserves the key afresh, replacing any route still
there — anything holding that older inbox stops receiving, which is what it already was.

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
rather than to row 7 — it belongs to that transaction. **Nothing then answers it.** sipx has no UAS
half of CANCEL anywhere in the workspace: no `200` for the CANCEL, no `487` for the INVITE it
cancels, and no way for an application holding an `Invitation` to learn it was cancelled. The
CANCEL reaches the inbox and stops there; the caller's INVITE transaction sits in Proceeding until
its own timer, and an application that answers afterwards leaves both ends in a call the caller
tried to give up on. Story `S-23` is the fix. This routing is only what stops the CANCEL being
lost as well as unanswered, and no part of this spec should be read as support for it.

## 4. Nothing is dropped silently

Every outcome above is a response, a surfaced event, or a counter. `Dispatcher::counts` returns
a `DispatchCounts` — the same shape and the same reasoning as `Handle::shed` (`T-19`):

| Field | What it counts | Row |
|---|---|---|
| `shed` | requests refused `503` because the call they belong to was not reading | §5 |
| `acks` | ACKs that could not be delivered and cannot be refused | 5, §5 |
| `unmatched` | in-dialog requests answered `481` | 6 |
| `unsupported` | out-of-dialog requests answered `405` | 8 |
| `malformed` | requests answered `400` for naming no dialog at all | 1 |
| `merged` | INVITEs answered `482` | 2 |

`DispatchCounts::total()` is every field, and **every refusal the table makes is on one of them**.
The last two exist because the first version of this type had four and the `400` and `482` branches
moved no counter: two refusals invisible to the counters this whole section exists to provide.

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
longer discarded. `serve` answers it, and the three cases are three different answers:

- **ACK**: nothing (§17.1.1.3).
- **It matches this dialog** but names a method this call does not implement: `405` with `Allow`
  (§8.2.1). An in-dialog OPTIONS is not one of those — `Call::handle` answers it `200 OK` with
  `Allow` and `Accept` (§11.2), because the `Allow` on that 405 names OPTIONS and an
  advertisement has to be true.
- **It names a dialog that is not this one** — it carries a `To` tag, or its method is one of the
  dialog-only set: `481` (§12.2.2).
- **It names no dialog at all**: `486 Busy Here` for an INVITE (§21.4.24 — "not willing or able
  to take additional calls", which is exactly the one-call contract), `405` for anything else.

The last case is a correction rather than a refinement. The first version answered `481` to
everything that failed `Dialog::matches`, and `matches` is false for any request without a `To`
tag — so a bare INVITE or CANCEL reaching a one-call `serve` was told that the dialog it named did
not exist, when it had named none. `§12.2.2`'s 481 is scoped to requests that name a dialog, and
the distinction the dispatcher's own table draws at rows 6–8 has to be drawn here too.

## 7. Test vectors

Every row of §3 and every bullet of §5 has a test in `crates/sipx-call/tests/dispatch.rs`, named
here so the claim can be checked rather than taken:

| Covers | Test |
|---|---|
| The story's acceptance | `two_calls_served_concurrently_from_one_endpoint` |
| Row 1 | `a_request_naming_no_dialog_at_all_is_refused_400` |
| Row 2 | `a_merged_invite_does_not_displace_the_call_it_duplicates` |
| Row 2, the `CSeq` term | `a_retry_with_a_higher_cseq_is_a_new_invitation_not_a_merged_request` |
| Row 2, the dead-route term | `a_refused_invitation_does_not_poison_its_key` |
| Row 3 | `a_dropped_inbox_releases_its_route` |
| Row 4, outbound registration | `a_dialled_call_is_reached_through_the_registration_handle` |
| Row 5 | `a_stray_ack_is_counted_because_it_cannot_be_refused` |
| Row 6 | `an_in_dialog_request_for_no_live_call_is_answered_481`, `a_dialog_only_method_outside_a_dialog_is_also_481` |
| Row 7 | `an_advertised_method_the_dispatcher_cannot_place_is_surfaced` |
| Row 8 | `an_unsupported_method_outside_a_dialog_is_refused_405` |
| §5, the ordinary shed | `a_full_call_queue_sheds_for_that_call_only` |
| §5, the ACK bullet | `an_ack_that_cannot_be_delivered_is_counted_rather_than_refused` |
| §6, `serve`'s three answers | `a_method_a_call_does_not_implement_is_refused_405_by_serve`, `an_in_dialog_options_keepalive_is_answered`, `a_second_invite_to_a_one_call_serve_is_refused_486` |
| The §12.2.2 guard, through dispatch | `a_replayed_bye_does_not_end_a_dispatched_call` |

The two `is_merged` terms are mutation-checked against each other: dropping the `CSeq` comparison
fails only the retry test, and dropping the closed-route comparison fails only the poisoning test,
so neither is passing on the other's account.

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
