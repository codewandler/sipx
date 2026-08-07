//! Hold and resume: changing a call's negotiated direction (RFC 3264).
//!
//! Hold is a direction, not a separate state — `sendonly` or `inactive` puts a call on hold
//! and `sendrecv` takes it off, by re-INVITE or by UPDATE. What the far end last signalled is
//! read back with [`Call::is_on_hold`], and mute is deliberately not here: it never signals.

use super::ice::IceOffer;
use super::*;

impl Call {
    /// Whether the far end has put the call on hold.
    #[must_use]
    pub fn is_on_hold(&self) -> bool {
        !self.hold.receives()
    }

    /// Renegotiate this call with an UPDATE (RFC 3311).
    ///
    /// [`Self::reinvite`] remains the right way to renegotiate a *confirmed* dialog — §5.1
    /// recommends it, because an UPDATE must be answered promptly and leaves the far end no
    /// window in which to ask a user whether the change is acceptable. This is here for the
    /// cases where that does not apply: a peer that asked for UPDATE, or a change that nobody
    /// would be asked about.
    ///
    /// Refuses locally rather than putting an illegal request on the wire when an offer of ours
    /// is unanswered or one of theirs is unanswered by us (§5.1, RFC 3264): the far end would
    /// answer 491 or 500 and the round trip would have told us only what we already knew.
    pub async fn update(&mut self, direction: Direction) -> Result<()> {
        if self.profile == MediaProfile::BrowserAudio {
            return Err(sipx_sdp::browser_audio::ProfileError::ProfileRemoved.into());
        }
        if self.keying == Keying::DtlsSrtp {
            return Err(Error::DtlsRenegotiation);
        }
        if !self.negotiation.may_offer() {
            return Err(Error::Rejected {
                status: sipx_sip::update::Refusal::Glare.status(),
                reason: "an offer is already outstanding on this dialog".to_owned(),
            });
        }

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        capabilities.direction = direction;
        // As for a re-INVITE: the version must increase with each modified offer, so the far
        // end can tell a changed description from a repeated one.
        capabilities.session_version = u64::from(self.dialog.local_cseq.saturating_add(1));
        let offer = offer_from(&capabilities);

        let (builder, routes) =
            crate::update::request(&self.endpoint, &mut self.dialog, &self.target, Some(&offer))?;
        let request = crate::update::finish(builder, &routes)?;

        self.negotiation.sent_offer();
        let response = crate::update::send(&self.endpoint, request, self.target.clone()).await;
        // Whatever came back closed the exchange: a 2xx carries the answer, and a failure means
        // there will never be one. Leaving the flag set would refuse every later offer of ours.
        self.negotiation.received_answer();
        let response = response?;
        if !response.status.is_success() {
            return Err(crate::update::rejected(&response));
        }

        self.dialog.refresh_target(&response.headers);
        self.target = in_dialog_target(&self.dialog, self.target.clone());
        self.peer_allows_update = update::peer_allows(&response.headers);

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body())) {
            // The answer's ICE half, before the codec comparison: on a restart it carries the
            // peer's new credentials and candidates, and an agent that is not told about them
            // checks a path nobody is answering on. On an ordinary re-offer it is the same half
            // again, which the agent merges (RFC 8839 §4.2) rather than replaces — so a
            // re-answer cannot silence ICE on a call that is working.
            if let Ok(renegotiated) = negotiated(&answer, self.codecs) {
                preserve_rtcp_mode(self.current.rtcp_mode, renegotiated.rtcp_mode)?;
                // Do not let an answer that failed the mode guard mutate the running ICE
                // generation. Socket ownership and candidate state move together or neither does.
                self.accept_answer_ice(&answer).await;
                self.move_media_if_changed(renegotiated).await?;
            }
        }
        self.hold = direction;
        self.adopt_session(&response);
        self.rearm();
        Ok(())
    }

    /// Send a re-INVITE renegotiating this call.
    ///
    /// `direction` puts the call on hold (`SendOnly` or `Inactive`) or takes it off
    /// (`SendRecv`).
    ///
    /// Note what hold is **not**: RFC 8839 §4.4.1.1.1 makes `c=0.0.0.0` imply an ICE restart, so a
    /// hold spelled with a null connection address would restart ICE on every mute. Hold here is a
    /// direction and nothing else (RFC 3264), which is what it has always been, and this is the
    /// story that makes that a decision rather than an accident.
    pub async fn reinvite(&mut self, direction: Direction) -> Result<()> {
        self.reoffer(direction, IceOffer::Continue).await
    }
}
