//! Application-level and transaction-level call harnesses for downstream tests.
//!
//! [`CallHarness`] drives [`sipx_call::dial`] and [`sipx_call::answer`] through ordinary
//! [`sipx_transport::Handle`] values joined by an in-process signalling path. [`TransactionHarness`]
//! is the lower-level deterministic fault and virtual-time surface for transaction tests.

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::error::BuildError;
use sipx_sip::transaction::{
    Dispatch, Output, Reliability, Timer, TransactionKey, TransactionLayer, TuEvent,
};
use sipx_sip::{HeaderName, Limits, Request, Response, StatusCode, parse_datagram};
use sipx_transport::timers::TimerQueue;
use sipx_transport::{Handle, Incoming, Target, TransportKind};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::link::{Faults, Link, Side};
use crate::time::Virtual;

/// A call harness operation that could not be performed.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum HarnessError {
    /// The request cannot create a client transaction (for example, it is an ACK).
    #[error("the request has no client transaction key")]
    NoClientTransaction,
    /// No INVITE has reached the answering side yet.
    #[error("no invitation is waiting to be answered")]
    NoInvitation,
    /// An answer helper was asked to use a status outside SIP's status range.
    #[error("{0} is outside the SIP response status range")]
    InvalidStatus(u16),
    /// The response could not safely be built from the invitation.
    #[error(transparent)]
    Build(#[from] BuildError),
    /// The in-process endpoint closed before the exchange completed.
    #[error("the in-process endpoint closed")]
    EndpointClosed,
    /// The call framework rejected or could not establish the exchange.
    #[error(transparent)]
    Call(#[from] sipx_call::Error),
    /// The dial task ended without returning its call result.
    #[error("the dial task stopped before returning a call")]
    DialTask,
    /// The dial completed without ever delivering its invitation to the peer.
    #[error("the dial completed before its invitation reached the peer")]
    DialBeforeInvitation,
    /// The request following the answer was not the dialog's ACK.
    #[error("the established dialog did not deliver its ACK")]
    MissingAck,
    /// The signalling harness could not construct its transport boundary.
    #[error(transparent)]
    Transport(#[from] sipx_transport::Error),
}

#[derive(Debug)]
struct Stack {
    transactions: TransactionLayer,
    timers: TimerQueue<(TransactionKey, Timer), Virtual>,
}

impl Stack {
    fn new() -> Self {
        Self {
            transactions: TransactionLayer::new(sipx_sip::transaction::Timers::default()),
            timers: TimerQueue::new(),
        }
    }

    fn perform(
        &mut self,
        key: &TransactionKey,
        outputs: Vec<Output>,
        now: Virtual,
        events: &mut Vec<TuEvent>,
    ) -> Vec<Bytes> {
        let mut wire = Vec::new();
        for output in outputs {
            match output {
                Output::Send(message) => wire.push(message.to_bytes()),
                Output::SetTimer { timer, after } => {
                    self.timers.set((key.clone(), timer), now, after);
                }
                Output::ClearTimer(timer) => self.timers.clear(&(key.clone(), timer)),
                Output::ToTu(event) => events.push(*event),
                Output::Terminated(_) => self.timers.forget_matching(|(other, _)| other == key),
            }
        }
        wire
    }

    fn fire(&mut self, now: Virtual, events: &mut Vec<TuEvent>) -> Vec<Bytes> {
        let mut wire = Vec::new();
        for (key, timer) in self.timers.take_due(now) {
            let outputs = self.transactions.on_timer(&key, timer);
            wire.extend(self.perform(&key, outputs, now, events));
        }
        wire
    }

    fn receive(&mut self, bytes: Bytes, now: Virtual, events: &mut Vec<TuEvent>) -> Vec<Bytes> {
        let Ok(message) = parse_datagram(bytes, &Limits::datagram()) else {
            return Vec::new();
        };
        match self.transactions.receive(message, Reliability::Unreliable) {
            Dispatch::Created { key, outputs } | Dispatch::Matched { key, outputs } => {
                self.perform(&key, outputs, now, events)
            }
            Dispatch::Unmatched(_) => Vec::new(),
        }
    }
}

/// A call harness that exercises the application API over an in-process signalling path.
#[derive(Debug)]
pub struct CallHarness {
    caller: Handle,
    callee: Handle,
    callee_incoming: mpsc::Receiver<Incoming>,
}

/// One invitation whose dial task is waiting for an application answer.
#[derive(Debug)]
pub struct PendingCall<'a> {
    invitation: Incoming,
    dial: DialTask,
    callee: &'a Handle,
    callee_incoming: &'a mut mpsc::Receiver<Incoming>,
}

#[derive(Debug)]
struct DialTask(Option<JoinHandle<Result<sipx_call::Call, sipx_call::Error>>>);

impl DialTask {
    async fn finish(&mut self) -> Result<sipx_call::Call, HarnessError> {
        let Some(task) = self.0.as_mut() else {
            return Err(HarnessError::DialTask);
        };
        let result = task
            .await
            .map_err(|_| HarnessError::DialTask)?
            .map_err(HarnessError::from);
        self.0.take();
        result
    }
}

impl Drop for DialTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.as_ref() {
            task.abort();
        }
    }
}

