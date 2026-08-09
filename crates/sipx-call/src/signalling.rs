//! Confirmed INVITE dialogs with no SDP or media session.
//!
//! Both halves live here. [`SignallingCall`] is the UAS primitive used by finite signalling and
//! interoperability workloads; [`dial_signalling`] is the UAC one, so a caller can place the same
//! shape of call. Neither is a shortcut through the call invariants: the INVITE's 2xx is
//! retransmitted until a valid ACK and re-acknowledged for as long as it arrives, every request is
//! checked against both dialog tags and Call-ID, remote sequence numbers only advance, and BYE is a
//! real transaction. What is absent is only offer/answer and the RTP socket.
//!
//! That absence is the point of the caller half (`T-46`). A negotiated session that sends no audio
//! still exchanges SDP, still binds a port and still carries a jitter buffer, so measuring against
//! it isolates the cost of *carrying* audio rather than the cost of the stack that carries it. A
//! deployment that runs its media somewhere else needs the other number, and only a call that
//! offers no session at all can produce it.

use std::future::Future;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::{CSeq, From as FromHeader, To};
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};
use tokio::sync::mpsc;

use crate::call::Credentials;
use crate::dialog::Dialog;
use crate::error::{Error, InvitationCancellation, Result};

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
    // discard: the endpoint is a capability witness; preparation deliberately performs no I/O.
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
        deferred_remote_bye: false,
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
    /// A valid BYE already answered while the peer's earlier ACK was still in flight.
    deferred_remote_bye: bool,
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
            if self.acknowledged && self.deferred_remote_bye {
                self.deferred_remote_bye = false;
                self.ended = true;
                self.last_response_status = Some(200);
                return Some(SignallingEvent::RemoteBye);
            }
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
                    if let Some(event) = event {
                        return Some(event);
                    }
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
        let response = send_bye(&self.endpoint, &self.dialog, &self.target, cseq, within).await?;
        self.ended = true;
        bye_outcome(&response, &self.dialog, cseq)
    }

    /// Stop owned retransmission work without sending BYE.
    ///
    /// Used only when another protocol outcome already ended the dialog or shutdown can no longer
    /// reach the peer. A live established dialog should prefer [`Self::hang_up`].
    pub fn stop(&mut self) {
        self.ended = true;
        self.finish_retransmission();
    }

    async fn handle(&mut self, incoming: Incoming) -> Option<SignallingEvent> {
        self.last_response_status = None;
        if !self.dialog.matches(&incoming.request) {
            if incoming.request.method != Method::Ack
                && respond(&self.endpoint, &incoming, 481, "Call Does Not Exist", None)
                    .await
                    .is_err()
            {
                return Some(SignallingEvent::TransportFailed);
            }
            if incoming.request.method != Method::Ack {
                self.last_response_status = Some(481);
            }
            return Some(SignallingEvent::InvalidDialog);
        }

        match incoming.request.method {
            Method::Ack => {
                let valid = cseq(&incoming.request).is_some_and(|value| {
                    value.method == Method::Ack && value.sequence == self.invite_cseq
                });
                if !valid {
                    return Some(SignallingEvent::InvalidAck);
                }
                self.acknowledged = true;
                self.finish_retransmission();
                Some(SignallingEvent::Acknowledged)
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
                        return Some(SignallingEvent::TransportFailed);
                    }
                    self.last_response_status = Some(400);
                    return Some(SignallingEvent::InvalidCSeq);
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
                        return Some(SignallingEvent::TransportFailed);
                    }
                    self.last_response_status = Some(500);
                    return Some(SignallingEvent::InvalidCSeq);
                }
                self.dialog.record_remote_cseq(&incoming.request);
                if respond(&self.endpoint, &incoming, 200, "OK", None)
                    .await
                    .is_err()
                {
                    Some(SignallingEvent::TransportFailed)
                } else if self.acknowledged {
                    self.finish_retransmission();
                    self.ended = true;
                    self.last_response_status = Some(200);
                    Some(SignallingEvent::RemoteBye)
                } else {
                    self.deferred_remote_bye = true;
                    None
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
                    Some(SignallingEvent::TransportFailed)
                } else {
                    self.last_response_status = Some(405);
                    Some(SignallingEvent::Unsupported)
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

/// What one SDP-free INVITE presents itself as.
///
/// Supplied by the caller rather than generated inside the exchange, for a reason a load generator
/// meets immediately: RFC 3261 §8.2.2.2 makes a second INVITE carrying the `Call-ID`, `From` tag
/// and `CSeq` of one a server has already accepted a *merged request*, answered `482 Loop
/// Detected`. Two addresses of one name commonly reach one server, so a caller walking a candidate
/// list needs an identity per attempt and must be able to decide what each one is. [`Self::fresh`]
/// is for a caller with nothing to reproduce and one address to reach.
#[derive(Debug, Clone)]
pub struct SignallingIdentity {
    to: Bytes,
    from: Bytes,
    call_id: Bytes,
    contact: Bytes,
}

impl SignallingIdentity {
    /// Present exactly these four field values.
    ///
    /// Each is a complete header value and none is validated here — the request builder rejects a
    /// malformed one when [`dial_signalling`] assembles the INVITE. `from` carries the local tag;
    /// `contact` is where this side's in-dialog requests can arrive.
    #[must_use]
    pub fn new(
        to: impl Into<Bytes>,
        from: impl Into<Bytes>,
        call_id: impl Into<Bytes>,
        contact: impl Into<Bytes>,
    ) -> Self {
        Self {
            to: to.into(),
            from: from.into(),
            call_id: call_id.into(),
            contact: contact.into(),
        }
    }

    /// Derive a fresh identity for one call from the endpoint and the two addresses.
    ///
    /// The `Call-ID` and the local tag are random per call, which is what keeps two calls toward
    /// one server distinct, and the `Contact` names this endpoint's advertised address because that
    /// is where its in-dialog requests can be delivered.
    #[must_use]
    pub fn fresh(endpoint: &Handle, to: &Uri, from: &Uri) -> Self {
        let rendered = |uri: &Uri| String::from_utf8_lossy(&uri.to_bytes()).into_owned();
        let user = from
            .decoded_user()
            .map(|user| String::from_utf8_lossy(&user).into_owned());
        let contact = user.map_or_else(
            || format!("<sip:{}>", endpoint.advertised()),
            |user| format!("<sip:{user}@{}>", endpoint.advertised()),
        );
        Self {
            to: Bytes::from(format!("<{}>", rendered(to))),
            from: Bytes::from(format!("<{}>;tag={}", rendered(from), crate::call::token())),
            call_id: Bytes::from(crate::call::token()),
            contact: Bytes::from(contact),
        }
    }

    /// The `To` header value this call is presented to the far end with.
    #[must_use]
    pub const fn to(&self) -> &Bytes {
        &self.to
    }

    /// The `From` header value, tag included.
    #[must_use]
    pub const fn from(&self) -> &Bytes {
        &self.from
    }

    /// The `Call-ID` value.
    #[must_use]
    pub const fn call_id(&self) -> &Bytes {
        &self.call_id
    }

    /// The `Contact` header value in-dialog requests to this side should use.
    #[must_use]
    pub const fn contact(&self) -> &Bytes {
        &self.contact
    }
}

/// What an SDP-free INVITE carries beyond its identity, and how long it may take.
///
/// Deliberately not [`DialOptions`](crate::DialOptions): that one requires a media address, and the
/// whole claim of this path is that there is no media to address.
#[derive(Debug, Clone)]
pub struct SignallingDialOptions {
    identity: SignallingIdentity,
    credentials: Option<Credentials>,
    headers: Vec<sipx_sip::Header>,
    timeout: Option<Duration>,
    cancellation_timeout: Duration,
}

impl SignallingDialOptions {
    /// Place one call under this identity, with no credentials and no stated deadline.
    #[must_use]
    pub fn new(identity: SignallingIdentity) -> Self {
        Self {
            identity,
            credentials: None,
            headers: Vec::new(),
            timeout: None,
            // The same withdrawal budget `DialOptions` defaults to, for the same reason: a CANCEL
            // that cannot be completed must not hold the caller open indefinitely.
            cancellation_timeout: Duration::from_secs(2),
        }
    }

    /// Add a validated application-owned field to every INVITE attempt.
    #[must_use]
    pub fn with_header(mut self, header: sipx_sip::Header) -> Self {
        self.headers.push(header);
        self
    }

    /// Answer a digest challenge with these credentials (RFC 3261 §22).
    #[must_use]
    pub fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Give up on the INVITE after this long, withdrawing it before returning.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Bound the distinct invitation-withdrawal phase after timeout or caller cancellation.
    #[must_use]
    pub const fn with_cancellation_timeout(mut self, timeout: Duration) -> Self {
        self.cancellation_timeout = timeout;
        self
    }
}

/// One confirmed INVITE dialog this endpoint originated, with no SDP and no media session.
///
/// The caller's mirror of [`SignallingCall`]. It holds no per-dialog inbox: a caller that wants to
/// observe the far end's BYE routes it with a [`Dispatcher`](crate::Dispatcher), and one that only
/// wants to hold the dialog and end it needs [`Self::hang_up`].
#[derive(Debug)]
pub struct SignallingDial {
    endpoint: Handle,
    dialog: Dialog,
    target: Target,
    status: u16,
    setup: Duration,
    ended: bool,
}

impl SignallingDial {
    /// The confirmed dialog, for explicit dispatcher-route release and observation.
    #[must_use]
    pub const fn dialog(&self) -> &Dialog {
        &self.dialog
    }

    /// The final response that established this dialog.
    #[must_use]
    pub const fn status(&self) -> u16 {
        self.status
    }

    /// How long the INVITE took, from handing it to the endpoint until the ACK went out.
    ///
    /// Measured inside the exchange rather than around it, so it is the signalling round trip and
    /// nothing the caller did before or after it.
    #[must_use]
    pub const fn setup(&self) -> Duration {
        self.setup
    }

    /// Whether this side has already ended the dialog.
    #[must_use]
    pub const fn is_ended(&self) -> bool {
        self.ended
    }

    /// Originate BYE and require a final response within `within`.
    ///
    /// The duration bounds a failure; success is the observed final response rather than elapsed
    /// wall time.
    pub async fn hang_up(&mut self, within: Duration) -> Result<u16> {
        let cseq = self.dialog.next_cseq();
        let response = send_bye(&self.endpoint, &self.dialog, &self.target, cseq, within).await?;
        self.ended = true;
        bye_outcome(&response, &self.dialog, cseq)
    }
}

/// Place an SDP-free INVITE and confirm it with an ACK.
///
/// No session is offered, so no RTP socket is bound and no media task is started — which is what
/// separates this from a call whose negotiated session happens to be silent. One digest challenge
/// is answered when credentials were supplied; anything else non-2xx is returned as
/// [`Error::Rejected`].
pub async fn dial_signalling(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &SignallingDialOptions,
) -> Result<SignallingDial> {
    dial_signalling_with(endpoint, target, to, options, None).await
}

/// [`dial_signalling`], stopping early if `cancelled` resolves first.
///
/// Cancellation is not merely ceasing to wait: the invitation is withdrawn with a CANCEL before
/// this returns, including the race where a success was already in flight, and the resulting
/// [`Error::Cancelled`] distinguishes that local stop from a refusal by the far end.
pub async fn dial_signalling_until<F>(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &SignallingDialOptions,
    cancelled: F,
) -> Result<SignallingDial>
where
    F: Future<Output = ()> + Send,
{
    tokio::pin!(cancelled);
    let cancelled: crate::call::Cancelled<'_> = cancelled.as_mut();
    dial_signalling_with(endpoint, target, to, options, Some(cancelled)).await
}

async fn dial_signalling_with(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &SignallingDialOptions,
    mut cancelled: Option<crate::call::Cancelled<'_>>,
) -> Result<SignallingDial> {
    let started = tokio::time::Instant::now();
    let mut authorization = None;
    let mut cseq = 1_u32;
    loop {
        let invite = signalling_invite(endpoint, &target, to, options, cseq, authorization.take())?;
        let mut responses = endpoint.send(invite.clone(), target.clone()).await?;
        let invitation_started = tokio::time::Instant::now();
        let response =
            match crate::call::await_final(&mut responses, options.timeout, None, &mut cancelled)
                .await
            {
                crate::call::Waited::Final { response, .. } => response,
                crate::call::Waited::Gone => return Err(Error::NoResponse),
                crate::call::Waited::Transport(error) => return Err(Error::Transport(error)),
                outcome @ (crate::call::Waited::GaveUp | crate::call::Waited::Cancelled) => {
                    let timed_out = matches!(outcome, crate::call::Waited::GaveUp);
                    let invitation_elapsed = invitation_started.elapsed();
                    let reason = if timed_out {
                        crate::call::request_timeout_reason()
                    } else {
                        crate::call::normal_clearing_reason()
                    };
                    let cleanup = crate::call::withdraw(
                        endpoint,
                        &invite,
                        target.clone(),
                        &mut responses,
                        &reason,
                        options.cancellation_timeout,
                    )
                    .await;
                    return Err(Error::Cancelled(InvitationCancellation {
                        timed_out,
                        invitation_limit: options.timeout,
                        invitation_elapsed,
                        cleanup,
                    }));
                }
            };

        // One retry and no more (RFC 3261 §22.2). A far end that challenges the answer to its own
        // challenge is not going to be satisfied by a third INVITE, and a loop here is an outbound
        // call flood.
        if matches!(response.status.code(), 401 | 407)
            && cseq == 1
            && let Some(credentials) = options.credentials.as_ref()
            && let Some(header) = digest_answer(&invite, &response, credentials)
        {
            cseq = cseq.saturating_add(1);
            authorization = Some(header);
            continue;
        }
        if !response.status.is_success() {
            // A non-2xx is acknowledged by the transaction layer itself, and this side has no
            // media port to release, so there is nothing left to undo.
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }

        // From here the far end believes a dialog exists, so every path has to acknowledge it.
        let dialog = Dialog::from_response(&invite, &response).ok_or(Error::NoDialog)?;
        let in_dialog = crate::call::in_dialog_target(&dialog, target.clone());
        let ack = crate::call::build_ack(endpoint, &dialog, &in_dialog)?;
        endpoint
            .send_directly(ack.clone(), in_dialog.clone())
            .await?;
        // RFC 3261 §13.2.2.4: a retransmitted 2xx means this ACK was lost and another is required.
        // Dropping the stream here would leave the far end retransmitting for 64*T1 and then
        // tearing down a dialog this side believes is up.
        tokio::spawn(crate::call::reack_retransmitted_2xx(
            endpoint.clone(),
            responses,
            ack,
            in_dialog.clone(),
        ));
        return Ok(SignallingDial {
            endpoint: endpoint.clone(),
            dialog,
            target: in_dialog,
            status: response.status.code(),
            setup: started.elapsed(),
            ended: false,
        });
    }
}

/// Build one SDP-free INVITE attempt.
///
/// No `Content-Type` and no body, which is the whole difference: an INVITE that names no session
/// names no port, and a port that is never named is never bound.
fn signalling_invite(
    endpoint: &Handle,
    target: &Target,
    to: &Uri,
    options: &SignallingDialOptions,
    cseq: u32,
    authorization: Option<sipx_sip::Header>,
) -> Result<Request> {
    let identity = &options.identity;
    let mut builder = RequestBuilder::new(Method::Invite, to.clone())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/{} {};rport;branch={}",
                target.transport.as_str(),
                endpoint.sent_by_for(target.transport),
                sipx_transport::new_branch()
            )),
        )?
        .header(HeaderName::To, identity.to.clone())?
        .header(HeaderName::From, identity.from.clone())?
        .header(HeaderName::CallId, identity.call_id.clone())?
        .cseq(cseq, &Method::Invite)?
        .header(HeaderName::Contact, identity.contact.clone())?
        .max_forwards(70);
    for header in &options.headers {
        builder = builder.header(
            header.name().clone(),
            Bytes::copy_from_slice(header.raw_value()),
        )?;
    }
    let mut request = builder.build();
    if let Some(header) = authorization {
        request.headers.push(header);
    }
    Ok(request)
}

