//! The TURN client profile used by ICE relayed candidates (RFC 8656).
//!
//! This module is bytes and long-term credential arithmetic only. It reads no socket and no
//! clock; `ice::gather` and `ice::driver` supply those at the bound media port. The
//! normative contract and byte vectors are in `docs/specs/ice.md` §11.4 and §14.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use hmac::{Hmac, Mac};
use md5::{Digest as _, Md5};
use precis_profiles::OpaqueString;
use precis_profiles::precis_core::profile::Profile as _;
use sha1::Sha1;
use sha2::Sha256;
use subtle::ConstantTimeEq as _;

use sipx_transport::stun::{HEADER_LEN, MAGIC_COOKIE, is_stun};
pub(crate) use sipx_transport::stun::{TransactionId, new_transaction_id};

type HmacSha1 = Hmac<Sha1>;
type HmacSha256 = Hmac<Sha256>;

const METHOD_ALLOCATE: u16 = 0x0003;
const METHOD_REFRESH: u16 = 0x0004;
const METHOD_SEND: u16 = 0x0006;
const METHOD_DATA: u16 = 0x0007;
const METHOD_CREATE_PERMISSION: u16 = 0x0008;

const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_LIFETIME: u16 = 0x000d;
const ATTR_XOR_PEER_ADDRESS: u16 = 0x0012;
const ATTR_DATA: u16 = 0x0013;
const ATTR_REALM: u16 = 0x0014;
const ATTR_NONCE: u16 = 0x0015;
const ATTR_XOR_RELAYED_ADDRESS: u16 = 0x0016;
const ATTR_REQUESTED_TRANSPORT: u16 = 0x0019;
const ATTR_MESSAGE_INTEGRITY_SHA256: u16 = 0x001c;
const ATTR_PASSWORD_ALGORITHM: u16 = 0x001d;
const ATTR_USERHASH: u16 = 0x001e;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_PASSWORD_ALGORITHMS: u16 = 0x8002;

const PASSWORD_ALGORITHM_MD5: u16 = 0x0001;
const PASSWORD_ALGORITHM_SHA256: u16 = 0x0002;
const NONCE_COOKIE_PREFIX: &[u8; 9] = b"obMatJos2";
const FEATURE_PASSWORD_ALGORITHMS: u32 = 1 << 0;
const FEATURE_USERNAME_ANONYMITY: u32 = 1 << 1;

const FAMILY_IPV4: u8 = 0x01;
const FAMILY_IPV6: u8 = 0x02;
const UDP_PROTOCOL: u8 = 17;
const SHA1_INTEGRITY_ATTRIBUTE_LEN: usize = 24;
const SHA256_INTEGRITY_ATTRIBUTE_LEN: usize = 36;
const MAX_DATA: usize = u16::MAX as usize;

/// Why TURN long-term credentials cannot be prepared for authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RelayCredentialError {
    /// The username is not valid under RFC 8265's `OpaqueString` profile.
    #[error("the TURN username is not a valid OpaqueString")]
    InvalidUsername,
    /// The password is not valid under RFC 8265's `OpaqueString` profile.
    #[error("the TURN password is not a valid OpaqueString")]
    InvalidPassword,
}

/// A configured TURN server and its long-term credentials (RFC 8656 §7.1; RFC 8489 §9.2).
///
/// The password is deliberately absent from `Debug`. Clone this into an ICE gathering operation;
/// the value never enters SDP and is dropped with the allocation owner.
#[derive(Clone, PartialEq, Eq)]
pub struct Relay {
    server: SocketAddr,
    username: String,
    password: String,
}

impl Relay {
    /// Configure a UDP TURN server with prepared long-term credentials.
    ///
    /// Preparation enforces RFC 8265's `OpaqueString` profile now, so allocation cannot discover
    /// an application credential error only after its component socket has been bound.
    ///
    /// # Errors
    ///
    /// A username or password containing a value prohibited by `OpaqueString` is refused.
    pub fn new(
        server: SocketAddr,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Result<Self, RelayCredentialError> {
        let username = enforce_opaque_string(&username.into())
            .map_err(|_| RelayCredentialError::InvalidUsername)?;
        let password = enforce_opaque_string(&password.into())
            .map_err(|_| RelayCredentialError::InvalidPassword)?;
        Ok(Self {
            server,
            username,
            password,
        })
    }

    /// The configured server address.
    #[must_use]
    pub const fn server(&self) -> SocketAddr {
        self.server
    }

    /// The long-term username.
    #[must_use]
    pub fn username(&self) -> &str {
        &self.username
    }
}

impl fmt::Debug for Relay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Relay")
            .field("server", &self.server)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// The challenge material retained with an allocation.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Auth {
    username: String,
    password: String,
    realm: String,
    nonce: String,
    password_algorithm: PasswordAlgorithm,
    password_algorithms: Option<PasswordAlgorithms>,
    username_anonymous: bool,
}

impl fmt::Debug for Auth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Auth")
            .field("username", &self.username)
            .field("realm", &self.realm)
            .field("nonce", &"<redacted>")
            .field("password", &"<redacted>")
            .field("password_algorithm", &self.password_algorithm)
            .field("password_algorithms", &self.password_algorithms)
            .field("username_anonymous", &self.username_anonymous)
            .finish()
    }
}

impl Auth {
    pub(crate) fn challenged(
        relay: &Relay,
        realm: &str,
        nonce: String,
        password_algorithms: Option<PasswordAlgorithms>,
    ) -> Result<Self, Error> {
        let username = enforce_opaque_string(&relay.username)?;
        let password = enforce_opaque_string(&relay.password)?;
        let realm = enforce_opaque_string(realm)?;
        let (password_algorithm, username_anonymous) =
            negotiate_authentication(&nonce, password_algorithms.as_ref())?;
        Ok(Self {
            username,
            password,
            realm,
            nonce,
            password_algorithm,
            password_algorithms,
            username_anonymous,
        })
    }

    pub(crate) fn replace_challenge(
        &mut self,
        realm: &str,
        nonce: String,
        password_algorithms: Option<PasswordAlgorithms>,
    ) -> Result<(), Error> {
        if enforce_opaque_string(realm)? != self.realm {
            return Err(Error::ChangedAuthenticationRealm);
        }
        let (password_algorithm, username_anonymous) =
            negotiate_authentication(&nonce, password_algorithms.as_ref())?;
        self.nonce = nonce;
        self.password_algorithm = password_algorithm;
        self.password_algorithms = password_algorithms;
        self.username_anonymous = username_anonymous;
        Ok(())
    }

    fn key(&self) -> Vec<u8> {
        let material = credential_material(&self.username, &self.realm, &self.password);
        match self.password_algorithm {
            PasswordAlgorithm::Md5 => Md5::digest(material).to_vec(),
            PasswordAlgorithm::Sha256 => Sha256::digest(material).to_vec(),
        }
    }

    fn userhash(&self) -> [u8; 32] {
        let mut material = Vec::with_capacity(self.username.len() + self.realm.len() + 1);
        material.extend_from_slice(self.username.as_bytes());
        material.push(b':');
        material.extend_from_slice(self.realm.as_bytes());
        Sha256::digest(material).into()
    }

