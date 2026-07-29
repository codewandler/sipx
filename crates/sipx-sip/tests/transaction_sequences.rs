//! The transaction-sequence fuzz harness, driven from ordinary tests.
//!
//! `X-19` fuzzes *sequences* — received messages, application requests and fired timers — into
//! the transaction layer, rather than bytes into the parser. The harness that decodes those
//! sequences and checks the invariants lives in `sipx_testkit::transaction_sequence` so that the
//! fuzz target and these tests drive exactly the same code: a corpus entry that crashes the
//! fuzzer is replayed here byte for byte, and a regression test is that entry plus a name.
//!
//! What is asserted here is the harness itself — that it decodes to a program, that the program
//! reaches the state machines, that the same bytes produce the same trace every time — plus one
//! regression test for the defect the first campaign found.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use sipx_testkit::transaction_sequence::{self as sequence, Event, Invariant, Program};

// ------------------------------------------------------------------------------------------
// The harness drives what it claims to drive
// ------------------------------------------------------------------------------------------

/// The named failing-first test of `X-19`.
///
/// It proves the harness drives what it claims to drive. A fuzz target over a decoded program
/// is worth nothing if the program never reaches the state machines: it would burn its budget
/// the way a raw-bytes-as-SIP target does, re-testing the parser that four other targets
/// already cover. So this asserts three separate things about one seed program.
///
/// 1. It replays deterministically — the same bytes, the same trace, twice.
/// 2. The trace shows a transaction being *created*, *driven* and *terminated*, which is the
///    claim "reaches the state machines" stated in a form a test can check.
/// 3. It records no invariant violation, so a violation later is a signal rather than noise.
#[test]
fn a_seeded_event_sequence_replays_the_same_transaction_trace() {
    let seed = sequence::seeds()
        .into_iter()
        .find(|seed| seed.name == "t2-invite-client-acks-a-non-2xx")
        .expect("the seed corpus names the INVITE client scenario");

    let bytes = seed.program.encode();
    let decoded = Program::decode(&bytes);
    assert_eq!(
        decoded, seed.program,
        "a seed must survive the encoding the fuzzer mutates"
    );

    let first = sequence::run(&decoded);
    let second = sequence::run(&Program::decode(&bytes));
    assert_eq!(
        first.trace, second.trace,
        "the same bytes must replay to the same trace, or a reported crash is unreproducible"
    );

    assert!(
        first.violations.is_empty(),
        "the seed must be clean: {:?}",
        first.violations
    );

    let trace = first.trace.join("\n");
    assert!(
        trace.contains("created client"),
        "the program must reach the client machine, not stop at the parser:\n{trace}"
    );
    assert!(
        trace.contains("state=Completed"),
        "a non-2xx final response must drive the client machine to Completed:\n{trace}"
    );
    assert!(
        trace.contains("terminated"),
        "Timer D must retire the transaction:\n{trace}"
    );
}

/// Every seed reaches a state machine and satisfies every invariant.
///
/// The second half is what makes the fuzz target's signal worth reading: a campaign that starts
/// from a corpus which already violates something reports that violation forever and nothing
/// else. The first half is the guard against the seeds decaying into no-ops — a program whose
/// every event was `unmatched` would still be "clean".
#[test]
fn every_seed_program_drives_a_transaction_and_breaks_no_invariant() {
    for seed in sequence::seeds() {
        let result = sequence::run(&seed.program);
        assert!(
            result.violations.is_empty(),
            "seed {} violated: {:?}",
            seed.name,
            result.violations
        );
        let trace = result.trace.join("\n");
        assert!(
            trace.contains("created client") || trace.contains("created server"),
            "seed {} never created a transaction:\n{trace}",
            seed.name
        );
    }
}

/// Together the seeds walk both client machines and both server machines.
///
/// Named states rather than a count, because "seventeen seeds" is a number that stays true while
/// the thing it stood for rots. `Confirmed` is the INVITE server's ACK-absorption state and
/// `Accepted` is RFC 6026's addition to both — the two that a corpus of happy paths would miss.
#[test]
fn the_seed_corpus_reaches_every_state_the_rfc_3261_tables_name() {
    let traces: String = sequence::seeds()
        .iter()
        .flat_map(|seed| sequence::run(&seed.program).trace)
        .collect::<Vec<_>>()
        .join("\n");

    for state in [
        "state=Calling",
        "state=Trying",
        "state=Proceeding",
        "state=Completed",
        "state=Confirmed",
        "state=Accepted",
    ] {
        assert!(
            traces.contains(state),
            "no seed reaches {state}; the corpus has stopped covering §17"
        );
    }
    for outcome in [
        "terminated=Completed",
        "terminated=Timeout",
        "terminated=TransportError",
    ] {
        assert!(
            traces.contains(outcome),
            "no seed reaches {outcome}; the corpus has stopped covering termination"
        );
    }
}

