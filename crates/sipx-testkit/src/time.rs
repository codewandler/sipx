//! Virtual time for deterministic test drivers.

use std::ops::Add;
use std::time::Duration;

/// Milliseconds since a harness began.
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
        Self(millis)
    }

    /// Milliseconds since the harness started.
    #[must_use]
    pub const fn millis(self) -> u64 {
        self.0
    }
}

impl Add<Duration> for Virtual {
    type Output = Self;

    fn add(self, after: Duration) -> Self {
        Self(
            self.0
                .saturating_add(u64::try_from(after.as_millis()).unwrap_or(u64::MAX)),
        )
    }
}
