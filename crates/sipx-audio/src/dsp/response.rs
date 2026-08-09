//! The packaged magnitude-response sweep: `docs/specs/call-dsp-effects.md` §10 (`X-109`).
//!
//! [`crate::dsp::conformance`] answers *does this processor keep its declaration*. This module
//! answers a different question the same way — with a fixed corpus, an exact integer figure and a
//! typed refusal where it cannot answer at all: **what does this processor do to a tone**.
//!
//! It exists because `call-dsp-effects.md` §8.4 had to state a negative it could not size. A
//! peaking filter's `band_gain` "is not the magnitude response at the centre frequency", and until
//! something swept the filter, how far below the number the real peak sits was nobody's figure.
//!
//! # What it promises
//!
//! For each bin of a fixed probe corpus, the ratio of the processor's settled output energy to the
//! probe's own energy, as `round(1000·√(Σy²/Σx²))` in thousandths. Everything in that sentence is
//! integer arithmetic over a shipped quarter-wave table, so **a figure is the same on every
//! machine, at any load, in debug and in release**. Nothing here reads a clock, and a response
//! recorded on a busy box is worth exactly as much as one recorded on an idle one.
//!
//! The measured block is one whole set of the probe's phases, which is what makes the figure a
//! magnitude: `Σ sin²(2πkn/N + φ) = N/2` for every phase `φ` when the block spans a whole number
//! of cycles, so a processor that shifts the tone in time does not change its measured energy.
//!
//! # What it does not promise
//!
//! - **Not phase, not group delay, not linearity.** The figure is an energy ratio. Feed it a
//!   clipper and it will return a number, and that number is not a transfer magnitude — a
//!   nonlinear processor moves energy to frequencies the probe never asked about and the ratio
//!   counts all of it. Interpreting a sweep as a transfer function is only sound for a linear
//!   time-invariant processor, and this module cannot tell whether it was handed one.
//! - **Not a resolution finer than `rate/256`**, and not a frequency at or above `rate·112/256`.
//!   The corpus is the corpus; [`ResponseSweep::with_bins`] moves within it and cannot go outside.
//! - **Not anything about a processor that delays, retains or resizes.** A declared latency, a
//!   declared tail or a length policy other than
//!   [`LengthPolicy::Preserving`] is refused by name
//!   ([`ResponseError`]) rather than measured against a block that no longer lines up with the one
//!   that produced it.
//! - **Not a decibel.** Thousandths of unity, because a logarithm is exactly the transcendental
//!   `docs/specs/custom-call-dsp.md` §4.6 keeps out of this crate, and a reader who wants dB has
//!   the ratio to take it from.
//! - **Not a cost.** CPU is measured by `crates/sipx-audio/examples/dsp_cost.rs`, on a clock, with
//!   the machine's load recorded beside every figure, because that measurement has none of the
//!   properties this one has.
//!
//! Like the conformance harness this is a test tool and not real-time code: it allocates freely,
//! which is exactly what the processors it measures may not do.
//!
//! ```
//! use sipx_audio::dsp::effects::LowPass;
//! use sipx_audio::dsp::response::ResponseSweep;
//! use sipx_audio::dsp::StreamFormat;
//!
//! let report = ResponseSweep::new(StreamFormat::new(8_000, 1)?).run(LowPass::new)?;
//! // A one-pole low-pass at its 3,400 Hz default passes the bottom of the band and rolls off.
//! let magnitudes: Vec<u32> = report.magnitudes().collect();
//! assert_eq!(magnitudes.first(), Some(&1_000));
//! assert!(magnitudes.last() < magnitudes.first());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use crate::analysis::AudioDirection;

use super::contract::{
    DspFrame, FormatError, FrameProcessor, FrameSink, LengthPolicy, ProcessError, Scratch,
    StreamFormat,
};
use super::effects::{narrow, scaled};

