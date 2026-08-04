//! Call errors.

use thiserror::Error;

/// What can go wrong establishing or running a call.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// A socket failed.
    #[error("io: {0}")]
    Io(#[source] std::io::Error),
    /// Negotiated media could not be constructed safely.
    #[error("media: {0}")]
    Media(#[from] sipx_media::SetupError),
    /// The transport failed.
    #[error("transport: {0}")]
    Transport(#[from] sipx_transport::Error),
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// A selected authentication service could not attest the outbound caller identity.
    #[error("caller identity: {0}")]
    IdentityAuthentication(#[from] sipx_ua::identity::AuthenticationError),
    /// The SDP could not be read.
    #[error("sdp: {0}")]
    Sdp(String),
    /// A named browser-audio policy boundary refused setup or renegotiation.
    #[error("browser-audio profile: {0}")]
    Profile(#[from] sipx_sdp::browser_audio::ProfileError),
    /// SDP was asked to advertise an unspecified address which no peer can reach.
    #[error("the advertised media address must not be unspecified")]
    UnspecifiedMediaAddress,
    /// DTLS-SRTP was selected in a build that does not contain its handshake implementation.
    #[error("DTLS-SRTP was selected, but sipx-call was built without its `dtls` feature")]
    DtlsUnavailable,
    /// A selected DTLS-SRTP media path could not be keyed.
    #[error("DTLS-SRTP: {0}")]
    Dtls(String),
    /// The DTLS setup exchange selected no role this endpoint can hold.
    #[error("DTLS setup: {0}")]
    DtlsSetup(#[from] sipx_sdp::fingerprint::SetupRoleError),
    /// DTLS-SRTP was selected through an early-dialog API that cannot yet preserve its ordering.
    #[error("DTLS-SRTP is not available for early media")]
    DtlsEarlyMedia,
    /// A running DTLS-SRTP call was asked to renegotiate without a rekeying exchange.
    #[error("DTLS-SRTP renegotiation is not implemented; the existing call is unchanged")]
    DtlsRenegotiation,
    /// An in-dialog description tried to change which sockets carry RTCP.
    #[error(
        "RTCP mode change from {current:?} to {proposed:?} is not implemented; the existing call is unchanged"
    )]
    RtcpModeChange {
        /// The mode owned by the running media session.
        current: sipx_sdp::RtcpMode,
        /// The mode the new offer/answer exchange selected.
        proposed: sipx_sdp::RtcpMode,
    },
    /// The INVITE got no final response.
    #[error("no final response to the INVITE")]
    NoResponse,
    /// The far end refused.
    #[error("rejected: {status} {reason}")]
    Rejected {
        /// The status code.
        status: u16,
        /// Its reason phrase.
        reason: String,
    },
    /// A supported digest challenge returned by an INVITE attempt.
    ///
    /// [`crate::dial`] and [`crate::dial_once`] consume this internally when credentials were
    /// supplied. It can surface from [`crate::dial_early`], whose handle names one INVITE and
    /// therefore does not silently replace it with a retry.
    #[error("authentication required: {status} {reason}")]
    AuthenticationChallenge {
        /// The 401 or 407 status.
        status: u16,
        /// Its reason phrase.
        reason: String,
        /// The strongest supported challenge the response offered.
        challenge: Box<sipx_sip::auth::Challenge>,
    },
    /// A 2xx that established no dialog — no `To` tag, or no `Contact` to send to.
    #[error("the response established no dialog")]
    NoDialog,
    /// The caller gave up before the far end answered, and cancelled the invitation.
    ///
    /// Distinct from a rejection: nobody refused the call, we stopped waiting for it.
    #[error("no answer within {0:?}; the invitation was cancelled")]
    Cancelled(std::time::Duration),
    /// An invitation was answered after the caller had already withdrawn it (RFC 3261 §9.2).
    ///
    /// The other side of [`Self::Cancelled`]: there, *this* stack gave up on an INVITE it sent;
    /// here, the far end gave up on one it sent us, the invitation was answered `487 Request
    /// Terminated`, and there is nothing left to accept. Answering anyway would put a `200` on a
    /// transaction that has already finished and leave this side holding a call the caller does
    /// not have.
    #[error("the invitation was cancelled by the caller")]
    InvitationCancelled,
    /// An INVITE asked to replace a dialog it did not name, or named one this is not.
    ///
    /// Deliberately one error for both. Telling a caller "the Call-ID matched but the tags did
    /// not" would be telling them how far their guess got.
    #[error("the Replaces header names no dialog we have")]
    NoReplaces,
    /// A transfer was accepted or refused when none had been asked for.
    #[error("no transfer has been requested on this call")]
    NoReferral,
    /// Negotiation produced no usable audio stream.
    #[error("no codec in common")]
    NoCommonCodec,
    /// A `422` refused the session interval we asked for (RFC 4028 §6), naming its own minimum.
    ///
    /// Carries the minimum rather than folding into [`Self::Rejected`] because the whole point
    /// of a 422 is that it is retryable, and only the value it carries makes the retry possible.
    #[error("the far end requires a session interval of at least {0:?}")]
    IntervalTooBrief(std::time::Duration),
    /// The far end never refreshed the session, so it was torn down locally (RFC 4028 §10).
    #[error("the session expired without a refresh")]
    SessionExpired,
    /// A 2xx was asked for while a reliable provisional carrying SDP is unacknowledged.
    ///
    /// RFC 3262 §5 makes the delay a MUST, and this is where it is enforced rather than
    /// silently deferred: a description sent in a provisional that never arrived, followed by a
    /// 200 that carries none, leaves the caller in a confirmed dialog with no answer at all.
    /// Keep feeding messages to [`Ringing::on_prack`](crate::Ringing::on_prack) and try again.
    #[error("the reliable provisional carrying the answer has not been acknowledged")]
    UnacknowledgedProvisional,
    /// An invitation was treated as having an early session when it never had one.
    ///
    /// On the answering side: either it was rung with `ring` rather than `ring_early`, or it has
    /// already been answered — a `Ringing` hands its media port and its dialog over exactly once.
    ///
    /// On the calling side, from [`Dialing::update`](crate::Dialing::update): the far end has
    /// established a dialog but has not answered our offer in a reliable provisional, so RFC
    /// 3311 §5.1 does not yet allow an UPDATE to carry one.
    /// [`Dialing::has_early_session`](crate::Dialing::has_early_session) is the same question
    /// asked in advance.
    #[error("this invitation has no early session to answer")]
    NoEarlySession,
}

/// A call result.
pub type Result<T> = std::result::Result<T, Error>;
