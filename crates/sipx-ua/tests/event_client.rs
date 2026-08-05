//! Public conformance tests derived from `docs/specs/event-client.md` S37-V1 through S37-V12.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::CSeq;
use sipx_sip::{Header, HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_ua::Credentials;
use sipx_ua::event_client::{
    Config, EventClient, Lifecycle, NotifyTrustPolicy, Output, PackageConsumer, PackageRejection,
    Peer, SamePeer, Start, StateChange, SubscriptionId, Termination, Timer, Transport,
};

const CALL_ID: &str = "sub-a@example.test";
const LOCAL_TAG: &str = "sub-a";
const REMOTE_TAG: &str = "notifier-a";

#[derive(Debug)]
struct TestPackage;

impl PackageConsumer for TestPackage {
    type Value = String;

    fn event(&self) -> &'static str {
        "test-state"
    }

    fn event_id(&self) -> Option<&str> {
        Some("alpha")
    }

    fn accept(&self) -> &[String] {
        static ACCEPT: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
        ACCEPT.get_or_init(|| vec!["application/test-state".to_owned()])
    }

    fn neutral(&mut self) -> Option<Self::Value> {
        Some("neutral".to_owned())
    }

    fn consume(
        &mut self,
        content_type: Option<&[u8]>,
        body: &[u8],
    ) -> Result<Self::Value, PackageRejection> {
        if content_type != Some(&b"application/test-state"[..]) && !body.is_empty() {
            return Err(PackageRejection::unsupported_media());
        }
        std::str::from_utf8(body)
            .map(str::to_owned)
            .map_err(|_| PackageRejection::malformed())
    }
}

fn peer(port: u16) -> Peer {
    Peer {
        address: SocketAddr::from(([192, 0, 2, 20], port)),
        transport: Transport::Udp,
        connection: None,
    }
}

fn start_with(cseq: u32, expires: u64, trust: Arc<dyn NotifyTrustPolicy>) -> Start<TestPackage> {
    Start {
        resource: Uri::parse(Bytes::from_static(b"sip:resource@example.test")).expect("URI"),
        local_identity: "<sip:client@example.test>".to_owned(),
        contact: "<sip:client@192.0.2.10:5060>".to_owned(),
        target: peer(5060),
        expires: Duration::from_secs(expires),
        body: Bytes::new(),
        content_type: None,
        credentials: Some(Credentials::new("client", "secret")),
        call_id: CALL_ID.to_owned(),
        from_tag: LOCAL_TAG.to_owned(),
        initial_cseq: cseq,
        consumer: TestPackage,
        trust,
    }
}

fn start(client: &mut EventClient<TestPackage>) -> (SubscriptionId, Vec<Output<String>>) {
    client
        .start(start_with(1, 3_600, Arc::new(SamePeer)))
        .expect("starts")
}

fn response(status: u16, tag: Option<&str>, headers: &[(&HeaderName, &str)]) -> Response {
    let mut response = Response::new(StatusCode::new(status).expect("status"), "fixture");
    if let Some(tag) = tag {
        response.headers.push(
            Header::build(
                HeaderName::To,
                Bytes::from(format!("<sip:resource@example.test>;tag={tag}")),
            )
            .expect("To"),
        );
    }
    for (name, value) in headers {
        response.headers.push(
            Header::build((*name).clone(), Bytes::copy_from_slice(value.as_bytes()))
                .expect("header"),
        );
    }
    response
}

fn notify(
    remote_tag: &str,
    cseq: u32,
    event: &str,
    state: &str,
    contacts: &[&str],
    body: &'static [u8],
) -> Request {
    let mut builder = RequestBuilder::new(
        Method::Notify,
        Uri::parse(Bytes::from_static(b"sip:client@192.0.2.10:5060")).expect("URI"),
    )
    .header(
        HeaderName::From,
        Bytes::from(format!("<sip:resource@example.test>;tag={remote_tag}")),
    )
    .expect("From")
    .header(
        HeaderName::To,
        Bytes::from(format!("<sip:client@example.test>;tag={LOCAL_TAG}")),
    )
    .expect("To")
    .header(HeaderName::CallId, CALL_ID)
    .expect("Call-ID")
    .cseq(cseq, &Method::Notify)
    .expect("CSeq")
    .header(HeaderName::Event, Bytes::copy_from_slice(event.as_bytes()))
    .expect("Event")
    .header(
        HeaderName::SubscriptionState,
        Bytes::copy_from_slice(state.as_bytes()),
    )
    .expect("Subscription-State")
    .max_forwards(70);
    for contact in contacts {
        builder = builder
            .header(
                HeaderName::Contact,
                Bytes::copy_from_slice(contact.as_bytes()),
            )
            .expect("Contact");
    }
    if !body.is_empty() {
        builder = builder
            .header(HeaderName::ContentType, "application/test-state")
            .expect("Content-Type");
    }
    builder.body(Bytes::from_static(body)).build()
}

