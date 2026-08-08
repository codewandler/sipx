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
//! `VoiceStarted` and `VoiceEnded` are emitted only after
//! [`Call::detect_voice_activity`](crate::Call::detect_voice_activity) has been asked for (`M-58`),
//! by a watcher the call owns and joins before it ends — which is what keeps `Ended` the last event
//! on the stream even though the transitions come from a task. See [`crate::voice`] for the
//! transition and coalescing policy behind them.
//!
//! `PlaybackFinished` and `RecordingFinished` are emitted by [`Call::play`](crate::Call::play)
//! and [`Call::record_until_idle`](crate::Call::record_until_idle). `M-17` added the *control*
//! half of playback — a queue, stopping, interrupting on a digit — and reports completion
//! through the same variant rather than a new one, naming the playback it is about.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// A provisional answer completed offer/answer and the early dialog's media session is now
    /// running (RFC 3960 section 3.2).
    ///
    /// Emitted after the session starts and before [`Self::Answered`], exactly once. This is the
    /// signal an application uses to stop a locally generated ringing tone and render what the
    /// far end is sending instead. A provisional that carries no usable reliable answer never
    /// emits it.
    EarlyMediaStarted,
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
    /// Voice began on one side of this call's audio (`M-58`).
    ///
    /// Deterministic signal analysis, not recognition: no speech model is loaded to produce this,
    /// and the decision is the integer window predicate of
    /// [`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md) §5.3.
    /// Emitted only after [`Call::detect_voice_activity`](crate::Call::detect_voice_activity) has
    /// been asked for, and only on transitions — the payload says which call, which direction,
    /// where in the audio, and where in this call's ordered observations.
    VoiceStarted(crate::VoiceActivity),
    /// Voice ended on one side of this call's audio (`M-58`).
    ///
    /// The position is the end of the last active window, not where the decision was reached: a
    /// hangover is how long the analyser waits before concluding, and reporting the wait as part of
    /// the speech would move every ending later than it happened.
    VoiceEnded {
        /// Which call, which direction, and where.
        activity: crate::VoiceActivity,
        /// Whether the hangover elapsed, or a reset cut it — a discontinuity, a format change, or
        /// the call's audio finishing.
        cause: sipx_audio::analysis::VoiceEndCause,
    },
    /// A deterministic signal-metric observation for one side of this call's audio (`M-59`).
    ///
    /// Level, energy, clipping and silence, shaped out of the same analyser's per-window facts
    /// that produce [`Self::VoiceStarted`] — the integer window arithmetic of
    /// [`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md) §5,
    /// coalesced into the reporting cadence the profile declares. Emitted only after
    /// [`Call::report_signal_metrics`](crate::Call::report_signal_metrics) has been asked for.
    ///
    /// **This is signal content, not network quality.** Loss, jitter, round-trip time and the MOS
    /// estimate are `M-10`'s RTP/RTCP snapshot ([`sipx_media::MediaSession::quality`]); nothing
    /// here describes packet delivery, no field of that snapshot changes meaning, and neither
    /// substitutes for the other. The call is this stream's call; direction, epoch, report
    /// sequence, sample position and window coverage all ride the payload.
    SignalMetrics(crate::SignalMetrics),
    /// An application-owned method arrived inside this dialog.
    ///
    /// INFO and MESSAGE are admitted directly. A private extension token appears here only after
    /// [`Call::admit_dialog_method`](crate::Call::admit_dialog_method) admitted it. The owned
    /// value must be answered or dropped; either outcome resolves its server transaction.
    ApplicationRequest(crate::ApplicationRequest),
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
pub struct CallEvents {
    rx: mpsc::Receiver<CallEvent>,
    drops: Arc<Drops>,
}

impl CallEvents {
    /// How many of this call's events were dropped because this consumer was behind (§12.1).
    ///
    /// **Per call, and reported to the consumer that lost them, rather than folded into an
    /// endpoint-wide total.** The overflow policy above is deliberate — a slow consumer loses
    /// history, not correctness — but §12.1's rule is that a discard is *counted*, and until
    /// `X-54` this one was reported with a `tracing::debug!` and nothing else, which is the exact
    /// failure that section names.
    ///
    /// It is not in [`SignallingCounts`](crate::SignallingCounts) because a crate-wide total would
    /// say that some call somewhere lost some events, which is not something anyone can act on.
    /// The party who can act is the one holding this receiver: it fell behind, and resynchronising
    /// from the next event's snapshot is the documented recovery (`docs/specs/app-contract.md`
    /// §5.1). Monotonic, and never reset by reading it.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.drops.get()
    }
}

