//! Fuzz the supervised worker's framed protocol, in both directions (`M-122`).
//!
//! The parser targets test adversarial bytes on the *signalling* path; `transaction_sequence` tests
//! adversarial timing and `call_audio_sequence` adversarial audio. This one tests the one place in
//! the media process where a *worker* — an application's own program, in a process of its own — is
//! the peer whose octets are parsed: `docs/specs/call-dsp-graph.md` §7.4's framing, read by the
//! runtime off a worker's standard output and by a worker off the runtime's writes.
//!
//! The oracle is not "did it panic". The decoder is already total, so a campaign checking only for
//! panics would stay green over one that had quietly stopped enforcing anything. What is asserted is
//! §7.4's own four sentences — a message is refused by its type and never by its length prefix,
//! `Oversized` is decided from the header alone, no declared length ever sizes a buffer, and a
//! refusal is terminal for the connection in both directions — with the last one checked against the
//! real callers, `serve` and `pump`, rather than against a model of them. The harness returns them as
//! data; this target is the part that turns a finding into a crash libFuzzer can minimise.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sipx_testkit::worker_protocol_sequence::{Program, run};

fuzz_target!(|data: &[u8]| {
    let program = Program::decode(data);
    if program.steps.is_empty() {
        return;
    }
    let outcome = run(&program);
    if !outcome.violations.is_empty() {
        // The trace goes with the report: the minimised input is four bytes a step and says nothing
        // on its own, and the first thing anyone will want is the exchange that got here.
        let trace = outcome.trace.join("\n");
        let violations = outcome
            .violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        panic!("worker protocol invariant violated:\n{violations}\n\ntrace:\n{trace}");
    }
});