fn sent(outputs: &[Output<String>]) -> &Request {
    outputs
        .iter()
        .find_map(|output| match output {
            Output::SendSubscribe { request, .. } => Some(request.as_ref()),
            _ => None,
        })
        .expect("SUBSCRIBE output")
}

fn status(outputs: &[Output<String>]) -> u16 {
    outputs
        .iter()
        .find_map(|output| match output {
            Output::RespondNotify { status, .. } => Some(*status),
            _ => None,
        })
        .expect("NOTIFY response")
}

fn timer(outputs: &[Output<String>], kind: Timer) -> (u64, Duration) {
    outputs
        .iter()
        .rev()
        .find_map(|output| match output {
            Output::ArmTimer {
                timer,
                generation,
                after,
                ..
            } if *timer == kind => Some((*generation, *after)),
            _ => None,
        })
        .expect("timer output")
}

fn has_change(outputs: &[Output<String>], wanted: &StateChange) -> bool {
    outputs
        .iter()
        .any(|output| matches!(output, Output::StateChanged { change, .. } if change == wanted))
}

fn establish(client: &mut EventClient<TestPackage>) -> (SubscriptionId, Vec<Output<String>>) {
    let (id, initial) = start(client);
    client.consumer_drained(id, 1);
    assert_eq!(sent(&initial).method, Method::Subscribe);
    let accepted = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "1800")]);
    let _ = client.response(id, Some(&accepted), "unused");
    let outputs = client.notify(
        1,
        &notify(
            REMOTE_TAG,
            40,
            "test-state;id=alpha",
            "active;expires=1800",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"state=one",
        ),
        peer(5060),
    );
    (id, outputs)
}

/// S37-V1.
#[test]
fn authenticated_subscription_establishes_from_notify() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, first) = start(&mut client);
    client.consumer_drained(id, 1);
    let initial = sent(&first);
    assert_eq!(
        initial.headers.typed::<CSeq>().unwrap().unwrap().sequence,
        1
    );
    let challenge = response(
        401,
        None,
        &[(
            &HeaderName::WwwAuthenticate,
            "Digest realm=\"example.test\", nonce=\"n1\", qop=\"auth\", algorithm=SHA-256",
        )],
    );
    let retry = client.response(id, Some(&challenge), "c1");
    let retry = sent(&retry);
    assert_eq!(retry.headers.typed::<CSeq>().unwrap().unwrap().sequence, 2);
    assert!(retry.headers.value(&HeaderName::Authorization).is_some());
    let accepted = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "1800")]);
    let _ = client.response(id, Some(&accepted), "unused");
    let outputs = client.notify(
        10,
        &notify(
            REMOTE_TAG,
            40,
            "test-state;id=alpha",
            "active;expires=1800",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"state=one",
        ),
        peer(5060),
    );
    assert_eq!(status(&outputs), 200);
    assert!(has_change(&outputs, &StateChange::State(Lifecycle::Active)));
    assert!(outputs.iter().any(|output| matches!(output, Output::Deliver { value, metadata: Some(meta), .. } if value == "state=one" && meta.remote_cseq == 40)));
    assert_eq!(timer(&outputs, Timer::Expiry).1, Duration::from_secs(1800));
    assert_eq!(timer(&outputs, Timer::Refresh).1, Duration::from_secs(1440));
}

/// S37-V2.
#[test]
fn notify_before_response_selects_one_dialog() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = start(&mut client);
    client.consumer_drained(id, 1);
    let selected = client.notify(
        1,
        &notify(
            REMOTE_TAG,
            40,
            "test-state;id=alpha",
            "active;expires=900",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"one",
        ),
        peer(5060),
    );
    assert_eq!(status(&selected), 200);
    let fork = client.notify(
        2,
        &notify(
            "notifier-b",
            1,
            "test-state;id=alpha",
            "active;expires=900",
            &["<sip:fork@192.0.2.30:5060>"],
            b"fork",
        ),
        peer(5060),
    );
    assert_eq!(status(&fork), 481);
    let late = response(200, Some("notifier-b"), &[(&HeaderName::Expires, "800")]);
    let outputs = client.response(id, Some(&late), "unused");
    assert!(!outputs.iter().any(|output| matches!(
        output,
        Output::ArmTimer {
            timer: Timer::Refresh,
            ..
        }
    )));
    assert_eq!(client.active(), 1);
}

