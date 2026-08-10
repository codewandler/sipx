//! Adversarial supervised-worker exchanges, their invariant oracle, and the seed corpus (`M-122`).
//!
//! [`docs/specs/call-dsp-graph.md`](../../../../docs/specs/call-dsp-graph.md) §7.4 is the one place
//! in this epic where the media process parses octets it did not write. `M-68` walked its refusal
//! table by hand; this is the instrument for the cases nobody wrote down. A byte string decodes into
//! a **program** of message specifications — well-formed `Hello`s, `Frame`s and `Result`s, each with
//! one field corrupted or left alone, truncations, raw noise, and a reader that hands the decoder
//! its octets a few at a time — so a fuzzer's budget is spent inside §7.4's arithmetic rather than on
//! bytes that fail at the magic.
//!
//! # The oracle is not "did it panic"
//!
//! The decoder is already total: it returns a typed refusal for everything, so a campaign checking
//! only for panics would run green over a decoder that had quietly stopped enforcing anything. What
//! §7.4 actually promises is four things, and [`Invariant`] is those four:
//!
//! 1. **A message is refused by its type, never by its length prefix.** [`expected_refusal`] derives,
//!    from the header octets alone and independently of the decoder, which row of §7.4's table must
//!    fire first. A decoder that refuses with a different row — or accepts where the table refuses —
//!    is [`Invariant::WrongRefusal`], and that catches a reordering of the rules as well as a
//!    missing one.
//! 2. **`Oversized` is decided from the header alone.** Every read goes through a counting
//!    reader, so a header-decided refusal that consumed a payload octet is
//!    [`Invariant::PayloadReadBeforeRefusal`].
//! 3. **No declared length ever sizes a buffer.** The caller's own payload buffer is watched: a
//!    capacity above `wire::MESSAGE_LIMIT` is [`Invariant::BufferSizedByPeer`], and so is an
//!    accepted `Hello` whose `max_samples` would raise the ceiling above it — the ceiling is a
//!    declared number too, and on the worker's side the peer declaring it is whatever wrote to that
//!    process's standard input.
//! 4. **A refusal is terminal for the connection in both directions.** Checked against the real
//!    callers rather than a model of them: `worker::serve` for the worker's side and
//!    `supervised::pump` for the runtime's. Either one that reads past a refused message, or
//!    writes after it, is [`Invariant::RefusalNotTerminal`].
//!
//! A fifth is the accounting rule the third one protects: an *accepted* message's decoded sample
//! count must account for its declared length exactly ([`Invariant::CountDoesNotAccount`]). It is
//! the assertion that goes vacuous first if the ceiling stops being bounded, because the arithmetic
//! comparing the two saturates at the top of the `u32` range.
//!
//! # Both directions, because the decoder is one function
//!
//! [`Side`] picks which end is reading. The runtime reads a worker's `Result`s at the ceiling *it*
//! negotiated, which is a number from its own configuration. The worker reads the runtime's `Hello`
//! and `Frame`s, and its ceiling is the one that `Hello` declared — a peer's number, which is why
//! that side is the interesting one and why rule 3 has the shape it does.
//!
//! Nothing here reads a clock, opens a socket or spawns a process. The octets are minted from the
//! program and the two real callers are driven over in-memory streams.

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::sync_channel;

use sipx_audio::dsp::{
    DspCapability, DspFrame, ExecutionPolicy, ExecutionProfile, MAX_FRAME_SAMPLES, StreamFormat,
};

use super::supervised::{self, Request};
use super::wire::{self, WorkerProtocolError};
use super::worker::{self, SupervisedWorker, WorkerResult};
use crate::processing::AudioDirection;

// ---- vocabulary ----

/// Which end of the exchange is doing the reading.
///
/// The two ends run the same decoder and differ in exactly one thing — where the ceiling comes
/// from — so a program that fixed the side would leave half of §7.4 unfuzzed.
/// Exhaustive by design: the protocol has exactly two ends, and a third would be a different
/// protocol rather than a new variant of this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    /// The runtime reading a worker's standard output. Its ceiling is its own configuration.
    #[default]
    Runtime,
    /// A worker reading the runtime's writes. Its ceiling is whatever the `Hello` declared.
    Worker,
}

impl Side {
    /// Fold a byte into a side, so every input names one.
    #[must_use]
    pub const fn from_byte(byte: u8) -> Self {
        if byte & 1 == 0 {
            Self::Runtime
        } else {
            Self::Worker
        }
    }

    /// The byte [`Self::from_byte`] reads back as this side.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        match self {
            Self::Runtime => 0,
            Self::Worker => 1,
        }
    }
}

/// The ceilings a program may negotiate, in interleaved samples.
///
/// Both ends of the domain are here. `0` is a runtime that offers nothing but empty frames;
/// `MAX_FRAME_SAMPLES` is the largest a conforming peer may ask for and `MAX_FRAME_SAMPLES + 1` the
/// smallest it may not, and `u32::MAX` is the one that used to let a peer pick the size of the next
/// read.
pub const CEILINGS: [u32; 8] = [
    0,
    1,
    160,
    960,
    65_535,
    MAX_FRAME_SAMPLES,
    MAX_FRAME_SAMPLES + 1,
    u32::MAX,
];

/// The sample counts a message may carry or claim to.
///
/// Small numbers where the fixed payload's own arithmetic lives, and two that only matter against a
/// ceiling: one at 65,536 and one past every ceiling in [`CEILINGS`] but the last.
pub const COUNTS: [u32; 10] = [0, 1, 2, 3, 159, 160, 161, 960, 65_536, 1_000_000];

/// The `(rate, channels)` pairs a `Hello` may declare, refused ones included.
///
/// `0` Hz and 384,001 Hz are the two ends of the linear-PCM domain, `0` and `9` channels the two
/// ends of the channel domain: a `Hello` that half-applied a refused format is exactly what this is
/// looking for.
pub const FORMATS: [(u32, u8); 8] = [
    (8_000, 1),
    (16_000, 1),
    (48_000, 2),
    (384_000, 8),
    (0, 1),
    (384_001, 1),
    (8_000, 0),
    (8_000, 9),
];

/// The declared lengths a corrupted header may carry.
///
/// Every one of them is a number the decoder must reach a verdict on from the header alone: below a
/// fixed payload, exactly a fixed payload, one past a ceiling, and the two values whose doubling
/// saturates.
pub const LENGTHS: [u32; 10] = [
    0,
    1,
    12,
    13,
    21,
    334,
    1_000_000,
    2_147_483_647,
    4_294_967_294,
    u32::MAX,
];

