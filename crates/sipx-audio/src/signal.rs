//! Level, energy, clipping and silence reporting, shaped out of [`analysis`](crate::analysis)'s
//! per-window facts (`M-59`).
//!
//! [`docs/specs/call-audio-processing.md`](../../../../docs/specs/call-audio-processing.md) §10
//! assigns this module exactly one job: *"`M-59` shapes level/clipping/silence reporting from
//! `Window` facts. Neither may add an observation source outside this contract."* So this module
//! computes **nothing** from samples. It owns no accumulator over audio, no window, no threshold
//! and no predicate. It is a pure reducer over the observations
//! [`AudioAnalyzer`](crate::analysis::AudioAnalyzer) already produced, and every number it reports
//! is either one of that analyser's own accumulators or an exactly-stated integer function of
//! them.
//!
//! That is deliberate and it is the whole design. A second window engine would be a second place
//! for `docs/specs/call-audio-processing.md` §5.3's arithmetic to live, and two implementations of
//! one predicate is how they start disagreeing about what a call sounded like.
//!
//! # What it adds
//!
//! **A bounded reporting cadence.** A 20 ms window at 8,000 Hz completes fifty times a second, and
//! an application that wanted a level meter does not want fifty events a second to build one. A
//! [`SignalReporter`] folds [`SignalReportProfile::windows_per_report`] consecutive windows into one
//! [`SignalReport`] in fixed-size state, so a coarser cadence costs no memory, loses no samples and
//! changes no fact — only resolution.
//!
//! **Coverage that cannot lie.** Every report names the epoch, rate, first window, window count,
//! first sample and sample count it was measured over. A period is discarded unreported if
//! anything could have holed it: a reset, a window index that is not the next one, or the
//! analyser's own [`Observation::Lost`] marker — which means window facts were coalesced away, and
//! a report summing across that gap would claim coverage it does not have. Better no report than a
//! wrong one, and the `Lost` marker that caused it is passed on so the absence is visible.
//!
//! # What it is not
//!
//! Not [`sipx_media::MediaSession::quality`]'s RTP/RTCP snapshot. Loss, jitter, round-trip time and
//! the MOS estimate are `M-10`'s surface and describe *delivery*; these describe *content*. A call
//! with no packet loss at all can be clipping, and a call losing a quarter of its packets can carry
//! a clean level in the audio that arrived. No field of either changes meaning because the other
//! exists.
//!
//! Not an ITU-T P.56 active speech level. [`SignalReport::rms`] is a plain quadratic mean of the
//! coverage with no speech gating and no gain assumption, and no door of this stack may describe it
//! as a P.56 measurement.
//!
//! Not voice activity. [`SignalReport::active_windows`] counts §5.3's per-window `active` fact
//! because it is one of the five facts measured over the same accumulators; the start, end and
//! hangover *transitions* derived from it are `M-58`'s
//! ([`Observation::VoiceStarted`](crate::analysis::Observation::VoiceStarted)) and this module
//! derives none of them.
//!
//! # Example
//!
//! ```
//! use sipx_audio::analysis::{AnalysisFrame, AnalysisProfile, AudioAnalyzer, AudioDirection};
//! use sipx_audio::signal::{SignalObservation, SignalReportProfile, SignalReporter};
//!
//! let profile = AnalysisProfile::new(AudioDirection::Inbound, 8_000);
//! let mut analyzer = AudioAnalyzer::new(profile)?;
//! let mut reporter = SignalReporter::new(SignalReportProfile::new(profile))?;
//!
//! // One 20 ms window of a stuck full-scale capture.
//! analyzer.process(&AnalysisFrame::new(AudioDirection::Inbound, 0, &[32_767i16; 160]))?;
//!
//! let window = analyzer.window_samples();
//! for observation in analyzer.drain() {
//!     if let Some(SignalObservation::Report(report)) = reporter.observe(&observation, window) {
//!         assert_eq!(report.peak, 32_767);
//!         assert_eq!(report.clipping_windows, 1);
//!         assert_eq!(report.samples, 160);
//!     }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use crate::analysis::{AnalysisError, AnalysisProfile, AudioAnalyzer, Observation, ResetCause};

/// The most windows one report may coalesce.
///
/// The same 4,096 the processing contract's §5.1 bounds an observation queue at, and for the same
/// kind of reason: it is what keeps every accumulated quantity provably inside `i64`. A window's
/// energy is at most `2^46` (§5.2), so a period's is at most `2^46 · 2^12 = 2^58`.
pub const MAX_WINDOWS_PER_REPORT: u32 = 4_096;

