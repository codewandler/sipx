//! Proving nothing grows without bound.
//!
//! This is the failure that only appears in production, because it is the only one that needs
//! hours to become visible. A stack that leaks a task per call is indistinguishable from a
//! correct one for the length of any test somebody runs impatiently.
//!
//! **Flat, not merely bounded.** A leak that fills a pool is still a leak; the pool only hides
//! it until it becomes a problem, and then hides the cause too. So the assertion is that the
//! reading at the end matches the reading at the start, within a tolerance for the noise a
//! runtime genuinely has — not that it stayed under some ceiling.
//!
//! **Measured after settling, and the settling period is longer than it looks.** Ending a call
//! is not instantaneous, and the slow part is not teardown — it is the protocol. RFC 3261 §17
//! keeps a completed server transaction alive for Timer J, 64·T1, **thirty-two seconds**, so it
//! can absorb a retransmitted request. For that whole time there is a task per call that has
//! ended, and it is doing exactly what the RFC requires.
//!
//! So a settle shorter than the longest transaction timer reports the specification as a leak.
//! The first version of sipx's own soak used five seconds and duly failed with "tasks grew from
//! 5 to 305" after 300 calls — a number that looks exactly like a one-task-per-call leak and was
//! not one. [`SETTLE_PAST_TIMERS`] is the floor.

use std::time::Duration;

/// The shortest settling period that does not accuse the protocol of leaking.
///
/// RFC 3261's Timer J and Timer K are both 64·T1 — thirty-two seconds with the default T1 — and
/// a completed transaction is *supposed* to sit there for that long. Forty seconds leaves room
/// for a run whose last call ended a little after the load did.
///
/// A soak measured over a shorter period is measuring the RFC, and the result of measuring the
/// RFC is a failing test that somebody eventually deletes.
///
/// This is a **definition of silence** in the sense `docs/designs/media.md` gives the term: how
/// long a hole has to be before "everything that was going to end has ended" is true, so that what
/// is still resident afterwards is a leak rather than the specification. It is not a bound on
/// failure and it is not a measurement — a run that settles for longer is more trustworthy, not
/// less, which is why the constant is a floor (`X-44`).
pub const SETTLE_PAST_TIMERS: Duration = Duration::from_secs(40);

/// A reading of the things that must not grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reading {
    /// Tasks alive in the runtime.
    pub tasks: usize,
    /// File descriptors held by this process, which is where sockets show up.
    pub descriptors: usize,
    /// Whatever the stack under test counts as outstanding — transactions, dialogs, connections.
    pub outstanding: usize,
    /// Resident memory, in kilobytes.
    ///
    /// The dimension the other three cannot see. A session that grows a `Vec` for every packet
    /// leaks steadily while its task count and transaction count stay perfectly flat, and that
    /// is an ordinary shape for a leak — a recording buffer, a statistics history, a queue with
    /// no bound.
    pub resident_kb: usize,
}

/// How much drift is noise rather than a leak.
///
/// Not zero, and the reason is worth stating: a runtime keeps blocking-pool threads alive after
/// use, an allocator does not return every page, and a descriptor may still be in `TIME_WAIT`.
/// A tolerance of zero produces a test that fails at random, and a test that fails at random is
/// a test that gets deleted.
#[derive(Debug, Clone, Copy)]
pub struct Tolerance {
    /// Extra tasks allowed at the end.
    pub tasks: usize,
    /// Extra descriptors allowed.
    pub descriptors: usize,
    /// Extra outstanding items allowed.
    pub outstanding: usize,
    /// Extra resident kilobytes allowed.
    ///
    /// Much the largest tolerance here, and it has to be. An allocator does not return every
    /// freed page to the kernel, a runtime grows its per-thread caches on first use, and the
    /// first few hundred calls touch code paths that are still being faulted in. Sixteen
    /// megabytes is loose enough not to fail on any of that and tight enough that a leak of a
    /// kilobyte per call shows up within a few thousand calls.
    pub resident_kb: usize,
}

