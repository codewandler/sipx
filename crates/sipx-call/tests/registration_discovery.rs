//! Live RFC 3680 consumer proof through the generic event client.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{Dispatcher, EventSubscriptions};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Method, Request, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, Target, bind};
use sipx_ua::event_client::{Config, Peer, SamePeer, Start, StateChange, Termination, Transport};
use sipx_ua::reginfo::RegistrationConsumer;
use tokio::sync::mpsc::Receiver;

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("address"),
    ))
    .await
    .expect("endpoint")
}

async fn next_request(incoming: &mut Receiver<Incoming>, method: Method) -> Incoming {
    let incoming = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("protocol exchange is bounded")
        .expect("endpoint remains open");
    assert_eq!(incoming.request.method, method);
    incoming
}

async fn subscribe_ok(endpoint: &Handle, incoming: &Incoming, expires: u64) {
    let mut response = ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(200).expect("status"),
        "OK",
    )
    .expect("response")
    .build();
    response.headers.remove_all(&HeaderName::To);
    response.headers.push(
        sipx_sip::Header::build(HeaderName::To, "<sip:all@example.test>;tag=registrar-a")
            .expect("To"),
    );
    response.headers.push(
        sipx_sip::Header::build(HeaderName::Expires, Bytes::from(expires.to_string()))
            .expect("Expires"),
    );
    endpoint
        .respond(&incoming.key, response)
        .await
        .expect("response sends");
}

fn notify(peer: &Handle, cseq: u32, state: &str, body: Bytes) -> Request {
    let mut builder = RequestBuilder::new(
        Method::Notify,
        Uri::parse(Bytes::from_static(b"sip:client@127.0.0.1")).expect("URI"),
    )
    .header(HeaderName::From, "<sip:all@example.test>;tag=registrar-a")
    .expect("From")
    .header(HeaderName::To, "<sip:client@example.test>;tag=client-a")
    .expect("To")
    .header(HeaderName::CallId, "reg-discovery@example.test")
    .expect("Call-ID")
    .cseq(cseq, &Method::Notify)
    .expect("CSeq")
    .header(
        HeaderName::Contact,
        Bytes::from(format!("<sip:registrar@{}>", peer.local_addr())),
    )
    .expect("Contact")
    .header(HeaderName::Event, "reg")
    .expect("Event")
    .header(
        HeaderName::SubscriptionState,
        Bytes::copy_from_slice(state.as_bytes()),
    )
    .expect("Subscription-State")
    .max_forwards(70);
    if !body.is_empty() {
        builder = builder
            .header(HeaderName::ContentType, "application/reginfo+xml")
            .expect("Content-Type");
    }
    builder.body(body).build()
}

async fn send_notify(peer: &Handle, destination: std::net::SocketAddr, request: Request) {
    let mut responses = peer
        .send(request, Target::udp(destination))
        .await
        .expect("NOTIFY starts");
    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("NOTIFY response is bounded")
        .expect("NOTIFY final response");
    assert_eq!(response.status.code(), 200);
}

fn document(version: u32, state: &str, contact: &str) -> Bytes {
    Bytes::from(format!(
        "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" state=\"{state}\">\
         <registration aor=\"sip:alice@example.test\" id=\"r1\" state=\"active\">\
         {contact}</registration></reginfo>"
    ))
}

