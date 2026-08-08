//! ABI error codes (`docs/specs/browser-sdk.md` §4.10).
//!
//! These report **host-contract violations**, never protocol outcomes. Malformed SIP arriving in
//! [`crate::Abi::input_bytes`] returns `0`: hostile network input is a value, handled inside the
//! kernel with typed errors and counters, exactly as the native stack handles it. A SIP request
//! that fails is reported through an event, not through a return code.

/// A host-contract violation, as the negative `i32` the ABI returns.
///
/// Values are stable within `sipx.browser.v1`: a new code may be appended, an existing value
/// never changes meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(i32)]
#[non_exhaustive]
pub enum Error {
    /// Unknown, freed or foreign handle.
    InvalidHandle = -1,
    /// Pointer/length outside the exported memory, or not obtained as §4.4 requires.
    BadPointer = -2,
    /// A control-plane buffer is not UTF-8.
    Utf8 = -3,
    /// A control-plane buffer is not RFC 8259 JSON.
    Json = -4,
    /// JSON valid, document not a §5 command: unknown verb, missing or ill-typed field.
    Schema = -5,
    /// Command is well-formed but illegal in the current state.
    State = -6,
    /// An input exceeds a §4.9 bound.
    Bounds = -7,
    /// The entropy pool cannot cover the operation.
    Entropy = -8,
    /// Linear memory growth failed.
    Oom = -9,
    /// A countable resource cap in §4.9.
    Limit = -10,
    /// `now_ms` regressed.
    Time = -11,
    /// A prior internal fault; the instance is dead.
    Poisoned = -12,
}

impl Error {
    /// The wire value: the negative integer an entry point returns.
    #[must_use]
    pub fn code(self) -> i32 {
        self as i32
    }

    /// The magnitude, for the packed-buffer error encoding in §4.2.
    #[must_use]
    pub fn magnitude(self) -> u32 {
        self.code().unsigned_abs()
    }

    /// The short stable token used in an `"outcome"` event's `"error"` object and in the
    /// snapshot's rejection counters.
    ///
    /// Distinct from [`Error::code`] because §5.3's error object is read by application code,
    /// which should not have to carry a table of integers to know what refused it.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::InvalidHandle => "invalid-handle",
            Self::BadPointer => "bad-pointer",
            Self::Utf8 => "utf8",
            Self::Json => "json",
            Self::Schema => "schema",
            Self::State => "state",
            Self::Bounds => "bounds",
            Self::Entropy => "entropy",
            Self::Oom => "oom",
            Self::Limit => "limit",
            Self::Time => "time",
            Self::Poisoned => "poisoned",
        }
    }

    /// Every code, in declaration order. Used by the snapshot to render its rejection counts
    /// with a stable field order.
    pub(crate) const ALL: [Self; 12] = [
        Self::InvalidHandle,
        Self::BadPointer,
        Self::Utf8,
        Self::Json,
        Self::Schema,
        Self::State,
        Self::Bounds,
        Self::Entropy,
        Self::Oom,
        Self::Limit,
        Self::Time,
        Self::Poisoned,
    ];
}

/// The result of an entry point that reports through §4.10.
pub type Result<T> = core::result::Result<T, Error>;
