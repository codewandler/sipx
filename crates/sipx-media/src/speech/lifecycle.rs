//! Warm-up, readiness, loss and cancellation — and what SIP never sees
//! (`docs/specs/speech-providers.md` §7).
//!
//! The whole point of this module is a *disjointness*. Provider lifecycle travels on the speech
//! event stream and nowhere else: no value here is representable as, or reportable through, a SIP
//! status code, and none of them is reachable from a dialog or transaction. A call stays
//! established through warm-up failure, loss and fallback, and ending it because speech failed is
//! an application decision rather than stack behaviour.
//!
//! The other direction matters as much. SIP teardown reaches a session only as a cancellation with
//! reason [`CancelReason::CallEnded`] — never as a provider failure — so a consumer can always
//! answer "did the call fail, or did speech fail?" from the event type alone.

use std::fmt;

/// Why work was cancelled (§7).
///
/// One set shared by both contracts, closed for meaning and open for extension: a consumer writes
/// a wildcard arm, and a new reason names a new fact rather than reinterpreting an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CancelReason {
    /// The application asked for it.
    Application,
    /// A synthesis `Enqueue` with `replace = true` displaced this request (§6).
    Replaced,
    /// The call ended. This is how SIP teardown appears inside a session, and it is a
    /// cancellation rather than a failure — see the module documentation.
    CallEnded,
    /// The provider's engine or execution device became unavailable.
    ///
    /// The reason token and the `Lost` output name the same fact, deliberately under different
    /// names: one is why this work stopped, the other is what happened to the session.
    ProviderLost,
    /// The session failed, and this work was open when it did.
    SessionFailed,
    /// The host is shutting down.
    Shutdown,
}

impl fmt::Display for CancelReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Application => "application",
            Self::Replaced => "replaced",
            Self::CallEnded => "call ended",
            Self::ProviderLost => "provider lost",
            Self::SessionFailed => "session failed",
            Self::Shutdown => "shutdown",
        })
    }
}

/// Why a session or a request failed terminally (§5, §6, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailureCause {
    /// The warm-up deadline fired before the provider signalled readiness (§7).
    WarmupTimeout,
    /// The driver sent an input the contract does not allow here — a `Frame` after `Flush`, or an
    /// input variant the provider's descriptor never declared (§5, §9).
    ///
    /// A provider that does not recognise an input fails the session with this rather than
    /// guessing what was meant.
    ProtocolViolation,
    /// The provider's own engine failed while producing.
    EngineFailed,
}

impl fmt::Display for FailureCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::WarmupTimeout => "warm-up timeout",
            Self::ProtocolViolation => "protocol violation",
            Self::EngineFailed => "engine failed",
        })
    }
}

/// What became unavailable when a session was lost (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LossCause {
    /// The provider's engine went away.
    Engine,
    /// The execution device went away.
    Device,
}

impl fmt::Display for LossCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Engine => "engine",
            Self::Device => "device",
        })
    }
}

/// Which deadline the driver fired (§7, §8).
///
/// Deadlines are the only time a session learns about elapsed wall-clock, and it learns about it
/// as an *input*: the provider reads no clock. A fired deadline carries a generation, and one with
/// a stale generation is ignored (§2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DeadlineKind {
    /// The bound on reaching readiness. Firing it fails the session with
    /// [`FailureCause::WarmupTimeout`].
    Warmup,
    /// The bound on stopping. Firing it makes the driver abort and report an aborted stop, which
    /// is a reportable provider defect rather than a hang.
    Drain,
}

impl fmt::Display for DeadlineKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Warmup => "warm-up",
            Self::Drain => "drain",
        })
    }
}
