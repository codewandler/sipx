//! The registration path observation reaches the runtime surface without becoming routing policy.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::headers::Via;
use sipx_sip::{Header, HeaderName, Host, HostName, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Incoming, Target, TransportKind, bind};
use sipx_ua::{
    Config, Credentials, RegistrationObservation, RegistrationObservationError, UserAgent,
};

const OBSERVED: &str = "203.0.113.9:41234";
const CHALLENGE_OBSERVED: &str = "198.51.100.7:50999";

#[derive(Clone, Copy)]
enum ReplyObservation {
    Learned(SocketAddr),
    Absent,
}

fn with_observation(
    request: &sipx_sip::Request,
    mut response: sipx_sip::Response,
    observation: ReplyObservation,
) -> sipx_sip::Response {
    let via = request
        .headers
        .typed::<Via>()
        .expect("REGISTER has Via")
        .expect("Via parses");
    let raw = request
        .headers
        .value(&HeaderName::Via)
        .expect("REGISTER has Via");
    let prefix = raw
        .split(|byte| *byte == b';')
        .next()
        .expect("Via has sent-protocol and sent-by");
    let branch = via.branch().expect("transport generated a branch");
    let observed = match observation {
        ReplyObservation::Learned(address) => format!(
            "{};received={};rport={};branch={}",
            String::from_utf8_lossy(prefix),
            address.ip(),
            address.port(),
            String::from_utf8_lossy(branch)
        ),
        ReplyObservation::Absent => format!(
            "{};branch={}",
            String::from_utf8_lossy(prefix),
            String::from_utf8_lossy(branch)
        ),
    };
    response.headers.remove_first(&HeaderName::Via);
    response.headers.push_front(
        Header::build(HeaderName::Via, Bytes::from(observed)).expect("replacement Via is valid"),
    );
    response
}

fn ok(incoming: &Incoming, observation: ReplyObservation) -> sipx_sip::Response {
    with_observation(
        &incoming.request,
        ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("response builds")
        .build(),
        observation,
    )
}

async fn server(
    transport: TransportKind,
    observation: ReplyObservation,
) -> (Target, tokio::task::JoinHandle<()>) {
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("valid address"),
    ))
    .await
    .expect("server binds");
    let target = Target::new(handle.local_addr(), transport);
    let task = tokio::spawn(async move {
        if let Some(request) = incoming.recv().await {
            let response = ok(&request, observation);
            handle
                .respond(&request.key, response)
                .await
                .expect("server responds");
        }
    });
    (target, task)
}

async fn agent(target: Target, credentials: Option<Credentials>) -> UserAgent {
    let (handle, _incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("valid address"),
    ))
    .await
    .expect("agent binds");
    let registrar = Uri::sip(Host::Name(
        HostName::new("registrar.example").expect("valid host"),
    ));
    let mut config = Config::new(
        "<sip:alice@example.test>",
        format!("<sip:alice@{}>", handle.local_addr()),
        registrar,
        target,
    );
    if let Some(credentials) = credentials {
        config = config.with_credentials(credentials);
    }
    UserAgent::new(handle, config)
}

#[tokio::test]
async fn udp_and_tcp_report_learned_and_absent_observations() {
    for transport in [TransportKind::Udp, TransportKind::Tcp] {
        for observation in [
            ReplyObservation::Learned(OBSERVED.parse().expect("address")),
            ReplyObservation::Absent,
        ] {
            let (target, served) = server(transport, observation).await;
            let mut ua = agent(target, None).await;
            let lease = tokio::time::timeout(Duration::from_secs(5), ua.register())
                .await
                .expect("a bound on registration")
                .expect("registration succeeds independently of observation");
            assert_eq!(lease.granted, Duration::from_secs(3600));
            let expected = match observation {
                ReplyObservation::Learned(address) => RegistrationObservation::Observed(address),
                ReplyObservation::Absent => RegistrationObservation::Absent,
            };
            assert_eq!(ua.registration_observation(), &expected);
            assert_eq!(ua.observed_registration_address(), expected.address());
            served.await.expect("server task completes");
        }
    }
}

