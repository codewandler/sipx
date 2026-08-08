//! What the public integration guide promises a library consumer about naming a destination.
//!
//! The guide is the only place an application author who never runs the CLI is told how a name
//! becomes an address. When resolution lived in `sipx-cli` the honest advice was "look it up
//! yourself and hand in an address", and that sentence outlives the reason for it: it reads as
//! current advice long after the library grew the resolver, and an application that follows it
//! pins one address, resolves without a deadline, and picks its own TLS verification name.
//!
//! So the retirement is asserted rather than remembered. These are content assertions, not API
//! ones — they fail when the page regresses, which is exactly when nothing else would notice.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

const GUIDE: &str = include_str!("../../../website/docs/guides/integrate-existing-system.md");

/// Markdown wraps prose at the column, not at the sentence, so a phrase spanning two source lines
/// is one phrase to a reader and two to `contains`. Collapsing every whitespace run makes the
/// assertions read the page the way the reader does.
fn flowed(page: &str) -> String {
    page.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_guide_does_not_tell_an_application_to_resolve_the_proxy_itself() {
    let page = flowed(GUIDE);
    for retired in [
        "Resolve the outermost proxy in the application",
        "pass that address as the `Target`",
    ] {
        assert!(
            !page.contains(retired),
            "the integration guide still instructs applications to resolve names themselves: \
             {retired:?}"
        );
    }
}

#[test]
fn the_guide_points_applications_at_the_library_resolver() {
    let page = flowed(GUIDE);
    for stated in [
        // The resolver an application actually calls, and the constructor that resolves a
        // registrar for it.
        "sipx_transport::destination::Resolver",
        "sipx_ua::Config::resolved",
        // What the lookup follows, and the records it goes through.
        "RFC 3263",
        "NAPTR",
        "SRV",
        // Both bounds, because a per-question wait is not a whole-resolution one.
        "two seconds",
        "eight seconds",
        // The two guarantees an application would otherwise have to reproduce by hand, and get
        // wrong: no cleartext fallback for a secure URI, and the name — not the resolved address
        // — as the identity a certificate is checked against.
        "sips:",
        "verification identity",
    ] {
        assert!(
            page.contains(stated),
            "the integration guide does not state {stated:?} about resolving a named destination"
        );
    }
}
