//! Sans-IO SIP core.
//!
//! This crate implements SIP (RFC 3261) as pure state machines: message parsing and
//! serialization, the client and server transaction FSMs, and dialog identity and state.
//! It performs **no I/O**, spawns no tasks, and reads no clock. Time enters as
//! [`Input::TimerFired`] and leaves as [`Output::SetTimer`]; bytes enter as
//! [`Input::Received`] and leave as [`Output::Send`].
//!
//! That separation is deliberate: every hard part of SIP — retransmission timing,
//! transaction matching, malformed input handling — becomes testable without sockets and
//! fuzzable without a runtime. Async transports live in `sipx-transport`.

/// An event fed into the stack.
#[derive(Debug)]
#[non_exhaustive]
pub enum Input {}

/// An action the driver must perform on the stack's behalf.
#[derive(Debug)]
#[non_exhaustive]
pub enum Output {}