/// S37-V3.
#[test]
fn notify_expiry_overrides_refresh_response() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, established) = establish(&mut client);
    client.consumer_drained(id, 1);
    let (refresh_generation, _) = timer(&established, Timer::Refresh);
    let refresh = client.timer_fired(id, Timer::Refresh, refresh_generation);
    let request = sent(&refresh);
    assert_eq!(
        request.headers.typed::<CSeq>().unwrap().unwrap().sequence,
        2
    );
    assert_eq!(
        request.headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"3600"[..])
    );
    let response = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "1200")]);
    let response_outputs = client.response(id, Some(&response), "unused");
    assert_eq!(
        timer(&response_outputs, Timer::Expiry).1,
        Duration::from_secs(1200)
    );
    let notify_outputs = client.notify(
        2,
        &notify(
            REMOTE_TAG,
            41,
            "test-state;id=alpha",
            "active;expires=900",
            &["<sip:moved@192.0.2.20:5070>"],
            b"two",
        ),
        peer(5060),
    );
    assert_eq!(
        timer(&notify_outputs, Timer::Expiry).1,
        Duration::from_secs(900)
    );
    assert_eq!(
        timer(&notify_outputs, Timer::Refresh).1,
        Duration::from_secs(720)
    );
}

/// S37-V4.
#[test]
fn local_expiry_releases_everything() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, established) = establish(&mut client);
    let (generation, _) = timer(&established, Timer::Expiry);
    let ended = client.timer_fired(id, Timer::Expiry, generation);
    assert!(has_change(
        &ended,
        &StateChange::Terminated(Termination::LocalExpiry)
    ));
    assert!(!client.contains(id));
    let late = client.notify(
        3,
        &notify(
            REMOTE_TAG,
            41,
            "test-state;id=alpha",
            "active",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"late",
        ),
        peer(5060),
    );
    assert_eq!(status(&late), 481);
}

/// S37-V5.
#[test]
fn unsubscribe_waits_for_terminal_notify() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = establish(&mut client);
    let outputs = client.unsubscribe(id);
    let request = sent(&outputs);
    assert_eq!(
        request.headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"0"[..])
    );
    let accepted = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "0")]);
    let response_outputs = client.response(id, Some(&accepted), "unused");
    assert!(client.contains(id));
    assert!(!response_outputs.iter().any(|output| matches!(
        output,
        Output::StateChanged {
            change: StateChange::Terminated(_),
            ..
        }
    )));
    let terminal = client.notify(
        4,
        &notify(
            REMOTE_TAG,
            41,
            "test-state;id=alpha",
            "terminated;reason=timeout",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"",
        ),
        peer(5060),
    );
    assert_eq!(status(&terminal), 200);
    assert!(has_change(
        &terminal,
        &StateChange::Terminated(Termination::Remote(Some(sipx_sip::event::Reason::Timeout)))
    ));
    assert!(!client.contains(id));
}

/// S37-V6.
#[test]
fn stale_notify_is_refused_without_delivery() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = establish(&mut client);
    client.consumer_drained(id, 1);
    let stale = client.notify(
        5,
        &notify(
            REMOTE_TAG,
            40,
            "test-state;id=alpha",
            "active;expires=30",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"stale",
        ),
        peer(5060),
    );
    assert_eq!(status(&stale), 500);
    assert!(
        !stale
            .iter()
            .any(|output| matches!(output, Output::Deliver { .. }))
    );
}

/// S37-V7.
#[test]
fn unsupported_event_is_489() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = establish(&mut client);
    client.consumer_drained(id, 1);
    let refused = client.notify(
        6,
        &notify(
            REMOTE_TAG,
            41,
            "other-state;id=alpha",
            "active",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"other",
        ),
        peer(5060),
    );
    assert_eq!(status(&refused), 489);
    assert_eq!(client.active(), 1);
}

