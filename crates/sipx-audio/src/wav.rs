//! WAV files, for the narrow case the tests need: 8 kHz 16-bit mono PCM.
//!
//! Deliberately not a general WAV library. It reads what it writes and what a call records,
//! and it refuses anything else by name rather than by producing noise — a WAV reader that
//! silently misinterprets a format produces audio that is *almost* right, which is far harder
//! to diagnose than a refusal.

use std::io::{Read, Write};

/// What can go wrong with a WAV file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WavError {
    /// The file could not be read or written.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// It is not a RIFF/WAVE file at all.
    #[error("not a WAVE file")]
    NotWave,
    /// It is a WAVE file this crate does not handle.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A chunk ran past the end of the file.
    #[error("truncated")]
    Truncated,
}

/// 16-bit mono PCM at a given sample rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wav {
    /// Samples per second.
    pub sample_rate: u32,
    /// The samples.
    pub samples: Vec<i16>,
}

impl Wav {
    /// A clip at 8 kHz, the rate G.711 uses.
    #[must_use]
    pub fn narrowband(samples: Vec<i16>) -> Self {
        Self {
            sample_rate: 8000,
            samples,
        }
    }

    /// How long the clip is.
    #[must_use]
    pub fn duration(&self) -> std::time::Duration {
        if self.sample_rate == 0 {
            return std::time::Duration::ZERO;
        }
        // Integer arithmetic rather than floating point: a duration computed in f64 is
        // exact for any clip that fits in memory, but saying so requires a proof that
        // nanoseconds do not.
        std::time::Duration::from_nanos(
            (self.samples.len() as u64).saturating_mul(1_000_000_000) / u64::from(self.sample_rate),
        )
    }
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// Read a WAV file.
pub fn read_wav(mut source: impl Read) -> Result<Wav, WavError> {
    let mut bytes = Vec::new();
    source.read_to_end(&mut bytes)?;

    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(WavError::NotWave);
    }

    let mut sample_rate = None;
    let mut samples = None;
    let mut offset = 12usize;

    // Chunks are walked rather than assumed in order: real files carry LIST and fact chunks
    // between fmt and data, and a reader that assumes a fixed 44-byte header reads metadata as
    // audio.
    while offset + 8 <= bytes.len() {
        let id = bytes.get(offset..offset + 4).ok_or(WavError::Truncated)?;
        let size = u32_at(&bytes, offset + 4).ok_or(WavError::Truncated)? as usize;
        let body_at = offset + 8;
        let body = bytes
            .get(body_at..body_at + size)
            .ok_or(WavError::Truncated)?;

        match id {
            b"fmt " => {
                let format = u16_at(body, 0).ok_or(WavError::Truncated)?;
                let channels = u16_at(body, 2).ok_or(WavError::Truncated)?;
                let rate = u32_at(body, 4).ok_or(WavError::Truncated)?;
                let bits = u16_at(body, 14).ok_or(WavError::Truncated)?;

                if format != 1 {
                    return Err(WavError::Unsupported(format!(
                        "format tag {format}; only uncompressed PCM is handled"
                    )));
                }
                if channels != 1 {
                    return Err(WavError::Unsupported(format!(
                        "{channels} channels; mono only"
                    )));
                }
                if bits != 16 {
                    return Err(WavError::Unsupported(format!("{bits}-bit; 16-bit only")));
                }
                sample_rate = Some(rate);
            }
            b"data" => {
                samples = Some(
                    body.chunks_exact(2)
                        .map(|pair| {
                            i16::from_le_bytes([
                                pair.first().copied().unwrap_or(0),
                                pair.get(1).copied().unwrap_or(0),
                            ])
                        })
                        .collect::<Vec<i16>>(),
                );
            }
            _ => {}
        }

        // Chunks are word-aligned: an odd size is followed by a pad byte that is not part of
        // it. Ignoring the pad shifts every later chunk by one.
        offset = body_at + size + (size % 2);
    }

    Ok(Wav {
        sample_rate: sample_rate.ok_or(WavError::NotWave)?,
        samples: samples.ok_or(WavError::NotWave)?,
    })
}

