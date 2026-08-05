//! Real endpoint proofs for both RFC 3903 roles (`S-39`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{
    AllowPublications, Dispatcher, PublicationConfig, Publications, ReplacePublicationState,
};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, Target, bind};
use sipx_ua::Credentials;
use sipx_ua::event_client::{Peer, Transport};
use sipx_ua::presence::{Compositor, PIDF_TYPE};
use sipx_ua::publication_client::{Start, StateChange, Termination};
use tokio::sync::mpsc::Receiver;

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("address"),
    ))
    .await
    .expect("endpoint")
}

async fn next_publish(incoming: &mut Receiver<Incoming>) -> Incoming {
    let incoming = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("PUBLISH is bounded")
        .expect("endpoint open");
    assert_eq!(incoming.request.method, Method::Publish);
    incoming
}

fn publish_request(
    resource: &str,
    cseq: u32,
    expires: u64,
    tag: Option<&str>,
    body: &'static [u8],
) -> Request {
    let mut builder = RequestBuilder::new(
        Method::Publish,
        Uri::parse(Bytes::copy_from_slice(resource.as_bytes())).expect("URI"),
    )
    .header(HeaderName::To, Bytes::from(format!("<{resource}>")))
    .expect("To")
    .header(HeaderName::From, "<sip:alice@example.test>;tag=epa-a")
    .expect("From")
    .header(HeaderName::CallId, "publication-a@example.test")
    .expect("Call-ID")
    .cseq(cseq, &Method::Publish)
    .expect("CSeq")
    .header(HeaderName::Event, "presence")
    .expect("Event")
    .header(HeaderName::Expires, Bytes::from(expires.to_string()))
    .expect("Expires")
    .max_forwards(70);
    if let Some(tag) = tag {
        builder = builder
            .header(HeaderName::SipIfMatch, Bytes::from(tag.to_owned()))
            .expect("SIP-If-Match");
    }
    if !body.is_empty() {
        builder = builder
            .header(HeaderName::ContentType, PIDF_TYPE)
            .expect("Content-Type");
    }
    builder.body(Bytes::from_static(body)).build()
}

async fn transact(endpoint: &Handle, target: std::net::SocketAddr, request: Request) -> Response {
    let mut responses = endpoint
        .send(request, Target::udp(target))
        .await
        .expect("transaction starts");
    tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("response is bounded")
        .expect("final response")
}

fn tag(response: &Response) -> String {
    String::from_utf8_lossy(
        &response
            .headers
            .value(&HeaderName::SipETag)
            .expect("SIP-ETag"),
    )
    .into_owned()
}

async fn success(endpoint: &Handle, incoming: &Incoming, tag: &str, expires: u64) {
    let response = ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(200).expect("status"),
        "OK",
    )
    .expect("response")
    .header(HeaderName::SipETag, Bytes::from(tag.to_owned()))
    .expect("SIP-ETag")
    .header(HeaderName::Expires, Bytes::from(expires.to_string()))
    .expect("Expires")
    .build();
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

fn config() -> PublicationConfig {
    PublicationConfig {
        minimum_expiry: Duration::from_secs(2),
        default_expiry: Duration::from_secs(10),
        capacity: 1,
        body_limit: 1_024,
        ..PublicationConfig::default()
    }
}

