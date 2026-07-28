//! Transactions (RFC 3261 §17, amended by RFC 6026).
//!
//! Four state machines, all of them sans-IO: they read no clock, own no socket and spawn
//! nothing. Time arrives as [`Timer`] inputs and leaves as [`Output::SetTimer`]. That is what
//! makes retransmission behaviour — the part of SIP that is hardest to get right and hardest
//! to test — reachable from an ordinary unit test with no sleeping and no flakiness.
//!
//! The state tables in `docs/specs/sip-transaction.md` are the specification; this code is
//! written from them and the tests walk them row by row.

mod client;
mod key;
mod layer;
mod server;
mod timing;

pub use client::{ClientState, ClientTransaction};
pub use key::TransactionKey;
pub use layer::{Dispatch, TransactionLayer, sent_messages, tu_events};
pub use server::{ServerState, ServerTransaction};
pub use timing::{Timer, Timers};

use std::time::Duration;

use crate::message::{Message, Request, Response};

/// Something the driver must do on the transaction's behalf.
///
/// Order matters and is preserved: a `Send` always precedes the `SetTimer` that will
/// retransmit it, so a retransmission timer can never start before the thing it retransmits
/// has gone out.
#[derive(Debug, Clone)]
pub enum Output {
    /// Put this message on the wire.
    Send(Box<Message>),
    /// Arrange for [`Timer`] to fire after this long.
    SetTimer {
        /// Which timer.
        timer: Timer,
        /// How long from now.
        after: Duration,
    },
    /// Cancel a timer that has not fired.
    ClearTimer(Timer),
    /// Hand this to the transaction user.
    ToTu(Box<TuEvent>),
    /// The transaction is over; the layer above should drop it.
    Terminated(Reason),
}

impl Output {
    fn send(message: Message) -> Self {
        Self::Send(Box::new(message))
    }

    fn to_tu(event: TuEvent) -> Self {
        Self::ToTu(Box::new(event))
    }
}

/// What the transaction has to tell the transaction user.
#[derive(Debug, Clone)]
pub enum TuEvent {
    /// A request arrived that the TU has not seen before.
    ///
    /// A retransmission never produces this: the transaction answers those itself. Without
    /// that, a UDP peer that misses one response makes the application process the same
    /// REGISTER seven times.
    Request(Box<Request>),
    /// A response arrived.
    ///
    /// Under RFC 6026 a 2xx to an INVITE can arrive more than once — a forking proxy produces
    /// exactly that — and each one is delivered. Two 200s for one INVITE is a fork, not a bug.
    Response(Box<Response>),
    /// An ACK for a 2xx response, which is a separate transaction and therefore the TU's
    /// business (RFC 3261 §13.2.2.4, RFC 6026).
    Ack(Box<Request>),
    /// No answer within 64·T1.
    Timeout,
    /// The transport could not deliver.
    TransportError,
}

/// Why a transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// It ran its course.
    Completed,
    /// Nothing was heard within 64·T1.
    Timeout,
    /// The transport failed.
    TransportError,
}

/// Whether the transport delivers reliably, which decides half the timer behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reliability {
    /// TCP, TLS, WebSocket: no retransmission timers, and the absorption timers fire at once.
    Reliable,
    /// UDP: retransmit until answered.
    Unreliable,
}

impl Reliability {
    /// Whether this transport retransmits.
    #[must_use]
    pub fn is_reliable(self) -> bool {
        matches!(self, Self::Reliable)
    }
}