// ------------------------------------------------------------------------------------------
// The corpus is generated, and proven so
// ------------------------------------------------------------------------------------------

/// The committed corpus is exactly `seeds()`, encoded.
///
/// The corpus is committed test data whose provenance has to be reproducible — the same property
/// `scripts/import-rfc4475-corpus.sh --check` gives the parser corpus. Without this test, a seed
/// edited in Rust and not regenerated leaves the fuzzer starting from the old program while
/// every other test reads the new one.
#[test]
fn the_committed_corpus_is_exactly_the_seed_programs() {
    let dir = sequence::corpus_dir();
    let mut on_disk = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).expect("the seed corpus directory exists") {
        let entry = entry.expect("a readable directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "README.md" {
            continue;
        }
        on_disk.insert(name, std::fs::read(entry.path()).expect("a readable seed"));
    }

    let expected: BTreeMap<String, Vec<u8>> = sequence::seeds()
        .into_iter()
        .map(|seed| (seed.name.to_owned(), seed.program.encode()))
        .collect();

    assert_eq!(
        on_disk.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "the corpus and seeds() name different programs; regenerate with \
         `cargo run -p sipx-testkit --example dump_sequences -- --write`"
    );
    for (name, bytes) in &expected {
        assert_eq!(
            on_disk.get(name),
            Some(bytes),
            "corpus file {name} is not what seeds() encodes to; regenerate with \
             `cargo run -p sipx-testkit --example dump_sequences -- --write`"
        );
    }
}

/// Every committed corpus file decodes and replays, and none of them is empty.
///
/// A seed the decoder turns into nothing is a seed the fuzzer gets no coverage from, and it
/// would look exactly like a working one from outside.
#[test]
fn every_committed_corpus_file_decodes_to_a_program_that_runs() {
    for seed in sequence::seeds() {
        let bytes = std::fs::read(sequence::corpus_dir().join(seed.name)).expect("a seed file");
        let program = Program::decode(&bytes);
        assert!(
            !program.events.is_empty(),
            "{} decoded to nothing",
            seed.name
        );
        assert_eq!(
            program, seed.program,
            "{} does not decode back to the program that wrote it",
            seed.name
        );
    }
}

// ------------------------------------------------------------------------------------------
// The decoder
// ------------------------------------------------------------------------------------------

/// The decoder is total, and every event kind is reachable from bytes.
///
/// Totality is what lets libFuzzer mutate freely: an input the decoder rejects is an input whose
/// coverage is zero, and a decoder that rejects most of its input space is a fuzzer that is
/// mostly idle. Reachability is the other half — an opcode no byte selects is a whole class of
/// event the campaign can never produce, which is invisible unless something counts.
#[test]
fn the_decoder_is_total_and_every_event_kind_is_reachable() {
    let mut kinds = std::collections::BTreeSet::new();
    for op in 0..=u8::MAX {
        let program = Program::decode(&[op, op, op, op]);
        assert_eq!(program.events.len(), 1, "byte {op} decoded to no event");
        kinds.insert(match program.events[0] {
            Event::SendRequest { .. } => "SendRequest",
            Event::ReceiveRequest { .. } => "ReceiveRequest",
            Event::ReceiveResponse { .. } => "ReceiveResponse",
            Event::SendResponse { .. } => "SendResponse",
            Event::FireTimer { .. } => "FireTimer",
            Event::TransportError { .. } => "TransportError",
            Event::Abandon { .. } => "Abandon",
        });
        // And a decoded event re-encodes to bytes that decode to the same event, which is what
        // makes a seed written in Rust and a corpus file the same thing.
        let round_tripped = Program::decode(&program.encode());
        assert_eq!(round_tripped, program, "byte {op} does not round-trip");
    }
    assert_eq!(
        kinds.len(),
        7,
        "not every event kind is reachable from the decoder: {kinds:?}"
    );

    // A trailing partial record is ignored rather than misread, which is what lets libFuzzer
    // shrink an input a byte at a time.
    assert!(Program::decode(&[0, 0, 0]).events.is_empty());
    assert_eq!(Program::decode(&[0, 0, 0, 0, 1]).events.len(), 1);
}

