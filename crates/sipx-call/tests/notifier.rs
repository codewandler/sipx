//! A SUBSCRIBE entering through a real endpoint reaches the RFC 6665 notifier (`S-35`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{Dispatcher, Notifier, NotifierHandle};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::headers::To;
use sipx_sip::{Header, HeaderName, Limits, Message, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, TransportKind, bind};
use sipx_ua::subscribe::Subscriptions;
use tokio::net::UdpSocket;
use tokio::sync::mpsc::Receiver;
use tokio::task::JoinHandle;

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn subscribe(
    peer: &Handle,
    call_id: &str,
    from_tag: &str,
    to_tag: Option<&str>,
    event: &str,
    expires: u64,
    cseq: u32,
) -> Request {
    let to = to_tag.map_or_else(
        || "<sip:alice@sipx.test>".to_owned(),
        |tag| format!("<sip:alice@sipx.test>;tag={tag}"),
    );
    RequestBuilder::new(
        Method::Subscribe,
        Uri::parse(Bytes::from_static(b"sip:alice@sipx.test")).expect("URI"),
    )
    .header(
        HeaderName::Via,
        Bytes::from(format!(
            "SIP/2.0/UDP {};rport;branch={}",
            peer.sent_by_for(TransportKind::Udp),
            sipx_transport::new_branch()
        )),
    )
    .expect("Via")
    .header(HeaderName::To, Bytes::from(to))
    .expect("To")
    .header(
        HeaderName::From,
        Bytes::from(format!("<sip:watcher@sipx.test>;tag={from_tag}")),
    )
    .expect("From")
    .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
    .expect("Call-ID")
    .cseq(cseq, &Method::Subscribe)
    .expect("CSeq")
    .header(
        HeaderName::Contact,
        Bytes::from(format!("<sip:watcher@{}>", peer.local_addr())),
    )
    .expect("Contact")
    .header(HeaderName::Event, Bytes::from(event.to_owned()))
    .expect("Event")
    .header(HeaderName::Expires, Bytes::from(expires.to_string()))
    .expect("Expires")
    .max_forwards(70)
    .body(Bytes::new())
    .build()
}

fn pump(endpoint: &Handle, incoming: Receiver<Incoming>, notifier: Notifier) -> JoinHandle<()> {
    let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming).with_notifier(notifier);
    tokio::spawn(async move { while dispatcher.next().await.is_some() {} })
}

async fn final_response(peer: &Handle, destination: SocketAddr, request: Request) -> Response {
    let mut responses = peer
        .send(request, Target::udp(destination))
        .await
        .expect("request starts");
    tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("response is bounded")
        .expect("a final response")
}

async fn next_notify(incoming: &mut Receiver<Incoming>) -> Incoming {
    let incoming = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("NOTIFY is immediate")
        .expect("peer endpoint remains open");
    assert_eq!(incoming.request.method, Method::Notify);
    incoming
}

async fn answer_notify(endpoint: &Handle, incoming: &Incoming) {
    let response = ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(200).expect("status"),
        "OK",
    )
    .expect("response builds")
    .build();
    endpoint
        .respond(&incoming.key, response)
        .await
        .expect("NOTIFY response sends");
}

fn replace_header(mut request: Request, name: HeaderName, value: &'static [u8]) -> Request {
    request.headers.remove_all(&name);
    request
        .headers
        .push(Header::build(name, Bytes::from_static(value)).expect("syntactic header"));
    request
}

async fn raw_final_response(
    socket: &UdpSocket,
    destination: SocketAddr,
    mut request: Request,
) -> Response {
    request.headers.remove_all(&HeaderName::Via);
    request.headers.push(
        Header::build(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                socket.local_addr().expect("raw socket address"),
                sipx_transport::new_branch()
            )),
        )
        .expect("Via"),
    );
    let bytes = Message::Request(request).to_bytes();
    socket
        .send_to(&bytes, destination)
        .await
        .expect("request sends");
    let mut received = vec![0; 65_535];
    let length = tokio::time::timeout(Duration::from_secs(2), socket.recv(&mut received))
        .await
        .expect("response is bounded")
        .expect("response arrives");
    received.truncate(length);
    match sipx_sip::parse_datagram(Bytes::from(received), &Limits::default())
        .expect("response parses")
    {
        Message::Response(response) => response,
        Message::Request(_) => panic!("expected a response"),
    }
}

