//! A media session: RTP sockets, paced sending, and buffered receiving.
//!
//! Three decisions shape this.
//!
//! **Symmetric RTP.** Media is sent back to where it arrives from, not to the address the SDP
//! advertised. Behind a NAT the advertised address is a private one and the only path back is
//! the pinhole the far end opened by sending. The SDP address is used until the first packet
//! arrives, then the observed source wins.
//!
//! **The clock lives in one place.** Audio is paced by a single interval timer at the
//! packetisation interval. Sending on a channel's readiness instead makes the packet rate
//! depend on how fast the application produces samples, which is how a call ends up sending
//! 200 packets per second to a jitter buffer expecting 50.
//!
//! **Mute substitutes silence; it does not stop the stream** ([`MediaSession::set_muted`], story
//! `M-18`). A muted session sends exactly the packets it would have sent unmuted, on the same
//! pacing, sequence numbers and timestamps, with the audio replaced by encoded silence. The
//! alternative — suppressing the packets while muted — was rejected on three counts: it closes
//! the NAT pinhole and invites a media-inactivity teardown on any path with an SBC in it; it
//! leaves the far end's jitter buffer to restart on unmute, so the first word after it is the one
//! that gets clipped; and it makes "muted" indistinguishable on the wire from "the far end has
//! gone away", which is the one thing a receiver most needs to be able to tell apart.
//!
//! **Playback is a queue with a handle on it** ([`MediaSession::start_playback`], story `M-17`).
//! Clips are played in the order they were started, one at a time; a second clip started while
//! one is running waits behind it rather than replacing it. Stopping is the explicit verb, and it
//! reaches into the send path: a stopped clip's frames are dropped as the send loop takes them
//! off the queue, so a stop costs at most [`Playback::STOP_BOUND_PACKETS`] packets on the wire
//! rather than however many the queue happened to be holding.
//!
//! **The RFC 3550 §6 consequence, either way, is that the reports must stay truthful**, and that
//! is what fixes *where* the gate goes rather than what it does. A sender report's packet and
//! octet counts (§6.4.1) describe what this side put on the wire, and the far end's loss estimate
//! is computed from the sequence numbers it received against the ones it expected. So the gate
//! sits **before the packet is built**: what goes out is counted, what is counted went out, and
//! the sequence space advances once per packet sent. A mute implemented one step later — building
//! the packet, then discarding the datagram — would make this side's own reports overstate what
//! it sent *and* manufacture a burst of apparent loss at the far end out of a caller who was
//! merely quiet. Silence substitution keeps the numbers describing a stream that never stopped;
//! had suppression been chosen, the same rule would have required the counters and the sequence
//! number to stay put for the duration.
//!
//! Dropping a stopped clip's frames is not the case that rule forbids, and the difference is
//! worth being exact about. A mute is a session that is *still talking* and must go on saying
//! something; a stopped playback is a session with **nothing left to say**, which is the state a
//! session is in whenever the application is not feeding it — the send loop simply parks on its
//! queue. So the counters and the sequence number stay put, exactly as they do between clips, and
//! what a stop leaves behind is silence in the ordinary sense: no packets, no gap, nothing for a
//! receiver to score.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::g711;
use sipx_rtp::dtmf::{self, Digit, Event as DtmfEvent};
use sipx_rtp::rtcp::{ReceiverReport, Rtcp, Sdes, StreamStats};
use sipx_rtp::{JitterBuffer, Packet};
use sipx_sdp::ice::ComponentId;
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc, watch};

use crate::counters::{DiscardMeters, MediaDiscardCounts};
use crate::ice;
use crate::processing::{AudioDirection, PcmProcessor, Processing, ProcessingError, Taps};

/// Which G.711 flavour a session carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// µ-law, payload type 0.
    Pcmu,
    /// A-law, payload type 8.
    Pcma,
    /// G.722 wideband sub-band ADPCM, static payload type 9 (RFC 3551 §4.5.2).
    ///
    /// The codec whose two rates differ: the audio is sampled at 16 kHz while the RTP
    /// timestamp clock advances at 8000 — a historical error the RFC preserves on purpose.
    /// Like Opus it carries encoder and decoder *state*, which lives in the send and receive
    /// loops; unlike Opus it is pure Rust and always available.
    G722,
    /// Signed 16-bit network-order linear PCM (RFC 3551 §4.5.11).
    ///
    /// Static payload type 11 is mono at 44.1 kHz. Other negotiated rates use a dynamic payload
    /// assignment and override [`Config::clock_rate`].
    L16,
    /// Opus (RFC 6716), on whatever dynamic payload type was negotiated.
    ///
    /// Unlike the G.711 pair this carries *state*: an Opus encoder and decoder each hold a
    /// model of the signal they have seen, which is how the codec achieves what it does and why
    /// it cannot be a pure function of one frame. The state lives in the send and receive
    /// loops, one each, so nothing is shared and nothing is locked.
    #[cfg(feature = "opus")]
    Opus,
}

/// Which half of a negotiated codec could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecDirection {
    /// The encoder for media sent to the peer.
    Encoder,
    /// The decoder for media received from the peer.
    Decoder,
}

impl std::fmt::Display for CodecDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encoder => f.write_str("encoder"),
            Self::Decoder => f.write_str("decoder"),
        }
    }
}

/// A negotiated media session that cannot be constructed safely.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SetupError {
    /// Packet pacing cannot represent a frame shorter than one millisecond.
    #[error("packet duration must be at least 1 ms, got {0:?}")]
    PacketDurationTooShort(Duration),
    /// A configured RTCP timer must make forward progress.
    #[error("RTCP interval must be at least 1 ms, got {0:?}")]
    RtcpIntervalTooShort(Duration),
    /// RFC 5761 cannot distinguish this marked RTP payload from an RTCP packet type.
    #[error("RTP payload type {0} collides with RTCP while rtcp-mux is active")]
    RtcpMuxPayloadCollision(u8),
    /// The codec agreed through SDP could not create one of its stateful directions.
    #[error("cannot construct {codec:?} {direction}: {reason}")]
    Codec {
        /// The negotiated wire codec.
        codec: Codec,
        /// Whether setup failed for sending or receiving.
        direction: CodecDirection,
        /// The codec library's diagnostic. It contains no media or key material.
        reason: String,
    },
}

/// Binding or constructing a media session failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StartError {
    /// A socket could not be bound.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The negotiated session could not be constructed.
    #[error(transparent)]
    Setup(#[from] SetupError),
}

/// A DTLS handshake could not take a bound media port into an SRTP session.
#[cfg(feature = "dtls")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DtlsStartError {
    /// The media socket could not be converted or configured for the handshake.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Something else retained the not-yet-started socket.
    #[error("the media socket is already shared")]
    SocketShared,
    /// Certificate verification, profile negotiation or key export failed.
    #[error("{0}")]
    Handshake(#[from] crate::dtls::Error),
    /// The bounded blocking handshake worker did not return normally.
    #[error("the DTLS handshake worker failed: {0}")]
    Worker(String),
}

impl Codec {
    /// The static payload type.
    #[must_use]
    pub fn payload_type(self) -> u8 {
        match self {
            Self::Pcmu => 0,
            Self::Pcma => 8,
            Self::G722 => 9,
            Self::L16 => 11,
            // Opus has no static type — RFC 7587 §7 assigns none — so 111 is convention and
            // nothing more. What goes on the wire is whatever SDP negotiated, which
            // [`Config::payload_type`] carries.
            #[cfg(feature = "opus")]
            Self::Opus => 111,
        }
    }

    /// The RTP clock rate, which is not always the sample rate.
    ///
    /// RFC 7587 §7 fixes Opus's RTP clock at 48000 whatever the audio is sampled at, and
    /// RFC 3551 §4.5.2 fixes G.722's at 8000 while its audio is 16 kHz. A stack that used the
    /// sample rate instead produces timestamps the far end reads at the wrong speed.
    #[must_use]
    pub fn clock_rate(self) -> u32 {
        match self {
            Self::Pcmu | Self::Pcma => 8_000,
            Self::G722 => sipx_audio::g722::CLOCK_RATE,
            Self::L16 => 44_100,
            #[cfg(feature = "opus")]
            Self::Opus => sipx_audio::opus::CLOCK_RATE,
        }
    }

    /// How many audio samples one RTP timestamp unit stands for.
    ///
    /// One, for every codec except G.722: RFC 3551 §4.5.2 keeps G.722's RTP clock at 8000
    /// while the audio is sampled at 16 kHz, so each timestamp unit stands for two samples.
    /// Everything that converts between a sample count and a timestamp advance — packet
    /// sizing, the send clock, PCM conversion rates — goes through this ratio, because
    /// advancing timestamps at the sample rate produces audio the far end plays at the wrong
    /// speed and nothing else catches it.
    #[must_use]
    pub fn samples_per_clock_unit(self) -> u32 {
        match self {
            Self::G722 => 2,
            _ => 1,
        }
    }

    /// The codec for a payload type, if it is one we carry.
    #[must_use]
    pub fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self::Pcmu),
            8 => Some(Self::Pcma),
            // RFC 3551 §6: 9 is G.722. The field-reported failure this line exists for is a
            // stack rejecting a bare `m=audio … 9` offer because no `a=rtpmap` accompanied a
            // static type; the number alone identifies the format (`M-44`).
            9 => Some(Self::G722),
            // RFC 3551 §6: 11 is mono L16 at 44.1 kHz. Type 10 is stereo, which this mono
            // application boundary deliberately does not claim.
            11 => Some(Self::L16),
            // Deliberately never Opus. A dynamic payload type means whatever `a=rtpmap` said,
            // and the number alone means nothing: guessing Opus from 111 would decode somebody
            // else's G.729 as Opus. The negotiated number lives on the session's config.
            _ => None,
        }
    }

    fn encode(self, samples: &[i16]) -> Vec<u8> {
        match self {
            Self::Pcmu => g711::ulaw_encode_all(samples),
            Self::Pcma => g711::alaw_encode_all(samples),
            Self::L16 => sipx_audio::l16::encode(samples),
            // Unreachable: a G.722 or Opus session encodes through [`Encoding`], which holds
            // the state this signature has nowhere to put.
            Self::G722 => Vec::new(),
            #[cfg(feature = "opus")]
            Self::Opus => Vec::new(),
        }
    }

    fn decode(self, payload: &[u8]) -> Option<Vec<i16>> {
        match self {
            Self::Pcmu => Some(g711::ulaw_decode_all(payload)),
            Self::Pcma => Some(g711::alaw_decode_all(payload)),
            Self::L16 => sipx_audio::l16::decode(payload).ok(),
            Self::G722 => None,
            #[cfg(feature = "opus")]
            Self::Opus => None,
        }
    }
}

/// Encoding for one outgoing stream.
///
/// Owned by the send loop, which is the whole design: a stateful codec behind a lock would put
/// a mutex in the packet path for no reason, since exactly one task ever encodes.
#[derive(Debug)]
enum Encoding {
    /// Stateless — a pure function of the samples.
    Direct(Codec),
    /// G.722's sub-band predictors, owned like Opus's state and always available.
    G722(Box<sipx_audio::g722::Encoder>),
    #[cfg(feature = "opus")]
    Opus(Box<sipx_audio::opus::Encoder>),
}

impl Encoding {
    #[cfg_attr(not(feature = "opus"), allow(clippy::unnecessary_wraps))]
    fn for_codec(codec: Codec, channels: usize) -> Result<Self, SetupError> {
        match codec {
            Codec::G722 => Ok(Self::G722(Box::new(sipx_audio::g722::Encoder::new()))),
            #[cfg(feature = "opus")]
            Codec::Opus => match sipx_audio::opus::Encoder::new(channels) {
                Ok(encoder) => Ok(Self::Opus(Box::new(encoder))),
                Err(error) => Err(SetupError::Codec {
                    codec,
                    direction: CodecDirection::Encoder,
                    reason: error.to_string(),
                }),
            },
            other => {
                // discard: `channels` exists only for the feature-gated stateful codec.
                let _ = channels;
                Ok(Self::Direct(other))
            }
        }
    }

    // `None` is unreachable without the `opus` feature, because G.711 cannot refuse a frame.
    // The signature stays fallible in both builds so the send loop is one piece of code rather
    // than two that have to be kept saying the same thing.
    #[cfg_attr(not(feature = "opus"), allow(clippy::unnecessary_wraps))]
    fn encode(&mut self, samples: &[i16]) -> Option<Vec<u8>> {
        match self {
            Self::Direct(codec) => Some(codec.encode(samples)),
            Self::G722(encoder) => Some(encoder.encode(samples)),
            #[cfg(feature = "opus")]
            Self::Opus(encoder) => match encoder.encode(samples) {
                Ok(packet) => Some(packet),
                Err(error) => {
                    // discard: the send loop counts the `None` returned from this callback.
                    tracing::debug!(%error, "dropping a frame Opus could not encode");
                    None
                }
            },
        }
    }
}

/// Decoding for one incoming stream. Owned by the receive loop, for the same reason.
#[derive(Debug)]
enum Decoding {
    Direct(Codec),
    G722(Box<sipx_audio::g722::Decoder>),
    #[cfg(feature = "opus")]
    Opus(Box<sipx_audio::opus::Decoder>),
}

impl Decoding {
    #[cfg_attr(not(feature = "opus"), allow(clippy::unnecessary_wraps))]
    fn for_codec(codec: Codec, channels: usize) -> Result<Self, SetupError> {
        match codec {
            Codec::G722 => Ok(Self::G722(Box::new(sipx_audio::g722::Decoder::new()))),
            #[cfg(feature = "opus")]
            Codec::Opus => match sipx_audio::opus::Decoder::new(channels) {
                Ok(decoder) => Ok(Self::Opus(Box::new(decoder))),
                Err(error) => Err(SetupError::Codec {
                    codec,
                    direction: CodecDirection::Decoder,
                    reason: error.to_string(),
                }),
            },
            other => {
                // discard: `channels` exists only for the feature-gated stateful codec.
                let _ = channels;
                Ok(Self::Direct(other))
            }
        }
    }

    #[cfg_attr(not(feature = "opus"), allow(clippy::unnecessary_wraps))]
    fn decode(&mut self, payload: &[u8]) -> Option<Vec<i16>> {
        match self {
            Self::Direct(codec) => codec.decode(payload),
            Self::G722(decoder) => Some(decoder.decode(payload)),
            #[cfg(feature = "opus")]
            Self::Opus(decoder) => match decoder.decode(payload) {
                Ok(samples) => Some(samples),
                Err(error) => {
                    // A packet the codec rejects is dropped, not played. A decoder pushed past
                    // a malformed packet produces noise, and noise is louder than a gap.
                    // discard: `deliver` counts the `None` returned from this callback.
                    tracing::debug!(%error, "dropping a packet Opus could not decode");
                    None
                }
            },
        }
    }
}

/// How a session is configured.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where to send until symmetric RTP learns better.
    pub remote: SocketAddr,
    /// Which codec.
    pub codec: Codec,
    /// The payload type to send with, when the codec's own is not the negotiated one.
    ///
    /// `None` uses [`Codec::payload_type`]. A *dynamic* codec has no number of its own — Opus
    /// has none at all (RFC 7587 §7) — so the number comes from the `a=rtpmap` the two sides
    /// agreed on, and it may differ from the one sipx would have proposed. Assuming otherwise
    /// sends audio on a number the far end has assigned to something else.
    pub payload_type: Option<u8>,
    /// The payload type accepted for this codec when our description assigned a different
    /// dynamic number from the peer's.
    ///
    /// `None` mirrors [`Self::wire_payload_type`], which preserves the ordinary symmetric case.
    /// Offer/answer may nevertheless assign the same codec independently in each direction:
    /// outgoing packets use the peer's number and incoming packets use ours (RFC 3264 §6.1).
    pub receive_payload_type: Option<u8>,
    /// How many channels the codec carries. One, for telephony.
    pub channels: usize,
    /// SRTP keys, if the media is to be encrypted (RFC 3711).
    ///
    /// `None` sends and expects plain RTP. There is deliberately no middle setting — no "accept
    /// either" — because a session that falls back to cleartext when a packet fails to
    /// authenticate is a session an attacker can downgrade by sending one bad packet.
    pub srtp: Option<SrtpKeys>,
    /// How much audio each packet carries. 20 ms is universal; values below 1 ms are rejected
    /// by [`Self::validate`] and every session-start API.
    pub packet_duration: Duration,
    /// RTP-clock samples per second for the exact negotiated format.
    ///
    /// G.711 is always 8000; L16 may use its static 44100 clock or a dynamically mapped rate.
    pub clock_rate: u32,
    /// How many packets the jitter buffer holds, and never fewer.
    pub jitter_depth: usize,
    /// The deepest it may grow when the network misbehaves, in packets.
    ///
    /// `None` fixes the depth at [`Self::jitter_depth`], which is what the comparison tests in
    /// `sipx-rtp` measure the adaptive buffer against. Adapting is the default because being
    /// too shallow is audible and being too deep is not — but the ceiling is a real ceiling: a
    /// call with a quarter-second of delay is still a call, and one with three seconds is not.
    pub jitter_max_depth: Option<usize>,
    /// How often to send RTCP receiver reports. `None` disables RTCP entirely; a configured
    /// interval must be at least 1 ms.
    ///
    /// RFC 3550 §6.2 scales the interval with the session's bandwidth and membership; for a
    /// two-party call that arithmetic lands at the five-second minimum, so sipx uses it
    /// directly rather than implementing a calculation that would always return the same
    /// answer.
    pub rtcp_interval: Option<Duration>,
    /// Whether RTCP uses the RTP socket or its adjacent control socket (RFC 5761).
    pub rtcp_mode: sipx_sdp::RtcpMode,
    /// The payload type carrying `telephone-event`, if the SDP negotiated one.
    ///
    /// It is dynamic, so the number is whatever the answer said — assuming 101 because that
    /// is what sipx offers would decode another endpoint's codec as keypresses.
    pub dtmf_payload_type: Option<u8>,
}

impl Config {
    const MIN_INTERVAL: Duration = Duration::from_millis(1);

    /// The payload type this session puts on the wire.
    #[must_use]
    pub fn wire_payload_type(&self) -> u8 {
        self.payload_type
            .unwrap_or_else(|| self.codec.payload_type())
    }

    /// The payload type this session accepts for its negotiated codec.
    #[must_use]
    pub fn receive_wire_payload_type(&self) -> u8 {
        self.receive_payload_type
            .unwrap_or_else(|| self.wire_payload_type())
    }

    /// A session to a peer in this codec, with the settings everything uses.
    #[must_use]
    pub fn new(remote: SocketAddr, codec: Codec) -> Self {
        Self {
            remote,
            codec,
            payload_type: None,
            receive_payload_type: None,
            channels: 1,
            srtp: None,
            packet_duration: Duration::from_millis(20),
            clock_rate: codec.clock_rate(),
            jitter_depth: 3,
            jitter_max_depth: Some(12),
            rtcp_interval: Some(Duration::from_secs(5)),
            rtcp_mode: sipx_sdp::RtcpMode::Separate,
            dtmf_payload_type: Some(dtmf::DEFAULT_PAYLOAD_TYPE),
        }
    }

    /// How many *audio samples* one packet carries.
    ///
    /// For every codec except G.722 this equals [`Self::clock_units_per_packet`]. G.722 is
    /// the codec whose audio runs at twice its RTP clock (RFC 3551 §4.5.2): a 20 ms packet
    /// carries 320 samples of 16 kHz audio and advances the timestamp by 160.
    #[must_use]
    pub fn samples_per_packet(&self) -> usize {
        self.clock_units_per_packet()
            .saturating_mul(usize::try_from(self.codec.samples_per_clock_unit()).unwrap_or(1))
    }

    /// How many RTP timestamp units one packet advances the clock by.
    #[must_use]
    pub fn clock_units_per_packet(&self) -> usize {
        let millis = u64::try_from(self.packet_duration.as_millis()).unwrap_or(20);
        usize::try_from(u64::from(self.clock_rate) * millis / 1000).unwrap_or(160)
    }

    /// The audio sampling rate of the samples this session consumes and produces.
    ///
    /// [`Self::clock_rate`] times the codec's samples-per-unit ratio: equal for every codec
    /// except G.722, whose audio is 16 kHz over an 8000 RTP clock. PCM conversion, resampling
    /// and WAV headers use this rate; timestamps, jitter and RFC 4733 durations use the clock.
    #[must_use]
    pub fn audio_rate(&self) -> u32 {
        self.clock_rate
            .saturating_mul(self.codec.samples_per_clock_unit())
    }

    /// Check the values used by worker timers before any worker or socket starts.
    ///
    /// # Errors
    ///
    /// Returns [`SetupError::PacketDurationTooShort`] or
    /// [`SetupError::RtcpIntervalTooShort`] for a duration below one millisecond.
    pub fn validate(&self) -> Result<(), SetupError> {
        if self.packet_duration < Self::MIN_INTERVAL {
            return Err(SetupError::PacketDurationTooShort(self.packet_duration));
        }
        if let Some(interval) = self.rtcp_interval
            && interval < Self::MIN_INTERVAL
        {
            return Err(SetupError::RtcpIntervalTooShort(interval));
        }
        if self.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            for payload in [
                Some(self.wire_payload_type()),
                Some(self.receive_wire_payload_type()),
                self.dtmf_payload_type,
            ]
            .into_iter()
            .flatten()
            {
                if (64..=95).contains(&payload) {
                    return Err(SetupError::RtcpMuxPayloadCollision(payload));
                }
            }
        }
        Ok(())
    }
}

/// Everything that can fail before a session's first worker is spawned.
struct Prepared {
    encoding: Encoding,
    decoding: Decoding,
}

impl Prepared {
    fn new(config: &Config) -> Result<Self, SetupError> {
        config.validate()?;
        // Construct both directions before spawning either. A half-started negotiated codec is
        // not a media session, and substituting another codec would break the payload contract.
        let encoding = Encoding::for_codec(config.codec, config.channels)?;
        let decoding = Decoding::for_codec(config.codec, config.channels)?;
        Ok(Self { encoding, decoding })
    }
}

/// What the paced send queue carries.
///
/// Audio and DTMF share one queue because they share one clock and one sequence number space.
/// A separate path for events would have to interleave them anyway, and would get the
/// sequence numbering wrong the first time both were busy.
#[derive(Debug)]
enum Frame {
    /// One packet's worth of samples.
    ///
    /// `playback` is the stop signal of the clip this frame belongs to, when it belongs to one
    /// (`M-17`). The send loop reads it and drops the frame if that playback has been stopped,
    /// which is what makes a stop cost a bounded number of packets rather than the whole depth of
    /// this queue. `None` for a frame the application sent directly through
    /// [`MediaSession::send`], which nothing can cancel.
    Audio {
        samples: Vec<i16>,
        playback: Option<Arc<Stop>>,
    },
    /// One telephone event, tagged with the keypress it belongs to.
    ///
    /// The tag is what holds a tone together. Every packet of one keypress must carry the same
    /// RTP timestamp, including the three end retransmissions — and the send loop cannot tell
    /// from an end packet alone whether more of them are coming. Without the tag it started a
    /// new tone on each retransmission, and one keypress arrived as three digits.
    ///
    /// `offset` is the packet's segment start within the event: zero until a keypress
    /// outlives the 16-bit duration field, after which each further segment stamps its
    /// packets that much past the event's start (RFC 4733 §2.5.1.3).
    Dtmf {
        event: DtmfEvent,
        offset: u32,
        tone: u64,
    },
    /// Payload to put on the wire exactly as given.
    ///
    /// For a bridge between two calls that agreed on the same codec. G.711 survives a decode
    /// and re-encode exactly, so for the codec sipx ships today this saves work rather than
    /// quality; for any codec whose decode is not invertible it saves both. See
    /// [`crate::bridge`].
    Encoded { payload_type: u8, payload: Bytes },
}

/// The master keys for one SRTP session, one direction each.
///
/// Separate directions because RFC 3711 keys them separately: each side offers its own key in
/// SDP and uses the other's to decrypt. Sharing one key between directions would give both ends
/// the same keystream for the same packet index, which is the classic way to lose a stream
/// cipher.
#[derive(Clone, PartialEq, Eq)]
pub struct SrtpKeys {
    /// The protection profile both ends negotiated.
    ///
    /// Carried rather than inferred. The keying path knows which transform was agreed — an SDES
    /// crypto-suite token or a DTLS-SRTP `use_srtp` profile — and a media session that guessed it
    /// back from how many octets of key arrived would install whichever cipher happened to share
    /// a key length. `docs/designs/media-runtime-safety.md`: never install a different cipher
    /// under a negotiated identifier.
    pub profile: sipx_rtp::srtp::Profile,
    /// Master key and salt this side encrypts with — the one offered in our SDP.
    pub local: (Vec<u8>, Vec<u8>),
    /// Master key and salt the far end encrypts with — the one from its SDP.
    pub remote: (Vec<u8>, Vec<u8>),
}

impl SrtpKeys {
    /// The keys an SDES answer settled on, **after** checking it against what was offered
    /// (RFC 4568 §5.1.3; `docs/specs/srtp.md` §5.4).
    ///
    /// This is the seam between the signalling and the media path, and the reason it is fallible
    /// rather than an `Option`: an answer that echoed a tag this side never sent has agreed to
    /// nothing, and the two outcomes that are not an error are both worse than one. Returning
    /// `None` would place the call unencrypted, so a user who asked for a secure call gets an
    /// insecure one and nothing says so; dropping the stream would end the call with no reason
    /// anyone can act on. The error carries which tag came back and why it was refused.
    ///
    /// `answered` is `None` when the answer carried no `a=crypto` this side can perform — the
    /// shape in which "a suite that was never offered" arrives, since
    /// [`sipx_sdp::crypto::Crypto::parse`] refuses one sipx cannot key.
    ///
    /// # Errors
    ///
    /// [`sipx_sdp::SdpError::Invalid`] when the answer accepted a tag and suite pair that was
    /// never offered, or carried no key. It never names key material.
    pub fn from_answer(
        offered: &[sipx_sdp::crypto::Crypto],
        answered: Option<&sipx_sdp::crypto::Crypto>,
    ) -> Result<Self, sipx_sdp::SdpError> {
        let ours = sipx_sdp::crypto::Crypto::verify_answer(offered, answered)?;
        // `verify_answer` returning `Ok` is what makes this `answered` usable at all, so the
        // far half is read only here and never from an answer that was not checked.
        let theirs = answered.ok_or(sipx_sdp::SdpError::Invalid {
            field: "crypto",
            value: "the answer carried no crypto attribute this side can perform".to_owned(),
        })?;
        Ok(Self {
            // `verify_answer` has already established that the two agree on the suite, so this is
            // the negotiated transform and not a preference of ours applied after the fact.
            profile: transform_of(ours.suite),
            local: (ours.master_key().to_vec(), ours.master_salt().to_vec()),
            remote: (theirs.master_key().to_vec(), theirs.master_salt().to_vec()),
        })
    }
}

/// The SRTP transform an SDES crypto-suite names.
///
/// The single point where an RFC 4568 token becomes a cipher, mirroring
/// [`crate::dtls::Profile::transform`] for the other keying path. One function per path and no
/// third opinion anywhere: that is what makes "never install a different cipher under a
/// negotiated identifier" something a reader can check.
#[must_use]
pub fn transform_of(suite: sipx_sdp::crypto::Suite) -> sipx_rtp::srtp::Profile {
    match suite {
        sipx_sdp::crypto::Suite::AeadAes256Gcm => sipx_rtp::srtp::Profile::AeadAes256Gcm,
        sipx_sdp::crypto::Suite::AeadAes128Gcm => sipx_rtp::srtp::Profile::AeadAes128Gcm,
        _ => sipx_rtp::srtp::Profile::AesCm128HmacSha1_80,
    }
}

impl std::fmt::Debug for SrtpKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keys. A derived `Debug` puts them in whatever log the caller writes.
        f.write_str("SrtpKeys { .. }")
    }
}

/// A packet's payload as it arrived, still encoded.
#[derive(Debug, Clone)]
pub struct Encoded {
    /// What it is encoded in.
    pub payload_type: u8,
    /// The bytes.
    pub payload: Bytes,
}

/// One peer RTCP report block describing this session's outbound RTP stream.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RtcpQualitySample {
    /// SSRC of the peer that sent the sender or receiver report.
    pub reporter_ssrc: u32,
    /// Local stream SSRC named by the report block.
    pub stream_ssrc: u32,
    /// Loss in this report interval, between zero and one.
    pub loss: f64,
    /// Packets lost since the peer began observing the stream.
    pub cumulative_lost: i32,
    /// Peer-observed interarrival jitter in time rather than RTP timestamp units.
    pub jitter: Duration,
    /// Round-trip time derived from `LSR` and `DLSR`, when the report carries a usable echo.
    pub round_trip: Option<Duration>,
}

/// Application-owned handling for peer RTCP quality reports.
///
/// The callback runs on the RTCP receive worker after parsing and outside sipx locks. It must
/// return promptly; applications that do blocking export put a bounded queue behind it. sipx
/// catches a callback panic so application code cannot terminate the media worker.
#[derive(Clone)]
pub struct RtcpQualityHook(Arc<dyn Fn(RtcpQualitySample) + Send + Sync + 'static>);

impl RtcpQualityHook {
    /// Wrap an application callback.
    pub fn new(callback: impl Fn(RtcpQualitySample) + Send + Sync + 'static) -> Self {
        Self(Arc::new(callback))
    }

    fn observe(&self, sample: RtcpQualitySample) {
        (self.0)(sample);
    }
}

impl std::fmt::Debug for RtcpQualityHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RtcpQualityHook { .. }")
    }
}

