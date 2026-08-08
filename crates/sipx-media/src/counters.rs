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
}

impl DiscardMeters {
    pub(crate) fn snapshot(&self) -> MediaDiscardCounts {
        MediaDiscardCounts {
            opus_encode_failures: self.opus_encode_failures.load(Ordering::Relaxed),
            opus_decode_failures: self.opus_decode_failures.load(Ordering::Relaxed),
            srtp_unprotect_failures: self.srtp_unprotect_failures.load(Ordering::Relaxed),
            srtcp_unprotect_failures: self.srtcp_unprotect_failures.load(Ordering::Relaxed),
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
        }
    }
}
