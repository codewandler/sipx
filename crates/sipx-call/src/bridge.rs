//! Connecting two calls a host owns, so each hears the other (`C-6`).
//!
//! [`sipx_media::Bridge`] forwards audio between two media sessions, and until this module it
//! could only be reached by owning those sessions: a [`Call`] holds its own and lends it out by
//! reference. [`CallBridge`] is the path from two `Call`s to that forwarding.
//!
//! **It is the media coupling and nothing else.** Neither call's signalling is relayed to the
//! other, no re-INVITE is sent, and each call is still answered, held, transferred and hung up on
//! its own. Owning two dialogs as one call — offer relay on every axis, glare, CANCEL/BYE
//! mapping — is [`Coupling`](crate::Coupling), which takes both calls by value instead.
//!
//! **Ownership is unchanged** (the vision's principle 3, *own, don't share*). Connecting borrows
//! each call mutably for the length of the [`CallBridge::connect`] call and keeps neither borrow:
//! what the bridge holds afterwards is each call's media *handle*, and every sample and payload
//! between them moves over the sessions' own channels. There is no `Arc<Mutex<Call>>` here, no
//! shared mutable session, and no raw port — a host goes on owning both `Call`s outright and
//! driving each one's signalling while they are bridged.
//!
use std::sync::{Arc, Mutex, MutexGuard};

use sipx_media::{Bridge, MediaSession};
use tokio::task::JoinHandle;

use crate::call::Call;
use crate::event::{CallEvent, ReservedEmitter};

/// What happens to a keypress that arrives while a call is bridged.
///
/// The choice is made once, when the bridge is made, and holds for its lifetime. It is a choice
/// and not a default because both answers are right for real hosts: an IVR that bridged a caller
/// to an agent still wants `*` to mean "transfer me", while a carrier-facing relay must let the
/// far end's keypresses reach the far end.
///
/// The two are **exclusive**, and that is the same principle 3 the rest of this module rests on: a
/// media session's keypress queue has exactly one consumer. Under [`Self::PassThrough`] the bridge
/// is that consumer, so a keypress crossing the bridge does not also appear on
/// [`Call::recv_digit`] or as [`CallEvent::Dtmf`] on the call it arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum DtmfBridging {
    /// Keep keypresses on the call they arrived on.
    ///
    /// The default, and what an unbridged call does: the RFC 4733 event is decoded and queued for
    /// [`Call::recv_digit`], and [`Call::drive_media_event`] moves it onto the call's event
    /// stream. Nothing is sent to the other call.
    #[default]
    Deliver,
    /// Regenerate each keypress on the other call.
    ///
    /// The digit and the duration the far end reported are re-sent as this side's own RFC 4733
    /// event on the other leg, rather than the arriving packets being forwarded verbatim. That is
    /// deliberate: the two legs negotiate their `telephone-event` payload types independently, and
    /// a payload relayed across a bridge whose legs disagree would be played as audio.
    ///
    /// A leg that negotiated no `telephone-event` payload type cannot carry a regenerated
    /// keypress, and one arriving from such a leg is never decoded in the first place; in both
    /// cases pass-through is silently a no-op for that direction. It is a property of what the
    /// peers offered, not of this bridge.
    PassThrough,
}

/// How a bridge was chosen, when it is made.
///
/// Separate from [`CallBridge::connect`] so that a later choice does not need a third entry point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BridgeOptions {
    dtmf: DtmfBridging,
}

impl BridgeOptions {
    /// The defaults: keypresses stay on the call they arrived on.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Choose what happens to a keypress that arrives while the calls are bridged.
    #[must_use]
    pub fn with_dtmf(mut self, dtmf: DtmfBridging) -> Self {
        self.dtmf = dtmf;
        self
    }

    /// What this will do with a keypress.
    #[must_use]
    pub fn dtmf(&self) -> DtmfBridging {
        self.dtmf
    }
}

/// Why a call stopped being bridged.
///
/// Carried by [`CallEvent::Unbridged`], which is the answer to "the audio stopped crossing — was
/// that us?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnbridgeCause {
    /// The owner released the bridge: [`CallBridge::unbridge`], dropping the [`CallBridge`], or
    /// replacing it by bridging either call to something else.
    ///
    /// Both calls are told, because both are back to independent operation.
    Released,
    /// The call at the other end of the bridge ended.
    ///
    /// Only the surviving call is told. The one that ended reports itself with
    /// [`CallEvent::Ended`], which stays the last event on its own stream.
    PeerEnded,
}

