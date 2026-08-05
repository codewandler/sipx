//! Conformance vectors for the sans-I/O RFC 3903 publisher.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::headers::CSeq;
use sipx_sip::{Header, HeaderName, Response, StatusCode, Uri};
use sipx_ua::Credentials;
use sipx_ua::event_client::{Peer, Transport};
use sipx_ua::publication_client::{
    Config, Output, Publisher, Start, StateChange, Termination, Timer,
};

fn peer() -> Peer {
    Peer::new("192.0.2.20:5060".parse().expect("peer"), Transport::Udp)
}

fn start(cseq: u32) -> Start {
    Start {
        resource: Uri::parse(Bytes::from_static(b"sip:alice@example.test")).expect("URI"),
        local_identity: "<sip:alice@example.test>".to_owned(),
        target: peer(),
        event: "presence".to_owned(),
        expires: Duration::from_secs(10),
        body: Bytes::from_static(b"<presence/>"),
        content_type: "application/pidf+xml".to_owned(),
        credentials: Some(Credentials::new("alice", "secret")),
        call_id: "publish-a@example.test".to_owned(),
        from_tag: "publisher-a".to_owned(),
        initial_cseq: cseq,
    }
}

fn response(status: u16, headers: &[(&HeaderName, &str)]) -> Response {
    let mut response = Response::new(StatusCode::new(status).expect("status"), "fixture");
    for (name, value) in headers {
        response.headers.push(
            Header::build((*name).clone(), Bytes::copy_from_slice(value.as_bytes()))
                .expect("header"),
        );
    }
    response
}

fn sent(outputs: &[Output]) -> &sipx_sip::Request {
    outputs
        .iter()
        .find_map(|output| match output {
            Output::SendPublish { request, .. } => Some(request.as_ref()),
            _ => None,
        })
        .expect("PUBLISH output")
}

fn sent_target(outputs: &[Output]) -> &Peer {
    outputs
        .iter()
        .find_map(|output| match output {
            Output::SendPublish { target, .. } => Some(target),
            _ => None,
        })
        .expect("PUBLISH target")
}

fn timer(outputs: &[Output], wanted: Timer) -> (u64, Duration) {
    outputs
        .iter()
        .rev()
        .find_map(|output| match output {
            Output::ArmTimer {
                timer,
                generation,
                after,
            } if *timer == wanted => Some((*generation, *after)),
            _ => None,
        })
        .expect("timer output")
}

fn terminated(outputs: &[Output], reason: &Termination) -> bool {
    outputs.iter().any(|output| {
        matches!(output, Output::StateChanged(StateChange::Terminated(actual)) if actual == reason)
    })
}

fn accepted(tag: &str, expires: u64) -> Response {
    response(
        200,
        &[
            (&HeaderName::SipETag, tag),
            (&HeaderName::Expires, &expires.to_string()),
        ],
    )
}

/// S39-V5.
#[test]
fn authenticated_publication_refreshes_modifies_and_removes() {
    let (mut publisher, initial) = Publisher::start(Config::default(), start(1)).expect("starts");
    assert_eq!(sent(&initial).body().as_ref(), b"<presence/>");
    let challenge = response(
        401,
        &[(
            &HeaderName::WwwAuthenticate,
            "Digest realm=\"example.test\", nonce=\"n1\", qop=\"auth\", algorithm=SHA-256",
        )],
    );
    let retry = publisher.response(Some(&challenge), "c1");
    assert_eq!(
        sent(&retry)
            .headers
            .typed::<CSeq>()
            .unwrap()
            .unwrap()
            .sequence,
        2
    );
    assert!(
        sent(&retry)
            .headers
            .value(&HeaderName::Authorization)
            .is_some()
    );

    let accepted_outputs = publisher.response(Some(&accepted("tag-a", 10)), "unused");
    assert_eq!(publisher.entity_tag(), Some("tag-a"));
    assert_eq!(publisher.granted_expiry(), Some(Duration::from_secs(10)));
    let (refresh_generation, refresh_after) = timer(&accepted_outputs, Timer::Refresh);
    assert_eq!(refresh_after, Duration::from_secs(8));
    let refresh = publisher.timer_fired(Timer::Refresh, refresh_generation);
    assert!(sent(&refresh).body().is_empty());
    assert_eq!(
        sent(&refresh)
            .headers
            .value(&HeaderName::SipIfMatch)
            .as_deref(),
        Some(&b"tag-a"[..])
    );
    let _ = publisher.response(Some(&accepted("tag-b", 9)), "unused");

    let modify = publisher
        .modify(
            Bytes::from_static(b"<presence><tuple/></presence>"),
            "application/pidf+xml".to_owned(),
        )
        .expect("modify");
    assert_eq!(
        sent(&modify)
            .headers
            .value(&HeaderName::SipIfMatch)
            .as_deref(),
        Some(&b"tag-b"[..])
    );
    assert!(!sent(&modify).body().is_empty());
    let _ = publisher.response(Some(&accepted("tag-c", 9)), "unused");

    let remove = publisher.remove().expect("remove");
    assert_eq!(
        sent(&remove).headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"0"[..])
    );
    assert_eq!(
        sent(&remove)
            .headers
            .value(&HeaderName::SipIfMatch)
            .as_deref(),
        Some(&b"tag-c"[..])
    );
    let removed = publisher.response(Some(&accepted("tag-d", 0)), "unused");
    assert!(terminated(&removed, &Termination::Removed));
    assert!(!publisher.is_active());
}