/// What to measure, and how often to say so.
///
/// The measuring half is [`AnalysisProfile`] unchanged — the same thresholds, window and rate the
/// analyser is configured with, because a reporter reading a *different* profile than the analyser
/// it summarises would describe windows nobody measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalReportProfile {
    analysis: AnalysisProfile,
    windows_per_report: u32,
}

impl SignalReportProfile {
    /// Report every completed window separately.
    ///
    /// The finest cadence there is, and the one the specification's §11.2 vectors are written
    /// against: with one window per report a report *is* a window's facts.
    #[must_use]
    pub const fn new(analysis: AnalysisProfile) -> Self {
        Self {
            analysis,
            windows_per_report: 1,
        }
    }

    /// Coalesce `windows` consecutive completed windows into one report
    /// (1..=[`MAX_WINDOWS_PER_REPORT`]).
    #[must_use]
    pub const fn with_windows_per_report(mut self, windows: u32) -> Self {
        self.windows_per_report = windows;
        self
    }

    /// The analysis profile being summarised.
    #[must_use]
    pub const fn analysis(self) -> AnalysisProfile {
        self.analysis
    }

    /// How many windows one report covers.
    #[must_use]
    pub const fn windows_per_report(self) -> u32 {
        self.windows_per_report
    }
}

/// Why a reporting profile was refused.
///
/// Returned before anything is built, so a refused profile costs a call nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SignalProfileError {
    /// The analysis half is outside a domain the processing contract's §5.1 declares.
    #[error(transparent)]
    Analysis(#[from] AnalysisError),
    /// `windows_per_report` is outside 1..=[`MAX_WINDOWS_PER_REPORT`].
    ///
    /// Zero would be a report over no windows; beyond the bound the accumulated energy would leave
    /// the width this module's arithmetic is proven inside.
    #[error("windows_per_report {requested} is outside 1..={MAX_WINDOWS_PER_REPORT}")]
    WindowsPerReport {
        /// What was asked for.
        requested: u32,
    },
}

/// Signal facts over a run of consecutive completed windows.
///
/// Every field is exact integer arithmetic over the windows the run covered: no smoothing, no
/// gain, no estimate, and nothing derived from a clock. [`Self::peak`], [`Self::rms`] and the
/// thresholds behind the window counts are all in signed 16-bit sample-amplitude units, where
/// 32,768 is the magnitude of the most negative representable sample.
///
/// [`Self::epoch`] and [`Self::rate`] are what make a report unmistakably about one measurement run
/// at one format: the epoch advances on every reset the analyser reports, and a report is only ever
/// built from windows completed inside one epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SignalReport {
    /// The measurement run, counting from 0 and advanced by every reset the analyser reported.
    pub epoch: u64,
    /// The sample rate the coverage below is counted in.
    pub rate: u32,
    /// This report's number within its epoch, from 0. Restarts with the epoch.
    pub sequence: u64,
    /// The index of the first window covered, within the epoch.
    pub first_window: u64,
    /// How many consecutive windows are covered — always the profile's `windows_per_report`.
    pub windows: u32,
    /// The position of the first sample covered, within the epoch.
    pub at_sample: u64,
    /// How many samples are covered: `windows · W`.
    pub samples: u64,
    /// The largest sample magnitude over the coverage, 0..=32,768.
    pub peak: i32,
    /// `Σ s` over the coverage — its DC content, signed.
    pub sum: i64,
    /// `Σ s²` over the coverage.
    pub energy: i64,
    /// `floor(sqrt(floor(energy / samples)))`, in sample-amplitude units.
    ///
    /// Stated as exact integer arithmetic rather than a floating-point root so that the same
    /// samples give the same level on every platform. A plain quadratic mean of the coverage, and
    /// deliberately **not** an ITU-T P.56 active speech level.
    pub rms: u32,
    /// How many samples sat at full scale, either sign.
    pub clipped_samples: u64,
    /// How many covered windows reached the profile's `clip_samples` count.
    pub clipping_windows: u32,
    /// How many covered windows put more than half their energy in one sample: clicks, not signal.
    pub impulsive_windows: u32,
    /// How many covered windows met the activation threshold on DC-free variance.
    ///
    /// A count of §5.3's per-window fact. The voice-activity *transitions* derived from it are
    /// `M-58`'s and are not derived here.
    pub active_windows: u32,
    /// How many covered windows carried a mean magnitude at or above the DC threshold.
    pub dc_offset_windows: u32,
    /// How many covered windows stayed entirely below the silence floor.
    pub silent_windows: u32,
}

