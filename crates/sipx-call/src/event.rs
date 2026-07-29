//! A call's event stream (story `C-3`, the `app-sdk` epic's keystone).
//!
//! Today a [`Call`](crate::Call) is only visible by calling methods on it at the right moment —
//! `is_on_hold`, `transfer`, `is_ended` — which means a host has to know when to look. This
//! module is the alternative: every state change a call goes through is also pushed, once, as a
//! [`CallEvent`], onto a channel the call owns and hands out exactly one receiver for
//! ([`CallEvents`]).
//!
//! The vocabulary here is deliberately close to
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5's wire events — that
//! spec is what this enum exists to make buildable — but this module has no wire format and no
//! serialization; that stays out of `sipx-call` entirely and lives in the (future)
//! `sipx-app-protocol` crate (`C-5`). See also
//! [`docs/designs/app-sdk.md`](../../../docs/designs/app-sdk.md).
//!
//! Every variant here is emitted by this crate except one, and that one is deliberate:
//!
//! - `EndCause::Rejected` has no producer at this layer for a structural reason rather than a
//!   sequencing one. A [`Call`](crate::Call) does not exist until an INVITE has already
//!   succeeded (2xx and ACK), so by the time there is a call to end, refusing it is no longer
//!   possible — what ends an answered call is a BYE. Refusing happens before a `Call` is built.
//!   It is kept in the enum because the app-visible call of `C-4`/`app-host` exists from the
//!   incoming INVITE onward and will produce it, and because adding a variant after the fact is
//!   exactly the kind of wire-breaking change §4 of the contract spec warns about.
//!
//! One stream here is not a call's: [`Invitation`](crate::Invitation) hands out a [`CallEvents`]
//! too, and its only event is `Ended(EndCause::RemoteCancel)` — an invitation that was withdrawn
//! before it could become a call (`S-23`, RFC 3261 §9.2). It is the same type deliberately. A host
//! that is ringing and a host that is talking both need to be told the thing ended and why, and
//! giving the pre-answer half a channel of its own would mean two vocabularies for one question.
//!
//! `PlaybackFinished` and `RecordingFinished` are emitted by [`Call::play`](crate::Call::play)
//! and [`Call::record_until_idle`](crate::Call::record_until_idle). `M-17` added the *control*
//! half of playback — a queue, stopping, interrupting on a digit — and reports completion
//! through the same variant rather than a new one, naming the playback it is about.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::transfer::TransferState;

/// How many events the bounded channel behind [`CallEvents`] holds.
///
/// One slot of this is reserved permanently for `Ended` by the internal sender, so ordinary events
/// only ever compete for `CAPACITY - 1` of them. Chosen generously enough that a consumer busy
/// for the length of one signalling exchange does not lose anything, without being large enough
/// to hide a consumer that has stopped reading altogether.
const CAPACITY: usize = 32;

