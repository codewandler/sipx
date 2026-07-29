//! Fuzz the transaction driver, not the parser.
//!
//! The other four targets stop at the parser: they test the half of the north star about
//! adversarial *input*. This one tests the other half — adversarial *timing*. The bytes decode
//! into a program of events over a small vocabulary (incoming messages, application requests,
//! fired timers), each of which becomes a well-formed message or a call on `TransactionLayer`,
//! so the budget is spent inside the RFC 3261 §17 state machines rather than on messages that
//! do not parse.
//!
//! The oracle is not "did it panic". A transaction machine is total; almost any sequence
//! "succeeds", and the failures that matter are silent — a transaction that reports its own
//! death and stays in the store, a timer that resurrects a retired machine, a store that grows,
//! a state no §17 table names. Those are asserted explicitly by the harness, which returns them
//! as data; this target is the part that turns a finding into a crash libFuzzer can minimise.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sipx_testkit::transaction_sequence::{Program, run};

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
        panic!("transaction-layer invariant violated:\n{violations}\n\ntrace:\n{trace}");
    }
});
