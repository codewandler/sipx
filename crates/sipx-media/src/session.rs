//! A media session: RTP sockets, paced sending, and buffered receiving.
//!
//! Two decisions shape this.
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

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::g711;
use sipx_rtp::dtmf::{self, Digit, Event as DtmfEvent};
use sipx_rtp::rtcp::{ReceiverReport, Rtcp, Sdes, StreamStats};
use sipx_rtp::{JitterBuffer, Packet};
use tokio::net::UdpSocket;
use tokio::sync::{Mutex, mpsc};

/// Which G.711 flavour a session carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// µ-law, payload type 0.
    Pcmu,
    /// A-law, payload type 8.
    Pcma,
    /// Opus (RFC 6716), on whatever dynamic payload type was negotiated.
    ///
    /// Unlike the G.711 pair this carries *state*: an Opus encoder and decoder each hold a
    /// model of the signal they have seen, which is how the codec achieves what it does and why
    /// it cannot be a pure function of one frame. The state lives in the send and receive
    /// loops, one each, so nothing is shared and nothing is locked.
    #[cfg(feature = "opus")]
    Opus,
}

impl Codec {
    /// The static payload type.
    #[must_use]
    pub fn payload_type(self) -> u8 {
        match self {
            Self::Pcmu => 0,
            Self::Pcma => 8,
            // Opus has no static type — RFC 7587 §7 assigns none — so 111 is convention and
            // nothing more. What goes on the wire is whatever SDP negotiated, which
            // [`Config::payload_type`] carries.
            #[cfg(feature = "opus")]
            Self::Opus => 111,
        }
    }

    /// The RTP clock rate, which is not always the sample rate.
    ///
    /// RFC 7587 §7 fixes Opus's RTP clock at 48000 whatever the audio is sampled at. A stack
    /// that used the sample rate instead produces timestamps the far end reads at the wrong
    /// speed.
    #[must_use]
    pub fn clock_rate(self) -> u32 {
        match self {
            Self::Pcmu | Self::Pcma => 8_000,
            #[cfg(feature = "opus")]
            Self::Opus => sipx_audio::opus::CLOCK_RATE,
        }
    }

