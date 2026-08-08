//! The bounded call PCM processing seam (`docs/specs/call-audio-seam.md`, `M-54`).
//!
//! Two epics want live call audio — local speech and deterministic call-audio analysis — and
//! neither may tap the media path itself. This is the one tap: an application attaches a processor
//! to a running [`MediaSession`](crate::MediaSession) and receives owned linear PCM in a format it
//! chose, with the metadata needed to put every sample on a timeline.
//!
//! Two properties shape everything here.
//!
//! **Offering a frame never waits.** The media loops offer to this seam on the same task that
//! decodes and encodes RTP. Offering takes a lock, moves a `Vec`, and returns; there is no await
//! and no channel to fill. A processor that has stopped reading therefore cannot stall decode,
//! encode, playback or capture — it can only lose its own frames, which it is told about.
//!
//! **Fan-out is by independent copy.** Every attachment owns its queue, its resampler, its
//! sequence and its epoch. Two consumers of one call observe the same audio without observing each
//! other, and no state is shared between two attachments, two directions or two calls.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use sipx_audio::{LinearResampler, Pcm, PcmError, PcmFormat};

use crate::counters::DiscardMeters;

/// Which side of the call an attachment observes (`docs/specs/call-audio-seam.md` §3).
///
/// Closed by design: a call has two audio directions, and a third would be a different contract
/// rather than a new variant of this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioDirection {
    /// Audio decoded from the remote peer, tapped at the jitter buffer's output.
    Inbound,
    /// Audio produced locally for transmission, tapped after the mute gate and before encoding.
    Outbound,
}

impl std::fmt::Display for AudioDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inbound => f.write_str("inbound"),
            Self::Outbound => f.write_str("outbound"),
        }
    }
}

/// Why a processor's sample timeline broke (`docs/specs/call-audio-seam.md` §7).
///
/// The vocabulary is shared with `docs/specs/call-audio-processing.md` §3.3 and
/// `docs/specs/speech-providers.md` §5, and is extended compatibly, so a consumer writes a
/// wildcard arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DiscontinuityKind {
    /// Upstream audio never became a frame: the decoder refused a packet.
    Loss,
    /// The seam's bounded-queue policy dropped frames this attachment had not read.
    Overflow,
    /// The seam re-anchored the timeline; the epoch restarts at sample time zero.
    Realign,
}

impl DiscontinuityKind {
    /// How disruptive this cause is, for coalescing several into one (§7).
    ///
    /// `Realign` subsumes a span because it restarts the epoch; the seam's own loss is named ahead
    /// of upstream loss because it is the one the consumer can do something about.
    const fn severity(self) -> u8 {
        match self {
            Self::Loss => 0,
            Self::Overflow => 1,
            Self::Realign => 2,
        }
    }
}

impl std::fmt::Display for DiscontinuityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loss => f.write_str("loss"),
            Self::Overflow => f.write_str("overflow"),
            Self::Realign => f.write_str("realign"),
        }
    }
}

/// The break immediately before the frame carrying it, and how much it swallowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Discontinuity {
    kind: DiscontinuityKind,
    frames: u64,
    samples: u64,
}

impl Discontinuity {
    /// What caused the break. When several causes coalesced, the most disruptive of them.
    #[must_use]
    pub const fn kind(self) -> DiscontinuityKind {
        self.kind
    }

    /// How many frames the gap swallowed.
    #[must_use]
    pub const fn frames(self) -> u64 {
        self.frames
    }

    /// The gap's span in samples, at the attachment's own rate.
    ///
    /// Zero for [`DiscontinuityKind::Realign`]: the epoch restarts rather than skipping a span
    /// anything in the new timeline could be measured against.
    #[must_use]
    pub const fn samples(self) -> u64 {
        self.samples
    }
}

impl std::fmt::Display for Discontinuity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} of {} frame(s), {} sample(s)",
            self.kind, self.frames, self.samples
        )
    }
}

/// One PCM frame delivered to a processor (`docs/specs/call-audio-seam.md` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcmFrame {
    direction: AudioDirection,
    pcm: Pcm,
    sample_time: u64,
    sequence: u64,
    discontinuity: Option<Discontinuity>,
}

