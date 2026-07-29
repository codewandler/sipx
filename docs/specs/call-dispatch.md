# Call dispatch — one endpoint, many calls

**Status:** implemented (`C-4`, `S-23`) · **Crate:** `sipx-call` (`crate::dispatch`) ·
**Design:** [app-sdk](../designs/app-sdk.md)

Normative references: RFC 3261 §8.2 (UAS behaviour and the 405), §8.2.2.2 (merged requests),
§8.2.6.2 (the `To` tag on a response), §9.1 and §9.2 (CANCEL), §11.2 (OPTIONS), §12.2.2 (in-dialog
requests and the 481), §17.1.1.3 (an ACK for a 2xx is a transaction of its own), §17.2.3
(transaction matching), RFC 3311 §4 (the `Allow` contract).

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
| 3 | INVITE with no `To` tag, not merged | **surfaced** as `Dispatched::Invitation`, route and INVITE transaction reserved |
| 4 | CANCEL | placed here, never routed and never surfaced — see §9 |
| 5 | The key is routed | delivered to that call's inbox — see §5 |
| 6 | ACK | counted, logged; **no response** (§17.1.1.3: there is none to send) |
| 7 | Has a `To` tag, or the method exists only inside a dialog | `481 Call/Transaction Does Not Exist` (§12.2.2) |
| 8 | The method is one `sipx_sip::update::ALLOW` advertises | **surfaced** as `Dispatched::OutOfDialog` |
| 9 | Otherwise | `405 Method Not Allowed` with `Allow` (§8.2.1) |

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

The methods row 7 calls dialog-only are BYE, UPDATE, PRACK, REFER, NOTIFY and INFO: each is
defined only inside a dialog, so one arriving without a `To` tag is an orphan of a dialog that is
gone, not a new transaction to offer the application.

