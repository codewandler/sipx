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
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

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

/// Finite admission and cleanup limits for an externally controllable run.
///
/// Unlike [`Plan`], either `calls` or `duration` may end admission. At least one must be present;
/// command layers validate that contract before calling this harness.
#[derive(Debug, Clone, Copy)]
pub struct BoundedPlan {
    /// Maximum calls admitted, if count-bounded.
    pub calls: Option<usize>,
    /// Maximum time during which calls may be admitted, if time-bounded.
    /// A duration beyond the runtime clock's range closes admission immediately rather than
    /// panicking; command layers should reject it as invalid input.
    pub duration: Option<Duration>,
    /// Calls admitted per second.
    pub rate: f64,
    /// Reproducible arrival-jitter seed.
    pub seed: u64,
    /// Maximum simultaneously active calls.
    ///
    /// The harness normalizes zero to one and values above Tokio's semaphore ceiling to that
    /// ceiling so a programmatically constructed plan cannot panic. User-facing command layers
    /// should reject either value and report the invalid configuration instead.
    pub most_in_flight: usize,
    /// Time allowed for every owned call to acknowledge stop and finish.
    pub cleanup: Duration,
}

impl BoundedPlan {
    fn interval(self) -> Duration {
        Plan {
            calls: self.calls.unwrap_or(0),
            rate: self.rate,
            most_in_flight: self.most_in_flight,
        }
        .interval()
    }

    fn gap(self, index: usize) -> Duration {
        let base = self.interval();
        if base.is_zero() {
            return base;
        }
        // A stateless integer mixer gives each call a stable, well-distributed value without
        // mutable scheduler state. The factor in [0.5, 1.5) preserves the requested average while
        // avoiding an artificial metronome. Wrapping arithmetic is intentional, not an overflow
        // of a workload bound.
        let mut value = self.seed.wrapping_add(
            u64::try_from(index)
                .unwrap_or(u64::MAX)
                .wrapping_mul(0x9e37_79b9_7f4a_7c15),
        );
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        let unit = (value >> 11) as f64 / ((1u64 << 53) as f64);
        Duration::try_from_secs_f64(base.as_secs_f64() * (0.5 + unit)).unwrap_or(base)
    }
}

/// A clonable stop signal shared by admission and every owned call.
#[derive(Debug, Clone, Default)]
pub struct Stop {
    token: CancellationToken,
}

impl Stop {
    /// A fresh signal that has not been requested.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Close admission and ask active calls to clean up.
    pub fn request(&self) {
        self.token.cancel();
    }

    /// Wait until cleanup has been requested.
    pub async fn requested(&self) {
        self.token.cancelled().await;
    }

    /// Whether cleanup has already been requested.
    #[must_use]
    pub fn is_requested(&self) -> bool {
        self.token.is_cancelled()
    }
}

/// Why the harness closed admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionEnd {
    /// The configured call count was admitted.
    Calls,
    /// The configured admission duration elapsed.
    Duration,
    /// The owner requested interruption.
    Requested,
}

/// Outcome plus the lifecycle facts a bounded command must report.
#[derive(Debug, Clone)]
pub struct BoundedOutcome {
    /// Per-call counts and setup measurements.
    pub outcome: Outcome,
    /// Greatest number of calls active simultaneously.
    pub peak_in_flight: usize,
    /// The event that closed admission.
    pub admission_end: AdmissionEnd,
    /// Whether every owned task finished inside the cleanup budget.
    pub cleanup_complete: bool,
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
    let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(
        plan.most_in_flight
            .clamp(1, tokio::sync::Semaphore::MAX_PERMITS),
    ));

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

fn account_bounded(
    joined: std::result::Result<(std::result::Result<(), Cause>, Duration), tokio::task::JoinError>,
    outcome: &mut Outcome,
) -> bool {
    match joined {
        Ok((Ok(()), took)) => {
            outcome.succeeded += 1;
            outcome.setup.push(took);
            false
        }
        Ok((Err(cause), _)) => {
            let internal = matches!(cause, Cause::Other(_));
            *outcome.failures.entry(cause).or_default() += 1;
            internal
        }
        Err(joined) => {
            let label = if joined.is_panic() {
                "panicked"
            } else {
                "cancelled"
            };
            *outcome
                .failures
                .entry(Cause::Other(label.to_owned()))
                .or_default() += 1;
            true
        }
    }
}

