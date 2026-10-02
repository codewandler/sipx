//! The supervised worker's framed protocol (`docs/specs/call-dsp-graph.md` §7.4).
//!
//! Sans-I/O in the sense that matters: the decisions are made on octets, and the two functions that
//! touch a pipe do nothing but fill or drain a buffer whose size was already decided here.
//!
//! One rule shapes the whole module. **A message is refused by its type, never by its length
//! prefix.** The type octet decides the arithmetic — how many octets are fixed, and how the
//! trailing samples relate to the count field inside them — and the declared length is then checked
//! against that. Sizing a read or an allocation from a number the peer chose is how a malformed
//! message becomes a memory fault, so the ceiling is compared against the header alone, before a
//! single octet of payload is read.
//!
//! The rule has a second half, and `M-122`'s fuzz campaign is what asked for it. The ceiling a
//! length is compared against is itself declared by a peer — it is the `Hello`'s `max_samples` —
//! and on the worker's side that peer is whatever wrote to its standard input. So the ceiling is
//! bounded too, by [`MAX_FRAME_SAMPLES`]: a frame larger than that is refused by the processor
//! contract before it could ever be offered to a stage, which makes a `Hello` promising one a
//! statement this version cannot honour rather than a large configuration.

use std::io::{self, Read, Write};

use sipx_audio::dsp::{MAX_FRAME_SAMPLES, StreamFormat};

use crate::processing::{AudioDirection, DiscontinuityKind};

use super::worker::WorkerResult;

/// Every message opens with this (§7.4).
const MAGIC: [u8; 2] = [0x53, 0x44];

/// Magic, type, reserved, length.
const HEADER: usize = 8;

/// The protocol this runtime speaks.
pub(crate) const VERSION: u16 = 1;

const TYPE_HELLO: u8 = 0x01;
const TYPE_FRAME: u8 = 0x02;
const TYPE_RESULT: u8 = 0x81;

/// version, direction, sample rate, channels, `max_samples`.
const HELLO_FIXED: u32 = 12;
/// sequence, position, discontinuity, samples.
const FRAME_FIXED: u32 = 21;
/// sequence, outcome, samples.
const RESULT_FIXED: u32 = 13;

/// Why a message off a worker pipe was refused (`docs/specs/call-dsp-graph.md` §7.4).
///
/// Every variant is a decision about octets and none of them is a decision about audio: a result
/// that is well formed here and carries the wrong number of positions is
/// [`BypassCause::MalformedResult`](super::BypassCause::MalformedResult) at the contract level
/// (§7.2), which is a different thing and is counted in a different place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WorkerProtocolError {
    /// The first two octets are not the protocol's magic, so this is not a message at all.
    #[error("a worker message does not open with the protocol's magic")]
    BadMagic,
    /// The reserved octet is set. Reserved means zero until a version says otherwise.
    #[error("a worker message sets its reserved octet to {value:#04x}")]
    Reserved {
        /// What it was set to.
        value: u8,
    },
    /// A type octet this version does not define.
    #[error("a worker message declares type {kind:#04x}, which this version does not define")]
    UnknownType {
        /// The type octet.
        kind: u8,
    },
    /// The declared length is below the type's own fixed payload.
    #[error(
        "a {kind:#04x} message declares {declared} payload octets, below its fixed {fixed} octets"
    )]
    Undersized {
        /// The type octet.
        kind: u8,
        /// What the header declared.
        declared: u32,
        /// What the type requires before any samples.
        fixed: u32,
    },
    /// The declared length is above what the type can carry at the negotiated ceiling.
    ///
    /// Decided from the header alone, so no declared length ever sizes a buffer.
    #[error(
        "a {kind:#04x} message declares {declared} payload octets against a ceiling of {limit}"
    )]
    Oversized {
        /// The type octet.
        kind: u8,
        /// What the header declared.
        declared: u32,
        /// The most that type may carry at the negotiated `max_samples`.
        limit: u32,
    },
    /// The payload's own sample count does not account for the declared length exactly.
    #[error(
        "a {kind:#04x} message carrying {samples} samples does not account for its {declared} \
         declared octets"
    )]
    LengthMismatch {
        /// The type octet.
        kind: u8,
        /// What the header declared.
        declared: u32,
        /// What the payload's own count field said.
        samples: u32,
    },
    /// The stream ended inside a message.
    #[error("the worker stream ended inside a message")]
    Truncated,
    /// A field carries a value this version does not define.
    #[error("a worker message carries {value} in `{field}`, which this version does not define")]
    Value {
        /// The field, spelled as §7.4 spells it.
        field: &'static str,
        /// What it carried.
        value: u32,
    },
    /// The peer speaks a protocol version this runtime does not.
    #[error("the worker protocol is version {theirs}, and this runtime speaks {ours}")]
    Version {
        /// What this runtime speaks.
        ours: u16,
        /// What the peer declared.
        theirs: u16,
    },
}

