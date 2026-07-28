//! Several registrations for one device, one per flow (RFC 5626 §4.2).
//!
//! The reason to register more than once is stated in §4.2: "a UA MUST send a REGISTER request to
//! each of the outbound proxies in the outbound-proxy-set", so that a proxy going away does not
//! take the user's reachability with it. That only works if the flows are genuinely independent,
//! which is a statement about *this* code rather than about the protocol: a set that reports one
//! `Result` for the whole batch cannot help but let one failure stand for all of them.
//!
//! So there is no aggregate `Result` here. [`Flows::register`] returns an outcome per flow, and
//! the failure of one is a fact about that flow — recorded, backed off according to §4.5, and
//! retried on its own schedule.

use std::time::Duration;

use sipx_transport::{Handle, Target};

use crate::agent::{Config, Flow, UserAgent};
use crate::error::{Error, Result};
use crate::outbound::{self, InstanceId, RegId};
use crate::registrar::Lease;

/// One flow's registration, and what has happened to it.
#[derive(Debug)]
struct Registered {
    agent: UserAgent,
    reg_id: RegId,
    /// Consecutive failures, which is the exponent in §4.5's backoff.
    failures: u32,
    /// The lease from the last success, if the flow is up.
    lease: Option<Lease>,
}

/// What one flow's registration attempt produced.
#[derive(Debug)]
pub struct Attempt {
    /// Which flow.
    pub reg_id: RegId,
    /// Whether the registrar reported an Outbound registration (RFC 5626 §6).
    pub flow_accepted: bool,
    /// The lease, or why there is none.
    pub outcome: Result<Lease>,
    /// How long to wait before retrying, when this attempt failed (RFC 5626 §4.5).
    ///
    /// `None` on success. Computed with the *whole set* in view, because §4.5's base interval
    /// depends on whether any flow is still up: 30 seconds when none is, 90 when one is.
    pub retry_after: Option<Duration>,
}

/// Every flow registered for one device.
///
/// The instance ID is shared — it identifies the device, not the flow — and each flow gets its own
/// `reg-id`, numbered from the order flows were added. §4.2 requires that numbering to be stable
/// across reboots, which is why it comes from position rather than from an allocator.
#[derive(Debug)]
pub struct Flows {
    instance: InstanceId,
    flows: Vec<Registered>,
}

impl Flows {
    /// An empty set for a device.
    ///
    /// The instance ID should be **loaded from storage, not generated here** on every start.
    /// §4.1 requires it to be persistent, and a UA that mints a fresh one each time accumulates
    /// dead bindings at the registrar and looks to it like a growing crowd of identical devices.
    #[must_use]
    pub fn for_instance(instance: InstanceId) -> Self {
        Self {
            instance,
            flows: Vec::new(),
        }
    }

    /// The device identity every flow in this set registers under.
    #[must_use]
    pub fn instance(&self) -> &InstanceId {
        &self.instance
    }

