//! The asynchronous session driver (`docs/specs/speech-providers.md` §2).
//!
//! §2 names four jobs and gives all of them to one owner: the driver is "the host-owned
//! asynchronous shell around a session. It owns every task and bounded queue, feeds inputs in
//! order, applies outputs in order, and fires deadlines." `A-39` shipped the contract those
//! sessions implement and nothing that runs one. This module is the shell, and it is the only
//! place in the epic where a task is spawned.
//!
//! Three properties are worth knowing before reading further.
//!
//! **Call audio comes from the one seam and nowhere else.** [`RecognitionDriver::attach`] is the
//! only way to start a recognition driver, and it reaches the call through
//! [`Selected::processing`] and [`MediaSession::attach_processor`] — `M-54`'s single tap
//! (`docs/specs/call-audio-seam.md`). The driver then awaits its own bounded attachment and
//! nothing else on the media path, so a slow provider or a slow consumer can lose named frames but
//! can never delay RTP decode, encode, playback or capture.
//!
//! **Output is bounded by coalescing, never by dropping a terminal.** §5 puts at most
//! [`SpeechBounds::pending_revisions`] non-terminal revisions per utterance in the queue and
//! replaces the newest when another arrives; terminal and lifecycle outputs are never coalesced,
//! and when [`SpeechBounds::unconsumed_outputs`] of them are waiting the driver stops consuming
//! provider output altogether. That stops frame consumption, which engages the seam's own
//! drop-oldest policy — so a consumer that stops reading degrades the pipeline to bounded, *named*
//! loss instead of growing memory.
//!
//! **Every stop is bounded.** A cancellation, a flush, a provider failure and call teardown all
//! arm §8's drain deadline. If the session's `Stopped` does not arrive before it expires, the
//! driver abandons the provider and emits `Stopped { aborted: true }` itself: an aborted stop is a
//! reportable provider defect rather than a task nobody can join.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sipx_audio::PcmFormat;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::bounds::SpeechBounds;
use super::descriptor::ProviderKind;
use super::lifecycle::{CancelReason, DeadlineKind};
use super::recognition::{
    RecognitionInput, RecognitionOutput, RecognitionSession, recognition_inputs,
};
use super::selection::Selected;
use super::synthesis::{CancelScope, RequestId, SynthesisInput, SynthesisOutput, SynthesisSession};
use crate::MediaSession;
use crate::processing::{AudioDirection, PcmProcessor, ProcessingError, hold};

/// Room the driver's own input queue keeps beyond §8's bound, for the stop that ends the session.
///
/// A stop is idempotent — the session ends at the first one — so two slots is one more than the
/// number that can ever matter.
const CONTROL_SLACK: usize = 2;

/// Why a driver could not be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DriverError {
    /// The selection resolves the other contract kind.
    ///
    /// The provider's *type* already pins which contract a driver runs; this catches the other
    /// half, a selection whose operating format and voice were decided for the other one.
    #[error("a {expected} driver cannot run a {selected} selection")]
    WrongKind {
        /// The contract this driver runs.
        expected: ProviderKind,
        /// The contract the selection resolved.
        selected: ProviderKind,
    },
    /// `M-54`'s seam refused the attachment (`docs/specs/call-audio-seam.md` §5).
    ///
    /// Reused rather than re-minted, so an out-of-domain queue depth refuses with exactly the type
    /// that domain names. Note that [`SpeechBounds::input_frames`] and the seam's queue capacity
    /// are compatible domains rather than the same one: a bound outside the seam's range surfaces
    /// here, at attach time, and not as a [`SpeechBounds`] error at configuration time.
    #[error("the call-audio seam refused the attachment: {0}")]
    Seam(#[from] ProcessingError),
}

/// How §5's output bound classifies one output.
trait BoundedOutput {
    /// The unit of work whose non-terminal revisions coalesce, when this output is one of them.
    ///
    /// `None` for terminal and lifecycle outputs, which §5 never coalesces or drops and §8 counts
    /// against [`SpeechBounds::unconsumed_outputs`].
    fn revision_of(&self) -> Option<u64>;

    /// Whether this is the session's last output. §5 and §6 put nothing after it.
    fn is_stop(&self) -> bool;
}

impl BoundedOutput for RecognitionOutput {
    fn revision_of(&self) -> Option<u64> {
        match self {
            Self::Partial(utterance) | Self::Replacement(utterance) => Some(utterance.id().index()),
            _ => None,
        }
    }

