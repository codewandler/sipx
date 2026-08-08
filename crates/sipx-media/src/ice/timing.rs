//! ICE's timers (RFC 8445 §14, RFC 5389 §7.2.1; [spec] §9).
//!
//! Everything the agent waits for is a value in [`Timers`], and nothing in the state machine is a
//! literal — the same rule the transaction timers follow
//! ([`sipx_sip::transaction::Timers`](https://docs.rs/sipx-sip)), for the same reason: a duration
//! written into a `match` arm is a duration nobody can configure and no test can shorten.
//!
//! The one that is not a constant at all is [`Timers::rto`]. §14.3 computes the retransmission
//! interval from how many checks are outstanding *right now* — "the RTO will be different for
//! each transaction as the number of checks in the Waiting and In-Progress states change" — so it
//! is a function of the checklist set and is evaluated when a check goes out, not once at
//! construction.
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::time::Duration;

/// The timers of RFC 8445 §14 and RFC 5389 §7.2.1, plus the one stopping value that is sipx's own.
///
/// Every field is a value the deployment may change; [`Timers::default`] is what the RFCs
/// recommend. Two of them have normative floors that [`Timers::pacing`] and [`Timers::rto`]
/// enforce rather than trust: Ta may not pace faster than 5 ms across every agent in the process
/// (§14.2), and an RTO may never be below 500 ms (§14.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timers {
    /// Ta — the pacing interval: one check leaves per tick, across the whole checklist set
    /// (§14.2). Default 50 ms.
    pub ta: Duration,
    /// The floor under Ta, "as though there were one global Ta value for pacing all agents"
    /// (§14.2). Default 5 ms.
    pub pacing_floor: Duration,
    /// The floor under the RTO. §14.3: agents "MUST NOT use an RTO value smaller than 500 ms".
    pub rto_floor: Duration,
    /// Rc — how many times a request is transmitted before the transaction fails
    /// (RFC 5389 §7.2.1). Default 7.
    pub rc: u32,
    /// Rm — the multiplier on the wait after the last transmission (RFC 5389 §7.2.1). Default 16.
    pub rm: u32,
    /// Tr — how long a selected pair may carry no data before a keepalive is sent (§11).
    /// Default 15 s, which §11 also makes the minimum: "MUST NOT use a value smaller".
    pub tr: Duration,
    /// Tn — sipx's own stopping value: how long the controlling agent keeps checking after the
    /// first valid pair appears before it nominates ([spec] §8). Default 1 s.
    ///
    /// §8.1.1 leaves the stopping criterion to local optimisation and requires only that exactly
    /// one pair is eventually nominated. A number here rather than an emergent behaviour is what
    /// makes that choice testable.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    pub tn: Duration,
}

impl Default for Timers {
    fn default() -> Self {
        Self {
            ta: Duration::from_millis(50),
            pacing_floor: Duration::from_millis(5),
            rto_floor: Duration::from_millis(500),
            rc: 7,
            rm: 16,
            tr: Duration::from_secs(15),
            tn: Duration::from_secs(1),
        }
    }
}

impl Timers {
    /// The interval between checks: Ta, but never below the process-wide floor (§14.2).
    #[must_use]
    pub fn pacing(&self) -> Duration {
        self.ta.max(self.pacing_floor)
    }

    /// The retransmission interval for a check being sent now (§14.3):
    ///
    /// ```text
    /// RTO = MAX(500ms, Ta * N * (Num-Waiting + Num-In-Progress))
    /// ```
    ///
    /// `checks` is `N`, the total number of connectivity checks to be performed — the size of the
    /// checklist set, not of one checklist. `outstanding` is `Num-Waiting + Num-In-Progress`
    /// across that same set, which is why this cannot be computed once: it falls as the checks
    /// drain, and §14.3 says so outright.
    #[must_use]
    pub fn rto(&self, checks: usize, outstanding: usize) -> Duration {
        let scale = u32::try_from(checks.saturating_mul(outstanding)).unwrap_or(u32::MAX);
        self.pacing().saturating_mul(scale).max(self.rto_floor)
    }

    /// The wait after the last transmission of a check, after which the transaction has timed out
    /// (RFC 5389 §7.2.1: "a duration equal to Rm times the RTO").
    #[must_use]
    pub fn final_wait(&self, rto: Duration) -> Duration {
        rto.saturating_mul(self.rm)
    }

    /// The next retransmission interval: RFC 5389 §7.2.1 doubles it after every transmission.
    #[must_use]
    pub fn double(&self, rto: Duration) -> Duration {
        rto.saturating_mul(2)
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

    #[test]
    fn the_defaults_are_the_values_the_spec_tabulates() {
        let timers = Timers::default();
        assert_eq!(timers.ta, Duration::from_millis(50));
        assert_eq!(timers.pacing_floor, Duration::from_millis(5));
        assert_eq!(timers.rto_floor, Duration::from_millis(500));
        assert_eq!(timers.rc, 7);
        assert_eq!(timers.rm, 16);
        assert_eq!(timers.tr, Duration::from_secs(15));
        assert_eq!(timers.tn, Duration::from_secs(1));
    }

    /// §14.2's floor is across every agent in the process, so it applies to a configured Ta as
    /// well as to the default one.
    #[test]
    fn pacing_never_goes_below_the_five_millisecond_floor() {
        let timers = Timers {
            ta: Duration::from_millis(1),
            ..Timers::default()
        };
        assert_eq!(timers.pacing(), Duration::from_millis(5));
    }

    /// The whole point of §14.3: the same agent computes a different RTO for the check it sends
    /// now and the one it sends when the checklist has drained.
    #[test]
    fn the_rto_falls_as_the_outstanding_checks_drain() {
        let timers = Timers::default();
        // 50 ms * 10 checks * 10 outstanding = 5 s.
        assert_eq!(timers.rto(10, 10), Duration::from_secs(5));
        // Same checklist, one check left outstanding: 50 ms * 10 * 1 = 500 ms.
        assert_eq!(timers.rto(10, 1), Duration::from_millis(500));
        // And below the floor it is the floor, never the product.
        assert_eq!(timers.rto(1, 1), Duration::from_millis(500));
        assert_eq!(timers.rto(0, 0), Duration::from_millis(500));
    }

    #[test]
    fn retransmissions_double_and_the_last_wait_is_rm_times_the_rto() {
        let timers = Timers::default();
        let rto = timers.rto(4, 4);
        assert_eq!(rto, Duration::from_millis(800));
        assert_eq!(timers.double(rto), Duration::from_millis(1600));
        assert_eq!(timers.final_wait(rto), Duration::from_millis(800 * 16));
    }
}
