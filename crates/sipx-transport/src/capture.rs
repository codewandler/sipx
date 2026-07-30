//! Recording the signalling an endpoint exchanged, for attaching to a bug report.
//!
//! `docs/specs/sip-transport.md` §13. Off by default; when off, the cost is one `Option` check per
//! message and nothing is opened, allocated or spawned.
//!
//! # The two things worth knowing before reading the code
//!
//! **Ordering is established in the driver loop; the write is not performed there** (§13.2). The
//! loop stamps each record with a sequence number and a timestamp and hands it to a writer over a
//! bounded channel. Writing inline reads as the more faithful design and is the opposite: the loop
//! that would do the writing is the loop that fires retransmission timers, so an inline write puts
//! the filesystem in the retransmission path and delays Timer A on a slow or full disk — which is
//! the "observation that perturbs a retransmission race" the story forbids. Faithfulness comes from
//! the order being *decided* at the observation point, which the sequence number records; the writer
//! may fall behind but cannot reorder what it was given.
//!
//! **A capture is a security surface.** It is written to be handed to someone outside the trust
//! boundary it was recorded in, so [`redact`] removes the secrets that would still be valid in
//! another person's hands: digest responses, SRTP master keys, push tokens, instance URNs. What it
//! cannot remove is identity — `To`, `From` and the SDP addresses survive, and they are enough to
//! say who called whom and from where. Redaction makes a capture safe to *attach*, not safe to
//! publish. See §13.3.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;

use crate::counters::Meters;
use crate::target::TransportKind;

/// How a capture is configured (`Config::capture`).
#[derive(Debug, Clone)]
pub struct CaptureConfig {
    /// Where to write the pcapng file. Created, or truncated if it exists.
    pub path: PathBuf,
    /// Whether to strip credentials before writing (§13.3). **On by default.**
    ///
    /// Turning it off is for a lab capture against a test registrar, where there is no secret worth
    /// removing and redaction would hide the digest bug the capture was taken to find. Never turn it
    /// off for a capture that will leave the machine.
    pub redact: bool,
    /// How many records may queue for the writer before records are dropped.
    ///
    /// Dropped rather than blocking the driver: see the module note. A dropped record is counted in
    /// [`crate::CaptureCounts::dropped`], never silent.
    pub queue: usize,
}

impl CaptureConfig {
    /// A redacting capture at `path`, with a queue deep enough for a burst.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            redact: true,
            queue: 1024,
        }
    }

    /// Keep credentials in the file.
    ///
    /// Named so that it cannot be reached without saying what it does at the call site.
    #[must_use]
    pub fn without_redaction(mut self) -> Self {
        self.redact = false;
        self
    }
}

/// Which way a message was going.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Received by this endpoint.
    In,
    /// Sent by this endpoint.
    Out,
}

impl Direction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// One observed message, stamped at the point it crossed the boundary.
#[derive(Debug)]
struct Record {
    /// The order the loop saw it in. This, and not the write, is what makes the capture faithful.
    seq: u64,
    at: SystemTime,
    local: SocketAddr,
    peer: SocketAddr,
    transport: TransportKind,
    direction: Direction,
    bytes: Bytes,
    /// Whether [`redact`] changed anything, so the comment can say so.
    redacted: bool,
}

/// The driver's end of a running capture.
///
/// Cheap to check and cheap to hold. The writer lives on its own thread; this is a bounded sender
/// and a counter.
#[derive(Debug)]
pub(crate) struct Capture {
    records: std::sync::mpsc::SyncSender<Record>,
    /// The next sequence number, assigned in the loop.
    seq: u64,
    redact: bool,
    /// Set by the writer when a write has failed, so the driver stops handing it records.
    ///
    /// A capture that is silently not happening is the same failure as a silent discard, one level
    /// up (§13.2), so the failure is counted and logged by the writer before this is set.
    failed: Arc<AtomicBool>,
}

impl Capture {
    /// Open `path` and start the writer thread.
    ///
    /// Fails only if the file cannot be created or its headers cannot be written — a
    /// misconfiguration the caller should hear about at `bind` rather than discover in an empty
    /// file later.
    pub(crate) fn start(config: &CaptureConfig, meters: Arc<Meters>) -> std::io::Result<Self> {
        let file = std::fs::File::create(&config.path)?;
        let mut writer = std::io::BufWriter::new(file);
        write_section_header(&mut writer)?;
        writer.flush()?;

        // Bounded, and `try_send` at the other end: an overrun drops a record rather than blocking
        // the driver. `sync_channel` rather than a tokio channel because the consumer is a plain
        // thread doing blocking file writes — which is the point of it not being on the loop.
        let (records, incoming) = std::sync::mpsc::sync_channel::<Record>(config.queue.max(1));
        let failed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&failed);
        let path = config.path.clone();

        std::thread::Builder::new()
            .name("sipx-capture".to_owned())
            .spawn(move || write_loop(&mut writer, &incoming, &meters, &flag, &path))?;

        Ok(Self {
            records,
            seq: 0,
            redact: config.redact,
            failed,
        })
    }

    /// Whether this capture has given up. Checked before a record is built, so a failed capture
    /// costs no redaction work.
    fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
}

/// Take one record off the queue at a time and write it, until the driver is gone or a write fails.
fn write_loop(
    writer: &mut std::io::BufWriter<std::fs::File>,
    incoming: &std::sync::mpsc::Receiver<Record>,
    meters: &Meters,
    failed: &AtomicBool,
    path: &Path,
) {
    while let Ok(record) = incoming.recv() {
        if let Err(error) = write_packet(writer, &record).and_then(|()| writer.flush()) {
            // Once, and then stop. A capture that cannot be written is over; continuing would log
            // per message and produce a file nobody can trust.
            meters.capture_error();
            failed.store(true, Ordering::Relaxed);
            tracing::error!(
                %error,
                path = %path.display(),
                "capture write failed; the capture is now off"
            );
            return;
        }
    }
    // The driver dropped the sender: an ordinary shutdown. Flush what is buffered so the last
    // messages before the shutdown are in the file, which is usually the interesting part.
    if let Err(error) = writer.flush() {
        meters.capture_error();
        tracing::warn!(%error, path = %path.display(), "capture could not be flushed at shutdown");
    }
}

// ---------------------------------------------------------------------------------------------
// pcapng (§13.1)
// ---------------------------------------------------------------------------------------------

/// Section Header Block.
const BLOCK_SECTION_HEADER: u32 = 0x0A0D_0D0A;
/// Interface Description Block.
const BLOCK_INTERFACE: u32 = 0x0000_0001;
/// Enhanced Packet Block.
const BLOCK_PACKET: u32 = 0x0000_0006;
/// The byte-order magic, written native so a reader knows which way round the file is.
const BYTE_ORDER_MAGIC: u32 = 0x1A2B_3C4D;
/// Raw IP: no link layer, because there is no link layer inside a process.
const LINKTYPE_RAW: u16 = 101;
/// `opt_comment`.
const OPT_COMMENT: u16 = 1;
/// `if_tsresol`.
const OPT_TSRESOL: u16 = 9;
/// `opt_endofopt`.
const OPT_END: u16 = 0;
/// Timestamps are nanoseconds: `if_tsresol` = 9.
const TSRESOL_NANOS: u8 = 9;
/// IP's protocol number for UDP. Synthetic for every transport — see [`synthesise`].
const IP_PROTO_UDP: u8 = 17;

