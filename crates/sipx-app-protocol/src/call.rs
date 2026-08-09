//! `sipx-call`'s [`CallEvent`] (`C-3`), lifted into the contract's vocabulary.
//!
//! Behind the `call` feature, because it is the one part of this crate that needs the call
//! framework — and a remote SDK, or a test of the state machine, must be able to have the wire and
//! the interpreter without a runtime, a socket stack and a media session arriving with them.
//!
//! There is deliberately no second event vocabulary here. `C-3`'s [`CallEvent`] is what a `Call`
//! reports; [`crate::EventKind`] is what §5.3 of the contract puts on a wire; this module is the
//! one mapping between them, so the two cannot drift into three.

use sipx_call::signal_metrics::SignalObservation;
use sipx_call::voice::{AudioDirection as CallAudioDirection, VoiceEndCause as CallVoiceEndCause};
use sipx_call::{CallEvent, EndCause as CallEndCause, TransferState as CallTransferState};

use crate::event::{
    AudioDirection, DialOutcome, EndCause, EventKind, TransferState, VoiceEndCause, VoiceThresholds,
};

/// RFC 3261 §21.4.7: the one refusal §5.3 spells with a word of its own rather than a status.
const BUSY_HERE: u16 = 486;

/// RFC 4028 §6: the status a `422` is, which is the whole of what [`sipx_call::Error`] keeps of it.
const SESSION_INTERVAL_TOO_SMALL: u16 = 422;

/// The seam's audio-direction vocabulary as the contract's (§5.3).
fn audio_direction(direction: CallAudioDirection) -> AudioDirection {
    match direction {
        CallAudioDirection::Inbound => AudioDirection::Inbound,
        CallAudioDirection::Outbound => AudioDirection::Outbound,
    }
}

/// The `sipx-call` events this bridge does not deliver, named rather than reached by falling off
/// the end of a match (`M-98`).
///
/// Each is here for a reason [`event_from_call`]'s own documentation states, and the point of the
/// set having a name is that a wildcard cannot state one. Until `M-98` this answer *was* a
/// wildcard, and it was swallowing `CallEvent::SignalMetrics` — an event §5.3 spells twice — while
/// claiming beside itself that the contract had no spelling for what it caught.
///
/// `M-99` took [`CallEvent::Bridged`] and [`CallEvent::Unbridged`] back out of this set. They were
/// here because §5.3's rows name the other `leg` and `C-6`'s events do not carry it — but that is a
/// missing *name*, not a missing event, and this function already had an answer for a name only the
/// caller knows. Membership here means the contract says nothing about an event; it never meant the
/// contract said something this function had not been given enough to say.
fn has_no_contract_event(event: &CallEvent) -> bool {
    matches!(
        event,
        CallEvent::Muted | CallEvent::Unmuted | CallEvent::ApplicationRequest(_)
    )
}

/// `M-59`'s observation stream as §5.3's two signal rows (`M-98`).
///
/// One `CallEvent` carries both, because `sipx-call` reports one observation stream per direction
/// and the contract splits it into the completed report and the silence transition. Which of the
/// two an observation is, is its own tag rather than something restated on the event.
fn signal_event(metrics: &sipx_call::SignalMetrics) -> Option<EventKind> {
    let direction = audio_direction(metrics.direction());
    Some(match metrics.observation() {
        SignalObservation::Report(report) => EventKind::SignalMetrics {
            direction,
            epoch: report.epoch,
            sequence: report.sequence,
            sample_time: report.at_sample,
            sample_rate: report.rate,
            samples: report.samples,
            windows: report.windows,
            // §10's peak is a magnitude in 0..=32,768, held in an `i32` for the arithmetic that
            // produces it. So this is exact rather than a saturating narrowing: there is no
            // negative value to lose and none above the range to clamp.
            peak: report.peak.unsigned_abs(),
            rms: report.rms,
            clipped_samples: report.clipped_samples,
            clipping_windows: report.clipping_windows,
            active_windows: report.active_windows,
            silent_windows: report.silent_windows,
        },
        SignalObservation::SilenceElapsed {
            at_sample,
            rate,
            epoch,
        } => EventKind::SignalSilence {
            direction,
            epoch: *epoch,
            sample_time: *at_sample,
            sample_rate: *rate,
        },
        // A reset, and the marker for observations the analyser's bounded queue coalesced away,
        // are its own bookkeeping and §5.3 has a row for neither. A reset is already visible to an
        // application as the `epoch` the next report names, and observations the analyser absorbed
        // are a different loss from the event stream's, which `CallEvents::dropped` counts for the
        // consumer that suffered it. The vocabulary is `#[non_exhaustive]`, so this also covers an
        // observation kind added after this arm was written.
        _ => return None,
    })
}