type QualityHookSlot = Arc<std::sync::RwLock<Option<RtcpQualityHook>>>;

fn current_quality_hook(slot: &QualityHookSlot) -> Option<RtcpQualityHook> {
    match slot.read() {
        Ok(held) => held.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn replace_quality_hook(slot: &QualityHookSlot, hook: Option<RtcpQualityHook>) {
    match slot.write() {
        Ok(mut held) => *held = hook,
        Err(poisoned) => *poisoned.into_inner() = hook,
    }
}

/// A running media session.
#[derive(Debug)]
pub struct MediaSession {
    /// The bound RTP socket, retained so an SDP renegotiation can rebuild codec workers without
    /// rebinding the address the peer already knows.
    socket: Arc<UdpSocket>,
    /// The paired RTCP socket, for the same purpose.
    rtcp_socket: Option<Arc<UdpSocket>>,
    outgoing: mpsc::Sender<Frame>,
    digits: Mutex<mpsc::Receiver<(Digit, Duration)>>,
    /// Distinguishes one keypress from the next.
    tones: AtomicU64,
    incoming: Mutex<mpsc::Receiver<Vec<i16>>>,
    encoded: Mutex<mpsc::Receiver<Encoded>>,
    /// Whether received packets are handed on encoded rather than decoded to samples.
    relay: Arc<AtomicBool>,
    /// Whether this side's outbound audio is gated to silence (`M-18`).
    muted: Arc<AtomicBool>,
    /// Clips waiting to be played, in the order they were started (`M-17`).
    clips: mpsc::Sender<Clip>,
    /// Names the next playback. Never reused within a session.
    playbacks: AtomicU64,
    /// How many started clips have not yet resolved, for [`Self::flush`].
    outstanding: Arc<AtomicUsize>,
    /// A counter of full keypresses received, bumped by the receive loop after the digit is on
    /// its way to the application. What an [`Interrupt::OnDigit`] playback watches.
    keypresses: Arc<watch::Sender<u64>>,
    codec: Codec,
    wire_payload_type: u8,
    receive_payload_type: u8,
    /// Retained non-secret wire facts for validated runtime attachment after a host restart.
    dtmf_payload_type: Option<u8>,
    rtcp_mode: sipx_sdp::RtcpMode,
    encrypted: bool,
    /// The protection profile the keying settled on, `None` when the media is not encrypted.
    srtp_profile: Option<sipx_rtp::srtp::Profile>,
    local_addr: SocketAddr,
    samples_per_packet: usize,
    packet_duration: Duration,
    clock_rate: u32,
    /// The local SSRC carried by both RTP and RTCP for this generation.
    ssrc: u32,
    /// Application observation follows the logical session across worker replacement.
    quality_hook: QualityHookSlot,
    sent: Arc<AtomicU64>,
    received: Arc<AtomicU64>,
    /// Losses owned by this session, including candidate gathering on the port it consumed.
    discards: Arc<DiscardMeters>,
    stats: Arc<Mutex<StreamStats>>,
    /// What the far end last told us, and when.
    feedback: Arc<Mutex<Feedback>>,
    /// The running ICE driver, for the exchanges that come after the one that started the session
    /// ([`Self::renegotiate_ice`], `docs/specs/ice.md` §13.5).
    ///
    /// The loops were handed their own clones when they were spawned; this one is the signalling
    /// layer's, and it is what makes a re-offer able to reach the agent at all. `None` is a stream
    /// with no ICE, which is the default and must stay indistinguishable from the pre-ICE session.
    ice: Option<crate::ice::driver::Handle>,
    /// Browser-component security facts, when the one-owner path established this session.
    browser_ingress: Option<Arc<std::sync::Mutex<crate::browser::ComponentIngress>>>,
    /// Every asynchronous worker this session started. A handle remains registered while
    /// `shutdown` awaits it, which makes a cancelled shutdown retryable instead of detached.
    owners: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Stopped generations replaced in place but not yet completely joined. The replacement owns
    /// them before its first await, so cancellation cannot turn reconfiguration into detachment.
    retired: Mutex<Vec<MediaSession>>,
    /// Attached application PCM processors (`M-54`, `docs/specs/call-audio-seam.md`).
    ///
    /// The registry, not the workers: the seam spawns nothing, so this adds no handle for
    /// shutdown to join. A renegotiation carries the attachments to the replacement generation.
    taps: Arc<Taps>,
    #[cfg(all(test, feature = "dtls"))]
    browser_profile_tasks: Option<Arc<crate::browser::ProfileTasks>>,
    #[cfg(all(test, feature = "dtls"))]
    browser_preparing_peak: Option<usize>,
    stop: Arc<Stop>,
}

/// A sole-consumer linear-PCM view of one session's received audio.
///
/// The handle owns its rate-conversion history, so consecutive RTP frames remain one continuous
/// output stream. Creating one does not spawn work; it reads the same bounded receive queue as
/// [`MediaSession::recv`].
#[derive(Debug)]
pub struct PcmCapture<'a> {
    session: &'a MediaSession,
    format: sipx_audio::PcmFormat,
    resampler: sipx_audio::LinearResampler,
}

impl PcmCapture<'_> {
    /// The application format this capture emits.
    #[must_use]
    pub const fn format(&self) -> sipx_audio::PcmFormat {
        self.format
    }

    /// Take the next non-empty converted PCM chunk.
    pub async fn recv(&mut self) -> Option<sipx_audio::Pcm> {
        loop {
            let frame = self.session.recv().await?;
            let converted = self.resampler.push_i16(&frame);
            if !converted.is_empty() {
                return Some(sipx_audio::Pcm::from_i16(self.format, converted));
            }
        }
    }

    /// Record at least `samples` in the chosen output format, or until `within` elapses.
    ///
    /// `within` bounds failure rather than defining stream silence, matching
    /// [`MediaSession::record_at_least`]. Whatever arrived before the bound is retained.
    pub async fn record_at_least(&mut self, samples: usize, within: Duration) -> sipx_audio::Pcm {
        let deadline = tokio::time::Instant::now() + within;
        let mut recorded = Vec::with_capacity(samples);
        while recorded.len() < samples {
            match tokio::time::timeout_at(deadline, self.session.recv()).await {
                Ok(Some(frame)) => recorded.extend(self.resampler.push_i16(&frame)),
                Ok(None) | Err(_) => break,
            }
        }
        recorded.truncate(samples);
        sipx_audio::Pcm::from_i16(self.format, recorded)
    }
}

/// What this side has sent, as a sender report describes it (RFC 3550 §6.4.1).
#[derive(Debug, Default)]
struct Outbound {
    packets: AtomicU64,
    octets: AtomicU64,
    /// The timestamp of the most recent packet, so a report can relate the RTP clock to the
    /// wallclock — which is what lets a receiver synchronise two streams.
    timestamp: std::sync::atomic::AtomicU32,
}

/// What the far end has told us, from the RTCP it sends back.
#[derive(Debug, Default, Clone, Copy)]
struct Feedback {
    /// The most recent round-trip time, computed per RFC 3550 §6.4.1.
    ///
    /// Most recent rather than averaged: the calculation is a difference of two clocks, and a
    /// clock that steps mid-call poisons an average for the rest of the session while it
    /// only spoils one sample.
    round_trip: Option<Duration>,
    /// The middle 32 bits of the last sender report the far end sent us, and when it arrived.
    /// Echoed back in our own reports so the far end can measure the round trip too.
    last_sender_report: u32,
    received_at: Option<tokio::time::Instant>,
}

/// Application-visible state updated by either RTCP receive shape.
#[derive(Clone)]
struct RtcpObservation {
    feedback: Arc<Mutex<Feedback>>,
    quality_hook: QualityHookSlot,
    clock_rate: u32,
}

/// A stop signal: for a session's tasks, and — the same shape, one scope down — for one
/// playback (`M-17`).
///
/// A flag *and* a notify. `Notify::notify_waiters` only wakes tasks already parked on it, so a
/// loop that happens to be blocked on its channel when stop is called would never learn — and
/// would go on sending audio into a call that had been hung up. The flag makes the signal
/// durable; the notify makes it prompt.
#[derive(Debug, Default)]
pub(crate) struct Stop {
    stopped: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Stop {
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    pub(crate) async fn wait(&self) {
        // Register before reading the durable flag. The opposite order has a lost-wake window:
        // `stop` can set the flag and notify after the check but before `notified()` registers,
        // leaving this future asleep forever despite the flag saying it is stopped.
        let notified = self.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !self.is_stopped() {
            notified.await;
        }
    }
}

/// The state every loop in a session shares, built in one place.
///
/// One place because two of these must not be rolled twice: RFC 3550 §8.1 requires a
/// participant's RTCP to carry the same SSRC as its RTP, so the send loop and the report loop
/// cannot each choose one, and §6.5.1's CNAME has to be stable for the session.
struct Shared {
    sent: Arc<AtomicU64>,
    received: Arc<AtomicU64>,
    discards: Arc<DiscardMeters>,
    outbound: Arc<Outbound>,
    feedback: Arc<Mutex<Feedback>>,
    /// Zero until the first packet names the far end's synchronisation source.
    stats: Arc<Mutex<StreamStats>>,
    stop: Arc<Stop>,
    ssrc: u32,
    cname: String,
    /// Whether received packets are handed on encoded rather than decoded to samples.
    relay: Arc<AtomicBool>,
    /// Whether this side's outbound audio is gated to silence (`M-18`).
    muted: Arc<AtomicBool>,
    /// A counter of full keypresses received, for an `Interrupt::OnDigit` playback to watch.
    keypresses: Arc<watch::Sender<u64>>,
    quality_hook: QualityHookSlot,
    /// The application PCM processing seam both media loops offer their frames to (`M-54`).
    taps: Arc<Taps>,
}

impl Shared {
    fn new(local_addr: SocketAddr, discards: Arc<DiscardMeters>) -> Self {
        Self::with_stop(local_addr, discards, Arc::new(Stop::default()))
    }

    fn with_stop(local_addr: SocketAddr, discards: Arc<DiscardMeters>, stop: Arc<Stop>) -> Self {
        let taps = Arc::new(Taps::new(Arc::clone(&discards)));
        Self {
            sent: Arc::new(AtomicU64::new(0)),
            received: Arc::new(AtomicU64::new(0)),
            discards,
            outbound: Arc::new(Outbound::default()),
            feedback: Arc::new(Mutex::new(Feedback::default())),
            stats: Arc::new(Mutex::new(StreamStats::new(0))),
            stop,
            ssrc: rand::random(),
            // Unique in user@host form and stable for the session: a random token distinguishes
            // sessions on this host, the local address distinguishes hosts, and neither needs a
            // name lookup on the media path.
            cname: format!("{:08x}@{}", rand::random::<u32>(), local_addr),
            relay: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(false)),
            keypresses: Arc::new(watch::Sender::new(0u64)),
            quality_hook: Arc::new(std::sync::RwLock::new(None)),
            taps,
        }
    }

    fn rtcp_observation(&self, clock_rate: u32) -> RtcpObservation {
        RtcpObservation {
            feedback: Arc::clone(&self.feedback),
            quality_hook: Arc::clone(&self.quality_hook),
            clock_rate,
        }
    }
}

/// Identifies one playback on one session.
///
/// Carried by [`Playback`] and by the completion event a call reports it through, so a caller
/// that started several clips can tell which of them the report is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlaybackId(u64);

impl std::fmt::Display for PlaybackId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// How a playback ended.
///
/// A caller needs to be able to tell these apart: "the announcement finished", "the application
/// cut it off", "the caller pressed a key", "the call went away underneath it" and "it never
/// played at all" lead to different next steps. [`Self::completed`] is the one-bit answer for
/// callers that only need to know whether the clip ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaybackEnd {
    /// The whole clip reached the send queue.
    Completed,
    /// [`Playback::stop`] cut it short.
    Stopped,
    /// A keypress from the far end cut it short — it was started [`Interrupt::OnDigit`] and the
    /// far end pressed a key (RFC 4733). The keypress itself is still delivered to whoever is
    /// reading [`MediaSession::recv_digit`]; interrupting consumes nothing.
    Interrupted,
    /// The session stopped, or the call ended, under a playback still running.
    SessionEnded,
    /// The playback queue was full ([`Playback::QUEUE_DEPTH`] clips already waiting), so nothing was
    /// played at all.
    Refused,
}

impl PlaybackEnd {
    /// Whether the clip ran to its end, as opposed to being cut short by anything.
    #[must_use]
    pub fn completed(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Whether a keypress from the far end cuts a playback short.
///
/// This is the switch under the application contract's `gather{prompt, interruptible}`
/// (`docs/specs/app-contract.md` §6.2): the prompt of a gather is interruptible by definition,
/// and a bare `play` is not unless it says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Interrupt {
    /// The clip plays to its end whatever the far end presses.
    #[default]
    Never,
    /// The first full keypress (RFC 4733) received *after this clip reaches the head of the
    /// queue* stops it.
    ///
    /// After, not before: a key pressed while an earlier clip was still playing belongs to that
    /// clip, and letting it arm this one would have a single keypress skip a whole prompt
    /// sequence.
    OnDigit,
}

/// A playback in progress, or one that has already ended.
///
/// Returned by [`MediaSession::start_playback`] without waiting for the clip: the point of the
/// handle is that the caller goes on to do something else — collect digits, watch for a hangup —
/// while the audio plays, and can reach back to stop it.
///
/// Cloneable, and deliberately so: the handle is a control surface, not ownership of a resource.
/// A call hands one clone to the application and keeps another to report the playback's end on
/// its event stream.
#[derive(Debug, Clone)]
pub struct Playback {
    id: PlaybackId,
    stop: Arc<Stop>,
    end: watch::Receiver<Option<PlaybackEnd>>,
}

impl Playback {
    /// How many packets of a stopped playback may still reach the wire after it is cut.
    ///
    /// **Two**, at 20 ms each by default — a number rather than "promptly", because the whole
    /// difference between playback that can be controlled and playback that cannot is whether an
    /// application can say how long barge-in takes.
    ///
    /// Where it comes from: [`MediaSession::start_playback`] runs ahead of the wire, so when a
    /// clip is stopped the send queue is generally holding its next few dozen packets. Those are
    /// not sent. The send loop tests each frame's playback against this signal as it takes the
    /// frame off the queue and discards a stopped one *without spending a packet interval on it*,
    /// so the whole backlog drains inside one tick. What can still go out is the packet the send
    /// loop had already committed to the socket when the signal was set, and — allowing for the
    /// stop landing between taking a frame and sending it — the one after it.
    ///
    /// The same bound covers [`Interrupt::OnDigit`]: an interruption sets the same signal, one
    /// task hop after the keypress is delivered.
    pub const STOP_BOUND_PACKETS: u64 = 2;

    /// How many clips may be waiting behind the one playing before further ones are refused.
    ///
    /// A bound rather than an unbounded queue because the caller of
    /// [`MediaSession::start_playback`] is not always a program somebody wrote by hand — under the
    /// application contract it is a remote app sending instructions, and a queue that grows
    /// without limit turns a buggy app into this process's memory problem. Deep enough that no
    /// prompt sequence a call actually has time for will reach it.
    pub const QUEUE_DEPTH: usize = 32;

    /// Which playback this is.
    #[must_use]
    pub fn id(&self) -> PlaybackId {
        self.id
    }

    /// Cut this playback short.
    ///
    /// Takes effect within [`Self::STOP_BOUND_PACKETS`] packets. Idempotent, and harmless on a
    /// playback that has already ended. Does not wait: [`Self::finished`] is how a caller learns
    /// it has landed.
    pub fn stop(&self) {
        self.stop.stop();
    }

    /// Whether this playback has been asked to stop — by [`Self::stop`] or by a keypress.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stop.is_stopped()
    }

    /// How it ended, if it has, without waiting.
    #[must_use]
    pub fn end(&self) -> Option<PlaybackEnd> {
        *self.end.borrow()
    }

    /// Wait for it to end, and stop it if the wait itself is abandoned.
    ///
    /// The difference from [`Self::finished`] is what happens when *this future* is dropped
    /// before the clip ends — a caller that wrapped the wait in a `timeout`, or lost a `select!`.
    /// It stops the playback, because that is what abandoning a `play` has always meant: the
    /// audio stops with the caller's interest in it, rather than playing on out of a task the
    /// caller no longer holds a handle to.
    ///
    /// [`MediaSession::play`] is this method, which is how it keeps that property now that the
    /// clip is fed by a task of its own rather than by the caller.
    pub async fn play_out(&self) -> PlaybackEnd {
        /// Stops the playback if the wait is dropped before it settles. Not on the way out of a
        /// clip that ended on its own: the last packets of a completed clip are still in the send
        /// queue, and stopping then would discard them and clip the tail off every announcement.
        struct StopIfAbandoned<'a>(&'a Playback);
        impl Drop for StopIfAbandoned<'_> {
            fn drop(&mut self) {
                if self.0.end().is_none() {
                    self.0.stop();
                }
            }
        }

        let guard = StopIfAbandoned(self);
        guard.0.finished().await
    }

    /// Wait for it to end, and say how. Observation only: dropping this wait does not touch the
    /// playback, which goes on to whatever end it was going to reach.
    ///
    /// Resolves when the decision is made rather than when the last packet is on the wire — which
    /// is what a caller wants of an interruption, since the next thing it does is act on the
    /// keypress. The stopped clip's remaining audio is already guaranteed not to be sent by then.
    ///
    /// Takes `&self`, so several parties may await the same playback.
    pub async fn finished(&self) -> PlaybackEnd {
        let mut end = self.end.clone();
        loop {
            let settled = *end.borrow_and_update();
            if let Some(settled) = settled {
                return settled;
            }
            if end.changed().await.is_err() {
                // The queue task is gone without having recorded an end, which only happens when
                // the session went away underneath this clip.
                return PlaybackEnd::SessionEnded;
            }
        }
    }
}

/// One clip on its way to the send queue, as the playback task receives it.
#[derive(Debug)]
struct Clip {
    samples: Vec<i16>,
    samples_per_packet: usize,
    interrupt: Interrupt,
    /// Shared with every [`Frame::Audio`] this clip produces, so stopping the playback also
    /// discards whatever of it the send queue is already holding.
    stop: Arc<Stop>,
    end: watch::Sender<Option<PlaybackEnd>>,
    /// How many full keypresses the receive loop has delivered. Watched, not consumed: an
    /// interruption must not take the digit away from the application.
    keypresses: watch::Receiver<u64>,
    /// How many clips the session has accepted and not yet resolved, so
    /// [`MediaSession::flush`] can tell a queue with work left in it from an empty one.
    /// Decremented by this clip's destructor, so it balances whether the clip played, was
    /// refused, or was dropped with the session.
    outstanding: Arc<AtomicUsize>,
    discards: Arc<DiscardMeters>,
}