/// Pad a length up to the next multiple of four, as every pcapng block requires.
const fn padded(len: usize) -> usize {
    len.next_multiple_of(4)
}

/// The zero bytes that carry `len` up to the next multiple of four.
fn padding(len: usize) -> &'static [u8] {
    const ZEROS: [u8; 3] = [0; 3];
    ZEROS.get(..padded(len).saturating_sub(len)).unwrap_or(&[])
}

/// Write a block: type, total length, body, total length again.
///
/// The trailing length is what makes a truncated file readable backwards from the end, which is
/// §13.1's third reason for the format.
fn write_block(out: &mut impl Write, kind: u32, body: &[u8]) -> std::io::Result<()> {
    let total = u32::try_from(12usize.saturating_add(padded(body.len())))
        .map_err(|_| std::io::Error::other("capture block too large"))?;
    out.write_all(&kind.to_ne_bytes())?;
    out.write_all(&total.to_ne_bytes())?;
    out.write_all(body)?;
    out.write_all(padding(body.len()))?;
    out.write_all(&total.to_ne_bytes())
}

/// An option: code, length, value, padded.
fn push_option(body: &mut Vec<u8>, code: u16, value: &[u8]) {
    body.extend_from_slice(&code.to_ne_bytes());
    let len = u16::try_from(value.len()).unwrap_or(u16::MAX);
    body.extend_from_slice(&len.to_ne_bytes());
    let value = value.get(..usize::from(len)).unwrap_or(value);
    body.extend_from_slice(value);
    body.extend_from_slice(padding(value.len()));
}

/// The Section Header Block and the one Interface Description Block.
fn write_section_header(out: &mut impl Write) -> std::io::Result<()> {
    let mut shb = Vec::new();
    shb.extend_from_slice(&BYTE_ORDER_MAGIC.to_ne_bytes());
    shb.extend_from_slice(&1u16.to_ne_bytes()); // major
    shb.extend_from_slice(&0u16.to_ne_bytes()); // minor
    shb.extend_from_slice(&(-1i64).to_ne_bytes()); // section length: unknown
    push_option(&mut shb, OPT_COMMENT, b"sipx signalling capture");
    push_option(&mut shb, OPT_END, &[]);
    write_block(out, BLOCK_SECTION_HEADER, &shb)?;

    let mut idb = Vec::new();
    idb.extend_from_slice(&LINKTYPE_RAW.to_ne_bytes());
    idb.extend_from_slice(&0u16.to_ne_bytes()); // reserved
    idb.extend_from_slice(&0u32.to_ne_bytes()); // snaplen: no limit
    push_option(&mut idb, OPT_TSRESOL, &[TSRESOL_NANOS]);
    push_option(&mut idb, OPT_END, &[]);
    write_block(out, BLOCK_INTERFACE, &idb)
}

/// One Enhanced Packet Block: the synthesised headers, the message, and the comment that carries
/// the truth about the transport.
fn write_packet(out: &mut impl Write, record: &Record) -> std::io::Result<()> {
    let packet = synthesise(record);
    let nanos = record
        .at
        .duration_since(UNIX_EPOCH)
        .map_or(0u128, |since| since.as_nanos());
    let timestamp = u64::try_from(nanos).unwrap_or(u64::MAX);

    let len = u32::try_from(packet.len())
        .map_err(|_| std::io::Error::other("captured message too large"))?;

    let mut body = Vec::with_capacity(packet.len() + 64);
    body.extend_from_slice(&0u32.to_ne_bytes()); // interface 0
    body.extend_from_slice(&((timestamp >> 32) as u32).to_ne_bytes());
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the low half of the timestamp is exactly what this field is"
    )]
    body.extend_from_slice(&(timestamp as u32).to_ne_bytes());
    body.extend_from_slice(&len.to_ne_bytes()); // captured length
    body.extend_from_slice(&len.to_ne_bytes()); // original length
    body.extend_from_slice(&packet);
    body.extend_from_slice(padding(packet.len()));
    push_option(&mut body, OPT_COMMENT, comment(record).as_bytes());
    push_option(&mut body, OPT_END, &[]);

    write_block(out, BLOCK_PACKET, &body)
}

/// What the block comment says. **This, not the packet, is the authoritative record of the
/// transport** (§13.1): the UDP header below is synthetic whatever the message really travelled on.
fn comment(record: &Record) -> String {
    let mut comment = format!(
        "seq={} dir={} transport={} local={} peer={}",
        record.seq,
        record.direction.as_str(),
        record.transport.as_str(),
        record.local,
        record.peer,
    );
    if matches!(record.transport, TransportKind::Tls | TransportKind::Wss) {
        comment.push_str(" decrypted-in-process=yes");
    }
    if record.redacted {
        comment.push_str(" redacted=yes");
    }
    comment
}

