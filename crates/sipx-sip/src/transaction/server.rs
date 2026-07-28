//! Server transactions: RFC 3261 §17.2, amended by RFC 6026.

use std::time::Duration;

use crate::message::{Message, Method, Request, Response, StatusCode};
use crate::transaction::timing::{Timer, Timers};
use crate::transaction::{Output, Reason, Reliability, TuEvent};

/// The state of a server transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Non-INVITE: the request is with the transaction user and nothing has been sent.
    Trying,
    /// A provisional response has been sent, or an INVITE is being processed.
    Proceeding,
    /// A final response has been sent; waiting out request retransmissions, or an ACK.
    Completed,
    /// INVITE: the ACK arrived; waiting out its retransmissions.
    Confirmed,
    /// RFC 6026: a 2xx was sent and the ACK belongs to the transaction user.
    Accepted,
    /// Over.
    Terminated,
}

impl ServerState {
    /// Whether the transaction has finished and can be dropped.
    #[must_use]
    pub fn is_terminated(self) -> bool {
        matches!(self, Self::Terminated)
    }
}

/// A server transaction.
#[derive(Debug)]
pub struct ServerTransaction {
    request: Request,
    is_invite: bool,
    state: ServerState,
    reliability: Reliability,
    timers: Timers,
    /// The last response sent, for answering request retransmissions.
    last_response: Option<Response>,
    /// The current Timer G interval.
    interval: Duration,
}