/// Positions in one probe block, and the period of the shipped tone table.
///
/// A bin is a whole number of cycles per block, so the block is also the measurement window: it
/// spans every phase the probe visits exactly once, which is what makes the energy ratio a
/// magnitude rather than a number that depends on where the filter put the waveform.
pub const RESPONSE_BLOCK_POSITIONS: u32 = 256;

/// Blocks fed before the measured one, so a section's state is settled when measurement starts.
///
/// Eight blocks is 2,048 positions. The slowest pole any built-in filter can have is at
/// `1 − 2G` for the smallest `G` the shipped coefficient table produces, and 2,048 positions is
/// past the point where its transient is below one LSB of the probe amplitude.
pub const RESPONSE_SETTLE_BLOCKS: u32 = 8;

/// The probe amplitude the recorded figures were taken at.
///
/// A quarter of full scale, chosen so that the largest lift the built-ins offer — a `band_gain` of
/// 4.0 on [`Peaking`](crate::dsp::effects::Peaking) — cannot reach the clamp. A sweep that clipped
/// would be measuring the clamp.
pub const SWEEP_AMPLITUDE: i16 = 8_192;

/// The bins the recorded sweep runs, in order.
///
/// Bin *k* is the tone at `k·rate/256`: on a narrowband call these are 62.5, 125, 250, 500, 1,000,
/// 2,000, 3,000 and 3,500 Hz. They are geometric up to the middle of the band and then linear,
/// because a one-pole's roll-off is a straight line in octaves and the top of a narrowband channel
/// is where the interesting part of a low-pass sits.
pub const SWEEP_BINS: &[u16] = &[2, 4, 8, 16, 32, 64, 96, 112];

/// The highest bin the corpus can carry.
///
/// Bin 128 is the folding frequency, where a sine probe is identically zero, and every bin above it
/// aliases onto one below. Neither is a measurement, so both are refused.
pub const MAX_SWEEP_BIN: u16 = 127;

/// Quarter-wave sine table at Q15: entry *k* is `round(32_767·sin(π·k/128))`.
///
/// A shipped table for the same reason [`super::effects`]'s coefficients are one —
/// `docs/specs/custom-call-dsp.md` §4.6 keeps transcendental functions out of this crate at run
/// time, and a table computed once and written down is the form that survives being read on
/// another machine. Sixty-five entries cover a quarter cycle; the other three quadrants are the
/// symmetries of a sine and are taken in [`sine_q15`].
const SINE_Q15: [i16; 65] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602, 6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530, 18204, 18868, 19519, 20159, 20787,
    21403, 22005, 22594, 23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790, 27245, 27683,
    28105, 28510, 28898, 29268, 29621, 29956, 30273, 30571, 30852, 31113, 31356, 31580, 31785,
    31971, 32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757, 32767,
];

/// `sin(2π·index/256)` at Q15, from [`SINE_Q15`] and the symmetries of a sine.
fn sine_q15(index: u32) -> i64 {
    let index = index % RESPONSE_BLOCK_POSITIONS;
    let (quadrant_index, negate) = match index {
        0..=64 => (index, false),
        65..=128 => (128 - index, false),
        129..=192 => (index - 128, true),
        _ => (256 - index, true),
    };
    let magnitude = usize::try_from(quadrant_index)
        .ok()
        .and_then(|at| SINE_Q15.get(at))
        .copied()
        .unwrap_or(0);
    if negate {
        -i64::from(magnitude)
    } else {
        i64::from(magnitude)
    }
}

/// One position of the probe tone at `bin`, `amplitude` and `position`.
///
/// `round(amplitude·sin(2π·bin·position/256))`, rounded half away from zero so that the tone and
/// its negation are exact negations of one another. That is the same rounding rule
/// [`super::effects`] uses everywhere, and it is what makes a polarity inversion measure exactly
/// unity rather than unity plus a rounding residue.
#[must_use]
pub fn probe_sample(bin: u16, amplitude: i16, position: u32) -> i16 {
    let phase = u32::from(bin).wrapping_mul(position) % RESPONSE_BLOCK_POSITIONS;
    let value = scaled(i64::from(amplitude), sine_q15(phase), 32_767);
    i16::try_from(narrow(value)).unwrap_or(0)
}

