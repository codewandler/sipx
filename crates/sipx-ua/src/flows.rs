//! Several registrations for one device, one per flow (RFC 5626 §4.2).
//!
//! The reason to register more than once is stated in §4.2: "a UA MUST send a REGISTER request to
//! each of the outbound proxies in the outbound-proxy-set", so that a proxy going away does not
//! take the user's reachability with it. That only works if the flows are genuinely independent,
//! which is a statement about *this* code rather than about the protocol: a set that reports one
//! `Result` for the whole batch cannot help but let one failure stand for all of them.
//!
//! [`Flows::register`] returns an outcome per flow rather than an aggregate `Result`. The lifetime
//! owner retains the same rule: one failure is an event about that stable `reg-id`, recorded,
//! backed off according to §4.5, and retried on its own schedule.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use sipx_transport::{Handle, Target};

use crate::agent::{Config, Flow, UserAgent};
use crate::error::{Error, Result};
use crate::outbound::{self, InstanceId, Keepalive, Power, RegId};
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

/// The default retained-flow and task bound.
pub const DEFAULT_MAX_FLOWS: usize = 8;

/// One observable transition from a driven Outbound flow.
///
/// The queue retaining these events has the same bound as the flow set. Slow observation can drop
/// progress records, reported by [`FlowCleanup::dropped_events`], but never delays protocol work or
/// cancellation.
#[derive(Debug)]
#[non_exhaustive]
pub enum FlowEvent {
    /// A flow acquired its first lease.
    Registered {
        /// The stable flow number.
        reg_id: RegId,
        /// The new lease generation.
        lease_generation: u64,
        /// The selected keep-alive, when the registrar accepted Outbound.
        keepalive: Option<Keepalive>,
        /// What the registrar granted.
        lease: Lease,
    },
    /// An existing flow replaced its lease.
    Refreshed {
        /// The stable flow number.
        reg_id: RegId,
        /// The new lease generation.
        lease_generation: u64,
        /// What the registrar granted.
        lease: Lease,
    },
    /// One keep-alive was answered on this flow.
    KeptAlive {
        /// The stable flow number.
        reg_id: RegId,
        /// The generation that fired.
        keepalive_generation: u64,
        /// The transport-selected technique.
        kind: Keepalive,
    },
    /// Registration, refresh, or keep-alive failed for this flow alone.
    Failed {
        /// The stable flow number.
        reg_id: RegId,
        /// The newly armed recovery generation.
        retry_generation: u64,
        /// Its independent backoff.
        retry_after: Duration,
        /// The typed cause. It contains no configured credentials.
        error: Error,
    },
}

/// What cancellation joined and released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct FlowCleanup {
    /// Flow tasks admitted under the configured bound.
    pub configured: usize,
    /// Flow tasks observed completing after endpoint shutdown.
    pub joined: usize,
    /// Active leases retained after cleanup. Always zero after a complete barrier.
    pub active: usize,
    /// Progress events dropped because the bounded observer queue was full or closed.
    pub dropped_events: usize,
}

#[derive(Debug)]
struct FlowExit;

/// A running, bounded Outbound registration lifetime.
///
/// Dropping requests cancellation. Call [`FlowLifetime::cancel`] to obtain the task-join and
/// endpoint-close barrier required before releasing application state.
#[derive(Debug)]
pub struct FlowLifetime {
    stop: tokio::sync::watch::Sender<bool>,
    events: tokio::sync::mpsc::Receiver<FlowEvent>,
    tasks: tokio::task::JoinSet<FlowExit>,
    configured: usize,
    active: Arc<AtomicUsize>,
    dropped_events: Arc<AtomicUsize>,
}

impl FlowLifetime {
    /// Receive the next retained flow transition.
    pub async fn next_event(&mut self) -> Option<FlowEvent> {
        self.events.recv().await
    }

    /// Cancel every generation, close every endpoint, and join every flow task.
    pub async fn cancel(mut self) -> FlowCleanup {
        let _ = self.stop.send(true);
        let mut joined = 0usize;
        while let Some(outcome) = self.tasks.join_next().await {
            if outcome.is_ok() {
                joined = joined.saturating_add(1);
            }
        }
        FlowCleanup {
            configured: self.configured,
            joined,
            active: self.active.load(Ordering::SeqCst),
            dropped_events: self.dropped_events.load(Ordering::Relaxed),
        }
    }
}