/// Which end of a bridge a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    One,
    Two,
}

/// One value per end of a bridge.
///
/// Two named fields rather than a two-element array, so selecting an end is a match on [`Side`]
/// and never an index that could be out of range — the same shape, and the same reason, as
/// [`crate::coupling`]'s per-leg state.
#[derive(Debug)]
struct PerSide<T> {
    one: T,
    two: T,
}

impl<T> PerSide<T> {
    const fn get(&self, side: Side) -> &T {
        match side {
            Side::One => &self.one,
            Side::Two => &self.two,
        }
    }

    const fn get_mut(&mut self, side: Side) -> &mut T {
        match side {
            Side::One => &mut self.one,
            Side::Two => &mut self.two,
        }
    }

    fn both_mut(&mut self) -> [&mut T; 2] {
        [&mut self.one, &mut self.two]
    }
}

/// The bridge as one of its two calls holds it.
#[derive(Debug)]
pub(crate) struct Membership {
    link: Arc<Link>,
    side: Side,
}

impl Membership {
    pub(crate) fn new(link: Arc<Link>, side: Side) -> Self {
        Self { link, side }
    }

    /// This call is ending. Tear the bridge down and tell the other one.
    pub(crate) fn call_ending(&self) {
        self.link.call_ending(self.side);
    }

    /// The owner released this bridge, or is replacing it.
    pub(crate) fn release(&self) {
        self.link.tear_down_now(UnbridgeCause::Released);
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.link.is_connected()
    }

    /// The call replaced its media generation. Keep this same bridge lifecycle over the new one.
    pub(crate) fn rebind(&self, session: Arc<MediaSession>) {
        self.link.rebind(self.side, session);
    }
}

/// Everything one bridge owns, shared by the two calls in it and by the [`CallBridge`] handle.
///
/// The [`Mutex`] is over the bridge's *own* lifecycle, never over a call or a media session, and
/// nothing awaits while it is held. What it buys beyond mutual exclusion is event ordering: an
/// `Unbridged` for one call is emitted while the lock is held, and that call's own `Ended` can
/// only be emitted after its `Call` has taken the same lock. So `Ended` stays last on every
/// stream even when both calls end at once, which no atomic flag on its own can promise.
#[derive(Debug)]
pub(crate) struct Link {
    dtmf: DtmfBridging,
    state: Mutex<LinkState>,
}

#[derive(Debug)]
struct LinkState {
    /// The current media generation for each call. Rebinding changes only this bridge-owned
    /// handle set; neither call nor either session becomes shared mutable state.
    sessions: PerSide<Arc<MediaSession>>,
    /// The forwarding, while it runs. Dropping it aborts both directions.
    bridge: Option<Bridge>,
    /// The keypress forwarders, under [`DtmfBridging::PassThrough`].
    digits: Vec<JoinHandle<()>>,
    /// Per side, the reserved emitter for that call's stream — until the bridge ends, or that
    /// call does. `None` means "say nothing more to this one".
    reports: PerSide<Option<ReservedEmitter>>,
    /// Set once, by whichever of the four possible enders gets here first.
    closed: bool,
}