fn local_tag(response: &Response) -> String {
    let to = response
        .headers
        .typed::<To>()
        .expect("To exists")
        .expect("To parses");
    String::from_utf8(to.tag().expect("response has local tag").to_vec()).expect("ASCII tag")
}

fn shared_store(handle: &NotifierHandle) -> Arc<Mutex<Subscriptions>> {
    handle.subscriptions()
}

/// The primary failing-first path: the socket driver mutates the public handle's exact store and
/// originates the first NOTIFY without another application action.
#[tokio::test]
async fn socket_subscribe_uses_the_shared_store_and_immediately_notifies() {
    let (watcher, mut watcher_incoming) = endpoint().await;
    let (notifier_endpoint, notifier_incoming) = endpoint().await;
    let notifier = Notifier::new(Duration::from_secs(30), 4);
    let handle = notifier.handle();
    let first_view = shared_store(&handle);
    let second_view = shared_store(&handle);
    assert!(Arc::ptr_eq(&first_view, &second_view));
    let pump = pump(&notifier_endpoint, notifier_incoming, notifier);

    for (index, (event, content_type, body_marker)) in [
        ("dialog", "application/dialog-info+xml", "state=\"full\""),
        ("reg", "application/reginfo+xml", "state=\"full\""),
        ("presence", "application/pidf+xml", "<presence "),
    ]
    .into_iter()
    .enumerate()
    {
        let response = final_response(
            &watcher,
            notifier_endpoint.local_addr(),
            subscribe(
                &watcher,
                &format!("same-store-{index}"),
                &format!("watcher-{index}"),
                None,
                event,
                300,
                1,
            ),
        )
        .await;
        assert_eq!(response.status.code(), 200);
        assert!(response.headers.value(&HeaderName::Contact).is_some());
        assert_eq!(
            response.headers.value(&HeaderName::Expires).as_deref(),
            Some(&b"30"[..])
        );

        let notify = next_notify(&mut watcher_incoming).await;
        assert_eq!(
            notify
                .request
                .headers
                .value(&HeaderName::SubscriptionState)
                .as_deref(),
            Some(&b"active;expires=30"[..])
        );
        assert_eq!(
            notify
                .request
                .headers
                .value(&HeaderName::ContentType)
                .as_deref(),
            Some(content_type.as_bytes())
        );
        assert!(String::from_utf8_lossy(notify.request.body()).contains(body_marker));
        answer_notify(&watcher, &notify).await;
    }
    assert_eq!(first_view.lock().expect("store lock").active(), 3);
    pump.abort();
}

#[tokio::test]
async fn refusals_expiry_negotiation_and_capacity_shed_are_visible() {
    let (watcher, mut watcher_incoming) = endpoint().await;
    let (notifier_endpoint, notifier_incoming) = endpoint().await;
    let notifier = Notifier::new(Duration::from_secs(20), 1);
    let handle = notifier.handle();
    let pump = pump(&notifier_endpoint, notifier_incoming, notifier);

    let bad_event = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "bad-event", "w1", None, "unknown", 20, 1),
    )
    .await;
    assert_eq!(bad_event.status.code(), 489);

    let unknown_dialog = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(
            &watcher,
            "missing-dialog",
            "w2",
            Some("not-here"),
            "dialog",
            20,
            2,
        ),
    )
    .await;
    assert_eq!(unknown_dialog.status.code(), 481);

    let accepted = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "capacity-one", "w3", None, "presence", 200, 1),
    )
    .await;
    assert_eq!(accepted.status.code(), 200);
    assert_eq!(
        accepted.headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"20"[..])
    );
    let initial = next_notify(&mut watcher_incoming).await;
    answer_notify(&watcher, &initial).await;

    let shed = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "over-capacity", "w4", None, "reg", 20, 1),
    )
    .await;
    assert_eq!(shed.status.code(), 503);
    assert_eq!(
        shed.headers.value(&HeaderName::RetryAfter).as_deref(),
        Some(&b"5"[..])
    );
    assert_eq!(handle.counts().shed, 1);
    assert_eq!(handle.counts().active_tasks, 1);
    assert_eq!(shared_store(&handle).lock().expect("store").active(), 1);
    pump.abort();
}

