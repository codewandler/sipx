# Spec: SIP transactions

**Status:** normative · **Crate:** `sipx-sip` · **Stories:** S-6, S-7, S-8 · **Design:**
[sip-core](../designs/sip-core.md)

## 1. Normative references

- RFC 3261 §17 (transactions), §17.1.1 (INVITE client), §17.1.2 (non-INVITE client),
  §17.2.1 (INVITE server), §17.2.2 (non-INVITE server), §17.1.3 and §17.2.3 (matching),
  §8.1.1.7 (branch), §9 (CANCEL).
- **RFC 6026** — corrects RFC 3261's handling of 2xx responses to INVITE. Adopted; see §5.
- RFC 2543 §D.2 — the pre-`branch` matching rules, still needed on the public internet.

**Out of scope:** dialogs, forking as seen by a UAC (the transaction layer sees each response
individually), and anything that touches a socket.

## 2. Sans-IO contract

A transaction is a state machine. It reads no clock and owns no socket.

```rust
enum Input  { Message, TimerFired(Timer), TuResponse(Response), TuRequest, TransportError }
enum Output { Send(Message), SetTimer{timer, after}, ClearTimer(Timer), ToTu(TuEvent), Terminated(Reason) }
```

Every method returns the outputs its input produced, in the order the driver must perform
them. `Send` before `SetTimer` always, so a retransmission timer never starts before the thing
it is retransmitting has gone out.

## 3. Timers

| Symbol | Value | Meaning |
|---|---|---|
| T1 | 500 ms | Round-trip estimate; the base of every backoff |
| T2 | 4 s | Ceiling for retransmission intervals |
| T4 | 5 s | Longest a message lingers in the network |

All three are configurable; nothing in the implementation may hard-code them.

| Timer | Machine | Initial | Behaviour |
|---|---|---|---|
| A | INVITE client | T1 | Retransmit; interval doubles each fire. Unreliable only |
| B | INVITE client | 64·T1 | Give up |
| D | INVITE client | ≥32 s unreliable, 0 reliable | Absorb response retransmissions |
| E | non-INVITE client | T1 | Retransmit; `min(2E, T2)`. Unreliable only |
| F | non-INVITE client | 64·T1 | Give up |
| K | non-INVITE client | T4 unreliable, 0 reliable | Absorb response retransmissions |
| G | INVITE server | T1 | Retransmit final response; `min(2G, T2)`. Unreliable only |
| H | INVITE server | 64·T1 | Give up waiting for ACK |
| I | INVITE server | T4 unreliable, 0 reliable | Absorb ACK retransmissions |
| J | non-INVITE server | 64·T1 unreliable, 0 reliable | Absorb request retransmissions |
| L | INVITE server | 64·T1 | RFC 6026: absorb ACK after a 2xx |
| M | INVITE client | 64·T1 | RFC 6026: absorb 2xx retransmissions |

## 4. State tables

These tables are the specification. The implementation is written from them and the tests walk
them row by row; if a row and the code disagree, the row is right.

`—` means no state change. Outputs are abbreviated: **S** send, **A** send ACK, **T** to the
TU, **X** terminate.

### 4.1 INVITE client transaction (§17.1.1, amended by RFC 6026)

| State | Input | → State | Outputs |
|---|---|---|---|
| *(init)* | — | Calling | S request; set A *(unreliable)*; set B |
| Calling | timer A | — | S request; set A ×2 |
| Calling | timer B | Terminated | T timeout; X |
| Calling | 1xx | Proceeding | T response; clear A |
| Calling | 2xx | Accepted | T response; clear A, B; set M |
| Calling | 300–699 | Completed | A; T response; clear A, B; set D |
| Calling | transport error | Terminated | T error; X |
| Proceeding | 1xx | — | T response |
| Proceeding | 2xx | Accepted | T response; clear B; set M |
| Proceeding | 300–699 | Completed | A; T response; clear B; set D |
| Completed | 300–699 | — | A *(retransmit; not passed to the TU)* |
| Completed | timer D | Terminated | X |
| Accepted | 2xx | — | T response *(a fork answered; the TU needs each one)* |
| Accepted | timer M | Terminated | X |

The ACK for a non-2xx final response is generated **by the transaction** and reuses the
request's `Via` branch, because it belongs to the same transaction. The ACK for a 2xx is
**not**: it is a separate transaction the TU generates, since only the TU knows the dialog
route set. Conflating the two is a perennial source of calls that connect and then drop.

