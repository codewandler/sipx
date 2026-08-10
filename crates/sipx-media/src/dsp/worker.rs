//! The worker side of a supervised stage: what runs in the spawned process
//! (`docs/specs/call-dsp-graph.md` §7.4).
//!
//! Everything in this module runs in the *worker's* process and none of it runs in the media
//! process. It is the half an application writes: implement [`SupervisedWorker`], call
//! [`serve_worker`] from the program's `main`, and name that program when the stage is planned.
//!
//! The loop has two threads and that is a requirement rather than an optimisation. §7.4 obliges a
//! conforming worker to end when its input ends **whatever its processing is doing**, and a worker
//! that only looked at its input between frames would not notice the end at all once a frame had
//! hung — which is exactly the frame whose worker most needs to go away. So one thread does nothing
//! but read, and the processing happens on the other.

use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use sipx_audio::dsp::{DspCapability, DspFrame, StreamFormat};

use super::wire::{self, Incoming};
use crate::processing::{AudioDirection, DiscontinuityKind};

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
    /// [`BypassCause::MalformedResult`](super::BypassCause::MalformedResult) and not audio.
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

/// Application DSP the runtime runs in a process of its own, reached only over §7.4's protocol.
///
/// **What implementing this promises the application.** Over-budget work here cannot stall RTP:
/// the media worker offers a frame to a bounded channel and takes a result only if one is present
/// by the declared deadline, so it never waits for this code. A hang, a loop, a panic or a
/// malformed result costs the declared failure action plus a termination and a reap — and because
/// this runs in an operating-system process, the termination is a kill that does not need this
/// code's cooperation and the reap is a `wait`.
///
/// **What it does not promise.** It does not make this code fast and it does not make it correct.
/// An abandoned result is audio that did not get processed, so a fail-open bypass here is audible:
/// the containment is of the stall, not of the artefact. Nor does it bound this worker's own memory
/// or CPU beyond what the operating system is configured to bound — that is deployment
/// configuration, and putting the worker in its own process is what makes it *configurable* rather
/// than what configures it.
///
/// The declared [`DspCapability`] must name [`ExecutionProfile::SupervisedIsolated`]; a plan whose
/// supervised stage declares anything else is refused before it activates. It is declared on both
/// sides — here, and in the [`WorkerProcess`](super::WorkerProcess) the runtime plans — because the
/// runtime must validate a chain before it spawns anything, and it cannot ask a process that does
/// not exist yet.
///
/// [`ExecutionProfile::SupervisedIsolated`]: sipx_audio::dsp::ExecutionProfile::SupervisedIsolated
pub trait SupervisedWorker: Send {
    /// Everything this worker declares before it may be attached
    /// (`docs/specs/custom-call-dsp.md` §5).
    fn capability(&self) -> DspCapability;

    /// Transform one frame, writing the output into `out`.
    ///
    /// `out` is empty on entry and is the worker loop's buffer, lent for this call. The frame's
    /// samples are borrowed and MUST NOT be retained. Producing a different number of positions
    /// than the frame carries is a malformed result, not a length policy.
    fn run(&mut self, frame: &DspFrame<'_>, out: &mut Vec<i16>) -> WorkerResult;
}

/// Serve §7.4's protocol on this process's standard input and output until the input ends.
///
/// This is the whole of a worker program: read the runtime's frames, answer each one, and return
/// when the runtime is gone. Standard error is untouched, so a worker may report to an operator
/// without corrupting the stream.
///
/// **Return from `main` when this returns.** It returns as soon as the input ends, which is the
/// runtime's signal to stop (§7.3) and is also what an orphaned worker sees when the media process
/// dies. Returning from `main` ends the process whatever the processing thread is still doing,
/// which is §7.4's third obligation; a program that went on to do other work here would keep a hung
/// frame's thread alive and would not be conforming.
///
/// # Errors
///
/// The [`io::Error`] that ended the exchange. A refused message (§7.4) is reported as
/// [`io::ErrorKind::InvalidData`] carrying the [`WorkerProtocolError`](super::WorkerProtocolError)
/// that refused it. A worker's own failure is not an error here: it is a
/// [`WorkerResult::Failed`] answer, which the runtime handles as §7.2 says.
pub fn serve_worker<W: SupervisedWorker + 'static>(worker: W) -> io::Result<()> {
    serve(worker, io::stdin().lock(), io::stdout())
}

/// One frame handed from the reading thread to the processing thread.
struct Job {
    sequence: u64,
    position: u64,
    discontinuity: Option<DiscontinuityKind>,
    samples: Vec<i16>,
}