impl Link {
    /// Ownership of a bridge cannot be made unsafe by a poisoned lock: what it guards is a set of
    /// abort handles and emit permits, and refusing the lock would turn one panic into a bridge
    /// that can never be torn down.
    fn lock(&self) -> MutexGuard<'_, LinkState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn call_ending(&self, side: Side) {
        let mut state = self.lock();
        // This call's stream is about to close with `Ended`, and nothing may be queued behind it.
        // Dropping the reservation here also returns the slot the call's own events compete for.
        *state.reports.get_mut(side) = None;
        Self::tear_down(&mut state, UnbridgeCause::PeerEnded);
    }

    fn tear_down_now(&self, cause: UnbridgeCause) {
        let mut state = self.lock();
        Self::tear_down(&mut state, cause);
    }

    /// Stop forwarding and tell whoever is still listening. Idempotent.
    fn tear_down(state: &mut LinkState, cause: UnbridgeCause) {
        if state.closed {
            return;
        }
        state.closed = true;
        // Dropping the bridge aborts both directions; the sessions themselves keep running,
        // because unbridging two calls must not end them.
        drop(state.bridge.take());
        for forwarder in state.digits.drain(..) {
            forwarder.abort();
        }
        // A non-transcoding bridge puts both sessions into encoded relay, which is what sends a
        // received payload to the other leg instead of decoding it into this session's own queue.
        // Leaving it set is what would make an unbridged call deaf: `Bridge` does not clear it,
        // deliberately, because replacing one bridge with another must not clear the flag the
        // replacement has already set.
        state.sessions.one.set_relay(false);
        state.sessions.two.set_relay(false);
        for report in state.reports.both_mut() {
            if let Some(report) = report.take() {
                report.emit_reserved(CallEvent::Unbridged { cause });
            }
        }
    }

    fn is_connected(&self) -> bool {
        let state = self.lock();
        state.bridge.as_ref().is_some_and(Bridge::is_connected)
    }

    fn is_transcoding(&self) -> bool {
        let state = self.lock();
        state.bridge.as_ref().is_some_and(Bridge::is_transcoding)
    }

    /// Replace one session while retaining this bridge's identity, reports, and lifecycle.
    fn rebind(&self, side: Side, session: Arc<MediaSession>) {
        let mut state = self.lock();
        if state.closed || Arc::ptr_eq(state.sessions.get(side), &session) {
            return;
        }

        let retired = std::mem::replace(state.sessions.get_mut(side), session);
        let replacement = Bridge::connect(
            Arc::clone(&state.sessions.one),
            Arc::clone(&state.sessions.two),
        );
        let digits = match self.dtmf {
            DtmfBridging::Deliver => Vec::new(),
            DtmfBridging::PassThrough => vec![
                forward_keypresses(
                    Arc::clone(&state.sessions.one),
                    Arc::clone(&state.sessions.two),
                ),
                forward_keypresses(
                    Arc::clone(&state.sessions.two),
                    Arc::clone(&state.sessions.one),
                ),
            ],
        };

        // The replacement establishes relay on both current sessions first. Clearing it on the
        // retired generation any earlier creates a gap in which neither bridge can forward.
        let previous = state.bridge.replace(replacement);
        let previous_digits = std::mem::replace(&mut state.digits, digits);
        retired.set_relay(false);
        drop(previous);
        for forwarder in previous_digits {
            forwarder.abort();
        }
    }
}

/// Two calls a host owns, connected so each hears the other.
///
/// Held by the host for as long as the two calls should be connected. Dropping it unbridges them,
/// exactly as [`Self::unbridge`] does — a bridge nobody holds is two calls forwarding audio into
/// each other that nothing can stop.
///
/// Neither call is borrowed for the lifetime of this value: the host goes on owning both, handles
/// their signalling, and hangs either one up while it exists.
#[derive(Debug)]
pub struct CallBridge {
    link: Arc<Link>,
    dtmf: DtmfBridging,
}

impl CallBridge {
    /// Connect two calls, keeping keypresses on the call they arrived on.
    ///
    /// [`Self::connect_with`] chooses otherwise. Both take each call mutably for the length of the
    /// call and keep neither borrow.
    #[must_use]
    pub fn connect(one: &mut Call, two: &mut Call) -> Self {
        Self::connect_with(one, two, BridgeOptions::new())
    }

    /// Connect two calls, choosing what happens to a keypress.
    ///
    /// Audio starts crossing immediately: when both calls negotiated the same codec the payloads
    /// are passed through untouched, and otherwise they are decoded and re-encoded, which
    /// [`Self::is_transcoding`] reports rather than leaving to be inferred.
    ///
    /// Each call is told on its own event stream ([`CallEvent::Bridged`]), and told again when the
    /// bridge ends ([`CallEvent::Unbridged`]) — so a host driving calls from their events never
    /// has to poll this handle.
    ///
    /// **Replacing, not stacking.** A call already in a bridge is released from it first, and that
    /// bridge's other call is told. Two calls can be bridged to each other and to nothing else,
    /// which the two `&mut` borrows also make the compiler's business: a call cannot be bridged to
    /// itself.
    ///
    /// **Bridging an ended call** produces a bridge that was never connected: nothing is spawned,
    /// [`Self::is_connected`] is `false` from the start and no event is emitted. There is no audio
    /// to forward, and manufacturing an error for it would make every host handle a case it can
    /// already see with [`Call::is_ended`].
    ///
    /// Must be called from within a Tokio runtime: forwarding is tasks.
    #[must_use]
    pub fn connect_with(one: &mut Call, two: &mut Call, options: BridgeOptions) -> Self {
        one.release_bridge();
        two.release_bridge();

        let sessions = PerSide {
            one: one.media_handle(),
            two: two.media_handle(),
        };
        let live = !one.is_ended() && !two.is_ended();

        let bridge =
            live.then(|| Bridge::connect(Arc::clone(&sessions.one), Arc::clone(&sessions.two)));
        let digits = match (live, options.dtmf()) {
            (true, DtmfBridging::PassThrough) => vec![
                forward_keypresses(Arc::clone(&sessions.one), Arc::clone(&sessions.two)),
                forward_keypresses(Arc::clone(&sessions.two), Arc::clone(&sessions.one)),
            ],
            _ => Vec::new(),
        };
        let reports = if live {
            PerSide {
                one: Some(one.reserved_emitter()),
                two: Some(two.reserved_emitter()),
            }
        } else {
            PerSide {
                one: None,
                two: None,
            }
        };

        let link = Arc::new(Link {
            dtmf: options.dtmf(),
            state: Mutex::new(LinkState {
                sessions,
                bridge,
                digits,
                reports,
                closed: !live,
            }),
        });

        if live {
            one.attach_bridge(Arc::clone(&link), Side::One);
            two.attach_bridge(Arc::clone(&link), Side::Two);
            one.emit(CallEvent::Bridged);
            two.emit(CallEvent::Bridged);
        }

        Self {
            link,
            dtmf: options.dtmf(),
        }
    }

