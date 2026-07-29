//! Declared failure semantics (§9.2).
//!
//! Ground rule 3 of the [host design](../../../../docs/designs/app-host.md): what a slow, wrong or
//! absent app means for a live call is **configuration, never code**. So this is a value the
//! scenario carries, and every branch that consults it reads it from here — a code path that
//! hard-coded one of these answers is the defect the rule names.

use std::time::Duration;

/// What to do when the app fails (§9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnFailure {
    /// Keep the program that is already running.
    Continue,
    /// End the call.
    Hangup,
    /// Refuse it with a status.
    Reject {
        /// The status.
        status: u16,
    },
}

/// The per-app failure declaration, with §9.2's defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailurePolicy {
    /// How long a callback may take before it counts as failed.
    pub timeout: Duration,
    /// The callback did not return in time.
    pub on_timeout: OnFailure,
    /// The app answered 5xx — or sent a document §6.4 rejects, which is treated the same way.
    pub on_5xx: OnFailure,
    /// The app could not be reached at all.
    pub on_unreachable: OnFailure,
    /// The app says the request itself is wrong.
    pub on_4xx: OnFailure,
}

impl Default for FailurePolicy {
    /// §9.2's stated defaults: `timeout_ms: 2000`, `on_timeout/on_5xx/on_unreachable: continue`,
    /// `on_4xx: reject{500}`.
    ///
    /// The asymmetry is the interesting part and is deliberate: a *flapping* app degrades a call it
    /// has already scripted rather than killing it, but an app that says the request itself is
    /// wrong is not going to do better on a retry, so that one refuses.
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(2),
            on_timeout: OnFailure::Continue,
            on_5xx: OnFailure::Continue,
            on_unreachable: OnFailure::Continue,
            on_4xx: OnFailure::Reject { status: 500 },
        }
    }
}

impl FailurePolicy {
    /// The §9.2 defaults.
    #[must_use]
    pub fn declared() -> Self {
        Self::default()
    }

    /// With this callback timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// With this `on_timeout`.
    #[must_use]
    pub fn on_timeout(mut self, action: OnFailure) -> Self {
        self.on_timeout = action;
        self
    }

    /// With this `on_5xx`.
    #[must_use]
    pub fn on_5xx(mut self, action: OnFailure) -> Self {
        self.on_5xx = action;
        self
    }

    /// With this `on_unreachable`.
    #[must_use]
    pub fn on_unreachable(mut self, action: OnFailure) -> Self {
        self.on_unreachable = action;
        self
    }

    /// With this `on_4xx`.
    #[must_use]
    pub fn on_4xx(mut self, action: OnFailure) -> Self {
        self.on_4xx = action;
        self
    }

    /// The action declared for a given failure.
    #[must_use]
    pub fn action_for(&self, failure: Failure) -> &OnFailure {
        match failure {
            Failure::Timeout => &self.on_timeout,
            Failure::ServerError => &self.on_5xx,
            Failure::Unreachable => &self.on_unreachable,
            Failure::ClientError => &self.on_4xx,
        }
    }
}

/// How a callback failed — the four §9.2 knobs, as the thing that selects one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// It did not return in time.
    Timeout,
    /// It answered 5xx, or sent a document §6.4 rejects whole.
    ServerError,
    /// It could not be reached.
    Unreachable,
    /// It answered 4xx.
    ClientError,
}

impl Failure {
    /// The knob's name in the configuration, for assertions and messages.
    #[must_use]
    pub fn knob(self) -> &'static str {
        match self {
            Self::Timeout => "on_timeout",
            Self::ServerError => "on_5xx",
            Self::Unreachable => "on_unreachable",
            Self::ClientError => "on_4xx",
        }
    }

    /// Every failure a policy declares an answer for.
    ///
    /// Acceptance point 3 asks for a scenario per knob; this is what makes "per knob" enumerable
    /// rather than a list someone has to remember to extend.
    #[must_use]
    pub fn all() -> [Self; 4] {
        [
            Self::Timeout,
            Self::ServerError,
            Self::Unreachable,
            Self::ClientError,
        ]
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

    /// §9.2's defaults, verbatim. A drift here silently changes what a flapping app does to a live
    /// call, which is the one thing this section exists to make predictable.
    #[test]
    fn the_defaults_are_the_ones_the_spec_states() {
        let policy = FailurePolicy::declared();
        assert_eq!(policy.timeout, Duration::from_secs(2));
        assert_eq!(policy.on_timeout, OnFailure::Continue);
        assert_eq!(policy.on_5xx, OnFailure::Continue);
        assert_eq!(policy.on_unreachable, OnFailure::Continue);
        assert_eq!(policy.on_4xx, OnFailure::Reject { status: 500 });
    }

    #[test]
    fn every_failure_selects_its_own_knob() {
        let policy = FailurePolicy::declared()
            .on_timeout(OnFailure::Hangup)
            .on_5xx(OnFailure::Reject { status: 503 })
            .on_unreachable(OnFailure::Continue)
            .on_4xx(OnFailure::Hangup);

        assert_eq!(policy.action_for(Failure::Timeout), &OnFailure::Hangup);
        assert_eq!(
            policy.action_for(Failure::ServerError),
            &OnFailure::Reject { status: 503 }
        );
        assert_eq!(
            policy.action_for(Failure::Unreachable),
            &OnFailure::Continue
        );
        assert_eq!(policy.action_for(Failure::ClientError), &OnFailure::Hangup);
    }

    #[test]
    fn the_knobs_are_enumerable_so_a_scenario_per_knob_is_checkable() {
        let names: Vec<&str> = Failure::all().iter().map(|f| f.knob()).collect();
        assert_eq!(
            names,
            vec!["on_timeout", "on_5xx", "on_unreachable", "on_4xx"]
        );
    }
}
