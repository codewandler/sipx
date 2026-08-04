//! Deployment-address contracts across signalling and media (`M-42`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(
    clippy::similar_names,
    reason = "caller and callee name distinct SIP roles in these two-endpoint integration tests"
)]

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{DialOptions, Error, MediaAddress, answer_at, dial};
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::Via;
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Target, bind};

fn ip(text: &str) -> IpAddr {
    text.parse().expect("a valid address")
}

/// The spec's initial-request vector is one consistency property, not three unrelated builders.
/// The advertised address is TEST-NET and therefore cannot be bound on this host; seeing the
/// INVITE also proves the RTP socket used the independent loopback bind address.
#[tokio::test]
async fn one_advertised_address_reaches_contact_via_and_sdp_while_media_binds_elsewhere() {
    let (peer, mut incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds peer");

    let advertised = ip("198.51.100.44");
    let mut caller_config = Config::new("127.0.0.1:0".parse().unwrap());
    caller_config.sent_by = advertised.to_string();
    caller_config.sent_by_port = Some(5080);
    let (caller, _caller_incoming) = bind(caller_config).await.expect("binds caller");

    let seen = tokio::spawn(async move { incoming.recv().await.expect("an INVITE") });
    let to = Uri::sip(Host::Name(HostName::new("callee.example").unwrap()));
    let _ = dial(
        &caller,
        Target::udp(peer.local_addr()),
        &to,
        &DialOptions::new("<sip:alice@example.net>", advertised)
            .with_media_bind_address(ip("127.0.0.1"))
            .with_timeout(Duration::from_millis(300)),
    )
    .await;

    let invite = seen.await.expect("capture task").request;
    let contact = String::from_utf8_lossy(
        &invite
            .headers
            .value(&HeaderName::Contact)
            .expect("a Contact"),
    )
    .into_owned();
    let via = String::from_utf8_lossy(&invite.headers.value(&HeaderName::Via).expect("a Via"))
        .into_owned();
    let sdp = String::from_utf8_lossy(invite.body()).into_owned();

    assert_eq!(contact, "<sip:sipx@198.51.100.44:5080>");
    assert!(
        via.starts_with("SIP/2.0/UDP 198.51.100.44:5080;"),
        "unexpected Via sent-by: {via}"
    );
    assert!(
        via.contains(";branch=z9hG4bK") && via.contains(";rport="),
        "unexpected Via: {via}"
    );
    assert!(
        sdp.contains("\r\nc=IN IP4 198.51.100.44\r\n"),
        "SDP did not advertise the chosen address: {sdp}"
    );
}

#[test]
fn an_ip_address_keeps_the_legacy_bind_equals_advertise_contract() {
    let address = ip("192.0.2.10");
    assert_eq!(MediaAddress::from(address), MediaAddress::new(address));
    assert_eq!(MediaAddress::new(address).advertised(), address);
    assert_eq!(MediaAddress::new(address).bind(), address);
}

#[tokio::test]
async fn an_unspecified_advertised_media_address_is_refused_before_the_invite_leaves() {
    let (peer, mut incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds peer");
    let (caller, _caller_incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds caller");
    let to = Uri::sip(Host::Name(HostName::new("callee.example").unwrap()));

    let error = dial(
        &caller,
        Target::udp(peer.local_addr()),
        &to,
        &DialOptions::new("<sip:alice@example.net>", ip("0.0.0.0")),
    )
    .await
    .expect_err("an unspecified SDP destination is refused");

    assert!(matches!(error, Error::UnspecifiedMediaAddress));
    assert!(
        incoming.try_recv().is_err(),
        "validation must happen before the INVITE is sent"
    );
}

#[tokio::test]
async fn an_inbound_answer_advertises_public_media_but_binds_the_local_interface() {
    let (callee, mut callee_incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds callee");
    let (caller, _caller_incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds caller");
    let to = Uri::sip(Host::Name(HostName::new("callee.example").unwrap()));
    let invite = RequestBuilder::new(Method::Invite, to)
        .header(HeaderName::To, "<sip:callee@example.net>")
        .unwrap()
        .header(HeaderName::From, "<sip:alice@example.net>;tag=a1")
        .unwrap()
        .header(HeaderName::CallId, "inbound-addresses@example.net")
        .unwrap()
        .cseq(1, &Method::Invite)
        .unwrap()
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:alice@{}>", caller.local_addr())),
        )
        .unwrap()
        .header(HeaderName::ContentType, "application/sdp")
        .unwrap()
        .max_forwards(70)
        .body(Bytes::from_static(
            b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 9 RTP/AVP 0\r\n",
        ))
        .build();

    let callee_handle = callee.clone();
    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("the INVITE");
        answer_at(
            &callee_handle,
            &incoming,
            MediaAddress::new(ip("198.51.100.44")).with_bind(ip("127.0.0.1")),
        )
        .await
        .expect("answers")
    });
    let mut responses = caller
        .send(invite, Target::udp(callee.local_addr()))
        .await
        .expect("sends");
    let response = responses.final_response().await.expect("a final response");
    let sdp = String::from_utf8_lossy(response.body());
    assert!(sdp.contains("c=IN IP4 198.51.100.44"), "{sdp}");

    let call = answering.await.expect("answer task");
    assert_eq!(call.media().local_addr().ip(), ip("127.0.0.1"));
}

/// RFC 3581 on an actual in-dialog request: the BYE exists only after INVITE, 200 and ACK have
/// established the dialog. A standalone request carrying BYE as its method is not this witness.
#[tokio::test]
async fn an_established_dialog_bye_records_the_observed_address_and_port() {
    let (callee, mut callee_incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds callee");
    let mut caller_config = Config::new("127.0.0.1:0".parse().unwrap());
    caller_config.sent_by = "198.51.100.44".to_owned();
    let (caller, _caller_incoming) = bind(caller_config).await.expect("binds caller");
    let observed_caller = caller.local_addr();

    let callee_handle = callee.clone();
    let answering = tokio::spawn(async move {
        let invite = callee_incoming.recv().await.expect("the initial INVITE");
        assert_eq!(invite.request.method, Method::Invite);
        let mut call = answer_at(&callee_handle, &invite, MediaAddress::new(ip("127.0.0.1")))
            .await
            .expect("answers the INVITE");

        loop {
            let in_dialog = callee_incoming.recv().await.expect("ACK or BYE");
            if in_dialog.request.method == Method::Bye {
                let via = in_dialog
                    .request
                    .headers
                    .typed::<Via>()
                    .expect("a Via")
                    .expect("Via parses");
                assert_eq!(
                    via.rport().flatten().map(<[u8]>::to_vec),
                    Some(observed_caller.port().to_string().into_bytes())
                );
                assert_eq!(
                    via.received().map(<[u8]>::to_vec),
                    Some(observed_caller.ip().to_string().into_bytes())
                );
                assert!(call.handle(&in_dialog).await.expect("handles the BYE"));
                break;
            }
            assert_eq!(in_dialog.request.method, Method::Ack);
            assert!(call.handle(&in_dialog).await.expect("handles the ACK"));
        }
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").unwrap()));
    let mut call = dial(
        &caller,
        Target::udp(callee.local_addr()),
        &to,
        &DialOptions::new("<sip:alice@example.net>", ip("198.51.100.44"))
            .with_media_bind_address(ip("127.0.0.1"))
            .with_timeout(Duration::from_secs(2)),
    )
    .await
    .expect("establishes the dialog");
    call.hang_up().await.expect("sends the in-dialog BYE");
    tokio::time::timeout(Duration::from_secs(5), answering)
        .await
        .expect("the dialog teardown is bounded")
        .expect("the callee task completes");
}
