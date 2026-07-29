---
id: S-26
title: Match a response to the RFC 2543 client transaction that sent it
pillar: Signalling
status: in-progress
priority: 7
design: docs/specs/sip-transaction.md
epic: sip-core
areas: [sipx-sip]
note: found by X-19's fuzzer — from_sent_request derives the client key by §17.2.3's server rules, so a legacy key carries a Request-URI and To tag that from_response cannot, and every response is Unmatched
---

# Match a response to the RFC 2543 client transaction that sent it

## Goal
Make a response to a client transaction whose `Via` carries no magic cookie find the transaction
that sent the request, instead of matching nothing and leaving the call to hang until Timer F.

## Acceptance
- [x] `TransactionKey::from_sent_request` stops deriving the client key through `from_request`.
      That function implements RFC 3261 **§17.2.3, the server rules**, so for a legacy `Via` it
      builds `Legacy { request_uri, to_tag, … }` from the request — and `from_response` builds the
      same variant with `request_uri: Vec::new()` and the *response's* `to_tag`. The two can never
      compare equal, on two independent fields.
      → `crates/sipx-sip/src/transaction/key.rs:151`. `from_sent_request` derives the key itself,
      and the legacy half is `Self::legacy_client`, which `from_response` calls too — so the two
      agree by construction rather than by inspection.
- [x] The client key is what `docs/specs/sip-transaction.md` §6.2 already specifies. **This is code
      deviating from a written spec, not an under-specified case** — the spec does not need
      changing, the code does. Remove §6.2's "Known deviation" note in the same commit.
      → Note removed. §6.2 gained no permission the code needed: what was added says which fields
      stand in for the branch when a legacy `Via` has none, and who can produce such a `Via`.
- [x] The fix is verified for the **legacy** path specifically, not only for RFC 3261 branches.
      Every existing transaction test uses a magic-cookie branch, which is why nothing caught this.
      → `a_legacy_client_transaction_matches_its_own_response`,
      `crates/sipx-sip/tests/transactions.rs`, next to `T13`'s legacy server-matching test. It
      asserts the key equality directly and then drives the response through `TransactionLayer`.
- [x] `crates/sipx-sip/tests/transaction_sequences.rs ::
      a_legacy_client_transaction_never_sees_its_response` loses its `#[ignore]` and passes. It is
      the minimised regression `X-19`'s fuzzer produced, already committed and already failing.
      → `#[ignore]` removed; the eight bytes are unchanged.
- [x] **The fuzzer's suppression comes out with the defect.** `X-19` added `KNOWN_DEFECTS` /
      `run_strict` so the campaign could explore past this bug, plus a test that fails once the fix
      lands. When that test fails, the suppression is what must be deleted — not the test.
      → `Known` is uninhabited and `KNOWN_DEFECTS` is empty
      (`crates/sipx-testkit/src/transaction_sequence.rs`), and the `UnroutableResponse` check no
      longer excludes the legacy slots, so the invariant holds on every slot.
- [x] Failing-first test: already committed and already red. Run
      `cargo test -p sipx-sip --test transaction_sequences -- --ignored` to see it.
      → Quoted in the handoff: `step 1: UnroutableResponse: a 180 response for 3/ACK matched
      nothing, but its client transaction is live`.

## Progress
- **Done.** The fix is one derivation. A `Legacy` key built for a *client* leaves `request_uri` and
  `to_tag` empty, and `from_sent_request` and `from_response` both build it through a private
  `legacy_client`, so the property the old doc comment claimed — "findable by the key its responses
  will produce" — is now structural. `from_request` is untouched, so §17.2.3's use of the `To` tag
  to tell one legacy *server* transaction from another still holds
  (`legacy_matching_distinguishes_the_to_tag` still passes).
- Dropping the `To` tag from the client key is not only about matching the request that created the
  transaction: it is what lets two forked 200s carrying two different tags both reach the one
  transaction that sent the INVITE, which RFC 6026's `Accepted` state requires. The RFC 3261 path
  never had the problem, because its key is the branch.
