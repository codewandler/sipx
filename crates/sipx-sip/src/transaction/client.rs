//! Client transactions: RFC 3261 §17.1, amended by RFC 6026.

use std::time::Duration;

use crate::message::{Header, Message, Method, Request, Response};
use crate::name::HeaderName;
use crate::transaction::timing::{Timer, Timers};
use crate::transaction::{Output, Reason, Reliability, TuEvent};

/// The state of a client transaction.
///
/// `Calling` belongs to the INVITE machine and `Trying` to the non-INVITE one; the rest are
/// shared. `Accepted` is RFC 6026's addition and exists so that a retransmitted 2xx — which
/// forking proxies produce as a matter of course — still has a transaction to arrive at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientState {
    /// INVITE: the request has gone out and nothing has come back.
    Calling,
    /// Non-INVITE: the request has gone out and nothing has come back.
    Trying,
    /// A provisional response has arrived.
    Proceeding,
    /// A final response has arrived; waiting out retransmissions.
    Completed,
    /// RFC 6026: a 2xx has arrived and more may follow.
    Accepted,
    /// Over.
    Terminated,
}

impl ClientState {
    /// Whether the transaction has finished and can be dropped.
    #[must_use]
    pub fn is_terminated(self) -> bool {
        matches!(self, Self::Terminated)
    }
}

/// A client transaction.
#[derive(Debug)]
pub struct ClientTransaction {
    request: Request,
    is_invite: bool,
    state: ClientState,
    reliability: Reliability,
    timers: Timers,
    /// The current retransmission interval — Timer A for INVITE, Timer E otherwise.
    interval: Duration,
    /// The ACK generated for a non-2xx final response, kept so a retransmitted response can
    /// be answered with the same ACK rather than a freshly built one.
    ack: Option<Request>,
}

impl ClientTransaction {
    /// Start a client transaction, returning the request to send and the timers to set.
    #[must_use]
    pub fn new(request: Request, reliability: Reliability, timers: Timers) -> (Self, Vec<Output>) {
        let is_invite = request.method == Method::Invite;
        let mut tx = Self {
            request,
            is_invite,
            state: if is_invite {
                ClientState::Calling
            } else {
                ClientState::Trying
            },
            reliability,
            timers,
            interval: timers.t1,
            ack: None,
        };

        let mut out = vec![Output::send(Message::Request(tx.request.clone()))];
        if !reliability.is_reliable() {
            out.push(Output::SetTimer {
                timer: if is_invite { Timer::A } else { Timer::E },
                after: tx.interval,
            });
        }
        out.push(Output::SetTimer {
            timer: if is_invite { Timer::B } else { Timer::F },
            after: timers.timeout(),
        });
        tx.state = if is_invite {
            ClientState::Calling
        } else {
            ClientState::Trying
        };
        (tx, out)
    }

    /// The current state.
    #[must_use]
    pub fn state(&self) -> ClientState {
        self.state
    }

    /// The request this transaction was created for.
    #[must_use]
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Feed a response in.
    pub fn on_response(&mut self, response: Response) -> Vec<Output> {
        if self.is_invite {
            self.invite_response(response)
        } else {
            self.non_invite_response(response)
        }
    }

    fn invite_response(&mut self, response: Response) -> Vec<Output> {
        let status = response.status;
        match self.state {
            ClientState::Calling | ClientState::Proceeding => {
                let from_calling = self.state == ClientState::Calling;
                let mut out = Vec::new();

                if status.is_provisional() {
                    if from_calling {
                        // Provisional means the far end is alive: stop retransmitting, but
                        // keep Timer B, because "alive" is not "answered".
                        out.push(Output::ClearTimer(Timer::A));
                    }
                    self.state = ClientState::Proceeding;
                    out.push(Output::to_tu(TuEvent::Response(Box::new(response))));
                    return out;
                }

                if from_calling {
                    out.push(Output::ClearTimer(Timer::A));
                }
                out.push(Output::ClearTimer(Timer::B));

                if status.is_success() {
                    // No ACK from here. The ACK for a 2xx is a separate transaction the TU
                    // must build, because only the TU knows the dialog's route set. Sending
                    // one here would use the wrong Request-URI and the wrong route.
                    self.state = ClientState::Accepted;
                    out.push(Output::to_tu(TuEvent::Response(Box::new(response))));
                    out.push(Output::SetTimer {
                        timer: Timer::M,
                        after: self.timers.timeout(),
                    });
                } else {
                    // The ACK for a non-2xx *is* part of this transaction and reuses its
                    // branch, so the far end matches it to the INVITE it is acknowledging.
                    let ack = make_ack(&self.request, &response);
                    out.push(Output::send(Message::Request(ack.clone())));
                    self.ack = Some(ack);
                    self.state = ClientState::Completed;
                    out.push(Output::to_tu(TuEvent::Response(Box::new(response))));
                    out.push(Output::SetTimer {
                        timer: Timer::D,
                        after: self.timers.timer_d(self.reliability),
                    });
                }
                out
            }
            ClientState::Completed => {
                // A retransmitted final response gets the same ACK again — and the TU is not
                // told, because it has already dealt with this response.
                self.ack
                    .as_ref()
                    .map(|ack| Output::send(Message::Request(ack.clone())))
                    .into_iter()
                    .collect()
            }
            ClientState::Accepted => {
                if status.is_success() {
                    // RFC 6026: another 2xx. A forking proxy produces these routinely, and
                    // each one is a distinct answered branch the TU has to know about.
                    vec![Output::to_tu(TuEvent::Response(Box::new(response)))]
                } else {
                    Vec::new()
                }
            }
            ClientState::Trying | ClientState::Terminated => Vec::new(),
        }
    }

