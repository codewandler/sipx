//! `M-119`: carry one call's activity hint from its analyser to its reducer.
//!
//! `M-114` produced the hint and `M-67` opened the door a parameter moves through mid-call. This is
//! the join, and it exists so that supplying a hint is a property of a call rather than four
//! correct decisions an application makes per call — the fourth of which, wiring a detector to the
//! other direction's graph, produces silence rather than an error when it is made wrongly.
//!
//! **Nothing here runs on the media worker**, which is the constraint that decided the shape. The
//! frames arrive at the direction-aware PCM seam, which already hands them to the side that asked
//! for them; the analyser, the policy and [`DspGraph::configure`] all run on that side's task.
//! `configure` validates off the media path and applies under the take a frame already needs, so a
//! per-frame hint costs the worker nothing it was not already paying.

use sipx_audio::analysis::{AnalysisFrame, AudioAnalyzer, AudioDirection};
use sipx_audio::dsp::noise::ActivityHint;

use crate::dsp::DspGraph;
use crate::processing::PcmProcessor;

/// Why a wiring was refused before it carried anything.
///
/// Typed rather than a log line, because the failure this guards against is the one that looks like
/// success: an inbound detector driving an outbound graph produces a hint about audio the reducer
/// is not processing, and every part keeps reporting itself healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WiringError {
    /// The hint follows one direction and the graph another.
    DirectionMismatch {
        /// The direction the hint's observations describe.
        hint: AudioDirection,
        /// The direction the graph is prepared for.
        graph: AudioDirection,
    },
}

impl std::fmt::Display for WiringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectionMismatch { hint, graph } => write!(
                f,
                "an activity hint for {hint:?} audio cannot drive a graph prepared for {graph:?}"
            ),
        }
    }
}

impl std::error::Error for WiringError {}

/// One direction's analyser, joined to that direction's reducer.
///
/// Opt-in and per direction by construction: nothing is wired until one of these is built, and one
/// carries exactly one direction's frames to exactly one stage.
#[derive(Debug)]
pub struct ActivityWiring {
    frames: PcmProcessor,
    analyzer: AudioAnalyzer,
    hint: ActivityHint,
    graph: DspGraph,
    stage: usize,
}

impl ActivityWiring {
    /// Join a seam attachment, an analyser, a hint policy and a graph stage.
    ///
    /// # Errors
    ///
    /// [`WiringError::DirectionMismatch`] when the hint and the graph do not follow the same
    /// direction. Refused at construction rather than at the first frame: a wiring that cannot be
    /// right should not be a thing that exists.
    pub fn new(
        frames: PcmProcessor,
        analyzer: AudioAnalyzer,
        hint: ActivityHint,
        graph: DspGraph,
        stage: usize,
    ) -> Result<Self, WiringError> {
        if hint.direction() != graph.direction() {
            return Err(WiringError::DirectionMismatch {
                hint: hint.direction(),
                graph: graph.direction(),
            });
        }
        Ok(Self {
            frames,
            analyzer,
            hint,
            graph,
            stage,
        })
    }

    /// Carry every frame the seam already holds, and stop when it holds none.
    ///
    /// Finite by construction — it drains what is queued rather than awaiting more — so a caller
    /// decides how often the hint is driven and nothing here owns a task or a clock.
    #[must_use]
    pub fn run_until_idle(mut self) -> WiringOutcome {
        let mut applied = 0u64;
        let mut refused = 0u64;
        let mut samples = Vec::new();
        let mut boundaries = Vec::new();

        while let Some(frame) = self.frames.try_recv() {
            let rate = frame.format().sample_rate();
            let position = frame.sample_time();
            let sequence = frame.sequence();
            let direction = frame.direction();
            let Ok(carried) = frame.into_pcm().to_i16(rate) else {
                // The seam hands back the format it was asked for, so this cannot be a conversion
                // this call configured. Skip the frame rather than end the wiring: one unreadable
                // frame is not a reason to stop hinting for the rest of the call.
                continue;
            };

            // §10.2's order, and the position it names. Observations are fed at the position the
            // frame **ended**, not the one it began at: the hint's granularity is one frame and it
            // is set *between* frames, so what the next frame is processed with is decided entirely
            // by frames already processed. That is the only placement a reducer declaring zero
            // latency can honestly be given.
            let ended = position.saturating_add(carried.len() as u64);
            let analysed = AnalysisFrame::new(direction, sequence, &carried);
            if self.analyzer.process(&analysed).is_ok() {
                let mut changed = false;
                for observation in self.analyzer.drain() {
                    changed |= self.hint.observe(ended, &observation).is_some();
                }
                // Then advance to that position, which is what arms §10.2's warm-up deferral: an
                // opening transition inside the reducer's declared warm-up arms the hint without
                // setting it, and the set lands at the first boundary at or after the warm-up.
                changed |= self.hint.advance_to(ended).is_some();

                // A change, and only a change: the door is for *moving* a parameter, and re-sending
                // the value already in force would spend a generation's queue on saying nothing.
                if let Some(parameter) = changed.then(|| self.hint.parameter()).flatten() {
                    match self
                        .graph
                        .configure(self.graph.generation(), self.stage, &[parameter])
                    {
                        Ok(update) => {
                            applied += 1;
                            boundaries.push(update.at_position());
                        }
                        Err(_) => refused += 1,
                    }
                }
            }
            samples.extend_from_slice(&carried);
        }

        WiringOutcome {
            graph: self.graph,
            applied,
            refused,
            samples,
            boundaries,
        }
    }
}

/// What one pass of a wiring did.
///
/// Its [`Debug`] is hand-written and carries the pass's counts and its sample **count**, never its
/// samples (`M-61`, `M-107`, `M-110`). The derived one rendered the call's audio in a record whose
/// length was the pass's, and this type is printed exactly where that hurts most: a failing
/// assertion about a wiring holds a whole pass of audio at the moment it writes the record.
pub struct WiringOutcome {
    graph: DspGraph,
    applied: u64,
    refused: u64,
    samples: Vec<i16>,
    boundaries: Vec<u64>,
}

impl std::fmt::Debug for WiringOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WiringOutcome")
            .field("direction", &self.graph.direction())
            .field("generation", &self.graph.generation())
            .field("applied", &self.applied)
            .field("refused", &self.refused)
            .field("samples", &self.samples.len())
            .field("boundaries", &self.boundaries)
            .finish()
    }
}

impl WiringOutcome {
    /// Parameter updates the reducer accepted.
    #[must_use]
    pub const fn updates_applied(&self) -> u64 {
        self.applied
    }

    /// Parameter updates the graph refused.
    ///
    /// Counted rather than dropped, because a wiring that is quietly refused every frame is
    /// indistinguishable from one that never had anything to say.
    #[must_use]
    pub const fn updates_refused(&self) -> u64 {
        self.refused
    }

    /// The graph this pass was driving, returned so a caller can read its counters and drive again.
    #[must_use]
    pub const fn graph(&self) -> &DspGraph {
        &self.graph
    }

    /// The positions the applied updates landed at, in order.
    ///
    /// §10.2 says the hint is set *between* frames, so every one of these must be a frame boundary.
    /// Reported rather than described, because "it lands on a boundary" is a claim a caller can
    /// check and a comment is not.
    #[must_use]
    pub fn boundaries(&self) -> &[u64] {
        &self.boundaries
    }

    /// The audio this pass carried, in the order it arrived.
    #[must_use]
    pub fn into_samples(self) -> Vec<i16> {
        self.samples
    }
}