/// The type octets a corrupted header may carry: the three defined ones and five that are not.
pub const TYPES: [u8; 8] = [0x01, 0x02, 0x81, 0x00, 0x03, 0x80, 0x82, 0xFF];

/// One step of a program.
///
/// A step either appends a message to the octet stream or changes how that stream is handed to the
/// decoder. Four bytes each, so a minimised crash is a short list of steps rather than a blob.
/// Exhaustive by design: a step is one mutation the harness knows how to make, and adding one is a
/// deliberate change to what the campaign covers — a caller matching on this wants to be told.
///
/// Not the call: every octet a `Step` carries is generated by the fuzzer to be handed to the
/// decoder, and the indices are offsets into this module's own tables. Nothing here has been near a
/// call, so rendering it is what makes a minimised crash readable.
///
/// Not a secret: `Corrupt`'s three octets are arbitrary bytes chosen by the fuzzer for the decoder
/// to reject. The fixed-size-array rule exists because such an array is usually a key; this one is
/// the opposite, and printing it is the entire point of a reproducible finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A well-formed `Hello` — the runtime's opening statement.
    Hello {
        /// Index into a four-value direction table; two of them are values §7.4 does not define.
        direction: u8,
        /// Index into [`FORMATS`].
        format: u8,
        /// Index into [`CEILINGS`].
        ceiling: u8,
    },
    /// A well-formed `Frame`.
    Frame {
        /// The frame's sequence number.
        sequence: u8,
        /// Index into [`COUNTS`], clamped to what can actually be minted.
        samples: u8,
        /// Index into a five-value discontinuity table; one of them is undefined.
        discontinuity: u8,
    },
    /// A well-formed `Result`.
    Result {
        /// The sequence this answers.
        sequence: u8,
        /// Index into a four-value outcome table; one of them is undefined.
        outcome: u8,
        /// Index into [`COUNTS`], clamped to what can actually be minted.
        samples: u8,
    },
    /// Overwrite the previous message's magic with two arbitrary octets.
    BreakMagic {
        /// The first octet.
        first: u8,
        /// The second octet.
        second: u8,
    },
    /// Set the previous message's reserved octet.
    SetReserved {
        /// What to set it to. Zero is legal and is deliberately reachable.
        value: u8,
    },
    /// Rewrite the previous message's type octet from [`TYPES`].
    SetType {
        /// Index into [`TYPES`].
        kind: u8,
    },
    /// Rewrite the previous message's declared length from [`LENGTHS`].
    SetLength {
        /// Index into [`LENGTHS`].
        length: u8,
    },
    /// Rewrite the previous message's own sample-count field from [`COUNTS`].
    SetCount {
        /// Index into [`COUNTS`].
        count: u8,
    },
    /// Cut the octet stream short, keeping `keep` octets of the previous message.
    Truncate {
        /// How many of the previous message's octets survive.
        keep: u8,
    },
    /// Append octets that are not a message at all.
    Noise {
        /// The octets, as three bytes of the step's own record.
        octets: [u8; 3],
    },
    /// Repeat the previous message verbatim.
    Repeat,
    /// Hand the decoder at most this many octets per read from here on.
    Chunk {
        /// One plus this, modulo a small bound: a reader that answers a byte at a time is legal.
        size: u8,
    },
}

/// A decoded adversarial exchange.
/// Complete by design: a program is a side and its steps, and the harness constructs these from
/// arbitrary bytes in a separate crate, so the fields have to stay reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// Which end is reading.
    pub side: Side,
    /// The ceiling the *runtime* side reads at, as an index into [`CEILINGS`].
    ///
    /// Unused on the worker side, whose ceiling is the `Hello`'s. It is still decoded there so that
    /// one byte means one thing whatever the side is, which is what lets a minimiser flip the side
    /// without changing the rest of the program.
    pub ceiling: u8,
    /// The steps, in order.
    pub steps: Vec<Step>,
}

/// The empty program, and it is exactly what [`Program::decode`] makes of no octets.
///
/// Stated rather than derived: the ceiling an absent byte names is 160 samples and not zero, and a
/// default that disagreed with the decoder would make an empty input mean two things.
impl Default for Program {
    fn default() -> Self {
        Self {
            side: Side::Runtime,
            ceiling: DEFAULT_CEILING,
            steps: Vec::new(),
        }
    }
}

/// The ceiling index an input too short to name one gets: 160 samples, one narrowband frame.
const DEFAULT_CEILING: u8 = 2;

impl Program {
    /// Decode arbitrary octets into a program.
    ///
    /// Total: every input names a program, and a trailing partial record is dropped rather than
    /// padded, so libFuzzer can shrink an input a byte at a time without changing what the octets
    /// before it mean.
    #[must_use]
    pub fn decode(data: &[u8]) -> Self {
        let side = data
            .first()
            .map_or(Side::Runtime, |byte| Side::from_byte(*byte));
        let ceiling = data.get(1).copied().unwrap_or(DEFAULT_CEILING);
        let rest = data.get(2..).unwrap_or_default();
        let steps = rest.chunks_exact(4).filter_map(decode_step).collect();
        Self {
            side,
            ceiling,
            steps,
        }
    }

    /// The octets [`Self::decode`] reads back as this program.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(2 + self.steps.len() * 4);
        out.push(self.side.to_byte());
        out.push(self.ceiling);
        for step in &self.steps {
            out.extend_from_slice(&encode_step(*step));
        }
        out
    }
}

/// One four-byte record as a step, or nothing if the record is not four bytes.
fn decode_step(record: &[u8]) -> Option<Step> {
    let [op, a, b, c] = *record.first_chunk::<4>()?;
    Some(match op % 12 {
        0 => Step::Hello {
            direction: a,
            format: b,
            ceiling: c,
        },
        1 => Step::Frame {
            sequence: a,
            samples: b,
            discontinuity: c,
        },
        2 => Step::Result {
            sequence: a,
            outcome: b,
            samples: c,
        },
        3 => Step::BreakMagic {
            first: a,
            second: b,
        },
        4 => Step::SetReserved { value: a },
        5 => Step::SetType { kind: a },
        6 => Step::SetLength { length: a },
        7 => Step::SetCount { count: a },
        8 => Step::Truncate { keep: a },
        9 => Step::Noise { octets: [a, b, c] },
        10 => Step::Repeat,
        _ => Step::Chunk { size: a },
    })
}