impl ServerTransaction {
    /// Start a server transaction from a received request.
    ///
    /// The request is handed to the transaction user exactly once. Every later copy of it is
    /// answered by the transaction itself — which is the whole point of the layer, because a
    /// UDP peer that misses one response resends the request every T1, and an application
    /// that saw each copy would process the same REGISTER seven times.
    #[must_use]
    pub fn new(request: Request, reliability: Reliability, timers: Timers) -> (Self, Vec<Output>) {
        let is_invite = request.method == Method::Invite;
        let tx = Self {
            request: request.clone(),
            is_invite,
            state: if is_invite {
                ServerState::Proceeding
            } else {
                ServerState::Trying
            },
            reliability,
            timers,
            last_response: None,
            interval: timers.t1,
        };

        let mut out = vec![Output::to_tu(TuEvent::Request(Box::new(request)))];
        if is_invite {
            // RFC 3261 §17.2.1: if the TU has not answered within 200 ms, the transaction
            // sends 100 Trying itself, so the far end stops retransmitting while the
            // application thinks.
            out.push(Output::SetTimer {
                timer: Timer::Trying100,
                after: timers.trying_100(),
            });
        }
        (tx, out)
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> ServerState {
        self.state
    }

    /// The request that created this transaction.
    #[must_use]
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Feed in a request that matched this transaction.
    ///
    /// This is either a retransmission of the original or, for an INVITE, an ACK.
    pub fn on_request(&mut self, request: &Request) -> Vec<Output> {
        if request.method == Method::Ack {
            return self.on_ack(request);
        }
        match self.state {
            ServerState::Proceeding | ServerState::Completed => self
                .last_response
                .as_ref()
                .map(|r| Output::send(Message::Response(r.clone())))
                .into_iter()
                .collect(),
            // Absorbed silently, for two different reasons that happen to look the same. In
            // Trying the TU has not answered, so there is nothing to resend. In Confirmed and
            // Accepted a repeat is exactly what the absorption timers are there to swallow.
            // In every case the TU hears nothing, which is the point of the layer.
            _ => Vec::new(),
        }
    }

    fn on_ack(&mut self, ack: &Request) -> Vec<Output> {
        match self.state {
            ServerState::Completed => {
                // The ACK for a non-2xx is part of this transaction and stops here.
                self.state = ServerState::Confirmed;
                vec![
                    Output::ClearTimer(Timer::G),
                    Output::ClearTimer(Timer::H),
                    Output::SetTimer {
                        timer: Timer::I,
                        after: self.timers.absorb(self.reliability),
                    },
                ]
            }
            ServerState::Accepted => {
                // RFC 6026: the ACK for a 2xx is a separate transaction, so it goes up rather
                // than being swallowed. The transaction stays alive on Timer L only so that a
                // retransmitted 2xx does not create a second one.
                vec![Output::to_tu(TuEvent::Ack(Box::new(ack.clone())))]
            }
            // In Confirmed, retransmitted ACKs are exactly what Timer I is absorbing.
            _ => Vec::new(),
        }
    }

    /// The transaction user wants to send a response.
    pub fn on_tu_response(&mut self, response: Response) -> Vec<Output> {
        let status = response.status;
        match self.state {
            ServerState::Trying | ServerState::Proceeding => {
                let mut out = vec![Output::ClearTimer(Timer::Trying100)];
                self.last_response = Some(response.clone());
                out.push(Output::send(Message::Response(response)));

                if status.is_provisional() {
                    self.state = ServerState::Proceeding;
                    return out;
                }

                if self.is_invite {
                    if status.is_success() {
                        self.state = ServerState::Accepted;
                        out.push(Output::SetTimer {
                            timer: Timer::L,
                            after: self.timers.timeout(),
                        });
                    } else {
                        self.state = ServerState::Completed;
                        if !self.reliability.is_reliable() {
                            self.interval = self.timers.t1;
                            out.push(Output::SetTimer {
                                timer: Timer::G,
                                after: self.interval,
                            });
                        }
                        out.push(Output::SetTimer {
                            timer: Timer::H,
                            after: self.timers.timeout(),
                        });
                    }
                } else {
                    self.state = ServerState::Completed;
                    out.push(Output::SetTimer {
                        timer: Timer::J,
                        after: self.timers.timer_j(self.reliability),
                    });
                }
                out
            }
            ServerState::Accepted if status.is_success() => {
                // The TU retransmitting its own 2xx, which it must do until it sees an ACK.
                self.last_response = Some(response.clone());
                vec![Output::send(Message::Response(response))]
            }
            _ => Vec::new(),
        }
    }

    /// Feed a fired timer in.
    pub fn on_timer(&mut self, timer: Timer) -> Vec<Output> {
        match (self.state, timer) {
            (ServerState::Proceeding, Timer::Trying100) => {
                // Only if the TU really has not answered. The transaction emits a ClearTimer
                // when the TU responds, but a state machine that depends on its driver having
                // honoured a cancellation is one race away from sending a 100 Trying after a
                // 180 Ringing.
                if self.last_response.is_some() {
                    return Vec::new();
                }
                // The TU is still thinking. Answer 100 so the far end stops retransmitting.
                let Some(trying) = self.build_trying() else {
                    return Vec::new();
                };
                self.last_response = Some(trying.clone());
                vec![Output::send(Message::Response(trying))]
            }
            (ServerState::Completed, Timer::G) => {
                self.interval = self.timers.double_capped(self.interval);
                let mut out = Vec::new();
                if let Some(response) = &self.last_response {
                    out.push(Output::send(Message::Response(response.clone())));
                }
                out.push(Output::SetTimer {
                    timer: Timer::G,
                    after: self.interval,
                });
                out
            }
            (ServerState::Completed, Timer::H) => {
                // No ACK ever came. The far end is gone.
                self.state = ServerState::Terminated;
                vec![
                    Output::to_tu(TuEvent::Timeout),
                    Output::Terminated(Reason::Timeout),
                ]
            }
            (ServerState::Completed, Timer::J)
            | (ServerState::Confirmed, Timer::I)
            | (ServerState::Accepted, Timer::L) => {
                self.state = ServerState::Terminated;
                vec![Output::Terminated(Reason::Completed)]
            }
            _ => Vec::new(),
        }
    }

    /// The transport could not deliver a response.
    pub fn on_transport_error(&mut self) -> Vec<Output> {
        if self.state.is_terminated() {
            return Vec::new();
        }
        self.state = ServerState::Terminated;
        vec![
            Output::to_tu(TuEvent::TransportError),
            Output::Terminated(Reason::TransportError),
        ]
    }

    fn build_trying(&self) -> Option<Response> {
        let status = StatusCode::new(100)?;
        crate::build::ResponseBuilder::to_request(&self.request, status, "Trying")
            .ok()
            .map(crate::build::ResponseBuilder::build)
    }
}
