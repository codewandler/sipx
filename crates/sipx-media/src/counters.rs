//! Plain snapshots of media-path discards (`docs/specs/media-runtime.md` §4).

use std::sync::atomic::{AtomicU64, Ordering};

/// What one media session discarded, split by consequence.
///
/// Each field is exact and monotonic. The snapshot as a whole is not instantaneous: independent
/// workers can increment fields between these loads, so relationships between fields are exact
/// only while the session is quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MediaDiscardCounts {
    /// Audio frames an Opus encoder refused.
    pub opus_encode_failures: u64,
    /// Opus packets the decoder refused.
    pub opus_decode_failures: u64,
    /// RTP packets that failed SRTP authentication or decryption.
    pub srtp_unprotect_failures: u64,
    /// RTCP reports that failed SRTCP authentication or decryption.
    pub srtcp_unprotect_failures: u64,
    /// Caller-supplied RTP header extensions left off the packet they were handed with (`M-85`).
    ///
    /// **The one field here that counts metadata rather than media.** The packet went out and the
    /// payload was whole; what did not go out is the extension, because its embedded length word
    /// disagreed with the bytes behind it and that word is where the far end reads the payload as
    /// starting. Writing it anyway puts media where a header is expected — unencrypted on an
    /// encrypted leg, and truncated on a plain one (`docs/specs/media-runtime.md` §4).
    ///
    /// **It describes the application on this side, not the far end and not the network.** An
    /// extension that arrived over the network was bounds-checked when its packet was decoded, so
    /// only an [`Encoded`](crate::Encoded) built by hand can move this, and it is not a number to
    /// read as evidence of an attack. It says the far end is not being sent metadata it may be
    /// relying on; it does not say any audio was lost.
    ///
    /// It is also the whole of what an operator sees from a self-inconsistent extension. `M-81`
    /// counted the same mistake a second time, one boundary later, as an SRTP protect failure;
    /// `M-85` moved the refusal to [`Packet::encode`](sipx_rtp::packet::Packet::encode), where the
    /// payload can still be saved, and `M-90` removed the second counter rather than publish one
    /// nothing could move (`docs/specs/media-runtime.md` §4).
    pub malformed_extensions_dropped: u64,
    /// RTP packets whose SSRC differed from the established stream.
    pub foreign_ssrc: u64,
    /// Complete DTMF digits refused because the application queue was full or closed.
    pub dtmf_delivery_failures: u64,
    /// RTP packets carrying neither the negotiated payload type nor a known static codec.
    pub unknown_payload_type: u64,
    /// Playback completion reports whose last observer had gone away.
    pub playback_completion_unobserved: u64,
    /// Connectivity checks refused by a full or closed ICE-driver queue.
    pub ice_driver_queue_refusals: u64,
    /// Media-sent notes refused by a full or closed ICE-driver queue.
    pub ice_data_sent_queue_refusals: u64,
    /// ICE renegotiation replies whose requester had stopped waiting.
    pub ice_renegotiation_reply_unobserved: u64,
    /// ICE connectivity checks the socket refused to send.
    pub ice_send_failures: u64,
    /// Server-reflexive candidates discarded because they duplicate a host candidate.
    pub ice_redundant_candidates: u64,
    /// Datagrams consumed during gathering that did not come from the queried STUN server.
    pub ice_gathering_foreign_datagrams: u64,
    /// Frames an attached PCM processor lost under the seam's bounded-queue policy
    /// (`docs/specs/call-audio-seam.md` §6).
    pub processor_frames_lost: u64,
    /// Decoded frames shed from the application's inbound queue under §4.3's time bound.
    ///
    /// The number that says an application is reading slower than the far end is talking. Every
    /// other counter here describes something the network or a codec did; this one describes the
    /// application, and it is the one to read before concluding that added delay came from the
    /// media path.
    pub inbound_frames_shed: u64,
    /// RTP packets the jitter buffer refused because their play-out slot had already gone.
    ///
    /// The audible failure the buffer exists to prevent, and the one number that says the depth
    /// is too shallow for this network. Playing them anyway would put audio out of order, which
    /// sounds worse than the gap they were going to fill.
    pub jitter_late: u64,
    /// RTP packets the jitter buffer refused because it was already holding that sequence.
    pub jitter_duplicates: u64,
    /// Play-out slots filled with silence because their packet never arrived
    /// (`docs/specs/media-runtime.md` §4.2).
    ///
    /// Not audio this session threw away — audio it never had. It is counted here because the
    /// consequence is the same shape as the rest: a span the far end sent that the application
    /// did not hear, and §4's rule is that it must not be invisible.
    pub jitter_concealed: u64,
}

