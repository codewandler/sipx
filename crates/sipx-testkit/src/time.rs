//! Virtual time for deterministic test drivers.

use std::ops::Add;
use std::time::Duration;

/// Nanoseconds since a harness began.
///
/// There is deliberately no `now()` constructor and no conversion from a wall clock. Tests move
/// this value explicitly, so a loaded machine cannot change a call flow's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Virtual(u64);

impl Virtual {
    /// The instant a harness starts.
    #[must_use]
    pub const fn epoch() -> Self {
        Self(0)
    }

    /// This many milliseconds after the harness started.
    #[must_use]
    pub const fn at_millis(millis: u64) -> Self {
        Self(millis.saturating_mul(1_000_000))
    }

    /// This many nanoseconds after the harness started.
    #[must_use]
    pub const fn at_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Milliseconds since the harness started.
    #[must_use]
    pub const fn millis(self) -> u64 {
        self.0 / 1_000_000
    }

    /// Nanoseconds since the harness started.
    #[must_use]
    pub const fn nanos(self) -> u64 {
        self.0
    }
}

impl Add<Duration> for Virtual {
    type Output = Self;

    fn add(self, after: Duration) -> Self {
        Self(
            self.0
                .saturating_add(u64::try_from(after.as_nanos()).unwrap_or(u64::MAX)),
        )
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn adding_sub_millisecond_time_preserves_every_nanosecond() {
        let instant = Virtual::epoch() + Duration::from_nanos(999_999);

        assert_eq!(instant.nanos(), 999_999);
        assert_eq!(instant.millis(), 0);
    }
}
