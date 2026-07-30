---
id: X-19
title: Fuzz the transaction driver, not only the parser
pillar: Build
status: done
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, sipx-testkit]
predicate: 2
note: M12 · four fuzz targets, all of them parsers; the timing half of the north star is untested
---

# Fuzz the transaction driver, not only the parser

## Goal
Fuzz *sequences* — messages and fired timers driven into the transaction layer — so the half of the
north star about adversarial **timing** is tested the way the half about adversarial input already is.

## Acceptance
- [x] A fuzz target drives `TransactionLayer` with a sequence decoded from the fuzzer's bytes:
      incoming messages, application requests and fired timers, in any order the decoder can produce.
      The existing four targets (`parse_datagram`, `parse_stream`, `parse_uri`, `roundtrip`) all stop
      at the parser.
      → `fuzz/fuzz_targets/transaction_sequence.rs`; the seven-event vocabulary is
      `transaction_sequence::Event`, driven by `Driver::step`.
- [x] Sequences are structured, not raw bytes reinterpreted as SIP. A fuzzer that spends its budget
      producing unparseable messages is re-testing `S-4`; this one must reach the state machines, so
      the input is a decoded program over a small vocabulary of events.
      → `Program::decode`, four bytes an event; messages are **built** with `RequestBuilder` /
      `ResponseBuilder`, never parsed. `the_decoder_is_total_and_every_event_kind_is_reachable`
      proves the decoder rejects nothing and reaches all seven kinds.
- [x] The invariants asserted are the ones a state machine can violate without panicking, because a
      panic-only oracle would find almost nothing here. At minimum: no transaction outlives its
      terminal state, no timer fires for a key that has been removed, the store never grows without
      bound over a bounded sequence, and no state is reachable that the RFC 3261 §17 tables (as
      amended by RFC 6026) do not name.
      → `transaction_sequence::Invariant`, five variants; the fifth is §6.2 matching. Each has a
      test: `no_state_outside_the_rfc_3261_tables_is_reachable`,
      `a_timer_that_fires_after_its_transaction_is_gone_changes_nothing`,
      `the_store_is_bounded_by_the_vocabulary_and_not_by_the_program`.
- [x] The corpus is seeded from something meaningful rather than from nothing — the sequences the
      existing FSM table tests already encode are the obvious seed, the way CI seeds the parser targets
      from the RFC 4475 corpus.
      → 17 programs in `crates/sipx-testkit/corpus/transaction-sequences/`, generated from
      `seeds()`, which is §7's T1–T14 rewritten as event programs.
- [x] It runs in the existing `fuzz smoke` CI job under the same time budget, with the corpus
      committed and proven unmodified, exactly as the parser targets are.
      → `ci.yml` job `fuzz`, step "Fuzz the transaction driver", `-max_total_time=60`, seed corpus
      passed second (read-only) and checked afterwards with `git diff --exit-code`.
- [x] Any crash or invariant violation is minimised, committed as a regression test in
      `crates/sipx-sip`, and fixed in its own story. The fuzzer is the instrument.
      → One found; minimised to 8 bytes and committed as
      `a_legacy_client_transaction_never_sees_its_response`, `#[ignore]`d pending its own story.
- [x] Failing-first test: the minimised regression test for the first sequence it finds — and if it
      finds none in its first campaign, `a_seeded_event_sequence_replays_the_same_transaction_trace`,
      which proves the harness is driving what it claims to drive.
      → Both forms are present, and both are needed: the harness test is the one that runs in the
      gate, and the minimised regression is the one that must stay red.

## Progress
- **Done, with one defect handed on.** The harness — event vocabulary, decoder, driver and
  invariant oracle — is `sipx_testkit::transaction_sequence`, so the fuzz target and the
  regression tests drive the same code and a corpus entry that crashes the fuzzer replays in an
  ordinary test byte for byte.
- **The first campaign found a real defect.** 115 217 runs over seven minutes, one class of
  finding, minimised by `cargo fuzz tmin` to eight bytes: a response to an RFC 2543 (no magic
  cookie) client transaction matches nothing, because `TransactionKey::from_sent_request` derives
  the client key by §17.2.3's *server* rules — Request-URI and `To` tag included — while
  `from_response` cannot carry either. `docs/specs/sip-transaction.md` §6.2 already specifies the
  client key as `(branch, CSeq method)`, so the code deviates from the spec rather than the spec
  being unclear. Not fixed here, per the story's last note: **the fuzzer is the instrument.** It
  needs a story of its own to give `TransactionKey` a client derivation.
- The campaign steps over that one defect (`transaction_sequence::KNOWN_DEFECTS`) so it can reach
  what is behind it; `run_strict` suppresses nothing and is what the ignored regression test
  calls. `the_known_defect_suppression_is_still_needed_and_still_works` fails the moment the fix
  lands, which is when the suppression and the `#[ignore]` should both go.
- After the suppression, seven more minutes found nothing: 1 987 coverage features, a corpus that
  grew to 779 entries, no second class.
- Adjacent, not fixed: the INVITE client transaction emits `ClearTimer(Timer::A)` on a *reliable*
  transport, where Timer A was never armed (visible in the `t9-…` seed's trace). Harmless — a
  driver clearing an unarmed timer is a no-op — but it is an output the §4.1 table does not have.

## Notes
- This is exactly the kind of testing the sans-IO design was chosen to make possible: the transaction
  layer takes fired timers as inputs and produces effects, so a fuzzer needs no clock, no socket and
  no runtime. If it turns out to be awkward, the awkwardness is a finding about the API.
- It pairs with `X-14`'s deterministic pieces — a generic timer queue and a seeded loopback link —
  which arrive in M7. Those make a *driver-level* campaign possible too, over two whole stacks; that
  is a bigger story and this one deliberately stays inside `sipx-sip`.
- The last acceptance item is written the way it is on purpose. A fuzzing story whose failing-first
  test is a bug it has not found yet cannot be started, and one that promises to find a bug cannot be
  finished honestly.