/// S39-V1 through V4: the dispatcher mutates the exact injected compositor and puts every
/// conditional/interval result on the wire.
#[tokio::test(start_paused = true)]
async fn inbound_publications_rotate_tags_fail_closed_and_release_state() {
    let (server, server_incoming) = endpoint().await;
    let service = Publications::new(
        config(),
        Compositor::new(Duration::from_secs(10)),
        Arc::new(ReplacePublicationState),
        Arc::new(AllowPublications),
    )
    .expect("service");
    let handle = service.handle();
    let mut dispatcher =
        Dispatcher::new(server.clone(), server_incoming).with_publications(service);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
    let (client, _) = endpoint().await;
    let resource = "sip:alice@example.test";

    let brief = transact(
        &client,
        server.local_addr(),
        publish_request(resource, 1, 1, None, b"<presence/>"),
    )
    .await;
    assert_eq!(brief.status.code(), 423);
    assert_eq!(
        brief.headers.value(&HeaderName::MinExpires).as_deref(),
        Some(&b"2"[..])
    );
    assert_eq!(handle.counts().active_publications, 0);

    let initial = transact(
        &client,
        server.local_addr(),
        publish_request(resource, 2, 10, None, b"<presence>one</presence>"),
    )
    .await;
    assert_eq!(initial.status.code(), 200);
    let tag_a = tag(&initial);
    assert_eq!(handle.counts().active_publications, 1);

    let wrong_resource = transact(
        &client,
        server.local_addr(),
        publish_request("sip:bob@example.test", 3, 10, Some(&tag_a), b""),
    )
    .await;
    assert_eq!(wrong_resource.status.code(), 412);

    let refresh = transact(
        &client,
        server.local_addr(),
        publish_request(resource, 4, 10, Some(&tag_a), b""),
    )
    .await;
    assert_eq!(refresh.status.code(), 200);
    let tag_b = tag(&refresh);
    assert_ne!(tag_a, tag_b);

    let stale = transact(
        &client,
        server.local_addr(),
        publish_request(resource, 5, 10, Some(&tag_a), b"<presence>bad</presence>"),
    )
    .await;
    assert_eq!(stale.status.code(), 412);
    assert_eq!(
        handle
            .compositor()
            .lock()
            .expect("compositor")
            .document(resource),
        Some("<presence>one</presence>")
    );

    let modified = transact(
        &client,
        server.local_addr(),
        publish_request(resource, 6, 10, Some(&tag_b), b"<presence>two</presence>"),
    )
    .await;
    let tag_c = tag(&modified);
    assert_eq!(
        handle
            .compositor()
            .lock()
            .expect("compositor")
            .document(resource),
        Some("<presence>two</presence>")
    );

    let removed = transact(
        &client,
        server.local_addr(),
        publish_request(resource, 7, 0, Some(&tag_c), b""),
    )
    .await;
    assert_eq!(removed.status.code(), 200);
    assert!(!tag(&removed).is_empty());
    assert_eq!(
        removed.headers.value(&HeaderName::Expires).as_deref(),
        Some(&b"0"[..])
    );
    tokio::task::yield_now().await;
    assert_eq!(handle.counts().active_publications, 0);
    assert_eq!(handle.counts().active_timers, 0);

    client.shutdown().await;
    server.shutdown().await;
    dispatch.await.expect("dispatcher joins");
}