/// What ended a read: a refusal, or the pipe itself.
#[derive(Debug)]
pub(crate) enum Fault {
    /// The peer sent something §7.4 refuses.
    Refused(WorkerProtocolError),
    /// The pipe failed. A closed pipe is reported as `Ok(None)` and never as this.
    Io(io::Error),
}

impl From<WorkerProtocolError> for Fault {
    fn from(error: WorkerProtocolError) -> Self {
        Self::Refused(error)
    }
}

impl From<io::Error> for Fault {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Fault> for io::Error {
    fn from(fault: Fault) -> Self {
        match fault {
            Fault::Refused(error) => Self::new(io::ErrorKind::InvalidData, error),
            Fault::Io(error) => error,
        }
    }
}

/// One decoded message, with its samples already in the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Incoming {
    /// The runtime's opening statement of what every later frame is (§7.4).
    Hello {
        /// Which side of the call this worker's frames belong to.
        direction: AudioDirection,
        /// The rate and channel count they are in.
        format: StreamFormat,
        /// The most interleaved samples any one frame may carry.
        max_samples: u32,
    },
    /// One frame offered to the worker.
    Frame {
        /// The frame's identity, echoed back in its result.
        sequence: u64,
        /// The first position's index in the current epoch.
        position: u64,
        /// The break immediately before this frame, if there was one.
        discontinuity: Option<DiscontinuityKind>,
    },
    /// One answer from the worker.
    Result {
        /// Which frame this answers.
        sequence: u64,
        /// What the worker did with it.
        outcome: WorkerResult,
    },
}

/// The most octets a message of `kind` may carry at a ceiling of `max_samples`.
///
/// `max_samples` is bounded by [`MAX_FRAME_SAMPLES`] whatever the caller passes, so the buffer this
/// sizes has a bound that holds without trusting either the peer or the call site. A negotiated
/// ceiling above that is outside the processor contract's own domain and could never carry a frame
/// a stage would accept.
const fn ceiling(kind: u8, max_samples: u32) -> Option<(u32, u32)> {
    let fixed = match kind {
        TYPE_HELLO => return Some((HELLO_FIXED, HELLO_FIXED)),
        TYPE_FRAME => FRAME_FIXED,
        TYPE_RESULT => RESULT_FIXED,
        _ => return None,
    };
    let bounded = if max_samples > MAX_FRAME_SAMPLES {
        MAX_FRAME_SAMPLES
    } else {
        max_samples
    };
    Some((fixed, fixed.saturating_add(bounded.saturating_mul(2))))
}

/// The most octets any message may carry, whatever a peer declared (`M-122`).
///
/// The invariant `docs/specs/call-dsp-graph.md` §7.4 states as "no declared length ever sizes a
/// buffer", as one number: no read this module performs is larger than this, for any octets and any
/// negotiated ceiling.
pub(crate) const MESSAGE_LIMIT: u32 = FRAME_FIXED.saturating_add(MAX_FRAME_SAMPLES * 2);

