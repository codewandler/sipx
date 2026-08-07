//! In-dialog renegotiation: the re-INVITE and UPDATE machinery of an established call.
//!
//! Inbound, a renegotiation that fails leaves the call running — 488 and carry on (RFC 3311
//! §5.2, `M-8`). Outbound, one engine builds every re-offer; hold, resume and an ICE restart
//! differ only in the direction and the ICE half they hand it.

use super::{
    Arc, Bytes, Call, CallEvent, CancellationToken, Direction, Error, HeaderName, IceOffer,
    Incoming, Keying, MediaPort, MediaProfile, Method, Negotiated, OwnedTask, Reception,
    RequestBuilder, ResponseBuilder, Result, SessionDescription, SessionExpires, SocketAddr,
    Target, add_routes, contact_for, exchanged_rtcp_mode, in_dialog_target, negotiated, offer_from,
    ok_status, preserve_rtcp_mode, required_interval, retransmit_until_acked, send_ack, session,
    settle_answer, update,
};

/// The pure half of accepting a peer's in-dialog offer.
///
/// Kept as one value so [`Call::can_accept_offer`] and [`Call::renegotiate`] cannot drift: the
/// coupling asks the first question before it changes its other leg, and the call later applies
/// exactly the description that passed that check.
struct PreparedRenegotiation {
    offer: SessionDescription,
    negotiated: Negotiated,
    answer: SessionDescription,
    direction: Direction,
}

impl Call {
    /// Renegotiate an established call from a re-INVITE.
    ///
    /// The rule that shapes this: **a renegotiation that fails must leave the call running.** A
    /// re-INVITE tries to change something about a call that already works, so answering 488
    /// and carrying on is right; tearing the call down because the new offer was unusable
    /// would lose a call that was fine a moment ago.
    pub(super) async fn on_reinvite(&mut self, incoming: &Incoming) -> Result<()> {
        if self.out_of_order(&incoming.request) {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        self.record_remote_cseq(&incoming.request);

        if incoming.request.body().is_empty() {
            return self.offer_in_reinvite_success(incoming).await;
        }

        // §5.2 rule 2's other source, and the reason the spec names INVITE alongside UPDATE: a
        // re-INVITE's offer is one this side owes an answer to until it produces one.
        if crate::update::carries_offer(&incoming.request) {
            self.negotiation.received_offer();
        }
        let renegotiated = self.renegotiate(incoming.request.body()).await;
        // On every path out of here the debt is settled: a 488 kills the offer and a 2xx
        // answers it, and a failure to renegotiate at all leaves nothing to answer.
        self.negotiation.sent_answer();
        let Some(answer_sdp) = renegotiated? else {
            return self.refuse_unacceptable(incoming).await;
        };

        // RFC 4028 §7.2: any re-INVITE inside the dialog refreshes the session, whether or not
        // it was sent for that reason. Only counting the ones that carry `Session-Expires`
        // would hang up on a peer that is demonstrably alive and talking to us.
        self.rearm();

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
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
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
        self.stop_ack_retransmission().await;
        let ack_stop = CancellationToken::new();
        let ack_retransmission = tokio::spawn(retransmit_until_acked(
            self.endpoint.clone(),
            incoming.key.clone(),
            response,
            ack_stop.clone(),
        ));
        self.ack_stop = Some(ack_stop);
        self.ack_retransmission = Some(OwnedTask::new(ack_retransmission));
        Ok(())
    }

    /// Put our offer in the 2xx to a bodyless re-INVITE (RFC 3261 §14.2).
    async fn offer_in_reinvite_success(&mut self, incoming: &Incoming) -> Result<()> {
        if self.profile == MediaProfile::BrowserAudio
            || self.encrypted
            || !self.negotiation.may_offer()
        {
            return self.refuse_unacceptable(incoming).await;
        }

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        capabilities.direction = self.hold;
        capabilities.session_version = self
            .dialog
            .remote_cseq
            .map_or(u64::from(self.dialog.local_cseq), u64::from);
        let mut offer = offer_from(&capabilities);
        self.offer_ice(&mut offer, IceOffer::Continue).await;

        // The request itself is a target refresh even though its offer was delayed.
        self.rearm();
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
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(offer.to_string_sdp()))
            .build();

        self.negotiation.sent_offer();
        self.delayed_offer = Some(capabilities);
        if let Err(error) = self.endpoint.respond(&incoming.key, response.clone()).await {
            self.delayed_offer = None;
            self.negotiation.received_answer();
            return Err(error.into());
        }

        self.stop_ack_retransmission().await;
        let ack_stop = CancellationToken::new();
        let ack_retransmission = tokio::spawn(retransmit_until_acked(
            self.endpoint.clone(),
            incoming.key.clone(),
            response,
            ack_stop.clone(),
        ));
        self.ack_stop = Some(ack_stop);
        self.ack_retransmission = Some(OwnedTask::new(ack_retransmission));
        Ok(())
    }

