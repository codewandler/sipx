//! RTP and RTCP (RFC 3550).
//!
//! Two things here earn their tests. The **sequence number is 16 bits** and wraps every
//! twenty-odd minutes at speech packet rates, so the wrap is an ordinary event and any
//! comparison that uses `<` is wrong. And the **jitter buffer** trades latency for order:
//! packets arrive late, early, twice or not at all, and audio needs them evenly spaced.
//!
//! Packet parsing rejects rather than guesses. A decoder that reads a malformed packet
//! optimistically plays header bytes as audio, which is heard as a loud click.
//!
//! # Stability
//!
//! sipx is pre-1.0, so **neither word below means frozen**. `1.0.0` is what freezes an API, and its
//! predicates are in `docs/roadmap.md`. Until then:
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry saying what
//!   to do instead. New enum variants and new struct fields may still appear in a minor release, so a
//!   downstream `match` should carry a `_` arm.
//! - **Experimental** — may change shape or be removed without a migration note. Depend on it only if
//!   you are prepared to follow it.
//!
//!
//! **Supported.** RTP, RTCP, the jitter buffer, quality statistics, SRTP and RFC 4733 DTMF are all
//! reachable from a call and exercised by it.

pub mod dtmf;
pub mod jitter;
pub mod packet;
pub mod quality;
pub mod rtcp;
pub mod srtp;

pub use dtmf::{Digit, Event as DtmfEvent, Receiver as DtmfReceiver};
pub use jitter::JitterBuffer;
pub use packet::{HEADER_LEN, Packet, RtpError, sequence_distance, sequence_is_newer};
pub use quality::{Quality, ntp_now, round_trip};
pub use rtcp::{
    ReceiverReport, ReportBlock, Rtcp, RtcpError, Sdes, SdesChunk, SdesItem, SenderReport,
    StreamStats,
};
pub use srtp::{Context as SrtpContext, SrtpError};