/// S37-V8.
#[test]
fn shutdown_cancels_a_due_refresh_and_drains() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, established) = establish(&mut client);
    let (old_refresh, _) = timer(&established, Timer::Refresh);
    let shutting_down = client.shutdown();
    assert_eq!(
        sent(&shutting_down)
            .headers
            .value(&HeaderName::Expires)
            .as_deref(),
        Some(&b"0"[..])
    );
    assert!(
        client
            .timer_fired(id, Timer::Refresh, old_refresh)
            .is_empty()
    );
    let stopped = client.shutdown_deadline();
    assert!(
        stopped
            .iter()
            .any(|output| matches!(output, Output::Stopped))
    );
    assert_eq!(client.active(), 0);
}

/// S37-V9.
#[test]
fn expiryless_notify_retains_a_finite_provisional_bound() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, initial) = client
        .start(start_with(1, 300, Arc::new(SamePeer)))
        .expect("starts");
    client.consumer_drained(id, 1);
    let (expiry_generation, expiry) = timer(&initial, Timer::Expiry);
    assert_eq!(expiry, Duration::from_secs(300));
    let accepted = client.notify(
        7,
        &notify(
            REMOTE_TAG,
            1,
            "test-state;id=alpha",
            "active",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"",
        ),
        peer(5060),
    );
    assert_eq!(status(&accepted), 200);
    let conflict = client.response(id, None, "unused");
    assert!(has_change(
        &conflict,
        &StateChange::ConflictingSubscribeResponse
    ));
    let ended = client.timer_fired(id, Timer::Expiry, expiry_generation);
    assert!(has_change(
        &ended,
        &StateChange::Terminated(Termination::LocalExpiry)
    ));
}

/// S37-V10.
#[test]
fn local_cseq_exhaustion_terminates_without_a_send() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = client
        .start(start_with(u32::MAX, 300, Arc::new(SamePeer)))
        .expect("starts");
    client.consumer_drained(id, 1);
    let accepted = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "300")]);
    let _ = client.response(id, Some(&accepted), "unused");
    let active = client.notify(
        8,
        &notify(
            REMOTE_TAG,
            1,
            "test-state;id=alpha",
            "active;expires=300",
            &["<sip:notifier@192.0.2.20:5060>"],
            b"",
        ),
        peer(5060),
    );
    let (refresh, _) = timer(&active, Timer::Refresh);
    let ended = client.timer_fired(id, Timer::Refresh, refresh);
    assert!(has_change(
        &ended,
        &StateChange::Terminated(Termination::LocalCSeqExhausted)
    ));
    assert!(
        !ended
            .iter()
            .any(|output| matches!(output, Output::SendSubscribe { .. }))
    );
}

/// S37-V11.
#[test]
fn response_intervals_fail_closed_for_every_operation() {
    for headers in [
        vec![],
        vec![(&HeaderName::Expires, "4294967296")],
        vec![(&HeaderName::Expires, "301")],
    ] {
        let mut client = EventClient::new(Config::default()).expect("config");
        let (id, _) = client
            .start(start_with(1, 300, Arc::new(SamePeer)))
            .expect("starts");
        let invalid = response(200, Some(REMOTE_TAG), &headers);
        let outputs = client.response(id, Some(&invalid), "unused");
        assert!(has_change(
            &outputs,
            &StateChange::Terminated(Termination::InvalidExpiry)
        ));
        assert!(!client.contains(id));
    }
    let config = Config {
        maximum_expiry: Duration::from_secs(300),
        ..Config::default()
    };
    let mut client = EventClient::new(config).expect("config");
    let (id, _) = client
        .start(start_with(1, 300, Arc::new(SamePeer)))
        .expect("starts");
    let too_large = response(423, None, &[(&HeaderName::MinExpires, "301")]);
    let outputs = client.response(id, Some(&too_large), "unused");
    assert!(has_change(
        &outputs,
        &StateChange::Terminated(Termination::IntervalRejected)
    ));
}