/// Both application call objects after the 2xx and its ACK crossed the in-process path.
#[derive(Debug)]
pub struct EstablishedCall {
    /// The call returned by [`sipx_call::dial`].
    pub caller: sipx_call::Call,
    /// The call returned by [`sipx_call::answer`].
    pub callee: sipx_call::Call,
}

impl CallHarness {
    /// Create a harness with two ordinary transport handles and no signalling sockets.
    ///
    /// Call establishment still opens the RTP/RTCP ports owned by `sipx-call`; the in-process
    /// boundary applies to SIP signalling, not media.
    pub fn new() -> Result<Self, HarnessError> {
        let ((originating, _originating_incoming), (answering, answering_incoming)) =
            sipx_transport::in_process_pair(32)?;
        Ok(Self {
            caller: originating,
            callee: answering,
            callee_incoming: answering_incoming,
        })
    }

    /// Begin a real [`sipx_call::dial`] and return only this exchange's invitation.
    pub async fn dial(
        &mut self,
        to: sipx_sip::Uri,
        options: sipx_call::DialOptions,
    ) -> Result<PendingCall<'_>, HarnessError> {
        let endpoint = self.caller.clone();
        let target = Target::new(self.callee.local_addr(), TransportKind::Udp);
        let mut dial = DialTask(Some(tokio::spawn(async move {
            sipx_call::dial(&endpoint, target, &to, &options).await
        })));
        let invitation = tokio::select! {
            result = dial.finish() => {
                return match result {
                    Ok(_) => Err(HarnessError::DialBeforeInvitation),
                    Err(error) => Err(error),
                };
            }
            incoming = self.callee_incoming.recv() => {
                incoming.ok_or(HarnessError::EndpointClosed)?
            }
        };
        if invitation.request.method != sipx_sip::Method::Invite {
            return Err(HarnessError::NoInvitation);
        }
        Ok(PendingCall {
            invitation,
            dial,
            callee: &self.callee,
            callee_incoming: &mut self.callee_incoming,
        })
    }
}

impl PendingCall<'_> {
    /// The exact invitation associated with this pending call.
    #[must_use]
    pub const fn invitation(&self) -> &Incoming {
        &self.invitation
    }

    /// Answer through [`sipx_call::answer`] and wait until the matching ACK reaches the call.
    pub async fn answer(self, media_address: IpAddr) -> Result<EstablishedCall, HarnessError> {
        let PendingCall {
            invitation,
            mut dial,
            callee,
            callee_incoming,
        } = self;
        let answer = sipx_call::answer(callee, &invitation, media_address);
        let (caller, mut callee_call) = tokio::try_join!(dial.finish(), async {
            answer.await.map_err(HarnessError::from)
        })?;
        let ack = callee_incoming
            .recv()
            .await
            .ok_or(HarnessError::EndpointClosed)?;
        if ack.request.method != sipx_sip::Method::Ack || !callee_call.handle(&ack).await? {
            return Err(HarnessError::MissingAck);
        }
        Ok(EstablishedCall {
            caller,
            callee: callee_call,
        })
    }
}