impl Clip {
    /// Record how this clip ended, for whoever is holding its [`Playback`].
    fn finish(&self, end: PlaybackEnd) {
        // Failure means every handle has been dropped, which is a caller that started a clip and
        // never looked back — a legitimate thing to do with an announcement.
        if self.end.send(Some(end)).is_err() {
            self.discards
                .playback_completion_unobserved
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Drop for Clip {
    fn drop(&mut self) {
        self.outstanding.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A bound media port that is not yet carrying anything.
///
/// This exists because of an ordering constraint in offer/answer: an SDP offer has to name the
/// port audio will arrive on, but the codec and the far end's address are not known until the
/// answer comes back. So the socket is bound first, its port goes into the offer, and the
/// session starts once there is something to start it with.
///
/// Binding twice instead — once to learn the port, once to start — fails with "address already
/// in use", which is how this type came to exist.
#[derive(Debug)]
pub struct MediaPort {
    socket: Arc<UdpSocket>,
    /// The control port, one above the media one (RFC 3550 §11).
    ///
    /// `None` when it could not be had. Media still works without it; what is lost is
    /// everything the far end would have told us about what it is hearing — including the
    /// round-trip time, which has nowhere else to come from.
    rtcp: Option<Arc<UdpSocket>>,
    local_addr: SocketAddr,
    /// Created at bind time so gathering losses survive the transition into a session.
    discards: Arc<DiscardMeters>,
}

impl MediaPort {
    /// Bind a port, and the control port above it. Port 0 asks the OS to choose.
    ///
    /// RFC 3550 §11: RTP on an even port, RTCP on the next one up. They are bound together
    /// because a session that sends reports and cannot receive them is half a control
    /// protocol — it can tell the far end what it is hearing and can never learn what the far
    /// end hears, and the round-trip time comes from exactly that.
    ///
    /// Failing to get the control port is not failing to place the call. The pair is attempted,
    /// and if no pair is free the media port is taken alone and reporting is one-way.
    pub async fn bind(bind: SocketAddr) -> std::io::Result<Self> {
        const ATTEMPTS: usize = 16;

        if bind.port() == 0 {
            for _ in 0..ATTEMPTS {
                let socket = UdpSocket::bind(bind).await?;
                let local_addr = socket.local_addr()?;
                // An odd media port has no room for its control port above it by convention.
                // Dropping the socket lets the OS hand the number out again.
                if local_addr.port() % 2 != 0 {
                    continue;
                }
                if let Some(rtcp) = bind_control_port(local_addr).await {
                    return Ok(Self {
                        socket: Arc::new(socket),
                        rtcp: Some(rtcp),
                        local_addr,
                        discards: Arc::new(DiscardMeters::default()),
                    });
                }
            }
        }

        let socket = Arc::new(UdpSocket::bind(bind).await?);
        let local_addr = socket.local_addr()?;
        let rtcp = bind_control_port(local_addr).await;
        if rtcp.is_none() {
            tracing::debug!(%local_addr, "no control port; RTCP will be send-only");
        }
        Ok(Self {
            socket,
            rtcp,
            local_addr,
            discards: Arc::new(DiscardMeters::default()),
        })
    }

    /// The port audio will arrive on — what goes in the SDP.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Whether this port got the control port above the media one (RFC 3550 §11).
    ///
    /// It decides what ICE may offer: `docs/specs/ice.md` §6.1 puts component 2 in the offer
    /// **only** when the control port was actually obtained, because a candidate for a socket
    /// that was never bound is an address the peer will check and nothing will answer on.
    #[must_use]
    pub fn has_control_port(&self) -> bool {
        self.rtcp.is_some()
    }

    /// Run DTLS on this port and return it with the derived SRTP master material.
    ///
    /// The handshake borrows a duplicated descriptor for the same bound socket. No RTP worker is
    /// running yet, so it is the only reader; once it finishes, that duplicate is dropped and the
    /// original descriptor is restored to Tokio for [`Self::start`]. The timeout is enforced by
    /// the DTLS socket itself, making the blocking worker bounded even if this future is cancelled.
    #[cfg(feature = "dtls")]
    pub async fn key_with_dtls(
        self,
        identity: crate::dtls::openssl::Identity,
        peer: SocketAddr,
        role: crate::dtls::Role,
        fingerprint: sipx_sdp::fingerprint::Fingerprint,
        timeout: Duration,
    ) -> Result<(Self, SrtpKeys), DtlsStartError> {
        let Self {
            socket,
            rtcp,
            local_addr,
            discards,
        } = self;
        let socket = Arc::try_unwrap(socket).map_err(|_| DtlsStartError::SocketShared)?;
        let socket = socket.into_std()?;
        socket.set_nonblocking(false)?;
        let handshake_socket = socket.try_clone()?;

        let keys = tokio::task::spawn_blocking(move || {
            let mut handshake =
                crate::dtls::openssl::Session::new(handshake_socket, peer, &identity, timeout)
                    .map_err(|error| crate::dtls::Error::Dtls(error.to_string()))?;
            crate::dtls::establish(&mut handshake, role, Some(&fingerprint))
        })
        .await
        .map_err(|error| DtlsStartError::Worker(error.to_string()))??
        .into_srtp_keys();

        socket.set_read_timeout(None)?;
        socket.set_write_timeout(None)?;
        socket.set_nonblocking(true)?;
        let socket = UdpSocket::from_std(socket)?;
        Ok((
            Self {
                socket: Arc::new(socket),
                rtcp,
                local_addr,
                discards,
            },
            keys,
        ))
    }

    /// Gather ICE candidates on this port's sockets (RFC 8445 §5.1.1).
    ///
    /// Called between binding and offering: the sockets are exclusively ours until
    /// [`Self::start`] or [`Self::start_with_ice`] spawns the loops that read them, which is the
    /// window a STUN transaction to a configured server needs.
    ///
    /// The result carries the `a=candidate` lines for the description
    /// ([`ice::LocalDescription::attributes`]) and the agent that will drive them.
    pub async fn gather(&self, gathering: &ice::Gathering) -> ice::LocalDescription {
        self.gather_with_rtcp_mode(gathering, sipx_sdp::RtcpMode::Separate)
            .await
    }

    /// Gather ICE candidates for the negotiated RTCP shape.
    ///
    /// A muxed stream has only component 1. The default [`Self::gather`] retains the historical
    /// two-component behavior for callers that have not selected RFC 5761.
    pub async fn gather_with_rtcp_mode(
        &self,
        gathering: &ice::Gathering,
        rtcp_mode: sipx_sdp::RtcpMode,
    ) -> ice::LocalDescription {
        let mut bases = vec![ice::gather::Base {
            index: ice::LocalBase(0),
            component: ComponentId::RTP,
            socket: &self.socket,
        }];
        // Component 2 only when the control port was actually obtained (`ice.md` §6.1).
        if rtcp_mode == sipx_sdp::RtcpMode::Separate
            && let Some(rtcp) = &self.rtcp
        {
            bases.push(ice::gather::Base {
                index: ice::LocalBase(1),
                component: ComponentId::RTCP,
                socket: rtcp,
            });
        }
        ice::gather::gather(&bases, gathering, Arc::clone(&self.discards)).await
    }

    /// Start carrying media, now that negotiation has said where and in what.
    ///
    /// Validation and construction finish before the first worker is spawned. On error this
    /// consumes and releases the bound port.
    ///
    /// # Errors
    ///
    /// Returns [`SetupError`] when timing is invalid or the negotiated codec cannot be built.
    pub fn start(self, config: Config) -> Result<MediaSession, SetupError> {
        let prepared = Prepared::new(&config)?;
        Ok(MediaSession::on_socket(
            &self.socket,
            self.rtcp,
            self.local_addr,
            config,
            None,
            prepared,
            self.discards,
        ))
    }

    /// Start carrying media with ICE driving the path (`docs/specs/ice.md` §2, §11).
    ///
    /// The `local` description must already have been given the peer's half through
    /// [`ice::LocalDescription::accept`]. If that returned `false` — the peer offered no candidates,
    /// or RFC 8839 §5.3's `ice-mismatch` applies — no agent is driven and this is
    /// [`Self::start`]: no check is sent, no timer runs, and the stream is carried by symmetric
    /// RTP exactly as it is today.
    ///
    /// # Errors
    ///
    /// Returns [`SetupError`] before starting ICE or media workers when session construction is
    /// invalid.
    pub fn start_with_ice(
        self,
        config: Config,
        local: ice::LocalDescription,
    ) -> Result<MediaSession, SetupError> {
        let prepared = Prepared::new(&config)?;
        if !local.running() {
            return Ok(MediaSession::on_socket(
                &self.socket,
                self.rtcp,
                self.local_addr,
                config,
                None,
                prepared,
                self.discards,
            ));
        }
        Ok(MediaSession::on_socket(
            &self.socket,
            self.rtcp,
            self.local_addr,
            config,
            Some(local),
            prepared,
            self.discards,
        ))
    }

    /// Start the fail-closed browser-audio runtime on this already-bound component.
    ///
    /// ICE begins first on the retained socket. Its selected component becomes the only DTLS
    /// peer; DTLS records pass through the component owner rather than a duplicated descriptor.
    /// Only a verified handshake installs SRTP/SRTCP keys and attaches the media workers.
    #[cfg(feature = "dtls")]
    #[allow(clippy::too_many_arguments)]
    pub async fn start_browser_audio(
        self,
        mut config: Config,
        local_ice: ice::LocalDescription,
        ice_generation: u64,
        identity: crate::dtls::openssl::Identity,
        role: crate::dtls::Role,
        peer_fingerprint: sipx_sdp::fingerprint::Fingerprint,
        timeout: Duration,
    ) -> Result<MediaSession, crate::browser::BrowserStartError> {
        if config.rtcp_mode != sipx_sdp::RtcpMode::Mux {
            return Err(crate::browser::BrowserStartError::RtcpMuxRequired);
        }
        let prepared = Prepared::new(&config)?;
        let stop = Arc::new(Stop::default());
        let (runtime, keys) = crate::browser::prepare(
            Arc::clone(&self.socket),
            local_ice,
            ice_generation,
            identity,
            role,
            peer_fingerprint,
            timeout,
            Arc::clone(&stop),
            Arc::clone(&self.discards),
        )
        .await?;
        let selected = crate::browser::lock_ingress(&runtime.ingress)
            .snapshot()
            .selected
            .ok_or(crate::browser::BrowserStartError::IceStopped)?;
        config.remote = selected.remote;
        config.srtp = Some(keys);
        Ok(MediaSession::on_browser(
            runtime,
            self.local_addr,
            config,
            prepared,
            self.discards,
        ))
    }
}

impl MediaSession {
    /// Bind a socket and start the session in one step.
    ///
    /// Only for callers that already know the far end — an answerer, which has the offer in
    /// hand. A caller making the offer needs [`MediaPort`] instead.
    ///
    /// # Errors
    ///
    /// Returns [`StartError::Setup`] before binding for invalid timing or codec construction,
    /// and [`StartError::Io`] if the media sockets cannot be bound.
    pub async fn start(bind: SocketAddr, config: Config) -> Result<Self, StartError> {
        // Validation and stateful codec setup happen before binding, so a rejected setup never
        // occupies even a temporary port and can never leave a worker behind.
        let prepared = Prepared::new(&config)?;
        let port = MediaPort::bind(bind).await?;
        Ok(Self::on_socket(
            &port.socket,
            port.rtcp,
            port.local_addr,
            config,
            None,
            prepared,
            port.discards,
        ))
    }

    // All fallible preparation precedes this constructor. Keeping worker creation contiguous is
    // what makes it reviewable that every worker shares the same sockets and stop token.
    #[allow(clippy::too_many_lines)]
    fn on_socket(
        socket: &Arc<UdpSocket>,
        rtcp: Option<Arc<UdpSocket>>,
        local_addr: SocketAddr,
        config: Config,
        ice: Option<ice::LocalDescription>,
        prepared: Prepared,
        discards: Arc<DiscardMeters>,
    ) -> Self {
        let srtp_profile = srtp_profile_of(&config);
        let samples_per_packet = config.samples_per_packet();
        let packet_duration = config.packet_duration;
        let clock_rate = config.clock_rate;
        let config_codec = config.codec;
        let wire_payload_type = config.wire_payload_type();
        let receive_payload_type = config.receive_wire_payload_type();
        let dtmf_payload_type = config.dtmf_payload_type;
        let rtcp_interval = config.rtcp_interval;
        let rtcp_mode = config.rtcp_mode;
        let encrypted = config.srtp.is_some();
        // A muxed session has exactly one running socket owner. The adjacent socket was reserved
        // before negotiation and is released now; no control worker may race the RTP reader.
        let rtcp = match rtcp_mode {
            sipx_sdp::RtcpMode::Separate => rtcp,
            sipx_sdp::RtcpMode::Mux => None,
        };
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Frame>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Vec<i16>>(256);
        let (encoded_tx, encoded_rx) = mpsc::channel::<Encoded>(256);
        let (digits_tx, digits_rx) = mpsc::channel::<(Digit, Duration)>(32);

        let shared = Shared::new(local_addr, discards);

        // Where to send. Starts at the SDP address and is replaced by the first observed
        // source: behind a NAT the advertised address is private and unreachable.
        let remote = Arc::new(Mutex::new(config.remote));

        // Taken before `config` is moved into the receive loop. Both control loops need them,
        // and cloning a pair of keys is cheaper than cloning the whole configuration twice.
        let srtp_keys = config.srtp.clone();

        // See `ice::driver::Destinations::rtcp`: `None` here, and for every stream without ICE,
        // leaves the report loop on RFC 3550 §11's convention.
        let rtcp_remote: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
        let (ice, ice_owner) = ice.map_or((None, None), |local| {
            let (handle, owner) = spawn_ice(
                local,
                socket,
                rtcp.as_ref(),
                &ice::driver::Destinations {
                    rtp: Arc::clone(&remote),
                    rtcp: Arc::clone(&rtcp_remote),
                },
                &shared.stop,
                &shared.discards,
            );
            (Some(handle), Some(owner))
        });

        let send_owner = tokio::spawn(send_loop(
            Arc::clone(socket),
            outgoing_rx,
            Sending {
                remote: Arc::clone(&remote),
                config: config.clone(),
                ssrc: shared.ssrc,
                sent: Arc::clone(&shared.sent),
                outbound: Arc::clone(&shared.outbound),
                muted: Arc::clone(&shared.muted),
                ice: ice.clone(),
                stop: Arc::clone(&shared.stop),
                encoding: prepared.encoding,
                discards: Arc::clone(&shared.discards),
                taps: Arc::clone(&shared.taps),
            },
        ));
        let (clips_tx, playback_owner) = spawn_playback_queue(&outgoing_tx, &shared.stop);
        let receive_owner = tokio::spawn(receive_loop(
            ReceiveInput::socket(Arc::clone(socket)),
            Inbound {
                audio: incoming_tx,
                encoded: encoded_tx,
                relay: Arc::clone(&shared.relay),
                digits: Keypresses {
                    to: digits_tx,
                    arrivals: Arc::clone(&shared.keypresses),
                },
                remote: Arc::clone(&remote),
                config,
                received: Arc::clone(&shared.received),
                stats: Arc::clone(&shared.stats),
                rtcp_observation: shared.rtcp_observation(clock_rate),
                ssrc: shared.ssrc,
                symmetric: ice.is_none(),
                ice: ice.clone(),
                browser_ingress: None,
                stop: Arc::clone(&shared.stop),
                decoding: prepared.decoding,
                discards: Arc::clone(&shared.discards),
                taps: Arc::clone(&shared.taps),
            },
        ));

        let rtcp_socket = rtcp.clone();
        let mut owners = vec![send_owner, playback_owner, receive_owner];
        owners.extend(spawn_control(Control {
            media: Arc::clone(socket),
            rtcp,
            remote: Arc::clone(&remote),
            rtcp_remote,
            interval: rtcp_interval,
            mode: rtcp_mode,
            ssrc: shared.ssrc,
            cname: shared.cname.clone(),
            stats: Arc::clone(&shared.stats),
            outbound: Arc::clone(&shared.outbound),
            rtcp_observation: shared.rtcp_observation(clock_rate),
            srtp: srtp_keys,
            ice: ice.clone(),
            stop: Arc::clone(&shared.stop),
            discards: Arc::clone(&shared.discards),
            #[cfg(feature = "dtls")]
            profile_tasks: None,
        }));
        if let Some(owner) = ice_owner {
            owners.push(owner);
        }

        Self {
            socket: Arc::clone(socket),
            rtcp_socket,
            ice,
            outgoing: outgoing_tx,
            digits: Mutex::new(digits_rx),
            tones: AtomicU64::new(0),
            incoming: Mutex::new(incoming_rx),
            encoded: Mutex::new(encoded_rx),
            relay: shared.relay,
            muted: shared.muted,
            clips: clips_tx,
            playbacks: AtomicU64::new(0),
            outstanding: Arc::new(AtomicUsize::new(0)),
            keypresses: shared.keypresses,
            codec: config_codec,
            wire_payload_type,
            receive_payload_type,
            dtmf_payload_type,
            rtcp_mode,
            encrypted,
            srtp_profile,
            local_addr,
            samples_per_packet,
            packet_duration,
            clock_rate,
            ssrc: shared.ssrc,
            quality_hook: shared.quality_hook,
            sent: shared.sent,
            received: shared.received,
            discards: shared.discards,
            stats: shared.stats,
            feedback: shared.feedback,
            browser_ingress: None,
            owners: Mutex::new(owners),
            retired: Mutex::new(Vec::new()),
            taps: shared.taps,
            #[cfg(all(test, feature = "dtls"))]
            browser_profile_tasks: None,
            #[cfg(all(test, feature = "dtls"))]
            browser_preparing_peak: None,
            stop: shared.stop,
        }
    }

    #[cfg(feature = "dtls")]
    #[allow(clippy::too_many_lines)]
    fn on_browser(
        runtime: crate::browser::Runtime,
        local_addr: SocketAddr,
        config: Config,
        prepared: Prepared,
        discards: Arc<DiscardMeters>,
    ) -> Self {
        let crate::browser::Runtime {
            socket,
            media,
            ice,
            ingress,
            owner,
            ice_owner,
            stop: runtime_stop,
            profile_tasks,
        } = runtime;
        let srtp_profile = srtp_profile_of(&config);
        let samples_per_packet = config.samples_per_packet();
        let packet_duration = config.packet_duration;
        let clock_rate = config.clock_rate;
        let config_codec = config.codec;
        let wire_payload_type = config.wire_payload_type();
        let receive_payload_type = config.receive_wire_payload_type();
        let dtmf_payload_type = config.dtmf_payload_type;
        let rtcp_mode = config.rtcp_mode;
        let encrypted = config.srtp.is_some();
        let rtcp_interval = config.rtcp_interval;
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Frame>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Vec<i16>>(256);
        let (encoded_tx, encoded_rx) = mpsc::channel::<Encoded>(256);
        let (digits_tx, digits_rx) = mpsc::channel::<(Digit, Duration)>(32);
        let shared = Shared::with_stop(local_addr, discards, runtime_stop);
        #[cfg(all(test, feature = "dtls"))]
        let preparing_peak = profile_tasks.counts().1;
        let remote = Arc::new(Mutex::new(config.remote));
        let srtp_keys = config.srtp.clone();
        let rtcp_remote: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
        let ice = Some(ice);

        let send_owner = tokio::spawn(crate::browser::profile_task(
            Arc::clone(&profile_tasks),
            send_loop(
                Arc::clone(&socket),
                outgoing_rx,
                Sending {
                    remote: Arc::clone(&remote),
                    config: config.clone(),
                    ssrc: shared.ssrc,
                    sent: Arc::clone(&shared.sent),
                    outbound: Arc::clone(&shared.outbound),
                    muted: Arc::clone(&shared.muted),
                    ice: ice.clone(),
                    stop: Arc::clone(&shared.stop),
                    encoding: prepared.encoding,
                    discards: Arc::clone(&shared.discards),
                    taps: Arc::clone(&shared.taps),
                },
            ),
        ));
        let (clips_tx, playback_owner) =
            spawn_browser_playback_queue(&outgoing_tx, &shared.stop, Arc::clone(&profile_tasks));
        let receive_owner = tokio::spawn(crate::browser::profile_task(
            Arc::clone(&profile_tasks),
            receive_loop(
                ReceiveInput::Browser(media),
                Inbound {
                    audio: incoming_tx,
                    encoded: encoded_tx,
                    relay: Arc::clone(&shared.relay),
                    digits: Keypresses {
                        to: digits_tx,
                        arrivals: Arc::clone(&shared.keypresses),
                    },
                    remote: Arc::clone(&remote),
                    config,
                    received: Arc::clone(&shared.received),
                    stats: Arc::clone(&shared.stats),
                    rtcp_observation: shared.rtcp_observation(clock_rate),
                    ssrc: shared.ssrc,
                    symmetric: false,
                    ice: ice.clone(),
                    browser_ingress: Some(Arc::clone(&ingress)),
                    stop: Arc::clone(&shared.stop),
                    decoding: prepared.decoding,
                    discards: Arc::clone(&shared.discards),
                    taps: Arc::clone(&shared.taps),
                },
            ),
        ));

        let mut owners = vec![owner, ice_owner, send_owner, playback_owner, receive_owner];
        owners.extend(spawn_control(Control {
            media: Arc::clone(&socket),
            rtcp: None,
            remote: Arc::clone(&remote),
            rtcp_remote,
            interval: rtcp_interval,
            mode: sipx_sdp::RtcpMode::Mux,
            ssrc: shared.ssrc,
            cname: shared.cname.clone(),
            stats: Arc::clone(&shared.stats),
            outbound: Arc::clone(&shared.outbound),
            rtcp_observation: shared.rtcp_observation(clock_rate),
            srtp: srtp_keys,
            ice: ice.clone(),
            stop: Arc::clone(&shared.stop),
            discards: Arc::clone(&shared.discards),
            #[cfg(feature = "dtls")]
            profile_tasks: Some(Arc::clone(&profile_tasks)),
        }));

        Self {
            socket,
            rtcp_socket: None,
            ice,
            browser_ingress: Some(ingress),
            owners: Mutex::new(owners),
            retired: Mutex::new(Vec::new()),
            taps: shared.taps,
            #[cfg(all(test, feature = "dtls"))]
            browser_profile_tasks: Some(profile_tasks),
            #[cfg(all(test, feature = "dtls"))]
            browser_preparing_peak: Some(preparing_peak),
            outgoing: outgoing_tx,
            digits: Mutex::new(digits_rx),
            tones: AtomicU64::new(0),
            incoming: Mutex::new(incoming_rx),
            encoded: Mutex::new(encoded_rx),
            relay: shared.relay,
            muted: shared.muted,
            clips: clips_tx,
            playbacks: AtomicU64::new(0),
            outstanding: Arc::new(AtomicUsize::new(0)),
            keypresses: shared.keypresses,
            codec: config_codec,
            wire_payload_type,
            receive_payload_type,
            dtmf_payload_type,
            rtcp_mode,
            encrypted,
            srtp_profile,
            local_addr,
            samples_per_packet,
            packet_duration,
            clock_rate,
            ssrc: shared.ssrc,
            quality_hook: shared.quality_hook,
            sent: shared.sent,
            received: shared.received,
            discards: shared.discards,
            stats: shared.stats,
            feedback: shared.feedback,
            stop: shared.stop,
        }
    }

    /// The address media arrives on, for the SDP.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// This session generation's local RTP synchronisation source.
    #[must_use]
    pub fn local_ssrc(&self) -> u32 {
        self.ssrc
    }

    /// Install or clear the application callback for peer RTCP quality reports.
    ///
    /// The slot is shared by this session's RTP/RTCP workers. Registering a callback does not
    /// enable RTCP when [`Config::rtcp_interval`] is `None`.
    pub fn set_rtcp_quality_hook(&self, hook: Option<RtcpQualityHook>) {
        replace_quality_hook(&self.quality_hook, hook);
    }

    /// The callback currently installed for peer RTCP quality reports.
    #[must_use]
    pub fn rtcp_quality_hook(&self) -> Option<RtcpQualityHook> {
        current_quality_hook(&self.quality_hook)
    }

    /// Whether ICE is driving this stream's path.
    ///
    /// The signalling layer's question before it builds any later description: RFC 8839 §4.4 makes
    /// the ICE attributes mandatory on every subsequent offer and answer for a stream doing ICE,
    /// and §6 makes their *absence* mean the peer has stopped. A session carrying no agent must
    /// therefore not grow ICE attributes on a re-offer, and one carrying an agent must not lose
    /// them.
    #[must_use]
    pub fn runs_ice(&self) -> bool {
        self.ice.is_some()
    }

    /// The candidate path ICE actually selected for RTP.
    ///
    /// `Checking` is honest intermediate state: an ICE exchange was negotiated, but no nominated
    /// pair has replaced the default destination yet. A terminal diagnostic can therefore report
    /// what happened without inferring it from the policy that was requested.
    #[must_use]
    pub fn ice_path(&self) -> crate::ice::IcePath {
        self.ice.as_ref().map_or(
            crate::ice::IcePath::Disabled,
            crate::ice::driver::Handle::path,
        )
    }

    /// Security and nominated-pair facts for a browser-audio component.
    #[must_use]
    pub fn browser_component(&self) -> Option<crate::browser::BrowserComponentSnapshot> {
        self.browser_ingress
            .as_ref()
            .map(|ingress| crate::browser::lock_ingress(ingress).snapshot())
    }

    #[cfg(all(test, feature = "dtls"))]
    pub(crate) fn browser_task_counts(&self) -> Option<(usize, usize, usize)> {
        self.browser_profile_tasks.as_ref().and_then(|tasks| {
            self.browser_preparing_peak.map(|preparing_peak| {
                let (active, peak) = tasks.counts();
                (preparing_peak, active, peak)
            })
        })
    }

    #[cfg(all(test, feature = "dtls"))]
    pub(crate) fn browser_task_probe(&self) -> Option<Arc<crate::browser::ProfileTasks>> {
        self.browser_profile_tasks.clone()
    }

    /// Rebuild codec and packet workers on this session's existing sockets.
    ///
    /// Used when a later SDP exchange changes the remote address, codec, payload type, or keys.
    /// The local RTP/RTCP addresses do not change: they are already published to the peer, and a
    /// replacement that rebound an ephemeral port would make the new description false. Mute and
    /// encoded-relay policy survive the transition. The stopped generation remains owned until
    /// all of its workers have joined; if this future is cancelled during that join, the next
    /// reconfiguration or shutdown resumes the cleanup.
    ///
    /// Returns `false` without changing the session when ICE owns the destinations. Rebuilding an
    /// ICE-backed session requires the agent and its selected pair to move with the workers; a
    /// caller must refuse that renegotiation rather than silently fall back to an unchecked path.
    ///
    /// # Errors
    ///
    /// Returns [`SetupError`] before stopping the current workers if the new timing or codec
    /// cannot be constructed.
    pub async fn reconfigure(&mut self, config: Config) -> Result<bool, SetupError> {
        if self.ice.is_some() {
            return Ok(false);
        }
        let prepared = Prepared::new(&config)?;
        self.reap_retired().await;
        let muted = self.is_muted();
        let relay = self.relay.load(Ordering::SeqCst);
        let quality_hook = self.rtcp_quality_hook();
        let socket = Arc::clone(&self.socket);
        let rtcp = self.rtcp_socket.clone();
        let local_addr = self.local_addr;

        self.stop.stop();
        let replacement = Self::on_socket(
            &socket,
            rtcp,
            local_addr,
            config,
            None,
            prepared,
            Arc::clone(&self.discards),
        );
        replacement.set_muted(muted);
        replacement.set_relay(relay);
        replacement.set_rtcp_quality_hook(quality_hook);
        // Attachments belong to the call, not to a worker generation, so a re-INVITE must not make
        // an application re-attach. Carried before the swap: the retired generation's `Drop` closes
        // whatever its registry still holds, and it must find nothing.
        replacement.taps.adopt(&self.taps);
        let previous = std::mem::replace(self, replacement);
        self.retired.get_mut().push(previous);
        self.reap_retired().await;
        Ok(true)
    }

    /// Apply a later exchange's ICE half, and read back what this side must now signal
    /// (RFC 8839 §4.4; `docs/specs/ice.md` §13.5).
    ///
    /// `local` carries fresh credentials and a fresh tiebreaker when this exchange is a restart —
    /// §4.4.1.1.1 says a new ICE session, and the answer to one names the answerer's *own* new
    /// credentials rather than the ones the finished session keyed its checks with. `None` is
    /// every ordinary re-offer: hold, resume, a codec change, a session refresh. `peer` is the
    /// description that arrived, when one has.
    ///
    /// Returns `None` when this stream is not running ICE, or when the driver has already stopped.
    /// Both mean the same thing to a caller: answer without ICE attributes rather than block on a
    /// session that is ending.
    ///
    /// Whether the peer's half *is* a restart is deliberately not asked here. That is
    /// §4.4.1.1.1's question about the peer's two credentials, the agent has always answered it,
    /// and a second implementation of it on this side would be a second thing to keep right.
    pub async fn renegotiate_ice(
        &self,
        local: Option<(sipx_sdp::ice::Credentials, u64)>,
        peer: Option<&ice::Negotiation>,
    ) -> Option<ice::Local> {
        let handle = self.ice.as_ref()?;
        let peer = match peer {
            Some(ice::Negotiation::Ice {
                credentials,
                candidates,
                lite,
            }) => Some(crate::ice::driver::Peer {
                credentials: credentials.clone(),
                candidates: candidates.clone(),
                lite: *lite,
            }),
            // `Absent` and `Mismatch` alike: RFC 8839 §5.3 says ICE MUST NOT be used for a
            // mismatched stream, and §6 says no candidates means no ICE. Neither is a description
            // to feed an agent — but this side still re-signals its own half, because the running
            // session is what the peer is sending media to.
            _ => None,
        };
        handle.renegotiated(local, peer).await
    }

    /// Queue one packet's worth of samples.
    ///
    /// Queued rather than sent: the pacing timer decides when it goes out.
    pub async fn send(&self, samples: Vec<i16>) -> bool {
        self.outgoing
            .send(Frame::Audio {
                samples,
                playback: None,
            })
            .await
            .is_ok()
    }

    /// Send a DTMF digit, held for `duration`.
    ///
    /// The packets go through the same paced queue as audio, so the tone occupies the slots
    /// audio would have. That is deliberate: RFC 4733 events replace the audio for their
    /// duration rather than being sent alongside it, and sending both means the far end hears
    /// the keypress twice.
    pub async fn send_digit(&self, digit: Digit, duration: Duration) -> bool {
        // RFC 4733 event durations run on the RTP clock, not the audio rate — for G.722 a
        // 20 ms packet is 320 audio samples but only 160 clock units, and a tone timed in
        // samples would claim twice its real duration.
        let per_packet = self.clock_units_per_packet();
        let packets = (duration.as_millis() / self.packet_duration.as_millis().max(1)).max(1);
        let events = dtmf::tone(
            digit,
            usize::try_from(packets).unwrap_or(1),
            u16::try_from(per_packet).unwrap_or(160),
        );
        let tone = self.tones.fetch_add(1, Ordering::Relaxed);
        for packet in events {
            if self
                .outgoing
                .send(Frame::Dtmf {
                    event: packet.event,
                    offset: packet.segment_offset,
                    tone,
                })
                .await
                .is_err()
            {
                return false;
            }
        }
        true
    }

    /// Take the next DTMF digit the far end pressed, and how long it was held.
    ///
    /// The duration comes from the RFC 4733 event itself (its `duration` field, converted from
    /// the negotiated clock rate to wall-clock time), not from timing our own arrival: the event
    /// carries the sender's own clock, and measuring anything else would make the number depend
    /// on jitter rather than on how long the key was actually down.
    pub async fn recv_digit(&self) -> Option<(Digit, Duration)> {
        self.digits.lock().await.recv().await
    }

    /// Collect the digits the far end presses, for at most `within`, stopping once it has been
    /// quiet for `gap`.
    ///
    /// Two questions, two bounds — the same split [`Self::record_at_least`] made on the audio
    /// path, and the reason this takes two durations rather than one.
    ///
    /// `within` bounds the wait for the **first** digit, and with it the whole collection. It is a
    /// **bound on failure**: how long this side is prepared to wait before concluding no digits
    /// are coming, so it belongs an order of magnitude above the honest answer — a whole call's
    /// worth, typically — rather than close to it. Nothing about it is a measurement: how long a
    /// caller takes to press the first key is a property of the caller, and how long the keypress
    /// takes to get here is a property of the machines between them.
    ///
    /// `gap` is a **definition of silence**: how long a caller has to leave a hole for the
    /// dialling to be treated as finished. It is the only question a fixed window can answer here,
    /// and it can only be asked once a digit has arrived, because a caller who has not dialled is
    /// not a caller who has stopped dialling.
    ///
    /// Whatever was collected is returned, including nothing. A collection cut short by `within`
    /// keeps the digits it already has.
    ///
    /// # Inferring the end of the dialling (`M-34`)
    ///
    /// RFC 4733 carries keypresses, not a completion signal: there is no "the caller is done"
    /// event to wait for, so *the digits ended* is always this side's inference from silence, and
    /// `gap` is the whole of that inference. What makes it safe to draw is that the input it draws
    /// on is exact rather than approximate. A digit is delivered here **once**, when the first
    /// packet carrying that tone's end bit arrives; the tone is identified by its own RTP
    /// timestamp, which is constant across every packet of the tone, so the end retransmissions
    /// RFC 4733 §2.5.1.3 asks for are absorbed rather than counted again, and "44" is told from a
    /// single long "4" by the timestamp changing. So a `gap` that elapses means no *keypress*
    /// completed in it — never that a packet was missed mid-tone.
    ///
    /// A digit that arrives a millisecond after `gap` expires is **not lost** — up to the 32 the
    /// keypress channel holds, past which the receive loop drops rather than blocks, deliberately
    /// and by the same reasoning as every other queue here. Within that bound it stays queued and
    /// is the first digit the next [`Self::recv_digit`] or `collect_digits` yields. It is in the
    /// wrong collection, though, and no wall clock can fix that — which is why `gap` is set past
    /// any plausible scheduling delay rather than close to the spacing digits actually arrive
    /// with, and why an application that knows how many digits it wants should stop at that count
    /// with [`Self::recv_digit`] instead of waiting for a silence at all.
    ///
    /// # Why this takes two durations (`M-34`)
    ///
    /// It used to take one, spent on both questions, and that is the defect `X-40` measured one
    /// layer up: a single window covering both "has it started" and "has it ended" is beaten by
    /// whichever of the two is slower on the day, and the result is not a degraded collection but
    /// an **empty** one, since the loop ends before its first iteration. `sipx answer` produced a
    /// valid recording of zero samples that way. Widening the single window would have moved that
    /// cliff rather than removed it, and left the same defect for a slower caller.
    pub async fn collect_digits(&self, within: Duration, gap: Duration) -> String {
        let deadline = tokio::time::Instant::now() + within;
        let mut out = String::new();

        // The first digit. Nothing has been pressed yet, so there is no silence to interpret —
        // only the caller's own bound on how long to wait for dialling that may never start.
        match tokio::time::timeout_at(deadline, self.recv_digit()).await {
            Ok(Some((digit, _held))) => out.push(digit.as_char()),
            // The session ended, or the bound elapsed. Either way nobody dialled.
            Ok(None) | Err(_) => return out,
        }

        // The rest of the sequence. A gap now does mean the dialling has finished, and `within`
        // still caps a far end that keeps pressing keys forever.
        loop {
            let next = tokio::time::Instant::now() + gap;
            match tokio::time::timeout_at(next.min(deadline), self.recv_digit()).await {
                Ok(Some((digit, _held))) => out.push(digit.as_char()),
                // The caller stopped, the session ended, or the collection's time is up. All
                // three mean this is every digit there is — and it is kept.
                Ok(None) | Err(_) => return out,
            }
        }
    }

    /// The codec this session negotiated.
    #[must_use]
    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// The payload type this negotiated stream puts on the wire.
    ///
    /// Static codecs usually return their assigned number. Dynamic codecs return the number
    /// from the negotiated description, which need not be the number this endpoint prefers in
    /// an offer.
    #[must_use]
    pub fn wire_payload_type(&self) -> u8 {
        self.wire_payload_type
    }

    /// The payload type this negotiated stream accepts from the wire.
    #[must_use]
    pub fn receive_payload_type(&self) -> u8 {
        self.receive_payload_type
    }

    /// The negotiated RTP payload type for telephone events, when enabled.
    #[must_use]
    pub fn dtmf_payload_type(&self) -> Option<u8> {
        self.dtmf_payload_type
    }

    /// Whether RTP and RTCP share one socket for this session.
    #[must_use]
    pub fn rtcp_mode(&self) -> sipx_sdp::RtcpMode {
        self.rtcp_mode
    }

    /// Whether this session was constructed with SRTP key material.
    ///
    /// The key bytes remain owned by the workers and are never exposed by this fact.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// The SRTP protection profile this session negotiated, if it is encrypted.
    ///
    /// [`Self::is_encrypted`] answers *whether* the media is protected; this answers *by what*,
    /// which since `M-41` is a question with three answers rather than one. Worth being able to ask
    /// without a packet capture, for the same reason: a call protected by the interoperability
    /// floor and one protected by AES-256-GCM look identical from the outside, and an operator who
    /// negotiated a policy has no other way to see whether it held.
    #[must_use]
    pub fn srtp_profile(&self) -> Option<sipx_rtp::srtp::Profile> {
        self.srtp_profile
    }

    /// The RTP timestamp clock negotiated for this stream.
    ///
    /// This is intentionally the wire clock rather than an inferred playback rate. In
    /// particular, RFC 7587 fixes Opus at 48 kHz on the RTP timeline, and RFC 3551 §4.5.2
    /// fixes G.722's timeline at 8000 while its audio is 16 kHz — a caller that wants the
    /// rate of the *samples* wants [`Self::audio_rate`].
    #[must_use]
    pub fn clock_rate(&self) -> u32 {
        self.clock_rate
    }

    /// The audio sampling rate of the samples this session consumes and produces.
    ///
    /// Equal to [`Self::clock_rate`] for every codec except G.722, whose audio runs at twice
    /// its RTP clock (RFC 3551 §4.5.2). This is the rate for PCM conversion, resampling and
    /// WAV headers; the clock rate is for timestamps and jitter.
    #[must_use]
    pub fn audio_rate(&self) -> u32 {
        self.clock_rate
            .saturating_mul(self.codec.samples_per_clock_unit())
    }

    /// How many RTP timestamp units one packet advances the clock by.
    fn clock_units_per_packet(&self) -> usize {
        self.samples_per_packet
            / usize::try_from(self.codec.samples_per_clock_unit().max(1)).unwrap_or(1)
    }

    /// Hand received packets on still encoded, rather than decoding them to samples.
    ///
    /// Switchable at run time because a bridge is formed between calls that are already
    /// running: the decision belongs to whoever connects them, and it is not known when the
    /// session starts.
    pub fn set_relay(&self, relay: bool) {
        self.relay.store(relay, Ordering::SeqCst);
    }

    /// Gate this session's outbound audio, or let it through again (`M-18`).
    ///
    /// Returns what the gate was set to before, so a caller that only wants to report real
    /// transitions does not have to read the flag and then write it — two steps that race each
    /// other when a call is muted from more than one place.
    ///
    /// **What muting does to the stream.** Every audio frame the send loop takes off the queue is
    /// replaced by the same number of samples of silence, encoded in this session's own codec and
    /// sent on its own payload type. The stream keeps its pacing, its sequence numbers and its
    /// timestamps; what changes is only what the far end decodes. See the module documentation
    /// for why this rather than suppressing the packets, and for the RFC 3550 §6 consequence.
    ///
    /// Reception is not affected in any way, and neither is DTMF: an RFC 4733 event is generated
    /// by this endpoint on purpose, the way a keypad tone is on a handset, so it goes out muted
    /// or not.
    pub fn set_muted(&self, muted: bool) -> bool {
        self.muted.swap(muted, Ordering::SeqCst)
    }

    /// Whether this session's outbound audio is gated to silence.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::SeqCst)
    }

    /// Take the next packet as it arrived, still encoded. Only ever yields under
    /// [`Self::set_relay`].
    pub async fn recv_encoded(&self) -> Option<Encoded> {
        self.encoded.lock().await.recv().await
    }

    /// Put a payload on the wire exactly as given, bypassing the codec.
    pub async fn send_encoded(&self, encoded: Encoded) -> bool {
        self.outgoing
            .send(Frame::Encoded {
                payload_type: encoded.payload_type,
                payload: encoded.payload,
            })
            .await
            .is_ok()
    }

    /// Take the next packet's worth of received samples.
    pub async fn recv(&self) -> Option<Vec<i16>> {
        self.incoming.lock().await.recv().await
    }

    /// Receive this session as linear PCM at an application-chosen rate and depth.
    ///
    /// # Errors
    ///
    /// Returns [`sipx_audio::PcmError::UnsupportedSampleRate`] when the requested format's rate
    /// cannot be converted safely.
    pub fn capture(
        &self,
        format: sipx_audio::PcmFormat,
    ) -> Result<PcmCapture<'_>, sipx_audio::PcmError> {
        Ok(PcmCapture {
            session: self,
            format,
            // The received samples are at the *audio* rate — for G.722 that is 16 kHz, not
            // the 8000 the RTP clock says (RFC 3551 §4.5.2).
            resampler: sipx_audio::LinearResampler::new(self.audio_rate(), format.sample_rate())?,
        })
    }

    /// Take received samples until the session goes quiet for `idle`.
    ///
    /// The idle timeout rather than a packet count, because the caller generally knows how long
    /// the far end will talk for and not how many packets that becomes.
    ///
    /// `idle` answers one question — *has the far end stopped talking* — and it answers it by
    /// wall clock. A caller that already knows how many samples it expects is asking a different
    /// question and wants [`Self::record_at_least`]; see there for what goes wrong when the two
    /// are confused (`X-28`).
    pub async fn record_until_idle(&self, idle: Duration) -> Vec<i16> {
        let mut samples = Vec::new();
        // `idle` is a definition of silence, and on the first pass it is also the deadline for the
        // stream to start — which is the trap the documentation above is about, kept deliberately
        // rather than hidden. The remedy is not a wider window here; it is [`Self::record_at_least`]
        // for every caller that knows the count, which is nearly all of them (`X-28`, `X-44`).
        while let Ok(Some(frame)) = tokio::time::timeout(idle, self.recv()).await {
            samples.extend_from_slice(&frame);
        }
        samples
    }

    /// Take received samples until `samples` of them have arrived, or `within` elapses.
    ///
    /// The wait for a caller that knows the size of what the far end was given — a test that
    /// played a clip of its own, most often. `within` is a **bound on failure**, not a
    /// measurement: it is how long this side is prepared to wait before concluding the audio is
    /// not coming, so it should be far longer than the clip rather than close to it. Whatever
    /// arrived is returned, so a caller that got fewer samples than it asked for can say so
    /// itself.
    ///
    /// # Why this exists (`X-28`)
    ///
    /// [`Self::record_until_idle`] spends one duration on two different jobs: how long to wait
    /// for the stream to *start*, and how long a gap means it has *ended*. Neither is a property
    /// of the audio — both are properties of how fast the machine happens to be — so a caller
    /// that knows the count and uses the idle window instead is racing a fixed wall clock
    /// against a pipeline that is merely slow. On a loaded machine that pipeline is slow in
    /// exactly the two places the single window covers: the first packet is the one that waits
    /// out both jitter buffers filling, and a stalled scheduler opens mid-stream gaps wider than
    /// any packet interval. The observed result is a recording of **zero** samples — not a
    /// degraded one — because once the first frame lands the rest follow at the packet rate.
    ///
    /// Widening the window would not have fixed that; it would have moved the cliff.
    pub async fn record_at_least(&self, samples: usize, within: Duration) -> Vec<i16> {
        let deadline = tokio::time::Instant::now() + within;
        let mut recorded = Vec::with_capacity(samples);
        while recorded.len() < samples {
            match tokio::time::timeout_at(deadline, self.recv()).await {
                Ok(Some(frame)) => recorded.extend_from_slice(&frame),
                // The session ended, or the bound elapsed. Either way this is everything there
                // is, and it is short — which is the caller's to report, not this method's.
                Ok(None) | Err(_) => break,
            }
        }
        recorded
    }

    /// How many samples one packet of this session's audio carries.
    ///
    /// Settled once when the session started, from the negotiated codec's clock rate and the
    /// packet duration. Exposed so a caller playing a clip does not have to recompute what the
    /// session already decided — and get it wrong for a codec whose rate is not 8 kHz.
    #[must_use]
    pub fn samples_per_packet(&self) -> usize {
        self.samples_per_packet
    }

    /// Send a whole clip, paced by the send loop, and wait for it.
    ///
    /// Returns whether the clip reached the end. `false` means it did not: the send queue closed
    /// part way — the call ended, or the session was stopped, under a playback still running — or
    /// something cut it short. The caller needs to be able to tell those apart: "the clip
    /// finished" and "the clip was cut off" are different things to anything waiting on the
    /// playback, and returning `()` made them indistinguishable.
    ///
    /// This is [`Self::start_playback`] with the handle thrown away and the answer awaited
    /// through [`Playback::play_out`], so it stays cancel-on-drop: a caller that wraps it in a
    /// `timeout` still stops the audio when the timeout fires. A caller that wants to stop the
    /// clip explicitly, or to have a keypress stop it, needs the handle.
    pub async fn play(&self, samples: &[i16], samples_per_packet: usize) -> bool {
        self.start_clip(samples.to_vec(), samples_per_packet, Interrupt::Never)
            .play_out()
            .await
            .completed()
    }

    /// Convert and play an explicit linear-PCM buffer.
    ///
    /// # Errors
    ///
    /// Returns [`sipx_audio::PcmError`] before queuing anything when its rate or representation
    /// cannot be converted.
    pub async fn play_pcm(&self, pcm: &sipx_audio::Pcm) -> Result<bool, sipx_audio::PcmError> {
        Ok(self
            .start_pcm_playback(pcm, Interrupt::Never)?
            .play_out()
            .await
            .completed())
    }

    /// Convert an explicit PCM buffer and start it as a controllable playback.
    ///
    /// # Errors
    ///
    /// Returns [`sipx_audio::PcmError`] before creating a playback when conversion is refused.
    pub fn start_pcm_playback(
        &self,
        pcm: &sipx_audio::Pcm,
        interrupt: Interrupt,
    ) -> Result<Playback, sipx_audio::PcmError> {
        // Converted to the *audio* rate: a G.722 session plays 16 kHz samples even though its
        // RTP clock is 8000 (RFC 3551 §4.5.2).
        let samples = pcm.to_i16(self.audio_rate())?;
        Ok(self.start_playback(samples, interrupt))
    }

    /// Start a clip and hand back a handle to it, without waiting (`M-17`).
    ///
    /// The clip is played at this session's own packet size, so it is right under a codec whose
    /// clock is not 8 kHz without the caller knowing the rate.
    ///
    /// # Clips queue; they do not replace
    ///
    /// Starting a second playback while one is running puts it **behind** the one playing, and it
    /// begins when that one ends — however that one ends. This is the choice the story left open,
    /// and it is recorded in [`docs/designs/app-sdk.md`](../../../docs/designs/app-sdk.md). The
    /// reasoning in short: replacement would make "stop" an implicit side effect of "play", so an
    /// application that wanted a prompt followed by a menu would hear only the menu, and the first
    /// clip's cancellation would be an event nobody asked for. Replacement is still available and
    /// still says what it means — [`Playback::stop`] the one playing, then start the next.
    ///
    /// Queueing while a clip is stopping is the case worth naming, because it is what barge-in
    /// does: stop the prompt, then immediately play something else. The clip being stopped
    /// releases the queue at once and its unsent packets are discarded rather than played, so the
    /// new clip starts within [`Playback::STOP_BOUND_PACKETS`] packets — it does not have to wait
    /// out the backlog of the clip it replaced.
    ///
    /// A queue [`Playback::QUEUE_DEPTH`] deep. A clip that arrives at a full queue is not played and its
    /// handle resolves immediately as [`PlaybackEnd::Refused`], rather than being silently
    /// dropped or waiting for room that a live call may never have.
    pub fn start_playback(&self, samples: Vec<i16>, interrupt: Interrupt) -> Playback {
        self.start_clip(samples, self.samples_per_packet, interrupt)
    }

    /// Queue a clip at an explicit packet size.
    ///
    /// Separate from [`Self::start_playback`] only because [`Self::play`] takes the size from its
    /// caller and has done since before this session knew its own.
    fn start_clip(
        &self,
        samples: Vec<i16>,
        samples_per_packet: usize,
        interrupt: Interrupt,
    ) -> Playback {
        let id = PlaybackId(self.playbacks.fetch_add(1, Ordering::Relaxed));
        let stop = Arc::new(Stop::default());
        let (end_tx, end_rx) = watch::channel(None);
        let playback = Playback {
            id,
            stop: Arc::clone(&stop),
            end: end_rx,
        };

        // Counted before the hand-off, so a `flush` racing this call never sees a queue it
        // believes is empty. The clip's destructor is what takes it back down again, on every
        // path out including the two below.
        self.outstanding.fetch_add(1, Ordering::SeqCst);
        let clip = Clip {
            samples,
            samples_per_packet: samples_per_packet.max(1),
            interrupt,
            stop,
            end: end_tx,
            keypresses: self.keypresses.subscribe(),
            outstanding: Arc::clone(&self.outstanding),
            discards: Arc::clone(&self.discards),
        };

        // `try_send` rather than an await, so starting a playback is not itself something that
        // can park — a handle the caller cannot yet hold is a handle it cannot stop.
        match self.clips.try_send(clip) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(clip)) => clip.finish(PlaybackEnd::Refused),
            Err(mpsc::error::TrySendError::Closed(clip)) => clip.finish(PlaybackEnd::SessionEnded),
        }
        playback
    }

    /// How many packets have been sent.
    #[must_use]
    pub fn packets_sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// How many have been received.
    #[must_use]
    pub fn packets_received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// A synchronous snapshot of everything this session's media path has discarded.
    ///
    /// This includes candidate-gathering losses on the [`MediaPort`] the session consumed.
    /// Each field is monotonic, but independent workers can advance different fields while this
    /// snapshot is read, so relationships across fields are exact only while the session is quiet.
    #[must_use]
    pub fn discard_counts(&self) -> MediaDiscardCounts {
        self.discards.snapshot()
    }

    /// How the call is going: loss, jitter, round-trip time and an estimated score.
    ///
    /// Readable at any point, not only at the end. The round-trip time is `None` until a report
    /// has come back from the far end carrying an echo of one of ours — which needs both a
    /// control port on this side and a peer that answers, so it stays `None` against a peer
    /// that does not do RTCP rather than being filled in with a guess.
    pub async fn quality(&self) -> sipx_rtp::Quality {
        // The counters directly, never `report_block()`: that one closes the reporting interval
        // (RFC 3550 §6.4.1), so an application polling quality every second would quietly empty
        // the window the next RTCP report was going to describe, and the far end would be told
        // the call was clean. `pending_report_block()` would be safe now, but the whole-call
        // figures below are not in a report block at all.
        let (lost, expected, jitter_units) = {
            let stats = self.stats.lock().await;
            (stats.cumulative_lost(), stats.expected(), stats.jitter())
        };

        // Loss over the whole call, not since the last report. The per-report fraction is the
        // right number to *send* and the wrong one to *show*: it swings between 0 and 1 with
        // each interval, and an application sampling it sees whichever interval it happened to
        // catch rather than how the call has gone.
        // `f64` throughout: a packet count large enough to lose precision here is a call that
        // has been running for tens of thousands of years.
        let fraction = if expected > 0 {
            let (missing, due) = (
                u32::try_from(lost.max(0)).map_or(f64::from(u32::MAX), f64::from),
                u32::try_from(expected).map_or(f64::from(u32::MAX), f64::from),
            );
            missing / due
        } else {
            0.0
        };
        // The jitter field is in timestamp units; turning it into a duration needs the clock
        // rate, which is the one number that makes it comparable between codecs.
        let jitter = Duration::from_secs_f64(
            (f64::from(jitter_units) / f64::from(self.clock_rate)).max(0.0),
        );
        let round_trip = self.feedback.lock().await.round_trip;

        sipx_rtp::Quality {
            loss: fraction,
            cumulative_lost: lost,
            jitter,
            round_trip,
            mos: sipx_rtp::Quality::mos(fraction, jitter, round_trip),
        }
    }

    /// The receiver report this session would send right now (RFC 3550 §6.4.1).
    ///
    /// **Safe to poll**, as often as a dashboard likes: reading does not close the reporting
    /// interval, so it cannot make the *next* RTCP report claim a clean interval that was in fact
    /// lossy. That is a decision and not an accident (`M-33`) — §6.4.1 defines `fraction_lost` as
    /// loss since the previous SR or RR *packet*, so the interval boundary is a report having been
    /// sent, and a read is not one. The RTCP loop closes it, via
    /// [`StreamStats::report_block`](sipx_rtp::rtcp::StreamStats::report_block); this reads with
    /// [`pending_report_block`](sipx_rtp::rtcp::StreamStats::pending_report_block).
    ///
    /// `fraction_lost` is therefore whatever has accumulated since the last report went out, which
    /// makes it a poor thing to *display*: it swings with each interval, and a poller sees whichever
    /// interval it happened to catch. [`Self::quality`] is the figure for a caller to show.
    ///
    /// The two echo fields are zero here. They are filled in by the sending loop, which is the only
    /// place that knows how long a peer's sender report has been held.
    pub async fn stats(&self) -> sipx_rtp::rtcp::ReportBlock {
        self.stats.lock().await.pending_report_block()
    }

    /// Wait until everything queued has actually been sent.
    ///
    /// Sending is paced, so `play` and `send_digit` return as soon as the packets are queued —
    /// which is long before they are on the wire. Hanging up at that point discards the tail:
    /// the last word of a clip, or the last digit of a PIN. Anything still queued after
    /// `within` is given up on, so this cannot hold a caller open indefinitely.
    pub async fn flush(&self, within: Duration) {
        let deadline = tokio::time::Instant::now() + within;
        // Both queues: a clip started but not yet fed to the send loop has nothing in the send
        // queue to see, and a flush that only looked there would hang up over the top of it.
        while self.outstanding.load(Ordering::SeqCst) > 0
            || self.outgoing.capacity() < self.outgoing.max_capacity()
        {
            if tokio::time::Instant::now() >= deadline || self.stop.is_stopped() {
                return;
            }
            tokio::time::sleep(self.packet_duration.max(Duration::from_millis(5))).await;
        }
        // The last packet has left the queue but not yet the socket.
        tokio::time::sleep(self.packet_duration).await;
    }

    /// Attach a bounded PCM processor to one direction of this call.
    ///
    /// The one call-audio tap (`M-54`, `docs/specs/call-audio-seam.md`): local speech and
    /// deterministic call-audio analysis both ride it rather than adding a second. The returned
    /// handle is the sole consumer of its own bounded queue, converts to the requested format with
    /// the shared linear-PCM resampler (`M-43`), and detaches when it is dropped.
    ///
    /// A processor that stops reading loses its own oldest frames under the seam's documented loss
    /// policy and is told so by a discontinuity on the next frame it receives. It can never delay
    /// RTP decode, RTP encode, playback or capture.
    ///
    /// Attachments survive a [`Self::reconfigure`]; the first frame of the new generation carries a
    /// [`crate::processing::DiscontinuityKind::Realign`].
    ///
    /// # Errors
    ///
    /// Returns [`ProcessingError::UnsupportedConversion`] when the requested format is outside the
    /// linear-PCM boundary, [`ProcessingError::QueueCapacity`] for a queue depth outside its
    /// domain, [`ProcessingError::TooManyProcessors`] at the per-session ceiling, and
    /// [`ProcessingError::SessionStopped`] once the session has stopped. Every refusal leaves the
    /// call exactly as it was.
    pub fn attach_processor(&self, request: Processing) -> Result<PcmProcessor, ProcessingError> {
        if self.is_stopped() {
            return Err(ProcessingError::SessionStopped);
        }
        self.taps.attach(request, self.audio_rate())
    }

    /// Stop the session and release its socket.
    pub fn stop(&self) {
        self.stop.stop();
        self.taps.close();
    }

    /// Stop and join every worker owned by this session.
    ///
    /// Handles stay in the registry until their await completes. Cancelling this future therefore
    /// leaves the current handle owned, and a later call resumes the same drain.
    pub async fn shutdown(&self) {
        self.stop.stop();
        self.taps.close();
        if let Some(ingress) = &self.browser_ingress {
            crate::browser::lock_ingress(ingress).close();
        }
        let mut owners = self.owners.lock().await;
        while let Some(owner) = owners.last_mut() {
            // discard: every worker shares the observed stop token; awaiting proves it is reaped,
            // while a cancellation JoinError adds no packet-level discard to count.
            let _ = owner.await;
            owners.pop();
        }
        drop(owners);
        self.reap_retired().await;
    }

    async fn reap_retired(&self) {
        let mut retired = self.retired.lock().await;
        while let Some(previous) = retired.last() {
            Box::pin(previous.shutdown()).await;
            retired.pop();
        }
    }

    #[cfg(test)]
    async fn owned_task_count(&self) -> usize {
        self.owners.lock().await.len() + self.retired.lock().await.len()
    }

    /// Whether the session has been stopped.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stop.is_stopped()
    }
}