/// Something that happened to a call, in the order it happened.
///
/// Carries what [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5.3 needs
/// as the "extra fields" of its wire event of the same shape; building the full per-event call
/// snapshot from a `Call`'s other state is the interpreter's job (`C-5`), not this enum's.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum CallEvent {
    /// A provisional response was sent or received (RFC 3262), other than a bare `100 Trying` —
    /// which acknowledges only that a request arrived, not that anything is ringing.
    Ringing {
        /// Whether the provisional was reliable (100rel, RFC 3262), i.e. numbered and
        /// PRACK-acknowledged rather than fire-and-forget.
        reliable: bool,
    },
    /// The 2xx/ACK exchange completed. Media may flow.
    Answered,
    /// A telephone-event run ended: one full keypress (RFC 4733).
    Dtmf {
        /// Which key.
        digit: sipx_rtp::Digit,
        /// How long it was held, from the event's own duration field.
        duration: Duration,
    },
    /// A playback ran out, or was cut short.
    ///
    /// Emitted by [`Call::play`](crate::Call::play) and by
    /// [`Call::start_playback`](crate::Call::start_playback) — every playback either call starts
    /// resolves here exactly once, whether it ran out, was stopped, was interrupted by a keypress,
    /// or was cut off by the call ending (`M-17`).
    PlaybackFinished {
        /// Which playback. Clips queue, so a call may have several outstanding at once and
        /// "a playback finished" on its own does not say which one to move on from.
        playback: sipx_media::PlaybackId,
        /// Whether it ran to the end, as opposed to being stopped or interrupted.
        completed: bool,
    },
    /// A recording resolved.
    ///
    /// Emitted by [`Call::record_until_idle`](crate::Call::record_until_idle) and
    /// [`Call::record_at_least`](crate::Call::record_at_least), whichever ended the recording.
    RecordingFinished {
        /// How much was recorded — the audio itself, not counting the trailing silence that
        /// detected the end of it.
        duration: Duration,
    },
    /// The far end asked to transfer this call here (RFC 3515 REFER).
    TransferRequested {
        /// Where the transferor wants the call sent.
        target: sipx_sip::Uri,
        /// Whether the `Refer-To` carries a `Replaces` — an attended transfer, handing this
        /// call the place of one the transferor already has, rather than a blind one.
        attended: bool,
    },
    /// A transfer this side asked for moved on (RFC 3515 NOTIFY).
    TransferProgress(TransferState),
    /// The far end put the call on hold.
    Hold,
    /// The far end took the call off hold.
    Resumed,
    /// This side gated its own outbound audio ([`Call::mute`](crate::Call::mute)).
    ///
    /// A local decision, not a signalled one: unlike [`Self::Hold`] this reports something *this*
    /// side did, and the far end was told nothing about it. It is emitted only on a transition —
    /// muting a call that is already muted is not something that happened.
    ///
    /// The contract's own vocabulary has no wire event for this, deliberately
    /// ([`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5.3 — `mute` is an
    /// instruction that completes immediately). What a remote app sees is `media.muted` on the
    /// next snapshot (§5.2). This variant is what lets the interpreter build that snapshot from a
    /// push rather than by polling the call.
    Muted,
    /// This side let its outbound audio through again ([`Call::unmute`](crate::Call::unmute)).
    Unmuted,
    /// The call is over. Always the last event on the stream — the channel's delivery policy
    /// reserves a slot for it specifically so this is never the one an overflow drops.
    Ended(EndCause),
}

/// Why a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EndCause {
    /// This side hung up.
    LocalHangup,
    /// The far end sent a BYE.
    RemoteBye,
    /// The far end gave up before this side answered, and sent a CANCEL (RFC 3261 §9.2).
    ///
    /// The one cause that belongs to an invitation rather than to a [`Call`](crate::Call), and it
    /// is why [`Invitation`](crate::Invitation) has an event stream of its own: an application
    /// that is ringing has to be *told* to stop, and polling
    /// [`Invitation::is_cancelled`](crate::Invitation::is_cancelled) is not being told.
    ///
    /// Distinct from [`Self::RemoteBye`] on purpose, even though both are the far end ending
    /// things. A BYE ends a call that was answered and may have carried media; a CANCEL ends one
    /// that never was, so there is no call to report duration or quality for and nothing to send
    /// a BYE of one's own about. On the wire vocabulary of
    /// [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5.3 both are the
    /// `remote` cause; the distinction is kept here because this enum is what a host in-process
    /// matches on, and collapsing it would make "stop ringing" indistinguishable from "hang up".
    RemoteCancel,
    /// The call was refused with a status, rather than answered and later ended.
    ///
    /// The direction is worth being exact about: this is the contract's `reject` *instruction*
    /// (`docs/specs/app-contract.md` §5.3, `call.ended` with cause `rejected{status}`) — **this
    /// side refusing an invitation** — not the far end refusing an attempt of ours. An outbound
    /// attempt that is refused is `call.dial.finished` with outcome `rejected`, a different
    /// event about a different leg.
    ///
    /// Has no producer at this layer, and the reason is structural rather than unfinished work:
    /// a [`Call`](crate::Call) does not exist until an INVITE has already succeeded (2xx and
    /// ACK), so by the time there is a call to end, refusing it is no longer possible — what
    /// ends an answered call is a BYE. Refusing happens before a `Call` is built, and the
    /// refusal is a response rather than an event on a stream nobody is holding yet.
    ///
    /// It stays in the enum because the app-visible call of `C-4`/`A-2` exists from the incoming
    /// INVITE onward and will produce it, and because adding a variant after the fact is exactly
    /// the wire-breaking change §4 of the contract spec warns about.
    Rejected {
        /// The status the call was refused with.
        status: u16,
    },
    /// The far end stopped answering (the RFC 4028 session timer expired, RFC 3261 Timer B/F
    /// gave up, or the far end otherwise went silent) and this side gave up on it.
    Timeout,
}

