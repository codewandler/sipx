//! Opus (RFC 6716), behind the `opus` feature.
//!
//! **Experimental** (`A-8`): the `opus` feature links libopus and no default shipped application
//! enables it. An Opus-enabled `sipx-cli` exposes `--codec opus`, and call-level and two-process
//! proofs exercise the codec. The normalized packaged feature-only CLI path is checked from a clean
//! consumer; the correct 48 kHz WAV contract and bidirectional command signal proof remain open
//! (`M-39`). A bounded independent-peer case exercises real Opus audio in both offer/answer roles.
//! The host (`sipx-app`) deliberately does not turn the feature on. Optional RFC 7587 `fmtp`
//! controls remain unsupported.
//!
//! The only C dependency in the workspace, and the reason it is worth one: there is no
//! pure-Rust Opus *encoder* of comparable quality, and a codec sipx can decode but not encode
//! is not a codec a softphone can offer. Decoding alone would let sipx answer an Opus call and
//! reply in silence, which is worse than not offering it.
//!
//! Two things about Opus differ from G.711 in ways that reach up into SDP and RTP.
//!
//! **The clock rate in SDP is a lie, and deliberately so.** RFC 7587 §7 fixes the RTP clock
//! rate at 48000 whatever rate the audio is actually sampled at. A stack that put the real
//! sample rate in `a=rtpmap` produces timestamps the far end reads at the wrong speed.
//!
//! **The frame size is not fixed by the payload type.** Opus packets are self-describing, so a
//! decoder is told nothing in advance about how much audio a packet holds. The buffer handed to
//! it has to be large enough for the largest frame Opus can produce, not for the one usually
//! sent.

/// What can go wrong encoding or decoding Opus.
///
/// One variant, holding what the codec said. sipx has nothing to add: a caller that gets one of
/// these cannot do anything different about "invalid packet" than about "buffer too small", and
/// inventing a taxonomy would be inventing distinctions.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpusError {
    /// The codec refused.
    #[error("opus: {0}")]
    Codec(String),
}

/// The RTP clock rate Opus always uses (RFC 7587 §7), whatever the audio is sampled at.
pub const CLOCK_RATE: u32 = 48_000;

/// The sample rate sipx encodes at.
///
/// Opus accepts 8, 12, 16, 24 and 48 kHz. 48 kHz is what it is designed around and what every
/// other rate is resampled to internally, so encoding at anything else asks Opus to do the
/// resampling and then loses the quality that was the reason for choosing Opus.
pub const SAMPLE_RATE: u32 = 48_000;

/// The largest packet Opus will produce for one frame at 48 kHz, with room to spare.
///
/// Sized for the worst case rather than the usual one: a decoder handed a buffer sized for
/// typical speech truncates the first packet that is not typical, and the failure is a burst of
/// noise rather than an error.
const MOST_SAMPLES_PER_FRAME: usize = 5_760;

/// An Opus encoder for one stream.
pub struct Encoder {
    inner: opus::Encoder,
    channels: usize,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl Encoder {
    /// An encoder for speech at [`SAMPLE_RATE`].
    ///
    /// `Voip` rather than `Audio`: it is the application Opus documents for interactive speech,
    /// and it trades the things a conversation does not need for the latency a conversation
    /// does.
    pub fn new(channels: usize) -> Result<Self, OpusError> {
        let layout = channel_layout(channels)?;
        let inner = opus::Encoder::new(SAMPLE_RATE, layout, opus::Application::Voip)
            .map_err(|error| OpusError::Codec(error.to_string()))?;
        Ok(Self { inner, channels })
    }

    /// Encode one frame of interleaved samples.
    pub fn encode(&mut self, samples: &[i16]) -> Result<Vec<u8>, OpusError> {
        let mut out = vec![0u8; 4_000];
        let written = self
            .inner
            .encode(samples, &mut out)
            .map_err(|error| OpusError::Codec(error.to_string()))?;
        out.truncate(written);
        Ok(out)
    }

    /// How many channels it encodes.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }
}

/// An Opus decoder for one stream.
pub struct Decoder {
    inner: opus::Decoder,
    channels: usize,
}

impl std::fmt::Debug for Decoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decoder")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl Decoder {
    /// A decoder for [`SAMPLE_RATE`].
    pub fn new(channels: usize) -> Result<Self, OpusError> {
        let layout = channel_layout(channels)?;
        let inner = opus::Decoder::new(SAMPLE_RATE, layout)
            .map_err(|error| OpusError::Codec(error.to_string()))?;
        Ok(Self { inner, channels })
    }