    const fn integrity_form(&self) -> IntegrityForm {
        if self.password_algorithms.is_some() {
            IntegrityForm::Sha256
        } else {
            IntegrityForm::Sha1
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasswordAlgorithm {
    Md5,
    Sha256,
}

impl PasswordAlgorithm {
    const fn code(self) -> u16 {
        match self {
            Self::Md5 => PASSWORD_ALGORITHM_MD5,
            Self::Sha256 => PASSWORD_ALGORITHM_SHA256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PasswordAlgorithms {
    raw: Vec<u8>,
    selected: Option<PasswordAlgorithm>,
}

impl PasswordAlgorithms {
    fn decode(raw: &[u8]) -> Result<Self, Error> {
        let mut offset = 0usize;
        let mut selected = None;
        while offset < raw.len() {
            let algorithm =
                read_u16(raw, offset).ok_or(Error::MalformedAttribute(ATTR_PASSWORD_ALGORITHMS))?;
            let parameters_at = offset.checked_add(2).ok_or(Error::Truncated)?;
            let parameters_len = usize::from(
                read_u16(raw, parameters_at)
                    .ok_or(Error::MalformedAttribute(ATTR_PASSWORD_ALGORITHMS))?,
            );
            let parameters_start = offset.checked_add(4).ok_or(Error::Truncated)?;
            let parameters_end = parameters_start
                .checked_add(parameters_len)
                .ok_or(Error::Truncated)?;
            raw.get(parameters_start..parameters_end)
                .ok_or(Error::MalformedAttribute(ATTR_PASSWORD_ALGORITHMS))?;
            if selected.is_none() && parameters_len == 0 {
                selected = match algorithm {
                    PASSWORD_ALGORITHM_MD5 => Some(PasswordAlgorithm::Md5),
                    PASSWORD_ALGORITHM_SHA256 => Some(PasswordAlgorithm::Sha256),
                    _ => None,
                };
            }
            let padded = parameters_len.checked_add(3).ok_or(Error::Truncated)? & !3;
            offset = parameters_start
                .checked_add(padded)
                .ok_or(Error::Truncated)?;
            if offset > raw.len() {
                return Err(Error::MalformedAttribute(ATTR_PASSWORD_ALGORITHMS));
            }
        }
        Ok(Self {
            raw: raw.to_vec(),
            selected,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegrityForm {
    Sha1,
    Sha256,
}

fn negotiate_authentication(
    nonce: &str,
    password_algorithms: Option<&PasswordAlgorithms>,
) -> Result<(PasswordAlgorithm, bool), Error> {
    let features = nonce_security_features(nonce)?;
    if features.is_some_and(|value| value & FEATURE_PASSWORD_ALGORITHMS != 0)
        && password_algorithms.is_none()
    {
        return Err(Error::AuthenticationDowngrade);
    }
    let password_algorithm = match password_algorithms {
        Some(algorithms) => algorithms
            .selected
            .ok_or(Error::UnsupportedPasswordAlgorithm)?,
        None => PasswordAlgorithm::Md5,
    };
    let username_anonymous = features.is_some_and(|value| value & FEATURE_USERNAME_ANONYMITY != 0);
    Ok((password_algorithm, username_anonymous))
}

fn credential_material(username: &str, realm: &str, password: &str) -> Vec<u8> {
    let mut material = Vec::with_capacity(username.len() + realm.len() + password.len() + 2);
    material.extend_from_slice(username.as_bytes());
    material.push(b':');
    material.extend_from_slice(realm.as_bytes());
    material.push(b':');
    material.extend_from_slice(password.as_bytes());
    material
}

fn enforce_opaque_string(value: &str) -> Result<String, Error> {
    OpaqueString::new()
        .enforce(value)
        .map(std::borrow::Cow::into_owned)
        .map_err(|_| Error::InvalidCredentialProfile)
}

fn nonce_security_features(nonce: &str) -> Result<Option<u32>, Error> {
    let bytes = nonce.as_bytes();
    if !bytes.starts_with(NONCE_COOKIE_PREFIX) {
        return Ok(None);
    }
    let encoded = bytes
        .get(NONCE_COOKIE_PREFIX.len()..NONCE_COOKIE_PREFIX.len() + 4)
        .ok_or(Error::AuthenticationDowngrade)?;
    let mut features = 0u32;
    for byte in encoded {
        features = features
            .checked_shl(6)
            .ok_or(Error::AuthenticationDowngrade)?
            | u32::from(base64_value(*byte).ok_or(Error::AuthenticationDowngrade)?);
    }
    Ok(Some(features))
}

const fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// A live allocation gathered on one ICE base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Allocation {
    pub(crate) base: super::LocalBase,
    pub(crate) server: SocketAddr,
    pub(crate) relayed: SocketAddr,
    pub(crate) mapped: SocketAddr,
    pub(crate) lifetime: Duration,
    pub(crate) auth: Auth,
}

/// A malformed or unsupported TURN message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum Error {
    /// The datagram does not have the STUN marker and magic cookie.
    #[error("not a STUN message")]
    NotStun,
    /// The datagram ends inside the header, body, or an attribute.
    #[error("the TURN message is truncated")]
    Truncated,
    /// The method is not one used by this TURN client.
    #[error("unsupported TURN method {0:#06x}")]
    UnsupportedMethod(u16),
    /// A known attribute has an invalid value.
    #[error("TURN attribute {0:#06x} is malformed")]
    MalformedAttribute(u16),
    /// The STUN length field cannot represent the message.
    #[error("the TURN message is too long")]
    TooLong,
    /// A nonce cookie claimed a security feature whose matching attribute was absent or malformed.
    #[error("the TURN authentication challenge failed downgrade protection")]
    AuthenticationDowngrade,
    /// The server offered no password algorithm this client supports.
    #[error("the TURN server offered no supported password algorithm")]
    UnsupportedPasswordAlgorithm,
    /// A credential could not be enforced under RFC 8265's `OpaqueString` profile.
    #[error("a TURN credential is not a valid OpaqueString")]
    InvalidCredentialProfile,
    /// A stale-nonce response tried to change the authenticated realm.
    #[error("a TURN stale-nonce response changed the authentication realm")]
    ChangedAuthenticationRealm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Request,
    Indication,
    Success,
    Error,
}

impl Class {
    const fn bits(self) -> u16 {
        match self {
            Self::Request => 0,
            Self::Indication => 1,
            Self::Success => 2,
            Self::Error => 3,
        }
    }

    const fn from_bits(bits: u16) -> Self {
        match bits {
            1 => Self::Indication,
            2 => Self::Success,
            3 => Self::Error,
            _ => Self::Request,
        }
    }
}

const fn message_type(class: Class, method: u16) -> u16 {
    let class = class.bits();
    (method & 0x000f)
        | ((method & 0x0070) << 1)
        | ((method & 0x0f80) << 2)
        | ((class & 0x1) << 4)
        | ((class & 0x2) << 7)
}

const fn split_type(raw: u16) -> (Class, u16) {
    let class = ((raw & 0x0100) >> 7) | ((raw & 0x0010) >> 4);
    let method = (raw & 0x000f) | ((raw & 0x00e0) >> 1) | ((raw & 0x3e00) >> 2);
    (Class::from_bits(class), method)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Attribute<'a> {
    RequestedTransport,
    Lifetime(u32),
    XorPeer(SocketAddr),
    Data(&'a [u8]),
}

/// A decoded response used by gathering and the running driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Response {
    Challenge {
        transaction: TransactionId,
        method: u16,
        code: u16,
        realm: Option<String>,
        nonce: Option<String>,
        password_algorithms: Option<PasswordAlgorithms>,
        integrity: Option<Integrity>,
    },
    Allocated {
        transaction: TransactionId,
        relayed: SocketAddr,
        mapped: SocketAddr,
        lifetime: Duration,
        integrity: Integrity,
    },
    Success {
        transaction: TransactionId,
        method: u16,
        lifetime: Option<Duration>,
        integrity: Integrity,
    },
}

impl Response {
    pub(crate) const fn transaction(&self) -> TransactionId {
        match self {
            Self::Challenge { transaction, .. }
            | Self::Allocated { transaction, .. }
            | Self::Success { transaction, .. } => *transaction,
        }
    }

    pub(crate) fn verify(&self, auth: &Auth) -> bool {
        match self {
            Self::Challenge {
                code,
                nonce,
                password_algorithms,
                integrity,
                ..
            } => {
                let negotiation_is_safe = !matches!(*code, 401 | 438)
                    || nonce.as_ref().is_some_and(|nonce| {
                        negotiate_authentication(nonce, password_algorithms.as_ref()).is_ok()
                    });
                negotiation_is_safe
                    && integrity
                        .as_ref()
                        .is_some_and(|integrity| integrity.verify(auth))
            }
            Self::Allocated { integrity, .. } | Self::Success { integrity, .. } => {
                integrity.verify(auth)
            }
        }
    }

    pub(crate) const fn is_refresh(&self) -> bool {
        matches!(
            self,
            Self::Challenge {
                method: METHOD_REFRESH,
                ..
            } | Self::Success {
                method: METHOD_REFRESH,
                ..
            }
        )
    }

    pub(crate) const fn is_permission(&self) -> bool {
        matches!(
            self,
            Self::Challenge {
                method: METHOD_CREATE_PERMISSION,
                ..
            } | Self::Success {
                method: METHOD_CREATE_PERMISSION,
                ..
            }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sha1Integrity {
    tag: [u8; 20],
    covered: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sha256Integrity {
    tag: [u8; 32],
    covered: Vec<u8>,
}

/// Received integrity tags and the adjusted prefixes they cover.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Integrity {
    sha1: Option<Sha1Integrity>,
    sha256: Option<Sha256Integrity>,
}

impl Integrity {
    fn verify(&self, auth: &Auth) -> bool {
        let key = auth.key();
        match auth.integrity_form() {
            IntegrityForm::Sha1 => self.sha1.as_ref().is_some_and(|integrity| {
                bool::from(hmac_sha1(&key, &integrity.covered).ct_eq(&integrity.tag))
            }),
            IntegrityForm::Sha256 => self.sha256.as_ref().is_some_and(|integrity| {
                bool::from(hmac_sha256(&key, &integrity.covered).ct_eq(&integrity.tag))
            }),
        }
    }
}

/// The peer address and payload in a TURN Data indication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PeerData<'a> {
    pub(crate) peer: SocketAddr,
    pub(crate) data: &'a [u8],
}

pub(crate) fn allocate(transaction: TransactionId, auth: Option<&Auth>) -> Result<Vec<u8>, Error> {
    encode(
        Class::Request,
        METHOD_ALLOCATE,
        transaction,
        &[Attribute::RequestedTransport],
        auth,
    )
}

pub(crate) fn refresh(
    transaction: TransactionId,
    lifetime: Duration,
    auth: &Auth,
) -> Result<Vec<u8>, Error> {
    encode(
        Class::Request,
        METHOD_REFRESH,
        transaction,
        &[Attribute::Lifetime(duration_seconds(lifetime))],
        Some(auth),
    )
}

pub(crate) fn create_permission(
    transaction: TransactionId,
    peer: SocketAddr,
    auth: &Auth,
) -> Result<Vec<u8>, Error> {
    encode(
        Class::Request,
        METHOD_CREATE_PERMISSION,
        transaction,
        &[Attribute::XorPeer(peer)],
        Some(auth),
    )
}

pub(crate) fn send_indication(
    transaction: TransactionId,
    peer: SocketAddr,
    data: &[u8],
) -> Result<Vec<u8>, Error> {
    if data.len() > MAX_DATA {
        return Err(Error::TooLong);
    }
    encode(
        Class::Indication,
        METHOD_SEND,
        transaction,
        &[Attribute::XorPeer(peer), Attribute::Data(data)],
        None,
    )
}

fn encode(
    class: Class,
    method: u16,
    transaction: TransactionId,
    attributes: &[Attribute<'_>],
    auth: Option<&Auth>,
) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(HEADER_LEN + 128);
    out.extend_from_slice(&message_type(class, method).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(&transaction);
    for attribute in attributes {
        encode_attribute(&mut out, attribute, &transaction)?;
    }
    if let Some(auth) = auth {
        if auth.username_anonymous {
            push_attribute(&mut out, ATTR_USERHASH, &auth.userhash())?;
        } else {
            push_attribute(&mut out, ATTR_USERNAME, auth.username.as_bytes())?;
        }
        push_attribute(&mut out, ATTR_REALM, auth.realm.as_bytes())?;
        push_attribute(&mut out, ATTR_NONCE, auth.nonce.as_bytes())?;
        if let Some(algorithms) = &auth.password_algorithms {
            push_attribute(&mut out, ATTR_PASSWORD_ALGORITHMS, &algorithms.raw)?;
            let mut selected = [0u8; 4];
            selected
                .get_mut(..2)
                .ok_or(Error::TooLong)?
                .copy_from_slice(&auth.password_algorithm.code().to_be_bytes());
            push_attribute(&mut out, ATTR_PASSWORD_ALGORITHM, &selected)?;
        }
        let key = auth.key();
        match auth.integrity_form() {
            IntegrityForm::Sha1 => {
                set_length(&mut out, SHA1_INTEGRITY_ATTRIBUTE_LEN)?;
                let tag = hmac_sha1(&key, &out);
                push_attribute(&mut out, ATTR_MESSAGE_INTEGRITY, &tag)?;
            }
            IntegrityForm::Sha256 => {
                set_length(&mut out, SHA256_INTEGRITY_ATTRIBUTE_LEN)?;
                let tag = hmac_sha256(&key, &out);
                push_attribute(&mut out, ATTR_MESSAGE_INTEGRITY_SHA256, &tag)?;
            }
        }
    } else {
        set_length(&mut out, 0)?;
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) fn challenge_for_test(
    transaction: TransactionId,
    code: u16,
    realm: &str,
    nonce: &str,
) -> Vec<u8> {
    let mut out = header(Class::Error, METHOD_ALLOCATE, transaction);
    let class = u8::try_from(code / 100).unwrap_or_default();
    let number = u8::try_from(code % 100).unwrap_or_default();
    let _ = push_attribute(&mut out, ATTR_ERROR_CODE, &[0, 0, class, number]);
    let _ = push_attribute(&mut out, ATTR_REALM, realm.as_bytes());
    let _ = push_attribute(&mut out, ATTR_NONCE, nonce.as_bytes());
    let _ = set_length(&mut out, 0);
    out
}

#[cfg(test)]
pub(crate) fn sha256_challenge_for_test(
    transaction: TransactionId,
    code: u16,
    realm: &str,
    nonce: &str,
) -> Vec<u8> {
    let mut out = header(Class::Error, METHOD_ALLOCATE, transaction);
    let class = u8::try_from(code / 100).unwrap_or_default();
    let number = u8::try_from(code % 100).unwrap_or_default();
    let _ = push_attribute(&mut out, ATTR_ERROR_CODE, &[0, 0, class, number]);
    let _ = push_attribute(&mut out, ATTR_REALM, realm.as_bytes());
    let _ = push_attribute(&mut out, ATTR_NONCE, nonce.as_bytes());
    let algorithms = [0, 2, 0, 0, 0, 1, 0, 0];
    let _ = push_attribute(&mut out, ATTR_PASSWORD_ALGORITHMS, &algorithms);
    let _ = set_length(&mut out, 0);
    out
}

#[cfg(test)]
pub(crate) fn authenticated_challenge_for_test(
    transaction: TransactionId,
    operation: &str,
    code: u16,
    nonce: &str,
    auth: &Auth,
) -> Vec<u8> {
    let method = match operation {
        "allocate" => METHOD_ALLOCATE,
        "permission" => METHOD_CREATE_PERMISSION,
        _ => METHOD_REFRESH,
    };
    let mut out = header(Class::Error, method, transaction);
    let class = u8::try_from(code / 100).unwrap_or_default();
    let number = u8::try_from(code % 100).unwrap_or_default();
    let _ = push_attribute(&mut out, ATTR_ERROR_CODE, &[0, 0, class, number]);
    let _ = push_attribute(&mut out, ATTR_REALM, auth.realm.as_bytes());
    let _ = push_attribute(&mut out, ATTR_NONCE, nonce.as_bytes());
    if let Some(algorithms) = &auth.password_algorithms {
        let _ = push_attribute(&mut out, ATTR_PASSWORD_ALGORITHMS, &algorithms.raw);
    }
    finish_with_integrity(&mut out, auth);
    out
}

#[cfg(test)]
pub(crate) fn refresh_success_for_test(
    transaction: TransactionId,
    lifetime: Duration,
    auth: &Auth,
) -> Vec<u8> {
    let mut out = header(Class::Success, METHOD_REFRESH, transaction);
    let _ = push_attribute(
        &mut out,
        ATTR_LIFETIME,
        &duration_seconds(lifetime).to_be_bytes(),
    );
    finish_with_integrity(&mut out, auth);
    out
}

#[cfg(test)]
pub(crate) fn permission_success_for_test(transaction: TransactionId, auth: &Auth) -> Vec<u8> {
    let mut out = header(Class::Success, METHOD_CREATE_PERMISSION, transaction);
    finish_with_integrity(&mut out, auth);
    out
}

#[cfg(test)]
fn finish_with_integrity(out: &mut Vec<u8>, auth: &Auth) {
    let key = auth.key();
    match auth.integrity_form() {
        IntegrityForm::Sha1 => {
            let _ = set_length(out, SHA1_INTEGRITY_ATTRIBUTE_LEN);
            let tag = hmac_sha1(&key, out);
            let _ = push_attribute(out, ATTR_MESSAGE_INTEGRITY, &tag);
        }
        IntegrityForm::Sha256 => {
            let _ = set_length(out, SHA256_INTEGRITY_ATTRIBUTE_LEN);
            let tag = hmac_sha256(&key, out);
            let _ = push_attribute(out, ATTR_MESSAGE_INTEGRITY_SHA256, &tag);
        }
    }
}

#[cfg(test)]
pub(crate) fn allocation_success_for_test(
    transaction: TransactionId,
    relayed: SocketAddr,
    mapped: SocketAddr,
    lifetime: Duration,
    auth: &Auth,
) -> Vec<u8> {
    let mut out = header(Class::Success, METHOD_ALLOCATE, transaction);
    let _ = push_attribute(
        &mut out,
        ATTR_XOR_RELAYED_ADDRESS,
        &encode_xor_address(relayed, &transaction),
    );
    let _ = push_attribute(
        &mut out,
        ATTR_XOR_MAPPED_ADDRESS,
        &encode_xor_address(mapped, &transaction),
    );
    let _ = push_attribute(
        &mut out,
        ATTR_LIFETIME,
        &duration_seconds(lifetime).to_be_bytes(),
    );
    finish_with_integrity(&mut out, auth);
    out
}

#[cfg(test)]
pub(crate) fn authenticated_allocate_for_test(datagram: &[u8], auth: &Auth) -> bool {
    let Ok(decoded) = decode(datagram) else {
        return false;
    };
    decoded.class == Class::Request
        && decoded.method == METHOD_ALLOCATE
        && decoded
            .integrity
            .as_ref()
            .is_some_and(|integrity| integrity.verify(auth))
}

#[cfg(test)]
pub(crate) fn operation_for_test(datagram: &[u8]) -> Option<&'static str> {
    let decoded = decode(datagram).ok()?;
    match decoded.method {
        METHOD_ALLOCATE => Some("allocate"),
        METHOD_REFRESH => Some("refresh"),
        METHOD_SEND => Some("send"),
        METHOD_DATA => Some("data"),
        METHOD_CREATE_PERMISSION => Some("permission"),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn transaction_for_test(datagram: &[u8]) -> Option<TransactionId> {
    Some(decode(datagram).ok()?.transaction)
}

#[cfg(test)]
pub(crate) fn lifetime_for_test(datagram: &[u8]) -> Option<Duration> {
    decode(datagram).ok()?.lifetime
}

#[cfg(test)]
fn header(class: Class, method: u16, transaction: TransactionId) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + 64);
    out.extend_from_slice(&message_type(class, method).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(&transaction);
    out
}

fn encode_attribute(
    out: &mut Vec<u8>,
    attribute: &Attribute<'_>,
    transaction: &TransactionId,
) -> Result<(), Error> {
    match attribute {
        Attribute::RequestedTransport => {
            push_attribute(out, ATTR_REQUESTED_TRANSPORT, &[UDP_PROTOCOL, 0, 0, 0])
        }
        Attribute::Lifetime(value) => push_attribute(out, ATTR_LIFETIME, &value.to_be_bytes()),
        Attribute::XorPeer(address) => push_attribute(
            out,
            ATTR_XOR_PEER_ADDRESS,
            &encode_xor_address(*address, transaction),
        ),
        Attribute::Data(value) => push_attribute(out, ATTR_DATA, value),
    }
}

/// Decode an Allocate/Refresh/CreatePermission response.
pub(crate) fn response(datagram: &[u8]) -> Result<Response, Error> {
    let decoded = decode(datagram)?;
    match (decoded.class, decoded.method) {
        (Class::Error, METHOD_ALLOCATE | METHOD_REFRESH | METHOD_CREATE_PERMISSION) => {
            Ok(Response::Challenge {
                transaction: decoded.transaction,
                method: decoded.method,
                code: decoded
                    .error_code
                    .ok_or(Error::MalformedAttribute(ATTR_ERROR_CODE))?,
                realm: decoded.realm,
                nonce: decoded.nonce,
                password_algorithms: decoded.password_algorithms,
                integrity: decoded.integrity,
            })
        }
        (Class::Success, METHOD_ALLOCATE) => Ok(Response::Allocated {
            transaction: decoded.transaction,
            relayed: decoded
                .relayed
                .ok_or(Error::MalformedAttribute(ATTR_XOR_RELAYED_ADDRESS))?,
            mapped: decoded
                .mapped
                .ok_or(Error::MalformedAttribute(ATTR_XOR_MAPPED_ADDRESS))?,
            lifetime: decoded
                .lifetime
                .ok_or(Error::MalformedAttribute(ATTR_LIFETIME))?,
            integrity: decoded
                .integrity
                .ok_or(Error::MalformedAttribute(ATTR_MESSAGE_INTEGRITY))?,
        }),
        (Class::Success, METHOD_REFRESH | METHOD_CREATE_PERMISSION) => Ok(Response::Success {
            transaction: decoded.transaction,
            method: decoded.method,
            lifetime: decoded.lifetime,
            integrity: decoded
                .integrity
                .ok_or(Error::MalformedAttribute(ATTR_MESSAGE_INTEGRITY))?,
        }),
        _ => Err(Error::UnsupportedMethod(decoded.method)),
    }
}

/// Decode a Data indication without copying its payload.
pub(crate) fn data_indication(datagram: &[u8]) -> Result<PeerData<'_>, Error> {
    peer_data(datagram, METHOD_DATA)
}

#[cfg(test)]
pub(crate) fn sent_data(datagram: &[u8]) -> Result<PeerData<'_>, Error> {
    peer_data(datagram, METHOD_SEND)
}

fn peer_data(datagram: &[u8], expected_method: u16) -> Result<PeerData<'_>, Error> {
    let decoded = decode(datagram)?;
    if decoded.class != Class::Indication || decoded.method != expected_method {
        return Err(Error::UnsupportedMethod(decoded.method));
    }
    Ok(PeerData {
        peer: decoded
            .peer
            .ok_or(Error::MalformedAttribute(ATTR_XOR_PEER_ADDRESS))?,
        data: decoded.data.ok_or(Error::MalformedAttribute(ATTR_DATA))?,
    })
}

#[cfg(test)]
pub(crate) fn data_indication_for_test(
    transaction: TransactionId,
    peer: SocketAddr,
    data: &[u8],
) -> Vec<u8> {
    encode(
        Class::Indication,
        METHOD_DATA,
        transaction,
        &[Attribute::XorPeer(peer), Attribute::Data(data)],
        None,
    )
    .unwrap_or_default()
}

struct Decoded<'a> {
    class: Class,
    method: u16,
    transaction: TransactionId,
    error_code: Option<u16>,
    realm: Option<String>,
    nonce: Option<String>,
    password_algorithms: Option<PasswordAlgorithms>,
    lifetime: Option<Duration>,
    peer: Option<SocketAddr>,
    relayed: Option<SocketAddr>,
    mapped: Option<SocketAddr>,
    data: Option<&'a [u8]>,
    integrity: Option<Integrity>,
}

fn decode(datagram: &[u8]) -> Result<Decoded<'_>, Error> {
    if !is_stun(datagram) {
        return Err(Error::NotStun);
    }
    let (class, method) = split_type(read_u16(datagram, 0).ok_or(Error::Truncated)?);
    if !matches!(
        method,
        METHOD_ALLOCATE | METHOD_REFRESH | METHOD_SEND | METHOD_DATA | METHOD_CREATE_PERMISSION
    ) {
        return Err(Error::UnsupportedMethod(method));
    }
    let transaction = datagram
        .get(8..HEADER_LEN)
        .and_then(|bytes| <TransactionId>::try_from(bytes).ok())
        .ok_or(Error::Truncated)?;
    let stated = usize::from(read_u16(datagram, 2).ok_or(Error::Truncated)?);
    let end = HEADER_LEN.checked_add(stated).ok_or(Error::Truncated)?;
    let body = datagram.get(HEADER_LEN..end).ok_or(Error::Truncated)?;
    let mut decoded = Decoded {
        class,
        method,
        transaction,
        error_code: None,
        realm: None,
        nonce: None,
        password_algorithms: None,
        lifetime: None,
        peer: None,
        relayed: None,
        mapped: None,
        data: None,
        integrity: None,
    };
    let mut offset = 0usize;
    let mut authenticated_boundary = false;
    while offset < body.len() {
        let kind = read_u16(body, offset).ok_or(Error::Truncated)?;
        let length_at = offset.checked_add(2).ok_or(Error::Truncated)?;
        let length = usize::from(read_u16(body, length_at).ok_or(Error::Truncated)?);
        let start = offset.checked_add(4).ok_or(Error::Truncated)?;
        let value = start
            .checked_add(length)
            .and_then(|end| body.get(start..end))
            .ok_or(Error::Truncated)?;
        // Operation attributes after the first integrity attribute cannot affect authenticated
        // state. MESSAGE-INTEGRITY-SHA256 may itself follow MESSAGE-INTEGRITY, so integrity
        // attributes remain parseable past that boundary.
        if authenticated_boundary
            && !matches!(kind, ATTR_MESSAGE_INTEGRITY | ATTR_MESSAGE_INTEGRITY_SHA256)
        {
            let padded = length.checked_add(3).ok_or(Error::Truncated)? & !3;
            offset = start.checked_add(padded).ok_or(Error::Truncated)?;
            continue;
        }
        match kind {
            ATTR_ERROR_CODE => decoded.error_code = Some(decode_error(value)?),
            ATTR_REALM => decoded.realm = Some(decode_text(value, kind)?),
            ATTR_NONCE => decoded.nonce = Some(decode_text(value, kind)?),
            ATTR_PASSWORD_ALGORITHMS => {
                decoded.password_algorithms = Some(PasswordAlgorithms::decode(value)?);
            }
            ATTR_LIFETIME => {
                decoded.lifetime = Some(Duration::from_secs(u64::from(fixed_u32(value, kind)?)));
            }
            ATTR_XOR_PEER_ADDRESS => {
                decoded.peer = Some(decode_xor_address(value, &transaction, kind)?);
            }
            ATTR_XOR_RELAYED_ADDRESS => {
                decoded.relayed = Some(decode_xor_address(value, &transaction, kind)?);
            }
            ATTR_XOR_MAPPED_ADDRESS => {
                decoded.mapped = Some(decode_xor_address(value, &transaction, kind)?);
            }
            ATTR_DATA => decoded.data = Some(value),
            ATTR_MESSAGE_INTEGRITY => {
                let integrity = decoded.integrity.get_or_insert_with(Integrity::default);
                if integrity.sha1.is_some() {
                    return Err(Error::MalformedAttribute(kind));
                }
                integrity.sha1 = Some(Sha1Integrity {
                    tag: <[u8; 20]>::try_from(value)
                        .map_err(|_| Error::MalformedAttribute(kind))?,
                    covered: covered_prefix(datagram, offset, SHA1_INTEGRITY_ATTRIBUTE_LEN)?,
                });
                authenticated_boundary = true;
            }
            ATTR_MESSAGE_INTEGRITY_SHA256 => {
                let integrity = decoded.integrity.get_or_insert_with(Integrity::default);
                if integrity.sha256.is_some() {
                    return Err(Error::MalformedAttribute(kind));
                }
                integrity.sha256 = Some(Sha256Integrity {
                    tag: <[u8; 32]>::try_from(value)
                        .map_err(|_| Error::MalformedAttribute(kind))?,
                    covered: covered_prefix(datagram, offset, SHA256_INTEGRITY_ATTRIBUTE_LEN)?,
                });
                authenticated_boundary = true;
            }
            _ => {}
        }
        let padded = length.checked_add(3).ok_or(Error::Truncated)? & !3;
        offset = start.checked_add(padded).ok_or(Error::Truncated)?;
    }
    Ok(decoded)
}

fn duration_seconds(duration: Duration) -> u32 {
    u32::try_from(duration.as_secs()).unwrap_or(u32::MAX)
}

fn decode_text(value: &[u8], kind: u16) -> Result<String, Error> {
    std::str::from_utf8(value)
        .map(str::to_owned)
        .map_err(|_| Error::MalformedAttribute(kind))
}

fn decode_error(value: &[u8]) -> Result<u16, Error> {
    let class = u16::from(
        *value
            .get(2)
            .ok_or(Error::MalformedAttribute(ATTR_ERROR_CODE))?
            & 0x07,
    );
    let number = u16::from(
        *value
            .get(3)
            .ok_or(Error::MalformedAttribute(ATTR_ERROR_CODE))?,
    );
    class
        .checked_mul(100)
        .and_then(|hundreds| hundreds.checked_add(number))
        .filter(|code| (300..=699).contains(code))
        .ok_or(Error::MalformedAttribute(ATTR_ERROR_CODE))
}

fn fixed_u32(value: &[u8], kind: u16) -> Result<u32, Error> {
    <[u8; 4]>::try_from(value)
        .map(u32::from_be_bytes)
        .map_err(|_| Error::MalformedAttribute(kind))
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    <[u8; 2]>::try_from(bytes.get(at..end)?)
        .ok()
        .map(u16::from_be_bytes)
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    <[u8; 4]>::try_from(bytes.get(at..end)?)
        .ok()
        .map(u32::from_be_bytes)
}

fn push_attribute(out: &mut Vec<u8>, kind: u16, value: &[u8]) -> Result<(), Error> {
    let length = u16::try_from(value.len()).map_err(|_| Error::TooLong)?;
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(value);
    out.extend(std::iter::repeat_n(0, (4 - value.len() % 4) % 4));
    Ok(())
}

fn set_length(out: &mut [u8], extra: usize) -> Result<(), Error> {
    let body = out
        .len()
        .checked_sub(HEADER_LEN)
        .and_then(|written| written.checked_add(extra))
        .ok_or(Error::TooLong)?;
    let length = u16::try_from(body).map_err(|_| Error::TooLong)?;
    out.get_mut(2..4)
        .ok_or(Error::TooLong)?
        .copy_from_slice(&length.to_be_bytes());
    Ok(())
}

fn covered_prefix(datagram: &[u8], offset: usize, attribute_len: usize) -> Result<Vec<u8>, Error> {
    let end = HEADER_LEN.checked_add(offset).ok_or(Error::Truncated)?;
    let mut prefix = datagram.get(..end).ok_or(Error::Truncated)?.to_vec();
    let body = offset.checked_add(attribute_len).ok_or(Error::Truncated)?;
    let length = u16::try_from(body).map_err(|_| Error::Truncated)?;
    prefix
        .get_mut(2..4)
        .ok_or(Error::Truncated)?
        .copy_from_slice(&length.to_be_bytes());
    Ok(prefix)
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut mac = <HmacSha1 as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts every key length"));
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts every key length"));
    mac.update(data);
    mac.finalize().into_bytes().into()
}

fn encode_xor_address(address: SocketAddr, transaction: &TransactionId) -> Vec<u8> {
    let mut value = Vec::with_capacity(20);
    value.push(0);
    match address.ip() {
        IpAddr::V4(ip) => {
            value.push(FAMILY_IPV4);
            value.extend_from_slice(&(address.port() ^ port_key()).to_be_bytes());
            value.extend_from_slice(&(u32::from(ip) ^ MAGIC_COOKIE).to_be_bytes());
        }
        IpAddr::V6(ip) => {
            value.push(FAMILY_IPV6);
            value.extend_from_slice(&(address.port() ^ port_key()).to_be_bytes());
            value.extend(
                ip.octets()
                    .into_iter()
                    .zip(xor_key(transaction))
                    .map(|(byte, key)| byte ^ key),
            );
        }
    }
    value
}

fn decode_xor_address(
    value: &[u8],
    transaction: &TransactionId,
    kind: u16,
) -> Result<SocketAddr, Error> {
    let malformed = || Error::MalformedAttribute(kind);
    let family = *value.get(1).ok_or_else(malformed)?;
    let port = read_u16(value, 2).ok_or_else(malformed)? ^ port_key();
    match family {
        FAMILY_IPV4 => {
            let address = Ipv4Addr::from(read_u32(value, 4).ok_or_else(malformed)? ^ MAGIC_COOKIE);
            Ok(SocketAddr::new(IpAddr::V4(address), port))
        }
        FAMILY_IPV6 => {
            let encoded = <[u8; 16]>::try_from(value.get(4..20).ok_or_else(malformed)?)
                .map_err(|_| malformed())?;
            let mut octets = [0u8; 16];
            for (slot, (byte, key)) in octets
                .iter_mut()
                .zip(encoded.into_iter().zip(xor_key(transaction)))
            {
                *slot = byte ^ key;
            }
            Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        _ => Err(malformed()),
    }
}

const fn port_key() -> u16 {
    (MAGIC_COOKIE >> 16) as u16
}

fn xor_key(transaction: &TransactionId) -> [u8; 16] {
    let mut key = [0u8; 16];
    for (slot, byte) in key.iter_mut().zip(
        MAGIC_COOKIE
            .to_be_bytes()
            .into_iter()
            .chain(transaction.iter().copied()),
    ) {
        *slot = byte;
    }
    key
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

    const ID: TransactionId = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];

    fn relay() -> Relay {
        Relay::new("192.0.2.10:3478".parse().unwrap(), "1000", "relay-password")
            .expect("valid fixture credentials")
    }

    fn auth() -> Auth {
        Auth::challenged(&relay(), "example.com", "nonce".to_owned(), None)
            .expect("legacy challenge is supported")
    }

    #[test]
    fn long_term_allocate_carries_udp_and_the_challenge_in_normative_order() {
        let request = allocate(ID, Some(&auth())).expect("encodes");
        assert_eq!(read_u16(&request, 0), Some(0x0003));
        let kinds: Vec<u16> = attributes(&request).map(|(kind, _)| kind).collect();
        assert_eq!(
            kinds,
            [
                ATTR_REQUESTED_TRANSPORT,
                ATTR_USERNAME,
                ATTR_REALM,
                ATTR_NONCE,
                ATTR_MESSAGE_INTEGRITY,
            ]
        );
        let transport = attributes(&request)
            .find(|(kind, _)| *kind == ATTR_REQUESTED_TRANSPORT)
            .map(|(_, value)| value)
            .expect("requested transport");
        assert_eq!(transport, [17, 0, 0, 0]);
    }

    #[test]
    fn password_algorithms_choose_the_first_supported_server_preference() {
        let raw = password_algorithms(&[
            (0x7777, &[1, 2, 3, 4]),
            (PASSWORD_ALGORITHM_SHA256, &[]),
            (PASSWORD_ALGORITHM_MD5, &[]),
        ]);
        let offered = PasswordAlgorithms::decode(&raw).expect("valid list");
        let auth = Auth::challenged(
            &relay(),
            "example.com",
            "obMatJos2AAABopaque".to_owned(),
            Some(offered),
        )
        .expect("SHA-256 is supported");

        let request = allocate(ID, Some(&auth)).expect("encodes");
        let attributes: Vec<_> = attributes(&request).collect();
        let kinds: Vec<_> = attributes.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            [
                ATTR_REQUESTED_TRANSPORT,
                ATTR_USERNAME,
                ATTR_REALM,
                ATTR_NONCE,
                ATTR_PASSWORD_ALGORITHMS,
                ATTR_PASSWORD_ALGORITHM,
                ATTR_MESSAGE_INTEGRITY_SHA256,
            ]
        );
        assert_eq!(
            attributes
                .iter()
                .find(|(kind, _)| *kind == ATTR_PASSWORD_ALGORITHMS)
                .map(|(_, value)| *value),
            Some(raw.as_slice())
        );
        assert_eq!(
            attributes
                .iter()
                .find(|(kind, _)| *kind == ATTR_PASSWORD_ALGORITHM)
                .map(|(_, value)| *value),
            Some([0, 2, 0, 0].as_slice())
        );
        let decoded = decode(&request).expect("decodes");
        assert!(
            decoded
                .integrity
                .as_ref()
                .is_some_and(|integrity| integrity.verify(&auth))
        );
    }

    #[test]
    fn anonymity_cookie_uses_userhash_and_never_exposes_the_username() {
        let auth = Auth::challenged(
            &relay(),
            "example.com",
            "obMatJos2AAACopaque".to_owned(),
            None,
        )
        .expect("visible ASCII credentials can be anonymised");
        let request = allocate(ID, Some(&auth)).expect("encodes");
        let attributes: Vec<_> = attributes(&request).collect();
        assert!(attributes.iter().any(|(kind, _)| *kind == ATTR_USERHASH));
        assert!(!attributes.iter().any(|(kind, _)| *kind == ATTR_USERNAME));

        let expected: [u8; 32] = Sha256::digest(b"1000:example.com").into();
        assert_eq!(
            attributes
                .iter()
                .find(|(kind, _)| *kind == ATTR_USERHASH)
                .map(|(_, value)| *value),
            Some(expected.as_slice())
        );
    }

    #[test]
    fn opaque_string_is_enforced_before_userhash_and_matches_the_rfc_vector() {
        let decomposed = Relay::new(
            "192.0.2.10:3478".parse().unwrap(),
            "cafe\u{301}",
            "relay-password",
        )
        .expect("OpaqueString normalises NFC");
        assert_eq!(decomposed.username(), "caf\u{e9}");
        let normalized = Auth::challenged(
            &decomposed,
            "example.com",
            "obMatJos2AAACanonymous".to_owned(),
            None,
        )
        .expect("OpaqueString normalises NFC");
        let expected: [u8; 32] = Sha256::digest("caf\u{e9}:example.com".as_bytes()).into();
        assert_eq!(normalized.userhash(), expected);

        let published = Relay::new(
            "192.0.2.10:3478".parse().unwrap(),
            "マトリックス",
            "relay-password",
        )
        .expect("published username is a valid OpaqueString");
        let published = Auth::challenged(
            &published,
            "example.org",
            "obMatJos2AAACpublished".to_owned(),
            None,
        )
        .expect("published username is a valid OpaqueString");
        assert_eq!(
            published.userhash(),
            [
                0x4a, 0x3c, 0xf3, 0x8f, 0xef, 0x69, 0x92, 0xbd, 0xa9, 0x52, 0xc6, 0x78, 0x04, 0x17,
                0xda, 0x0f, 0x24, 0x81, 0x94, 0x15, 0x56, 0x9e, 0x60, 0xb2, 0x05, 0xc4, 0x6e, 0x41,
                0x40, 0x7f, 0x17, 0x04,
            ]
        );
    }

    #[test]
    fn rfc8489_sha256_integrity_vector_matches_the_verified_erratum() {
        let transaction = [
            0x78, 0xad, 0x34, 0x33, 0xc6, 0xad, 0x72, 0xc0, 0x29, 0xda, 0x41, 0x2e,
        ];
        let userhash = [
            0x4a, 0x3c, 0xf3, 0x8f, 0xef, 0x69, 0x92, 0xbd, 0xa9, 0x52, 0xc6, 0x78, 0x04, 0x17,
            0xda, 0x0f, 0x24, 0x81, 0x94, 0x15, 0x56, 0x9e, 0x60, 0xb2, 0x05, 0xc4, 0x6e, 0x41,
            0x40, 0x7f, 0x17, 0x04,
        ];
        let mut request = header(Class::Request, 0x0001, transaction);
        push_attribute(&mut request, ATTR_USERHASH, &userhash).expect("fits");
        push_attribute(
            &mut request,
            ATTR_NONCE,
            b"obMatJos2AAACf//499k954d6OL34oL9FSTvy64sA",
        )
        .expect("fits");
        push_attribute(&mut request, ATTR_REALM, b"example.org").expect("fits");
        push_attribute(&mut request, ATTR_PASSWORD_ALGORITHM, &[0, 2, 0, 0]).expect("fits");
        set_length(&mut request, SHA256_INTEGRITY_ATTRIBUTE_LEN).expect("fits");

        let key: [u8; 32] = Sha256::digest("マトリックス:example.org:TheMatrIX".as_bytes()).into();
        assert_eq!(
            hmac_sha256(&key, &request),
            [
                0xb5, 0xc7, 0xbf, 0x00, 0x5b, 0x6c, 0x52, 0xa2, 0x1c, 0x51, 0xc5, 0xe8, 0x92, 0xf8,
                0x19, 0x24, 0x13, 0x62, 0x96, 0xcb, 0x92, 0x7c, 0x43, 0x14, 0x93, 0x09, 0x27, 0x8c,
                0xc6, 0x51, 0x8e, 0x65,
            ]
        );
    }

    #[test]
    fn nonce_cookie_prevents_algorithm_and_anonymity_downgrades() {
        assert_eq!(
            Auth::challenged(
                &relay(),
                "example.com",
                "obMatJos2AAABstripped".to_owned(),
                None,
            ),
            Err(Error::AuthenticationDowngrade)
        );
        assert_eq!(
            Auth::challenged(
                &relay(),
                "example.com",
                "obMatJos2AA?Bcorrupt".to_owned(),
                None,
            ),
            Err(Error::AuthenticationDowngrade)
        );

        let offered = PasswordAlgorithms::decode(&password_algorithms(&[(0x7777, &[])]))
            .expect("well-formed unsupported list");
        assert_eq!(
            Auth::challenged(
                &relay(),
                "example.com",
                "obMatJos2AAABunsupported".to_owned(),
                Some(offered),
            ),
            Err(Error::UnsupportedPasswordAlgorithm)
        );

        assert_eq!(
            Relay::new(
                "192.0.2.10:3478".parse().unwrap(),
                "contains-\0-control",
                "relay-password",
            ),
            Err(RelayCredentialError::InvalidUsername)
        );
        assert_eq!(
            Relay::new(
                "192.0.2.10:3478".parse().unwrap(),
                "1000",
                "contains-\0-control",
            ),
            Err(RelayCredentialError::InvalidPassword)
        );
        assert_eq!(
            PasswordAlgorithms::decode(&[0, 2, 0, 4, 1]),
            Err(Error::MalformedAttribute(ATTR_PASSWORD_ALGORITHMS))
        );
    }

    #[test]
    fn negotiated_sha256_rejects_a_sha1_response_even_with_the_right_key() {
        let offered =
            PasswordAlgorithms::decode(&password_algorithms(&[(PASSWORD_ALGORITHM_SHA256, &[])]))
                .expect("valid list");
        let auth = Auth::challenged(
            &relay(),
            "example.com",
            "obMatJos2AAABopaque".to_owned(),
            Some(offered),
        )
        .expect("SHA-256 is supported");
        let mut message = header(Class::Success, METHOD_REFRESH, ID);
        push_attribute(&mut message, ATTR_LIFETIME, &600u32.to_be_bytes()).expect("fits");
        set_length(&mut message, SHA1_INTEGRITY_ATTRIBUTE_LEN).expect("fits");
        let tag = hmac_sha1(&auth.key(), &message);
        push_attribute(&mut message, ATTR_MESSAGE_INTEGRITY, &tag).expect("fits");

        let parsed = response(&message).expect("framing and tag decode");
        assert!(!parsed.verify(&auth));
    }

    #[test]
    fn send_and_data_indications_preserve_the_peer_and_payload() {
        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        let bytes = [0x80, 0x00, 0x00, 0x01];
        let send = send_indication(ID, peer, &bytes).expect("encodes");
        assert_eq!(
            sent_data(&send).expect("send indication"),
            PeerData { peer, data: &bytes }
        );
        let decoded = decode(&send).expect("decodes");
        assert_eq!(decoded.class, Class::Indication);
        assert_eq!(decoded.method, METHOD_SEND);
        assert_eq!(decoded.peer, Some(peer));
        assert_eq!(decoded.data, Some(bytes.as_slice()));

        let data = encode(
            Class::Indication,
            METHOD_DATA,
            ID,
            &[Attribute::XorPeer(peer), Attribute::Data(&bytes)],
            None,
        )
        .expect("encodes");
        assert_eq!(
            data_indication(&data).expect("data indication"),
            PeerData { peer, data: &bytes }
        );
    }

    #[test]
    fn refresh_and_permission_requests_use_the_long_term_key() {
        let auth = auth();
        let peer: SocketAddr = "203.0.113.9:9".parse().unwrap();
        let permission = create_permission(ID, peer, &auth).expect("encodes");
        let decoded = decode(&permission).expect("decodes");
        assert_eq!(decoded.method, METHOD_CREATE_PERMISSION);
        assert_eq!(decoded.peer, Some(peer));
        assert!(
            decoded
                .integrity
                .as_ref()
                .is_some_and(|integrity| integrity.verify(&auth))
        );

        let refresh = refresh(ID, Duration::from_secs(600), &auth).expect("encodes");
        let decoded = decode(&refresh).expect("decodes");
        assert_eq!(decoded.method, METHOD_REFRESH);
        assert_eq!(decoded.lifetime, Some(Duration::from_secs(600)));
        assert!(
            decoded
                .integrity
                .as_ref()
                .is_some_and(|integrity| integrity.verify(&auth))
        );
    }

    #[test]
    fn authenticated_stale_nonce_rejects_a_tampered_integrity_tag() {
        let auth = auth();
        let mut challenge = authenticated_challenge_for_test(ID, "refresh", 438, "fresh", &auth);
        let parsed = response(&challenge).expect("authenticated challenge decodes");
        assert!(parsed.verify(&auth));

        if let Some(last) = challenge.last_mut() {
            *last ^= 1;
        }
        let parsed = response(&challenge).expect("tampered challenge still has valid framing");
        assert!(!parsed.verify(&auth));
    }

    #[test]
    fn authenticated_stale_nonce_cannot_change_the_cached_realm() {
        let mut auth = auth();
        let mut challenge = header(Class::Error, METHOD_REFRESH, ID);
        push_attribute(&mut challenge, ATTR_ERROR_CODE, &[0, 0, 4, 38]).expect("fits");
        push_attribute(&mut challenge, ATTR_REALM, b"changed.example.com").expect("fits");
        push_attribute(&mut challenge, ATTR_NONCE, b"fresh").expect("fits");
        finish_with_integrity(&mut challenge, &auth);

        let parsed = response(&challenge).expect("challenge decodes");
        assert!(parsed.verify(&auth), "the response itself uses the old key");
        let Response::Challenge {
            realm: Some(realm),
            nonce: Some(nonce),
            password_algorithms,
            ..
        } = parsed
        else {
            panic!("expected challenge");
        };
        assert_eq!(
            auth.replace_challenge(&realm, nonce, password_algorithms),
            Err(Error::ChangedAuthenticationRealm)
        );
    }

    #[test]
    fn attributes_after_message_integrity_cannot_change_authenticated_state() {
        let auth = auth();
        let mut message = refresh_success_for_test(ID, Duration::from_secs(600), &auth);
        push_attribute(&mut message, ATTR_LIFETIME, &1u32.to_be_bytes()).expect("attribute fits");
        set_length(&mut message, 0).expect("message fits");

        let parsed = response(&message).expect("response decodes");
        assert!(parsed.verify(&auth));
        assert!(matches!(
            parsed,
            Response::Success {
                lifetime: Some(lifetime),
                ..
            } if lifetime == Duration::from_secs(600)
        ));
    }

    #[test]
    fn sha256_after_sha1_is_verified_but_interposed_operation_data_is_ignored() {
        let offered =
            PasswordAlgorithms::decode(&password_algorithms(&[(PASSWORD_ALGORITHM_SHA256, &[])]))
                .expect("valid list");
        let auth = Auth::challenged(
            &relay(),
            "example.com",
            "obMatJos2AAABopaque".to_owned(),
            Some(offered),
        )
        .expect("SHA-256 auth");
        let key = auth.key();
        let mut message = header(Class::Success, METHOD_REFRESH, ID);
        push_attribute(&mut message, ATTR_LIFETIME, &600u32.to_be_bytes()).expect("fits");
        set_length(&mut message, SHA1_INTEGRITY_ATTRIBUTE_LEN).expect("fits");
        let sha1 = hmac_sha1(&key, &message);
        push_attribute(&mut message, ATTR_MESSAGE_INTEGRITY, &sha1).expect("fits");
        push_attribute(&mut message, ATTR_LIFETIME, &1u32.to_be_bytes()).expect("fits");
        set_length(&mut message, SHA256_INTEGRITY_ATTRIBUTE_LEN).expect("fits");
        let sha256 = hmac_sha256(&key, &message);
        push_attribute(&mut message, ATTR_MESSAGE_INTEGRITY_SHA256, &sha256).expect("fits");

        let parsed = response(&message).expect("response decodes");
        assert!(parsed.verify(&auth));
        assert!(matches!(
            parsed,
            Response::Success {
                lifetime: Some(lifetime),
                ..
            } if lifetime == Duration::from_secs(600)
        ));
    }

    #[test]
    fn malformed_turn_input_is_a_typed_error() {
        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        let data = encode(
            Class::Indication,
            METHOD_DATA,
            ID,
            &[Attribute::XorPeer(peer), Attribute::Data(&[1, 2, 3])],
            None,
        )
        .expect("encodes");
        for end in 0..data.len() {
            assert!(data_indication(&data[..end]).is_err());
        }
    }

    #[test]
    fn relay_debug_never_contains_the_password() {
        let debug = format!("{:?}", relay());
        assert!(!debug.contains("relay-password"));
        assert!(debug.contains("<redacted>"));
    }

    fn attributes(datagram: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
        let end = HEADER_LEN + usize::from(read_u16(datagram, 2).unwrap_or_default());
        let mut body = datagram.get(HEADER_LEN..end).unwrap_or_default();
        std::iter::from_fn(move || {
            let kind = read_u16(body, 0)?;
            let length = usize::from(read_u16(body, 2)?);
            let value = body.get(4..4 + length)?;
            let padded = (length + 3) & !3;
            body = body.get(4 + padded..).unwrap_or_default();
            Some((kind, value))
        })
    }

    fn password_algorithms(entries: &[(u16, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (algorithm, parameters) in entries {
            out.extend_from_slice(&algorithm.to_be_bytes());
            out.extend_from_slice(
                &u16::try_from(parameters.len())
                    .expect("test parameter length fits")
                    .to_be_bytes(),
            );
            out.extend_from_slice(parameters);
            out.extend(std::iter::repeat_n(0, (4 - parameters.len() % 4) % 4));
        }
        out
    }
}
