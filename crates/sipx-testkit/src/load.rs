//! Placing many calls at once, and reporting honestly about what happened.
//!
//! Generic over what "a call" means — it is a closure returning a future — so the same harness
//! drives sipx against itself and sipx against a third-party server. That is not generality for
//! its own sake: a limit found with sipx on both ends cannot be attributed to either half, and
//! the whole point of a load test is to find out which side gives out first.
//!
//! Two rules about the reporting, both of which exist because breaking them makes the numbers
//! worse than useless:
//!
//! **Failures are counted by cause, never aggregated.** A run that goes from 99% to 97% success
//! looks like mild degradation and may be a new failure appearing while an old one recedes.
//! Which failure is growing is the entire question.
//!
//! **Latency is reported as percentiles, never as a mean.** Call setup latency is not normally
//! distributed — it is a tight cluster with a tail of retransmission timeouts — and a mean sits
//! in the empty space between the two, describing a call that never happened.

// Counts here are call counts and percentile ranks: a run large enough to lose `f64` precision
// would need more calls than there are microseconds in a century.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::collections::BTreeMap;
use std::time::Duration;

/// Why a call did not succeed.
///
/// Deliberately coarse. Finer categories would be guesses: the harness sees a failure and a
/// duration, and inventing a taxonomy it cannot actually distinguish would produce a report
/// that looks precise and is not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cause {
    /// The far end refused, with the status it gave.
    Rejected(u16),
    /// Nothing came back in time.
    Timeout,
    /// The transport failed — refused connection, closed socket, unreachable host.
    Transport,
    /// Something else, described.
    Other(String),
}

impl std::fmt::Display for Cause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(status) => write!(f, "rejected {status}"),
            Self::Timeout => f.write_str("timeout"),
            Self::Transport => f.write_str("transport"),
            Self::Other(what) => write!(f, "{what}"),
        }
    }
}

/// How much load to apply.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    /// How many calls to place in total.
    pub calls: usize,
    /// How many to start per second.
    ///
    /// The arrival rate, not the concurrency: a rate of 50 with calls lasting two seconds
    /// settles at about a hundred in progress. Driving by rate rather than by concurrency is
    /// what makes a run reproducible — a harness that keeps N in flight speeds up when the
    /// system under test slows down, which is the opposite of a load test.
    pub rate: f64,
    /// The most to have in progress at once, whatever the rate says.
    ///
    /// A backstop, not a target. Without it, a system that has stopped answering entirely
    /// accumulates every call the plan asks for and the harness runs out of sockets before the
    /// thing it is testing does.
    pub most_in_flight: usize,
}

impl Plan {
    /// A plan placing this many calls at this rate.
    #[must_use]
    pub fn new(calls: usize, rate: f64) -> Self {
        Self {
            calls,
            rate,
            most_in_flight: 512,
        }
    }

    /// The gap between two calls starting.
    ///
    /// `rate` is a public field, so it can be anything a caller can write — and
    /// `Duration::from_secs_f64` *panics* on NaN or on a value too large to represent. A
    /// denormal rate such as `1e-300` reaches the second case. A load harness that panics on
    /// its own configuration is not a load harness, so both collapse to "as fast as possible",
    /// which is what a nonsensical rate most nearly means.
    #[must_use]
    pub fn interval(&self) -> Duration {
        if !self.rate.is_finite() || self.rate <= 0.0 {
            return Duration::ZERO;
        }
        Duration::try_from_secs_f64(1.0 / self.rate).unwrap_or(Duration::ZERO)
    }
}

/// What a run produced.
#[derive(Debug, Default, Clone)]
pub struct Outcome {
    /// How many were attempted.
    pub attempted: usize,
    /// How many succeeded.
    pub succeeded: usize,
    /// Failures, by cause. Never summed into a single number.
    pub failures: BTreeMap<Cause, usize>,
    /// How long each successful call took to set up.
    pub setup: Vec<Duration>,
    /// How long the whole run took.
    pub elapsed: Duration,
}

impl Outcome {
    /// Calls completed per second over the run.
    #[must_use]
    pub fn calls_per_second(&self) -> f64 {
        let seconds = self.elapsed.as_secs_f64();
        if seconds <= 0.0 {
            return 0.0;
        }
        // Successes, not attempts: a harness that counted attempts would report its highest
        // throughput at the moment the system under test stopped working.
        self.succeeded as f64 / seconds
    }

    /// The setup latency at this percentile, from 0.0 to 1.0.
    ///
    /// Nearest-rank, which is the definition that always names a measurement that actually
    /// happened rather than interpolating between two that did.
    #[must_use]
    pub fn percentile(&self, fraction: f64) -> Option<Duration> {
        if self.setup.is_empty() || !fraction.is_finite() {
            // A NaN fraction would `clamp` to NaN, cast to 0, and silently return the *fastest*
            // call as though it were the answer to whatever was asked.
            return None;
        }
        let mut sorted = self.setup.clone();
        sorted.sort_unstable();
        let rank = (fraction.clamp(0.0, 1.0) * sorted.len() as f64).ceil() as usize;
        sorted
            .get(rank.saturating_sub(1).min(sorted.len() - 1))
            .copied()
    }