/// Write a WAV file.
pub fn write_wav(mut sink: impl Write, wav: &Wav) -> Result<(), WavError> {
    let data_len = u32::try_from(wav.samples.len() * 2).unwrap_or(u32::MAX);
    let byte_rate = wav.sample_rate * 2;

    sink.write_all(b"RIFF")?;
    sink.write_all(&(36 + data_len).to_le_bytes())?;
    sink.write_all(b"WAVE")?;

    sink.write_all(b"fmt ")?;
    sink.write_all(&16u32.to_le_bytes())?;
    sink.write_all(&1u16.to_le_bytes())?; // PCM
    sink.write_all(&1u16.to_le_bytes())?; // mono
    sink.write_all(&wav.sample_rate.to_le_bytes())?;
    sink.write_all(&byte_rate.to_le_bytes())?;
    sink.write_all(&2u16.to_le_bytes())?; // block align
    sink.write_all(&16u16.to_le_bytes())?; // bits per sample

    sink.write_all(b"data")?;
    sink.write_all(&data_len.to_le_bytes())?;
    for sample in &wav.samples {
        sink.write_all(&sample.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn tone(samples: usize) -> Wav {
        Wav::narrowband(
            (0..samples)
                .map(|i| {
                    let phase = f64::from(u32::try_from(i).unwrap_or(0))
                        * 2.0
                        * std::f64::consts::PI
                        * 440.0
                        / 8000.0;
                    let value = (phase.sin() * 16000.0).round();
                    i16::try_from(value as i32).unwrap_or(0)
                })
                .collect(),
        )
    }

    #[test]
    fn a_clip_survives_a_round_trip_exactly() {
        let original = tone(800);
        let mut buffer = Vec::new();
        write_wav(&mut buffer, &original).expect("writes");
        let read = read_wav(buffer.as_slice()).expect("reads");
        assert_eq!(read, original);
    }

    #[test]
    fn the_duration_follows_the_sample_count_and_rate() {
        assert_eq!(tone(8000).duration(), std::time::Duration::from_secs(1));
        assert_eq!(tone(4000).duration(), std::time::Duration::from_millis(500));
    }

    /// Real files put LIST and fact chunks between fmt and data. A reader that assumes a
    /// fixed 44-byte header reads that metadata as audio.
    #[test]
    fn chunks_between_fmt_and_data_are_skipped() {
        let clip = Wav::narrowband(vec![1, -1, 2, -2]);
        let mut buffer = Vec::new();
        write_wav(&mut buffer, &clip).expect("writes");

        // Splice a LIST chunk in before `data`.
        let data_at = buffer
            .windows(4)
            .position(|w| w == b"data")
            .expect("a data chunk");
        let mut spliced = buffer[..data_at].to_vec();
        spliced.extend_from_slice(b"LIST");
        spliced.extend_from_slice(&8u32.to_le_bytes());
        spliced.extend_from_slice(b"INFOxxxx");
        spliced.extend_from_slice(&buffer[data_at..]);
        // Fix the RIFF size so the file stays consistent.
        let total = u32::try_from(spliced.len() - 8).expect("fits");
        spliced[4..8].copy_from_slice(&total.to_le_bytes());

        assert_eq!(read_wav(spliced.as_slice()).expect("reads"), clip);
    }

    /// An odd-sized chunk is followed by a pad byte that is not part of it. Ignoring the pad
    /// shifts every later chunk by one and turns the audio into noise.
    #[test]
    fn an_odd_sized_chunk_is_padded_to_a_word_boundary() {
        let clip = Wav::narrowband(vec![7, -7]);
        let mut buffer = Vec::new();
        write_wav(&mut buffer, &clip).expect("writes");
        let data_at = buffer
            .windows(4)
            .position(|w| w == b"data")
            .expect("a data chunk");

        let mut spliced = buffer[..data_at].to_vec();
        spliced.extend_from_slice(b"ODDC");
        spliced.extend_from_slice(&3u32.to_le_bytes());
        spliced.extend_from_slice(b"abc\0"); // three bytes plus the pad
        spliced.extend_from_slice(&buffer[data_at..]);
        let total = u32::try_from(spliced.len() - 8).expect("fits");
        spliced[4..8].copy_from_slice(&total.to_le_bytes());

        assert_eq!(read_wav(spliced.as_slice()).expect("reads"), clip);
    }

    /// A format this crate cannot handle is refused by name. Reading it as if it were
    /// 16-bit mono would produce audio that is almost right, which is far harder to diagnose.
    #[test]
    fn an_unsupported_format_is_refused_rather_than_misread() {
        let mut stereo = Vec::new();
        write_wav(&mut stereo, &Wav::narrowband(vec![1, 2, 3, 4])).expect("writes");
        // Claim two channels.
        let fmt_at = stereo
            .windows(4)
            .position(|w| w == b"fmt ")
            .expect("a fmt chunk");
        stereo[fmt_at + 10..fmt_at + 12].copy_from_slice(&2u16.to_le_bytes());

        match read_wav(stereo.as_slice()) {
            Err(WavError::Unsupported(message)) => assert!(message.contains("channels")),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_file_that_is_not_a_wave_is_refused() {
        assert!(matches!(
            read_wav(b"this is not a wav file at all".as_slice()),
            Err(WavError::NotWave)
        ));
    }

    #[test]
    fn a_truncated_chunk_is_an_error_not_a_partial_read() {
        let clip = Wav::narrowband(vec![1, 2, 3, 4]);
        let mut buffer = Vec::new();
        write_wav(&mut buffer, &clip).expect("writes");
        buffer.truncate(buffer.len() - 4);
        assert!(matches!(
            read_wav(buffer.as_slice()),
            Err(WavError::Truncated)
        ));
    }

    #[test]
    fn an_empty_clip_round_trips() {
        let clip = Wav::narrowband(Vec::new());
        let mut buffer = Vec::new();
        write_wav(&mut buffer, &clip).expect("writes");
        assert_eq!(read_wav(buffer.as_slice()).expect("reads"), clip);
    }
}