/// Read one message, decoding its samples into `samples`.
///
/// `payload` is the caller's reusable octet buffer and never grows beyond the ceiling `max_samples`
/// implies. Returns `Ok(None)` when the stream ends cleanly between messages, which is the pipe's
/// own way of saying the peer is gone and is not a refusal.
pub(crate) fn read_message<R: Read>(
    reader: &mut R,
    payload: &mut Vec<u8>,
    samples: &mut Vec<i16>,
    max_samples: u32,
) -> Result<Option<Incoming>, Fault> {
    let mut header = [0_u8; HEADER];
    if !read_header(reader, &mut header)? {
        return Ok(None);
    }
    if header.first_chunk::<2>() != Some(&MAGIC) {
        return Err(WorkerProtocolError::BadMagic.into());
    }
    let kind = octet(&header, 2)?;
    let reserved = octet(&header, 3)?;
    if reserved != 0 {
        return Err(WorkerProtocolError::Reserved { value: reserved }.into());
    }
    let declared = quad(&header, 4)?;
    let Some((fixed, limit)) = ceiling(kind, max_samples) else {
        return Err(WorkerProtocolError::UnknownType { kind }.into());
    };
    if declared < fixed {
        return Err(WorkerProtocolError::Undersized {
            kind,
            declared,
            fixed,
        }
        .into());
    }
    // §7.4: the ceiling is decided here, from the header alone, so the resize below is bounded by
    // what this runtime negotiated and never by what the peer declared.
    if declared > limit {
        return Err(WorkerProtocolError::Oversized {
            kind,
            declared,
            limit,
        }
        .into());
    }
    payload.clear();
    payload.resize(declared as usize, 0);
    reader.read_exact(payload).map_err(truncated)?;
    decode(kind, declared, fixed, payload, samples).map(Some)
}

/// Fill `header`, distinguishing a clean end of stream from a truncated message.
fn read_header<R: Read>(reader: &mut R, header: &mut [u8; HEADER]) -> Result<bool, Fault> {
    let mut filled = 0;
    while filled < HEADER {
        let Some(rest) = header.get_mut(filled..) else {
            return Err(WorkerProtocolError::Truncated.into());
        };
        match reader.read(rest) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(WorkerProtocolError::Truncated.into()),
            Ok(read) => filled = filled.saturating_add(read),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Fault::Io(error)),
        }
    }
    Ok(true)
}

/// Turn an end of stream in the middle of a payload into the refusal it is.
fn truncated(error: io::Error) -> Fault {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        WorkerProtocolError::Truncated.into()
    } else {
        Fault::Io(error)
    }
}

fn decode(
    kind: u8,
    declared: u32,
    fixed: u32,
    payload: &[u8],
    samples: &mut Vec<i16>,
) -> Result<Incoming, Fault> {
    samples.clear();
    match kind {
        TYPE_HELLO => {
            let theirs = pair(payload, 0)?;
            if theirs != VERSION {
                return Err(WorkerProtocolError::Version {
                    ours: VERSION,
                    theirs,
                }
                .into());
            }
            let direction = direction(octet(payload, 2)?)?;
            let rate = quad(payload, 3)?;
            let channels = octet(payload, 7)?;
            let format =
                StreamFormat::new(rate, channels).map_err(|_| WorkerProtocolError::Value {
                    field: "sample rate",
                    value: rate,
                })?;
            // §7.4: the ceiling every later message is checked against, so a value out of the
            // contract's domain is refused here rather than adopted. Left unchecked it is the peer
            // choosing the size of the next read: the frame limit saturates at `u32::MAX`, and so
            // does `fill`'s exact-accounting arithmetic, which stops deciding anything.
            let max_samples = quad(payload, 8)?;
            if max_samples > MAX_FRAME_SAMPLES {
                return Err(WorkerProtocolError::Value {
                    field: "max_samples",
                    value: max_samples,
                }
                .into());
            }
            Ok(Incoming::Hello {
                direction,
                format,
                max_samples,
            })
        }
        TYPE_FRAME => {
            let sequence = octuple(payload, 0)?;
            let position = octuple(payload, 8)?;
            let discontinuity = discontinuity(octet(payload, 16)?)?;
            fill(kind, declared, fixed, payload, 17, samples)?;
            Ok(Incoming::Frame {
                sequence,
                position,
                discontinuity,
            })
        }
        TYPE_RESULT => {
            let sequence = octuple(payload, 0)?;
            let outcome = outcome(octet(payload, 8)?)?;
            fill(kind, declared, fixed, payload, 9, samples)?;
            Ok(Incoming::Result { sequence, outcome })
        }
        _ => Err(WorkerProtocolError::UnknownType { kind }.into()),
    }
}

