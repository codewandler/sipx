//! The recognition contract (`docs/specs/speech-providers.md` §5).
//!
//! A recognition session consumes ordered inputs and emits ordered outputs. Every input is atomic
//! and outputs are applied in order; the session owns its own state and shares nothing with any
//! other session, including another session of the same provider on another call (`A-28`).
//!
//! **Every result carries the utterance's complete text, never a delta.** That is what makes a
//! coalesced or missed revision harmless: a consumer that sees only the newest revision is not
//! permanently wrong about the ones it skipped.
//!
//! **Audio arrives from the one seam.** [`recognition_inputs`] is the adapter: it turns a
//! [`PcmFrame`] from `M-54`'s bounded processing seam into the ordered inputs §5 requires, putting
//! the [`RecognitionInput::Discontinuity`] ahead of the frame that follows the gap. The seam's
//! drop-oldest queue at §8's input bound *is* this contract's input-bound obligation — there is no
//! second queue here and no second tap.

use std::fmt;

use sipx_audio::Pcm;

use super::lifecycle::{CancelReason, DeadlineKind, FailureCause, LossCause};
use super::privacy::{DataClass, Redacted};
use crate::processing::{AudioDirection, DiscontinuityKind, PcmFrame};

/// One utterance's identity within one session.
///
/// Strictly increasing per session, and never carried across sessions: a fallback successor starts
/// its own numbering (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtteranceId(u64);

impl UtteranceId {
    /// The first identity a session issues.
    pub const FIRST: Self = Self(0);

    /// An identity by number.
    #[must_use]
    pub const fn new(index: u64) -> Self {
        Self(index)
    }

    /// The number.
    #[must_use]
    pub const fn index(self) -> u64 {
        self.0
    }

    /// The next identity. Utterance `n + 1` cannot open before utterance `n` terminates.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// The span of audio a result covers, on the session's sample timeline (§5).
///
/// Derived from `Frame` sample times, never from a clock: RFC 3550 §5's media clock is the only
/// time in this contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleSpan {
    start: u64,
    samples: u64,
}

impl SampleSpan {
    /// A span starting at `start` and covering `samples` samples.
    #[must_use]
    pub const fn new(start: u64, samples: u64) -> Self {
        Self { start, samples }
    }

    /// The first sample covered.
    #[must_use]
    pub const fn start(self) -> u64 {
        self.start
    }

    /// How many samples are covered.
    #[must_use]
    pub const fn samples(self) -> u64 {
        self.samples
    }

    /// One past the last sample covered.
    #[must_use]
    pub const fn end(self) -> u64 {
        self.start.saturating_add(self.samples)
    }
}

/// One revision of one utterance (§5).
///
/// Its `Debug` redacts the transcript (§11.4): a revision is exactly the value a driver, a host or
/// a bug report is most likely to print, and printing it is how a transcript ends up somewhere it
/// was never retained on purpose.
#[derive(Clone, PartialEq, Eq)]
pub struct Utterance {
    id: UtteranceId,
    revision: u32,
    text: String,
    span: SampleSpan,
    discontinuities: u32,
}

impl Utterance {
    /// Open or revise an utterance.
    ///
    /// `revision` is 1 for the `Partial` that opens it and exactly one greater for each
    /// `Replacement`; `text` is the utterance's complete text, not a delta.
    #[must_use]
    pub fn new(id: UtteranceId, revision: u32, text: impl Into<String>, span: SampleSpan) -> Self {
        Self {
            id,
            revision,
            text: text.into(),
            span,
            discontinuities: 0,
        }
    }

    /// Record how many discontinuity spans fall inside the covered span (§5).
    #[must_use]
    pub const fn with_discontinuities(mut self, spans: u32) -> Self {
        self.discontinuities = spans;
        self
    }

    /// Which utterance this is.
    #[must_use]
    pub const fn id(&self) -> UtteranceId {
        self.id
    }

    /// Which revision of it. Strictly increasing by one.
    #[must_use]
    pub const fn revision(&self) -> u32 {
        self.revision
    }

    /// The utterance's complete text so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The audio this revision covers.
    #[must_use]
    pub const fn span(&self) -> SampleSpan {
        self.span
    }