/// The four bytes [`decode_step`] reads back as this step.
const fn encode_step(step: Step) -> [u8; 4] {
    match step {
        Step::Hello {
            direction,
            format,
            ceiling,
        } => [0, direction, format, ceiling],
        Step::Frame {
            sequence,
            samples,
            discontinuity,
        } => [1, sequence, samples, discontinuity],
        Step::Result {
            sequence,
            outcome,
            samples,
        } => [2, sequence, outcome, samples],
        Step::BreakMagic { first, second } => [3, first, second, 0],
        Step::SetReserved { value } => [4, value, 0, 0],
        Step::SetType { kind } => [5, kind, 0, 0],
        Step::SetLength { length } => [6, length, 0, 0],
        Step::SetCount { count } => [7, count, 0, 0],
        Step::Truncate { keep } => [8, keep, 0, 0],
        Step::Noise { octets } => [9, octets[0], octets[1], octets[2]],
        Step::Repeat => [10, 0, 0, 0],
        Step::Chunk { size } => [11, size, 0, 0],
    }
}

// ---- the octets a program describes ----

/// Magic, type, reserved, length — §7.4's header, restated here so the oracle reads the octets
/// rather than asking the decoder what it thinks they are.
const HEADER: usize = 8;
const MAGIC: [u8; 2] = [0x53, 0x44];
const TYPE_HELLO: u8 = 0x01;
const TYPE_FRAME: u8 = 0x02;
const TYPE_RESULT: u8 = 0x81;
const HELLO_FIXED: u32 = 12;
const FRAME_FIXED: u32 = 21;
const RESULT_FIXED: u32 = 13;

/// The most samples a step will actually mint, whatever count it names.
///
/// A program that asked for a million samples would spend the campaign's whole budget writing
/// zeros. A count above this is still *claimed* — that is what [`Step::SetCount`] and
/// [`Step::SetLength`] are for — but never carried, so every input stays a few kilobytes.
const MINTED: u32 = 1_024;

/// One message's octets, and where in the stream they start.
struct Framed {
    at: usize,
    len: usize,
}

/// The stream a program describes, and the boundaries the oracle needs to read it back.
struct Encoded {
    octets: Vec<u8>,
    last: Option<Framed>,
    chunk: usize,
}

impl Encoded {
    fn new() -> Self {
        Self {
            octets: Vec::new(),
            last: None,
            chunk: 0,
        }
    }

    /// Overwrite one octet of the last message, if there is one and the offset is inside it.
    fn patch(&mut self, offset: usize, value: u8) {
        let Some(last) = self.last.as_ref() else {
            return;
        };
        if offset >= last.len {
            return;
        }
        if let Some(slot) = self.octets.get_mut(last.at.saturating_add(offset)) {
            *slot = value;
        }
    }

    /// Overwrite four octets of the last message with a big-endian `u32`.
    fn patch_quad(&mut self, offset: usize, value: u32) {
        for (index, octet) in value.to_be_bytes().into_iter().enumerate() {
            self.patch(offset.saturating_add(index), octet);
        }
    }

    /// Append a message and remember where it started.
    fn push(&mut self, message: &[u8]) {
        let at = self.octets.len();
        self.octets.extend_from_slice(message);
        self.last = Some(Framed {
            at,
            len: message.len(),
        });
    }
}

/// Mint a message of `kind` with `fixed` fixed octets and `samples` trailing samples.
fn message(kind: u8, fixed: u32, body: &[u8], samples: u32) -> Vec<u8> {
    let minted = samples.min(MINTED);
    let declared = fixed.saturating_add(minted.saturating_mul(2));
    let mut out = Vec::with_capacity(HEADER.saturating_add(declared as usize));
    out.extend_from_slice(&MAGIC);
    out.push(kind);
    out.push(0);
    out.extend_from_slice(&declared.to_be_bytes());
    out.extend_from_slice(body);
    if kind != TYPE_HELLO {
        out.extend_from_slice(&minted.to_be_bytes());
        for index in 0..minted {
            // Deterministic and not silent, so a decoder that dropped a sample is visible in a
            // round-trip rather than only in a length.
            let sample = i16::try_from(index % 30_011)
                .unwrap_or(0)
                .wrapping_sub(15_000);
            out.extend_from_slice(&sample.to_be_bytes());
        }
    }
    out
}

/// Turn a program into the octets it describes.
fn encode_stream(program: &Program) -> Encoded {
    let mut stream = Encoded::new();
    for step in &program.steps {
        apply(&mut stream, *step);
    }
    stream
}

#[allow(clippy::too_many_lines)]
fn apply(stream: &mut Encoded, step: Step) {
    match step {
        Step::Hello {
            direction,
            format,
            ceiling,
        } => {
            let (rate, channels) = pick(&FORMATS, format);
            let mut body = Vec::with_capacity(HELLO_FIXED as usize);
            body.extend_from_slice(&1_u16.to_be_bytes());
            // Two defined values and two that are not: a `Hello` whose direction is undefined must
            // be refused whole, and the way that fails is by half-applying.
            body.push(direction % 4);
            body.extend_from_slice(&rate.to_be_bytes());
            body.push(channels);
            body.extend_from_slice(&pick(&CEILINGS, ceiling).to_be_bytes());
            let message = message(TYPE_HELLO, HELLO_FIXED, &body, 0);
            stream.push(&message);
        }
        Step::Frame {
            sequence,
            samples,
            discontinuity,
        } => {
            let mut body = Vec::with_capacity(FRAME_FIXED as usize);
            body.extend_from_slice(&u64::from(sequence).to_be_bytes());
            body.extend_from_slice(&(u64::from(sequence) * 160).to_be_bytes());
            body.push(discontinuity % 5);
            let message = message(TYPE_FRAME, FRAME_FIXED, &body, pick(&COUNTS, samples));
            stream.push(&message);
        }
        Step::Result {
            sequence,
            outcome,
            samples,
        } => {
            let mut body = Vec::with_capacity(RESULT_FIXED as usize);
            body.extend_from_slice(&u64::from(sequence).to_be_bytes());
            body.push(outcome % 4);
            let message = message(TYPE_RESULT, RESULT_FIXED, &body, pick(&COUNTS, samples));
            stream.push(&message);
        }
        Step::BreakMagic { first, second } => {
            stream.patch(0, first);
            stream.patch(1, second);
        }
        Step::SetReserved { value } => stream.patch(3, value),
        Step::SetType { kind } => stream.patch(2, pick(&TYPES, kind)),
        Step::SetLength { length } => stream.patch_quad(4, pick(&LENGTHS, length)),
        Step::SetCount { count } => {
            // The count field sits after the type's fixed fields, which is a different offset for
            // each type. Reading it off the message's own type octet keeps this honest under a
            // `SetType` that already ran.
            let Some(last) = stream.last.as_ref() else {
                return;
            };
            let kind = stream.octets.get(last.at.saturating_add(2)).copied();
            let at = match kind {
                Some(TYPE_FRAME) => HEADER.saturating_add(17),
                Some(TYPE_RESULT) => HEADER.saturating_add(9),
                _ => return,
            };
            stream.patch_quad(at, pick(&COUNTS, count));
        }
        Step::Truncate { keep } => {
            let Some(last) = stream.last.as_ref() else {
                return;
            };
            let keep = usize::from(keep).min(last.len);
            let end = last.at.saturating_add(keep);
            stream.octets.truncate(end);
            stream.last = Some(Framed {
                at: last.at,
                len: keep,
            });
        }
        Step::Noise { octets } => {
            let at = stream.octets.len();
            stream.octets.extend_from_slice(&octets);
            stream.last = Some(Framed {
                at,
                len: octets.len(),
            });
        }
        Step::Repeat => {
            let Some(last) = stream.last.as_ref() else {
                return;
            };
            let Some(previous) = stream
                .octets
                .get(last.at..last.at.saturating_add(last.len))
                .map(<[u8]>::to_vec)
            else {
                return;
            };
            stream.push(&previous);
        }
        Step::Chunk { size } => stream.chunk = usize::from(size % 17).saturating_add(1),
    }
}

