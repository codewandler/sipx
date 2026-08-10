//! `M-123`: the peer's SDP answer, not the signalling target, proves a private-range collision.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{DialOptions, Error, dial};
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Target, bind};

const BOUND: Duration = Duration::from_secs(10);

#[tokio::test]
async fn a_colliding_answer_is_acknowledged_torn_down_and_refused() {
    let (answerer, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("answerer binds");
    let answerer_addr = answerer.local_addr();
    let answering_endpoint = answerer.clone();
    let mut answering = tokio::spawn(async move {
        let mut methods = Vec::new();
        while let Some(incoming) = incoming.recv().await {
            methods.push(incoming.request.method.clone());
            if incoming.request.method == Method::Invite {
                let answer = String::from(
                    "v=0\r\no=- 1 1 IN IP4 10.43.2.8\r\ns=-\r\nc=IN IP4 10.43.2.8\r\n\
                     t=0 0\r\nm=audio 40000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
                );
                let response = sipx_sip::build::ResponseBuilder::to_request(
                    &incoming.request,
                    sipx_sip::StatusCode::new(200).expect("status"),
                    "OK",
                )
                .expect("response")
                .set_header(
                    &HeaderName::To,
                    Bytes::from_static(b"<sip:callee@example.test>;tag=answer"),
                )
                .expect("To")
                .header(
                    HeaderName::Contact,
                    Bytes::from(format!("<sip:callee@{answerer_addr}>")),
                )
                .expect("Contact")
                .header(
                    HeaderName::ContentType,
                    Bytes::from_static(b"application/sdp"),
                )
                .expect("Content-Type")
                .body(Bytes::from(answer))
                .build();
                answering_endpoint
                    .respond(&incoming.key, response)
                    .await
                    .expect("answer leaves");
            } else if incoming.request.method == Method::Bye {
                let response = sipx_sip::build::ResponseBuilder::to_request(
                    &incoming.request,
                    sipx_sip::StatusCode::new(200).expect("status"),
                    "OK",
                )
                .expect("response")
                .build();
                answering_endpoint
                    .respond(&incoming.key, response)
                    .await
                    .expect("BYE is answered");
                break;
            }
        }
        methods
    });

    let (caller, _caller_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("address")))
        .await
        .expect("caller binds");
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("host")));
    let advertised: IpAddr = "10.99.0.3".parse().expect("address");
    let result = dial(
        &caller,
        Target::udp(answerer_addr),
        &to,
        &DialOptions::new("<sip:caller@example.test>", advertised)
            .with_media_bind_address("127.0.0.1".parse().expect("address")),
    )
    .await;
    assert!(matches!(
        result,
        Err(Error::MediaRangeCollision {
            advertised: found_advertised,
            peer,
            range: "10.0.0.0/8",
        }) if found_advertised == advertised && peer == "10.43.2.8".parse::<IpAddr>().expect("address")
    ));

    let methods = if let Ok(joined) = tokio::time::timeout(BOUND, &mut answering).await {
        joined.expect("answerer joins")
    } else {
        answering.abort();
        let _ = answering.await;
        panic!("ACK and BYE did not arrive within {BOUND:?}");
    };
    assert_eq!(methods, vec![Method::Invite, Method::Ack, Method::Bye]);
}
