//! RFC 8224 authentication and verification services.
//!
//! **Supported** (`S-34`): outbound and inbound call policies select these services through
//! `sipx-call`, which constrains their public shape. sipx remains pre-1.0, so Supported does not
//! mean frozen; breaking changes receive a migration note.
//!
//! Network retrieval and trust remain caller-owned. This module only orders policy decisions,
//! supplies a bounded cache, and maps verification failures to their specified SIP statuses.

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use sipx_sip::headers::Date;
use sipx_sip::identity::{
    CanonicalIdentity, Es256SigningKey, Es256VerifyingKey, IdentityError, IdentityHeader,
    date_from_timestamp, date_timestamp, passport_issued_at, request_identities, sign_passport,
    verify_passport,
};
use sipx_sip::{Header, HeaderName, Request, Uri};
use thiserror::Error;

/// Maximum accepted absolute clock skew.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Freshness {
    seconds: u64,
}

impl Freshness {
    /// A caller-selected freshness window.
    #[must_use]
    pub const fn from_seconds(seconds: u64) -> Self {
        Self { seconds }
    }

    fn accepts(self, left: i64, right: i64) -> bool {
        left.abs_diff(right) <= self.seconds
    }
}

impl Default for Freshness {
    fn default() -> Self {
        Self::from_seconds(60)
    }
}

/// Local policy deciding which originating identities may be asserted.
pub trait Authority {
    /// Whether this authentication service is authoritative for `origin`.
    fn authorizes(&self, origin: &CanonicalIdentity) -> bool;
}

impl<F> Authority for F
where
    F: Fn(&CanonicalIdentity) -> bool,
{
    fn authorizes(&self, origin: &CanonicalIdentity) -> bool {
        self(origin)
    }
}

/// An application-owned signing credential.
pub struct SigningCredential {
    key: Es256SigningKey,
    info: String,
    not_before: i64,
    not_after: i64,
}

impl std::fmt::Debug for SigningCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SigningCredential")
            .field("info", &self.info)
            .field("not_before", &self.not_before)
            .field("not_after", &self.not_after)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

impl SigningCredential {
    /// Build an `ES256` credential from an unencrypted `PKCS #8` PEM key.
    pub fn from_pkcs8_pem(
        pem: &str,
        info: impl Into<String>,
        not_before: i64,
        not_after: i64,
    ) -> Result<Self, AuthenticationError> {
        let info = info.into();
        validate_info(&info)?;
        if not_before > not_after {
            return Err(AuthenticationError::Credential);
        }
        Ok(Self {
            key: Es256SigningKey::from_pkcs8_pem(pem)
                .map_err(|_| AuthenticationError::Credential)?,
            info,
            not_before,
            not_after,
        })
    }

    /// The corresponding public verification key.
    #[must_use]
    pub fn verifying_key(&self) -> Es256VerifyingKey {
        self.key.verifying_key()
    }

    fn valid_at(&self, timestamp: i64) -> bool {
        (self.not_before..=self.not_after).contains(&timestamp)
    }
}

fn validate_info(info: &str) -> Result<(), AuthenticationError> {
    Uri::parse(Bytes::copy_from_slice(info.as_bytes()))
        .map(|_| ())
        .map_err(|_| AuthenticationError::Credential)
}

/// Authentication-service failure before an `Identity` field could be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum AuthenticationError {
    /// From or To did not contain a canonical identity.
    #[error("the request does not carry usable From and To identities")]
    Identity,
    /// Local policy does not authorize this service to assert the From identity.
    #[error("the authentication service is not authoritative for the origin")]
    NotAuthoritative,
    /// Date is malformed or outside the freshness policy.
    #[error("the request carries a stale or invalid Date")]
    StaleDate,
    /// The signing credential is malformed or outside its validity interval.
    #[error("the signing credential is not valid for this request")]
    Credential,
    /// The finished header could not be built.
    #[error("the Identity header could not be built")]
    Build,
}

/// RFC 8224 §6.1 authentication service.
#[derive(Debug)]
pub struct AuthenticationService<A> {
    authority: A,
    credential: SigningCredential,
    freshness: Freshness,
}

impl<A: Authority> AuthenticationService<A> {
    /// Use RFC 8224's recommended 60-second freshness policy.
    #[must_use]
    pub fn new(authority: A, credential: SigningCredential) -> Self {
        Self {
            authority,
            credential,
            freshness: Freshness::default(),
        }
    }