impl Default for Tolerance {
    fn default() -> Self {
        // Small absolute numbers rather than percentages. A percentage of a small reading is
        // less than one, and a percentage of a large one hides exactly the leak that matters.
        Self {
            tasks: 4,
            descriptors: 8,
            outstanding: 0,
            resident_kb: 16 * 1024,
        }
    }
}

/// What a soak run found.
#[derive(Debug, Clone)]
pub struct Soak {
    /// Before the load.
    pub before: Reading,
    /// After it, once things have settled.
    pub after: Reading,
    /// How long the run was given to settle before the second reading.
    pub settled_for: Duration,
}

impl Soak {
    /// Everything that grew beyond its tolerance, described.
    ///
    /// A list rather than a boolean, because "something leaked" is not an actionable report and
    /// the whole point of separating the readings is to say *what*.
    #[must_use]
    pub fn leaks(&self, tolerance: Tolerance) -> Vec<String> {
        let mut found = Vec::new();
        for (what, before, after, allowed) in [
            (
                "tasks",
                self.before.tasks,
                self.after.tasks,
                tolerance.tasks,
            ),
            (
                "descriptors",
                self.before.descriptors,
                self.after.descriptors,
                tolerance.descriptors,
            ),
            (
                "outstanding",
                self.before.outstanding,
                self.after.outstanding,
                tolerance.outstanding,
            ),
            (
                "resident_kb",
                self.before.resident_kb,
                self.after.resident_kb,
                tolerance.resident_kb,
            ),
        ] {
            let grew = after.saturating_sub(before);
            if grew > allowed {
                found.push(format!(
                    "{what} grew from {before} to {after} (+{grew}, tolerance {allowed})"
                ));
            }
        }
        found
    }

    /// Whether it is flat within tolerance.
    #[must_use]
    pub fn is_flat(&self, tolerance: Tolerance) -> bool {
        self.leaks(tolerance).is_empty()
    }

    /// A report a person can read.
    #[must_use]
    pub fn report(&self, tolerance: Tolerance) -> String {
        let leaks = self.leaks(tolerance);
        if leaks.is_empty() {
            return format!(
                "flat after {:.0}s: tasks {}→{}, descriptors {}→{}, outstanding {}→{}, \
                 resident {} kB→{} kB",
                self.settled_for.as_secs_f64(),
                self.before.tasks,
                self.after.tasks,
                self.before.descriptors,
                self.after.descriptors,
                self.before.outstanding,
                self.after.outstanding,
                self.before.resident_kb,
                self.after.resident_kb
            );
        }
        format!("leaked:\n  {}", leaks.join("\n  "))
    }
}

/// How many file descriptors this process holds.
///
/// Linux only, through `/proc`. Elsewhere it reports zero, which the tolerance then trivially
/// accepts — a soak run on another platform still checks tasks and outstanding items, and
/// pretending to a descriptor count that was never taken would be worse than admitting to none.
#[must_use]
pub fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, std::iter::Iterator::count)
}

/// Resident memory in kilobytes.
///
/// Linux only, from `/proc/self/statm`, whose second field is the resident set in pages.
/// Elsewhere it reports zero and the tolerance trivially accepts it — a soak on another
/// platform still checks the other three, and inventing a figure that was never measured would
/// be worse than admitting to none.
#[must_use]
pub fn resident_kb() -> usize {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let Some(pages) = statm.split_whitespace().nth(1) else {
        return 0;
    };
    let Ok(pages) = pages.parse::<usize>() else {
        return 0;
    };
    // 4 kB pages on every platform this runs on. Reading the real page size would mean a libc
    // call for a number that has not changed on x86-64 or aarch64 Linux.
    pages.saturating_mul(4)
}

/// The number of tasks alive in the current runtime.
///
/// `alive_tasks` is the runtime's own count and needs no bookkeeping from the caller, which
/// matters: a count the test maintains itself would be counting the test's model of the
/// system rather than the system.
#[must_use]
pub fn alive_tasks() -> usize {
    tokio::runtime::Handle::try_current().map_or(0, |handle| handle.metrics().num_alive_tasks())
}

