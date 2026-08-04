//! A configuration that is in force, and the calls admitted under it — §6 of
//! [`specs/host-config.md`](../../../../docs/specs/host-config.md).
//!
//! Two normative points live here rather than in the schema, because neither is a property of a
//! document on its own:
//!
//! - **N9/N10 — a reload applies wholly or not at all.** The new document is read and validated in
//!   full before anything moves, and the only things a reload may change are values this process
//!   already holds. Nothing that can fail at the operating system is reloadable, which is what
//!   makes "atomic" true rather than aspirational.
//! - **N11 — a live call keeps the policy it was admitted with.** A call captures its app's
//!   failure semantics, grants and binding at admission and carries them to its end. A reload is
//!   invisible to it, and cannot end it.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::harness::policy::FailurePolicy;

use super::{AppBinding, ConfigError, Grants, HostConfig, Route};

/// Everything a live call needs to know about its app, captured when the call was admitted.
///
/// A copy rather than a reference into the running configuration, and that is the whole point: a
/// reload replaces the configuration, and a call holding one of these does not notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPolicy {
    /// Which app.
    pub app: String,
    /// How to reach it.
    pub binding: AppBinding,
    /// The contract's §9.2 declaration, as it stood at admission.
    pub failure: FailurePolicy,
    /// What the host will do on its behalf, as it stood at admission.
    pub grants: Grants,
}

/// What happened to a call arriving on a listener. Total (N6): there is no fourth answer in which
/// nothing is decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// Admitted to this app, under this policy.
    App(Box<AppPolicy>),
    /// Refused with this status, as the listener declares.
    Refuse(u16),
    /// No listener of that name — a host bug rather than a call's problem, and never silence.
    NoSuchListener,
    /// A session listener: apps connect there, calls do not arrive there.
    NotACallListener,
}

/// The configuration in force, and the calls running under it.
#[derive(Debug)]
pub struct Running {
    current: HostConfig,
    live: BTreeMap<String, LiveCall>,
    completed: Arc<Completed>,
    next_generation: u128,
}

impl Clone for Running {
    fn clone(&self) -> Self {
        let completed = self
            .completed
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let live = self
            .live
            .iter()
            .filter(|(call, live)| !completed.contains(&((*call).clone(), live.generation)))
            .map(|(call, live)| (call.clone(), live.clone()))
            .collect();
        Self {
            current: self.current.clone(),
            live,
            completed: Arc::new(Completed {
                calls: Mutex::new(BTreeSet::new()),
                changed: Notify::new(),
            }),
            next_generation: self.next_generation,
        }
    }
}

#[derive(Debug, Clone)]
struct LiveCall {
    policy: AppPolicy,
    generation: u128,
}

#[derive(Debug)]
struct Completed {
    calls: Mutex<BTreeSet<(String, u128)>>,
    changed: Notify,
}

#[derive(Debug)]
pub(crate) struct ActorCompletion {
    call_id: String,
    generation: u128,
}

impl ActorCompletion {
    #[cfg(test)]
    pub(crate) fn call_id(&self) -> &str {
        &self.call_id
    }
}

#[derive(Debug)]
pub(crate) struct CompletionLease {
    completed: Arc<Completed>,
    call_id: String,
    generation: u128,
}

impl CompletionLease {
    pub(crate) fn completion(&self) -> ActorCompletion {
        ActorCompletion {
            call_id: self.call_id.clone(),
            generation: self.generation,
        }
    }
}

impl Drop for CompletionLease {
    fn drop(&mut self) {
        self.completed
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((self.call_id.clone(), self.generation));
        self.completed.changed.notify_waiters();
    }
}

impl Running {
    /// Put a configuration in force, with no calls yet.
    #[must_use]
    pub fn start(config: HostConfig) -> Self {
        Self {
            current: config,
            live: BTreeMap::new(),
            completed: Arc::new(Completed {
                calls: Mutex::new(BTreeSet::new()),
                changed: Notify::new(),
            }),
            next_generation: 1,
        }
    }

    /// The configuration new calls are admitted under.
    #[must_use]
    pub fn current(&self) -> &HostConfig {
        &self.current
    }

    /// Admit a call arriving on a listener, capturing the policy it will keep (N11).
    pub fn admit(&mut self, call: &str, listener: &str) -> Admission {
        self.reconcile();
        let Some(listener) = self.current.listener(listener) else {
            return Admission::NoSuchListener;
        };
        let app = match &listener.route {
            Route::App(app) => app.clone(),
            Route::Refuse(status) => return Admission::Refuse(*status),
            Route::Sessions => return Admission::NotACallListener,
        };
        // Referential integrity is a load-time invariant (N6), so a route naming an app that is not
        // there cannot be reached — but a refusal is still the honest answer if it ever were.
        let Some(config) = self.current.app(&app) else {
            return Admission::NoSuchListener;
        };

        let policy = AppPolicy {
            app,
            binding: config.binding.clone(),
            failure: config.failure.clone(),
            grants: config.grants.clone(),
        };
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.live.insert(
            call.to_owned(),
            LiveCall {
                policy: policy.clone(),
                generation,
            },
        );
        Admission::App(Box::new(policy))
    }