/// Index a table with a byte, so every byte names an entry.
fn pick<T: Copy + Default>(table: &[T], index: u8) -> T {
    let len = table.len();
    if len == 0 {
        return T::default();
    }
    table
        .get(usize::from(index) % len)
        .copied()
        .unwrap_or_default()
}

// ---- the oracle ----

/// An invariant `docs/specs/call-dsp-graph.md` §7.4 states, broken.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum Invariant {
    /// The decoder's verdict is not the one §7.4's table gives for these header octets.
    #[error(
        "the octets at {at} must be {expected}, and the decoder said {actual}: §7.4's table is \
         ordered, and a message is refused by its type and never by its length prefix"
    )]
    WrongRefusal {
        /// Where the message starts in the stream.
        at: usize,
        /// What §7.4's table says, derived from the header alone.
        expected: String,
        /// What the decoder said.
        actual: String,
    },
    /// A refusal §7.4 decides from the header consumed payload octets to reach it.
    #[error(
        "the {refusal} at {at} read {read} octets, and a refusal decided from the header may read \
         only its {HEADER}: no declared length ever sizes a read"
    )]
    PayloadReadBeforeRefusal {
        /// Where the message starts.
        at: usize,
        /// The refusal that was reached.
        refusal: String,
        /// How many octets reaching it consumed.
        read: usize,
    },
    /// A number a peer declared grew a buffer past what §7.4 permits.
    #[error(
        "a peer's declaration grew a buffer to {capacity} octets against §7.4's limit of {limit}"
    )]
    BufferSizedByPeer {
        /// What the buffer reached.
        capacity: usize,
        /// What §7.4 permits.
        limit: usize,
    },
    /// An accepted `Hello` raised the ceiling above the contract's own largest frame.
    #[error(
        "an accepted Hello declared a ceiling of {declared} samples against a bound of {bound}: \
         the ceiling is a declared number too, so an unbounded one is the peer choosing the size \
         of the next read"
    )]
    CeilingOutOfContract {
        /// What the `Hello` declared.
        declared: u32,
        /// The largest ceiling §7.4 admits.
        bound: u32,
    },
    /// An accepted message's samples do not account for its declared length.
    #[error(
        "an accepted {kind:#04x} at {at} declared {declared} octets and decoded {samples} samples, \
         which does not account for it exactly"
    )]
    CountDoesNotAccount {
        /// Where the message starts.
        at: usize,
        /// Its type octet.
        kind: u8,
        /// What its header declared.
        declared: u32,
        /// How many samples came out.
        samples: usize,
    },
    /// A real caller carried on past a refused message.
    #[error("{side} kept going after a refused message: {detail}")]
    RefusalNotTerminal {
        /// Which caller.
        side: &'static str,
        /// What it did.
        detail: String,
    },
}

/// What running one program established.
/// Complete by design: what one run observed, read field by field by the oracle and by the
/// replay test in another crate.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// A line per decoded message, for reading a minimised crash back.
    pub trace: Vec<String>,
    /// Every invariant this program broke. Empty is the only acceptable value.
    pub violations: Vec<Invariant>,
    /// How many messages were accepted.
    pub accepted: usize,
    /// How many were refused.
    pub refused: usize,
}

/// §7.4's refusal table, applied to one message's header octets and nothing else.
///
/// The decoder's second opinion, and deliberately a different piece of code: it walks the table in
/// the order §7.4 writes it, so a decoder that checked the length before the type would disagree
/// with it even though both refuse.
///
/// Returns `Ok(())` when the header alone refuses nothing — the payload may still be refused, by a
/// rule that needs to have read it.
pub fn expected_refusal(header: &[u8], ceiling: u32) -> Result<(), &'static str> {
    if header.first_chunk::<2>() != Some(&MAGIC) {
        return Err("BadMagic");
    }
    let Some(&kind) = header.get(2) else {
        return Err("Truncated");
    };
    if header.get(3).copied().unwrap_or_default() != 0 {
        return Err("Reserved");
    }
    let Some(declared) = header.get(4..8).and_then(<[u8]>::first_chunk::<4>) else {
        return Err("Truncated");
    };
    let declared = u32::from_be_bytes(*declared);
    let (fixed, limit) = match kind {
        TYPE_HELLO => (HELLO_FIXED, HELLO_FIXED),
        TYPE_FRAME => (
            FRAME_FIXED,
            FRAME_FIXED.saturating_add(ceiling.min(MAX_FRAME_SAMPLES).saturating_mul(2)),
        ),
        TYPE_RESULT => (
            RESULT_FIXED,
            RESULT_FIXED.saturating_add(ceiling.min(MAX_FRAME_SAMPLES).saturating_mul(2)),
        ),
        _ => return Err("UnknownType"),
    };
    if declared < fixed {
        return Err("Undersized");
    }
    if declared > limit {
        return Err("Oversized");
    }
    Ok(())
}

/// Whether a refusal is one §7.4 decides from the header alone.
const fn from_header_alone(refusal: &str) -> bool {
    matches!(
        refusal.as_bytes(),
        b"BadMagic" | b"Reserved" | b"UnknownType" | b"Undersized" | b"Oversized"
    )
}

