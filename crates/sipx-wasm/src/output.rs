//! Output records and the drain obligation (`docs/specs/browser-sdk.md` §4.6).
//!
//! The kernel never calls the host — it has no imports at all (§4.1) — so everything it wants to
//! say is queued here and drained by the host with [`crate::Abi::next_output`] until that returns
//! `0`. Records are strictly FIFO, and each is framed as
//!
//! ```text
//! offset 0: u32 little-endian  record type
//! offset 4: u32 little-endian  payload length N
//! offset 8: N payload bytes
//! ```

use std::collections::VecDeque;

use crate::bounds;

/// The framed record header, in octets: type and length.
pub(crate) const HEADER_LEN: usize = 8;

/// One thing the kernel has to hand back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    /// Raw bytes of exactly one SIP message, to be sent as one WebSocket message.
    Wire(Vec<u8>),
    /// Arrange a timer.
    TimerSet {
        /// Fresh id, monotonically increasing and never reused within a handle.
        id: u64,
        /// Absolute deadline on the host's monotonic epoch.
        fire_at_ms: u64,
    },
    /// Clear a timer that has not fired.
    TimerCancel(u64),
    /// One §5.3 JSON event document.
    Event(Vec<u8>),
}

impl Record {
    /// The `u32` type tag from §4.6's table.
    #[must_use]
    pub fn type_tag(&self) -> u32 {
        match self {
            Self::Wire(_) => 1,
            Self::TimerSet { .. } => 2,
            Self::TimerCancel(_) => 3,
            Self::Event(_) => 4,
        }
    }

    /// The payload, without the eight-octet header.
    #[must_use]
    pub fn payload(&self) -> Vec<u8> {
        match self {
            Self::Wire(bytes) | Self::Event(bytes) => bytes.clone(),
            Self::TimerSet { id, fire_at_ms } => {
                let mut out = Vec::with_capacity(16);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&fire_at_ms.to_le_bytes());
                out
            }
            Self::TimerCancel(id) => id.to_le_bytes().to_vec(),
        }
    }

    /// The complete framed record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let payload = self.payload();
        // A payload cannot exceed `MAX_EVENT`, and the queue refuses anything that would; the
        // saturating conversion is here so the framing has no arithmetic that can fail rather
        // than because the value is in doubt.
        let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
        let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
        out.extend_from_slice(&self.type_tag().to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Framed size, for the queue's byte budget.
    fn framed_len(&self) -> usize {
        HEADER_LEN
            + match self {
                Self::Wire(bytes) | Self::Event(bytes) => bytes.len(),
                Self::TimerSet { .. } => 16,
                Self::TimerCancel(_) => 8,
            }
    }
}

/// The FIFO the host drains.
///
/// Both §4.9 caps live here. Reaching either means the host ignored §4.6's drain obligation, so
/// the queue reports the overflow and the kernel poisons itself rather than dropping a record and
/// leaving the page's view of the call quietly wrong.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    records: VecDeque<Record>,
    bytes: usize,
}

impl Queue {
    /// Append a record, or report that the host has stopped draining.
    pub(crate) fn push(&mut self, record: Record) -> Result<(), Overflow> {
        let framed = record.framed_len();
        if self.records.len() >= bounds::MAX_QUEUED_RECORDS {
            return Err(Overflow::Records);
        }
        if self.bytes.saturating_add(framed) > bounds::MAX_QUEUED_BYTES {
            return Err(Overflow::Bytes);
        }
        self.bytes = self.bytes.saturating_add(framed);
        self.records.push_back(record);
        Ok(())
    }

    /// Take the oldest record, or `None` when drained.
    pub(crate) fn pop(&mut self) -> Option<Record> {
        let record = self.records.pop_front()?;
        self.bytes = self.bytes.saturating_sub(record.framed_len());
        Some(record)
    }

    /// How many records are waiting.
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// Discard everything still queued, reporting how many records went with it.
    ///
    /// Used by `sipx_kernel_free`: §9.6's `BSDK-STATE-6` requires that no output record survives
    /// the free, and §4.11 counts what was dropped.
    pub(crate) fn drain_count(&mut self) -> usize {
        let dropped = self.records.len();
        self.records.clear();
        self.bytes = 0;
        dropped
    }
}

/// Which §4.9 queue cap was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// More than 256 records.
    Records,
    /// More than 256 KiB of framed records.
    Bytes,
}

impl Overflow {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::Records => "queued output records exceeded 256",
            Self::Bytes => "queued output records exceeded 256 KiB",
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

    #[test]
    fn timer_set_frames_as_bsdk_out_2() {
        // `docs/specs/browser-sdk.md` §9.3, `BSDK-OUT-2`: timer id 1, fire_at_ms 500.
        let record = Record::TimerSet {
            id: 1,
            fire_at_ms: 500,
        };
        let expected: Vec<u8> = vec![
            0x02, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xf4, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(record.encode(), expected);
        assert_eq!(record.encode().len(), 24);
    }

    #[test]
    fn queue_refuses_a_host_that_stopped_draining() {
        let mut queue = Queue::default();
        for _ in 0..bounds::MAX_QUEUED_RECORDS {
            queue.push(Record::TimerCancel(1)).expect("under the cap");
        }
        assert_eq!(queue.push(Record::TimerCancel(1)), Err(Overflow::Records));
    }

    #[test]
    fn queue_refuses_a_byte_budget_overrun_before_the_record_count() {
        let mut queue = Queue::default();
        // Four 64 KiB messages already exceed the 256 KiB budget once framed, long before 256
        // records.
        for _ in 0..4 {
            let _ = queue.push(Record::Wire(vec![0u8; 64 * 1024]));
        }
        assert_eq!(
            queue.push(Record::Wire(vec![0u8; 64 * 1024])),
            Err(Overflow::Bytes)
        );
        assert!(queue.len() < bounds::MAX_QUEUED_RECORDS);
    }
}