/// A synthetic IP and UDP header in front of the message.
///
/// The addresses and ports are real; the headers are not, and §13.1 says so plainly. Writing a
/// truthful TCP header would mean inventing per-connection sequence numbers to let a tool reassemble
/// a stream that is already framed here — one message is one packet — so the transport header is UDP
/// for every transport and the block comment carries which it really was.
fn synthesise(record: &Record) -> Vec<u8> {
    let (source, destination) = match record.direction {
        Direction::In => (record.peer, record.local),
        Direction::Out => (record.local, record.peer),
    };
    let payload = &record.bytes;
    let udp_len = u16::try_from(8usize.saturating_add(payload.len())).unwrap_or(u16::MAX);

    let mut packet = Vec::with_capacity(40 + 8 + payload.len());
    match (source.ip(), destination.ip()) {
        (IpAddr::V4(from), IpAddr::V4(to)) => {
            let total =
                u16::try_from(20usize.saturating_add(usize::from(udp_len))).unwrap_or(u16::MAX);
            let mut header = Vec::with_capacity(20);
            header.push(0x45); // IPv4, 20-byte header
            header.push(0); // DSCP/ECN
            header.extend_from_slice(&total.to_be_bytes());
            header.extend_from_slice(&0u16.to_be_bytes()); // identification
            header.extend_from_slice(&0u16.to_be_bytes()); // flags/fragment
            header.push(64); // TTL
            header.push(IP_PROTO_UDP);
            header.extend_from_slice(&0u16.to_be_bytes()); // checksum, filled in below
            header.extend_from_slice(&from.octets());
            header.extend_from_slice(&to.octets());
            // Computed, unlike the UDP checksum: it covers only these twenty bytes, so it is a real
            // checksum over what was really written rather than one over a datagram that never
            // existed — and leaving it zero would have every tool flag every packet (§13.1).
            let checksum = ones_complement(&header);
            if let Some(slot) = header.get_mut(10..12) {
                slot.copy_from_slice(&checksum.to_be_bytes());
            }
            packet.extend_from_slice(&header);
        }
        (IpAddr::V6(from), IpAddr::V6(to)) => {
            packet.extend_from_slice(&0x6000_0000u32.to_be_bytes()); // version 6
            packet.extend_from_slice(&udp_len.to_be_bytes()); // payload length
            packet.push(IP_PROTO_UDP);
            packet.push(64); // hop limit
            packet.extend_from_slice(&from.octets());
            packet.extend_from_slice(&to.octets());
        }
        // One end IPv4 and the other IPv6 cannot happen on a real socket pair; if it somehow does,
        // the message still matters more than the headers, so it is written with no IP header at
        // all rather than dropped or guessed at.
        _ => {}
    }

    packet.extend_from_slice(&source.port().to_be_bytes());
    packet.extend_from_slice(&destination.port().to_be_bytes());
    packet.extend_from_slice(&udp_len.to_be_bytes());
    // Zero: "not computed", which is what it is. Legal on IPv4; §13.1 records that a strict reader
    // may flag it on IPv6, and why inventing one is worse.
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

/// The internet checksum (RFC 1071) over an IPv4 header.
fn ones_complement(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for pair in header.chunks(2) {
        let high = u32::from(pair.first().copied().unwrap_or(0));
        let low = u32::from(pair.get(1).copied().unwrap_or(0));
        sum = sum.wrapping_add((high << 8) | low);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF).wrapping_add(sum >> 16);
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the fold above leaves at most sixteen significant bits"
    )]
    let folded = sum as u16;
    !folded
}

// ---------------------------------------------------------------------------------------------
// Redaction (§13.3)
// ---------------------------------------------------------------------------------------------
//
// # Why this reads bytes rather than a parsed message
//
// A UDP datagram is captured *before* parsing (§13.2), so redaction has to work on whatever a peer
// sent — including a message the parser would reject, which is precisely where a credential turns up
// somewhere unexpected. That decision is right and is kept.
//
// What it costs is that this code cannot assume one spelling. SIP's grammar permits several for the
// same header, the parser accepts them, and a first version of this module gated on the single
// literal `"authorization:"` — so a folded header, an `Authorization : …` with whitespace before the
// colon, and a bare-LF message each carried a digest response into a capture file in cleartext. The
// shape below exists to stop that class rather than those three cases:
//
// 1. **Lines are split on CRLF, bare LF or bare CR.** Anything else makes a malformed message one
//    long line, and one long line matches no header name at all.
// 2. **Continuation lines are unfolded into one logical header** before anything looks at it
//    (RFC 3261 §7.3.1), because a fold can fall in the middle of a parameter name.
// 3. **A header's name is the bytes before its first colon, with trailing whitespace trimmed**, not
//    a literal prefix — HCOLON allows whitespace before the colon (§25.1).
// 4. **A line whose name cannot be determined is redacted conservatively rather than skipped.** If
//    the structure is not there, a credential could be anywhere, and the cost of guessing wrong is a
//    mangled value in a capture instead of a leaked one.

/// Header parameters whose values are live credentials.
const REDACTED_PARAMS: &[&[u8]] = &[
    // RFC 7616 §3.4: the digest response. With the nonce beside it in the same capture it is
    // replayable, which is what makes it the one that must go.
    b"response",
    // RFC 7616 §3.5: the server's half of the same exchange.
    b"nextnonce",
    b"rspauth",
    // RFC 8599 §4: a push token is a bearer credential for waking a device.
    b"pn-prid",
    b"pn-param",
    // RFC 5626 §4.1: a stable device identifier that outlives the call.
    b"+sip.instance",
];

/// Headers whose value is an authentication credential.
///
/// Separate from [`CONTACT_HEADERS`] because only these carry an auth *scheme*, and the scheme
/// decides whether the credential is a named parameter or one opaque token.
const AUTH_HEADERS: &[&[u8]] = &[
    b"authorization",
    b"proxy-authorization",
    b"authentication-info",
    b"proxy-authenticate",
    b"www-authenticate",
];

/// Headers that carry credential *parameters* without a scheme. `m` is `Contact`'s compact form.
const CONTACT_HEADERS: &[&[u8]] = &[b"contact", b"m"];

/// Schemes whose credential is one opaque token rather than named parameters.
///
/// RFC 8898 registers `Bearer` for SIP, and `Basic` — removed from SIP by RFC 3261 §22.1 — is still
/// what a misconfigured gateway sends. In both the token *is* the credential, so there is no
/// parameter to find and the whole of it goes. An unrecognised scheme whose value carries no `=` is
/// treated the same way, because a token68 is the only other thing it can be.
const OPAQUE_SCHEMES: &[&[u8]] = &[b"bearer", b"basic"];

/// What a redacted value is replaced with.
const REDACTION: &[u8] = b"REDACTED";

/// One physical line and the terminator that ended it.
///
/// The terminator is carried rather than normalised because the **body** must keep its exact byte
/// length: `Content-Length` counts it, and rewriting a bare LF inside an SDP body as CRLF would leave
/// every message in the capture inconsistent with its own header.
struct Line<'a> {
    text: &'a [u8],
    terminator: &'a [u8],
}

/// Split a message on any of the three terminators one arrives with.
///
/// RFC 3261 §7 says CRLF, and §13.2 promises a malformed message is captured anyway — so a peer that
/// sends bare LF must not thereby switch redaction off.
fn lines(message: &[u8]) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut at = 0usize;
    while at < message.len() {
        let width = match message.get(at) {
            Some(b'\r') if message.get(at.saturating_add(1)) == Some(&b'\n') => 2,
            Some(b'\r' | b'\n') => 1,
            _ => 0,
        };
        if width == 0 {
            at = at.saturating_add(1);
            continue;
        }
        let end = at.saturating_add(width);
        out.push(Line {
            text: message.get(start..at).unwrap_or(&[]),
            terminator: message.get(at..end).unwrap_or(&[]),
        });
        at = end;
        start = end;
    }
    if start < message.len() {
        out.push(Line {
            text: message.get(start..).unwrap_or(&[]),
            terminator: &[],
        });
    }
    out
}

