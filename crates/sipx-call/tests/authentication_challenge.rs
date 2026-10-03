//! Dispatcher-owned authentication refusal (RFC 3261 sections 9.2 and 22).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use bytes::Bytes;
use sipx_call::{Dispatched, Dispatcher, Invitation};
use sipx_sip::{HeaderName, Limits, Message, Response, ResponseBuilder, StatusCode};
use sipx_transport::{Config, Handle, bind};
use sipx_ua::{Algorithm, Authenticator, Challenge, Credentials, Presented, Verdict};
use std::time::Duration;
use tokio::{net::UdpSocket, sync::mpsc, task::JoinHandle};

const LIMIT: Duration = Duration::from_secs(3);
const URI: &str = "sip:service@fixture.invalid";
const CHALLENGE: &str =
    "Digest realm=\"fixture.invalid\", nonce=\"fixture-nonce\", algorithm=SHA-256, qop=\"auth\"";

struct Harness {
    peer: UdpSocket,
    endpoint: Handle,
    invitations: mpsc::Receiver<Invitation>,
    driver: JoinHandle<()>,
}
impl Harness {
    async fn new() -> Self {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (endpoint, incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
            .await
            .unwrap();
        let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
        let (tx, invitations) = mpsc::channel(4);
        let handle = endpoint.clone();
        let driver = tokio::spawn(async move {
            while let Some(event) = dispatcher.next().await {
                match event {
                    Dispatched::Invitation(invitation) => tx.send(invitation).await.unwrap(),
                    Dispatched::OutOfDialog(request) => {
                        let response = ResponseBuilder::to_request(
                            &request.request,
                            StatusCode::new(200).unwrap(),
                            "OK",
                        )
                        .unwrap()
                        .build();
                        handle.respond(&request.key, response).await.unwrap();
                    }
                    _ => {}
                }
            }
        });
        Self {
            peer,
            endpoint,
            invitations,
            driver,
        }
    }
    async fn send(&self, method: &str, sequence: u32, branch: &str, authorization: Option<&str>) {
        let auth =
            authorization.map_or_else(String::new, |value| format!("Authorization: {value}\r\n"));
        let call_id = if method == "OPTIONS" {
            "barrier-fixture"
        } else {
            "challenge-fixture"
        };
        let message = format!(
            "{method} {URI} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch={branch};rport\r\nFrom: <sip:browser@fixture.invalid>;tag=browser\r\nTo: <sip:service@fixture.invalid>\r\nCall-ID: {call_id}\r\nCSeq: {sequence} {method}\r\nContact: <sip:browser@{}>\r\nMax-Forwards: 70\r\n{auth}Content-Length: 0\r\n\r\n",
            self.peer.local_addr().unwrap(),
            self.peer.local_addr().unwrap()
        );
        self.peer
            .send_to(message.as_bytes(), self.endpoint.local_addr())
            .await
            .unwrap();
    }
    async fn invitation(&mut self) -> Invitation {
        tokio::time::timeout(LIMIT, self.invitations.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn response(&self) -> Response {
        let mut data = [0; 8192];
        loop {
            let (length, _) = tokio::time::timeout(LIMIT, self.peer.recv_from(&mut data))
                .await
                .unwrap()
                .unwrap();
            let Message::Response(response) = sipx_sip::parse_datagram(
                Bytes::copy_from_slice(&data[..length]),
                &Limits::default(),
            )
            .unwrap() else {
                panic!("expected response")
            };
            if response.status.code() >= 200 {
                return response;
            }
        }
    }
    // The OPTIONS response is a causal barrier behind already awaited response sends;
    // this tests absence of a conflicting final without substituting a fixed sleep.
    async fn barrier(&self) -> Vec<Response> {
        self.send("OPTIONS", 3, "z9hG4bK-barrier", None).await;
        let mut observed = Vec::new();
        loop {
            let response = self.response().await;
            if response.headers.value(&HeaderName::CSeq).unwrap().as_ref() == b"3 OPTIONS" {
                assert_eq!(response.status.code(), 200);
                return observed;
            }
            assert!(observed.len() < 16, "bounded response collection");
            observed.push(response);
        }
    }
    async fn close(self) {
        self.endpoint.shutdown().await;
        tokio::time::timeout(LIMIT, self.driver)
            .await
            .unwrap()
            .unwrap();
    }
}
fn tag(response: &Response) -> String {
    let to = response.headers.value(&HeaderName::To).unwrap();
    let parsed = sipx_sip::Address::parse(&to, "To").unwrap();
    String::from_utf8(parsed.tag().unwrap().to_vec()).unwrap()
}

#[tokio::test]
async fn exact_challenge_preserves_transaction_and_authenticates_fresh_retry() {
    let mut h = Harness::new().await;
    let mut authenticator =
        Authenticator::new("fixture.invalid", [31; 32]).with_algorithm(Algorithm::Sha256);
    let challenge = authenticator.challenge(false);
    h.send("INVITE", 1, "z9hG4bK-initial", None).await;
    let invitation = h.invitation().await;
    invitation
        .challenge(&h.endpoint, challenge.clone())
        .await
        .unwrap();
    let response = h.response().await;
    assert_eq!(response.status.code(), 401);
    assert_eq!(response.headers.count(&HeaderName::WwwAuthenticate), 1);
    assert_eq!(
        response
            .headers
            .value(&HeaderName::WwwAuthenticate)
            .unwrap()
            .as_ref(),
        challenge.as_bytes()
    );
    for name in [
        HeaderName::Via,
        HeaderName::From,
        HeaderName::CallId,
        HeaderName::CSeq,
    ] {
        assert_eq!(
            response.headers.value(&name),
            invitation.request().request.headers.value(&name)
        );
    }
    assert!(!tag(&response).is_empty());
    assert!(response.body().is_empty());
    assert_eq!(response.headers.count(&HeaderName::Contact), 0);
    let parsed = Challenge::parse(challenge.as_bytes(), false).unwrap();
    let authorization = sipx_ua::auth::respond(
        &parsed,
        &Credentials::new("browser", "fixture-password"),
        "INVITE",
        URI,
        1,
        "fixture-client-nonce",
    );
    h.send("INVITE", 2, "z9hG4bK-retry", Some(&authorization))
        .await;
    let retry = h.invitation().await;
    let presented = Presented::from_request(&retry.request().request, false).unwrap();
    assert_eq!(presented.uri, retry.request().request.uri.to_string());
    assert_eq!(
        authenticator.verify(&presented, "INVITE", "fixture-password"),
        Verdict::Authenticated
    );
    retry.refuse(&h.endpoint, 486, "Busy Here").await.unwrap();
    assert_eq!(h.response().await.status.code(), 486);
    h.close().await;
}

#[tokio::test]
async fn cancel_before_challenge_owns_the_only_invite_final() {
    let mut h = Harness::new().await;
    h.send("INVITE", 1, "z9hG4bK-initial", None).await;
    let invitation = h.invitation().await;
    h.send("CANCEL", 1, "z9hG4bK-initial", None).await;
    let a = h.response().await;
    let b = h.response().await;
    let result = invitation.challenge(&h.endpoint, CHALLENGE).await;
    let later = h.barrier().await;
    h.close().await;
    assert!(matches!(result, Err(sipx_call::Error::InvitationCancelled)));
    assert_eq!([a.status.code(), b.status.code()], [200, 487]);
    assert_eq!(tag(&a), tag(&b));
    assert!(later.iter().all(|response| response.status.code() != 401));
}

#[tokio::test]
async fn challenge_before_cancel_preserves_401_and_native_tag() {
    let mut h = Harness::new().await;
    h.send("INVITE", 1, "z9hG4bK-initial", None).await;
    let invitation = h.invitation().await;
    invitation.challenge(&h.endpoint, CHALLENGE).await.unwrap();
    let challenged = h.response().await;
    h.send("CANCEL", 1, "z9hG4bK-initial", None).await;
    let cancelled = h.response().await;
    let later = h.barrier().await;
    h.close().await;
    assert_eq!(challenged.status.code(), 401);
    assert_eq!(cancelled.status.code(), 200);
    assert_eq!(tag(&challenged), tag(&cancelled));
    assert!(!invitation.is_cancelled());
    assert!(later.iter().all(|response| response.status.code() != 487));
}

#[tokio::test]
async fn invalid_challenge_value_leaves_pending_invitation_cancellable() {
    let mut h = Harness::new().await;
    h.send("INVITE", 1, "z9hG4bK-initial", None).await;
    let invitation = h.invitation().await;
    let result = invitation
        .challenge(
            &h.endpoint,
            "Digest realm=\"fixture.invalid\"\r\nTo: <sip:injected@fixture.invalid>",
        )
        .await;
    h.send("CANCEL", 1, "z9hG4bK-initial", None).await;
    let a = h.response().await;
    let b = h.response().await;
    let later = h.barrier().await;
    h.close().await;
    assert!(
        matches!(result, Err(sipx_call::Error::Build(_))),
        "invalid header must fail before claim: {result:?}"
    );
    assert_eq!([a.status.code(), b.status.code()], [200, 487]);
    assert!(invitation.is_cancelled());
    assert!(later.iter().all(|response| response.status.code() != 401));
}

#[tokio::test]
async fn adversary_oversized_response_currently_reports_success_but_retains_challenge_ownership() {
    let mut h = Harness::new().await;
    h.send("INVITE", 1, "z9hG4bK-send-error", None).await;
    let invitation = h.invitation().await;
    // A legal header value larger than one UDP datagram drives a real socket-send
    // failure after response construction and claiming; no endpoint mock is used.
    // Current behavior: the transport counts the failure but reports Ok.
    // story:sipx-response-send-result tracks this gap; change the result expectation
    // to the typed transport error when that story fixes response-send reporting.
    let oversized = format!("Digest realm=\"{}\", nonce=\"fixture\"", "r".repeat(70_000));
    let result = tokio::time::timeout(LIMIT, invitation.challenge(&h.endpoint, oversized))
        .await
        .unwrap();
    let send_failures = h.endpoint.counters().discards.send_failures;
    h.send("CANCEL", 1, "z9hG4bK-send-error", None).await;
    let cancellation = h.response().await;
    let later = h.barrier().await;
    let still_claimed = !invitation.is_cancelled();
    h.close().await;
    assert_eq!(cancellation.status.code(), 200);
    assert_eq!(
        cancellation
            .headers
            .value(&HeaderName::CSeq)
            .unwrap()
            .as_ref(),
        b"1 CANCEL"
    );
    assert!(
        still_claimed,
        "send failure must not return final ownership to CANCEL"
    );
    assert!(later.iter().all(|response| response.status.code() != 487));
    assert_eq!(
        send_failures, 1,
        "the actual socket refused exactly one send"
    );
    println!(
        "observed send_failures={send_failures}, CANCEL=200, retained_claim={still_claimed}, challenge_result={result:?}"
    );
    assert!(
        result.is_ok(),
        "current response-send reporting returns Ok despite the observed failure: {result:?}"
    );
}

#[tokio::test]
async fn adversary_multiple_invalid_headers_leave_same_invitation_available_for_valid_challenge() {
    let mut h = Harness::new().await;
    h.send("INVITE", 1, "z9hG4bK-validation", None).await;
    let invitation = h.invitation().await;
    for value in [
        "Digest realm=\"fixture.invalid\"\rProxy-Authenticate: injected",
        "Digest realm=\"fixture.invalid\"\nProxy-Authenticate: injected",
        "Digest realm=\"fixture.invalid\"\r\n Proxy-Authenticate: injected",
    ] {
        let error = invitation.challenge(&h.endpoint, value).await;
        assert!(
            matches!(error, Err(sipx_call::Error::Build(_))),
            "injection was not refused: {error:?}"
        );
        assert!(!invitation.is_cancelled());
    }
    invitation.challenge(&h.endpoint, CHALLENGE).await.unwrap();
    let challenge = h.response().await;
    h.send("CANCEL", 1, "z9hG4bK-validation", None).await;
    let cancellation = h.response().await;
    let later = h.barrier().await;
    h.close().await;
    assert_eq!(challenge.status.code(), 401);
    assert_eq!(challenge.headers.count(&HeaderName::WwwAuthenticate), 1);
    assert_eq!(challenge.headers.count(&HeaderName::ProxyAuthenticate), 0);
    assert_eq!(
        challenge
            .headers
            .value(&HeaderName::WwwAuthenticate)
            .unwrap()
            .as_ref(),
        CHALLENGE.as_bytes()
    );
    assert_eq!(cancellation.status.code(), 200);
    assert_eq!(tag(&challenge), tag(&cancellation));
    assert!(later.iter().all(|response| response.status.code() != 487));
}

#[tokio::test]
async fn adversary_endpoint_closed_challenge_claim_survives_already_received_cancel() {
    for challenge_first in [false, true] {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (endpoint, mut incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
            .await
            .unwrap();
        // Buffer actual transport-produced requests so endpoint shutdown can occur
        // between receipt and dispatch. This is the ordinary queued-CANCEL ordering,
        // with one application owner and the same endpoint for every operation.
        let (queued, requests) = mpsc::channel(2);
        let mut dispatcher = Dispatcher::new(endpoint.clone(), requests);
        let mut invitation = None;
        for method in ["INVITE", "CANCEL"] {
            let message = format!(
                "{method} {URI} SIP/2.0\r\nVia: SIP/2.0/UDP {};branch=z9hG4bK-stopped;rport\r\nFrom: <sip:browser@fixture.invalid>;tag=browser\r\nTo: <sip:service@fixture.invalid>\r\nCall-ID: stopped-fixture\r\nCSeq: 1 {method}\r\nContact: <sip:browser@fixture.invalid>\r\nMax-Forwards: 70\r\nContent-Length: 0\r\n\r\n",
                peer.local_addr().unwrap()
            );
            peer.send_to(message.as_bytes(), endpoint.local_addr())
                .await
                .unwrap();
            let actual = tokio::time::timeout(LIMIT, incoming.recv())
                .await
                .unwrap()
                .unwrap();
            queued.send(actual).await.unwrap();
            if method == "INVITE" {
                let event = tokio::time::timeout(LIMIT, dispatcher.next())
                    .await
                    .unwrap()
                    .unwrap();
                let Dispatched::Invitation(value) = event else {
                    panic!("expected actual invitation");
                };
                invitation = Some(value);
            }
        }
        endpoint.shutdown().await;
        let invitation = invitation.unwrap();
        if challenge_first {
            let result = invitation.challenge(&endpoint, CHALLENGE).await;
            assert!(matches!(
                result,
                Err(sipx_call::Error::Transport(
                    sipx_transport::Error::EndpointClosed
                ))
            ));
        }
        drop(queued);
        assert!(
            tokio::time::timeout(LIMIT, dispatcher.next())
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            invitation.is_cancelled(),
            !challenge_first,
            "stopped-endpoint send error retains ownership; the no-challenge control cancels"
        );
    }
}