#[tokio::test]
async fn unsubscribe_observably_stops_the_owned_timer_task() {
    let (watcher, mut watcher_incoming) = endpoint().await;
    let (notifier_endpoint, notifier_incoming) = endpoint().await;
    let notifier = Notifier::new(Duration::from_secs(60), 1);
    let handle = notifier.handle();
    let pump = pump(&notifier_endpoint, notifier_incoming, notifier);

    let accepted = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "terminates", "w5", None, "reg", 60, 1),
    )
    .await;
    let tag = local_tag(&accepted);
    let initial = next_notify(&mut watcher_incoming).await;
    answer_notify(&watcher, &initial).await;
    assert_eq!(handle.counts().active_tasks, 1);

    let ended = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "terminates", "w5", Some(&tag), "reg", 0, 2),
    )
    .await;
    assert_eq!(ended.status.code(), 200);
    let terminal = next_notify(&mut watcher_incoming).await;
    assert_eq!(
        terminal
            .request
            .headers
            .value(&HeaderName::SubscriptionState)
            .as_deref(),
        Some(&b"terminated;reason=deactivated"[..])
    );
    answer_notify(&watcher, &terminal).await;

    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.counts().active_tasks != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the lifecycle task exits");
    let counts = handle.counts();
    assert_eq!(counts.started_tasks, 1);
    assert_eq!(counts.finished_tasks, 1);
    assert_eq!(shared_store(&handle).lock().expect("store").active(), 0);
    pump.abort();
}

#[tokio::test(start_paused = true)]
async fn expiry_sends_timeout_notify_and_releases_the_timer_task() {
    let (watcher, mut watcher_incoming) = endpoint().await;
    let (notifier_endpoint, notifier_incoming) = endpoint().await;
    let notifier = Notifier::new(Duration::from_secs(5), 1);
    let handle = notifier.handle();
    let pump = pump(&notifier_endpoint, notifier_incoming, notifier);

    let accepted = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "expires", "w6", None, "presence", 5, 1),
    )
    .await;
    assert_eq!(accepted.status.code(), 200);
    let initial = next_notify(&mut watcher_incoming).await;
    answer_notify(&watcher, &initial).await;
    tokio::task::yield_now().await;
    assert_eq!(handle.counts().active_tasks, 1);

    tokio::time::advance(Duration::from_secs(5)).await;
    let terminal = next_notify(&mut watcher_incoming).await;
    assert_eq!(
        terminal
            .request
            .headers
            .value(&HeaderName::SubscriptionState)
            .as_deref(),
        Some(&b"terminated;reason=timeout"[..])
    );
    answer_notify(&watcher, &terminal).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while handle.counts().active_tasks != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the expired lifecycle task exits");
    assert_eq!(handle.counts().finished_tasks, 1);
    let store = shared_store(&handle);
    let store = store.lock().expect("store");
    assert_eq!(store.active(), 0);
    assert!(store.all().is_empty());
    pump.abort();
}

