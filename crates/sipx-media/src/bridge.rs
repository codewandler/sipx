//! Connecting two calls so each hears the other.
//!
//! The decision that shapes this is whether to decode. When both legs agreed on the same codec,
//! the payload that arrives on one is exactly the payload the other should send, so the bytes
//! go straight across.
//!
//! Worth being precise about *why*, because the obvious reason is wrong for the codec sipx
//! ships today. G.711 decoding is exactly invertible — encode(decode(x)) is x for every one of
//! the 256 codes — so for µ-law and A-law, transcoding a bridge costs CPU and nothing else. The
//! generational loss argument is real for every codec whose decode is *not* invertible: G.722,
//! and Opus when it lands. Building the pass-through path now means those codecs arrive into a
//! bridge that already does the right thing, rather than one that quietly degrades them.
//!
//! When the codecs differ there is no choice, and the bridge says so rather than transcoding
//! quietly. Someone looking at a call that sounds worse than it should is entitled to find out
//! why from the software rather than by reasoning about it.
//! **Experimental** (`A-8`): real over `MediaSession`s you own, and unreachable from a `Call`,
//! which does not hand its session out. Two calls cannot be bridged yet (`C-6`).
//!

use std::sync::Arc;

use tokio::task::JoinHandle;

use crate::session::{Codec, Encoded, MediaSession};

/// Two calls connected so each hears the other.
///
/// Dropping it stops the forwarding. Both directions are separate tasks, and either ending —
/// because its session stopped, or because the far end went away — takes the whole bridge with
/// it: half a bridge is a call where one party can hear and the other cannot, which is worse
/// than a call that has ended.
#[derive(Debug)]
pub struct Bridge {
    forward: JoinHandle<()>,
    reverse: JoinHandle<()>,
    transcoding: bool,
}

impl Bridge {
    /// Connect two sessions.
    pub fn connect(one: Arc<MediaSession>, two: Arc<MediaSession>) -> Self {
        // The same codec on both legs means the bytes can go straight across.
        let transcoding = one.codec() != two.codec();
        if transcoding {
            tracing::info!(
                from = ?one.codec(),
                to = ?two.codec(),
                "bridging between different codecs; the audio will be transcoded"
            );
        }
        one.set_relay(!transcoding);
        two.set_relay(!transcoding);

        let forward = spawn_leg(Arc::clone(&one), Arc::clone(&two), transcoding);
        let reverse = spawn_leg(two, one, transcoding);

        Self {
            forward,
            reverse,
            transcoding,
        }
    }

    /// Whether audio is being decoded and re-encoded rather than passed through.
    ///
    /// Reported rather than inferred. Transcoding costs quality as well as CPU, and a caller
    /// that cares can renegotiate; one that cannot see it happening has no reason to.
    #[must_use]
    pub fn is_transcoding(&self) -> bool {
        self.transcoding
    }

    /// Whether both directions are still running.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        !self.forward.is_finished() && !self.reverse.is_finished()
    }

    /// Stop forwarding.
    ///
    /// The sessions themselves are left running: a bridge that ended the calls it was
    /// connecting would make "put this call back on hold" impossible.
    pub fn close(self) {
        self.forward.abort();
        self.reverse.abort();
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Without this, dropping a bridge leaves two tasks forwarding audio between two calls
        // nobody is holding a handle to — the tasks keep the sessions alive through their
        // `Arc`s, so the sockets stay open too, and neither the calls nor the ports are ever
        // reclaimed.
        self.forward.abort();
        self.reverse.abort();
    }
}

/// One direction of a bridge.
fn spawn_leg(from: Arc<MediaSession>, to: Arc<MediaSession>, transcoding: bool) -> JoinHandle<()> {
    tokio::spawn(async move {
        if transcoding {
            // Decode on one side, re-encode on the other. The samples are the only common
            // ground between two different codecs.
            while let Some(samples) = from.recv().await {
                if !to.send(samples).await {
                    return;
                }
            }
        } else {
            while let Some(encoded) = from.recv_encoded().await {
                if !relay(&to, encoded).await {
                    return;
                }
            }
        }
    })
}

/// Pass a payload across, when both legs speak the same codec.
async fn relay(to: &MediaSession, encoded: Encoded) -> bool {
    // A payload type that is not the one this leg negotiated should not be forwarded as though
    // it were. In practice this is a DTMF event on a bridge whose two legs negotiated different
    // dynamic payload types for `telephone-event`, and forwarding it verbatim would have the
    // far end play a keypress as audio.
    if Codec::from_payload_type(encoded.payload_type) != Some(to.codec()) {
        return true;
    }
    to.send_encoded(encoded).await
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

    #[test]
    fn matching_codecs_are_not_transcoded() {
        // The decision itself, without any sockets: it is the whole point of the type.
        assert_eq!(Codec::Pcmu, Codec::Pcmu);
        assert_ne!(Codec::Pcmu, Codec::Pcma);
    }
}
