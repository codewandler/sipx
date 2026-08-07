//! Call transfer: REFER out and in, its NOTIFY progress, and Replaces (RFC 3515, RFC 3891).
//!
//! A REFER is the one in-dialog request the stack does not answer on its own — only the
//! application knows whether a call may be placed on its behalf, so the two answers are
//! [`Call::accept_referral`] and [`Call::refuse_referral`], and until one is given the
//! transferor waits.

use super::*;

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
                // discard: nothing can be lost. `write!` into a `String` returns `fmt::Error`
                // only if the formatter itself fails, and `String`'s never does — there is no
                // I/O and no allocation failure path to report.
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Whether a `Refer-To` value carries a `Replaces` header parameter — RFC 3891's marker of an
/// attended transfer, as [`Call::refer_attended`] builds it (`<target>?Replaces=...`).
///
/// A substring check rather than a full URI-header parse: `Uri` does not expose its header
/// component, and telling "asks to replace a dialog" from "does not" is all this needs to do.
fn contains_replaces(value: &[u8]) -> bool {
    String::from_utf8_lossy(value)
        .to_ascii_lowercase()
        .contains("replaces=")
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
///
/// Answers from the default codec set, [`Codecs::G711`]. [`answer_replacing_with`] takes a
/// selection.
pub async fn answer_replacing(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    replaced: &mut Call,
) -> Result<Call> {
    answer_replacing_with(
        endpoint,
        incoming,
        media_address,
        replaced,
        Codecs::default(),
    )
    .await
}

/// [`answer_replacing`], from a chosen codec set rather than the default one (`M-30`).
///
/// `codecs` applies to the *replacement*, which is the only call being negotiated here. The one
/// being replaced is hung up, and nothing renegotiates it on the way out.
pub async fn answer_replacing_with(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    replaced: &mut Call,
    codecs: Codecs,
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
    let taken_over = answer_with(endpoint, incoming, media_address, codecs).await?;

    // Then end the one being replaced (RFC 3891 §3). Its media stops with it.
    //
    // discard: the BYE this sends is counted at the transmit as
    // `sipx_transport::UnsentCounts::bye` if the endpoint cannot put it on the wire. The `Result`
    // is discarded because the takeover has already succeeded on the line above and reporting a
    // teardown failure as the *transfer* failing would be false — the caller has the new call
    // either way, and `Call::end` has already marked the old one ended locally before the BYE was
    // ever built.
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

impl Call {
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
    pub(super) async fn on_refer(&mut self, incoming: &Incoming) -> Result<()> {
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

        // An attended transfer's `Refer-To` carries a `Replaces` (RFC 3891 + 3515), built by
        // `refer_attended` above as a URI header parameter. A substring check on the raw value
        // rather than a full URI-header parse: `Uri` does not expose its header component, and
        // this only has to distinguish "asks to replace a dialog" from "does not", not validate
        // one.
        let attended = refer_to.as_deref().is_some_and(contains_replaces);

        self.referral = Some(Referral {
            target: target.clone(),
            referred_by: incoming
                .request
                .headers
                .value(&HeaderName::ReferredBy)
                .map(|value| String::from_utf8_lossy(&value).into_owned()),
            event_id: sequence,
            key: incoming.key.clone(),
            request: incoming.request.clone(),
        });
        self.events
            .emit(CallEvent::TransferRequested { target, attended });
        Ok(())
    }

    /// Take in what the transferee says about a transfer we asked for.
    pub(super) async fn on_notify(&mut self, incoming: &Incoming) -> Result<()> {
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
            transfer.state = state.clone();
            self.events.emit(CallEvent::TransferProgress(state));
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
        //
        // discard: nothing is thrown away that anyone could act on. The NOTIFY itself was handed
        // over — one the endpoint could not put on the wire is counted at the transmit by
        // `sipx_transport::UnsentCounts` — and what is dropped here is only *waiting* for its
        // answer. The bound bounds a failure (`X-29`): the transfer's outcome does not depend on
        // the reply arriving.
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
}