    fn is_stop(&self) -> bool {
        matches!(self, Self::Stopped { .. })
    }
}

impl BoundedOutput for SynthesisOutput {
    /// Nothing a synthesis session emits is coalesced.
    ///
    /// §6 bounds production with the chunk window instead, and for a reason the recognition side
    /// does not have: a dropped chunk is lost audio and a replaced one is audio delivered out of
    /// order, neither of which a "newest wins" rule could repair. Credit returned as the consumer
    /// reads is what stops the queue growing.
    fn revision_of(&self) -> Option<u64> {
        None
    }

    fn is_stop(&self) -> bool {
        matches!(self, Self::Stopped { .. })
    }
}

/// Everything the outbox holds, behind one lock.
#[derive(Debug)]
struct Held<T> {
    queue: VecDeque<T>,
    /// Terminal and lifecycle outputs waiting. This is the count §8 bounds.
    retained: usize,
    /// The session emitted its last output; nothing more will arrive.
    closed: bool,
}

/// The driver's bounded output queue (§5's output bound, §8's `unconsumed_outputs`).
#[derive(Debug)]
struct Outbox<T> {
    state: Mutex<Held<T>>,
    /// Woken on every push and on close.
    ready: Notify,
    /// Woken on every take: it may have made room past the bound, and it may have returned
    /// chunk-window credit.
    room: Notify,
    bounds: SpeechBounds,
}

impl<T: BoundedOutput> Outbox<T> {
    fn new(bounds: SpeechBounds) -> Self {
        Self {
            state: Mutex::new(Held {
                queue: VecDeque::new(),
                retained: 0,
                closed: false,
            }),
            ready: Notify::new(),
            room: Notify::new(),
            bounds,
        }
    }

    /// Whether §8's unconsumed-output bound has been reached.
    fn saturated(&self) -> bool {
        hold(&self.state).retained >= self.bounds.unconsumed_outputs()
    }

    fn pending(&self) -> usize {
        hold(&self.state).queue.len()
    }

    /// Apply one output, in order (§5).
    fn push(&self, output: T) {
        {
            let mut state = hold(&self.state);
            if let Some(unit) = output.revision_of() {
                let held = state
                    .queue
                    .iter()
                    .filter(|queued| queued.revision_of() == Some(unit))
                    .count();
                if held >= self.bounds.pending_revisions() {
                    // §5: at the bound revisions coalesce and the newest wins. Replacing the
                    // newest in place rather than appending keeps every output derived from
                    // pre-gap audio ahead of every output derived from post-gap audio, and costs a
                    // consumer nothing, because no event is ever a delta.
                    let newest = state
                        .queue
                        .iter()
                        .rposition(|queued| queued.revision_of() == Some(unit));
                    if let Some(slot) = newest.and_then(|at| state.queue.get_mut(at)) {
                        *slot = output;
                    }
                } else {
                    state.queue.push_back(output);
                }
            } else {
                // Terminal and lifecycle outputs are never coalesced or dropped, so this may pass
                // the bound by the one output the driver itself had to emit. The bound governs
                // what is *consumed* from the provider, not what is kept.
                state.retained = state.retained.saturating_add(1);
                state.queue.push_back(output);
            }
        }
        self.ready.notify_one();
    }

    /// The session's last output has been applied.
    fn close(&self) {
        hold(&self.state).closed = true;
        self.ready.notify_waiters();
    }

    fn take(&self) -> Option<T> {
        let mut state = hold(&self.state);
        let output = state.queue.pop_front()?;
        if output.revision_of().is_none() {
            state.retained = state.retained.saturating_sub(1);
        }
        drop(state);
        self.room.notify_one();
        Some(output)
    }

    /// Wait for the next output, or for the session to have delivered its last.
    async fn recv(&self) -> Option<T> {
        loop {
            // The wait is registered before the queue is read: the opposite order has a lost-wake
            // window, where a push between the read and the await parks this future with an
            // output waiting. The same reasoning as the seam's own consumer.
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            if let Some(output) = self.take() {
                return Some(output);
            }
            if hold(&self.state).closed {
                return None;
            }
            ready.await;
        }
    }

    /// Wait until the consumer has taken something.
    async fn taken(&self) {
        self.room.notified().await;
    }
}