impl PcmFrame {
    /// Which side of the call this audio belongs to.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The owned samples, in the format this attachment asked for.
    #[must_use]
    pub const fn pcm(&self) -> &Pcm {
        &self.pcm
    }

    /// The rate and depth of [`Self::pcm`].
    #[must_use]
    pub const fn format(&self) -> PcmFormat {
        self.pcm.format()
    }

    /// Consume the frame and return its samples.
    #[must_use]
    pub fn into_pcm(self) -> Pcm {
        self.pcm
    }

    /// The position of the first sample within the attachment's current epoch, in samples at this
    /// frame's own rate.
    ///
    /// The epoch opens at attachment and re-opens at every [`DiscontinuityKind::Realign`]. It
    /// advances by each delivered frame's length and, across a gap, by the span the discontinuity
    /// names — so the timeline never compresses over loss.
    #[must_use]
    pub const fn sample_time(&self) -> u64 {
        self.sample_time
    }

    /// This attachment's frame number, counting every frame the seam offered it.
    ///
    /// A gap here is exactly the frames this consumer lost, and a gap is always flagged.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// The break immediately before this frame, if there was one.
    #[must_use]
    pub const fn discontinuity(&self) -> Option<Discontinuity> {
        self.discontinuity
    }
}

/// What a processor asks the seam for (`docs/specs/call-audio-seam.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processing {
    direction: AudioDirection,
    format: PcmFormat,
    queue_capacity: usize,
}

impl Processing {
    /// Frames a per-call queue holds by default.
    ///
    /// `docs/specs/speech-providers.md` §8's default recognition input bound, not a number chosen
    /// here.
    pub const DEFAULT_QUEUE_CAPACITY: usize = 32;
    /// The shallowest queue the seam accepts, from `docs/specs/call-audio-processing.md` §5.1.
    pub const MIN_QUEUE_CAPACITY: usize = 2;
    /// The deepest queue the seam accepts, from the same domain.
    pub const MAX_QUEUE_CAPACITY: usize = 4_096;

    /// Ask for one direction's audio in one format, with the default queue.
    #[must_use]
    pub const fn new(direction: AudioDirection, format: PcmFormat) -> Self {
        Self {
            direction,
            format,
            queue_capacity: Self::DEFAULT_QUEUE_CAPACITY,
        }
    }

    /// Choose how many frames this attachment buffers before the loss policy engages.
    ///
    /// Deeper tolerates a slower consumer; shallower bounds how stale the oldest queued frame can
    /// be. Neither changes the call.
    #[must_use]
    pub const fn with_queue_capacity(mut self, frames: usize) -> Self {
        self.queue_capacity = frames;
        self
    }

    /// The direction asked for.
    #[must_use]
    pub const fn direction(self) -> AudioDirection {
        self.direction
    }

    /// The format asked for.
    #[must_use]
    pub const fn format(self) -> PcmFormat {
        self.format
    }

    /// The queue depth asked for.
    #[must_use]
    pub const fn queue_capacity(self) -> usize {
        self.queue_capacity
    }
}

/// Why the seam refused an attachment (`docs/specs/call-audio-seam.md` §5).
///
/// Every variant leaves the call exactly as it was: the seam refuses rather than distorting audio
/// or dropping a session to satisfy a processor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProcessingError {
    /// The requested format is outside the `docs/specs/linear-pcm.md` boundary.
    ///
    /// The inner error is that boundary's own, reused rather than re-minted, so rate 0 and
    /// rate 384,001 refuse with exactly the type its PCM-4 vector names.
    #[error("the media session cannot convert to the requested processing format: {0}")]
    UnsupportedConversion(#[from] PcmError),
    /// The requested queue depth is outside `Processing`'s domain.
    #[error(
        "processor queue capacity {requested} is outside {}..={} frames",
        Processing::MIN_QUEUE_CAPACITY,
        Processing::MAX_QUEUE_CAPACITY
    )]
    QueueCapacity {
        /// What was asked for.
        requested: usize,
    },
    /// The session already carries as many processors as the seam bounds it to.
    #[error("the media session already carries {limit} processors")]
    TooManyProcessors {
        /// The per-session ceiling.
        limit: usize,
    },
    /// The session has stopped, so an attachment could never produce a frame.
    #[error("the media session has stopped")]
    SessionStopped,
}

