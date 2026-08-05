//! Explicit linear-PCM formats and streaming rate conversion.

/// Highest application PCM rate accepted by the conversion boundary.
pub const MAX_SAMPLE_RATE: u32 = 384_000;

/// A supported linear sample representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PcmEncoding {
    /// Unsigned eight-bit PCM, whose silence midpoint is 128.
    Unsigned8,
    /// Signed native `i16` samples.
    Signed16,
}

/// The representation and rate of a mono linear-PCM stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PcmFormat {
    sample_rate: u32,
    encoding: PcmEncoding,
}

impl PcmFormat {
    /// Validate an application PCM format.
    ///
    /// # Errors
    ///
    /// Returns [`PcmError::UnsupportedSampleRate`] for zero or a rate above
    /// [`MAX_SAMPLE_RATE`].
    pub const fn new(sample_rate: u32, encoding: PcmEncoding) -> Result<Self, PcmError> {
        if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
            return Err(PcmError::UnsupportedSampleRate(sample_rate));
        }
        Ok(Self {
            sample_rate,
            encoding,
        })
    }

    /// Samples per second.
    #[must_use]
    pub const fn sample_rate(self) -> u32 {
        self.sample_rate
    }

    /// Sample representation.
    #[must_use]
    pub const fn encoding(self) -> PcmEncoding {
        self.encoding
    }
}

/// Owned samples whose variant states their depth.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PcmSamples {
    /// Unsigned eight-bit linear samples.
    Unsigned8(Vec<u8>),
    /// Signed sixteen-bit linear samples.
    Signed16(Vec<i16>),
}

impl PcmSamples {
    /// Number of mono samples.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Unsigned8(samples) => samples.len(),
            Self::Signed16(samples) => samples.len(),
        }
    }

    /// Whether no samples are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One owned mono PCM buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    format: PcmFormat,
    samples: PcmSamples,
}

impl Pcm {
    /// Pair a validated format with samples of the same depth.
    ///
    /// # Errors
    ///
    /// Returns [`PcmError::EncodingMismatch`] when the format and sample variant disagree.
    pub fn new(format: PcmFormat, samples: PcmSamples) -> Result<Self, PcmError> {
        if !matches!(
            (format.encoding, &samples),
            (PcmEncoding::Unsigned8, PcmSamples::Unsigned8(_))
                | (PcmEncoding::Signed16, PcmSamples::Signed16(_))
        ) {
            return Err(PcmError::EncodingMismatch);
        }
        Ok(Self { format, samples })
    }

    /// The rate and depth attached to these samples.
    #[must_use]
    pub const fn format(&self) -> PcmFormat {
        self.format
    }

    /// The owned sample representation.
    #[must_use]
    pub const fn samples(&self) -> &PcmSamples {
        &self.samples
    }

    /// Consume the buffer and return its samples.
    #[must_use]
    pub fn into_samples(self) -> PcmSamples {
        self.samples
    }

    /// Convert to signed 16-bit samples at `target_rate`.
    ///
    /// # Errors
    ///
    /// Returns [`PcmError::UnsupportedSampleRate`] when `target_rate` is unsupported.
    pub fn to_i16(&self, target_rate: u32) -> Result<Vec<i16>, PcmError> {
        let input = match &self.samples {
            PcmSamples::Unsigned8(samples) => samples
                .iter()
                .map(|sample| (i16::from(*sample) - 128) << 8)
                .collect(),
            PcmSamples::Signed16(samples) => samples.clone(),
        };
        let mut resampler = LinearResampler::new(self.format.sample_rate, target_rate)?;
        Ok(resampler.push_i16(&input))
    }

    /// Build a buffer in `format` from signed samples already at that format's rate.
    #[must_use]
    pub fn from_i16(format: PcmFormat, samples: Vec<i16>) -> Self {
        let samples = match format.encoding {
            PcmEncoding::Unsigned8 => PcmSamples::Unsigned8(
                samples
                    .into_iter()
                    .map(|sample| {
                        let shifted = (i32::from(sample) + 32_768) >> 8;
                        u8::try_from(shifted).unwrap_or(if shifted < 0 { 0 } else { u8::MAX })
                    })
                    .collect(),
            ),
            PcmEncoding::Signed16 => PcmSamples::Signed16(samples),
        };
        Self { format, samples }
    }
}

