//! Establishing a call: INVITE with an SDP offer, media bound to the answer, and BYE.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_media::{Codec, MediaPort, MediaSession};
use sipx_sdp::{Capabilities, Direction, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target, TransportKind};

use crate::dialog::{Dialog, strip_header_params};
use crate::error::{Error, Result};

/// 200 OK.
///
/// `StatusCode::new` is fallible because most codes come from the wire; this one is a literal
/// that is always in range. Threading a `Result` out of every call site for it would mean
/// inventing an error that can never happen — and the previous attempt reported it as "no
/// final response to the INVITE", which would have been actively misleading.
fn ok_status() -> StatusCode {
    const OK: u16 = 200;
    StatusCode::new(OK).unwrap_or_else(|| unreachable!("200 is a valid status code"))
}

/// A fresh token for a `Call-ID` or a `tag`.
///
/// Its own function rather than the user agent's digest `cnonce`: a dialog identifier is not an
/// authentication nonce, and borrowing one ties this layer to the one that handles credentials
/// for no reason beyond both wanting random hex.
fn token() -> String {
    use rand::Rng as _;
    let value: u64 = rand::rng().random();
    format!("{value:016x}")
}

/// A call in progress.
#[derive(Debug)]
pub struct Call {
    /// The dialog it runs in.
    pub dialog: Dialog,
    media: MediaSession,
    endpoint: Handle,
    /// Where in-dialog requests go: the peer's `Contact`, not where the INVITE was sent.
    target: Target,
    /// Set while a 2xx is still being retransmitted; cleared when the ACK arrives.
    awaiting_ack: Option<Arc<tokio::sync::Notify>>,
    ended: bool,
    /// Where this side receives media, so a re-offer can name the same address.
    media_address: IpAddr,
    /// What the running session negotiated, for comparison against a re-offer.
    current: Negotiated,
    /// Whether the call is on hold, and which way.
    hold: Direction,
}

impl Call {
    /// The audio.
    #[must_use]
    pub fn media(&self) -> &MediaSession {
        &self.media
    }

    /// Send a DTMF digit.
    pub async fn send_digit(&self, digit: sipx_rtp::Digit, duration: Duration) -> bool {
        self.media.send_digit(digit, duration).await
    }

    /// Send a string of digits, each held for `duration`.
    ///
    /// Characters that are not DTMF digits are skipped rather than rejected: a caller passing
    /// a formatted number should not have to strip the spaces and dashes itself.
    pub async fn send_digits(&self, digits: &str, duration: Duration) -> bool {
        for c in digits.chars() {
            let Some(digit) = sipx_rtp::Digit::from_char(c) else {
                continue;
            };
            if !self.media.send_digit(digit, duration).await {
                return false;
            }
        }
        true
    }

    /// Take the next digit the far end pressed.
    pub async fn recv_digit(&self) -> Option<sipx_rtp::Digit> {
        self.media.recv_digit().await
    }