/// S37-V12.
#[test]
fn notify_trust_and_contact_rejections_do_not_mutate() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = start(&mut client);
    client.consumer_drained(id, 1);
    let valid = notify(
        REMOTE_TAG,
        1,
        "test-state;id=alpha",
        "active;expires=300",
        &["<sip:notifier@192.0.2.20:5060>"],
        b"ok",
    );
    assert_eq!(status(&client.notify(9, &valid, peer(5099))), 403);
    let missing = notify(
        REMOTE_TAG,
        1,
        "test-state;id=alpha",
        "active;expires=300",
        &[],
        b"ok",
    );
    assert_eq!(status(&client.notify(10, &missing, peer(5060))), 400);
    let duplicate = notify(
        REMOTE_TAG,
        1,
        "test-state;id=alpha",
        "active;expires=300",
        &[
            "<sip:notifier@192.0.2.20:5060>",
            "<sip:other@192.0.2.21:5060>",
        ],
        b"ok",
    );
    assert_eq!(status(&client.notify(11, &duplicate, peer(5060))), 400);
    let accepted = client.notify(12, &valid, peer(5060));
    assert_eq!(status(&accepted), 200);
    assert!(has_change(
        &accepted,
        &StateChange::State(Lifecycle::Active)
    ));
}

/// S37-V13.
#[test]
fn refresh_timer_n_preserves_only_the_authoritative_expiry() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, established) = establish(&mut client);
    let (expiry_generation, expiry_duration) = timer(&established, Timer::Expiry);
    let (refresh_generation, _) = timer(&established, Timer::Refresh);
    let refresh = client.timer_fired(id, Timer::Refresh, refresh_generation);
    let (n_generation, _) = timer(&refresh, Timer::N);
    let timed_out = client.timer_fired(id, Timer::N, n_generation);
    assert!(has_change(&timed_out, &StateChange::RefreshUnconfirmed));
    assert!(!timed_out.iter().any(|output| matches!(
        output,
        Output::SendSubscribe { .. }
            | Output::ArmTimer {
                timer: Timer::Refresh,
                ..
            }
    )));
    assert!(client.contains(id));
    let ended = client.timer_fired(id, Timer::Expiry, expiry_generation);
    assert_eq!(expiry_duration, Duration::from_secs(1800));
    assert!(has_change(
        &ended,
        &StateChange::Terminated(Termination::LocalExpiry)
    ));
    assert!(!client.contains(id));
}

#[test]
fn unsubscribe_waits_behind_an_in_flight_refresh() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, established) = establish(&mut client);
    let (refresh_generation, _) = timer(&established, Timer::Refresh);
    let refresh = client.timer_fired(id, Timer::Refresh, refresh_generation);
    assert_eq!(
        sent(&refresh)
            .headers
            .typed::<CSeq>()
            .unwrap()
            .unwrap()
            .sequence,
        2
    );

    let queued = client.unsubscribe(id);
    assert!(
        !queued
            .iter()
            .any(|output| matches!(output, Output::SendSubscribe { .. }))
    );

    let accepted = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "1200")]);
    let drained = client.response(id, Some(&accepted), "unused");
    let unsubscribe = sent(&drained);
    assert_eq!(
        unsubscribe
            .headers
            .typed::<CSeq>()
            .unwrap()
            .unwrap()
            .sequence,
        3
    );
    assert_eq!(
        unsubscribe.headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"0"[..])
    );
}

#[test]
fn strict_route_set_rewrites_the_in_dialog_request_target() {
    let mut client = EventClient::new(Config::default()).expect("config");
    let (id, _) = start(&mut client);
    client.consumer_drained(id, 1);
    let accepted = response(200, Some(REMOTE_TAG), &[(&HeaderName::Expires, "300")]);
    let _ = client.response(id, Some(&accepted), "unused");
    let mut establishing = notify(
        REMOTE_TAG,
        1,
        "test-state;id=alpha",
        "active;expires=300",
        &["<sip:notifier@192.0.2.20:5060>"],
        b"one",
    );
    establishing.headers.push(
        Header::build(
            HeaderName::RecordRoute,
            "<sip:strict@192.0.2.40:5060>, <sip:loose@192.0.2.41:5060;lr>",
        )
        .expect("Record-Route"),
    );
    let established = client.notify(20, &establishing, peer(5060));
    let (refresh_generation, _) = timer(&established, Timer::Refresh);
    let refresh = client.timer_fired(id, Timer::Refresh, refresh_generation);
    let request = sent(&refresh);
    assert_eq!(
        request.uri.to_bytes().as_ref(),
        b"sip:strict@192.0.2.40:5060"
    );
    let routes: Vec<_> = request
        .headers
        .get_all(&HeaderName::Route)
        .map(|header| header.value().into_owned())
        .collect();
    assert_eq!(
        routes,
        vec![
            b"<sip:loose@192.0.2.41:5060;lr>".to_vec(),
            b"<sip:notifier@192.0.2.20:5060>".to_vec(),
        ]
    );
}