/// A typed PCM-boundary refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PcmError {
    /// The rate is zero or beyond the supported conversion bound.
    #[error("unsupported linear PCM sample rate {0} Hz; expected 1..={MAX_SAMPLE_RATE}")]
    UnsupportedSampleRate(u32),
    /// The format's depth does not match the owned sample variant.
    #[error("linear PCM format and sample representation do not match")]
    EncodingMismatch,
}

/// Convert one complete signed-16 stream between explicit rates.
///
/// For adjacent chunks use [`LinearResampler`] directly so interpolation history crosses the
/// chunk boundary.
///
/// # Errors
///
/// Returns [`PcmError::UnsupportedSampleRate`] when either rate is unsupported.
pub fn resample_i16(
    samples: &[i16],
    source_rate: u32,
    target_rate: u32,
) -> Result<Vec<i16>, PcmError> {
    let mut resampler = LinearResampler::new(source_rate, target_rate)?;
    Ok(resampler.push_i16(samples))
}

/// Streaming linear interpolation between two sample rates.
#[derive(Debug, Clone)]
pub struct LinearResampler {
    source_rate: u32,
    target_rate: u32,
    previous: Option<i16>,
    source_index: u64,
    next_numerator: u64,
}

impl LinearResampler {
    /// Start one continuous conversion stream.
    ///
    /// # Errors
    ///
    /// Returns [`PcmError::UnsupportedSampleRate`] when either rate is unsupported.
    pub const fn new(source_rate: u32, target_rate: u32) -> Result<Self, PcmError> {
        if source_rate == 0 || source_rate > MAX_SAMPLE_RATE {
            return Err(PcmError::UnsupportedSampleRate(source_rate));
        }
        if target_rate == 0 || target_rate > MAX_SAMPLE_RATE {
            return Err(PcmError::UnsupportedSampleRate(target_rate));
        }
        Ok(Self {
            source_rate,
            target_rate,
            previous: None,
            source_index: 0,
            next_numerator: 0,
        })
    }

    /// Convert the next adjacent signed-16 chunk.
    #[must_use]
    pub fn push_i16(&mut self, samples: &[i16]) -> Vec<i16> {
        let estimate = (samples
            .len()
            .saturating_mul(usize::try_from(self.target_rate).unwrap_or(usize::MAX))
            / usize::try_from(self.source_rate).unwrap_or(1))
        .saturating_add(1);
        let mut output = Vec::with_capacity(estimate.min(1_048_576));
        for &current in samples {
            let Some(previous) = self.previous else {
                output.push(current);
                self.previous = Some(current);
                self.next_numerator = u64::from(self.source_rate);
                continue;
            };
            self.source_index = self.source_index.saturating_add(1);
            let interval_start = self
                .source_index
                .saturating_sub(1)
                .saturating_mul(u64::from(self.target_rate));
            let interval_end = self
                .source_index
                .saturating_mul(u64::from(self.target_rate));
            while self.next_numerator <= interval_end {
                let fraction_numerator =
                    u32::try_from(self.next_numerator.saturating_sub(interval_start))
                        .unwrap_or(self.target_rate);
                let span = f64::from(self.target_rate);
                let fraction = f64::from(fraction_numerator) / span;
                let value =
                    f64::from(previous) + (f64::from(current) - f64::from(previous)) * fraction;
                output.push(round_i16(value));
                self.next_numerator = self
                    .next_numerator
                    .saturating_add(u64::from(self.source_rate));
            }
            self.previous = Some(current);
        }
        output
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the rounded value is clamped to the i16 domain before conversion"
)]
fn round_i16(value: f64) -> i16 {
    let rounded = value
        .round()
        .clamp(f64::from(i16::MIN), f64::from(i16::MAX));
    i16::try_from(rounded as i32).unwrap_or(if rounded.is_sign_negative() {
        i16::MIN
    } else {
        i16::MAX
    })
}