    /// Replace the freshness policy.
    #[must_use]
    pub const fn with_freshness(mut self, freshness: Freshness) -> Self {
        self.freshness = freshness;
        self
    }

    /// Add a full baseline `PASSporT` `Identity` header.
    ///
    /// `now` is Unix time supplied by the caller; the service reads no clock.
    pub fn sign(&self, request: &mut Request, now: i64) -> Result<(), AuthenticationError> {
        let (origin, destination) =
            request_identities(request).map_err(|_| AuthenticationError::Identity)?;
        if !self.authority.authorizes(&origin) {
            return Err(AuthenticationError::NotAuthoritative);
        }

        let date_count = request.headers.get_all(&HeaderName::Date).count();
        if date_count > 1 {
            return Err(AuthenticationError::StaleDate);
        }
        let existing_date = request.headers.typed::<Date>();
        let (date, issued_at, add_date) = match existing_date {
            None => (
                date_from_timestamp(now).map_err(|_| AuthenticationError::StaleDate)?,
                now,
                true,
            ),
            Some(Ok(date)) => {
                let timestamp =
                    date_timestamp(&date).map_err(|_| AuthenticationError::StaleDate)?;
                if !self.freshness.accepts(now, timestamp) {
                    return Err(AuthenticationError::StaleDate);
                }
                (date, timestamp, false)
            }
            Some(Err(_)) => return Err(AuthenticationError::StaleDate),
        };
        if !self.credential.valid_at(now) || !self.credential.valid_at(issued_at) {
            return Err(AuthenticationError::Credential);
        }

        let identity = sign_passport(
            &self.credential.key,
            &origin,
            &destination,
            issued_at,
            &self.credential.info,
        );
        let header = Header::build(HeaderName::Identity, identity.to_bytes())
            .map_err(|_| AuthenticationError::Build)?;
        if add_date {
            request.headers.push(
                Header::build(HeaderName::Date, Bytes::from(date.0))
                    .map_err(|_| AuthenticationError::Build)?,
            );
        }
        request.headers.push(header);
        Ok(())
    }
}

/// A verification credential already interpreted and trusted by caller policy.
#[derive(Debug, Clone)]
pub struct VerificationCredential {
    key: Es256VerifyingKey,
    not_before: i64,
    not_after: i64,
}

impl VerificationCredential {
    /// Build a credential from a `SubjectPublicKeyInfo` PEM public key.
    pub fn from_public_key_pem(
        pem: &str,
        not_before: i64,
        not_after: i64,
    ) -> Result<Self, IdentityError> {
        Self::new(
            Es256VerifyingKey::from_public_key_pem(pem)?,
            not_before,
            not_after,
        )
    }

    /// Build a credential from an already parsed `ES256` public key.
    pub fn new(
        key: Es256VerifyingKey,
        not_before: i64,
        not_after: i64,
    ) -> Result<Self, IdentityError> {
        if not_before > not_after {
            return Err(IdentityError::Credential);
        }
        Ok(Self {
            key,
            not_before,
            not_after,
        })
    }

    fn valid_at(&self, timestamp: i64) -> bool {
        (self.not_before..=self.not_after).contains(&timestamp)
    }
}

/// Caller-owned credential acquisition outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum CredentialError {
    /// The `info` resource could not be acquired.
    #[error("credential unavailable")]
    Unavailable,
    /// A resource was acquired but its scheme, trust, or key is unsupported.
    #[error("credential unsupported")]
    Unsupported,
}

/// Caller-supplied `info` URI acquisition and authority policy.
pub trait CredentialFetcher {
    /// Acquire and validate the credential named by the exact `info` URI at caller-supplied time.
    ///
    /// `at` lets bounded caches discard an expired key without reading a clock themselves.
    fn fetch(&mut self, info: &str, at: i64) -> Result<VerificationCredential, CredentialError>;

    /// Whether this credential is trusted and authoritative for `origin`.
    fn authorizes(&self, credential: &VerificationCredential, origin: &CanonicalIdentity) -> bool;
}

