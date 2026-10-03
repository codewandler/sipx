//! Public protocol recovery after local media-capacity refusal.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use sipx_call::{
    Call, CallEvent, DialOptions, MediaAddress, MediaPolicy, answer_at, dial,
    dial_early_without_offer, ring_offer_early_with_policy_at,
};
use sipx_media::{MediaPort, MediaPortRange};
use sipx_sdp::Direction;
use sipx_sip::{HeaderName, Host, HostName, Method, Request, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

fn ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}
fn uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("peer.test").unwrap()))
}

fn reoffer(call: &Call, method: &Method, sequence: u32, remote_port: u16) -> Request {
    let (to, from) = call.dialog.local_and_remote();
    let body = format!(
        "v=0\r\no=- 1 {sequence} IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {remote_port} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=rtcp-mux\r\na=sendonly\r\n"
    );
    sipx_sip::build::RequestBuilder::new(method.clone(), uri())
        .header(HeaderName::To, Bytes::from(to))
        .unwrap()
        .header(HeaderName::From, Bytes::from(from))
        .unwrap()
        .header(
            HeaderName::CallId,
            Bytes::from(call.dialog.id.call_id.clone()),
        )
        .unwrap()
        .cseq(sequence, method)
        .unwrap()
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )
        .unwrap()
        .max_forwards(70)
        .body(Bytes::from(body))
        .build()
}

async fn exchange(
    peer: &Handle,
    local: &Handle,
    incoming: &mut Receiver<Incoming>,
    call: &mut Call,
    request: Request,
) -> u16 {
    let method = request.method.clone();
    let mut response = peer
        .send(request, Target::udp(local.local_addr()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let received = incoming.recv().await.unwrap();
            call.handle(&received).await.unwrap();
            if received.request.method == method {
                break;
            }
        }
        response.final_response().await.unwrap().status.code()
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn refused_capacity_does_not_leave_offer_debt_or_block_later_hold() {
    for method in [Method::Invite, Method::Update] {
        let reservation = MediaPort::bind((ip(), 0).into()).await.unwrap();
        let port = reservation.local_addr().port();
        let range = MediaPortRange::new(port, port + 1).unwrap();
        let (caller, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
        let (local, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
        let target_uri = uri();
        let options = DialOptions::new("<sip:caller@example.test>", ip());
        drop(reservation);
        let (outbound, inbound) = tokio::join!(
            dial(
                &caller,
                Target::udp(local.local_addr()),
                &target_uri,
                &options
            ),
            async {
                let invite = incoming.recv().await.unwrap();
                answer_at(
                    &local,
                    &invite,
                    MediaAddress::new(ip()).with_port_range(range),
                )
                .await
            }
        );
        let outbound = outbound.unwrap();
        let mut inbound = inbound.unwrap();
        let mut events = inbound.events().unwrap();
        while events.try_recv().is_some() {}
        let original = std::ptr::from_ref(inbound.media()) as usize;
        let remote_port = outbound.media().local_addr().port();
        let changed_port = if remote_port == 45000 { 45002 } else { 45000 };
        let rejected = reoffer(&inbound, &method, 99, changed_port);
        assert_eq!(
            exchange(&caller, &local, &mut incoming, &mut inbound, rejected).await,
            488
        );
        assert!(!inbound.is_on_hold());
        assert!(!matches!(
            events.try_recv(),
            Some(CallEvent::Hold | CallEvent::Resumed)
        ));
        assert_eq!(std::ptr::from_ref(inbound.media()) as usize, original);
        let accepted = reoffer(&inbound, &method, 100, remote_port);
        assert_eq!(
            exchange(&caller, &local, &mut incoming, &mut inbound, accepted).await,
            200
        );
        assert!(inbound.is_on_hold());
        assert!(matches!(events.try_recv(), Some(CallEvent::Hold)));
        assert!(events.try_recv().is_none());
        assert!(!inbound.is_ended());
        assert_eq!(std::ptr::from_ref(inbound.media()) as usize, original);
        assert_eq!(inbound.media().local_addr().port(), port);
        inbound.media().shutdown().await;
        outbound.media().shutdown().await;
        drop((inbound, outbound));
        caller.shutdown().await;
        local.shutdown().await;
    }
}

#[tokio::test]
async fn offerless_available_range_answers_prack_with_actual_bounded_media() {
    let reservation = MediaPort::bind((ip(), 0).into()).await.unwrap();
    let port = reservation.local_addr().port();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    let peer_reservation = MediaPort::bind((ip(), 0).into()).await.unwrap();
    let peer_port = peer_reservation.local_addr().port();
    let peer_range = MediaPortRange::new(peer_port, peer_port + 1).unwrap();
    let (caller, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let (peer, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let options = DialOptions::new("<sip:caller@example.test>", ip()).with_media_port_range(range);
    let endpoint = caller.clone();
    let target = Target::udp(peer.local_addr());
    let calling =
        tokio::spawn(
            async move { dial_early_without_offer(&endpoint, target, &uri(), &options).await },
        );
    let invite = tokio::time::timeout(Duration::from_secs(3), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(invite.request.body().is_empty());
    drop((reservation, peer_reservation));
    let mut ringing = ring_offer_early_with_policy_at(
        &peer,
        &invite,
        183,
        "Session Progress",
        MediaAddress::new(ip()).with_port_range(peer_range),
        Direction::SendRecv,
        MediaPolicy::default(),
    )
    .await
    .unwrap();
    let prack = tokio::time::timeout(Duration::from_secs(3), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prack.request.method, Method::Prack);
    assert!(!prack.request.body().is_empty());
    assert!(ringing.on_prack(&prack).await.unwrap());
    let dialing = tokio::time::timeout(Duration::from_secs(3), calling)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(dialing.media().unwrap().local_addr().port(), port);
    assert_eq!(ringing.media().unwrap().local_addr().port(), peer_port);
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(dialing.cancel(), async {
            let cancel = incoming.recv().await.unwrap();
            assert_eq!(cancel.request.method, Method::Cancel);
            let reply = sipx_sip::ResponseBuilder::to_request(
                &cancel.request,
                sipx_sip::StatusCode::new(200).unwrap(),
                "OK",
            )
            .unwrap()
            .build();
            peer.respond(&cancel.key, reply).await.unwrap();
            let reply = sipx_sip::ResponseBuilder::to_request(
                &invite.request,
                sipx_sip::StatusCode::new(487).unwrap(),
                "Request Terminated",
            )
            .unwrap()
            .build();
            peer.respond(&invite.key, reply).await.unwrap();
        });
    })
    .await
    .unwrap();
    drop(ringing);
    caller.shutdown().await;
    peer.shutdown().await;
}