- **The legacy client key is wider than §6.2's headline `(branch, CSeq method)`**, and deliberately:
  a legacy `Via` may carry no branch at all, so keying on the branch alone would let two such
  transactions answer each other's responses — trading a hang for a mismatch, which is worse. It is
  the top `Via` verbatim, the `From` tag, the `Call-ID`, the `CSeq` number and the method: every
  field of §6.1's legacy key except the two a response cannot reproduce. §6.2 now says so.
- **Severity as corrected, not as first filed.** Nothing here is written as an interop fix. The
  topmost `Via` on a client transaction is always ours, so no peer reaches this path; the wording in
  §6.2, in the `from_sent_request` doc comment and in both tests says *application-supplied `Via`*.
- `the_known_defect_suppression_is_still_needed_and_still_works` did its job — it went red the
  moment the fix landed and named the three things to delete. It is replaced by
  `the_campaign_suppresses_nothing_and_run_agrees_with_run_strict`, which keeps the half of it that
  outlives the defect: `run` and `run_strict` must not diverge while nothing is suppressed, and a
  new suppression has to arrive with a regression test.
- The suppression was keyed on `slot >= FIRST_LEGACY_SLOT` — by slot, not by cause — so under `run`
  no `UnroutableResponse` was reported for any legacy slot. Removing it outright (rather than
  narrowing it) is what takes that breadth away with it.

## Notes
- **Found by `X-19`'s transaction-sequence fuzzer**, which is exactly what that story built the
  instrument for: four fuzz targets existed and all four stopped at the parser, so nothing had ever
  driven the §17 state machines with adversarial *sequences*. The minimised input is 8 bytes —
  `[0,127,0,12,65,3,1,9]` — a `SendRequest` (INVITE, UDP) followed by a `ReceiveResponse` (180).
- **Confirmed independently at integration**, in `crates/sipx-sip/src/transaction/key.rs`, by
  reading both constructors rather than by trusting the report. The doc comment on
  `from_sent_request` states the intent it fails to honour: *"a client transaction has to be
  findable by the key its responses will produce."*
- **Impact is a hang, not an error** — every response is `Dispatch::Unmatched`, so the transaction
  sits until Timer F expires instead of failing. RFC 3261 §17.1.3 requires the match; §17.2.3's
  rules are the server's and were never the client's.
- **Not reachable from the network, and the story was filed saying otherwise.** Corrected by X-19's
  independent review and verified again at integration: the topmost `Via` on a *client* transaction
  is always ours. `Endpoint::send` adds one when the request has none
  (`crates/sipx-transport/src/endpoint.rs:376-384`) and always via `new_branch()`, which prefixes
  `z9hG4bK` (`endpoint.rs:635`); every in-tree caller that builds its own Via uses the same helper
  (`call.rs:1934`, `call.rs:3524`). **No peer can trigger this** — it takes a downstream application
  handing `Endpoint::send` a request carrying its own cookieless Via. The original filing said
  "exactly the kind of peer an interop story would meet", which was wrong.
- **Priority dropped 2 → 7 because of that.** It stays worth fixing: the code contradicts both
  `docs/specs/sip-transaction.md` §6.2 and its own doc comment, and it is a loaded gun for any
  application that builds its own Via. It is not the interop hazard the first filing implied.
- The spec's "Known deviation" note and the `#[ignore]` reason inherit the same overstatement and
  should be reworded to say *application-supplied Via*, not *old peers*, when this is fixed.
  → Done, and the same rewording was applied to the RFC 2543 entry in `docs/rfc/registry.toml`,
  whose note read as though the fallback existed for messages that arrive from old peers. That is
  true of the *server* half only.
- Adjacent, found by the same harness and deliberately left: an INVITE client emits
  `ClearTimer(Timer::A)` on a reliable transport where Timer A was never armed (harmless, but not
  in §4.1's table); `send_request` silently replaces a client transaction that reuses a key; and
  one key can name both a client and a server transaction, with `on_timer` checking the client
  store first. The last of those is a real timer-identity question and probably its own story.
- Adjacent, found here: a legacy key uses the top `Via` **verbatim**, so a `received` or `rport`
  parameter added to the `Via` a UAS echoes (RFC 3261 §18.2.1) changes those bytes and the response
  stops matching. Pre-existing in `from_response`, not introduced by this fix, and now reachable.