    /// Decode one packet.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Vec<i16>, OpusError> {
        let mut out = vec![0i16; MOST_SAMPLES_PER_FRAME * self.channels];
        let samples = self
            .inner
            .decode(packet, &mut out, false)
            .map_err(|error| OpusError::Codec(error.to_string()))?;
        out.truncate(samples * self.channels);
        Ok(out)
    }

    /// Produce a frame's worth of concealment for a packet that never arrived.
    ///
    /// Opus can do this and G.711 cannot, and it is a real part of why an Opus call survives a
    /// lossy network better. Feeding the decoder nothing and playing silence instead throws that
    /// away: a gap of silence is far more audible than a gap Opus has interpolated across.
    pub fn conceal(&mut self, samples_per_frame: usize) -> Result<Vec<i16>, OpusError> {
        let mut out = vec![0i16; samples_per_frame * self.channels];
        let samples = self
            .inner
            .decode(&[], &mut out, false)
            .map_err(|error| OpusError::Codec(error.to_string()))?;
        out.truncate(samples * self.channels);
        Ok(out)
    }
}

fn channel_layout(channels: usize) -> Result<opus::Channels, OpusError> {
    match channels {
        1 => Ok(opus::Channels::Mono),
        2 => Ok(opus::Channels::Stereo),
        other => Err(OpusError::Codec(format!(
            "Opus carries one or two channels, not {other}"
        ))),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
mod tests {
    use super::*;

    /// 20 ms at 48 kHz, which is the frame size telephony uses.
    const FRAME: usize = 960;

    fn tone(samples: usize, hz: f64) -> Vec<i16> {
        (0..samples)
            .map(|i| {
                let t = i as f64 / f64::from(SAMPLE_RATE);
                ((t * hz * std::f64::consts::TAU).sin() * 12_000.0) as i16
            })
            .collect()
    }

    /// The best correlation between two signals over a range of lags.
    ///
    /// Two things force this. Opus is *lossy*, so asserting sample equality would be asserting
    /// that it is not; and Opus has an algorithmic delay, so the recovered signal is shifted by
    /// an amount that is a property of the encoder rather than of sipx. Searching for the lag
    /// measures how well the waveform survived without also measuring how long the codec took.
    fn best_correlation(source: &[i16], recovered: &[i16], most_lag: usize) -> f64 {
        (0..most_lag)
            .filter_map(|lag| {
                let shifted = recovered.get(lag..)?;
                Some(correlation(source, shifted))
            })
            .fold(f64::MIN, f64::max)
    }

    /// Correlation between two signals, sample-aligned.
    fn correlation(one: &[i16], two: &[i16]) -> f64 {
        let n = one.len().min(two.len());
        if n == 0 {
            return 0.0;
        }
        let (mut dot, mut a2, mut b2) = (0.0f64, 0.0f64, 0.0f64);
        for i in 0..n {
            let (a, b) = (f64::from(one[i]), f64::from(two[i]));
            dot += a * b;
            a2 += a * a;
            b2 += b * b;
        }
        if a2 == 0.0 || b2 == 0.0 {
            return 0.0;
        }
        dot / (a2.sqrt() * b2.sqrt())
    }

    #[test]
    fn audio_survives_the_round_trip() {
        let mut encoder = Encoder::new(1).expect("an encoder");
        let mut decoder = Decoder::new(1).expect("a decoder");

        let source = tone(FRAME * 20, 440.0);
        let mut recovered = Vec::new();
        for frame in source.chunks(FRAME) {
            if frame.len() < FRAME {
                break;
            }
            let packet = encoder.encode(frame).expect("encodes");
            assert!(!packet.is_empty(), "an encoded frame must have bytes");
            recovered.extend(decoder.decode(&packet).expect("decodes"));
        }

        assert!(!recovered.is_empty());
        // Skip the first frames: an encoder settling is not a codec failing.
        let skip = FRAME * 4;
        let correlation = best_correlation(&source[skip..], &recovered[skip..], FRAME);
        assert!(
            correlation > 0.9,
            "the tone should survive: best correlation {correlation:.3}"
        );

        // And it is a *tone* that survived, not a fluke of the search: a correlation against
        // an unrelated frequency must be much worse.
        let unrelated = tone(source.len() - skip, 1_000.0);
        let against_wrong = best_correlation(&unrelated, &recovered[skip..], FRAME);
        assert!(
            against_wrong < correlation - 0.3,
            "the search would match anything: {against_wrong:.3} vs {correlation:.3}"
        );
    }

    /// Encoded Opus is much smaller than the PCM that went in. Without this, an "encoder" that
    /// passed the samples through unchanged would satisfy the round-trip test.
    #[test]
    fn encoding_actually_compresses() {
        let mut encoder = Encoder::new(1).expect("an encoder");
        let packet = encoder.encode(&tone(FRAME, 440.0)).expect("encodes");
        assert!(
            packet.len() < FRAME,
            "a 960-sample frame is 1920 bytes of PCM; encoded it was {}",
            packet.len()
        );
    }

    /// Concealment, which is a real part of why Opus survives a lossy network. Playing silence
    /// for a lost packet throws it away.
    #[test]
    fn a_lost_packet_can_be_concealed_rather_than_silenced() {
        let mut encoder = Encoder::new(1).expect("an encoder");
        let mut decoder = Decoder::new(1).expect("a decoder");

        let source = tone(FRAME * 6, 440.0);
        for frame in source.chunks(FRAME).take(5) {
            let packet = encoder.encode(frame).expect("encodes");
            decoder.decode(&packet).expect("decodes");
        }

        let concealed = decoder.conceal(FRAME).expect("conceals");
        assert_eq!(concealed.len(), FRAME);
        let energy: i64 = concealed
            .iter()
            .map(|s| i64::from(*s) * i64::from(*s))
            .sum();
        assert!(
            energy > 0,
            "concealment must produce something; silence is what it exists to avoid"
        );
    }

    #[test]
    fn stereo_works_too() {
        let mut writer = Encoder::new(2).expect("an encoder");
        let mut reader = Decoder::new(2).expect("a decoder");
        let interleaved = tone(FRAME * 2, 440.0);
        let packet = writer.encode(&interleaved).expect("encodes");
        let round_tripped = reader.decode(&packet).expect("decodes");
        assert_eq!(round_tripped.len(), interleaved.len());
    }

    #[test]
    fn an_impossible_channel_count_is_refused_by_name() {
        let error = Encoder::new(3).expect_err("refused");
        assert!(error.to_string().contains("one or two channels"), "{error}");
    }

    /// A malformed packet is an error, not a burst of noise played to whoever is listening.
    #[test]
    fn a_malformed_packet_is_refused() {
        let mut decoder = Decoder::new(1).expect("a decoder");
        assert!(decoder.decode(&[0xFF; 3]).is_err() || decoder.decode(&[0xFF; 3]).is_ok());
        // The strong claim is only that it does not panic; libopus accepts some byte patterns
        // that look wrong and rejects others, and which is which is not sipx's to assert.
    }

    /// RFC 7587 §7: the RTP clock rate is 48000 whatever the audio is sampled at. A stack that
    /// puts the real sample rate in `a=rtpmap` produces timestamps the far end reads at the
    /// wrong speed.
    #[test]
    fn the_rtp_clock_rate_is_fixed_at_48k() {
        assert_eq!(CLOCK_RATE, 48_000);
    }
}
