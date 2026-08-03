//! A conference: every party hears every other party, and never themselves.
//!
//! Unlike a [`crate::bridge::Bridge`], a conference cannot pass bytes through. Mixing happens on
//! samples, so every leg is decoded on the way in and encoded on the way out whatever codec it
//! negotiated. That is not a shortcoming to be optimised away later — adding two µ-law codes is
//! not adding two amplitudes, and a mixer that tried would produce noise.
//!
//! The shape is a clock, not a chain of forwards. A bridge can forward each packet as it
//! arrives because there is exactly one thing to send it to; a mixer has to decide *when* a
//! frame is complete, because it is waiting on N participants who will not arrive together. So
//! one task ticks at the packet interval, takes whatever each participant has produced since
//! the last tick, and sends each of them the sum of the others.
//!
//! A participant who has said nothing contributes silence, which is exactly right: the mix goes
//! out on time and the quiet participant is simply quiet. The alternative — waiting for
//! everyone — makes the whole conference as late as its worst connection.
//! **Experimental** (`A-8`): as with [`super::bridge`], real over sessions you own and not
//! reachable from a `Call` (`C-6`).
//!

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use sipx_audio::mix::mix_into;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::session::{MediaSession, Stop};

/// A conference worker configuration that cannot make forward progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ConferenceError {
    /// The mixer interval is below the one-millisecond runtime floor.
    #[error("conference mix interval must be at least 1 ms, got {0:?}")]
    IntervalTooShort(Duration),
}

/// Who is in the conference.
type Members = Arc<Mutex<HashMap<u64, Member>>>;

/// The most audio held for a participant the mixer is not draining.
///
/// Half a second at 8 kHz: ample slack for a mixer that is keeping up, and a hard stop for one
/// that is not. Without a bound, a conference whose mixing task has died grows a buffer per
/// participant for as long as anybody keeps talking.
const MOST_PENDING: usize = 4_000;

struct Member {
    session: Arc<MediaSession>,
    /// What this participant has contributed since the last tick.
    pending: Vec<i16>,
}

