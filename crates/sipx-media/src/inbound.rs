//! The queue between the receive loop and the application (`docs/specs/media-runtime.md` §4.3).
//!
//! This is the last hop of the inbound media path and the one place seconds of delay could
//! accumulate without anything saying so. `M-45` measured the jitter buffer ahead of it and
//! cleared it — bounded, no ratchet, worst measured hold 515 ms — so the bound, the policy and
//! the counter this module adds are what stand between a slow reader and a call that is minutes
//! behind live audio by the time it ends.
//!
//! Three decisions, each stated in §4.3 and enforced here.
//!
//! **The bound is a duration**, held as a number of *samples* at the session's audio rate rather
//! than a number of frames. A frame is 10 ms of audio in one codec and 60 in another, so a depth
//! counted in frames means a different delay in every session — and it means a different delay in
//! the *same* session if the far end changes its packetisation mid-call, which nothing on this
//! side can refuse. Counting the audio itself is the only form of the bound that is true whatever
//! arrives.
//!
//! **Overflow sheds the oldest frame.** The alternative policies both fail the thing this queue
//! exists to prevent: backpressure moves the delay into the socket, where it is unbounded and
//! uncounted, and shedding the newest leaves the application listening to the oldest audio it
//! could possibly still be holding. This is `call-audio-seam.md` §6.1's policy and
//! `speech-providers.md` §8's, for their reason: the newest frame is the one the consumer has the
//! best chance of still being able to use.
//!
//! **Every shed frame is counted**, as `MediaDiscardCounts::inbound_frames_shed`. §4's rule is
//! that a discard in the media path is counted or reasoned, and audio the far end sent that the
//! application will never hear is exactly a discard.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::counters::DiscardMeters;
use crate::processing::hold;

/// What is waiting, and whether more is coming.
#[derive(Debug)]
struct State {
    frames: VecDeque<Vec<i16>>,
    /// The total length of `frames`, kept alongside them so the bound is checked without walking
    /// the queue on every packet.
    queued: usize,
    closed: bool,
}

/// One session's inbound audio queue: bounded in time, shed-oldest, counted.
///
/// Shared between the receive loop, which pushes, and the application, which reads. Both halves
/// hold an `Arc` of it; there is no separate sender type because there is exactly one producer
/// and the session owns both ends for its whole life.
#[derive(Debug)]
pub(crate) struct InboundQueue {
    state: Mutex<State>,
    /// The bound, in samples at the session's audio rate. Zero is legal and means "one frame,
    /// whatever it is worth" — see [`Self::capacity_samples`].
    capacity: usize,
    /// Woken on every push and on close. Durable: `notify_one` stores a permit, and the reader
    /// registers before it looks at the queue, so a push between those two cannot be lost.
    ready: tokio::sync::Notify,
    /// Serialises concurrent readers. [`crate::session::MediaSession::recv`] takes `&self`, so
    /// two tasks may call it at once; without this they would both take the same wake-up and one
    /// would return a frame the other had already taken.
    reading: tokio::sync::Mutex<()>,
    discards: Arc<DiscardMeters>,
}

impl InboundQueue {
    /// A queue holding at most `bound` of audio sampled at `audio_rate`.
    pub(crate) fn new(bound: Duration, audio_rate: u32, discards: Arc<DiscardMeters>) -> Self {
        Self {
            state: Mutex::new(State {
                frames: VecDeque::new(),
                queued: 0,
                closed: false,
            }),
            capacity: Self::capacity_samples(bound, audio_rate),
            ready: tokio::sync::Notify::new(),
            reading: tokio::sync::Mutex::new(()),
            discards,
        }
    }

    /// How many samples `bound` is worth at `audio_rate`.
    ///
    /// Saturating rather than wrapping, and a `u128` intermediate, because the product of a
    /// caller-supplied `Duration` and a negotiated clock rate is not otherwise bounded by
    /// anything. A bound of zero yields zero, which [`Self::push`] reads as "hold one frame":
    /// the queue always accepts the frame in front of it, because delivering nothing is not a
    /// smaller delay, it is a silent call.
    fn capacity_samples(bound: Duration, audio_rate: u32) -> usize {
        let samples = bound.as_micros().saturating_mul(u128::from(audio_rate)) / 1_000_000;
        usize::try_from(samples).unwrap_or(usize::MAX)
    }

    /// Hand one frame to the application. Never waits and never grows.
    ///
    /// Returns whether the queue is still open — `false` once the session has stopped, which is
    /// the receive loop's signal that nothing is reading any more.
    pub(crate) fn push(&self, samples: Vec<i16>) -> bool {
        let mut shed = 0u64;
        {
            let mut state = hold(&self.state);
            if state.closed {
                return false;
            }
            // The frame in front is always accepted: a packetisation longer than the whole bound
            // would otherwise shed every frame on arrival and deliver silence forever.
            while !state.frames.is_empty()
                && state.queued.saturating_add(samples.len()) > self.capacity
            {
                // discard: §4.3's shed-oldest policy. Counted on `inbound_frames_shed` below —
                // outside this loop so one lock covers the whole eviction rather than one atomic
                // per frame.
                let Some(evicted) = state.frames.pop_front() else {
                    break;
                };
                state.queued = state.queued.saturating_sub(evicted.len());
                shed = shed.saturating_add(1);
            }
            state.queued = state.queued.saturating_add(samples.len());
            state.frames.push_back(samples);
        }
        if shed > 0 {
            self.discards
                .inbound_frames_shed
                .fetch_add(shed, Ordering::Relaxed);
        }
        self.ready.notify_one();
        true
    }

    /// Take the next frame, or report that no more are coming.
    ///
    /// `None` only once the queue is closed *and* drained, so stopping a session still delivers
    /// the audio it had already accepted rather than dropping it on the floor.
    pub(crate) async fn recv(&self) -> Option<Vec<i16>> {
        let _reader = self.reading.lock().await;
        loop {
            // Registered before the queue is read: the opposite order has a lost-wake window,
            // where a push between the read and the await leaves this future parked with a frame
            // waiting.
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            {
                let mut state = hold(&self.state);
                if let Some(frame) = state.frames.pop_front() {
                    state.queued = state.queued.saturating_sub(frame.len());
                    return Some(frame);
                }
                if state.closed {
                    return None;
                }
            }
            ready.await;
        }
    }

    /// No more audio is coming. Whatever is queued is still delivered.
    pub(crate) fn close(&self) {
        hold(&self.state).closed = true;
        self.ready.notify_waiters();
    }
}