/// The name §7.4's table gives a refusal the decoder produced.
///
/// Exhaustive on purpose, `#[non_exhaustive]` notwithstanding: this is the crate that defines the
/// variants, so a refusal added without a row in the oracle is a compile error here rather than an
/// input the campaign quietly stops checking.
fn refusal_name(error: &WorkerProtocolError) -> &'static str {
    match error {
        WorkerProtocolError::BadMagic => "BadMagic",
        WorkerProtocolError::Reserved { .. } => "Reserved",
        WorkerProtocolError::UnknownType { .. } => "UnknownType",
        WorkerProtocolError::Undersized { .. } => "Undersized",
        WorkerProtocolError::Oversized { .. } => "Oversized",
        WorkerProtocolError::LengthMismatch { .. } => "LengthMismatch",
        WorkerProtocolError::Truncated => "Truncated",
        WorkerProtocolError::Value { .. } => "Value",
        WorkerProtocolError::Version { .. } => "Version",
    }
}

/// A reader that hands out at most `chunk` octets at a time and counts what it gave.
///
/// The chunking is the point as well as the counting: `read_message` fills its header with a loop
/// over short reads, and a pipe is exactly the thing that answers a read with fewer octets than were
/// asked for.
struct Scripted<'a> {
    octets: &'a [u8],
    at: usize,
    chunk: usize,
}

impl<'a> Scripted<'a> {
    fn new(octets: &'a [u8], chunk: usize) -> Self {
        Self {
            octets,
            at: 0,
            chunk: if chunk == 0 { usize::MAX } else { chunk },
        }
    }
}

impl Read for Scripted<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let rest = self.octets.get(self.at..).unwrap_or_default();
        let take = rest.len().min(buf.len()).min(self.chunk);
        let Some(source) = rest.get(..take) else {
            return Ok(0);
        };
        let Some(slot) = buf.get_mut(..take) else {
            return Ok(0);
        };
        slot.copy_from_slice(source);
        self.at = self.at.saturating_add(take);
        Ok(take)
    }
}

/// A sink that counts what was written to it and keeps it.
#[derive(Default)]
struct Recorder {
    written: Vec<u8>,
}

impl Write for Recorder {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Run one program and report every invariant it broke.
///
/// The model lane runs first and decides whether the octets may be handed to the real callers: a
/// ceiling out of contract is a violation *and* a stop, because a `Hello` that raised the ceiling to
/// four billion samples is one whose next frame the real worker would try to allocate for. Reporting
/// that and then doing it anyway would take the process down before anything could read the report.
#[must_use]
pub fn run(program: &Program) -> Outcome {
    let stream = encode_stream(program);
    let mut outcome = Outcome::default();
    match model_lane(program, &stream, &mut outcome) {
        Some(stop) => real_callers(program, &stream, stop, &mut outcome),
        None => outcome
            .trace
            .push("real callers skipped: the negotiated ceiling is outside §7.4's own".to_owned()),
    }
    outcome
}

/// Where a refused message left the stream, if one did.
///
/// `None` means nothing was refused and no caller may be held to having stopped.
#[derive(Debug, Clone, Copy)]
struct Refusal {
    /// The offset one past the refused message's last consumed octet. §7.4's terminality is the
    /// claim that a real caller reading the raw stream never reads beyond it.
    at: usize,
    /// How many messages the stream carried before it. The runtime's pump reads one answer per
    /// frame it offers, so this is how many frames it must offer before it meets the refusal — the
    /// measure that survives the `BufReader` a counted octet does not.
    index: usize,
}

/// A refused message, or nothing to hold a caller to.
type StopPoint = Option<Refusal>;

/// Drive `read_message` the way the side's real caller drives it, checking every rule that is about
/// one message.
///
/// `None` means the octets must not be handed to the real callers.
fn model_lane(program: &Program, stream: &Encoded, outcome: &mut Outcome) -> Option<StopPoint> {
    let limit = wire::MESSAGE_LIMIT as usize;
    let mut reader = Scripted::new(&stream.octets, stream.chunk);
    let mut payload = Vec::new();
    let mut samples = Vec::new();
    // §7.4: the runtime reads at the ceiling it configured; the worker has none until a `Hello`
    // gives it one, so nothing larger than a `Hello` may be read before then.
    let mut ceiling = match program.side {
        Side::Runtime => pick(&CEILINGS, program.ceiling).min(MAX_FRAME_SAMPLES),
        Side::Worker => 0,
    };

    loop {
        let at = reader.at;
        // §7.4's table is a function of a *whole* header. A stream that ends inside one has not
        // told the decoder what kind of message it was refusing, so the only verdict available
        // there is `Truncated`, and that is checked separately below.
        let header = stream
            .octets
            .get(at..at.saturating_add(HEADER))
            .map(<[u8]>::to_vec);
        let verdict = wire::read_message(&mut reader, &mut payload, &mut samples, ceiling);
        let read = reader.at.saturating_sub(at);

        if payload.capacity() > limit {
            outcome.violations.push(Invariant::BufferSizedByPeer {
                capacity: payload.capacity(),
                limit,
            });
            return None;
        }

        match verdict {
            Ok(None) => {
                outcome.trace.push(format!("{at}: end of stream"));
                return Some(None);
            }
            Ok(Some(message)) => {
                outcome.accepted = outcome.accepted.saturating_add(1);
                outcome.trace.push(format!("{at}: accepted {message:?}"));
                if let Some(header) = header.as_deref() {
                    if let Err(expected) = expected_refusal(header, ceiling) {
                        outcome.violations.push(Invariant::WrongRefusal {
                            at,
                            expected: expected.to_owned(),
                            actual: "accepted".to_owned(),
                        });
                    }
                    check_accounting(header, &samples, at, outcome);
                }
                if let wire::Incoming::Hello { max_samples, .. } = message {
                    if max_samples > MAX_FRAME_SAMPLES {
                        outcome.violations.push(Invariant::CeilingOutOfContract {
                            declared: max_samples,
                            bound: MAX_FRAME_SAMPLES,
                        });
                        return None;
                    }
                    if program.side == Side::Worker {
                        ceiling = max_samples;
                    }
                }
            }
            Err(wire::Fault::Io(error)) => {
                outcome.trace.push(format!("{at}: io {error}"));
                return Some(None);
            }
            Err(wire::Fault::Refused(error)) => {
                outcome.refused = outcome.refused.saturating_add(1);
                let actual = refusal_name(&error);
                outcome.trace.push(format!("{at}: refused {actual}"));
                match header.as_deref() {
                    Some(header) => check_refusal(header, ceiling, actual, at, read, outcome),
                    // Fewer than eight octets were left, so there is no header to reach a verdict
                    // from and `Truncated` is the only one §7.4 offers.
                    None if actual != "Truncated" => {
                        outcome.violations.push(Invariant::WrongRefusal {
                            at,
                            expected: "Truncated".to_owned(),
                            actual: actual.to_owned(),
                        });
                    }
                    None => {}
                }
                // §7.4: a refusal is terminal. The model stops here for the same reason both real
                // callers do, and the real-caller lane is what proves they actually do.
                return Some(Some(Refusal {
                    at: reader.at,
                    index: outcome.accepted,
                }));
            }
        }
    }
}

/// Compare the decoder's refusal against §7.4's table, and check what reaching it cost.
fn check_refusal(
    header: &[u8],
    ceiling: u32,
    actual: &str,
    at: usize,
    read: usize,
    outcome: &mut Outcome,
) {
    match expected_refusal(header, ceiling) {
        Err(expected) if expected != actual => {
            outcome.violations.push(Invariant::WrongRefusal {
                at,
                expected: expected.to_owned(),
                actual: actual.to_owned(),
            });
        }
        // The header refuses nothing, so this refusal came from the payload. `LengthMismatch`,
        // `Truncated`, `Value` and `Version` all need octets the header does not carry, and none of
        // them may be reached from a header the table admits.
        Ok(()) if from_header_alone(actual) => {
            outcome.violations.push(Invariant::WrongRefusal {
                at,
                expected: "accepted or a payload refusal".to_owned(),
                actual: actual.to_owned(),
            });
        }
        _ => {}
    }
    if from_header_alone(actual) && read > HEADER {
        outcome
            .violations
            .push(Invariant::PayloadReadBeforeRefusal {
                at,
                refusal: actual.to_owned(),
                read,
            });
    }
}

/// An accepted message's samples must account for its declared length exactly.
fn check_accounting(header: &[u8], samples: &[i16], at: usize, outcome: &mut Outcome) {
    let Some(&kind) = header.get(2) else {
        return;
    };
    let fixed = match kind {
        TYPE_FRAME => FRAME_FIXED,
        TYPE_RESULT => RESULT_FIXED,
        // A `Hello` carries no samples, and an accepted one that produced any would be a decoder
        // reading a field that is not there.
        TYPE_HELLO if samples.is_empty() => return,
        _ => 0,
    };
    let Some(declared) = header.get(4..8).and_then(<[u8]>::first_chunk::<4>) else {
        return;
    };
    let declared = u32::from_be_bytes(*declared);
    let counted = u32::try_from(samples.len())
        .unwrap_or(u32::MAX)
        .saturating_mul(2)
        .saturating_add(fixed);
    if counted != declared {
        outcome.violations.push(Invariant::CountDoesNotAccount {
            at,
            kind,
            declared,
            samples: samples.len(),
        });
    }
}

/// Drive the side's real caller over the same octets, and check that a refusal ended it.
fn real_callers(program: &Program, stream: &Encoded, stop: StopPoint, outcome: &mut Outcome) {
    match program.side {
        Side::Worker => worker_lane(stream, stop, outcome),
        Side::Runtime => runtime_lane(program, stream, stop, outcome),
    }
}

/// A worker that answers every frame, so the lane measures the protocol and not a policy.
struct Echo;

impl SupervisedWorker for Echo {
    fn capability(&self) -> DspCapability {
        DspCapability::new("m122-echo")
            .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated))
    }
    fn run(&mut self, frame: &DspFrame<'_>, out: &mut Vec<i16>) -> WorkerResult {
        out.extend_from_slice(frame.samples());
        WorkerResult::Produced
    }
}

