//! Confirmed INVITE dialogs with no SDP or media session.
//!
//! This is the narrow UAS primitive used by finite signalling and interoperability workloads. It
//! is not a shortcut through the call invariants: the INVITE's 2xx is retransmitted until a valid
//! ACK, every request is checked against both dialog tags and Call-ID, remote sequence numbers only
//! advance, and BYE is a real transaction. What is absent is only offer/answer and the RTP socket.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::CSeq;
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode};
use sipx_transport::{Handle, Incoming, Target};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

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
    let cancellation = CancellationToken::new();
    let (terminal_tx, terminal_rx) = oneshot::channel();
    let retransmission = tokio::spawn(retransmit_final(
        endpoint.clone(),
        incoming.key,
        prepared.response,
        cancellation.clone(),
        terminal_tx,
    ));
    Ok(SignallingCall {
        endpoint,
        dialog: prepared.dialog,
        target: prepared.target,
        requests,
        invite_cseq: prepared.invite_cseq,
        acknowledged: false,
        ended: false,
        cancellation,
        retransmission: Some(retransmission),
        retransmission_event: Some(terminal_rx),
        last_request_elapsed: None,
    })
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
    cancellation: CancellationToken,
    retransmission: Option<JoinHandle<()>>,
    retransmission_event: Option<oneshot::Receiver<SignallingEvent>>,
    last_request_elapsed: Option<Duration>,
}

enum SignallingInput {
    Request(Option<Box<Incoming>>),
    Retransmission(Option<SignallingEvent>),
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

    /// Drive one routed request or final-response timer outcome.
    ///
    /// Network-invalid input becomes a typed event and, when SIP defines one, a response. It never
    /// panics or escapes as an internal error.
    pub async fn next(&mut self) -> Option<SignallingEvent> {
        if self.ended {
            return None;
        }
        let input = tokio::select! {
            incoming = self.requests.recv() => {
                SignallingInput::Request(incoming.map(Box::new))
            },
            event = wait_retransmission(&mut self.retransmission_event), if !self.acknowledged => {
                SignallingInput::Retransmission(event)
            }
        };
        match input {
            SignallingInput::Request(Some(incoming)) => {
                let started = tokio::time::Instant::now();
                let event = self.handle(*incoming).await;
                self.last_request_elapsed = Some(started.elapsed());
                Some(event)
            }
            SignallingInput::Request(None) => {
                self.stop().await;
                None
            }
            SignallingInput::Retransmission(Some(event)) => {
                self.ended = true;
                self.finish_retransmission().await;
                Some(event)
            }
            // The sender closes without an event only when cancellation already owns completion.
            SignallingInput::Retransmission(None) => None,
        }
    }

    /// Originate BYE and require a final response within `within`.
    ///
    /// The duration bounds a failure; success is the observed final response rather than elapsed
    /// wall time.
    pub async fn hang_up(&mut self, within: Duration) -> Result<u16> {
        self.finish_retransmission().await;
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
    pub async fn stop(&mut self) {
        self.ended = true;
        self.finish_retransmission().await;
    }

    async fn handle(&mut self, incoming: Incoming) -> SignallingEvent {
        if !self.dialog.matches(&incoming.request) {
            if incoming.request.method != Method::Ack
                && respond(&self.endpoint, &incoming, 481, "Call Does Not Exist", None)
                    .await
                    .is_err()
            {
                return SignallingEvent::TransportFailed;
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
                self.finish_retransmission().await;
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
                    return SignallingEvent::InvalidCSeq;
                }
                self.dialog.record_remote_cseq(&incoming.request);
                self.finish_retransmission().await;
                self.ended = true;
                if respond(&self.endpoint, &incoming, 200, "OK", None)
                    .await
                    .is_err()
                {
                    SignallingEvent::TransportFailed
                } else {
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
                    SignallingEvent::Unsupported
                }
            }
        }
    }

    async fn finish_retransmission(&mut self) {
        self.cancellation.cancel();
        self.retransmission_event = None;
        if let Some(task) = self.retransmission.take() {
            // The cancellation token is the happens-before; awaiting the task proves no owned
            // retransmission remains.
            let _ = task.await;
        }
    }
}

impl Drop for SignallingCall {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.retransmission.take() {
            task.abort();
        }
    }
}

async fn wait_retransmission(
    receiver: &mut Option<oneshot::Receiver<SignallingEvent>>,
) -> Option<SignallingEvent> {
    match receiver {
        Some(receiver) => receiver.await.ok(),
        None => std::future::pending().await,
    }
}

async fn retransmit_final(
    endpoint: Handle,
    key: sipx_sip::transaction::TransactionKey,
    response: Response,
    cancellation: CancellationToken,
    terminal: oneshot::Sender<SignallingEvent>,
) {
    let mut interval = T1;
    let deadline = tokio::time::Instant::now() + TIMER_H;
    loop {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => return,
            () = tokio::time::sleep_until(deadline) => {
                let _ = terminal.send(SignallingEvent::AckTimedOut);
                return;
            }
            () = tokio::time::sleep(interval) => {}
        }
        if endpoint.respond(&key, response.clone()).await.is_err() {
            let _ = terminal.send(SignallingEvent::TransportFailed);
            return;
        }
        interval = interval.saturating_mul(2).min(T2);
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
    use super::valid_tag;

    #[test]
    fn dialog_tags_are_bounded_sip_tokens() {
        assert!(valid_tag("t-0123456789abcdef"));
        assert!(valid_tag("all.!%*_+`'~tokens"));
        assert!(!valid_tag(""));
        assert!(!valid_tag("space is not a token"));
        assert!(!valid_tag(&"x".repeat(129)));
    }
}
