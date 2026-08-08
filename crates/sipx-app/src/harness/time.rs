//! The only clock the harness has.
//!
//! [`Virtual`] is milliseconds since a scenario began, and it is the whole of acceptance point 4:
//! a scenario that wanted real time would have to name a real instant, and there is no way to get
//! one here. `Virtual` has a zero, which `tokio::time::Instant` does not — that type's only
//! constructors read the machine clock or take a `std::time::Instant` with no zero either, which
//! is exactly why `X-21` made the timer queue generic over its instant. This is that parameter's
//! second caller.

use std::ops::Add;
use std::time::Duration;

/// Milliseconds since the scenario began.
///
/// Deliberately not convertible from any real clock. The absence of a `now()` is the point: a
/// harness that could read the machine clock would let a scenario's outcome depend on how fast the
/// machine running it happens to be, which is the property this whole layer exists to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Virtual(u64);

impl Virtual {
    /// The instant a scenario starts at.
    #[must_use]
    pub const fn epoch() -> Self {
        Self(0)
    }

    /// This many milliseconds after the epoch.
    #[must_use]
    pub const fn at_millis(millis: u64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the epoch.
    #[must_use]
    pub const fn millis(self) -> u64 {
        self.0
    }

    /// How long since `earlier`, saturating at zero rather than wrapping.
    #[must_use]
    pub fn since(self, earlier: Self) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

impl Add<Duration> for Virtual {
    type Output = Self;

    fn add(self, after: Duration) -> Self {
        // A scenario that ran past u64 milliseconds is not a case worth a fallible conversion;
        // saturating keeps the clock monotonic either way.
        Self(
            self.0
                .saturating_add(u64::try_from(after.as_millis()).unwrap_or(u64::MAX)),
        )
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
    fn the_clock_starts_at_a_zero_a_real_instant_does_not_have() {
        assert_eq!(Virtual::epoch().millis(), 0);
        assert_eq!(Virtual::default(), Virtual::epoch());
    }

    #[test]
    fn adding_a_duration_moves_the_clock_forward_by_that_much() {
        let later = Virtual::epoch() + Duration::from_millis(2500);
        assert_eq!(later, Virtual::at_millis(2500));
        assert_eq!(later.since(Virtual::epoch()), Duration::from_millis(2500));
    }

    /// Ordering is what the timer queue needs of an instant, and it is the ordinary one.
    #[test]
    fn instants_order_by_when_they_are() {
        assert!(Virtual::at_millis(1) < Virtual::at_millis(2));
        assert_eq!(
            Virtual::at_millis(10).since(Virtual::at_millis(40)),
            Duration::ZERO,
            "time does not run backwards, and asking does not underflow"
        );
    }
}
