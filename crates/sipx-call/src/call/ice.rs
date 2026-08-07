//! ICE on an established call: restarts and the re-signalled halves (RFC 8445 §9, RFC 8839 §4.4).
//!
//! A stream doing ICE restates this side's half in every later offer and answer, and a restart
//! is nothing but both credentials changing (RFC 8839 §4.4.1.1.1). The initial gathering lives
//! with the dial and answer paths in `call.rs`; this is the running call's share.

use super::*;

/// Put one gathered local description into the audio stream it belongs to.
pub(super) fn add_ice(
    description: &mut SessionDescription,
    local: &LocalDescription,
    additional: &[sipx_sdp::Attribute],
) {
    let Some(default) = local.default_destination(ComponentId::RTP) else {
        return;
    };
    description.connection = Some(Connection::new(default.ip()));
    if let Some(audio) = description.media.first_mut() {
        audio.port = default.port();
        if let Some(control) = local.default_destination(ComponentId::RTCP) {
            let address_type = if control.is_ipv6() { "IP6" } else { "IP4" };
            audio.attributes.push(sipx_sdp::Attribute::valued(
                "rtcp",
                format!("{} IN {address_type} {}", control.port(), control.ip()),
            ));
        }
        audio.attributes.extend(local.attributes());
        audio.attributes.extend_from_slice(additional);
    }
}

/// The `a=` names RFC 8839 §5 gives ICE, so a later description can replace its own half.
///
/// Replaced rather than appended: `sipx_sdp::answer` copies the stream it is answering, so an
/// answer built from an offer that carried ICE starts out holding the *peer's* `ice-ufrag`,
/// `ice-pwd` and candidates. Extending that with ours would produce a description claiming both
/// sets, and a peer reading the first `ice-ufrag` it finds would key its checks to its own
/// credentials.
const ICE_ATTRIBUTES: &[&str] = &[
    "ice-ufrag",
    "ice-pwd",
    "ice-options",
    "ice-lite",
    "ice-pacing",
    "candidate",
    "remote-candidates",
];

/// Whether an attribute is one of the ICE names a later description restates.
///
/// `ice-mismatch` is deliberately **not** here. RFC 8839 §5.3 makes it a statement about the
/// exchange rather than a parameter of this side's ICE session, and a stream that carries it is
/// one ICE is not running for at all.
fn is_ice_attribute(attribute: &sipx_sdp::Attribute) -> bool {
    ICE_ATTRIBUTES
        .iter()
        .any(|name| attribute.name.eq_ignore_ascii_case(name))
}

/// This side's ICE half for a later offer or answer (RFC 8839 §4.4; `ice.md` §13.5).
///
/// The same three lines an initial description carries, from the agent rather than from the
/// gathering that has long since finished — `ice2` included, because §13.5's re-signalling has to
/// restate the whole half and a peer that stopped seeing `ice-options` would read it as a change.
fn ice_attributes(local: &sipx_media::ice::Local) -> Vec<sipx_sdp::Attribute> {
    let mut attributes = vec![
        sipx_sdp::Attribute::valued("ice-ufrag", local.credentials.ufrag()),
        sipx_sdp::Attribute::valued("ice-pwd", local.credentials.pwd()),
        sipx_sdp::Attribute::valued("ice-options", sipx_sdp::ice::ICE2),
    ];
    attributes.extend(
        local
            .candidates
            .iter()
            .map(|candidate| sipx_sdp::Attribute::valued("candidate", candidate.to_value())),
    );
    attributes
}

/// What a later offer says about the ICE session already running (RFC 8839 §4.4; `ice.md` §13.5).
///
/// Two variants and not a `bool`, because the wire difference between them is not a flag: a
/// continuing offer restates the credentials in force, and a restart states new ones. §4.4.1.1.1
/// makes *that change* the entire signal, so there is nothing else for either variant to set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IceOffer {
    /// Restate this side's half unchanged — hold, resume, a codec change, a session refresh.
    Continue,
    /// Draw new credentials and a new tiebreaker, which is what begins a new ICE session.
    Restart,
}