/// A call's event stream: one receiver, bounded, owned by whoever calls
/// [`Call::events`](crate::Call::events) first.
///
/// There is exactly one consumer by construction (vision principle 3 — own it, don't share it):
/// `Call::events` hands this out once and returns `None` on every call after, rather than a
/// value anyone could clone a second reader from.
#[derive(Debug)]
pub struct CallEvents(mpsc::Receiver<CallEvent>);

impl CallEvents {
    /// The next event, or `None` once the call has dropped its sender.
    ///
    /// A well-behaved consumer only ever sees `None` *after* it has already seen
    /// [`CallEvent::Ended`] — the sender is not dropped until the call's own destructor runs,
    /// which is after the last event has been queued.
    pub async fn recv(&mut self) -> Option<CallEvent> {
        self.0.recv().await
    }

    /// The next event if one is already queued, without waiting.
    #[must_use]
    pub fn try_recv(&mut self) -> Option<CallEvent> {
        self.0.try_recv().ok()
    }
}

/// Where a call's events go.
///
/// # Overflow policy
///
/// The channel is bounded at [`CAPACITY`], and a slow consumer must not be able to stall a
/// call's signalling — the whole reason a channel replaces a method call is that the caller of
/// [`Call::handle`](crate::Call::handle) cannot be made to wait on some other party's attention.
/// So every ordinary event is enqueued with [`mpsc::Sender::try_send`]: if the queue is full,
/// the event is dropped rather than awaited for room. A consumer that falls behind loses
/// history, not correctness, which matches the contract's own recovery story
/// ([`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5.1: every event
/// carries a full snapshot, and a gap is resolved by resynchronising from the next one).
///
/// `Ended` is the one event this must never happen to — it is a call's last word, and a
/// consumer that never learns a call ended has no way to know it should stop waiting for one.
/// So one slot of the channel's capacity is reserved the moment the channel is built, before any
/// ordinary event has had the chance to claim it, and held, unused, until the call ends.
/// [`Self::end`] spends that reservation, which guarantees `Ended` a place to land regardless of
/// how full the other `CAPACITY - 1` slots are — and does it without an `await`, so nothing
/// about ending a call can block on whether anyone is reading its events.
#[derive(Debug)]
pub(crate) struct EventSink {
    tx: mpsc::Sender<CallEvent>,
    /// Reserved at construction, spent by [`Self::end`]. `None` afterwards — or, defensively,
    /// if the reservation itself could not be made, which does not happen in practice: `CAPACITY`
    /// is never zero, so a freshly built channel always has a free slot to reserve.
    ended_slot: Option<mpsc::OwnedPermit<CallEvent>>,
}

impl EventSink {
    /// A fresh channel, with its `Ended` slot already reserved.
    pub(crate) fn new() -> (Self, CallEvents) {
        let (tx, rx) = mpsc::channel(CAPACITY);
        let ended_slot = tx.clone().try_reserve_owned().ok();
        (Self { tx, ended_slot }, CallEvents(rx))
    }

    /// Emit an event other than `Ended` — use [`Self::end`] for that one.
    ///
    /// Never blocks; dropped rather than queued if the consumer is behind (see the type's
    /// overflow policy above).
    pub(crate) fn emit(&self, event: CallEvent) {
        self.emitter().emit(event);
    }