/// `positions` positions of the probe tone at `bin` and `amplitude`, one channel.
///
/// The corpus is a function and not a file: a reader who wants to check a recorded figure
/// regenerates the exact samples that produced it from this, at any length, on any machine.
#[must_use]
pub fn probe_tone(bin: u16, amplitude: i16, positions: u32) -> Vec<i16> {
    (0..positions)
        .map(|position| probe_sample(bin, amplitude, position))
        .collect()
}

/// One bin's measured magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponsePoint {
    bin: u16,
    frequency_millihertz: u64,
    magnitude_thousandths: u32,
}

impl ResponsePoint {
    /// The bin, in cycles per [`RESPONSE_BLOCK_POSITIONS`]-position block.
    #[must_use]
    pub const fn bin(&self) -> u16 {
        self.bin
    }

    /// The probe frequency in **millihertz**, because `bin·rate/256` is not always a whole hertz:
    /// bin 2 on a narrowband call is 62.5 Hz, and rounding it in the report would make two
    /// different bins print the same frequency.
    #[must_use]
    pub const fn frequency_millihertz(&self) -> u64 {
        self.frequency_millihertz
    }

    /// The measured magnitude in thousandths of unity. 1,000 is "unchanged".
    #[must_use]
    pub const fn magnitude_thousandths(&self) -> u32 {
        self.magnitude_thousandths
    }
}

impl fmt::Display for ResponsePoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "bin {:>4} ({:>9} mHz): {:>5}",
            self.bin, self.frequency_millihertz, self.magnitude_thousandths
        )
    }
}

/// Every bin of one sweep, in the order [`ResponseSweep`] ran them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseReport {
    format: StreamFormat,
    amplitude: i16,
    points: Vec<ResponsePoint>,
}

impl ResponseReport {
    /// The format the sweep ran on. A response is a statement about a rate and a channel count and
    /// is meaningless without them, so they travel with the figures.
    #[must_use]
    pub const fn format(&self) -> StreamFormat {
        self.format
    }

    /// The probe amplitude the figures were taken at.
    #[must_use]
    pub const fn amplitude(&self) -> i16 {
        self.amplitude
    }

    /// Every measured point.
    #[must_use]
    pub fn points(&self) -> &[ResponsePoint] {
        &self.points
    }

    /// Just the magnitudes, in bin order — the shape a recorded table is compared against.
    pub fn magnitudes(&self) -> impl Iterator<Item = u32> + '_ {
        self.points.iter().map(ResponsePoint::magnitude_thousandths)
    }

    /// The magnitude at one bin, or `None` if the sweep did not run it.
    #[must_use]
    pub fn magnitude_at(&self, bin: u16) -> Option<u32> {
        self.points
            .iter()
            .find(|point| point.bin == bin)
            .map(ResponsePoint::magnitude_thousandths)
    }
}

impl fmt::Display for ResponseReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "{} Hz, {} channel(s), probe amplitude {}",
            self.format.sample_rate(),
            self.format.channels(),
            self.amplitude
        )?;
        for point in &self.points {
            writeln!(formatter, "  {point}")?;
        }
        Ok(())
    }
}