impl CallEvents {
    /// The next event, or `None` once the call has dropped its sender.
    ///
    /// A well-behaved consumer only ever sees `None` *after* it has already seen
    /// [`CallEvent::Ended`] — the sender is not dropped until the call's own destructor runs,
    /// which is after the last event has been queued.
    pub async fn recv(&mut self) -> Option<CallEvent> {
        self.rx.recv().await
    }

    /// The next event if one is already queued, without waiting.
    #[must_use]
    pub fn try_recv(&mut self) -> Option<CallEvent> {
        self.rx.try_recv().ok()
    }
}

/// Events this call's consumer was too far behind to be given.
///
/// One atomic, shared by the sink, every [`Emitter`] it hands out, and the [`CallEvents`] that
/// reads it — which is what lets the count be taken at the site that drops the event without
/// threading anything through the call's construction. `Relaxed`, for the same reason
/// `sipx-transport`'s meters are: nothing here guards data and no reader draws a conclusion from
/// the order of two increments.
#[derive(Debug, Default)]
pub(crate) struct Drops(AtomicU64);

impl Drops {
    fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
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
    /// Shared with every [`Emitter`] and with the [`CallEvents`] that reads it.
    drops: Arc<Drops>,
}

impl EventSink {
    /// A fresh channel, with its `Ended` slot already reserved.
    pub(crate) fn new() -> (Self, CallEvents) {
        let (tx, rx) = mpsc::channel(CAPACITY);
        let ended_slot = tx.clone().try_reserve_owned().ok();
        let drops = Arc::new(Drops::default());
        (
            Self {
                tx,
                ended_slot,
                drops: Arc::clone(&drops),
            },
            CallEvents { rx, drops },
        )
    }

    /// Emit an event other than `Ended` — use [`Self::end`] for that one.
    ///
    /// Never blocks; dropped rather than queued if the consumer is behind (see the type's
    /// overflow policy above).
    pub(crate) fn emit(&self, event: CallEvent) {
        self.emitter().emit(event);
    }

    /// An emitter that also holds a slot reserved for one event it must never lose.
    ///
    /// `M-58`'s voice-activity watcher is the case this exists for. It reports *transitions*, so a
    /// dropped close would leave an application believing voice is still open on a call whose audio
    /// has finished — the one drop in that stream that is a correctness failure rather than lost
    /// history. The reservation is taken when detection starts and held, unused, until it ends,
    /// exactly as [`EventSink::new`] reserves the slot for `Ended`.
    ///
    /// It costs one of the channel's [`CAPACITY`] slots for as long as it is held, so ordinary
    /// events compete for one fewer. That is the price of the guarantee, and it is bounded: there
    /// is at most one such reservation per attached watcher, and the seam bounds those.
    pub(crate) fn reserved_emitter(&self) -> ReservedEmitter {
        ReservedEmitter {
            emitter: self.emitter(),
            reserved: self.tx.clone().try_reserve_owned().ok(),
        }
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
        Emitter {
            tx: self.tx.clone(),
            drops: Arc::clone(&self.drops),
        }
    }

