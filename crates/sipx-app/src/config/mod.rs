//! The host configuration document (story `A-1`).
//!
//! One document describes a running host: its listeners, its apps, what each app is granted, and
//! what a slow, wrong or absent app does to a live call. The spec is
//! [`specs/host-config.md`](../../../../docs/specs/host-config.md) and it is normative — this
//! module is its reader, and [`vectors`] is the set of documents that pins what the page means.
//!
//! Three things are load-bearing and are worth stating here as well as there:
//!
//! 1. **The failure knobs are not defined here.** They are the contract's §9.2, and this module
//!    reads them through [`crate::harness::policy`] — one definition in the crate, whose defaults
//!    already have a test of their own. A knob added to §9.2 fails a test here rather than becoming
//!    a key nobody can set.
//! 2. **Unknown keys are refused, not ignored** — the opposite of the contract's wire rule, and
//!    deliberately so: an ignored `on_5xxx` is an operator who declared `hangup` and silently got
//!    `continue`.
//! 3. **Nothing here is secret.** Keys that concern secrets take a *name*; the grammar of a name
//!    (§4.4) is narrow enough that a pasted key does not fit through it. The document is
//!    committable, and that is a property of the schema rather than of the author's care.

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;

use crate::harness::policy::{Failure, FailurePolicy};

mod running;
mod schema;
mod syntax;
pub mod vectors;

pub use running::{Admission, AppPolicy, Running};

// The list of normative points is deliberately **not** here. §3 of the spec is the source of
// truth for what they are and which vector pins which, and a copy of that list in this crate would
// be a second claim that can drift from the first — silently, and in the direction that matters:
// the page grows a point and the coverage check does not notice. `tests/config_vectors.rs` parses
// the page instead.

/// The keys of an `[app.<name>.on_failure]` table.
///
/// Derived from the contract's own [`Failure`] enumeration rather than written out a second time,
/// which is what makes "byte-identical to §9.2" a property of the code instead of a promise in a
/// comment. `timeout_ms` leads because it is the one knob that is a duration rather than an action.
#[must_use]
pub fn failure_keys() -> Vec<&'static str> {
    let mut keys = vec!["timeout_ms"];
    keys.extend(Failure::all().iter().map(|failure| failure.knob()));
    keys
}

/// Why a document was refused.
///
/// Whole or nothing: there is no partially loaded configuration, so one of these means the running
/// host is exactly as it was. The [`code`](Self::code) is the machine-readable half — a closed set
/// (§5 of the spec) a vector can name and an operator can search for — and the message is the
/// human half.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    code: &'static str,
    line: Option<usize>,
    message: String,
}

impl ConfigError {
    /// Every code a refusal can carry, in the spec's order.
    pub const CODES: [&'static str; 13] = [
        "syntax",
        "unknown-table",
        "unknown-key",
        "key-not-allowed",
        "duplicate-table",
        "duplicate-key",
        "missing-key",
        "bad-value",
        "not-a-secret-name",
        "unknown-app",
        "unrouted-listener",
        "unreachable-no-app",
        "topology-changed",
    ];