/// Why a sweep produced no figure.
///
/// Every variant is a case the sweep **will not guess at**. A response table with a row quietly
/// missing is the failure `docs/specs/custom-call-dsp.md` §11 names for the conformance harness,
/// and it is the same failure here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResponseError {
    /// The processor refused the format the sweep was asked to run on.
    Format(FormatError),
    /// The processor refused, or failed on, a probe frame.
    Process(ProcessError),
    /// The bin is 0 — a probe of silence — or above [`MAX_SWEEP_BIN`].
    BinOutOfRange {
        /// The bin that was asked for.
        bin: u16,
    },
    /// The processor declares a latency, so its output is the probe shifted in time and the
    /// measured block is not the block that produced it.
    LatencyNotZero {
        /// The declared latency, in positions.
        positions: u32,
    },
    /// The processor declares a tail, so part of its answer is only reachable through `flush`,
    /// which a sweep of a continuous tone never calls.
    TailNotZero {
        /// The declared tail, in positions.
        positions: u32,
    },
    /// The processor does not declare [`LengthPolicy::Preserving`], so one position out for one
    /// position in — which every alignment here assumes — is not something it promised.
    LengthNotPreserving {
        /// What the processor declared instead.
        policy: LengthPolicy,
    },
    /// The processor's declared frame ceiling is below one probe block.
    FrameCeilingTooLow {
        /// The declared ceiling, in samples.
        max_frame_samples: u32,
        /// What one block of the sweep's format needs, in samples.
        needed: u32,
    },
    /// The processor wrote fewer positions than it was given, despite declaring
    /// [`LengthPolicy::Preserving`].
    ShortOutput {
        /// Samples expected in the measured block.
        expected: usize,
        /// Samples the processor actually wrote.
        produced: usize,
    },
}

impl fmt::Display for ResponseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(error) => write!(formatter, "the processor refused the format: {error}"),
            Self::Process(error) => {
                write!(formatter, "the processor refused a probe frame: {error}")
            }
            Self::BinOutOfRange { bin } => write!(
                formatter,
                "bin {bin} is not a tone this corpus carries; bins run 1..={MAX_SWEEP_BIN}"
            ),
            Self::LatencyNotZero { positions } => write!(
                formatter,
                "the processor declares a latency of {positions} positions, which this sweep does \
                 not correct for"
            ),
            Self::TailNotZero { positions } => write!(
                formatter,
                "the processor declares a tail of {positions} positions, which this sweep never \
                 flushes"
            ),
            Self::LengthNotPreserving { policy } => write!(
                formatter,
                "the processor declares {policy:?} rather than a preserving length policy"
            ),
            Self::FrameCeilingTooLow {
                max_frame_samples,
                needed,
            } => write!(
                formatter,
                "the processor accepts at most {max_frame_samples} samples per frame and one probe \
                 block is {needed}"
            ),
            Self::ShortOutput { expected, produced } => write!(
                formatter,
                "the processor wrote {produced} samples where a preserving policy owes {expected}"
            ),
        }
    }
}

impl std::error::Error for ResponseError {}

impl From<FormatError> for ResponseError {
    fn from(error: FormatError) -> Self {
        Self::Format(error)
    }
}

impl From<ProcessError> for ResponseError {
    fn from(error: ProcessError) -> Self {
        Self::Process(error)
    }
}

/// A magnitude-response sweep over one processor.
///
/// Built like [`Conformance`](crate::dsp::Conformance) and for the same reason: it takes a
/// **factory** rather than a processor, because each bin has to start from an instance that has
/// seen nothing. A sweep that reused one instance would measure the previous bin's settled state
/// at the start of the next.
#[derive(Debug, Clone)]
pub struct ResponseSweep {
    format: StreamFormat,
    direction: AudioDirection,
    amplitude: i16,
    settle_blocks: u32,
    bins: Vec<u16>,
}

impl ResponseSweep {
    /// The recorded configuration on `format`: [`SWEEP_BINS`] at [`SWEEP_AMPLITUDE`], inbound,
    /// after [`RESPONSE_SETTLE_BLOCKS`] settling blocks.
    #[must_use]
    pub fn new(format: StreamFormat) -> Self {
        Self {
            format,
            direction: AudioDirection::Inbound,
            amplitude: SWEEP_AMPLITUDE,
            settle_blocks: RESPONSE_SETTLE_BLOCKS,
            bins: SWEEP_BINS.to_vec(),
        }
    }