    /// How many discontinuity spans fall inside that audio.
    #[must_use]
    pub const fn discontinuities(&self) -> u32 {
        self.discontinuities
    }
}

impl fmt::Debug for Utterance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Utterance")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field(
                "text",
                &Redacted::octets(DataClass::Transcript, self.text.len()),
            )
            .field("span", &self.span)
            .field("discontinuities", &self.discontinuities)
            .finish()
    }
}

/// One frame of call audio, as a recognition session receives it (§5 `Frame`).
///
/// The PCM boundary is `docs/specs/linear-pcm.md`'s and this contract adds no second audio
/// representation. The seam's break flag is deliberately *not* here: a discontinuity is its own
/// ordered input, so a gap is one fact in one place.
///
/// Its `Debug` redacts the samples (§11.4). Call audio is user data, and a frame's derived `Debug`
/// would put a hundred and sixty of them in one log line.
#[derive(Clone, PartialEq, Eq)]
pub struct RecognitionFrame {
    direction: AudioDirection,
    pcm: Pcm,
    sample_time: u64,
    sequence: u64,
}

impl RecognitionFrame {
    /// Build a frame in the session's operating format.
    #[must_use]
    pub const fn new(direction: AudioDirection, pcm: Pcm, sample_time: u64, sequence: u64) -> Self {
        Self {
            direction,
            pcm,
            sample_time,
            sequence,
        }
    }

    /// Which side of the call this audio is.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The owned samples.
    #[must_use]
    pub const fn pcm(&self) -> &Pcm {
        &self.pcm
    }

    /// The position of the first sample on the session's timeline.
    #[must_use]
    pub const fn sample_time(&self) -> u64 {
        self.sample_time
    }

    /// The seam's frame number for this attachment. A gap here is exactly what was lost.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Consume the frame and return its samples.
    #[must_use]
    pub fn into_pcm(self) -> Pcm {
        self.pcm
    }
}

impl fmt::Debug for RecognitionFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecognitionFrame")
            .field("direction", &self.direction)
            .field(
                "pcm",
                &Redacted::samples(DataClass::CallAudio, self.pcm.samples().len()),
            )
            .field("sample_time", &self.sample_time)
            .field("sequence", &self.sequence)
            .finish()
    }
}

impl From<PcmFrame> for RecognitionFrame {
    fn from(frame: PcmFrame) -> Self {
        Self {
            direction: frame.direction(),
            sample_time: frame.sample_time(),
            sequence: frame.sequence(),
            pcm: frame.into_pcm(),
        }
    }
}

/// What a driver delivers to a recognition session (§5).
///
/// `#[non_exhaustive]` for type-level compatibility. A driver must not send a variant to a provider
/// whose descriptor has not declared the corresponding capability; a provider receiving an input it
/// does not recognise fails the session with [`FailureCause::ProtocolViolation`] rather than
/// guessing (§9).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecognitionInput {
    /// Audio, in the session's operating format.
    Frame(RecognitionFrame),
    /// The timeline broke before the next frame.
    ///
    /// The vocabulary is the seam's, pinned normatively by `docs/specs/call-audio-processing.md`
    /// §3.3 — this contract shares it rather than minting a parallel one.
    Discontinuity {
        /// What caused the break.
        kind: DiscontinuityKind,
        /// How many frames the gap swallowed.
        frames: u64,
        /// The gap's span in samples, at the session's operating rate.
        samples: u64,
    },
    /// End of audio input. No `Frame` may follow.
    Flush,
    /// Cancel the session, with a typed reason.
    Cancel(CancelReason),
    /// A driver-fired deadline. One carrying a stale generation is ignored.
    DeadlineFired {
        /// Which deadline fired.
        kind: DeadlineKind,
        /// The generation it was armed in.
        generation: u64,
    },
}