    /// Capture one app's policy for a session-originated call before asynchronous dialing.
    #[must_use]
    pub(crate) fn app_policy(&self, app: &str) -> Option<AppPolicy> {
        let config = self.current.app(app)?;
        Some(AppPolicy {
            app: app.to_owned(),
            binding: config.binding.clone(),
            failure: config.failure.clone(),
            grants: config.grants.clone(),
        })
    }

    /// Record an outbound call under the policy captured when originate was accepted.
    pub(crate) fn admit_outbound(&mut self, call: &str, policy: AppPolicy) {
        self.reconcile();
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        self.live
            .insert(call.to_owned(), LiveCall { policy, generation });
    }

    /// The policy a live call is running under.
    #[must_use]
    pub fn policy_of(&self, call: &str) -> Option<&AppPolicy> {
        let live = self.live.get(call)?;
        if self.is_completed(call, live.generation) {
            return None;
        }
        Some(&live.policy)
    }

    /// A call ended: its captured policy goes with it.
    pub fn end(&mut self, call: &str) {
        self.live.remove(call);
        self.completed
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(completed, _)| completed != call);
    }

    /// How many calls are live — an app removed by a reload is kept alive by these.
    #[must_use]
    pub fn live_calls(&self) -> usize {
        let completed = self
            .completed
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.live
            .iter()
            .filter(|(call, live)| !completed.contains(&((*call).clone(), live.generation)))
            .count()
    }

    /// Wait until no admitted call remains.
    ///
    /// The notification is shared with actor-owned admission leases, so one can wake a host whose
    /// serving future was cancelled while asynchronous SIP teardown continued.
    pub async fn wait_until_idle(&self) {
        loop {
            let changed = self.completed.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.live_calls() == 0 {
                return;
            }
            changed.await;
        }
    }

    /// Replace the running configuration with a new document, wholly or not at all.
    ///
    /// Live calls are untouched either way: on refusal because nothing happened, on acceptance
    /// because each of them is holding its own copy of what it was admitted with.
    ///
    /// # Errors
    /// The document's own refusal, or `topology-changed` if it moves a listener (N10). In both
    /// cases the configuration in force is exactly what it was.
    pub fn reload(&mut self, text: &str) -> Result<(), ConfigError> {
        self.reconcile();
        let next = HostConfig::parse(text)?;
        self.current.topology_allows(&next)?;
        self.current = next;
        Ok(())
    }

    pub(crate) fn completion_lease(&self, call_id: String) -> Option<CompletionLease> {
        let generation = self.live.get(&call_id)?.generation;
        Some(CompletionLease {
            completed: self.completed.clone(),
            call_id,
            generation,
        })
    }

    pub(crate) fn complete(&mut self, completion: ActorCompletion) {
        if self
            .live
            .get(&completion.call_id)
            .is_some_and(|live| live.generation == completion.generation)
        {
            self.live.remove(&completion.call_id);
        }
        self.completed
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(completion.call_id, completion.generation));
    }

    fn reconcile(&mut self) {
        let completed = {
            let mut completed = self
                .completed
                .calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *completed)
        };
        for (call, generation) in completed {
            if self
                .live
                .get(&call)
                .is_some_and(|live| live.generation == generation)
            {
                self.live.remove(&call);
            }
        }
    }

    fn is_completed(&self, call: &str, generation: u128) -> bool {
        self.completed
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(call.to_owned(), generation))
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

    const DOCUMENT: &str = r#"
[listener.edge]
protocol = "sip"
transport = "udp"
bind = "127.0.0.1:0"
app = "greeter"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
"#;

    #[test]
    fn an_old_completion_cannot_retire_a_reused_call_id() {
        let mut running = Running::start(HostConfig::parse(DOCUMENT).expect("valid document"));
        let _ = running.admit("same-id", "edge");
        let old = running
            .completion_lease("same-id".to_owned())
            .expect("the first admission has a generation");

        let _ = running.admit("same-id", "edge");
        let replacement = running
            .completion_lease("same-id".to_owned())
            .expect("the replacement has a generation");
        drop(old);

        let policy: Option<&AppPolicy> = running.policy_of("same-id");
        assert!(
            policy.is_some(),
            "the old generation cannot hide the replacement"
        );
        assert_eq!(running.live_calls(), 1);
        running.reconcile();
        assert!(running.policy_of("same-id").is_some());

        drop(replacement);
        assert!(running.policy_of("same-id").is_none());
        assert_eq!(running.live_calls(), 0);
    }

    #[test]
    fn a_clone_has_independent_completion_bookkeeping() {
        let mut running = Running::start(HostConfig::parse(DOCUMENT).expect("valid document"));
        let _ = running.admit("live", "edge");
        let lease = running
            .completion_lease("live".to_owned())
            .expect("the admission has a generation");
        let cloned = running.clone();

        drop(lease);
        assert!(running.policy_of("live").is_none());
        assert!(cloned.policy_of("live").is_some());
        assert_eq!(cloned.live_calls(), 1);
    }

    #[test]
    fn cloning_cannot_resurrect_a_completed_unreconciled_call() {
        let mut running = Running::start(HostConfig::parse(DOCUMENT).expect("valid document"));
        let _ = running.admit("ended", "edge");
        let lease = running
            .completion_lease("ended".to_owned())
            .expect("the admission has a generation");
        drop(lease);
        assert_eq!(running.live_calls(), 0);

        let cloned = running.clone();
        assert_eq!(cloned.live_calls(), 0);
        assert!(cloned.policy_of("ended").is_none());
    }
}
