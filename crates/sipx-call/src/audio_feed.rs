//! One analyser, fed from the call PCM seam (`M-77`).
//!
//! [`crate::voice`] and [`crate::signal_metrics`] are two readings of the same windows. Each
//! attaches to `sipx-media`'s one seam
//! ([`docs/specs/call-audio-seam.md`](../../../docs/specs/call-audio-seam.md)), feeds a
//! [`AudioAnalyzer`] and shapes what it observes into call events; everything *before* that shaping
//! is the same work in both. This module is that part, held once — because it was held twice, and
//! the two copies disagreed about the case below for a release.
//!
//! # A break is owed, not dropped
//!
//! Audio that never reaches the analyser leaves a hole in the stream the analyser measured, and the
//! analyser cannot see the hole: nothing was fed, so its epoch simply continues across it. Both
//! readings are then wrong, each in its own way. `M-59`'s reports sum across the gap and name
//! coverage nobody measured. `M-58`'s voice state stays *latched* across it, so a transition that
//! happened inside the unmeasured span is reported as nothing at all.
//!
//! So a frame that does not reach the analyser sets a [`DiscontinuityKind::Loss`] **owed** to the
//! next one, which carries it as a declared discontinuity. The analyser restarts its epoch there,
//! which cuts open voice and opens a new measurement run — the honest answer about audio nobody
//! saw, and the one thing that cannot be said by dropping the frame quietly.
//!
//! Nothing here reads a clock or awaits anything: it is the analyser's own sans-I/O contract, so
//! both joins' policies are exercised by feeding them frames.

use sipx_audio::PcmSamples;
use sipx_audio::analysis::{
    AnalysisFrame, AudioAnalyzer, AudioDirection, DiscontinuityKind, Observation,
};
use sipx_media::PcmFrame;

/// One call attachment's analyser, plus the bookkeeping that keeps the fed stream honest.
#[derive(Debug)]
pub(crate) struct AudioFeed {
    analyzer: AudioAnalyzer,
    /// The frame number handed to the analyser, which counts only frames it accepted. The seam's
    /// own sequence is checked against §4 of the seam contract below and then left behind: a frame
    /// that never reaches the analyser must not derange its sequence expectation.
    fed: u64,
    /// The last seam sequence seen, for detecting a gap the seam failed to flag.
    seam_sequence: Option<u64>,
    /// A break owed to the analyser before the next frame's samples are measured.
    ///
    /// Set when audio that should have been measured was not — a frame this layer could not
    /// forward, or one the analyser refused. See the module docs for what carrying it forward buys
    /// each of the two readings.
    owed: Option<DiscontinuityKind>,
}

impl AudioFeed {
    pub(crate) fn new(analyzer: AudioAnalyzer) -> Self {
        Self {
            analyzer,
            fed: 0,
            seam_sequence: None,
            owed: None,
        }
    }

    /// Offer one frame the seam delivered; `true` when the analyser accepted it and there is
    /// something new to read.
    pub(crate) fn offer(&mut self, frame: &PcmFrame) -> bool {
        let PcmSamples::Signed16(samples) = frame.pcm().samples() else {
            // discard: unreachable through `Call`, which always attaches at signed 16-bit — the
            // depth the analyser's contract is written in. Converting here instead would make this
            // a second place where call audio is reinterpreted. The audio is still owed to the
            // analyser, so the next frame carries the break rather than either join claiming to
            // have measured samples that never reached it.
            self.owe_break();
            return false;
        };
        self.offer_samples(
            frame.direction(),
            frame.sequence(),
            frame.discontinuity().map(sipx_media::Discontinuity::kind),
            samples,
        )
    }

    /// The body of [`Self::offer`], reachable without a live media session.
    pub(crate) fn offer_samples(
        &mut self,
        direction: AudioDirection,
        seam_sequence: u64,
        discontinuity: Option<DiscontinuityKind>,
        samples: &[i16],
    ) -> bool {
        if samples.is_empty() {
            // discard: an empty frame measures nothing and the analyser refuses one by contract
            // (§7.3). Nothing is owed, because nothing was lost.
            return false;
        }

        // The seam counts every frame it offered, delivered or dropped, and always flags a gap
        // (seam §4). An unflagged gap is a defect in the seam rather than a loss to smooth over, so
        // it is named `Loss` here instead of being handed to the analyser as a legal stream.
        // A break owed from a frame that never reached the analyser is named before anything the
        // seam declared, and the more disruptive of the two wins — the seam's vocabulary is closed
        // and `Realign` subsumes a span (its §7).
        let mut discontinuity = match (discontinuity, self.owed.take()) {
            (Some(declared), Some(DiscontinuityKind::Loss)) => Some(declared),
            (declared, owed) => declared.or(owed),
        };
        let unflagged_gap = discontinuity.is_none()
            && self
                .seam_sequence
                .is_some_and(|previous| seam_sequence > previous.saturating_add(1));
        if unflagged_gap {
            tracing::debug!(
                seam_sequence,
                "the call PCM seam skipped a frame without flagging it"
            );
            discontinuity = Some(DiscontinuityKind::Loss);
        }
        self.seam_sequence = Some(seam_sequence);

        let mut frame = AnalysisFrame::new(direction, self.fed, samples);
        if let Some(kind) = discontinuity {
            frame = frame.with_discontinuity(kind);
        }
        if let Err(refusal) = self.analyzer.process(&frame) {
            // discard: these samples are gone — a refusal mutates nothing, so there is no second
            // way to present them — and the seam's contract makes every refusal here a defect in
            // this join rather than in the caller's audio, which is why it is a debug record and
            // not a counter. What is *not* discarded is the break they left: the next frame carries
            // it, so the epoch restarts rather than a report summing across the hole or voice
            // staying latched over it.
            tracing::debug!(%refusal, "the call audio analyser refused a seam frame");
            self.owe_break();
            return false;
        }
        self.fed = self.fed.saturating_add(1);
        true
    }

    /// Note audio that should have been measured and was not.
    fn owe_break(&mut self) {
        self.owed = Some(DiscontinuityKind::Loss);
    }

    /// Read everything the analyser has enqueued.
    pub(crate) fn drain(&mut self) -> impl Iterator<Item = Observation> + '_ {
        self.analyzer.drain()
    }

    /// Restart measurement, which cuts open voice and opens a new epoch (§7.1).
    pub(crate) fn reset(&mut self) {
        self.analyzer.reset();
    }

    /// Which side of the call this analyser measures.
    pub(crate) const fn direction(&self) -> AudioDirection {
        self.analyzer.direction()
    }

    /// The rate every sample position is counted at.
    pub(crate) const fn sample_rate(&self) -> u32 {
        self.analyzer.sample_rate()
    }

    /// How many samples one window covers.
    pub(crate) const fn window_samples(&self) -> u32 {
        self.analyzer.window_samples()
    }
}
