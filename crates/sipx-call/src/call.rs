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
use crate::transfer::{
    Referral, Replaces, Transfer, TransferState, is_terminated, parse_sipfrag, sipfrag,
};

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
    /// A transfer the far end has asked for and we have not yet answered.
    referral: Option<Referral>,
    /// A transfer we asked for, and what has become of it.
    transfer: Option<Transfer>,
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
            // A REFER is not answered here, and that is deliberate. Every other in-dialog
            // request has one correct response; a REFER asks *may I place a call on your
            // behalf*, and only the application knows whether it may. `accept_referral` and
            // `refuse_referral` are the two answers, and until one is given the transferor is
            // waiting — which is honest, because it is.
            Method::Refer => {
                self.on_refer(incoming).await?;
                Ok(true)
            }
            Method::Notify => {
                self.on_notify(incoming).await?;
                Ok(true)
            }
            Method::Bye => {
                // §12.2.2 applies to every in-dialog request, not only the ones that
                // renegotiate: a BYE from behind the current sequence number is a stale
                // duplicate, and honouring it ends a call that is still running.
                if self.out_of_order(&incoming.request) {
                    self.refuse(incoming, 500, "Server Internal Error").await?;
                    return Ok(true);
                }
                self.record_remote_cseq(&incoming.request);

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
        if self.out_of_order(&incoming.request) {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        self.record_remote_cseq(&incoming.request);

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

        // RFC 3261 §12.2.2: a re-INVITE is a target refresh request, so its `Contact` replaces
        // the dialog's remote target. Without this the BYE still goes to where the peer was
        // when the call started, and a peer that has moved can never be told it is over.
        self.dialog.refresh_target(&incoming.request.headers);
        self.target = in_dialog_target(
            &self.dialog,
            Target::new(incoming.source, incoming.transport),
        );

        let response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer_sdp.to_string_sdp()))
            .build();
        self.endpoint
            .respond(&incoming.key, response.clone())
            .await?;

        // RFC 3261 §13.3.1.4 applies to the 2xx of *any* INVITE, not only the first: it is
        // retransmitted until the ACK arrives. The server transaction deliberately absorbs
        // retransmitted INVITEs without answering them again (RFC 6026), so if the TU does not
        // resend, one lost 200 deadlocks the renegotiation until the peer's Timer B — a single
        // dropped packet breaking hold and resume for half a minute.
        if let Some(previous) = self.awaiting_ack.take() {
            previous.notify_waiters();
        }
        let acked = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(retransmit_until_acked(
            self.endpoint.clone(),
            incoming.key.clone(),
            response,
            Arc::clone(&acked),
        ));
        self.awaiting_ack = Some(acked);
        Ok(())
    }

    /// Whether an in-dialog request arrived out of order.
    ///
    /// RFC 3261 §12.2.2 rejects a request behind the dialog's sequence number with a 500
    /// rather than applying it, and §12.2.1.1 requires each new in-dialog request to
    /// *increment* the number — so a repeat of the current one is a duplicate that has escaped
    /// the transaction layer's absorption window, not a fresh request, and is refused on the
    /// same grounds. This is not only the re-INVITE case: a stale BYE honoured here ends a
    /// call that a later request has already changed.
    fn out_of_order(&self, request: &Request) -> bool {
        let Some(sequence) = remote_cseq(request) else {
            return false;
        };
        self.dialog.remote_cseq.is_some_and(|last| sequence <= last)
    }

    /// Record the sequence number of an in-dialog request this side has accepted.
    fn record_remote_cseq(&mut self, request: &Request) {
        if let Some(sequence) = remote_cseq(request) {
            self.dialog.remote_cseq = Some(sequence);
        }
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

        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Invite, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Invite)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .max_forwards(70)
            .body(Bytes::from(offer.to_string_sdp()));

        let request = add_routes(builder, &routes)?.build();
        let mut responses = self.endpoint.send(request, self.target.clone()).await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;

        if !response.status.is_success() {
            // The far end refused the change. The call it refused to change is still running,
            // so this is an error about the renegotiation, not about the call.
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }

        // RFC 3261 §12.2.1.2: the 2xx to a target refresh request refreshes the target here
        // too, and it must be applied before the ACK — which is itself an in-dialog request
        // and belongs at the peer's new location.
        self.dialog.refresh_target(&response.headers);
        self.target = in_dialog_target(&self.dialog, self.target.clone());

        send_ack(&self.endpoint, &self.dialog, self.target.clone()).await?;

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            && let Ok(renegotiated) = negotiated(&answer)
        {
            self.move_media_if_changed(renegotiated).await?;
        }
        self.hold = direction;
        Ok(())
    }

    /// Ask the far end to transfer this call to `target` (RFC 3515).
    ///
    /// Returns once the transferee has accepted the *request*, which is not the same as the
    /// transfer having worked: a `202 Accepted` means "I will try". What became of it arrives
    /// afterwards, as NOTIFY, and shows up in [`Self::transfer`]. Reporting success here would
    /// tell a user their call was handed over when it may have been refused or rung out.
    pub async fn refer(&mut self, target: &Uri) -> Result<()> {
        let refer_to = String::from_utf8_lossy(&target.to_bytes()).into_owned();
        self.refer_to_raw(&refer_to).await
    }

    /// Ask the far end to replace `other` with a call to this one's peer (RFC 3891 + 3515).
    ///
    /// The attended half of a transfer. Where a blind transfer says "call this number", this
    /// says "call this number, and when you get through, take the place of the call I already
    /// have with them" — which is what makes the handover seamless rather than a second ring.
    pub async fn refer_attended(&mut self, other: &Call) -> Result<()> {
        let replaces = Replaces {
            call_id: other.dialog.id.call_id.clone(),
            // From the point of view of the party that will receive the eventual INVITE, our
            // *remote* tag on `other` is that party's own local tag. Writing our own tag here
            // produces a header that names nothing and a transfer that always fails.
            to_tag: other.dialog.id.remote_tag.clone(),
            from_tag: other.dialog.id.local_tag.clone(),
            early_only: false,
        };
        let target = String::from_utf8_lossy(&other.dialog.remote_target.to_bytes()).into_owned();
        // `?` separates a URI from the headers it asks to be put in the request built from it
        // (RFC 3261 §19.1.1), and `Replaces` is one of those headers.
        let refer_to = format!(
            "{target}?Replaces={}",
            escape_uri_header(&replaces.to_header())
        );
        self.refer_to_raw(&refer_to).await
    }

    /// Send a REFER whose `Refer-To` is this text.
    async fn refer_to_raw(&mut self, refer_to: &str) -> Result<()> {
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();

        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Refer, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local.clone()))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Refer)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(HeaderName::ReferTo, Bytes::from(format!("<{refer_to}>")))?
            // RFC 3892. The transferee is being asked to call a stranger on our say-so; saying
            // who we are is the only basis it has for deciding whether to.
            .header(
                HeaderName::ReferredBy,
                Bytes::from(strip_header_params(&local)),
            )?
            .max_forwards(70);

        let request = add_routes(builder, &routes)?.build();
        let mut responses = self.endpoint.send(request, self.target.clone()).await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;

        if !response.status.is_success() {
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }

        // Nothing is known yet beyond "it was taken on". The first NOTIFY replaces this.
        self.transfer = Some(Transfer {
            state: TransferState::Trying,
            finished: false,
        });
        Ok(())
    }

    /// The transfer the far end has asked for, if it has asked and we have not answered.
    #[must_use]
    pub fn referral(&self) -> Option<&Referral> {
        self.referral.as_ref()
    }

    /// A transfer we asked for, and what has become of it. `None` if we asked for none.
    #[must_use]
    pub fn transfer(&self) -> Option<&Transfer> {
        self.transfer.as_ref()
    }

    /// Accept the transfer, place the call, and report the outcome (RFC 3515 §2.4.5).
    ///
    /// `target` is where to *send* the new INVITE; the `Refer-To` URI is what goes in it. The
    /// two are separate for the same reason they are separate in [`dial`]: resolving a URI to
    /// an address is RFC 3263's job and lives in the transport, not here.
    ///
    /// The original call is left running. Whether to hang up on the transferor is a policy
    /// decision — a blind transfer usually ends it, an attended one does not — and it belongs
    /// to whoever is making that decision, not to this function.
    pub async fn accept_referral(&mut self, target: Target, options: &DialOptions) -> Result<Call> {
        let Some(referral) = self.referral.take() else {
            return Err(Error::NoReferral);
        };

        let accepted = ResponseBuilder::to_request(
            &referral.request,
            StatusCode::new(202).unwrap_or_else(|| unreachable!("202 is a valid status code")),
            "Accepted",
        )?
        .build();
        self.endpoint.respond(&referral.key, accepted).await?;

        // "I am trying", straight away. RFC 3515 §2.4.4 asks for an immediate NOTIFY so the
        // transferor knows the subscription exists before anything can go wrong with the call.
        self.notify_transfer(&referral, 100, "Trying", false)
            .await?;

        let placed = dial(&self.endpoint, target, &referral.target, options).await;

        let (status, reason) = match &placed {
            Ok(_) => (200, "OK".to_owned()),
            Err(Error::Rejected { status, reason }) => (*status, reason.clone()),
            // Anything else never reached the target at all. 503 is what a proxy would say for
            // the same situation, and it tells the transferor something true.
            Err(_) => (503, "Service Unavailable".to_owned()),
        };
        // Terminating, whether it worked or not. A transferee that reports the outcome and then
        // says nothing leaves a subscription open on both sides for a transfer that is over.
        self.notify_transfer(&referral, status, &reason, true)
            .await?;

        placed
    }

    /// Refuse the transfer (RFC 3515 §2.4.2).
    ///
    /// No subscription is created by a REFER that was not accepted, so nothing further is owed
    /// and no NOTIFY is sent. The transferor learns the outcome from the status, which is why
    /// it should be one they can act on — 603 for "no", 488 for "not that target".
    pub async fn refuse_referral(&mut self, status: u16, reason: &'static str) -> Result<()> {
        let Some(referral) = self.referral.take() else {
            return Err(Error::NoReferral);
        };
        let code = StatusCode::new(status).ok_or(Error::NoReferral)?;
        let response = ResponseBuilder::to_request(&referral.request, code, reason)?.build();
        self.endpoint.respond(&referral.key, response).await?;
        Ok(())
    }

    /// Note a REFER, or refuse one that cannot be honoured whatever the application thinks.
    async fn on_refer(&mut self, incoming: &Incoming) -> Result<()> {
        let sequence = incoming
            .request
            .headers
            .typed::<sipx_sip::headers::CSeq>()
            .and_then(std::result::Result::ok)
            .map_or(0, |cseq| cseq.sequence);

        let refer_to = incoming.request.headers.value(&HeaderName::ReferTo);
        let target = refer_to.as_deref().and_then(|value| {
            let text = String::from_utf8_lossy(value);
            Uri::parse(Bytes::from(unbracket(text.trim()))).ok()
        });

        let Some(target) = target else {
            // A missing or unparseable `Refer-To` is not a decision for the application: there
            // is nowhere to transfer to, and 400 says exactly that.
            self.refuse_now(incoming, 400, "Bad Request").await?;
            return Ok(());
        };

        self.referral = Some(Referral {
            target,
            referred_by: incoming
                .request
                .headers
                .value(&HeaderName::ReferredBy)
                .map(|value| String::from_utf8_lossy(&value).into_owned()),
            event_id: sequence,
            key: incoming.key.clone(),
            request: incoming.request.clone(),
        });
        Ok(())
    }

    /// Take in what the transferee says about a transfer we asked for.
    async fn on_notify(&mut self, incoming: &Incoming) -> Result<()> {
        // Answered first and unconditionally. A NOTIFY we do not understand is still a request
        // that must not be left to time out, and the subscription is ours whether or not this
        // particular notification made sense.
        let ok = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?.build();
        self.endpoint.respond(&incoming.key, ok).await?;

        let is_refer = incoming
            .request
            .headers
            .value(&HeaderName::Event)
            .is_some_and(|value| {
                String::from_utf8_lossy(&value)
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("refer")
            });
        if !is_refer {
            return Ok(());
        }

        let finished = incoming
            .request
            .headers
            .value(&HeaderName::SubscriptionState)
            .is_some_and(|value| is_terminated(&value));

        let state = parse_sipfrag(incoming.request.body())
            .map(|(status, reason)| TransferState::from_status(status, &reason));

        let transfer = self.transfer.get_or_insert(Transfer {
            state: TransferState::Trying,
            finished: false,
        });
        if let Some(state) = state {
            transfer.state = state;
        }
        // Once terminated, always terminated: a stray notification afterwards must not reopen a
        // subscription the transferee has already closed.
        transfer.finished |= finished;
        Ok(())
    }

    /// Report progress on a transfer we accepted.
    async fn notify_transfer(
        &mut self,
        referral: &Referral,
        status: u16,
        reason: &str,
        terminate: bool,
    ) -> Result<()> {
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();
        let subscription = if terminate {
            // `noresource` is the reason RFC 6665 §4.1.3 gives for "the thing you subscribed to
            // no longer exists", which is what a finished transfer is.
            "terminated;reason=noresource".to_owned()
        } else {
            "active;expires=60".to_owned()
        };

        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Notify, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Notify)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            // The `id` ties this to the REFER that created the subscription, so a transferor
            // with two transfers in flight can tell which one this is about (RFC 3515 §2.4.4).
            .header(
                HeaderName::Event,
                Bytes::from(format!("refer;id={}", referral.event_id)),
            )?
            .header(HeaderName::SubscriptionState, Bytes::from(subscription))?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"message/sipfrag;version=2.0"),
            )?
            .max_forwards(70)
            .body(Bytes::from(sipfrag(status, reason)));

        let request = add_routes(builder, &routes)?.build();
        let mut responses = self.endpoint.send(request, self.target.clone()).await?;
        // A NOTIFY the transferor never answers does not undo the transfer; the call it asked
        // for has already happened either way.
        let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
        Ok(())
    }

    /// Refuse a request outright, without involving the application.
    async fn refuse_now(
        &mut self,
        incoming: &Incoming,
        status: u16,
        reason: &'static str,
    ) -> Result<()> {
        let Some(code) = StatusCode::new(status) else {
            return Ok(());
        };
        let response = ResponseBuilder::to_request(&incoming.request, code, reason)?.build();
        self.endpoint.respond(&incoming.key, response).await?;
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
        let mut responses = self.endpoint.send(bye, self.target.clone()).await?;
        // A BYE that is never answered still ends the call locally: the alternative is a call
        // that cannot be hung up because the far end has already gone.
        let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
        Ok(())
    }
}

