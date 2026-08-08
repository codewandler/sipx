//! The per-session limits a host exposes (`docs/specs/speech-providers.md` §8).
//!
//! Every bound here is **per session**, and sessions are per call: one call's stalled consumer
//! cannot consume another call's budget, and no queue is shared mutable state (`A-28`).
//!
//! Lowering a bound is supported; raising one is an explicit host decision. Zero is refused for
//! all of them, because a queue of zero is not a tighter policy — it is a session that can never
//! make progress, reported as a configuration error rather than discovered as a stall.
//!
//! The two durations bound *failure detection* only. Neither stands in for an ordering relation:
//! §10's vectors assert order by event order, never by waiting.

use std::time::Duration;

/// A bound that was set to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the speech bound `{name}` cannot be zero")]
pub struct ZeroBound {
    name: &'static str,
}

impl ZeroBound {
    /// Which bound was refused.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }
}

/// §8's limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechBounds {
    input_frames: usize,
    pending_revisions: usize,
    unconsumed_outputs: usize,
    queued_requests: usize,
    request_text_octets: usize,
    chunk_window: usize,
    warmup: Duration,
    drain: Duration,
}

impl Default for SpeechBounds {
    fn default() -> Self {
        Self::DEFAULTS
    }
}

impl SpeechBounds {
    /// §8's table, verbatim.
    pub const DEFAULTS: Self = Self {
        input_frames: 32,
        pending_revisions: 1,
        unconsumed_outputs: 16,
        queued_requests: 8,
        request_text_octets: 8_192,
        chunk_window: 4,
        warmup: Duration::from_secs(30),
        drain: Duration::from_secs(5),
    };

    /// Recognition input frames per session. At the bound the oldest is dropped and one
    /// `Discontinuity` names the accumulated loss.
    ///
    /// The default is also
    /// [`Processing::DEFAULT_QUEUE_CAPACITY`](crate::processing::Processing::DEFAULT_QUEUE_CAPACITY):
    /// the seam that carries these frames inherits this number rather than choosing its own, which
    /// is what makes the seam's loss policy *be* §5's input-bound obligation instead of resembling
    /// it.
    #[must_use]
    pub const fn input_frames(self) -> usize {
        self.input_frames
    }

    /// Pending non-terminal revisions per utterance. At the bound they coalesce, newest wins.
    #[must_use]
    pub const fn pending_revisions(self) -> usize {
        self.pending_revisions
    }

    /// Unconsumed terminal and lifecycle outputs per session. At the bound output consumption
    /// pauses and the input-frame policy absorbs the stall.
    #[must_use]
    pub const fn unconsumed_outputs(self) -> usize {
        self.unconsumed_outputs
    }

    /// Queued synthesis requests per session. At the bound an `Enqueue` is refused `QueueFull`.
    #[must_use]
    pub const fn queued_requests(self) -> usize {
        self.queued_requests
    }

    /// Octets of one synthesis request's text. Beyond it, `TextTooLarge`.
    #[must_use]
    pub const fn request_text_octets(self) -> usize {
        self.request_text_octets
    }

    /// Unconsumed synthesis chunks per session. The provider withholds production until `Drained`
    /// returns credit.
    #[must_use]
    pub const fn chunk_window(self) -> usize {
        self.chunk_window
    }

    /// How long a session may stay `Warming` before it fails `WarmupTimeout`.
    #[must_use]
    pub const fn warmup(self) -> Duration {
        self.warmup
    }

    /// How long a stop may take before the driver aborts and reports an aborted stop.
    #[must_use]
    pub const fn drain(self) -> Duration {
        self.drain
    }

    /// Set the recognition input-frame bound.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for zero.
    pub const fn with_input_frames(mut self, frames: usize) -> Result<Self, ZeroBound> {
        if frames == 0 {
            return Err(ZeroBound {
                name: "input frames",
            });
        }
        self.input_frames = frames;
        Ok(self)
    }

    /// Set the pending-revision bound.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for zero.
    pub const fn with_pending_revisions(mut self, revisions: usize) -> Result<Self, ZeroBound> {
        if revisions == 0 {
            return Err(ZeroBound {
                name: "pending revisions",
            });
        }
        self.pending_revisions = revisions;
        Ok(self)
    }