/// Check the payload's own count against the declared length, then decode the samples.
fn fill(
    kind: u8,
    declared: u32,
    fixed: u32,
    payload: &[u8],
    at: usize,
    samples: &mut Vec<i16>,
) -> Result<(), Fault> {
    let count = quad(payload, at)?;
    // §7.4: the count field and the length prefix must agree exactly. They are two statements of
    // the same fact, and a peer that makes them disagree has told us nothing we can act on.
    if fixed.saturating_add(count.saturating_mul(2)) != declared {
        return Err(WorkerProtocolError::LengthMismatch {
            kind,
            declared,
            samples: count,
        }
        .into());
    }
    let Some(body) = payload.get(at.saturating_add(4)..) else {
        return Err(WorkerProtocolError::Truncated.into());
    };
    samples.clear();
    samples.reserve(body.len() / 2);
    for pair in body.as_chunks::<2>().0 {
        samples.push(i16::from_be_bytes(*pair));
    }
    Ok(())
}

fn octet(bytes: &[u8], at: usize) -> Result<u8, Fault> {
    bytes
        .get(at)
        .copied()
        .ok_or_else(|| WorkerProtocolError::Truncated.into())
}

fn pair(bytes: &[u8], at: usize) -> Result<u16, Fault> {
    let slice = bytes.get(at..).and_then(<[u8]>::first_chunk::<2>);
    slice
        .map(|octets| u16::from_be_bytes(*octets))
        .ok_or_else(|| WorkerProtocolError::Truncated.into())
}

fn quad(bytes: &[u8], at: usize) -> Result<u32, Fault> {
    let slice = bytes.get(at..).and_then(<[u8]>::first_chunk::<4>);
    slice
        .map(|octets| u32::from_be_bytes(*octets))
        .ok_or_else(|| WorkerProtocolError::Truncated.into())
}

fn octuple(bytes: &[u8], at: usize) -> Result<u64, Fault> {
    let slice = bytes.get(at..).and_then(<[u8]>::first_chunk::<8>);
    slice
        .map(|octets| u64::from_be_bytes(*octets))
        .ok_or_else(|| WorkerProtocolError::Truncated.into())
}

const fn direction(value: u8) -> Result<AudioDirection, WorkerProtocolError> {
    match value {
        1 => Ok(AudioDirection::Inbound),
        2 => Ok(AudioDirection::Outbound),
        other => Err(WorkerProtocolError::Value {
            field: "direction",
            value: other as u32,
        }),
    }
}

const fn direction_octet(direction: AudioDirection) -> u8 {
    match direction {
        AudioDirection::Inbound => 1,
        AudioDirection::Outbound => 2,
    }
}

const fn discontinuity(value: u8) -> Result<Option<DiscontinuityKind>, WorkerProtocolError> {
    match value {
        0 => Ok(None),
        1 => Ok(Some(DiscontinuityKind::Loss)),
        2 => Ok(Some(DiscontinuityKind::Overflow)),
        3 => Ok(Some(DiscontinuityKind::Realign)),
        other => Err(WorkerProtocolError::Value {
            field: "discontinuity",
            value: other as u32,
        }),
    }
}

const fn discontinuity_octet(kind: Option<DiscontinuityKind>) -> u8 {
    match kind {
        None => 0,
        Some(DiscontinuityKind::Overflow) => 2,
        Some(DiscontinuityKind::Realign) => 3,
        // The seam's kinds may grow (`call-audio-seam.md` §7). A kind this version has no octet for
        // is carried as loss rather than as none: a worker that is told nothing broke resets
        // nothing, and under-reporting a break is the failure that produces audio nobody can
        // explain.
        Some(DiscontinuityKind::Loss | _) => 1,
    }
}

const fn outcome(value: u8) -> Result<WorkerResult, WorkerProtocolError> {
    match value {
        0 => Ok(WorkerResult::Produced),
        1 => Ok(WorkerResult::Withheld),
        2 => Ok(WorkerResult::Failed),
        other => Err(WorkerProtocolError::Value {
            field: "outcome",
            value: other as u32,
        }),
    }
}