### 4.2 Non-INVITE client transaction (§17.1.2)

| State | Input | → State | Outputs |
|---|---|---|---|
| *(init)* | — | Trying | S request; set E *(unreliable)*; set F |
| Trying | timer E | — | S request; set E = min(2E, T2) |
| Trying | timer F | Terminated | T timeout; X |
| Trying | 1xx | Proceeding | T response |
| Trying | 200–699 | Completed | T response; clear E, F; set K |
| Proceeding | timer E | — | S request; set E = T2 |
| Proceeding | timer F | Terminated | T timeout; X |
| Proceeding | 1xx | — | T response |
| Proceeding | 200–699 | Completed | T response; clear E, F; set K |
| Completed | any response | — | *(absorbed)* |
| Completed | timer K | Terminated | X |

### 4.3 INVITE server transaction (§17.2.1, amended by RFC 6026)

| State | Input | → State | Outputs |
|---|---|---|---|
| *(init)* | INVITE | Proceeding | T request; set 100-timer (200 ms) |
| Proceeding | 100-timer | — | S 100 Trying *(only if the TU has not responded)* |
| Proceeding | INVITE *(retransmission)* | — | S last response *(**not** passed to the TU)* |
| Proceeding | TU 1xx | — | S response |
| Proceeding | TU 2xx | Accepted | S response; set L |
| Proceeding | TU 300–699 | Completed | S response; set G *(unreliable)*; set H |
| Completed | INVITE | — | S last response |
| Completed | timer G | — | S last response; set G = min(2G, T2) |
| Completed | timer H | Terminated | T timeout; X |
| Completed | ACK | Confirmed | clear G, H; set I |
| Confirmed | ACK | — | *(absorbed)* |
| Confirmed | timer I | Terminated | X |
| Accepted | TU 2xx | — | S response *(a retransmission the TU asked for)* |
| Accepted | ACK | — | T ACK *(the TU needs it; the transaction does not absorb it)* |
| Accepted | timer L | Terminated | X |

The asymmetry in the last block is RFC 6026's point: an ACK for a 2xx is not part of this
transaction, so it goes to the TU rather than being swallowed, but the transaction stays alive
long enough (Timer L) that a retransmitted 2xx does not create a second one.

### 4.4 Non-INVITE server transaction (§17.2.2)

| State | Input | → State | Outputs |
|---|---|---|---|
| *(init)* | request | Trying | T request |
| Trying | request *(retransmission)* | — | *(absorbed — **not** passed to the TU)* |
| Trying | TU 1xx | Proceeding | S response |
| Trying | TU 200–699 | Completed | S response; set J |
| Proceeding | request | — | S last response |
| Proceeding | TU 1xx | — | S response |
| Proceeding | TU 200–699 | Completed | S response; set J |
| Completed | request | — | S last response |
| Completed | timer J | Terminated | X |

Absorbing retransmissions is the load-bearing behaviour here. A UDP peer that does not see a
response resends the request every T1; if each copy reached the application, a REGISTER would
be processed seven times.

## 5. RFC 6026

RFC 3261 sends the INVITE client transaction straight to Terminated on a 2xx, and the server
transaction likewise. That leaves retransmitted 2xx responses — which forking proxies produce
routinely — matching no transaction, and an ACK for a 2xx arriving at a transaction that no
longer exists.

sipx implements RFC 6026: an `Accepted` state on both machines, with Timer M and Timer L. The
consequence worth stating is that **a 2xx may be delivered to the TU more than once**, and the
TU must be able to see two 200s for one INVITE and answer each — that is a fork, not a bug.

## 6. Matching

### 6.1 Server transactions (§17.2.3)

If the request's top `Via` `branch` begins with `z9hG4bK`, the key is
`(branch, sent-by, method)`, where the method is `INVITE` for an ACK so that the ACK matches
the INVITE it acknowledges.

Otherwise the sender predates RFC 3261 and the key is derived from the `Request-URI`, the top
`Via`, the `From` tag, the `Call-ID`, the `CSeq` number and the method — with, for an ACK, the
`To` tag of the response that was sent, since an ACK for a non-2xx must match the INVITE.

### 6.2 Client transactions (§17.1.3)