    /// Whether audio is being decoded and re-encoded rather than passed through.
    ///
    /// Reported rather than inferred, for the reason [`sipx_media::Bridge::is_transcoding`] gives:
    /// transcoding costs quality as well as CPU, and a host that cares can renegotiate one of the
    /// legs.
    #[must_use]
    pub fn is_transcoding(&self) -> bool {
        self.link.is_transcoding()
    }

    /// What this bridge does with a keypress.
    #[must_use]
    pub fn dtmf(&self) -> DtmfBridging {
        self.dtmf
    }

    /// Whether audio is still crossing in both directions.
    ///
    /// `false` once either call has ended or once the bridge has been released. A successful media
    /// renegotiation rebinds this bridge before retiring the old session, so it remains truthful
    /// across that transition.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.link.is_connected()
    }

    /// Stop forwarding and return both calls to independent operation.
    ///
    /// Neither call is ended: each receives its own far end's audio again, keeps its keypresses
    /// again, and is told on its event stream that this happened
    /// ([`CallEvent::Unbridged`] with [`UnbridgeCause::Released`]).
    ///
    /// Dropping this value does the same thing. This exists to make the moment explicit and to
    /// read as the opposite of [`Self::connect`].
    pub fn unbridge(self) {
        self.link.tear_down_now(UnbridgeCause::Released);
    }
}

impl Drop for CallBridge {
    fn drop(&mut self) {
        // Idempotent, so an explicit `unbridge` followed by this drop tears down once and emits
        // once. Without it, a bridge whose handle was abandoned would keep two calls' audio
        // crossing with nothing left to stop it.
        self.link.tear_down_now(UnbridgeCause::Released);
    }
}

/// One direction of pass-through keypresses.
///
/// Regenerated rather than relayed, and paced by the sending session, so the tone occupies the
/// slots audio would have on the leg it goes out on — exactly as a keypress this side originated.
fn forward_keypresses(from: Arc<MediaSession>, to: Arc<MediaSession>) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some((digit, held)) = from.recv_digit().await {
            if !to.send_digit(digit, held).await {
                return;
            }
        }
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn the_default_keeps_keypresses_on_the_call_they_arrived_on() {
        // The whole point of the option: a host that says nothing gets today's behaviour, so
        // bridging cannot silently start swallowing an IVR's digits.
        assert_eq!(BridgeOptions::new().dtmf(), DtmfBridging::Deliver);
        assert_eq!(
            BridgeOptions::new()
                .with_dtmf(DtmfBridging::PassThrough)
                .dtmf(),
            DtmfBridging::PassThrough
        );
    }

    #[test]
    fn each_side_selects_its_own_value() {
        // The reason `PerSide` exists rather than a two-element array: selecting an end is a
        // match, so no index can name the wrong call's stream or run off the end.
        let mut sides = PerSide { one: 1, two: 2 };
        *sides.get_mut(Side::One) = 10;
        assert_eq!(sides.one, 10);
        assert_eq!(sides.two, 2);
        assert_eq!(sides.both_mut().map(|value| *value), [10, 2]);
    }
}