/// Two SIP transaction layers connected through a seeded virtual-time link.
#[derive(Debug)]
pub struct TransactionHarness {
    now: Virtual,
    link: Link<Virtual>,
    caller: Stack,
    callee: Stack,
    caller_events: Vec<TuEvent>,
    callee_events: Vec<TuEvent>,
    caller_scope: usize,
    callee_scope: usize,
}

impl TransactionHarness {
    /// Start at virtual time zero with a reproducible faulty link.
    #[must_use]
    pub fn new(seed: u64, faults: Faults) -> Self {
        Self {
            now: Virtual::epoch(),
            link: Link::new(seed, faults),
            caller: Stack::new(),
            callee: Stack::new(),
            caller_events: Vec::new(),
            callee_events: Vec::new(),
            caller_scope: 0,
            callee_scope: 0,
        }
    }

    /// Start with a link that neither loses nor delays signalling.
    #[must_use]
    pub fn perfect() -> Self {
        Self::new(0, Faults::default())
    }

    /// Place a request from the calling side and deliver everything due now.
    pub fn place(&mut self, request: Request) -> Result<(), HarnessError> {
        self.caller_scope = self.caller_events.len();
        self.callee_scope = self.callee_events.len();
        let Some((key, outputs)) = self
            .caller
            .transactions
            .send_request(request, Reliability::Unreliable)
        else {
            return Err(HarnessError::NoClientTransaction);
        };
        let wire = self
            .caller
            .perform(&key, outputs, self.now, &mut self.caller_events);
        self.send(Side::Left, wire);
        self.pump();
        Ok(())
    }

    /// Answer the most recent invitation delivered to the callee.
    ///
    /// The harness supplies deterministic `To` and `Contact` values. Tests that need exact header
    /// policy can build a [`Response`] and use [`Self::answer_with`] instead.
    pub fn answer(
        &mut self,
        status: StatusCode,
        reason: impl Into<Bytes>,
    ) -> Result<(), HarnessError> {
        let Some(request) = self.invitation().cloned() else {
            return Err(HarnessError::NoInvitation);
        };
        let Some(to) = request.headers.value(&HeaderName::To) else {
            return Err(BuildError::MissingRequiredResponseHeader { header: "To" }.into());
        };
        let mut tagged_to = to.into_owned();
        tagged_to.extend_from_slice(b";tag=sipx-testkit");
        let contact = Bytes::from(format!("<{}>", request.uri));
        let response = ResponseBuilder::to_request(&request, status, reason)?
            .set_header(&HeaderName::To, Bytes::from(tagged_to))?
            .header(HeaderName::Contact, contact)?
            .build();
        self.answer_with(response)
    }

    /// Answer the pending invitation with `200 OK`.
    pub fn answer_ok(&mut self) -> Result<(), HarnessError> {
        let ok = StatusCode::new(200).ok_or(HarnessError::InvalidStatus(200))?;
        self.answer(ok, "OK")
    }

    /// Send an application-built response from the answering side.
    pub fn answer_with(&mut self, response: Response) -> Result<(), HarnessError> {
        let Some(request) = self.invitation() else {
            return Err(HarnessError::NoInvitation);
        };
        let Some(key) = TransactionKey::from_request(request) else {
            return Err(HarnessError::NoInvitation);
        };
        let outputs = self.callee.transactions.send_response(&key, response);
        let wire = self
            .callee
            .perform(&key, outputs, self.now, &mut self.callee_events);
        self.send(Side::Right, wire);
        self.pump();
        Ok(())
    }