/// The protocol is lockstep (§7.4), so exactly one frame is ever outstanding.
const OUTSTANDING: usize = 1;

/// [`serve_worker`] over two arbitrary streams rather than this process's own.
///
/// `M-122`'s protocol probe puts a scripted octet stream where the runtime's writes go, so §7.4's
/// "a refused message is terminal, and a worker stops reading and exits" is checked against this
/// function and not against a second copy of it.
pub(super) fn serve<W, R, O>(worker: W, mut input: R, output: O) -> io::Result<()>
where
    W: SupervisedWorker + 'static,
    R: Read,
    O: Write + Send + 'static,
{
    let mut payload = Vec::new();
    let mut samples = Vec::new();

    // §7.4: `Hello` comes first and states the ceiling every later message is bounded by. Until it
    // has arrived there is no ceiling, so nothing larger than a `Hello` may be read.
    let hello = wire::read_message(&mut input, &mut payload, &mut samples, 0)?;
    let Some(Incoming::Hello {
        direction,
        format,
        max_samples,
    }) = hello
    else {
        return Ok(());
    };

    let (jobs, inbox) = sync_channel::<Job>(OUTSTANDING);
    let (spent, recycled) = sync_channel::<Vec<i16>>(OUTSTANDING);
    let processing = std::thread::spawn(move || {
        process(
            worker,
            output,
            &inbox,
            &spent,
            direction,
            format,
            max_samples,
        );
    });

    let ending = read_frames(&mut input, &jobs, &recycled, &mut payload, max_samples);

    // Dropping the job channel is how the processing thread learns there is nothing more, on the
    // one path where it is not already stuck. It is never joined: a thread inside a frame that
    // will not return is exactly the thread this must not wait for, and returning from `main` is
    // what ends it (§7.4).
    drop(jobs);
    drop(processing);
    ending
}

/// Read frames until the input ends, handing each to the processing thread.
fn read_frames<R: Read>(
    input: &mut R,
    jobs: &SyncSender<Job>,
    recycled: &Receiver<Vec<i16>>,
    payload: &mut Vec<u8>,
    max_samples: u32,
) -> io::Result<()> {
    let mut samples = Vec::with_capacity(max_samples as usize);
    loop {
        match wire::read_message(input, payload, &mut samples, max_samples)? {
            // The runtime is gone, or has said everything it is going to say; or it sent a second
            // `Hello`, or anything else it is not supposed to send twice. Both end the exchange:
            // one because there is nothing more, the other because a runtime that is not speaking
            // §7.4 cannot be answered in it.
            None | Some(Incoming::Hello { .. } | Incoming::Result { .. }) => return Ok(()),
            Some(Incoming::Frame {
                sequence,
                position,
                discontinuity,
            }) => {
                let job = Job {
                    sequence,
                    position,
                    discontinuity,
                    samples: std::mem::take(&mut samples),
                };
                if jobs.send(job).is_err() {
                    return Ok(());
                }
                samples = match recycled.try_recv() {
                    Ok(buffer) => buffer,
                    // The steady state recycles; the first frames and a lost processing thread do
                    // not, and a worker's own allocations are its business (§7.2).
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => {
                        Vec::with_capacity(max_samples as usize)
                    }
                };
            }
        }
    }
}