    /// Set the unconsumed-output bound.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for zero.
    pub const fn with_unconsumed_outputs(mut self, outputs: usize) -> Result<Self, ZeroBound> {
        if outputs == 0 {
            return Err(ZeroBound {
                name: "unconsumed outputs",
            });
        }
        self.unconsumed_outputs = outputs;
        Ok(self)
    }

    /// Set the queued-request bound.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for zero.
    pub const fn with_queued_requests(mut self, requests: usize) -> Result<Self, ZeroBound> {
        if requests == 0 {
            return Err(ZeroBound {
                name: "queued requests",
            });
        }
        self.queued_requests = requests;
        Ok(self)
    }

    /// Set the request-text bound, in octets.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for zero.
    pub const fn with_request_text_octets(mut self, octets: usize) -> Result<Self, ZeroBound> {
        if octets == 0 {
            return Err(ZeroBound {
                name: "request text octets",
            });
        }
        self.request_text_octets = octets;
        Ok(self)
    }

    /// Set the synthesis chunk window.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for zero.
    pub const fn with_chunk_window(mut self, chunks: usize) -> Result<Self, ZeroBound> {
        if chunks == 0 {
            return Err(ZeroBound {
                name: "chunk window",
            });
        }
        self.chunk_window = chunks;
        Ok(self)
    }

    /// Set the warm-up deadline.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for a zero duration.
    pub const fn with_warmup(mut self, warmup: Duration) -> Result<Self, ZeroBound> {
        if warmup.is_zero() {
            return Err(ZeroBound {
                name: "warm-up deadline",
            });
        }
        self.warmup = warmup;
        Ok(self)
    }

    /// Set the drain deadline.
    ///
    /// # Errors
    ///
    /// Returns [`ZeroBound`] for a zero duration.
    pub const fn with_drain(mut self, drain: Duration) -> Result<Self, ZeroBound> {
        if drain.is_zero() {
            return Err(ZeroBound {
                name: "drain deadline",
            });
        }
        self.drain = drain;
        Ok(self)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::processing::Processing;

    /// §8's defaults, and the one that two specifications share.
    #[test]
    fn the_defaults_are_the_specification_table() {
        let bounds = SpeechBounds::default();
        assert_eq!(bounds.input_frames(), 32);
        assert_eq!(bounds.pending_revisions(), 1);
        assert_eq!(bounds.unconsumed_outputs(), 16);
        assert_eq!(bounds.queued_requests(), 8);
        assert_eq!(bounds.request_text_octets(), 8_192);
        assert_eq!(bounds.chunk_window(), 4);
        assert_eq!(bounds.warmup(), Duration::from_secs(30));
        assert_eq!(bounds.drain(), Duration::from_secs(5));
        assert_eq!(
            bounds.input_frames(),
            Processing::DEFAULT_QUEUE_CAPACITY,
            "the seam's default queue is this bound, not a second number"
        );
    }

    /// §8: the host configuration refuses zero values with a typed error.
    #[test]
    fn every_bound_refuses_zero() {
        let bounds = SpeechBounds::DEFAULTS;
        let refusals = [
            bounds.with_input_frames(0),
            bounds.with_pending_revisions(0),
            bounds.with_unconsumed_outputs(0),
            bounds.with_queued_requests(0),
            bounds.with_request_text_octets(0),
            bounds.with_chunk_window(0),
            bounds.with_warmup(Duration::ZERO),
            bounds.with_drain(Duration::ZERO),
        ];
        let mut named = Vec::new();
        for refusal in refusals {
            named.push(refusal.expect_err("zero is refused").name());
        }
        named.sort_unstable();
        named.dedup();
        assert_eq!(named.len(), 8, "each bound names itself: {named:?}");
    }

    /// Lowering is supported and leaves everything else alone.
    #[test]
    fn lowering_one_bound_changes_only_that_bound() {
        let lowered = SpeechBounds::DEFAULTS
            .with_input_frames(4)
            .expect("four frames is a bound");
        assert_eq!(lowered.input_frames(), 4);
        assert_eq!(
            lowered.queued_requests(),
            SpeechBounds::DEFAULTS.queued_requests()
        );
    }
}