/// Percent-escape a value going into a URI header field.
///
/// A `Replaces` value contains `;` and `=`, both of which end a URI header in the grammar of
/// RFC 3261 §19.1.1. Left unescaped, the `Refer-To` would be truncated at the first semicolon,
/// the transferee would place an ordinary call, and the transfer would appear to work while the
/// original call was never replaced.
fn escape_uri_header(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'@' => {
                out.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Strip the angle brackets a `Refer-To` almost always carries.
///
/// `Refer-To: <sip:x@y>` and `Refer-To: sip:x@y` are both legal; only the first can carry URI
/// parameters unambiguously, so it is the one everything sends. Any display name before the
/// bracket goes with them.
fn unbracket(value: &str) -> String {
    match (value.find('<'), value.rfind('>')) {
        (Some(open), Some(close)) if close > open => value
            .get(open + 1..close)
            .unwrap_or(value)
            .trim()
            .to_owned(),
        _ => value.to_owned(),
    }
}

/// Add the dialog's route set, in order, as `Route` headers.
///
/// Without these, a request through a Record-Routing proxy — which is to say almost any real
/// deployment — is addressed straight at the peer's `Contact`, which the proxy will not relay
/// and the peer may not be reachable at. The call establishes and cannot be ended.
/// The sequence number of a request, if it has a well-formed `CSeq`.
fn remote_cseq(request: &Request) -> Option<u32> {
    request
        .headers
        .typed::<sipx_sip::headers::CSeq>()
        .and_then(std::result::Result::ok)
        .map(|cseq| cseq.sequence)
}

fn add_routes(
    mut builder: RequestBuilder,
    routes: &[String],
) -> std::result::Result<RequestBuilder, sipx_sip::error::BuildError> {
    for route in routes {
        builder = builder.header(HeaderName::Route, Bytes::from(route.clone()))?;
    }
    Ok(builder)
}

fn bye_request(dialog: &Dialog, cseq: u32) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let builder = RequestBuilder::new(Method::Bye, uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .cseq(cseq, &Method::Bye)?
        .max_forwards(70);
    Ok(add_routes(builder, &routes)?.build())
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
/// The INVITE that opens a call.
///
/// Its own function only because `dial` had grown past the point where the interesting part —
/// what happens to the *response* — was visible among the header construction.
fn build_invite(
    endpoint: &Handle,
    target: &Target,
    to: &Uri,
    from: &str,
    via: &str,
    offer: &SessionDescription,
) -> Result<Request> {
    Ok(RequestBuilder::new(Method::Invite, to.clone())
        .header(HeaderName::Via, Bytes::from(via.to_owned()))?
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
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, target.transport)),
        )?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .max_forwards(70)
        .body(Bytes::from(offer.to_string_sdp()))
        .build())
}

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
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );

    let invite = build_invite(endpoint, &target, to, from, &via, &offer)?;

    let mut responses = endpoint.send(invite.clone(), target.clone()).await?;

    let response = match await_final(&mut responses, options.timeout).await {
        Waited::Final(response) => response,
        Waited::Gone => return Err(Error::NoResponse),
        Waited::GaveUp { provisional } => {
            // Giving up is not just ceasing to wait. The far end is ringing and has been told
            // nothing; without a CANCEL it goes on ringing, and someone answering afterwards
            // ends up in a call with a party that has left.
            //
            // Only once it has answered provisionally, though. RFC 3261 §9.1: with nothing
            // received, "the client MUST wait for the arrival of a provisional response before
            // sending" the CANCEL — one sent first can overtake the INVITE it refers to, match
            // nothing at the far end, and leave the invitation it was meant to withdraw
            // running. So this waits rather than giving up on cancelling, bounded because a
            // peer that never answers at all would otherwise hold the call attempt open.
            if provisional {
                let _ = send_cancel(endpoint, &invite, &via, target.clone()).await;
            }

            // CANCEL cannot close the race it exists to manage: a 200 already in flight
            // arrives anyway, and RFC 3261 §15 says a UAC that will not proceed must
            // acknowledge it and then hang up rather than leave it unanswered.
            let mut cancelled = provisional;
            let grace = tokio::time::Instant::now() + Duration::from_secs(2);
            while let Ok(Some(event)) = tokio::time::timeout_at(grace, responses.next()).await {
                let sipx_sip::transaction::TuEvent::Response(late) = event else {
                    continue;
                };
                if !late.status.is_final() {
                    if !cancelled {
                        cancelled = true;
                        let _ = send_cancel(endpoint, &invite, &via, target.clone()).await;
                    }
                    continue;
                }
                if late.status.is_success()
                    && let Some(dialog) = Dialog::from_response(&invite, &late)
                {
                    let in_dialog = in_dialog_target(&dialog, target.clone());
                    let _ = send_ack(endpoint, &dialog, in_dialog.clone()).await;
                    if let Ok(bye) = bye_request(&dialog, dialog.local_cseq.saturating_add(1)) {
                        let _ = endpoint.send(bye, in_dialog).await;
                    }
                }
                break;
            }
            return Err(Error::Cancelled(options.timeout.unwrap_or(Duration::ZERO)));
        }
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
    match establish(&invite, &response, target.clone(), port) {
        Ok((dialog, media, in_dialog, negotiated)) => {
            let ack = build_ack(endpoint, &dialog, &in_dialog)?;
            endpoint
                .send_directly(ack.clone(), in_dialog.clone())
                .await?;
            // The stream stays open rather than being dropped here: a retransmitted 2xx means
            // this ACK was lost and RFC 3261 §13.2.2.4 requires another (see
            // `reack_retransmitted_2xx`).
            tokio::spawn(reack_retransmitted_2xx(
                endpoint.clone(),
                responses,
                ack,
                in_dialog.clone(),
            ));
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
                referral: None,
                transfer: None,
            })
        }
        Err(error) => {
            // RFC 3261 §15: a UAC that cannot proceed after a 2xx acknowledges it and then
            // sends BYE. Walking away silently is what leaves the far end streaming.
            if let Some(dialog) = Dialog::from_response(&invite, &response) {
                let in_dialog = in_dialog_target(&dialog, target.clone());
                let _ = send_ack(endpoint, &dialog, in_dialog.clone()).await;
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
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, incoming.transport)),
        )?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .body(Bytes::from(answer_sdp.to_string_sdp()))
        .build();

    // Before the 200, not after. An INVITE with no usable `Contact` cannot form a dialog
    // (RFC 3261 §12.1.1), and answering first would put a 2xx on the wire for a call this side
    // is then unable to hold: the caller ACKs, believes it has a confirmed dialog, and streams
    // media at an endpoint that has forgotten it and can never send the BYE.
    let dialog = Dialog::from_request(&incoming.request, &tag).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));

    endpoint.respond(&incoming.key, response.clone()).await?;

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
        referral: None,
        transfer: None,
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