    /// How many failed, all causes.
    #[must_use]
    pub fn failed(&self) -> usize {
        self.failures.values().sum()
    }

    /// A report a person can read.
    #[must_use]
    pub fn report(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} attempted, {} succeeded, {} failed in {:.1}s ({:.1} calls/s)",
            self.attempted,
            self.succeeded,
            self.failed(),
            self.elapsed.as_secs_f64(),
            self.calls_per_second()
        );
        for (label, fraction) in [("p50", 0.50), ("p95", 0.95), ("p99", 0.99)] {
            if let Some(at) = self.percentile(fraction) {
                let _ = writeln!(out, "  setup {label}: {:.0} ms", at.as_secs_f64() * 1000.0);
            }
        }
        // Every cause on its own line. Which one is growing is the whole question.
        for (cause, count) in &self.failures {
            let _ = writeln!(out, "  {count} × {cause}");
        }
        out
    }
}

/// Run a plan, placing calls with `place`.
///
/// `place` is given the call's index and returns whether it worked. Everything about what a
/// call *is* lives there, which is what lets the same harness point at sipx or at somebody
/// else's server.
pub async fn run<F, Fut>(plan: Plan, place: F) -> Outcome
where
    F: Fn(usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), Cause>> + Send + 'static,
{
    let place = std::sync::Arc::new(place);
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(plan.most_in_flight.max(1)));

    let started = tokio::time::Instant::now();
    let interval = plan.interval();
    let mut running = Vec::with_capacity(plan.calls);

    for index in 0..plan.calls {
        // Paced by the clock rather than by completion, so the load applied does not depend on
        // how fast the system under test is answering.
        if !interval.is_zero() {
            tokio::time::sleep_until(started + interval * u32::try_from(index).unwrap_or(0)).await;
        }
        let Ok(permit) = std::sync::Arc::clone(&permits).acquire_owned().await else {
            break;
        };

        let place = std::sync::Arc::clone(&place);
        running.push(tokio::spawn(async move {
            let at = tokio::time::Instant::now();
            let outcome = place(index).await;
            let took = at.elapsed();
            drop(permit);
            (outcome, took)
        }));
    }

    let mut outcome = Outcome {
        attempted: running.len(),
        ..Outcome::default()
    };
    // Joined rather than collected from a channel, because a channel only hears from calls that
    // *finished*. A `place` future that panics unwinds its task and sends nothing, so its call
    // would appear in neither `succeeded` nor `failures` — a run with fifty panics reporting
    // "300 attempted, 250 succeeded, 0 failed" and an empty cause map. The one thing this
    // module promises is that every failure has a cause.
    for handle in running {
        match handle.await {
            Ok((Ok(()), took)) => {
                outcome.succeeded += 1;
                outcome.setup.push(took);
            }
            Ok((Err(cause), _)) => *outcome.failures.entry(cause).or_default() += 1,
            Err(joined) => {
                let what = if joined.is_panic() {
                    "panicked"
                } else {
                    "cancelled"
                };
                *outcome
                    .failures
                    .entry(Cause::Other(what.to_owned()))
                    .or_default() += 1;
            }
        }
    }
    outcome.elapsed = started.elapsed();
    outcome
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// X-4's exit criterion, and the reason it is about the harness rather than about sipx: a
    /// load harness that miscounts is worse than no load harness, because the numbers look
    /// like measurements.
    #[tokio::test]
    async fn the_harness_reports_a_failure_it_was_given() {
        let outcome = run(Plan::new(10, 1000.0), |index| async move {
            if index % 2 == 0 {
                Err(Cause::Rejected(486))
            } else {
                Ok(())
            }
        })
        .await;

        assert_eq!(outcome.attempted, 10);
        assert_eq!(outcome.succeeded, 5);
        assert_eq!(outcome.failed(), 5);
        assert_eq!(outcome.failures.get(&Cause::Rejected(486)), Some(&5));
    }

    /// Causes are kept apart. A run whose success rate slips two points may be a new failure
    /// appearing while an old one recedes, and an aggregate hides exactly that.
    #[tokio::test]
    async fn failures_are_counted_by_cause() {
        let outcome = run(Plan::new(9, 1000.0), |index| async move {
            match index % 3 {
                0 => Err(Cause::Timeout),
                1 => Err(Cause::Rejected(503)),
                _ => Err(Cause::Transport),
            }
        })
        .await;

        assert_eq!(outcome.succeeded, 0);
        assert_eq!(outcome.failures.get(&Cause::Timeout), Some(&3));
        assert_eq!(outcome.failures.get(&Cause::Rejected(503)), Some(&3));
        assert_eq!(outcome.failures.get(&Cause::Transport), Some(&3));
        assert_eq!(outcome.failures.len(), 3, "three causes, not one number");
    }

    /// The tail is the point. A mean of these would sit in the empty space between the cluster
    /// and the tail, describing a call that never happened.
    #[tokio::test]
    async fn percentiles_describe_the_tail_rather_than_the_average() {
        let mut outcome = Outcome {
            attempted: 100,
            succeeded: 100,
            elapsed: Duration::from_secs(1),
            ..Outcome::default()
        };
        // Ninety fast calls and ten very slow ones — the shape call setup actually has.
        outcome.setup = (0..90)
            .map(|_| Duration::from_millis(20))
            .chain((0..10).map(|_| Duration::from_secs(2)))
            .collect();

        assert_eq!(outcome.percentile(0.50), Some(Duration::from_millis(20)));
        assert_eq!(
            outcome.percentile(0.95),
            Some(Duration::from_secs(2)),
            "the tail must be visible at p95"
        );

        // What a mean would have said, for contrast: 218 ms, which describes none of them.
        let mean: Duration = outcome.setup.iter().sum::<Duration>() / 100;
        assert!(mean > Duration::from_millis(200) && mean < Duration::from_millis(230));
        assert_ne!(
            Some(mean),
            outcome.percentile(0.50),
            "a mean here is not the typical call"
        );
    }

    #[tokio::test]
    async fn percentiles_of_nothing_are_nothing() {
        let outcome = Outcome::default();
        assert_eq!(outcome.percentile(0.5), None, "not zero, which is a claim");
    }

    /// Throughput counts successes. Counting attempts would report the highest number at the
    /// moment the system under test stopped working.
    #[test]
    fn throughput_counts_calls_that_worked() {
        let outcome = Outcome {
            attempted: 100,
            succeeded: 40,
            elapsed: Duration::from_secs(2),
            ..Outcome::default()
        };
        assert!((outcome.calls_per_second() - 20.0).abs() < 0.001);
    }

    /// The rate is an arrival rate. A harness that instead kept N in flight would speed up as
    /// the system under test slowed down, which is the opposite of applying load.
    #[tokio::test(start_paused = true)]
    async fn calls_are_paced_by_the_clock_not_by_completion() {
        let started = tokio::time::Instant::now();
        let outcome = run(Plan::new(10, 10.0), |_| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok(())
        })
        .await;

        assert_eq!(outcome.succeeded, 10);
        // Ten calls at ten per second is about a second of launching, plus the five seconds the
        // last one takes. A completion-driven harness would have taken fifty.
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the harness waited for each call before starting the next: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn the_report_names_every_cause() {
        let outcome = run(Plan::new(4, 1000.0), |index| async move {
            if index == 0 {
                Err(Cause::Timeout)
            } else {
                Err(Cause::Other("no route".to_owned()))
            }
        })
        .await;

        let report = outcome.report();
        assert!(report.contains("timeout"), "{report}");
        assert!(report.contains("no route"), "{report}");
        assert!(report.contains("4 attempted"), "{report}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod robustness {
    use super::*;

    /// A call whose future panics must still be accounted for. Losing it makes the harness
    /// under-report silently, and the one thing this module promises is that every failure has
    /// a cause.
    #[tokio::test]
    async fn a_panicking_call_is_reported_rather_than_lost() {
        let outcome = run(Plan::new(6, 1000.0), |index| async move {
            assert!(index % 2 != 0, "deliberate");
            Ok(())
        })
        .await;

        assert_eq!(outcome.attempted, 6);
        assert_eq!(outcome.succeeded, 3);
        assert_eq!(
            outcome.succeeded + outcome.failed(),
            outcome.attempted,
            "every attempt must land somewhere: {outcome:?}"
        );
        assert_eq!(
            outcome.failures.get(&Cause::Other("panicked".to_owned())),
            Some(&3)
        );
    }

    /// `rate` is a public field, so it can be anything a caller can write — and
    /// `Duration::from_secs_f64` panics on NaN. A load harness that panics on its own
    /// configuration is not a load harness.
    #[test]
    fn a_nonsensical_rate_does_not_panic() {
        for rate in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            0.0,
            1e-300,
        ] {
            let interval = Plan {
                calls: 1,
                rate,
                most_in_flight: 1,
            }
            .interval();
            assert!(
                interval.is_zero() || interval > Duration::ZERO,
                "rate {rate}"
            );
        }
    }

    /// And a NaN percentile answers "no measurement" rather than silently returning the fastest
    /// call as though it were the answer.
    #[test]
    fn a_nonsensical_percentile_is_none_rather_than_the_fastest_call() {
        let outcome = Outcome {
            setup: vec![Duration::from_millis(1), Duration::from_secs(9)],
            ..Outcome::default()
        };
        assert_eq!(outcome.percentile(f64::NAN), None);
        assert_eq!(outcome.percentile(0.0), Some(Duration::from_millis(1)));
    }
}
