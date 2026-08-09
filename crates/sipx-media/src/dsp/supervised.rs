//! Supervised stages: application code the runtime runs in a process of its own
//! (`docs/specs/call-dsp-graph.md` §7).
//!
//! The whole of this module exists to make two sentences true — **`process` is never called on the
//! media worker**, and **a worker that will not stop can be terminated regardless** — and to make
//! the price of that placement exactly what `docs/specs/custom-call-dsp.md` §7.2 says it is: a
//! bounded wait, a declared action at its expiry, and a termination and a reap.
//!
//! The media worker's contact with a supervised stage is two non-blocking operations against two
//! bounded channels: offer this frame, and take the result belonging to the frame `deadline_frames`
//! back. Neither awaits, neither blocks and neither allocates. Between those channels and the
//! worker process sits a **pump**, on a thread of its own, and it is the only thing that touches a
//! pipe. A worker that hangs, loops, panics, crashes or answers with the wrong number of samples
//! costs the frames it owed and nothing else.
//!
//! Two threads run per stage, and they are two rather than one because they block on different
//! things. The pump blocks on a pipe. The **reaper** blocks on the termination signal, owns the
//! child, and is what makes `terminate` a non-blocking call the media worker may make: it kills and
//! `wait`s on its own thread, and never on anyone else's.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, BufReader};
use std::process::{ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use sipx_audio::dsp::{DspCapability, StreamFormat};

use super::BypassCause;
use super::wire::{self, Fault, Incoming};
use super::worker::WorkerResult;
use crate::processing::{AudioDirection, DiscontinuityKind};
use crate::session::Stop;

/// The program a supervised stage runs, and what it declares before it may be attached
/// (`docs/specs/call-dsp-graph.md` §7.4).
///
/// A supervised worker is a **program**, not a callback: the runtime spawns it, writes frames to
/// its standard input and reads results from its standard output. It may be the application's own
/// executable re-invoked with an argument — [`std::env::current_exe`] and a flag its `main` looks
/// for is the shape most applications want — a separate program, or the `sipx-dsp-worker` reference
/// worker this crate ships. What runs inside it is [`SupervisedWorker`](super::SupervisedWorker)
/// and [`serve_worker`](super::serve_worker).
///
/// The capability is declared **here**, on the runtime's side, and not asked of the process. A
/// chain is validated whole before anything activates (§3.1), and a validation that had to spawn a
/// process to ask it what it was would have already done the thing it was deciding whether to do.
#[derive(Debug, Clone)]
pub struct WorkerProcess {
    capability: DspCapability,
    program: OsString,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
}

impl WorkerProcess {
    /// Declare a worker program and the capability the runtime validates it against.
    ///
    /// The capability must name
    /// [`ExecutionProfile::SupervisedIsolated`](sipx_audio::dsp::ExecutionProfile::SupervisedIsolated);
    /// a plan whose supervised stage declares anything else is refused before it activates.
    #[must_use]
    pub fn new(capability: DspCapability, program: impl Into<OsString>) -> Self {
        Self {
            capability,
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
        }
    }

    /// Append one argument to the worker's command line.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Set one environment variable for the worker, on top of the ones it inherits.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    /// What this stage declares (`docs/specs/custom-call-dsp.md` §5).
    #[must_use]
    pub fn capability(&self) -> DspCapability {
        self.capability
    }
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
    position: u64,
    discontinuity: Option<DiscontinuityKind>,
    samples: Vec<i16>,
}

/// One worker answer, carrying the buffer the runtime lent it back again.
#[derive(Debug)]
struct Response {
    sequence: u64,
    result: WorkerResult,
    samples: Vec<i16>,
}

/// The runtime half of a supervised stage: two bounded channels and one process it owns.
///
/// Every buffer this will ever use is allocated at validation. The pool is `capacity + 1` deep so
/// that one frame can be in flight for every slot of the request channel and one more can be being
/// filled; when it is empty the stage misses rather than allocating, because a bounded channel that
/// grows a buffer under pressure is not bounded.
#[derive(Debug)]
pub(crate) struct Supervised {
    requests: Option<SyncSender<Request>>,
    results: Receiver<Response>,
    reaper: Option<JoinHandle<()>>,
    terminate: Option<SyncSender<()>>,
    reaped: Arc<Stop>,
    pid: u32,
    pool: Vec<Vec<i16>>,
    held: VecDeque<Response>,
    sequence: u64,
    in_flight: u32,
    lost: bool,
}

impl Supervised {
    /// Spawn the worker process and allocate every buffer and channel slot the stage will ever use.
    ///
    /// The process is the graph's, terminated and reaped at the teardown barrier
    /// (`docs/specs/call-dsp-graph.md` §8); nothing on the live path ever spawns.
    ///
    /// # Errors
    ///
    /// Whatever the operating system said about spawning the program. Nothing is left running: a
    /// plan is refused whole (§3.1), and a stage that failed to spawn has no process to leak.
    pub(crate) fn spawn(
        spec: &WorkerProcess,
        capacity: u32,
        frame_samples: usize,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> io::Result<Self> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .envs(spec.envs.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The worker's standard error is the worker's: a worker reports to the operator there,
            // and nothing it says can corrupt the stream carrying audio (§7.4).
            .stderr(Stdio::inherit())
            .spawn()?;
        let pid = child.id();
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            drop(child.kill());
            drop(child.wait());
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "a spawned worker has no pipes",
            ));
        };

        let depth = capacity as usize;
        let (requests, inbox) = sync_channel::<Request>(depth);
        let (outbox, results) = sync_channel::<Response>(depth);
        // One slot is all the reaper needs: the first signal wakes it and every later one is the
        // same signal. The pump holds a sender too, so a worker that ends on its own is reaped
        // without waiting for a teardown that may be a whole call away.
        let (terminate, wakeup) = sync_channel::<()>(1);
        let reaped = Arc::new(Stop::default());
        let finished = Arc::clone(&reaped);

        let ended = terminate.clone();
        let pump = std::thread::spawn(move || {
            pump(
                stdin,
                stdout,
                &inbox,
                &outbox,
                direction,
                format,
                frame_samples,
            );
            // discard: a full channel already holds the wakeup this would add.
            let _ = ended.try_send(());
        });

        let supervisor = std::thread::spawn(move || {
            // §7.3 step 2 happened when the pump dropped its end of the worker's input. This is
            // steps 3 and 4, and neither is conditional on the worker having agreed to anything.
            let _ = wakeup.recv();
            drop(child.kill());
            drop(child.wait());
            drop(pump.join());
            // The reap signal is durable as well as prompt, so a barrier that arrives after the
            // process is already gone answers instead of parking forever.
            finished.stop();
        });

        Ok(Self {
            requests: Some(requests),
            results,
            reaper: Some(supervisor),
            terminate: Some(terminate),
            reaped,
            pid,
            pool: (0..=depth)
                .map(|_| Vec::with_capacity(frame_samples))
                .collect(),
            held: VecDeque::with_capacity(depth),
            sequence: 0,
            in_flight: 0,
            lost: false,
        })
    }

    /// The operating-system process this stage's worker runs in (§7).
    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }

    /// Offer one frame and take the result belonging to the frame `deadline_frames` back.
    ///
    /// Never awaits, never blocks and never allocates. Returns [`Exchange::Missed`] whenever there
    /// is no usable result, which the caller counts against
    /// `docs/specs/custom-call-dsp.md` §7.4's miss budget.
    pub(crate) fn exchange(
        &mut self,
        samples: &[i16],
        position: u64,
        discontinuity: Option<DiscontinuityKind>,
        deadline_frames: u32,
        out: &mut Vec<i16>,
    ) -> Exchange {
        if self.lost {
            return Exchange::Missed(BypassCause::WorkerLost);
        }
        let offered = self.sequence;
        self.offer(offered, position, discontinuity, samples);
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
    fn offer(
        &mut self,
        sequence: u64,
        position: u64,
        discontinuity: Option<DiscontinuityKind>,
        samples: &[i16],
    ) {
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
            position,
            discontinuity,
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

    /// Whether the worker process has still to be reaped.
    pub(crate) const fn is_live(&self) -> bool {
        self.reaper.is_some()
    }

    /// Terminate the worker: close the request channel, close its input, and kill it (§7.3).
    ///
    /// Non-blocking, idempotent, and callable from the media worker and from `drop`. It closes the
    /// request channel, which ends the pump and with it the worker's input, and it wakes the reaper,
    /// which does the killing and the `wait`ing on its own thread. Separate from [`Self::reap`]
    /// because the barrier's wait must not be what starts the termination — a caller that
    /// terminates every stage first lets all of them wind down at once.
    pub(crate) fn terminate(&mut self) {
        self.requests = None;
        if let Some(signal) = self.terminate.take() {
            // discard: a full channel already holds the wakeup this would add, and the reaper needs
            // exactly one.
            let _ = signal.try_send(());
        }
    }

    /// Wait for the worker process to have been reaped, then join what supervised it (§7.3).
    ///
    /// The wait is an event and never a duration: the reaper's last act is to signal, and it
    /// signals only once `wait` has returned, so a barrier that reports zero workers is reporting a
    /// process the operating system no longer has an entry for. A stopped session's teardown
    /// answers rather than holding a runtime worker nothing can reclaim.
    pub(crate) async fn reap(&mut self) {
        self.terminate();
        let Some(handle) = self.reaper.take() else {
            return;
        };
        self.reaped.wait().await;
        // The signal is the reaper's last act, so this join finds a finished thread and returns
        // without parking the runtime.
        drop(handle.join());
        // Both channel ends and every buffer they carried are dropped with this stage. Nothing
        // holds a frame of this call once the worker is gone, which is what the barrier reports.
        self.held.clear();
        self.pool.clear();
        self.in_flight = 0;
    }
}

impl Drop for Supervised {
    /// A dropped stage still reaps its process.
    ///
    /// A drop cannot await, so it does not join — but it does terminate, and the reaper thread it
    /// wakes kills and `wait`s whether or not anything is left to observe that. A supervised stage
    /// that leaked one process per call would be worse than the thread it replaced.
    fn drop(&mut self) {
        self.terminate();
    }
}

/// The pump's whole life: one frame written, one result read, in lockstep (§7.4).
///
/// It owns both pipes, which is what makes the worker's input close when it returns — §7.3's step
/// 2, delivered by dropping a handle rather than by asking the worker for anything.
fn pump(
    stdin: ChildStdin,
    stdout: ChildStdout,
    inbox: &Receiver<Request>,
    outbox: &SyncSender<Response>,
    direction: AudioDirection,
    format: StreamFormat,
    frame_samples: usize,
) {
    let mut stdin = stdin;
    let mut stdout = BufReader::new(stdout);
    let max_samples = u32::try_from(frame_samples).unwrap_or(u32::MAX);
    let mut message = Vec::with_capacity(wire::message_capacity(max_samples));
    let mut payload = Vec::with_capacity(wire::message_capacity(max_samples));

    if wire::write_hello(&mut stdin, &mut message, direction, format, max_samples).is_err() {
        return;
    }

    while let Ok(request) = inbox.recv() {
        let mut buffer = request.samples;
        if wire::write_frame(
            &mut stdin,
            &mut message,
            request.sequence,
            request.position,
            request.discontinuity,
            &buffer,
        )
        .is_err()
        {
            return;
        }
        buffer.clear();
        let answer = wire::read_message(&mut stdout, &mut payload, &mut buffer, max_samples);
        let result = match answer {
            Ok(Some(Incoming::Result { sequence, outcome })) if sequence == request.sequence => {
                outcome
            }
            // A well-formed answer to a frame nobody asked about, a message this direction does not
            // define, or a message §7.4 refuses. All three are the same fact about this worker:
            // it is not speaking the protocol, so nothing it says next can be believed either.
            Ok(Some(_)) | Err(Fault::Refused(_)) => WorkerResult::Failed,
            // The worker exited, crashed or was killed. Its results end with it, and the stage
            // learns that as §7.2's `WorkerLost` when the channel closes below.
            Ok(None) | Err(Fault::Io(_)) => return,
        };
        let terminal = result == WorkerResult::Failed;
        // discard: §7. A full result channel is the media worker not taking results, which is
        // already a miss on its side; blocking here would make this worker's liveness depend on the
        // media loop's. The buffer goes with it, and the stage misses rather than allocating.
        drop(outbox.try_send(Response {
            sequence: request.sequence,
            result,
            samples: buffer,
        }));
        if terminal {
            return;
        }
    }
}
