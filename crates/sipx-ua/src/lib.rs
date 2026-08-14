//! SIP user agent: registration, authentication, and answering what arrives.
//!
//! This crate sits on `sipx-transport` and turns transactions into the things a phone or a
//! service actually does. Digest authentication and registration leases live here because
//! both are about *state over time* rather than about a single message, which is what
//! separates a user agent from a transaction layer.
//!
//! Dialogs and calls are the next layer up, in `sipx-call`.

//! # Without a runtime
//!
//! Digest is hashing and header text, and a caller whose decision logic touches no IO must be able
//! to use it without linking one. `default-features = false` drops the `runtime` feature and with
//! it the modules that drive a socket — `agent`, `flows`, and the error type that wraps a transport
//! failure — leaving `auth`, `challenge`, `gruu`, `identity`, `outbound`, `push` and `registrar`.
//! Identity signing and verification take caller-supplied time, authority policy, and credential
//! acquisition, so they remain usable without a runtime as well. The alternative for such a caller
//! is to write digest or identity processing a second time, and two implementations of one
//! algorithm eventually disagree about who is authenticated.
//!
//! # Stability
//!
//! The Supported Rust API is frozen for compatible v1 evolution. Existing Supported paths and
//! signatures remain source-compatible throughout major version 1; compatible additions use the
//! reservations documented on their types.
//!
//! - **Supported** — covered by the v1 compatibility contract. A breaking change waits for the next
//!   major version.
//! - **Experimental** — remains unfrozen and may change shape or be removed without a migration
//!   note. Depend on it only if you are prepared to follow it.
//!
//!
//! **Supported**: registration leases, digest authentication, authenticated caller identity, Path,
//! Service-Route, bounded Outbound registration lifetimes, push, and the subscription store plus built-in
//! dialog, registration and presence package documents selected by `sipx-call::Notifier`.
//! `S-34` gives identity its caller:
//! outbound and inbound policies in `sipx-call` select the authentication and verification
//! services. `S-29` first gave Outbound and push their callers — `sipx register --outbound` and
//! `--push-provider`/`--push-prid` — while `T-47` makes `Flows::start` own accepted flows,
//! keep-alives, refresh, independent recovery and joined cancellation for the complete lifetime.
//!
//! **Which application backs that claim, stated because the two are not the same** (`X-38`). Every
//! Registration, Outbound and push are called by `sipx-cli`, while authenticated identity is called
//! by `sipx-call`, which is itself the call framework used by `sipx-app`. The host uses only this
//! crate's answering half directly: `Host::agent_config` builds a [`Config`] to answer OPTIONS with
//! and names the listener's own address as a registrar that nothing ever sends to, so `register` is
//! never called. `X-38` defines the *call*-reachable surface as what the host uses, and registration
//! is not call-reachable in principle rather than by omission — it happens before and outside any
//! call. So the registration claim rests on `A-8`'s other rule: the CLI's promise is its command-line
//! surface, documented in `website/docs/reference/cli.md` and asserted by `tests/cli.rs`.
//! `scripts/check-app-surface.py` checks that citation rather than trusting it, so this paragraph
//! cannot rot into a claim with no caller at all. Push is earned in full: the `pn-*` parameters,
//! §8.2's answer read back, and §4.1.3's refresh through `UserAgent::woken`. Outbound's supported
//! surface is configuration plus the bounded `Flows::start` / `FlowLifetime::cancel` owner and its
//! event and cleanup reports; the CLI reaches it as the one-entry case.
//!
//! **Experimental**: `event_client` and `publication_client`. They are public and tested.
//! `event_client` is the bounded sans-I/O subscriber driven by
//! `sipx-call::EventSubscriptions`, and `sipx-call::Publications` carries the publication core and
//! exact compositor through live endpoints. The bounded `reginfo` consumer is reached by
//! `sipx peers --registrar`; no CLI command publishes, and published presence is not automatically
//! projected into later NOTIFY documents. Their Experimental API shape remains unfrozen during v1.
//!
//! The manual-pass Outbound surface remains experimental: `Flows::register`, `Flows::keepalive`,
//! `Attempt`, `UserAgent::keepalive_after`, and `UserAgent::dialog_contact`'s `ob` parameter are
//! useful protocol pieces but are exercised only below the lifetime owner, not called directly by
//! an application contract. The registrar and proxy roles remain outside this crate.
//!
//! <!-- BEGIN sipx-api-classification -->
//! **Experimental Rust API roots:**
//!
//! - [`sipx_ua::agent::UserAgent::dialog_contact`](crate::agent::UserAgent::dialog_contact)
//! - [`sipx_ua::agent::UserAgent::keepalive_after`](crate::agent::UserAgent::keepalive_after)
//! - [`sipx_ua::event_client`](crate::event_client)
//! - [`sipx_ua::flows::Attempt`](crate::flows::Attempt)
//! - [`sipx_ua::flows::Flows::keepalive`](crate::flows::Flows::keepalive)
//! - [`sipx_ua::flows::Flows::register`](crate::flows::Flows::register)
//! - [`sipx_ua::publication_client`](crate::publication_client)
//! - [`sipx_ua::reginfo`](crate::reginfo)
//! <!-- END sipx-api-classification -->

// This crate's inline test modules opt out of coverage instrumentation, so the
// published figure measures the code rather than the tests measuring it. Never set outside
// `cargo llvm-cov`, so every other build parses this and discards it. Applied by
// `./scripts/coverage-report.py --annotate`; `docs/coverage.md` states what it costs.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

#[cfg(feature = "runtime")]
pub mod agent;
pub mod auth;
pub mod challenge;
#[cfg(feature = "runtime")]
pub mod error;
pub mod event_client;
#[cfg(feature = "runtime")]
pub mod flows;
pub mod gruu;
pub mod history;
pub mod identity;
pub mod outbound;
pub mod packages;
pub mod presence;
pub mod publication_client;
pub mod push;
pub mod reginfo;
pub mod registrar;
pub mod subscribe;

#[cfg(feature = "runtime")]
pub use agent::{Config, Flow, UserAgent};
pub use auth::{Algorithm, Challenge, Credentials};
pub use challenge::{Authenticator, Presented, Reason, Verdict};
#[cfg(feature = "runtime")]
pub use error::{Error, Result};
#[cfg(feature = "runtime")]
pub use flows::{Attempt, FlowCleanup, FlowEvent, FlowLifetime, Flows};
pub use gruu::{Gruus, Kind as GruuKind};
pub use history::{RetargetError, retarget};
pub use outbound::{InstanceId, Keepalive, Power, RegId};
pub use push::{Pending, PushService, Support};
pub use registrar::{
    Lease, Outcome, PathSet, Registered, Registration, RegistrationObservation,
    RegistrationObservationError, ServiceRoute,
};
