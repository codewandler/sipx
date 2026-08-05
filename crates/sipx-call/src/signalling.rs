//! Confirmed INVITE dialogs with no SDP or media session.
//!
//! This is the narrow UAS primitive used by finite signalling and interoperability workloads. It
//! is not a shortcut through the call invariants: the INVITE's 2xx is retransmitted until a valid
//! ACK, every request is checked against both dialog tags and Call-ID, remote sequence numbers only
//! advance, and BYE is a real transaction. What is absent is only offer/answer and the RTP socket.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::{CSeq, From as FromHeader, To};
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode};
use sipx_transport::{Handle, Incoming, Target};
use tokio::sync::mpsc;

use crate::dialog::Dialog;
use crate::error::{Error, Result};

const T1: Duration = Duration::from_millis(500);
const T2: Duration = Duration::from_secs(4);
const TIMER_H: Duration = Duration::from_secs(32);

/// One observable transition of an SDP-free confirmed dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignallingEvent {
    /// The ACK matched both dialog identity and the INVITE's sequence number.
    Acknowledged,
    /// A valid increasing BYE ended the dialog and received `200 OK`.
    RemoteBye,
    /// An ACK named the dialog but did not carry the INVITE's `CSeq` and method.
    InvalidAck,
    /// A request did not match both tags and the Call-ID. Non-ACK requests receive `481`.
    InvalidDialog,
    /// A request's `CSeq` was malformed, named another method or did not increase. It receives
    /// `400` or `500` as appropriate.
    InvalidCSeq,
    /// A matched request used a method this signalling-only dialog does not implement. It receives
    /// `405` with the narrow `Allow` set.
    Unsupported,
    /// Timer H expired before a valid ACK arrived.
    AckTimedOut,
    /// Sending a required dialog response failed because the endpoint stopped accepting it.
    TransportFailed,
}

/// A prepared response and dialog whose fallible validation ran before the INVITE was claimed.
pub(crate) struct Prepared {
    response: Response,
    dialog: Dialog,
    target: Target,
    invite_cseq: u32,
}

/// Build the bodyless 2xx and dialog without taking ownership of the invitation transaction.
pub(crate) fn prepare(
    endpoint: &Handle,
    incoming: &Incoming,
    tag: &str,
    contact: Bytes,
) -> Result<Prepared> {
    if !valid_tag(tag) {
        return Err(Error::InvalidDialogTag);
    }
    let Some(invite_cseq) = cseq(&incoming.request)
        .filter(|value| value.method == Method::Invite)
        .map(|value| value.sequence)
    else {
        return Err(Error::NoDialog);
    };
    let Some(dialog) = Dialog::from_request(&incoming.request, tag) else {
        return Err(Error::NoDialog);
    };
    let Some(to) = incoming.request.headers.value(&HeaderName::To) else {
        return Err(Error::NoDialog);
    };
    let to = format!("{};tag={tag}", String::from_utf8_lossy(&to));
    let status = StatusCode::new(200).ok_or_else(|| Error::Rejected {
        status: 200,
        reason: "invalid success status".to_owned(),
    })?;
    let response = ResponseBuilder::to_request(&incoming.request, status, "OK")?
        .set_header(&HeaderName::To, Bytes::from(to))?
        .header(HeaderName::Contact, contact)?
        .build();
    let target =
        crate::call::in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));
    // `endpoint` is intentionally part of the preparation signature: the response's Contact is
    // caller-selected, but all subsequent requests remain tied to this endpoint. Reading its
    // advertised address here would silently override that explicit Contact.
    let _ = endpoint;
    Ok(Prepared {
        response,
        dialog,
        target,
        invite_cseq,
    })
}

/// Send a prepared response and transfer the reserved inbox into a confirmed signalling call.
pub(crate) async fn establish(
    endpoint: Handle,
    incoming: Incoming,
    requests: mpsc::Receiver<Incoming>,
    prepared: Prepared,
) -> Result<SignallingCall> {
    endpoint
        .respond(&incoming.key, prepared.response.clone())
        .await?;
    let now = tokio::time::Instant::now();
    let retransmission = Retransmission {
        key: incoming.key,
        response: prepared.response,
        interval: T1,
        next: now + T1,
        deadline: now + TIMER_H,
    };
    Ok(SignallingCall {
        endpoint,
        dialog: prepared.dialog,
        target: prepared.target,
        requests,
        invite_cseq: prepared.invite_cseq,
        acknowledged: false,
        ended: false,
        retransmission: Some(retransmission),
        last_request_elapsed: None,
        last_response_status: None,
    })
}

