//! The adversarial call-audio corpus, replayed without a fuzzer (`M-61`).
//!
//! `fuzz/fuzz_targets/call_audio_sequence.rs` needs a nightly toolchain and a `cargo-fuzz` install,
//! which is why the campaign is a CI job and not a gate step. This file is the half of it that
//! every local run gets: the committed seeds are exactly what `seeds()` produces, every one of them
//! decodes and drives the analyser, none of them breaks an invariant, and a bounded deterministic
//! sweep over generated inputs finds nothing either.
//!
//! The sweep is not a substitute for the campaign and does not pretend to be one — it has no
//! coverage feedback and no minimiser. What it is, is the thing that fails when somebody changes
//! the analyser and does not push: the same oracle, on the same driver, over inputs nobody chose.
//!
//! Nothing here reads a clock. The generator is a fixed-seed linear congruential sequence, so the
//! run is the same run on every machine and a failure is reproducible from the seed printed with
//! it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use sipx_testkit::call_audio_sequence::{
    Event, Pattern, Program, corpus_dir, reference_profile, run, seeds, subject_profile,
};

/// Every seed's bytes, by name, as `seeds()` produces them.
fn expected() -> BTreeMap<String, Vec<u8>> {
    seeds()
        .into_iter()
        .map(|seed| (seed.name.to_owned(), seed.program.encode()))
        .collect()
}

/// Every file in the committed corpus directory, by name.
fn committed() -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(corpus_dir()).expect("the corpus directory exists") {
        let entry = entry.expect("a readable directory entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "README.md" {
            continue;
        }
        found.insert(name, std::fs::read(entry.path()).expect("a readable seed"));
    }
    found
}

/// The committed corpus is generated, so forgetting to regenerate it is a failure and not a
/// surprise.
#[test]
fn the_committed_corpus_is_exactly_the_seed_programs() {
    assert_eq!(
        committed(),
        expected(),
        "regenerate with: cargo run -p sipx-testkit --example dump_call_audio_sequences -- --write"
    );
}

/// Every committed file is a program the driver can run, read back off disk rather than from
/// `seeds()` — which is what the fuzzer will do with it.
#[test]
fn every_committed_corpus_file_decodes_to_a_program_that_runs() {
    for (name, bytes) in committed() {
        let program = Program::decode(&bytes);
        assert!(!program.events.is_empty(), "{name} decodes to no events");
        let result = run(&program);
        assert!(
            result.violations.is_empty(),
            "{name}: {:?}\ntrace:\n{}",
            result.violations,
            result.trace.join("\n")
        );
    }
}

/// The corpus is a campaign's input set, so every shape the story names has a seed carrying it.
#[test]
fn the_corpus_covers_every_shape_the_story_names() {
    let names: Vec<&str> = seeds().iter().map(|seed| seed.name).collect();
    for required in [
        "extreme-amplitudes",
        "impulse-storm",
        "dc-bias",
        "alternating-full-swing",
        "arbitrary-chunking",
        "sequence-gaps",
        "format-churn",
        "long-silence",
        "permanent-activity",
        "event-backpressure",
        "every-source-format",
    ] {
        assert!(
            names.contains(&required),
            "{required} has no seed: {names:?}"
        );
    }
}

/// Encoding and decoding are inverses, which is what lets a minimised crash be replayed as a
/// program and a program be written as a seed.
#[test]
fn every_program_round_trips_through_its_bytes() {
    for seed in seeds() {
        assert_eq!(
            Program::decode(&seed.program.encode()),
            seed.program,
            "{} does not round-trip",
            seed.name
        );
    }
}

/// The decoder is total and every event kind is reachable, so no byte the fuzzer produces is wasted
/// and no arm of the driver is unreachable from an input.
#[test]
fn the_decoder_is_total_and_every_event_kind_is_reachable() {
    let mut seen = [false; 8];
    for op in 0..=u8::MAX {
        let program = Program::decode(&[op, 3, 5, 6]);
        assert_eq!(program.events.len(), 1, "op {op} decoded to nothing");
        let index = match program.events[0] {
            Event::Frame { .. } => 0,
            Event::Broken { .. } => 1,
            Event::Gap { .. } => 2,
            Event::Replay { .. } => 3,
            Event::DeclareFormat { .. } => 4,
            Event::SourceFormat { .. } => 5,
            Event::Reset => 6,
            Event::Drain => 7,
        };
        seen[index] = true;
    }
    assert!(seen.iter().all(|reached| *reached), "{seen:?}");

    // A trailing partial record is ignored rather than turning into noise, which is what lets
    // libFuzzer shrink an input a byte at a time.
    assert_eq!(Program::decode(&[1, 2, 3]).events.len(), 0);
    assert_eq!(Program::decode(&[1, 2, 3, 4, 5]).events.len(), 1);
}

/// Every pattern folds from a byte and back, so a fuzzer byte and a written seed name the same
/// audio.
#[test]
fn every_pattern_round_trips_through_its_byte() {
    for byte in 0..=u8::MAX {
        let pattern = Pattern::from_byte(byte);
        assert_eq!(Pattern::from_byte(pattern.to_byte()), pattern);
    }
}

/// The two lanes are the ones §12.11's first clause is a claim about: the same measurement, one
/// adapting inside `[floor, ceiling]` and one fixed at that floor.
#[test]
fn the_reference_lane_is_the_floor_the_subject_declares() {
    let subject = subject_profile();
    let reference = reference_profile();
    let calibration = subject.calibration().expect("the subject calibrates");
    assert_eq!(
        reference.activation_amplitude(),
        calibration.floor_amplitude(),
        "the reference is fixed at the subject's own floor, not at a second guess"
    );
    assert!(
        reference.calibration().is_none(),
        "a reference that adapted would answer a different question"
    );
    assert!(
        reference.queue_capacity() > subject.queue_capacity(),
        "the reference must not lose an observation the subject is measured against"
    );
}

/// A bounded deterministic sweep over generated programs: the same oracle, over inputs nobody
/// chose.
///
/// Not the campaign — no coverage feedback, no minimiser — but the part of it a local run gets. The
/// generator is a fixed-seed linear congruential sequence so a failure names the iteration that
/// produced it and that iteration reproduces.
#[test]
fn a_bounded_sweep_of_generated_programs_breaks_no_invariant() {
    const ITERATIONS: u32 = 2_000;
    let mut state: u32 = 0x1357_9BDF;
    let mut total_events = 0usize;
    for iteration in 0..ITERATIONS {
        let mut bytes = Vec::new();
        // 1..=48 events: long enough for an update period to end and a threshold to move, short
        // enough that two thousand of them stay a test rather than a campaign. The campaign is the
        // CI job; it has a nightly toolchain, coverage feedback and a minute, and this has none of
        // the three.
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let events = (state >> 20) % 48 + 1;
        for _ in 0..events * 4 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            bytes.push(u8::try_from((state >> 16) & 0xFF).unwrap_or(0));
        }
        let program = Program::decode(&bytes);
        total_events += program.events.len();
        let result = run(&program);
        assert!(
            result.violations.is_empty(),
            "iteration {iteration} ({} events): {:?}\ntrace:\n{}",
            program.events.len(),
            result.violations,
            result.trace.join("\n")
        );
    }
    assert!(
        total_events > 40_000,
        "a sweep that drove {total_events} events proves little"
    );
}