    /// Whether the call has ended, from either side.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// Feed an in-dialog request to the call.
    ///
    /// Returns whether it belonged here. Without this an incoming BYE reaches nothing and the
    /// local media session goes on sending RTP into a call the far end has torn down — worse
    /// than a call that never connects, because it does not stop.
    pub async fn handle(&mut self, incoming: &Incoming) -> Result<bool> {
        if !self.dialog.matches(&incoming.request) {
            return Ok(false);
        }

        match incoming.request.method {
            Method::Ack => {
                // The 2xx got through; stop retransmitting it.
                if let Some(notify) = self.awaiting_ack.take() {
                    notify.notify_waiters();
                }
                Ok(true)
            }
            // An INVITE inside an existing dialog is a re-INVITE: a renegotiation of the call
            // already running, not a new one.
            Method::Invite => {
                self.on_reinvite(incoming).await?;
                Ok(true)
            }
            Method::Bye => {
                self.media.stop();
                self.ended = true;
                if let Some(notify) = self.awaiting_ack.take() {
                    notify.notify_waiters();
                }
                let response =
                    ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?.build();
                self.endpoint.respond(&incoming.key, response).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Whether the far end has put the call on hold.
    #[must_use]
    pub fn is_on_hold(&self) -> bool {
        !self.hold.receives()
    }

    /// Renegotiate an established call from a re-INVITE.
    ///
    /// The rule that shapes this: **a renegotiation that fails must leave the call running.** A
    /// re-INVITE tries to change something about a call that already works, so answering 488
    /// and carrying on is right; tearing the call down because the new offer was unusable
    /// would lose a call that was fine a moment ago.
    async fn on_reinvite(&mut self, incoming: &Incoming) -> Result<()> {
        // RFC 3261 §12.2.2: a sequence number that is not greater than the last one is out of
        // order, and applying it would let a delayed packet undo a later change.
        let sequence = incoming
            .request
            .headers
            .typed::<sipx_sip::headers::CSeq>()
            .and_then(std::result::Result::ok)
            .map(|cseq| cseq.sequence);
        if let (Some(sequence), Some(last)) = (sequence, self.dialog.remote_cseq)
            && sequence <= last
        {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        if let Some(sequence) = sequence {
            self.dialog.remote_cseq = Some(sequence);
        }

        let Ok(offer) = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body())) else {
            return self.refuse(incoming, 488, "Not Acceptable Here").await;
        };
        let Ok(renegotiated) = negotiated(&offer) else {
            return self.refuse(incoming, 488, "Not Acceptable Here").await;
        };

        let capabilities = Capabilities::g711(self.media_address, self.media.local_addr().port());
        let answer_sdp = sipx_sdp::answer(&offer, &capabilities);
        if answer_sdp
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return self.refuse(incoming, 488, "Not Acceptable Here").await;
        }

        // Hold is a direction, not a separate state: `sendonly` or `inactive` from the far end
        // means it will not play what we send.
        self.hold = offer
            .media
            .iter()
            .find(|m| m.media == "audio" && !m.is_rejected())
            .map_or(Direction::SendRecv, sipx_sdp::MediaDescription::direction);

        self.move_media_if_changed(renegotiated).await?;

        let response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint)),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer_sdp.to_string_sdp()))
            .build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// Refuse a renegotiation without ending the call.
    async fn refuse(&self, incoming: &Incoming, code: u16, reason: &'static str) -> Result<()> {
        let status = StatusCode::new(code).unwrap_or_else(ok_status);
        let response = ResponseBuilder::to_request(&incoming.request, status, reason)?.build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// Rebuild the media session, but only if where or how the media flows actually changed.
    ///
    /// Restarting an unchanged session would drop packets for no reason on every re-INVITE, and
    /// some peers send one every thirty seconds as a keep-alive.
    async fn move_media_if_changed(&mut self, to: Negotiated) -> Result<()> {
        if to.remote != self.current.remote || to.codec != self.current.codec {
            let port = MediaPort::bind(SocketAddr::new(self.media_address, 0))
                .await
                .map_err(Error::Io)?;
            let replacement = port.start(to.media_config());
            let previous = std::mem::replace(&mut self.media, replacement);
            previous.stop();
        }
        self.current = to;
        Ok(())
    }

    /// Send a re-INVITE renegotiating this call.
    ///
    /// `direction` puts the call on hold (`SendOnly` or `Inactive`) or takes it off
    /// (`SendRecv`).
    pub async fn reinvite(&mut self, direction: Direction) -> Result<()> {
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();

        let mut capabilities =
            Capabilities::g711(self.media_address, self.media.local_addr().port());
        capabilities.direction = direction;
        // The session version must increase with each modified offer, so the far end can tell
        // a changed description from a repeated one.
        capabilities.session_version = u64::from(cseq);
        let offer = offer_from(&capabilities);

        let builder = RequestBuilder::new(Method::Invite, self.dialog.remote_target.clone())
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Invite)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint)),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .max_forwards(70)
            .body(Bytes::from(offer.to_string_sdp()));

        let request = add_route_set(builder, &self.dialog)?.build();
        let mut responses = self.endpoint.send(request, self.target).await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;

        if !response.status.is_success() {
            // The far end refused the change. The call it refused to change is still running,
            // so this is an error about the renegotiation, not about the call.
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }

        send_ack(&self.endpoint, &self.dialog, self.target).await?;

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            && let Ok(renegotiated) = negotiated(&answer)
        {
            self.move_media_if_changed(renegotiated).await?;
        }
        self.hold = direction;
        Ok(())
    }

    /// End the call.
    ///
    /// Anything still queued is sent first, then the media stops, then the BYE goes out.
    /// Stopping first would discard the tail of whatever was playing — the last word of a
    /// clip, the last digit of a PIN — because sending is paced and the queue outlives the
    /// call by however much is left in it.
    pub async fn hang_up(&mut self) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        self.media.flush(Duration::from_secs(5)).await;
        self.media.stop();
        self.ended = true;
        if let Some(notify) = self.awaiting_ack.take() {
            notify.notify_waiters();
        }

        let cseq = self.dialog.next_cseq();
        let bye = bye_request(&self.dialog, cseq)?;
        let mut responses = self.endpoint.send(bye, self.target).await?;
        // A BYE that is never answered still ends the call locally: the alternative is a call
        // that cannot be hung up because the far end has already gone.
        let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
        Ok(())
    }
}