    /// Add a flow to an outbound proxy, taking the next `reg-id`.
    ///
    /// Returns the `reg-id` assigned, which the caller should persist alongside the proxy it
    /// belongs to: §4.2 wants the same number for the same flow after a restart.
    ///
    /// Fails only if the set has grown past `reg-id`'s range, which would take 2^31 proxies.
    pub fn add(&mut self, endpoint: Handle, config: Config, target: Target) -> Result<RegId> {
        let next = u32::try_from(self.flows.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .and_then(RegId::new)
            .ok_or(Error::TooManyFlows)?;
        let mut config = config;
        config.target = target;
        let config = config.with_outbound(Flow {
            instance: self.instance.clone(),
            reg_id: next,
        });
        self.flows.push(Registered {
            agent: UserAgent::new(endpoint, config),
            reg_id: next,
            failures: 0,
            lease: None,
        });
        Ok(next)
    }

    /// How many flows are currently registered.
    #[must_use]
    pub fn active(&self) -> usize {
        self.flows
            .iter()
            .filter(|flow| flow.lease.is_some())
            .count()
    }

    /// Whether any flow is up.
    ///
    /// This is the question §4.5's backoff turns on, and the reason a set is worth having: a UA
    /// with one working flow is reachable, and hurrying to re-establish the others only adds load
    /// to a registrar that is plainly having a bad day.
    #[must_use]
    pub fn any_active(&self) -> bool {
        self.active() > 0
    }

    /// The flows that are up, by `reg-id`.
    #[must_use]
    pub fn active_flows(&self) -> Vec<RegId> {
        self.flows
            .iter()
            .filter(|flow| flow.lease.is_some())
            .map(|flow| flow.reg_id)
            .collect()
    }

    /// Register every flow, and report what each one did.
    ///
    /// **One flow's failure is not the set's failure**, which is the entire point. Every flow is
    /// attempted regardless of what the others did, and each result is returned separately —
    /// there is deliberately no `Result` wrapping the whole call for a caller to `?` on.
    ///
    /// Sequential rather than concurrent: the flows share a device and a set of credentials, and a
    /// registrar that is going to challenge will challenge all of them. Registering in parallel
    /// turns one nonce into a race, and §3.4.3's `nc` counting is per nonce.
    pub async fn register(&mut self) -> Vec<Attempt> {
        let mut attempts = Vec::with_capacity(self.flows.len());
        for index in 0..self.flows.len() {
            let (reg_id, outcome, flow_accepted) = {
                let Some(flow) = self.flows.get_mut(index) else {
                    continue;
                };
                let outcome = flow.agent.register().await;
                if let Ok(lease) = &outcome {
                    flow.lease = Some(*lease);
                    flow.failures = 0;
                } else {
                    flow.lease = None;
                    flow.failures = flow.failures.saturating_add(1);
                }
                (flow.reg_id, outcome, flow.agent.flow_accepted())
            };
            // Computed after the assignment above, so `any_active` reflects this attempt as well.
            let retry_after = outcome.is_err().then(|| self.backoff_for(index));
            attempts.push(Attempt {
                reg_id,
                flow_accepted,
                outcome,
                retry_after,
            });
        }
        attempts
    }

    /// Keep every flow alive, and report which ones failed (RFC 5626 §4.4).
    ///
    /// A flow whose keep-alive fails is marked down and given a §4.5 retry delay; **the others are
    /// pinged and judged regardless**. That is the criterion this whole module exists for: the
    /// point of registering to several outbound proxies is that one of them going away is
    /// survivable, and a keep-alive pass that stopped at the first failure would throw that away
    /// at exactly the moment it mattered.
    ///
    /// Flows the registrar did not accept as Outbound are skipped: there is no flow, so there is
    /// nothing to keep alive, and pinging would be traffic nothing at the far end cares about.
    pub async fn keepalive(&mut self) -> Vec<Attempt> {
        let mut attempts = Vec::new();
        for index in 0..self.flows.len() {
            let (reg_id, flow_accepted, failed) = {
                let Some(flow) = self.flows.get_mut(index) else {
                    continue;
                };
                if flow.lease.is_none() || !flow.agent.flow_accepted() {
                    continue;
                }
                let result = flow.agent.keepalive().await;
                let failed = result.err();
                if failed.is_some() {
                    flow.lease = None;
                    flow.failures = flow.failures.saturating_add(1);
                }
                (flow.reg_id, true, failed)
            };
            let retry_after = failed.as_ref().map(|_| self.backoff_for(index));
            let outcome = match failed {
                Some(error) => Err(error),
                None => self
                    .flows
                    .get(index)
                    .and_then(|flow| flow.lease)
                    .ok_or(Error::NoResponse),
            };
            attempts.push(Attempt {
                reg_id,
                flow_accepted,
                outcome,
                retry_after,
            });
        }
        attempts
    }

    /// How long the flow at `index` should wait before its next attempt (RFC 5626 §4.5).
    ///
    /// The failure count has already been incremented by the caller, so one is taken off: §4.5's
    /// exponent is the number of failures *before* this one, and starting at 2^1 would double the
    /// very first wait.
    fn backoff_for(&self, index: usize) -> Duration {
        let failures = self
            .flows
            .get(index)
            .map_or(1, |flow| flow.failures.saturating_sub(1));
        outbound::recovery_delay(failures, self.any_active(), outbound::fraction())
    }

    /// The agent for one flow, for a caller that needs to send through it.
    #[must_use]
    pub fn flow(&self, reg_id: RegId) -> Option<&UserAgent> {
        self.flows
            .iter()
            .find(|flow| flow.reg_id == reg_id)
            .map(|flow| &flow.agent)
    }

    /// How long to wait before retrying a flow that has failed, per RFC 5626 §4.5.
    ///
    /// `None` for a flow that is up, or one this set does not have.
    #[must_use]
    pub fn retry_after(&self, reg_id: RegId) -> Option<Duration> {
        let flow = self.flows.iter().find(|flow| flow.reg_id == reg_id)?;
        (flow.lease.is_none() && flow.failures > 0).then(|| {
            outbound::recovery_delay(
                flow.failures.saturating_sub(1),
                self.any_active(),
                outbound::fraction(),
            )
        })
    }
}
