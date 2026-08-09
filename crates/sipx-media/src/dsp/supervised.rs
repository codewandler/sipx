//! Supervised stages: application code the runtime runs off the media worker
//! (`docs/specs/call-dsp-graph.md` §7).
//!
//! The whole of this module exists to make one sentence true — **`process` is never called on the
//! media worker** — and to make the price of that placement exactly what
//! `docs/specs/custom-call-dsp.md` §7.2 says it is: a bounded wait, a declared action at its
//! expiry, and a termination and a reap.
//!
//! The media worker's contact with a supervised stage is two non-blocking operations against two
//! bounded channels: offer this frame, and take the result belonging to the frame `deadline_frames`
//! back. Neither awaits, neither blocks and neither allocates. A worker that hangs, loops, panics
//! or answers with the wrong number of samples costs the frames it owed and nothing else.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use sipx_audio::dsp::DspCapability;

use super::BypassCause;
use crate::session::Stop;

/// What one supervised worker did with one frame (`docs/specs/call-dsp-graph.md` §7.2).
///
/// A worker reports what it did rather than returning audio directly, because the samples it wrote
/// are in a buffer the runtime lent it and the runtime is what decides whether they were on time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WorkerResult {
    /// The worker wrote this frame's output into the buffer it was lent.
    ///
    /// The runtime still checks the position count: a result of the wrong length is a
    /// [`BypassCause::MalformedResult`] and not audio.
    Produced,
    /// The worker declined to answer this frame at all, and wrote nothing.
    ///
    /// Counted as a miss exactly as a deadline expiry is — "no result by the deadline" is the same
    /// fact whatever caused it.
    Withheld,
    /// The worker failed terminally and must be terminated and reaped.
    ///
    /// It is never restarted behind the application's back: a processor that failed is a processor
    /// whose state is gone, and a silent restart would silently reset audio state the application
    /// believes is continuous.
    Failed,
}

/// Application DSP the runtime runs off the media worker, reached only through bounded channels
/// (`docs/specs/call-dsp-graph.md` §7).
///
/// **What implementing this promises the application.** Over-budget work here cannot stall RTP:
/// the media worker offers a frame and takes a result only if one is present by the declared
/// deadline, so it never waits for this code. A hang, a loop, a panic or a malformed result costs
/// the declared failure action plus a termination and a reap.
///
/// **What it does not promise.** It does not make this code fast and it does not make it correct.
/// An abandoned result is audio that did not get processed, so a fail-open bypass here is audible:
/// the containment is of the stall, not of the artefact. Nor does it bound this worker's own memory
/// or CPU beyond what the operating system is configured to bound.
///
/// The declared [`DspCapability`] must name [`ExecutionProfile::SupervisedIsolated`]; a plan whose
/// supervised stage declares anything else is refused before it activates.
///
/// [`ExecutionProfile::SupervisedIsolated`]: sipx_audio::dsp::ExecutionProfile::SupervisedIsolated
pub trait SupervisedWorker: Send {
    /// Everything this worker declares before it may be attached
    /// (`docs/specs/custom-call-dsp.md` §5).
    ///
    /// Read once, at validation, and constant thereafter: every channel and buffer the stage owns
    /// is sized from it.
    fn capability(&self) -> DspCapability;

    /// Transform one frame, writing the output into `out`.
    ///
    /// `out` is empty on entry and is the runtime's buffer, lent for this call. `samples` is
    /// borrowed and MUST NOT be retained. Producing a different number of positions than `samples`
    /// carries is a malformed result, not a length policy.
    fn run(&mut self, samples: &[i16], out: &mut Vec<i16>) -> WorkerResult;
}

/// What the media worker got back from one exchange with a supervised stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exchange {
    /// A result for the frame at this deadline was taken and written to the output buffer.
    Produced,
    /// There was no usable result. The cause is what the transition will name.
    Missed(BypassCause),
}

/// One frame offered to a worker, carrying the buffer it borrows.
#[derive(Debug)]
struct Request {
    sequence: u64,
    samples: Vec<i16>,
}

/// One worker answer, carrying the buffer the runtime lent it back again.
#[derive(Debug)]
struct Response {
    sequence: u64,
    result: WorkerResult,
    samples: Vec<i16>,
}