/// What a recognition session emits (§5).
///
/// Extended compatibly, so a consumer writes a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RecognitionOutput {
    /// The session is warming. No result output may precede `Ready`.
    Warming,
    /// The session is ready.
    Ready,
    /// Opens utterance `u` at revision 1.
    Partial(Utterance),
    /// Revises the open utterance, replacing its entire prior text.
    Replacement(Utterance),
    /// Terminal for the utterance: its complete text and span.
    Final(Utterance),
    /// Terminal for the open utterance, with a typed reason.
    Cancelled {
        /// Which utterance ended.
        utterance: UtteranceId,
        /// Why.
        reason: CancelReason,
    },
    /// Terminal for the session, with a typed cause.
    Failed(FailureCause),
    /// The provider's engine or execution device became unavailable (§7).
    ///
    /// Open work has already been resolved terminally when this is emitted.
    Lost(LossCause),
    /// The session owns no task, queue, buffer or device allocation. Always the last output.
    Stopped {
        /// Whether the driver aborted the provider at the drain deadline rather than the provider
        /// stopping itself. An aborted stop is a reportable provider defect, not a hang.
        aborted: bool,
    },
}

/// One per-call, per-direction recognition session (§2, §5).
///
/// Sans-I/O by construction: a session is driven by delivering inputs and draining outputs, and
/// implementations open no socket, read no clock and hold no call handle. Their entire world is
/// this trait's inputs and outputs.
///
/// Implementable downstream — that is the point of the contract. Adding a required method here is
/// a breaking change; a new capability arrives as descriptor data plus a defaulted method (§9).
pub trait RecognitionSession {
    /// Deliver one input. Inputs are consumed in the order they are delivered.
    fn deliver(&mut self, input: RecognitionInput);

    /// Take the next output, oldest first, or `None` when none is pending.
    fn poll_output(&mut self) -> Option<RecognitionOutput>;
}

/// The ordered recognition inputs one seam frame carries (§5).
///
/// At most two, and always in this order: the break the frame is flagged with, then the frame.
/// §5 requires the driver to deliver the `Discontinuity` naming the accumulated lost span *before*
/// the next `Frame`, and the seam flags the frame that follows a gap — so putting them in that
/// order here is the whole of the adaptation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameInputs {
    /// Emitted first when the seam flagged a break before this frame.
    discontinuity: Option<RecognitionInput>,
    frame: Option<RecognitionInput>,
}

impl Iterator for FrameInputs {
    type Item = RecognitionInput;

    fn next(&mut self) -> Option<Self::Item> {
        self.discontinuity.take().or_else(|| self.frame.take())
    }
}

impl std::iter::FusedIterator for FrameInputs {}

/// Adapt one frame from `M-54`'s bounded processing seam into recognition inputs.
///
/// This is the contract's only consumption of call audio. There is no second tap
/// (`docs/specs/call-audio-seam.md` §1, `docs/specs/speech-providers.md` §1), and no queue is
/// created here: the seam already bounds the attachment at §8's input-frame default and names its
/// own loss as [`DiscontinuityKind::Overflow`].
#[must_use]
pub fn recognition_inputs(frame: PcmFrame) -> FrameInputs {
    let discontinuity = frame
        .discontinuity()
        .map(|gap| RecognitionInput::Discontinuity {
            kind: gap.kind(),
            frames: gap.frames(),
            samples: gap.samples(),
        });
    FrameInputs {
        discontinuity,
        frame: Some(RecognitionInput::Frame(frame.into())),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// §5: identities are strictly increasing and a span never runs backwards.
    #[test]
    fn identities_increase_and_spans_add_up() {
        assert_eq!(UtteranceId::FIRST.next(), UtteranceId::new(1));
        assert!(UtteranceId::new(1) > UtteranceId::FIRST);
        let span = SampleSpan::new(320, 160);
        assert_eq!(span.end(), 480);
    }

    /// §5: every result event carries the utterance's complete text, so a revision is a
    /// replacement rather than an append.
    #[test]
    fn a_revision_replaces_the_whole_text() {
        let opened = Utterance::new(UtteranceId::FIRST, 1, "", SampleSpan::new(0, 160));
        let revised = Utterance::new(UtteranceId::FIRST, 2, "", SampleSpan::new(0, 320))
            .with_discontinuities(1);
        assert_eq!(revised.revision(), opened.revision() + 1);
        assert_eq!(revised.id(), opened.id());
        assert_eq!(revised.discontinuities(), 1);
        assert_eq!(revised.text(), opened.text());
    }
}