impl MediaDiscardCounts {
    /// Whether this session has discarded anything.
    #[must_use]
    pub fn any(self) -> bool {
        self.total() > 0
    }

    /// All counted media discards, saturating rather than wrapping back to a plausible zero.
    #[must_use]
    pub fn total(self) -> u64 {
        [
            self.opus_encode_failures,
            self.opus_decode_failures,
            self.srtp_unprotect_failures,
            self.srtcp_unprotect_failures,
            self.malformed_extensions_dropped,
            self.foreign_ssrc,
            self.dtmf_delivery_failures,
            self.unknown_payload_type,
            self.playback_completion_unobserved,
            self.ice_driver_queue_refusals,
            self.ice_data_sent_queue_refusals,
            self.ice_renegotiation_reply_unobserved,
            self.ice_send_failures,
            self.ice_redundant_candidates,
            self.ice_gathering_foreign_datagrams,
            self.processor_frames_lost,
            self.inbound_frames_shed,
            self.jitter_late,
            self.jitter_duplicates,
            self.jitter_concealed,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

/// The live form shared by every worker belonging to one session.
#[derive(Debug, Default)]
pub(crate) struct DiscardMeters {
    pub(crate) opus_encode_failures: AtomicU64,
    pub(crate) opus_decode_failures: AtomicU64,
    pub(crate) srtp_unprotect_failures: AtomicU64,
    pub(crate) srtcp_unprotect_failures: AtomicU64,
    pub(crate) malformed_extensions_dropped: AtomicU64,
    pub(crate) foreign_ssrc: AtomicU64,
    pub(crate) dtmf_delivery_failures: AtomicU64,
    pub(crate) unknown_payload_type: AtomicU64,
    pub(crate) playback_completion_unobserved: AtomicU64,
    pub(crate) ice_driver_queue_refusals: AtomicU64,
    pub(crate) ice_data_sent_queue_refusals: AtomicU64,
    pub(crate) ice_renegotiation_reply_unobserved: AtomicU64,
    pub(crate) ice_send_failures: AtomicU64,
    pub(crate) ice_redundant_candidates: AtomicU64,
    pub(crate) ice_gathering_foreign_datagrams: AtomicU64,
    pub(crate) processor_frames_lost: AtomicU64,
    pub(crate) inbound_frames_shed: AtomicU64,
    pub(crate) jitter_late: AtomicU64,
    pub(crate) jitter_duplicates: AtomicU64,
    pub(crate) jitter_concealed: AtomicU64,
}

impl DiscardMeters {
    pub(crate) fn snapshot(&self) -> MediaDiscardCounts {
        MediaDiscardCounts {
            opus_encode_failures: self.opus_encode_failures.load(Ordering::Relaxed),
            opus_decode_failures: self.opus_decode_failures.load(Ordering::Relaxed),
            srtp_unprotect_failures: self.srtp_unprotect_failures.load(Ordering::Relaxed),
            srtcp_unprotect_failures: self.srtcp_unprotect_failures.load(Ordering::Relaxed),
            malformed_extensions_dropped: self.malformed_extensions_dropped.load(Ordering::Relaxed),
            foreign_ssrc: self.foreign_ssrc.load(Ordering::Relaxed),
            dtmf_delivery_failures: self.dtmf_delivery_failures.load(Ordering::Relaxed),
            unknown_payload_type: self.unknown_payload_type.load(Ordering::Relaxed),
            playback_completion_unobserved: self
                .playback_completion_unobserved
                .load(Ordering::Relaxed),
            ice_driver_queue_refusals: self.ice_driver_queue_refusals.load(Ordering::Relaxed),
            ice_data_sent_queue_refusals: self.ice_data_sent_queue_refusals.load(Ordering::Relaxed),
            ice_renegotiation_reply_unobserved: self
                .ice_renegotiation_reply_unobserved
                .load(Ordering::Relaxed),
            ice_send_failures: self.ice_send_failures.load(Ordering::Relaxed),
            ice_redundant_candidates: self.ice_redundant_candidates.load(Ordering::Relaxed),
            ice_gathering_foreign_datagrams: self
                .ice_gathering_foreign_datagrams
                .load(Ordering::Relaxed),
            processor_frames_lost: self.processor_frames_lost.load(Ordering::Relaxed),
            inbound_frames_shed: self.inbound_frames_shed.load(Ordering::Relaxed),
            jitter_late: self.jitter_late.load(Ordering::Relaxed),
            jitter_duplicates: self.jitter_duplicates.load(Ordering::Relaxed),
            jitter_concealed: self.jitter_concealed.load(Ordering::Relaxed),
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

    /// How many fields [`MediaDiscardCounts`] publishes, and therefore how many bits
    /// [`one_bit_per_field`] sets.
    const FIELDS: u32 = 20;

    /// The published field set, written out, with a distinct bit in every field.
    ///
    /// Exhaustive on purpose — no `..Default::default()`. The literal is the field list, so
    /// adding or removing a counter cannot happen without this file saying so, which is what
    /// makes `M-90`'s removal of `srtp_protect_failures` a decision a reader meets rather than a
    /// line that quietly left a struct.
    fn one_bit_per_field() -> MediaDiscardCounts {
        MediaDiscardCounts {
            opus_encode_failures: 1,
            opus_decode_failures: 1 << 1,
            srtp_unprotect_failures: 1 << 2,
            srtcp_unprotect_failures: 1 << 3,
            malformed_extensions_dropped: 1 << 4,
            foreign_ssrc: 1 << 5,
            dtmf_delivery_failures: 1 << 6,
            unknown_payload_type: 1 << 7,
            playback_completion_unobserved: 1 << 8,
            ice_driver_queue_refusals: 1 << 9,
            ice_data_sent_queue_refusals: 1 << 10,
            ice_renegotiation_reply_unobserved: 1 << 11,
            ice_send_failures: 1 << 12,
            ice_redundant_candidates: 1 << 13,
            ice_gathering_foreign_datagrams: 1 << 14,
            processor_frames_lost: 1 << 15,
            inbound_frames_shed: 1 << 16,
            jitter_late: 1 << 17,
            jitter_duplicates: 1 << 18,
            jitter_concealed: 1 << 19,
        }
    }

    /// One bit per field means the sum can only be the saturated pattern if `total()` reads each
    /// field exactly once: a field left out of the fold clears its bit, and a field read twice
    /// carries into its neighbour's.
    ///
    /// This is what keeps `total()` meaningful across a change to the field set — removing a
    /// counter must remove it from the fold as well, and must leave every other field summed as
    /// it was.
    #[test]
    fn total_sums_every_published_field_exactly_once() {
        assert_eq!(
            one_bit_per_field().total(),
            (1u64 << FIELDS) - 1,
            "`total()` does not sum the published fields one apiece — a counter is missing from \
             the fold, counted twice, or the field set changed without this test"
        );
        assert!(one_bit_per_field().any());
    }

    /// A session that discarded nothing reports nothing, and `any()` agrees with `total()`.
    #[test]
    fn a_quiet_session_counts_nothing() {
        let quiet = MediaDiscardCounts::default();
        assert_eq!(quiet.total(), 0);
        assert!(!quiet.any());
    }

    /// A single field moving moves `total()` by exactly that much and nothing else.
    #[test]
    fn one_discard_moves_one_field() {
        let meters = DiscardMeters::default();
        meters
            .malformed_extensions_dropped
            .fetch_add(1, Ordering::Relaxed);
        let counted = meters.snapshot();
        assert_eq!(counted.malformed_extensions_dropped, 1);
        assert_eq!(
            counted.total(),
            1,
            "one discard moved more than one published counter"
        );
    }
}