/// The runtime half of a supervised stage: two bounded channels and one worker it owns.
///
/// Every buffer this will ever use is allocated here, at validation. The pool is `capacity + 1`
/// deep so that one frame can be in flight for every slot of the request channel and one more can
/// be being filled; when it is empty the stage misses rather than allocating, because a bounded
/// channel that grows a buffer under pressure is not bounded.
#[derive(Debug)]
pub(crate) struct Supervised {
    requests: Option<SyncSender<Request>>,
    results: Receiver<Response>,
    worker: Option<JoinHandle<()>>,
    reaped: Arc<Stop>,
    pool: Vec<Vec<i16>>,
    held: VecDeque<Response>,
    sequence: u64,
    in_flight: u32,
    lost: bool,
}

impl Supervised {
    /// Spawn the worker and allocate every buffer and channel slot the stage will ever use.
    ///
    /// The thread is the graph's, joined at the teardown barrier
    /// (`docs/specs/call-dsp-graph.md` §8); nothing on the live path ever spawns.
    pub(crate) fn spawn(
        mut worker: Box<dyn SupervisedWorker>,
        capacity: u32,
        frame_samples: usize,
    ) -> Self {
        let depth = capacity as usize;
        let (requests, inbox) = sync_channel::<Request>(depth);
        let (outbox, results) = sync_channel::<Response>(depth);
        let reaped = Arc::new(Stop::default());
        let finished = Arc::clone(&reaped);

        let handle = std::thread::spawn(move || {
            serve(worker.as_mut(), &inbox, &outbox, frame_samples);
            // The reap signal is durable as well as prompt, so a barrier that arrives after the
            // thread has already finished answers instead of parking forever.
            finished.stop();
        });

        Self {
            requests: Some(requests),
            results,
            worker: Some(handle),
            reaped,
            pool: (0..=depth)
                .map(|_| Vec::with_capacity(frame_samples))
                .collect(),
            held: VecDeque::with_capacity(depth),
            sequence: 0,
            in_flight: 0,
            lost: false,
        }
    }

    /// Offer one frame and take the result belonging to the frame `deadline_frames` back.
    ///
    /// Never awaits, never blocks and never allocates. Returns [`Exchange::Missed`] whenever there
    /// is no usable result, which the caller counts against
    /// `docs/specs/custom-call-dsp.md` §7.4's miss budget.
    pub(crate) fn exchange(
        &mut self,
        samples: &[i16],
        deadline_frames: u32,
        out: &mut Vec<i16>,
    ) -> Exchange {
        if self.lost {
            return Exchange::Missed(BypassCause::WorkerLost);
        }
        let offered = self.sequence;
        self.offer(offered, samples);
        self.sequence = self.sequence.saturating_add(1);
        self.collect();

        // §7.1: the worker has `deadline_frames` frame durations to answer, so the result due now
        // is the one for the frame that many back. While the pipeline fills there is none, and
        // that is a miss like any other rather than a special case.
        let Some(due) = offered.checked_sub(u64::from(deadline_frames)) else {
            return Exchange::Missed(BypassCause::DeadlineMissed);
        };
        self.take(due, samples.len(), out)
    }

    /// Offer one frame, or record why it could not be offered.
    fn offer(&mut self, sequence: u64, samples: &[i16]) {
        let Some(requests) = self.requests.as_ref() else {
            self.lost = true;
            return;
        };
        // discard: `docs/specs/call-dsp-graph.md` §7. An exhausted pool means every buffer this
        // stage owns is already in flight, which is the bounded channel's budget being spent. The
        // frame is not offered and the caller counts a miss; allocating here would make the bound
        // a suggestion.
        let Some(mut buffer) = self.pool.pop() else {
            return;
        };
        buffer.clear();
        buffer.extend_from_slice(samples);
        match requests.try_send(Request {
            sequence,
            samples: buffer,
        }) {
            Ok(()) => self.in_flight = self.in_flight.saturating_add(1),
            Err(TrySendError::Full(request)) => self.pool.push(request.samples),
            Err(TrySendError::Disconnected(request)) => {
                self.pool.push(request.samples);
                self.lost = true;
            }
        }
    }

    /// Drain whatever the worker has answered, without waiting for anything.
    fn collect(&mut self) {
        while let Ok(response) = self.results.try_recv() {
            self.in_flight = self.in_flight.saturating_sub(1);
            self.held.push_back(response);
        }
    }

