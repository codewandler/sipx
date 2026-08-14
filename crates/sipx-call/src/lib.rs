//! Calls: dialogs, INVITE with SDP offer/answer, and the media that results.
//!
//! This is the layer where the signalling and the media stacks meet. The join is narrower than
//! it looks: SDP negotiation decides an address, a port and a codec, and everything else about
//! media follows from those three.
//!
//! The ordering constraint worth knowing is that an SDP offer has to name the port audio will
//! arrive on, and only a bound socket knows that port. So the media socket is bound *before*
//! the INVITE is sent, not after the answer comes back.
//!
//! # Stability
//!
//! The Supported Rust API is frozen for compatible v1 evolution. Existing Supported paths and
//! signatures remain source-compatible throughout major version 1; compatible additions use the
//! reservations documented on their types.
//!
//! - **Supported** — covered by the v1 compatibility contract. A breaking change waits for the next
//!   major version.
//! - **Experimental** — remains unfrozen and may change shape or be removed without a migration
//!   note. Depend on it only if you are prepared to follow it.
//!
//!
//! **Supported**: the call lifecycle — dial, answer, early dialogs, hold and resume, both transfer
//! flavours, DTMF, playback, recording, session timers — plus its v1 configuration boundary:
//! [`CallConfig`], [`MediaAddress`], [`CodecPreference`], [`Codecs`], [`IcePolicy`], [`TurnPolicy`],
//! [`Keying`], [`SrtpSuite`], [`MediaPolicy`], [`MediaProfile`], [`OutboundIdentityPolicy`] and
//! [`InboundIdentityPolicy`]. [`DialOptions::with_call_config`] and
//! [`Invitation::answer_with_config`] consume the same opaque configuration in the two SIP roles.
//! Identity stays role-correct at [`DialOptions::with_identity`] and [`Dispatcher::with_identity`].
//! Initial request/response extension fields are validated `sipx-sip` headers supplied by the
//! owning application; stack-owned field policy remains the application's responsibility.
//!
//! **Experimental**: the bounded inbound event [`Notifier`], outbound [`EventSubscriptions`] and bidirectional
//! publication [`Publications`] runtimes,
//! the two-dialog ownership and relay surface in [`coupling`],
//! and the legacy policy-specific answering entry
//! points that take a selection, policy, or independent media addresses ([`answer_at`],
//! [`answer_with`], [`answer_with_policy`], [`answer_with_policy_at`],
//! [`answer_ringing_with`], [`answer_ringing_with_policy`], [`answer_replacing_with`],
//! [`Invitation::answer_with`], [`Invitation::answer_with_policy`], [`ring_early_with`],
//! [`ring_early_with_policy`], [`ring_offer_early`], [`ring_offer_early_with_policy`] and
//! [`dial_early_without_offer`]). These choices remain Experimental during v1 and their shape may
//! still move.
//! The caller's half of an SDP-free dialog is Experimental and new (`T-46`): [`dial_signalling`],
//! [`dial_signalling_until`], [`SignallingDial`], [`SignallingDialOptions`] and
//! [`SignallingIdentity`] place and end a call that offers no session, so no RTP socket is bound
//! and the cost of signalling can be measured apart from the cost of having a media stack at all.
//! [`Invitation::answer_signalling`] has been the answering half of the same shape since `P-15`.
//! Confirmed-dialog persistence is Experimental too: [`Call::dialog_snapshot`],
//! [`Call::restore_dialog`], [`DialogSnapshot`] and [`DialogRestoreContext`] expose a versioned
//! boundary whose schema remains deliberately narrower than a serialized `Call`.
//!
//! The set is the G.711 pair unless a call says otherwise. An application may provide an exact
//! non-empty order with [`Codecs::ordered`], including mono L16; selecting Opus is a typed error
//! unless this crate is built with its `opus` feature, which links libopus.
//! [`MediaProfile::BrowserAudio`] is the fail-closed composition of WSS, ICE, DTLS-SRTP,
//! multiplexed RTCP, and that required audio vocabulary. It is one bounded audio endpoint profile,
//! not a browser API or a general WebRTC compatibility claim.
//!
//! **Experimental**, and new: the media coupling of calls one host owns — [`CallBridge`] connects
//! two `Call`s so each hears the other, and [`CallConference`] mixes several of them (`C-6`). Both
//! are the *media* coupling only; owning two dialogs as one call, with the offer relayed on every
//! axis, is [`Coupling`]. Neither hands out a `MediaSession` or a port, and neither takes ownership
//! of a call: a host holds its `Call`s and drives their signalling throughout. Both keep their
//! composition identity when an in-dialog renegotiation replaces one call's media session.
//!
//! [`Error`] is `#[non_exhaustive]`: additive diagnostics stay additive for downstream callers, so
//! a `match` over it carries a `_` arm.
//!
//! <!-- BEGIN sipx-api-classification -->
//! **Experimental Rust API roots:**
//!
//! - [`sipx_call::DialogPersistenceError`](crate::DialogPersistenceError)
//! - [`sipx_call::DialogRestoreContext`](crate::DialogRestoreContext)
//! - [`sipx_call::DialogSnapshot`](crate::DialogSnapshot)
//! - [`sipx_call::NegotiatedKeying`](crate::NegotiatedKeying)
//! - [`sipx_call::SignallingCall`](crate::SignallingCall)
//! - [`sipx_call::SignallingDial`](crate::SignallingDial)
//! - [`sipx_call::SignallingDialOptions`](crate::SignallingDialOptions)
//! - [`sipx_call::SignallingEvent`](crate::SignallingEvent)
//! - [`sipx_call::SignallingIdentity`](crate::SignallingIdentity)
//! - [`sipx_call::bridge::BridgeOptions`](crate::bridge::BridgeOptions)
//! - [`sipx_call::bridge::CallBridge`](crate::bridge::CallBridge)
//! - [`sipx_call::bridge::DtmfBridging`](crate::bridge::DtmfBridging)
//! - [`sipx_call::call::Call::dialog_snapshot`](crate::call::Call::dialog_snapshot)
//! - [`sipx_call::call::Call::restore_dialog`](crate::call::Call::restore_dialog)
//! - [`sipx_call::call::DialOptions::with_codecs`](crate::call::DialOptions::with_codecs)
//! - [`sipx_call::call::DialOptions::with_initial_direction`](crate::call::DialOptions::with_initial_direction)
//! - [`sipx_call::call::DialOptions::with_media_policy`](crate::call::DialOptions::with_media_policy)
//! - [`sipx_call::call::answer_at`](crate::call::answer_at)
//! - [`sipx_call::call::answer_replacing_with`](crate::call::answer_replacing_with)
//! - [`sipx_call::call::answer_ringing_with`](crate::call::answer_ringing_with)
//! - [`sipx_call::call::answer_ringing_with_policy`](crate::call::answer_ringing_with_policy)
//! - [`sipx_call::call::answer_with`](crate::call::answer_with)
//! - [`sipx_call::call::answer_with_policy`](crate::call::answer_with_policy)
//! - [`sipx_call::call::answer_with_policy_at`](crate::call::answer_with_policy_at)
//! - [`sipx_call::call::dial_early_without_offer`](crate::call::dial_early_without_offer)
//! - [`sipx_call::conference`](crate::conference)
//! - [`sipx_call::coupling`](crate::coupling)
//! - [`sipx_call::dial_signalling`](crate::dial_signalling)
//! - [`sipx_call::dial_signalling_until`](crate::dial_signalling_until)
//! - [`sipx_call::dispatch::Dispatcher::with_event_subscriptions`](crate::dispatch::Dispatcher::with_event_subscriptions)
//! - [`sipx_call::dispatch::Dispatcher::with_notifier`](crate::dispatch::Dispatcher::with_notifier)
//! - [`sipx_call::dispatch::Dispatcher::with_publications`](crate::dispatch::Dispatcher::with_publications)
//! - [`sipx_call::dispatch::Invitation::answer_with`](crate::dispatch::Invitation::answer_with)
//! - [`sipx_call::dispatch::Invitation::answer_with_policy`](crate::dispatch::Invitation::answer_with_policy)
//! - [`sipx_call::notifier`](crate::notifier)
//! - [`sipx_call::publication`](crate::publication)
//! - [`sipx_call::rel::ring_early_with`](crate::rel::ring_early_with)
//! - [`sipx_call::rel::ring_early_with_policy`](crate::rel::ring_early_with_policy)
//! - [`sipx_call::rel::ring_offer_early`](crate::rel::ring_offer_early)
//! - [`sipx_call::rel::ring_offer_early_with_policy`](crate::rel::ring_offer_early_with_policy)
//! - [`sipx_call::subscriber`](crate::subscriber)
//! <!-- END sipx-api-classification -->