impl Drop for MediaSession {
    fn drop(&mut self) {
        // A session that outlives its call keeps a socket and two tasks alive. On a server
        // taking calls all day that is the difference between steady and unbounded.
        self.stop.stop();
        // Attachments complete rather than idle: a consumer parked on `recv` learns the call is
        // over from the seam instead of from a timeout. A retired generation's registry is already
        // empty, because `reconfigure` moved it to the replacement.
        self.taps.close();
        if let Some(ingress) = &self.browser_ingress {
            crate::browser::lock_ingress(ingress).close();
        }
        for owner in self.owners.get_mut().drain(..) {
            owner.abort();
        }
    }
}

/// Where a received packet goes, gathered so the receive loop reads as a loop.
fn delivery<'a>(
    audio: &'a mpsc::Sender<Vec<i16>>,
    encoded: &'a mpsc::Sender<Encoded>,
    relay: &'a AtomicBool,
    discards: &'a DiscardMeters,
    taps: &'a Taps,
) -> Delivery<'a> {
    Delivery {
        audio,
        encoded,
        relay,
        discards,
        taps,
    }
}

/// Release everything the jitter buffer is holding, because nothing more is coming.
#[allow(clippy::too_many_arguments)]
async fn flush(
    buffer: &mut JitterBuffer,
    to: &Delivery<'_>,
    decoding: &mut Decoding,
    digits: &Keypresses,
    dtmf: &mut sipx_rtp::dtmf::Receiver,
    dtmf_deadline: &mut Option<tokio::time::Instant>,
    config: &Config,
    stop: &Stop,
) -> bool {
    for packet in buffer.drain() {
        if !deliver(to, decoding, digits, dtmf, config, stop, &packet).await {
            return false;
        }
        if dtmf.in_progress().is_none() {
            *dtmf_deadline = None;
        }
    }
    true
}

fn is_telephone_report(packet: &Packet, config: &Config) -> bool {
    config.dtmf_payload_type == Some(packet.payload_type)
        && DtmfEvent::decode(&packet.payload).is_some()
}

/// Authenticate and decrypt a datagram, or drop it.
///
/// `None` means the packet does not belong to this stream and nothing about it should reach the
/// parser, the jitter buffer or the statistics — which is the point of authenticating at all:
/// a forged packet must not be able to move any state.
fn authenticated(
    context: Option<&mut sipx_rtp::SrtpContext>,
    bytes: Bytes,
    source: SocketAddr,
    discards: &DiscardMeters,
    browser_ingress: Option<&Arc<std::sync::Mutex<crate::browser::ComponentIngress>>>,
) -> Option<Bytes> {
    let Some(context) = context else {
        return Some(bytes);
    };
    match context.unprotect(&bytes) {
        Ok(plain) => Some(Bytes::from(plain)),
        Err(error) => {
            discards
                .srtp_unprotect_failures
                .fetch_add(1, Ordering::Relaxed);
            if let Some(ingress) = browser_ingress {
                let mut ingress = crate::browser::lock_ingress(ingress);
                match &error {
                    sipx_rtp::srtp::SrtpError::TooShort(_) => {
                        ingress.note_malformed(crate::browser::IngressClass::Srtp);
                    }
                    sipx_rtp::srtp::SrtpError::Replayed(_) => {
                        ingress.note_replay(crate::browser::IngressClass::Srtp);
                    }
                    _ => ingress.note_authentication_failure(crate::browser::IngressClass::Srtp),
                }
            }
            // discard: `srtp_unprotect_failures` was incremented above; browser sessions also
            // classified the same refusal into malformed, replay or authentication exactly once.
            tracing::debug!(%error, %source, "dropping a packet that failed SRTP");
            None
        }
    }
}

/// The profile a configuration was keyed for, if it was keyed at all.
fn srtp_profile_of(config: &Config) -> Option<sipx_rtp::srtp::Profile> {
    config.srtp.as_ref().map(|keys| keys.profile)
}

/// An SRTP context for one direction of a keyed session, or `None` when the media is not
/// encrypted.
///
/// Takes the whole [`SrtpKeys`] and a selector rather than a bare key pair, so the profile and the
/// key material it belongs to are read from one value. Splitting them at the call site is how a
/// context ends up keyed for one transform and told it is another.
fn srtp_context(
    keys: Option<&SrtpKeys>,
    direction: fn(&SrtpKeys) -> &(Vec<u8>, Vec<u8>),
) -> Option<sipx_rtp::SrtpContext> {
    let keys = keys?;
    let (key, salt) = direction(keys);
    match sipx_rtp::SrtpContext::new(keys.profile, key, salt) {
        Ok(context) => Some(context),
        Err(error) => {
            // discard: this refuses configuration before any packet exists; runtime discard
            // counters count media the path was actually handed, not setup values.
            // Carrying on unencrypted would be the worst of the three options: the far end
            // expects SRTP, so the media is useless to it *and* readable to everyone else.
            tracing::error!(%error, "SRTP keys were refused; this session will carry nothing");
            None
        }
    }
}

/// Bind the control port for a media port, if it is free.
async fn bind_control_port(media: SocketAddr) -> Option<Arc<UdpSocket>> {
    let port = media.port().checked_add(1)?;
    UdpSocket::bind(SocketAddr::new(media.ip(), port))
        .await
        .ok()
        .map(Arc::new)
}

#[allow(clippy::too_many_arguments)]
/// The RTP clock for one outgoing stream.
///
/// One type rather than a pile of locals, because these six values are one thing: a tone and
/// the audio around it share a timeline, and the bookkeeping that keeps them from overlapping
/// is the fiddliest part of sending. Keeping it here leaves the send loop about pacing, which
/// is what the send loop is about.
struct SendClock {
    sequence: u16,
    timestamp: u32,
    /// The tone in progress, if any, and the timestamp it started at.
    current_tone: Option<u64>,
    tone_started_at: Option<u32>,
    /// What the clock owes the tone in progress, charged when it is over.
    tone_duration: u32,
    /// Whether the next audio packet begins a new talkspurt — RFC 3550's marker bit says so,
    /// and a tone interrupts the audio.
    ending_a_tone: bool,
}

impl SendClock {
    /// A clock starting at a random point, as RFC 3550 §5.1 requires of both counters.
    fn new() -> Self {
        Self {
            sequence: rand::random(),
            timestamp: rand::random(),
            current_tone: None,
            tone_started_at: None,
            tone_duration: 0,
            ending_a_tone: false,
        }
    }

    /// Move past a packet that has been sent. Both counters wrap, and both are supposed to.
    fn advance(&mut self, samples: u32) {
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(samples);
    }

    /// Build one packet of audio.
    ///
    /// `None` when the codec refused the frame, which is not the same as an empty packet: an
    /// empty packet would have the far end decode nothing as audio.
    fn audio(
        &mut self,
        encoding: &mut Encoding,
        payload_type: u8,
        ssrc: u32,
        samples: &[i16],
        samples_per_clock_unit: u32,
    ) -> Option<(Packet, u32)> {
        // A tone that has just finished owes the clock its duration; pay it before stamping the
        // audio that follows, or the audio overlaps the keypress.
        self.timestamp = self
            .timestamp
            .wrapping_add(std::mem::take(&mut self.tone_duration));
        self.current_tone = None;
        self.tone_started_at = None;

        let encoded = encoding.encode(samples)?;
        let mut packet = Packet::new(
            payload_type,
            self.sequence,
            self.timestamp,
            ssrc,
            Bytes::from(encoded),
        );
        packet.marker = std::mem::take(&mut self.ending_a_tone);

        // The timestamp advances by the samples this packet actually carried, not by the
        // configured packet size. They are usually the same, and when they are not — a caller
        // sending 10 ms frames on a 20 ms config — advancing by the configured size builds a
        // timeline at the wrong rate, and the far end plays the call with a gap between every
        // packet.
        //
        // Carried samples are converted to *clock units* first. For every codec but G.722 the
        // divisor is one; for G.722 the RTP clock runs at half the sample rate
        // (RFC 3551 §4.5.2), and advancing by the sample count doubles the timeline.
        let advance = u32::try_from(samples.len()).unwrap_or(0) / samples_per_clock_unit.max(1);
        Some((packet, advance))
    }

    /// Build one packet of an RFC 4733 tone.
    fn tone(
        &mut self,
        payload_type: u8,
        ssrc: u32,
        event: DtmfEvent,
        offset: u32,
        tone: u64,
    ) -> (Packet, u32) {
        // A new keypress starts here; anything with the same tag continues the one in progress
        // and reuses its timestamp. That shared timestamp is what marks the packets as one
        // press — including the end retransmissions, which is the case that gets this wrong.
        let starting = self.current_tone != Some(tone);
        if starting {
            // The previous tone's duration is charged to the clock now, so audio resumes past
            // it rather than on top of it.
            self.timestamp = self
                .timestamp
                .wrapping_add(std::mem::take(&mut self.tone_duration));
            self.current_tone = Some(tone);
            self.tone_started_at = Some(self.timestamp);
        }

        // Within a keypress, the segment offset moves the stamp: a segment past the duration
        // field's range starts at its own timestamp (RFC 4733 §2.5.1.3). The marker stays on
        // the event's first packet only — a segment continues a keypress, it does not start one.
        let mut packet = Packet::new(
            payload_type,
            self.sequence,
            self.tone_started_at
                .unwrap_or(self.timestamp)
                .wrapping_add(offset),
            ssrc,
            event.encode(),
        );
        packet.marker = starting;
        if event.end {
            // The whole event's length: the final segment's start plus what it carried.
            self.tone_duration = offset.wrapping_add(u32::from(event.duration));
            self.ending_a_tone = true;
        }
        (packet, 0)
    }
}

/// Everything the send loop needs beyond its socket and its channel.
struct Sending {
    remote: Arc<Mutex<SocketAddr>>,
    config: Config,
    ssrc: u32,
    sent: Arc<AtomicU64>,
    outbound: Arc<Outbound>,
    muted: Arc<AtomicBool>,
    /// Where the fact that a packet went out is reported, so §11's keepalive is only sent on a
    /// pair that has actually been quiet for Tr.
    ice: Option<crate::ice::driver::Handle>,
    stop: Arc<Stop>,
    /// Constructed before this worker is spawned, so startup cannot fail inside the task.
    encoding: Encoding,
    discards: Arc<DiscardMeters>,
    /// Where transmitted audio is offered to attached processors (`M-54`).
    taps: Arc<Taps>,
}