/// Add the dialog's route set, in order, as `Route` headers.
///
/// Without these, a request through a Record-Routing proxy — which is to say almost any real
/// deployment — is addressed straight at the peer's `Contact`, which the proxy will not relay
/// and the peer may not be reachable at. The call establishes and cannot be ended.
fn add_route_set(
    mut builder: RequestBuilder,
    dialog: &Dialog,
) -> std::result::Result<RequestBuilder, sipx_sip::error::BuildError> {
    for route in &dialog.route_set {
        builder = builder.header(HeaderName::Route, Bytes::from(route.clone()))?;
    }
    Ok(builder)
}

fn bye_request(dialog: &Dialog, cseq: u32) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let builder = RequestBuilder::new(Method::Bye, dialog.remote_target.clone())
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .cseq(cseq, &Method::Bye)?
        .max_forwards(70);
    Ok(add_route_set(builder, dialog)?.build())
}

/// How a call is placed.
#[derive(Debug, Clone)]
pub struct DialOptions {
    /// Our own address of record.
    pub from: String,
    /// Where this side receives media.
    pub media_address: IpAddr,
    /// How long to wait for an answer before giving up and cancelling.
    ///
    /// `None` waits as long as the transaction layer does — 64·T1, or 32 seconds with the
    /// default constants. A bound *here* rather than around the call is what makes giving up
    /// correct: dropping the future partway through leaves the far end believing it is in a
    /// call, and only code inside the exchange can send the CANCEL that stops it.
    pub timeout: Option<Duration>,
}

impl DialOptions {
    /// Options for a call from an address of record.
    #[must_use]
    pub fn new(from: impl Into<String>, media_address: IpAddr) -> Self {
        Self {
            from: from.into(),
            media_address,
            timeout: None,
        }
    }

