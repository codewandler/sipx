//! The synthesis contract (`docs/specs/speech-providers.md` §6).
//!
//! A synthesis session accepts bounded text requests and produces owned PCM chunks. Each request
//! reaches exactly one terminal, requests start in FIFO order, and a `replace` enqueue cancels
//! everything before it *before* the new request is accepted — so a consumer never sees a new
//! request start while an old one is still notionally live.
//!
//! **Chunks are contiguous unless a gap is named.** Chunk `n + 1`'s offset equals chunk `n`'s
//! offset plus its duration, unless a [`SynthesisOutput::Discontinuity`] between them says
//! otherwise. A provider that falls behind real time marks the gap rather than emitting late audio
//! labelled as continuous; whether that gap becomes silence or a shifted playout is the driver's
//! policy at the `M-54` seam (`A-27`), not the provider's.
//!
//! **Production is windowed.** The driver grants a chunk window (§8) and returns credit with
//! [`SynthesisInput::Drained`], so a provider cannot run ahead of a slow call into unbounded audio.

use std::fmt;

use sipx_audio::Pcm;

use super::lifecycle::{CancelReason, DeadlineKind, FailureCause, LossCause};
use super::privacy::{DataClass, Redacted};

/// One synthesis request's identity within one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(u64);

impl RequestId {
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
}

/// One chunk of synthesized audio (§6 `Chunk`).
///
/// Its `Debug` redacts the samples (§11.4), on the same terms as
/// [`RecognitionFrame`](super::RecognitionFrame): synthesized speech is call audio and call audio
/// is user data, whichever direction it was going.
#[derive(Clone, PartialEq, Eq)]
pub struct SynthesisChunk {
    request: RequestId,
    sequence: u64,
    offset: u64,
    pcm: Pcm,
}

impl SynthesisChunk {
    /// Build a chunk in the session's operating format.
    #[must_use]
    pub const fn new(request: RequestId, sequence: u64, offset: u64, pcm: Pcm) -> Self {
        Self {
            request,
            sequence,
            offset,
            pcm,
        }
    }

    /// Which request produced it.
    #[must_use]
    pub const fn request(&self) -> RequestId {
        self.request
    }

    /// Its position in that request's chunk stream, monotonic per request.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Its sample-time offset from the start of the request.
    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// The owned samples.
    #[must_use]
    pub const fn pcm(&self) -> &Pcm {
        &self.pcm
    }

    /// Consume the chunk and return its samples.
    #[must_use]
    pub fn into_pcm(self) -> Pcm {
        self.pcm
    }
}

impl fmt::Debug for SynthesisChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SynthesisChunk")
            .field("request", &self.request)
            .field("sequence", &self.sequence)
            .field("offset", &self.offset)
            .field(
                "pcm",
                &Redacted::samples(DataClass::CallAudio, self.pcm.samples().len()),
            )
            .finish()
    }
}

/// What a cancellation applies to (§6 `Cancel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CancelScope {
    /// One request. Cancelling one that is already terminal, unknown or refused is ignored.
    Request(RequestId),
    /// The whole session: the started request and every queued one, resolved in queue order.
    Session,
}

/// Why a request was not queued (§6 `Refused`).
///
/// A refused request has no further events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SynthesisRefusal {
    /// The request queue is at §8's bound.
    QueueFull,
    /// The text is longer than §8's request-text bound.
    TextTooLarge,
    /// The session has ended. This is what an `Enqueue` after session end receives — never a
    /// cancellation, because there was never a request to cancel.
    SessionEnded,
}

/// What a driver delivers to a synthesis session (§6).
///
/// `#[non_exhaustive]` on the same terms as [`RecognitionInput`](super::RecognitionInput). Its
/// `Debug` redacts the enqueued text (§11.4): what a host asked a call to say is user data, and
/// `Enqueue` is the one value in this contract that carries it.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SynthesisInput {
    /// Queue one bounded text request.
    Enqueue {
        /// The request's identity.
        request: RequestId,
        /// Owned UTF-8 within §8's text bound.
        text: String,
        /// Whether to cancel the started request and every queued one first, each with
        /// [`CancelReason::Replaced`] in queue order.
        replace: bool,
    },
    /// Cancel one request or the whole session, with a typed reason.
    Cancel {
        /// What the cancellation applies to.
        scope: CancelScope,
        /// Why.
        reason: CancelReason,
    },
    /// Return chunk-window credit.
    Drained {
        /// The request the chunks belonged to.
        request: RequestId,
        /// How many chunks the driver consumed.
        chunks: u32,
    },
    /// A driver-fired deadline. One carrying a stale generation is ignored.
    DeadlineFired {
        /// Which deadline fired.
        kind: DeadlineKind,
        /// The generation it was armed in.
        generation: u64,
    },
}

