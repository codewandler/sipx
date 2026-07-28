//! RTP and RTCP (RFC 3550).
//!
//! Two things here earn their tests. The **sequence number is 16 bits** and wraps every
//! twenty-odd minutes at speech packet rates, so the wrap is an ordinary event and any
//! comparison that uses `<` is wrong. And the **jitter buffer** trades latency for order:
//! packets arrive late, early, twice or not at all, and audio needs them evenly spaced.
//!
//! Packet parsing rejects rather than guesses. A decoder that reads a malformed packet
//! optimistically plays header bytes as audio, which is heard as a loud click.

pub mod dtmf;
pub mod jitter;
pub mod packet;

pub use dtmf::{Digit, Event as DtmfEvent, Receiver as DtmfReceiver};
pub use jitter::JitterBuffer;
pub use packet::{HEADER_LEN, Packet, RtpError, sequence_distance, sequence_is_newer};
