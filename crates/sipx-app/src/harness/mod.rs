//! The deterministic harness (story `A-7`).
//!
//! The apparatus every behaviour claim about the host rests on. It drives the host's decision
//! logic — the interpreter's input and output, timers, redelivery, declared failure semantics —
//! with **fake time, a scripted app and scripted call events**. No sockets, no engine, no transport
//! endpoint, and no clock. The slow app, the flapping app and the absent app are ordinary test
//! cases rather than things nobody can reproduce.
//!
//! This is the sans-IO discipline of the kernel applied one layer up, and the
//! [design](../../../../docs/designs/app-host.md) says it is built *with* the first host code
//! rather than after it. It is startable today because it needs only the contract's vectors: when
//! The runner drives `sipx-app-protocol::Interpreter` directly; fake and production bindings share
//! one instruction execution path.
//!
//! ## The layers
//!
//! | Module | What it is |
//! |---|---|
//! | [`time`] | `Virtual` — the only clock, with a zero a real instant does not have |
//! | [`contract`] | re-exports of the protocol vocabulary plus observable driver effects |
//! | [`policy`] | §9.2's declared failure semantics, with the spec's defaults |
//! | [`binding`] | the app, as a trait a real socket cannot honestly implement |
//! | [`scenario`] | a scenario as data, and the discrete-event loop that runs it |
//! | [`vectors`] | §11's `AC-1`…`AC-9` and a scenario per failure knob, shared by `A-2`/`A-4` |
//!
//! ## Why a real socket cannot get in
//!
//! [`binding::Binding`] does not return what an app said after waiting for it — it returns what the
//! app *will* say and **how long that will take**. A real HTTP client cannot answer the second half
//! before making the call, so it cannot implement the trait in good faith. Combined with a clock
//! that has no `now()`, a scenario whose outcome depends on the machine running it is not
//! discouraged here; it cannot be written down. That is acceptance point 4, enforced by types
//! rather than by review.

pub mod binding;
pub mod contract;
pub mod policy;
pub mod scenario;
pub mod time;
pub mod vectors;

pub use binding::{Binding, Outcome, Reply, ScriptedApp};
pub use contract::{
    DialOutcome, Document, Effect, EndCause, Event, EventKind, Gather, GatherReason, Instruction,
    Source, Verb,
};
pub use policy::{Failure, FailurePolicy, OnFailure};
pub use scenario::{Conclusion, EVENT_QUEUE, Run, Scenario, Step};
pub use time::Virtual;
