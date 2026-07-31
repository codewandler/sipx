//! The signalling path's losses, read as one thing.
//!
//! `docs/specs/sip-transport.md` §12.1 requires every discard in the signalling path to be counted.
//! §12.3 says which crates that path is, and why the atomics behind those counts are still two sets
//! while the *reading* of them is one.
//!
//! # Why a joined reading rather than joined storage
//!
//! `sipx-transport` cannot depend on `sipx-call` — the dependency runs the other way and reversing
//! it would put the dialog layer underneath the socket. So the counters themselves stay where the
//! events happen: the transport's in `sipx_transport::Counters`, the dispatcher's in
//! [`DispatchCounts`]. That much is forced.
//!
//! What was *not* forced, and what `X-54` is about, is that an operator had to know the crate
//! boundary to ask. `Handle::counters` and `Calls::counts` were two snapshots that nothing outside
//! each crate's own tests ever read, so M12's clause — every discard counted **and exportable next
//! to a capture** — was two features that existed separately. [`SignallingCounts`] is the one
//! reading, and `sipx --counters` (`crates/sipx-cli`) is the export beside `--capture`.
//!
//! # The join embeds, it does not recount
//!
//! [`SignallingCounts::transport`] is exactly what [`sipx_transport::Handle::counters`] returns,
//! copied and not re-derived, for the reason `sipx_transport::Counters::shed` already states about
//! itself: two tallies of one event eventually disagree, and then neither can be trusted. The same
//! rule is why this type has no arithmetic of its own beyond [`SignallingCounts::any_loss`], which
//! is a disjunction of the two halves' own answers rather than a third opinion.
//!
//! # Why `dispatch` is an `Option` and not a zeroed struct
//!
//! An endpoint with no dispatcher running has not dispatched nothing — it has not been asked. Those
//! are different claims and a zero cannot tell them apart, which is the failure `X-18` deleted
//! `DiscardCounts::adopted_late` over: a counter structurally stuck at zero tells an operator "this
//! never happens", and that is worse than silence.

use sipx_transport::{Counters, Handle};

use crate::dispatch::{Calls, DispatchCounts};

/// Every loss in the signalling path, from both crates that own one (§12.3).
///
/// Built by [`SignallingCounts::of`] for an endpoint alone, or [`SignallingCounts::with_dispatcher`]
/// when a dispatcher is running on it. Both are plain snapshots taken at the moment they are asked
/// for: no metrics library, no background aggregation, and nothing here reads a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct SignallingCounts {
    /// What the transport counted, embedded unaltered.
    pub transport: Counters,
    /// What the dispatcher counted, or `None` when no dispatcher is running on this endpoint.
    pub dispatch: Option<DispatchCounts>,
}

impl SignallingCounts {
    /// The losses an endpoint alone can report.
    ///
    /// [`SignallingCounts::dispatch`] is `None`: no dispatcher has been named, so nothing is claimed
    /// about the dialog layer rather than zero being claimed about it.
    #[must_use]
    pub fn of(endpoint: &Handle) -> Self {
        Self {
            transport: endpoint.counters(),
            dispatch: None,
        }
    }

    /// The losses an endpoint and the dispatcher running on it report together.
    ///
    /// The two halves are read one after the other and not under a shared lock, so a message being
    /// dispatched as this is called can be counted by one half and not yet the other. That is the
    /// same skew §12.2 already states for the transport's own counters, and it is the honest
    /// trade: a lock spanning both would put the dialog layer's mutex in the socket's path.
    #[must_use]
    pub fn with_dispatcher(endpoint: &Handle, calls: &Calls) -> Self {
        Self {
            transport: endpoint.counters(),
            dispatch: Some(calls.counts()),
        }
    }

    /// Whether anything in the signalling path has been thrown away.
    ///
    /// A disjunction of the two halves' own answers, never a third tally. A `None` dispatcher
    /// contributes nothing, because an unasked question is not a negative answer.
    #[must_use]
    pub fn any_loss(&self) -> bool {
        self.transport.any_loss() || self.dispatch.is_some_and(|dispatch| dispatch.total() > 0)
    }
}