#[tokio::test]
async fn success_replaces_and_failure_preserves_the_last_registration_observation() {
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("valid address"),
    ))
    .await
    .expect("server binds");
    let target = Target::udp(handle.local_addr());
    let served = tokio::spawn(async move {
        let first = incoming.recv().await.expect("initial REGISTER");
        handle
            .respond(
                &first.key,
                ok(
                    &first,
                    ReplyObservation::Learned(OBSERVED.parse().expect("address")),
                ),
            )
            .await
            .expect("initial success is sent");

        let refresh = incoming.recv().await.expect("refresh REGISTER");
        handle
            .respond(&refresh.key, ok(&refresh, ReplyObservation::Absent))
            .await
            .expect("refresh success is sent");

        let failed = incoming.recv().await.expect("failed REGISTER");
        let rejection = ResponseBuilder::to_request(
            &failed.request,
            StatusCode::new(403).expect("valid"),
            "Forbidden",
        )
        .expect("rejection builds")
        .build();
        let rejection = with_observation(
            &failed.request,
            rejection,
            ReplyObservation::Learned(CHALLENGE_OBSERVED.parse().expect("address")),
        );
        handle
            .respond(&failed.key, rejection)
            .await
            .expect("rejection is sent");
    });

    let mut ua = agent(target, None).await;
    assert_eq!(
        ua.registration_observation(),
        &RegistrationObservation::NotRegistered,
        "an agent with no successful REGISTER has no response Via to report"
    );

    ua.register().await.expect("initial registration succeeds");
    assert_eq!(
        ua.registration_observation(),
        &RegistrationObservation::Observed(OBSERVED.parse().expect("address"))
    );

    ua.register().await.expect("refresh succeeds");
    assert_eq!(
        ua.registration_observation(),
        &RegistrationObservation::Absent,
        "every successful refresh replaces the previous observation"
    );

    assert!(
        ua.register().await.is_err(),
        "the registrar rejects the attempt"
    );
    assert_eq!(
        ua.registration_observation(),
        &RegistrationObservation::Absent,
        "a failed attempt cannot replace the last successful observation"
    );
    served.await.expect("server task completes");
}

#[tokio::test]
async fn authentication_retains_only_the_final_success_observation() {
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("valid address"),
    ))
    .await
    .expect("server binds");
    let target = Target::udp(handle.local_addr());
    let served = tokio::spawn(async move {
        let first = incoming.recv().await.expect("first REGISTER");
        let challenge = ResponseBuilder::to_request(
            &first.request,
            StatusCode::new(401).expect("valid"),
            "Unauthorized",
        )
        .expect("challenge builds")
        .header(
            HeaderName::WwwAuthenticate,
            r#"Digest realm="example.test", nonce="one", qop="auth""#,
        )
        .expect("challenge header")
        .build();
        let challenge = with_observation(
            &first.request,
            challenge,
            ReplyObservation::Learned(CHALLENGE_OBSERVED.parse().expect("address")),
        );
        handle
            .respond(&first.key, challenge)
            .await
            .expect("challenge is sent");

        let final_request = incoming.recv().await.expect("authenticated REGISTER");
        assert!(
            final_request
                .request
                .headers
                .get(&HeaderName::Authorization)
                .is_some(),
            "the second request answers the challenge"
        );
        let final_response = ok(
            &final_request,
            ReplyObservation::Learned(OBSERVED.parse().expect("address")),
        );
        handle
            .respond(&final_request.key, final_response)
            .await
            .expect("final response is sent");
    });

    let mut ua = agent(target, Some(Credentials::new("alice", "secret"))).await;
    ua.register()
        .await
        .expect("authenticated registration succeeds");
    assert_eq!(
        ua.registration_observation(),
        &RegistrationObservation::Observed(OBSERVED.parse().expect("address"))
    );
    assert_ne!(
        ua.observed_registration_address(),
        Some(CHALLENGE_OBSERVED.parse().expect("address")),
        "a challenge observation is not the successful registration outcome"
    );
    served.await.expect("server task completes");
}

#[test]
fn invalid_observation_does_not_offer_a_fallback_address() {
    let observation = RegistrationObservation::Invalid(RegistrationObservationError::NonIpReceived);
    assert_eq!(observation.address(), None);
}
