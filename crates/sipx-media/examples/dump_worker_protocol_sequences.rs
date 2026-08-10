//! Print every adversarial worker-protocol program's trace and any invariant it violates.
//!
//! ```sh
//! cargo run -p sipx-media --example dump_worker_protocol_sequences            # read the traces
//! cargo run -p sipx-media --example dump_worker_protocol_sequences -- --write # regenerate
//! ```
//!
//! The readable form of what the `worker_protocol_sequence` fuzz target sees, and the quickest way
//! to tell whether a change to `docs/specs/call-dsp-graph.md` §7.4's decoder changed behaviour the
//! seeds cover. `--write` regenerates the committed corpus from `seeds()`; the test
//! `the_committed_corpus_is_exactly_the_seed_programs` in `crates/sipx-media/tests/` is what makes
//! forgetting to run it a failure rather than a surprise.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use sipx_media::dsp::worker_protocol_sequence::{corpus_dir, run, seeds, write_corpus};

fn main() -> std::io::Result<()> {
    if std::env::args().any(|arg| arg == "--write") {
        let written = write_corpus()?;
        println!(
            "wrote {written} seed programs to {}",
            corpus_dir().display()
        );
        return Ok(());
    }

    let mut violations = 0;
    for seed in seeds() {
        let outcome = run(&seed.program);
        println!(
            "=== {} ({} bytes, {:?}, {} accepted, {} refused)",
            seed.name,
            seed.program.encode().len(),
            seed.program.side,
            outcome.accepted,
            outcome.refused
        );
        for line in &outcome.trace {
            println!("    {line}");
        }
        for violation in &outcome.violations {
            println!("    !! {violation}");
            violations += 1;
        }
    }
    if violations > 0 {
        eprintln!("{violations} invariant violation(s) in the seed corpus");
    }
    Ok(())
}