const fn outcome_octet(outcome: WorkerResult) -> u8 {
    match outcome {
        WorkerResult::Produced => 0,
        WorkerResult::Withheld => 1,
        WorkerResult::Failed => 2,
    }
}

/// Build a message into `out`, replacing whatever was there.
fn frame_message(out: &mut Vec<u8>, kind: u8, fixed: u32, samples: &[i16]) {
    let count = u32::try_from(samples.len()).unwrap_or(u32::MAX);
    let declared = fixed.saturating_add(count.saturating_mul(2));
    out.clear();
    out.extend_from_slice(&MAGIC);
    out.push(kind);
    out.push(0);
    out.extend_from_slice(&declared.to_be_bytes());
}

/// Write one `Hello` (§7.4).
pub(crate) fn write_hello<W: Write>(
    writer: &mut W,
    out: &mut Vec<u8>,
    direction: AudioDirection,
    format: StreamFormat,
    max_samples: u32,
) -> io::Result<()> {
    frame_message(out, TYPE_HELLO, HELLO_FIXED, &[]);
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.push(direction_octet(direction));
    out.extend_from_slice(&format.sample_rate().to_be_bytes());
    out.push(format.channels());
    out.extend_from_slice(&max_samples.to_be_bytes());
    writer.write_all(out)?;
    writer.flush()
}

/// Write one `Frame` (§7.4).
pub(crate) fn write_frame<W: Write>(
    writer: &mut W,
    out: &mut Vec<u8>,
    sequence: u64,
    position: u64,
    discontinuity: Option<DiscontinuityKind>,
    samples: &[i16],
) -> io::Result<()> {
    frame_message(out, TYPE_FRAME, FRAME_FIXED, samples);
    out.extend_from_slice(&sequence.to_be_bytes());
    out.extend_from_slice(&position.to_be_bytes());
    out.push(discontinuity_octet(discontinuity));
    push_samples(out, samples);
    writer.write_all(out)?;
    writer.flush()
}

/// Write one `Result` (§7.4).
pub(crate) fn write_result<W: Write>(
    writer: &mut W,
    out: &mut Vec<u8>,
    sequence: u64,
    outcome: WorkerResult,
    samples: &[i16],
) -> io::Result<()> {
    frame_message(out, TYPE_RESULT, RESULT_FIXED, samples);
    out.extend_from_slice(&sequence.to_be_bytes());
    out.push(outcome_octet(outcome));
    push_samples(out, samples);
    writer.write_all(out)?;
    writer.flush()
}