/// Worker registration and shutdown are one state transition.
///
/// A collector must never exist outside this registry while shutdown can drain it. Keeping the
/// closed bit beside the handles makes the decision durable even if a notification races a task's
/// first poll.
#[derive(Debug)]
struct Workers {
    closed: bool,
    collectors: HashMap<u64, JoinHandle<()>>,
    mixer: Option<JoinHandle<()>>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct LifecycleHooks {
    join_before_registration: StdMutex<Option<JoinRegistrationHook>>,
    close_waiting_for_members: StdMutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[cfg(test)]
#[derive(Debug)]
struct JoinRegistrationHook {
    reached: tokio::sync::oneshot::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl std::fmt::Debug for Member {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The session is deliberately not printed: it is large, and what a reader of a debug
        // dump wants to know about a participant is how far behind they are.
        f.debug_struct("Member")
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

/// Several calls mixed together.
///
/// Participants join and leave while it runs. Neither disturbs the others: the mixing clock
/// does not stop, and a participant who leaves simply stops contributing and stops being sent
/// to.
#[derive(Debug)]
pub struct Conference {
    members: Members,
    next_id: std::sync::atomic::AtomicU64,
    workers: StdMutex<Workers>,
    samples_per_frame: usize,
    stop: Arc<Stop>,
    #[cfg(test)]
    lifecycle_hooks: LifecycleHooks,
}

impl Conference {
    /// Start an empty conference, mixing at this frame size and interval.
    ///
    /// The interval must match what the participants' sessions send at, or the conference
    /// produces frames faster or slower than they can be played and the queues drift. It must
    /// also be at least one millisecond.
    ///
    /// # Errors
    ///
    /// Returns [`ConferenceError::IntervalTooShort`] before spawning the mixer when `interval`
    /// is shorter than one millisecond.
    pub fn new(samples_per_frame: usize, interval: Duration) -> Result<Self, ConferenceError> {
        if interval < Duration::from_millis(1) {
            return Err(ConferenceError::IntervalTooShort(interval));
        }
        let members: Members = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(Stop::default());
        let mixer = tokio::spawn(mix_loop(
            Arc::clone(&members),
            samples_per_frame,
            interval,
            Arc::clone(&stop),
        ));
        Ok(Self {
            members,
            next_id: std::sync::atomic::AtomicU64::new(0),
            workers: StdMutex::new(Workers {
                closed: false,
                collectors: HashMap::new(),
                mixer: Some(mixer),
            }),
            samples_per_frame,
            stop,
            #[cfg(test)]
            lifecycle_hooks: LifecycleHooks::default(),
        })
    }

    /// A conference at the usual telephony rate: 20 ms frames of 8 kHz audio.
    ///
    /// # Errors
    ///
    /// The fixed interval currently satisfies [`ConferenceError`]'s minimum. The fallible return
    /// keeps this convenience constructor on the same explicit startup contract as [`Self::new`].
    pub fn narrowband() -> Result<Self, ConferenceError> {
        Self::new(160, Duration::from_millis(20))
    }

    /// Add a participant, and return the handle used to remove them.
    pub async fn join(&self, session: Arc<MediaSession>) -> u64 {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let mut participants = self.members.lock().await;
        let mut workers = self.workers_lock();
        if workers.closed {
            return id;
        }
        participants.insert(
            id,
            Member {
                session: Arc::clone(&session),
                pending: Vec::new(),
            },
        );

        // One collector per participant, because `recv` is a blocking wait on that
        // participant's channel and the mixer cannot afford to wait on any of them. Spawn and
        // handle insertion happen while the lifecycle lock is held: close either drains this
        // handle or marks the conference closed before this point, never between the two.
        let members = Arc::clone(&self.members);
        let stop = Arc::clone(&self.stop);
        let collector = tokio::spawn(async move {
            loop {
                let samples = tokio::select! {
                    () = stop.wait() => return,
                    samples = session.recv() => samples,
                };
                let Some(samples) = samples else {
                    return;
                };
                let mut members = members.lock().await;
                let Some(member) = members.get_mut(&id) else {
                    return;
                };
                member.pending.extend_from_slice(&samples);
                if member.pending.len() > MOST_PENDING {
                    let excess = member.pending.len() - MOST_PENDING;
                    member.pending.drain(..excess);
                }
            }
        });
        #[cfg(test)]
        self.pause_join_before_registration();
        workers.collectors.insert(id, collector);
        id
    }

    /// Remove a participant.
    ///
    /// The others carry on. Their mixes simply stop containing this one, which is what leaving
    /// a conversation sounds like.
    pub async fn leave(&self, id: u64) {
        let mut members = self.members.lock().await;
        let collector = self.workers_lock().collectors.remove(&id);
        members.remove(&id);
        drop(members);
        if let Some(collector) = collector {
            collector.abort();
            let _ = collector.await;
        }
    }

    /// How many are in it.
    pub async fn len(&self) -> usize {
        self.members.lock().await.len()
    }

    /// Whether nobody is in it.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// The frame size being mixed at.
    #[must_use]
    pub fn samples_per_frame(&self) -> usize {
        self.samples_per_frame
    }

    /// Stop mixing. The participants' sessions are left running.
    pub async fn close(&self) {
        // Acquire every async lock before changing the lifecycle. Cancellation while waiting is
        // therefore a no-op. After this await, worker cancellation and participant release are a
        // synchronous transition, so dropping this future cannot strand session Arcs in a closed
        // conference.
        let mut members = self.lock_members_for_close().await;
        let workers = self.shutdown();
        members.clear();
        drop(members);
        for worker in workers {
            // An abort completes promptly at the task's next cancellation point. The result is
            // cancellation itself, which is the expected shutdown outcome rather than an error
            // for the caller.
            let _ = worker.await;
        }
    }

    /// Lock worker ownership even if a previous holder was cancelled while mutating it.
    /// Poisoning cannot make an abort handle unsafe to use, and refusing the lock here would
    /// turn one cancelled operation into a permanent worker leak.
    fn workers_lock(&self) -> std::sync::MutexGuard<'_, Workers> {
        match self.workers.lock() {
            Ok(workers) => workers,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    async fn lock_members_for_close(&self) -> tokio::sync::MutexGuard<'_, HashMap<u64, Member>> {
        #[cfg(not(test))]
        {
            self.members.lock().await
        }
        #[cfg(test)]
        {
            use std::future::Future as _;
            use std::task::Poll;

            let mut waiting = match self.lifecycle_hooks.close_waiting_for_members.lock() {
                Ok(mut hook) => hook.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            };
            let mut lock = Box::pin(self.members.lock());
            std::future::poll_fn(|cx| match lock.as_mut().poll(cx) {
                Poll::Ready(members) => Poll::Ready(members),
                Poll::Pending => {
                    if let Some(waiting) = waiting.take() {
                        // The receiver is test-owned and may have been cancelled with its task.
                        // Either way, polling the contended lock is the state the hook records.
                        let _ = waiting.send(());
                    }
                    Poll::Pending
                }
            })
            .await
        }
    }

    #[cfg(test)]
    fn pause_join_before_registration(&self) {
        let hook = match self.lifecycle_hooks.join_before_registration.lock() {
            Ok(mut hook) => hook.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        if let Some(hook) = hook {
            // Both halves are test-owned and bounded. A failed receiver means the test has already
            // ended; a missing release times out instead of pinning a runtime worker forever.
            let _ = hook.reached.send(());
            let _ = hook.release.recv_timeout(Duration::from_secs(2));
        }
    }

    /// Idempotently signal and take ownership of every worker this conference owns.
    fn shutdown(&self) -> Vec<JoinHandle<()>> {
        let mut state = self.workers_lock();
        state.closed = true;
        self.stop.stop();
        let mut workers = Vec::new();
        if let Some(mixer) = state.mixer.take() {
            mixer.abort();
            workers.push(mixer);
        }
        workers.extend(state.collectors.drain().map(|(_, collector)| {
            collector.abort();
            collector
        }));
        workers
    }
}

impl Drop for Conference {
    fn drop(&mut self) {
        // Drop cannot await, but aborting before releasing the retained handles makes every task
        // cancellation-ready and prevents detached work from retaining participant sessions.
        drop(self.shutdown());
    }
}

/// Mix and send, once per interval.
async fn mix_loop(members: Members, samples_per_frame: usize, interval: Duration, stop: Arc<Stop>) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            () = stop.wait() => return,
            _ = tick.tick() => {}
        }

        // Take a frame's worth from each participant, and the sessions to send to, in one
        // pass. The lock is released before any sending: holding it across an await would let
        // one slow participant stall the whole conference.
        let (ids, frames, sessions) = {
            let mut members = members.lock().await;
            if members.is_empty() {
                continue;
            }
            let mut ids = Vec::with_capacity(members.len());
            let mut frames = Vec::with_capacity(members.len());
            let mut sessions = Vec::with_capacity(members.len());
            for (id, member) in members.iter_mut() {
                let take = member.pending.len().min(samples_per_frame);
                let mut frame: Vec<i16> = member.pending.drain(..take).collect();
                // A participant who has said nothing this tick contributes silence. Waiting for
                // them instead would make the conference as late as its worst connection.
                frame.resize(samples_per_frame, 0);
                ids.push(*id);
                frames.push(frame);
                sessions.push(Arc::clone(&member.session));
            }
            (ids, frames, sessions)
        };

        for (index, session) in sessions.iter().enumerate() {
            // N-1: everyone except this one. Including their own audio would send their voice
            // back a round trip late, which is the single most disorienting artefact a
            // conference can produce.
            let mut mixed = vec![0i16; samples_per_frame];
            for (other, frame) in frames.iter().enumerate() {
                if other == index {
                    continue;
                }
                mix_into(&mut mixed, frame);
            }
            if !session.send(mixed).await {
                // The session has gone. Its collector will notice too; nothing here needs to
                // tear it down, and doing so would need the lock again mid-send.
                tracing::debug!(id = ids.get(index), "a conference participant has gone");
            }
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::session::{Codec, Config, MediaPort};

    fn set_close_wait_hook(conference: &Conference) -> tokio::sync::oneshot::Receiver<()> {
        let (waiting, reached) = tokio::sync::oneshot::channel();
        match conference.lifecycle_hooks.close_waiting_for_members.lock() {
            Ok(mut hook) => *hook = Some(waiting),
            Err(poisoned) => *poisoned.into_inner() = Some(waiting),
        }
        reached
    }

    async fn wait_for_close_to_block(reached: tokio::sync::oneshot::Receiver<()>) {
        tokio::time::timeout(Duration::from_secs(2), reached)
            .await
            .expect("close polls the members lock")
            .expect("close reports its blocked lock poll");
    }

    #[tokio::test]
    async fn cancelling_close_while_it_waits_leaves_no_half_closed_conference() {
        let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
            .await
            .expect("binds");
        let mut config = Config::new("127.0.0.1:9".parse().expect("valid"), Codec::Pcmu);
        config.rtcp_interval = None;
        let session = Arc::new(port.start(config).expect("valid media setup"));
        let weak = Arc::downgrade(&session);
        let conference = Arc::new(Conference::narrowband().expect("valid conference timing"));
        conference.join(Arc::clone(&session)).await;

        // Hold the only async lock close needs. The old ordering marked the conference stopped
        // and drained every worker before parking here; cancelling the future then stranded the
        // member Arc in a conference which could no longer run.
        let members = conference.members.lock().await;
        let close_waiting = set_close_wait_hook(&conference);
        let closing = {
            let conference = Arc::clone(&conference);
            tokio::spawn(async move { conference.close().await })
        };
        wait_for_close_to_block(close_waiting).await;
        assert!(
            !closing.is_finished(),
            "the close future is parked on the held members lock"
        );
        closing.abort();
        let _ = closing.await;
        assert!(
            !conference.workers_lock().closed,
            "cancellation before all locks are held must not half-close the conference"
        );
        drop(members);

        drop(session);
        conference.close().await;
        assert!(
            weak.upgrade().is_none(),
            "a later close releases the participant rather than finding stranded state"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_cannot_pass_join_between_collector_spawn_and_registration() {
        let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
            .await
            .expect("binds");
        let mut config = Config::new("127.0.0.1:9".parse().expect("valid"), Codec::Pcmu);
        config.rtcp_interval = None;
        let session = Arc::new(port.start(config).expect("valid media setup"));
        let weak = Arc::downgrade(&session);
        let conference = Arc::new(Conference::narrowband().expect("valid conference timing"));

        let (join_reached, reached) = tokio::sync::oneshot::channel();
        let (release, join_release) = std::sync::mpsc::sync_channel(0);
        {
            let mut hook = match conference.lifecycle_hooks.join_before_registration.lock() {
                Ok(hook) => hook,
                Err(poisoned) => poisoned.into_inner(),
            };
            *hook = Some(JoinRegistrationHook {
                reached: join_reached,
                release: join_release,
            });
        }

        let joining = {
            let conference = Arc::clone(&conference);
            let session = Arc::clone(&session);
            tokio::spawn(async move { conference.join(session).await })
        };
        tokio::time::timeout(Duration::from_secs(2), reached)
            .await
            .expect("join reaches the registration boundary")
            .expect("join reports the registration boundary");

        let close_waiting = set_close_wait_hook(&conference);
        let closing = {
            let conference = Arc::clone(&conference);
            tokio::spawn(async move { conference.close().await })
        };
        wait_for_close_to_block(close_waiting).await;
        assert!(
            !closing.is_finished(),
            "close cannot drain workers while join owns the lifecycle transition"
        );

        release
            .send(())
            .expect("join is released to register its collector");
        joining.await.expect("join task finishes");
        closing.await.expect("close task finishes");
        drop(session);

        assert!(conference.is_empty().await);
        assert!(
            weak.upgrade().is_none(),
            "close drains the collector registered by the serialized join"
        );
    }
}
