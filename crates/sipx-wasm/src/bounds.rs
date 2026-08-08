//! Resource bounds (`docs/specs/browser-sdk.md` §4.9).
//!
//! Every constant here is a contract value, not a tuning knob: changing one changes what the
//! kernel promises a page, and §9's vectors are written against these numbers.

/// Declared maximum linear memory, in bytes.
///
/// Stated for the module's memory declaration and for the record; the kernel enforces the
/// bounds below rather than waiting for an allocation failure at 32 MiB.
pub const MAX_LINEAR_MEMORY: usize = 32 * 1024 * 1024;

/// Live handles per instantiation. A seventeenth [`crate::Abi::kernel_new`] is `E_LIMIT`.
pub const MAX_HANDLES: usize = 16;

/// One SIP message, inbound or outbound. Inbound over the bound is `E_BOUNDS` and is not parsed;
/// outbound over the bound is a kernel defect and poisons the instance.
pub const MAX_SIP_MESSAGE: usize = 64 * 1024;

/// One command document, checked **before** JSON parsing.
pub const MAX_COMMAND: usize = 32 * 1024;

/// One SDP body inside a command. Over the bound is a typed refusal, not an ABI error.
pub const MAX_SDP: usize = 16 * 1024;

/// One event document. The kernel must never emit a larger one; truncation is forbidden.
pub const MAX_EVENT: usize = 32 * 1024;

/// Entropy pool capacity, in octets.
pub const ENTROPY_CAPACITY: usize = 1024;

/// Entropy low-water mark: below this the kernel asks for more.
pub const ENTROPY_LOW_WATER: usize = 64;

/// Pending timers. Exceeding is a kernel defect.
pub const MAX_PENDING_TIMERS: usize = 128;

/// Queued output records. Reachable only by a host that ignores the §4.6 drain obligation.
pub const MAX_QUEUED_RECORDS: usize = 256;

/// Queued output bytes, across all records.
pub const MAX_QUEUED_BYTES: usize = 256 * 1024;

/// Concurrent calls. A ninth outbound `"dial"` is refused `call-limit`; a ninth inbound INVITE
/// is answered `486 Busy Here`.
pub const MAX_CALLS: usize = 8;
