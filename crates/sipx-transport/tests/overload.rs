//! Endpoint integration for RFC 7339 client admission.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::{OcParameter, OverloadAlgorithm, Via};
use sipx_sip::{HeaderName, Host, HostName, Method, TuEvent, Uri};
use sipx_transport::{Config, Target, bind};

fn supporting_client_config() -> Config {
    let mut config = Config::new("127.0.0.1:0".parse().expect("client address"));
    config.overload.advertise = true;
    config
}

fn request(call_id: &str) -> sipx_sip::Request {
    let uri = Uri::sip(Host::Name(HostName::new("callee.example").expect("host")));
    RequestBuilder::new(Method::Options, uri)
        .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
        .expect("to")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller.example>;tag=one"),
        )
        .expect("from")
        .header(
            HeaderName::CallId,
            Bytes::copy_from_slice(call_id.as_bytes()),
        )
        .expect("call-id")
        .cseq(1, &Method::Options)
        .expect("cseq")
        .max_forwards(70)
        .build()
}

fn invite(call_id: &str) -> sipx_sip::Request {
    let uri = Uri::sip(Host::Name(HostName::new("callee.example").expect("host")));
    RequestBuilder::new(Method::Invite, uri)
        .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
        .expect("to")
        .header(
            HeaderName::From,
            Bytes::from_static(b"<sip:caller.example>;tag=one"),
        )
        .expect("from")
        .header(
            HeaderName::CallId,
            Bytes::copy_from_slice(call_id.as_bytes()),
        )
        .expect("call-id")
        .cseq(1, &Method::Invite)
        .expect("cseq")
        .max_forwards(70)
        .build()
}

#[tokio::test]
async fn a_default_endpoint_does_not_advertise_the_overload_extension() {
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("peer binds");
    let peer_addr = peer.local_addr().expect("peer address");
    let (client, _incoming) = bind(Config::new("127.0.0.1:0".parse().expect("client address")))
        .await
        .expect("client binds");

    let sent = client
        .send(request("default@sipx"), Target::udp(peer_addr))
        .await;
    assert!(sent.is_ok(), "default request sends: {sent:?}");
    let mut bytes = [0u8; 4096];
    let (length, _) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut bytes))
        .await
        .expect("request is bounded")
        .expect("request arrives");
    let message = String::from_utf8_lossy(bytes.get(..length).expect("received range"));
    let via = message
        .lines()
        .find(|line| line.starts_with("Via:"))
        .expect("Via exists");
    assert!(
        !via.contains(";oc"),
        "default Via advertised overload: {via}"
    );

    client.shutdown().await;
}

#[tokio::test]
async fn an_unmatched_response_cannot_install_overload_control() {
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("peer binds");
    let peer_addr = peer.local_addr().expect("peer address");
    let (client, _incoming) = bind(supporting_client_config())
        .await
        .expect("client binds");
    let mut unmatched = client
        .watch_unmatched(1)
        .await
        .expect("unmatched watcher installs");
    let forged = format!(
        "SIP/2.0 200 OK\r\n\
         Via: SIP/2.0/UDP {};branch=z9hG4bK-forged;oc=100;oc-algo=loss;oc-validity=10000;oc-seq=9.0\r\n\
         To: <sip:callee.example>;tag=server\r\n\
         From: <sip:caller.example>;tag=one\r\n\
         Call-ID: forged@sipx\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n",
        client.sent_by_for(sipx_transport::TransportKind::Udp)
    );
    peer.send_to(forged.as_bytes(), client.local_addr())
        .await
        .expect("forged response sends");
    let seen = tokio::time::timeout(Duration::from_secs(2), unmatched.recv())
        .await
        .expect("driver processes the unmatched response")
        .expect("watcher remains open");
    assert_eq!(seen.response.status.code(), 200);

    for number in 0..8 {
        client
            .send_directly(
                request(&format!("after-forgery-{number}@sipx")),
                Target::udp(peer_addr),
            )
            .await
            .expect("unmatched feedback cannot reject a request");
    }
    assert_eq!(client.counters().overload_rejections, 0);

    client.shutdown().await;
}

#[tokio::test]
async fn an_ordinary_response_selects_overload_control_and_reports_it_off() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("server address")))
        .await
        .expect("server binds");
    let (client, _incoming) = bind(supporting_client_config())
        .await
        .expect("client binds");
    let target = Target::udp(server.local_addr());
    let server_handle = server.clone();
    let answered = tokio::spawn(async move {
        let incoming = incoming.recv().await.expect("request arrives");
        let status = sipx_sip::StatusCode::new(200).expect("status");
        let response = sipx_sip::ResponseBuilder::to_request(&incoming.request, status, "OK")
            .expect("response")
            .build();
        server_handle
            .respond(&incoming.key, response)
            .await
            .expect("response sends");
    });
    let mut responses = client
        .send(request("control-off@sipx"), target)
        .await
        .expect("request sends");
    let response = responses.final_response().await.expect("final response");
    answered.await.expect("answer task");

    let via_value = response.headers.value(&HeaderName::Via).expect("Via");
    assert!(
        via_value
            .windows(b"oc-algo=\"loss\"".len())
            .any(|part| part == b"oc-algo=\"loss\""),
        "the selected server algorithm is quoted"
    );
    let overload = response
        .headers
        .typed::<Via>()
        .expect("Via")
        .expect("Via parses")
        .overload()
        .expect("overload parses");
    assert_eq!(overload.oc, Some(OcParameter::Value(0)));
    assert_eq!(overload.algorithms, vec![OverloadAlgorithm::Loss]);
    assert_eq!(overload.validity, Some(Duration::ZERO));
    assert!(overload.sequence.is_some());

    client.shutdown().await;
    server.shutdown().await;
}

#[tokio::test]
async fn a_transaction_generated_trying_response_reports_current_overload_state() {
    let (server, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("server address")))
        .await
        .expect("server binds");
    let (client, _incoming) = bind(supporting_client_config())
        .await
        .expect("client binds");
    let mut responses = client
        .send(invite("trying@sipx"), Target::udp(server.local_addr()))
        .await
        .expect("INVITE sends");
    let pending = tokio::time::timeout(Duration::from_secs(2), incoming.recv())
        .await
        .expect("request arrives")
        .expect("incoming channel remains open");

    let event = tokio::time::timeout(Duration::from_secs(2), responses.next())
        .await
        .expect("the transaction generates 100 Trying")
        .expect("client transaction remains open");
    let TuEvent::Response(response) = event else {
        panic!("expected the generated provisional response, got {event:?}");
    };
    assert_eq!(response.status.code(), 100);
    let overload = response
        .headers
        .typed::<Via>()
        .expect("100 has a Via")
        .expect("Via parses")
        .overload()
        .expect("overload parameters parse");
    assert_eq!(overload.oc, Some(OcParameter::Value(0)));
    assert_eq!(overload.algorithms, vec![OverloadAlgorithm::Loss]);
    assert_eq!(overload.validity, Some(Duration::ZERO));
    assert!(overload.sequence.is_some());

    let status = sipx_sip::StatusCode::new(487).expect("status");
    let final_response =
        sipx_sip::ResponseBuilder::to_request(&pending.request, status, "Request Terminated")
            .expect("response")
            .build();
    server
        .respond(&pending.key, final_response)
        .await
        .expect("final response sends");
    assert_eq!(
        responses
            .final_response()
            .await
            .expect("final response arrives")
            .status
            .code(),
        487
    );

    client.shutdown().await;
    server.shutdown().await;
}
