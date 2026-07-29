//! Reading a peer's description: is ICE on for this stream at all (RFC 8839 §5.3, §6; [spec]
//! §13.2, §13.3)?
//!
//! Three answers and no fourth, because the driver has to branch on exactly one of them before it
//! binds anything to the media port:
//!
//! - the peer sent no `a=candidate`, so ICE is off and symmetric RTP carries the call as it does
//!   today — RFC 8839 §6: "An agent can determine that its peer supports ICE by the presence of
//!   'candidate' attributes for each media session";
//! - the peer sent candidates and they line up with where it says to send, so ICE is on;
//! - the peer sent candidates and its **default destination** for a component matches none of
//!   them, which is §5.3's `ice-mismatch`: ICE MUST NOT be used for that stream, the answer says
//!   so, and RFC 3264's procedures apply instead.
//!
//! Pure SDP. No clock and no socket reach this module, which is why it is scanned by the same
//! guard as the agent.
//!
//! [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::net::SocketAddr;

use sipx_sdp::ice::{Candidate, ComponentId, Credentials};
use sipx_sdp::{MediaDescription, SessionDescription};

/// The attribute an answer carries when §5.3's condition holds. Media level, answer only.
pub const ICE_MISMATCH: &str = "ice-mismatch";

/// What a peer's description says about ICE for one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Negotiation {
    /// No usable `a=candidate`: the peer is not doing ICE ([spec] §13.3).
    ///
    /// Nothing is offered back, no check is sent, no timer runs, and the stream is carried by
    /// symmetric RTP exactly as it is today. **This is the common case and must stay the common
    /// case** — a stack that requires ICE to place a call has regressed.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    Absent,
    /// ICE is on for this stream, with these parameters.
    Ice {
        /// The peer's `a=ice-ufrag` and `a=ice-pwd`, media level winning over session level
        /// (RFC 8839 §5.4).
        credentials: Credentials,
        /// Its `a=candidate` lines, in the order they appeared.
        candidates: Vec<Candidate>,
        /// Whether the session carried `a=ice-lite` (§5.3). A full agent facing a lite one
        /// controls unconditionally (RFC 8445 §6.1.1).
        lite: bool,
    },
    /// RFC 8839 §5.3: the offer's default destination for a component matched none of its
    /// candidates for that component.
    ///
    /// The answer carries [`ICE_MISMATCH`] for the stream and ICE MUST NOT be used for it; the
    /// stream falls back to RFC 3264 — which is to say to [`Negotiation::Absent`]'s behaviour,
    /// arrived at by a different route and reported differently, because the offerer needs to
    /// know that something between the two of us rewrote the address it advertised.
    Mismatch,
}

impl Negotiation {
    /// Whether an agent should be driven for this stream.
    #[must_use]
    pub const fn runs_ice(&self) -> bool {
        matches!(self, Self::Ice { .. })
    }

    /// The attributes this decision adds to the answer's media section.
    ///
    /// Only §5.3's flag today: the ICE attributes an answer carries for a stream that *is* doing
    /// ICE are the local description's ([`super::LocalDescription::attributes`]), not the
    /// remote's, so they come from the other side of the exchange.
    #[must_use]
    pub fn answer_attributes(&self) -> Vec<sipx_sdp::Attribute> {
        match self {
            Self::Mismatch => vec![sipx_sdp::Attribute::flag(ICE_MISMATCH)],
            Self::Absent | Self::Ice { .. } => Vec::new(),
        }
    }
}

/// Read one stream of a peer's description (RFC 8839 §5.3, §6).
///
/// `session` is the whole description because three of the four inputs are session-level or may
/// be: `a=ice-lite` is session-level only, the credentials default to the session's, and the
/// `c=` line a stream inherits when it has none of its own is the session's.
#[must_use]
pub fn negotiate(session: &SessionDescription, media: &MediaDescription) -> Negotiation {
    let candidates = media.ice_candidates();
    if candidates.is_empty() {
        return Negotiation::Absent;
    }
    let Some(credentials) = session.ice_credentials_for(media) else {
        // Candidates with no `ice-ufrag`/`ice-pwd` for the stream. RFC 8839 §4.2 makes both
        // mandatory, and without them there is no key for a connectivity check in either
        // direction, so ICE cannot run whatever the candidates say.
        //
        // Deliberately not `Mismatch`: §5.3's flag is a specific diagnosis — "your default
        // destination was rewritten between us" — and reporting it here would tell the offerer to
        // look at its NAT when the fault is in its SDP. The fallback is the same either way.
        tracing::debug!("ice candidates with no credentials; carrying the stream without ice");
        return Negotiation::Absent;
    };

    for (component, default) in default_destinations(session, media) {
        if !candidates
            .iter()
            .any(|candidate| matches(candidate, component, default))
        {
            tracing::debug!(
                component = component.get(),
                %default,
                "no candidate for the default destination; RFC 8839 §5.3 ice-mismatch"
            );
            return Negotiation::Mismatch;
        }
    }

    Negotiation::Ice {
        credentials,
        candidates,
        lite: session.is_ice_lite(),
    }
}