/// The peer's ICE credentials as the description in this body states them, for [`Call::peer_ice`].
///
/// Read from the message that completed the initial exchange — the 2xx for a caller, the INVITE
/// for a callee — because a restart is only ever recognisable as a *change* from what was last
/// seen, and a call that recorded nothing would read the peer's first re-offer as one.
///
/// `None` for a description with no ICE, which is the ordinary call: nothing that arrives later can
/// restart a session that never began.
pub(super) fn peer_ice_credentials(body: &[u8]) -> Option<sipx_sdp::ice::Credentials> {
    let description = sipx_sdp::parse(&String::from_utf8_lossy(body)).ok()?;
    let audio = description.media.first()?;
    match sipx_media::ice::negotiate(&description, audio) {
        IceNegotiation::Ice { credentials, .. } => Some(credentials),
        IceNegotiation::Absent | IceNegotiation::Mismatch => None,
    }
}

/// Credentials and a tiebreaker for a new ICE session (RFC 8839 §5.4, RFC 8445 §7.1.3).
///
/// Drawn per session and never reused, for the reason `ice.md` §13.4 gives about the initial
/// exchange and which applies unchanged to a restart: credentials that outlive the session they
/// authenticated make one session's checks valid in another. A tiebreaker carried across would
/// resolve a role conflict the way the *previous* session resolved it.
///
/// `None` when credentials could not be built — the same failure `MediaPolicy::gathering` reports
/// on the initial exchange, from the same generator, so it is not reachable in practice. It
/// degrades rather than failing the renegotiation: the agent keeps the credentials it has, the
/// answer restates them, and the peer keys its new session's checks to those. RFC 8839 §4.4.1.1.1
/// asks the answerer for new ones; reusing them is worse than complying and much better than
/// refusing a re-offer on a call that is working.
fn fresh_ice_parameters() -> Option<(sipx_sdp::ice::Credentials, u64)> {
    let credentials = IceCredentials::new(token(), format!("{}{}", token(), token()))?;
    Some((credentials, rand::random()))
}

impl Call {
    /// Give the running agent the ICE half of an answer to one of our later offers.
    ///
    /// The offering side's mirror of [`Self::answer_ice`], and it signals nothing: the answer is
    /// the end of this exchange, so what comes back from the agent has no description left to go
    /// into. What matters is that the agent hears it at all — a restart this side offered is only
    /// half a restart until the peer's new credentials and candidates arrive.
    pub(super) async fn accept_answer_ice(&mut self, answer: &SessionDescription) {
        if !self.media.runs_ice() {
            return;
        }
        let peer = answer
            .media
            .first()
            .map_or(IceNegotiation::Absent, |audio| {
                sipx_media::ice::negotiate(answer, audio)
            });
        // Recorded for the same reason the initial exchange records it: the next offer from the
        // peer is a restart only if it differs from what was last seen, and an answer is a
        // description like any other.
        self.peer_ice_restarted(&peer);
        // discard: this is the media path, and M12's clause is about the signalling one — the
        // counters for a media session that could not apply a renegotiation are `M-32`, which is
        // why `sipx-media` is not in the guard's `CRATES`. Nothing signalling is lost here in any
        // case: the peer's ICE half was recorded on the line above, which is what the *next* offer
        // is compared against, and a renegotiation that does not take leaves the candidate pair
        // already carrying the call in use.
        let _ = self.media.renegotiate_ice(None, Some(&peer)).await;
    }

    /// Put this side's ICE half into a later offer (RFC 8839 §4.4; `ice.md` §13.5).
    ///
    /// The offering counterpart of [`Self::answer_ice`], and it carries the same rule: a stream
    /// doing ICE restates its half in **every** subsequent offer, because §6 makes their absence
    /// mean this side has stopped. [`Self::restart_ice`] is the one caller that also draws new
    /// credentials, and drawing them is the whole of what it does — §4.4.1.1.1 says both values
    /// changing *is* the restart, so there is no second flag to set on the wire.
    pub(super) async fn offer_ice(&mut self, offer: &mut SessionDescription, ice: IceOffer) {
        if !self.media.runs_ice() {
            return;
        }
        let local = match ice {
            IceOffer::Continue => None,
            IceOffer::Restart => fresh_ice_parameters(),
        };
        // No peer half: this is an offer, and the answer that responds to it comes back through
        // `Dialing`/`renegotiate` like any other.
        let Some(signalled) = self.media.renegotiate_ice(local, None).await else {
            return;
        };
        let Some(audio) = offer.media.first_mut() else {
            return;
        };
        audio
            .attributes
            .retain(|attribute| !is_ice_attribute(attribute));
        audio.attributes.extend(ice_attributes(&signalled));
    }