    /// The machine-readable reason.
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.code
    }

    /// The line that caused it, where a line is meaningful.
    #[must_use]
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    fn at(code: &'static str, line: usize, message: String) -> Self {
        Self {
            code,
            line: Some(line),
            message,
        }
    }

    fn whole(code: &'static str, message: String) -> Self {
        Self {
            code,
            line: None,
            message,
        }
    }

    pub(crate) fn syntax(line: usize, what: &str) -> Self {
        Self::at("syntax", line, what.to_owned())
    }

    pub(crate) fn duplicate_table(line: usize, path: &str) -> Self {
        Self::at(
            "duplicate-table",
            line,
            format!("[{path}] is opened more than once"),
        )
    }

    pub(crate) fn duplicate_key(line: usize, path: &str) -> Self {
        Self::at(
            "duplicate-key",
            line,
            format!("`{path}` is set more than once"),
        )
    }

    pub(crate) fn unknown_table(line: usize, path: &str) -> Self {
        Self::at(
            "unknown-table",
            line,
            format!("[{path}] is not a table this schema defines"),
        )
    }

    pub(crate) fn unknown_key(line: usize, path: &str) -> Self {
        Self::at(
            "unknown-key",
            line,
            format!("`{path}` is not a key this schema defines"),
        )
    }

    pub(crate) fn key_not_allowed(line: usize, path: &str, why: &str) -> Self {
        Self::at("key-not-allowed", line, format!("`{path}` {why}"))
    }

    pub(crate) fn missing_key(path: &str, why: &str) -> Self {
        Self::whole("missing-key", format!("`{path}` is required: {why}"))
    }

    pub(crate) fn bad_value(line: usize, path: &str, why: &str) -> Self {
        Self::at("bad-value", line, format!("`{path}`: {why}"))
    }

    pub(crate) fn not_a_secret_name(line: usize, path: &str) -> Self {
        Self::at(
            "not-a-secret-name",
            line,
            format!(
                "`{path}` takes the *name* of a secret, matching [a-z][a-z0-9._-]{{0,63}} — never \
                 the secret itself"
            ),
        )
    }

    pub(crate) fn unknown_app(listener: &str, app: &str) -> Self {
        Self::whole(
            "unknown-app",
            format!("listener `{listener}` routes to `{app}`, which is not declared"),
        )
    }

    pub(crate) fn unrouted_listener(listener: &str) -> Self {
        Self::whole(
            "unrouted-listener",
            format!(
                "listener `{listener}` declares neither `app` nor `no_app`; a call arriving there \
                 would be met with silence"
            ),
        )
    }

    pub(crate) fn unreachable_no_app(listener: &str) -> Self {
        Self::whole(
            "unreachable-no-app",
            format!(
                "listener `{listener}` declares both `app` and `no_app`; the refusal can never \
                 apply"
            ),
        )
    }

    pub(crate) fn topology_changed(listener: &str, field: &str) -> Self {
        Self::whole(
            "topology-changed",
            format!(
                "a reload may not change listener topology: `{listener}` {field}. Changing a \
                 listener is a restart"
            ),
        )
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}: line {line}: {}", self.code, self.message),
            None => write!(f, "{}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

/// What connects to a listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Calls: a SIP endpoint.
    Sip,
    /// Session-mode apps.
    Session,
}

/// A SIP listener's transport, per [`sip-transport.md`](../../../../docs/specs/sip-transport.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// UDP.
    Udp,
    /// TCP.
    Tcp,
    /// TLS over TCP.
    Tls,
    /// WebSocket.
    Ws,
    /// WebSocket over TLS.
    Wss,
}

/// Where a call arriving on a listener goes. Total by construction (N6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// To this app.
    App(String),
    /// Nowhere; refuse with this status.
    Refuse(u16),
    /// Nothing arrives here — apps connect here instead.
    Sessions,
}

/// One `[listener.<name>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listener {
    /// The name in the header.
    pub name: String,
    /// What connects here.
    pub protocol: Protocol,
    /// The local address.
    pub bind: SocketAddr,
    /// The transport, for a `sip` listener.
    pub transport: Option<Transport>,
    /// The address to advertise, when it differs from `bind`.
    pub advertise: Option<String>,
    /// Where calls go.
    pub route: Route,
}

impl Listener {
    /// The part of a listener a reload may not change (N10), as a comparable tuple.
    fn topology(&self) -> (Protocol, SocketAddr, Option<Transport>, Option<&str>) {
        (
            self.protocol,
            self.bind,
            self.transport,
            self.advertise.as_deref(),
        )
    }
}

/// The name of a secret, resolved outside this document.
///
/// A newtype rather than a `String` because that is the enforcement of N7: there is no constructor
/// that accepts material, so there is no way to write a value here that would have to be redacted
/// before the file is committed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SecretRef(String);

impl SecretRef {
    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Read a name, or refuse it.
    ///
    /// The grammar is `[a-z][a-z0-9._-]{0,63}`. Base64 and PEM both use characters it excludes, so
    /// the common accident — pasting the key where the name goes — does not get through.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut chars = text.chars();
        if !matches!(chars.next(), Some(c) if c.is_ascii_lowercase()) {
            return None;
        }
        if text.len() > 64 {
            return None;
        }
        if !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c)) {
            return None;
        }
        Some(Self(text.to_owned()))
    }
}

impl fmt::Display for SecretRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which transport of the contract an app speaks, and what that transport needs named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppBinding {
    /// Document mode: the host `POST`s events.
    Webhook {
        /// Where to `POST` them.
        url: String,
        /// The `Sipx-Signature` keys, by name, the head first. A second entry is a rotation
        /// window; what the host does with it is `A-2`'s.
        signing_secrets: Vec<SecretRef>,
    },
    /// Session mode: the app connects and stays.
    Session {
        /// What it presents at upgrade, by name.
        bearer_secret: SecretRef,
    },
    /// The embedded runtime: no wire at all.
    Embedded {
        /// The handler file. What it resolves against is `A-6`'s.
        handler: String,
    },
}

/// What an app is allowed to make the host do on its behalf. Deny-by-default (N5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Grants {
    /// Directories `play` may name a file under. Empty means inline audio only.
    pub play_roots: Vec<String>,
    /// Header fields `dial` may set. Empty means none.
    pub dial_headers: Vec<String>,
    /// Whether the app may place outbound calls.
    pub originate: bool,
}

