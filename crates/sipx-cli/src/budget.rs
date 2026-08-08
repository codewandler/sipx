//! The one budget a command's stated deadline buys, and the single clock it is measured on.
//!
//! `P-25` gave `register` this accounting and `P-29` made it every command's: a deadline is the
//! bound on the *process*, so everything that can wait — resolution, the transaction, each further
//! candidate — is funded from what the phase before it left rather than given a schedule of its
//! own. `P-26` bounded resolution *by* the stated value instead of subtracting it *from* it, which
//! left `dial --timeout 5` able to spend five seconds looking a name up and five more waiting for
//! an answer: two phases of the number the caller typed, and a command that obeyed neither.
//!
//! The restatement rule lives with it. A report names the deadline the caller stated, never the
//! slice of it one phase happened to be funded from: "did not complete within 999ms" describes an
//! accounting detail, and the person reading it asked for one second. What that costs is that a
//! measured elapsed has to be read on the same clock as the limit beside it, which is why
//! [`Attempt::elapsed`] is the one every command reports.

use std::time::Duration;

/// The caller's bound over one attempt, and the single clock it is measured on.
///
/// `--timeout 0` keeps the pre-deadline behaviour and is `limit: None` here: the clock still runs,
/// so a report can say how long the attempt took, but nothing expires.
#[derive(Debug)]
pub(crate) struct Attempt {
    started: tokio::time::Instant,
    limit: Option<Duration>,
}

impl Attempt {
    /// Start the clock on a stated deadline, where zero states none.
    pub(crate) fn new(limit: Duration) -> Self {
        Self {
            started: tokio::time::Instant::now(),
            limit: (!limit.is_zero()).then_some(limit),
        }
    }

    /// How long the attempt has been running.
    pub(crate) fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// What is left of the budget, or `None` when the caller asked for no deadline.
    ///
    /// This is what every phase is funded from, and `Some(0)` is a budget with nothing left in it
    /// rather than an absent one — the distinction [`Attempt::spent`] asks about, and the one a
    /// caller that collapsed it into `None` would answer by running the next phase unbounded.
    pub(crate) fn remaining(&self) -> Option<Duration> {
        self.limit.map(|limit| limit.saturating_sub(self.elapsed()))
    }

    /// Whether the budget existed and is now gone.
    pub(crate) fn spent(&self) -> bool {
        self.remaining().is_some_and(|left| left.is_zero())
    }

    /// The deadline as the caller stated it, for the phases that take it or leave it.
    pub(crate) fn stated(&self) -> Option<Duration> {
        self.limit
    }

    /// The deadline as it is reported and as an expiry names it.
    pub(crate) fn limit(&self) -> Duration {
        self.limit.unwrap_or_default()
    }

    /// One invitation's options, funded from what the budget has left right now.
    ///
    /// `None` is a budget with nothing left in it, which ends a candidate pass rather than
    /// starting an invitation nothing would bound. The pass is where a per-phase deadline is
    /// easiest to miss: handing each candidate the *stated* value turns a name with three
    /// addresses into three times the number the caller typed, and `register`'s pass is funded
    /// this way inside the library for exactly that reason (`T-41`).
    pub(crate) fn fund(&self, options: &sipx_call::DialOptions) -> Option<sipx_call::DialOptions> {
        match self.remaining() {
            Some(remaining) if remaining.is_zero() => None,
            Some(remaining) => Some(options.clone().with_timeout(remaining)),
            None => Some(options.clone()),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// An attempt that has already been running for `spent`.
    ///
    /// The clock is moved rather than waited on: `tokio`'s virtual time needs its `test-util`
    /// feature, which the binary does not carry, and a real wait here would answer a question
    /// about the scheduler instead of one about the arithmetic.
    fn running_for(spent: Duration, limit: Duration) -> Attempt {
        let mut attempt = Attempt::new(limit);
        attempt.started -= spent;
        attempt
    }

    /// The whole point of the type: a second phase sees what the first one spent.
    #[test]
    fn a_phase_is_funded_from_what_the_last_one_left() {
        let attempt = running_for(Duration::from_secs(2), Duration::from_secs(5));

        let left = attempt.remaining().expect("a stated deadline");
        assert!(
            left <= Duration::from_secs(3),
            "the second phase gets the remainder, not a fresh copy of the deadline: {left:?}"
        );
        assert!(
            left > Duration::from_millis(2_900),
            "and it gets the whole of the remainder: {left:?}"
        );
        assert!(!attempt.spent());
        assert_eq!(
            attempt.limit(),
            Duration::from_secs(5),
            "the reported deadline stays the number the caller typed"
        );
    }

    /// A budget with nothing left is not an absent one. Collapsing the two is exactly how a phase
    /// ends up unbounded at the moment there is least reason to allow it.
    #[test]
    fn an_exhausted_budget_is_distinguishable_from_no_budget() {
        let attempt = running_for(Duration::from_secs(4), Duration::from_secs(1));
        assert_eq!(
            attempt.remaining(),
            Some(Duration::ZERO),
            "saturating, never wrapping past zero"
        );
        assert!(attempt.spent());

        let unbounded = running_for(Duration::from_secs(9), Duration::ZERO);
        assert_eq!(unbounded.remaining(), None);
        assert_eq!(unbounded.stated(), None);
        assert!(
            !unbounded.spent(),
            "a deadline that was never stated cannot run out"
        );
        // The clock is itself the measurement here: nothing is waited for, the start was moved
        // back nine seconds by `running_for`, and the assertion is about which duration that
        // leaves. Load can only push the reading up, which is the direction that keeps it true.
        assert!(
            unbounded.elapsed() >= Duration::from_secs(9),
            "the clock runs either way, so a report can still say how long this took"
        );
    }

    /// The candidate pass is funded the same way, and an empty budget ends it rather than
    /// producing an invitation nothing would bound.
    #[test]
    fn a_candidate_is_funded_from_the_remainder_and_an_empty_budget_funds_none() {
        let options = sipx_call::DialOptions::new(
            "<sip:sipx@127.0.0.1>",
            std::net::IpAddr::from([127, 0, 0, 1]),
        );

        let attempt = running_for(Duration::from_secs(2), Duration::from_secs(5));
        let funded = attempt
            .fund(&options)
            .expect("a budget with something left");
        let timeout = funded.timeout.expect("the candidate is bounded");
        assert!(
            timeout <= Duration::from_secs(3) && timeout > Duration::from_millis(2_900),
            "each candidate is bounded by the remainder, not by the stated deadline: {timeout:?}"
        );

        assert!(
            running_for(Duration::from_secs(6), Duration::from_secs(5))
                .fund(&options)
                .is_none(),
            "a spent budget funds no further candidate"
        );
        assert!(
            running_for(Duration::from_secs(6), Duration::ZERO)
                .fund(&options)
                .expect("no deadline was stated")
                .timeout
                .is_none(),
            "and a caller that stated no deadline still gets none"
        );
    }
}