/// Whether a candidate line *is* this default destination for this component.
fn matches(candidate: &Candidate, component: ComponentId, default: SocketAddr) -> bool {
    candidate.component == component
        && candidate.address == default.ip()
        && candidate.port == default.port()
}

/// Where the offer says to send each component if ICE were not used at all.
///
/// Component 1 is the `c=`/`m=` pair, the stream's own `c=` winning over the session's. Component
/// 2 is RFC 3550 §11's convention — the next port up — and is only consulted when the peer offered
/// candidates for that component: a peer offering RTP alone has no RTCP default to mismatch, and
/// §6.1.2.2 already reduces the stream to the components both agents have.
///
/// `a=rtcp` (RFC 3605) would override the convention and is not parsed by [`sipx_sdp`]; until it
/// is, the convention is the only default there is, and it is the same one this crate's own RTCP
/// sender already follows.
fn default_destinations(
    session: &SessionDescription,
    media: &MediaDescription,
) -> Vec<(ComponentId, SocketAddr)> {
    let Some(address) = media
        .connection
        .as_ref()
        .or(session.connection.as_ref())
        .and_then(|connection| connection.address.ip())
    else {
        // No `c=` at all, or one naming an FQDN. There is no default destination to compare
        // against, so there is nothing §5.3 can be true of.
        return Vec::new();
    };
    if media.port == 0 {
        // A rejected stream (RFC 3264 §6): nothing is sent to it and nothing is mismatched.
        return Vec::new();
    }

    let mut defaults = vec![(ComponentId::RTP, SocketAddr::new(address, media.port))];
    let rtcp = media.port.checked_add(1);
    if let Some(port) = rtcp {
        if media
            .ice_candidates()
            .iter()
            .any(|candidate| candidate.component == ComponentId::RTCP)
        {
            defaults.push((ComponentId::RTCP, SocketAddr::new(address, port)));
        }
    }
    defaults
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// An offer whose stream carries `attributes` under the `m=` line, sending to `port` at
    /// 192.0.2.1.
    fn offer(port: u16, attributes: &str) -> SessionDescription {
        let text = format!(
            concat!(
                "v=0\r\n",
                "o=- 1 1 IN IP4 192.0.2.1\r\n",
                "s=-\r\n",
                "c=IN IP4 192.0.2.1\r\n",
                "t=0 0\r\n",
                "m=audio {port} RTP/AVP 0\r\n",
                "{attributes}",
            ),
            port = port,
            attributes = attributes,
        );
        sipx_sdp::parse(&text).expect("the fixture parses")
    }

    fn read(session: &SessionDescription) -> Negotiation {
        negotiate(session, session.media.first().expect("one stream"))
    }

    const CREDENTIALS: &str = "a=ice-ufrag:8hhY\r\na=ice-pwd:asd88fgpdd777uzjYhagZg\r\n";

    /// [spec] §13.3 and the vision's hard line: no `a=candidate` is not an error, it is the
    /// common case, and it must produce no ICE at all rather than a degraded ICE.
    ///
    /// [spec]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md
    #[test]
    fn a_peer_that_offers_no_candidates_is_not_doing_ice() {
        assert_eq!(read(&offer(49170, "")), Negotiation::Absent);
        // Nor is one that sent credentials and nothing to check.
        assert_eq!(read(&offer(49170, CREDENTIALS)), Negotiation::Absent);
    }

    /// The ordinary case: the default destination is one of the candidates.
    #[test]
    fn a_default_destination_that_is_a_candidate_runs_ice() {
        let session = offer(
            49170,
            &format!("{CREDENTIALS}a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n"),
        );
        let Negotiation::Ice {
            credentials,
            candidates,
            lite,
        } = read(&session)
        else {
            panic!("the default destination is the candidate");
        };
        assert_eq!(credentials.ufrag(), "8hhY");
        assert_eq!(candidates.len(), 1);
        assert!(!lite);
    }

    /// RFC 8839 §5.3, and the reason the attribute exists: something between the two agents
    /// rewrote the address the offerer advertised, so the candidates describe one path and the
    /// `c=`/`m=` pair another.
    #[test]
    fn a_default_destination_no_candidate_matches_is_an_ice_mismatch() {
        let session = offer(
            49170,
            &format!("{CREDENTIALS}a=candidate:1 1 UDP 2130706431 192.0.2.9 8998 typ host\r\n"),
        );
        assert_eq!(read(&session), Negotiation::Mismatch);
        assert_eq!(
            read(&session).answer_attributes(),
            vec![sipx_sdp::Attribute::flag("ice-mismatch")]
        );
        assert!(!read(&session).runs_ice());
    }

    /// The port has to match as well as the address — an ALG that rewrites only the port produces
    /// exactly this, and it is the case a comparison on the IP alone lets through.
    #[test]
    fn the_default_destination_matches_on_the_port_too() {
        let session = offer(
            49170,
            &format!("{CREDENTIALS}a=candidate:1 1 UDP 2130706431 192.0.2.1 8998 typ host\r\n"),
        );
        assert_eq!(read(&session), Negotiation::Mismatch);
    }

    /// A peer offering an RTCP component has an RTCP default destination too (RFC 3550 §11), and
    /// a peer offering RTP alone does not — §6.1.2.2 reduces the stream to the components both
    /// agents have, so there is nothing there to mismatch.
    #[test]
    fn the_rtcp_default_is_only_checked_when_the_peer_offered_that_component() {
        let rtp_only = offer(
            49170,
            &format!("{CREDENTIALS}a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n"),
        );
        assert!(read(&rtp_only).runs_ice());

        let both = offer(
            49170,
            &format!(
                "{CREDENTIALS}\
                 a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n\
                 a=candidate:1 2 UDP 2130706430 192.0.2.1 49171 typ host\r\n"
            ),
        );
        assert!(read(&both).runs_ice());

        // The same pair of components, with the RTCP candidate on a port that is not the
        // convention's. That is §5.3 for component 2.
        let moved = offer(
            49170,
            &format!(
                "{CREDENTIALS}\
                 a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n\
                 a=candidate:1 2 UDP 2130706430 192.0.2.1 60000 typ host\r\n"
            ),
        );
        assert_eq!(read(&moved), Negotiation::Mismatch);
    }

    /// Candidates and no credentials cannot run ICE — but they are not §5.3's diagnosis either,
    /// because nothing rewrote the default destination.
    #[test]
    fn candidates_without_credentials_fall_back_rather_than_report_a_mismatch() {
        let session = offer(
            49170,
            "a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n",
        );
        assert_eq!(read(&session), Negotiation::Absent);
    }

    /// A rejected stream (RFC 3264 §6) has no destination, so it has no default destination to
    /// mismatch — and a `c=` naming a name rather than a literal has none either.
    #[test]
    fn a_stream_with_no_destination_is_not_a_mismatch() {
        let rejected = offer(
            0,
            &format!("{CREDENTIALS}a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n"),
        );
        assert!(read(&rejected).runs_ice());
    }

    /// §5.3 puts `a=ice-lite` at session level, and a full agent facing a lite peer controls
    /// unconditionally (RFC 8445 §6.1.1) — so the flag has to survive the read.
    #[test]
    fn a_lite_peer_is_reported_as_one() {
        let text = concat!(
            "v=0\r\n",
            "o=- 1 1 IN IP4 192.0.2.1\r\n",
            "s=-\r\n",
            "c=IN IP4 192.0.2.1\r\n",
            "t=0 0\r\n",
            "a=ice-lite\r\n",
            "a=ice-ufrag:8hhY\r\n",
            "a=ice-pwd:asd88fgpdd777uzjYhagZg\r\n",
            "m=audio 49170 RTP/AVP 0\r\n",
            "a=candidate:1 1 UDP 2130706431 192.0.2.1 49170 typ host\r\n",
        );
        let session = sipx_sdp::parse(text).expect("parses");
        let Negotiation::Ice { lite, .. } = read(&session) else {
            panic!("ice is on");
        };
        assert!(lite, "a=ice-lite is session level");
    }
}
