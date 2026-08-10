//! The adversarial worker-protocol corpus, replayed without a fuzzer (`M-122`).
//!
//! `fuzz/fuzz_targets/worker_protocol_sequence.rs` needs a nightly toolchain and a `cargo-fuzz`
//! install, which is why the campaign is a CI job and not a gate step. This file is the half of it
//! every local run gets: the committed seeds are exactly what `seeds()` produces, every one of them
//! decodes and drives both real callers, none of them breaks an invariant, and a bounded
//! deterministic sweep over generated inputs finds nothing either.
//!
//! The sweep is not a substitute for the campaign and does not pretend to be one — it has no
//! coverage feedback and no minimiser. What it is, is the thing that fails when somebody changes
//! `docs/specs/call-dsp-graph.md` §7.4's decoder and does not push: the same oracle, on the same
//! driver, over inputs nobody chose.
//!
//! Nothing here reads a clock, opens a pipe or spawns a process. The generator is a fixed-seed
//! linear congruential sequence, so the run is the same run on every machine and a failure is
//! reproducible from the iteration printed with it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;

use sipx_media::dsp::worker_protocol_sequence::{
    CEILINGS, Program, Side, Step, corpus_dir, expected_refusal, run, seeds,
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
        "regenerate with: cargo run -p sipx-media --example dump_worker_protocol_sequences -- \
         --write"
    );
}

/// Every committed file is a program the driver can run, read back off disk rather than from
/// `seeds()` — which is what the fuzzer will do with it.
#[test]
fn every_committed_corpus_file_decodes_to_a_program_that_runs() {
    for (name, bytes) in committed() {
        let program = Program::decode(&bytes);
        assert!(!program.steps.is_empty(), "{name} decodes to no steps");
        let outcome = run(&program);
        assert!(
            outcome.violations.is_empty(),
            "{name}: {:?}\ntrace:\n{}",
            outcome.violations,
            outcome.trace.join("\n")
        );
    }
}

/// The corpus is a campaign's input set, so every row of §7.4's refusal table has a seed reaching
/// it, and both directions have one.
#[test]
fn the_corpus_covers_every_row_of_the_refusal_table() {
    let mut reached: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut sides = (false, false);
    for seed in seeds() {
        match seed.program.side {
            Side::Runtime => sides.0 = true,
            Side::Worker => sides.1 = true,
        }
        for line in run(&seed.program).trace {
            if let Some(refusal) = line.split("refused ").nth(1) {
                reached
                    .entry(refusal.to_owned())
                    .or_default()
                    .push(seed.name);
            }
        }
    }
    for row in [
        "BadMagic",
        "Reserved",
        "UnknownType",
        "Undersized",
        "Oversized",
        "LengthMismatch",
        "Truncated",
        "Value",
        "Version",
    ] {
        // `Version` is the one row a seed cannot reach: the encoder always writes this runtime's
        // own version, and a program that could rewrite it would be a program about one octet. The
        // fuzzer reaches it by mutation, and `wire`'s own unit test asserts the refusal.
        if row == "Version" {
            continue;
        }
        assert!(
            reached.contains_key(row),
            "no seed reaches {row}: {:?}",
            reached.keys().collect::<Vec<_>>()
        );
    }
    assert!(sides.0 && sides.1, "both directions need seeds: {sides:?}");
}