/// The one deadline a driver has armed, and the generation it was armed in (§2, §7, §8).
///
/// Deadlines bound failure detection only. A fired deadline carries the generation it was armed
/// in, and disarming moves the generation on — so a firing already in flight when the session
/// became ready is stale, and ignored.
#[derive(Debug)]
struct Deadlines {
    bounds: SpeechBounds,
    armed: Option<DeadlineKind>,
    generation: u64,
    /// The reset the driver's timer still owes.
    rearm: Option<Duration>,
    /// Whether the session has been told to stop, so §5 allows no `Frame` after it.
    stopping: bool,
}

impl Deadlines {
    /// §7: the driver arms the warm-up deadline at session start.
    fn warming(bounds: SpeechBounds) -> Self {
        Self {
            armed: Some(DeadlineKind::Warmup),
            generation: 1,
            rearm: Some(bounds.warmup()),
            stopping: false,
            bounds,
        }
    }

    fn armed(&self) -> bool {
        self.armed.is_some()
    }

    fn rearm(&mut self) -> Option<Duration> {
        self.rearm.take()
    }

    /// Readiness is what the warm-up deadline was bounding (§7).
    fn disarm(&mut self) {
        if self.armed.take().is_some() {
            self.generation = self.generation.saturating_add(1);
        }
    }

    /// Begin stopping, bounded by §8's drain deadline.
    ///
    /// The first stop wins: a second cancellation does not extend the deadline the first one armed,
    /// which is what keeps "cancel in a loop" from being a way to never stop.
    fn begin_stop(&mut self) {
        if self.stopping {
            return;
        }
        self.stopping = true;
        self.disarm();
        self.armed = Some(DeadlineKind::Drain);
        self.rearm = Some(self.bounds.drain());
    }

    /// Take the deadline that fired, with the generation it was armed in.
    fn fired(&mut self) -> Option<(DeadlineKind, u64)> {
        self.armed.take().map(|kind| (kind, self.generation))
    }
}

/// The chunk-window credit a consumer has returned but the driver has not yet handed back (§6).
///
/// A ledger rather than a queue of messages: credit for one request coalesces into one `Drained`,
/// so even a provider that ignores the window cannot make this grow.
#[derive(Debug, Default)]
struct Credit {
    returned: Mutex<VecDeque<(RequestId, u32)>>,
}

impl Credit {
    fn returned(&self, request: RequestId, chunks: u32) {
        let mut held = hold(&self.returned);
        match held.iter_mut().find(|(queued, _)| *queued == request) {
            Some((_, count)) => *count = count.saturating_add(chunks),
            None => held.push_back((request, chunks)),
        }
    }

    fn take(&self) -> Option<(RequestId, u32)> {
        hold(&self.returned).pop_front()
    }
}

/// Move what the provider has emitted into the outbox, in order, while §8 leaves room.
///
/// Returns whether the session's `Stopped` has been applied; §5 puts nothing after it.
fn pump_recognition<S: RecognitionSession>(
    provider: &mut S,
    outbox: &Outbox<RecognitionOutput>,
    deadlines: &mut Deadlines,
) -> bool {
    while !outbox.saturated() {
        let Some(output) = provider.poll_output() else {
            return false;
        };
        match &output {
            RecognitionOutput::Ready => deadlines.disarm(),
            // The session is ending without the driver having asked. §5 bounds every stop with the
            // drain deadline, including the ones the provider starts.
            RecognitionOutput::Failed(_) | RecognitionOutput::Lost(_) => deadlines.begin_stop(),
            _ => {}
        }
        let last = output.is_stop();
        outbox.push(output);
        if last {
            return true;
        }
    }
    false
}

/// The same, for a synthesis session.
fn pump_synthesis<S: SynthesisSession>(
    provider: &mut S,
    outbox: &Outbox<SynthesisOutput>,
    deadlines: &mut Deadlines,
) -> bool {
    while !outbox.saturated() {
        let Some(output) = provider.poll_output() else {
            return false;
        };
        match &output {
            SynthesisOutput::Ready => deadlines.disarm(),
            // A request failing is not the session failing; §6 gives the session's own failure no
            // request identity, and only that one ends the session.
            SynthesisOutput::Failed { request: None, .. } | SynthesisOutput::Lost(_) => {
                deadlines.begin_stop();
            }
            _ => {}
        }
        let last = output.is_stop();
        outbox.push(output);
        if last {
            return true;
        }
    }
    false
}

