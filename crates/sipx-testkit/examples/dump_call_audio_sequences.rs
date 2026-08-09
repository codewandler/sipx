//! Print every adversarial call-audio program's trace and any invariant it violates.
//!
//! ```sh
//! cargo run -p sipx-testkit --example dump_call_audio_sequences            # read the traces
//! cargo run -p sipx-testkit --example dump_call_audio_sequences -- --write # regenerate the corpus
//! ```
//!
//! The readable form of what the `call_audio_sequence` fuzz target sees, and the quickest way to
//! tell whether a change to the analyser changed behaviour the seeds cover. `--write` regenerates
//! the committed corpus from `seeds()`; the test `the_committed_corpus_is_exactly_the_seed_programs`
//! in `crates/sipx-audio/tests/` is what makes forgetting to run it a failure rather than a
//! surprise.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use sipx_testkit::call_audio_sequence::{corpus_dir, run, seeds, write_corpus};

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
        let result = run(&seed.program);
        println!(
            "=== {} ({} bytes, {} accepted, {} refused, {} observations)",
            seed.name,
            seed.program.encode().len(),
            result.accepted,
            result.refused,
            result.observations
        );
        for line in &result.trace {
            println!("    {line}");
        }
        for violation in &result.violations {
            println!("    !! {violation}");
            violations += 1;
        }
    }
    if violations > 0 {
        eprintln!("{violations} invariant violation(s) in the seed corpus");
    }
    Ok(())
}