    fn non_invite_response(&mut self, response: Response) -> Vec<Output> {
        match self.state {
            ClientState::Trying | ClientState::Proceeding => {
                if response.status.is_provisional() {
                    self.state = ClientState::Proceeding;
                    return vec![Output::to_tu(TuEvent::Response(Box::new(response)))];
                }
                self.state = ClientState::Completed;
                vec![
                    Output::ClearTimer(Timer::E),
                    Output::ClearTimer(Timer::F),
                    Output::to_tu(TuEvent::Response(Box::new(response))),
                    Output::SetTimer {
                        timer: Timer::K,
                        after: self.timers.absorb(self.reliability),
                    },
                ]
            }
            // Retransmissions in Completed are absorbed outright: the TU has its answer.
            _ => Vec::new(),
        }
    }

    /// Feed a fired timer in.
    pub fn on_timer(&mut self, timer: Timer) -> Vec<Output> {
        match (self.state, timer) {
            (ClientState::Calling, Timer::A) => {
                self.interval = self.timers.double(self.interval);
                vec![
                    Output::send(Message::Request(self.request.clone())),
                    Output::SetTimer {
                        timer: Timer::A,
                        after: self.interval,
                    },
                ]
            }
            (ClientState::Trying, Timer::E) => {
                self.interval = self.timers.double_capped(self.interval);
                vec![
                    Output::send(Message::Request(self.request.clone())),
                    Output::SetTimer {
                        timer: Timer::E,
                        after: self.interval,
                    },
                ]
            }
            (ClientState::Proceeding, Timer::E) if !self.is_invite => {
                // Once provisional responses are flowing the interval stops backing off and
                // sits at T2: the far end is clearly alive, so this is keep-alive rather than
                // recovery.
                self.interval = self.timers.t2;
                vec![
                    Output::send(Message::Request(self.request.clone())),
                    Output::SetTimer {
                        timer: Timer::E,
                        after: self.interval,
                    },
                ]
            }
            (ClientState::Calling | ClientState::Proceeding, Timer::B | Timer::F) => {
                self.state = ClientState::Terminated;
                vec![
                    Output::to_tu(TuEvent::Timeout),
                    Output::Terminated(Reason::Timeout),
                ]
            }
            (ClientState::Completed, Timer::D | Timer::K) | (ClientState::Accepted, Timer::M) => {
                self.state = ClientState::Terminated;
                vec![Output::Terminated(Reason::Completed)]
            }
            _ => Vec::new(),
        }
    }

    /// The transport could not deliver the request.
    pub fn on_transport_error(&mut self) -> Vec<Output> {
        if self.state.is_terminated() {
            return Vec::new();
        }
        self.state = ClientState::Terminated;
        vec![
            Output::to_tu(TuEvent::TransportError),
            Output::Terminated(Reason::TransportError),
        ]
    }
}

/// Build the ACK for a non-2xx final response (RFC 3261 §17.1.1.3).
///
/// It reuses the INVITE's `Via` — the same branch — because it is part of the same
/// transaction, takes `To` from the *response* so the tag the far end chose is echoed back,
/// and copies the `Route` set from the request so it follows the same path.
fn make_ack(request: &Request, response: &Response) -> Request {
    let mut ack = Request::new(Method::Ack, request.uri.clone());

    // Exactly one Via: the topmost one from the request.
    if let Some(via) = request.headers.get(&HeaderName::Via) {
        ack.headers.push(via.clone());
    }
    if let Some(from) = request.headers.get(&HeaderName::From) {
        ack.headers.push(from.clone());
    }
    // To comes from the response: it carries the tag the far end assigned, and an ACK without
    // it will not match anything at the far end.
    if let Some(to) = response.headers.get(&HeaderName::To) {
        ack.headers.push(to.clone());
    }
    if let Some(call_id) = request.headers.get(&HeaderName::CallId) {
        ack.headers.push(call_id.clone());
    }
    for route in request.headers.get_all(&HeaderName::Route) {
        ack.headers.push(route.clone());
    }

    // Same sequence number, method ACK.
    let sequence = request
        .headers
        .value(&HeaderName::CSeq)
        .and_then(|v| {
            let digits: Vec<u8> = v.iter().copied().take_while(u8::is_ascii_digit).collect();
            String::from_utf8(digits).ok()
        })
        .unwrap_or_default();
    let mut cseq = sequence.into_bytes();
    cseq.extend_from_slice(b" ACK");
    ack.headers.push(Header::new_unchecked(
        HeaderName::CSeq,
        bytes::Bytes::from(cseq),
    ));
    ack.headers.push(Header::new_unchecked(
        HeaderName::ContentLength,
        bytes::Bytes::from_static(b"0"),
    ));

    ack
}