#[tokio::test(start_paused = true)]
async fn silent_notify_peer_is_bounded_and_every_transaction_eventually_drains() {
    let (watcher, mut watcher_incoming) = endpoint().await;
    let (notifier_endpoint, notifier_incoming) = endpoint().await;
    let notifier = Notifier::new(Duration::from_secs(5), 1);
    let handle = notifier.handle();
    let pump = pump(&notifier_endpoint, notifier_incoming, notifier);

    let accepted = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "silent", "w-silent", None, "dialog", 5, 1),
    )
    .await;
    assert_eq!(accepted.status.code(), 200);
    let initial = next_notify(&mut watcher_incoming).await;
    assert_eq!(initial.request.method, Method::Notify);

    // Protocol/application failure bound: the notifier stops awaiting this unanswered NOTIFY.
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(handle.counts().active_tasks, 1);
    assert_eq!(shared_store(&handle).lock().expect("store").active(), 1);

    // Protocol expiry: the original five-second lease, not the unanswered send, ends the usage.
    tokio::time::advance(Duration::from_secs(3)).await;
    let terminal = next_notify(&mut watcher_incoming).await;
    assert_eq!(
        terminal
            .request
            .headers
            .value(&HeaderName::SubscriptionState)
            .as_deref(),
        Some(&b"terminated;reason=timeout"[..])
    );

    // Protocol/application failure bound: the terminal send is bounded independently as well.
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert_eq!(handle.counts().active_tasks, 0);
    assert_eq!(handle.counts().finished_tasks, 1);
    assert!(
        shared_store(&handle)
            .lock()
            .expect("store")
            .all()
            .is_empty()
    );
    assert!(
        notifier_endpoint.outstanding().await.expect("endpoint") > 0,
        "the endpoint still owns bounded RFC transaction residue"
    );

    // Protocol Timer F/J cleanup: transport-owned residue has its own finite lifetime.
    tokio::time::advance(Duration::from_secs(64)).await;
    for _ in 0..100 {
        if notifier_endpoint.outstanding().await.expect("endpoint") == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(notifier_endpoint.outstanding().await.expect("endpoint"), 0);
    pump.abort();
}

#[tokio::test]
async fn malformed_colliding_and_template_subscriptions_never_mutate_the_store() {
    let (watcher, mut watcher_incoming) = endpoint().await;
    let (notifier_endpoint, notifier_incoming) = endpoint().await;
    let notifier = Notifier::new(Duration::from_secs(30), 2);
    let handle = notifier.handle();
    let pump = pump(&notifier_endpoint, notifier_incoming, notifier);
    let raw = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("raw peer binds");

    for malformed in [
        replace_header(
            subscribe(&watcher, "bad-expires", "w7", None, "dialog", 5, 1),
            HeaderName::Expires,
            b"4294967296",
        ),
        replace_header(
            subscribe(&watcher, "bad-cseq", "w8", None, "dialog", 5, 1),
            HeaderName::CSeq,
            b"2 MESSAGE",
        ),
    ] {
        // A malformed CSeq cannot be associated with the well-formed client transaction that
        // originated it, so inspect the response on the wire rather than through that matcher.
        let response = raw_final_response(&raw, notifier_endpoint.local_addr(), malformed).await;
        assert_eq!(response.status.code(), 400);
    }
    assert_eq!(shared_store(&handle).lock().expect("store").active(), 0);
    assert_eq!(handle.counts().started_tasks, 0);

    let template = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "template", "w9", None, "dialog.winfo", 5, 1),
    )
    .await;
    assert_eq!(template.status.code(), 489);
    assert_eq!(shared_store(&handle).lock().expect("store").active(), 0);

    let first = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "collision", "w10", None, "dialog", 5, 1),
    )
    .await;
    assert_eq!(first.status.code(), 200);
    let initial = next_notify(&mut watcher_incoming).await;
    answer_notify(&watcher, &initial).await;

    let collision = final_response(
        &watcher,
        notifier_endpoint.local_addr(),
        subscribe(&watcher, "collision", "w10", None, "dialog", 5, 2),
    )
    .await;
    assert_eq!(collision.status.code(), 481);
    assert_eq!(shared_store(&handle).lock().expect("store").active(), 1);
    assert_eq!(handle.counts().started_tasks, 1);
    pump.abort();
}