/// Take a reading of the process now.
#[must_use]
pub fn sample(outstanding: usize) -> Reading {
    Reading {
        tasks: alive_tasks(),
        descriptors: open_descriptors(),
        outstanding,
        resident_kb: resident_kb(),
    }
}

/// Run `load`, then wait for things to settle, and report what grew.
///
/// `outstanding` is asked twice — before and after — for whatever the stack under test counts
/// as work in progress.
/// `settle` should be at least [`SETTLE_PAST_TIMERS`] for anything driving SIP — see the module
/// documentation for why a shorter one reports RFC-mandated state as a leak.
pub async fn soak<L, Load, O, Count>(settle: Duration, outstanding: O, load: L) -> Soak
where
    L: FnOnce() -> Load,
    Load: std::future::Future<Output = ()>,
    // Async, because what is being counted usually lives behind an event loop. A synchronous
    // bound forces the caller into `block_in_place` and a nested `block_on`, which panics
    // outright on a current-thread runtime — an undocumented runtime-flavour requirement
    // inherited by everyone who samples an async quantity.
    O: Fn() -> Count,
    Count: std::future::Future<Output = usize>,
{
    let before = sample(outstanding().await);
    load().await;
    // Settling is not optional, and it is not about teardown. A completed SIP transaction sits
    // in `Completed` for Timer J — 64·T1, thirty-two seconds — absorbing retransmissions, which
    // is what the RFC asks of it. Sampling before that has elapsed counts every one of those as
    // a leaked task.
    tokio::time::sleep(settle).await;
    let after = sample(outstanding().await);
    Soak {
        before,
        after,
        settled_for: settle,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn reading(tasks: usize, descriptors: usize, outstanding: usize) -> Reading {
        Reading {
            tasks,
            descriptors,
            outstanding,
            resident_kb: 0,
        }
    }

    fn soak_of(before: Reading, after: Reading) -> Soak {
        Soak {
            before,
            after,
            settled_for: Duration::from_secs(1),
        }
    }

    /// X-5's exit criterion. The assertion has to have teeth, and the only way to know it does
    /// is to give it a leak and watch it fail.
    #[tokio::test]
    async fn an_injected_leak_fails_the_soak() {
        // Tasks that outlive the load, which is exactly what a leaked call looks like.
        let held: std::sync::Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let leaking = std::sync::Arc::clone(&held);
        let result = soak(
            Duration::from_millis(200),
            || async { 0 },
            || async move {
                for _ in 0..40 {
                    let handle = tokio::spawn(async {
                        // Never finishes: the leak.
                        std::future::pending::<()>().await;
                    });
                    leaking.lock().expect("not poisoned").push(handle);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            },
        )
        .await;

        assert!(
            !result.is_flat(Tolerance::default()),
            "forty leaked tasks must fail the run: {}",
            result.report(Tolerance::default())
        );
        assert!(
            result.report(Tolerance::default()).contains("tasks grew"),
            "and must say what leaked: {}",
            result.report(Tolerance::default())
        );

        for handle in held.lock().expect("not poisoned").drain(..) {
            handle.abort();
        }
    }

    /// The floor is longer than the longest transaction timer, or the soak accuses the
    /// specification of leaking.
    #[test]
    fn the_settling_floor_outlasts_a_sip_transaction() {
        // 64·T1 with the default T1 of 500 ms.
        assert!(
            SETTLE_PAST_TIMERS > Duration::from_secs(32),
            "Timer J is 32 s; anything shorter counts a completed transaction as a leak"
        );
    }

    /// And the other half: a run that leaks nothing passes. Without this the test above holds
    /// against an assertion that fails everything.
    #[tokio::test]
    async fn a_clean_run_is_flat() {
        let result = soak(
            Duration::from_millis(200),
            || async { 0 },
            || async {
                for _ in 0..40 {
                    tokio::spawn(async {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    });
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            },
        )
        .await;

        assert!(
            result.is_flat(Tolerance::default()),
            "{}",
            result.report(Tolerance::default())
        );
    }

    /// Flat, not merely bounded. A pool that filled up would satisfy "under the ceiling" and is
    /// still a leak — the pool hides it until it becomes a problem, and then hides the cause.
    #[test]
    fn growth_within_a_ceiling_is_still_a_leak() {
        let run = soak_of(reading(10, 20, 0), reading(10, 20, 40));
        assert!(!run.is_flat(Tolerance::default()));
        assert!(
            run.report(Tolerance::default())
                .contains("outstanding grew")
        );
    }

    /// Each dimension is reported on its own. "Something leaked" is not actionable.
    #[test]
    fn every_dimension_that_grew_is_named() {
        let run = soak_of(reading(10, 20, 0), reading(100, 200, 40));
        let leaks = run.leaks(Tolerance::default());
        assert_eq!(leaks.len(), 3, "{leaks:?}");
        assert!(leaks.iter().any(|l| l.starts_with("tasks")));
        assert!(leaks.iter().any(|l| l.starts_with("descriptors")));
        assert!(leaks.iter().any(|l| l.starts_with("outstanding")));
    }

    /// A tolerance of zero produces a test that fails at random, and a test that fails at
    /// random is a test that gets deleted. Small drift must pass.
    #[test]
    fn ordinary_runtime_drift_is_not_a_leak() {
        let run = soak_of(reading(10, 20, 0), reading(12, 24, 0));
        assert!(
            run.is_flat(Tolerance::default()),
            "{}",
            run.report(Tolerance::default())
        );
    }

    /// Shrinking is never a leak.
    #[test]
    fn a_reading_that_fell_is_not_growth() {
        let run = soak_of(reading(100, 200, 40), reading(10, 20, 0));
        assert!(run.is_flat(Tolerance::default()));
    }

    /// Outstanding work has no tolerance at all. A transaction store that ends a run holding
    /// anything is holding a transaction whose call is over.
    #[test]
    fn one_leftover_transaction_is_one_too_many() {
        let run = soak_of(reading(10, 20, 0), reading(10, 20, 1));
        assert!(
            !run.is_flat(Tolerance::default()),
            "a single leftover transaction is a leak: {}",
            run.report(Tolerance::default())
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod memory_tests {
    use super::*;

    /// The dimension the other three cannot see: a leak that grows buffers while task and
    /// transaction counts stay perfectly flat. `X-5` names memory in its acceptance, and a
    /// `Reading` without it would have that criterion ticked and unmet.
    #[test]
    fn memory_growth_is_a_leak_the_other_dimensions_would_miss() {
        let run = Soak {
            before: Reading {
                tasks: 10,
                descriptors: 20,
                outstanding: 0,
                resident_kb: 50_000,
            },
            after: Reading {
                tasks: 10,
                descriptors: 20,
                outstanding: 0,
                // A hundred megabytes more, with everything else identical.
                resident_kb: 150_000,
            },
            settled_for: Duration::from_secs(1),
        };
        assert!(!run.is_flat(Tolerance::default()));
        assert!(
            run.leaks(Tolerance::default())
                .iter()
                .any(|leak| leak.starts_with("resident_kb")),
            "{:?}",
            run.leaks(Tolerance::default())
        );
    }

    /// And the tolerance is loose enough for the ordinary case. An allocator that has not
    /// returned a few megabytes of freed pages is not a leak, and a soak that said it was would
    /// fail at random.
    #[test]
    fn a_few_megabytes_of_allocator_drift_is_not_a_leak() {
        let run = Soak {
            before: Reading {
                tasks: 10,
                descriptors: 20,
                outstanding: 0,
                resident_kb: 50_000,
            },
            after: Reading {
                tasks: 10,
                descriptors: 20,
                outstanding: 0,
                resident_kb: 54_000,
            },
            settled_for: Duration::from_secs(1),
        };
        assert!(
            run.is_flat(Tolerance::default()),
            "{}",
            run.report(Tolerance::default())
        );
    }

    /// It reports something real on Linux, which is where it claims to work.
    #[test]
    fn resident_memory_is_readable_here() {
        if std::path::Path::new("/proc/self/statm").exists() {
            assert!(resident_kb() > 0, "a running process has a resident set");
        }
    }
}
