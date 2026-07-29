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

use std::collections::BTreeMap;

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
#[derive(Debug, Clone)]
pub struct Running {
    current: HostConfig,
    live: BTreeMap<String, AppPolicy>,
}

impl Running {
    /// Put a configuration in force, with no calls yet.
    #[must_use]
    pub fn start(config: HostConfig) -> Self {
        Self {
            current: config,
            live: BTreeMap::new(),
        }
    }

    /// The configuration new calls are admitted under.
    #[must_use]
    pub fn current(&self) -> &HostConfig {
        &self.current
    }

    /// Admit a call arriving on a listener, capturing the policy it will keep (N11).
    pub fn admit(&mut self, call: &str, listener: &str) -> Admission {
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
        self.live.insert(call.to_owned(), policy.clone());
        Admission::App(Box::new(policy))
    }

    /// The policy a live call is running under.
    #[must_use]
    pub fn policy_of(&self, call: &str) -> Option<&AppPolicy> {
        self.live.get(call)
    }

    /// A call ended: its captured policy goes with it.
    pub fn end(&mut self, call: &str) {
        self.live.remove(call);
    }

    /// How many calls are live — an app removed by a reload is kept alive by these.
    #[must_use]
    pub fn live_calls(&self) -> usize {
        self.live.len()
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
        let next = HostConfig::parse(text)?;
        self.current.topology_allows(&next)?;
        self.current = next;
        Ok(())
    }
}
