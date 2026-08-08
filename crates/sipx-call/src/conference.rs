//! Joining calls a host owns to a mixed conference (`C-6`).
//!
//! [`sipx_media::Conference`] is `M-12`'s mixer: one clock, every participant sent the sum of the
//! others and never themselves. [`CallConference`] is the path to it from a [`Call`], and it is
//! the same path [`CallBridge`](crate::CallBridge) takes — the host keeps owning its calls, hands
//! over no media session and no port, and the audio moves over the sessions' own channels.
//!
//! A conference is not a bridge with more legs. A bridge can pass payloads through untouched
//! because there is exactly one place to send each one; a mixer has to decode every leg, add, and
//! re-encode, whatever codec each one negotiated. So joining a conference always costs a
//! transcode, and that is a property of mixing rather than of this wrapper.
//!
//! # What a participation does not survive
//!
//! A participant is the media session the call had when it joined. Renegotiating that call
//! replaces its session, and the conference goes on mixing the one it was given, which has
//! stopped. Rejoining is the remedy; `C-9` is the story that removes the manual step. For the same
//! reason a call that ends should be [`CallConference::leave`]-d: the mixer keeps a slot for a
//! participant nobody removed, contributing silence, until it is.

use std::time::Duration;

use sipx_media::{Conference, ConferenceError};

use crate::call::Call;

/// One call's place in a conference, and the handle that removes it.
///
/// Opaque and per-conference: passing one to a different [`CallConference`] removes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a participant that is never left keeps a slot in the mix"]
pub struct Participant(u64);

/// Several calls a host owns, mixed so each hears all the others.
///
/// Participants join and leave while it runs, and neither disturbs the others: the mixing clock
/// does not stop, and a participant who leaves simply stops contributing and stops being mixed
/// into.
#[derive(Debug)]
pub struct CallConference {
    inner: Conference,
}

impl CallConference {
    /// Start an empty conference, mixing at this frame size and interval.
    ///
    /// The interval must match what the participating calls' sessions send at, or the conference
    /// produces frames faster or slower than they can be played and the queues drift.
    /// [`Self::narrowband`] is the telephony default and is what most hosts want.
    ///
    /// # Errors
    ///
    /// Returns [`ConferenceError::IntervalTooShort`] when `interval` is under one millisecond,
    /// before anything is spawned.
    pub fn new(samples_per_frame: usize, interval: Duration) -> Result<Self, ConferenceError> {
        Ok(Self {
            inner: Conference::new(samples_per_frame, interval)?,
        })
    }

    /// A conference at the usual telephony rate: 20 ms frames of 8 kHz audio.
    ///
    /// # Errors
    ///
    /// The fixed interval satisfies [`ConferenceError`]'s minimum today. The fallible return keeps
    /// this on the same explicit startup contract as [`Self::new`].
    pub fn narrowband() -> Result<Self, ConferenceError> {
        Ok(Self {
            inner: Conference::narrowband()?,
        })
    }

    /// Add a call, and return the handle that removes it.
    ///
    /// **A call is in a conference or in a bridge, never both** — both consume the call's received
    /// audio, and a media session has one consumer (the vision's principle 3). So joining releases
    /// any bridge this call is in first, and that bridge's other call is told on its own event
    /// stream, rather than the two quietly competing for the same frames.
    ///
    /// A call whose media the host is also reading directly — [`Call::record_until_idle`],
    /// [`Call::media`]'s own receive methods — is the same conflict, and this cannot see it. Pick
    /// one reader.
    pub async fn join(&self, call: &mut Call) -> Participant {
        call.release_bridge();
        Participant(self.inner.join(call.media_handle()).await)
    }

    /// Remove a participant. The others carry on, and their mixes simply stop containing this one.
    ///
    /// The call itself is untouched: it is not ended, and it receives its own far end's audio
    /// again.
    pub async fn leave(&self, participant: Participant) {
        self.inner.leave(participant.0).await;
    }

    /// How many calls are in it.
    pub async fn len(&self) -> usize {
        self.inner.len().await
    }

    /// Whether nobody is in it.
    pub async fn is_empty(&self) -> bool {
        self.inner.is_empty().await
    }

    /// The frame size being mixed at.
    #[must_use]
    pub fn samples_per_frame(&self) -> usize {
        self.inner.samples_per_frame()
    }

    /// Stop mixing. The participating calls are left running.
    pub async fn close(&self) {
        self.inner.close().await;
    }
}
