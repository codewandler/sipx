//! RFC 8224 authentication and verification services.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::identity::{CanonicalIdentity, Es256VerifyingKey, IdentityHeader};
use sipx_sip::{HeaderName, Method, Uri};
use sipx_ua::identity::{
    AuthenticationService, CredentialError, CredentialFetcher, SigningCredential,
    VerificationCredential, VerificationService,
};

const RFC_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgi7q2TZvN9VDFg8Vy\n\
qCP06bETrR2v8MRvr89rn4i+UAahRANCAAQWfaj1HUETpoNCrOtp9KA8o0V79IuW\n\
ARKt9C1cFPkyd3FBP4SeiNZxQhDrD0tdBHls3/wFe8++K2FrPyQF9vuh\n\
-----END PRIVATE KEY-----";

struct RfcCredential(Es256VerifyingKey);

impl CredentialFetcher for RfcCredential {
    fn fetch(&mut self, info: &str, _at: i64) -> Result<VerificationCredential, CredentialError> {
        assert_eq!(info, "https://cert.example.org/passport.cer");
        VerificationCredential::new(self.0.clone(), i64::MIN, i64::MAX)
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

/// `S-20`'s failing-first test: a verifier reaches cryptographic validation and assigns the
/// specific failure RFC 8224 §6.2.2 gives it, rather than accepting a parsed token or returning 400.
#[test]
fn a_request_whose_identity_signature_does_not_verify_is_refused_with_438() {
    let to = Uri::parse(Bytes::from_static(b"sip:alice@example.com")).expect("valid URI");
    let mut request = RequestBuilder::new(Method::Invite, to)
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:+12155551212@example.com;user=phone>;tag=rfc"),
        )
        .expect("valid From")
        .header(
            HeaderName::To,
            Bytes::from_static(b"<sip:alice@example.com>"),
        )
        .expect("valid To")
        .header(
            HeaderName::Date,
            Bytes::from_static(b"Tue, 16 Aug 2016 19:23:38 GMT"),
        )
        .expect("valid Date")
        .build();

    let signing = SigningCredential::from_pkcs8_pem(
        RFC_PRIVATE_KEY,
        "https://cert.example.org/passport.cer",
        i64::MIN,
        i64::MAX,
    )
    .expect("the RFC key is valid");
    let public = signing.verifying_key();
    AuthenticationService::new(|_: &CanonicalIdentity| true, signing)
        .sign(&mut request, 1_471_375_418)
        .expect("signs the request");
    let mut identity = request
        .headers
        .typed::<IdentityHeader>()
        .expect("Identity is present")
        .expect("Identity is typed");
    let final_character = identity.digest.pop().expect("signature is non-empty");
    identity
        .digest
        .push(if final_character == 'A' { 'B' } else { 'A' });
    request.headers.remove_all(&HeaderName::Identity);
    request.headers.push(
        sipx_sip::Header::build(HeaderName::Identity, identity.to_bytes()).expect("valid header"),
    );

    let mut verifier = VerificationService::new(RfcCredential(public));
    let failure = verifier
        .verify(&request, 1_471_375_418, true)
        .expect_err("the changed RFC signature must fail");
    assert_eq!(failure.status(), 438);
}