    /// Give up after this long.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Place a call.
pub async fn dial(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Call> {
    let media_address = options.media_address;
    let from = options.from.as_str();
    // The offer has to name the port audio will arrive on, and only a bound socket knows it.
    // So the port is bound now and the session started once the answer says where and in what.
    let port = MediaPort::bind(SocketAddr::new(media_address, 0))
        .await
        .map_err(Error::Io)?;

    let capabilities = Capabilities::g711(media_address, port.local_addr().port());
    let offer = offer_from(&capabilities);

    // The `Via` is built here rather than left to the transport, because a CANCEL has to carry
    // the *same* branch as the INVITE it cancels — that identity is what matches the two at the
    // far end (RFC 3261 §9.1). Letting the transport generate it would leave this layer unable
    // to name the transaction it started.
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.advertised(),
        sipx_transport::new_branch()
    );

    let invite = RequestBuilder::new(Method::Invite, to.clone())
        .header(HeaderName::Via, Bytes::from(via.clone()))?
        .header(
            HeaderName::To,
            Bytes::from(format!("<{}>", String::from_utf8_lossy(&to.to_bytes()))),
        )?
        .header(
            HeaderName::From,
            Bytes::from(format!("{from};tag={}", token())),
        )?
        .header(HeaderName::CallId, Bytes::from(format!("{}@sipx", token())))?
        .cseq(1, &Method::Invite)?
        .header(HeaderName::Contact, Bytes::from(contact_for(endpoint)))?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .max_forwards(70)
        .body(Bytes::from(offer.to_string_sdp()))
        .build();

    let mut responses = endpoint.send(invite.clone(), target).await?;

    let response = match options.timeout {
        None => responses.final_response().await.ok_or(Error::NoResponse)?,
        Some(limit) => match tokio::time::timeout(limit, responses.final_response()).await {
            Ok(response) => response.ok_or(Error::NoResponse)?,
            Err(_elapsed) => {
                // Giving up is not just ceasing to wait. The far end is ringing and has been
                // told nothing; without a CANCEL it goes on ringing, and someone answering
                // afterwards ends up in a call with a party that has left.
                let _ = send_cancel(endpoint, &invite, &via, target).await;

                // CANCEL cannot close the race it exists to manage: a 200 already in flight
                // arrives anyway, and RFC 3261 §15 says a UAC that will not proceed must
                // acknowledge it and then hang up rather than leave it unanswered.
                if let Ok(Some(late)) =
                    tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await
                    && late.status.is_success()
                    && let Some(dialog) = Dialog::from_response(&invite, &late)
                {
                    let in_dialog = in_dialog_target(&dialog, target);
                    let _ = send_ack(endpoint, &dialog, in_dialog).await;
                    if let Ok(bye) = bye_request(&dialog, dialog.local_cseq.saturating_add(1)) {
                        let _ = endpoint.send(bye, in_dialog).await;
                    }
                }
                return Err(Error::Cancelled(limit));
            }
        },
    };

    if !response.status.is_success() {
        // A non-2xx is acknowledged by the transaction layer itself, so there is nothing to
        // send here — only a media port to release, which happens when `port` drops.
        return Err(Error::Rejected {
            status: response.status.code(),
            reason: String::from_utf8_lossy(&response.reason).into_owned(),
        });
    }

    // From here the far end believes a dialog exists, so *every* path must acknowledge.
    // Returning an error without one leaves it retransmitting its 200 for 32 seconds and then
    // streaming media at a port we have closed.
    match establish(&invite, &response, target, port) {
        Ok((dialog, media, in_dialog, negotiated)) => {
            send_ack(endpoint, &dialog, in_dialog).await?;
            Ok(Call {
                dialog,
                media,
                endpoint: endpoint.clone(),
                target: in_dialog,
                awaiting_ack: None,
                ended: false,
                media_address,
                current: negotiated,
                hold: Direction::SendRecv,
            })
        }
        Err(error) => {
            // RFC 3261 §15: a UAC that cannot proceed after a 2xx acknowledges it and then
            // sends BYE. Walking away silently is what leaves the far end streaming.
            if let Some(dialog) = Dialog::from_response(&invite, &response) {
                let in_dialog = in_dialog_target(&dialog, target);
                let _ = send_ack(endpoint, &dialog, in_dialog).await;
                if let Ok(bye) = bye_request(&dialog, dialog.local_cseq.saturating_add(1)) {
                    let _ = endpoint.send(bye, in_dialog).await;
                }
            }
            Err(error)
        }
    }
}

/// Everything after a 2xx that can fail, kept together so the caller can ACK on either path.
fn establish(
    invite: &Request,
    response: &Response,
    fallback: Target,
    port: MediaPort,
) -> Result<(Dialog, MediaSession, Target, Negotiated)> {
    let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
    let negotiated = negotiated(&answer)?;
    let dialog = Dialog::from_response(invite, response).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, fallback);
    let media = port.start(negotiated.media_config());
    Ok((dialog, media, target, negotiated))
}

