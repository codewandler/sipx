//! Command policy over the library's bounded destination resolver.
//!
//! Resolution itself lives in [`sipx_transport::destination`], where an application can reach it
//! too: a capability that stops at the phone is invisible to the one predicate this repository
//! cannot self-prove. What stays here is what is genuinely the command layer's — the transport
//! selection a flag made explicit, the `--tls-server-name` override, and the mapping from a
//! resolution failure to a process exit code.

use std::time::Duration;

use sipx_sip::Uri;
use sipx_transport::Target;

use crate::output::Exit;

pub(crate) use sipx_transport::destination::{Error, MAX_ATTEMPTS, first};

/// A resolver that also applies this command's verification and transport policy.
#[derive(Debug)]
pub(crate) struct Resolver(sipx_transport::destination::Resolver);

impl Resolver {
    /// A system resolver whose own deadlines fit inside a command's remaining budget.
    ///
    /// `P-26`: the budget is not optional at the call site. The library's `system()` is private
    /// for the same reason — a command that gains a deadline later cannot keep an unbounded
    /// resolver by forgetting to say so.
    pub(crate) fn within(budget: Option<Duration>) -> Self {
        Self(sipx_transport::destination::Resolver::within(budget))
    }

    /// The same nameserver client, and the same cache, under one attempt's budget.
    ///
    /// `scenario` is one process placing many calls, each with its own deadline; building a client
    /// per `dial` would throw away what the previous lookups established — including the negative
    /// answers — between calls the actor is expected to place back to back.
    pub(crate) fn narrowed(&self, budget: Option<Duration>) -> Self {
        Self(self.0.narrowed(budget))
    }

    /// Resolve the request URI or an explicit next hop without changing the request URI.
    pub(crate) async fn resolve(
        &self,
        uri: &Uri,
        next_hop: Option<&str>,
        selection: crate::signalling::Selection,
        options: &crate::cli::SignallingOptions,
    ) -> Result<Vec<Target>, Error> {
        // The resolver already keeps this as the verification identity of every secure candidate.
        // It is named again here because `--tls-server-name` may replace it, and that is the one
        // decision this layer owns: an override is operator input and is validated as such.
        let identity = uri.host().map(ToString::to_string).unwrap_or_default();
        let candidates = self.0.resolve(uri, next_hop, selection.requested()).await?;

        candidates
            .into_iter()
            .map(|target| selection.resolved_target(options, target, &identity))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|message| Error::Input { message })
    }
}

/// Keep usage, DNS failure and deadline exits distinct.
pub(crate) fn exit(error: &Error) -> Exit {
    match error.kind() {
        sipx_transport::destination::Kind::Input => Exit::Usage,
        sipx_transport::destination::Kind::Timeout => Exit::Timeout,
        _ => Exit::Failed,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    use sipx_transport::dns::ResolutionError;

    #[test]
    fn outbound_command_inventory_uses_the_shared_resolver() {
        for (name, source) in [
            ("dial", include_str!("dial.rs")),
            ("register", include_str!("register.rs")),
            ("load", include_str!("load.rs")),
            ("peers", include_str!("peers.rs")),
            ("scenario", include_str!("scenario.rs")),
        ] {
            assert!(
                source.contains(".resolve("),
                "outbound command {name} bypasses destination::Resolver"
            );
            // `P-26`: the budget is not optional at the call site. `system()` is private for this
            // reason, and naming it here keeps the reason findable from the command that would
            // have wanted it.
            assert!(
                source.contains("Resolver::within(") || source.contains(".narrowed("),
                "outbound command {name} resolves without stating the deadline it resolves under"
            );
        }
    }

    #[test]
    fn resolution_deadlines_and_failures_have_distinct_exits() {
        let timeout = Error::Resolution(ResolutionError::ResolutionTimeout);
        let failure = Error::Resolution(ResolutionError::LookupUnavailable {
            query: "A/AAAA example.test".to_owned(),
        });
        let usage = Error::Input {
            message: "bad target".to_owned(),
        };
        let setup = Error::Setup {
            message: "no nameservers".to_owned(),
        };
        assert_eq!(exit(&timeout), Exit::Timeout);
        assert_eq!(exit(&failure), Exit::Failed);
        assert_eq!(exit(&usage), Exit::Usage);
        assert_eq!(exit(&setup), Exit::Failed);
    }
}
