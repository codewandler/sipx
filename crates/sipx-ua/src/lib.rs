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
//! failure — leaving `auth`, `challenge`, `gruu`, `outbound`, `push` and `registrar`. The
//! alternative for such a caller is to write digest a second time, and two implementations of one
//! algorithm eventually disagree about who is authenticated.
//!
//! # Stability
//!
//! sipx is pre-1.0, so **neither word below means frozen**. `1.0.0` is what freezes an API, and its
//! predicates are in `docs/roadmap.md`. Until then:
//!
//! - **Supported** — meant to be depended on. Breaking changes get a `CHANGELOG.md` entry saying what
//!   to do instead. New enum variants and new struct fields may still appear in a minor release, so a
//!   downstream `match` should carry a `_` arm.
//! - **Experimental** — may change shape or be removed without a migration note. Depend on it only if
//!   you are prepared to follow it.
//!
//!
//! **Supported**: registration leases, digest authentication, Path, Service-Route, registering as
//! one Outbound flow, and push. `S-29` is what gives the last two a caller above this crate —
//! `sipx register --outbound` and `--push-provider`/`--push-prid` — and it is why `X-37` had
//! demoted their compliance rows in the first place. Push is earned in full: the `pn-*` parameters,
//! §8.2's answer read back, and §4.1.3's refresh through `UserAgent::woken`. Outbound is earned
//! only as far as the registration goes, which is what the wording above says and no further.
//!
//! **Experimental**: `presence`, `subscribe` and `packages`. They are public, tested and reachable from
//! nothing above this crate — no `sipx-cli` command subscribes or publishes, and nothing in the
//! workspace receives a SUBSCRIBE or PUBLISH off a socket. Their shape has never been constrained by a
//! caller, which is exactly when an API is still soft.
//!
//! By that same rule, and named here rather than left for a reader to discover: the rest of
//! Outbound is experimental too. `Flows` and `Attempt` — one registration per outbound proxy,
//! each flow failing independently under §4.5's backoff — plus `UserAgent::keepalive_after`
//! (§4.4) and `UserAgent::dialog_contact`'s `ob` parameter (§4.3) are exercised by this crate's
//! own tests and by nothing above them. `sipx register` places a single flow and does not hold it
//! open, so those shapes have never been constrained by a caller either.

#[cfg(feature = "runtime")]
pub mod agent;
pub mod auth;
pub mod challenge;
#[cfg(feature = "runtime")]
pub mod error;
#[cfg(feature = "runtime")]
pub mod flows;
pub mod gruu;
pub mod outbound;
pub mod packages;
pub mod presence;
pub mod push;
pub mod registrar;
pub mod subscribe;

#[cfg(feature = "runtime")]
pub use agent::{Config, Flow, UserAgent};
pub use auth::{Algorithm, Challenge, Credentials};
pub use challenge::{Authenticator, Presented, Reason, Verdict};
#[cfg(feature = "runtime")]
pub use error::{Error, Result};
#[cfg(feature = "runtime")]
pub use flows::{Attempt, Flows};
pub use gruu::{Gruus, Kind as GruuKind};
pub use outbound::{InstanceId, Keepalive, Power, RegId};
pub use push::{Pending, PushService, Support};
pub use registrar::{Lease, Outcome, PathSet, Registered, Registration, ServiceRoute};