impl Grants {
    /// Everything denied — what an absent `grants` table means.
    #[must_use]
    pub fn denied() -> Self {
        Self::default()
    }
}

/// One `[app.<name>]`, with its failure semantics and grants resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppConfig {
    /// The name in the header.
    pub name: String,
    /// Which transport, and what it needs.
    pub binding: AppBinding,
    /// The contract's §9.2 declaration.
    pub failure: FailurePolicy,
    /// What the host will do on this app's behalf.
    pub grants: Grants,
}

/// A whole, validated document.
///
/// Constructible only through [`parse`](Self::parse), so a value of this type is a document that
/// satisfied every normative point — there is no half-valid configuration to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    listeners: BTreeMap<String, Listener>,
    apps: BTreeMap<String, AppConfig>,
}

impl HostConfig {
    /// Read a document, whole or not at all.
    ///
    /// # Errors
    /// The first thing that made the document invalid, with its code and — where a line means
    /// something — the line.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        schema::load(text)
    }

    /// A listener by name.
    #[must_use]
    pub fn listener(&self, name: &str) -> Option<&Listener> {
        self.listeners.get(name)
    }

    /// An app by name.
    #[must_use]
    pub fn app(&self, name: &str) -> Option<&AppConfig> {
        self.apps.get(name)
    }

    /// Every listener, by name, in name order.
    pub fn listeners(&self) -> impl Iterator<Item = &Listener> {
        self.listeners.values()
    }

    /// Every app, by name, in name order.
    pub fn apps(&self) -> impl Iterator<Item = &AppConfig> {
        self.apps.values()
    }

    /// Every secret this document references, by name, sorted and deduplicated.
    ///
    /// The list an operator has to be able to resolve before starting the host — and, read the
    /// other way, the complete inventory of what this file does *not* contain.
    #[must_use]
    pub fn secret_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .apps
            .values()
            .flat_map(|app| match &app.binding {
                AppBinding::Webhook {
                    signing_secrets, ..
                } => signing_secrets.iter().map(SecretRef::as_str).collect(),
                AppBinding::Session { bearer_secret } => vec![bearer_secret.as_str()],
                AppBinding::Embedded { .. } => Vec::new(),
            })
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Whether `next` may replace this configuration without a restart (N10).
    ///
    /// # Errors
    /// `topology-changed`, naming the listener and what about it moved.
    pub(crate) fn topology_allows(&self, next: &Self) -> Result<(), ConfigError> {
        for name in self.listeners.keys() {
            if !next.listeners.contains_key(name) {
                return Err(ConfigError::topology_changed(name, "is no longer declared"));
            }
        }
        for (name, listener) in &next.listeners {
            let Some(current) = self.listeners.get(name) else {
                return Err(ConfigError::topology_changed(
                    name,
                    "was not declared before",
                ));
            };
            if current.topology() != listener.topology() {
                return Err(ConfigError::topology_changed(
                    name,
                    "changes its protocol, bind, transport or advertised address",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// §4.4's grammar, rule by rule. Each row is one thing the grammar refuses, and between them
    /// they are why "the document is committable" is a property rather than a habit — the shapes a
    /// pasted key actually takes (PEM armour, base64, hex with punctuation, an over-long blob) each
    /// fail on a different clause.
    #[test]
    fn a_secret_name_is_a_name_and_a_pasted_key_is_not() {
        for name in [
            "a",
            "reception-hook",
            "reception-hook-2026-07",
            "app.reception.hook_1",
            &"a".repeat(64),
        ] {
            assert!(SecretRef::parse(name).is_some(), "{name:?} is a name");
        }

        for pasted in [
            "",
            "Reception",                   // uppercase, as base64 so often starts
            "-----BEGIN PRIVATE KEY-----", // PEM armour: leading dash
            "2026-hook",                   // a digit is not a first character
            "aGVsbG8sIHRoaXMgaXMgbm90IGEgbmFtZQ==", // base64: uppercase and =
            "hook+key/rotation=",          // base64's punctuation
            "reception hook",              // a space
            &"a".repeat(65),               // longer than any name needs to be
        ] {
            assert!(
                SecretRef::parse(pasted).is_none(),
                "{pasted:?} must not pass for a name"
            );
        }
    }

    /// The other half of N7, stated over the type rather than over a document: the only way to hold
    /// a secret reference is to hold a name, so there is no field a value could be smuggled into.
    #[test]
    fn a_secret_reference_is_only_ever_its_name() {
        let reference = SecretRef::parse("reception-hook").unwrap();
        assert_eq!(reference.as_str(), "reception-hook");
        assert_eq!(reference.to_string(), "reception-hook");
    }
}
