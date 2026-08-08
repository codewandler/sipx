//! One process-stop vocabulary for every long-running diagnostic command.

use std::sync::{Arc, Mutex, MutexGuard};

/// The first supported process-stop observation.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cause {
    Interrupt,
    #[cfg(unix)]
    Terminate,
    Failed(String),
}

/// A cloneable listener whose first observation remains available to terminal reporting.
#[derive(Debug, Clone, Default)]
pub(crate) struct Stop {
    cause: Arc<Mutex<Option<Cause>>>,
}

impl Stop {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Wait for one supported stop or a handler failure.
    ///
    /// The runtime retains installed process handlers after the receiving future is dropped.
    /// Therefore a repeated supported signal during bounded cleanup cannot restore the platform's
    /// immediate default action, race a second cleanup owner, or produce a duplicate report.
    pub(crate) async fn wait(&self) {
        let cause = wait_one().await;
        let mut state = self.state();
        if state.is_none() {
            *state = Some(cause);
        }
    }

    /// Stable terminal field for a successfully observed signal.
    pub(crate) fn signal(&self) -> Option<&'static str> {
        match self.state().as_ref() {
            Some(Cause::Interrupt) => Some("interrupt"),
            #[cfg(unix)]
            Some(Cause::Terminate) => Some("terminate"),
            Some(Cause::Failed(_)) | None => None,
        }
    }

    /// Handler setup/receive failure, if that was the first observation.
    pub(crate) fn failure(&self) -> Option<String> {
        match self.state().as_ref() {
            Some(Cause::Failed(message)) => Some(message.clone()),
            _ => None,
        }
    }

    fn state(&self) -> MutexGuard<'_, Option<Cause>> {
        match self.cause.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(unix)]
async fn wait_one() -> Cause {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => return Cause::Failed(format!("SIGTERM handler failed: {error}")),
        };
    tokio::select! {
        biased;
        interrupt = tokio::signal::ctrl_c() => match interrupt {
            Ok(()) => Cause::Interrupt,
            Err(error) => Cause::Failed(format!("SIGINT handler failed: {error}")),
        },
        terminated = terminate.recv() => match terminated {
            Some(()) => Cause::Terminate,
            None => Cause::Failed("SIGTERM handler closed".to_owned()),
        },
    }
}

#[cfg(not(unix))]
async fn wait_one() -> Cause {
    match tokio::signal::ctrl_c().await {
        Ok(()) => Cause::Interrupt,
        Err(error) => Cause::Failed(format!("interrupt handler failed: {error}")),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn first_stop_cause_is_stable() {
        let stop = Stop::new();
        *stop.state() = Some(Cause::Interrupt);
        {
            let mut state = stop.state();
            if state.is_none() {
                *state = Some(Cause::Failed("late".to_owned()));
            }
        }
        assert_eq!(stop.signal(), Some("interrupt"));
        assert_eq!(stop.failure(), None);
    }
}
