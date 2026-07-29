//! `sipx-call`'s [`CallEvent`] (`C-3`), lifted into the contract's vocabulary.
//!
//! Behind the `call` feature, because it is the one part of this crate that needs the call
//! framework — and a remote SDK, or a test of the state machine, must be able to have the wire and
//! the interpreter without a runtime, a socket stack and a media session arriving with them.
//!
//! There is deliberately no second event vocabulary here. `C-3`'s [`CallEvent`] is what a `Call`
//! reports; [`crate::EventKind`] is what §5.3 of the contract puts on a wire; this module is the
//! one mapping between them, so the two cannot drift into three.

use sipx_call::{CallEvent, EndCause as CallEndCause, TransferState as CallTransferState};

use crate::event::{EndCause, EventKind, TransferState};

/// One `sipx-call` event as a contract event (§5.3).
///
/// `None` for the events §5.3 has no type for, and that is not a gap: `M-18`'s
/// [`CallEvent::Muted`] and [`CallEvent::Unmuted`] surface to a remote app as `media.muted` on the
/// next snapshot (§5.2) rather than as an event of their own, so a driver feeds those to the
/// interpreter as [`crate::Input::MediaGate`] instead.
///
/// The correlation ids §5.3 asks for are the caller's to supply. `sipx-call` names a playback by
/// its own `PlaybackId` and a recording by nothing at all, whereas the contract names both by the
/// **app's** instruction id (§6.1) — so the driver, which is what issued the effect and therefore
/// knows which instruction a handle belongs to, passes it in.
#[must_use]
pub fn event_from_call(event: &CallEvent, instruction_id: &str) -> Option<EventKind> {
    Some(match event {
        CallEvent::Ringing { reliable } => EventKind::Ringing {
            reliable: *reliable,
        },
        CallEvent::Answered => EventKind::Answered,
        CallEvent::Dtmf { digit, duration } => EventKind::Dtmf {
            digit: digit.as_char(),
            duration_ms: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
        },
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
                CallTransferState::Trying => TransferState::Trying,
                CallTransferState::Ringing => TransferState::Ringing,
                CallTransferState::Succeeded => TransferState::Succeeded,
                // §5.3 spells this `failed{status}` and carries no reason phrase; the one
                // `sipx-call` has is for a log, not for a wire the contract has to keep stable.
                CallTransferState::Failed { status, .. } => {
                    TransferState::Failed { status: *status }
                }
                // `C-3`'s enum is `#[non_exhaustive]`; a state added there with no §5.3 spelling
                // is not ours to invent a wire form for.
                _ => return None,
            },
        },
        CallEvent::Hold => EventKind::Hold,
        CallEvent::Resumed => EventKind::Resumed,
        CallEvent::Ended(cause) => EventKind::Ended {
            cause: match cause {
                CallEndCause::LocalHangup => EndCause::Hangup,
                CallEndCause::RemoteBye => EndCause::Remote,
                CallEndCause::Rejected { status } => EndCause::Rejected { status: *status },
                CallEndCause::Timeout => EndCause::Timeout,
                // `C-3`'s enum is `#[non_exhaustive]`. A cause added there that §5.3 has no
                // spelling for is `error` — the contract's own "the host could not go on" —
                // rather than a guess at which of the four it resembles.
                _ => EndCause::Error,
            },
        },
        // §5.2, not §5.3: see this function's own documentation.
        CallEvent::Muted | CallEvent::Unmuted => return None,
        _ => return None,
    })
}