**Row 9's `Allow` is `sipx_sip::update::ALLOW`, the one constant the rest of the stack
advertises.** §8.2.1 requires the 405 to list what this UAS supports, and a second copy of that
list is a peer that is told the wrong thing on a path no test looks at (RFC 3311 §4 makes the
header the peer's *only* permission to send an UPDATE). Because the list is that constant, row 8
exists: OPTIONS is on it and the dispatcher does not place it itself, so it is handed to the
application rather than refused with a status contradicting the list.

**Row 4 is CANCEL, and it is above the route lookup rather than below it.** A CANCEL does not
belong to a *dialog*; it belongs to the INVITE transaction whose branch it carries (§9.1), which
in the ordinary case is an invitation nobody has answered and therefore a call that does not exist
yet. Routing it by key would put it in an inbox from which neither of the two responses §9.2 owes
could be sent, which is exactly where it went before `S-23`. It is also the one advertised method
row 8 does not surface: §9.2 says precisely what to answer, and both halves of that answer are the
dispatcher's to give. §9 has the rule.

## 4. Nothing is dropped silently

Every outcome above is a response, a surfaced event, or a counter. `Dispatcher::counts` returns
a `DispatchCounts` — the same shape and the same reasoning as `Handle::shed` (`T-19`):

| Field | What it counts | Row |
|---|---|---|
| `shed` | requests refused `503` because the call they belong to was not reading | §5 |
| `acks` | ACKs that could not be delivered and cannot be refused | 6, §5 |
| `unmatched` | requests answered `481`: in-dialog ones for no live call, and CANCELs for no live transaction | 4, 7 |
| `unsupported` | out-of-dialog requests answered `405` | 9 |
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

## 9. CANCEL, the UAS half (RFC 3261 §9.2)

Normative reference: RFC 3261 §9.1 (what a CANCEL carries), §9.2 (what a UAS does with one),
§17.2.3 (transaction matching), §12.2.2 (the 481). Implemented by `S-23` in
`Dispatcher::cancel`; the application's half is on `Invitation`.

### 9.1 The matching

A CANCEL names the request it withdraws by **carrying that request's topmost `Via` branch**
(§9.1). So the match is the server transaction match of §17.2.3 — the branch, the sent-by, and
the method of the transaction *being cancelled* — and `sipx_sip::TransactionKey::for_cancelled_invite`
is that key: `from_request` with the method put back to INVITE. Nothing else about the CANCEL
selects a transaction.

**The `Call-ID` is not the match.** A CANCEL that shares an invitation's `Call-ID`, `From` tag and
`CSeq` but names another branch names another transaction, and gets the 481 of §9.2 — the route
table is not consulted at all, in either direction.

One term is added to §9.2's, and it comes from §9.1 rather than from anywhere else: a CANCEL "MUST
have the same Call-ID, To, From and CSeq" as the request it cancels, so one whose dialog
identifiers disagree with the transaction its branch names cannot be a legitimate CANCEL for it
and is refused 481. Every well-formed CANCEL passes it, which is what makes it free. What it costs
is the off-path attacker: the sent-by in a `Via` is whatever the sender writes, so §17.2.3's match
on its own means that observing or guessing a branch is enough to stop somebody else's phone
ringing. §9.2 itself notes that CANCEL is a request a UAS may want to authenticate; this is the
part of that which costs nothing.

### 9.2 The two responses

The answer is **two responses on two transactions**, and a stack that keeps only the CANCEL's key
can only ever send one of them. That is why an `Invitation` holds the INVITE's `TransactionKey`
rather than only its inbox.

| # | On | Status | Condition |
|---|---|---|---|
| 1 | the CANCEL's own transaction | `200 OK` | unconditional, once it matched (§9.2 MUST) |
| 2 | the INVITE transaction it withdraws | `487 Request Terminated` | only while that transaction has sent no final response |

The first is unconditional because it means "I received your CANCEL", not "I stopped". The second
is §9.2's "if the transaction for the original request still exists", and the two things that make
it not exist are an answer and an earlier CANCEL.

Both carry **one `To` tag** — §9.2: "the `To` tag of the response to the CANCEL and the `To` tag in
the response to the original request SHOULD be the same". The tag is minted when the invitation is
surfaced and belongs to the invitation, so `Invitation::answer` uses it for the `200` accepting the
call as well.

### 9.3 A CANCEL after a final response is not a teardown

§9.2 is explicit that a CANCEL has no effect on a transaction that has already sent a final
response, and **BYE is the request for ending a call that was answered**. This is the rule an
implementation most often gets wrong in the permissive direction, so it is stated as a state
machine rather than as a condition, with three states and no more:

| Phase | A matching CANCEL does | `Invitation::answer` |
|---|---|---|
| `Ringing` — no final response yet | `200` + `487`, emits `Ended(RemoteCancel)` | answers, and moves to `Answered` |
| `Answered` — a final response has gone out | `200`, and nothing else | answers (a second time, which is the caller's business) |
| `Cancelled` — a CANCEL ended it | `200`, and nothing else | `Error::InvitationCancelled` |

`Answered` is entered **immediately before the `200` is handed to the transport**, and that exact
placement is the contract. A CANCEL arriving mid-answer must not put a `487` on the wire behind a
`200`, because that is the one ordering that leaves the two ends disagreeing about whether there is
a call — so the transition has to precede the send. But it must not precede it by any more than
necessary, because every fallible step that builds the response (parsing the offer, binding the
media port, negotiating the session, building the response, forming the dialog) can return `Err`
with **nothing on the wire**. An invitation taken by one of those failures is one no CANCEL can
ever end: the CANCEL draws its `200`, the `487` is suppressed because the invitation looks
answered, and the INVITE transaction is left with no final response at all for the caller's Timer B
to resolve. That is a request that reached sipx and produced no response, which is the failure this
project does not ship.

So the claim is a hook (`call::Claim`) handed down from `Invitation::answer` and invoked at one
line in `answer_negotiated`, directly above `endpoint.respond`. From that line on, the only
fallible expression is `respond` itself; everything after it — the retransmit task, the event sink,
the `Call` construction — is infallible.

`respond` failing is therefore the single case that stays claimed, and it stays claimed on purpose:
a stream transport can write part of a response before erroring, so a failure is not proof that
nothing reached the caller, and a `487` chasing a `200` is the worse of the two outcomes. The cost
is the one already noted — a CANCEL that says `200` and ends nothing, which Timer B resolves.

`Claim` is `Send + Sync` because it is a *reference* to a trait object, and `&T` is `Send` only
while `T: Sync`. Dropping either bound would make `Invitation::answer`'s future unspawnable, which
is a break in a public API that no behavioural test would catch — `#[tokio::test]` drives futures
on one thread. `an_answer_future_is_spawnable` is the compile-time assertion that holds it;
removing `Sync` from the alias fails it with "cannot be shared between threads safely".

The free `answer()` and `answer_ringing()` pass no claim and cannot make that transition, because
they are not given the invitation. They still answer correctly; what they cannot do is tell the
dispatcher that they did. **Prefer `Invitation::answer` for anything a dispatcher surfaced.**

### 9.4 What the application is told

An invitation that is cancelled must stop the host ringing, and a host that has to *poll* for that
is a host that keeps ringing. So `Invitation` carries `C-3`'s own stream — one `CallEvents`, handed
out once, exactly as `Call::events` is — and emits a single event on it:
`CallEvent::Ended(EndCause::RemoteCancel)`.

That is the existing vocabulary rather than a parallel channel, deliberately. A host that is
ringing and a host that is talking both need to be told the thing ended and why; giving the
pre-answer half its own type would mean two vocabularies for one question. `RemoteCancel` is
distinct from `RemoteBye` because the two are different instructions to a host: one says stop
ringing, the other says the call you have is over. On the wire vocabulary of `app-contract.md`
§5.3 both are the `remote` cause.

`Invitation::is_cancelled` is the same fact as a poll, for code that is not waiting on anything.

### 9.5 Lifetime

An INVITE transaction is remembered as long as the route it reserved, and swept with it — a CANCEL
for an invitation that was answered still owes the `200` of row 1, and by then the `Invitation`
handle is long gone. Retransmitted CANCELs never reach the dispatcher at all: the server
transaction below absorbs them and replays the response it already sent.

### 9.6 Test vectors

`crates/sipx-call/tests/cancel.rs`.

| Covers | Test |
|---|---|
| The story's acceptance — both responses, one `To` tag, the application told | `a_caller_that_gives_up_before_the_answer_ends_the_invitation` |
| §9.2's 481 | `a_cancel_for_no_invitation_of_ours_is_answered_481` |
| §9.1, the branch is the match and the `Call-ID` is not | `a_cancel_on_another_branch_does_not_match_by_call_id_alone` |
| §9.1's added identifier term, with the transaction match satisfied | `a_cancel_on_the_right_transaction_from_the_wrong_dialog_is_refused` |
| §9.3, the negative | `a_cancel_after_the_answer_does_not_tear_the_dialog_down` |
| §9.5, and cancelled-once | `a_replayed_cancel_draws_the_same_answer_and_nothing_more` |
| §9.3's claim placement — a failed answer sent nothing, so the invitation is still cancellable | `an_invitation_whose_answer_failed_before_responding_is_still_cancellable` |
| `Invitation::answer`'s future is still `Send`, so it can be spawned | `an_answer_future_is_spawnable` |
| A third party cannot end someone else's invitation | `a_cancel_from_a_third_party_does_not_reach_someone_elses_invitation` |
| §9.4 | `a_ringing_host_is_told_the_caller_gave_up_and_why` |

Two of these are mutation-checked, because both are negatives and a negative that asserts the
wrong thing passes against everything. Dropping §9.1's identifier term fails only
`a_cancel_on_the_right_transaction_from_the_wrong_dialog_is_refused`; letting a CANCEL end an
invitation that has already answered fails only `a_cancel_after_the_answer_does_not_tear_the_dialog_down`.
That second one is why both of those tests watch the invitation's **event stream**: the `serve`
loop survives a stray `487` and the caller's client transaction has already finished, so the only
instrument sharp enough to see the difference is the event that a cancellation would have emitted.

The claim's *placement* is mutation-checked from both sides, because it is a position rather than a
condition and either direction of drift is a real bug. Moving it earlier — taking the invitation
before the response is built, as this first did — fails
`an_invitation_whose_answer_failed_before_responding_is_still_cancellable` with the INVITE never
answered at all (`the INVITE is answered rather than left to time out: Elapsed(())`). Removing it
altogether fails three: `a_cancel_after_the_answer_does_not_tear_the_dialog_down`,
`a_caller_that_gives_up_before_the_answer_ends_the_invitation` and
`a_ringing_host_is_told_the_caller_gave_up_and_why`.