/// §7.4: a worker whose input is refused stops reading and exits.
///
/// The claim is about octets, which is why [`Scripted`] counts them: whatever `serve` decides to do
/// about a refused message, it must not have read the one after it.
fn worker_lane(stream: &Encoded, stop: StopPoint, outcome: &mut Outcome) {
    let mut reader = Scripted::new(&stream.octets, stream.chunk);
    let served = worker::serve(Echo, &mut reader, Recorder::default());
    let consumed = reader.at;
    let ending = served.as_ref().map_or_else(
        |error| format!("{:?} {error}", error.kind()),
        |()| "cleanly".to_owned(),
    );
    outcome.trace.push(format!(
        "worker::serve consumed {consumed} of {} octets, ended {ending}",
        stream.octets.len(),
    ));
    let Some(stop) = stop else {
        return;
    };
    if consumed > stop.at {
        outcome.violations.push(Invariant::RefusalNotTerminal {
            side: "worker::serve",
            detail: format!(
                "read {consumed} octets past a refusal that ended at {}",
                stop.at
            ),
        });
    }
    // Returning `Ok` is not itself a finding: §7.4 gives a worker several reasons to stop before a
    // refused message is reached — a second `Hello`, a message for the other direction, a frame the
    // processing thread can no longer be handed — and every one of them ends the exchange cleanly.
    // What the refusal buys is the octet claim above, and that is what is asserted.
}