/// Answer an incoming INVITE.
///
/// The 200 OK is retransmitted until the ACK arrives, which is the transaction user's job:
/// `sipx-sip`'s server transaction moves to `Accepted` and absorbs retransmissions of the
/// *request*, but it does not resend the response. Over UDP one lost 200 means the caller
/// gives up while this side holds an established call, so this is not optional.
pub async fn answer(endpoint: &Handle, incoming: &Incoming, media_address: IpAddr) -> Result<Call> {
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;

    let negotiated = negotiated(&offer)?;
    let media = MediaSession::start(SocketAddr::new(media_address, 0), negotiated.media_config())
        .await
        .map_err(Error::Io)?;

    let capabilities = Capabilities::g711(media_address, media.local_addr().port());
    let answer_sdp = sipx_sdp::answer(&offer, &capabilities);
    if answer_sdp
        .media
        .iter()
        .all(sipx_sdp::MediaDescription::is_rejected)
    {
        return Err(Error::NoCommonCodec);
    }

    let tag = token();
    let to_with_tag = {
        let existing = incoming
            .request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        format!("{};tag={tag}", strip_header_params(&existing))
    };

    let response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
        .set_header(&HeaderName::To, Bytes::from(to_with_tag))?
        .header(HeaderName::Contact, Bytes::from(contact_for(endpoint)))?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .body(Bytes::from(answer_sdp.to_string_sdp()))
        .build();

    endpoint.respond(&incoming.key, response.clone()).await?;

    let dialog = Dialog::from_request(&incoming.request, &tag).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));

    let acked = Arc::new(tokio::sync::Notify::new());
    tokio::spawn(retransmit_until_acked(
        endpoint.clone(),
        incoming.key.clone(),
        response,
        Arc::clone(&acked),
    ));

    Ok(Call {
        dialog,
        media,
        endpoint: endpoint.clone(),
        target,
        awaiting_ack: Some(acked),
        ended: false,
        media_address,
        current: negotiated,
        hold: Direction::SendRecv,
    })
}

/// Resend a 2xx on the T1 backoff until the ACK arrives or 64·T1 has passed.
async fn retransmit_until_acked(
    endpoint: Handle,
    key: sipx_sip::transaction::TransactionKey,
    response: Response,
    acked: Arc<tokio::sync::Notify>,
) {
    let t1 = Duration::from_millis(500);
    let mut interval = t1;
    let mut elapsed = Duration::ZERO;
    let give_up = t1 * 64;

    loop {
        tokio::select! {
            () = acked.notified() => return,
            () = tokio::time::sleep(interval) => {}
        }
        elapsed += interval;
        if elapsed >= give_up {
            tracing::warn!("no ACK for our 2xx after 64*T1; giving up");
            return;
        }
        if endpoint.respond(&key, response.clone()).await.is_err() {
            return;
        }
        // Doubling capped at T2, exactly as the INVITE client transaction retransmits.
        interval = (interval * 2).min(Duration::from_secs(4));
    }
}

/// Cancel an INVITE that has not been answered (RFC 3261 §9.1).
///
/// A CANCEL is not a new request in its own right: it carries the INVITE's `Via` verbatim —
/// branch and all — its `Call-ID`, `To`, `From` and sequence *number*, differing only in the
/// method. That is what identifies which invitation it is cancelling.
async fn send_cancel(endpoint: &Handle, invite: &Request, via: &str, target: Target) -> Result<()> {
    let copy = |name: &HeaderName| {
        invite
            .headers
            .value(name)
            .map(|value| Bytes::from(value.into_owned()))
    };

    let mut builder = RequestBuilder::new(Method::Cancel, invite.uri.clone())
        .header(HeaderName::Via, Bytes::from(via.to_owned()))?;
    for name in [HeaderName::To, HeaderName::From, HeaderName::CallId] {
        if let Some(value) = copy(&name) {
            builder = builder.header(name, value)?;
        }
    }
    // The same sequence number as the INVITE, with the method changed. A fresh number would
    // make it a new request rather than a cancellation of that one.
    let sequence = invite
        .headers
        .typed::<sipx_sip::headers::CSeq>()
        .and_then(std::result::Result::ok)
        .map_or(1, |cseq| cseq.sequence);

    let request = builder
        .cseq(sequence, &Method::Cancel)?
        .max_forwards(70)
        .build();
    endpoint.send(request, target).await?;
    Ok(())
}

async fn send_ack(endpoint: &Handle, dialog: &Dialog, target: Target) -> Result<()> {
    let (local, remote) = dialog.local_and_remote();
    let ack = RequestBuilder::new(Method::Ack, dialog.remote_target.clone())
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        // The ACK for a 2xx carries the INVITE's sequence number, not a new one: it
        // acknowledges that request rather than being one of its own.
        .cseq(dialog.local_cseq, &Method::Ack)?
        .max_forwards(70);
    endpoint
        .send(add_route_set(ack, dialog)?.build(), target)
        .await?;
    Ok(())
}