/// S39-V5 and V8: the public publisher authenticates, refreshes, modifies and removes through real
/// transactions, then leaves no owned work.
#[tokio::test(start_paused = true)]
#[allow(
    clippy::too_many_lines,
    reason = "one real exchange proves tag rotation and cleanup across the complete lifecycle"
)]
async fn outbound_publisher_authenticates_and_drains_every_owned_resource() {
    let (client, client_incoming) = endpoint().await;
    let runtime = Publications::new(
        config(),
        Compositor::new(Duration::from_secs(10)),
        Arc::new(ReplacePublicationState),
        Arc::new(AllowPublications),
    )
    .expect("runtime");
    let handle = runtime.handle();
    let mut dispatcher =
        Dispatcher::new(client.clone(), client_incoming).with_publications(runtime);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
    let (server, mut server_incoming) = endpoint().await;
    let mut publication = handle
        .publish(Start {
            resource: Uri::parse(Bytes::from_static(b"sip:alice@example.test")).expect("URI"),
            local_identity: "<sip:alice@example.test>".to_owned(),
            target: Peer::new(server.local_addr(), Transport::Udp),
            event: "presence".to_owned(),
            expires: Duration::from_secs(10),
            body: Bytes::from_static(b"<presence>one</presence>"),
            content_type: PIDF_TYPE.to_owned(),
            credentials: Some(Credentials::new("alice", "secret")),
            call_id: "outbound-publication@example.test".to_owned(),
            from_tag: "epa-outbound".to_owned(),
            initial_cseq: 1,
        })
        .expect("publisher starts");
    tokio::task::yield_now().await;

    let initial = next_publish(&mut server_incoming).await;
    challenge(&server, &initial).await;
    let authenticated = next_publish(&mut server_incoming).await;
    assert!(
        authenticated
            .request
            .headers
            .value(&HeaderName::Authorization)
            .is_some()
    );
    success(&server, &authenticated, "tag-a", 10).await;
    assert!(matches!(
        publication.next_state().await,
        Some(StateChange::Published(state)) if state.tag == "tag-a"
    ));

    tokio::time::advance(Duration::from_secs(8)).await; // the clock is the measurement: assert the granted refresh deadline itself
    let refresh = next_publish(&mut server_incoming).await;
    assert!(refresh.request.body().is_empty());
    assert_eq!(
        refresh
            .request
            .headers
            .value(&HeaderName::SipIfMatch)
            .as_deref(),
        Some(&b"tag-a"[..])
    );
    success(&server, &refresh, "tag-b", 10).await;
    assert!(matches!(
        publication.next_state().await,
        Some(StateChange::Published(state)) if state.tag == "tag-b"
    ));

    publication
        .modify(Bytes::from_static(b"<presence>two</presence>"), PIDF_TYPE)
        .await
        .expect("modify admitted");
    let modify = next_publish(&mut server_incoming).await;
    assert_eq!(
        modify
            .request
            .headers
            .value(&HeaderName::SipIfMatch)
            .as_deref(),
        Some(&b"tag-b"[..])
    );
    success(&server, &modify, "tag-c", 10).await;
    assert!(matches!(
        publication.next_state().await,
        Some(StateChange::Published(state)) if state.tag == "tag-c"
    ));

    publication.remove().await.expect("remove admitted");
    let remove = next_publish(&mut server_incoming).await;
    assert_eq!(
        remove
            .request
            .headers
            .value(&HeaderName::Expires)
            .as_deref(),
        Some(&b"0"[..])
    );
    assert_eq!(
        remove
            .request
            .headers
            .value(&HeaderName::SipIfMatch)
            .as_deref(),
        Some(&b"tag-c"[..])
    );
    success(&server, &remove, "tag-d", 0).await;
    assert_eq!(
        publication.next_state().await,
        Some(StateChange::Terminated(Termination::Removed))
    );
    for _ in 0..8 {
        if handle.counts().active_tasks == 0 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let counts = handle.counts();
    assert_eq!(counts.active_publishers, 0);
    assert_eq!(counts.active_tasks, 0);
    assert_eq!(counts.active_timers, 0);
    assert_eq!(counts.active_transactions, 0);

    tokio::time::advance(Duration::from_secs(64)).await; // a definition of silence: no late transaction survives the protocol retention horizon
    for _ in 0..100 {
        if client.outstanding().await.expect("client counts") == 0
            && server.outstanding().await.expect("server counts") == 0
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(client.outstanding().await.expect("client counts"), 0);
    assert_eq!(server.outstanding().await.expect("server counts"), 0);
    client.shutdown().await;
    server.shutdown().await;
    dispatch.await.expect("dispatcher joins");
}

/// S39-V8: dispatcher shutdown is an ownership barrier even while the peer is silent.
#[tokio::test(start_paused = true)]
async fn dispatcher_shutdown_joins_a_live_publication_transaction_and_timer() {
    let (client, client_incoming) = endpoint().await;
    let runtime = Publications::new(
        config(),
        Compositor::new(Duration::from_secs(10)),
        Arc::new(ReplacePublicationState),
        Arc::new(AllowPublications),
    )
    .expect("runtime");
    let handle = runtime.handle();
    let mut dispatcher =
        Dispatcher::new(client.clone(), client_incoming).with_publications(runtime);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
    let (silent, mut silent_incoming) = endpoint().await;
    let mut publication = handle
        .publish(Start {
            resource: Uri::parse(Bytes::from_static(b"sip:alice@example.test")).expect("URI"),
            local_identity: "<sip:alice@example.test>".to_owned(),
            target: Peer::new(silent.local_addr(), Transport::Udp),
            event: "presence".to_owned(),
            expires: Duration::from_secs(10),
            body: Bytes::from_static(b"<presence>one</presence>"),
            content_type: PIDF_TYPE.to_owned(),
            credentials: None,
            call_id: "cancel-publication@example.test".to_owned(),
            from_tag: "cancel-outbound".to_owned(),
            initial_cseq: 1,
        })
        .expect("publisher starts");
    tokio::task::yield_now().await;
    let initial = next_publish(&mut silent_incoming).await;
    success(&silent, &initial, "tag-a", 10).await;
    assert!(matches!(
        publication.next_state().await,
        Some(StateChange::Published(state)) if state.tag == "tag-a"
    ));
    publication
        .modify(Bytes::from_static(b"<presence>two</presence>"), PIDF_TYPE)
        .await
        .expect("modify admitted");
    let _unanswered = next_publish(&mut silent_incoming).await;
    for _ in 0..8 {
        let counts = handle.counts();
        if counts.active_transactions == 1 && counts.active_timers >= 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    let live = handle.counts();
    assert_eq!(live.active_publishers, 1);
    assert_eq!(live.active_transactions, 1);
    assert!(live.active_timers >= 1);

    client.shutdown().await;
    dispatch
        .await
        .expect("dispatcher joins every publication task");
    assert!(matches!(
        publication.next_state().await,
        Some(StateChange::Terminated(
            Termination::Shutdown | Termination::TransactionFailed
        ))
    ));
    let stopped = handle.counts();
    assert_eq!(stopped.active_publishers, 0);
    assert_eq!(stopped.active_tasks, 0);
    assert_eq!(stopped.active_timers, 0);
    assert_eq!(stopped.active_transactions, 0);
    silent.shutdown().await;
}