fn is_wsp(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

/// Whether a line is a continuation of the header above it (RFC 3261 §7.3.1).
fn is_continuation(line: &[u8]) -> bool {
    line.first().copied().is_some_and(is_wsp)
}

/// Join a header and its continuation lines into one logical line.
///
/// §7.3.1 makes a fold equivalent to a single space, which is what it is replaced with. Unfolding has
/// to happen before anything reads the line: a fold may fall inside a parameter name, so
/// `respo\r\n nse="…"` is a `response` parameter and matches nothing until it is joined up.
fn unfold(physical: &[Line<'_>], from: usize, to: usize, separator: &[u8]) -> Vec<u8> {
    let mut logical = Vec::new();
    for index in from..to {
        let Some(line) = physical.get(index) else {
            continue;
        };
        if index == from {
            logical.extend_from_slice(line.text);
        } else {
            logical.extend_from_slice(separator);
            logical.extend_from_slice(line.text.trim_ascii_start());
        }
    }
    logical
}

/// A header's name, lowercased — the bytes before the first colon with trailing whitespace trimmed.
///
/// `None` when there is no colon, which means this is not a line whose name can be established.
fn header_name(line: &[u8]) -> Option<Vec<u8>> {
    let at = line.iter().position(|byte| *byte == b':')?;
    Some(line.get(..at)?.trim_ascii_end().to_ascii_lowercase())
}

/// Strip the secrets §13.3 names.
///
/// Returns `None` when nothing was found, so an unredacted message is never copied.
pub(crate) fn redact(message: &[u8]) -> Option<Bytes> {
    let physical = lines(message);
    let mut out: Vec<u8> = Vec::with_capacity(message.len().saturating_add(16));
    let mut changed = false;
    let mut in_body = false;
    let mut index = 0usize;

    while index < physical.len() {
        let Some(line) = physical.get(index) else {
            break;
        };

        if in_body {
            // Length-preserving, because `Content-Length` counts these bytes.
            match redact_body_line(line.text) {
                Some(redacted) => {
                    changed = true;
                    out.extend_from_slice(&redacted);
                }
                None => out.extend_from_slice(line.text),
            }
            out.extend_from_slice(line.terminator);
            index = index.saturating_add(1);
            continue;
        }

        // The empty line ends the headers. Its own terminator is part of the separator and is copied
        // through unchanged.
        if line.text.is_empty() {
            in_body = true;
            out.extend_from_slice(line.terminator);
            index = index.saturating_add(1);
            continue;
        }

        // This line plus any continuation of it are one header.
        let mut end = index.saturating_add(1);
        while physical
            .get(end)
            .is_some_and(|next| is_continuation(next.text))
        {
            end = end.saturating_add(1);
        }

        // §7.3.1 makes a fold equivalent to a single space, so that is the reading a parser gets and
        // the one tried first. If it finds nothing and the header *was* folded, the fold is removed
        // entirely and the line is scanned again: a fold inside a token names no parameter in SIP, but
        // "no parser would read that as a credential" is a worse thing to be wrong about than one
        // extra scan of a rare line. Fail safe on spellings — that is the whole lesson of this module.
        let redacted = redact_header(&unfold(&physical, index, end, b" ")).or_else(|| {
            (end.saturating_sub(index) > 1)
                .then(|| redact_header(&unfold(&physical, index, end, b"")))
                .flatten()
        });
        match redacted {
            Some(redacted) => {
                changed = true;
                // Emitted unfolded: the fold is equivalent to a space (§7.3.1), and a redacted
                // record is not byte-exact in any case (§13.3).
                out.extend_from_slice(&redacted);
                out.extend_from_slice(b"\r\n");
            }
            None => {
                // Untouched, so the original bytes go through exactly — folds, terminators and all.
                for at in index..end {
                    if let Some(original) = physical.get(at) {
                        out.extend_from_slice(original.text);
                        out.extend_from_slice(original.terminator);
                    }
                }
            }
        }
        index = end;
    }

    changed.then(|| Bytes::from(out))
}

/// Redact one logical header line, if it is one that can carry a secret.
fn redact_header(line: &[u8]) -> Option<Vec<u8>> {
    match header_name(line) {
        Some(name) if AUTH_HEADERS.contains(&name.as_slice()) => redact_auth_header(line),
        Some(name) if CONTACT_HEADERS.contains(&name.as_slice()) => redact_params(line, false),
        // A named header that carries no credential: a `From` display name reading `response=me` is
        // not a credential and is left alone.
        Some(_) => None,
        // No colon, so there is no name to go on. Redact conservatively: this is the malformed case,
        // and being wrong costs a mangled value rather than a leaked one.
        None => redact_params(line, false),
    }
}

/// Redact an authentication header, whichever shape its scheme gives it.
fn redact_auth_header(line: &[u8]) -> Option<Vec<u8>> {
    let colon = line.iter().position(|byte| *byte == b':')?;
    let after_colon = colon.saturating_add(1);
    let value = line.get(after_colon..).unwrap_or(&[]);

    // The scheme is the first token of the value; the credential is whatever follows it.
    let lead = value
        .iter()
        .position(|byte| !is_wsp(*byte))
        .unwrap_or(value.len());
    let token = value.get(lead..).unwrap_or(&[]);
    let width = token
        .iter()
        .position(|byte| is_wsp(*byte))
        .unwrap_or(token.len());
    let scheme = token.get(..width).unwrap_or(&[]).to_ascii_lowercase();
    let rest_at = after_colon.saturating_add(lead).saturating_add(width);
    let rest = line.get(rest_at..).unwrap_or(&[]);

    let opaque = OPAQUE_SCHEMES.contains(&scheme.as_slice())
        // An unrecognised scheme whose credential carries no `=` has no parameter to find, so the
        // credential is the token itself. Fail safe rather than leave it.
        || (!scheme.is_empty() && !rest.trim_ascii().is_empty() && !rest.contains(&b'='));

    if opaque {
        let mut redacted = Vec::with_capacity(line.len());
        redacted.extend_from_slice(line.get(..rest_at).unwrap_or(&[]));
        redacted.push(b' ');
        redacted.extend_from_slice(REDACTION);
        return Some(redacted);
    }
    redact_params(line, false)
}

/// Replace every credential parameter on a line.
///
/// `preserve_len` keeps each replacement the same width as what it replaced, which the body needs and
/// a header does not — see [`redact_body_line`].
fn redact_params(line: &[u8], preserve_len: bool) -> Option<Vec<u8>> {
    let mut out = line.to_vec();
    let mut changed = false;
    for name in REDACTED_PARAMS {
        while let Some(replaced) = redact_param(&out, name, preserve_len) {
            out = replaced;
            changed = true;
        }
    }
    changed.then_some(out)
}

/// The bytes a redacted value is replaced with.
fn replacement(width: usize, preserve_len: bool) -> Vec<u8> {
    if !preserve_len {
        return REDACTION.to_vec();
    }
    let mut padded = Vec::with_capacity(width);
    padded.extend_from_slice(REDACTION.get(..width.min(REDACTION.len())).unwrap_or(&[]));
    while padded.len() < width {
        padded.push(b'X');
    }
    padded
}

/// Replace the first not-yet-redacted `name=value` on a line.
///
/// Returns `None` once there is nothing left to do, which is what terminates the caller's loop.
fn redact_param(line: &[u8], name: &[u8], preserve_len: bool) -> Option<Vec<u8>> {
    let mut from = 0usize;
    loop {
        let at = find_ci(line, name, from)?;
        let before_ok = at == 0
            || line
                .get(at.wrapping_sub(1))
                .is_some_and(|byte| matches!(byte, b',' | b';' | b' ' | b'\t' | b'"' | b'='));
        // Past the name, optional whitespace, then `=`.
        let mut cursor = at.saturating_add(name.len());
        while line.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor = cursor.saturating_add(1);
        }
        if !before_ok || line.get(cursor) != Some(&b'=') {
            from = at.saturating_add(1);
            continue;
        }
        cursor = cursor.saturating_add(1);
        while line.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor = cursor.saturating_add(1);
        }

        let quoted = line.get(cursor) == Some(&b'"');
        let value_start = if quoted {
            cursor.saturating_add(1)
        } else {
            cursor
        };
        let mut end = value_start;
        while let Some(&byte) = line.get(end) {
            if quoted {
                // A quoted-pair escapes the next octet, including a quote (RFC 3261 §25.1), so the
                // string does not end here and the escaped byte is part of the value.
                if byte == b'\\' && line.get(end.saturating_add(1)).is_some() {
                    end = end.saturating_add(2);
                    continue;
                }
                if byte == b'"' {
                    break;
                }
            } else if matches!(byte, b',' | b';' | b' ' | b'\t') {
                break;
            }
            end = end.saturating_add(1);
        }

        let value = line.get(value_start..end).unwrap_or(&[]);
        // Already done, or nothing there to do. Without the first the caller's loop would not
        // terminate; the second keeps a message that had nothing to hide from being rewritten.
        if value == replacement(value.len(), preserve_len).as_slice() || value.is_empty() {
            from = end.max(at.saturating_add(1));
            continue;
        }

        let mut out = Vec::with_capacity(line.len());
        out.extend_from_slice(line.get(..value_start).unwrap_or(&[]));
        out.extend_from_slice(&replacement(value.len(), preserve_len));
        out.extend_from_slice(line.get(end..).unwrap_or(&[]));
        return Some(out);
    }
}