// ------------------------------------------------------------------------------------------
// The invariants, each with a program that would break it
// ------------------------------------------------------------------------------------------

/// A timer that fires after its transaction has gone changes nothing.
///
/// A driver's timer wheel cannot cancel atomically: `ClearTimer` and the fired callback race, so
/// a timer arriving at a retired transaction is the normal case. It must not produce outputs and
/// above all must not put the transaction back — a resurrected machine is a leak that also
/// answers messages.
#[test]
fn a_timer_that_fires_after_its_transaction_is_gone_changes_nothing() {
    let seed = sequence::seeds()
        .into_iter()
        .find(|seed| seed.name == "stale-timers-fire-after-the-transaction-is-gone")
        .expect("the seed corpus names the stale-timer scenario");

    let result = sequence::run(&seed.program);
    assert!(
        !result
            .violations
            .iter()
            .any(|v| v.invariant == Invariant::TimerForRemovedKey),
        "{:?}",
        result.violations
    );

    let trace = result.trace.join("\n");
    assert!(
        trace.contains("stale"),
        "the scenario must actually fire a timer at a transaction that is gone:\n{trace}"
    );
    assert!(
        trace.ends_with("quiescent store=0/0 live=[]"),
        "the stale timers must leave the store empty:\n{trace}"
    );
}

/// The store is bounded by the vocabulary, not by the length of the program.
///
/// "Does not grow without bound over a bounded sequence" only means something if the bound is
/// independent of the sequence. Ten thousand events over four slots can name at most as many
/// transactions as four slots have keys, however they interleave.
#[test]
fn the_store_is_bounded_by_the_vocabulary_and_not_by_the_program() {
    // A deterministic pseudo-random walk over the whole opcode space: no timers are ever fired
    // deliberately, so nothing retires except by the machines' own doing.
    let mut bytes = Vec::new();
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    for _ in 0..10_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes()[..4]);
    }

    let result = sequence::run(&Program::decode(&bytes));
    let growth: Vec<_> = result
        .violations
        .iter()
        .filter(|v| v.invariant == Invariant::StoreGrowth)
        .collect();
    assert!(
        growth.is_empty(),
        "10 000 events over {} slots grew the store: {growth:?}",
        sequence::SLOTS
    );
}

/// No state outside the four §17 tables is reachable, over a long random walk.
///
/// `ClientState` and `ServerState` each cover two machines, so the type proves nothing: an
/// INVITE client transaction sitting in `Trying` — the *non*-INVITE machine's waiting state —
/// compiles and means nothing. This drives every opcode at every machine and checks the legal
/// set per machine.
#[test]
fn no_state_outside_the_rfc_3261_tables_is_reachable() {
    let mut bytes = Vec::new();
    let mut state = 0x853c_49e6_748f_ea9b_u64;
    for _ in 0..20_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes()[..4]);
    }

    let result = sequence::run(&Program::decode(&bytes));
    let unnamed: Vec<_> = result
        .violations
        .iter()
        .filter(|v| {
            v.invariant == Invariant::UnnamedState || v.invariant == Invariant::OutlivedTermination
        })
        .collect();
    assert!(unnamed.is_empty(), "{unnamed:?}");
}

// ------------------------------------------------------------------------------------------
// Regression: what the first campaign found
// ------------------------------------------------------------------------------------------

/// The eight bytes libFuzzer minimised the first campaign's crash down to.
///
/// `00 7f 00 0c` — `SendRequest` on slot 3, method `INVITE`, unreliable transport.
/// `41 03 01 09` — `ReceiveResponse` on slot 3, `CSeq` method `ACK` (which §17.2.3 folds onto
/// `INVITE`), status `180`.
///
/// Slot 3's branch carries no magic cookie, so both keys go down the RFC 2543 path.
const LEGACY_CLIENT_CRASH: [u8; 8] = [0x00, 0x7f, 0x00, 0x0c, 0x41, 0x03, 0x01, 0x09];