// This is the single owner of the RTP send sequence, codec, SRTP context, pacing and their discard
// meters. Splitting those state transitions across helpers would make their ordering harder to audit.
#[allow(clippy::too_many_lines)]
async fn send_loop(socket: Arc<UdpSocket>, mut outgoing: mpsc::Receiver<Frame>, sending: Sending) {
    let Sending {
        remote,
        config,
        ssrc,
        sent,
        outbound,
        muted,
        ice,
        stop,
        mut encoding,
        discards,
        taps,
    } = sending;
    let audio_rate = config.audio_rate();
    let mut clock = SendClock::new();
    // One context, owned by this loop. SRTP keeps a rollover counter and a replay window per
    // stream, and a context behind a lock would put a mutex in the packet path for state exactly
    // one task ever touches.
    let mut protect = srtp_context(config.srtp.as_ref(), |keys| &keys.local);

    // One clock for the whole stream. Sending on channel readiness instead makes the packet
    // rate depend on how fast the application produces samples.
    let mut tick = tokio::time::interval(config.packet_duration);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        if stop.is_stopped() {
            return;
        }
        tokio::select! {
            () = stop.wait() => return,
            _ = tick.tick() => {}
        }

        let Some(frame) = next_frame(&mut outgoing, &stop).await else {
            return;
        };

        // The mute gate goes here — before the packet is built, and therefore before the
        // sequence number, the send counters and the sender report's octet count have been moved
        // by it. See [`gated`].
        let frame = gated(frame, &muted, config.samples_per_packet());

        // The outbound tap (`M-54`, `docs/specs/call-audio-seam.md` §3): after the mute gate, so a
        // muted call is not reported as transmitting, and before encoding, so a processor sees the
        // samples rather than the codec's opinion of them. It never awaits, so no processor can
        // delay this packet.
        if let Frame::Audio { samples, .. } = &frame {
            taps.offer(AudioDirection::Outbound, audio_rate, samples);
        }

        let (packet, advance) = match &frame {
            Frame::Audio { samples, .. } => {
                let Some(built) = clock.audio(
                    &mut encoding,
                    config.wire_payload_type(),
                    ssrc,
                    samples,
                    config.codec.samples_per_clock_unit(),
                ) else {
                    discards
                        .opus_encode_failures
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                built
            }
            Frame::Encoded {
                payload_type,
                payload,
            } => {
                // Verbatim, on this leg's own sequence and timestamp. The advance is the
                // configured packet duration in *clock units*: the bytes came from a stream
                // with the same packetisation, and nothing here can look inside them to check.
                let packet = Packet::new(
                    *payload_type,
                    clock.sequence,
                    clock.timestamp,
                    ssrc,
                    payload.clone(),
                );
                (
                    packet,
                    u32::try_from(config.clock_units_per_packet()).unwrap_or(0),
                )
            }
            Frame::Dtmf {
                event,
                offset,
                tone,
            } => {
                let Some(payload_type) = config.dtmf_payload_type else {
                    // Nothing negotiated `telephone-event`, so there is no payload type to send
                    // it on. Dropping is right: guessing one means sending keypresses on
                    // whatever the far end uses that number for.
                    continue;
                };
                clock.tone(payload_type, ssrc, *event, *offset, *tone)
            }
        };

        let destination = *remote.lock().await;
        let payload_len = packet.payload.len();
        let encoded = packet.encode();
        let datagram = match protect.as_mut() {
            Some(context) => match context.protect(&encoded) {
                Ok(protected) => Bytes::from(protected),
                Err(error) => {
                    // Sending it in the clear instead is not an option: the far end negotiated
                    // encryption and a cleartext packet is both unreadable to it and readable to
                    // everyone else.
                    // discard: `Packet::encode` always makes the complete header `protect`
                    // requires, so this error branch is structurally unreachable here.
                    tracing::warn!(%error, "dropping a packet SRTP could not protect");
                    continue;
                }
            },
            None => encoded,
        };
        if socket.send_to(&datagram, destination).await.is_err() {
            return;
        }
        // §11: a keepalive goes out only when nothing has been sent on the selected pair for Tr,
        // so the agent has to be told that something was. Reported after the send and not before,
        // because a packet that did not leave has not held the binding open.
        if let Some(handle) = &ice {
            handle.data_sent(ComponentId::RTP);
        }
        sent.fetch_add(1, Ordering::Relaxed);
        // What a sender report describes. The octet count is payload only, headers excluded
        // (RFC 3550 §6.4.1) — counting headers would overstate the bandwidth by a fifth on
        // 20 ms G.711 and by far more on anything smaller.
        outbound.packets.fetch_add(1, Ordering::Relaxed);
        outbound
            .octets
            .fetch_add(payload_len as u64, Ordering::Relaxed);
        outbound
            .timestamp
            .store(packet.timestamp, Ordering::Relaxed);

        // Both counters wrap, and both are supposed to.
        //
        // The timestamp advances by the samples this packet actually carried, not by the
        // configured packet size. They are usually the same, and when they are not — a caller
        // sending 10 ms frames on a 20 ms config — advancing by the configured size builds a
        // timeline at the wrong rate, and the far end plays the call with a gap between every
        // packet.
        clock.advance(advance);
    }
}

/// The next frame the send loop should put on the wire, or `None` when there will not be another.
///
/// Both awaits check the stop signal. A loop parked on its channel when the call is hung up would
/// otherwise go on sending audio into a torn-down call.
///
/// Frames belonging to a stopped playback are skipped here rather than sent, and skipping one
/// costs nothing — this takes the next frame straight away rather than returning to the caller's
/// pacing tick. That is what bounds a stop at [`Playback::STOP_BOUND_PACKETS`]: whatever backlog
/// the queue was holding for the stopped clip drains inside one packet interval, and none of it
/// reaches the wire.
async fn next_frame(outgoing: &mut mpsc::Receiver<Frame>, stop: &Stop) -> Option<Frame> {
    loop {
        let frame = tokio::select! {
            () = stop.wait() => return None,
            received = outgoing.recv() => received?,
        };
        if stop.is_stopped() {
            return None;
        }
        if !discarded(&frame) {
            return Some(frame);
        }
    }
}

/// One frame as the session actually sends it: itself, or what a muted session puts in its place
/// (`M-18`).
///
/// Applied *before* the packet is built, which is the part RFC 3550 §6 constrains: a mute that
/// dropped the finished datagram at the socket instead would leave the sequence number, the send
/// counters and the sender report's octet count (§6.4.1) all describing packets that never went
/// out, and would open a sequence gap the far end scores as loss.
///
/// Audio becomes the same number of samples of silence, so the timestamp this frame advances the
/// clock by is the one it would have advanced it by unmuted — the far end's timeline does not
/// move under it.
///
/// A relayed payload ([`Frame::Encoded`], a bridge passing bytes across) becomes one packet's
/// worth of silence in *this* session's codec. Its bytes cannot be silenced in place: they are an
/// opaque payload in whatever the other leg negotiated, and there is nothing here that can look
/// inside them. Substituting this session's own silence keeps the leg saying nothing rather than
/// saying what the muted party said.
///
/// A telephone event passes through. It is not audio: it is generated by this endpoint on
/// purpose, the way a keypad tone is on a handset, and a mute that swallowed keypresses would
/// make a muted caller unable to answer an IVR.
fn gated(frame: Frame, muted: &AtomicBool, samples_per_packet: usize) -> Frame {
    if !muted.load(Ordering::SeqCst) {
        return frame;
    }
    match frame {
        Frame::Audio { samples, playback } => Frame::Audio {
            samples: vec![0; samples.len()],
            playback,
        },
        Frame::Encoded { .. } => Frame::Audio {
            samples: vec![0; samples_per_packet],
            playback: None,
        },
        event @ Frame::Dtmf { .. } => event,
    }
}

/// Whether this frame belongs to a playback that has been stopped, and so must not be sent
/// (`M-17`).
///
/// Read *before* the packet is built, for the same RFC 3550 §6 reason the mute gate is
/// ([`gated`]): the sequence number, the send counters and the sender report's octet count must
/// describe packets that actually went out. A frame discarded here never touches any of them,
/// which leaves the stream in exactly the state it is in whenever the application has nothing to
/// say — no gap for the far end to score as loss, because a gap needs a sequence number that was
/// allocated and never sent.
fn discarded(frame: &Frame) -> bool {
    matches!(frame, Frame::Audio { playback: Some(playback), .. } if playback.is_stopped())
}

/// Start the playback queue and hand back the end a session keeps (`M-17`).
fn spawn_playback_queue(
    outgoing: &mpsc::Sender<Frame>,
    stop: &Arc<Stop>,
) -> (mpsc::Sender<Clip>, tokio::task::JoinHandle<()>) {
    let (clips_tx, clips_rx) = mpsc::channel::<Clip>(Playback::QUEUE_DEPTH);
    let owner = tokio::spawn(playback_loop(clips_rx, outgoing.clone(), Arc::clone(stop)));
    (clips_tx, owner)
}

#[cfg(feature = "dtls")]
fn spawn_browser_playback_queue(
    outgoing: &mpsc::Sender<Frame>,
    stop: &Arc<Stop>,
    profile_tasks: Arc<crate::browser::ProfileTasks>,
) -> (mpsc::Sender<Clip>, tokio::task::JoinHandle<()>) {
    let (clips_tx, clips_rx) = mpsc::channel::<Clip>(Playback::QUEUE_DEPTH);
    let owner = tokio::spawn(crate::browser::profile_task(
        profile_tasks,
        playback_loop(clips_rx, outgoing.clone(), Arc::clone(stop)),
    ));
    (clips_tx, owner)
}

/// The playback queue: one clip at a time, in the order they were started (`M-17`).
///
/// One task owns it, which is what makes "clips queue" true by construction: the order clips are
/// handed to the channel is the order they reach the send queue, and no two of them can ever
/// interleave their packets.
///
/// A task rather than a lock around the send queue. One owner is what makes the ordering a
/// property of the type instead of something every caller has to be careful about: two clips
/// started at once cannot interleave their packets, and the order they were started in is the
/// order the far end hears them, whatever order the callers' tasks happened to be scheduled in.
async fn playback_loop(
    mut clips: mpsc::Receiver<Clip>,
    outgoing: mpsc::Sender<Frame>,
    stop: Arc<Stop>,
) {
    loop {
        let clip = tokio::select! {
            () = stop.wait() => return,
            next = clips.recv() => match next {
                Some(clip) => clip,
                None => return,
            },
        };

        let end = feed(&clip, &outgoing, &stop).await;
        clip.finish(end);
        if end == PlaybackEnd::SessionEnded {
            // Whatever is still queued goes down with the receiver, and every handle waiting on
            // one of those clips learns the same thing when its sender drops.
            return;
        }
    }
}

/// Hand one clip to the send queue, packet by packet, until it runs out or something cuts it
/// short.
async fn feed(clip: &Clip, outgoing: &mpsc::Sender<Frame>, stop: &Stop) -> PlaybackEnd {
    // Armed at the head of the queue, not when the clip was started: a key pressed while an
    // earlier clip was still playing belongs to that clip. Reading the counter here is what marks
    // everything before this moment as already seen.
    let mut keypresses = match clip.interrupt {
        Interrupt::OnDigit => {
            let mut keypresses = clip.keypresses.clone();
            let _seen = *keypresses.borrow_and_update();
            Some(keypresses)
        }
        Interrupt::Never => None,
    };

    for chunk in clip.samples.chunks(clip.samples_per_packet) {
        if clip.stop.is_stopped() {
            return PlaybackEnd::Stopped;
        }
        let mut samples = chunk.to_vec();
        // The last chunk may be short. Padding with silence keeps every packet the same size,
        // which is what a far-end jitter buffer expects.
        samples.resize(clip.samples_per_packet, 0);
        let frame = Frame::Audio {
            samples,
            playback: Some(Arc::clone(&clip.stop)),
        };

        // Biased so that a stop or a keypress wins over queueing one more packet. Without it a
        // clip whose send queue happens to have room would go on feeding it for as long as the
        // scheduler kept picking that branch.
        tokio::select! {
            biased;
            () = stop.wait() => return PlaybackEnd::SessionEnded,
            () = clip.stop.wait() => return PlaybackEnd::Stopped,
            () = keypress(keypresses.as_mut()) => {
                // Set here rather than left to the caller: it is what tells the send loop to
                // discard the packets of this clip it is already holding, and until it is set
                // they would go out.
                clip.stop.stop();
                return PlaybackEnd::Interrupted;
            }
            queued = outgoing.send(frame) => {
                if queued.is_err() {
                    return PlaybackEnd::SessionEnded;
                }
            }
        }
    }
    PlaybackEnd::Completed
}

/// Resolve when the far end presses a key, or never.
///
/// Never in two cases, and they are the same case to a caller: the clip was not started
/// interruptible, or the receive loop is gone — in which case no keypress is coming and treating
/// the channel's closure as one would cut every remaining clip short at the end of a call.
async fn keypress(keypresses: Option<&mut watch::Receiver<u64>>) {
    if let Some(keypresses) = keypresses
        && keypresses.changed().await.is_ok()
    {
        return;
    }
    std::future::pending::<()>().await;
}

/// Where a keypress goes: the application's channel, and the tick an interruptible playback
/// watches.
///
/// One type rather than two parameters, because the ordering between them is load-bearing — see
/// [`deliver`].
struct Keypresses {
    to: mpsc::Sender<(Digit, Duration)>,
    arrivals: Arc<watch::Sender<u64>>,
}

/// Everything the receive loop needs, grouped because eight positional arguments is a
/// mis-ordering waiting to happen — two of them are `Arc<AtomicU64>`-shaped and swapping them
/// would compile.
struct Inbound {
    audio: mpsc::Sender<Vec<i16>>,
    encoded: mpsc::Sender<Encoded>,
    relay: Arc<AtomicBool>,
    digits: Keypresses,
    remote: Arc<Mutex<SocketAddr>>,
    config: Config,
    received: Arc<AtomicU64>,
    stats: Arc<Mutex<StreamStats>>,
    rtcp_observation: RtcpObservation,
    ssrc: u32,
    /// Whether the first packet's source replaces the advertised address (symmetric RTP).
    ///
    /// False for a stream ICE is driving: RFC 8445 §8.1.1's selected pair replaces this, and it
    /// has to *replace* it rather than race it — a stream that also learned from the first RTP
    /// packet to arrive would let an off-path sender who guessed the port undo the one thing a
    /// checked path bought.
    symmetric: bool,
    /// Where a STUN datagram goes (RFC 5764 §5.1.2), when ICE is running.
    ice: Option<crate::ice::driver::Handle>,
    browser_ingress: Option<Arc<std::sync::Mutex<crate::browser::ComponentIngress>>>,
    stop: Arc<Stop>,
    /// Constructed before this worker is spawned, so startup cannot fail inside the task.
    decoding: Decoding,
    discards: Arc<DiscardMeters>,
    /// Where received audio is offered to attached processors (`M-54`).
    taps: Arc<Taps>,
}

/// Split a datagram arriving on a port that carries media three ways (RFC 5764 §5.1.2).
///
/// The first byte decides, and it decides **before anything else looks at the datagram**: a
/// connectivity check must never reach the jitter buffer and an RTP packet must never reach the
/// ICE agent. Returns the bytes only for the RTP path; a check goes to the agent and anything
/// else is dropped by name rather than handed to whichever parser happens to be first.
///
/// A check is dropped rather than kept when no agent is running, which is what the media loops
/// did with one before ICE existed — it would fail to parse as RTP one line later.
fn demultiplex<'a>(
    datagram: &'a [u8],
    from: SocketAddr,
    on: ice::LocalBase,
    ice: Option<&ice::driver::Handle>,
) -> Option<&'a [u8]> {
    match crate::dtls::classify(datagram) {
        crate::dtls::Arriving::Rtp => Some(datagram),
        crate::dtls::Arriving::Stun => {
            if let Some(handle) = ice {
                handle.datagram(from, on, datagram.to_vec());
            }
            None
        }
        crate::dtls::Arriving::Dtls | crate::dtls::Arriving::Unknown => None,
    }
}

/// The RFC 5761 second-stage class inside RFC 5764's RTP-or-RTCP first-byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MuxedPacket<'a> {
    Rtp(&'a [u8]),
    Rtcp(&'a [u8]),
}

fn classify_muxed(datagram: &[u8]) -> Option<MuxedPacket<'_>> {
    let packet_type = *datagram.get(1)?;
    if (192..=223).contains(&packet_type) {
        Some(MuxedPacket::Rtcp(datagram))
    } else {
        Some(MuxedPacket::Rtp(datagram))
    }
}

/// Everything the two RTCP loops need, grouped because they share most of it.
struct Control {
    media: Arc<UdpSocket>,
    rtcp: Option<Arc<UdpSocket>>,
    remote: Arc<Mutex<SocketAddr>>,
    rtcp_remote: Arc<Mutex<Option<SocketAddr>>>,
    interval: Option<Duration>,
    mode: sipx_sdp::RtcpMode,
    ssrc: u32,
    cname: String,
    stats: Arc<Mutex<StreamStats>>,
    outbound: Arc<Outbound>,
    rtcp_observation: RtcpObservation,
    srtp: Option<SrtpKeys>,
    ice: Option<ice::driver::Handle>,
    stop: Arc<Stop>,
    discards: Arc<DiscardMeters>,
    #[cfg(feature = "dtls")]
    profile_tasks: Option<Arc<crate::browser::ProfileTasks>>,
}

/// Start the report loops, as far as this session's configuration and sockets allow.
fn spawn_control(control: Control) -> Vec<tokio::task::JoinHandle<()>> {
    let mut owners = Vec::with_capacity(2);
    #[cfg(feature = "dtls")]
    let profile_tasks = control.profile_tasks.clone();
    if let Some(interval) = control.interval {
        let reporter = rtcp_loop(
            // Reports go out from the control port when there is one, which is what a peer
            // expects to see them come from; from the media port otherwise, which some peers
            // will refuse but is better than not reporting at all.
            control.rtcp.clone().unwrap_or(control.media),
            control.remote,
            control.rtcp_remote,
            interval,
            control.mode,
            control.ssrc,
            control.cname,
            control.stats,
            control.outbound,
            Arc::clone(&control.rtcp_observation.feedback),
            control.srtp.clone(),
            Arc::clone(&control.stop),
        );
        #[cfg(feature = "dtls")]
        let owner = if let Some(tasks) = profile_tasks.clone() {
            tokio::spawn(crate::browser::profile_task(tasks, reporter))
        } else {
            tokio::spawn(reporter)
        };
        #[cfg(not(feature = "dtls"))]
        let owner = tokio::spawn(reporter);
        owners.push(owner);
    }
    if control.mode == sipx_sdp::RtcpMode::Separate
        && let Some(port) = control.rtcp
    {
        let receiver = rtcp_receive_loop(
            port,
            control.ssrc,
            control.rtcp_observation,
            control.srtp,
            control.ice,
            control.stop,
            control.discards,
        );
        #[cfg(feature = "dtls")]
        let owner = if let Some(tasks) = profile_tasks {
            tokio::spawn(crate::browser::profile_task(tasks, receiver))
        } else {
            tokio::spawn(receiver)
        };
        #[cfg(not(feature = "dtls"))]
        let owner = tokio::spawn(receiver);
        owners.push(owner);
    }
    owners
}

/// Start the ICE driver for this session, over the sockets the session is running on.
///
/// The base numbering is the one gathering used and the only one there is: base 0 is the media
/// socket, base 1 the control port when there is one. The agent names a socket by that index
/// alone (`docs/specs/ice.md` §2), so the two lists have to be built the same way in both places
/// or a check leaves the wrong port.
fn spawn_ice(
    local: ice::LocalDescription,
    socket: &Arc<UdpSocket>,
    rtcp: Option<&Arc<UdpSocket>>,
    destinations: &ice::driver::Destinations,
    stop: &Arc<Stop>,
    discards: &Arc<DiscardMeters>,
) -> (ice::driver::Handle, tokio::task::JoinHandle<()>) {
    let (agent, pending) = local.into_driver_parts();
    let mut sockets = vec![Arc::clone(socket)];
    if let Some(control) = rtcp {
        sockets.push(Arc::clone(control));
    }
    ice::driver::spawn(
        agent,
        pending,
        sockets,
        destinations.clone(),
        Arc::clone(stop),
        Arc::clone(discards),
    )
}

/// Decide whether a packet belongs to the stream this session is carrying.
///
/// RTP has no authentication, so this is not a security control — anyone who can guess the port
/// can still forge a first packet. What it buys is that once a stream is established, a *later*
/// forged packet with a different SSRC cannot redirect our media or poison the jitter buffer,
/// which is the difference between a race an attacker has to win and one they can win at
/// leisure.
async fn accept_source(
    stream: &mut Option<u32>,
    packet: &Packet,
    source: SocketAddr,
    remote: &Arc<Mutex<SocketAddr>>,
    stats: &Arc<Mutex<StreamStats>>,
    symmetric: bool,
    discards: &DiscardMeters,
) -> bool {
    match *stream {
        None => {
            // Symmetric RTP: the observed source replaces the advertised address, because
            // behind a NAT the advertised one is private and this is the only path back.
            // Deliberately after the packet parses, so a stray STUN probe cannot move it.
            //
            // Not for a stream ICE is driving. There the address is the selected pair's
            // (RFC 8445 §8.1.1) and an unauthenticated packet must not be able to move it —
            // `docs/specs/ice.md` §11.3. The SSRC is still learned either way, because that is
            // what keeps a second source out of the jitter buffer.
            if symmetric {
                *remote.lock().await = source;
            }
            *stream = Some(packet.ssrc);
            // The far end names itself in its first packet, and the statistics carry that name
            // into every report block (RFC 3550 §6.4.1: a block's SSRC is the source it
            // describes).
            stats.lock().await.set_ssrc(packet.ssrc);
            true
        }
        Some(established) if established != packet.ssrc => {
            // Another source on our port. Dropped rather than mixed in: one packet with a high
            // sequence number would otherwise advance the jitter buffer past every genuine
            // packet still to come, and the call goes silent.
            discards.foreign_ssrc.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                %source,
                ssrc = packet.ssrc,
                "ignoring a packet from a different synchronisation source"
            );
            false
        }
        Some(_) => true,
    }
}

enum ReceiveInput {
    Socket {
        socket: Arc<UdpSocket>,
        datagram: Vec<u8>,
    },
    #[cfg(feature = "dtls")]
    Browser(crate::browser::MediaIngress),
}

enum ReceivedDatagram {
    Rtp { source: SocketAddr, bytes: Vec<u8> },
    Rtcp { bytes: Vec<u8> },
    Silence,
    Closed,
}

impl ReceiveInput {
    fn socket(socket: Arc<UdpSocket>) -> Self {
        Self::Socket {
            socket,
            datagram: vec![0u8; 2048],
        }
    }

    async fn next(
        &mut self,
        deadline: tokio::time::Instant,
        config: &Config,
        ice: Option<&crate::ice::driver::Handle>,
        stop: &Stop,
    ) -> ReceivedDatagram {
        loop {
            match self {
                Self::Socket { socket, datagram } => {
                    let read = tokio::select! {
                        () = stop.wait() => return ReceivedDatagram::Closed,
                        read = tokio::time::timeout_at(deadline, socket.recv_from(datagram)) => read,
                    };
                    let (length, source) = match read {
                        Ok(Ok(received)) => received,
                        Ok(Err(_)) => return ReceivedDatagram::Closed,
                        Err(_elapsed) => return ReceivedDatagram::Silence,
                    };
                    let arrived = datagram.get(..length).unwrap_or_default();
                    let Some(media) = demultiplex(arrived, source, crate::ice::LocalBase(0), ice)
                    else {
                        continue;
                    };
                    if config.rtcp_mode == sipx_sdp::RtcpMode::Mux {
                        match classify_muxed(media) {
                            Some(MuxedPacket::Rtp(media)) => {
                                return ReceivedDatagram::Rtp {
                                    source,
                                    bytes: media.to_vec(),
                                };
                            }
                            Some(MuxedPacket::Rtcp(control)) => {
                                return ReceivedDatagram::Rtcp {
                                    bytes: control.to_vec(),
                                };
                            }
                            None => continue,
                        }
                    }
                    return ReceivedDatagram::Rtp {
                        source,
                        bytes: media.to_vec(),
                    };
                }
                #[cfg(feature = "dtls")]
                Self::Browser(media) => {
                    let next = tokio::select! {
                        () = stop.wait() => return ReceivedDatagram::Closed,
                        next = tokio::time::timeout_at(deadline, async {
                            tokio::select! {
                                packet = media.srtp.recv() => packet.map(|packet| (false, packet)),
                                packet = media.srtcp.recv() => packet.map(|packet| (true, packet)),
                            }
                        }) => next,
                    };
                    return match next {
                        Err(_elapsed) => ReceivedDatagram::Silence,
                        Ok(Some((true, packet))) => ReceivedDatagram::Rtcp {
                            bytes: packet.bytes,
                        },
                        Ok(Some((false, packet))) => ReceivedDatagram::Rtp {
                            source: packet.source,
                            bytes: packet.bytes,
                        },
                        Ok(None) => ReceivedDatagram::Closed,
                    };
                }
            }
        }
    }
}

// This is the single ordered path from demultiplexing through authentication, source pinning,
// statistics and delivery. Its length keeps that security-sensitive order visible in one place.
#[allow(clippy::too_many_lines)]
async fn receive_loop(mut input: ReceiveInput, inbound: Inbound) {
    let Inbound {
        audio: incoming,
        encoded,
        relay,
        digits,
        remote,
        config,
        received,
        stats,
        rtcp_observation,
        ssrc,
        symmetric,
        ice,
        browser_ingress,
        stop,
        mut decoding,
        discards,
        taps,
    } = inbound;
    let mut buffer = match config.jitter_max_depth {
        Some(max) => JitterBuffer::adaptive(config.jitter_depth, max),
        None => JitterBuffer::new(config.jitter_depth),
    };
    let mut unprotect = srtp_context(config.srtp.as_ref(), |keys| &keys.remote);
    let mut unprotect_rtcp = srtp_context(config.srtp.as_ref(), |keys| &keys.remote);
    let mut dtmf = sipx_rtp::dtmf::Receiver::new();
    let started = tokio::time::Instant::now();
    // The synchronisation source this session is carrying; see `accept_source` for why one is
    // pinned at all, and for what it does and does not buy.
    let mut stream: Option<u32> = None;

    // When the far end stops, whatever the buffer is still holding has to come out. Without
    // this the last `depth - 1` packets are never played: in a continuous call that is
    // invisible, but at the end of every clip it clips the tail off.
    let flush_after = config
        .packet_duration
        .saturating_mul(4)
        .max(Duration::from_millis(60));
    // RFC 4733 §2.5.2.2 bounds a receiver's stuck-tone extension at three packet intervals.
    // This is a definition of media silence, not a substitute for a happens-before relation: the
    // next accepted event or ordinary-media packet closes the tone immediately.
    let dtmf_silence = config.packet_duration.saturating_mul(3);
    let mut dtmf_deadline = None;

    loop {
        if stop.is_stopped() {
            return;
        }
        let silence_deadline =
            dtmf_deadline.unwrap_or_else(|| tokio::time::Instant::now() + flush_after);
        let (media, source) = match input
            .next(silence_deadline, &config, ice.as_ref(), &stop)
            .await
        {
            ReceivedDatagram::Rtp { source, bytes } => (bytes, source),
            ReceivedDatagram::Rtcp { bytes } => {
                process_rtcp(
                    &bytes,
                    ssrc,
                    &rtcp_observation,
                    &mut unprotect_rtcp,
                    &discards,
                    browser_ingress.as_ref(),
                )
                .await;
                continue;
            }
            ReceivedDatagram::Closed => return,
            ReceivedDatagram::Silence => {
                // Silence. Release what is held rather than holding it against a packet that is
                // not coming — otherwise the last `depth - 1` packets of every clip are lost.
                let to = delivery(&incoming, &encoded, &relay, &discards, &taps);
                if !flush(
                    &mut buffer,
                    &to,
                    &mut decoding,
                    &digits,
                    &mut dtmf,
                    &mut dtmf_deadline,
                    &config,
                    &stop,
                )
                .await
                {
                    return;
                }
                // `flush` may have delivered an E-bit report. Only an event still current after
                // that ordered drain is completed by the fired RFC 4733 silence input.
                if let Some(completed) = dtmf.timeout() {
                    deliver_digit(&to, &digits, completed, config.clock_rate);
                }
                dtmf_deadline = None;
                continue;
            }
        };
        let bytes = Bytes::from(media);
        // Authenticated before it is parsed. A packet that fails is dropped and nothing about it
        // reaches the parser, the jitter buffer or the statistics — which is the point of
        // authenticating at all: forged packets must not be able to move any state.
        let Some(bytes) = authenticated(
            unprotect.as_mut(),
            bytes,
            source,
            &discards,
            browser_ingress.as_ref(),
        ) else {
            continue;
        };
        let Ok(packet) = Packet::decode(&bytes) else {
            // A malformed packet is dropped, not fatal. Media ports attract stray traffic —
            // STUN probes, port scans, the occasional scanner — and none of it should end a
            // call.
            if let Some(ingress) = &browser_ingress {
                crate::browser::lock_ingress(ingress)
                    .note_malformed(crate::browser::IngressClass::Srtp);
            }
            continue;
        };

        if !accept_source(
            &mut stream,
            &packet,
            source,
            &remote,
            &stats,
            symmetric,
            &discards,
        )
        .await
        {
            continue;
        }

        received.fetch_add(1, Ordering::Relaxed);

        // The arrival clock has to be in the same units as the RTP timestamp — 8000 per second
        // for G.711 — or the jitter estimate measures the difference between two unit systems
        // rather than between two packets.
        // One arrival instant, used by both. Reading the clock twice would have the jitter
        // buffer and the RTCP report disagree about when the same packet turned up, by however
        // long the lock took — and the whole point of both numbers is that they are comparable.
        let arrival = arrival_in_timestamp_units(started, config.clock_rate);
        note_arrival(&stats, &packet, &config, arrival).await;

        // Arrival, rather than later jitter-buffer release, arms the RFC 4733 silence bound.
        // Keeping the absolute deadline outside `ReceiveInput::next` means ignored datagrams and
        // RTCP cannot postpone it by repeatedly restarting a relative timeout.
        if is_telephone_report(&packet, &config) {
            dtmf_deadline = Some(tokio::time::Instant::now() + dtmf_silence);
        }
        buffer.push_at(packet, arrival);

        while let Some(packet) = buffer.pop() {
            if !deliver(
                &Delivery {
                    audio: &incoming,
                    encoded: &encoded,
                    relay: &relay,
                    discards: &discards,
                    taps: &taps,
                },
                &mut decoding,
                &digits,
                &mut dtmf,
                &config,
                &stop,
                &packet,
            )
            .await
            {
                return;
            }
            if dtmf.in_progress().is_none() {
                dtmf_deadline = None;
            }
        }
    }
}

