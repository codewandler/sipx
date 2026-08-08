//! Deterministic signal metrics for a live call: level, energy, clipping and silence (`M-59`).
//!
//! [`sipx_audio::signal`] computes the facts; this module is what puts a call's audio in front of
//! it and what carries the answers out as [`CallEvent::SignalMetrics`]. Between the two sits
//! `M-54`'s bounded PCM processing seam ([`docs/specs/call-audio-seam.md`](../../../docs/specs/call-audio-seam.md)),
//! which is the **one** tap into call media: this module opens no second one, and
//! [`docs/specs/call-audio-processing.md`](../../../docs/specs/call-audio-processing.md) §9
//! forbids it by name.
//!
//! # What these numbers are, and what they are not
//!
//! They are **signal content**: what the audio itself contained. They are not
//! [`MediaSession::quality`](sipx_media::MediaSession::quality)'s RTP/RTCP snapshot — loss, jitter,
//! round-trip time and the MOS estimate — which is `M-10`'s surface and stays exactly as it was.
//! Nothing here measures packet delivery and nothing here changes the meaning of a field there.
//! The two answer different questions: a call with no packet loss at all can be clipping, and a
//! call losing a quarter of its packets can carry a perfectly clean level in the audio that did
//! arrive.
//!
//! # Three things it cannot do
//!
//! **It cannot block the call.** Frames reach the observer through the seam's per-attachment
//! bounded queue, whose offer never waits; observations leave through the call's own event
//! channel, which drops rather than blocking when a consumer is behind. Neither RTP decode, RTP
//! encode, playback nor capture can be stalled by an application that stopped reading.
//!
//! **It cannot grow with call duration.** The processor's state is a constant of its profile, the
//! seam's queue is bounded at attachment, and the reporting cadence
//! ([`SignalProfile::with_windows_per_report`]) bounds how many events a second of audio can
//! produce. An overrun is reported as [`SignalObservation::Lost`] rather than buffered.
//!
//! **It cannot report samples from an earlier call or format.** Every observer owns its processor
//! and its attachment; no state is shared between two calls or two directions. Within one call, a
//! discontinuity, a re-anchored timeline (which is how a media renegotiation arrives here) and an
//! explicit reset all open a new epoch, and every report names the epoch and rate it was measured
//! in.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sipx_audio::PcmSamples;
use sipx_audio::signal::{
    ProfileError, SignalDirection, SignalDiscontinuity, SignalFrame, SignalProcessor, SignalProfile,
};
use sipx_media::{
    AudioDirection, DiscontinuityKind, MediaSession, PcmEncoding, PcmFormat, PcmProcessor,
    Processing, ProcessingError,
};
use tokio_util::sync::CancellationToken;

use crate::event::{CallEvent, Emitter};

