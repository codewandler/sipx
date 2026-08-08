---
id: T-36
title: Refresh server-transaction liveness on provisional progress
pillar: Signalling
status: done
priority:
design: docs/designs/sip-transport.md
epic: sip-transport
areas: [sipx-transport]
predicate:
announcement:
note: requested by sipx-clstr CX-19 · unanswered_limit currently measures from first handoff even while the application keeps reporting progress
---

# Refresh server-transaction liveness on provisional progress

## Goal

Keep the finite backstop for a server transaction the application abandons, while allowing an
unbounded sequence of genuine provisional progress followed by a final response. Today
`Config::unanswered_limit` measures from the request's first handoff even after successful 1xx
responses, so a live long-ringing transaction is eventually removed and a later final response
fails with `Error::NoTransaction`.

## Contract decided before implementation

This changes the contract of the existing keyed `Handle::respond` operation; it does **not** add
a second public progress operation.

- The guard begins when a newly created server transaction is handed to the application.
- A successful application `Handle::respond` with a 100–199 response refreshes that exact server
  transaction's guard after the response has been performed. Transaction-generated 100 Trying and
  retransmission of a stored response do not refresh it: neither is new application progress.
- A successful final response removes the transaction from the unanswered guard immediately. The
  transaction layer still owns its RFC 3261 absorption/termination timers; final response does not
  mean those transaction internals are discarded.
- Silence for longer than `unanswered_limit` since handoff or the last successful provisional
  response retains today's finite abandonment, warning and `discard.unanswered` observation.
- A provisional or final response for a missing, abandoned or fully terminated key returns the
  existing `Error::NoTransaction` and cannot recreate or refresh anything.

An explicit `progress(key)` is deliberately absent. It would let an application preserve a leaked
transaction without producing any SIP progress, while every useful progress signal at this layer
is already an exact-key provisional response. `Handle::outstanding()` remains an aggregate
operational snapshot, not an inference surface for per-transaction lifecycle.

## Acceptance

- [x] `docs/specs/sip-transport.md` defines the unanswered-guard state table above before driver
      implementation, including which automatic and retransmitted responses do not count.
- [x] Failing-first deterministic test
      `repeated_provisional_progress_refreshes_the_unanswered_backstop` pauses time with a 60-second
      limit, hands an INVITE to the application, successfully sends `180` at 20 seconds and `183`
      at 50 seconds, runs the 90-second sweep, and proves a later final response still succeeds.
      The last progress is only 40 seconds old although the original absolute deadline has passed.
- [x] No-progress coverage proves a request with no application response is still abandoned after
      the finite limit, increments `discard.unanswered`, and rejects a later response with
      `Error::NoTransaction`.
- [x] One-provisional-then-silence coverage proves a successful 1xx starts a fresh finite interval
      rather than exempting the transaction from collection.
- [x] Final-response coverage proves the unanswered guard stops counting a transaction immediately
      after the successful final response while the RFC transaction remains available for its
      normal retransmission/ACK absorption lifetime.
- [x] Stale-key coverage proves provisional and final responses after abandonment or full
      termination return `Error::NoTransaction` and do not alter counters, timers or maps.
- [x] The implementation refreshes only the endpoint driver's existing exact-key guard. It exposes
      no transaction internals, adds no shadow lifecycle table, and preserves the finite warning
      and counter backstop.
- [x] The full repository gate is green.

## Filing evidence

- 2026-08-05: Reproduced on immutable `1.0.0-beta.7` commit
  `0034ee364252aef2c996eb24278c6cd70cb3c48f` with paused Tokio time and a 60-second
  `unanswered_limit`. An INVITE received `180` at 20 seconds and `183` at 50 seconds; at the
  90-second sweep the driver abandoned it from the original handoff timestamp, and the final
  response failed exactly as `NoTransaction`. The failing-first test completed in virtual time
  with no wall-clock wait.
- The cause is local to `sipx-transport`: `Endpoint::on_message` inserts one timestamp when the
  request is handed over, `on_respond_command` retains but never refreshes it for a provisional
  response, and `abandon_unanswered` compares every entry with that original timestamp.

## Progress

- 2026-08-05: The guard now records handoff or last successfully transmitted application
  provisional progress under the existing exact transaction key. Transaction-generated output
  does not enter that path; final response and transaction termination remove the guard. The
  output executor reports whether a message reached its configured transport boundary, so an
  output with no destination cannot refresh the timestamp merely because the transaction layer
  produced `Output::Send`.
- 2026-08-05: The failing-first paused-time test initially lost the transaction at the old absolute
  deadline and returned `NoTransaction`. The complete UDP target now passes 18 tests, including
  repeated progress at 20 and 50 seconds, one-progress-then-silence, final-response absorption,
  no-progress collection and stale provisional/final keys. A driver-level test also removes the
  destination and proves an untransmitted provisional cannot move the guard timestamp. Strict
  all-feature Clippy and the no-default-feature library check pass. The story remains in progress
  only because the one full repository gate is deliberately deferred until the integrated tree is
  frozen.

## Notes

- Requested by downstream
  [sipx-clstr CX-19](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/CX-19-file-server-transaction-progress-upstream.md)
  through its
  [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
  Proxy Timer C and branch progress policy stay in the downstream forwarding application; the
  generic exact server-transaction lifetime stays here.
- Normative basis: RFC 3261 §17.2.1 permits an INVITE server transaction to remain in Proceeding
  while provisional responses are sent and defines its later final-response transition. The
  application leak backstop is endpoint policy layered above that state machine.