/// Hand one packet's audio to the application.
///
/// Returns whether the loop should keep running.
/// Score one arrival against the stream's statistics (RFC 3550 §6.4.1).
///
/// A telephone event's timestamp is the event's *start* (RFC 4733 §2.5.1.2) and not this
/// packet's sampling instant, so its transit grows per packet by design and would fabricate
/// jitter out of a keypress. It still counts for loss and sequence continuity.
async fn note_arrival(
    stats: &Arc<Mutex<StreamStats>>,
    packet: &Packet,
    config: &Config,
    arrival: u32,
) {
    let mut stats = stats.lock().await;
    if config.dtmf_payload_type == Some(packet.payload_type) {
        stats.on_untimed_packet(packet.sequence);
    } else {
        stats.on_packet(packet.sequence, packet.timestamp, arrival);
    }
}

/// The local clock in RTP timestamp units.
fn arrival_in_timestamp_units(started: tokio::time::Instant, clock_rate: u32) -> u32 {
    let elapsed = started.elapsed().as_secs_f64() * f64::from(clock_rate);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let units = elapsed as u64;
    u32::try_from(units & u64::from(u32::MAX)).unwrap_or(0)
}

/// One report interval, randomized as RFC 3550 §6.3.1 requires.
///
/// The computed interval is scaled by a factor drawn uniformly from [0.5, 1.5] and divided
/// by e − 3/2 ≈ 1.21828. The randomness is what keeps the participants' reports from
/// falling into lockstep — a fixed timer synchronises with any peer that computed the same
/// interval — and the division compensates so the mean stays at the configured value.
fn randomized_rtcp_interval(base: Duration, unit: f64) -> Duration {
    const COMPENSATION: f64 = std::f64::consts::E - 1.5;
    // A draw is a number in [0, 1); anything else — including NaN — collapses to the
    // middle of the range rather than panicking the report loop.
    let unit = if unit.is_finite() {
        unit.clamp(0.0, 1.0)
    } else {
        0.5
    };
    base.mul_f64((0.5 + unit) / COMPENSATION)
}

/// Send a report every interval, describing what we have sent and what we have heard.
///
/// A **sender** report when we have sent anything, a receiver report otherwise — RFC 3550 §6.4
/// draws that line, and it is not cosmetic. Only a sender report carries the NTP timestamp the
/// far end echoes back, so a session that only ever sent receiver reports could never be told
/// its own round-trip time.
#[allow(clippy::too_many_arguments)]
async fn rtcp_loop(
    socket: Arc<UdpSocket>,
    remote: Arc<Mutex<SocketAddr>>,
    rtcp_remote: Arc<Mutex<Option<SocketAddr>>>,
    interval: Duration,
    mode: sipx_sdp::RtcpMode,
    ssrc: u32,
    cname: String,
    stats: Arc<Mutex<StreamStats>>,
    outbound: Arc<Outbound>,
    feedback: Arc<Mutex<Feedback>>,
    srtp: Option<SrtpKeys>,
    stop: Arc<Stop>,
) {
    // Owned by this loop, like the RTP contexts: SRTCP keeps its own index, and one task sends.
    let mut protect = srtp_context(srtp.as_ref(), |keys| &keys.local);

    loop {
        // Drawn afresh every cycle: reusing one draw for the whole session would leave the
        // reports evenly spaced again, just at a different spacing.
        let wait = randomized_rtcp_interval(interval, rand::random::<f64>());
        tokio::select! {
            () = stop.wait() => return,
            () = tokio::time::sleep(wait) => {}
        }
        if stop.is_stopped() {
            return;
        }

        let sent_packets = outbound.packets.load(Ordering::Relaxed);
        let block = {
            let mut stats = stats.lock().await;
            // Asked of the counters rather than of a report, because `report_block()` closes the
            // reporting interval and RFC 3550 §6.4.1 bounds that interval by a report *packet*
            // going out. A tick that turns out to have nothing to say must leave the window for
            // the tick that does.
            if stats.extended_highest_sequence() == 0 && sent_packets == 0 {
                None
            } else {
                Some(stats.report_block())
            }
        };
        // Nothing has happened in either direction, so there is nothing to report on.
        let Some(block) = block else {
            continue;
        };
        let heard_anything = block.extended_highest_sequence != 0;

        // Echo the far end's last sender report and how long we have sat on it, so *it* can
        // measure the round trip. Without these two fields the exchange is one-way: we could
        // learn our own round-trip time and the far end never could.
        let block = if heard_anything {
            let echo = *feedback.lock().await;
            let mut block = block;
            block.last_sender_report = echo.last_sender_report;
            block.delay_since_last_sender_report = echo.received_at.map_or(0, |at| {
                // In units of 1/65536 of a second.
                let held = tokio::time::Instant::now().saturating_duration_since(at);
                u32::try_from((held.as_nanos() * 65_536) / 1_000_000_000).unwrap_or(u32::MAX)
            });
            vec![block]
        } else {
            Vec::new()
        };

        let report = if sent_packets > 0 {
            Rtcp::Sender(sipx_rtp::rtcp::SenderReport {
                ssrc,
                ntp_timestamp: sipx_rtp::quality::ntp_now(),
                rtp_timestamp: outbound.timestamp.load(Ordering::Relaxed),
                packet_count: u32::try_from(sent_packets).unwrap_or(u32::MAX),
                octet_count: u32::try_from(outbound.octets.load(Ordering::Relaxed))
                    .unwrap_or(u32::MAX),
                reports: block,
            })
        } else {
            // The first word after the header is the SSRC of the packet's *sender* — us — not
            // of the stream being described (RFC 3550 §6.4.2); the described stream is named
            // inside the block.
            Rtcp::Receiver(ReceiverReport {
                ssrc,
                reports: block,
            })
        };

        // Never a bare report: RFC 3550 §6.1 requires a compound of at least two packets
        // with an SDES CNAME in each.
        let datagram = Rtcp::encode_compound(&[report, Rtcp::Sdes(Sdes::cname(ssrc, &cname))]);
        // RFC 3711 §3.4. A report says who is talking to whom and how well — exactly the
        // metadata that encrypting the media was meant to withhold.
        let datagram = match protect.as_mut() {
            Some(context) => match context.protect_rtcp(&datagram) {
                Ok(protected) => Bytes::from(protected),
                Err(error) => {
                    // discard: the compound encoder above always emits the eight-byte header
                    // `protect_rtcp` requires, so this error branch is structurally unreachable.
                    tracing::warn!(%error, "dropping a report SRTCP could not protect");
                    continue;
                }
            },
            None => datagram,
        };

        // RTCP conventionally travels on the RTP port plus one (RFC 3550 §11) — unless ICE has
        // selected a pair for component 2, in which case the checked path is where the reports
        // go and the convention is only what got them there.
        let destination = *remote.lock().await;
        let rtcp_to = if mode == sipx_sdp::RtcpMode::Mux {
            destination
        } else {
            let selected = *rtcp_remote.lock().await;
            selected.unwrap_or_else(|| {
                SocketAddr::new(destination.ip(), destination.port().saturating_add(1))
            })
        };
        if socket.send_to(&datagram, rtcp_to).await.is_err() {
            return;
        }
    }
}

/// Take in what the far end sends back.
///
/// Two things come out of this. A **sender report** from the far end has to be remembered so
/// our next report can echo it, which is how the far end measures its round trip. A **report
/// block describing us** carries the far end's echo of *our* report, which is how we measure
/// ours.
async fn rtcp_receive_loop(
    socket: Arc<UdpSocket>,
    ssrc: u32,
    rtcp_observation: RtcpObservation,
    srtp: Option<SrtpKeys>,
    ice: Option<crate::ice::driver::Handle>,
    stop: Arc<Stop>,
    discards: Arc<DiscardMeters>,
) {
    let mut unprotect = srtp_context(srtp.as_ref(), |keys| &keys.remote);
    let mut datagram = vec![0u8; 2048];
    loop {
        let read = tokio::select! {
            () = stop.wait() => return,
            read = socket.recv_from(&mut datagram) => read,
        };
        if stop.is_stopped() {
            return;
        }
        let Ok((len, source)) = read else {
            return;
        };

        // The control port carries the same three protocols the media port does, because ICE
        // checks component 2 over it.
        let arrived = datagram.get(..len).unwrap_or(&[]);
        let Some(control) = demultiplex(arrived, source, ice::LocalBase(1), ice.as_ref()) else {
            continue;
        };

        process_rtcp(
            control,
            ssrc,
            &rtcp_observation,
            &mut unprotect,
            &discards,
            None,
        )
        .await;
    }
}

/// Authenticate, decode and apply one RTCP compound packet.
///
/// Called by either the adjacent control-port owner or the muxed media-port owner; there is never
/// more than one caller for a running session.
async fn process_rtcp(
    control: &[u8],
    ssrc: u32,
    rtcp_observation: &RtcpObservation,
    unprotect: &mut Option<sipx_rtp::SrtpContext>,
    discards: &DiscardMeters,
    browser_ingress: Option<&Arc<std::sync::Mutex<crate::browser::ComponentIngress>>>,
) {
    let bytes = Bytes::copy_from_slice(control);
    let bytes = match unprotect.as_mut() {
        Some(context) => match context.unprotect_rtcp(&bytes) {
            Ok(plain) => Bytes::from(plain),
            Err(error) => {
                discards
                    .srtcp_unprotect_failures
                    .fetch_add(1, Ordering::Relaxed);
                if let Some(ingress) = browser_ingress {
                    let mut ingress = crate::browser::lock_ingress(ingress);
                    match &error {
                        sipx_rtp::srtp::SrtpError::TooShort(_) => {
                            ingress.note_malformed(crate::browser::IngressClass::Srtcp);
                        }
                        sipx_rtp::srtp::SrtpError::ReplayedRtcp(_) => {
                            ingress.note_replay(crate::browser::IngressClass::Srtcp);
                        }
                        _ => {
                            ingress
                                .note_authentication_failure(crate::browser::IngressClass::Srtcp);
                        }
                    }
                }
                // discard: `srtcp_unprotect_failures` was incremented above; browser sessions
                // also classified the refusal into malformed, replay or authentication once.
                tracing::debug!(%error, "dropping a report that failed SRTCP");
                return;
            }
        },
        None => bytes,
    };
    // Malformed control input is a drop, not a session failure, on either socket shape.
    let Ok(packets) = Rtcp::decode_compound(&bytes) else {
        if let Some(ingress) = browser_ingress {
            crate::browser::lock_ingress(ingress)
                .note_malformed(crate::browser::IngressClass::Srtcp);
        }
        return;
    };
    if let Some(ingress) = browser_ingress {
        crate::browser::lock_ingress(ingress).note_srtcp_processed();
    }

    let arrival = tokio::time::Instant::now();
    for packet in packets {
        match packet {
            Rtcp::Sender(report) => {
                {
                    let mut held = rtcp_observation.feedback.lock().await;
                    held.last_sender_report = sipx_rtp::quality::middle_32(report.ntp_timestamp);
                    held.received_at = Some(arrival);
                }
                note_quality(
                    report.ssrc,
                    feedback_of(&report.reports, ssrc),
                    &rtcp_observation.feedback,
                    &rtcp_observation.quality_hook,
                    rtcp_observation.clock_rate,
                )
                .await;
            }
            Rtcp::Receiver(report) => {
                note_quality(
                    report.ssrc,
                    feedback_of(&report.reports, ssrc),
                    &rtcp_observation.feedback,
                    &rtcp_observation.quality_hook,
                    rtcp_observation.clock_rate,
                )
                .await;
            }
            Rtcp::Sdes(_) | Rtcp::Other { .. } => {}
        }
    }
}

/// The block in this report that describes *our* stream, if there is one.
///
/// A report may carry blocks about several sources. Taking the first regardless would have a
/// three-party call measuring its round trip to whoever happened to be listed first.
fn feedback_of(blocks: &[sipx_rtp::ReportBlock], ssrc: u32) -> Option<sipx_rtp::ReportBlock> {
    blocks.iter().find(|block| block.ssrc == ssrc).copied()
}

async fn note_quality(
    reporter_ssrc: u32,
    block: Option<sipx_rtp::ReportBlock>,
    feedback: &Arc<Mutex<Feedback>>,
    quality_hook: &QualityHookSlot,
    clock_rate: u32,
) {
    let Some(block) = block else {
        return;
    };
    let now = sipx_rtp::quality::middle_32(sipx_rtp::quality::ntp_now());
    let round_trip = sipx_rtp::quality::round_trip(
        now,
        block.last_sender_report,
        block.delay_since_last_sender_report,
    );
    if let Some(trip) = round_trip {
        feedback.lock().await.round_trip = Some(trip);
    }
    let Some(hook) = current_quality_hook(quality_hook) else {
        return;
    };
    let jitter = if clock_rate == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(f64::from(block.jitter) / f64::from(clock_rate))
    };
    let sample = RtcpQualitySample {
        reporter_ssrc,
        stream_ssrc: block.ssrc,
        loss: f64::from(block.fraction_lost) / 256.0,
        cumulative_lost: block.cumulative_lost,
        jitter,
        round_trip,
    };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| hook.observe(sample))).is_err() {
        tracing::warn!(
            reporter_ssrc,
            stream_ssrc = block.ssrc,
            "RTCP quality callback panicked; media reporting continues"
        );
    }
}

/// Where a received packet goes.
struct Delivery<'a> {
    audio: &'a mpsc::Sender<Vec<i16>>,
    encoded: &'a mpsc::Sender<Encoded>,
    relay: &'a AtomicBool,
    discards: &'a DiscardMeters,
    taps: &'a Taps,
}

async fn deliver(
    to: &Delivery<'_>,
    decoding: &mut Decoding,
    digits: &Keypresses,
    dtmf: &mut sipx_rtp::dtmf::Receiver,
    config: &Config,
    stop: &Stop,
    packet: &Packet,
) -> bool {
    // A telephone event is a keypress, not audio. It goes to the DTMF path and never to the
    // audio one — decoding a four-byte event payload as µ-law injects four garbage samples and
    // is heard as a click.
    if config.dtmf_payload_type == Some(packet.payload_type) {
        match DtmfEvent::decode(&packet.payload) {
            Some(event) => {
                for completed in dtmf
                    .push(packet.sequence, packet.timestamp, packet.marker, &event)
                    .into_iter()
                    .flatten()
                {
                    deliver_digit(to, digits, completed, config.clock_rate);
                }
            }
            None => dtmf.observe_non_event(packet.sequence),
        }
        return true;
    }

    // Relaying: hand the payload on exactly as it arrived. The bridge on the other side will
    // put it on its own wire with its own sequence and timestamp, which is right — the two
    // legs are separate RTP streams that happen to carry the same audio.
    if to.relay.load(Ordering::SeqCst) {
        if let Some(completed) = dtmf.finish_on_media(packet.sequence) {
            deliver_digit(to, digits, completed, config.clock_rate);
        }
        let encoded = Encoded {
            payload_type: packet.payload_type,
            payload: packet.payload.clone(),
        };
        return tokio::select! {
            () = stop.wait() => false,
            result = to.encoded.send(encoded) => result.is_ok(),
        };
    }

    // The negotiated payload type is the session's codec, whatever number it was given. Every
    // other number is looked up among the static types, and one that is neither is dropped
    // rather than decoded as the negotiated codec — decoding somebody else's format produces a
    // burst of noise, which is worse than a gap.
    if packet.payload_type != config.receive_wire_payload_type()
        && Codec::from_payload_type(packet.payload_type).is_none()
    {
        dtmf.observe_non_event(packet.sequence);
        to.discards
            .unknown_payload_type
            .fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            payload_type = packet.payload_type,
            "dropping a packet with an unknown payload type"
        );
        return true;
    }

    if let Some(completed) = dtmf.finish_on_media(packet.sequence) {
        deliver_digit(to, digits, completed, config.clock_rate);
    }

    // The stop signal is checked here too. This is the one await that can park indefinitely —
    // a full channel means the application has stopped reading — and a task parked here when
    // the call is hung up would hold its socket and its port for the life of the process.
    let Some(samples) = decoding.decode(&packet.payload) else {
        to.discards
            .opus_decode_failures
            .fetch_add(1, Ordering::Relaxed);
        // Audio the far end sent that never became a frame is exactly the seam's `Loss`
        // (`docs/specs/call-audio-seam.md` §7). Its span is this session's packetisation, which is
        // all a refused payload can truthfully say about how much audio it was carrying.
        to.taps
            .note_loss(AudioDirection::Inbound, config.samples_per_packet() as u64);
        return true;
    };

    // The inbound tap (`M-54`, `docs/specs/call-audio-seam.md` §3): the jitter buffer's output,
    // after decode, so a processor observes the played order rather than the arrival order. It
    // never awaits, so a stalled processor cannot hold up this loop or the packet behind it.
    to.taps
        .offer(AudioDirection::Inbound, config.audio_rate(), &samples);

    tokio::select! {
        () = stop.wait() => false,
        result = to.audio.send(samples) => result.is_ok(),
    }
}

