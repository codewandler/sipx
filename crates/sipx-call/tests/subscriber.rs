//! A real transaction-layer proof for the public RFC 6665 subscriber path (`S-38`).

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
use sipx_ua::Credentials;
use sipx_ua::event_client::{
    Config, Lifecycle, PackageConsumer, PackageRejection, Peer, SamePeer, Start, StateChange,
    Termination, Transport,
};
use tokio::sync::mpsc::Receiver;

#[derive(Debug)]
struct TextPackage;

impl PackageConsumer for TextPackage {
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
        if !body.is_empty() && content_type != Some(&b"application/test-state"[..]) {
            return Err(PackageRejection::unsupported_media());
        }
        std::str::from_utf8(body)
            .map(str::to_owned)
            .map_err(|_| PackageRejection::malformed())
    }
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("address"),
    ))
    .await
    .expect("endpoint")
}

async fn next_request(incoming: &mut Receiver<Incoming>, method: Method) -> Incoming {
    let request = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("protocol exchange is bounded")
        .expect("endpoint remains open");
    assert_eq!(request.request.method, method);
    request
}

async fn respond(endpoint: &Handle, incoming: &Incoming, expires: u64, tag: &str) {
    let mut response = ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(200).expect("status"),
        "OK",
    )
    .expect("response")
    .build();
    response.headers.remove_all(&HeaderName::To);
    response.headers.push(
        sipx_sip::Header::build(
            HeaderName::To,
            Bytes::from(format!("<sip:resource@example.test>;tag={tag}")),
        )
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

async fn challenge(endpoint: &Handle, incoming: &Incoming) {
    let response = ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(401).expect("status"),
        "Unauthorized",
    )
    .expect("response")
    .header(
        HeaderName::WwwAuthenticate,
        "Digest realm=\"example.test\", nonce=\"n1\", qop=\"auth\", algorithm=SHA-256",
    )
    .expect("challenge")
    .build();
    endpoint
        .respond(&incoming.key, response)
        .await
        .expect("challenge sends");
}

fn notify(peer: &Handle, cseq: u32, state: &str, body: &'static [u8]) -> Request {
    let mut builder = RequestBuilder::new(
        Method::Notify,
        Uri::parse(Bytes::from_static(b"sip:client@127.0.0.1")).expect("URI"),
    )
    .header(
        HeaderName::From,
        "<sip:resource@example.test>;tag=notifier-a",
    )
    .expect("From")
    .header(HeaderName::To, "<sip:client@example.test>;tag=sub-a")
    .expect("To")
    .header(HeaderName::CallId, "sub-a@example.test")
    .expect("Call-ID")
    .cseq(cseq, &Method::Notify)
    .expect("CSeq")
    .header(
        HeaderName::Contact,
        Bytes::from(format!("<sip:notifier@{}>", peer.local_addr())),
    )
    .expect("Contact")
    .header(HeaderName::Event, "test-state;id=alpha")
    .expect("Event")
    .header(
        HeaderName::SubscriptionState,
        Bytes::copy_from_slice(state.as_bytes()),
    )
    .expect("Subscription-State")
    .max_forwards(70);
    if !body.is_empty() {
        builder = builder
            .header(HeaderName::ContentType, "application/test-state")
            .expect("Content-Type");
    }
    builder.body(Bytes::from_static(body)).build()
}

async fn send_notify(peer: &Handle, destination: std::net::SocketAddr, request: Request) -> u16 {
    let mut responses = peer
        .send(request, Target::udp(destination))
        .await
        .expect("NOTIFY starts");
    tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("NOTIFY answer is bounded")
        .expect("NOTIFY has a final answer")
        .status
        .code()
}