#[derive(Debug)]
struct Retransmission {
    key: sipx_sip::transaction::TransactionKey,
    response: Response,
    interval: Duration,
    next: tokio::time::Instant,
    deadline: tokio::time::Instant,
}

enum SignallingInput {
    Request(Option<Box<Incoming>>),
    Retransmit,
}

/// One confirmed INVITE dialog without SDP or media ownership.
#[derive(Debug)]
pub struct SignallingCall {
    endpoint: Handle,
    dialog: Dialog,
    target: Target,
    requests: mpsc::Receiver<Incoming>,
    invite_cseq: u32,
    acknowledged: bool,
    ended: bool,
    retransmission: Option<Retransmission>,
    last_request_elapsed: Option<Duration>,
    last_response_status: Option<u16>,
}

impl SignallingCall {
    /// The confirmed dialog, for explicit dispatcher-route release and observation.
    #[must_use]
    pub fn dialog(&self) -> &Dialog {
        &self.dialog
    }

    /// Whether a valid ACK has stopped the INVITE final-response retransmission.
    #[must_use]
    pub const fn is_acknowledged(&self) -> bool {
        self.acknowledged
    }

    /// Whether either side has ended this local dialog.
    #[must_use]
    pub const fn is_ended(&self) -> bool {
        self.ended
    }

    /// Processing time from dequeuing the last routed request through its response handoff.
    ///
    /// This is responder-side service time, not end-to-end latency. It is `None` before any
    /// request has been handled and for timer-only events.
    #[must_use]
    pub const fn last_request_elapsed(&self) -> Option<Duration> {
        self.last_request_elapsed
    }

    /// Take the status successfully sent while producing the most recent event.
    pub fn take_response_status(&mut self) -> Option<u16> {
        self.last_response_status.take()
    }

    /// Drive one routed request or final-response timer outcome.
    ///
    /// Network-invalid input becomes a typed event and, when SIP defines one, a response. It never
    /// panics or escapes as an internal error.
    pub async fn next(&mut self) -> Option<SignallingEvent> {
        loop {
            if self.ended {
                return None;
            }
            let wake = self
                .retransmission
                .as_ref()
                .map(|state| state.next.min(state.deadline));
            let input = tokio::select! {
                incoming = self.requests.recv() => SignallingInput::Request(incoming.map(Box::new)),
                () = wait_until(wake), if wake.is_some() => SignallingInput::Retransmit,
            };
            match input {
                SignallingInput::Request(Some(incoming)) => {
                    let started = tokio::time::Instant::now();
                    let event = self.handle(*incoming).await;
                    self.last_request_elapsed = Some(started.elapsed());
                    return Some(event);
                }
                SignallingInput::Request(None) => {
                    self.stop();
                    return None;
                }
                SignallingInput::Retransmit => {
                    if let Some(event) = self.retransmit().await {
                        self.ended = true;
                        return Some(event);
                    }
                }
            }
        }
    }