    /// Sweep the other direction. A processor is bound to one direction at `prepare`, so this is
    /// part of the configuration a figure is true of.
    #[must_use]
    pub fn with_direction(mut self, direction: AudioDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Probe at a different amplitude. Worth doing to a processor suspected of nonlinearity: a
    /// linear one reports the same magnitudes at every amplitude and a nonlinear one does not.
    #[must_use]
    pub fn with_amplitude(mut self, amplitude: i16) -> Self {
        self.amplitude = amplitude;
        self
    }

    /// Settle for a different number of blocks.
    #[must_use]
    pub fn with_settle_blocks(mut self, blocks: u32) -> Self {
        self.settle_blocks = blocks;
        self
    }

    /// Sweep a different set of bins, in the given order.
    #[must_use]
    pub fn with_bins(mut self, bins: &[u16]) -> Self {
        self.bins = bins.to_vec();
        self
    }

    /// Run every bin, stopping at the first the processor cannot be measured on.
    ///
    /// Stopping rather than recording a hole: a partial table read as a whole one is the way a
    /// response claim goes wrong, and a caller who wants the bins that did work can ask for them
    /// one at a time.
    pub fn run<P, F>(&self, mut factory: F) -> Result<ResponseReport, ResponseError>
    where
        P: FrameProcessor,
        F: FnMut() -> P,
    {
        let mut points = Vec::with_capacity(self.bins.len());
        for &bin in &self.bins {
            points.push(self.measure(bin, &mut factory())?);
        }
        Ok(ResponseReport {
            format: self.format,
            amplitude: self.amplitude,
            points,
        })
    }

    /// One bin, on one fresh processor.
    fn measure<P: FrameProcessor>(
        &self,
        bin: u16,
        processor: &mut P,
    ) -> Result<ResponsePoint, ResponseError> {
        if bin == 0 || bin > MAX_SWEEP_BIN {
            return Err(ResponseError::BinOutOfRange { bin });
        }
        let capability = processor.capability();
        if capability.latency_positions() != 0 {
            return Err(ResponseError::LatencyNotZero {
                positions: capability.latency_positions(),
            });
        }
        if capability.tail_positions() != 0 {
            return Err(ResponseError::TailNotZero {
                positions: capability.tail_positions(),
            });
        }
        if capability.length() != LengthPolicy::Preserving {
            return Err(ResponseError::LengthNotPreserving {
                policy: capability.length(),
            });
        }
        let channels = u32::from(self.format.channels());
        let block_samples = RESPONSE_BLOCK_POSITIONS.saturating_mul(channels);
        if capability.max_frame_samples() < block_samples {
            return Err(ResponseError::FrameCeilingTooLow {
                max_frame_samples: capability.max_frame_samples(),
                needed: block_samples,
            });
        }
        processor.prepare(self.direction, self.format)?;

        let block_samples = usize::try_from(block_samples).unwrap_or(usize::MAX);
        let mut output = vec![0i16; block_samples];
        let mut scratch_buffer =
            vec![0i16; usize::try_from(capability.scratch_samples()).unwrap_or(0)];
        let mut input = vec![0i16; block_samples];
        let mut input_energy: u128 = 0;
        let mut output_energy: u128 = 0;

        for block in 0..=self.settle_blocks {
            let start = block.saturating_mul(RESPONSE_BLOCK_POSITIONS);
            fill_block(&mut input, bin, self.amplitude, start, channels);
            let frame = DspFrame::new(
                self.direction,
                self.format,
                u64::from(start),
                input.as_slice(),
            );
            let mut observations = Vec::new();
            let written = {
                let mut scratch = Scratch::new(&mut scratch_buffer);
                let mut sink = FrameSink::new(&mut output, &mut observations, 32);
                processor.process(&frame, &mut scratch, &mut sink)?;
                sink.written()
            };
            if block != self.settle_blocks {
                continue;
            }
            if written != block_samples {
                return Err(ResponseError::ShortOutput {
                    expected: block_samples,
                    produced: written,
                });
            }
            input_energy = energy(&input);
            output_energy = energy(output.get(..written).unwrap_or_default());
        }

        Ok(ResponsePoint {
            bin,
            frequency_millihertz: u64::from(self.format.sample_rate())
                .saturating_mul(u64::from(bin))
                .saturating_mul(1_000)
                / u64::from(RESPONSE_BLOCK_POSITIONS),
            magnitude_thousandths: magnitude_thousandths(output_energy, input_energy),
        })
    }
}

/// Write one interleaved block of the probe tone, the same tone in every channel.
fn fill_block(block: &mut [i16], bin: u16, amplitude: i16, start: u32, channels: u32) {
    for (index, sample) in block.iter_mut().enumerate() {
        let position =
            start.saturating_add(u32::try_from(index).unwrap_or(u32::MAX) / channels.max(1));
        *sample = probe_sample(bin, amplitude, position);
    }
}

/// `Σ x²` over a block, at a width that cannot overflow: 65,536 samples of full scale is 2^47.
fn energy(samples: &[i16]) -> u128 {
    samples
        .iter()
        .map(|sample| {
            let value = i128::from(*sample);
            u128::try_from(value.saturating_mul(value)).unwrap_or(0)
        })
        .sum()
}

/// `round(1000·√(output/input))`, entirely in integers.
///
/// `isqrt` truncates, so the ratio is scaled by four million rather than one million and the
/// result halved: `⌊2000·√r⌋ + 1` over two is `1000·√r` rounded to nearest.
fn magnitude_thousandths(output_energy: u128, input_energy: u128) -> u32 {
    if input_energy == 0 {
        return 0;
    }
    let scaled_ratio = output_energy.saturating_mul(4_000_000) / input_energy;
    let doubled = scaled_ratio.isqrt();
    u32::try_from(doubled.saturating_add(1) / 2).unwrap_or(u32::MAX)
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

    /// The table is a quarter of a sine and the symmetries reconstruct the other three quadrants.
    #[test]
    fn the_tone_is_a_sine() {
        assert_eq!(sine_q15(0), 0);
        assert_eq!(sine_q15(64), 32_767);
        assert_eq!(sine_q15(128), 0);
        assert_eq!(sine_q15(192), -32_767);
        for index in 0..256 {
            assert_eq!(sine_q15(index), -sine_q15(index + 128), "at {index}");
        }
    }

    /// The probe is exactly antisymmetric across half a cycle, which is what makes a polarity
    /// inversion measure unity rather than unity plus a residue.
    #[test]
    fn the_probe_is_antisymmetric() {
        for &bin in &[1_u16, 2, 4, 8, 16, 32, 64] {
            let half_cycle = 128 / u32::from(bin);
            for position in 0..256 {
                assert_eq!(
                    probe_sample(bin, SWEEP_AMPLITUDE, position),
                    -probe_sample(bin, SWEEP_AMPLITUDE, position + half_cycle),
                    "bin {bin} at {position}",
                );
            }
        }
    }

    /// A block spans a whole number of cycles, so its mean is zero and its energy is `N·A²/2` to
    /// within the table's own rounding.
    #[test]
    fn a_block_is_a_whole_number_of_cycles() {
        for &bin in SWEEP_BINS {
            let tone = probe_tone(bin, SWEEP_AMPLITUDE, RESPONSE_BLOCK_POSITIONS);
            let sum: i64 = tone.iter().map(|sample| i64::from(*sample)).sum();
            assert_eq!(sum, 0, "bin {bin} has a DC offset");
            let ideal = u128::from(RESPONSE_BLOCK_POSITIONS)
                * u128::try_from(i64::from(SWEEP_AMPLITUDE).pow(2)).unwrap()
                / 2;
            let measured = energy(&tone);
            let error = measured.abs_diff(ideal);
            assert!(
                error * 1_000 < ideal,
                "bin {bin}: {measured} against {ideal}"
            );
        }
    }

    /// The ratio is rounded to nearest and unity is exactly 1,000.
    #[test]
    fn the_magnitude_is_a_rounded_ratio() {
        assert_eq!(magnitude_thousandths(100, 100), 1_000);
        assert_eq!(magnitude_thousandths(25, 100), 500);
        assert_eq!(magnitude_thousandths(0, 100), 0);
        assert_eq!(magnitude_thousandths(100, 0), 0);
        assert_eq!(magnitude_thousandths(400, 100), 2_000);
    }
}