    /// The codec for a payload type, if it is one we carry.
    #[must_use]
    pub fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self::Pcmu),
            8 => Some(Self::Pcma),
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
            // Unreachable: an Opus session encodes through [`Encoding`], which holds the state
            // this signature has nowhere to put.
            #[cfg(feature = "opus")]
            Self::Opus => Vec::new(),
        }
    }

    fn decode(self, payload: &[u8]) -> Vec<i16> {
        match self {
            Self::Pcmu => g711::ulaw_decode_all(payload),
            Self::Pcma => g711::alaw_decode_all(payload),
            #[cfg(feature = "opus")]
            Self::Opus => Vec::new(),
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
    #[cfg(feature = "opus")]
    Opus(Box<sipx_audio::opus::Encoder>),
}

impl Encoding {
    fn for_codec(codec: Codec, channels: usize) -> Self {
        match codec {
            #[cfg(feature = "opus")]
            Codec::Opus => match sipx_audio::opus::Encoder::new(channels) {
                Ok(encoder) => Self::Opus(Box::new(encoder)),
                Err(error) => {
                    // Falling back would send µ-law on the payload type the far end agreed was
                    // Opus, which it would decode as noise. Sending nothing is the honest
                    // failure, and it is loud in the log rather than in someone's ear.
                    tracing::error!(%error, "no Opus encoder; this session will send nothing");
                    Self::Direct(Codec::Pcmu)
                }
            },
            other => {
                let _ = channels;
                Self::Direct(other)
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
            #[cfg(feature = "opus")]
            Self::Opus(encoder) => match encoder.encode(samples) {
                Ok(packet) => Some(packet),
                Err(error) => {
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
    #[cfg(feature = "opus")]
    Opus(Box<sipx_audio::opus::Decoder>),
}

impl Decoding {
    fn for_codec(codec: Codec, channels: usize) -> Self {
        match codec {
            #[cfg(feature = "opus")]
            Codec::Opus => match sipx_audio::opus::Decoder::new(channels) {
                Ok(decoder) => Self::Opus(Box::new(decoder)),
                Err(error) => {
                    tracing::error!(%error, "no Opus decoder; this session will hear nothing");
                    Self::Direct(Codec::Pcmu)
                }
            },
            other => {
                let _ = channels;
                Self::Direct(other)
            }
        }
    }

    #[cfg_attr(not(feature = "opus"), allow(clippy::unnecessary_wraps))]
    fn decode(&mut self, payload: &[u8]) -> Option<Vec<i16>> {
        match self {
            Self::Direct(codec) => Some(codec.decode(payload)),
            #[cfg(feature = "opus")]
            Self::Opus(decoder) => match decoder.decode(payload) {
                Ok(samples) => Some(samples),
                Err(error) => {
                    // A packet the codec rejects is dropped, not played. A decoder pushed past
                    // a malformed packet produces noise, and noise is louder than a gap.
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
    /// How many channels the codec carries. One, for telephony.
    pub channels: usize,
    /// How much audio each packet carries. 20 ms is universal.
    pub packet_duration: Duration,
    /// Samples per second. G.711 is always 8000.
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
    /// How often to send RTCP receiver reports. `None` disables RTCP entirely.
    ///
    /// RFC 3550 §6.2 scales the interval with the session's bandwidth and membership; for a
    /// two-party call that arithmetic lands at the five-second minimum, so sipx uses it
    /// directly rather than implementing a calculation that would always return the same
    /// answer.
    pub rtcp_interval: Option<Duration>,
    /// The payload type carrying `telephone-event`, if the SDP negotiated one.
    ///
    /// It is dynamic, so the number is whatever the answer said — assuming 101 because that
    /// is what sipx offers would decode another endpoint's codec as keypresses.
    pub dtmf_payload_type: Option<u8>,
}

impl Config {
    /// The payload type this session puts on the wire.
    #[must_use]
    pub fn wire_payload_type(&self) -> u8 {
        self.payload_type
            .unwrap_or_else(|| self.codec.payload_type())
    }

    /// A session to a peer in this codec, with the settings everything uses.
    #[must_use]
    pub fn new(remote: SocketAddr, codec: Codec) -> Self {
        Self {
            remote,
            codec,
            payload_type: None,
            channels: 1,
            packet_duration: Duration::from_millis(20),
            clock_rate: codec.clock_rate(),
            jitter_depth: 3,
            jitter_max_depth: Some(12),
            rtcp_interval: Some(Duration::from_secs(5)),
            dtmf_payload_type: Some(dtmf::DEFAULT_PAYLOAD_TYPE),
        }
    }

    /// How many samples one packet carries.
    #[must_use]
    pub fn samples_per_packet(&self) -> usize {
        let millis = u64::try_from(self.packet_duration.as_millis()).unwrap_or(20);
        usize::try_from(u64::from(self.clock_rate) * millis / 1000).unwrap_or(160)
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
    Audio(Vec<i16>),
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

/// A packet's payload as it arrived, still encoded.
#[derive(Debug, Clone)]
pub struct Encoded {
    /// What it is encoded in.
    pub payload_type: u8,
    /// The bytes.
    pub payload: Bytes,
}

/// A running media session.
#[derive(Debug)]
pub struct MediaSession {
    outgoing: mpsc::Sender<Frame>,
    digits: Mutex<mpsc::Receiver<Digit>>,
    /// Distinguishes one keypress from the next.
    tones: AtomicU64,
    incoming: Mutex<mpsc::Receiver<Vec<i16>>>,
    encoded: Mutex<mpsc::Receiver<Encoded>>,
    /// Whether received packets are handed on encoded rather than decoded to samples.
    relay: Arc<AtomicBool>,
    codec: Codec,
    local_addr: SocketAddr,
    samples_per_packet: usize,
    packet_duration: Duration,
    clock_rate: u32,
    sent: Arc<AtomicU64>,
    received: Arc<AtomicU64>,
    stats: Arc<Mutex<StreamStats>>,
    /// What the far end last told us, and when.
    feedback: Arc<Mutex<Feedback>>,
    stop: Arc<Stop>,
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

/// The stop signal for a session's tasks.
///
/// A flag *and* a notify. `Notify::notify_waiters` only wakes tasks already parked on it, so a
/// loop that happens to be blocked on its channel when stop is called would never learn — and
/// would go on sending audio into a call that had been hung up. The flag makes the signal
/// durable; the notify makes it prompt.
#[derive(Debug, Default)]
struct Stop {
    stopped: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Stop {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    async fn wait(&self) {
        if self.is_stopped() {
            return;
        }
        self.notify.notified().await;
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
        })
    }

    /// The port audio will arrive on — what goes in the SDP.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Start carrying media, now that negotiation has said where and in what.
    #[must_use]
    pub fn start(self, config: Config) -> MediaSession {
        MediaSession::on_socket(&self.socket, self.rtcp, self.local_addr, config)
    }
}

impl MediaSession {
    /// Bind a socket and start the session in one step.
    ///
    /// Only for callers that already know the far end — an answerer, which has the offer in
    /// hand. A caller making the offer needs [`MediaPort`] instead.
    pub async fn start(bind: SocketAddr, config: Config) -> std::io::Result<Self> {
        let port = MediaPort::bind(bind).await?;
        Ok(port.start(config))
    }

    fn on_socket(
        socket: &Arc<UdpSocket>,
        rtcp: Option<Arc<UdpSocket>>,
        local_addr: SocketAddr,
        config: Config,
    ) -> Self {
        let samples_per_packet = config.samples_per_packet();
        let packet_duration = config.packet_duration;
        let clock_rate = config.clock_rate;
        let config_codec = config.codec;
        let rtcp_interval = config.rtcp_interval;
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Frame>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Vec<i16>>(256);
        let (encoded_tx, encoded_rx) = mpsc::channel::<Encoded>(256);
        let relay = Arc::new(AtomicBool::new(false));
        let (digits_tx, digits_rx) = mpsc::channel::<Digit>(32);

        let sent = Arc::new(AtomicU64::new(0));
        let received = Arc::new(AtomicU64::new(0));
        let outbound = Arc::new(Outbound::default());
        let feedback = Arc::new(Mutex::new(Feedback::default()));
        // Zero until the first packet names the far end's synchronisation source.
        let stats = Arc::new(Mutex::new(StreamStats::new(0)));
        let stop = Arc::new(Stop::default());
        // Chosen once and shared: RFC 3550 §8.1 requires a participant's RTCP to carry the
        // same SSRC as its RTP, so the send loop and the report loop cannot each roll
        // their own.
        let ssrc: u32 = rand::random();
        // RFC 3550 §6.5.1 wants the CNAME unique in user@host form and stable for the
        // session: a random token distinguishes sessions on this host, the local address
        // distinguishes hosts, and neither needs a name lookup on the media path.
        let cname = format!("{:08x}@{}", rand::random::<u32>(), local_addr);

        // Where to send. Starts at the SDP address and is replaced by the first observed
        // source: behind a NAT the advertised address is private and unreachable.
        let remote = Arc::new(Mutex::new(config.remote));

        tokio::spawn(send_loop(
            Arc::clone(socket),
            outgoing_rx,
            Sending {
                remote: Arc::clone(&remote),
                config: config.clone(),
                ssrc,
                sent: Arc::clone(&sent),
                outbound: Arc::clone(&outbound),
                stop: Arc::clone(&stop),
            },
        ));
        tokio::spawn(receive_loop(
            Arc::clone(socket),
            Inbound {
                audio: incoming_tx,
                encoded: encoded_tx,
                relay: Arc::clone(&relay),
                digits: digits_tx,
                remote: Arc::clone(&remote),
                config,
                received: Arc::clone(&received),
                stats: Arc::clone(&stats),
                stop: Arc::clone(&stop),
            },
        ));

        if let Some(interval) = rtcp_interval {
            tokio::spawn(rtcp_loop(
                // Reports go out from the control port when there is one, which is what a peer
                // expects to see them come from; from the media port otherwise, which some
                // peers will refuse but is better than not reporting at all.
                rtcp.clone().unwrap_or_else(|| Arc::clone(socket)),
                Arc::clone(&remote),
                interval,
                ssrc,
                cname,
                Arc::clone(&stats),
                Arc::clone(&outbound),
                Arc::clone(&feedback),
                Arc::clone(&stop),
            ));
        }
        if let Some(control) = rtcp {
            tokio::spawn(rtcp_receive_loop(
                control,
                ssrc,
                Arc::clone(&feedback),
                Arc::clone(&stop),
            ));
        }

        Self {
            outgoing: outgoing_tx,
            digits: Mutex::new(digits_rx),
            tones: AtomicU64::new(0),
            incoming: Mutex::new(incoming_rx),
            encoded: Mutex::new(encoded_rx),
            relay,
            codec: config_codec,
            local_addr,
            samples_per_packet,
            packet_duration,
            clock_rate,
            sent,
            received,
            stats,
            feedback,
            stop,
        }
    }

    /// The address media arrives on, for the SDP.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Queue one packet's worth of samples.
    ///
    /// Queued rather than sent: the pacing timer decides when it goes out.
    pub async fn send(&self, samples: Vec<i16>) -> bool {
        self.outgoing.send(Frame::Audio(samples)).await.is_ok()
    }

    /// Send a DTMF digit, held for `duration`.
    ///
    /// The packets go through the same paced queue as audio, so the tone occupies the slots
    /// audio would have. That is deliberate: RFC 4733 events replace the audio for their
    /// duration rather than being sent alongside it, and sending both means the far end hears
    /// the keypress twice.
    pub async fn send_digit(&self, digit: Digit, duration: Duration) -> bool {
        let per_packet = self.samples_per_packet;
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

    /// Take the next DTMF digit the far end pressed.
    pub async fn recv_digit(&self) -> Option<Digit> {
        self.digits.lock().await.recv().await
    }

    /// Collect digits until none arrives for `idle`.
    pub async fn collect_digits(&self, idle: Duration) -> String {
        let mut out = String::new();
        while let Ok(Some(digit)) = tokio::time::timeout(idle, self.recv_digit()).await {
            out.push(digit.as_char());
        }
        out
    }

    /// The codec this session negotiated.
    #[must_use]
    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// Hand received packets on still encoded, rather than decoding them to samples.
    ///
    /// Switchable at run time because a bridge is formed between calls that are already
    /// running: the decision belongs to whoever connects them, and it is not known when the
    /// session starts.
    pub fn set_relay(&self, relay: bool) {
        self.relay.store(relay, Ordering::SeqCst);
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

    /// Take received samples until the session goes quiet for `idle`.
    ///
    /// The idle timeout rather than a packet count, because the caller generally knows how long
    /// the far end will talk for and not how many packets that becomes.
    pub async fn record_until_idle(&self, idle: Duration) -> Vec<i16> {
        let mut samples = Vec::new();
        while let Ok(Some(frame)) = tokio::time::timeout(idle, self.recv()).await {
            samples.extend_from_slice(&frame);
        }
        samples
    }

    /// Send a whole clip, paced by the send loop.
    pub async fn play(&self, samples: &[i16], samples_per_packet: usize) {
        for chunk in samples.chunks(samples_per_packet) {
            let mut frame = chunk.to_vec();
            // The last chunk may be short. Padding with silence keeps every packet the same
            // size, which is what a far-end jitter buffer expects.
            frame.resize(samples_per_packet, 0);
            if !self.send(frame).await {
                return;
            }
        }
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

    /// What the receive path has seen: loss, jitter and sequence position.
    ///
    /// Readable mid-call, which is the point — statistics that only appear when the call ends
    /// cannot be used to do anything about the call.
    /// How the call is going: loss, jitter, round-trip time and an estimated score.
    ///
    /// Readable at any point, not only at the end. The round-trip time is `None` until a report
    /// has come back from the far end carrying an echo of one of ours — which needs both a
    /// control port on this side and a peer that answers, so it stays `None` against a peer
    /// that does not do RTCP rather than being filled in with a guess.
    pub async fn quality(&self) -> sipx_rtp::Quality {
        // Read, never `report_block()`. That function *consumes* a reporting window — it is how
        // `fraction_lost` is computed — so an application polling quality every second would
        // quietly empty the window the next RTCP report was going to describe, and the far end
        // would be told the call was clean.
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

    pub async fn stats(&self) -> sipx_rtp::rtcp::ReportBlock {
        self.stats.lock().await.report_block()
    }

    /// Wait until everything queued has actually been sent.
    ///
    /// Sending is paced, so `play` and `send_digit` return as soon as the packets are queued —
    /// which is long before they are on the wire. Hanging up at that point discards the tail:
    /// the last word of a clip, or the last digit of a PIN. Anything still queued after
    /// `within` is given up on, so this cannot hold a caller open indefinitely.
    pub async fn flush(&self, within: Duration) {
        let deadline = tokio::time::Instant::now() + within;
        while self.outgoing.capacity() < self.outgoing.max_capacity() {
            if tokio::time::Instant::now() >= deadline || self.stop.is_stopped() {
                return;
            }
            tokio::time::sleep(self.packet_duration.max(Duration::from_millis(5))).await;
        }
        // The last packet has left the queue but not yet the socket.
        tokio::time::sleep(self.packet_duration).await;
    }

    /// Stop the session and release its socket.
    pub fn stop(&self) {
        self.stop.stop();
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
        Some((packet, u32::try_from(samples.len()).unwrap_or(0)))
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
    stop: Arc<Stop>,
}

async fn send_loop(socket: Arc<UdpSocket>, mut outgoing: mpsc::Receiver<Frame>, sending: Sending) {
    let Sending {
        remote,
        config,
        ssrc,
        sent,
        outbound,
        stop,
    } = sending;
    let mut encoding = Encoding::for_codec(config.codec, config.channels);
    let mut clock = SendClock::new();

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

        // Both awaits check the stop signal. A loop parked on its channel when the call is
        // hung up would otherwise go on sending audio into a torn-down call.
        let frame = tokio::select! {
            () = stop.wait() => return,
            received = outgoing.recv() => match received {
                Some(frame) => frame,
                None => return,
            },
        };
        if stop.is_stopped() {
            return;
        }

        let (packet, advance) = match &frame {
            Frame::Audio(samples) => {
                let Some(built) =
                    clock.audio(&mut encoding, config.wire_payload_type(), ssrc, samples)
                else {
                    continue;
                };
                built
            }
            Frame::Encoded {
                payload_type,
                payload,
            } => {
                // Verbatim, on this leg's own sequence and timestamp. The advance is the
                // configured packet size: the bytes came from a stream with the same
                // packetisation, and nothing here can look inside them to check.
                let packet = Packet::new(
                    *payload_type,
                    clock.sequence,
                    clock.timestamp,
                    ssrc,
                    payload.clone(),
                );
                (
                    packet,
                    u32::try_from(config.samples_per_packet()).unwrap_or(0),
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
        if socket.send_to(&packet.encode(), destination).await.is_err() {
            return;
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

/// Everything the receive loop needs, grouped because eight positional arguments is a
/// mis-ordering waiting to happen — two of them are `Arc<AtomicU64>`-shaped and swapping them
/// would compile.
struct Inbound {
    audio: mpsc::Sender<Vec<i16>>,
    encoded: mpsc::Sender<Encoded>,
    relay: Arc<AtomicBool>,
    digits: mpsc::Sender<Digit>,
    remote: Arc<Mutex<SocketAddr>>,
    config: Config,
    received: Arc<AtomicU64>,
    stats: Arc<Mutex<StreamStats>>,
    stop: Arc<Stop>,
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
) -> bool {
    match *stream {
        None => {
            // Symmetric RTP: the observed source replaces the advertised address, because
            // behind a NAT the advertised one is private and this is the only path back.
            // Deliberately after the packet parses, so a stray STUN probe cannot move it.
            *remote.lock().await = source;
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

async fn receive_loop(socket: Arc<UdpSocket>, inbound: Inbound) {
    let Inbound {
        audio: incoming,
        encoded,
        relay,
        digits,
        remote,
        config,
        received,
        stats,
        stop,
    } = inbound;
    let mut buffer = match config.jitter_max_depth {
        Some(max) => JitterBuffer::adaptive(config.jitter_depth, max),
        None => JitterBuffer::new(config.jitter_depth),
    };
    let mut decoding = Decoding::for_codec(config.codec, config.channels);
    let mut datagram = vec![0u8; 2048];
    let mut dtmf = sipx_rtp::dtmf::Receiver::new();
    let started = tokio::time::Instant::now();
    // The synchronisation source this session is carrying. RTP has no authentication, so this
    // is not a security control — anyone who can guess the port can still forge a first
    // packet. What it does buy is that once a stream is established, a *later* forged packet
    // with a different SSRC cannot redirect our media or poison the jitter buffer, which is
    // the difference between a race an attacker has to win and one they can win at leisure.
    let mut stream: Option<u32> = None;

    // When the far end stops, whatever the buffer is still holding has to come out. Without
    // this the last `depth - 1` packets are never played: in a continuous call that is
    // invisible, but at the end of every clip it clips the tail off.
    let flush_after = config
        .packet_duration
        .saturating_mul(4)
        .max(Duration::from_millis(60));

    loop {
        if stop.is_stopped() {
            return;
        }
        let read = tokio::select! {
            () = stop.wait() => return,
            read = tokio::time::timeout(flush_after, socket.recv_from(&mut datagram)) => read,
        };

        let (len, source) = match read {
            Ok(Ok(received)) => received,
            Ok(Err(_)) => return,
            Err(_elapsed) => {
                // Silence. Release what is held rather than holding it against a packet that
                // is not coming.
                for packet in buffer.drain() {
                    if !deliver(
                        &Delivery {
                            audio: &incoming,
                            encoded: &encoded,
                            relay: &relay,
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
                }
                continue;
            }
        };

        let bytes = Bytes::copy_from_slice(datagram.get(..len).unwrap_or(&[]));
        let Ok(packet) = Packet::decode(&bytes) else {
            // A malformed packet is dropped, not fatal. Media ports attract stray traffic —
            // STUN probes, port scans, the occasional scanner — and none of it should end a
            // call.
            continue;
        };

        if !accept_source(&mut stream, &packet, source, &remote, &stats).await {
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
        {
            let mut stats = stats.lock().await;
            if config.dtmf_payload_type == Some(packet.payload_type) {
                // A telephone event's timestamp is the event's *start* (RFC 4733
                // §2.5.1.2), not this packet's sampling instant, so its transit grows per
                // packet by design and would fabricate jitter out of a keypress. It still
                // counts for loss and sequence continuity.
                stats.on_untimed_packet(packet.sequence);
            } else {
                stats.on_packet(packet.sequence, packet.timestamp, arrival);
            }
        }

        buffer.push_at(packet, arrival);

        while let Some(packet) = buffer.pop() {
            if !deliver(
                &Delivery {
                    audio: &incoming,
                    encoded: &encoded,
                    relay: &relay,
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
        }
    }
}

/// Hand one packet's audio to the application.
///
/// Returns whether the loop should keep running.
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
    interval: Duration,
    ssrc: u32,
    cname: String,
    stats: Arc<Mutex<StreamStats>>,
    outbound: Arc<Outbound>,
    feedback: Arc<Mutex<Feedback>>,
    stop: Arc<Stop>,
) {
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

        let block = stats.lock().await.report_block();
        let heard_anything = block.extended_highest_sequence != 0;
        let sent_packets = outbound.packets.load(Ordering::Relaxed);
        if !heard_anything && sent_packets == 0 {
            // Nothing has happened in either direction, so there is nothing to report on.
            continue;
        }

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

        // RTCP conventionally travels on the RTP port plus one (RFC 3550 §11).
        let destination = *remote.lock().await;
        let rtcp_port = destination.port().saturating_add(1);
        let rtcp_to = SocketAddr::new(destination.ip(), rtcp_port);
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
    feedback: Arc<Mutex<Feedback>>,
    stop: Arc<Stop>,
) {
    let mut datagram = vec![0u8; 2048];
    loop {
        let read = tokio::select! {
            () = stop.wait() => return,
            read = socket.recv_from(&mut datagram) => read,
        };
        if stop.is_stopped() {
            return;
        }
        let Ok((len, _source)) = read else {
            return;
        };

        let bytes = Bytes::copy_from_slice(datagram.get(..len).unwrap_or(&[]));
        // A malformed control packet is dropped, not fatal. The control port attracts the same
        // stray traffic the media port does, and none of it should end a call.
        let Ok(packets) = Rtcp::decode_compound(&bytes) else {
            continue;
        };

        let arrival = tokio::time::Instant::now();
        for packet in packets {
            match packet {
                Rtcp::Sender(report) => {
                    {
                        let mut held = feedback.lock().await;
                        held.last_sender_report =
                            sipx_rtp::quality::middle_32(report.ntp_timestamp);
                        held.received_at = Some(arrival);
                    }
                    note_round_trip(feedback_of(&report.reports, ssrc), &feedback).await;
                }
                Rtcp::Receiver(report) => {
                    note_round_trip(feedback_of(&report.reports, ssrc), &feedback).await;
                }
                Rtcp::Sdes(_) | Rtcp::Other { .. } => {}
            }
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

async fn note_round_trip(block: Option<sipx_rtp::ReportBlock>, feedback: &Arc<Mutex<Feedback>>) {
    let Some(block) = block else {
        return;
    };
    let now = sipx_rtp::quality::middle_32(sipx_rtp::quality::ntp_now());
    if let Some(trip) = sipx_rtp::quality::round_trip(
        now,
        block.last_sender_report,
        block.delay_since_last_sender_report,
    ) {
        feedback.lock().await.round_trip = Some(trip);
    }
}

/// Where a received packet goes.
struct Delivery<'a> {
    audio: &'a mpsc::Sender<Vec<i16>>,
    encoded: &'a mpsc::Sender<Encoded>,
    relay: &'a AtomicBool,
}

async fn deliver(
    to: &Delivery<'_>,
    decoding: &mut Decoding,
    digits: &mpsc::Sender<Digit>,
    dtmf: &mut sipx_rtp::dtmf::Receiver,
    config: &Config,
    stop: &Stop,
    packet: &Packet,
) -> bool {
    // A telephone event is a keypress, not audio. It goes to the DTMF path and never to the
    // audio one — decoding a four-byte event payload as µ-law injects four garbage samples and
    // is heard as a click.
    if config.dtmf_payload_type == Some(packet.payload_type) {
        if let Some(event) = DtmfEvent::decode(&packet.payload)
            && let Some(digit) = dtmf.push(packet.timestamp, &event)
        {
            // A full channel means the application is not reading digits. Dropping is
            // right: a keypress delivered late is worse than one not delivered, since the
            // application has already moved on.
            let _ = digits.try_send(digit);
        }
        return true;
    }

    // Relaying: hand the payload on exactly as it arrived. The bridge on the other side will
    // put it on its own wire with its own sequence and timestamp, which is right — the two
    // legs are separate RTP streams that happen to carry the same audio.
    if to.relay.load(Ordering::SeqCst) {
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
    if packet.payload_type != config.wire_payload_type()
        && Codec::from_payload_type(packet.payload_type).is_none()
    {
        return true;
    }

    // The stop signal is checked here too. This is the one await that can park indefinitely —
    // a full channel means the application has stopped reading — and a task parked here when
    // the call is hung up would hold its socket and its port for the life of the process.
    let Some(samples) = decoding.decode(&packet.payload) else {
        return true;
    };

    tokio::select! {
        () = stop.wait() => false,
        result = to.audio.send(samples) => result.is_ok(),
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

    /// The failing-first test for this story.
    #[tokio::test]
    async fn audio_played_into_a_session_arrives_at_the_far_end() {
        let (left, right) = pair(Codec::Pcmu).await;
        let source = tone(1600); // 200 ms

        // `right` speaks; `left` listens. `left` does not know `right`'s address until a
        // packet arrives, which is exactly what symmetric RTP is for.
        right.play(&source, 160).await;
        let recorded = left.record_until_idle(Duration::from_millis(400)).await;

        assert_eq!(recorded.len(), source.len(), "every packet arrived");

        // G.711 is lossy, so the samples cannot be compared directly — but the codec is
        // idempotent, so encoding the source and encoding what came back must agree exactly.
        assert_eq!(
            g711::ulaw_encode_all(&source),
            g711::ulaw_encode_all(&recorded),
            "the audio that arrived is the audio that was sent"
        );
    }

    #[tokio::test]
    async fn audio_flows_in_both_directions_at_once() {
        let (left, right) = pair(Codec::Pcmu).await;
        let from_left = tone(800);
        let from_right: Vec<i16> = tone(800).iter().map(|s| -s).collect();

        // Left must learn right's address, which it does from right's first packet.
        right.play(&from_right[..160], 160).await;
        tokio::time::sleep(Duration::from_millis(60)).await;

        let (left_recorded, right_recorded) = tokio::join!(
            async {
                left.play(&from_left, 160).await;
                left.record_until_idle(Duration::from_millis(300)).await
            },
            async {
                right.play(&from_right[160..], 160).await;
                right.record_until_idle(Duration::from_millis(300)).await
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
        let recorded = left.record_until_idle(Duration::from_millis(300)).await;
        assert_eq!(
            g711::alaw_encode_all(&source),
            g711::alaw_encode_all(&recorded)
        );
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
        let recorded = left.record_until_idle(Duration::from_millis(300)).await;
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

        right.play(&tone(320), 160).await;
        // Give left time to latch the source address.
        tokio::time::sleep(Duration::from_millis(120)).await;

        let reply = tone(320);
        left.play(&reply, 160).await;
        let heard = right.record_until_idle(Duration::from_millis(300)).await;

        assert!(
            !heard.is_empty(),
            "left was configured with 127.0.0.1:1 and must have learned the real address"
        );
    }

    #[tokio::test]
    async fn packets_are_counted_on_both_sides() {
        let (left, right) = pair(Codec::Pcmu).await;
        right.play(&tone(800), 160).await;
        let _ = left.record_until_idle(Duration::from_millis(300)).await;

        assert_eq!(right.packets_sent(), 5);
        assert_eq!(left.packets_received(), 5);
    }

    /// A short final chunk is padded so every packet is the same size, which is what a far-end
    /// jitter buffer expects.
    #[tokio::test]
    async fn a_partial_final_frame_is_padded_rather_than_sent_short() {
        let (left, right) = pair(Codec::Pcmu).await;
        right.play(&tone(400), 160).await; // 2.5 packets
        let recorded = left.record_until_idle(Duration::from_millis(300)).await;
        assert_eq!(recorded.len(), 480, "three whole packets");
        assert_eq!(&recorded[400..], &[0i16; 80], "padded with silence");
    }

    /// The acceptance test for M-7: a keypress crosses a real media session and arrives once.
    #[tokio::test]
    async fn a_dtmf_digit_survives_a_media_session() {
        let (left, right) = pair(Codec::Pcmu).await;

        // Establish the stream so `left` knows where `right` is.
        right.play(&tone(320), 160).await;
        assert_eq!(
            left.record_until_idle(Duration::from_millis(300))
                .await
                .len(),
            320
        );

        right
            .send_digit(
                Digit::from_char('5').expect("a digit"),
                Duration::from_millis(100),
            )
            .await;

        let digit = tokio::time::timeout(Duration::from_secs(2), left.recv_digit())
            .await
            .expect("no timeout")
            .expect("a digit arrives");
        assert_eq!(digit.as_char(), '5');

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
        let _ = left.record_until_idle(Duration::from_millis(200)).await;

        for c in "1234".chars() {
            right
                .send_digit(
                    Digit::from_char(c).expect("a digit"),
                    Duration::from_millis(80),
                )
                .await;
        }

        let collected = left.collect_digits(Duration::from_millis(600)).await;
        assert_eq!(collected, "1234");
    }

    /// DTMF must not become audio and audio must not become digits.
    #[tokio::test]
    async fn keypresses_and_audio_stay_on_their_own_paths() {
        let (left, right) = pair(Codec::Pcmu).await;

        right.play(&tone(320), 160).await;
        let audio = left.record_until_idle(Duration::from_millis(300)).await;
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
        let digit = tokio::time::timeout(Duration::from_secs(2), left.recv_digit())
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
        tokio::time::sleep(Duration::from_millis(150)).await;

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

    /// sipx's own offers advertise `telephone-event` on payload type 101, so a peer that sends
    /// DTMF sends packets this loop must not treat as speech. Decoding a four-byte event
    /// payload as µ-law injects four garbage samples and is heard as a click.
    #[tokio::test]
    async fn an_unknown_payload_type_is_dropped_rather_than_decoded_as_audio() {
        let (left, right) = pair(Codec::Pcmu).await;

        // Establish the stream so `left` latches on to `right`.
        right.play(&tone(320), 160).await;
        let established = left.record_until_idle(Duration::from_millis(300)).await;
        assert_eq!(established.len(), 320);

        // Now a DTMF packet on the same stream. It must not reach the audio path.
        let dtmf = Packet::new(
            101,
            9000,
            999_999,
            1,
            Bytes::from_static(&[0x05, 0x0A, 0x01, 0x40]),
        );
        let raw = UdpSocket::bind(any()).await.expect("binds");
        raw.send_to(&dtmf.encode(), left.local_addr())
            .await
            .expect("sends");

        let after = left.record_until_idle(Duration::from_millis(200)).await;
        assert!(
            after.is_empty(),
            "a telephone-event packet must not become audio samples: {after:?}"
        );
    }

    /// Once a stream is established, a packet from a different synchronisation source is
    /// dropped. Without this, one forged packet with a high sequence number advances the
    /// jitter buffer past every genuine packet still to come, and the call goes silent.
    #[tokio::test]
    async fn a_packet_from_another_source_cannot_silence_the_stream() {
        let (left, right) = pair(Codec::Pcmu).await;

        right.play(&tone(320), 160).await;
        assert_eq!(
            left.record_until_idle(Duration::from_millis(300))
                .await
                .len(),
            320
        );

        // A forged packet: valid RTP, different SSRC, sequence number far in the future.
        let forged = Packet::new(0, 60_000, 0, 0xBAD0_BAD0, Bytes::from(vec![0xFFu8; 160]));
        let attacker = UdpSocket::bind(any()).await.expect("binds");
        attacker
            .send_to(&forged.encode(), left.local_addr())
            .await
            .expect("sends");
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The genuine stream still gets through.
        let more = tone(320);
        right.play(&more, 160).await;
        let heard = left.record_until_idle(Duration::from_millis(300)).await;
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
        tokio::time::sleep(Duration::from_millis(100)).await;

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
        assert_eq!(Codec::from_payload_type(0), Some(Codec::Pcmu));
        assert_eq!(Codec::from_payload_type(8), Some(Codec::Pcma));
        assert_eq!(Codec::from_payload_type(9), None, "G.722 is not ours");
    }
}