/// Drive one recognition session until it stops (§5).
async fn run_recognition<S: RecognitionSession>(
    mut provider: S,
    mut processor: PcmProcessor,
    outbox: Arc<Outbox<RecognitionOutput>>,
    bounds: SpeechBounds,
    mut control: mpsc::Receiver<RecognitionInput>,
) {
    let mut deadlines = Deadlines::warming(bounds);
    let timer = tokio::time::sleep(bounds.warmup());
    tokio::pin!(timer);
    let mut listening = true;

    loop {
        if pump_recognition(&mut provider, &outbox, &mut deadlines) {
            break;
        }
        if let Some(after) = deadlines.rearm() {
            timer.as_mut().reset(Instant::now() + after);
        }
        tokio::select! {
            biased;
            // A stop is never made to wait behind audio.
            input = control.recv(), if listening => match input {
                Some(input) => {
                    let stops = matches!(
                        input,
                        RecognitionInput::Flush | RecognitionInput::Cancel(_)
                    );
                    provider.deliver(input);
                    if stops {
                        deadlines.begin_stop();
                    }
                }
                // Every handle is gone. Dropping one aborts this task, so there is simply nothing
                // left to listen to.
                None => listening = false,
            },
            () = outbox.taken() => {}
            // The one read of call audio, and it is this attachment's own bounded queue.
            frame = processor.recv(), if !deadlines.stopping && !outbox.saturated() => {
                if let Some(frame) = frame {
                    for input in recognition_inputs(frame) {
                        provider.deliver(input);
                    }
                } else {
                    // The seam completed: the call ended. §7 lets SIP teardown reach a session as
                    // a cancellation and never as a provider failure.
                    provider.deliver(RecognitionInput::Cancel(CancelReason::CallEnded));
                    deadlines.begin_stop();
                }
            }
            () = &mut timer, if deadlines.armed() => {
                let Some((kind, generation)) = deadlines.fired() else {
                    continue;
                };
                provider.deliver(RecognitionInput::DeadlineFired { kind, generation });
                match kind {
                    // §7: the provider fails the session with `WarmupTimeout`. Its `Stopped` is
                    // what the drain deadline armed here now bounds.
                    DeadlineKind::Warmup => deadlines.begin_stop(),
                    // §5: the drain expired. Whatever the provider still owns is abandoned with
                    // it, and the driver reports the stop it had to make in its place.
                    DeadlineKind::Drain => {
                        if !pump_recognition(&mut provider, &outbox, &mut deadlines) {
                            outbox.push(RecognitionOutput::Stopped { aborted: true });
                        }
                        break;
                    }
                }
            }
            // Nothing can make progress: the session has stopped without saying so.
            else => break,
        }
    }
    outbox.close();
}

/// Drive one synthesis session until it stops (§6).
async fn run_synthesis<S: SynthesisSession>(
    mut provider: S,
    outbox: Arc<Outbox<SynthesisOutput>>,
    credit: Arc<Credit>,
    bounds: SpeechBounds,
    mut control: mpsc::Receiver<SynthesisInput>,
) {
    let mut deadlines = Deadlines::warming(bounds);
    let timer = tokio::time::sleep(bounds.warmup());
    tokio::pin!(timer);
    let mut listening = true;

    loop {
        // §6: the window's credit returns when the driver has handed a chunk on, and at no other
        // time, so a provider can never run ahead of a slow call into unbounded audio.
        while let Some((request, chunks)) = credit.take() {
            provider.deliver(SynthesisInput::Drained { request, chunks });
        }
        if pump_synthesis(&mut provider, &outbox, &mut deadlines) {
            break;
        }
        if let Some(after) = deadlines.rearm() {
            timer.as_mut().reset(Instant::now() + after);
        }
        tokio::select! {
            biased;
            input = control.recv(), if listening => match input {
                Some(input) => {
                    let stops = matches!(
                        input,
                        SynthesisInput::Cancel { scope: CancelScope::Session, .. }
                    );
                    provider.deliver(input);
                    if stops {
                        deadlines.begin_stop();
                    }
                }
                None => listening = false,
            },
            () = outbox.taken() => {}
            () = &mut timer, if deadlines.armed() => {
                let Some((kind, generation)) = deadlines.fired() else {
                    continue;
                };
                provider.deliver(SynthesisInput::DeadlineFired { kind, generation });
                match kind {
                    DeadlineKind::Warmup => deadlines.begin_stop(),
                    DeadlineKind::Drain => {
                        if !pump_synthesis(&mut provider, &outbox, &mut deadlines) {
                            outbox.push(SynthesisOutput::Stopped { aborted: true });
                        }
                        break;
                    }
                }
            }
            else => break,
        }
    }
    outbox.close();
}