fn deliver_digit(
    to: &Delivery<'_>,
    digits: &Keypresses,
    completed: sipx_rtp::dtmf::Completed,
    clock_rate: u32,
) {
    // The event's own duration, in its own clock units (RFC 4733 §2.3.5), converted to
    // wall-clock time here so every consumer of the channel gets a real duration without knowing
    // the clock rate itself.
    let millis = u64::from(completed.duration) * 1000 / u64::from(clock_rate.max(1));
    // A full channel means the application is not reading digits. Dropping is right: a keypress
    // delivered late is worse than one not delivered, since the application has already moved on.
    if digits
        .to
        .try_send((completed.digit, Duration::from_millis(millis)))
        .is_ok()
    {
        // Announced only once the digit is on its way to the application, and only when it got
        // there (`M-17`). This is the ordering that makes interrupt-on-digit safe to build a
        // `gather` on: a playback can only be cut short by a keypress the application can read.
        digits
            .arrivals
            .send_modify(|count| *count = count.wrapping_add(1));
    } else {
        to.discards
            .dtmf_delivery_failures
            .fetch_add(1, Ordering::Relaxed);
        tracing::debug!(
            digit = %completed.digit,
            "dropping a DTMF digit the application queue could not take"
        );
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
mod tests {
    use super::*;

    fn any() -> SocketAddr {
        "127.0.0.1:0".parse().expect("valid")
    }

    fn tone(samples: usize) -> Vec<i16> {
        (0..samples)
            .map(|i| {
                let phase = f64::from(u32::try_from(i).unwrap_or(0)) * 0.05;
                (phase.sin() * 8000.0).round() as i16
            })
            .collect()
    }

    /// RTCP conventionally travels on the RTP port plus one (RFC 3550 §11), so observing
    /// it takes a pair of adjacent ports: the OS picks the first, and the second either
    /// binds or the pair is retried.
    async fn adjacent_ports() -> (UdpSocket, UdpSocket) {
        for _ in 0..32 {
            let rtp = UdpSocket::bind(any()).await.expect("binds");
            let port = rtp.local_addr().expect("addr").port();
            let Some(next) = port.checked_add(1) else {
                continue;
            };
            let rtcp_addr: SocketAddr = format!("127.0.0.1:{next}").parse().expect("valid");
            if let Ok(rtcp) = UdpSocket::bind(rtcp_addr).await {
                return (rtp, rtcp);
            }
        }
        panic!("no adjacent port pair could be bound");
    }

    /// RFC 5761 mux has one ICE component even though [`MediaPort`] reserved an adjacent control
    /// socket for the no-mux fallback. This pins the public gathering entry point rather than the
    /// lower gatherer, so a call path cannot accidentally advertise a second receive owner.
    #[tokio::test]
    async fn mux_mode_gathers_exactly_one_ice_component() {
        let port = MediaPort::bind(any()).await.expect("binds adjacent ports");
        let credentials = sipx_sdp::ice::Credentials::new("mux1", "muxPassword0123456789AB")
            .expect("valid ICE credentials");
        let gathering = ice::Gathering::new(credentials, false);

        let mux = port
            .gather_with_rtcp_mode(&gathering, sipx_sdp::RtcpMode::Mux)
            .await;
        assert_eq!(mux.candidates().len(), 1);
        assert!(
            mux.candidates()
                .iter()
                .all(|candidate| candidate.component == ComponentId::RTP)
        );
        assert_eq!(mux.default_destination(ComponentId::RTCP), None);
    }

    /// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
    ///
    /// A bound on failure, not a window to measure in. Every clip below is well under a second,
    /// so this is more than an order of magnitude past the honest answer, and its only job is to
    /// stop a broken pipeline hanging the suite. The `record_until_idle(300ms)` these replaced
    /// was a measurement, and under load it measured the machine rather than the audio — see
    /// [`MediaSession::record_at_least`].
    ///
    /// The tests that still use `record_until_idle` do so deliberately: each asserts its
    /// recording is *empty*, so the fixed window is a window to look in rather than a deadline
    /// to beat, and a loaded machine can only make them pass. Waiting by count for samples that
    /// must never arrive would be a ten-second sleep apiece.
    const DELIVERY_BOUND: Duration = Duration::from_secs(10);

    async fn pair(codec: Codec) -> (MediaSession, MediaSession) {
        // Each side needs the other's port, so bind one first and point the second at it.
        let placeholder: SocketAddr = "127.0.0.1:1".parse().expect("valid");
        let left = MediaSession::start(any(), Config::new(placeholder, codec))
            .await
            .expect("binds");
        let right = MediaSession::start(any(), Config::new(left.local_addr(), codec))
            .await
            .expect("binds");
        (left, right)
    }

    /// `MediaSession` is the ownership boundary for every socket worker it starts. Returning
    /// from shutdown with an empty registry is the happens-before used by callers that report
    /// zero post-drain work; no wall-clock grace period stands in for a join.
    #[tokio::test]
    async fn ordinary_shutdown_joins_every_owned_worker() {
        let placeholder: SocketAddr = "127.0.0.1:1".parse().expect("valid");
        let session = MediaSession::start(any(), Config::new(placeholder, Codec::Pcmu))
            .await
            .expect("session starts");

        assert!(
            session.owned_task_count().await >= 5,
            "send, receive, playback and both separate-RTCP workers are owned"
        );
        session.shutdown().await;
        assert_eq!(session.owned_task_count().await, 0);
    }

    /// A shutdown future can itself be cancelled by an outer lifecycle deadline. The handle it
    /// was joining must remain in the session so the owner can retry and still prove reaping.
    #[tokio::test]
    async fn cancelled_shutdown_keeps_the_in_flight_worker_owned() {
        let placeholder: SocketAddr = "127.0.0.1:1".parse().expect("valid");
        let session = Arc::new(
            MediaSession::start(any(), Config::new(placeholder, Codec::Pcmu))
                .await
                .expect("session starts"),
        );
        let (worker_started_tx, worker_started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        session.owners.lock().await.push(tokio::spawn(async move {
            let _ = worker_started_tx.send(());
            let _ = release_rx.await;
        }));
        worker_started_rx.await.expect("test worker starts");

        let shutdown_session = Arc::clone(&session);
        let shutdown = tokio::spawn(async move { shutdown_session.shutdown().await });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if session.owners.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await // bound on failure: ownership-lock acquisition has no timing semantics.
        .expect("shutdown begins joining");
        shutdown.abort();
        let _ = shutdown.await;

        assert!(session.owned_task_count().await >= 1);
        release_tx.send(()).expect("test worker remains owned");
        session.shutdown().await;
        assert_eq!(session.owned_task_count().await, 0);
    }

    /// Reconfiguration installs a new generation before joining the old one. Cancellation in
    /// that join must leave the old generation attached to the replacement so retry can finish
    /// the same drain and release the shared socket deterministically.
    #[tokio::test]
    async fn cancelled_reconfigure_retains_the_old_generation_for_retry() {
        let placeholder: SocketAddr = "127.0.0.1:1".parse().expect("valid");
        let config = Config::new(placeholder, Codec::Pcmu);
        let session = Arc::new(Mutex::new(
            MediaSession::start(any(), config.clone())
                .await
                .expect("session starts"),
        ));
        let (stop_seen_tx, stop_seen_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        {
            let mut session = session.lock().await;
            let old_stop = Arc::clone(&session.stop);
            session.owners.get_mut().push(tokio::spawn(async move {
                old_stop.wait().await;
                let _ = stop_seen_tx.send(());
                let _ = release_rx.await;
            }));
        }

        let reconfiguring_session = Arc::clone(&session);
        let retry_config = config.clone();
        let reconfiguring = tokio::spawn(async move {
            reconfiguring_session
                .lock()
                .await
                .reconfigure(retry_config)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), stop_seen_rx)
            .await // bound on failure: the old generation's stop signal has no timing semantics.
            .expect("old generation observes stop")
            .expect("stop observer remains alive");
        reconfiguring.abort();
        let _ = reconfiguring.await;

        let local_addr = {
            let mut session = session.lock().await;
            assert_eq!(
                session.retired.get_mut().len(),
                1,
                "the cancelled join retained its old generation"
            );
            release_tx.send(()).expect("old generation remains owned");
            assert!(session.reconfigure(config).await.expect("retry succeeds"));
            assert_eq!(session.retired.get_mut().len(), 0);
            session.shutdown().await;
            session.local_addr()
        };
        let session = Arc::try_unwrap(session)
            .expect("the test owns the replacement")
            .into_inner();
        drop(session);
        let rebound = UdpSocket::bind(local_addr)
            .await
            .expect("retry joined every old socket worker");
        drop(rebound);
    }

    /// M-71: hold/resume and negotiated payload changes rebuild the media generation. An
    /// unfinished event belongs to the stopped receiver and must not time out into the fresh
    /// generation's application queue.
    #[tokio::test]
    async fn reconfiguration_does_not_leak_an_incomplete_digit_between_generations() {
        let raw = UdpSocket::bind(any()).await.expect("raw peer binds");
        let mut config = Config::new(raw.local_addr().expect("raw peer address"), Codec::Pcmu);
        config.dtmf_payload_type = Some(96);
        config.jitter_depth = 1;
        config.jitter_max_depth = None;
        let mut session = MediaSession::start(any(), config.clone())
            .await
            .expect("session starts");

        let mut start = Packet::new(
            96,
            1,
            1000,
            7,
            DtmfEvent::new(Digit::Number(7), 160).encode(),
        );
        start.marker = true;
        raw.send_to(&start.encode(), session.local_addr())
            .await
            .expect("event start sends");
        let deadline = tokio::time::Instant::now() + DELIVERY_BOUND;
        while session.packets_received() == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the old generation did not accept the event start"
            );
            tokio::task::yield_now().await;
        }

        assert!(session.reconfigure(config).await.expect("reconfigures"));
        assert!(
            tokio::time::timeout(Duration::from_millis(300), session.recv_digit())
                .await
                .is_err(),
            "the stopped generation's timeout cannot enter the replacement queue"
        );

        let mut complete = Packet::new(
            96,
            1,
            5000,
            7,
            DtmfEvent {
                digit: Digit::Number(8),
                end: true,
                volume: 10,
                duration: 800,
            }
            .encode(),
        );
        complete.marker = true;
        raw.send_to(&complete.encode(), session.local_addr())
            .await
            .expect("replacement event sends");
        assert_eq!(
            tokio::time::timeout(DELIVERY_BOUND, session.recv_digit())
                .await
                .expect("replacement digit is bounded"),
            Some((Digit::Number(8), Duration::from_millis(100)))
        );
    }

    /// The failing-first test for this story.
    #[tokio::test]
    async fn audio_played_into_a_session_arrives_at_the_far_end() {
        let (left, right) = pair(Codec::Pcmu).await;
        let source = tone(1600); // 200 ms

        // `right` speaks; `left` listens. `left` does not know `right`'s address until a
        // packet arrives, which is exactly what symmetric RTP is for.
        right.play(&source, 160).await;
        let recorded = left.record_at_least(source.len(), DELIVERY_BOUND).await;

        assert_eq!(recorded.len(), source.len(), "every packet arrived");

        // G.711 is lossy, so the samples cannot be compared directly — but the codec is
        // idempotent, so encoding the source and encoding what came back must agree exactly.
        assert_eq!(
            g711::ulaw_encode_all(&source),
            g711::ulaw_encode_all(&recorded),
            "the audio that arrived is the audio that was sent"
        );
    }

    /// M-43 / `linear-pcm.md` §3: playback converts both depth and rate before codec encoding.
    /// The decoded output is asserted so an implementation that merely accepts the format cannot
    /// pass while still putting byte-depth garbage on the wire.
    #[tokio::test]
    async fn pcm_playback_converts_unsigned_eight_and_signed_sixteen_bit_sources() {
        let (left, right) = pair(Codec::Pcmu).await;
        let eight = sipx_audio::Pcm::new(
            sipx_audio::PcmFormat::new(8_000, sipx_audio::PcmEncoding::Unsigned8).expect("format"),
            sipx_audio::PcmSamples::Unsigned8(vec![128; 160]),
        )
        .expect("samples");
        assert!(right.play_pcm(&eight).await.expect("converts"));
        let silent = left.record_at_least(160, DELIVERY_BOUND).await;
        assert_eq!(silent.len(), 160);
        assert!(silent.iter().all(|sample| sample.abs() <= 4));

        let source = tone(320);
        let sixteen = sipx_audio::Pcm::new(
            sipx_audio::PcmFormat::new(16_000, sipx_audio::PcmEncoding::Signed16).expect("format"),
            sipx_audio::PcmSamples::Signed16(source),
        )
        .expect("samples");
        assert!(right.play_pcm(&sixteen).await.expect("resamples"));
        let downsampled = left.record_at_least(160, DELIVERY_BOUND).await;
        assert_eq!(
            downsampled.len(),
            160,
            "16 kHz becomes the same duration at 8 kHz"
        );
        assert!(downsampled.iter().any(|sample| sample.abs() > 1_000));
    }

    /// M-43 / `linear-pcm.md` §3: capture owns a continuous resampler and emits the caller's
    /// chosen rate instead of exposing the negotiated codec clock as an application assumption.
    #[tokio::test]
    async fn pcm_capture_resamples_received_audio_to_the_callers_rate() {
        let (left, right) = pair(Codec::Pcmu).await;
        let source = tone(320);
        let format =
            sipx_audio::PcmFormat::new(16_000, sipx_audio::PcmEncoding::Signed16).expect("format");
        let mut capture = left.capture(format).expect("capture format");
        right.play(&source, 160).await;
        let pcm = capture.record_at_least(639, DELIVERY_BOUND).await;
        assert_eq!(pcm.format(), format);
        assert_eq!(pcm.samples().len(), 639);
    }

    /// Mute, from the receiving side (`M-18`): the far end gets every packet it would have got,
    /// and decodes silence out of all of them. Asserting the count as well as the content is the
    /// point — it is what distinguishes the decision that was made from the one that was not.
    #[tokio::test]
    async fn a_muted_session_sends_silence_rather_than_stopping() {
        let (left, right) = pair(Codec::Pcmu).await;
        let source = tone(800); // five packets

        right.set_muted(true);
        right.play(&source, 160).await;
        let recorded = left.record_at_least(source.len(), DELIVERY_BOUND).await;

        assert_eq!(recorded.len(), source.len(), "the stream did not stop");
        assert!(
            recorded.iter().all(|sample| *sample == 0),
            "a muted session put audio on the wire"
        );
        // RFC 3550 §6.4.1: what we say we sent is what arrived, and the far end saw no gap in the
        // sequence space to score as loss.
        assert_eq!(right.packets_sent(), 5);
        assert_eq!(left.packets_received(), 5);
        assert_eq!(left.quality().await.cumulative_lost, 0);
    }

    /// The gate opens again, on the same session — no renegotiation is involved at this layer or
    /// any other.
    #[tokio::test]
    async fn unmuting_a_session_restores_the_audio() {
        let (left, right) = pair(Codec::Pcmu).await;
        let source = tone(480);

        right.set_muted(true);
        right.play(&source, 160).await;
        // Drained by count, not by an idle window, and that matters more here than anywhere
        // else in this module (`X-28`): a short first recording leaves the rest of the muted
        // silence in the channel, where the second recording picks it up and compares it
        // against the source. The failure then reads as "unmuting did not restore the audio",
        // which is a lie about the code under test.
        let muted = left.record_at_least(source.len(), DELIVERY_BOUND).await;
        assert!(muted.iter().all(|sample| *sample == 0));

        assert!(right.set_muted(false), "the gate was closed before this");
        right.play(&source, 160).await;
        let after = left.record_at_least(source.len(), DELIVERY_BOUND).await;

        assert_eq!(
            g711::ulaw_encode_all(&source),
            g711::ulaw_encode_all(&after),
            "the audio that arrived after unmuting is the audio that was sent"
        );
    }

    /// A relayed payload is opaque — a bridge's bytes in whatever the other leg negotiated — so a
    /// muted session substitutes its own silence for it rather than passing it on.
    #[tokio::test]
    async fn a_muted_session_does_not_relay_a_payload_either() {
        let (left, right) = pair(Codec::Pcmu).await;

        right.set_muted(true);
        for _ in 0..3 {
            assert!(
                right
                    .send_encoded(Encoded {
                        payload_type: 0,
                        payload: Bytes::from(g711::ulaw_encode_all(&tone(160))),
                    })
                    .await
            );
        }
        let recorded = left.record_at_least(480, DELIVERY_BOUND).await;

        assert_eq!(recorded.len(), 480, "the stream did not stop");
        assert!(
            recorded.iter().all(|sample| *sample == 0),
            "a muted session forwarded somebody else's audio"
        );
    }

    /// A keypress is not audio. It is generated by this endpoint on purpose, and a mute that
    /// swallowed it would leave a muted caller unable to answer an IVR.
    #[tokio::test]
    async fn a_muted_session_still_sends_a_keypress() {
        let (left, right) = pair(Codec::Pcmu).await;
        right.play(&tone(320), 160).await;
        let _ = left.record_at_least(320, DELIVERY_BOUND).await;

        right.set_muted(true);
        right
            .send_digit(
                Digit::from_char('9').expect("a digit"),
                Duration::from_millis(100),
            )
            .await;

        let (digit, _duration) = tokio::time::timeout(Duration::from_secs(2), left.recv_digit())
            .await
            .expect("no timeout")
            .expect("a digit arrives");
        assert_eq!(digit.as_char(), '9');
    }

    /// Reception is not part of the gate: a muted session hears everything it would have heard.
    #[tokio::test]
    async fn a_muted_session_still_receives() {
        let (left, right) = pair(Codec::Pcmu).await;
        let source = tone(480);

        left.set_muted(true);
        right.play(&source, 160).await;
        let recorded = left.record_at_least(source.len(), DELIVERY_BOUND).await;

        assert_eq!(
            g711::ulaw_encode_all(&source),
            g711::ulaw_encode_all(&recorded),
            "muting this side must not touch what it hears"
        );
    }

    /// A queue has to say what it does when it is full. Refusing, with the handle resolving to
    /// say so, beats the two alternatives: silently dropping the clip leaves the caller waiting
    /// on audio that is never coming, and waiting for room parks a call's control path on a
    /// backlog it may never work through.
    #[tokio::test]
    async fn a_full_playback_queue_refuses_rather_than_dropping_or_waiting() {
        let (left, _right) = pair(Codec::Pcmu).await;

        // One clip reaches the head of the queue and starts feeding, so the queue proper only
        // has room for `Playback::QUEUE_DEPTH` behind it.
        let mut started = Vec::new();
        for _ in 0..=Playback::QUEUE_DEPTH + 1 {
            started.push(left.start_playback(tone(160 * 200), Interrupt::Never));
        }

        let refused = started
            .iter()
            .filter(|playback| playback.end() == Some(PlaybackEnd::Refused))
            .count();
        assert!(
            refused > 0,
            "a queue {} deep must refuse the clip past its depth rather than \
             growing without bound",
            Playback::QUEUE_DEPTH
        );
        assert!(
            started
                .first()
                .is_some_and(|playback| playback.end().is_none()),
            "and it must refuse the newest clip, not the one already playing"
        );
    }

    /// A playback started on a session that has already stopped resolves at once rather than
    /// hanging. The `Call` this reports through has no other way to learn it will never play.
    #[tokio::test]
    async fn a_playback_started_on_a_stopped_session_resolves_at_once() {
        let (left, _right) = pair(Codec::Pcmu).await;
        left.stop();
        // The playback task takes the stop signal on its next poll; until then a clip is accepted
        // and then resolved by the task itself.
        let playback = left.start_playback(tone(320), Interrupt::Never);
        let end = tokio::time::timeout(Duration::from_secs(2), playback.finished())
            .await
            .expect("a stopped session must not leave a playback hanging");
        assert_eq!(end, PlaybackEnd::SessionEnded);
    }

    /// `play` is cancel-on-drop, and has to stay that way: callers wrap it in a `timeout` to cap
    /// how long a clip may run, and feeding the clip from a task of its own would have quietly
    /// turned that into a timeout that returns while the audio plays on.
    #[tokio::test]
    async fn abandoning_a_play_stops_the_clip_rather_than_leaving_it_running() {
        let (left, right) = pair(Codec::Pcmu).await;
        // Far longer than the timeout, so a clip that survives it is unmistakable.
        let long = tone(160 * 250);

        let capped = tokio::time::timeout(Duration::from_millis(60), left.play(&long, 160)).await;
        assert!(capped.is_err(), "the timeout is what ends this play");

        let before = left.packets_sent();
        // A definition of silence: how long a hole has to be before "the clip stopped" is true.
        // Both assertions are negative — no more than the bound went out, and the far end did not
        // hear the whole clip — so load lengthens the window and can only make them fail, and
        // there is no arrival to poll for (`X-44`).
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            left.packets_sent() - before <= Playback::STOP_BOUND_PACKETS,
            "abandoning the play must stop the clip, not leave it going"
        );
        assert!(
            right.packets_received() < 250,
            "the far end heard the whole clip despite the timeout"
        );
    }

    /// A frame that belongs to no playback is never discarded — [`MediaSession::send`] is the
    /// path a bridge and a conference mixer use, and nothing there has a handle to stop.
    #[test]
    fn only_a_stopped_playback_s_frames_are_discarded() {
        let stop = Arc::new(Stop::default());
        let untagged = Frame::Audio {
            samples: vec![0; 160],
            playback: None,
        };
        let tagged = Frame::Audio {
            samples: vec![0; 160],
            playback: Some(Arc::clone(&stop)),
        };
        assert!(!discarded(&untagged));
        assert!(!discarded(&tagged));
        stop.stop();
        assert!(discarded(&tagged));
        assert!(!discarded(&untagged));
    }

    #[tokio::test]
    async fn audio_flows_in_both_directions_at_once() {
        let (left, right) = pair(Codec::Pcmu).await;
        let from_left = tone(800);
        let from_right: Vec<i16> = tone(800).iter().map(|s| -s).collect();

        // Left must learn right's address, which it does from right's first packet — so wait for
        // that packet to have *arrived* rather than for a window to pass (`X-44`). The primer is
        // one packet's worth, and recording it here is also what makes the count below exact:
        // what `left` hears afterwards is everything except this primer.
        right.play(&from_right[..160], 160).await;
        assert_eq!(
            left.record_at_least(160, DELIVERY_BOUND).await.len(),
            160,
            "left never heard right's primer, so it cannot have learned right's address"
        );

        let (left_recorded, right_recorded) = tokio::join!(
            async {
                left.play(&from_left, 160).await;
                // What `left` hears is what `right` still has to play: everything after the
                // 160-sample primer above.
                left.record_at_least(from_right.len() - 160, DELIVERY_BOUND)
                    .await
            },
            async {
                right.play(&from_right[160..], 160).await;
                right.record_at_least(from_left.len(), DELIVERY_BOUND).await
            }
        );

        assert!(!left_recorded.is_empty(), "left heard nothing");
        assert!(!right_recorded.is_empty(), "right heard nothing");
    }

    #[tokio::test]
    async fn a_pcma_session_carries_a_law() {
        let (left, right) = pair(Codec::Pcma).await;
        let source = tone(480);
        right.play(&source, 160).await;
        let recorded = left.record_at_least(source.len(), DELIVERY_BOUND).await;
        assert_eq!(
            g711::alaw_encode_all(&source),
            g711::alaw_encode_all(&recorded)
        );
    }

    /// M-43: dynamic L16 uses the negotiated clock and payload assignment, while the encoded
    /// samples remain signed network-order PCM and therefore arrive bit-for-bit unchanged.
    #[tokio::test]
    async fn a_dynamic_eight_kilohertz_l16_session_carries_linear_pcm() {
        let placeholder: SocketAddr = "127.0.0.1:1".parse().expect("valid");
        let mut left_config = Config::new(placeholder, Codec::L16);
        left_config.clock_rate = 8_000;
        left_config.payload_type = Some(96);
        left_config.receive_payload_type = Some(96);
        let left = MediaSession::start(any(), left_config)
            .await
            .expect("binds");

        let mut right_config = Config::new(left.local_addr(), Codec::L16);
        right_config.clock_rate = 8_000;
        right_config.payload_type = Some(96);
        right_config.receive_payload_type = Some(96);
        let right = MediaSession::start(any(), right_config)
            .await
            .expect("binds");

        let source = tone(480);
        right.play(&source, 160).await;
        let recorded = left.record_at_least(source.len(), DELIVERY_BOUND).await;
        assert_eq!(recorded, source);
        assert_eq!(left.clock_rate(), 8_000);
        assert_eq!(left.wire_payload_type(), 96);
    }

    /// Media ports attract stray traffic — STUN probes, port scans. None of it should end a
    /// call.
    #[tokio::test]
    async fn junk_on_the_media_port_does_not_stop_the_session() {
        let (left, right) = pair(Codec::Pcmu).await;

        let junk = UdpSocket::bind(any()).await.expect("binds");
        for _ in 0..5 {
            junk.send_to(b"not an RTP packet", left.local_addr())
                .await
                .expect("sends");
        }

        let source = tone(320);
        right.play(&source, 160).await;
        let recorded = left.record_at_least(source.len(), DELIVERY_BOUND).await;
        assert_eq!(
            g711::ulaw_encode_all(&source),
            g711::ulaw_encode_all(&recorded),
            "the session survived the junk"
        );
    }

    /// Symmetric RTP: `left` was configured with a useless address and still answers, because
    /// the observed source replaced it.
    #[tokio::test]
    async fn media_returns_to_where_it_came_from_not_where_the_sdp_said() {
        let (left, right) = pair(Codec::Pcmu).await;

        // Left latches the source address off right's first packet, so wait for that packet to
        // have arrived rather than for a window to pass (`X-44`). A fixed window here was racing
        // the same pipeline `X-28` measured — two 20 ms pacers and a jitter buffer entitled to
        // grow — and losing it produced a reply sent to 127.0.0.1:1 and an empty recording.
        right.play(&tone(320), 160).await;
        assert_eq!(
            left.record_at_least(320, DELIVERY_BOUND).await.len(),
            320,
            "left never heard right, so it cannot have latched right's address"
        );

        let reply = tone(320);
        left.play(&reply, 160).await;
        let heard = right.record_at_least(reply.len(), DELIVERY_BOUND).await;

        assert!(
            !heard.is_empty(),
            "left was configured with 127.0.0.1:1 and must have learned the real address"
        );
    }

    /// ICE and symmetric RTP must not race to write the same destination. `on_socket` supplies
    /// `symmetric = false` exactly when an ICE driver owns the session; a valid ordinary RTP
    /// packet may establish the SSRC, but cannot replace the nominated pair.
    #[tokio::test]
    async fn an_ice_owned_destination_cannot_be_replaced_by_ordinary_rtp() {
        let nominated: SocketAddr = "127.0.0.1:40000".parse().expect("valid");
        let unsolicited: SocketAddr = "127.0.0.1:50000".parse().expect("valid");
        let remote = Arc::new(Mutex::new(nominated));
        let stats = Arc::new(Mutex::new(StreamStats::new(1)));
        let discards = DiscardMeters::default();
        let packet = Packet::new(0, 1, 160, 7, Bytes::from(vec![0xff; 160]));
        let mut stream = None;

        assert!(
            accept_source(
                &mut stream,
                &packet,
                unsolicited,
                &remote,
                &stats,
                false,
                &discards,
            )
            .await,
            "the nominated path may still receive the stream"
        );
        assert_eq!(
            *remote.lock().await,
            nominated,
            "RFC 8445 nomination, not the first ordinary RTP source, owns the destination"
        );
    }

    #[tokio::test]
    async fn packets_are_counted_on_both_sides() {
        let (left, right) = pair(Codec::Pcmu).await;
        right.play(&tone(800), 160).await;
        // Counted, not timed (`X-28`). The recording is discarded, but waiting for it is what
        // gives all five packets time to land — so an idle window that closed early made
        // `packets_received` short and blamed the counters.
        let _ = left.record_at_least(800, DELIVERY_BOUND).await;

        assert_eq!(right.packets_sent(), 5);
        assert_eq!(left.packets_received(), 5);
    }

    /// A short final chunk is padded so every packet is the same size, which is what a far-end
    /// jitter buffer expects.
    #[tokio::test]
    async fn a_partial_final_frame_is_padded_rather_than_sent_short() {
        let (left, right) = pair(Codec::Pcmu).await;
        right.play(&tone(400), 160).await; // 2.5 packets
        let recorded = left.record_at_least(480, DELIVERY_BOUND).await;
        assert_eq!(recorded.len(), 480, "three whole packets");
        assert_eq!(&recorded[400..], &[0i16; 80], "padded with silence");
    }

    /// The acceptance test for M-7: a keypress crosses a real media session and arrives once.
    #[tokio::test]
    async fn a_dtmf_digit_survives_a_media_session() {
        let (left, right) = pair(Codec::Pcmu).await;

        // Establish the stream so `left` knows where `right` is.
        right.play(&tone(320), 160).await;
        assert_eq!(left.record_at_least(320, DELIVERY_BOUND).await.len(), 320);

        right
            .send_digit(
                Digit::from_char('5').expect("a digit"),
                Duration::from_millis(100),
            )
            .await;

        let (digit, duration) = tokio::time::timeout(Duration::from_secs(2), left.recv_digit())
            .await
            .expect("no timeout")
            .expect("a digit arrives");
        assert_eq!(digit.as_char(), '5');
        assert!(
            duration >= Duration::from_millis(80) && duration <= Duration::from_millis(140),
            "the reported duration must reflect how long the digit was held: {duration:?}"
        );

        // Exactly once, however many packets carried it.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), left.recv_digit())
                .await
                .is_err(),
            "one keypress must not be reported twice"
        );
    }

    /// A whole sequence, as an application collecting a PIN would see it.
    #[tokio::test]
    async fn a_sequence_of_keypresses_arrives_in_order() {
        let (left, right) = pair(Codec::Pcmu).await;
        right.play(&tone(160), 160).await;
        let _ = left.record_at_least(160, DELIVERY_BOUND).await;

        for c in "1234".chars() {
            right
                .send_digit(
                    Digit::from_char(c).expect("a digit"),
                    Duration::from_millis(80),
                )
                .await;
        }

        let collected = left.collect_digits(FIRST_DIGIT_BOUND, DIGIT_GAP).await;
        assert_eq!(collected, "1234");
    }

    /// How long a collection here waits for the **first** digit before calling it lost (`M-34`).
    ///
    /// A bound on failure, like [`DELIVERY_BOUND`], and for the same reason: how long a caller
    /// takes to press the first key is a property of the caller and of the machine carrying the
    /// call, never of the digits.
    const FIRST_DIGIT_BOUND: Duration = Duration::from_secs(10);

    /// How long a silence means the caller has stopped dialling, for the tests here (`M-34`).
    ///
    /// A definition of silence, so it is set past any scheduling delay rather than close to the
    /// spacing the digits actually arrive with — fifty missed packet intervals, which is `X-28`'s
    /// treatment of the windows that genuinely have to stay wall-clock.
    const DIGIT_GAP: Duration = Duration::from_secs(1);

    /// A first digit the caller is slow to press is still collected (`M-34`).
    ///
    /// The defect this pins: `collect_digits` spent one window on both "how long to wait for the
    /// first digit" and "how long a gap means the digits ended", so a caller who took longer than
    /// that window to press anything collected **nothing at all** — not a short sequence, an empty
    /// one, because the loop ended before its first iteration. That is the same one-window shape
    /// that made `sipx answer` write a valid WAV with zero samples (`X-40`), one layer down, and
    /// the reproduction is the same: delay the first thing the far end sends.
    #[tokio::test]
    async fn a_first_digit_that_arrives_late_is_still_collected() {
        let (left, right) = pair(Codec::Pcmu).await;

        // Establish the stream first, so the only variable below is *when* the digits start.
        right.play(&tone(160), 160).await;
        let _ = left.record_at_least(160, DELIVERY_BOUND).await;

        // Longer than the gap, so a collection that spends its gap on the first digit has already
        // given up by the time the caller presses anything. Load can only push the digits later,
        // which makes the pre-split failure more certain rather than less.
        let late = Duration::from_secs(2);

        let (collected, ()) =
            tokio::join!(left.collect_digits(FIRST_DIGIT_BOUND, DIGIT_GAP), async {
                tokio::time::sleep(late).await;
                for c in "1234".chars() {
                    right
                        .send_digit(
                            Digit::from_char(c).expect("a digit"),
                            Duration::from_millis(80),
                        )
                        .await;
                }
            });

        assert_eq!(
            collected, "1234",
            "a caller slow to press the first key has not finished dialling"
        );
    }

    /// Digits that never arrive still end the collection, and end it empty (`M-34`).
    ///
    /// The other half of the split: separating the two bounds must not turn "nobody pressed
    /// anything" into a wait that never ends, and it must not invent digits to return.
    #[tokio::test]
    async fn a_collection_with_no_digits_at_all_ends_empty() {
        let (left, right) = pair(Codec::Pcmu).await;

        right.play(&tone(160), 160).await;
        let _ = left.record_at_least(160, DELIVERY_BOUND).await;

        let collected = tokio::time::timeout(
            DELIVERY_BOUND,
            left.collect_digits(Duration::from_millis(300), Duration::from_millis(300)),
        )
        .await
        .expect("the collection is bounded when no digit ever arrives");

        assert_eq!(collected, "", "audio alone is not a keypress");
    }

    /// DTMF must not become audio and audio must not become digits.
    #[tokio::test]
    async fn keypresses_and_audio_stay_on_their_own_paths() {
        let (left, right) = pair(Codec::Pcmu).await;

        right.play(&tone(320), 160).await;
        let audio = left.record_at_least(320, DELIVERY_BOUND).await;
        assert_eq!(audio.len(), 320, "audio arrived");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), left.recv_digit())
                .await
                .is_err(),
            "audio must not be reported as a keypress"
        );

        right
            .send_digit(
                Digit::from_char('#').expect("a digit"),
                Duration::from_millis(80),
            )
            .await;
        let (digit, _duration) = tokio::time::timeout(Duration::from_secs(2), left.recv_digit())
            .await
            .expect("no timeout")
            .expect("a digit");
        assert_eq!(digit.as_char(), '#');

        let after = left.record_until_idle(Duration::from_millis(200)).await;
        assert!(
            after.is_empty(),
            "a keypress must not become audio samples: {after:?}"
        );
    }

    /// With nothing negotiated for `telephone-event`, a digit cannot be sent — and guessing a
    /// payload type would put keypresses on whatever the far end uses that number for.
    #[tokio::test]
    async fn a_digit_is_not_sent_when_no_payload_type_was_negotiated() {
        let listener = UdpSocket::bind(any()).await.expect("binds");
        let mut config = Config::new(listener.local_addr().expect("addr"), Codec::Pcmu);
        config.dtmf_payload_type = None;
        let session = MediaSession::start(any(), config).await.expect("binds");

        session
            .send_digit(
                Digit::from_char('7').expect("a digit"),
                Duration::from_millis(80),
            )
            .await;

        let mut datagram = vec![0u8; 2048];
        assert!(
            tokio::time::timeout(
                Duration::from_millis(300),
                listener.recv_from(&mut datagram)
            )
            .await
            .is_err(),
            "nothing should go on the wire"
        );
    }

    /// Statistics are readable mid-call, and count what the receive path actually saw.
    /// Numbers that only appear when a call ends cannot be used to do anything about it.
    #[tokio::test]
    async fn a_session_reports_the_loss_it_saw() {
        let (left, right) = pair(Codec::Pcmu).await;

        // Ten packets from `right`, of which two are dropped in flight by sending them from a
        // socket the far end will ignore — simpler: send nine of ten sequence numbers by
        // hand, so the gap is exact.
        let raw = UdpSocket::bind(any()).await.expect("binds");
        for sequence in 1u16..=10 {
            if sequence == 4 || sequence == 8 {
                continue;
            }
            let packet = Packet::new(
                0,
                sequence,
                u32::from(sequence) * 160,
                0xAB,
                Bytes::from(vec![0xFFu8; 160]),
            );
            raw.send_to(&packet.encode(), left.local_addr())
                .await
                .expect("sends");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Wait for all eight to have been through the receive path, rather than sleeping 150 ms
        // and assuming they have (`X-29`). The counts below are exact, so a packet still in
        // flight does not degrade the answer — it changes it, and reports loss that was never
        // injected. The bound is on failure, not a window to measure in.
        //
        // The precondition this leans on, since it is not obvious: `packets_received()` reaching 8
        // implies the statistics have seen all 8 only because nothing suspends between
        // `received.fetch_add` (`:2258`) and `note_arrival`'s lock (`:2301-2313`) — an uncontended
        // `Mutex::lock().await` on a `current_thread` runtime does not yield. Move these tests to a
        // multi-thread runtime, or add an await in that gap, and the counter can lead the
        // statistics: inserting a 20 ms sleep between the two fails this test with
        // `extended_highest_sequence  left: 9  right: 10`.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while left.packets_received() != 8 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the eight hand-sent packets never reached the receive path"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let block = left.stats().await;
        assert_eq!(
            block.extended_highest_sequence, 10,
            "sequence 10 was the highest"
        );
        assert_eq!(block.cumulative_lost, 2, "four and eight never arrived");
        assert!(block.fraction_lost > 0, "and the interval shows loss");

        drop(right);
    }

    #[test]
    fn the_packet_size_follows_the_clock_rate_and_interval() {
        let config = Config::new("127.0.0.1:1".parse().expect("valid"), Codec::Pcmu);
        assert_eq!(config.samples_per_packet(), 160, "8 kHz at 20 ms");

        let mut ten_ms = config.clone();
        ten_ms.packet_duration = Duration::from_millis(10);
        assert_eq!(ten_ms.samples_per_packet(), 80);
    }

    /// RFC 3551 §4.5.2 preserves a historical error on purpose: G.722 samples audio at
    /// 16 kHz, but its RTP timestamp clock advances at 8000 per second. A stack that
    /// advances timestamps at the sample rate produces audio the far end plays at the
    /// wrong speed, and nothing else catches it — the packets parse, the codec decodes,
    /// only the timeline is double.
    #[test]
    fn g722_advances_rtp_timestamps_at_8000_while_the_audio_is_16_khz() {
        let codec = Codec::from_payload_type(9)
            .expect("G.722 is negotiable as static payload type 9 (RFC 3551 §6)");
        assert_eq!(codec.clock_rate(), 8_000, "the RTP clock stays at 8000");
        let config = Config::new("127.0.0.1:1".parse().expect("valid"), codec);
        assert_eq!(
            config.clock_rate, 8_000,
            "and the session keeps the wire clock"
        );
        assert_eq!(
            config.samples_per_packet(),
            320,
            "while a 20 ms packet carries 16 kHz audio: 320 samples"
        );
        assert_eq!(
            config.clock_units_per_packet(),
            160,
            "which advance the RTP timestamp by only 160"
        );
        assert_eq!(config.audio_rate(), 16_000, "and the sample rate is 16 kHz");

        // The send clock itself: a full 320-sample G.722 frame moves the timestamp by 160.
        // Advancing by the sample count is the wrong-speed-audio defect this test pins.
        let mut clock = SendClock::new();
        let before = clock.timestamp;
        let mut encoding = Encoding::for_codec(codec, 1).expect("G.722 always constructs");
        let (packet, advance) = clock
            .audio(
                &mut encoding,
                9,
                0x5155,
                &vec![0i16; 320],
                codec.samples_per_clock_unit(),
            )
            .expect("G.722 encodes a full frame");
        assert_eq!(packet.payload.len(), 160, "one octet per two samples");
        assert_eq!(advance, 160, "the timestamp advance is in clock units");
        clock.advance(advance);
        assert_eq!(clock.timestamp, before.wrapping_add(160));
    }

    #[tokio::test]
    async fn zero_packet_duration_is_rejected_before_binding_or_spawning() {
        let reservation = UdpSocket::bind(any()).await.expect("reserves a port");
        let address = reservation.local_addr().expect("has an address");
        drop(reservation);

        let mut config = Config::new("127.0.0.1:9".parse().expect("valid"), Codec::Pcmu);
        config.packet_duration = Duration::ZERO;
        let error = MediaSession::start(address, config)
            .await
            .expect_err("zero cannot pace a worker");
        assert!(matches!(
            error,
            StartError::Setup(SetupError::PacketDurationTooShort(Duration::ZERO))
        ));

        let rebound = UdpSocket::bind(address)
            .await
            .expect("rejected setup left no socket behind");
        drop(rebound);
    }

    #[tokio::test]
    async fn zero_rtcp_interval_is_rejected_before_binding_or_spawning() {
        let reservation = UdpSocket::bind(any()).await.expect("reserves a port");
        let address = reservation.local_addr().expect("has an address");
        drop(reservation);

        let mut config = Config::new("127.0.0.1:9".parse().expect("valid"), Codec::Pcmu);
        config.rtcp_interval = Some(Duration::ZERO);
        let error = MediaSession::start(address, config)
            .await
            .expect_err("zero cannot schedule reports");
        assert!(matches!(
            error,
            StartError::Setup(SetupError::RtcpIntervalTooShort(Duration::ZERO))
        ));

        let rebound = UdpSocket::bind(address)
            .await
            .expect("rejected setup left no socket behind");
        drop(rebound);
    }

    #[test]
    fn rtcp_mux_refuses_payload_types_that_collide_with_rtcp() {
        let mut config = Config::new(any(), Codec::Pcmu);
        config.rtcp_mode = sipx_sdp::RtcpMode::Mux;
        config.payload_type = Some(72);
        assert!(matches!(
            config.validate(),
            Err(SetupError::RtcpMuxPayloadCollision(72))
        ));

        config.payload_type = Some(0);
        config.dtmf_payload_type = Some(95);
        assert!(matches!(
            config.validate(),
            Err(SetupError::RtcpMuxPayloadCollision(95))
        ));
    }

    #[tokio::test]
    async fn one_millisecond_media_and_report_intervals_keep_running() {
        let peer = UdpSocket::bind(any()).await.expect("binds peer");
        let mut config = Config::new(peer.local_addr().expect("has an address"), Codec::Pcmu);
        config.packet_duration = Duration::from_millis(1);
        config.rtcp_interval = Some(Duration::from_millis(1));
        let samples = config.samples_per_packet();
        let session = MediaSession::start(any(), config)
            .await
            .expect("minimum intervals are valid");

        assert!(session.send(vec![0; samples]).await);
        let mut datagram = vec![0u8; 2048];
        tokio::time::timeout(Duration::from_secs(1), peer.recv_from(&mut datagram))
            .await
            .expect("the pacing worker remains alive")
            .expect("receives a packet");
        session.stop();
    }

    #[cfg(feature = "opus")]
    #[test]
    fn refused_opus_encoder_has_no_fallback_pipeline() {
        let error =
            Encoding::for_codec(Codec::Opus, 3).expect_err("Opus carries at most two channels");
        assert!(matches!(
            error,
            SetupError::Codec {
                codec: Codec::Opus,
                direction: CodecDirection::Encoder,
                ..
            }
        ));
    }

    #[cfg(feature = "opus")]
    #[test]
    fn refused_opus_decoder_has_no_fallback_pipeline() {
        let error =
            Decoding::for_codec(Codec::Opus, 3).expect_err("Opus carries at most two channels");
        assert!(matches!(
            error,
            SetupError::Codec {
                codec: Codec::Opus,
                direction: CodecDirection::Decoder,
                ..
            }
        ));
    }

    /// The RTP timestamp must advance by the samples actually sent. Advancing by the
    /// configured packet size instead builds a timeline at the wrong rate, and the far end
    /// plays the call with a gap between every packet.
    #[tokio::test]
    async fn the_timestamp_follows_the_frame_actually_sent() {
        let listener = UdpSocket::bind(any()).await.expect("binds");
        let listen_addr = listener.local_addr().expect("has an address");
        let session = MediaSession::start(any(), Config::new(listen_addr, Codec::Pcmu))
            .await
            .expect("binds");

        // Half-sized frames on a config that says 160.
        for _ in 0..3 {
            session.send(vec![0i16; 80]).await;
        }

        let mut stamps = Vec::new();
        let mut datagram = vec![0u8; 2048];
        for _ in 0..3 {
            let (len, _) =
                tokio::time::timeout(Duration::from_secs(2), listener.recv_from(&mut datagram))
                    .await
                    .expect("no timeout")
                    .expect("receives");
            let packet =
                Packet::decode(&Bytes::copy_from_slice(&datagram[..len])).expect("a valid packet");
            stamps.push(packet.timestamp);
        }

        assert_eq!(
            stamps[1].wrapping_sub(stamps[0]),
            80,
            "80 samples sent must advance the clock by 80, not by the configured 160"
        );
        assert_eq!(stamps[2].wrapping_sub(stamps[1]), 80);
    }

    /// A dynamic payload number means only what SDP assigned it. A number that names neither
    /// the negotiated codec nor a known static codec is loss, and that loss must be observable.
    #[tokio::test]
    async fn an_unknown_payload_type_is_dropped_rather_than_decoded_as_audio() {
        let raw = UdpSocket::bind(any()).await.expect("binds");
        let mut config = Config::new(raw.local_addr().expect("address"), Codec::Pcmu);
        config.jitter_depth = 1;
        config.jitter_max_depth = None;
        config.dtmf_payload_type = None;
        let session = MediaSession::start(any(), config).await.expect("starts");

        // First establish the SSRC, then offer an unassigned dynamic payload on that same stream.
        let valid = Packet::new(0, 1, 160, 7, Bytes::from(vec![0xFF; 160]));
        raw.send_to(&valid.encode(), session.local_addr())
            .await
            .expect("sends");
        let heard = session.record_at_least(160, DELIVERY_BOUND).await;
        assert_eq!(heard.len(), 160);

        let unknown = Packet::new(96, 2, 320, 7, Bytes::from_static(&[1, 2, 3, 4]));
        raw.send_to(&unknown.encode(), session.local_addr())
            .await
            .expect("sends");

        let deadline = tokio::time::Instant::now() + DELIVERY_BOUND;
        while session.discard_counts().unknown_payload_type == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the unknown payload never reached the discard site"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let after = session.record_until_idle(Duration::from_millis(200)).await;
        assert!(
            after.is_empty(),
            "an unknown payload must not become audio samples: {after:?}"
        );
        assert_eq!(session.discard_counts().unknown_payload_type, 1);
    }

    /// D9: `telephone-event` has no static number. SDP selected 96 here, so the same valid event
    /// bytes on 101 are an unknown payload and cannot become a digit.
    #[tokio::test]
    async fn only_the_negotiated_non_101_payload_can_create_a_digit() {
        let (audio, _audio_rx) = mpsc::channel(1);
        let (encoded, _encoded_rx) = mpsc::channel(1);
        let relay = AtomicBool::new(false);
        let discards = Arc::new(DiscardMeters::default());
        let taps = Taps::new(Arc::clone(&discards));
        let delivery = Delivery {
            audio: &audio,
            encoded: &encoded,
            relay: &relay,
            discards: &discards,
            taps: &taps,
        };
        let (digits_tx, mut digits_rx) = mpsc::channel(32);
        let digits = Keypresses {
            to: digits_tx,
            arrivals: Arc::new(watch::Sender::new(0)),
        };
        let mut decoding = Decoding::for_codec(Codec::Pcmu, 1).expect("codec");
        let mut receiver = sipx_rtp::dtmf::Receiver::new();
        let mut config = Config::new(any(), Codec::Pcmu);
        config.dtmf_payload_type = Some(96);
        let stop = Stop::default();
        let event = DtmfEvent {
            digit: Digit::Number(6),
            end: true,
            volume: 10,
            duration: 800,
        };

        let wrong = Packet::new(101, 1, 1000, 7, event.encode());
        assert!(
            deliver(
                &delivery,
                &mut decoding,
                &digits,
                &mut receiver,
                &config,
                &stop,
                &wrong,
            )
            .await
        );
        assert!(digits_rx.try_recv().is_err(), "PT 101 was not negotiated");
        assert_eq!(discards.snapshot().unknown_payload_type, 1);

        let mut negotiated = Packet::new(96, 2, 2000, 7, event.encode());
        negotiated.marker = true;
        assert!(
            deliver(
                &delivery,
                &mut decoding,
                &digits,
                &mut receiver,
                &config,
                &stop,
                &negotiated,
            )
            .await
        );
        assert_eq!(
            digits_rx.try_recv(),
            Ok((Digit::Number(6), Duration::from_millis(100)))
        );
    }

    /// D7 at the worker boundary: losing every final report cannot leave a digit stuck forever.
    /// The media timer fires into the pure RTP receiver, which retains the wire duration.
    #[tokio::test]
    async fn media_silence_completes_an_event_whose_final_reports_were_lost() {
        let raw = UdpSocket::bind(any()).await.expect("raw peer binds");
        let mut config = Config::new(raw.local_addr().expect("raw peer address"), Codec::Pcmu);
        config.dtmf_payload_type = Some(96);
        config.jitter_depth = 1;
        config.jitter_max_depth = None;
        let session = MediaSession::start(any(), config)
            .await
            .expect("session starts");

        for (sequence, duration, marker) in [(1, 160, true), (2, 320, false)] {
            let mut packet = Packet::new(
                96,
                sequence,
                3000,
                7,
                DtmfEvent::new(Digit::Number(3), duration).encode(),
            );
            packet.marker = marker;
            raw.send_to(&packet.encode(), session.local_addr())
                .await
                .expect("event report sends");
        }

        assert_eq!(
            tokio::time::timeout(DELIVERY_BOUND, session.recv_digit())
                .await
                .expect("silence completion is bounded"),
            Some((Digit::Number(3), Duration::from_millis(40)))
        );

        let late_end = Packet::new(
            96,
            3,
            3000,
            7,
            DtmfEvent {
                digit: Digit::Number(3),
                end: true,
                volume: 10,
                duration: 320,
            }
            .encode(),
        );
        raw.send_to(&late_end.encode(), session.local_addr())
            .await
            .expect("late end sends");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), session.recv_digit())
                .await
                .is_err(),
            "a late final report cannot duplicate the timed-out digit"
        );
    }

    /// S-36 / RFC 3264 §6.1: each description assigns the dynamic number its author receives.
    /// Therefore a session sends with the peer's answer number while accepting the different
    /// number from its own offer; collapsing both directions into one number loses one stream.
    #[tokio::test]
    async fn asymmetric_dynamic_payload_types_are_honoured_in_both_directions() {
        let raw = UdpSocket::bind(any()).await.expect("binds");
        let mut config = Config::new(raw.local_addr().expect("address"), Codec::Pcmu);
        config.payload_type = Some(96);
        config.receive_payload_type = Some(111);
        config.jitter_depth = 1;
        config.jitter_max_depth = None;
        config.dtmf_payload_type = None;
        let session = MediaSession::start(any(), config).await.expect("starts");

        session.send(vec![0; 160]).await;
        let mut datagram = vec![0; 2048];
        let (len, _) = tokio::time::timeout(DELIVERY_BOUND, raw.recv_from(&mut datagram))
            .await
            .expect("outbound packet arrives")
            .expect("receives");
        let outbound = Packet::decode(&Bytes::copy_from_slice(&datagram[..len])).expect("RTP");
        assert_eq!(outbound.payload_type, 96, "send with the peer's number");

        let inbound = Packet::new(111, 1, 160, 7, Bytes::from(vec![0xFF; 160]));
        raw.send_to(&inbound.encode(), session.local_addr())
            .await
            .expect("sends inbound packet");
        let heard = session.record_at_least(160, DELIVERY_BOUND).await;
        assert_eq!(heard.len(), 160, "receive with our number");
    }

    /// M-32's failing-first witness: unlike every other media discard in the original census,
    /// this loss had neither a trace nor a number. Fill the application queue, offer one more
    /// complete keypress, and assert the loss itself rather than a timeout in a consumer.
    #[tokio::test]
    async fn a_dtmf_digit_refused_by_the_application_queue_is_counted() {
        let (audio, _audio_rx) = mpsc::channel(1);
        let (encoded, _encoded_rx) = mpsc::channel(1);
        let relay = AtomicBool::new(false);
        let discards = Arc::new(DiscardMeters::default());
        let taps = Taps::new(Arc::clone(&discards));
        let delivery = Delivery {
            audio: &audio,
            encoded: &encoded,
            relay: &relay,
            discards: &discards,
            taps: &taps,
        };
        let (digits_tx, _digits_rx) = mpsc::channel(32);
        let arrivals = Arc::new(watch::Sender::new(0));
        let digits = Keypresses {
            to: digits_tx,
            arrivals,
        };
        let mut decoding = Decoding::for_codec(Codec::Pcmu, 1).expect("codec");
        let mut receiver = sipx_rtp::dtmf::Receiver::new();
        let config = Config::new(any(), Codec::Pcmu);
        let stop = Stop::default();

        for sequence in 0u16..33 {
            let event = DtmfEvent {
                digit: Digit::Number(5),
                end: true,
                volume: 10,
                duration: 160,
            };
            let packet = Packet::new(
                dtmf::DEFAULT_PAYLOAD_TYPE,
                sequence,
                u32::from(sequence) * 160,
                1,
                event.encode(),
            );
            assert!(
                deliver(
                    &delivery,
                    &mut decoding,
                    &digits,
                    &mut receiver,
                    &config,
                    &stop,
                    &packet,
                )
                .await
            );
        }

        assert_eq!(discards.snapshot().dtmf_delivery_failures, 1);
    }

    /// Once a stream is established, a packet from a different synchronisation source is
    /// dropped. Without this, one forged packet with a high sequence number advances the
    /// jitter buffer past every genuine packet still to come, and the call goes silent.
    #[tokio::test]
    async fn a_packet_from_another_source_cannot_silence_the_stream() {
        let (left, right) = pair(Codec::Pcmu).await;

        right.play(&tone(320), 160).await;
        assert_eq!(left.record_at_least(320, DELIVERY_BOUND).await.len(), 320);

        // A forged packet: valid RTP, different SSRC, sequence number far in the future.
        let forged = Packet::new(0, 60_000, 0, 0xBAD0_BAD0, Bytes::from(vec![0xFFu8; 160]));
        let attacker = UdpSocket::bind(any()).await.expect("binds");
        attacker
            .send_to(&forged.encode(), left.local_addr())
            .await
            .expect("sends");
        // Order on the observable effect, not on elapsed wall time: under load a fixed sleep can
        // let the genuine stream resume before this packet reaches the discard site.
        let deadline = tokio::time::Instant::now() + DELIVERY_BOUND;
        while left.discard_counts().foreign_ssrc == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the foreign packet never reached the discard site"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // The genuine stream still gets through.
        let more = tone(320);
        right.play(&more, 160).await;
        let heard = left.record_at_least(more.len(), DELIVERY_BOUND).await;
        assert_eq!(
            g711::ulaw_encode_all(&more),
            g711::ulaw_encode_all(&heard),
            "the forged packet must not have poisoned the buffer"
        );
    }

    /// RFC 3550 §6.4.2: a report's first field is the SSRC of the *reporter*, and §8.1
    /// requires that to be the SSRC the reporter's own RTP carries; each report block names
    /// the source it describes. A report saying "SSRC 0 heard SSRC 0" is unusable.
    #[tokio::test]
    async fn rtcp_reports_name_both_parties_by_their_real_ssrcs() {
        let (peer_media, peer_control) = adjacent_ports().await;
        let mut config = Config::new(peer_media.local_addr().expect("addr"), Codec::Pcmu);
        config.rtcp_interval = Some(Duration::from_millis(200));
        let session = MediaSession::start(any(), config).await.expect("binds");

        // The peer speaks first, so the session latches its address and its
        // synchronisation source.
        for sequence in 1u16..=5 {
            let packet = Packet::new(
                0,
                sequence,
                u32::from(sequence) * 160,
                0x5EED_CAFE,
                Bytes::from(vec![0xFFu8; 160]),
            );
            peer_media
                .send_to(&packet.encode(), session.local_addr())
                .await
                .expect("sends");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // And the session speaks, so the SSRC its own RTP carries is observable.
        session.play(&tone(320), 160).await;
        let mut datagram = vec![0u8; 2048];
        let (len, _) =
            tokio::time::timeout(Duration::from_secs(2), peer_media.recv_from(&mut datagram))
                .await
                .expect("no timeout")
                .expect("receives");
        let rtp_ssrc = Packet::decode(&Bytes::copy_from_slice(&datagram[..len]))
            .expect("a valid packet")
            .ssrc;

        let (len, _) = tokio::time::timeout(
            Duration::from_secs(3),
            peer_control.recv_from(&mut datagram),
        )
        .await
        .expect("a report arrives")
        .expect("receives");
        let packets =
            Rtcp::decode_compound(&Bytes::copy_from_slice(&datagram[..len])).expect("valid RTCP");
        let (reporter, blocks) = match &packets[0] {
            Rtcp::Sender(report) => (report.ssrc, report.reports.clone()),
            Rtcp::Receiver(report) => (report.ssrc, report.reports.clone()),
            other => panic!("a report must lead, got {other:?}"),
        };
        assert_eq!(
            reporter, rtp_ssrc,
            "the reporter names itself by the SSRC its RTP carries"
        );
        assert_eq!(blocks[0].ssrc, 0x5EED_CAFE, "the block names the far end");
    }

    /// RFC 3550 §6.1: every RTCP packet travels in a compound of at least two, the first a
    /// report, and each compound carries an SDES CNAME. The CNAME is what lets a receiver
    /// tie streams to one participant across an SSRC change, so it must be stable.
    #[tokio::test]
    async fn rtcp_goes_out_compound_with_a_stable_cname() {
        let (peer_media, peer_control) = adjacent_ports().await;
        let mut config = Config::new(peer_media.local_addr().expect("addr"), Codec::Pcmu);
        config.rtcp_interval = Some(Duration::from_millis(150));
        let session = MediaSession::start(any(), config).await.expect("binds");

        // The peer speaks so there is something to report on.
        for sequence in 1u16..=5 {
            let packet = Packet::new(
                0,
                sequence,
                u32::from(sequence) * 160,
                0xABCD,
                Bytes::from(vec![0xFFu8; 160]),
            );
            peer_media
                .send_to(&packet.encode(), session.local_addr())
                .await
                .expect("sends");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let mut cnames = Vec::new();
        let mut datagram = vec![0u8; 2048];
        for _ in 0..2 {
            let (len, _) = tokio::time::timeout(
                Duration::from_secs(3),
                peer_control.recv_from(&mut datagram),
            )
            .await
            .expect("a report arrives")
            .expect("receives");
            let packets = Rtcp::decode_compound(&Bytes::copy_from_slice(&datagram[..len]))
                .expect("valid RTCP");
            assert!(packets.len() >= 2, "a lone report is not a compound");
            assert!(
                matches!(packets[0], Rtcp::Sender(_) | Rtcp::Receiver(_)),
                "a report leads the compound"
            );
            let sdes = packets
                .iter()
                .find_map(|packet| match packet {
                    Rtcp::Sdes(sdes) => Some(sdes),
                    _ => None,
                })
                .expect("an SDES in every compound");
            let cname = sdes.chunks[0]
                .items
                .iter()
                .find(|item| item.kind == sipx_rtp::rtcp::SDES_CNAME)
                .expect("a CNAME item");
            assert!(!cname.value.is_empty());
            cnames.push(cname.value.clone());
        }
        assert_eq!(cnames[0], cnames[1], "the CNAME does not change mid-call");
    }

    /// RFC 4733 §2.5.1.2: every packet of one telephone event carries the *same* timestamp
    /// — the event's start — while being sent one packetisation interval apart, so its
    /// transit grows by one interval per packet by design. RFC 3550 §6.4.1 defines jitter
    /// over packets whose timestamps track sampling instants; a keypress must not fabricate
    /// jitter, but it must still count for loss and sequence continuity.
    #[tokio::test]
    async fn a_keypress_does_not_register_as_jitter() {
        let raw = UdpSocket::bind(any()).await.expect("binds");
        let session = MediaSession::start(
            any(),
            Config::new(raw.local_addr().expect("addr"), Codec::Pcmu),
        )
        .await
        .expect("binds");

        // One long keypress: same timestamp throughout, spaced a packet interval apart,
        // with one packet lost in flight.
        for sequence in 1u16..=10 {
            if sequence == 4 {
                continue;
            }
            let duration = sequence * 160;
            let event = DtmfEvent::new(Digit::Number(5), duration);
            let packet = Packet::new(101, sequence, 5000, 0xAB, event.encode());
            raw.send_to(&packet.encode(), session.local_addr())
                .await
                .expect("sends");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // As in `a_session_reports_the_loss_it_saw`: wait for the nine to arrive rather than
        // sleeping 100 ms and assuming they have (`X-29`). `extended_highest_sequence` and
        // `cumulative_lost` are asserted exactly, so a straggler reports loss nobody injected.
        // Same precondition as that test, and it is the same fragility: the counter only implies
        // the statistics because nothing suspends between `received.fetch_add` (`:2258`) and
        // `note_arrival`'s lock (`:2301-2313`) on a `current_thread` runtime.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while session.packets_received() != 9 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the nine hand-sent keypress packets never reached the receive path"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        let block = session.stats().await;
        assert_eq!(block.jitter, 0, "a keypress is not network jitter");
        assert_eq!(
            block.extended_highest_sequence, 10,
            "the events still advance the sequence accounting"
        );
        assert_eq!(block.cumulative_lost, 1, "and still count for loss");
    }

    /// RFC 3550 §6.4: a participant that sent data during the interval sends a *sender*
    /// report. The RR it would otherwise send carries no NTP/RTP pair and no counts, and
    /// without those the far end can never compute round-trip time or line the clocks up.
    #[tokio::test]
    async fn an_active_sender_reports_with_a_sender_report() {
        let (peer_media, peer_control) = adjacent_ports().await;
        let mut config = Config::new(peer_media.local_addr().expect("addr"), Codec::Pcmu);
        config.rtcp_interval = Some(Duration::from_millis(200));
        let session = MediaSession::start(any(), config).await.expect("binds");

        // Both directions are busy: the peer speaks once, the session streams audio.
        let packet = Packet::new(0, 1, 160, 0xABCD, Bytes::from(vec![0xFFu8; 160]));
        peer_media
            .send_to(&packet.encode(), session.local_addr())
            .await
            .expect("sends");
        session.play(&tone(1600), 160).await;

        // The first interval may legitimately elapse before the first packet leaves — an
        // RR is correct then — so wait for the first report from an interval in which
        // data went out. A stack that never sends one fails here by timing out.
        let mut datagram = vec![0u8; 2048];
        let report = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (len, _) = peer_control
                    .recv_from(&mut datagram)
                    .await
                    .expect("receives");
                let packets = Rtcp::decode_compound(&Bytes::copy_from_slice(&datagram[..len]))
                    .expect("valid RTCP");
                match &packets[0] {
                    Rtcp::Sender(report) => return report.clone(),
                    Rtcp::Receiver(_) => {}
                    other => panic!("a report must lead the compound, got {other:?}"),
                }
            }
        })
        .await
        .expect("an active sender must send a sender report");

        assert!(report.packet_count >= 1, "it counts the packets it sent");
        assert!(report.octet_count >= 160, "and the payload bytes");
        // RFC 3550 §4: the high word is seconds since 1900 — around 3.97 billion now.
        let seconds = report.ntp_timestamp >> 32;
        assert!(
            (3_700_000_000..4_294_967_295).contains(&seconds),
            "the NTP word counts seconds since 1900: {seconds}"
        );
        assert_eq!(report.reports.len(), 1, "reception is appended as a block");
    }

    fn peer_sender_report(ntp_timestamp: u64) -> Bytes {
        Rtcp::encode_compound(&[
            Rtcp::Sender(sipx_rtp::rtcp::SenderReport {
                ssrc: 0x5EED_CAFE,
                ntp_timestamp,
                rtp_timestamp: 160,
                packet_count: 1,
                octet_count: 160,
                reports: Vec::new(),
            }),
            Rtcp::Sdes(Sdes::cname(0x5EED_CAFE, "peer@example.invalid")),
        ])
    }

    async fn wait_for_rtcp_echo(socket: &UdpSocket, expected: u32) -> SocketAddr {
        let mut datagram = vec![0u8; 2048];
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (len, from) = socket.recv_from(&mut datagram).await.expect("receives");
                let packets = Rtcp::decode_compound(&Bytes::copy_from_slice(&datagram[..len]))
                    .expect("valid RTCP");
                let echoed = packets.iter().any(|packet| match packet {
                    Rtcp::Sender(report) => report
                        .reports
                        .iter()
                        .any(|block| block.last_sender_report == expected),
                    Rtcp::Receiver(report) => report
                        .reports
                        .iter()
                        .any(|block| block.last_sender_report == expected),
                    Rtcp::Sdes(_) | Rtcp::Other { .. } => false,
                });
                if echoed {
                    return from;
                }
            }
        })
        .await
        .expect("the peer sender report is processed and echoed")
    }

    /// `MUX-PKT-1`, failing first: RTCP arriving on the negotiated RTP port reaches RTCP state,
    /// and the response leaves from and returns to that same port.
    #[tokio::test]
    async fn muxed_rtcp_arriving_on_the_rtp_port_is_processed_not_dropped() {
        let peer = UdpSocket::bind(any()).await.expect("binds peer");
        let mut config = Config::new(peer.local_addr().expect("peer address"), Codec::Pcmu);
        config.rtcp_mode = sipx_sdp::RtcpMode::Mux;
        config.rtcp_interval = Some(Duration::from_millis(20));
        let session = MediaSession::start(any(), config).await.expect("starts");

        let rtp = Packet::new(0, 1, 160, 0x5EED_CAFE, Bytes::from(vec![0xFF; 160]));
        peer.send_to(&rtp.encode(), session.local_addr())
            .await
            .expect("sends RTP");
        let ntp = 0x0123_4567_89AB_CDEF;
        peer.send_to(&peer_sender_report(ntp), session.local_addr())
            .await
            .expect("sends RTCP on the RTP port");

        let report_source = wait_for_rtcp_echo(&peer, sipx_rtp::quality::middle_32(ntp)).await;
        assert_eq!(
            report_source,
            session.local_addr(),
            "muxed RTCP leaves from the exact socket address advertised for RTP"
        );
        session.stop();
    }

    /// `MUX-PKT-2`: omission of mux leaves the established adjacent control-port path live.
    #[tokio::test]
    async fn separate_rtcp_still_uses_and_processes_the_control_port() {
        let (peer_media, peer_control) = adjacent_ports().await;
        let mut config = Config::new(peer_media.local_addr().expect("peer address"), Codec::Pcmu);
        config.rtcp_mode = sipx_sdp::RtcpMode::Separate;
        config.rtcp_interval = Some(Duration::from_millis(20));
        let session = MediaSession::start(any(), config).await.expect("starts");

        let rtp = Packet::new(0, 1, 160, 0x5EED_CAFE, Bytes::from(vec![0xFF; 160]));
        peer_media
            .send_to(&rtp.encode(), session.local_addr())
            .await
            .expect("sends RTP");
        let control_addr = session
            .rtcp_socket
            .as_ref()
            .expect("the session bound its adjacent control port")
            .local_addr()
            .expect("control address");
        let ntp = 0x89AB_CDEF_0123_4567;
        peer_control
            .send_to(&peer_sender_report(ntp), control_addr)
            .await
            .expect("sends RTCP on the control port");

        let _ = wait_for_rtcp_echo(&peer_control, sipx_rtp::quality::middle_32(ntp)).await;
        session.stop();
    }

    /// RFC 3550 §6.3.1: each report interval is drawn uniformly from [0.5, 1.5] of the
    /// computed value and divided by e − 3/2 ≈ 1.21828. Without the randomness every
    /// participant that computed the same interval reports at the same instant, forever.
    #[test]
    fn the_rtcp_interval_is_randomised_over_the_rfc_range() {
        let base = Duration::from_secs(5);
        let compensation = std::f64::consts::E - 1.5;

        let low = randomized_rtcp_interval(base, 0.0);
        let high = randomized_rtcp_interval(base, 1.0);
        assert!((low.as_secs_f64() - 2.5 / compensation).abs() < 1e-9);
        assert!((high.as_secs_f64() - 7.5 / compensation).abs() < 1e-9);
        assert!(
            low < base && base < high,
            "the range straddles the configured value: {low:?}..{high:?}"
        );

        // A draw outside the unit range must not panic the media path or leave the range.
        assert!(randomized_rtcp_interval(base, f64::NAN) >= low);
        assert!(randomized_rtcp_interval(base, 7.0) <= high);
    }

    #[test]
    fn codecs_map_to_their_static_payload_types() {
        assert_eq!(Codec::Pcmu.payload_type(), 0);
        assert_eq!(Codec::Pcma.payload_type(), 8);
        assert_eq!(Codec::G722.payload_type(), 9);
        assert_eq!(Codec::L16.payload_type(), 11);
        assert_eq!(Codec::from_payload_type(0), Some(Codec::Pcmu));
        assert_eq!(Codec::from_payload_type(8), Some(Codec::Pcma));
        // M-44 reversed X-26's pinned absence: 9 is G.722 and G.722 is ours now.
        assert_eq!(Codec::from_payload_type(9), Some(Codec::G722));
        assert_eq!(Codec::from_payload_type(11), Some(Codec::L16));
        assert_eq!(Codec::from_payload_type(10), None, "stereo L16 is not ours");
    }
}
