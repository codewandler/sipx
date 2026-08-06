//! Stable INFO progress emitted at typed command lifecycle transitions.

use std::time::Duration;

/// Which side of one diagnostic call owns the transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallRole {
    Dial,
    Answer,
}

impl CallRole {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Dial => "dial",
            Self::Answer => "answer",
        }
    }
}

/// The first terminal cause selected by a call command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallEnd {
    Remote,
    Duration,
    Interrupted,
    Refused(&'static str),
    Timeout,
    Failed,
}

impl CallEnd {
    /// The terminal result status owned by this cause.
    pub(crate) const fn status(self) -> &'static str {
        match self {
            Self::Remote | Self::Duration => "answered",
            Self::Interrupted => "interrupted",
            Self::Refused(status) => status,
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }

    /// The terminal call result's `ended_by` value.
    pub(crate) const fn ended_by(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Duration => "duration",
            Self::Interrupted => "interrupt",
            Self::Refused(_) => "refused",
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }

    const fn cause(self) -> &'static str {
        match self {
            Self::Remote => "remote",
            Self::Duration => "duration",
            Self::Interrupted => "interrupted",
            Self::Refused(_) => "refused",
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// One call command's first-cause progress owner.
///
/// Declare this before transport and call resources. Rust drops later declarations first, so the
/// fallback internal-failure record is emitted only after those resources have been dropped.
#[derive(Debug)]
pub(crate) struct Call {
    role: CallRole,
    peer: String,
    started: Option<std::time::Instant>,
    terminal: Option<CallEnd>,
}

impl Call {
    pub(crate) fn new(role: CallRole, peer: impl Into<String>) -> Self {
        Self {
            role,
            peer: peer.into(),
            started: None,
            terminal: None,
        }
    }

    pub(crate) fn waiting(&mut self, address: impl std::fmt::Display, within: Duration) {
        self.peer = address.to_string();
        self.started = Some(std::time::Instant::now());
        waiting(address, within);
    }

    pub(crate) fn placed(&mut self, transport: sipx_transport::TransportKind) {
        self.started = Some(std::time::Instant::now());
        placed(&self.peer, transport);
    }

    pub(crate) fn caller_observed(&mut self, caller: &str) {
        caller.clone_into(&mut self.peer);
        caller_observed(caller);
    }

    pub(crate) fn peer(&self) -> &str {
        &self.peer
    }

    pub(crate) fn answered(&self) {
        answered(self.role, &self.peer, self.elapsed());
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started.map_or(Duration::ZERO, |start| start.elapsed())
    }

    pub(crate) fn finish(&mut self, end: CallEnd) {
        if self.started.is_some() && self.terminal.is_none() {
            self.terminal = Some(end);
            ended(self.role, &self.peer, end, self.elapsed());
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.finish(CallEnd::Failed);
    }
}

fn waiting(address: impl std::fmt::Display, within: Duration) {
    tracing::info!(
        event = "call.waiting",
        role = CallRole::Answer.as_str(),
        address = %address,
        wait_ms = millis(within),
        "waiting for a call"
    );
}

fn placed(peer: &str, transport: sipx_transport::TransportKind) {
    tracing::info!(
        event = "call.placed",
        role = CallRole::Dial.as_str(),
        peer,
        transport = ?transport,
        "calling"
    );
}

fn caller_observed(caller: &str) {
    tracing::info!(
        event = "call.caller_observed",
        role = CallRole::Answer.as_str(),
        caller,
        "caller observed"
    );
}

fn answered(role: CallRole, peer: &str, setup: Duration) {
    tracing::info!(
        event = "call.answered",
        role = role.as_str(),
        peer,
        setup_ms = millis(setup),
        "answered"
    );
}

fn ended(role: CallRole, peer: &str, end: CallEnd, elapsed: Duration) {
    tracing::info!(
        event = "call.ended",
        role = role.as_str(),
        peer,
        status = end.status(),
        cause = end.cause(),
        elapsed_ms = millis(elapsed),
        "hung up"
    );
}

/// One bounded-load admission transition.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoadStart<'a> {
    pub(crate) target: &'a str,
    pub(crate) mode: &'a str,
    pub(crate) rate: f64,
    pub(crate) concurrency: usize,
    pub(crate) calls: Option<usize>,
    pub(crate) duration: Option<Duration>,
}

impl LoadStart<'_> {
    pub(crate) fn emit(self) {
        tracing::info!(
            event = "load.admission_started",
            target = self.target,
            mode = self.mode,
            rate = self.rate,
            concurrency = self.concurrency,
            calls = ?self.calls,
            duration_ms = ?self.duration.map(millis),
            "load admission started"
        );
    }
}

/// The aggregate facts shared with one `sipx.load.v1` terminal result.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LoadSummary<'a> {
    pub(crate) status: &'a str,
    pub(crate) attempted: usize,
    pub(crate) connected: usize,
    pub(crate) rejected: usize,
    pub(crate) timed_out: usize,
    pub(crate) failed: usize,
    pub(crate) peak_concurrency: usize,
}

impl LoadSummary<'_> {
    pub(crate) fn emit(self) {
        tracing::info!(
            event = "load.summary",
            status = self.status,
            attempted = self.attempted,
            connected = self.connected,
            rejected = self.rejected,
            timed_out = self.timed_out,
            failed = self.failed,
            peak_concurrency = self.peak_concurrency,
            "load summary"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_cause_owns_result_words() {
        assert_eq!(CallEnd::Remote.status(), "answered");
        assert_eq!(CallEnd::Remote.ended_by(), "remote");
        assert_eq!(CallEnd::Interrupted.status(), "interrupted");
        assert_eq!(CallEnd::Interrupted.ended_by(), "interrupt");
        assert_eq!(CallEnd::Refused("busy").status(), "busy");
        assert_eq!(CallEnd::Timeout.status(), "timeout");
        assert_eq!(CallEnd::Failed.status(), "failed");
    }

    #[test]
    fn first_terminal_cause_wins() {
        let mut call = Call::new(CallRole::Dial, "sip:test@example.com");
        call.placed(sipx_transport::TransportKind::Udp);
        call.finish(CallEnd::Remote);
        call.finish(CallEnd::Interrupted);
        assert_eq!(call.terminal, Some(CallEnd::Remote));
    }
}
