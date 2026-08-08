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
//! sipx is pre-1.0, so **neither word below means frozen**. `1.0.0` is what freezes an API, and its
//! predicates are in `docs/roadmap.md`. Until then:
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry saying what
//!   to do instead. New enum variants and new struct fields may still appear in a minor release, so a
//!   downstream `match` should carry a `_` arm.
//! - **Experimental** — may change shape or be removed without a migration note. Depend on it only if
//!   you are prepared to follow it.
//!
//!
//! **Supported**: the call lifecycle — dial, answer, early dialogs, hold and resume, both transfer
//! flavours, DTMF, playback, recording, session timers — plus the bounded generic load scheduler
//! in [`load`]. Initial request/response extension fields are validated `sipx-sip` headers supplied
//! by the owning application; stack-owned field policy remains the application's responsibility.
//!
//! **Experimental**: choosing what a call offers — [`CodecPreference`], [`Codecs`], [`IcePolicy`],
//! [`Keying`], [`MediaPolicy`], [`MediaProfile`], [`MediaAddress`],
//! [`OutboundIdentityPolicy`], [`InboundIdentityPolicy`],
//! the bounded inbound event [`Notifier`], outbound [`EventSubscriptions`] and bidirectional
//! publication [`Publications`] runtimes,
//! the two-dialog ownership and relay surface in [`coupling`],
//! [`DialOptions::with_codecs`], [`DialOptions::with_initial_direction`],
//! [`DialOptions::with_media_policy`],
//! [`DialOptions::with_identity`], [`Dispatcher::with_identity`], and the answering entry
//! points that take a selection, policy, or independent media addresses ([`answer_at`],
//! [`answer_with`], [`answer_with_policy`], [`answer_with_policy_at`],
//! [`answer_ringing_with`], [`answer_ringing_with_policy`], [`answer_replacing_with`],
//! [`Invitation::answer_with`], [`Invitation::answer_with_policy`], [`ring_early_with`],
//! [`ring_early_with_policy`], [`ring_offer_early`], [`ring_offer_early_with_policy`] and
//! [`dial_early_without_offer`]). These choices are pre-1.0 and their shape may still move.
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
//! Absent rather than experimental, so that nobody looks for it: **multi-party** call bridging or
//! conferencing. A [`Coupling`] can own two calls and attach a bounded media bridge, but `Call`
//! does not expose its `MediaSession` for arbitrary application-side mixing.
//!
//! [`Error`] is `#[non_exhaustive]`: additive diagnostics stay additive for downstream callers, so
//! a `match` over it carries a `_` arm.

pub mod call;
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
mod signalling;
mod snapshot;
pub mod subscriber;
pub mod transfer;
// Crate-private: every item in it is `pub(crate)`, and a `pub mod` whose contents are all
// private renders as an empty page in the API reference — a promise of surface that is not there.
mod update;

pub use call::{
    Call, Credentials, DialOptions, Dialing, MediaAddress, Served, answer, answer_at, answer_early,
    answer_replacing, answer_replacing_with, answer_ringing, answer_ringing_with,
    answer_ringing_with_policy, answer_ringing_with_policy_at, answer_with, answer_with_policy,
    answer_with_policy_and_headers, answer_with_policy_and_headers_at, answer_with_policy_at, dial,
    dial_early, dial_early_until, dial_early_without_offer, dial_once, dial_until, serve,
    serve_until,
};
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
    CodecPreference, CodecSelectionError, Codecs, IcePolicy, Keying, MediaPolicy, MediaProfile,
    NegotiatedKeying,
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
pub use signalling::{SignallingCall, SignallingEvent};
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