/// What waiting for a final response ended in.
enum Waited {
    /// A final response arrived.
    Final(Response),
    /// The deadline passed. Whether a provisional had arrived decides what may be done about
    /// it, so it is carried out rather than discarded.
    GaveUp {
        /// Whether the far end had answered provisionally.
        provisional: bool,
    },
    /// The transaction ended without a final response.
    Gone,
}

/// Wait for the final response to an INVITE, remembering whether a provisional arrived.
///
/// The provisional is not incidental bookkeeping: RFC 3261 §9.1 forbids cancelling an
/// invitation the far end has not answered provisionally, so the deadline alone does not
/// decide what to do when the wait runs out — what came back matters too.
async fn await_final(responses: &mut sipx_transport::Responses, limit: Option<Duration>) -> Waited {
    let deadline = limit.map(|limit| tokio::time::Instant::now() + limit);
    let mut provisional = false;
    loop {
        let event = match deadline {
            None => responses.next().await,
            Some(deadline) => match tokio::time::timeout_at(deadline, responses.next()).await {
                Ok(event) => event,
                Err(_elapsed) => return Waited::GaveUp { provisional },
            },
        };
        match event {
            Some(sipx_sip::transaction::TuEvent::Response(response)) => {
                if response.status.is_final() {
                    return Waited::Final(*response);
                }
                provisional = true;
            }
            Some(_) => {}
            None => return Waited::Gone,
        }
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

/// Acknowledge a 2xx (RFC 3261 §13.2.2.4).
///
/// This ACK is not part of the INVITE transaction and has no transaction of its own: it is
/// "passed to the transport layer directly for transmission", carries a *new* branch because
/// any proxy must treat it as a new request, and is resent only when a retransmitted 2xx
/// arrives. Handing it to the transaction layer instead earns it the retransmission timers of
/// a non-INVITE request — a stream of duplicate ACKs toward a response that will never come,
/// and a spurious timeout 32 seconds into a call that is up and talking.
async fn send_ack(endpoint: &Handle, dialog: &Dialog, target: Target) -> Result<()> {
    let ack = build_ack(endpoint, dialog, &target)?;
    endpoint.send_directly(ack, target).await?;
    Ok(())
}

/// Keep acknowledging a 2xx for as long as the far end keeps retransmitting it.
///
/// RFC 3261 §13.2.2.4: the UAC core "MUST generate an ACK for each 2xx received", and a
/// retransmitted 2xx says the previous ACK never arrived. Nobody else can do this — the INVITE
/// transaction has already passed the response up, and RFC 6026's `Accepted` state exists so
/// that the retransmission still has somewhere to arrive. Dropping the stream after the first
/// answer leaves the far end retransmitting for 64*T1 and then tearing down, from its side, a
/// call this side believes is established and is already sending audio into.
///
/// The same ACK goes out each time, rather than a freshly built one: it acknowledges one
/// response, and a new branch on every repeat would present each as a new request.
async fn reack_retransmitted_2xx(
    endpoint: Handle,
    mut responses: sipx_transport::Responses,
    ack: Request,
    target: Target,
) {
    while let Some(event) = responses.next().await {
        if let sipx_sip::transaction::TuEvent::Response(response) = event
            && response.status.is_success()
            && endpoint
                .send_directly(ack.clone(), target.clone())
                .await
                .is_err()
        {
            return;
        }
    }
}

fn build_ack(endpoint: &Handle, dialog: &Dialog, target: &Target) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );
    let ack = RequestBuilder::new(Method::Ack, uri)
        .header(HeaderName::Via, Bytes::from(via))?
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        // The ACK for a 2xx carries the INVITE's sequence number, not a new one: it
        // acknowledges that request rather than being one of its own.
        .cseq(dialog.local_cseq, &Method::Ack)?
        .max_forwards(70);
    Ok(add_routes(ack, &routes)?.build())
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
    // Over a WebSocket the `Contact` is not consulted at all. RFC 7118 §5.2: the peer has no
    // listening port, its `Contact` names something that will never resolve, and the connection
    // the dialog was established on is the only way to reach it. This is the RFC 5923 rule for
    // stream transports made absolute — there is no fallback because there is nowhere to fall
    // back to, and honouring a `Contact` here would send the BYE to an address that either does
    // not answer or belongs to somebody else.
    if matches!(fallback.transport, TransportKind::Ws | TransportKind::Wss) {
        return fallback;
    }

    // The first hop, which is the remote target only when the dialog has no route set. With
    // one they are different elements, and RFC 3261 §12.2.1.1 hands the request to the former.
    let hop = dialog.hop();
    let Some(sipx_sip::Host::Ip(ip)) = hop.host() else {
        return fallback;
    };
    let transport = hop
        .transport()
        .and_then(TransportKind::parse)
        .unwrap_or(fallback.transport);
    let port = hop.port().unwrap_or_else(|| transport.default_port());
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

/// The `Contact` this endpoint should advertise for a dialog on this transport.
///
/// Built from the endpoint's *advertised* address rather than its socket's local one. An
/// endpoint bound to `0.0.0.0` has a local address that means nothing to a peer, and behind a
/// NAT it is private — either way the peer stores it as the dialog's remote target and every
/// in-dialog request it sends becomes unroutable.
///
/// The transport matters because over a WebSocket there is no address to advertise at all: the
/// endpoint gives the invented name RFC 7118 §5.2 requires, and marks the URI with the same
/// transport token it puts in the `Via`, so a peer that does route on `Contact` knows not to
/// try `sip:` on port 5060.
#[must_use]
pub fn contact_for(endpoint: &Handle, transport: TransportKind) -> String {
    match transport {
        TransportKind::Ws | TransportKind::Wss => format!(
            "<sip:sipx@{};transport={}>",
            endpoint.sent_by_for(transport),
            transport.as_str().to_ascii_lowercase()
        ),
        _ => format!("<sip:sipx@{}>", endpoint.advertised()),
    }
}

/// Answer an INVITE that asks to take the place of an existing call (RFC 3891).
///
/// The second half of an attended transfer: the transferor has spoken to the target, and hands
/// its original call over by telling one party to call the other with a `Replaces` header
/// naming the dialog to displace.
///
/// **The header must name `replaced`, all three fields of it.** A `Call-ID` travels in every
/// message of a dialog and is visible to every element on the path; the tags are random and
/// known only to the two parties. Accepting a match on the `Call-ID` alone — or trusting the
/// caller to have checked — turns this into a call-hijack primitive, so the check is here and
/// not in whoever calls it.
///
/// On success the replaced call is hung up and its media torn down. On failure the new INVITE
/// is refused and the existing call is left exactly as it was: a replacement that cannot be
/// honoured must not cost the user the call they already had.
pub async fn answer_replacing(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    replaced: &mut Call,
) -> Result<Call> {
    let Some(asked_for) = Replaces::of(&incoming.request) else {
        refuse_request(endpoint, incoming, 400, "Bad Request").await?;
        return Err(Error::NoReplaces);
    };

    if !asked_for.matches(&replaced.dialog) {
        // 481, which RFC 3891 §3 asks for and which also gives nothing away: a caller guessing
        // tags gets the same answer whether the Call-ID was right or not, so there is nothing
        // to search.
        refuse_request(endpoint, incoming, 481, "Call/Transaction Does Not Exist").await?;
        return Err(Error::NoReplaces);
    }

    // Answer first. If this fails the old call is untouched, which is the right way round:
    // hanging up first and then failing to answer would leave the user with no call at all.
    let taken_over = answer(endpoint, incoming, media_address).await?;

    // Then end the one being replaced (RFC 3891 §3). Its media stops with it.
    let _ = replaced.hang_up().await;

    Ok(taken_over)
}

/// Refuse a request outright.
async fn refuse_request(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
) -> Result<()> {
    let Some(code) = StatusCode::new(status) else {
        return Ok(());
    };
    let response = ResponseBuilder::to_request(&incoming.request, code, reason)?.build();
    endpoint.respond(&incoming.key, response).await?;
    Ok(())
}