/// A break the seam has recorded but not yet handed to a consumer.
///
/// Its span is in *source* samples — the session's media clock — because the attachment that will
/// receive it is the only thing that knows the rate to express it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Break {
    kind: DiscontinuityKind,
    frames: u64,
    source_samples: u64,
}

impl Break {
    /// Merge two causes into the one discontinuity a consumer sees (§7).
    fn merged(self, other: Self) -> Self {
        let kind = if other.kind.severity() > self.kind.severity() {
            other.kind
        } else {
            self.kind
        };
        Self {
            kind,
            frames: self.frames.saturating_add(other.frames),
            source_samples: self.source_samples.saturating_add(other.source_samples),
        }
    }

    /// Express the span at the consumer's rate (§7).
    fn resolve(self, source_rate: u32, target_rate: u32) -> Discontinuity {
        let samples = if self.kind == DiscontinuityKind::Realign {
            0
        } else {
            self.source_samples
                .saturating_mul(u64::from(target_rate))
                .checked_div(u64::from(source_rate))
                .unwrap_or(0)
        };
        Discontinuity {
            kind: self.kind,
            frames: self.frames,
            samples,
        }
    }
}

/// One frame as the media path offered it, before this attachment's own conversion.
#[derive(Debug)]
struct Offered {
    sequence: u64,
    source_rate: u32,
    samples: Vec<i16>,
    /// The break immediately before this frame.
    preceded_by: Option<Break>,
}

/// Everything one attachment's queue holds, behind one lock.
#[derive(Debug)]
struct QueueState {
    frames: VecDeque<Offered>,
    capacity: usize,
    /// Assigned to the next frame offered, dropped frames included, so a sequence gap is exactly
    /// what this consumer lost.
    next_sequence: u64,
    /// A break recorded while the queue was empty, waiting for the frame it precedes.
    ///
    /// Only ever set when `frames` is empty: with a frame present the break belongs to the front,
    /// which is what the consumer will receive next.
    pending: Option<Break>,
    closed: bool,
    detached: bool,
}

impl QueueState {
    /// Record a break against whatever the consumer will receive next.
    fn note(&mut self, brk: Break) {
        match self.frames.front_mut() {
            Some(front) => {
                front.preceded_by = Some(match front.preceded_by {
                    Some(existing) => existing.merged(brk),
                    None => brk,
                });
            }
            None => {
                self.pending = Some(match self.pending {
                    Some(existing) => existing.merged(brk),
                    None => brk,
                });
            }
        }
    }
}

/// One attachment's bounded queue, shared between the media loops and its consumer.
#[derive(Debug)]
struct Queue {
    state: Mutex<QueueState>,
    /// Woken on every push and on close. Durable: `notify_one` stores a permit, and the consumer
    /// registers before it reads the queue, so a push between those two cannot be lost.
    ready: tokio::sync::Notify,
    lost: AtomicU64,
}