/// A bounded successful-credential cache in front of a caller's fetcher.
#[derive(Debug)]
pub struct CachedCredentials<F> {
    fetcher: F,
    capacity: usize,
    entries: HashMap<String, VerificationCredential>,
    order: VecDeque<String>,
}

impl<F> CachedCredentials<F> {
    /// Cache up to `capacity` exact `info` URIs. Zero disables retention.
    #[must_use]
    pub fn new(fetcher: F, capacity: usize) -> Self {
        Self {
            fetcher,
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Recover the caller's fetcher.
    pub fn into_inner(self) -> F {
        self.fetcher
    }
}

impl<F: CredentialFetcher> CredentialFetcher for CachedCredentials<F> {
    fn fetch(&mut self, info: &str, at: i64) -> Result<VerificationCredential, CredentialError> {
        if let Some(credential) = self.entries.get(info) {
            if credential.valid_at(at) {
                return Ok(credential.clone());
            }
            self.entries.remove(info);
            self.order.retain(|cached| cached != info);
        }
        let credential = self.fetcher.fetch(info, at)?;
        if self.capacity > 0 {
            while self.entries.len() >= self.capacity {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.entries.remove(&oldest);
            }
            self.entries.insert(info.to_owned(), credential.clone());
            self.order.push_back(info.to_owned());
        }
        Ok(credential)
    }

    fn authorizes(&self, credential: &VerificationCredential, origin: &CanonicalIdentity) -> bool {
        self.fetcher.authorizes(credential, origin)
    }
}

/// Successful verification state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// One trusted header verified.
    Verified {
        /// SIP-derived originating identity.
        origin: CanonicalIdentity,
        /// Exact credential-reference URI used.
        info: String,
    },
    /// Local policy did not require identity and no usable header was present.
    Unverified,
}

/// Verification-service failure with its RFC 8224 §6.2.2 status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum VerificationFailure {
    /// No usable baseline `Identity` was present when policy required one.
    #[error("428 Use Identity Header")]
    MissingIdentity,
    /// No credential could be acquired.
    #[error("436 Bad Identity Info")]
    BadIdentityInfo,
    /// An acquired credential is unsupported or untrusted.
    #[error("437 Unsupported Credential")]
    UnsupportedCredential,
    /// Date or `iat` is outside freshness policy.
    #[error("403 Stale Date")]
    StaleDate,
    /// No supported `PASSporT` had valid claims and signature.
    #[error("438 Invalid Identity Header")]
    InvalidIdentity,
}

impl VerificationFailure {
    /// SIP response status assigned by RFC 8224 §6.2.2.
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::MissingIdentity => 428,
            Self::BadIdentityInfo => 436,
            Self::UnsupportedCredential => 437,
            Self::StaleDate => 403,
            Self::InvalidIdentity => 438,
        }
    }

    /// SIP reason phrase paired with [`Self::status`].
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::MissingIdentity => "Use Identity Header",
            Self::BadIdentityInfo => "Bad Identity Info",
            Self::UnsupportedCredential => "Unsupported Credential",
            Self::StaleDate => "Stale Date",
            Self::InvalidIdentity => "Invalid Identity Header",
        }
    }
}

/// RFC 8224 §6.2 verification service.
#[derive(Debug)]
pub struct VerificationService<S> {
    source: S,
    freshness: Freshness,
}

impl<F: CredentialFetcher> VerificationService<CachedCredentials<F>> {
    /// Use a bounded 64-entry cache and the default 60-second freshness policy.
    #[must_use]
    pub fn new(fetcher: F) -> Self {
        Self {
            source: CachedCredentials::new(fetcher, 64),
            freshness: Freshness::default(),
        }
    }
}

impl<S: CredentialFetcher> VerificationService<S> {
    /// Use a caller-constructed credential source or cache.
    #[must_use]
    pub fn with_source(source: S) -> Self {
        Self {
            source,
            freshness: Freshness::default(),
        }
    }

    /// Replace the freshness policy.
    #[must_use]
    pub const fn with_freshness(mut self, freshness: Freshness) -> Self {
        self.freshness = freshness;
        self
    }

    /// Recover the credential source, including cache and caller fetcher state.
    pub fn into_source(self) -> S {
        self.source
    }