/// A refusal is terminal, so a seed with two of them only ever exercises the first.
///
/// The corpus would then claim a coverage it does not have, which is exactly what a *generated*
/// corpus exists to rule out.
#[test]
fn no_seed_hides_a_refusal_behind_another() {
    for seed in seeds() {
        let outcome = run(&seed.program);
        assert!(
            outcome.refused <= 1,
            "{} refuses {} messages; only the first can ever fire",
            seed.name,
            outcome.refused
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

/// The decoder is total and every step kind is reachable, so no byte the fuzzer produces is wasted
/// and no arm of the driver is unreachable from an input.
#[test]
fn the_decoder_is_total_and_every_step_kind_is_reachable() {
    let mut seen = [false; 12];
    for op in 0..=u8::MAX {
        let program = Program::decode(&[0, 2, op, 3, 5, 6]);
        assert_eq!(program.steps.len(), 1, "op {op} decoded to nothing");
        let index = match program.steps[0] {
            Step::Hello { .. } => 0,
            Step::Frame { .. } => 1,
            Step::Result { .. } => 2,
            Step::BreakMagic { .. } => 3,
            Step::SetReserved { .. } => 4,
            Step::SetType { .. } => 5,
            Step::SetLength { .. } => 6,
            Step::SetCount { .. } => 7,
            Step::Truncate { .. } => 8,
            Step::Noise { .. } => 9,
            Step::Repeat => 10,
            Step::Chunk { .. } => 11,
        };
        seen[index] = true;
    }
    assert!(seen.iter().all(|reached| *reached), "{seen:?}");

    // A trailing partial record is ignored rather than turning into noise, which is what lets
    // libFuzzer shrink an input a byte at a time.
    assert_eq!(Program::decode(&[0, 2, 1, 2, 3]).steps.len(), 0);
    assert_eq!(Program::decode(&[0, 2, 1, 2, 3, 4, 5]).steps.len(), 1);
    assert_eq!(Program::decode(&[]), Program::default());
}

/// Both sides fold from a byte and back, so a minimiser can flip the reading end of a program
/// without disturbing the rest of it.
#[test]
fn every_side_round_trips_through_its_byte() {
    for byte in 0..=u8::MAX {
        let side = Side::from_byte(byte);
        assert_eq!(Side::from_byte(side.to_byte()), side);
    }
}

/// The oracle's second opinion is a second opinion: it refuses what §7.4's table refuses, in the
/// order the table is written, from the header octets and nothing else.
#[test]
fn the_tables_own_order_is_what_the_oracle_checks() {
    let header = |kind: u8, reserved: u8, declared: u32| {
        let mut out = vec![0x53, 0x44, kind, reserved];
        out.extend_from_slice(&declared.to_be_bytes());
        out
    };
    // Magic first, and it outranks a set reserved octet and an unknown type both.
    assert_eq!(
        expected_refusal(&[0, 0, 0xFF, 0xFF, 0, 0, 0, 0], 160),
        Err("BadMagic")
    );
    // Reserved outranks the type, which outranks the length.
    assert_eq!(
        expected_refusal(&header(0x42, 1, 0), 160),
        Err("Reserved"),
        "a set reserved octet is decided before the type it sits beside"
    );
    assert_eq!(
        expected_refusal(&header(0x42, 0, 0), 160),
        Err("UnknownType")
    );
    assert_eq!(
        expected_refusal(&header(0x81, 0, 12), 160),
        Err("Undersized")
    );
    assert_eq!(expected_refusal(&header(0x81, 0, 13), 160), Ok(()));
    assert_eq!(expected_refusal(&header(0x81, 0, 333), 160), Ok(()));
    assert_eq!(
        expected_refusal(&header(0x81, 0, 334), 160),
        Err("Oversized")
    );
    // A `Hello` has no trailing samples, so its ceiling is its fixed payload whatever was
    // negotiated.
    assert_eq!(
        expected_refusal(&header(0x01, 0, 13), u32::MAX),
        Err("Oversized")
    );
    // And the ceiling itself is bounded, so `u32::MAX` buys a peer nothing.
    assert_eq!(
        expected_refusal(&header(0x81, 0, u32::MAX), u32::MAX),
        Err("Oversized"),
        "a negotiated ceiling above the contract's own cannot admit a length"
    );
}

/// The ceiling table names both sides of the contract's bound, so a program can ask for one past it.
#[test]
fn the_ceiling_table_straddles_the_contracts_bound() {
    assert!(CEILINGS.contains(&65_536), "{CEILINGS:?}");
    assert!(CEILINGS.contains(&65_537), "{CEILINGS:?}");
    assert!(CEILINGS.contains(&u32::MAX), "{CEILINGS:?}");
}

/// A bounded deterministic sweep over generated programs: the same oracle, over inputs nobody
/// chose.
///
/// Not the campaign — no coverage feedback, no minimiser — but the part of it a local run gets. The
/// generator is a fixed-seed linear congruential sequence so a failure names the iteration that
/// produced it and that iteration reproduces.
#[test]
fn a_bounded_sweep_of_generated_programs_breaks_no_invariant() {
    const ITERATIONS: u32 = 3_000;
    let mut state: u32 = 0x0FED_CBA9;
    let mut next = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        state
    };
    let mut total_steps = 0usize;
    for iteration in 0..ITERATIONS {
        // 1..=24 steps: long enough for a `Hello` to negotiate a ceiling and several messages to be
        // read under it, short enough that three thousand of them stay a test rather than a
        // campaign.
        let steps = (next() >> 20) % 24 + 1;
        let mut bytes = Vec::with_capacity(2 + steps as usize * 4);
        for _ in 0..steps * 4 + 2 {
            bytes.push(u8::try_from((next() >> 16) & 0xFF).unwrap_or(0));
        }
        let program = Program::decode(&bytes);
        total_steps += program.steps.len();
        let outcome = run(&program);
        assert!(
            outcome.violations.is_empty(),
            "iteration {iteration} ({} steps, side {:?}): {:?}\ntrace:\n{}",
            program.steps.len(),
            program.side,
            outcome.violations,
            outcome.trace.join("\n")
        );
    }
    assert!(
        total_steps > 30_000,
        "a sweep that drove {total_steps} steps proves little"
    );
}
