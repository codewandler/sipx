//! Live-call authenticated identity (story `S-34`).
//!
//! The outbound proof deliberately does not call sipx's `PASSporT` parser or verifier. It reads the
//! field off the request the transport delivered, decodes the two JWS JSON objects directly, and
//! asks a separate cryptographic implementation to verify the RFC 7518 signature.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use openssl::bn::BigNum;
use openssl::ecdsa::EcdsaSig;
use openssl::hash::MessageDigest;
use openssl::pkey::PKey;
use openssl::sign::Verifier;
use serde_json::Value;
use sipx_call::{
    DialOptions, Dispatched, Dispatcher, InboundIdentityPolicy, OutboundIdentityPolicy, dial,
};
use sipx_sip::build::RequestBuilder;
use sipx_sip::identity::{CanonicalIdentity, Es256VerifyingKey};
use sipx_sip::{Header, HeaderName, Method, Request, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use sipx_ua::identity::{
    AuthenticationService, CredentialError, CredentialFetcher, SigningCredential,
    VerificationCredential, VerificationService,
};
use tokio::sync::mpsc::{Receiver, error::TryRecvError};

const NOW: i64 = 1_471_375_418;
const INFO: &str = "https://cert.example.org/passport.cer";
const RFC_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgi7q2TZvN9VDFg8Vy\n\
qCP06bETrR2v8MRvr89rn4i+UAahRANCAAQWfaj1HUETpoNCrOtp9KA8o0V79IuW\n\
ARKt9C1cFPkyd3FBP4SeiNZxQhDrD0tdBHls3/wFe8++K2FrPyQF9vuh\n\
-----END PRIVATE KEY-----";
const RFC_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEFn2o9R1BE6aDQqzrafSgPKNFe/SL\n\
lgESrfQtXBT5MndxQT+EnojWcUIQ6w9LXQR5bN/8BXvPvithaz8kBfb7oQ==\n\
-----END PUBLIC KEY-----";

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid loopback")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid address")))
        .await
        .expect("endpoint binds")
}

fn callee_uri() -> Uri {
    Uri::parse(Bytes::from_static(b"sip:alice@example.com")).expect("valid callee URI")
}

fn signing_credential() -> SigningCredential {
    SigningCredential::from_pkcs8_pem(RFC_PRIVATE_KEY, INFO, i64::MIN, i64::MAX)
        .expect("the RFC key is valid")
}

fn outbound_policy() -> OutboundIdentityPolicy {
    OutboundIdentityPolicy::new(
        AuthenticationService::new(|_: &CanonicalIdentity| true, signing_credential()),
        || NOW,
    )
}

fn independently_verify_wire_identity(request: &Request) {
    let value = request
        .headers
        .get(&HeaderName::Identity)
        .expect("the live INVITE carries Identity")
        .value();
    let text = std::str::from_utf8(&value).expect("Identity is ASCII");
    let mut fields = text.split(';');
    let digest = fields.next().expect("the digest precedes parameters");
    let mut saw_info = false;
    // RFC 8224 §4.1 defaults an absent `alg` parameter to ES256. Keep that default in this
    // independent parser rather than borrowing the typed header's interpretation.
    let mut algorithm = "ES256";
    for field in fields {
        let (name, value) = field
            .trim()
            .split_once('=')
            .expect("every baseline Identity parameter has a value");
        match name {
            "info" => {
                assert_eq!(value, format!("<{INFO}>"));
                saw_info = true;
            }
            "alg" => {
                algorithm = value;
            }
            other => panic!("unexpected baseline Identity parameter: {other}"),
        }
    }
    assert!(saw_info, "Identity carries its credential-reference URI");
    assert_eq!(
        algorithm, "ES256",
        "Identity selects the baseline algorithm"
    );
    assert_eq!(
        request
            .headers
            .get(&HeaderName::Date)
            .expect("the live INVITE carries Date")
            .value()
            .as_ref(),
        b"Tue, 16 Aug 2016 19:23:38 GMT"
    );
    let segments: Vec<_> = digest.split('.').collect();
    assert_eq!(segments.len(), 3, "a full PASSporT has three JWS segments");

    let header: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(segments[0])
            .expect("the protected header is base64url"),
    )
    .expect("the protected header is JSON");
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(segments[1])
            .expect("the claims are base64url"),
    )
    .expect("the claims are JSON");
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["typ"], "passport");
    assert_eq!(header["x5u"], INFO);
    assert_eq!(claims["iat"], NOW);
    assert_eq!(claims["orig"]["tn"], "12155551212");
    assert_eq!(claims["dest"]["uri"][0], "sip:alice@example.com");

    let raw = URL_SAFE_NO_PAD
        .decode(segments[2])
        .expect("the signature is base64url");
    assert_eq!(raw.len(), 64, "RFC 7518 ES256 is a 64-octet R || S");
    let signature = EcdsaSig::from_private_components(
        BigNum::from_slice(&raw[..32]).expect("R is an integer"),
        BigNum::from_slice(&raw[32..]).expect("S is an integer"),
    )
    .expect("R and S form a signature")
    .to_der()
    .expect("the verifier's native signature form");
    let public =
        PKey::public_key_from_pem(RFC_PUBLIC_KEY.as_bytes()).expect("the RFC public key parses");
    let mut verifier =
        Verifier::new(MessageDigest::sha256(), &public).expect("the independent verifier starts");
    verifier
        .update(format!("{}.{}", segments[0], segments[1]).as_bytes())
        .expect("the signing input is accepted");
    assert!(
        verifier
            .verify(&signature)
            .expect("the independent verifier returns a result"),
        "an independent ES256 verifier rejected the live INVITE"
    );
}