/// The host-owned asynchronous shell around one recognition session (§2, §5).
///
/// Created by [`Self::attach`], which is the contract's only reach into call media. Outputs are
/// read with [`Self::recv`] until it reports the session's last one; [`Self::flush`] and
/// [`Self::cancel`] end the session, and so does the call itself.
///
/// Dropping the handle aborts the driver's task and releases its seam attachment immediately.
/// [`Self::join`] is the graceful counterpart: it waits for the task that has already been asked
/// to stop, which is an observed completion rather than an elapsed duration.
#[derive(Debug)]
pub struct RecognitionDriver {
    outbox: Arc<Outbox<RecognitionOutput>>,
    inputs: mpsc::Sender<RecognitionInput>,
    task: Option<JoinHandle<()>>,
}

impl RecognitionDriver {
    /// Attach through `M-54`'s seam and drive `provider` off it.
    ///
    /// The attachment runs in the operating format `selected` fixed and at
    /// [`SpeechBounds::input_frames`] deep, so the seam's drop-oldest policy *is* §5's input-bound
    /// obligation rather than a second queue resembling it.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::WrongKind`] for a synthesis selection, and [`DriverError::Seam`] for
    /// every refusal the seam makes — an unsupported format, an out-of-domain queue depth, the
    /// per-session attachment ceiling, or a session that has already stopped. A refusal leaves the
    /// call exactly as it was and spawns nothing.
    pub fn attach<S>(
        media: &MediaSession,
        selected: &Selected,
        direction: AudioDirection,
        bounds: SpeechBounds,
        provider: S,
    ) -> Result<Self, DriverError>
    where
        S: RecognitionSession + Send + 'static,
    {
        if selected.kind() != ProviderKind::Recognition {
            return Err(DriverError::WrongKind {
                expected: ProviderKind::Recognition,
                selected: selected.kind(),
            });
        }
        let processor = media.attach_processor(selected.processing(direction, bounds))?;
        let outbox = Arc::new(Outbox::new(bounds));
        let (inputs, control) = mpsc::channel(CONTROL_SLACK);
        let task = tokio::spawn(run_recognition(
            provider,
            processor,
            Arc::clone(&outbox),
            bounds,
            control,
        ));
        Ok(Self {
            outbox,
            inputs,
            task: Some(task),
        })
    }

    /// Wait for the next output, or `None` once the session's last one has been read.
    pub async fn recv(&mut self) -> Option<RecognitionOutput> {
        self.outbox.recv().await
    }

    /// Take the next output if one is already waiting.
    ///
    /// `None` means nothing is waiting *now*; it does not mean the session has stopped.
    pub fn try_recv(&mut self) -> Option<RecognitionOutput> {
        self.outbox.take()
    }

    /// How many outputs are waiting to be read.
    pub fn pending(&self) -> usize {
        self.outbox.pending()
    }

    /// End the audio input (§5 `Flush`). The open utterance resolves terminally, then the session
    /// stops.
    pub fn flush(&self) {
        self.stop(RecognitionInput::Flush);
    }

    /// Cancel the session with a typed reason (§5 `Cancel`, §7).
    pub fn cancel(&self, reason: CancelReason) {
        self.stop(RecognitionInput::Cancel(reason));
    }

    /// Offer one stop. The session ends at the first one, so a later one that finds the driver's
    /// input queue full is redundant rather than lost.
    fn stop(&self, input: RecognitionInput) {
        drop(self.inputs.try_send(input));
    }

    /// Wait for the driver's task to finish.
    ///
    /// Every stop is bounded by §8's drain deadline, so this completes on the session's own
    /// terminal output or on the abort the driver made in its place — never on a duration this
    /// caller has to guess.
    pub async fn join(mut self) {
        if let Some(task) = self.task.take() {
            // discard: a cancelled or panicking task is already reported through the outputs this
            // driver applied, and the join is here to prove the task is reaped.
            drop(task.await);
        }
    }
}

impl Drop for RecognitionDriver {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            // An abandoned driver holds a seam attachment and a provider. Aborting releases both
            // now rather than at the end of the call.
            task.abort();
        }
    }
}

