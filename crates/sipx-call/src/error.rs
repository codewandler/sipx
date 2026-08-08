//! Call errors.

use sipx_sip::{HeaderName, Method};
use thiserror::Error;

/// What invitation withdrawal accomplished before its caller-owned allowance ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancellationCleanup {
    /// Maximum time the caller allowed for withdrawal.
    pub limit: std::time::Duration,
    /// Monotonic time actually spent withdrawing the invitation.
    pub elapsed: std::time::Duration,
    /// Transaction disposition when the phase ended.
    pub disposition: CancellationDisposition,
}

/// Why the invitation-withdrawal phase ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancellationDisposition {
    /// The INVITE transaction reached a terminal event.
    Completed {
        /// Whether the provisional-response precondition admitted one CANCEL transaction.
        cancel_sent: bool,
        /// Whether a final INVITE response was observed during withdrawal.
        final_response_observed: bool,
    },
    /// The caller-owned allowance ended the phase.
    Exhausted {
        /// Whether a CANCEL transaction had already been created.
        cancel_sent: bool,
        /// Whether a crossed final response was observed before its remaining cleanup exhausted.
        final_response_observed: bool,
    },
    /// A local error prevented the protocol operation from being completed.
    Failed {
        /// Whether a CANCEL transaction had already been created.
        cancel_sent: bool,
    },
}

impl CancellationCleanup {
    /// Whether the provisional-response precondition admitted one CANCEL transaction.
    #[must_use]
    pub const fn cancel_sent(self) -> bool {
        match self.disposition {
            CancellationDisposition::Completed { cancel_sent, .. }
            | CancellationDisposition::Exhausted { cancel_sent, .. }
            | CancellationDisposition::Failed { cancel_sent } => cancel_sent,
        }
    }

    /// Whether a final INVITE response was observed during withdrawal.
    #[must_use]
    pub const fn final_response_observed(self) -> bool {
        match self.disposition {
            CancellationDisposition::Completed {
                final_response_observed,
                ..
            }
            | CancellationDisposition::Exhausted {
                final_response_observed,
                ..
            } => final_response_observed,
            CancellationDisposition::Failed { .. } => false,
        }
    }

    /// Whether the INVITE transaction reached a terminal event during withdrawal.
    #[must_use]
    pub const fn completed(self) -> bool {
        matches!(self.disposition, CancellationDisposition::Completed { .. })
    }

    /// Whether the allowance, rather than a transaction event, ended withdrawal.
    #[must_use]
    pub const fn exhausted(self) -> bool {
        matches!(self.disposition, CancellationDisposition::Exhausted { .. })
    }
}

/// Measurements from one locally stopped outbound invitation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvitationCancellation {
    /// `true` when the answer budget expired; `false` for caller cancellation.
    pub timed_out: bool,
    /// Configured answer budget, absent when the transaction layer owns expiry.
    pub invitation_limit: Option<std::time::Duration>,
    /// Monotonic time from handing the INVITE to the endpoint until local stop won.
    pub invitation_elapsed: std::time::Duration,
    /// The distinct withdrawal phase.
    pub cleanup: CancellationCleanup,
}