fn push_samples(out: &mut Vec<u8>, samples: &[i16]) {
    out.extend_from_slice(
        &u32::try_from(samples.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for sample in samples {
        out.extend_from_slice(&sample.to_be_bytes());
    }
}

/// The octets a message of this many samples occupies, for sizing a buffer once.
pub(crate) const fn message_capacity(max_samples: u32) -> usize {
    HEADER
        .saturating_add(FRAME_FIXED as usize)
        .saturating_add((max_samples as usize).saturating_mul(2))
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

    fn narrowband() -> StreamFormat {
        StreamFormat::new(8_000, 1).unwrap()
    }

    fn round_trip(samples: &[i16], max: u32) -> (Incoming, Vec<i16>) {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_frame(
            &mut wire,
            &mut out,
            7,
            1_600,
            Some(DiscontinuityKind::Overflow),
            samples,
        )
        .unwrap();
        let mut payload = Vec::new();
        let mut decoded = Vec::new();
        let message = read_message(&mut wire.as_slice(), &mut payload, &mut decoded, max)
            .unwrap()
            .unwrap();
        (message, decoded)
    }

    /// §7.4: every field of a frame survives the wire, samples included.
    #[test]
    fn a_frame_carries_its_metadata_and_its_samples() {
        let samples = vec![-32_768, -1, 0, 1, 32_767];
        let (message, decoded) = round_trip(&samples, 8);
        assert_eq!(
            message,
            Incoming::Frame {
                sequence: 7,
                position: 1_600,
                discontinuity: Some(DiscontinuityKind::Overflow),
            }
        );
        assert_eq!(decoded, samples);
    }

    /// §7.4: a `Hello` carries the direction, the format and the ceiling.
    #[test]
    fn a_hello_carries_the_direction_the_format_and_the_ceiling() {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_hello(
            &mut wire,
            &mut out,
            AudioDirection::Inbound,
            narrowband(),
            160,
        )
        .unwrap();
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let message = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 0)
            .unwrap()
            .unwrap();
        assert_eq!(
            message,
            Incoming::Hello {
                direction: AudioDirection::Inbound,
                format: narrowband(),
                max_samples: 160,
            }
        );
    }

    /// GRAPH-19: a length above the ceiling is refused from the header, before any payload is read.
    #[test]
    fn an_oversized_message_is_refused_from_its_header_alone() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&MAGIC);
        wire.push(TYPE_RESULT);
        wire.push(0);
        wire.extend_from_slice(&1_000_000_u32.to_be_bytes());
        // No payload follows at all: a decoder that trusted the prefix would block or allocate a
        // megabyte before it discovered that.
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(
                fault,
                Fault::Refused(WorkerProtocolError::Oversized {
                    kind: TYPE_RESULT,
                    declared: 1_000_000,
                    limit: 333,
                })
            ),
            "{fault:?}"
        );
        assert!(payload.is_empty(), "nothing of that size was allocated");
    }

    /// §7.4's other end of the same rule (`M-68`): a length *below* the type's fixed payload is
    /// refused from the header too.
    ///
    /// The ceiling gets the attention because a huge prefix is the memory fault; this is the one
    /// that would have the decoder read a fixed payload out of a buffer shorter than it, and it is
    /// refused for the same reason — the type decides the arithmetic and the prefix is checked
    /// against it, never the reverse.
    #[test]
    fn an_undersized_message_is_refused_from_its_header_alone() {
        for (kind, fixed) in [
            (TYPE_HELLO, HELLO_FIXED),
            (TYPE_FRAME, FRAME_FIXED),
            (TYPE_RESULT, RESULT_FIXED),
        ] {
            let mut wire = Vec::new();
            wire.extend_from_slice(&MAGIC);
            wire.push(kind);
            wire.push(0);
            wire.extend_from_slice(&(fixed - 1).to_be_bytes());
            // The payload that follows is as short as the header claimed, so a decoder that read
            // the prefix and then reached for its fixed fields would be reading past it.
            wire.resize(HEADER + fixed as usize - 1, 0);
            let mut payload = Vec::new();
            let mut samples = Vec::new();
            let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
                .expect_err("refused");
            assert!(
                matches!(
                    fault,
                    Fault::Refused(WorkerProtocolError::Undersized {
                        kind: refused,
                        declared,
                        fixed: required,
                    }) if refused == kind && declared == fixed - 1 && required == fixed
                ),
                "{kind:#04x}: {fault:?}"
            );
            assert!(payload.is_empty(), "no payload was read");
        }
    }

    /// §7.4: the count field and the length prefix must agree, and neither is believed alone.
    #[test]
    fn a_count_that_does_not_account_for_the_length_is_refused() {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_result(&mut wire, &mut out, 1, WorkerResult::Produced, &[1, 2, 3]).unwrap();
        // Keep the declared length; claim one sample fewer.
        wire[8 + 9..8 + 13].copy_from_slice(&2_u32.to_be_bytes());
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(
                fault,
                Fault::Refused(WorkerProtocolError::LengthMismatch {
                    kind: TYPE_RESULT,
                    declared: 19,
                    samples: 2,
                })
            ),
            "{fault:?}"
        );
    }

    /// §7.4: a type this version does not define is refused by type and not read past.
    #[test]
    fn an_unknown_type_is_refused() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&MAGIC);
        wire.push(0x42);
        wire.push(0);
        wire.extend_from_slice(&0_u32.to_be_bytes());
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(
                fault,
                Fault::Refused(WorkerProtocolError::UnknownType { kind: 0x42 })
            ),
            "{fault:?}"
        );
    }

    /// §7.4: anything that does not open with the magic is not a message.
    #[test]
    fn a_stream_that_is_not_this_protocol_is_refused() {
        let noise = b"running 1 test\n";
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut noise.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(fault, Fault::Refused(WorkerProtocolError::BadMagic)),
            "{fault:?}"
        );
    }

    /// §7.4: a clean end between messages is the peer being gone, not a refusal.
    #[test]
    fn a_clean_end_of_stream_is_not_a_refusal() {
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let end = read_message(&mut [].as_slice(), &mut payload, &mut samples, 160).unwrap();
        assert_eq!(end, None);
    }

    /// §7.4: a stream that stops inside a message is truncated, which is a refusal.
    #[test]
    fn a_stream_that_stops_inside_a_message_is_truncated() {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_result(&mut wire, &mut out, 1, WorkerResult::Produced, &[1, 2, 3]).unwrap();
        wire.truncate(12);
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(fault, Fault::Refused(WorkerProtocolError::Truncated)),
            "{fault:?}"
        );
    }

    /// §7.4: a reserved octet is reserved.
    #[test]
    fn a_set_reserved_octet_is_refused() {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_result(&mut wire, &mut out, 1, WorkerResult::Produced, &[]).unwrap();
        wire[3] = 1;
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(
                fault,
                Fault::Refused(WorkerProtocolError::Reserved { value: 1 })
            ),
            "{fault:?}"
        );
    }

    /// §7.4: a ceiling above the contract's own largest frame is a value this version does not
    /// define (`M-122`).
    ///
    /// The other end of "no declared length ever sizes a buffer". A `Hello`'s `max_samples` *is*
    /// the ceiling every later message is checked against, so a peer that declares one this
    /// runtime cannot honour has chosen the size of the next read: at `u32::MAX` the frame limit
    /// saturates, `payload.resize` is handed four gigabytes, and `fill`'s exact-accounting rule
    /// saturates with it and stops deciding anything.
    #[test]
    fn a_ceiling_above_the_contracts_largest_frame_is_refused() {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_hello(
            &mut wire,
            &mut out,
            AudioDirection::Inbound,
            narrowband(),
            160,
        )
        .unwrap();
        let at = HEADER + 8;
        wire[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault =
            read_message(&mut wire.as_slice(), &mut payload, &mut samples, 0).expect_err("refused");
        assert!(
            matches!(
                fault,
                Fault::Refused(WorkerProtocolError::Value {
                    field: "max_samples",
                    value: u32::MAX,
                })
            ),
            "{fault:?}"
        );
    }

    /// §7.4: a version this runtime does not speak is refused rather than guessed at.
    #[test]
    fn a_version_this_runtime_does_not_speak_is_refused() {
        let mut wire = Vec::new();
        let mut out = Vec::new();
        write_hello(
            &mut wire,
            &mut out,
            AudioDirection::Outbound,
            narrowband(),
            160,
        )
        .unwrap();
        wire[8..10].copy_from_slice(&9_u16.to_be_bytes());
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let fault = read_message(&mut wire.as_slice(), &mut payload, &mut samples, 160)
            .expect_err("refused");
        assert!(
            matches!(
                fault,
                Fault::Refused(WorkerProtocolError::Version { ours: 1, theirs: 9 })
            ),
            "{fault:?}"
        );
    }

    /// §7.4: every outcome and every discontinuity kind has exactly one octet.
    #[test]
    fn every_enumerated_field_round_trips() {
        for value in [
            WorkerResult::Produced,
            WorkerResult::Withheld,
            WorkerResult::Failed,
        ] {
            assert_eq!(outcome(outcome_octet(value)).unwrap(), value);
        }
        for value in [
            None,
            Some(DiscontinuityKind::Loss),
            Some(DiscontinuityKind::Overflow),
            Some(DiscontinuityKind::Realign),
        ] {
            assert_eq!(discontinuity(discontinuity_octet(value)).unwrap(), value);
        }
        for value in [AudioDirection::Inbound, AudioDirection::Outbound] {
            assert_eq!(direction(direction_octet(value)).unwrap(), value);
        }
    }
}