/// The host-owned asynchronous shell around one synthesis session (§2, §6).
///
/// Requests go in with [`Self::enqueue`]; outputs come out with [`Self::recv`], which is also what
/// returns §8's chunk-window credit. There is no media attachment here: `M-54`'s seam observes call
/// audio and does not inject it, so placing synthesized audio into a call is `A-27`'s, and this
/// driver's whole job is to run the session that produces it.
#[derive(Debug)]
pub struct SynthesisDriver {
    outbox: Arc<Outbox<SynthesisOutput>>,
    credit: Arc<Credit>,
    inputs: mpsc::Sender<SynthesisInput>,
    format: PcmFormat,
    task: Option<JoinHandle<()>>,
}

impl SynthesisDriver {
    /// Drive `provider` under `selected`.
    ///
    /// # Errors
    ///
    /// Returns [`DriverError::WrongKind`] for a recognition selection.
    pub fn spawn<S>(
        selected: &Selected,
        bounds: SpeechBounds,
        provider: S,
    ) -> Result<Self, DriverError>
    where
        S: SynthesisSession + Send + 'static,
    {
        if selected.kind() != ProviderKind::Synthesis {
            return Err(DriverError::WrongKind {
                expected: ProviderKind::Synthesis,
                selected: selected.kind(),
            });
        }
        let outbox = Arc::new(Outbox::new(bounds));
        let credit = Arc::new(Credit::default());
        let (inputs, control) = mpsc::channel(bounds.queued_requests() + CONTROL_SLACK);
        let task = tokio::spawn(run_synthesis(
            provider,
            Arc::clone(&outbox),
            Arc::clone(&credit),
            bounds,
            control,
        ));
        Ok(Self {
            outbox,
            credit,
            inputs,
            format: selected.format(),
            task: Some(task),
        })
    }

    /// The operating format every chunk arrives in, fixed by selection (§4).
    pub fn format(&self) -> PcmFormat {
        self.format
    }

    /// Queue one bounded text request (§6 `Enqueue`).
    ///
    /// Returns whether the request reached the session. `false` means the driver's own input queue
    /// is full — the host is offering requests faster than the session consumes them — and is a
    /// different fact from §8's request bound, which the session reports as
    /// `Refused(QueueFull)`. Nothing is queued in either case.
    pub fn enqueue(&self, request: RequestId, text: String, replace: bool) -> bool {
        self.inputs
            .try_send(SynthesisInput::Enqueue {
                request,
                text,
                replace,
            })
            .is_ok()
    }

    /// Cancel one request or the whole session, with a typed reason (§6 `Cancel`, §7).
    pub fn cancel(&self, scope: CancelScope, reason: CancelReason) {
        drop(
            self.inputs
                .try_send(SynthesisInput::Cancel { scope, reason }),
        );
    }

    /// Wait for the next output, or `None` once the session's last one has been read.
    ///
    /// Taking a `Chunk` here is what returns its window credit (§6), so a consumer that stops
    /// reading stops production rather than accumulating audio.
    pub async fn recv(&mut self) -> Option<SynthesisOutput> {
        let output = self.outbox.recv().await?;
        self.credited(&output);
        Some(output)
    }

    /// Take the next output if one is already waiting, returning its window credit.
    pub fn try_recv(&mut self) -> Option<SynthesisOutput> {
        let output = self.outbox.take()?;
        self.credited(&output);
        Some(output)
    }

    /// How many outputs are waiting to be read.
    pub fn pending(&self) -> usize {
        self.outbox.pending()
    }

    /// Wait for the driver's task to finish. Bounded on the same terms as
    /// [`RecognitionDriver::join`].
    pub async fn join(mut self) {
        if let Some(task) = self.task.take() {
            // discard: as for the recognition driver, the outcome is already in the outputs.
            drop(task.await);
        }
    }

    fn credited(&self, output: &SynthesisOutput) {
        if let SynthesisOutput::Chunk(chunk) = output {
            self.credit.returned(chunk.request(), 1);
            // The driver is parked on this notification whenever it has nothing else to do, so the
            // credit is applied on the next turn of its loop rather than at the next input.
            self.outbox.room.notify_one();
        }
    }
}

impl Drop for SynthesisDriver {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::speech::{SampleSpan, Utterance, UtteranceId};