/// One signal-metric fact, shaped from what the analyser observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SignalObservation {
    /// A completed reporting period.
    Report(SignalReport),
    /// Unbroken silence first reached the analyser's configured timeout.
    ///
    /// Reported once per run of silent windows; the analyser re-arms it after a non-silent window
    /// or a reset.
    SilenceElapsed {
        /// The position of the run's first sample, within the epoch.
        at_sample: u64,
        /// The rate [`Self::SilenceElapsed::at_sample`] is counted at.
        rate: u32,
        /// The epoch the run belongs to.
        epoch: u64,
    },
    /// Measurement restarted. The epoch named is the one this reset opened.
    ///
    /// Any partially accumulated reporting period is discarded unreported: a period cut short
    /// covers fewer windows than it claims, and the windows before a reset belong to a different
    /// epoch — a different call position, and after a format change a different rate.
    Reset {
        /// Why, in the analyser's own vocabulary.
        cause: ResetCause,
        /// The epoch now in force.
        epoch: u64,
    },
    /// The analyser's bounded queue coalesced observations away, and this many were lost.
    ///
    /// Passed on rather than swallowed, because the lost observations may have been window facts:
    /// the period in progress is discarded with it, so no report ever sums across the hole.
    Lost {
        /// How many observations the marker stands for.
        count: u64,
    },
}

/// What one reporting period has accumulated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Period {
    windows: u32,
    first_window: u64,
    at_sample: u64,
    /// The index the next window must carry for the period to stay contiguous.
    next_window: u64,
    peak: i32,
    sum: i64,
    energy: i64,
    clipped: u64,
    clipping: u32,
    impulsive: u32,
    active: u32,
    dc_offset: u32,
    silent: u32,
}

impl Period {
    const EMPTY: Self = Self {
        windows: 0,
        first_window: 0,
        at_sample: 0,
        next_window: 0,
        peak: 0,
        sum: 0,
        energy: 0,
        clipped: 0,
        clipping: 0,
        impulsive: 0,
        active: 0,
        dc_offset: 0,
        silent: 0,
    };
}

/// Folds an analyser's observations into bounded signal reports.
///
/// Pure and sans-I/O, like the analyser it summarises: no clock, no task, no allocation after
/// construction, and its state is a constant of its profile. Feed it every observation the analyser
/// drains, in order, and it yields at most one [`SignalObservation`] per input.
#[derive(Debug)]
pub struct SignalReporter {
    profile: SignalReportProfile,
    period: Period,
    epoch: u64,
    rate: u32,
    sequence: u64,
}

impl SignalReporter {
    /// Validate a profile and build the reporter it describes.
    ///
    /// Both halves are checked: the analysis profile against the processing contract's §5.1
    /// domains, and the cadence against [`MAX_WINDOWS_PER_REPORT`].
    ///
    /// # Errors
    ///
    /// [`SignalProfileError`], naming which half was refused.
    pub fn new(profile: SignalReportProfile) -> Result<Self, SignalProfileError> {
        // Built and dropped: the analyser is the authority on its own domains, and re-checking
        // them here would be a second copy of §5.1 to keep in step with the first.
        drop(AudioAnalyzer::new(profile.analysis)?);
        if !(1..=MAX_WINDOWS_PER_REPORT).contains(&profile.windows_per_report) {
            return Err(SignalProfileError::WindowsPerReport {
                requested: profile.windows_per_report,
            });
        }
        Ok(Self {
            profile,
            period: Period::EMPTY,
            epoch: 0,
            rate: profile.analysis.rate(),
            sequence: 0,
        })
    }

    /// The profile in force.
    #[must_use]
    pub const fn profile(&self) -> SignalReportProfile {
        self.profile
    }

    /// The measurement run in force, counting from 0 and advanced by every reset observed.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The rate reports are currently measured at, which a format-change reset replaces.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }

    /// Fold one observation, yielding whatever it completed.
    ///
    /// `window_samples` is the analyser's derived window length
    /// ([`AudioAnalyzer::window_samples`](crate::analysis::AudioAnalyzer::window_samples)), passed
    /// in rather than re-derived here. Two derivations of one number is how they start disagreeing,
    /// and this module deliberately owns no copy of §4's conversion.
    ///
    /// Voice transitions are ignored: they are `M-58`'s reading of the same windows, not a signal
    /// metric.
    pub fn observe(
        &mut self,
        observation: &Observation,
        window_samples: u32,
    ) -> Option<SignalObservation> {
        match observation {
            Observation::Window {
                index,
                peak,
                sum,
                energy,
                clipped,
                clipping,
                impulsive,
                active,
                dc_offset,
                silent,
            } => {
                // A period must be contiguous to be able to name its coverage. Anything that could
                // have holed it starts a fresh one instead of quietly summing across the gap.
                if self.period.windows > 0 && *index != self.period.next_window {
                    self.period = Period::EMPTY;
                }
                if self.period.windows == 0 {
                    self.period.first_window = *index;
                    self.period.at_sample = *index * u64::from(window_samples);
                }
                self.period.next_window = *index + 1;
                self.period.windows += 1;
                if *peak > self.period.peak {
                    self.period.peak = *peak;
                }
                self.period.sum += *sum;
                self.period.energy += *energy;
                self.period.clipped += u64::from(*clipped);
                self.period.clipping += u32::from(*clipping);
                self.period.impulsive += u32::from(*impulsive);
                self.period.active += u32::from(*active);
                self.period.dc_offset += u32::from(*dc_offset);
                self.period.silent += u32::from(*silent);

                (self.period.windows >= self.profile.windows_per_report)
                    .then(|| SignalObservation::Report(self.close(window_samples)))
            }
            Observation::SilenceElapsed { at_sample } => Some(SignalObservation::SilenceElapsed {
                at_sample: *at_sample,
                rate: self.rate,
                epoch: self.epoch,
            }),
            Observation::Reset { cause } => {
                self.epoch += 1;
                if let ResetCause::FormatChange { rate } = cause {
                    self.rate = *rate;
                }
                self.period = Period::EMPTY;
                self.sequence = 0;
                Some(SignalObservation::Reset {
                    cause: *cause,
                    epoch: self.epoch,
                })
            }
            // Window facts may be among what the queue coalesced away, so the period in progress
            // can no longer name its own coverage and is dropped with the marker.
            Observation::Lost { count } => {
                self.period = Period::EMPTY;
                Some(SignalObservation::Lost { count: *count })
            }
            // Voice transitions, and anything a later revision of the closed observation set adds:
            // not a signal metric, and not this module's to reinterpret.
            _ => None,
        }
    }

