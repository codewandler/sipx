//! Fuzz the call-audio analyser, not the parser.
//!
//! The parser targets test adversarial *bytes* and `transaction_sequence` tests adversarial
//! *timing*. This one tests adversarial *audio* and the lifecycle around it (`M-61`): the bytes
//! decode into a program of frames in every supported PCM format, declared format changes,
//! requested resets, sequence gaps and replays, and a consumer that stops draining — so the budget
//! is spent inside `docs/specs/call-audio-processing.md` §5's integer arithmetic and §12's bounded
//! calibration rather than on samples that measure nothing.
//!
//! The oracle is not "did it panic". The analyser is total: almost any sequence "succeeds", and the
//! failures that matter are silent — a queue that grew past its capacity, a refusal that moved the
//! stream position, a threshold outside the interval it declares, a window whose reported facts
//! disagree with its own accumulators, a position naming audio nobody fed, voice left latched
//! across a reset, a report claiming coverage it does not have. Those are asserted explicitly by
//! the harness, which returns them as data; this target is the part that turns a finding into a
//! crash libFuzzer can minimise.
//!
//! It also runs in a build where integer overflow panics, so §5.2's width proof is under test
//! rather than assumed.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sipx_testkit::call_audio_sequence::{Program, run};

fuzz_target!(|data: &[u8]| {
    let program = Program::decode(data);
    if program.events.is_empty() {
        return;
    }
    let result = run(&program);
    if !result.violations.is_empty() {
        // The trace goes with the report: the minimised input is four bytes an event and says
        // nothing on its own, and the first thing anyone will want is the sequence that got here.
        let trace = result.trace.join("\n");
        let violations = result
            .violations
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        panic!("call-audio analyser invariant violated:\n{violations}\n\ntrace:\n{trace}");
    }
});