/// `S-34`'s outbound failing-first path and independent-verifier evidence.
#[tokio::test]
async fn signing_selected_on_an_outbound_call_reaches_an_independent_verifier() {
    let (answerer, incoming) = endpoint().await;
    let destination = answerer.local_addr();
    let server = tokio::spawn(async move {
        let mut dispatcher = Dispatcher::new(answerer.clone(), incoming);
        let Some(Dispatched::Invitation(invitation)) = dispatcher.next().await else {
            panic!("the signed call is surfaced")
        };
        independently_verify_wire_identity(&invitation.request().request);
        invitation
            .answer(&answerer, loopback())
            .await
            .expect("the verified call is answered")
    });

    let (originator, _incoming) = endpoint().await;
    let options = DialOptions::new("<sip:+12155551212@example.com;user=phone>", loopback())
        .with_identity(outbound_policy());
    let outbound_call = dial(
        &originator,
        Target::udp(destination),
        &callee_uri(),
        &options,
    )
    .await
    .expect("the signed live call connects");
    let inbound_call = server.await.expect("the answerer finishes");

    drop(outbound_call);
    drop(inbound_call);
}

#[derive(Debug)]
struct RfcCredential(Es256VerifyingKey);

impl CredentialFetcher for RfcCredential {
    fn fetch(&mut self, info: &str, _at: i64) -> Result<VerificationCredential, CredentialError> {
        if info != INFO {
            return Err(CredentialError::Unavailable);
        }
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

fn inbound_policy(required: bool) -> InboundIdentityPolicy {
    let credential = signing_credential();
    InboundIdentityPolicy::new(
        VerificationService::new(RfcCredential(credential.verifying_key())),
        required,
        || NOW,
    )
}

fn via(endpoint: &Handle) -> Bytes {
    Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ))
}

fn raw_invite(endpoint: &Handle, call_id: &str) -> Request {
    RequestBuilder::new(Method::Invite, callee_uri())
        .header(HeaderName::Via, via(endpoint))
        .expect("Via")
        .header(
            HeaderName::To,
            Bytes::from_static(b"<sip:alice@example.com>"),
        )
        .expect("To")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:+12155551212@example.com;user=phone>;tag=caller"),
        )
        .expect("From")
        .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
        .expect("Call-ID")
        .cseq(1, &Method::Invite)
        .expect("CSeq")
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:caller@{}>", endpoint.local_addr())),
        )
        .expect("Contact")
        .max_forwards(70)
        .build()
}

fn signed_invite(endpoint: &Handle, call_id: &str) -> Request {
    let mut request = raw_invite(endpoint, call_id);
    AuthenticationService::new(|_: &CanonicalIdentity| true, signing_credential())
        .sign(&mut request, NOW)
        .expect("the test INVITE signs");
    request
}

fn corrupt_signature(request: &mut Request) {
    let mut value = request
        .headers
        .get(&HeaderName::Identity)
        .expect("Identity exists")
        .value()
        .into_owned();
    let digest_end = value
        .iter()
        .position(|byte| *byte == b';')
        .unwrap_or(value.len());
    let signature = value[..digest_end]
        .iter()
        .rposition(|byte| *byte == b'.')
        .and_then(|dot| dot.checked_add(1))
        .expect("the full token has a signature");
    value[signature] = if value[signature] == b'A' { b'B' } else { b'A' };
    request.headers.remove_all(&HeaderName::Identity);
    request.headers.push(
        Header::build(HeaderName::Identity, Bytes::from(value))
            .expect("changed Identity is a header"),
    );
}