    /// Verify `Identity` rows in wire order using caller-supplied Unix time.
    pub fn verify(
        &mut self,
        request: &Request,
        now: i64,
        required: bool,
    ) -> Result<Verification, VerificationFailure> {
        let rows: Vec<_> = request.headers.typed_all::<IdentityHeader>().collect();
        if rows.is_empty() {
            return if required {
                Err(VerificationFailure::MissingIdentity)
            } else {
                Ok(Verification::Unverified)
            };
        }

        let mut best = None;
        let mut usable = false;
        for row in rows {
            let Ok(header) = row else {
                promote(&mut best, VerificationFailure::InvalidIdentity);
                continue;
            };
            // Step 1: an unsupported mandatory extension makes this row unusable, with no fetch.
            if header.passport_type.is_some() {
                continue;
            }
            usable = true;
            if header.algorithm != "ES256" {
                promote(&mut best, VerificationFailure::InvalidIdentity);
                continue;
            }

            // Step 2: derive identities from SIP, never from untrusted token claims.
            let Ok((origin, destination)) = request_identities(request) else {
                promote(&mut best, VerificationFailure::InvalidIdentity);
                continue;
            };

            // Step 3: all dereferencing is delegated to the caller/cache.
            let credential = match self.source.fetch(&header.info, now) {
                Ok(credential) => credential,
                Err(CredentialError::Unavailable) => {
                    promote(&mut best, VerificationFailure::BadIdentityInfo);
                    continue;
                }
                Err(CredentialError::Unsupported) => {
                    promote(&mut best, VerificationFailure::UnsupportedCredential);
                    continue;
                }
            };
            if !self.source.authorizes(&credential, &origin) {
                promote(&mut best, VerificationFailure::UnsupportedCredential);
                continue;
            }

            // Step 4: Date freshness and credential validity.
            if request.headers.get_all(&HeaderName::Date).count() != 1 {
                promote(&mut best, VerificationFailure::InvalidIdentity);
                continue;
            }
            let Some(Ok(date)) = request.headers.typed::<Date>() else {
                promote(&mut best, VerificationFailure::InvalidIdentity);
                continue;
            };
            let Ok(date) = date_timestamp(&date) else {
                promote(&mut best, VerificationFailure::InvalidIdentity);
                continue;
            };
            let issued_at = match passport_issued_at(&header) {
                Ok(Some(issued_at)) => issued_at,
                Ok(None) => date,
                Err(_) => {
                    promote(&mut best, VerificationFailure::InvalidIdentity);
                    continue;
                }
            };
            if !self.freshness.accepts(now, date) || !self.freshness.accepts(now, issued_at) {
                promote(&mut best, VerificationFailure::StaleDate);
                continue;
            }
            if !credential.valid_at(date) || !credential.valid_at(now) {
                promote(&mut best, VerificationFailure::UnsupportedCredential);
                continue;
            }

            // Step 5: exact deterministic claims followed by ES256.
            // A full PASSporT's `iat` must describe the SIP Date, not merely be independently
            // fresh. Passing the Date here makes the exact-claims check reject a signed token
            // whose otherwise-fresh `iat` disagrees with the request it is authenticating.
            if verify_passport(&header, &credential.key, &origin, &destination, date).is_ok() {
                return Ok(Verification::Verified {
                    origin,
                    info: header.info,
                });
            }
            promote(&mut best, VerificationFailure::InvalidIdentity);
        }

        if usable {
            Err(best.unwrap_or(VerificationFailure::InvalidIdentity))
        } else if required {
            // An unsupported mandatory `ppt` behaves like no usable Identity (428), but a row
            // that was present and malformed is an invalid Identity (438).
            Err(best.unwrap_or(VerificationFailure::MissingIdentity))
        } else {
            Ok(Verification::Unverified)
        }
    }
}