/// How an outbound leg resolved, in §5.3's closed `outcome` vocabulary (`M-103`).
///
/// `None` is the answered case: an attempt that produced a [`sipx_call::Call`] refused nothing, so
/// the caller passes the `Err` half of its dial and nothing else. Written as
/// `dial_outcome(result.as_ref().err())`.
///
/// # Why this is not an arm of [`event_from_call`]
///
/// Because there is no event to map. §5.3's `call.dial.finished` is the one row whose fact never
/// reaches a [`CallEvent`] at all: `sipx-call` reports a refused invitation as the `Err` of
/// [`sipx_call::dial`] — [`sipx_call::Dialing`] drops its event sink without an ending — so the
/// status a `486` carried exists only in [`sipx_call::Error::Rejected`]. A driver that watched the
/// outbound leg's event stream would see it close and never learn why. That is also why the row is
/// the driver's to compose rather than this bridge's: the composing needs the app's
/// `instruction_id` and `leg` *and* the dial's own result, and only the thing that issued the
/// effect holds all three.
///
/// # What it promises, and what it does not
///
/// It answers one question — which of §5.3's four words describes this attempt — and no others.
/// It does not say whether the leg is still up, does not read a clock, and deliberately keeps no
/// reason phrase: §5.3's `rejected{status}` carries a status and nothing else, and a second
/// spelling of the far end's prose is a second thing that can disagree with the status.
///
/// The mapping, and the judgement in each row:
///
/// - **`486`** is `busy`. It is the only status §5.3 gives a word to, so every other refusal keeps
///   its number rather than being sorted into a category the section does not have.
/// - **Any other refusal** is `rejected{status}`, including a `401`/`407` that
///   [`sipx_call::Error::AuthenticationChallenge`] carries: the far end did refuse, with that
///   status, and an app reading the number is reading what arrived.
/// - **`422`** ([`sipx_call::Error::IntervalTooBrief`]) is `rejected{422}`. The variant keeps a
///   duration instead of the status because a Rust caller retries with it; an app cannot, and
///   reporting a refusal that did arrive as `timeout` would be a false statement about the far end.
/// - **Everything else** is `timeout` — [`DialOutcome::Timeout`]'s own documented meaning is *it
///   never resolved*, which is exactly true of a deadline, of a local cancellation, of a
///   transaction that got no final response, and of a setup this side could not complete. It is the
///   value that concludes nothing about the far end, which is the right answer where nothing about
///   the far end is known. [`sipx_call::Error`] is `#[non_exhaustive]`, so this also covers a
///   variant added after this function was written.
#[must_use]
pub fn dial_outcome(refusal: Option<&sipx_call::Error>) -> DialOutcome {
    let Some(refusal) = refusal else {
        return DialOutcome::Answered;
    };
    match refusal {
        sipx_call::Error::Rejected {
            status: BUSY_HERE, ..
        } => DialOutcome::Busy,
        sipx_call::Error::Rejected { status, .. }
        | sipx_call::Error::AuthenticationChallenge { status, .. } => {
            DialOutcome::Rejected { status: *status }
        }
        sipx_call::Error::IntervalTooBrief(_) => DialOutcome::Rejected {
            status: SESSION_INTERVAL_TOO_SMALL,
        },
        _ => DialOutcome::Timeout,
    }
}