async fn send_for_final(peer: &Handle, callee: SocketAddr, request: Request) -> sipx_sip::Response {
    let mut responses = peer
        .send(request, Target::udp(callee))
        .await
        .expect("the INVITE sends");
    tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("the verifier answers")
        .expect("the INVITE gets a final response")
}

/// `S-34`'s inbound failing-first path: the application never receives an answerable handle.
#[tokio::test]
async fn verification_selected_on_an_inbound_call_refuses_an_invalid_signature_before_answer() {
    let (callee, incoming) = endpoint().await;
    let callee_addr = callee.local_addr();
    let mut dispatcher =
        Dispatcher::new(callee.clone(), incoming).with_identity(inbound_policy(true));
    let calls = dispatcher.calls();
    let (surfaced, mut application) = tokio::sync::mpsc::channel(1);
    let pump = tokio::spawn(async move {
        while let Some(event) = dispatcher.next().await {
            if surfaced.send(event).await.is_err() {
                return;
            }
        }
    });

    let (peer, _incoming) = endpoint().await;
    let mut invite = signed_invite(&peer, "bad-signature@sipx");
    corrupt_signature(&mut invite);
    let response = send_for_final(&peer, callee_addr, invite).await;
    assert_eq!(response.status.code(), 438);
    assert_eq!(response.reason.as_ref(), b"Invalid Identity Header");
    assert_eq!(calls.counts().identity, 1);
    assert!(
        calls.is_empty(),
        "a refused identity reserves no call route"
    );
    assert!(
        matches!(application.try_recv(), Err(TryRecvError::Empty)),
        "the application must never receive an answerable invitation"
    );
    pump.abort();
    pump.await.expect_err("the bounded test pump is cancelled");
}

#[tokio::test]
async fn required_and_optional_missing_identity_are_distinct_call_policies() {
    let (required_endpoint, required_incoming) = endpoint().await;
    let required_addr = required_endpoint.local_addr();
    let mut required =
        Dispatcher::new(required_endpoint, required_incoming).with_identity(inbound_policy(true));
    let required_task = tokio::spawn(async move { required.next().await });
    let (peer, _incoming) = endpoint().await;
    let response = send_for_final(
        &peer,
        required_addr,
        raw_invite(&peer, "missing-required@sipx"),
    )
    .await;
    assert_eq!(response.status.code(), 428);
    required_task.abort();
    required_task
        .await
        .expect_err("the rejected dispatcher is stopped");

    let (optional_endpoint, optional_incoming) = endpoint().await;
    let optional_addr = optional_endpoint.local_addr();
    let mut optional =
        Dispatcher::new(optional_endpoint, optional_incoming).with_identity(inbound_policy(false));
    let (peer, _incoming) = endpoint().await;
    let responses = peer
        .send(
            raw_invite(&peer, "missing-optional@sipx"),
            Target::udp(optional_addr),
        )
        .await
        .expect("the optional INVITE sends");
    let surfaced = tokio::time::timeout(Duration::from_secs(5), optional.next())
        .await
        .expect("the optional request is dispatched")
        .expect("the dispatcher remains open");
    assert!(matches!(surfaced, Dispatched::Invitation(_)));
    drop(responses);
}

#[tokio::test]
async fn an_unselected_outbound_call_adds_no_identity_and_reads_no_policy_input() {
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&reads);
    let unused = OutboundIdentityPolicy::new(
        AuthenticationService::new(|_: &CanonicalIdentity| true, signing_credential()),
        move || {
            observed.fetch_add(1, Ordering::Relaxed);
            NOW
        },
    );

    let (receiver, mut incoming) = endpoint().await;
    let destination = receiver.local_addr();
    let seen = tokio::spawn(async move {
        let invite = incoming.recv().await.expect("the ordinary INVITE arrives");
        assert!(invite.request.headers.get(&HeaderName::Identity).is_none());
        assert!(invite.request.headers.get(&HeaderName::Date).is_none());
    });
    let (originator, _incoming) = endpoint().await;
    let _ = dial(
        &originator,
        Target::udp(destination),
        &callee_uri(),
        &DialOptions::new("<sip:caller@example.com>", loopback())
            .with_timeout(Duration::from_millis(100)),
    )
    .await;
    seen.await.expect("the wire assertion finishes");
    assert_eq!(reads.load(Ordering::Relaxed), 0);
    drop(unused);
}