    /// A handle that can emit ordinary events from somewhere the `Call` itself is not.
    ///
    /// Needed by playback (`M-17`): a clip started with
    /// [`Call::start_playback`](crate::Call::start_playback) is not awaited by the caller, so
    /// something has to be watching it in order to report its end — and that something outlives
    /// the borrow of the call that started it.
    ///
    /// It cannot emit `Ended`: the reserved slot is not clonable, and a call's last word belongs
    /// to the call.
    pub(crate) fn emitter(&self) -> Emitter {
        Emitter(self.tx.clone())
    }

    /// Emit `Ended`, through the capacity reserved for it at construction.
    ///
    /// Infallible in the sense this type cares about: whether or not anyone is still listening,
    /// this returns without blocking and without silently discarding the call's last event.
    pub(crate) fn end(&mut self, cause: EndCause) {
        match self.ended_slot.take() {
            Some(permit) => {
                // `OwnedPermit::send` cannot fail — the capacity was already reserved — and
                // hands back the `Sender`, which there is no further use for.
                let _ = permit.send(CallEvent::Ended(cause));
            }
            // Only reachable if the reservation at construction failed, which a nonzero
            // `CAPACITY` never lets happen. Falling back to `try_send` rather than doing
            // nothing means this is still delivered whenever the queue happens to have room.
            None => {
                let _ = self.tx.try_send(CallEvent::Ended(cause));
            }
        }
    }
}

/// A detached emitter for ordinary events, handed out by [`EventSink::emitter`].
///
/// The same overflow policy as the sink it came from — `try_send`, dropped rather than awaited —
/// which is what makes it safe to hold in a spawned task: nothing about reporting a playback can
/// park on whether anyone is reading the call's events.
#[derive(Debug, Clone)]
pub(crate) struct Emitter(mpsc::Sender<CallEvent>);

impl Emitter {
    pub(crate) fn emit(&self, event: CallEvent) {
        debug_assert!(
            !matches!(event, CallEvent::Ended(_)),
            "Ended must go through `EventSink::end`, which spends the reserved slot"
        );
        if self.0.try_send(event).is_err() {
            tracing::debug!("a call event was dropped: the consumer is behind");
        }
    }
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

    /// The overflow policy: once the queue is full, further ordinary events are dropped rather
    /// than awaited for room or left to grow the queue without bound.
    #[test]
    fn ordinary_events_are_dropped_once_the_queue_is_full() {
        let (sink, mut events) = EventSink::new();

        // One slot is reserved for `Ended` at construction, so ordinary events can only ever
        // fill `CAPACITY - 1` of the channel's slots.
        for _ in 0..CAPACITY + 8 {
            sink.emit(CallEvent::Answered);
        }

        let mut received = 0usize;
        while events.try_recv().is_some() {
            received += 1;
        }
        assert_eq!(
            received,
            CAPACITY - 1,
            "the queue must hold exactly the ordinary capacity and no more"
        );
    }

    /// `Ended` must survive even when every ordinary slot is already spoken for — it is a
    /// call's last word, and the one event the overflow policy above may never touch.
    #[test]
    fn ended_survives_a_full_queue() {
        let (mut sink, mut events) = EventSink::new();

        for _ in 0..CAPACITY + 8 {
            sink.emit(CallEvent::Answered);
        }
        sink.end(EndCause::LocalHangup);

        let mut received = Vec::new();
        while let Some(event) = events.try_recv() {
            received.push(event);
        }

        assert!(
            received.len() <= CAPACITY,
            "the queue must not grow past its bound: {}",
            received.len()
        );
        assert!(
            matches!(
                received.last(),
                Some(CallEvent::Ended(EndCause::LocalHangup))
            ),
            "Ended must arrive, and last, despite the queue having been full: {received:?}"
        );
    }

    /// A channel nobody has read from at all is the same case as a full one, and `end` must
    /// not block on it either.
    #[test]
    fn ending_never_blocks_even_with_no_consumer() {
        let (mut sink, events) = EventSink::new();
        drop(events);
        sink.end(EndCause::RemoteBye);
        // Reaching this line at all is the assertion: `end` took no `.await` and cannot have
        // parked waiting for capacity that will now never be reclaimed.
    }
}
