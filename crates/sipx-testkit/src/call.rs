//! A socket-free call-signalling harness for downstream tests.
//!
//! [`CallHarness`] drives two real SIP transaction layers over the same seeded [`Link`] used by
//! sipx's own tests. Time starts at a virtual zero and advances only when the test asks. The
//! harness covers signalling through the answered INVITE; media and a production endpoint remain
//! the responsibility of integration tests.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::error::BuildError;
use sipx_sip::transaction::{
    Dispatch, Output, Reliability, Timer, TransactionKey, TransactionLayer, TuEvent,
};
use sipx_sip::{HeaderName, Limits, Request, Response, StatusCode, parse_datagram};
use sipx_transport::timers::TimerQueue;
use thiserror::Error;

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

/// Two SIP transaction layers connected inside one process.
#[derive(Debug)]
pub struct CallHarness {
    now: Virtual,
    link: Link<Virtual>,
    caller: Stack,
    callee: Stack,
    caller_events: Vec<TuEvent>,
    callee_events: Vec<TuEvent>,
}

impl CallHarness {
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
        }
    }

    /// Start with a link that neither loses nor delays signalling.
    #[must_use]
    pub fn perfect() -> Self {
        Self::new(0, Faults::default())
    }

    /// Place a request from the calling side and deliver everything due now.
    pub fn place(&mut self, request: Request) -> Result<(), HarnessError> {
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
        self.now = self.now + by;
        let wire = self.caller.fire(self.now, &mut self.caller_events);
        self.send(Side::Left, wire);
        let wire = self.callee.fire(self.now, &mut self.callee_events);
        self.send(Side::Right, wire);
        self.pump();
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