impl fmt::Debug for SynthesisInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enqueue {
                request,
                text,
                replace,
            } => f
                .debug_struct("Enqueue")
                .field("request", request)
                .field(
                    "text",
                    &Redacted::octets(DataClass::SynthesisInput, text.len()),
                )
                .field("replace", replace)
                .finish(),
            Self::Cancel { scope, reason } => f
                .debug_struct("Cancel")
                .field("scope", scope)
                .field("reason", reason)
                .finish(),
            Self::Drained { request, chunks } => f
                .debug_struct("Drained")
                .field("request", request)
                .field("chunks", chunks)
                .finish(),
            Self::DeadlineFired { kind, generation } => f
                .debug_struct("DeadlineFired")
                .field("kind", kind)
                .field("generation", generation)
                .finish(),
        }
    }
}

/// What a synthesis session emits (§6).
///
/// Extended compatibly, so a consumer writes a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SynthesisOutput {
    /// The session is warming. `Started` may not precede `Ready`.
    Warming,
    /// The session is ready.
    Ready,
    /// The request is queued, at this position.
    Accepted {
        /// The request queued.
        request: RequestId,
        /// Its position in the queue.
        position: usize,
    },
    /// The request was not queued.
    Refused {
        /// The request refused.
        request: RequestId,
        /// Why.
        reason: SynthesisRefusal,
    },
    /// The request began producing audio.
    Started {
        /// The request that started.
        request: RequestId,
    },
    /// Audio for the started request.
    Chunk(SynthesisChunk),
    /// A named production gap inside the current request.
    Discontinuity {
        /// The request the gap is inside.
        request: RequestId,
        /// The gap's duration in samples.
        samples: u64,
    },
    /// Terminal: the request produced all of its audio.
    Completed {
        /// The request that completed.
        request: RequestId,
        /// Total samples produced.
        samples: u64,
    },
    /// Terminal for one request, with a typed reason.
    Cancelled {
        /// The request cancelled.
        request: RequestId,
        /// Why.
        reason: CancelReason,
    },
    /// Terminal for a request, or — with no request identity — for the session.
    Failed {
        /// The request that failed, or `None` for the session.
        request: Option<RequestId>,
        /// Why.
        cause: FailureCause,
    },
    /// The provider's engine or execution device became unavailable (§7).
    ///
    /// Open work has already been resolved terminally when this is emitted.
    Lost(LossCause),
    /// Nothing owned remains. Always the last output.
    Stopped {
        /// Whether the driver aborted the provider at the drain deadline.
        aborted: bool,
    },
}

/// One per-call synthesis session (§2, §6).
///
/// Sans-I/O on the same terms as [`RecognitionSession`](super::RecognitionSession), and
/// implementable downstream for the same reason.
pub trait SynthesisSession {
    /// Deliver one input. Inputs are consumed in the order they are delivered.
    fn deliver(&mut self, input: SynthesisInput);

    /// Take the next output, oldest first, or `None` when none is pending.
    fn poll_output(&mut self) -> Option<SynthesisOutput>;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use sipx_audio::{PcmEncoding, PcmFormat};

    /// §6: chunk `n + 1`'s offset is chunk `n`'s offset plus chunk `n`'s duration.
    #[test]
    fn chunk_offsets_are_contiguous_by_construction() {
        let format = PcmFormat::new(8_000, PcmEncoding::Signed16).unwrap();
        let request = RequestId::new(0);
        let first = SynthesisChunk::new(request, 0, 0, Pcm::from_i16(format, vec![0; 160]));
        let second = SynthesisChunk::new(
            request,
            1,
            first.offset() + first.pcm().samples().len() as u64,
            Pcm::from_i16(format, vec![0; 160]),
        );
        assert_eq!(second.offset(), 160);
        assert_eq!(second.sequence(), first.sequence() + 1);
    }
}