    /// Settle the answer carried by the ACK of a delayed-offer re-INVITE.
    pub(super) async fn accept_delayed_offer_answer(&mut self, body: &[u8]) -> Result<()> {
        let Some(offered) = self.delayed_offer.take() else {
            return Ok(());
        };
        // Clear the exchange on every path: ACK has no response with which to repair a malformed
        // answer, and retaining this flag would turn one peer error into permanent glare.
        self.negotiation.received_answer();

        let answer = sipx_sdp::parse(&String::from_utf8_lossy(body))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        let settled = settle_answer(&offered, &answer, self.codecs)?;
        preserve_rtcp_mode(self.current.rtcp_mode, settled.negotiated.rtcp_mode)?;
        self.accept_answer_ice(&answer).await;
        self.move_media_if_changed(settled.negotiated).await
    }

    /// Apply an offer that arrived in-dialog, and produce the answer to send back.
    ///
    /// `None` means the description is unusable and the caller must refuse — 488 for a
    /// re-INVITE (`M-8`) and for an UPDATE (RFC 3311 §5.2), which is the same rule for the same
    /// reason: **a renegotiation that fails leaves the call running.** Both requests try to
    /// change something that already works, so refusing the change and keeping the session is
    /// right; tearing the call down because the new offer was unusable would lose a call that
    /// was fine a moment ago.
    ///
    /// Shared by the two paths because they ask exactly the same question of exactly the same
    /// session and differ only in what carries the answer back.
    fn prepare_renegotiation(&self, body: &[u8]) -> Option<PreparedRenegotiation> {
        if self.keying == Keying::DtlsSrtp {
            return None;
        }
        let offer = sipx_sdp::parse(&String::from_utf8_lossy(body)).ok()?;
        let mut negotiated = negotiated(&offer, self.codecs).ok()?;

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        let answer = sipx_sdp::answer(&offer, &capabilities);
        if answer
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return None;
        }
        let proposed_mode = exchanged_rtcp_mode(&offer, &answer);
        // A muxed session owns one receive socket. Answering an offer that removed mux while
        // silently retaining that owner would put the wire and the running state in disagreement.
        // Refuse the offer with 488; the typed error is shared with the outbound paths below.
        preserve_rtcp_mode(self.current.rtcp_mode, proposed_mode).ok()?;
        negotiated.rtcp_mode = proposed_mode;
        let direction = offer
            .media
            .iter()
            .find(|media| media.media == "audio" && !media.is_rejected())
            .map(sipx_sdp::MediaDescription::direction)?;
        Some(PreparedRenegotiation {
            offer,
            negotiated,
            answer,
            direction,
        })
    }

    /// Whether this call can answer an in-dialog offer, without changing call or media state.
    ///
    /// The coupling uses this before opening an exchange on its other leg. Syntax alone is not
    /// enough: the source call may have no common codec or may use DTLS-SRTP, whose renegotiation
    /// this layer deliberately refuses.
    pub(crate) fn can_accept_offer(&self, body: &[u8]) -> Option<Direction> {
        self.prepare_renegotiation(body)
            .map(|prepared| prepared.direction)
    }

    async fn renegotiate(&mut self, body: &[u8]) -> Result<Option<SessionDescription>> {
        let Some(mut prepared) = self.prepare_renegotiation(body) else {
            return Ok(None);
        };
        self.answer_ice(&prepared.offer, &mut prepared.answer).await;

        // Hold is a direction, not a separate state: `sendonly` or `inactive` from the far end
        // means it will not play what we send.
        let was_on_hold = self.is_on_hold();
        self.hold = prepared.direction;
        // Emitted right where `hold` changes, not by polling it afterwards — a renegotiation
        // that does not change the direction (a keep-alive, say) must not report a hold that
        // never happened.
        match (was_on_hold, self.is_on_hold()) {
            (false, true) => self.events.emit(CallEvent::Hold),
            (true, false) => self.events.emit(CallEvent::Resumed),
            _ => {}
        }

        self.move_media_if_changed(prepared.negotiated).await?;
        Ok(Some(prepared.answer))
    }

    /// Answer an UPDATE that arrived in this dialog (RFC 3311 §5.2).
    ///
    /// The three refusals are three different answers, and the difference is the point: 491
    /// means the two sides collided and both should wait a randomised interval before trying
    /// again; a 500 with `Retry-After` means the request was well formed and simply early. A
    /// peer told the wrong one either backs off when it did not need to or retries straight
    /// into the same wall.
    ///
    /// Whichever it is, **the dialog survives** — including the 488 for a description this side
    /// cannot use. Every one of these is about a change that will not happen, not about the
    /// session that is already running.
    pub(super) async fn on_update(&mut self, incoming: &Incoming) -> Result<()> {
        if self.out_of_order(&incoming.request) {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        self.record_remote_cseq(&incoming.request);

        let has_offer = crate::update::carries_offer(&incoming.request);
        if let Reception::Refuse(refusal) = self.negotiation.receive(has_offer) {
            return crate::update::refuse(&self.endpoint, incoming, refusal).await;
        }

        let mut builder = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?;

        if has_offer {
            // §5.2: the UAS "MUST adjust the session parameters accordingly and generate an
            // answer in the 2xx response".
            //
            // The result is captured rather than propagated with `?`, because `renegotiate`
            // can fail on something that has nothing to do with the peer — a media port that
            // will not bind. Returning through the `?` would leave this UPDATE forever in
            // progress and the offer forever owed, and every later UPDATE on the dialog would
            // draw §5.2's "you are too early" for a transaction nobody is waiting on.
            let renegotiated = self.renegotiate(incoming.request.body()).await;
            let Some(answer_sdp) = renegotiated.inspect_err(|_| self.negotiation.answered())?
            else {
                // The offer is dead, so nothing is owed for it any more — and this is a final
                // response, so no UPDATE is in progress either.
                self.negotiation.answered();
                return self.refuse_unacceptable(incoming).await;
            };
            builder = builder
                .header(
                    HeaderName::ContentType,
                    Bytes::from_static(b"application/sdp"),
                )?
                .body(Bytes::from(answer_sdp.to_string_sdp()));
        }

        // RFC 4028 §7.4: an UPDATE refreshes the session whether or not it was sent for that
        // reason, so the 2xx names the terms in force and the deadline moves. Only counting the
        // ones that carry `Session-Expires` would hang up on a peer that is demonstrably alive.
        if let Some(state) = self.session {
            let expires = SessionExpires {
                interval: state.terms.interval,
                refresher: Some(if state.terms.we_refresh {
                    session::Refresher::Uas
                } else {
                    session::Refresher::Uac
                }),
            };
            builder = builder
                .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
                .header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        }

        // §5.1: UPDATE is a target refresh request, so its `Contact` replaces the dialog's
        // remote target — the same rule RFC 3261 §12.2.2 gives a re-INVITE, and for the same
        // reason: without it the BYE goes to where the peer used to be.
        self.dialog.refresh_target(&incoming.request.headers);
        self.target = in_dialog_target(
            &self.dialog,
            Target::new(incoming.source, incoming.transport),
        );
        self.peer_allows_update = update::peer_allows(&incoming.request.headers);

        let sent = self.endpoint.respond(&incoming.key, builder.build()).await;
        // Cleared whether or not the response got out. A send that failed will not be retried
        // here, so leaving the exchange open would answer every later UPDATE on this dialog
        // with §5.2's "you are too early" — permanently, for a transaction nobody is waiting
        // on any more.
        self.negotiation.answered();
        sent?;
        self.rearm();
        Ok(())
    }

    /// Rebuild the media session, but only if where or how the media flows actually changed.
    ///
    /// Restarting an unchanged session would drop packets for no reason on every re-INVITE, and
    /// some peers send one every thirty seconds as a keep-alive.
    pub(super) async fn move_media_if_changed(&mut self, to: Negotiated) -> Result<()> {
        self.reap_retired_media().await;
        // The payload type is the codec's number on the wire: a re-offer can move Opus from
        // 111 to 96 and leave the codec unchanged, and a session not rebuilt for that goes on
        // sending on the number the far end just reassigned.
        //
        // Compared as the *wire* number, not as the raw `Option`. A peer may add or drop the
        // redundant `a=rtpmap:0 PCMU/8000` between two descriptions of the same static codec, and
        // `Some(0)` against `None` would read as a change when nothing changed — rebuilding the
        // session, and dropping audio, on a re-INVITE that only reworded the SDP.
        if to.remote != self.current.remote
            || to.codec != self.current.codec
            || to.clock_rate != self.current.clock_rate
            || to.wire_payload_type() != self.current.wire_payload_type()
            || to.receive_wire_payload_type() != self.current.receive_wire_payload_type()
            || to.rtcp_mode != self.current.rtcp_mode
        {
            let port = MediaPort::bind(SocketAddr::new(self.media_bind_address, 0))
                .await
                .map_err(Error::Io)?;
            let replacement = port.start(to.media_config())?;
            // Mute is a property of the call, not of the session that happens to be carrying it
            // (`M-18`). Without this a re-INVITE that moves the media — the far end changing
            // address or codec, which this side did not ask for and cannot refuse — unmutes the
            // call behind the application's back.
            replacement.set_muted(self.media.is_muted());
            replacement.set_rtcp_quality_hook(self.media.rtcp_quality_hook());
            let previous = std::mem::replace(&mut self.media, Arc::new(replacement));
            self.retired_media.push(previous);
            self.reap_retired_media().await;
        }
        self.current = to;
        Ok(())
    }

    /// The re-INVITE both public entry points send, and the one place their difference lives.
    ///
    /// `ice` is a parameter rather than a field on [`Call`] because it is a property of *this*
    /// offer and of nothing else. Held as state it would be a fourth `bool` on a struct that
    /// already has three — which is what `clippy::struct_excessive_bools` objects to, and the
    /// objection is right: a flag set before a call and cleared after it is a state machine
    /// written in the hardest way to read.
    pub(super) async fn reoffer(&mut self, direction: Direction, ice: IceOffer) -> Result<()> {
        if self.profile == MediaProfile::BrowserAudio {
            return Err(sipx_sdp::browser_audio::ProfileError::ProfileRemoved.into());
        }
        if self.keying == Keying::DtlsSrtp {
            return Err(Error::DtlsRenegotiation);
        }
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        capabilities.direction = direction;
        // The session version must increase with each modified offer, so the far end can tell
        // a changed description from a repeated one.
        capabilities.session_version = u64::from(cseq);
        let mut offer = offer_from(&capabilities);
        self.offer_ice(&mut offer, ice).await;

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
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .max_forwards(70)
            .body(Bytes::from(offer.to_string_sdp()));

        // RFC 4028 §7.4: a refresh names the current interval and the current refresher, so
        // that proxies on the path can see the value in force and object to it. Any re-INVITE
        // refreshes the session (§7.2), so these go on every one rather than only on the ones
        // sent because the timer asked.
        let mut builder = builder.header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        if let Some(state) = self.session {
            let expires = SessionExpires {
                interval: state.terms.interval,
                refresher: Some(if state.terms.we_refresh {
                    session::Refresher::Uac
                } else {
                    session::Refresher::Uas
                }),
            };
            builder = builder
                .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
                .header(
                    HeaderName::MinSe,
                    Bytes::from(session::ABSOLUTE_MIN_INTERVAL.as_secs().to_string()),
                )?;
        }

        let request = add_routes(builder, &routes)?.build();
        // RFC 3311 §5.2 rule 2 names an offer sent "in an UPDATE, PRACK or INVITE", and this is
        // the INVITE case: the offer is outstanding for as long as the response takes. Marked
        // and cleared around the whole exchange, so a failure cannot leave the flag set and
        // refuse every later offer of ours.
        self.negotiation.sent_offer();
        let exchange = async {
            let mut responses = self.endpoint.send(request, self.target.clone()).await?;
            responses.final_response().await.ok_or(Error::NoResponse)
        }
        .await;
        self.negotiation.received_answer();
        let response = exchange?;

        if !response.status.is_success() {
            // The far end refused the change. The call it refused to change is still running,
            // so this is an error about the renegotiation, not about the call.
            const INTERVAL_TOO_SMALL: u16 = 422;
            if response.status.code() == INTERVAL_TOO_SMALL
                && let Some(required) = required_interval(&response)
                && let Some(state) = self.session.as_mut()
            {
                // §10: only a 2xx extends the expiration, so adopting the longer interval does
                // *not* buy time — the refresh still has to succeed before the deadline that
                // was already running. The next attempt is the one that must land.
                state.terms.interval = required.max(session::ABSOLUTE_MIN_INTERVAL);
            }
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

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body())) {
            // The answer's ICE half, before the codec comparison: on a restart it carries the
            // peer's new credentials and candidates, and an agent that is not told about them
            // checks a path nobody is answering on. On an ordinary re-offer it is the same half
            // again, which the agent merges (RFC 8839 §4.2) rather than replaces — so a
            // re-answer cannot silence ICE on a call that is working.
            if let Ok(settled) = settle_answer(&capabilities, &answer, self.codecs) {
                preserve_rtcp_mode(self.current.rtcp_mode, settled.negotiated.rtcp_mode)?;
                self.accept_answer_ice(&answer).await;
                self.move_media_if_changed(settled.negotiated).await?;
            }
        }
        self.hold = direction;
        // §7.2: the session expiration is measured from the 2xx, and a re-INVITE sent for any
        // other reason refreshes it just the same.
        self.adopt_session(&response);
        self.rearm();
        Ok(())
    }
}
