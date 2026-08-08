//! §9.2's declared failure semantics: what happens when the app fails.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §9.2 opens with the
//! design rule this module exists to hold: what happens when the app fails is **configuration
//! declared per app, never code**. So the interpreter has no branch anywhere that decides what a
//! timeout means — it looks the answer up here. The consequence is that a host can change the
//! behaviour without a release, and that the behaviour is testable without a failing app.
//!
//! The defaults are the section's own, and they encode a judgement worth restating: a flapping app
//! **degrades** a call it has already scripted, it does not kill it. `continue` keeps the program
//! running, because a call that is midway through a prompt the caller is listening to should not
//! be torn down by a webhook that missed a deadline. `on_4xx` is the exception, and the asymmetry
//! is deliberate: a `4xx` is the app saying *the request itself is wrong*, which will be just as
//! wrong on the next event, so continuing would only produce the same answer forever.

/// What to do when the app fails (§9.2's `continue · hangup · reject{status}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnFailure {
    /// Keep the program. Whatever was queued goes on running.
    #[default]
    Continue,
    /// End the call.
    Hangup,
    /// Refuse the invitation with a status. Only meaningful before the call is answered; after
    /// that the interpreter has nothing to refuse and hangs up instead.
    Reject {
        /// The status to refuse with.
        status: u16,
    },
}

/// How the app failed — the row of §9.2's table to look up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The callback did not return within `timeout_ms`.
    Timeout,
    /// The app answered `5xx`, or with something this binding could not read.
    ServerError,
    /// The app could not be reached at all.
    Unreachable,
    /// The app answered `4xx`: the request itself is wrong.
    ClientError,
}

/// One app's declared failure semantics (§9.2), plus the one other thing §6.5 leaves to host
/// configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// How long a callback may take before [`Failure::Timeout`] applies.
    pub timeout_ms: u32,
    /// Applied on [`Failure::Timeout`].
    pub on_timeout: OnFailure,
    /// Applied on [`Failure::ServerError`].
    pub on_5xx: OnFailure,
    /// Applied on [`Failure::Unreachable`].
    pub on_unreachable: OnFailure,
    /// Applied on [`Failure::ClientError`].
    pub on_4xx: OnFailure,
    /// §6.5: the header fields a `dial` may set, matched case-insensitively.
    ///
    /// **Empty by default, and that is the safe default rather than an oversight.** §6.5's
    /// reasoning is that the kernel's builders make header injection unrepresentable, and a free
    /// header map here would hand that property away — so a host that has not said which fields
    /// an app may set has said none, and a `dial` naming one is a document rejected whole (§6.4).
    pub dial_headers: Vec<String>,
}

impl Default for Policy {
    /// §9.2's own defaults: `timeout_ms: 2000`, `on_timeout`/`on_5xx`/`on_unreachable:
    /// continue`, `on_4xx: reject{500}`.
    fn default() -> Self {
        Self {
            timeout_ms: 2_000,
            on_timeout: OnFailure::Continue,
            on_5xx: OnFailure::Continue,
            on_unreachable: OnFailure::Continue,
            on_4xx: OnFailure::Reject { status: 500 },
            dial_headers: Vec::new(),
        }
    }
}

impl Policy {
    /// What this policy says to do about that failure.
    ///
    /// §9.2's table, and the only place in the crate that reads it — which is what makes "never
    /// code" checkable rather than aspirational.
    #[must_use]
    pub fn on(&self, failure: Failure) -> OnFailure {
        match failure {
            Failure::Timeout => self.on_timeout,
            Failure::ServerError => self.on_5xx,
            Failure::Unreachable => self.on_unreachable,
            Failure::ClientError => self.on_4xx,
        }
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

    /// §9.2's defaults, exactly as the section writes them.
    #[test]
    fn the_defaults_are_the_section_s_own() {
        let policy = Policy::default();
        assert_eq!(policy.timeout_ms, 2_000);
        assert_eq!(policy.on(Failure::Timeout), OnFailure::Continue);
        assert_eq!(policy.on(Failure::ServerError), OnFailure::Continue);
        assert_eq!(policy.on(Failure::Unreachable), OnFailure::Continue);
        assert_eq!(
            policy.on(Failure::ClientError),
            OnFailure::Reject { status: 500 }
        );
    }
}