/// Redact a line of a message body.
///
/// **Length-preserving, unlike a header**, and that is not fussiness: the body's length is declared in
/// `Content-Length`, so shortening a line here would leave every message in the capture inconsistent
/// with its own header and unparseable by the tool the capture exists to be read in.
fn redact_body_line(line: &[u8]) -> Option<Vec<u8>> {
    if starts_with_ci(line, b"a=crypto:") {
        return redact_inline_keys(line);
    }
    if starts_with_ci(line, b"k=") {
        return redact_sdp_key(line);
    }
    // A SIP message nested in a body — `message/sipfrag` (RFC 3420), or a part of a multipart — puts
    // real headers where this function sees body lines. Handled by name, like any other header, but
    // length-preserving because it is inside the body either way.
    match header_name(line) {
        Some(name)
            if AUTH_HEADERS.contains(&name.as_slice())
                || CONTACT_HEADERS.contains(&name.as_slice()) =>
        {
            redact_params(line, true)
        }
        _ => None,
    }
}

/// Redact every `inline:` key on an `a=crypto` line (RFC 4568 §6.1).
///
/// Every one, because `key-params = key-param *(";" key-param)` (§9.1) permits more than one and a
/// single-occurrence search left the second key in the file.
fn redact_inline_keys(line: &[u8]) -> Option<Vec<u8>> {
    const INLINE: &[u8] = b"inline:";
    let mut out: Vec<u8> = Vec::with_capacity(line.len());
    let mut cursor = 0usize;
    let mut changed = false;

    while let Some(found) = find_from(line, INLINE, cursor) {
        let value_start = found.saturating_add(INLINE.len());
        let mut end = value_start;
        while let Some(&byte) = line.get(end) {
            // The key runs to the `|` that begins the optional lifetime or MKI, or to the end of the
            // parameter. Neither of those is secret and both are kept.
            if matches!(byte, b'|' | b' ' | b'\t' | b';') {
                break;
            }
            end = end.saturating_add(1);
        }
        let width = end.saturating_sub(value_start);
        out.extend_from_slice(line.get(cursor..value_start).unwrap_or(&[]));
        if width > 0 {
            out.extend_from_slice(&replacement(width, true));
            changed = true;
        }
        cursor = end;
    }
    out.extend_from_slice(line.get(cursor..).unwrap_or(&[]));
    changed.then_some(out)
}

/// Redact an SDP `k=` key (RFC 4566 §5.12).
///
/// Deprecated by the RFC itself and still a key in cleartext when it appears. `k=<method>:<key>` —
/// the method is kept, the key goes. `k=prompt` carries no key and is left alone.
fn redact_sdp_key(line: &[u8]) -> Option<Vec<u8>> {
    let colon = line.iter().position(|byte| *byte == b':')?;
    let value_start = colon.saturating_add(1);
    let width = line.len().saturating_sub(value_start);
    if width == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(line.len());
    out.extend_from_slice(line.get(..value_start).unwrap_or(&[]));
    out.extend_from_slice(&replacement(width, true));
    Some(out)
}

/// Case-sensitive substring search from `from`.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len().saturating_sub(needle.len()))
        .find(|&at| haystack.get(at..at.saturating_add(needle.len())) == Some(needle))
}

/// Case-insensitive substring search from `from`.
fn find_ci(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (from..=haystack.len().saturating_sub(needle.len())).find(|&at| {
        haystack
            .get(at..at.saturating_add(needle.len()))
            .is_some_and(|window| window.eq_ignore_ascii_case(needle))
    })
}

fn starts_with_ci(haystack: &[u8], prefix: &[u8]) -> bool {
    haystack
        .get(..prefix.len())
        .is_some_and(|window| window.eq_ignore_ascii_case(prefix))
}

impl Capture {
    /// Stamp a message and hand it to the writer.
    ///
    /// Called from the driver loop, which is what makes the sequence number meaningful. Everything
    /// expensive — redaction, the synthetic headers, the write — happens after this returns or on
    /// another thread.
    /// Observe a message if a capture is running, and do **no work at all** if one is not.
    ///
    /// The laziness is a contract rather than an optimisation, which is why it lives in one function
    /// with a test on it instead of being a discipline at three call sites. `bytes` is a closure
    /// because producing them is not free on every path — an inbound stream message has to be
    /// re-serialised to be captured (§13.2) — and an endpoint with no capture configured must not pay
    /// for a file nobody asked for. The story's Acceptance says capture "costs nothing when off", and
    /// a version of this that took `&Bytes` made that false for every TCP, TLS and WebSocket message.
    pub(crate) fn observe_if_capturing(
        capture: Option<&mut Self>,
        meters: &Meters,
        local: SocketAddr,
        peer: SocketAddr,
        transport: TransportKind,
        direction: Direction,
        bytes: impl FnOnce() -> Bytes,
    ) {
        // Both arms of this return before `bytes` is called, which is the whole point.
        let Some(capture) = capture else {
            return;
        };
        if capture.is_failed() {
            return;
        }
        capture.observe(meters, &bytes(), local, peer, transport, direction);
    }