/// S24-V1, V2 and V5; the name is the story's required failing-first proof.
#[tokio::test(start_paused = true)]
#[allow(
    clippy::too_many_lines,
    reason = "one live usage proves the generic subscriber retains and removes package state"
)]
async fn a_contact_that_registers_while_subscribed_appears_in_the_list() {
    let (client, client_incoming) = endpoint().await;
    let runtime = EventSubscriptions::new(Config {
        timer_n: Duration::from_secs(10),
        ..Config::default()
    })
    .expect("runtime");
    let handle = runtime.handle();
    let mut dispatcher =
        Dispatcher::new(client.clone(), client_incoming).with_event_subscriptions(runtime);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
    let (registrar, mut registrar_incoming) = endpoint().await;
    let package = RegistrationConsumer::new("sip:all@example.test", 8).expect("package");
    let mut subscription = handle
        .subscribe(Start {
            resource: Uri::parse(Bytes::from_static(b"sip:all@example.test")).expect("URI"),
            local_identity: "<sip:client@example.test>".to_owned(),
            contact: format!("<sip:client@{}>", client.local_addr()),
            target: Peer::new(registrar.local_addr(), Transport::Udp),
            expires: Duration::from_secs(20),
            body: Bytes::new(),
            content_type: None,
            credentials: None,
            call_id: "reg-discovery@example.test".to_owned(),
            from_tag: "client-a".to_owned(),
            initial_cseq: 1,
            consumer: package,
            trust: Arc::new(SamePeer),
        })
        .expect("subscription starts");
    tokio::task::yield_now().await;

    let initial = next_request(&mut registrar_incoming, Method::Subscribe).await;
    assert_eq!(
        initial.request.headers.value(&HeaderName::Event).as_deref(),
        Some(&b"reg"[..])
    );
    assert!(
        initial
            .request
            .headers
            .value(&HeaderName::Accept)
            .is_some_and(|value| value.as_ref() == b"application/reginfo+xml")
    );
    subscribe_ok(&registrar, &initial, 20).await;

    send_notify(
        &registrar,
        client.local_addr(),
        notify(&registrar, 1, "active;expires=20", document(0, "full", "")),
    )
    .await;
    let empty = subscription.recv().await.expect("full snapshot");
    assert!(empty.value.peers.is_empty());

    let active = "<contact id=\"c1\" state=\"active\" event=\"registered\">\
                  <uri>sip:alice@192.0.2.10</uri></contact>";
    send_notify(
        &registrar,
        client.local_addr(),
        notify(
            &registrar,
            2,
            "active;expires=20",
            document(1, "partial", active),
        ),
    )
    .await;
    let registered = subscription.recv().await.expect("registration update");
    assert_eq!(registered.value.peers.len(), 1);
    assert_eq!(registered.value.peers[0].uri, "sip:alice@192.0.2.10");
    assert_eq!(
        registered.value.peers[0].source.resource,
        "sip:all@example.test"
    );
    assert!(registered.received_at.elapsed() < Duration::from_secs(1)); // the clock is the measurement: this asserts the reported snapshot age

    let expired = "<contact id=\"c1\" state=\"terminated\" event=\"expired\"/>";
    send_notify(
        &registrar,
        client.local_addr(),
        notify(
            &registrar,
            3,
            "active;expires=20",
            document(2, "partial", expired),
        ),
    )
    .await;
    assert!(
        subscription
            .recv()
            .await
            .expect("expiry update")
            .value
            .peers
            .is_empty()
    );

    subscription
        .unsubscribe()
        .await
        .expect("unsubscribe admitted");
    let unsubscribe = next_request(&mut registrar_incoming, Method::Subscribe).await;
    subscribe_ok(&registrar, &unsubscribe, 0).await;
    send_notify(
        &registrar,
        client.local_addr(),
        notify(&registrar, 4, "terminated;reason=deactivated", Bytes::new()),
    )
    .await;
    loop {
        let state = subscription.next_state().await.expect("terminal state");
        if matches!(state, StateChange::Terminated(Termination::Remote(_))) {
            break;
        }
    }
    for _ in 0..8 {
        if handle.counts().active_tasks == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(handle.counts().active_tasks, 0);
    assert_eq!(handle.counts().active_timers, 0);
    assert_eq!(handle.counts().active_transactions, 0);

    client.shutdown().await;
    registrar.shutdown().await;
    dispatch.await.expect("dispatcher joins");
}

/// S24-V4.
#[tokio::test(start_paused = true)]
async fn registrar_refusals_are_typed_before_any_peer_snapshot() {
    for status in [403, 489] {
        let (client, client_incoming) = endpoint().await;
        let runtime = EventSubscriptions::new(Config::default()).expect("runtime");
        let handle = runtime.handle();
        let mut dispatcher =
            Dispatcher::new(client.clone(), client_incoming).with_event_subscriptions(runtime);
        let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
        let (registrar, mut registrar_incoming) = endpoint().await;
        let mut subscription = handle
            .subscribe(Start {
                resource: Uri::parse(Bytes::from_static(b"sip:all@example.test")).expect("URI"),
                local_identity: "<sip:client@example.test>".to_owned(),
                contact: format!("<sip:client@{}>", client.local_addr()),
                target: Peer::new(registrar.local_addr(), Transport::Udp),
                expires: Duration::from_secs(20),
                body: Bytes::new(),
                content_type: None,
                credentials: None,
                call_id: format!("refusal-{status}@example.test"),
                from_tag: format!("client-{status}"),
                initial_cseq: 1,
                consumer: RegistrationConsumer::new("sip:all@example.test", 8).expect("package"),
                trust: Arc::new(SamePeer),
            })
            .expect("subscription");
        tokio::task::yield_now().await;
        let request = next_request(&mut registrar_incoming, Method::Subscribe).await;
        let response = ResponseBuilder::to_request(
            &request.request,
            StatusCode::new(status).expect("status"),
            "Refused",
        )
        .expect("response")
        .build();
        registrar
            .respond(&request.key, response)
            .await
            .expect("refusal");
        loop {
            let state = subscription.next_state().await.expect("terminal state");
            if matches!(state, StateChange::Terminated(Termination::Rejected(code)) if code == status)
            {
                break;
            }
        }
        assert!(subscription.recv().await.is_none());
        client.shutdown().await;
        registrar.shutdown().await;
        dispatch.await.expect("dispatcher joins");
    }
}