/// One `sipx-call` event as a contract event (§5.3).
///
/// `None` means **this event is not delivered to the app**, and every case of it is named below
/// rather than reached by falling off the end. That distinction is not pedantry: `M-59` shipped
/// `call.signal.metrics` and `call.signal.silence` — the §5.3 rows, the [`EventKind`] variants and
/// the wire round trip — and no arm here, so for two releases a wildcard documented as "the
/// contract has no spelling for this" was swallowing two events the contract spelled, and no host
/// could emit either. `M-98` closed that arm and made this function testable at all; the derived
/// test in `tests/spec_tables.rs` is what keeps the next one from getting the same silence.
///
/// The three reasons an event yields `None`, none of which is "unfinished":
///
/// - **§5.2, not §5.3.** `M-18`'s [`CallEvent::Muted`] and [`CallEvent::Unmuted`] surface to a
///   remote app as `media.muted` on the next snapshot rather than as an event of their own, so a
///   driver feeds those to the interpreter as [`crate::Input::MediaGate`] instead.
/// - **The caller did not name the coupling.** `C-6`'s bridge events say the media started or
///   stopped crossing and not what it crossed to; §5.3's rows require the other `leg`. A caller
///   that passes no `bridged_leg` is a host that coupled media outside §6.2's `bridge` verb, and it
///   has no contract name to put there — inventing one would be worse than saying nothing.
/// - **The contract has no row.** An in-dialog INFO or MESSAGE
///   ([`CallEvent::ApplicationRequest`]) is an owned request somebody must answer or drop, and a
///   `#[non_exhaustive]` vocabulary may add a variant this function was written before.
///
/// # What the caller supplies, and why
///
/// The names §5.3 asks for that a call's own events do not carry are the caller's, because the
/// caller is what issued the instruction they belong to:
///
/// - `instruction_id` — `sipx-call` names a playback by its own `PlaybackId` and a recording by
///   nothing at all, whereas the contract names both by the **app's** instruction id (§6.1).
/// - `bridged_leg` — the name the app gave the other leg in its `bridge` instruction (§6.2), which
///   the interpreter carries on [`crate::Effect::Bridge`]. `None` when this call is not coupled.
///   A driver must keep the name until the [`CallEvent::Unbridged`] it produces has been mapped:
///   the contract reports the coupling *ending* with the same leg it reported it beginning with,
///   and §6.2 ends a bridge on `unbridge` **or either leg ending**, so the last thing to hold the
///   name is not always the thing that dropped the coupling.
///
/// `M-99` is why the bridge rows are here rather than left to a driver. The *fact* is the call's
/// alone: [`sipx_call::UnbridgeCause::PeerEnded`] is §6.2's "either leg ending", and a driver
/// watching only its own instructions could not tell that from the coupling it broke itself.
#[must_use]
pub fn event_from_call(
    event: &CallEvent,
    instruction_id: &str,
    bridged_leg: Option<&str>,
) -> Option<EventKind> {
    if has_no_contract_event(event) {
        return None;
    }
    Some(match event {
        CallEvent::Ringing { reliable } => EventKind::Ringing {
            reliable: *reliable,
        },
        CallEvent::EarlyMediaStarted => EventKind::EarlyMediaStarted,
        CallEvent::Answered => EventKind::Answered,
        CallEvent::Dtmf { digit, duration } => EventKind::Dtmf {
            digit: digit.as_char(),
            duration_ms: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
        },
        // `M-58`. The call identity `sipx-call` puts on the transition is deliberately not repeated
        // here: §5.2's snapshot already names the call every envelope is about, and a second
        // spelling of it in the event body is a second thing that can disagree.
        CallEvent::VoiceStarted(activity) => EventKind::VoiceStarted {
            direction: audio_direction(activity.direction()),
            sequence: activity.sequence(),
            sample_time: activity.at_sample(),
            sample_rate: activity.sample_rate(),
        },
        CallEvent::VoiceEnded { activity, cause } => EventKind::VoiceEnded {
            direction: audio_direction(activity.direction()),
            sequence: activity.sequence(),
            sample_time: activity.at_sample(),
            sample_rate: activity.sample_rate(),
            cause: match cause {
                CallVoiceEndCause::Hangover => VoiceEndCause::Hangover,
                // The analyser's vocabulary is `#[non_exhaustive]`. Anything added to it that is
                // not a hangover is a reset of some kind, which §5.3 spells `cut`.
                _ => VoiceEndCause::Cut,
            },
        },
        // `M-84`. The analyser's own snapshot, narrowed to the counts and amplitudes §5.2 puts on
        // the wire: the update count, the last observed floor and the last period's outcome stay in
        // process, and §4's field rule is what lets any of them be added later without a new line.
        // Nothing here can carry audio — every value read below is a scalar, and the processing
        // contract's §3.3 and §8.1 mean there is no retained audio upstream to read.
        CallEvent::VoiceThresholds(thresholds) => {
            let effective = thresholds.effective();
            let mut wire = VoiceThresholds::new(
                audio_direction(thresholds.direction()),
                thresholds.sample_rate(),
                effective.activation_amplitude(),
                effective.window_samples(),
                effective.hangover_samples(),
            )
            .with_silence_timeout(effective.silence_timeout_samples());
            // §12.3 derives all three together or not at all, so one of them being present is the
            // whole of "this threshold moves".
            if let (Some(calibration), Some(update)) =
                (effective.calibration_samples(), effective.update_samples())
            {
                wire = wire.with_calibration(calibration, update, effective.freeze_limit_samples());
            }
            EventKind::VoiceThresholds {
                sample_time: thresholds.at_sample(),
                thresholds: wire,
            }
        }
        // `M-59`'s two rows, reachable only since `M-98`.
        CallEvent::SignalMetrics(metrics) => return signal_event(metrics),
        CallEvent::PlaybackFinished { completed, .. } => EventKind::PlaybackFinished {
            instruction_id: instruction_id.to_owned(),
            completed: *completed,
        },
        CallEvent::RecordingFinished { duration } => EventKind::RecordingFinished {
            instruction_id: instruction_id.to_owned(),
            duration_ms: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
        },
        CallEvent::TransferRequested { target, attended } => EventKind::TransferRequested {
            target: target.to_string(),
            attended: *attended,
        },
        CallEvent::TransferProgress(state) => EventKind::TransferProgress {
            state: match state {
                CallTransferState::Ringing => TransferState::Ringing,
                CallTransferState::Succeeded => TransferState::Succeeded,
                // §5.3 spells this `failed{status}` and carries no reason phrase; the one
                // `sipx-call` has is for a log, not for a wire the contract has to keep stable.
                CallTransferState::Failed { status, .. } => {
                    TransferState::Failed { status: *status }
                }
                // `Trying`, and any state `sipx-call` adds to a `#[non_exhaustive]` vocabulary
                // that §5.3's closed one has no word for. One arm, because `trying` is the value
                // that concludes nothing: reporting an unrecognised state as `succeeded` or
                // `failed` would have the application act on an outcome that has not happened.
                _ => TransferState::Trying,
            },
        },
        // `C-6`'s coupling, as §5.3's two rows (`M-99`). The leg is the caller's — see above — and
        // an unnamed coupling is the one case here that yields `None` for a row the contract does
        // spell, which is why `bridged_leg` is an `Option` a driver has to answer rather than a
        // string it can leave empty.
        CallEvent::Bridged => EventKind::Bridged {
            leg: bridged_leg?.to_owned(),
        },
        // §5.3's `call.unbridged` carries the leg and nothing else. Why the coupling ended is
        // `sipx-call`'s [`sipx_call::UnbridgeCause`], which the contract has no word for: an app
        // learns that the far leg hung up from that leg's own `call.ended`, and a second spelling
        // of it here is a second thing that can disagree.
        CallEvent::Unbridged { .. } => EventKind::Unbridged {
            leg: bridged_leg?.to_owned(),
        },
        CallEvent::Hold => EventKind::Hold,
        CallEvent::Resumed => EventKind::Resumed,
        CallEvent::Ended(cause) => EventKind::Ended {
            cause: match cause {
                CallEndCause::LocalHangup => EndCause::Hangup,
                // Both far-end endings are §5.3's one `remote` cause, which is what `C-3`'s own
                // documentation of `RemoteCancel` says they are. It is kept distinct in `sipx-call`
                // because an in-process host matching on that enum has to tell "stop ringing" from
                // "hang up"; the contract does not, and reporting a withdrawn invitation as `error`
                // would tell an application the host could not go on when the far end simply
                // changed its mind (`M-98`).
                CallEndCause::RemoteBye | CallEndCause::RemoteCancel => EndCause::Remote,
                CallEndCause::Rejected { status } => EndCause::Rejected { status: *status },
                CallEndCause::Timeout => EndCause::Timeout,
                // `C-3`'s enum is `#[non_exhaustive]`. A cause added there that §5.3 has no
                // spelling for is `error` — the contract's own "the host could not go on" —
                // rather than a guess at which of the four it resembles.
                _ => EndCause::Error,
            },
        },
        // A variant `C-3` grew after this bridge was written, and nothing else: every event
        // `sipx-call` spells today either has an arm above or is named in
        // [`has_no_contract_event`]. Not delivered, rather than guessed at — and
        // `tests/spec_tables.rs` is what makes the contract's half of that a red build.
        _ => return None,
    })
}