    /// Put this side's ICE half into the answer to a later offer (RFC 8839 §4.4; `ice.md` §13.5).
    ///
    /// Three things happen here and they are one operation because they must not be reordered:
    /// the offer is read for the peer's half, this side takes new parameters when §4.4.1.1.1 says
    /// the offer is a restart, and both are handed to the running agent — which is what decides
    /// whether the session is rebuilt. What comes back is what this answer signals.
    ///
    /// **A stream doing ICE re-signals on every exchange**, not only on a restart. §6 makes the
    /// absence of `candidate` attributes mean the peer has stopped doing ICE, so an answer that
    /// dropped them mid-call would tell the far end to fall back to symmetric RTP on a path it had
    /// already agreed to check. Hold, resume, a codec change and a session refresh all come
    /// through here, and none of them is a restart.
    ///
    /// A call with no agent is left exactly as it was: no attributes, no round trip to a driver
    /// that does not exist.
    pub(super) async fn answer_ice(
        &mut self,
        offer: &SessionDescription,
        answer: &mut SessionDescription,
    ) {
        if !self.media.runs_ice() {
            return;
        }
        let peer = offer.media.first().map_or(IceNegotiation::Absent, |audio| {
            sipx_media::ice::negotiate(offer, audio)
        });
        // §4.4.1.1.1 is a question about the *peer's* two credentials, and this side answers it
        // only to know whether to draw its own new ones. The agent asks it again for itself, from
        // the credentials it is actually keyed to; see `MediaSession::renegotiate_ice`.
        let local = self
            .peer_ice_restarted(&peer)
            .then(fresh_ice_parameters)
            .flatten();
        let Some(signalled) = self.media.renegotiate_ice(local, Some(&peer)).await else {
            return;
        };
        let Some(audio) = answer.media.first_mut() else {
            return;
        };
        audio
            .attributes
            .retain(|attribute| !is_ice_attribute(attribute));
        audio.attributes.extend(ice_attributes(&signalled));
    }

    /// Whether this offer restarts ICE (RFC 8839 §4.4.1.1.1).
    ///
    /// **Both** credentials changed, and only both. One alone is not a restart, which is the case
    /// the rule is worded to exclude: a peer may legitimately re-send a description with one value
    /// re-derived and the other unchanged, and treating that as a restart would tear down a
    /// working session for nothing.
    ///
    /// The comparison is against what this side last *saw*, which is why it is recorded here
    /// rather than derived from the SDP twice — "the same value moving between the session level
    /// and the media level is not a restart" is only true if what is compared is the effective
    /// value for the stream, which is what [`sipx_media::ice::negotiate`] resolves.
    fn peer_ice_restarted(&mut self, peer: &IceNegotiation) -> bool {
        let IceNegotiation::Ice { credentials, .. } = peer else {
            return false;
        };
        let restarted = self.peer_ice.as_ref().is_some_and(|seen| {
            seen.ufrag() != credentials.ufrag() && seen.pwd() != credentials.pwd()
        });
        self.peer_ice = Some(credentials.clone());
        restarted
    }

    /// Restart ICE on this call (RFC 8445 §9, RFC 8839 §4.4.1.1.1; `ice.md` §13.5).
    ///
    /// Sends a re-INVITE whose offer carries **new** `ice-ufrag` and `ice-pwd` for this stream,
    /// which is the entire signal — the peer reads both having changed and begins a new ICE
    /// session. Everything else about the call is unchanged, including its direction, so a
    /// restart does not resume a call that was on hold.
    ///
    /// Media keeps flowing on the pair the finished session selected until the new one selects its
    /// own. That is what makes a restart usable in the situation it exists for: the path has become
    /// doubtful, not yet unusable, and going silent while checks converge would turn a recoverable
    /// call into a dropped one.
    ///
    /// A call not running ICE is left alone and reports success. There is nothing to restart, and
    /// making the caller distinguish "no ICE" from "restart failed" would push the check to every
    /// call site.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the re-INVITE cannot be built or sent, or when the far end refuses
    /// it — the same failures as any other renegotiation, and like them it leaves the call running.
    pub async fn restart_ice(&mut self) -> Result<()> {
        if !self.media.runs_ice() {
            return Ok(());
        }
        self.reoffer(self.hold, IceOffer::Restart).await
    }
}