impl Drop for FlowLifetime {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

/// Every flow registered for one device.
///
/// The instance ID is shared — it identifies the device, not the flow — and each flow gets its own
/// `reg-id`, numbered from the order flows were added. §4.2 requires that numbering to be stable
/// across reboots, which is why it comes from position rather than from an allocator.
#[derive(Debug)]
pub struct Flows {
    instance: InstanceId,
    entries: Vec<Registered>,
    limit: usize,
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
            entries: Vec::new(),
            limit: DEFAULT_MAX_FLOWS,
        }
    }

    /// An empty set with an explicit retained-flow and task bound.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidFlowLimit`] when `max_flows` is zero.
    pub fn with_limit(instance: InstanceId, max_flows: usize) -> Result<Self> {
        if max_flows == 0 {
            return Err(Error::InvalidFlowLimit);
        }
        Ok(Self {
            instance,
            entries: Vec::new(),
            limit: max_flows,
        })
    }

    /// Continue a lease already acquired by a candidate-selection pass.
    ///
    /// This is the one-entry command case: the accepted REGISTER is adopted rather than sent a
    /// second time merely to transfer ownership to the lifetime driver.
    pub fn continue_registered(max_flows: usize, agent: UserAgent, lease: Lease) -> Result<Self> {
        let identity = agent.outbound_identity().ok_or(Error::NotOutboundFlow)?;
        let mut flows = Self::with_limit(identity.instance, max_flows)?;
        flows.entries.push(Registered {
            agent,
            reg_id: identity.reg_id,
            failures: 0,
            lease: Some(lease),
        });
        Ok(flows)
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
    /// Fails if the configured retained-state bound is full, or if the set somehow grows past
    /// `reg-id`'s range.
    pub fn add(&mut self, endpoint: Handle, config: Config, target: Target) -> Result<RegId> {
        if self.entries.len() >= self.limit {
            return Err(Error::FlowLimitReached { limit: self.limit });
        }
        let next = u32::try_from(self.entries.len())
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
        self.entries.push(Registered {
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
        self.entries
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
        self.entries
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
        let mut attempts = Vec::with_capacity(self.entries.len());
        for index in 0..self.entries.len() {
            let (reg_id, outcome, flow_accepted) = {
                let Some(flow) = self.entries.get_mut(index) else {
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
        for index in 0..self.entries.len() {
            let (reg_id, flow_accepted, failed) = {
                let Some(flow) = self.entries.get_mut(index) else {
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
                    .entries
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
            .entries
            .get(index)
            .map_or(1, |flow| flow.failures.saturating_sub(1));
        outbound::recovery_delay(failures, self.any_active(), outbound::fraction())
    }

    /// The agent for one flow, for a caller that needs to send through it.
    #[must_use]
    pub fn flow(&self, reg_id: RegId) -> Option<&UserAgent> {
        self.entries
            .iter()
            .find(|flow| flow.reg_id == reg_id)
            .map(|flow| &flow.agent)
    }

    /// How long to wait before retrying a flow that has failed, per RFC 5626 §4.5.
    ///
    /// `None` for a flow that is up, or one this set does not have.
    #[must_use]
    pub fn retry_after(&self, reg_id: RegId) -> Option<Duration> {
        let flow = self.entries.iter().find(|flow| flow.reg_id == reg_id)?;
        (flow.lease.is_none() && flow.failures > 0).then(|| {
            outbound::recovery_delay(
                flow.failures.saturating_sub(1),
                self.any_active(),
                outbound::fraction(),
            )
        })
    }

    /// Consume this configured set and drive every flow until cancellation.
    ///
    /// `attempt_limit` bounds each REGISTER exchange. `None` retains the transaction layer's own
    /// finite timer schedule. The event queue, task set, flow state and cleanup rows are all bounded
    /// by the same configured maximum.
    pub fn start(self, power: Power, attempt_limit: Option<Duration>) -> Result<FlowLifetime> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| Error::RuntimeUnavailable)?;
        let configured = self.entries.len();
        let (stop, _) = tokio::sync::watch::channel(false);
        let (events, receiver) = tokio::sync::mpsc::channel(self.limit);
        let active = Arc::new(AtomicUsize::new(0));
        let dropped_events = Arc::new(AtomicUsize::new(0));
        let mut tasks = tokio::task::JoinSet::new();

        for flow in self.entries {
            tasks.spawn_on(
                drive_flow(
                    flow,
                    power,
                    attempt_limit,
                    stop.subscribe(),
                    events.clone(),
                    Arc::clone(&active),
                    Arc::clone(&dropped_events),
                ),
                &runtime,
            );
        }
        drop(events);

        Ok(FlowLifetime {
            stop,
            events: receiver,
            tasks,
            configured,
            active,
            dropped_events,
        })
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one task receives the complete bounded state it exclusively owns"
)]
async fn drive_flow(
    mut flow: Registered,
    power: Power,
    attempt_limit: Option<Duration>,
    mut stop: tokio::sync::watch::Receiver<bool>,
    events: tokio::sync::mpsc::Sender<FlowEvent>,
    active: Arc<AtomicUsize>,
    dropped_events: Arc<AtomicUsize>,
) -> FlowExit {
    let kind = outbound::keepalive_for(flow.agent.target().transport);
    let mut lease_generation = 0u64;
    let mut keepalive_generation = 0u64;
    let mut retry_generation = 0u64;
    let mut was_registered = false;
    let mut retry_at = tokio::time::Instant::now();
    let mut refresh_at = None;
    let mut keepalive_at = None;

    if let Some(lease) = flow.lease {
        active.fetch_add(1, Ordering::SeqCst);
        lease_generation = lease_generation.saturating_add(1);
        was_registered = true;
        refresh_at = Some(tokio::time::Instant::now() + lease.refresh_after);
        keepalive_at = flow
            .agent
            .keepalive_after(power)
            .map(|after| tokio::time::Instant::now() + after);
        retain_event(
            &events,
            &dropped_events,
            FlowEvent::Registered {
                reg_id: flow.reg_id,
                lease_generation,
                keepalive: flow.agent.flow_accepted().then_some(kind),
                lease,
            },
        );
    }

    loop {
        if flow.lease.is_none() {
            tokio::select! {
                biased;
                () = cancelled(&mut stop) => break,
                () = tokio::time::sleep_until(retry_at) => {}
            }
            let attempt = match attempt_limit {
                Some(limit) => {
                    tokio::select! {
                        biased;
                        () = cancelled(&mut stop) => break,
                        outcome = flow.agent.register_within(limit) => outcome,
                    }
                }
                None => {
                    tokio::select! {
                        biased;
                        () = cancelled(&mut stop) => break,
                        outcome = flow.agent.register() => outcome,
                    }
                }
            };
            match attempt {
                Ok(lease) => {
                    flow.failures = 0;
                    flow.lease = Some(lease);
                    active.fetch_add(1, Ordering::SeqCst);
                    lease_generation = lease_generation.saturating_add(1);
                    refresh_at = Some(tokio::time::Instant::now() + lease.refresh_after);
                    keepalive_at = flow
                        .agent
                        .keepalive_after(power)
                        .map(|after| tokio::time::Instant::now() + after);
                    let event = if was_registered {
                        FlowEvent::Refreshed {
                            reg_id: flow.reg_id,
                            lease_generation,
                            lease,
                        }
                    } else {
                        was_registered = true;
                        FlowEvent::Registered {
                            reg_id: flow.reg_id,
                            lease_generation,
                            keepalive: flow.agent.flow_accepted().then_some(kind),
                            lease,
                        }
                    };
                    retain_event(&events, &dropped_events, event);
                }
                Err(error) => {
                    flow.failures = flow.failures.saturating_add(1);
                    retry_generation = retry_generation.saturating_add(1);
                    let retry_after = outbound::recovery_delay(
                        flow.failures.saturating_sub(1),
                        active.load(Ordering::SeqCst) > 0,
                        outbound::fraction(),
                    );
                    retry_at = tokio::time::Instant::now() + retry_after;
                    retain_event(
                        &events,
                        &dropped_events,
                        FlowEvent::Failed {
                            reg_id: flow.reg_id,
                            retry_generation,
                            retry_after,
                            error,
                        },
                    );
                }
            }
            continue;
        }

        tokio::select! {
            biased;
            () = cancelled(&mut stop) => break,
            () = wait_until(refresh_at) => {
                let refreshed = match attempt_limit {
                    Some(limit) => {
                        tokio::select! {
                            biased;
                            () = cancelled(&mut stop) => break,
                            outcome = flow.agent.register_within(limit) => outcome,
                        }
                    }
                    None => {
                        tokio::select! {
                            biased;
                            () = cancelled(&mut stop) => break,
                            outcome = flow.agent.register() => outcome,
                        }
                    }
                };
                match refreshed {
                    Ok(lease) => {
                        flow.failures = 0;
                        flow.lease = Some(lease);
                        lease_generation = lease_generation.saturating_add(1);
                        refresh_at = Some(tokio::time::Instant::now() + lease.refresh_after);
                        retain_event(&events, &dropped_events, FlowEvent::Refreshed {
                            reg_id: flow.reg_id,
                            lease_generation,
                            lease,
                        });
                    }
                    Err(error) => fail_active_flow(
                        &mut flow,
                        &active,
                        &events,
                        &dropped_events,
                        &mut retry_generation,
                        &mut retry_at,
                        error,
                    ),
                }
            }
            () = wait_until(keepalive_at) => {
                let kept_alive = tokio::select! {
                    biased;
                    () = cancelled(&mut stop) => break,
                    outcome = flow.agent.keepalive() => outcome,
                };
                match kept_alive {
                    Ok(()) => {
                        keepalive_generation = keepalive_generation.saturating_add(1);
                        retain_event(&events, &dropped_events, FlowEvent::KeptAlive {
                            reg_id: flow.reg_id,
                            keepalive_generation,
                            kind,
                        });
                        keepalive_at = flow
                            .agent
                            .keepalive_after(power)
                            .map(|after| tokio::time::Instant::now() + after);
                    }
                    Err(error) => fail_active_flow(
                        &mut flow,
                        &active,
                        &events,
                        &dropped_events,
                        &mut retry_generation,
                        &mut retry_at,
                        error,
                    ),
                }
            }
        }
    }

    if flow.lease.take().is_some() {
        active.fetch_sub(1, Ordering::SeqCst);
    }
    flow.agent.shutdown().await;
    FlowExit
}

fn fail_active_flow(
    flow: &mut Registered,
    active: &AtomicUsize,
    events: &tokio::sync::mpsc::Sender<FlowEvent>,
    dropped_events: &AtomicUsize,
    retry_generation: &mut u64,
    retry_at: &mut tokio::time::Instant,
    error: Error,
) {
    if flow.lease.take().is_some() {
        active.fetch_sub(1, Ordering::SeqCst);
    }
    flow.failures = flow.failures.saturating_add(1);
    *retry_generation = retry_generation.saturating_add(1);
    let retry_after = outbound::recovery_delay(
        flow.failures.saturating_sub(1),
        active.load(Ordering::SeqCst) > 0,
        outbound::fraction(),
    );
    *retry_at = tokio::time::Instant::now() + retry_after;
    retain_event(
        events,
        dropped_events,
        FlowEvent::Failed {
            reg_id: flow.reg_id,
            retry_generation: *retry_generation,
            retry_after,
            error,
        },
    );
}

fn retain_event(
    events: &tokio::sync::mpsc::Sender<FlowEvent>,
    dropped_events: &AtomicUsize,
    event: FlowEvent,
) {
    if events.try_send(event).is_err() {
        dropped_events.fetch_add(1, Ordering::Relaxed);
    }
}

async fn cancelled(stop: &mut tokio::sync::watch::Receiver<bool>) {
    if *stop.borrow() {
        return;
    }
    let _ = stop.changed().await;
}

async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}
