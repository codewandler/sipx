//! The `BSDK-CFG` configuration document (`docs/specs/browser-sdk.md` §4.3, §8.6).

use serde_json::Value;

use crate::command::{field, object, require_version};
use crate::error::{Error, Result};

/// How the host declared the signalling transport's security.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Insecure {
    /// The contract default: WSS only.
    Refuse,
    /// Local development against loopback over plain `ws:`.
    ///
    /// §8.6: with this set the kernel **refuses to answer a digest challenge**. A fingerprint
    /// carried over unauthenticated signalling authenticates nothing, and credentials do not
    /// cross a transport the host itself declared insecure.
    AllowDevelopment,
}

/// The transport the page will open. The kernel opens nothing; this is what it must agree with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Transport {
    pub(crate) scheme: String,
    pub(crate) host: String,
    pub(crate) resource: String,
}

/// One kernel's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    pub(crate) aor: String,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) transport: Transport,
    pub(crate) insecure: Insecure,
}

impl Config {
    /// Parse a `BSDK-CFG` document.
    ///
    /// A configuration this contract refuses — an unknown scheme, or plain `ws:` without the
    /// explicit development opt-in — is `E_SCHEMA` rather than a handle that would fail later:
    /// §6.2 says there is no half-started client.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let text = core::str::from_utf8(bytes).map_err(|_| Error::Utf8)?;
        let document: Value = serde_json::from_str(text).map_err(|_| Error::Json)?;
        let root = object(&document)?;
        require_version(root)?;

        let aor = field(root, "aor")?
            .as_str()
            .ok_or(Error::Schema)?
            .to_owned();
        // A configured AOR that is not a SIP URI would produce a REGISTER nobody can route, and
        // the first sign of it would be a 4xx from the far end.
        let _ = sipx_sip::Uri::parse(bytes::Bytes::from(aor.clone().into_bytes()))
            .map_err(|_| Error::Schema)?;

        let auth = object(field(root, "auth")?)?;
        let username = field(auth, "username")?
            .as_str()
            .ok_or(Error::Schema)?
            .to_owned();
        let password = field(auth, "password")?
            .as_str()
            .ok_or(Error::Schema)?
            .to_owned();

        let transport = object(field(root, "transport")?)?;
        let scheme = field(transport, "scheme")?
            .as_str()
            .ok_or(Error::Schema)?
            .to_owned();
        let host = field(transport, "host")?
            .as_str()
            .ok_or(Error::Schema)?
            .to_owned();
        let resource = field(transport, "resource")?
            .as_str()
            .ok_or(Error::Schema)?
            .to_owned();

        let insecure = match field(root, "insecure")?.as_str().ok_or(Error::Schema)? {
            "refuse" => Insecure::Refuse,
            "allow-development" => Insecure::AllowDevelopment,
            _ => return Err(Error::Schema),
        };

        match (scheme.as_str(), insecure) {
            ("wss", _) | ("ws", Insecure::AllowDevelopment) => {}
            _ => return Err(Error::Schema),
        }

        Ok(Self {
            aor,
            username,
            password,
            transport: Transport {
                scheme,
                host,
                resource,
            },
            insecure,
        })
    }

    /// Whether the kernel may answer a digest challenge on this transport (§8.6).
    pub(crate) fn may_authenticate(&self) -> bool {
        matches!(self.insecure, Insecure::Refuse)
    }

    /// The user part of the AOR, for the RFC 7118 `Contact`.
    pub(crate) fn aor_user(&self) -> &str {
        self.aor
            .strip_prefix("sip:")
            .or_else(|| self.aor.strip_prefix("sips:"))
            .and_then(|rest| rest.split('@').next())
            .unwrap_or("anonymous")
    }

    /// The registrar's domain, which is the REGISTER Request-URI's host.
    pub(crate) fn aor_domain(&self) -> &str {
        self.aor
            .strip_prefix("sip:")
            .or_else(|| self.aor.strip_prefix("sips:"))
            .and_then(|rest| rest.split('@').nth(1))
            .map_or_else(
                || self.transport.host.as_str(),
                |host| host.split(';').next().unwrap_or(host),
            )
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// `BSDK-CFG-1`, byte for byte from `docs/specs/browser-sdk.md` §9.2.
    pub(crate) const BSDK_CFG_1: &[u8] = br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}"#;

    #[test]
    fn bsdk_cfg_1_is_one_hundred_and_seventy_eight_octets() {
        assert_eq!(BSDK_CFG_1.len(), 178);
    }

    #[test]
    fn bsdk_cfg_1_parses_into_every_field() {
        let config = Config::parse(BSDK_CFG_1).expect("the contract's own vector");
        assert_eq!(config.aor, "sip:alice@example.net");
        assert_eq!(config.username, "alice");
        assert_eq!(config.password, "secret");
        assert_eq!(config.transport.scheme, "wss");
        assert_eq!(config.transport.host, "edge.example.net");
        assert_eq!(config.transport.resource, "/sip");
        assert_eq!(config.insecure, Insecure::Refuse);
        assert!(config.may_authenticate());
        assert_eq!(config.aor_user(), "alice");
        assert_eq!(config.aor_domain(), "example.net");
    }

    #[test]
    fn plain_ws_without_the_development_opt_in_is_refused() {
        let document = br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"a","password":"b"},"transport":{"scheme":"ws","host":"localhost","resource":"/sip"},"insecure":"refuse"}"#;
        assert_eq!(Config::parse(document), Err(Error::Schema));
    }

    #[test]
    fn plain_ws_with_the_opt_in_may_not_answer_a_challenge() {
        let document = br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"a","password":"b"},"transport":{"scheme":"ws","host":"localhost","resource":"/sip"},"insecure":"allow-development"}"#;
        let config = Config::parse(document).expect("the development opt-in is explicit");
        assert!(!config.may_authenticate());
    }

    #[test]
    fn an_unknown_scheme_is_refused() {
        let document = br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"a","password":"b"},"transport":{"scheme":"https","host":"h","resource":"/sip"},"insecure":"allow-development"}"#;
        assert_eq!(Config::parse(document), Err(Error::Schema));
    }
}