#[tokio::test(start_paused = true)]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end exchange keeps authentication, refresh and cleanup on the same usage"
)]
async fn a_public_subscription_authenticates_refreshes_and_drains_owned_work() {
    let (client_endpoint, client_incoming) = endpoint().await;
    let (notifier, mut notifier_incoming) = endpoint().await;
    let runtime = EventSubscriptions::new(Config::default()).expect("runtime");
    let handle = runtime.handle();
    let mut dispatcher =
        Dispatcher::new(client_endpoint.clone(), client_incoming).with_event_subscriptions(runtime);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });

    let mut subscription = handle
        .subscribe(Start {
            resource: Uri::parse(Bytes::from_static(b"sip:resource@example.test")).expect("URI"),
            local_identity: "<sip:client@example.test>".to_owned(),
            contact: format!("<sip:client@{}>", client_endpoint.local_addr()),
            target: Peer::new(notifier.local_addr(), Transport::Udp),
            expires: Duration::from_secs(10),
            body: Bytes::new(),
            content_type: None,
            credentials: Some(Credentials::new("client", "secret")),
            call_id: "sub-a@example.test".to_owned(),
            from_tag: "sub-a".to_owned(),
            initial_cseq: 1,
            consumer: TextPackage,
            trust: Arc::new(SamePeer),
        })
        .expect("subscription starts");

    assert_eq!(subscription.recv().await.expect("neutral").value, "neutral");
    let initial = next_request(&mut notifier_incoming, Method::Subscribe).await;
    assert_eq!(
        initial
            .request
            .headers
            .value(&HeaderName::Expires)
            .as_deref(),
        Some(&b"10"[..])
    );
    challenge(&notifier, &initial).await;
    let authenticated = next_request(&mut notifier_incoming, Method::Subscribe).await;
    assert!(
        authenticated
            .request
            .headers
            .value(&HeaderName::Authorization)
            .is_some()
    );
    respond(&notifier, &authenticated, 10, "notifier-a").await;
    assert_eq!(
        send_notify(
            &notifier,
            client_endpoint.local_addr(),
            notify(&notifier, 1, "active;expires=10", b"one"),
        )
        .await,
        200
    );
    let delivery = subscription.recv().await.expect("package value");
    assert_eq!(delivery.value, "one");
    assert_eq!(
        delivery
            .metadata
            .expect("framework metadata")
            .subscription
            .state,
        sipx_sip::event::State::Active
    );

    // Protocol refresh deadline: four fifths of the authoritative ten-second lease.
    tokio::time::advance(Duration::from_secs(8)).await; // the clock is the measurement: assert the refresh deadline itself
    let refresh = next_request(&mut notifier_incoming, Method::Subscribe).await;
    assert_eq!(
        refresh
            .request
            .headers
            .value(&HeaderName::Expires)
            .as_deref(),
        Some(&b"10"[..])
    );
    respond(&notifier, &refresh, 10, "notifier-a").await;
    assert_eq!(
        send_notify(
            &notifier,
            client_endpoint.local_addr(),
            notify(&notifier, 2, "active;expires=10", b"two"),
        )
        .await,
        200
    );
    assert_eq!(
        subscription.recv().await.expect("refresh value").value,
        "two"
    );

    subscription.unsubscribe().await.expect("command accepted");
    let unsubscribe = next_request(&mut notifier_incoming, Method::Subscribe).await;
    assert_eq!(
        unsubscribe
            .request
            .headers
            .value(&HeaderName::Expires)
            .as_deref(),
        Some(&b"0"[..])
    );
    respond(&notifier, &unsubscribe, 0, "notifier-a").await;
    assert_eq!(
        send_notify(
            &notifier,
            client_endpoint.local_addr(),
            notify(&notifier, 3, "terminated;reason=timeout", b""),
        )
        .await,
        200
    );

    let mut terminated = false;
    while let Some(change) = subscription.next_state().await {
        if matches!(
            change,
            StateChange::Terminated(Termination::Remote(Some(sipx_sip::event::Reason::Timeout)))
        ) {
            terminated = true;
            break;
        }
        assert!(matches!(
            change,
            StateChange::State(
                Lifecycle::NotifyWait | Lifecycle::Active | Lifecycle::Unsubscribing
            )
        ));
    }
    assert!(terminated);
    tokio::task::yield_now().await;
    let counts = handle.counts();
    assert_eq!(counts.active_tasks, 0);
    assert_eq!(counts.active_timers, 0);
    assert_eq!(counts.active_transactions, 0);
    assert_eq!(counts.started_tasks, 1);
    assert_eq!(counts.finished_tasks, 1);

    // Protocol Timer F/J cleanup: completed transaction residue is finite and observable.
    tokio::time::advance(Duration::from_secs(64)).await; // a definition of silence: no late transaction or timer survives the protocol horizon
    for _ in 0..100 {
        if client_endpoint.outstanding().await.expect("endpoint") == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(client_endpoint.outstanding().await.expect("endpoint"), 0);
    dispatch.abort();
}

#[tokio::test(start_paused = true)]
async fn dispatcher_shutdown_joins_a_silent_subscription_transaction_and_timers() {
    let (client, client_incoming) = endpoint().await;
    let (silent, mut silent_incoming) = endpoint().await;
    let runtime = EventSubscriptions::new(Config::default()).expect("runtime");
    let handle = runtime.handle();
    let mut dispatcher =
        Dispatcher::new(client.clone(), client_incoming).with_event_subscriptions(runtime);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
    let mut subscription = handle
        .subscribe(Start {
            resource: Uri::parse(Bytes::from_static(b"sip:resource@example.test")).expect("URI"),
            local_identity: "<sip:client@example.test>".to_owned(),
            contact: format!("<sip:client@{}>", client.local_addr()),
            target: Peer::new(silent.local_addr(), Transport::Udp),
            expires: Duration::from_secs(10),
            body: Bytes::new(),
            content_type: None,
            credentials: None,
            call_id: "cancel-subscription@example.test".to_owned(),
            from_tag: "cancel-subscription".to_owned(),
            initial_cseq: 1,
            consumer: TextPackage,
            trust: Arc::new(SamePeer),
        })
        .expect("subscription starts");
    let _neutral = subscription.recv().await.expect("neutral");
    let _unanswered = next_request(&mut silent_incoming, Method::Subscribe).await;
    for _ in 0..8 {
        let counts = handle.counts();
        if counts.active_transactions == 1 && counts.active_timers >= 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let live = handle.counts();
    assert_eq!(live.active_transactions, 1);
    assert!(live.active_timers >= 1);

    client.shutdown().await;
    dispatch.await.expect("dispatcher joins subscription work");
    while let Some(change) = subscription.next_state().await {
        if matches!(
            change,
            StateChange::Terminated(Termination::Shutdown | Termination::TransactionFailed)
        ) {
            break;
        }
    }
    let stopped = handle.counts();
    assert_eq!(stopped.active_tasks, 0);
    assert_eq!(stopped.active_timers, 0);
    assert_eq!(stopped.active_transactions, 0);
    silent.shutdown().await;
}