    /// Originate BYE and require a final response within `within`.
    ///
    /// The duration bounds a failure; success is the observed final response rather than elapsed
    /// wall time.
    pub async fn hang_up(&mut self, within: Duration) -> Result<u16> {
        self.finish_retransmission();
        let cseq = self.dialog.next_cseq();
        let (local, remote) = self.dialog.local_and_remote();
        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Bye, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Bye)?
            .max_forwards(70);
        let bye = crate::call::add_routes(builder, &routes)?.build();
        let mut responses = self.endpoint.send(bye, self.target.clone()).await?;
        // Fixed duration bounds a failed teardown; the final response is the happens-before.
        let response = tokio::time::timeout(within, responses.final_response())
            .await
            .map_err(|_| Error::SignallingTeardownTimeout(within))?
            .ok_or(Error::SignallingTeardownTimeout(within))?;
        self.ended = true;
        if !response_matches_dialog(&response, &self.dialog, cseq) {
            return Err(Error::InvalidDialogResponse);
        }
        let status = response.status.code();
        if !response.status.is_success() {
            return Err(Error::Rejected {
                status,
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }
        Ok(status)
    }

    /// Stop owned retransmission work without sending BYE.
    ///
    /// Used only when another protocol outcome already ended the dialog or shutdown can no longer
    /// reach the peer. A live established dialog should prefer [`Self::hang_up`].
    pub fn stop(&mut self) {
        self.ended = true;
        self.finish_retransmission();
    }

    async fn handle(&mut self, incoming: Incoming) -> SignallingEvent {
        self.last_response_status = None;
        if !self.dialog.matches(&incoming.request) {
            if incoming.request.method != Method::Ack
                && respond(&self.endpoint, &incoming, 481, "Call Does Not Exist", None)
                    .await
                    .is_err()
            {
                return SignallingEvent::TransportFailed;
            }
            if incoming.request.method != Method::Ack {
                self.last_response_status = Some(481);
            }
            return SignallingEvent::InvalidDialog;
        }

        match incoming.request.method {
            Method::Ack => {
                let valid = cseq(&incoming.request).is_some_and(|value| {
                    value.method == Method::Ack && value.sequence == self.invite_cseq
                });
                if !valid {
                    return SignallingEvent::InvalidAck;
                }
                self.acknowledged = true;
                self.finish_retransmission();
                SignallingEvent::Acknowledged
            }
            Method::Bye => {
                let Some(sequence) = cseq(&incoming.request)
                    .filter(|value| value.method == Method::Bye)
                    .map(|value| value.sequence)
                else {
                    if respond(&self.endpoint, &incoming, 400, "Bad Request", None)
                        .await
                        .is_err()
                    {
                        return SignallingEvent::TransportFailed;
                    }
                    self.last_response_status = Some(400);
                    return SignallingEvent::InvalidCSeq;
                };
                if self
                    .dialog
                    .remote_cseq
                    .is_some_and(|previous| sequence <= previous)
                {
                    if respond(
                        &self.endpoint,
                        &incoming,
                        500,
                        "Server Internal Error",
                        None,
                    )
                    .await
                    .is_err()
                    {
                        return SignallingEvent::TransportFailed;
                    }
                    self.last_response_status = Some(500);
                    return SignallingEvent::InvalidCSeq;
                }
                self.dialog.record_remote_cseq(&incoming.request);
                self.finish_retransmission();
                self.ended = true;
                if respond(&self.endpoint, &incoming, 200, "OK", None)
                    .await
                    .is_err()
                {
                    SignallingEvent::TransportFailed
                } else {
                    self.last_response_status = Some(200);
                    SignallingEvent::RemoteBye
                }
            }
            _ => {
                if respond(
                    &self.endpoint,
                    &incoming,
                    405,
                    "Method Not Allowed",
                    Some((HeaderName::Allow, Bytes::from_static(b"ACK, BYE"))),
                )
                .await
                .is_err()
                {
                    SignallingEvent::TransportFailed
                } else {
                    self.last_response_status = Some(405);
                    SignallingEvent::Unsupported
                }
            }
        }
    }

    fn finish_retransmission(&mut self) {
        self.retransmission = None;
    }

    async fn retransmit(&mut self) -> Option<SignallingEvent> {
        let now = tokio::time::Instant::now();
        let state = self.retransmission.as_mut()?;
        if now >= state.deadline {
            self.retransmission = None;
            return Some(SignallingEvent::AckTimedOut);
        }
        if self
            .endpoint
            .respond(&state.key, state.response.clone())
            .await
            .is_err()
        {
            let event = if tokio::time::Instant::now() >= state.deadline {
                SignallingEvent::AckTimedOut
            } else {
                SignallingEvent::TransportFailed
            };
            self.retransmission = None;
            return Some(event);
        }
        state.interval = state.interval.saturating_mul(2).min(T2);
        state.next = tokio::time::Instant::now() + state.interval;
        None
    }
}

pub(crate) fn response_matches_dialog(response: &Response, dialog: &Dialog, sequence: u32) -> bool {
    let unique_required = [
        HeaderName::CallId,
        HeaderName::From,
        HeaderName::To,
        HeaderName::CSeq,
    ]
    .iter()
    .all(|name| response.headers.count(name) == 1);
    if !unique_required {
        return false;
    }
    let call_id_matches = response
        .headers
        .value(&HeaderName::CallId)
        .is_some_and(|value| value.as_ref() == dialog.id.call_id.as_slice());
    let from_matches = response
        .headers
        .typed::<FromHeader>()
        .and_then(std::result::Result::ok)
        .and_then(|value| value.tag().map(ToOwned::to_owned))
        .is_some_and(|tag| tag == dialog.id.local_tag);
    let to_matches = response
        .headers
        .typed::<To>()
        .and_then(std::result::Result::ok)
        .and_then(|value| value.tag().map(ToOwned::to_owned))
        .is_some_and(|tag| tag == dialog.id.remote_tag);
    let cseq_matches = response
        .headers
        .typed::<CSeq>()
        .and_then(std::result::Result::ok)
        .is_some_and(|value| value.method == Method::Bye && value.sequence == sequence);
    call_id_matches && from_matches && to_matches && cseq_matches
}

async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn respond(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    extra: Option<(HeaderName, Bytes)>,
) -> Result<()> {
    let status = StatusCode::new(status).ok_or_else(|| Error::Rejected {
        status,
        reason: "invalid response status".to_owned(),
    })?;
    let mut builder = ResponseBuilder::to_request(&incoming.request, status, reason)?;
    if let Some((name, value)) = extra {
        builder = builder.header(name, value)?;
    }
    endpoint.respond(&incoming.key, builder.build()).await?;
    Ok(())
}

fn cseq(request: &Request) -> Option<CSeq> {
    request
        .headers
        .typed::<CSeq>()
        .and_then(std::result::Result::ok)
}

fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 128
        && tag.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
                )
        })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use bytes::Bytes;
    use sipx_sip::{HeaderName, Message, Uri};

    use super::{response_matches_dialog, valid_tag};
    use crate::{Dialog, DialogId, Role};

    #[test]
    fn dialog_tags_are_bounded_sip_tokens() {
        assert!(valid_tag("t-0123456789abcdef"));
        assert!(valid_tag("all.!%*_+`'~tokens"));
        assert!(!valid_tag(""));
        assert!(!valid_tag("space is not a token"));
        assert!(!valid_tag(&"x".repeat(129)));
    }

    fn bye_response(call_id: &str, from_tag: &str, to_tag: &str, cseq: &str) -> sipx_sip::Response {
        let wire = format!(
            "SIP/2.0 200 OK\r\nCall-ID: {call_id}\r\n\
             From: <sip:local@load.invalid>;tag={from_tag}\r\n\
             To: <sip:remote@driver.invalid>;tag={to_tag}\r\n\
             CSeq: {cseq}\r\nContent-Length: 0\r\n\r\n"
        );
        match sipx_sip::parse_datagram(Bytes::from(wire), &sipx_sip::Limits::datagram())
            .expect("response parses")
        {
            Message::Response(response) => response,
            Message::Request(_) => panic!("a response"),
        }
    }

    #[test]
    fn observed_bye_response_requires_every_dialog_identifier_and_exact_cseq() {
        // Each mutation below leaves a syntactically valid final response and changes one identity
        // coordinate, so transaction matching alone cannot make this assertion pass.
        let dialog = Dialog {
            role: Role::Callee,
            id: DialogId {
                call_id: b"observed@load.invalid".to_vec(),
                local_tag: b"local".to_vec(),
                remote_tag: b"remote".to_vec(),
            },
            local_uri: "<sip:local@load.invalid>".to_owned(),
            remote_uri: "<sip:remote@driver.invalid>".to_owned(),
            remote_target: Uri::parse(Bytes::from_static(b"sip:remote@driver.invalid"))
                .expect("target URI"),
            local_cseq: 2,
            remote_cseq: Some(1),
            route_set: Vec::new(),
        };
        assert!(response_matches_dialog(
            &bye_response("observed@load.invalid", "local", "remote", "2 BYE"),
            &dialog,
            2
        ));
        for invalid in [
            bye_response("wrong@load.invalid", "local", "remote", "2 BYE"),
            bye_response("observed@load.invalid", "wrong", "remote", "2 BYE"),
            bye_response("observed@load.invalid", "local", "wrong", "2 BYE"),
            bye_response("observed@load.invalid", "local", "remote", "3 BYE"),
            bye_response("observed@load.invalid", "local", "remote", "2 INVITE"),
        ] {
            assert!(!response_matches_dialog(&invalid, &dialog, 2));
        }

        let valid = bye_response("observed@load.invalid", "local", "remote", "2 BYE");
        for name in [
            HeaderName::CallId,
            HeaderName::From,
            HeaderName::To,
            HeaderName::CSeq,
        ] {
            let mut duplicate = valid.clone();
            let value = duplicate
                .headers
                .value(&name)
                .expect("required response header")
                .into_owned();
            duplicate
                .headers
                .push(sipx_sip::Header::build(name, Bytes::from(value)).expect("duplicate header"));
            assert!(!response_matches_dialog(&duplicate, &dialog, 2));
        }
    }
}
