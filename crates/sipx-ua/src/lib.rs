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
//! **Supported**: registration leases, digest authentication, authenticated caller identity, Path,
//! Service-Route, registering as one Outbound flow, and push. `S-34` gives identity its caller:
//! outbound and inbound policies in `sipx-call` select the authentication and verification
//! services. `S-29` is what gives Outbound and push their callers — `sipx register --outbound` and
//! `--push-provider`/`--push-prid` — and it is why `X-37` had demoted their compliance rows in the
//! first place.
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
pub mod history;
pub mod identity;
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
pub use history::{RetargetError, retarget};
pub use outbound::{InstanceId, Keepalive, Power, RegId};
pub use push::{Pending, PushService, Support};
pub use registrar::{Lease, Outcome, PathSet, Registered, Registration, ServiceRoute};