    /// Emit `Ended`, through the capacity reserved for it at construction.
    ///
    /// Infallible in the sense this type cares about: whether or not anyone is still listening,
    /// this returns without blocking and without silently discarding the call's last event.
    pub(crate) fn end(&mut self, cause: EndCause) {
        match self.ended_slot.take() {
            Some(permit) => {
                // discard: nothing is lost here. `OwnedPermit::send` cannot fail — the capacity
                // was reserved at construction and is held until this moment — and what it returns
                // is the `Sender`, not a result about the event. The event is delivered.
                let _ = permit.send(CallEvent::Ended(cause));
            }
            // Only reachable if the reservation at construction failed, which a nonzero
            // `CAPACITY` never lets happen. Falling back to `try_send` rather than doing
            // nothing means this is still delivered whenever the queue happens to have room.
            None => {
                if self.tx.try_send(CallEvent::Ended(cause)).is_err() {
                    // A call's last word, lost. Counted rather than reasoned away: this branch is
                    // unreachable today, and a count is how anyone would ever learn that stopped
                    // being true — which is precisely what a reason saying "cannot happen" cannot
                    // do.
                    self.drops.bump();
                }
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
pub(crate) struct Emitter {
    tx: mpsc::Sender<CallEvent>,
    drops: Arc<Drops>,
}

impl Emitter {
    pub(crate) fn emit(&self, event: CallEvent) {
        // discard: the dropped *event* is already counted inside `try_emit`, which increments
        // `drops` on a full channel. What is discarded here is only the boolean saying whether
        // the consumer got it, which this caller has no use for.
        let _ = self.try_emit(event);
    }

    /// Emit, and say whether the consumer got it.
    ///
    /// The overflow policy is unchanged — dropped rather than awaited, and counted. What the
    /// answer buys is a producer that reports *transitions* rather than facts: `M-58`'s watcher
    /// keeps the state it could not deliver and retries it against the latest one, instead of
    /// leaving an application holding a picture the drop made wrong.
    pub(crate) fn try_emit(&self, event: CallEvent) -> bool {
        debug_assert!(
            !matches!(event, CallEvent::Ended(_)),
            "Ended must go through `EventSink::end`, which spends the reserved slot"
        );
        if self.tx.try_send(event).is_err() {
            // §12.1: a discard whose reason is logged but not counted is still a failure, because
            // logs rotate and "how often" should not be answered with `grep | wc -l`. The count is
            // read by the consumer that lost them, through `CallEvents::dropped`.
            self.drops.bump();
            tracing::debug!("a call event was dropped: the consumer is behind");
            return false;
        }
        true
    }
}

/// An [`Emitter`] carrying one reserved slot, for a producer with exactly one event it may not
/// lose.
///
/// See [`EventSink::reserved_emitter`] for why voice activity needs one. Dropping this without
/// spending the reservation releases it back to the channel.
#[derive(Debug)]
pub(crate) struct ReservedEmitter {
    emitter: Emitter,
    /// Taken when the watcher starts, spent by [`Self::emit_reserved`]. `None` afterwards — or,
    /// defensively, if the reservation could not be made because the channel was already full of
    /// events nobody had read.
    reserved: Option<mpsc::OwnedPermit<CallEvent>>,
}

impl ReservedEmitter {
    /// Emit an ordinary event, reporting whether the consumer got it.
    pub(crate) fn try_emit(&self, event: CallEvent) -> bool {
        self.emitter.try_emit(event)
    }

    /// Spend the reservation on the one event that has to arrive.
    ///
    /// Never awaits, so nothing about a call's audio finishing can park on whether anyone is
    /// reading its events.
    pub(crate) fn emit_reserved(mut self, event: CallEvent) {
        match self.reserved.take() {
            // discard: nothing is lost. `OwnedPermit::send` cannot fail — the capacity was reserved
            // when the watcher started and held until this moment — and what it returns is the
            // `Sender`, not a result about the event.
            Some(permit) => {
                let _ = permit.send(event);
            }
            // Only reachable if the reservation itself could not be made. Falling back to the
            // ordinary path means this is still delivered whenever the queue happens to have room,
            // and counted when it does not.
            None => self.emitter.emit(event),
        }
    }
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

    /// §12.1: the drops the policy above permits are **counted**, not only logged.
    ///
    /// This was a `tracing::debug!` and nothing else until `X-54` — one of the seven dialog-layer
    /// discards `X-51` found by hand, and the exact failure §12.1 names: a discard whose reason is
    /// logged but not counted still leaves "how often" to be answered with `grep | wc -l`.
    #[test]
    fn dropped_events_are_counted_for_the_consumer_that_lost_them() {
        const OVERRUN: usize = 8;

        let (sink, events) = EventSink::new();
        assert_eq!(events.dropped(), 0, "nothing has been dropped yet");

        for _ in 0..CAPACITY + OVERRUN {
            sink.emit(CallEvent::Answered);
        }

        assert_eq!(
            events.dropped(),
            OVERRUN as u64 + 1,
            "every event past the ordinary capacity is one drop — and the reserved `Ended` slot \
             is why there is one more of them than the overrun"
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