/// A response to an RFC 2543 client transaction never reaches it.
///
/// **The defect the first campaign found, minimised. It is not fixed here** — `X-19` builds the
/// instrument and the story explicitly says the fuzzer is the instrument, not the fix — so this
/// test is `#[ignore]`d and the story that fixes it removes the attribute. Reproduce it outside
/// the test with `cargo fuzz run transaction_sequence` and the bytes above.
///
/// What goes wrong: `TransactionKey::from_sent_request` derives a client transaction's key with
/// `from_request`, which implements §17.2.3 — the *server* matching rules — and therefore
/// includes the Request-URI and the `To` tag. `TransactionKey::from_response` leaves the
/// Request-URI empty, because a response has none. The two legacy keys can never compare equal,
/// so every response to a pre-RFC-3261 client transaction is `Dispatch::Unmatched` and the
/// transaction sits retransmitting until Timer F.
///
/// §17.1.3's legacy client rule is narrower than §17.2.3's server rule on purpose: a response
/// matches a client transaction on the `Via` branch of the request that created it plus the
/// `CSeq` method, and on nothing else. The client key needs its own derivation.
///
/// It is a silent failure — no panic, no error, a call that simply never completes against
/// exactly the peers that are old enough to still be on UDP. That is the class of defect the
/// story wanted an oracle for, and a panic-only fuzz target would have run for a week and
/// reported nothing.
#[test]
#[ignore = "X-19 found this; fixing TransactionKey's client derivation is its own story"]
fn a_legacy_client_transaction_never_sees_its_response() {
    let program = Program::decode(&LEGACY_CLIENT_CRASH);
    // `run_strict`, not `run`: the campaign steps over this defect so it can reach what lies
    // behind it, and this is the test that keeps "stepped over" from becoming "forgotten".
    let result = sequence::run_strict(&program);
    assert!(
        result.violations.is_empty(),
        "a response to an RFC 2543 client transaction must reach it:\n{}\n\ntrace:\n{}",
        result
            .violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
        result.trace.join("\n")
    );
}

/// The minimised input still describes the sequence the defect needs.
///
/// This one is *not* ignored: it is what keeps the eight bytes above from decaying into
/// something else as the vocabulary changes. A regression test whose input has silently stopped
/// reproducing its defect is worse than no regression test, and an ignored test cannot say so.
#[test]
fn the_minimised_legacy_crash_still_decodes_to_the_sequence_it_names() {
    let program = Program::decode(&LEGACY_CLIENT_CRASH);
    assert_eq!(
        program.events,
        vec![
            Event::SendRequest {
                slot: 3,
                method: 0,
                reliable: false,
            },
            Event::ReceiveResponse {
                slot: 3,
                method: 1,
                status: 1,
                to_tag: 0,
            },
        ],
        "the minimised crash no longer decodes to \"send an INVITE on a legacy branch, then \
         answer it\""
    );
    assert!(
        program.events.iter().all(|event| match event {
            Event::SendRequest { slot, .. } | Event::ReceiveResponse { slot, .. } =>
                *slot >= sequence::FIRST_LEGACY_SLOT,
            _ => false,
        }),
        "the defect is in the RFC 2543 fallback; the input must use a legacy slot"
    );

    let trace = sequence::run(&program).trace.join("\n");
    assert!(
        trace.contains("created client c3/INVITE"),
        "the input must still create the client transaction:\n{trace}"
    );
    assert!(
        trace.contains("ReceiveResponse(3/ACK 180) | unmatched"),
        "the input must still show the response failing to match it:\n{trace}"
    );
}

/// Every suppression the campaign carries is still load-bearing, and the campaign is clean.
///
/// Two halves, and both matter. A suppression outlives its defect silently: once
/// `TransactionKey` derives client keys by §17.1.3, `Known::LegacyClientResponseMatching` hides
/// nothing and every future defect of that shape with it — so this fails the moment the fix
/// lands, which is exactly when somebody should be deleting the suppression and the `#[ignore]`
/// above. And `run` must be clean on the same input, or the fuzz job would report this one
/// defect forever and never reach anything behind it.
#[test]
fn the_known_defect_suppression_is_still_needed_and_still_works() {
    let program = Program::decode(&LEGACY_CLIENT_CRASH);

    assert!(
        sequence::run(&program).violations.is_empty(),
        "the campaign must step over the known defect, or it reports nothing else"
    );

    let strict = sequence::run_strict(&program);
    assert!(
        strict
            .violations
            .iter()
            .any(|v| v.invariant == Invariant::UnroutableResponse),
        "the suppression `{:?}` no longer hides anything. If §17.1.3 client matching has been \
         fixed, delete it from KNOWN_DEFECTS, un-ignore \
         `a_legacy_client_transaction_never_sees_its_response`, and delete this test",
        sequence::KNOWN_DEFECTS,
    );
}
