//! Caller-owned authenticated-identity policies for live calls.
//!
//! The cryptographic services live in `sipx-ua`; this module only composes them with call
//! establishment. Time, authority, credentials, retrieval and trust all remain values supplied by
//! the application.

use std::fmt;
use std::sync::Arc;

use sipx_sip::Request;
use sipx_ua::identity::{
    AuthenticationError, AuthenticationService, Authority, CredentialFetcher, VerificationFailure,
    VerificationService,
};

trait SignCall: Send + Sync {
    fn sign(&self, request: &mut Request) -> Result<(), AuthenticationError>;
}

struct SelectedAuthentication<A, N> {
    service: AuthenticationService<A>,
    now: N,
}

impl<A, N> SignCall for SelectedAuthentication<A, N>
where
    A: Authority + Send + Sync,
    N: Fn() -> i64 + Send + Sync,
{
    fn sign(&self, request: &mut Request) -> Result<(), AuthenticationError> {
        self.service.sign(request, (self.now)())
    }
}

/// Authentication-service selection for an outbound call.
///
/// The caller supplies both the already configured service and the function that reads its notion
/// of current Unix time. Constructing a policy performs no authority check and reads no clock;
/// those inputs are used only when an INVITE attempt is about to be sent.
#[derive(Clone)]
pub struct OutboundIdentityPolicy {
    signer: Arc<dyn SignCall>,
}

impl OutboundIdentityPolicy {
    /// Select an authentication service and a caller-owned time source.
    #[must_use]
    pub fn new<A, N>(service: AuthenticationService<A>, now: N) -> Self
    where
        A: Authority + Send + Sync + 'static,
        N: Fn() -> i64 + Send + Sync + 'static,
    {
        Self {
            signer: Arc::new(SelectedAuthentication { service, now }),
        }
    }

    pub(crate) fn sign(&self, request: &mut Request) -> Result<(), AuthenticationError> {
        self.signer.sign(request)
    }
}

impl fmt::Debug for OutboundIdentityPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundIdentityPolicy")
            .finish_non_exhaustive()
    }
}

trait VerifyCall: Send + Sync {
    fn verify(&mut self, request: &Request) -> Result<(), VerificationFailure>;
}

struct SelectedVerification<S, N> {
    service: VerificationService<S>,
    now: N,
    required: bool,
}

impl<S, N> VerifyCall for SelectedVerification<S, N>
where
    S: CredentialFetcher + Send + Sync,
    N: Fn() -> i64 + Send + Sync,
{
    fn verify(&mut self, request: &Request) -> Result<(), VerificationFailure> {
        self.service
            .verify(request, (self.now)(), self.required)
            .map(|_| ())
    }
}

/// Verification-service selection for inbound calls handled by a dispatcher.
///
/// The policy owns the service because its bounded credential cache is stateful. `required`
/// decides whether an INVITE without a usable `Identity` is refused with 428 or surfaced as an
/// unverified ordinary call. The time function and the service's credential fetcher are both
/// caller-owned; the call layer performs neither clock nor network I/O for them.
pub struct InboundIdentityPolicy {
    verifier: Box<dyn VerifyCall>,
}

impl InboundIdentityPolicy {
    /// Select a verification service, missing-identity policy, and caller-owned time source.
    #[must_use]
    pub fn new<S, N>(service: VerificationService<S>, required: bool, now: N) -> Self
    where
        S: CredentialFetcher + Send + Sync + 'static,
        N: Fn() -> i64 + Send + Sync + 'static,
    {
        Self {
            verifier: Box::new(SelectedVerification {
                service,
                now,
                required,
            }),
        }
    }

    pub(crate) fn verify(&mut self, request: &Request) -> Result<(), VerificationFailure> {
        self.verifier.verify(request)
    }
}

impl fmt::Debug for InboundIdentityPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InboundIdentityPolicy")
            .finish_non_exhaustive()
    }
}