pub(crate) fn hold<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    // A poisoned seam lock is a panic somewhere else in this process, not a reason to lose the
    // call's audio: the state behind it is a queue of frames with no invariant a panic can break.
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Queue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(QueueState {
                frames: VecDeque::with_capacity(capacity),
                capacity,
                next_sequence: 0,
                pending: None,
                closed: false,
                detached: false,
            }),
            ready: tokio::sync::Notify::new(),
            lost: AtomicU64::new(0),
        }
    }

    /// Offer one frame. Never waits, never grows, never fails.
    ///
    /// Returns how many frames the loss policy dropped to make room, for the session's own
    /// accounting (`docs/specs/call-audio-seam.md` §6.3).
    fn offer(&self, source_rate: u32, samples: &[i16]) -> u64 {
        let mut dropped = 0u64;
        {
            let mut state = hold(&self.state);
            if state.closed || state.detached {
                return 0;
            }
            let sequence = state.next_sequence;
            state.next_sequence = state.next_sequence.saturating_add(1);

            while state.frames.len() >= state.capacity {
                // discard: §6.1's loss policy. The frame is counted on this attachment's `lost`
                // meter below, added to the session's `processor_frames_lost` by the caller, and
                // named by the `Overflow` discontinuity `note` puts on the frame that follows it.
                let Some(evicted) = state.frames.pop_front() else {
                    break;
                };
                dropped = dropped.saturating_add(1);
                let carried = Break {
                    kind: DiscontinuityKind::Overflow,
                    frames: 1,
                    source_samples: evicted.samples.len() as u64,
                };
                let brk = match evicted.preceded_by {
                    Some(existing) => existing.merged(carried),
                    None => carried,
                };
                state.note(brk);
            }

            let preceded_by = if state.frames.is_empty() {
                state.pending.take()
            } else {
                None
            };
            state.frames.push_back(Offered {
                sequence,
                source_rate,
                samples: samples.to_vec(),
                preceded_by,
            });
        }
        if dropped > 0 {
            self.lost.fetch_add(dropped, Ordering::Relaxed);
        }
        self.ready.notify_one();
        dropped
    }

    /// Record upstream loss that never became a frame.
    fn note_loss(&self, kind: DiscontinuityKind, frames: u64, source_samples: u64) {
        {
            let mut state = hold(&self.state);
            if state.closed || state.detached {
                return;
            }
            state.note(Break {
                kind,
                frames,
                source_samples,
            });
        }
        self.ready.notify_one();
    }

    /// Re-anchor the timeline: drop what is queued and open a new epoch (§7).
    ///
    /// Returns the frames discarded, for the session's accounting.
    fn realign(&self) -> u64 {
        let discarded;
        {
            let mut state = hold(&self.state);
            if state.closed || state.detached {
                return 0;
            }
            // discard: §7. Frames queued under a media generation that no longer exists would land
            // in the new epoch as old audio at a new position. They are counted here and named by
            // the `Realign` the next frame carries.
            discarded = state.frames.drain(..).count() as u64;
            state.pending = None;
            state.note(Break {
                kind: DiscontinuityKind::Realign,
                frames: discarded,
                source_samples: 0,
            });
        }
        if discarded > 0 {
            self.lost.fetch_add(discarded, Ordering::Relaxed);
        }
        self.ready.notify_one();
        discarded
    }

    fn close(&self) {
        hold(&self.state).closed = true;
        self.ready.notify_waiters();
    }

    fn detach(&self) {
        let mut state = hold(&self.state);
        state.detached = true;
        state.frames.clear();
        state.pending = None;
        drop(state);
        self.ready.notify_waiters();
    }

    fn is_gone(&self) -> bool {
        hold(&self.state).detached
    }

    /// Take the next frame, or report that no more are coming.
    fn take(&self) -> Taken {
        let mut state = hold(&self.state);
        match state.frames.pop_front() {
            Some(frame) => Taken::Frame(frame),
            None if state.closed || state.detached => Taken::Finished,
            None => Taken::Empty,
        }
    }
}

enum Taken {
    Frame(Offered),
    Empty,
    Finished,
}

/// One registered attachment, as the session sees it.
#[derive(Debug)]
struct Attachment {
    direction: AudioDirection,
    queue: Arc<Queue>,
}

/// A session's processor registry: the one place live call audio leaves the media path.
#[derive(Debug)]
pub(crate) struct Taps {
    attached: Mutex<Vec<Attachment>>,
    discards: Arc<DiscardMeters>,
    closed: AtomicBool,
}

impl Taps {
    /// The per-session attachment ceiling (`docs/specs/call-audio-seam.md` §5).
    ///
    /// Each attachment is a queue whose worst case is its own capacity, so an unbounded count
    /// would make per-call memory unbounded while every individual bound still held.
    pub(crate) const LIMIT: usize = 8;

    pub(crate) fn new(discards: Arc<DiscardMeters>) -> Self {
        Self {
            attached: Mutex::new(Vec::new()),
            discards,
            closed: AtomicBool::new(false),
        }
    }

