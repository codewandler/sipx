---
id: P-31
title: Walk every candidate in peers
pillar: Phone
status: in-progress
priority: 3
design: docs/designs/endpoint-resolution.md
epic: endpoint-resolution
areas: [sipx-cli]
predicate:
announcement:
note: peers --registrar resolves and then takes first() only, so a registrar whose first address is dead is unreachable from peers while register recovers
---

# Walk every candidate in peers

## Goal

Make `peers --registrar` try the resolved candidates the way every other outbound command does,
instead of giving up after the first address.

## Acceptance

- [x] `peers` walks the resolved candidate list under its own deadline, so a registrar whose first
      address refuses is still reached — matching `register`, `dial` and `load`.
- [x] A failing-first test proves a name whose first address is dead and whose second answers is
      reachable from `peers`, and is not today.
- [x] The failure report carries the same `candidates_attempted` / `candidates_resolved` fields
      `T-41` established, so a script reads one shape across commands.
- [x] The serial pass is not written a fifth time: reuse the helper `register` now delegates to.
- [ ] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-41`'s adjacent findings and verified — `crates/sipx-cli/src/peers.rs`
  resolves and then uses `first()`, never walking the list. This is a missing retry rather than a
  reporting gap: `register` recovers from a dead first address and `peers` does not, against the
  same registrar.

- 2026-08-08: **deferred out of rc.8 after reading the code.** Unlike `register`, which reuses one
  endpoint across candidates, `peers` selects an address *before* it binds: the chosen transport
  configures `TransportConfig`, which binds the endpoint, which the dispatcher and subscription are
  built on. Walking candidates therefore means retrying bind, dispatch and the first NOTIFY per
  candidate, not just re-sending a request — roughly the whole 80-line setup block restructured. That
  is worth doing properly rather than rushing; the defect is real and unchanged.

- 2026-08-08: **attempted and reverted.** The refactor is right and it compiled: the per-candidate
  work — transport configuration, bind, dispatcher, subscription and first notification — extracted
  into `attempt_candidate`, looped over the resolved list under `MAX_ATTEMPTS`, retrying only on a
  connection failure since a refusal or an authentication failure is the registrar speaking and a
  second address would not change it. All 93 `sipx-cli` tests passed.
  It was reverted because the **failing-first proof could not be finished**: the multi-candidate
  test needs the first candidate to *refuse*, and nothing refuses a UDP connection — so it needs a
  TCP registrar fixture that accepts, reads and responds, which the suite does not have. Landing a
  restructure of the `peers` setup path with only "the existing tests still pass" behind it is the
  shape this project's discipline exists to prevent. The `SPREAD` fixture name and its three
  loopback addresses are the right starting point for whoever picks this up.

- 2026-08-08: **done on the second attempt, and the fixture the first one was blocked on exists.**
  Every row but the gate is satisfied; the gate row is left for the wave run.

  **The blocker dissolved once the endpoint stopped being per-candidate.** The first attempt read
  the setup block as inseparable — transport config, bind, dispatcher, subscription and first
  NOTIFY all downstream of the selected address — and restructured all of it. It does not have to
  be. `register` binds *one* endpoint from the head of the list and attempts every candidate over
  it; `peers` can do exactly the same, because a `Target`'s transport is chosen when a request is
  sent, not when the socket is opened. So the endpoint, the dispatcher and the subscription runtime
  are still built once, and only `subscribe` + the first notification are per-candidate. The diff to
  `discover` is a fraction of the 80-line restructure that was reverted.

  **The pass is now written once, in the library.** `sipx_transport::destination::walk` is the
  serial pass extracted from `UserAgent::register_candidates` — generic over the per-candidate
  future and over the caller's error, with `Attempted::{Reached, Unreachable, Answered}` as the
  caller's classification and `Unreached::{Nothing, Expired, Unreachable, Answered}` as the outcome.
  `register_candidates` now delegates to it and keeps its own error mapping, so `register`'s
  behaviour is unchanged; `peers` reuses the same function. That is the "reuse the helper `register`
  now delegates to" row: before this story there was no reusable helper at all — the loop was inline
  and REGISTER-typed — so satisfying that row and the "not a fifth time" row meant extracting one.

  **Retry classification.** `Termination::TransactionFailed` alone moves the pass on: the SUBSCRIBE
  never reached a final response, so nothing has answered and a later address still can. A refusal,
  an exhausted challenge, or an accepted subscription that stays silent is the registrar speaking,
  and a second address repeats a question already answered — the rule `register` applies, applied to
  a SUBSCRIBE. `only_a_transaction_that_never_answered_moves_to_the_next_address` pins it.

  **The deadline.** `FIRST_NOTIFY` (20s) becomes the bound over the *whole* pass rather than over
  one address's wait, funding every candidate together — sixteen copies of it would multiply the
  only duration this command states by sixteen (`P-26`). `--expires` is untouched as the resolution
  ceiling, so `P-26`'s judgement call stands and did not need revisiting: this adds no new flag and
  no new clock, it generalises the one that was already there. `--watch` is deliberately outside the
  budget — it is an observation window opened after a registrar has been reached.

  **The fixture the first attempt could not build.** `notifying_registrar` in
  `crates/sipx-cli/tests/cli.rs`: a TCP listener that frames SIP off the stream (blank line plus
  `Content-Length`, RFC 3261 §7.5), answers each SUBSCRIBE 200 OK with a dialog tag and a Contact,
  and sends a reg-info NOTIFY. It stays in a loop rather than answering once, so the command's
  unsubscribe is answered instead of waiting for a response nothing would send. It binds the
  *second* `SPREAD` address (`127.0.0.2`) at a port reserved and released on the first, so the head
  of the list refuses and the pass has to walk past it. TCP throughout, which is the point the first
  attempt got stuck on: a closed UDP port is not refused, merely unanswered.

  One wrong turn worth recording: the first cut built the NOTIFY's `Via` sent-by from the Contact
  URI, giving `registrar@127.0.0.2:PORT`. A sent-by is a host and a port and nothing else
  (RFC 3261 §20.42), so every NOTIFY was discarded and the command timed out at 20s having reached
  the right address. The fixture now takes the bound `SocketAddr` and derives Via and Contact
  separately.

  Failing-first at `ae65b5b`, `peers_reaches_a_registrar_whose_first_address_is_dead`:
  `{"status":"failed","error":"registrar subscription failed: TransactionFailed"}` — exit 1, the
  first address's refusal reported as the command's answer with the other two never tried. Green
  after, in 0.04s. `a_registrar_that_answers_at_no_address_reports_what_it_attempted` covers the
  `T-41` field row: three attempted of three resolved, from `peers`.

  Verified: `cargo test -p sipx-cli --all-features` (115 + 97 + others, all pass),
  `cargo test -p sipx-ua -p sipx-transport --all-features`, `cargo clippy` over all three touched
  crates with `-D warnings`, `cargo fmt --all`, `check-fixed-sleep.py --check`,
  `check-cli-reference.py`, `check-provenance.sh`. `./scripts/gate.py` was **not** run — it belongs
  to the wave.

  CHANGELOG sentence for whoever reconciles it: *`peers --registrar` now tries every resolved
  address of a registrar in turn instead of giving up after the first, and reports
  `candidates_attempted`/`candidates_resolved` when none of them answers.*

## Notes

- `T-41` moved the serial pass into `UserAgent::register_candidates`. `load` and `scenario` still
  hand-roll their own — the loop now exists four times across three files and only two of them
  count attempts. This story should take the helper rather than add a fifth copy.
  **Filed as `T-42`:** five hand-rolled loops remain in `dial` (two), `load` (two) and `scenario`,
  and the three in `load` and `scenario` still count no attempts, so their connection failures
  cannot say how far they got. `walk` now exists for them to take.
- `P-26` made `--expires` the resolution ceiling for `peers`, which was a judgement call recorded
  in that story; if this work gives `peers` a real attempt deadline, revisit that reading.