/// The processing thread's whole life: one frame in, one result out, in order.
fn process<W: SupervisedWorker, O: Write>(
    mut worker: W,
    mut output: O,
    inbox: &Receiver<Job>,
    spent: &SyncSender<Vec<i16>>,
    direction: AudioDirection,
    format: StreamFormat,
    max_samples: u32,
) {
    let mut out: Vec<i16> = Vec::with_capacity(max_samples as usize);
    let mut message: Vec<u8> = Vec::with_capacity(wire::message_capacity(max_samples));
    while let Ok(job) = inbox.recv() {
        let mut frame = DspFrame::new(direction, format, job.position, &job.samples);
        if let Some(kind) = job.discontinuity {
            frame = frame.with_discontinuity(kind);
        }
        out.clear();
        // A panicking worker is a finding, not a reason to take this process down without saying
        // why: it is caught here and reported as a terminal failure, which the runtime handles as
        // §7.2's `WorkerLost`. Under the aborting panic strategy the process dies instead, and the
        // runtime reads that ending the same way.
        let result = catch_unwind(AssertUnwindSafe(|| worker.run(&frame, &mut out)))
            .unwrap_or(WorkerResult::Failed);
        if result != WorkerResult::Produced {
            out.clear();
        }
        let written = wire::write_result(&mut output, &mut message, job.sequence, result, &out);
        let mut buffer = job.samples;
        buffer.clear();
        // discard: a full recycling channel means a buffer is already waiting, which is all the
        // reading thread needs.
        drop(spent.try_send(buffer));
        if written.is_err() || result == WorkerResult::Failed {
            return;
        }
    }
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
    use sipx_audio::dsp::{ExecutionPolicy, ExecutionProfile};

    struct Gain2;

    impl SupervisedWorker for Gain2 {
        fn capability(&self) -> DspCapability {
            DspCapability::new("gain2")
                .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated))
        }
        fn run(&mut self, frame: &DspFrame<'_>, out: &mut Vec<i16>) -> WorkerResult {
            out.extend(frame.samples().iter().map(|n| n.saturating_mul(2)));
            WorkerResult::Produced
        }
    }

    fn narrowband() -> StreamFormat {
        StreamFormat::new(8_000, 1).unwrap()
    }

    /// §7.4: a worker answers every frame in order, and the metadata reaches it.
    #[test]
    fn a_worker_answers_every_frame_it_is_offered() {
        let mut input = Vec::new();
        let mut scratch = Vec::new();
        wire::write_hello(
            &mut input,
            &mut scratch,
            AudioDirection::Outbound,
            narrowband(),
            8,
        )
        .unwrap();
        for sequence in 0..3 {
            wire::write_frame(
                &mut input,
                &mut scratch,
                sequence,
                sequence * 4,
                None,
                &[1, 2, 3, 4],
            )
            .unwrap();
        }

        let (output, results) = std::sync::mpsc::channel::<Vec<u8>>();
        serve(Gain2, input.as_slice(), Collector(output)).unwrap();

        let mut reader = Served::new(results);
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        for sequence in 0..3 {
            let message = wire::read_message(&mut reader, &mut payload, &mut samples, 8)
                .unwrap()
                .unwrap();
            assert_eq!(
                message,
                Incoming::Result {
                    sequence,
                    outcome: WorkerResult::Produced,
                }
            );
            assert_eq!(samples, vec![2, 4, 6, 8]);
        }
    }

    /// §7.2: a panicking worker answers `Failed` rather than taking the exchange down silently.
    #[test]
    fn a_panicking_worker_reports_a_terminal_failure() {
        struct Panics;
        impl SupervisedWorker for Panics {
            fn capability(&self) -> DspCapability {
                DspCapability::new("panics")
                    .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated))
            }
            fn run(&mut self, _frame: &DspFrame<'_>, _out: &mut Vec<i16>) -> WorkerResult {
                panic!("this worker is a finding");
            }
        }

        let mut input = Vec::new();
        let mut scratch = Vec::new();
        wire::write_hello(
            &mut input,
            &mut scratch,
            AudioDirection::Inbound,
            narrowband(),
            4,
        )
        .unwrap();
        wire::write_frame(&mut input, &mut scratch, 0, 0, None, &[1, 2]).unwrap();

        let (output, results) = std::sync::mpsc::channel::<Vec<u8>>();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        serve(Panics, input.as_slice(), Collector(output)).unwrap();
        std::panic::set_hook(previous);

        let mut reader = Served::new(results);
        let mut payload = Vec::new();
        let mut samples = Vec::new();
        let message = wire::read_message(&mut reader, &mut payload, &mut samples, 4)
            .unwrap()
            .unwrap();
        assert_eq!(
            message,
            Incoming::Result {
                sequence: 0,
                outcome: WorkerResult::Failed,
            }
        );
    }

    /// A sink that hands what was written to [`Served`], so a served exchange can be read back.
    struct Collector(std::sync::mpsc::Sender<Vec<u8>>);

    impl Write for Collector {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            drop(self.0.send(buf.to_vec()));
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Everything the processing thread wrote, read as a stream.
    ///
    /// It blocks until the next chunk arrives and ends when the sink is dropped, so a test reads
    /// the exchange without joining a thread `serve` deliberately never joins.
    struct Served {
        chunks: Receiver<Vec<u8>>,
        buffered: std::collections::VecDeque<u8>,
    }

    impl Served {
        fn new(chunks: Receiver<Vec<u8>>) -> Self {
            Self {
                chunks,
                buffered: std::collections::VecDeque::new(),
            }
        }
    }

    impl Read for Served {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            while self.buffered.is_empty() {
                match self.chunks.recv() {
                    Ok(chunk) => self.buffered.extend(chunk),
                    Err(_) => return Ok(0),
                }
            }
            let mut written = 0;
            for slot in buf.iter_mut() {
                match self.buffered.pop_front() {
                    Some(octet) => {
                        *slot = octet;
                        written += 1;
                    }
                    None => break,
                }
            }
            Ok(written)
        }
    }
}