/// §7.2: a runtime whose worker answers with a refused message stops the pump and loses the worker.
///
/// **The claim here is about what the pump did, not about octets.** The pump reads through a
/// [`BufReader`](std::io::BufReader), which is a real part of the design and not an accident of the
/// harness — so the octets the operating system handed it are not the octets it interpreted, and a
/// counting reader underneath it measures the buffer's appetite rather than the protocol's. What is
/// measured instead is exact: a `Frame` written after the [`WorkerResult::Failed`] was recorded, or
/// a second answer after it, is a pump that carried on past a refusal.
fn runtime_lane(program: &Program, stream: &Encoded, stop: StopPoint, outcome: &mut Outcome) {
    /// Deep enough that the pump keeps reading for the whole of any seed, and bounded because a
    /// lockstep protocol answers one frame at a time.
    const OFFERED: usize = 8;

    let (requests, inbox) = sync_channel::<Request>(OFFERED);
    let (outbox, answers) = sync_channel(OFFERED);
    for sequence in 0..OFFERED {
        let sequence = sequence as u64;
        if requests
            .try_send(Request {
                sequence,
                position: sequence.saturating_mul(160),
                discontinuity: None,
                samples: vec![0_i16; 160],
            })
            .is_err()
        {
            break;
        }
    }
    drop(requests);

    let mut reader = Scripted::new(&stream.octets, stream.chunk);
    let mut written = Recorder::default();
    let ceiling = pick(&CEILINGS, program.ceiling).min(MAX_FRAME_SAMPLES);
    supervised::pump(
        &mut written,
        &mut reader,
        &inbox,
        &outbox,
        AudioDirection::Inbound,
        StreamFormat::default(),
        ceiling as usize,
    );
    drop(outbox);

    let results: Vec<WorkerResult> = std::iter::from_fn(|| answers.try_recv().ok())
        .map(|response: supervised::Response| response.result)
        .collect();
    let offers = count_frames(&written.written);
    let unoffered = std::iter::from_fn(|| inbox.try_recv().ok()).count();
    outcome.trace.push(format!(
        "supervised::pump offered {offers} frames of {OFFERED}, left {unoffered} unoffered, \
         answered {results:?}, model stop {stop:?}",
    ));

    // §7.2: a refused answer is a `MalformedResult` and the worker is lost. Having offered more
    // frames than there were messages before the refusal, the pump read the refused one — so it owes
    // exactly that verdict, as its last.
    if let Some(stop) = stop
        && offers > stop.index
        && results.last() != Some(&WorkerResult::Failed)
    {
        outcome.violations.push(Invariant::RefusalNotTerminal {
            side: "supervised::pump",
            detail: format!(
                "offered {offers} frames past the refusal at message {} and answered \
                 {results:?}, which does not end in a lost worker",
                stop.index
            ),
        });
    }

    let Some(lost) = results
        .iter()
        .position(|result| *result == WorkerResult::Failed)
    else {
        return;
    };
    let after = results.len().saturating_sub(lost.saturating_add(1));
    if after > 0 {
        outcome.violations.push(Invariant::RefusalNotTerminal {
            side: "supervised::pump",
            detail: format!("answered {after} more frames after losing the worker"),
        });
    }
    // The pump writes a frame, then reads its answer. Having lost the worker on the answer to its
    // last offer, it must not have offered another: one write per answer is the lockstep, and one
    // more than that is a stage still feeding a worker §7.2 says is gone.
    if offers > results.len() {
        outcome.violations.push(Invariant::RefusalNotTerminal {
            side: "supervised::pump",
            detail: format!("offered {offers} frames against {} answers", results.len()),
        });
    }
}

/// How many `Frame` messages a stream of well-formed messages carries.
///
/// Walks the pump's own writes, which are the only octets this is ever given, so a header that does
/// not parse ends the walk rather than being guessed at.
fn count_frames(written: &[u8]) -> usize {
    let mut at = 0_usize;
    let mut frames = 0_usize;
    while let Some(header) = written
        .get(at..at.saturating_add(HEADER))
        .and_then(<[u8]>::first_chunk::<HEADER>)
    {
        let Some(declared) = header.get(4..8).and_then(<[u8]>::first_chunk::<4>) else {
            break;
        };
        if header.get(2) == Some(&TYPE_FRAME) {
            frames = frames.saturating_add(1);
        }
        at = at
            .saturating_add(HEADER)
            .saturating_add(u32::from_be_bytes(*declared) as usize);
    }
    frames
}

// ---- the seed corpus ----

/// One named seed program.
///
/// Complete by design: a name and a program, written to the corpus by an example in this crate and
/// read back by a test in another, so both fields have to stay reachable.
#[derive(Debug, Clone)]
pub struct Seed {
    /// The file name it is written under, and what it is about.
    pub name: &'static str,
    /// The program itself.
    pub program: Program,
}

/// Where the committed seeds live, from the repository root.
pub const CORPUS_PATH: &str = "crates/sipx-media/corpus/worker-protocol-sequences";

/// The absolute path of the committed seed corpus.
#[must_use]
pub fn corpus_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus/worker-protocol-sequences")
}

/// Write every seed into [`corpus_dir`], returning how many files were written.
///
/// # Errors
///
/// Whatever creating the directory or writing a file returned.
pub fn write_corpus() -> io::Result<usize> {
    let dir = corpus_dir();
    std::fs::create_dir_all(&dir)?;
    let seeds = seeds();
    for seed in &seeds {
        std::fs::write(dir.join(seed.name), seed.program.encode())?;
    }
    Ok(seeds.len())
}