    /// Register one processor against a session running at `source_rate`.
    pub(crate) fn attach(
        &self,
        request: Processing,
        source_rate: u32,
    ) -> Result<PcmProcessor, ProcessingError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(ProcessingError::SessionStopped);
        }
        if request.queue_capacity < Processing::MIN_QUEUE_CAPACITY
            || request.queue_capacity > Processing::MAX_QUEUE_CAPACITY
        {
            return Err(ProcessingError::QueueCapacity {
                requested: request.queue_capacity,
            });
        }
        // Before any queue is allocated, and through the shared boundary rather than a private
        // domain check, so an unsupported rate refuses with exactly that boundary's type.
        let resampler = LinearResampler::new(source_rate, request.format.sample_rate())?;

        let mut attached = hold(&self.attached);
        attached.retain(|entry| !entry.queue.is_gone());
        if attached.len() >= Self::LIMIT {
            return Err(ProcessingError::TooManyProcessors { limit: Self::LIMIT });
        }
        let queue = Arc::new(Queue::new(request.queue_capacity));
        attached.push(Attachment {
            direction: request.direction,
            queue: Arc::clone(&queue),
        });
        drop(attached);

        Ok(PcmProcessor {
            queue,
            direction: request.direction,
            format: request.format,
            source_rate,
            resampler,
            epoch: 0,
        })
    }

    /// Hand one direction's frame to every processor attached to it.
    ///
    /// Called from the media loops. It takes a lock, copies the samples once per attachment, and
    /// returns; it never awaits and never blocks on a consumer.
    pub(crate) fn offer(&self, direction: AudioDirection, source_rate: u32, samples: &[i16]) {
        let mut lost = 0u64;
        {
            let mut attached = hold(&self.attached);
            if attached.is_empty() {
                return;
            }
            attached.retain(|entry| !entry.queue.is_gone());
            for entry in attached.iter() {
                if entry.direction == direction {
                    lost = lost.saturating_add(entry.queue.offer(source_rate, samples));
                }
            }
        }
        if lost > 0 {
            self.discards
                .processor_frames_lost
                .fetch_add(lost, Ordering::Relaxed);
        }
    }

    /// Tell one direction's processors that upstream audio never became a frame.
    pub(crate) fn note_loss(&self, direction: AudioDirection, source_samples: u64) {
        let attached = hold(&self.attached);
        for entry in attached.iter() {
            if entry.direction == direction {
                entry
                    .queue
                    .note_loss(DiscontinuityKind::Loss, 1, source_samples);
            }
        }
    }

    /// Carry a previous generation's attachments across a renegotiation, re-anchored (§7, §8).
    pub(crate) fn adopt(&self, previous: &Self) {
        let migrating: Vec<Attachment> = {
            let mut held = hold(&previous.attached);
            held.drain(..).collect()
        };
        if migrating.is_empty() {
            return;
        }
        let mut discarded = 0u64;
        for entry in &migrating {
            discarded = discarded.saturating_add(entry.queue.realign());
        }
        hold(&self.attached).extend(migrating);
        if discarded > 0 {
            self.discards
                .processor_frames_lost
                .fetch_add(discarded, Ordering::Relaxed);
        }
    }

    /// Complete every attachment: each drains what it holds, then reports no more frames.
    pub(crate) fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let attached = hold(&self.attached);
        for entry in attached.iter() {
            entry.queue.close();
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        hold(&self.attached).len()
    }
}

/// A sole-consumer view of one direction of one call's audio.
///
/// The handle owns its rate conversion, so consecutive frames form one continuous stream. Dropping
/// it detaches: an abandoned processor is released immediately rather than buffering audio nobody
/// will read.
#[derive(Debug)]
pub struct PcmProcessor {
    queue: Arc<Queue>,
    direction: AudioDirection,
    format: PcmFormat,
    /// The session rate the resampler was built against; a change re-anchors the timeline.
    source_rate: u32,
    resampler: LinearResampler,
    /// The next sample position in the current epoch.
    epoch: u64,
}

impl PcmProcessor {
    /// The direction this processor observes.
    #[must_use]
    pub const fn direction(&self) -> AudioDirection {
        self.direction
    }

    /// The format every frame is delivered in.
    #[must_use]
    pub const fn format(&self) -> PcmFormat {
        self.format
    }

    /// How many frames this attachment has lost, over its whole life.
    ///
    /// Monotonic, and never affected by another attachment's losses.
    #[must_use]
    pub fn lost_frames(&self) -> u64 {
        self.queue.lost.load(Ordering::Relaxed)
    }

    /// Take the next frame if one is already queued.
    ///
    /// `None` means nothing is waiting *now*; it does not mean the attachment is finished. Use
    /// [`Self::recv`] to wait for the next frame or for completion.
    #[must_use]
    pub fn try_recv(&mut self) -> Option<PcmFrame> {
        match self.queue.take() {
            Taken::Frame(offered) => Some(self.convert(&offered)),
            Taken::Empty | Taken::Finished => None,
        }
    }