fn promote(best: &mut Option<VerificationFailure>, candidate: VerificationFailure) {
    let rank = |failure: VerificationFailure| match failure {
        VerificationFailure::MissingIdentity => 0,
        VerificationFailure::BadIdentityInfo => 1,
        VerificationFailure::UnsupportedCredential => 2,
        VerificationFailure::StaleDate => 3,
        VerificationFailure::InvalidIdentity => 4,
    };
    if best.is_none_or(|current| rank(candidate) > rank(current)) {
        *best = Some(candidate);
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
    use sipx_sip::Method;
    use sipx_sip::build::RequestBuilder;

    const PRIVATE: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgi7q2TZvN9VDFg8Vy\n\
qCP06bETrR2v8MRvr89rn4i+UAahRANCAAQWfaj1HUETpoNCrOtp9KA8o0V79IuW\n\
ARKt9C1cFPkyd3FBP4SeiNZxQhDrD0tdBHls3/wFe8++K2FrPyQF9vuh\n\
-----END PRIVATE KEY-----";

    fn request() -> Request {
        RequestBuilder::new(
            Method::Invite,
            Uri::parse(Bytes::from_static(b"sip:alice@example.com")).unwrap(),
        )
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:+12155551212@example.com;user=phone>;tag=x"),
        )
        .unwrap()
        .header(
            HeaderName::To,
            Bytes::from_static(b"<sip:alice@example.com>"),
        )
        .unwrap()
        .build()
    }

    fn credential() -> SigningCredential {
        SigningCredential::from_pkcs8_pem(
            PRIVATE,
            "https://cert.example.org/passport.cer",
            i64::MIN,
            i64::MAX,
        )
        .unwrap()
    }

    #[test]
    fn signing_adds_date_and_identity_only_after_authority() {
        let mut denied = request();
        let service = AuthenticationService::new(|_: &CanonicalIdentity| false, credential());
        assert_eq!(
            service.sign(&mut denied, 1_471_375_418),
            Err(AuthenticationError::NotAuthoritative)
        );
        assert!(denied.headers.get(&HeaderName::Date).is_none());
        assert!(denied.headers.get(&HeaderName::Identity).is_none());

        let mut allowed = request();
        let service = AuthenticationService::new(|_: &CanonicalIdentity| true, credential());
        service.sign(&mut allowed, 1_471_375_418).unwrap();
        assert!(
            allowed
                .headers
                .typed::<Date>()
                .is_some_and(|date| date.is_ok())
        );
        assert!(
            allowed
                .headers
                .typed::<IdentityHeader>()
                .is_some_and(|identity| identity.is_ok())
        );
    }

    #[derive(Debug)]
    struct CountingFetcher {
        calls: usize,
        key: Es256VerifyingKey,
        result: Option<CredentialError>,
    }

    impl CredentialFetcher for CountingFetcher {
        fn fetch(
            &mut self,
            _info: &str,
            _at: i64,
        ) -> Result<VerificationCredential, CredentialError> {
            self.calls += 1;
            if let Some(error) = self.result {
                return Err(error);
            }
            VerificationCredential::new(self.key.clone(), i64::MIN, i64::MAX)
                .map_err(|_| CredentialError::Unsupported)
        }

        fn authorizes(
            &self,
            _credential: &VerificationCredential,
            _origin: &CanonicalIdentity,
        ) -> bool {
            true
        }
    }

    fn signed() -> (Request, Es256VerifyingKey) {
        let credential = credential();
        let key = credential.verifying_key();
        let service = AuthenticationService::new(|_: &CanonicalIdentity| true, credential);
        let mut request = request();
        service.sign(&mut request, 1_471_375_418).unwrap();
        (request, key)
    }

    #[test]
    fn unsupported_ppt_is_428_and_does_not_fetch() {
        let (mut request, key) = signed();
        let parsed = request.headers.typed::<IdentityHeader>().unwrap().unwrap();
        request.headers.remove_all(&HeaderName::Identity);
        let mut unsupported = parsed;
        unsupported.passport_type = Some("unknown".to_owned());
        request
            .headers
            .push(Header::build(HeaderName::Identity, unsupported.to_bytes()).unwrap());
        let fetcher = CountingFetcher {
            calls: 0,
            key,
            result: None,
        };
        let mut verifier = VerificationService::new(fetcher);
        assert_eq!(
            verifier.verify(&request, 1_471_375_418, true),
            Err(VerificationFailure::MissingIdentity)
        );
        assert_eq!(verifier.into_source().into_inner().calls, 0);
    }

    #[test]
    fn acquisition_failures_keep_their_distinct_statuses() {
        let (request, key) = signed();
        for (error, status) in [
            (CredentialError::Unavailable, 436),
            (CredentialError::Unsupported, 437),
        ] {
            let fetcher = CountingFetcher {
                calls: 0,
                key: key.clone(),
                result: Some(error),
            };
            let mut verifier = VerificationService::new(fetcher);
            assert_eq!(
                verifier
                    .verify(&request, 1_471_375_418, true)
                    .unwrap_err()
                    .status(),
                status
            );
        }
    }

    #[test]
    fn stale_date_is_403_after_credential_acquisition() {
        let (request, key) = signed();
        let fetcher = CountingFetcher {
            calls: 0,
            key,
            result: None,
        };
        let mut verifier = VerificationService::new(fetcher);
        let failure = verifier.verify(&request, 1_471_375_479, true).unwrap_err();
        assert_eq!(failure.status(), 403);
        assert_eq!(verifier.into_source().into_inner().calls, 1);
    }

    #[test]
    fn successful_credentials_are_cached_by_exact_info_uri() {
        let (request, key) = signed();
        let fetcher = CountingFetcher {
            calls: 0,
            key,
            result: None,
        };
        let mut verifier = VerificationService::new(fetcher);
        verifier.verify(&request, 1_471_375_418, true).unwrap();
        verifier.verify(&request, 1_471_375_418, true).unwrap();
        assert_eq!(verifier.into_source().into_inner().calls, 1);
    }

    #[test]
    fn cached_credentials_are_bounded_and_evict_by_exact_info_uri() {
        let (_, key) = signed();
        let fetcher = CountingFetcher {
            calls: 0,
            key,
            result: None,
        };
        let mut cache = CachedCredentials::new(fetcher, 1);
        cache.fetch("https://cert.example.org/one", 1).unwrap();
        cache.fetch("https://cert.example.org/one", 1).unwrap();
        cache.fetch("https://cert.example.org/two", 1).unwrap();
        cache.fetch("https://cert.example.org/one", 1).unwrap();
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.into_inner().calls, 3);
    }

    #[test]
    fn an_expired_cached_credential_is_refetched_at_the_same_uri() {
        #[derive(Debug)]
        struct RotatingFetcher {
            calls: usize,
            key: Es256VerifyingKey,
        }

        impl CredentialFetcher for RotatingFetcher {
            fn fetch(
                &mut self,
                _info: &str,
                _at: i64,
            ) -> Result<VerificationCredential, CredentialError> {
                self.calls += 1;
                let not_after = if self.calls == 1 { 10 } else { i64::MAX };
                VerificationCredential::new(self.key.clone(), i64::MIN, not_after)
                    .map_err(|_| CredentialError::Unsupported)
            }

            fn authorizes(
                &self,
                _credential: &VerificationCredential,
                _origin: &CanonicalIdentity,
            ) -> bool {
                true
            }
        }

        let (_, key) = signed();
        let fetcher = RotatingFetcher { calls: 0, key };
        let mut cache = CachedCredentials::new(fetcher, 1);
        cache.fetch("https://cert.example.org/one", 10).unwrap();
        cache.fetch("https://cert.example.org/one", 11).unwrap();
        assert_eq!(cache.entries.len(), 1);
        assert_eq!(cache.into_inner().calls, 2);
    }

    #[test]
    fn malformed_identity_is_438_when_identity_is_required() {
        let (_, key) = signed();
        let mut request = request();
        request.headers.push(
            Header::build(HeaderName::Identity, Bytes::from_static(b"not-a-passport")).unwrap(),
        );
        let fetcher = CountingFetcher {
            calls: 0,
            key,
            result: None,
        };
        let mut verifier = VerificationService::new(fetcher);
        assert_eq!(
            verifier.verify(&request, 1_471_375_418, true),
            Err(VerificationFailure::InvalidIdentity)
        );
        assert_eq!(verifier.into_source().into_inner().calls, 0);
    }

    #[test]
    fn full_passport_iat_must_equal_the_sip_date() {
        let (mut request, key) = signed();
        request.headers.remove_all(&HeaderName::Date);
        let different_but_fresh = date_from_timestamp(1_471_375_419).unwrap();
        request
            .headers
            .push(Header::build(HeaderName::Date, Bytes::from(different_but_fresh.0)).unwrap());
        let fetcher = CountingFetcher {
            calls: 0,
            key,
            result: None,
        };
        let mut verifier = VerificationService::new(fetcher);
        assert_eq!(
            verifier.verify(&request, 1_471_375_419, true),
            Err(VerificationFailure::InvalidIdentity)
        );
    }
}
