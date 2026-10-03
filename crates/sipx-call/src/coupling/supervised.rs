//! Application-owned supervision; cancellation observers never own the protocol driver.
use super::{Coupling, EarlyCoupling, Leg};
use crate::{Call, Calls, DialOptions, Invitation, MediaPolicy};
use sipx_sip::Uri;
use sipx_transport::Incoming;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Evidence for one owned leg. Local resource disposal alone is never terminal evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LegTermination {
    /// No outbound INVITE was created.
    NotStarted,
    /// A pending invitation was refused before a successful final response.
    Rejected(u16),
    /// The peer's BYE was accepted.
    RemoteBye,
    /// The exact originated BYE received a successful final response.
    Bye(u16),
    /// Invitation withdrawal observed its final and any crossed dialog's BYE acknowledgement.
    Cancelled,
    /// Teardown failed, timed out, or lacked sufficient peer evidence.
    Unknown,
}
/// Final observations after both owned legs and their local media have been disposed.
#[derive(Debug, Clone)]
pub struct CouplingTermination {
    /// Source leg evidence.
    pub one: LegTermination,
    /// Target leg evidence.
    pub two: LegTermination,
}
impl CouplingTermination {
    /// Whether both legs have positive terminal evidence rather than mere local disposal.
    #[must_use]
    pub const fn observed(&self) -> bool {
        !matches!(self.one, LegTermination::Unknown) && !matches!(self.two, LegTermination::Unknown)
    }
}
#[derive(Debug, Clone)]
struct State {
    one: LegTermination,
    two: LegTermination,
    confirmed: bool,
    terminal: Option<Arc<CouplingTermination>>,
}
/// Sticky local stop plus bounded observers, independent of the tracked owner future.
///
/// Drive exactly one `dial_supervised`/`run_supervised` future in your task registry. Keep that
/// owner alive until it returns. Dropping a `wait`/`stop` future cannot abort that owner. Aborting
/// the owner produces no observed report: its application must retain the resulting uncertainty.
#[derive(Debug, Clone)]
pub struct CouplingControl {
    stop: CancellationToken,
    state: watch::Sender<State>,
    claimed: Arc<AtomicBool>,
    pub(super) within: Duration,
}
impl CouplingControl {
    /// Set the allowance for each concurrently observed confirmed-leg BYE.
    #[must_use]
    pub fn new(within: Duration) -> Self {
        let (state, _) = watch::channel(State {
            one: LegTermination::Unknown,
            two: LegTermination::NotStarted,
            confirmed: false,
            terminal: None,
        });
        Self {
            stop: CancellationToken::new(),
            state,
            claimed: Arc::new(AtomicBool::new(false)),
            within,
        }
    }
    /// Request one sticky local stop. Repetition does not create another operation.
    pub fn request_stop(&self) {
        self.stop.cancel();
    }
    /// Whether both calls reached the supervised confirmed driver.
    #[must_use]
    pub fn is_confirmed(&self) -> bool {
        self.state.borrow().confirmed
    }
    /// Wait until the owner confirms both dialogs, or ends first. The allowance bounds waiting.
    pub async fn wait_confirmed(&self, within: Duration) -> bool {
        let mut state = self.state.subscribe();
        tokio::time::timeout(within, async {
            loop {
                if state.borrow().confirmed {
                    return true;
                }
                if state.borrow().terminal.is_some() {
                    return false;
                }
                if state.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false)
    }
    /// Observe completion within a caller-owned allowance. `None` means uncertainty, not absence.
    pub async fn wait(&self, within: Duration) -> Option<Arc<CouplingTermination>> {
        let mut state = self.state.subscribe();
        tokio::time::timeout(within, async {
            loop {
                if let Some(report) = state.borrow().terminal.clone() {
                    return Some(report);
                }
                if state.changed().await.is_err() {
                    return None;
                }
            }
        })
        .await
        .ok()
        .flatten()
    }
    /// Request stop and wait without taking ownership of the registry's driver future.
    pub async fn stop(&self, within: Duration) -> Option<Arc<CouplingTermination>> {
        self.request_stop();
        self.wait(within).await
    }
    pub(super) async fn cancelled(&self) {
        self.stop.cancelled().await;
    }
    pub(super) fn stopped(&self) -> bool {
        self.stop.is_cancelled()
    }
    pub(super) fn record(&self, leg: Leg, evidence: LegTermination) {
        self.state.send_modify(|state| match leg {
            Leg::One => state.one = evidence,
            Leg::Two => state.two = evidence,
        });
    }
    pub(super) fn cancellation(&self, cancellation: crate::InvitationCancellation) {
        self.record(
            Leg::Two,
            if cancellation.cleanup.completed() && cancellation.cleanup.final_response_observed() {
                LegTermination::Cancelled
            } else {
                LegTermination::Unknown
            },
        );
    }
    fn claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }
    fn complete(&self) -> CouplingTermination {
        let state = self.state.borrow();
        let report = CouplingTermination {
            one: state.one,
            two: state.two,
        };
        drop(state);
        self.state
            .send_modify(|state| state.terminal = Some(Arc::new(report.clone())));
        report
    }
}
impl EarlyCoupling {
    /// Drive a media-bridged coupling under one application-tracked owner from initial dialing
    /// through teardown. The control is a separately cancellable observer, never a spawned task.
    #[allow(clippy::too_many_arguments)]
    pub async fn dial_supervised(
        invitation: Invitation,
        calls: &Calls,
        endpoint: &sipx_transport::Handle,
        target: sipx_transport::Target,
        to: &Uri,
        options: &DialOptions,
        media_address: crate::MediaAddress,
        source_policy: MediaPolicy,
        control: &CouplingControl,
    ) -> CouplingTermination {
        if !control.claim() {
            return CouplingTermination {
                one: LegTermination::Unknown,
                two: LegTermination::Unknown,
            };
        }
        match Box::pin(Self::dial_owned(
            invitation,
            calls,
            endpoint,
            target,
            to,
            options,
            media_address,
            source_policy,
            Some(control.clone()),
        ))
        .await
        {
            Ok(early) => match Box::pin(early.confirmed()).await {
                Ok(confirmed) => {
                    let (mut coupling, one, two) = confirmed.into_parts();
                    coupling.bridge_media();
                    return Box::pin(coupling.drive_supervised(one, two, control)).await;
                }
                Err(error) => tracing::debug!(%error, "supervised early coupling ended"),
            },
            Err(error) => tracing::debug!(%error, "supervised coupling setup ended"),
        }
        control.complete()
    }
}
impl Coupling {
    /// Drive a confirmed owner and both inboxes with sticky, separately observed local stop.
    /// No task is spawned: the application retains this owner future in its task registry.
    pub async fn run_supervised(
        self,
        one: mpsc::Receiver<Incoming>,
        two: mpsc::Receiver<Incoming>,
        control: &CouplingControl,
    ) -> CouplingTermination {
        if !control.claim() {
            return CouplingTermination {
                one: LegTermination::Unknown,
                two: LegTermination::Unknown,
            };
        }
        Box::pin(self.drive_supervised(one, two, control)).await
    }
    async fn drive_supervised(
        mut self,
        mut one: mpsc::Receiver<Incoming>,
        mut two: mpsc::Receiver<Incoming>,
        control: &CouplingControl,
    ) -> CouplingTermination {
        self.supervision = Some(control.clone());
        control.state.send_modify(|state| {
            state.confirmed = true;
            state.two = LegTermination::Unknown;
        });
        tokio::select! {
            biased;
            () = control.cancelled() => {},
            result = self.run(&mut one,&mut two) => {
                if let Err(error) = result { tracing::debug!(%error, "supervised confirmed coupling ended"); }
            }
        }
        self.bridge.take();
        let (a, b) = tokio::join!(
            end_leg(&mut self.one, &mut one, control.within),
            end_leg(&mut self.two, &mut two, control.within)
        );
        if let Some(evidence) = a {
            control.record(Leg::One, evidence);
        }
        if let Some(evidence) = b {
            control.record(Leg::Two, evidence);
        }
        // The result becomes visible only after native media owners have joined.
        self.one.media().shutdown().await;
        self.two.media().shutdown().await;
        drop(self);
        control.complete()
    }
}
async fn end_leg(
    call: &mut Call,
    incoming: &mut mpsc::Receiver<Incoming>,
    within: Duration,
) -> Option<LegTermination> {
    if call.accepted_remote_bye() {
        return Some(LegTermination::RemoteBye);
    }
    if call.is_ended() {
        return None;
    }
    let observed = call.hang_up_observed_while_serving(incoming, within).await;
    Some(termination(call, &observed))
}

// All coupled cleanup paths share the same evidence rule. A peer's accepted BYE is independent
// of the response to our own BYE, and remains valid if an intermediate wait is cancelled.
pub(super) fn termination(call: &Call, observed: &crate::Result<u16>) -> LegTermination {
    if call.accepted_remote_bye() {
        LegTermination::RemoteBye
    } else {
        match observed {
            Ok(status @ 200..=299) => LegTermination::Bye(*status),
            _ => LegTermination::Unknown,
        }
    }
}
