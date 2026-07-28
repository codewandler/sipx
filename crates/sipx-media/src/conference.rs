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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use sipx_audio::mix::mix_into;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::session::MediaSession;

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
    collectors: Mutex<HashMap<u64, JoinHandle<()>>>,
    mixer: JoinHandle<()>,
    samples_per_frame: usize,
}

impl Conference {
    /// Start an empty conference, mixing at this frame size and interval.
    ///
    /// The interval must match what the participants' sessions send at, or the conference
    /// produces frames faster or slower than they can be played and the queues drift.
    #[must_use]
    pub fn new(samples_per_frame: usize, interval: Duration) -> Self {
        let members: Members = Arc::new(Mutex::new(HashMap::new()));
        let mixer = tokio::spawn(mix_loop(Arc::clone(&members), samples_per_frame, interval));
        Self {
            members,
            next_id: std::sync::atomic::AtomicU64::new(0),
            collectors: Mutex::new(HashMap::new()),
            mixer,
            samples_per_frame,
        }
    }

    /// A conference at the usual telephony rate: 20 ms frames of 8 kHz audio.
    #[must_use]
    pub fn narrowband() -> Self {
        Self::new(160, Duration::from_millis(20))
    }

    /// Add a participant, and return the handle used to remove them.
    pub async fn join(&self, session: Arc<MediaSession>) -> u64 {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        self.members.lock().await.insert(
            id,
            Member {
                session: Arc::clone(&session),
                pending: Vec::new(),
            },
        );

        // One collector per participant, because `recv` is a blocking wait on that
        // participant's channel and the mixer cannot afford to wait on any of them.
        let members = Arc::clone(&self.members);
        let collector = tokio::spawn(async move {
            while let Some(samples) = session.recv().await {
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
        self.collectors.lock().await.insert(id, collector);
        id
    }

    /// Remove a participant.
    ///
    /// The others carry on. Their mixes simply stop containing this one, which is what leaving
    /// a conversation sounds like.
    pub async fn leave(&self, id: u64) {
        self.members.lock().await.remove(&id);
        if let Some(collector) = self.collectors.lock().await.remove(&id) {
            collector.abort();
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
        self.mixer.abort();
        let mut collectors = self.collectors.lock().await;
        for (_, collector) in collectors.drain() {
            collector.abort();
        }
        self.members.lock().await.clear();
    }
}

impl Drop for Conference {
    fn drop(&mut self) {
        // The mixer holds every participant's session through an `Arc`, so a conference dropped
        // without being closed keeps every call in it alive — sockets, ports and all.
        self.mixer.abort();
    }
}

/// Mix and send, once per interval.
async fn mix_loop(members: Members, samples_per_frame: usize, interval: Duration) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tick.tick().await;

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