    fn observe(
        &mut self,
        meters: &Meters,
        bytes: &Bytes,
        local: SocketAddr,
        peer: SocketAddr,
        transport: TransportKind,
        direction: Direction,
    ) {
        let (bytes, redacted) = if self.redact {
            match redact(bytes) {
                Some(clean) => (clean, true),
                None => (bytes.clone(), false),
            }
        } else {
            (bytes.clone(), false)
        };

        self.seq = self.seq.saturating_add(1);
        let record = Record {
            seq: self.seq,
            at: SystemTime::now(),
            local,
            peer,
            transport,
            direction,
            bytes,
            redacted,
        };
        // `try_send`, never `send`: blocking here would put the writer's queue in the
        // retransmission path, which is the whole thing §13.2 refuses.
        match self.records.try_send(record) {
            Ok(()) => meters.capture_record(),
            Err(_) => meters.capture_drop(),
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

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// Vector X14. The precedent this follows is `sipx-sdp`'s: an error names the tag and never the
    /// key material.
    #[test]
    fn a_digest_response_is_removed_and_the_challenge_is_kept() {
        let message = b"REGISTER sip:example.net SIP/2.0\r\n\
             Authorization: Digest username=\"alice\", realm=\"example.net\", \
             nonce=\"abc123\", uri=\"sip:example.net\", response=\"deadbeefcafe0001\", qop=auth\r\n\
             Content-Length: 0\r\n\r\n";
        let redacted = redact(message).expect("a digest response must be redacted");
        let out = text(&redacted);

        assert!(
            !out.contains("deadbeefcafe0001"),
            "the digest response is still in the capture: {out}"
        );
        assert!(out.contains("response=\"REDACTED\""), "{out}");
        // Kept, and deliberately: a digest failure is unreadable without them, and a nonce with no
        // response beside it is not a credential.
        assert!(out.contains("realm=\"example.net\""), "{out}");
        assert!(out.contains("nonce=\"abc123\""), "{out}");
        assert!(out.contains("username=\"alice\""), "{out}");
        assert!(out.contains("qop=auth"), "{out}");
        assert!(
            out.starts_with("REGISTER sip:example.net SIP/2.0\r\n"),
            "{out}"
        );
    }

    /// Vector X15, and the length-preservation rule that keeps `Content-Length` honest.
    #[test]
    fn an_srtp_key_is_removed_without_changing_the_body_length() {
        let message = b"INVITE sip:bob@example.net SIP/2.0\r\n\
             Content-Type: application/sdp\r\n\
             Content-Length: 68\r\n\r\n\
             v=0\r\n\
             a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj|2^20|1:32\r\n";
        let redacted = redact(message).expect("an SRTP key must be redacted");
        let out = text(&redacted);

        assert!(
            !out.contains("d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj"),
            "the SRTP master key is still in the capture: {out}"
        );
        assert!(
            out.contains("a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:"),
            "{out}"
        );
        // The tag and the crypto-suite are kept: §5.1.2 makes them the thing an answer has to echo,
        // so a negotiation bug is invisible without them.
        assert!(out.contains("AES_CM_128_HMAC_SHA1_80"), "{out}");
        assert!(
            out.contains("|2^20|1:32"),
            "the lifetime and MKI are not secret: {out}"
        );
        assert_eq!(
            redacted.len(),
            message.len(),
            "body redaction must preserve length or Content-Length becomes a lie"
        );
    }

    #[test]
    fn a_push_token_and_an_instance_urn_are_removed() {
        let message = b"REGISTER sip:example.net SIP/2.0\r\n\
             Contact: <sip:alice@192.0.2.4>;+sip.instance=\"<urn:uuid:00000000-0000-0000-0000-00000000042>\";\
             pn-provider=apns;pn-prid=SECRETTOKEN;pn-param=SECRETPARAM\r\n\
             Content-Length: 0\r\n\r\n";
        let out = text(&redact(message).expect("push credentials must be redacted"));

        assert!(!out.contains("SECRETTOKEN"), "{out}");
        assert!(!out.contains("SECRETPARAM"), "{out}");
        assert!(!out.contains("urn:uuid:00000000"), "{out}");
        // The provider is not a credential and says which push service a bug is about.
        assert!(out.contains("pn-provider=apns"), "{out}");
        assert!(out.contains("sip:alice@192.0.2.4"), "{out}");
    }

    /// What redaction deliberately keeps. Stated as a test because §13's disclosure depends on it
    /// being true: an operator told the file is safe would be worse off than one told it is not.
    #[test]
    fn redaction_keeps_what_makes_a_message_diagnosable() {
        let message = b"INVITE sip:bob@example.net SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.4:5060;branch=z9hG4bKtrace\r\n\
             From: \"Alice Example\" <sip:alice@example.net>;tag=abcd\r\n\
             To: <sip:bob@example.net>\r\n\
             Call-ID: trace@sipx\r\n\
             CSeq: 1 INVITE\r\n\
             Authorization: Digest response=\"secret\"\r\n\
             Content-Length: 0\r\n\r\n";
        let out = text(&redact(message).expect("the digest response is redacted"));

        for kept in [
            "z9hG4bKtrace",
            "Alice Example",
            "sip:alice@example.net",
            "sip:bob@example.net",
            "trace@sipx",
            "CSeq: 1 INVITE",
        ] {
            assert!(
                out.contains(kept),
                "redaction removed {kept}, which it needs: {out}"
            );
        }
        assert!(!out.contains("\"secret\""), "{out}");
    }

    /// A message with no secret in it is not copied at all, which is what keeps redaction off the
    /// cost of an ordinary capture.
    #[test]
    fn a_message_with_no_credential_is_left_untouched() {
        let message = b"OPTIONS sip:example.net SIP/2.0\r\n\
             To: <sip:example.net>\r\n\
             Content-Length: 0\r\n\r\n";
        assert!(
            redact(message).is_none(),
            "nothing to redact must mean no copy"
        );
    }

    /// A `To` display name that happens to contain a parameter name must not be mangled: redaction
    /// is by header *and* by parameter, not by substring.
    #[test]
    fn a_display_name_that_looks_like_a_parameter_survives() {
        let message = b"INVITE sip:bob@example.net SIP/2.0\r\n\
             From: \"response=me\" <sip:alice@example.net>\r\n\
             Content-Length: 0\r\n\r\n";
        assert!(
            redact(message).is_none(),
            "a From display name is not a credential-bearing header"
        );
    }

    /// Malformed input must be redacted too — it is exactly where a credential turns up somewhere
    /// unexpected — and must not panic (`AGENTS.md` §3).
    #[test]
    fn a_truncated_message_is_redacted_without_panicking() {
        assert!(redact(b"").is_none());
        assert!(redact(b"\r\n").is_none());
        assert!(redact(b"Authorization: Digest response=").is_none());
        let partial = redact(b"Authorization: Digest response=\"abc")
            .expect("an unterminated quoted value is still a credential");
        assert!(!text(&partial).contains("abc"));
        // No trailing CRLF: the last line must still be processed.
        let no_crlf = redact(b"Authorization: Digest response=\"xyz\"").expect("redacted");
        assert!(!text(&no_crlf).contains("xyz"));
    }

    /// Join lines with CRLF and end the headers, so a fixture cannot accidentally indent a line and
    /// thereby turn it into a folded continuation of the one above — which is a real SIP rule and was
    /// how the first draft of these tests fooled itself.
    fn message(lines: &[&str]) -> Vec<u8> {
        joined(lines, "\r\n")
    }

    /// The same, with a chosen terminator, for the spellings that are the point of the test.
    fn joined(lines: &[&str], terminator: &str) -> Vec<u8> {
        let mut out = String::new();
        for line in lines {
            out.push_str(line);
            out.push_str(terminator);
        }
        out.push_str(terminator);
        out.into_bytes()
    }

    /// **The class the security review found**: legal spellings of the same header that a literal
    /// `"authorization:"` prefix does not match, each of which carried a digest response into a
    /// capture file in cleartext. Table-driven because the class is the point, not the cases — a new
    /// spelling belongs here as a row.
    #[test]
    fn every_legal_spelling_of_a_credential_header_is_redacted() {
        const SECRET: &str = "SPELLINGSECRET0001";

        let folded = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "Authorization: Digest username=\"alice\",",
            "\tresponse=\"SPELLINGSECRET0001\"",
            "Content-Length: 0",
        ]);
        let folded_mid_name = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "Authorization: Digest respo",
            " nse=\"SPELLINGSECRET0001\"",
        ]);
        let space_before_colon = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "Authorization : Digest response=\"SPELLINGSECRET0001\"",
        ]);
        let tab_before_colon = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "Authorization\t: Digest response=\"SPELLINGSECRET0001\"",
        ]);
        let bare_lf = joined(
            &[
                "REGISTER sip:example.net SIP/2.0",
                "Authorization: Digest response=\"SPELLINGSECRET0001\"",
            ],
            "\n",
        );
        let bare_cr = joined(
            &[
                "REGISTER sip:example.net SIP/2.0",
                "Authorization: Digest response=\"SPELLINGSECRET0001\"",
            ],
            "\r",
        );
        let unterminated =
            b"REGISTER sip:example.net SIP/2.0\r\nAuthorization: Digest response=\"SPELLINGSECRET0001\"".to_vec();

        let cases: [(&str, &[u8]); 7] = [
            ("folded onto a continuation line (RFC 3261 §7.3.1)", &folded),
            (
                "folded in the middle of the parameter name",
                &folded_mid_name,
            ),
            (
                "whitespace before the colon, which HCOLON permits (§25.1)",
                &space_before_colon,
            ),
            ("a tab before the colon", &tab_before_colon),
            ("bare LF, which made the whole datagram one line", &bare_lf),
            ("bare CR", &bare_cr),
            ("no trailing terminator at all", &unterminated),
        ];

        for (spelling, raw) in cases {
            let redacted =
                redact(raw).unwrap_or_else(|| panic!("{spelling}: nothing was redacted at all"));
            let out = text(&redacted);
            assert!(
                !out.contains(SECRET),
                "{spelling}: the credential survived redaction: {out}"
            );
        }
    }

    /// A line whose name cannot be established is redacted conservatively rather than skipped.
    ///
    /// The fail-safe half of the fix. "Unparseable" is exactly when a credential turns up somewhere
    /// unexpected, so the absence of structure must not become the absence of redaction.
    #[test]
    fn a_line_with_no_header_name_is_redacted_conservatively() {
        let raw = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "GARBAGE WITHOUT A COLON response=\"CONSERVATIVE0004\"",
        ]);
        let out = text(&redact(&raw).expect("a nameless line is still scanned"));
        assert!(!out.contains("CONSERVATIVE0004"), "{out}");
    }

    /// **B2**: `key-params = key-param *(";" key-param)` (RFC 4568 §9.1), so one line can carry more
    /// than one key, and a single-occurrence search left the second in the file.
    #[test]
    fn every_inline_key_on_a_crypto_line_is_redacted() {
        let raw = message(&[
            "INVITE sip:bob@example.net SIP/2.0",
            "Content-Length: 0",
            "",
            "v=0",
            "a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:FIRSTKEY0005aaaaaaaaaaaaaaaaaaaa|2^20|1;inline:SECONDKEY0006bbbbbbbbbbbbbbbbbbb|2^20|2",
        ]);
        let redacted = redact(&raw).expect("both keys are redacted");
        let out = text(&redacted);
        assert!(
            !out.contains("FIRSTKEY0005"),
            "the first key survived: {out}"
        );
        assert!(
            !out.contains("SECONDKEY0006"),
            "the second key survived: {out}"
        );
        assert!(out.contains("|2^20|1"), "the lifetime is not secret: {out}");
        assert!(out.contains("|2^20|2"), "{out}");
        assert_eq!(
            redacted.len(),
            raw.len(),
            "body redaction must preserve length"
        );
    }

    /// An opaque credential is the whole token, so there is no parameter to find (RFC 8898).
    #[test]
    fn an_opaque_scheme_has_its_whole_credential_removed() {
        for (scheme, secret) in [
            ("Bearer", "BEARERTOKEN0007"),
            ("bearer", "BEARERTOKEN0007"),
            ("Basic", "BASICSECRET0008"),
            // An unregistered scheme whose value carries no `=` can only be a token68.
            ("Weird", "WEIRDTOKEN0009"),
        ] {
            let raw = message(&[
                "REGISTER sip:example.net SIP/2.0",
                &format!("Authorization: {scheme} {secret}"),
            ]);
            let out =
                text(&redact(&raw).unwrap_or_else(|| panic!("{scheme} was not redacted at all")));
            assert!(!out.contains(secret), "{scheme}: {out}");
            // The scheme is kept: which scheme failed is the diagnosis.
            assert!(out.contains(scheme), "{scheme} should survive: {out}");
        }
    }

    /// A digest challenge still redacts by parameter rather than being read as an opaque token.
    #[test]
    fn a_digest_header_is_not_mistaken_for_an_opaque_credential() {
        let raw = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "Authorization: Digest realm=\"example.net\", nonce=\"n\", response=\"DIGEST0010\"",
        ]);
        let out = text(&redact(&raw).expect("redacted"));
        assert!(!out.contains("DIGEST0010"), "{out}");
        assert!(out.contains("realm=\"example.net\""), "{out}");
        assert!(out.contains("nonce=\"n\""), "{out}");
    }

    /// SDP `k=` carries a key in cleartext (RFC 4566 §5.12). Deprecated, and still a key.
    #[test]
    fn an_sdp_key_field_is_redacted_but_prompt_is_not() {
        let raw = message(&[
            "INVITE sip:bob@example.net SIP/2.0",
            "",
            "v=0",
            "k=base64:SDPKEY0011aaaa",
        ]);
        let redacted = redact(&raw).expect("a k= key is redacted");
        let out = text(&redacted);
        assert!(!out.contains("SDPKEY0011"), "{out}");
        assert!(out.contains("k=base64:"), "the method is kept: {out}");
        assert_eq!(redacted.len(), raw.len(), "length preserved");

        let prompt = message(&["INVITE sip:bob@example.net SIP/2.0", "", "v=0", "k=prompt"]);
        assert!(
            redact(&prompt).is_none(),
            "k=prompt carries no key, so there is nothing to rewrite"
        );
    }

    /// A SIP message nested in a body (RFC 3420 `message/sipfrag`, or a multipart part) puts real
    /// headers where the body scanner sees body lines.
    #[test]
    fn a_credential_nested_in_a_body_is_redacted() {
        let raw = message(&[
            "INVITE sip:bob@example.net SIP/2.0",
            "Content-Type: message/sipfrag",
            "",
            "REGISTER sip:inner SIP/2.0",
            "Authorization: Digest response=\"NESTEDSECRET0012\"",
        ]);
        let redacted = redact(&raw).expect("a nested credential is redacted");
        let out = text(&redacted);
        assert!(!out.contains("NESTEDSECRET0012"), "{out}");
        assert_eq!(
            redacted.len(),
            raw.len(),
            "a body stays the length Content-Length claims"
        );
    }

    /// A quoted-pair does not end the quoted string (RFC 3261 §25.1), so the tail after an escaped
    /// quote is part of the value rather than something to leave behind.
    #[test]
    fn an_escaped_quote_inside_a_value_does_not_end_it() {
        let raw = message(&[
            "REGISTER sip:example.net SIP/2.0",
            "Authorization: Digest response=\"aaa\\\"TAIL0013\"",
        ]);
        let out = text(&redact(&raw).expect("redacted"));
        assert!(
            !out.contains("TAIL0013"),
            "the escaped tail survived: {out}"
        );
    }

    /// A message with nothing to hide is not copied, and is not marked as redacted.
    ///
    /// `redact`'s own contract, and the thing that keeps redaction off the cost of an ordinary
    /// capture: a folded `From` header must come back untouched rather than silently unfolded.
    #[test]
    fn a_message_with_no_credential_is_never_rewritten() {
        let folded = message(&[
            "INVITE sip:bob@example.net SIP/2.0",
            "From: \"Alice\"",
            " <sip:alice@example.net>;tag=abcd",
            "Subject: a response= that is not a parameter",
        ]);
        assert!(
            redact(&folded).is_none(),
            "nothing to redact must mean no copy, so a fold survives untouched"
        );
    }

    /// **The "costs nothing when off" claim, as a test that fails if the cost comes back.**
    ///
    /// The Acceptance says capture must cost nothing when off, and a version of this module took
    /// `&Bytes` — so every inbound TCP, TLS and WebSocket message was re-serialised and heap-allocated
    /// to be handed to a capture that did not exist. That is invisible to a test asserting "no file
    /// and zero counters", which is why the guard lives in one function and this asserts the closure
    /// is never called.
    #[test]
    fn no_capture_means_the_bytes_are_never_produced() {
        let meters = Meters::default();
        let produced = std::cell::Cell::new(false);

        Capture::observe_if_capturing(
            None,
            &meters,
            "127.0.0.1:5060".parse().expect("valid"),
            "127.0.0.1:5061".parse().expect("valid"),
            TransportKind::Tcp,
            Direction::In,
            || {
                produced.set(true);
                Bytes::from_static(b"OPTIONS sip:x SIP/2.0\r\n\r\n")
            },
        );

        assert!(
            !produced.get(),
            "with no capture configured the message must not even be serialised"
        );
        assert_eq!(
            meters.snapshot().capture,
            crate::counters::CaptureCounts::default(),
            "and nothing is counted"
        );
    }

    #[test]
    fn a_header_name_is_matched_without_regard_to_case() {
        let out = text(
            &redact(b"AUTHORIZATION: Digest RESPONSE=\"secret\"\r\n\r\n")
                .expect("header and parameter names are case-insensitive"),
        );
        assert!(!out.contains("secret"), "{out}");
    }

    /// The IPv4 header checksum is the one checksum §13.1 keeps. A header that already carries a
    /// correct checksum sums to zero, which is the standard way to check one.
    #[test]
    fn the_synthesised_ipv4_header_carries_a_valid_checksum() {
        let record = Record {
            seq: 1,
            at: UNIX_EPOCH,
            local: "192.0.2.1:5060".parse().unwrap(),
            peer: "192.0.2.9:5061".parse().unwrap(),
            transport: TransportKind::Udp,
            direction: Direction::Out,
            bytes: Bytes::from_static(b"OPTIONS sip:x SIP/2.0\r\n\r\n"),
            redacted: false,
        };
        let packet = synthesise(&record);
        assert_eq!(packet[0], 0x45, "IPv4 with a twenty-byte header");
        assert_eq!(packet[9], IP_PROTO_UDP);
        assert_eq!(
            ones_complement(&packet[..20]),
            0,
            "a header with a correct checksum sums to zero"
        );
        // The real ports, in the synthetic UDP header.
        assert_eq!(&packet[20..22], &5060u16.to_be_bytes());
        assert_eq!(&packet[22..24], &5061u16.to_be_bytes());
        assert_eq!(&packet[28..], record.bytes.as_ref());
    }

    #[test]
    fn a_tls_record_says_it_was_decrypted_in_process() {
        let record = Record {
            seq: 7,
            at: UNIX_EPOCH,
            local: "192.0.2.1:5061".parse().unwrap(),
            peer: "192.0.2.9:5061".parse().unwrap(),
            transport: TransportKind::Tls,
            direction: Direction::In,
            bytes: Bytes::from_static(b"SIP/2.0 200 OK\r\n\r\n"),
            redacted: true,
        };
        let comment = comment(&record);
        assert!(comment.contains("seq=7"), "{comment}");
        assert!(comment.contains("dir=in"), "{comment}");
        assert!(comment.contains("transport=TLS"), "{comment}");
        assert!(comment.contains("decrypted-in-process=yes"), "{comment}");
        assert!(comment.contains("redacted=yes"), "{comment}");
    }

    #[test]
    fn ipv6_is_synthesised_as_ipv6() {
        let record = Record {
            seq: 1,
            at: UNIX_EPOCH,
            local: "[2001:db8::1]:5060".parse().unwrap(),
            peer: "[2001:db8::2]:5060".parse().unwrap(),
            transport: TransportKind::Udp,
            direction: Direction::In,
            bytes: Bytes::from_static(b"x"),
            redacted: false,
        };
        let packet = synthesise(&record);
        assert_eq!(packet[0] >> 4, 6, "version 6");
        assert_eq!(packet[6], IP_PROTO_UDP, "next header");
        assert_eq!(packet.len(), 40 + 8 + 1);
    }
}