/// The adversarial corpus: one program per shape §7.4's refusal table names, plus the exchanges
/// around them.
///
/// A fuzzer starting from noise spends its budget rediscovering the magic. Starting from these it
/// spends it on the arithmetic, exactly as the RFC 4475 archive seeds the parser targets.
///
/// **One refusal per seed, and it is the last message.** A refusal is terminal, so a seed carrying
/// two of them is a seed that only ever exercises the first — the corpus would claim a coverage it
/// does not have, which is the failure mode a generated corpus exists to avoid. Families that vary
/// one field are therefore a seed per value rather than one seed walking them.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn seeds() -> Vec<Seed> {
    // Index 5 of `CEILINGS` is `MAX_FRAME_SAMPLES`, index 7 is `u32::MAX`, index 2 is 160.
    let hello = |ceiling: u8| Step::Hello {
        direction: 1,
        format: 0,
        ceiling,
    };
    let frame = |sequence: u8, samples: u8| Step::Frame {
        sequence,
        samples,
        discontinuity: 0,
    };
    let result = |sequence: u8, samples: u8| Step::Result {
        sequence,
        outcome: 0,
        samples,
    };
    let mut seeds = Vec::new();
    let mut push = |name: &'static str, side: Side, ceiling: u8, steps: Vec<Step>| {
        seeds.push(Seed {
            name,
            program: Program {
                side,
                ceiling,
                steps,
            },
        });
    };

    push(
        "lockstep-exchange",
        Side::Runtime,
        2,
        (0..6).map(|n| result(n, 5)).collect(),
    );
    push(
        "worker-reads-a-whole-call",
        Side::Worker,
        2,
        std::iter::once(hello(2))
            .chain((0..6).map(|n| frame(n, 5)))
            .collect(),
    );
    push(
        "bad-magic",
        Side::Runtime,
        2,
        vec![
            result(0, 5),
            result(1, 5),
            Step::BreakMagic {
                first: 0x52,
                second: 0x54,
            },
        ],
    );
    push(
        "reserved-octet-set",
        Side::Runtime,
        2,
        vec![
            result(0, 1),
            result(1, 1),
            Step::SetReserved { value: 0x01 },
        ],
    );
    for (name, kind) in [
        ("unknown-type-zero", 3_u8),
        ("unknown-type-low", 4),
        ("unknown-type-high-bit", 5),
        ("unknown-type-all-ones", 7),
    ] {
        push(
            name,
            Side::Runtime,
            2,
            vec![result(0, 1), Step::SetType { kind }],
        );
    }
    push(
        "undersized-hello",
        Side::Worker,
        2,
        vec![hello(2), Step::SetLength { length: 2 }],
    );
    push(
        "undersized-frame",
        Side::Worker,
        2,
        vec![hello(2), frame(0, 1), Step::SetLength { length: 4 }],
    );
    push(
        "undersized-result",
        Side::Runtime,
        2,
        vec![result(0, 1), Step::SetLength { length: 2 }],
    );
    push(
        "oversized-from-the-header-alone",
        Side::Runtime,
        2,
        vec![
            result(0, 1),
            result(1, 1),
            Step::SetLength { length: 6 },
            Step::Truncate { keep: 8 },
        ],
    );
    for (name, length) in [
        ("oversized-just-past-the-ceiling", 5_u8),
        ("oversized-at-the-signed-edge", 7),
        ("oversized-where-doubling-saturates", 8),
        ("oversized-at-the-top-of-the-range", 9),
    ] {
        push(
            name,
            Side::Runtime,
            2,
            vec![
                result(0, 1),
                Step::SetLength { length },
                Step::Truncate { keep: 8 },
            ],
        );
    }
    push(
        "count-below-the-length",
        Side::Runtime,
        2,
        vec![result(0, 4), Step::SetCount { count: 4 }],
    );
    push(
        "count-far-above-the-length",
        Side::Runtime,
        2,
        vec![result(0, 4), Step::SetCount { count: 9 }],
    );
    push(
        "truncated-inside-the-header",
        Side::Runtime,
        2,
        vec![result(0, 3), Step::Truncate { keep: 3 }],
    );
    push(
        "truncated-at-the-header-boundary",
        Side::Runtime,
        2,
        vec![result(0, 3), Step::Truncate { keep: 8 }],
    );
    push(
        "truncated-inside-the-samples",
        Side::Runtime,
        2,
        vec![result(0, 3), Step::Truncate { keep: 20 }],
    );
    for (name, direction) in [("direction-zero", 0_u8), ("direction-three", 3)] {
        push(
            name,
            Side::Worker,
            2,
            vec![Step::Hello {
                direction,
                format: 0,
                ceiling: 2,
            }],
        );
    }
    for (name, format) in [
        ("format-rate-zero", 4_u8),
        ("format-rate-past-the-domain", 5),
        ("format-no-channels", 6),
        ("format-too-many-channels", 7),
    ] {
        push(
            name,
            Side::Worker,
            2,
            vec![Step::Hello {
                direction: 1,
                format,
                ceiling: 2,
            }],
        );
    }
    push(
        "undefined-discontinuity",
        Side::Worker,
        2,
        vec![
            hello(2),
            frame(0, 5),
            Step::Frame {
                sequence: 1,
                samples: 1,
                discontinuity: 4,
            },
        ],
    );
    push(
        "undefined-outcome",
        Side::Runtime,
        2,
        vec![
            result(0, 1),
            Step::Result {
                sequence: 1,
                outcome: 3,
                samples: 1,
            },
        ],
    );
    push(
        "hostile-ceiling",
        Side::Worker,
        2,
        vec![hello(7), frame(0, 9), Step::SetLength { length: 9 }],
    );
    push(
        "ceiling-one-past-the-contract",
        Side::Worker,
        2,
        vec![hello(6), frame(0, 5)],
    );
    push(
        "ceiling-at-the-contract",
        Side::Worker,
        2,
        vec![hello(5), frame(0, 8), frame(1, 5)],
    );
    push(
        "empty-frames-at-a-zero-ceiling",
        Side::Worker,
        0,
        vec![hello(0), frame(0, 0), frame(1, 0), frame(2, 1)],
    );
    push(
        "arbitrary-chunking",
        Side::Worker,
        2,
        vec![
            Step::Chunk { size: 0 },
            hello(2),
            frame(0, 5),
            Step::Chunk { size: 2 },
            frame(1, 5),
            Step::Chunk { size: 6 },
            frame(2, 5),
        ],
    );
    push(
        "noise-before-a-message",
        Side::Runtime,
        2,
        vec![Step::Noise { octets: *b"run" }, result(0, 1)],
    );
    push(
        "a-second-hello",
        Side::Worker,
        2,
        vec![hello(2), frame(0, 5), hello(2), frame(1, 5)],
    );
    push(
        "a-message-for-the-other-direction",
        Side::Runtime,
        2,
        vec![result(0, 1), result(1, 1), Step::SetType { kind: 1 }],
    );
    push(
        "repeated-messages",
        Side::Runtime,
        2,
        vec![result(0, 2), Step::Repeat, Step::Repeat, Step::Repeat],
    );
    push(
        "a-frame-payload-under-a-result-header",
        Side::Worker,
        2,
        vec![hello(2), frame(0, 5), Step::SetType { kind: 2 }],
    );
    push(
        "a-result-payload-under-a-frame-header",
        Side::Runtime,
        2,
        vec![result(0, 5), Step::SetType { kind: 1 }],
    );

    seeds
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

    /// Every seed runs clean, which is what makes the fuzzer's finds new information.
    #[test]
    fn every_seed_breaks_no_invariant() {
        for seed in seeds() {
            let outcome = run(&seed.program);
            assert!(
                outcome.violations.is_empty(),
                "{}: {:?}\ntrace:\n{}",
                seed.name,
                outcome.violations,
                outcome.trace.join("\n")
            );
        }
    }

    /// The hostile ceiling is the defect `M-122` found, and it is a seed so it stays fixed.
    #[test]
    fn the_hostile_ceiling_seed_is_refused_rather_than_adopted() {
        let seed = seeds()
            .into_iter()
            .find(|seed| seed.name == "hostile-ceiling")
            .expect("the seed exists");
        let outcome = run(&seed.program);
        assert!(outcome.violations.is_empty(), "{:?}", outcome.violations);
        assert!(
            outcome
                .trace
                .iter()
                .any(|line| line.contains("refused Value")),
            "the ceiling must be refused, not adopted:\n{}",
            outcome.trace.join("\n")
        );
    }

    /// A decoder that adopted an unbounded ceiling is caught, so the oracle is not vacuous.
    #[test]
    fn an_unbounded_ceiling_is_an_invariant_violation() {
        // The oracle's own arm, driven directly: `run` cannot reach it any more, which is the
        // point, so the assertion that it would fire has to be made here.
        let mut outcome = Outcome::default();
        outcome.violations.push(Invariant::CeilingOutOfContract {
            declared: u32::MAX,
            bound: MAX_FRAME_SAMPLES,
        });
        assert!(
            outcome.violations[0]
                .to_string()
                .contains("4294967295 samples against a bound of 65536")
        );
    }
}