    /// Wait for the next frame.
    ///
    /// Returns `None` once the attachment is finished — the session stopped or the handle was
    /// detached — and everything it already held has been drained. That completion is an event, so
    /// a caller never waits a fixed duration to learn the seam is done.
    pub async fn recv(&mut self) -> Option<PcmFrame> {
        loop {
            // The wait is scoped so it is released before the frame is converted, and registered
            // before the queue is read: the opposite order has a lost-wake window, where a push
            // between the read and the await leaves this future parked with a frame waiting.
            let offered = {
                let ready = self.queue.ready.notified();
                tokio::pin!(ready);
                ready.as_mut().enable();
                match self.queue.take() {
                    Taken::Frame(offered) => offered,
                    Taken::Finished => return None,
                    Taken::Empty => {
                        ready.await;
                        continue;
                    }
                }
            };
            return Some(self.convert(&offered));
        }
    }

    /// Release this attachment now, rather than at the end of its scope.
    pub fn detach(self) {
        drop(self);
    }

    /// Apply `M-43`'s conversion and place the frame on this attachment's timeline.
    fn convert(&mut self, offered: &Offered) -> PcmFrame {
        // A source-rate change is a re-anchor whichever way the seam learned about it: the
        // interpolation history belongs to the old rate and carrying it forward would bend the
        // first frames of the new one.
        let mut brk = offered.preceded_by;
        if offered.source_rate != self.source_rate {
            self.source_rate = offered.source_rate;
            // Both rates are inside the boundary — one came through `attach`, the other from a
            // running session's media clock — so the refusal arm is unreachable. Keeping the
            // previous converter there rather than unwrapping keeps the media path panic-free.
            if let Ok(fresh) = LinearResampler::new(offered.source_rate, self.format.sample_rate())
            {
                self.resampler = fresh;
            }
            let realign = Break {
                kind: DiscontinuityKind::Realign,
                frames: 0,
                source_samples: 0,
            };
            brk = Some(match brk {
                Some(existing) => existing.merged(realign),
                None => realign,
            });
        }

        let discontinuity =
            brk.map(|brk| brk.resolve(offered.source_rate, self.format.sample_rate()));
        if let Some(discontinuity) = discontinuity {
            if discontinuity.kind == DiscontinuityKind::Realign {
                self.epoch = 0;
            } else {
                self.epoch = self.epoch.saturating_add(discontinuity.samples);
            }
        }

        let converted = self.resampler.push_i16(&offered.samples);
        let sample_time = self.epoch;
        self.epoch = self.epoch.saturating_add(converted.len() as u64);
        PcmFrame {
            direction: self.direction,
            pcm: Pcm::from_i16(self.format, converted),
            sample_time,
            sequence: offered.sequence,
            discontinuity,
        }
    }
}

impl Drop for PcmProcessor {
    fn drop(&mut self) {
        self.queue.detach();
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;
    use sipx_audio::{PcmEncoding, PcmSamples};

    fn meters() -> Arc<DiscardMeters> {
        Arc::new(DiscardMeters::default())
    }

    fn narrowband() -> PcmFormat {
        PcmFormat::new(8_000, PcmEncoding::Signed16).unwrap()
    }

    fn samples(of: &PcmFrame) -> &[i16] {
        match of.pcm().samples() {
            PcmSamples::Signed16(samples) => samples,
            other => panic!("expected signed samples, got {other:?}"),
        }
    }

    /// SEAM-6: the loss policy drops the oldest, coalesces, and names the gap on the frame that
    /// follows it.
    #[test]
    fn a_full_queue_drops_the_oldest_and_names_the_gap() {
        let taps = Taps::new(meters());
        let mut processor = taps
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()).with_queue_capacity(2),
                8_000,
            )
            .unwrap();
        for value in 0..5i16 {
            taps.offer(AudioDirection::Inbound, 8_000, &[value; 160]);
        }