    /// Move virtual time forward, fire transaction timers, and deliver packets now due.
    pub fn advance(&mut self, by: Duration) {
        let until = self.now + by;
        while self.now < until {
            let next = [
                self.link.next_arrival(),
                self.caller.timers.next_deadline(),
                self.callee.timers.next_deadline(),
                Some(until),
            ]
            .into_iter()
            .flatten()
            .filter(|instant| *instant >= self.now)
            .min()
            .unwrap_or(until);
            self.now = next;
            self.pump();
            let wire = self.caller.fire(self.now, &mut self.caller_events);
            self.send(Side::Left, wire);
            let wire = self.callee.fire(self.now, &mut self.callee_events);
            self.send(Side::Right, wire);
            self.pump();
            if self.now == until {
                break;
            }
        }
    }

    /// Current virtual time.
    #[must_use]
    pub const fn now(&self) -> Virtual {
        self.now
    }

    /// The most recently delivered INVITE, if one reached the callee.
    #[must_use]
    pub fn invitation(&self) -> Option<&Request> {
        self.callee_events
            .get(self.callee_scope..)
            .unwrap_or(&[])
            .iter()
            .rev()
            .find_map(|event| match event {
                TuEvent::Request(request) if request.method == sipx_sip::Method::Invite => {
                    Some(request.as_ref())
                }
                _ => None,
            })
    }

    /// The most recently delivered response, if one reached the caller.
    #[must_use]
    pub fn response(&self) -> Option<&Response> {
        self.caller_events
            .get(self.caller_scope..)
            .unwrap_or(&[])
            .iter()
            .rev()
            .find_map(|event| match event {
                TuEvent::Response(response) => Some(response.as_ref()),
                _ => None,
            })
    }

    /// How many datagrams the configured link discarded.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.link.dropped()
    }

    fn send(&mut self, from: Side, wire: Vec<Bytes>) {
        for bytes in wire {
            self.link.send(from, bytes, self.now);
        }
    }

    fn pump(&mut self) {
        loop {
            let deliveries = self.link.take_due(self.now);
            if deliveries.is_empty() {
                break;
            }
            for delivery in deliveries {
                let wire = match delivery.to {
                    Side::Left => {
                        self.caller
                            .receive(delivery.bytes, self.now, &mut self.caller_events)
                    }
                    Side::Right => {
                        self.callee
                            .receive(delivery.bytes, self.now, &mut self.callee_events)
                    }
                };
                self.send(delivery.to, wire);
            }
        }
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
    use std::future::{Future, poll_fn};
    use std::task::Poll;

    use tokio::sync::oneshot;

    use super::DialTask;

    struct OnDrop(Option<oneshot::Sender<()>>);

    impl Drop for OnDrop {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_a_pending_dial_aborts_its_owned_task() {
        let (started, running) = oneshot::channel();
        let (dropped, cancelled) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _on_drop = OnDrop(Some(dropped));
            let _ = started.send(());
            std::future::pending::<Result<sipx_call::Call, sipx_call::Error>>().await
        });
        let dial = DialTask(Some(task));
        running.await.expect("dial task started");

        drop(dial);

        cancelled.await.expect("dial task was cancelled");
    }

    #[tokio::test]
    async fn cancelling_finish_after_it_was_polled_still_aborts_the_dial() {
        let (started, running) = oneshot::channel();
        let (dropped, cancelled) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _on_drop = OnDrop(Some(dropped));
            let _ = started.send(());
            std::future::pending::<Result<sipx_call::Call, sipx_call::Error>>().await
        });
        let mut dial = DialTask(Some(task));
        running.await.expect("dial task started");
        let mut finish = Box::pin(dial.finish());
        poll_fn(|context| {
            assert!(matches!(finish.as_mut().poll(context), Poll::Pending));
            Poll::Ready(())
        })
        .await;

        drop(finish);
        drop(dial);

        cancelled.await.expect("polled dial task was cancelled");
    }
}