impl std::fmt::Display for InvitationCancellation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.timed_out {
            write!(
                formatter,
                "no answer within {:?}; cancellation cleanup used {:?} of {:?}",
                self.invitation_limit.unwrap_or_default(),
                self.cleanup.elapsed,
                self.cleanup.limit
            )
        } else {
            write!(
                formatter,
                "the invitation was cancelled after {:?}; cleanup used {:?} of {:?}",
                self.invitation_elapsed, self.cleanup.elapsed, self.cleanup.limit
            )
        }
    }
}

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
    /// A voice-activity profile was refused before anything was attached to the call (`M-58`).
    #[error("voice activity profile: {0}")]
    VoiceActivityProfile(#[from] sipx_audio::analysis::AnalysisError),
    /// A signal-metric reporting profile was refused before anything was attached (`M-59`).
    ///
    /// Either the analysis half is outside a domain `call-audio-processing.md` §5.1 declares, or
    /// the reporting cadence is outside its own.
    #[error("signal metrics profile: {0}")]
    SignalMetricsProfile(#[from] sipx_audio::signal::SignalProfileError),
    /// The call's bounded PCM processing seam refused an attachment (`M-54`).
    #[error("call audio processing: {0}")]
    CallAudioProcessing(#[from] sipx_media::ProcessingError),
    /// A message could not be built.
    #[error("build: {0}")]
    Build(#[from] sipx_sip::error::BuildError),
    /// A selected authentication service could not attest the outbound caller identity.
    #[error("caller identity: {0}")]
    IdentityAuthentication(#[from] sipx_ua::identity::AuthenticationError),
    /// The SDP could not be read.
    #[error("sdp: {0}")]
    Sdp(String),
    /// A description could not be put in front of the other dialog of an off-media coupling.
    ///
    /// Distinct from [`Self::Sdp`] because it is not the reader that failed: the description
    /// parsed and this side declined to relay it, which is a decision with a named reason.
    #[error("relayed description: {0}")]
    Relay(#[from] sipx_sdp::RelayError),
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
    #[error("{0}")]
    Cancelled(InvitationCancellation),
    /// An invitation was answered after the caller had already withdrawn it (RFC 3261 §9.2).
    ///
    /// The other side of [`Self::Cancelled`]: there, *this* stack gave up on an INVITE it sent;
    /// here, the far end gave up on one it sent us, the invitation was answered `487 Request
    /// Terminated`, and there is nothing left to accept. Answering anyway would put a `200` on a
    /// transaction that has already finished and leave this side holding a call the caller does
    /// not have.
    #[error("the invitation was cancelled by the caller")]
    InvitationCancelled,
    /// A caller-selected signalling-dialog tag was empty, overlong or not a SIP token.
    ///
    /// The rejected value is deliberately absent: reflecting arbitrary application or fixture
    /// bytes in an error is unnecessary and makes it too easy to copy them into a log.
    #[error("the signalling dialog tag must be a non-empty SIP token of at most 128 bytes")]
    InvalidDialogTag,
    /// An originated BYE did not receive a final response inside its caller-owned failure bound.
    #[error("no final response to the dialog BYE within {0:?}")]
    SignallingTeardownTimeout(std::time::Duration),
    /// A final response to an originated BYE did not name that dialog and `CSeq`.
    ///
    /// The mismatching values are intentionally absent so an untrusted packet is not reflected
    /// into logs by displaying this error.
    #[error("the dialog BYE response did not match its dialog and CSeq")]
    InvalidDialogResponse,
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
    /// A stack-owned or unadmitted method was passed to the application-owned dialog API.
    #[error("{0} is not an application-owned method on this dialog")]
    StackOwnedDialogMethod(Method),
    /// Application content exceeded the retained request-body bound.
    #[error("application-owned body is {actual} octets; the limit is {limit}")]
    ApplicationBodyTooLarge {
        /// Observed body length.
        actual: usize,
        /// Maximum retained body length.
        limit: usize,
    },
    /// A non-empty application-owned body did not name its media type.
    #[error("a non-empty application-owned body requires Content-Type")]
    ApplicationContentTypeRequired,
    /// An application tried to supply a dialog- or transaction-owned header.
    #[error("the stack owns the {0:?} header on in-dialog requests")]
    ProtectedApplicationHeader(HeaderName),
    /// An application response must end the transaction rather than being provisional.
    #[error("application response status {0} is provisional; a final response is required")]
    ApplicationFinalResponseRequired(u16),
    /// The request's exactly-once response capability was already claimed.
    #[error("the application-owned request has already been answered")]
    ApplicationResponseAlreadySent,
    /// A response capability could not capture the runtime that owns its transport work.
    #[error("an application response requires an active Tokio runtime")]
    ApplicationRuntimeUnavailable,
    /// An operation requiring a live dialog was attempted after call teardown.
    #[error("the dialog has ended")]
    DialogEnded,
}

/// A call result.
pub type Result<T> = std::result::Result<T, Error>;