/// Why a call could not start observing its own signal (`M-59`).
///
/// Every variant leaves the call exactly as it was: an observer is refused rather than a call
/// distorted or dropped to make room for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SignalMetricsError {
    /// The analysis profile is outside a domain `call-audio-processing.md` §5.1 declares.
    #[error("signal analysis profile: {0}")]
    Profile(#[from] ProfileError),
    /// `M-54`'s seam refused the attachment — an unsupported format, a queue depth outside its
    /// domain, too many processors on this call, or a session that has already stopped.
    #[error("call media seam: {0}")]
    Attachment(#[from] ProcessingError),
}

/// A running signal-metric observer, owned by whoever asked for it.
///
/// Dropping it detaches from the seam and ends the observer; nothing about the call changes.
/// [`Self::stop`] does the same and *waits* for it, so completion is an event a caller can observe
/// rather than a duration it has to guess.
#[derive(Debug)]
pub struct SignalMetrics {
    profile: SignalProfile,
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
    refused: Arc<AtomicU64>,
}

impl SignalMetrics {
    /// The profile this observer measures under.
    #[must_use]
    pub const fn profile(&self) -> SignalProfile {
        self.profile
    }

    /// Which side of the call it observes.
    #[must_use]
    pub const fn direction(&self) -> SignalDirection {
        self.profile.direction()
    }

    /// Frames the seam delivered that the processor refused, over this observer's whole life.
    ///
    /// Expected to stay zero: the seam's sequence is strictly increasing and it always flags a
    /// gap, which is exactly what `call-audio-processing.md` §3.4 accepts. It is counted rather
    /// than only logged because a discard nobody counts is answered with `grep | wc -l`
    /// (`docs/specs/media-runtime.md` §4), and a non-zero value here is a defect in the seam
    /// rather than in the audio.
    #[must_use]
    pub fn refused_frames(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Detach and wait for the observer to finish.
    ///
    /// After this returns, the attachment is released and no further
    /// [`CallEvent::SignalMetrics`] for it can be queued.
    pub async fn stop(mut self) {
        self.stop.cancel();
        if let Some(task) = self.task.take() {
            // discard: the observer's loop returns `()` and a cancellation `JoinError` says only
            // that it was already gone, which is the outcome being waited for either way.
            let _ = task.await;
        }
    }
}

impl Drop for SignalMetrics {
    fn drop(&mut self) {
        // Cancelling is enough: the loop selects on this token, so it wakes, returns, and drops
        // the attachment — which is what deregisters it from the seam. Aborting instead could cut
        // the task between draining an observation and emitting it.
        self.stop.cancel();
    }
}

/// Attach an observer to one direction of a session's audio and start reporting to `emitter`.
pub(crate) fn observe(
    media: &MediaSession,
    profile: SignalProfile,
    emitter: Emitter,
) -> Result<SignalMetrics, SignalMetricsError> {
    // The profile is validated before the seam is asked for anything, so a bad threshold never
    // costs the call an attachment slot.
    let processor = SignalProcessor::new(profile)?;

    // The processor consumes signed 16-bit mono at its declared rate, and the seam owns the
    // conversion into it (`call-audio-seam.md` §5, reusing `M-43`). This crate resamples nothing.
    let format = PcmFormat::new(profile.rate(), PcmEncoding::Signed16)
        .map_err(|error| SignalMetricsError::Attachment(ProcessingError::from(error)))?;
    let attachment = media.attach_processor(Processing::new(seam_direction(profile.direction()), format))?;

    let stop = CancellationToken::new();
    let refused = Arc::new(AtomicU64::new(0));
    let task = tokio::spawn(run(
        attachment,
        processor,
        emitter,
        stop.clone(),
        Arc::clone(&refused),
    ));

    Ok(SignalMetrics {
        profile,
        stop,
        task: Some(task),
        refused,
    })
}

/// Read the seam, feed the processor, and emit what it drains.
///
/// Ends when the attachment finishes — the session stopped, or the handle was dropped — or when
/// the observer is cancelled. Either way the attachment is dropped here, which deregisters it.
async fn run(
    mut attachment: PcmProcessor,
    mut processor: SignalProcessor,
    emitter: Emitter,
    stop: CancellationToken,
    refused: Arc<AtomicU64>,
) {
    let direction = processor.profile().direction();
    loop {
        let frame = tokio::select! {
            biased;
            () = stop.cancelled() => break,
            frame = attachment.recv() => match frame {
                Some(frame) => frame,
                None => break,
            },
        };

        let sequence = frame.sequence();
        let discontinuity = frame.discontinuity().map(|brk| analysis_kind(brk.kind()));
        let samples = match frame.into_pcm().into_samples() {
            PcmSamples::Signed16(samples) => samples,
            // Unreachable: the attachment above asked for `Signed16`. Counted rather than
            // unwrapped, because a panic here would be on the call's own runtime.
            other => {
                debug_assert!(false, "the seam delivered {other:?} to a Signed16 attachment");
                refused.fetch_add(1, Ordering::Relaxed);
                continue;
            }
        };

        let mut input = SignalFrame::new(direction, sequence, &samples);
        if let Some(kind) = discontinuity {
            input = input.with_discontinuity(kind);
        }
        if let Err(error) = processor.process(input) {
            refused.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(%error, "a call PCM frame was refused by signal analysis");
        }

        for observation in processor.drain() {
            emitter.emit(CallEvent::SignalMetrics {
                direction,
                observation,
            });
        }
    }
}

/// The seam's direction vocabulary, from the analyser's.
const fn seam_direction(direction: SignalDirection) -> AudioDirection {
    match direction {
        SignalDirection::Inbound => AudioDirection::Inbound,
        SignalDirection::Outbound => AudioDirection::Outbound,
    }
}

/// The analyser's discontinuity vocabulary, from the seam's.
///
/// Both are `call-audio-processing.md` §3.3's closed set, extended compatibly on the seam's side,
/// so a kind this build does not know is read as the most disruptive one: `Realign` restarts the
/// epoch, which is the only response that cannot report old audio at a new position.
fn analysis_kind(kind: DiscontinuityKind) -> SignalDiscontinuity {
    match kind {
        DiscontinuityKind::Loss => SignalDiscontinuity::Loss,
        DiscontinuityKind::Overflow => SignalDiscontinuity::Overflow,
        _ => SignalDiscontinuity::Realign,
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

    /// The two direction vocabularies name the same two sides, and the mapping is total.
    #[test]
    fn the_direction_mapping_is_total_and_order_preserving() {
        assert_eq!(
            seam_direction(SignalDirection::Inbound),
            AudioDirection::Inbound
        );
        assert_eq!(
            seam_direction(SignalDirection::Outbound),
            AudioDirection::Outbound
        );
    }

    /// A discontinuity kind this build does not know restarts the epoch rather than being
    /// dropped: the closed set is extended compatibly, so the wildcard has to mean something.
    #[test]
    fn an_unknown_discontinuity_is_read_as_the_most_disruptive_one() {
        assert_eq!(
            analysis_kind(DiscontinuityKind::Loss),
            SignalDiscontinuity::Loss
        );
        assert_eq!(
            analysis_kind(DiscontinuityKind::Overflow),
            SignalDiscontinuity::Overflow
        );
        assert_eq!(
            analysis_kind(DiscontinuityKind::Realign),
            SignalDiscontinuity::Realign
        );
    }
}