struct ActiveCall {
    active: Arc<AtomicUsize>,
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Run a finitely bounded plan, stopping admission and draining every owned call before return.
///
/// The call future receives the same [`Stop`] as the scheduler. Once it has established a call it
/// should select that signal alongside its normal holding period, then perform its protocol cleanup
/// before returning. The harness never detaches work: even a cleanup-budget failure aborts and joins
/// every local task before it reports that cleanup was incomplete.
#[allow(
    clippy::too_many_lines,
    reason = "admission and drain are one lifecycle; splitting them would make detached cleanup easier to write"
)]
pub async fn run_bounded<F, Fut>(plan: BoundedPlan, stop: Stop, place: F) -> BoundedOutcome
where
    F: Fn(usize, Stop) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), Cause>> + Send + 'static,
{
    let place = Arc::new(place);
    let permits = Arc::new(tokio::sync::Semaphore::new(
        plan.most_in_flight
            .clamp(1, tokio::sync::Semaphore::MAX_PERMITS),
    ));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let started = tokio::time::Instant::now();
    let duration_deadline = plan
        .duration
        .map(|duration| started.checked_add(duration).unwrap_or(started));
    let mut running = tokio::task::JoinSet::new();
    let mut admitted = 0usize;
    let mut scheduled = started;
    let mut outcome = Outcome::default();

    let admission_end = loop {
        // Completed tasks stay allocated inside a JoinSet until they are joined. Drain them on
        // every admission turn rather than retaining one record per call until admission closes;
        // a long count-bounded run then uses memory in proportion to active concurrency.
        let mut internal_failure = false;
        while let Some(joined) = running.try_join_next() {
            internal_failure |= account_bounded(joined, &mut outcome);
        }
        if internal_failure {
            stop.request();
        }
        if stop.is_requested() {
            break AdmissionEnd::Requested;
        }
        if plan.calls.is_some_and(|calls| admitted >= calls) {
            break AdmissionEnd::Calls;
        }

        let admission_wait = tokio::time::sleep_until(scheduled);
        tokio::pin!(admission_wait);
        if let Some(deadline) = duration_deadline {
            tokio::select! {
                biased;
                () = stop.requested() => break AdmissionEnd::Requested,
                () = tokio::time::sleep_until(deadline) => break AdmissionEnd::Duration,
                () = &mut admission_wait => {}
            }
        } else {
            tokio::select! {
                biased;
                () = stop.requested() => break AdmissionEnd::Requested,
                () = &mut admission_wait => {}
            }
        }

        let permit = if let Some(deadline) = duration_deadline {
            tokio::select! {
                biased;
                () = stop.requested() => break AdmissionEnd::Requested,
                () = tokio::time::sleep_until(deadline) => break AdmissionEnd::Duration,
                permit = Arc::clone(&permits).acquire_owned() => permit,
            }
        } else {
            tokio::select! {
                biased;
                () = stop.requested() => break AdmissionEnd::Requested,
                permit = Arc::clone(&permits).acquire_owned() => permit,
            }
        };
        let Ok(permit) = permit else {
            break AdmissionEnd::Requested;
        };

        let index = admitted;
        admitted += 1;
        scheduled += plan.gap(index);
        let place = Arc::clone(&place);
        let call_stop = stop.clone();
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        running.spawn(async move {
            let _permit = permit;
            let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
            let _active = ActiveCall { active };
            peak.fetch_max(now_active, Ordering::SeqCst);
            let at = tokio::time::Instant::now();
            // The JoinSet owns this future directly. Aborting the set therefore drops the call
            // future, its permit and its active guard before the harness can return.
            let result = place(index, call_stop).await;
            (result, at.elapsed())
        });
    };

    // A count or duration bound is also an instruction to end the calls it owns. A call may have
    // connected just before admission closed; it observes this before the summary is emitted.
    stop.request();
    let cleanup_deadline = tokio::time::Instant::now() + plan.cleanup;
    outcome.attempted = admitted;
    let mut cleanup_complete = true;
    while !running.is_empty() {
        match tokio::time::timeout_at(cleanup_deadline, running.join_next()).await {
            Ok(Some(joined)) => {
                let _internal = account_bounded(joined, &mut outcome);
            }
            Ok(None) => break,
            Err(_) => {
                cleanup_complete = false;
                let unfinished = running.len();
                running.abort_all();
                while running.join_next().await.is_some() {}
                *outcome
                    .failures
                    .entry(Cause::Other("cleanup budget exhausted".to_owned()))
                    .or_default() += unfinished;
            }
        }
    }
    outcome.elapsed = started.elapsed();

    BoundedOutcome {
        outcome,
        peak_in_flight: peak.load(Ordering::SeqCst),
        admission_end,
        cleanup_complete,
    }
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

    struct DropFlag(Arc<AtomicUsize>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// DPH-10: the count bound closes admission, signals every owned call, and the result is not
    /// returned until all of them have acknowledged cleanup.
    #[tokio::test]
    async fn bounded_run_reaches_its_call_bound_and_cleans_every_owned_call() {
        let cleaned = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&cleaned);
        let bounded = run_bounded(
            BoundedPlan {
                calls: Some(6),
                duration: None,
                rate: 100_000.0,
                seed: 7,
                most_in_flight: 6,
                cleanup: Duration::from_secs(1),
            },
            Stop::new(),
            move |_, stop| {
                let cleaned = Arc::clone(&seen);
                async move {
                    stop.requested().await;
                    cleaned.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await;

        assert_eq!(bounded.admission_end, AdmissionEnd::Calls);
        assert_eq!(bounded.outcome.attempted, 6);
        assert_eq!(bounded.outcome.succeeded, 6);
        assert_eq!(cleaned.load(Ordering::SeqCst), 6);
        assert!(bounded.cleanup_complete);
    }

    /// DPH-11: interruption is a causal signal, not a sleep followed by an assumption. Once the
    /// first call announces that it started, interruption closes admission and cleanup completes
    /// before the harness returns.
    #[tokio::test]
    async fn interrupted_run_stops_admission_and_waits_for_cleanup() {
        let stop = Stop::new();
        let controller = stop.clone();
        let started = Arc::new(tokio::sync::Notify::new());
        let began = Arc::clone(&started);
        let cleaned = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&cleaned);

        let run = tokio::spawn(run_bounded(
            BoundedPlan {
                calls: Some(10_000),
                duration: None,
                rate: 1.0,
                seed: 9,
                most_in_flight: 2,
                cleanup: Duration::from_secs(1),
            },
            stop,
            move |_, stop| {
                began.notify_one();
                let cleaned = Arc::clone(&seen);
                async move {
                    stop.requested().await;
                    cleaned.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        ));

        started.notified().await;
        controller.request();
        let bounded = run.await.expect("the bounded harness joins");

        assert_eq!(bounded.admission_end, AdmissionEnd::Requested);
        assert!(bounded.outcome.attempted < 10_000);
        assert_eq!(
            cleaned.load(Ordering::SeqCst),
            bounded.outcome.attempted,
            "the summary follows cleanup of every owned call"
        );
        assert!(bounded.cleanup_complete);
    }

    /// A cleanup deadline may abort work, but may never detach it. The flag is owned by the call
    /// future itself, so observing its drop proves `run_bounded` joined the aborted future before
    /// returning rather than only aborting an outer wrapper.
    #[tokio::test]
    async fn cleanup_timeout_drops_the_owned_call_before_returning() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&dropped);
        let bounded = run_bounded(
            BoundedPlan {
                calls: Some(1),
                duration: None,
                rate: 1.0,
                seed: 0,
                most_in_flight: 1,
                cleanup: Duration::from_millis(20), // A bound on failure: this call never ends.
            },
            Stop::new(),
            move |_, _| {
                let flag = DropFlag(Arc::clone(&observed));
                async move {
                    let _flag = flag;
                    std::future::pending::<Result<(), Cause>>().await
                }
            },
        )
        .await;

        assert!(!bounded.cleanup_complete);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
        assert_eq!(bounded.outcome.attempted, 1);
        assert_eq!(bounded.outcome.failed(), 1);
    }

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
