---
id: T-45
title: Give each load candidate its own call identity
pillar: Transport
status: done
priority: 3
design: docs/designs/endpoint-resolution.md
epic: endpoint-resolution
areas: [sipx-cli]
predicate:
announcement:
note: load's signalling workload reuses one Call-ID and From tag across every address of a target, so a second candidate arrives at the same server as a merged request
---

# Give each load candidate its own call identity

## Goal

Present each address of a `load` target with a request the far end can accept, so a name whose
first address is dead is reachable at the second even when both addresses lead to the same server.

## Acceptance

- [x] `load --mode signalling` builds a fresh `Call-ID` and `From` tag for each candidate its
      serial pass attempts, as `peers` does for each subscription attempt.
- [x] A failing-first test proves it: a target whose addresses reach one server, whose first
      address refuses, is connected rather than answered `482 Loop Detected`.
- [x] The run's reproducibility is unchanged — the identity stays derived from `--seed` and the
      call index, with the candidate position as the only new input.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `T-42`'s adjacent findings. `SignallingIdentity::new` is called once per
  admitted call, outside the candidate pass, and every candidate reuses its `call_id` and its
  `From` tag; only the Via branch differs. Two addresses of one name commonly resolve to the same
  server, and RFC 3261 §8.2.2.2 makes the second INVITE a *merged request* there — same Call-ID,
  same From tag, same CSeq, different branch — which a compliant server answers `482 Loop
  Detected`. `load` then classifies that as a rejection and stops the pass, so the fallback the
  pass exists for does not happen.

  `peers` states the rule this should follow, in `crates/sipx-cli/src/peers.rs`: *"A fresh dialog
  identity per candidate. A subscription attempted at another address is a new usage (RFC 6665
  §4.1.2.1), and reusing the Call-ID and tag of the attempt that just failed would present it to
  the registrar as the first one arriving twice."* The same sentence is true of an INVITE.

  Not fixed in `T-42` because that story's contract was the pass and its reporting, and changing
  what goes on the wire for every load call is a separate, separately testable decision.

- 2026-08-08: renumbered from `T-43` to `T-45`. The coordinator reserved `T-43` for this story's
  author without noticing `T-33` had already filed a `T-43` for reaching a real WSS endpoint from a
  browser fixture — so the reservation that was meant to stop id collisions caused this one. Only
  the id moved.

- 2026-08-09: implemented. `SignallingIdentity` is split in two. `CallIdentity` is what an admitted
  call has before the pass chooses an address — the `To` header, the `From` URI and the `Contact` —
  and `CallIdentity::at(position)` mints the `Call-ID` and the `From` tag the pass presents at one
  candidate. The tag is now `f-<seed>-<call index>-<candidate position>` and the Call-ID
  `cl-<run id>-<call index>-<candidate position>@driver.invalid`: still derived from `--seed` and
  the call index, with the position as the only new input, so a replayed run puts the same bytes on
  the wire. The position is counted in the closure `sipx_transport::destination::walk` already
  calls once per attempted candidate, in order — no second pass, and nothing added to the shared
  one. `--mode generated-media` needed no change: `sipx_call::dial_until` builds `Identity::fresh()`
  itself and the pass calls it per candidate, so only the hand-rolled signalling path had the
  defect.

  Failing-first, at the merge base `c405a60`:

  ```
  $ cargo test -p sipx-cli --all-features --bin sipx load::tests::a_second_candidate
  test load::tests::a_second_candidate_at_the_same_server_gets_a_call_of_its_own ... FAILED

  panicked at crates/sipx-cli/src/load.rs:1369:13:
  the second address of a name whose first one took the INVITE and dropped must be a call of its
  own, not the first one arriving twice: Rejected(482)
  ```

  The fixture is one server on two loopback addresses over one shared record of what it has
  accepted. The first address reads the INVITE and drops the connection — the only classification
  the pass walks past, and `sipx-transport` fails such a transaction at once rather than at Timer B
  — which leaves the server holding the request. The second address then answers `482 Loop
  Detected` to a copy of it and `200 OK` to a new call, which is what RFC 3261 §8.2.2.2 requires of
  a UAS and what the defect could not tell apart. `peers` and `load` now write down the same rule:
  `peers.rs` for a subscription usage (RFC 6665 §4.1.2.1), `CallIdentity`'s doc comment for an
  INVITE.

  Not run here, by dispatch: `./scripts/gate.py` — the wave runs one gate, so the row stays
  unticked. Verified in this worktree instead: `cargo test -p sipx-cli -p sipx-transport
  --all-features`, `cargo clippy` on both with `-D warnings`, `cargo fmt --all`,
  `check-fixed-sleep.py --check`, `check-outcome-parity.py --check`, `check-cli-reference.py`.

  Owed to the CHANGELOG, for the coordinator to write (this story may not touch it): *`sipx load
  --mode signalling` now gives every address of a target its own `Call-ID` and `From` tag, so a
  name whose addresses lead to one server is reachable at the second when the first one fails
  instead of being refused `482 Loop Detected` as a merged request (RFC 3261 §8.2.2.2).*

- 2026-08-09: closed at the `1.0.0-rc.13` boundary, against the wave gate run on this tree.