    /// Build the report for a completed period and clear it.
    ///
    /// Width: a window's energy is at most `2^46` and a period covers at most `2^12` windows, so
    /// the accumulated energy is at most `2^58`; `|sum|` is at most `2^31 · 2^12 = 2^43`. Both are
    /// far inside `i64`, which is what makes the plain arithmetic above unable to overflow.
    fn close(&mut self, window_samples: u32) -> SignalReport {
        let period = std::mem::replace(&mut self.period, Period::EMPTY);
        debug_assert!(period.energy >= 0, "a sum of squares is never negative");

        let samples = u64::from(period.windows) * u64::from(window_samples);
        // `samples` is at least one window, and `energy` is a sum of squares.
        let mean_square = u64::try_from(period.energy).unwrap_or(0) / samples.max(1);
        let rms = u32::try_from(mean_square.isqrt()).unwrap_or(u32::MAX);

        let report = SignalReport {
            epoch: self.epoch,
            rate: self.rate,
            sequence: self.sequence,
            first_window: period.first_window,
            windows: period.windows,
            at_sample: period.at_sample,
            samples,
            peak: period.peak,
            sum: period.sum,
            energy: period.energy,
            rms,
            clipped_samples: period.clipped,
            clipping_windows: period.clipping,
            impulsive_windows: period.impulsive,
            active_windows: period.active,
            dc_offset_windows: period.dc_offset,
            silent_windows: period.silent,
        };
        self.sequence += 1;
        report
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

    use crate::analysis::{AudioDirection, DiscontinuityKind};

    const W: u32 = 160;

    fn reporter(windows_per_report: u32) -> SignalReporter {
        SignalReporter::new(
            SignalReportProfile::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000))
                .with_windows_per_report(windows_per_report),
        )
        .unwrap()
    }

    /// A window observation carrying nothing but its index, for the bookkeeping tests.
    const fn window(index: u64) -> Observation {
        Observation::Window {
            index,
            peak: 0,
            sum: 0,
            energy: 0,
            clipped: 0,
            clipping: false,
            impulsive: false,
            active: false,
            dc_offset: false,
            silent: true,
        }
    }

    /// The cadence is a bound on events, and a partial period is never reported early.
    #[test]
    fn a_period_completes_only_when_its_windows_are_all_in() {
        let mut reporter = reporter(4);
        for index in 0..3 {
            assert!(reporter.observe(&window(index), W).is_none());
        }
        let Some(SignalObservation::Report(report)) = reporter.observe(&window(3), W) else {
            panic!("the fourth window closes the period");
        };
        assert_eq!(report.windows, 4);
        assert_eq!(report.first_window, 0);
        assert_eq!(report.at_sample, 0);
        assert_eq!(report.samples, 640);
        assert_eq!(report.sequence, 0);
    }

    /// A hole in the window indices restarts the period rather than being summed across.
    #[test]
    fn a_non_contiguous_window_starts_a_fresh_period() {
        let mut reporter = reporter(2);
        assert!(reporter.observe(&window(0), W).is_none());

        // Window 1 never arrived. Window 2 must not be folded in beside window 0 and reported as
        // "windows 0..=1", which is coverage nobody measured.
        assert!(reporter.observe(&window(2), W).is_none());
        let Some(SignalObservation::Report(report)) = reporter.observe(&window(3), W) else {
            panic!("the fresh period closes on window 3");
        };
        assert_eq!(report.first_window, 2);
        assert_eq!(report.at_sample, 320);
    }

    /// The analyser's loss marker discards the period in progress and is passed on.
    #[test]
    fn a_lost_marker_discards_the_period_and_is_reported() {
        let mut reporter = reporter(2);
        assert!(reporter.observe(&window(0), W).is_none());

        assert_eq!(
            reporter.observe(&Observation::Lost { count: 3 }, W),
            Some(SignalObservation::Lost { count: 3 })
        );

        // Window 1 alone must not now close a two-window period that lost one of them.
        assert!(reporter.observe(&window(1), W).is_none());
    }

    /// A reset opens an epoch, restarts the report sequence, and a format change carries its rate.
    #[test]
    fn a_reset_opens_an_epoch_and_a_format_change_carries_its_rate() {
        let mut reporter = reporter(1);
        assert!(matches!(
            reporter.observe(&window(0), W),
            Some(SignalObservation::Report(SignalReport {
                epoch: 0,
                sequence: 0,
                rate: 8_000,
                ..
            }))
        ));

        assert_eq!(
            reporter.observe(
                &Observation::Reset {
                    cause: ResetCause::FormatChange { rate: 16_000 }
                },
                W
            ),
            Some(SignalObservation::Reset {
                cause: ResetCause::FormatChange { rate: 16_000 },
                epoch: 1,
            })
        );

        let Some(SignalObservation::Report(report)) = reporter.observe(&window(0), 320) else {
            panic!("the new epoch reports at its own rate");
        };
        assert_eq!(report.epoch, 1);
        assert_eq!(report.rate, 16_000);
        assert_eq!(report.sequence, 0, "the sequence restarts with the epoch");
        assert_eq!(report.samples, 320);
    }

    /// A discontinuity reset is passed through in the analyser's own vocabulary.
    #[test]
    fn a_discontinuity_reset_keeps_the_analysers_vocabulary() {
        let mut reporter = reporter(1);
        assert_eq!(
            reporter.observe(
                &Observation::Reset {
                    cause: ResetCause::Discontinuity {
                        kind: DiscontinuityKind::Overflow
                    }
                },
                W
            ),
            Some(SignalObservation::Reset {
                cause: ResetCause::Discontinuity {
                    kind: DiscontinuityKind::Overflow
                },
                epoch: 1,
            })
        );
    }

    /// Voice transitions belong to `M-58` and produce no signal observation here.
    #[test]
    fn voice_transitions_are_not_signal_metrics() {
        let mut reporter = reporter(1);
        assert!(
            reporter
                .observe(&Observation::VoiceStarted { at_sample: 0 }, W)
                .is_none()
        );
    }

    /// The cadence domain is refused at both ends, before anything is built.
    #[test]
    fn the_cadence_domain_is_refused_at_both_ends() {
        let analysis = AnalysisProfile::new(AudioDirection::Inbound, 8_000);
        assert!(matches!(
            SignalReporter::new(SignalReportProfile::new(analysis).with_windows_per_report(0)),
            Err(SignalProfileError::WindowsPerReport { requested: 0 })
        ));
        assert!(matches!(
            SignalReporter::new(
                SignalReportProfile::new(analysis)
                    .with_windows_per_report(MAX_WINDOWS_PER_REPORT + 1)
            ),
            Err(SignalProfileError::WindowsPerReport { .. })
        ));
    }

    /// An analysis profile the analyser refuses is refused here, in its own words.
    #[test]
    fn an_impossible_analysis_profile_is_refused_by_the_analyser_that_owns_the_domain() {
        let analysis = AnalysisProfile::new(AudioDirection::Inbound, 0);
        assert!(matches!(
            SignalReporter::new(SignalReportProfile::new(analysis)),
            Err(SignalProfileError::Analysis(_))
        ));
    }
}