        let first = processor.try_recv().unwrap();
        assert_eq!(first.sequence(), 3);
        assert_eq!(first.sample_time(), 3 * 160);
        let gap = first.discontinuity().unwrap();
        assert_eq!(gap.kind(), DiscontinuityKind::Overflow);
        assert_eq!(gap.frames(), 3);
        assert_eq!(gap.samples(), 3 * 160);
        assert_eq!(samples(&first)[0], 3);

        let second = processor.try_recv().unwrap();
        assert_eq!(second.sequence(), 4);
        assert_eq!(second.sample_time(), 4 * 160);
        assert_eq!(second.discontinuity(), None);

        assert!(processor.try_recv().is_none());
        assert_eq!(processor.lost_frames(), 3);
        assert_eq!(
            taps.discards.processor_frames_lost.load(Ordering::Relaxed),
            3
        );
    }

    /// §7: several causes before one delivery coalesce into one discontinuity, named by the most
    /// disruptive of them, with the spans added.
    #[test]
    fn several_causes_coalesce_into_one_discontinuity() {
        let taps = Taps::new(meters());
        let mut processor = taps
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()).with_queue_capacity(2),
                8_000,
            )
            .unwrap();
        taps.note_loss(AudioDirection::Inbound, 160);
        for _ in 0..3 {
            taps.offer(AudioDirection::Inbound, 8_000, &[7i16; 160]);
        }

        let first = processor.try_recv().unwrap();
        let gap = first.discontinuity().unwrap();
        assert_eq!(
            gap.kind(),
            DiscontinuityKind::Overflow,
            "the seam's own loss is named ahead of upstream loss"
        );
        assert_eq!(gap.frames(), 2, "one decoder loss and one dropped frame");
        assert_eq!(gap.samples(), 2 * 160);
        assert_eq!(first.sample_time(), 2 * 160);
    }

    /// SEAM-12: a re-anchor discards the queue, opens a new epoch, and names itself.
    #[test]
    fn a_realign_restarts_the_epoch() {
        let discards = meters();
        let before = Taps::new(Arc::clone(&discards));
        let after = Taps::new(Arc::clone(&discards));
        let mut processor = before
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()),
                8_000,
            )
            .unwrap();
        taps_offer(&before, 2);
        assert_eq!(processor.try_recv().unwrap().sample_time(), 0);

        after.adopt(&before);
        assert_eq!(before.len(), 0);
        assert_eq!(after.len(), 1);

        taps_offer(&after, 1);
        let frame = processor.try_recv().unwrap();
        let gap = frame.discontinuity().unwrap();
        assert_eq!(gap.kind(), DiscontinuityKind::Realign);
        assert_eq!(gap.frames(), 1, "the frame still queued was discarded");
        assert_eq!(gap.samples(), 0);
        assert_eq!(frame.sample_time(), 0, "the epoch restarts");
    }

    fn taps_offer(taps: &Taps, frames: usize) {
        for _ in 0..frames {
            taps.offer(AudioDirection::Inbound, 8_000, &[1i16; 160]);
        }
    }

    /// SEAM-8/SEAM-9: two attachments share no queue, no epoch and no losses.
    #[test]
    fn attachments_are_independent() {
        let taps = Taps::new(meters());
        let mut prompt = taps
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()),
                8_000,
            )
            .unwrap();
        let mut slow = taps
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()).with_queue_capacity(2),
                8_000,
            )
            .unwrap();
        taps_offer(&taps, 5);

        for expected in 0..5u64 {
            let frame = prompt.try_recv().unwrap();
            assert_eq!(frame.sequence(), expected);
            assert_eq!(frame.discontinuity(), None);
        }
        assert_eq!(prompt.lost_frames(), 0);

        assert!(slow.try_recv().unwrap().discontinuity().is_some());
        assert_eq!(slow.lost_frames(), 3);
    }

    /// §3: the other direction's audio is not this attachment's.
    #[test]
    fn a_direction_only_receives_its_own_audio() {
        let taps = Taps::new(meters());
        let mut outbound = taps
            .attach(
                Processing::new(AudioDirection::Outbound, narrowband()),
                8_000,
            )
            .unwrap();
        taps.offer(AudioDirection::Inbound, 8_000, &[1i16; 160]);
        assert!(outbound.try_recv().is_none());
        taps.offer(AudioDirection::Outbound, 8_000, &[2i16; 160]);
        assert_eq!(samples(&outbound.try_recv().unwrap())[0], 2);
    }

    /// SEAM-4/SEAM-5/SEAM-13/SEAM-14: every refusal is typed and allocates nothing.
    #[test]
    fn refusals_are_typed() {
        let taps = Taps::new(meters());
        for rate in [0, 384_001] {
            let format = PcmFormat::new(8_000, PcmEncoding::Signed16).unwrap();
            let refusal = taps
                .attach(Processing::new(AudioDirection::Inbound, format), rate)
                .expect_err("an unsupported session rate is refused");
            assert!(matches!(
                refusal,
                ProcessingError::UnsupportedConversion(PcmError::UnsupportedSampleRate(_))
            ));
        }
        for capacity in [1, 4_097] {
            let refusal = taps
                .attach(
                    Processing::new(AudioDirection::Inbound, narrowband())
                        .with_queue_capacity(capacity),
                    8_000,
                )
                .expect_err("an out-of-domain capacity is refused");
            assert_eq!(
                refusal,
                ProcessingError::QueueCapacity {
                    requested: capacity
                }
            );
        }
        assert_eq!(taps.len(), 0, "a refusal registers nothing");

        let mut held = Vec::new();
        for _ in 0..Taps::LIMIT {
            held.push(
                taps.attach(
                    Processing::new(AudioDirection::Inbound, narrowband()),
                    8_000,
                )
                .unwrap(),
            );
        }
        assert_eq!(
            taps.attach(
                Processing::new(AudioDirection::Inbound, narrowband()),
                8_000
            )
            .expect_err("the ceiling holds"),
            ProcessingError::TooManyProcessors { limit: Taps::LIMIT }
        );

        taps.close();
        assert_eq!(
            taps.attach(
                Processing::new(AudioDirection::Inbound, narrowband()),
                8_000
            )
            .expect_err("a closed seam hands out nothing"),
            ProcessingError::SessionStopped
        );
    }

    /// SEAM-10: an abandoned attachment releases its frames and stops taking new ones.
    #[test]
    fn dropping_the_handle_detaches() {
        let taps = Taps::new(meters());
        let processor = taps
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()),
                8_000,
            )
            .unwrap();
        taps_offer(&taps, 2);
        drop(processor);
        taps_offer(&taps, 2);
        assert_eq!(taps.len(), 0, "the next offer prunes the dead attachment");
    }

    /// SEAM-11: a closed attachment drains, then completes.
    #[tokio::test]
    async fn closing_drains_then_completes() {
        let taps = Taps::new(meters());
        let mut processor = taps
            .attach(
                Processing::new(AudioDirection::Inbound, narrowband()),
                8_000,
            )
            .unwrap();
        taps_offer(&taps, 2);
        taps.close();
        assert!(processor.recv().await.is_some());
        assert!(processor.recv().await.is_some());
        assert!(processor.recv().await.is_none());
    }

    /// SEAM-3: conversion is `M-43`'s, and the epoch counts what was delivered.
    #[test]
    fn conversion_is_continuous_across_frames() {
        let taps = Taps::new(meters());
        let wideband = PcmFormat::new(16_000, PcmEncoding::Signed16).unwrap();
        let mut processor = taps
            .attach(Processing::new(AudioDirection::Inbound, wideband), 8_000)
            .unwrap();
        let first_input: Vec<i16> = (0..160i16).map(|index| index * 100).collect();
        let second_input: Vec<i16> = (160..320i16).map(|index| index * 100).collect();
        taps.offer(AudioDirection::Inbound, 8_000, &first_input);
        taps.offer(AudioDirection::Inbound, 8_000, &second_input);

        let first = processor.try_recv().unwrap();
        let second = processor.try_recv().unwrap();
        assert_eq!(first.format(), wideband);
        assert_eq!(first.sample_time(), 0);
        assert_eq!(second.sample_time(), samples(&first).len() as u64);

        let mut reference = LinearResampler::new(8_000, 16_000).unwrap();
        let mut combined = reference.push_i16(&first_input);
        combined.extend(reference.push_i16(&second_input));
        let mut seen = samples(&first).to_vec();
        seen.extend_from_slice(samples(&second));
        assert_eq!(seen, combined, "the seam holds one interpolation history");
    }
}
