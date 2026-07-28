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
use sipx_rtp::rtcp::{ReceiverReport, Rtcp, StreamStats};
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
}

impl Codec {
    /// The static payload type.
    #[must_use]
    pub fn payload_type(self) -> u8 {
        match self {
            Self::Pcmu => 0,
            Self::Pcma => 8,
        }
    }

    /// The codec for a payload type, if it is one we carry.
    #[must_use]
    pub fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self::Pcmu),
            8 => Some(Self::Pcma),
            _ => None,
        }
    }

    fn encode(self, samples: &[i16]) -> Vec<u8> {
        match self {
            Self::Pcmu => g711::ulaw_encode_all(samples),
            Self::Pcma => g711::alaw_encode_all(samples),
        }
    }

    fn decode(self, payload: &[u8]) -> Vec<i16> {
        match self {
            Self::Pcmu => g711::ulaw_decode_all(payload),
            Self::Pcma => g711::alaw_decode_all(payload),
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
    /// How much audio each packet carries. 20 ms is universal.
    pub packet_duration: Duration,
    /// Samples per second. G.711 is always 8000.
    pub clock_rate: u32,
    /// How many packets the jitter buffer holds.
    pub jitter_depth: usize,
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
    /// A G.711 session to a peer, with the settings everything uses.
    #[must_use]
    pub fn new(remote: SocketAddr, codec: Codec) -> Self {
        Self {
            remote,
            codec,
            packet_duration: Duration::from_millis(20),
            clock_rate: 8000,
            jitter_depth: 3,
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
    Dtmf { event: DtmfEvent, tone: u64 },
}

/// A running media session.
#[derive(Debug)]
pub struct MediaSession {
    outgoing: mpsc::Sender<Frame>,
    digits: Mutex<mpsc::Receiver<Digit>>,
    /// Distinguishes one keypress from the next.
    tones: AtomicU64,
    incoming: Mutex<mpsc::Receiver<Vec<i16>>>,
    local_addr: SocketAddr,
    samples_per_packet: usize,
    packet_duration: Duration,
    sent: Arc<AtomicU64>,
    received: Arc<AtomicU64>,
    stats: Arc<Mutex<StreamStats>>,
    stop: Arc<Stop>,
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
    local_addr: SocketAddr,
}

impl MediaPort {
    /// Bind a port. Port 0 asks the OS to choose one.
    pub async fn bind(bind: SocketAddr) -> std::io::Result<Self> {
        let socket = Arc::new(UdpSocket::bind(bind).await?);
        let local_addr = socket.local_addr()?;
        Ok(Self { socket, local_addr })
    }

    /// The port audio will arrive on — what goes in the SDP.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Start carrying media, now that negotiation has said where and in what.
    #[must_use]
    pub fn start(self, config: Config) -> MediaSession {
        MediaSession::on_socket(self.socket, self.local_addr, config)
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

    fn on_socket(socket: Arc<UdpSocket>, local_addr: SocketAddr, config: Config) -> Self {
        let samples_per_packet = config.samples_per_packet();
        let packet_duration = config.packet_duration;
        let rtcp_interval = config.rtcp_interval;
        let (outgoing_tx, outgoing_rx) = mpsc::channel::<Frame>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<Vec<i16>>(256);
        let (digits_tx, digits_rx) = mpsc::channel::<Digit>(32);

        let sent = Arc::new(AtomicU64::new(0));
        let received = Arc::new(AtomicU64::new(0));
        // Zero until the first packet names the far end's synchronisation source.
        let stats = Arc::new(Mutex::new(StreamStats::new(0)));
        let stop = Arc::new(Stop::default());

        // Where to send. Starts at the SDP address and is replaced by the first observed
        // source: behind a NAT the advertised address is private and unreachable.
        let remote = Arc::new(Mutex::new(config.remote));

        tokio::spawn(send_loop(
            Arc::clone(&socket),
            outgoing_rx,
            Arc::clone(&remote),
            config.clone(),
            Arc::clone(&sent),
            Arc::clone(&stop),
        ));
        tokio::spawn(receive_loop(
            Arc::clone(&socket),
            Inbound {
                audio: incoming_tx,
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
                socket,
                remote,
                interval,
                Arc::clone(&stats),
                Arc::clone(&stop),
            ));
        }

        Self {
            outgoing: outgoing_tx,
            digits: Mutex::new(digits_rx),
            tones: AtomicU64::new(0),
            incoming: Mutex::new(incoming_rx),
            local_addr,
            samples_per_packet,
            packet_duration,
            sent,
            received,
            stats,
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
        for event in events {
            if self
                .outgoing
                .send(Frame::Dtmf { event, tone })
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

async fn send_loop(
    socket: Arc<UdpSocket>,
    mut outgoing: mpsc::Receiver<Frame>,
    remote: Arc<Mutex<SocketAddr>>,
    config: Config,
    sent: Arc<AtomicU64>,
    stop: Arc<Stop>,
) {
    let mut sequence: u16 = rand::random();
    let mut timestamp: u32 = rand::random();
    let ssrc: u32 = rand::random();
    // The timestamp a tone in progress started at, and whether the next audio packet begins a
    // new talkspurt (RFC 3550: the marker bit says so, and a tone interrupts the audio).
    let mut tone_timestamp: Option<u32> = None;
    let mut current_tone: Option<u64> = None;
    // How much the clock owes the tone in progress, charged when it is over.
    let mut tone_duration: u32 = 0;
    let mut ending_a_tone = false;

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
                // A tone that has just finished owes the clock its duration; pay it before
                // stamping the audio that follows, or the audio overlaps the keypress.
                timestamp = timestamp.wrapping_add(std::mem::take(&mut tone_duration));
                current_tone = None;
                tone_timestamp = None;
                let mut packet = Packet::new(
                    config.codec.payload_type(),
                    sequence,
                    timestamp,
                    ssrc,
                    Bytes::from(config.codec.encode(samples)),
                );
                packet.marker = ending_a_tone;
                ending_a_tone = false;
                // The timestamp advances by the samples this packet actually carried, not by
                // the configured packet size. They are usually the same, and when they are not
                // — a caller sending 10 ms frames on a 20 ms config — advancing by the
                // configured size builds a timeline at the wrong rate, and the far end plays
                // the call with a gap between every packet.
                (packet, u32::try_from(samples.len()).unwrap_or(0))
            }
            Frame::Dtmf { event, tone } => {
                let Some(payload_type) = config.dtmf_payload_type else {
                    // Nothing negotiated `telephone-event`, so there is no payload type to
                    // send it on. Dropping is right: guessing one means sending keypresses on
                    // whatever the far end uses that number for.
                    continue;
                };

                // A new keypress starts here; anything with the same tag continues the one in
                // progress and reuses its timestamp. That shared timestamp is what marks the
                // packets as one press — including the end retransmissions, which is the case
                // that gets this wrong.
                let starting = current_tone != Some(*tone);
                if starting {
                    // The previous tone's duration is charged to the clock now, so audio
                    // resumes past it rather than on top of it.
                    timestamp = timestamp.wrapping_add(std::mem::take(&mut tone_duration));
                    current_tone = Some(*tone);
                    tone_timestamp = Some(timestamp);
                }

                let mut packet = Packet::new(
                    payload_type,
                    sequence,
                    tone_timestamp.unwrap_or(timestamp),
                    ssrc,
                    event.encode(),
                );
                packet.marker = starting;
                if event.end {
                    tone_duration = u32::from(event.duration);
                    ending_a_tone = true;
                }
                (packet, 0)
            }
        };

        let destination = *remote.lock().await;
        if socket.send_to(&packet.encode(), destination).await.is_err() {
            return;
        }
        sent.fetch_add(1, Ordering::Relaxed);

        // Both counters wrap, and both are supposed to.
        //
        // The timestamp advances by the samples this packet actually carried, not by the
        // configured packet size. They are usually the same, and when they are not — a caller
        // sending 10 ms frames on a 20 ms config — advancing by the configured size builds a
        // timeline at the wrong rate, and the far end plays the call with a gap between every
        // packet.
        sequence = sequence.wrapping_add(1);
        timestamp = timestamp.wrapping_add(advance);
    }
}

/// Everything the receive loop needs, grouped because eight positional arguments is a
/// mis-ordering waiting to happen — two of them are `Arc<AtomicU64>`-shaped and swapping them
/// would compile.
struct Inbound {
    audio: mpsc::Sender<Vec<i16>>,
    digits: mpsc::Sender<Digit>,
    remote: Arc<Mutex<SocketAddr>>,
    config: Config,
    received: Arc<AtomicU64>,
    stats: Arc<Mutex<StreamStats>>,
    stop: Arc<Stop>,
}

async fn receive_loop(socket: Arc<UdpSocket>, inbound: Inbound) {
    let Inbound {
        audio: incoming,
        digits,
        remote,
        config,
        received,
        stats,
        stop,
    } = inbound;
    let mut buffer = JitterBuffer::new(config.jitter_depth);
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
                    if !deliver(&incoming, &digits, &mut dtmf, &config, &stop, &packet).await {
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

        match stream {
            None => {
                // Symmetric RTP: the observed source replaces the advertised address, because
                // behind a NAT the advertised one is private and this is the only path back.
                // Deliberately after the packet parses, so a stray STUN probe cannot move it.
                *remote.lock().await = source;
                stream = Some(packet.ssrc);
            }
            Some(established) if established != packet.ssrc => {
                // Another source on our port. Dropped rather than mixed in: one packet with a
                // high sequence number would otherwise advance the jitter buffer past every
                // genuine packet still to come, and the call goes silent.
                tracing::debug!(
                    %source,
                    ssrc = packet.ssrc,
                    "ignoring a packet from a different synchronisation source"
                );
                continue;
            }
            Some(_) => {}
        }

        received.fetch_add(1, Ordering::Relaxed);

        // The arrival clock has to be in the same units as the RTP timestamp — 8000 per second
        // for G.711 — or the jitter estimate measures the difference between two unit systems
        // rather than between two packets.
        {
            let mut stats = stats.lock().await;
            let arrival = arrival_in_timestamp_units(started, config.clock_rate);
            stats.on_packet(packet.sequence, packet.timestamp, arrival);
        }

        buffer.push(packet);

        while let Some(packet) = buffer.pop() {
            if !deliver(&incoming, &digits, &mut dtmf, &config, &stop, &packet).await {
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

/// Send a receiver report every interval, so the far end can see what we saw.
async fn rtcp_loop(
    socket: Arc<UdpSocket>,
    remote: Arc<Mutex<SocketAddr>>,
    interval: Duration,
    stats: Arc<Mutex<StreamStats>>,
    stop: Arc<Stop>,
) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The first tick is immediate and would report an empty stream.
    tick.tick().await;

    loop {
        tokio::select! {
            () = stop.wait() => return,
            _ = tick.tick() => {}
        }
        if stop.is_stopped() {
            return;
        }

        let block = stats.lock().await.report_block();
        // Nothing has arrived yet, so there is nothing to report on.
        if block.extended_highest_sequence == 0 {
            continue;
        }

        let report = Rtcp::Receiver(ReceiverReport {
            ssrc: block.ssrc,
            reports: vec![block],
        });

        // RTCP conventionally travels on the RTP port plus one (RFC 3550 §11).
        let destination = *remote.lock().await;
        let rtcp_port = destination.port().saturating_add(1);
        let rtcp_to = SocketAddr::new(destination.ip(), rtcp_port);
        if socket.send_to(&report.encode(), rtcp_to).await.is_err() {
            return;
        }
    }
}

async fn deliver(
    incoming: &mpsc::Sender<Vec<i16>>,
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

    // Any other unknown payload type is dropped rather than decoded as the negotiated codec.
    let Some(codec) = Codec::from_payload_type(packet.payload_type) else {
        return true;
    };

    // The stop signal is checked here too. This is the one await that can park indefinitely —
    // a full channel means the application has stopped reading — and a task parked here when
    // the call is hung up would hold its socket and its port for the life of the process.
    tokio::select! {
        () = stop.wait() => false,
        result = incoming.send(codec.decode(&packet.payload)) => result.is_ok(),
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

    #[test]
    fn codecs_map_to_their_static_payload_types() {
        assert_eq!(Codec::Pcmu.payload_type(), 0);
        assert_eq!(Codec::Pcma.payload_type(), 8);
        assert_eq!(Codec::from_payload_type(0), Some(Codec::Pcmu));
        assert_eq!(Codec::from_payload_type(8), Some(Codec::Pcma));
        assert_eq!(Codec::from_payload_type(9), None, "G.722 is not ours");
    }
}