    fn utterance(id: u64, revision: u32) -> Utterance {
        Utterance::new(
            UtteranceId::new(id),
            revision,
            String::new(),
            SampleSpan::new(0, u64::from(revision) * 160),
        )
    }

    /// §5: at the revision bound the newest wins, in the slot the coalesced one held, and terminal
    /// outputs are never touched by any of it.
    #[test]
    fn revisions_coalesce_in_place_and_terminals_do_not() {
        let bounds = SpeechBounds::DEFAULTS;
        let outbox = Outbox::new(bounds);
        outbox.push(RecognitionOutput::Warming);
        outbox.push(RecognitionOutput::Partial(utterance(0, 1)));
        outbox.push(RecognitionOutput::Ready);
        for revision in 2..=8 {
            outbox.push(RecognitionOutput::Replacement(utterance(0, revision)));
        }
        outbox.push(RecognitionOutput::Final(utterance(0, 8)));

        assert_eq!(outbox.pending(), 4, "seven revisions coalesced into one");
        assert_eq!(outbox.take(), Some(RecognitionOutput::Warming));
        assert_eq!(
            outbox.take(),
            Some(RecognitionOutput::Replacement(utterance(0, 8))),
            "the newest revision holds the slot the first one opened"
        );
        assert_eq!(outbox.take(), Some(RecognitionOutput::Ready));
        assert_eq!(
            outbox.take(),
            Some(RecognitionOutput::Final(utterance(0, 8)))
        );
        assert_eq!(outbox.take(), None);
    }

    /// §8: the bound counts terminal and lifecycle outputs, and only those. Revisions never
    /// saturate it, which is what makes a chatty provider a coalescing consumer rather than a
    /// stalled one.
    #[test]
    fn only_retained_outputs_reach_the_bound() {
        let bounds = SpeechBounds::DEFAULTS
            .with_unconsumed_outputs(2)
            .unwrap()
            .with_pending_revisions(1)
            .unwrap();
        let outbox = Outbox::new(bounds);
        for revision in 1..=20 {
            outbox.push(RecognitionOutput::Replacement(utterance(0, revision)));
        }
        assert!(!outbox.saturated(), "revisions never reach the bound");
        assert_eq!(outbox.pending(), 1);

        outbox.push(RecognitionOutput::Warming);
        assert!(!outbox.saturated());
        outbox.push(RecognitionOutput::Ready);
        assert!(outbox.saturated(), "two lifecycle outputs are the bound");
        assert_eq!(
            outbox.take(),
            Some(RecognitionOutput::Replacement(utterance(0, 20)))
        );
        assert!(
            outbox.saturated(),
            "taking a revision returns no room: it was never counted"
        );
        assert_eq!(outbox.take(), Some(RecognitionOutput::Warming));
        assert!(!outbox.saturated());
    }

    /// §2: disarming moves the generation on, so a deadline already in flight is stale; a second
    /// stop does not extend the first one's drain.
    #[test]
    fn a_disarmed_deadline_is_stale_and_the_first_stop_wins() {
        let mut deadlines = Deadlines::warming(SpeechBounds::DEFAULTS);
        assert_eq!(deadlines.rearm(), Some(SpeechBounds::DEFAULTS.warmup()));
        deadlines.disarm();
        assert!(!deadlines.armed());
        assert_eq!(deadlines.generation, 2);

        deadlines.begin_stop();
        assert_eq!(deadlines.rearm(), Some(SpeechBounds::DEFAULTS.drain()));
        assert_eq!(deadlines.fired(), Some((DeadlineKind::Drain, 2)));

        deadlines.begin_stop();
        assert_eq!(
            deadlines.rearm(),
            None,
            "a second stop re-arms nothing the first one did not"
        );
        assert!(
            !deadlines.armed(),
            "and it does not revive a fired deadline"
        );
        assert_eq!(deadlines.generation, 2);
    }

    /// §6: credit for one request coalesces, so the ledger is bounded by the requests in flight
    /// rather than by the chunks a provider chose to emit.
    #[test]
    fn returned_credit_coalesces_per_request() {
        let credit = Credit::default();
        for _ in 0..9 {
            credit.returned(RequestId::new(0), 1);
        }
        credit.returned(RequestId::new(1), 2);
        assert_eq!(credit.take(), Some((RequestId::new(0), 9)));
        assert_eq!(credit.take(), Some((RequestId::new(1), 2)));
        assert_eq!(credit.take(), None);
    }
}