The key is `(branch, CSeq method)`, both taken from the response's top `Via` and `CSeq`. A
response whose branch matches nothing is passed to the TU rather than dropped: it may be a
stray fork response the core has no business discarding silently.

Note that this is **narrower than §6.1**, on purpose. The server rule needs the `Request-URI`,
the `From` tag and the `To` tag to tell one legacy transaction from another; the client rule
does not, and cannot — a response has no Request-URI to compare.

> **Known deviation.** `TransactionKey::from_sent_request` derives a client key with
> `from_request`, which is §6.1's rule, so a client transaction on a pre-RFC-3261 branch is
> keyed with a Request-URI and a `To` tag that its responses can never carry. Those two keys
> never compare equal, and every response to such a transaction is `Unmatched`: the call does
> not fail, it hangs until Timer F. Found by the `transaction_sequence` fuzz target (§8) and
> pinned by the ignored test `a_legacy_client_transaction_never_sees_its_response`. The fix is
> a separate client derivation and belongs to its own story.

### 6.3 CANCEL

CANCEL matches the transaction of the request it cancels, not one of its own: same branch,
method `INVITE`. A CANCEL that matches nothing gets a 481.

## 7. Test vectors

Every row of every table above, driven as `(state, input) → (state, outputs)`, with no clock
and no socket. Beyond that:

| # | Scenario | Expected |
|---|---|---|
| T1 | INVITE client, unreliable, no response | Request sent at 0, T1, 3·T1, 7·T1 … then timeout at 64·T1 |
| T2 | INVITE client receives 302 | Exactly one ACK, generated by the transaction, reusing the request branch |
| T3 | INVITE client receives 200 | **No** ACK from the transaction; the TU is told |
| T4 | INVITE client receives 200 twice | The TU is told twice; the transaction stays in Accepted |
| T5 | Server, request retransmitted before any response | The TU sees the request exactly once |
| T6 | Server, request retransmitted after a final response | The last response is resent; the TU sees nothing |
| T7 | INVITE server, TU silent for 200 ms | A 100 Trying goes out |
| T8 | INVITE server, TU responds within 200 ms | **No** 100 Trying |
| T9 | Reliable transport | Timers A, G and J are never set; D, I and K fire immediately |
| T10 | 10 000 transactions created and terminated | The stores are empty |
| T11 | ACK for a non-2xx | Absorbed by the server transaction |
| T12 | ACK for a 2xx | Delivered to the TU |
| T13 | RFC 2543 request (no magic cookie) | Matched by the legacy key |
| T14 | CANCEL | Matches the INVITE's transaction, not a new one |

## 8. Fuzzing the driver

The vectors above prove the rows somebody thought of. The `transaction_sequence` fuzz target
covers the sequences nobody did: its input decodes into a program of events — an incoming
message, an application request, a fired timer — driven into `TransactionLayer` in whatever
order the decoder produces. Nothing in it parses. Messages are *built*, so the budget is spent
in the machines rather than on bytes that never become a message; the four parser targets
already cover that. The harness is `sipx_testkit::transaction_sequence`, seeded from §7's
scenarios encoded as programs.

This is only possible because of §2. A machine that read a clock would need one to be faked; a
machine that owned a socket could not be driven at all. Time enters as an input, so a campaign
needs no runtime and no sleeping, and 64·T1 costs one event.

A panic-only oracle would find nothing here — the machines are total, so almost every sequence
"succeeds" and the failures that matter are silent. These are asserted instead:

| Invariant | What it rules out |
|---|---|
| No transaction outlives its terminal state | A machine that emits `Terminated` and stays in the store, or arms a timer in the batch that retires it |
| No timer fires for a key that has been removed | A timer arriving after cancellation resurrecting a retired machine — the race a driver's timer wheel cannot close |
| The store is bounded by the vocabulary | Growth as a function of sequence length rather than of key space: the slow, quiet outage |
| No unnamed state | A state legal in the type and absent from §4's table for *that* machine — an INVITE client in `Trying`, a non-INVITE server in `Confirmed` |
| A response reaches the transaction that sent its request | §6.2 matching failing silently, which is a call that hangs rather than one that errors |

Defects the campaign has found and that are not yet fixed are listed in
`transaction_sequence::KNOWN_DEFECTS`, each with an ignored regression test. Suppressing one is
how the campaign reaches what is behind it; a test that fails once the defect is fixed is how
the suppression gets removed.