/// Where in-dialog requests go.
///
/// RFC 3261 §12.2.1.1: the peer's `Contact`, not the address the INVITE was sent to. Those
/// differ whenever a redirect, a B2BUA or a load balancer is involved, and using the original
/// address means the ACK and the BYE reach the wrong element.
///
/// A `Contact` naming a hostname would need resolution, which this layer does not do; the
/// address the exchange arrived from is the honest fallback, and behind a NAT it is the only
/// one that works.
fn in_dialog_target(dialog: &Dialog, fallback: Target) -> Target {
    let Some(sipx_sip::Host::Ip(ip)) = dialog.remote_target.host() else {
        return fallback;
    };
    let transport = dialog
        .remote_target
        .transport()
        .and_then(TransportKind::parse)
        .unwrap_or(fallback.transport);
    let port = dialog
        .remote_target
        .port()
        .unwrap_or_else(|| transport.default_port());
    Target::new(SocketAddr::new(*ip, port), transport)
}

fn offer_from(capabilities: &Capabilities) -> SessionDescription {
    let mut sdp = SessionDescription::new(
        capabilities.address,
        capabilities.session_id,
        capabilities.session_version,
    );
    let mut audio = sipx_sdp::MediaDescription::audio(
        capabilities.audio_port,
        capabilities.audio_formats.clone(),
    );
    for (payload, mapping) in &capabilities.rtpmaps {
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "rtpmap",
            format!("{payload} {mapping}"),
        ));
    }
    audio.set_direction(capabilities.direction);
    sdp.media.push(audio);
    sdp
}

/// What negotiation settled on.
#[derive(Debug, Clone, Copy)]
struct Negotiated {
    remote: SocketAddr,
    codec: Codec,
    /// The payload type the far end uses for `telephone-event`, if it offered one.
    ///
    /// Taken from the description rather than assumed, because it is a *dynamic* type: 101 is
    /// what sipx offers, not what everyone uses, and assuming it would send keypresses on
    /// whatever the far end put that number to.
    dtmf: Option<u8>,
}

impl Negotiated {
    fn media_config(self) -> sipx_media::Config {
        let mut config = sipx_media::Config::new(self.remote, self.codec);
        config.dtmf_payload_type = self.dtmf;
        config
    }
}

/// The payload type carrying `telephone-event`, per the description's own rtpmaps.
fn telephone_event_payload_type(audio: &sipx_sdp::MediaDescription) -> Option<u8> {
    audio.formats.iter().find_map(|format| {
        let mapping = audio.rtpmap(format)?;
        let encoding = mapping.split('/').next().unwrap_or(mapping);
        encoding
            .eq_ignore_ascii_case("telephone-event")
            .then(|| format.parse::<u8>().ok())
            .flatten()
    })
}

/// Where to send media, and in what codec, from a description.
fn negotiated(sdp: &SessionDescription) -> Result<Negotiated> {
    let audio = sdp
        .media
        .iter()
        .find(|m| m.media == "audio" && !m.is_rejected())
        .ok_or(Error::NoCommonCodec)?;

    // A stream marked `inactive` carries nothing in either direction. Treating it as a working
    // call means holding a media session open for audio that will never come.
    if audio.direction() == Direction::Inactive {
        return Err(Error::NoCommonCodec);
    }

    let address = sdp.address_for(audio).ok_or(Error::NoCommonCodec)?;

    // The first format both sides can carry. The list is already in the offerer's preference
    // order, so the first playable one is the one to use.
    let codec = audio
        .formats
        .iter()
        .find_map(|format| format.parse::<u8>().ok().and_then(Codec::from_payload_type))
        .ok_or(Error::NoCommonCodec)?;

    Ok(Negotiated {
        remote: SocketAddr::new(address, audio.port),
        codec,
        dtmf: telephone_event_payload_type(audio),
    })
}

/// The `Contact` this endpoint should advertise.
///
/// Built from the endpoint's *advertised* address rather than its socket's local one. An
/// endpoint bound to `0.0.0.0` has a local address that means nothing to a peer, and behind a
/// NAT it is private — either way the peer stores it as the dialog's remote target and every
/// in-dialog request it sends becomes unroutable.
#[must_use]
pub fn contact_for(endpoint: &Handle) -> String {
    format!("<sip:sipx@{}>", endpoint.advertised())
}