    /// Take the result for `due`, recycling everything older than it.
    fn take(&mut self, due: u64, positions: usize, out: &mut Vec<i16>) -> Exchange {
        // discard: §7.2. A result older than the one due is a result whose audio has already been
        // sent; it is abandoned unread, and the miss it stands for was counted at its own frame.
        while self.held.front().is_some_and(|held| held.sequence < due) {
            if let Some(stale) = self.held.pop_front() {
                self.pool.push(stale.samples);
            }
        }
        let Some(response) = self.held.front() else {
            return Exchange::Missed(BypassCause::DeadlineMissed);
        };
        if response.sequence != due {
            return Exchange::Missed(BypassCause::DeadlineMissed);
        }
        let Some(response) = self.held.pop_front() else {
            return Exchange::Missed(BypassCause::DeadlineMissed);
        };
        let verdict = match response.result {
            WorkerResult::Produced if response.samples.len() == positions => {
                out.clear();
                out.extend_from_slice(&response.samples);
                Exchange::Produced
            }
            WorkerResult::Produced => Exchange::Missed(BypassCause::MalformedResult),
            WorkerResult::Withheld => Exchange::Missed(BypassCause::DeadlineMissed),
            WorkerResult::Failed => {
                self.lost = true;
                Exchange::Missed(BypassCause::WorkerLost)
            }
        };
        self.pool.push(response.samples);
        verdict
    }

    /// How many frames this stage has offered whose result it has not accounted for (§8).
    pub(crate) const fn frames_in_flight(&self) -> u32 {
        self.in_flight
    }

    /// Whether the worker is still running.
    pub(crate) fn is_live(&self) -> bool {
        self.worker.is_some()
    }

    /// Terminate the worker: close the request channel so its next receive returns.
    ///
    /// Separate from [`Self::reap`] because the barrier's wait must not be what starts the
    /// termination — a caller that terminates every stage first lets all of them wind down at once.
    pub(crate) fn terminate(&mut self) {
        self.requests = None;
    }

    /// Wait for the worker to have finished, then join it (§7.3).
    ///
    /// The wait is an event and never a duration: the worker's last act is to signal, so a stopped
    /// session's teardown answers rather than holding a runtime worker nothing can reclaim.
    pub(crate) async fn reap(&mut self) {
        self.terminate();
        let Some(handle) = self.worker.take() else {
            return;
        };
        self.reaped.wait().await;
        // The signal is the worker's last act, so this join finds a finished thread and returns
        // without parking the runtime. A worker that panicked is reaped exactly the same way.
        drop(handle.join());
        // Both channel ends and every buffer they carried are dropped with this stage. Nothing
        // holds a frame of this call once the worker is gone, which is what the barrier reports.
        self.held.clear();
        self.pool.clear();
        self.in_flight = 0;
    }
}

/// The worker thread's whole life.
///
/// It owns two buffers and swaps their roles each frame — the request's buffer becomes the next
/// output — so the steady state allocates nothing on either side of the channel.
fn serve(
    worker: &mut dyn SupervisedWorker,
    inbox: &Receiver<Request>,
    outbox: &SyncSender<Response>,
    frame_samples: usize,
) {
    let mut spare: Vec<i16> = Vec::with_capacity(frame_samples);
    while let Ok(request) = inbox.recv() {
        spare.clear();
        // A panicking worker is a finding, not a reason to take the process down with it: it is
        // caught here, reported as a terminal failure and reaped. This needs the unwinding panic
        // strategy, exactly as `docs/specs/custom-call-dsp.md` §11.1 notes for the harness.
        let result = catch_unwind(AssertUnwindSafe(|| {
            worker.run(&request.samples, &mut spare)
        }))
        .unwrap_or(WorkerResult::Failed);
        let terminal = matches!(result, WorkerResult::Failed);
        let response = Response {
            sequence: request.sequence,
            result,
            samples: std::mem::take(&mut spare),
        };
        spare = request.samples;
        // discard: §7. A full result channel is the media worker not taking results, which is
        // already a miss on its side; blocking here would make this worker's liveness depend on the
        // media loop's. The buffer goes with it, and the stage misses rather than allocating.
        drop(outbox.try_send(response));
        if terminal {
            return;
        }
    }
}