#[test]
fn secure_target_identity_and_resource_survive_every_publish() {
    let selected = Peer::new("192.0.2.20:7443".parse().expect("peer"), Transport::Wss)
        .verifying("compositor.example.test")
        .at_path("/publish");
    let mut start = start(1);
    start.target = selected.clone();
    let (mut publisher, initial) = Publisher::start(Config::default(), start).expect("starts");
    assert_eq!(sent_target(&initial), &selected);

    let challenge = response(
        401,
        &[(
            &HeaderName::WwwAuthenticate,
            "Digest realm=\"example.test\", nonce=\"n1\", qop=\"auth\", algorithm=SHA-256",
        )],
    );
    let retry = publisher.response(Some(&challenge), "c1");
    assert_eq!(sent_target(&retry), &selected);
    let accepted_outputs = publisher.response(Some(&accepted("tag-a", 10)), "unused");
    let (refresh_generation, _) = timer(&accepted_outputs, Timer::Refresh);
    let refresh = publisher.timer_fired(Timer::Refresh, refresh_generation);
    assert_eq!(sent_target(&refresh), &selected);
}

/// S39-V6.
#[test]
fn conditional_failure_discards_the_tag_without_retry() {
    let (mut publisher, _) = Publisher::start(Config::default(), start(1)).expect("starts");
    let accepted_outputs = publisher.response(Some(&accepted("tag-a", 10)), "unused");
    let (refresh, _) = timer(&accepted_outputs, Timer::Refresh);
    let _ = publisher.timer_fired(Timer::Refresh, refresh);
    let stale = publisher.response(Some(&response(412, &[])), "unused");
    assert!(terminated(&stale, &Termination::StaleTag));
    assert!(
        !stale
            .iter()
            .any(|output| matches!(output, Output::SendPublish { .. }))
    );
    assert_eq!(publisher.entity_tag(), None);
}

/// S39-V3 and S39-V7.
#[test]
fn interval_retry_is_bounded_and_success_authority_is_exact() {
    let config = Config {
        maximum_expiry: Duration::from_secs(30),
        ..Config::default()
    };
    let (mut publisher, _) = Publisher::start(config, start(1)).expect("starts");
    let brief = response(423, &[(&HeaderName::MinExpires, "20")]);
    let retry = publisher.response(Some(&brief), "unused");
    assert_eq!(
        sent(&retry).headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"20"[..])
    );
    assert_eq!(
        sent(&retry)
            .headers
            .typed::<CSeq>()
            .unwrap()
            .unwrap()
            .sequence,
        2
    );
    let _ = publisher.response(Some(&accepted("tag-a", 20)), "unused");
    assert_eq!(publisher.granted_expiry(), Some(Duration::from_secs(20)));

    let (mut malformed, _) = Publisher::start(Config::default(), start(1)).expect("starts");
    let missing_tag = response(200, &[(&HeaderName::Expires, "10")]);
    let ended = malformed.response(Some(&missing_tag), "unused");
    assert!(terminated(&ended, &Termination::MalformedResponse));
}

/// S39-V7.
#[test]
fn cseq_exhaustion_emits_no_conditional_request() {
    let (mut publisher, _) = Publisher::start(Config::default(), start(u32::MAX)).expect("starts");
    let accepted_outputs = publisher.response(Some(&accepted("tag-a", 10)), "unused");
    let (refresh, _) = timer(&accepted_outputs, Timer::Refresh);
    let ended = publisher.timer_fired(Timer::Refresh, refresh);
    assert!(terminated(&ended, &Termination::LocalCSeqExhausted));
    assert!(
        !ended
            .iter()
            .any(|output| matches!(output, Output::SendPublish { .. }))
    );
}