// This crate's inline test modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

// Crate-private for the reason `update` gives below: every item in it is `pub(crate)`, and it is
// the shared half of `voice` and `signal_metrics` rather than a surface of its own.
mod audio_feed;
pub mod bridge;
pub mod call;
pub mod conference;
pub mod counters;
pub mod coupling;
pub mod dialog;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod extension;
pub mod identity;
pub mod load;
mod media_policy;
pub mod notifier;
pub mod publication;
pub mod rel;
pub mod signal_metrics;
mod signalling;
mod snapshot;
pub mod subscriber;
pub mod transfer;
// Crate-private: every item in it is `pub(crate)`, and a `pub mod` whose contents are all
// private renders as an empty page in the API reference — a promise of surface that is not there.
mod update;
pub mod voice;

pub use bridge::{BridgeOptions, CallBridge, DtmfBridging, UnbridgeCause};
pub use call::{
    Call, CallConfig, Credentials, DialOptions, Dialing, MediaAddress, Served, answer, answer_at,
    answer_early, answer_replacing, answer_replacing_with, answer_ringing, answer_ringing_with,
    answer_ringing_with_policy, answer_ringing_with_policy_at, answer_with, answer_with_policy,
    answer_with_policy_and_headers, answer_with_policy_and_headers_at, answer_with_policy_at, dial,
    dial_early, dial_early_until, dial_early_without_offer, dial_once, dial_until, serve,
    serve_until,
};
pub use conference::{CallConference, Participant};
pub use counters::SignallingCounts;
pub use coupling::transparent::{OffMediaCoupling, OffMediaOptions};
pub use coupling::{
    CancelAction, ConfirmedCoupling, Coupling, CouplingEnd, CouplingState, EarlyCoupling,
    FailureAction, Leg, OfferAction, OfferAxis,
};
pub use dialog::{Dialog, DialogId, Role};
pub use dispatch::{
    Calls, DispatchCounts, Dispatched, Dispatcher, DrainProgress, DrainReport, Invitation,
};
pub use error::{
    CancellationCleanup, CancellationDisposition, Error, InvitationCancellation, Result,
};
pub use event::{CallEvent, CallEvents, EndCause};
pub use extension::{ApplicationRequest, MAX_APPLICATION_BODY};
pub use identity::{InboundIdentityPolicy, OutboundIdentityPolicy};
pub use media_policy::{
    CodecPreference, CodecSelectionError, Codecs, IcePolicy, Keying, MAX_TURN_USERNAME_BYTES,
    MediaPolicy, MediaProfile, NegotiatedKeying, SrtpSuite, TurnPolicy, TurnPolicyError,
};
pub use notifier::{Notifier, NotifierCounts, NotifierHandle};
pub use publication::{
    AllowPublications, Publication, PublicationAuthorization, PublicationComposition,
    PublicationConfig, PublicationCounts, PublicationError, Publications, PublicationsHandle,
    ReplacePublicationState,
};
pub use rel::{
    Ringing, ring, ring_early, ring_early_with, ring_early_with_policy, ring_early_with_policy_at,
    ring_offer_early, ring_offer_early_with_policy, ring_offer_early_with_policy_at,
};
pub use signal_metrics::SignalMetrics;
pub use signalling::{
    SignallingCall, SignallingDial, SignallingDialOptions, SignallingEvent, SignallingIdentity,
    dial_signalling, dial_signalling_until,
};
pub use sipx_sdp::Direction;
pub use snapshot::{
    DialogNotQuiescent, DialogPersistenceError, DialogRestoreContext, DialogSessionAction,
    DialogSnapshot, MAX_FIELD_BYTES, MAX_ID_BYTES, MAX_ROUTES, MAX_SNAPSHOT_BYTES,
    MAX_VARIABLE_BYTES,
};
pub use subscriber::{
    EventNotification, EventSubscription, EventSubscriptionCounts, EventSubscriptionError,
    EventSubscriptions, EventSubscriptionsHandle,
};
pub use transfer::{Referral, Replaces, Transfer, TransferState};
pub use voice::{VoiceActivity, VoiceThresholds};
