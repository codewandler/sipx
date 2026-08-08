//! A bound on a child process's answer that is derived from this machine, not tuned on an idle one.
//!
//! # The problem these tests actually have (`X-118`)
//!
//! Every wait in this file is a `timeout` around an awaited event, so none of them is a fixed sleep
//! and none asserts a performance figure. Their five seconds exists to stop a hung child hanging
//! the suite. But five seconds is a number somebody typed while watching an idle box, and on a busy
//! one it stops bounding a hang and starts reporting one: the process was going to answer, and the
//! machine had not scheduled it yet. `scripts/contention-proof.py` measured that rather than
//! assuming it — at two burners per core,
//! `interrupting_a_waiting_answerer_reports_after_listener_cleanup` failed while the suite's other
//! bounded assertions held.
//!
//! A red like that is worse than a slow one. It is indistinguishable from the defect the assertion
//! is for, so the next person reads a real failure as noise, and the time after that they are right
//! to.
//!
//! # What is derived, and from what
//!
//! The thing these waits are really waiting for is a `sipx` process being scheduled — forked,
//! linked, run to its first output. So that is what gets measured, once per test binary: start
//! `sipx version`, which does nothing but print a line, and see how long the machine takes over it.
//! On an unloaded box that is tens of milliseconds. Under contention it is the same work taking
//! longer, which is exactly the quantity every bound in here needs to be told about.
//!
//! [`bound`] multiplies a stated base by that ratio. The base still says what the wait is for and
//! stays readable at the call site; the multiplier says how much slower this machine is than the
//! one the base was written on.
//!
//! **This does not weaken an assertion.** What is asserted after the wait is unchanged, and the
//! only outcome the scaling converts is a timeout — from a failure into more waiting, on a machine
//! that has been shown to need it. A command that never answers still fails, because the scale is
//! clamped: [`CEILING`] caps it, so an infinite hang is still bounded and the suite still ends.
//!
//! # What it is not
//!
//! Not a sleep: nothing here suspends on a clock. The calibration runs a process and measures it,
//! and `check-fixed-sleep.py` sees a bound around an awaited event at every call site, which is the
//! shape the rule asks for.
//!
//! Not a retry: the operation under test runs once. `join_probe::until_bound` is the retry, and it
//! is for a different mechanism — a port collision, where the attempt is invalid rather than slow.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// What starting one `sipx` process costs on a machine with nothing else to do.
///
/// Measured on the development box at rest: roughly 25 ms for fork, dynamic linking and a single
/// `println!`. Rounded up, because the base durations at the call sites were written against a
/// machine at least this quick and a scale below 1 would tighten them.
const AT_REST: Duration = Duration::from_millis(40);

/// The most this may stretch a bound.
///
/// Twelve, so a five-second wait can become a minute. A machine slower than that is not one these
/// tests can say anything useful about, and the cap is what keeps a genuinely unbounded operation —
/// the one thing every bound here exists to catch — still reported as one. Without a ceiling this
/// would be an unbounded wait wearing a bound's clothes.
const CEILING: u32 = 12;

/// How many calibration runs. The **median** is taken, so one process losing a scheduling race does
/// not set the scale for the whole binary.
const SAMPLES: usize = 5;

static SCALE: OnceLock<u32> = OnceLock::new();

/// How much slower this machine is than the one the base durations were written on. At least 1.
pub(crate) fn scale() -> u32 {
    *SCALE.get_or_init(|| {
        let mut samples: Vec<Duration> = (0..SAMPLES).filter_map(|_| measure()).collect();
        if samples.is_empty() {
            // The calibration could not run. Assuming a fast machine would be the one assumption
            // that reintroduces the failure this exists to remove, so assume a slow one: the tests
            // are then slower to declare a hang and still declare it.
            return CEILING;
        }
        samples.sort_unstable();
        let median = samples[samples.len() / 2];
        let ratio =
            u32::try_from(median.as_micros() / AT_REST.as_micros().max(1)).unwrap_or(CEILING);
        ratio.clamp(1, CEILING)
    })
}

/// A stated base, stretched to this machine.
pub(crate) fn bound(base: Duration) -> Duration {
    base * scale()
}

/// One `sipx version`, wall clock. `None` if the process could not be run at all, which is a
/// different problem and not one to guess a number for.
fn measure() -> Option<Duration> {
    let started = Instant::now();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sipx"))
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    output.success().then(|| started.elapsed())
}

// Not `#[cfg(test)]`. This is an integration-test crate, where that cfg is *off* — gating these
// behind it would compile them away and leave two tests that look like coverage and never run,
// which is the defect `X-122` removed from the Python side of this repository.
mod tests {
    use super::*;

    #[test]
    fn the_scale_is_at_least_one_and_bounded() {
        let scale = scale();
        assert!(
            (1..=CEILING).contains(&scale),
            "scale {scale} outside 1..={CEILING}"
        );
    }

    #[test]
    fn a_bound_never_shrinks_its_base() {
        let base = Duration::from_secs(5);
        assert!(bound(base) >= base);
        assert!(bound(base) <= base * CEILING);
    }
}