/// Answer the strongest digest challenge a 401 or 407 offered, if any is supported.
fn digest_answer(
    request: &Request,
    response: &Response,
    credentials: &Credentials,
) -> Option<sipx_sip::Header> {
    let from_proxy = response.status.code() == 407;
    let name = if from_proxy {
        HeaderName::ProxyAuthenticate
    } else {
        HeaderName::WwwAuthenticate
    };
    let challenges = response
        .headers
        .get_all(&name)
        .filter_map(|header| sipx_sip::auth::Challenge::parse(&header.value(), from_proxy))
        .collect();
    let challenge = sipx_sip::auth::strongest(challenges)?;
    let uri_bytes = request.uri.to_bytes();
    let uri = String::from_utf8_lossy(&uri_bytes);
    let cnonce = sipx_transport::new_branch();
    let value = sipx_sip::auth::respond(&challenge, credentials, "INVITE", &uri, 1, &cnonce);
    sipx_sip::Header::build(challenge.response_header(), Bytes::from(value)).ok()
}

/// Originate BYE on a confirmed dialog and wait for the final response that ends it.
///
/// `within` bounds a *failure*; success is the observed final response rather than elapsed wall
/// time. Shared by both halves, which end a dialog the same way: the identity is the dialog's and
/// neither side's media, or lack of it, has any say in it.
async fn send_bye(
    endpoint: &Handle,
    dialog: &Dialog,
    target: &Target,
    cseq: u32,
    within: Duration,
) -> Result<Response> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let builder = RequestBuilder::new(Method::Bye, uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .cseq(cseq, &Method::Bye)?
        .max_forwards(70);
    let bye = crate::call::add_routes(builder, &routes)?.build();
    let mut responses = endpoint.send(bye, target.clone()).await?;
    // Fixed duration bounds a failed teardown; the final response is the happens-before.
    tokio::time::timeout(within, responses.final_response())
        .await
        .map_err(|_| Error::SignallingTeardownTimeout(within))?
        .ok_or(Error::SignallingTeardownTimeout(within))
}

/// What a final response to an originated BYE says about the dialog it was meant to end.
fn bye_outcome(response: &Response, dialog: &Dialog, cseq: u32) -> Result<u16> {
    if !response_matches_dialog(response, dialog, cseq) {
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
#[cfg_attr(coverage_nightly, coverage(off))]
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
