//! Public-path conformance probes for the opt-in media port policy.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sipx_call::{DialOptions, dial_early_without_offer, ring_offer_early};
use sipx_media::{MediaPort, MediaPortRange};
use sipx_sdp::Direction;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

fn ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}

#[tokio::test]
async fn replacement_exhaustion_refuses_without_changing_hold_or_dialog() {
    use bytes::Bytes;
    use sipx_call::{MediaAddress, answer_at, dial};
    use sipx_sip::{HeaderName, Method};

    let reservation = MediaPort::bind((ip(), 0).into()).await.unwrap();
    let port = reservation.local_addr().port();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    let (caller, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let (peer, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let uri = Uri::sip(Host::Name(HostName::new("peer.test").unwrap()));
    let options = DialOptions::new("<sip:caller@example.test>", ip());
    drop(reservation);
    let (outbound, inbound) = tokio::join!(
        dial(&caller, Target::udp(peer.local_addr()), &uri, &options),
        async {
            let invite = incoming.recv().await.unwrap();
            answer_at(
                &peer,
                &invite,
                MediaAddress::new(ip()).with_port_range(range),
            )
            .await
        }
    );
    let outbound = outbound.unwrap();
    let mut inbound = inbound.unwrap();
    assert!(!inbound.is_on_hold());
    assert_eq!(inbound.media().local_addr().port(), port);
    let original = std::ptr::from_ref(inbound.media()) as usize;
    let (to, from) = inbound.dialog.local_and_remote();
    let offer = format!(
        "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=rtcp-mux\r\na=sendonly\r\n",
        if outbound.media().local_addr().port() == 45000 {
            45002
        } else {
            45000
        }
    );
    let request = sipx_sip::build::RequestBuilder::new(Method::Invite, uri)
        .header(HeaderName::To, Bytes::from(to))
        .unwrap()
        .header(HeaderName::From, Bytes::from(from))
        .unwrap()
        .header(
            HeaderName::CallId,
            Bytes::from(inbound.dialog.id.call_id.clone()),
        )
        .unwrap()
        .cseq(99, &Method::Invite)
        .unwrap()
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )
        .unwrap()
        .max_forwards(70)
        .body(Bytes::from(offer))
        .build();
    let mut responses = caller
        .send(request, Target::udp(peer.local_addr()))
        .await
        .unwrap();
    let handling = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let request = incoming.recv().await.unwrap();
            let result = inbound.handle(&request).await;
            if request.request.method == Method::Invite {
                break result;
            }
            result.unwrap();
        }
    })
    .await
    .unwrap();
    let hold = inbound.is_on_hold();
    let same_media = original == std::ptr::from_ref(inbound.media()) as usize;
    let ended = inbound.is_ended();
    let response =
        tokio::time::timeout(Duration::from_millis(250), responses.final_response()).await;
    let status = response
        .ok()
        .flatten()
        .map(|response| response.status.code());
    inbound.media().shutdown().await;
    outbound.media().shutdown().await;
    drop((inbound, outbound));
    caller.shutdown().await;
    peer.shutdown().await;
    assert!(
        handling.is_ok() && !hold && same_media && !ended && status == Some(488),
        "capacity refusal must be 488 with unchanged call: handle={handling:?}, hold={hold}, same_media={same_media}, ended={ended}, status={status:?}"
    );
}

#[tokio::test]
async fn offerless_provisional_cannot_escape_an_exhausted_range() {
    let reservation = MediaPort::bind((ip(), 0).into()).await.unwrap();
    let port = reservation.local_addr().port();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    let (caller, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let (peer, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let options = DialOptions::new("<sip:caller@example.test>", ip()).with_media_port_range(range);
    let uri = Uri::sip(Host::Name(HostName::new("peer.test").unwrap()));
    let target = Target::udp(peer.local_addr());
    let calling_endpoint = caller.clone();
    let mut calling = tokio::spawn(async move {
        dial_early_without_offer(&calling_endpoint, target, &uri, &options).await
    });
    let invite = tokio::time::timeout(Duration::from_secs(3), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(invite.request.body().is_empty());
    let _ringing = ring_offer_early(
        &peer,
        &invite,
        183,
        "Session Progress",
        ip(),
        Direction::SendRecv,
    )
    .await
    .unwrap();
    let mut cancelled = false;
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            tokio::select! {
                result = &mut calling => break result.unwrap(),
                request = incoming.recv() => {
                    let request = request.unwrap();
                    let (status, reason) = match request.request.method {
                        sipx_sip::Method::Prack => (400, "Missing SDP Answer"),
                        sipx_sip::Method::Cancel => {
                            cancelled = true;
                            (200, "OK")
                        }
                        _ => panic!("unexpected request during allocation refusal"),
                    };
                    // No local capacity means there is no SDP answer to accept in PRACK.
                    // Refuse it, then complete the caller's CANCEL with a final 487.
                    let reply = sipx_sip::ResponseBuilder::to_request(
                        &request.request,
                        sipx_sip::StatusCode::new(status).unwrap(),
                        reason,
                    ).unwrap().build();
                    peer.respond(&request.key, reply).await.unwrap();
                    if cancelled {
                        let terminated = sipx_sip::ResponseBuilder::to_request(
                            &invite.request,
                            sipx_sip::StatusCode::new(487).unwrap(),
                            "Request Terminated",
                        ).unwrap().build();
                        peer.respond(&invite.key, terminated).await.unwrap();
                    }
                }
            }
        }
    })
    .await
    .unwrap();
    let observed = match result {
        Err(sipx_call::Error::Io(error)) if error.kind() == std::io::ErrorKind::AddrInUse => None,
        Err(error) => Some(format!("unexpected error: {error}")),
        Ok(dialing) => {
            let actual = dialing.media().unwrap().local_addr().port();
            dialing.cancel().await;
            Some(format!(
                "allocated RTP port {actual} outside exhausted {port}..={}",
                port + 1
            ))
        }
    };
    caller.shutdown().await;
    peer.shutdown().await;
    drop(reservation);
    assert_eq!(
        observed, None,
        "offerless early media must honor the configured range"
    );
    assert!(cancelled, "allocation refusal must withdraw the invitation");
}

#[tokio::test]
async fn update_exhaustion_refuses_without_changing_hold_or_dialog() {
    use bytes::Bytes;
    use sipx_call::{MediaAddress, answer_at, dial};
    use sipx_sip::{HeaderName, Method};

    let reservation = MediaPort::bind((ip(), 0).into()).await.unwrap();
    let port = reservation.local_addr().port();
    let range = MediaPortRange::new(port, port + 1).unwrap();
    let (caller, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let (peer, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let uri = Uri::sip(Host::Name(HostName::new("peer.test").unwrap()));
    let options = DialOptions::new("<sip:caller@example.test>", ip());
    drop(reservation);
    let (outbound, inbound) = tokio::join!(
        dial(&caller, Target::udp(peer.local_addr()), &uri, &options),
        async {
            let invite = incoming.recv().await.unwrap();
            answer_at(
                &peer,
                &invite,
                MediaAddress::new(ip()).with_port_range(range),
            )
            .await
        }
    );
    let outbound = outbound.unwrap();
    let mut inbound = inbound.unwrap();
    assert!(!inbound.is_on_hold());
    assert_eq!(inbound.media().local_addr().port(), port);
    let original = std::ptr::from_ref(inbound.media()) as usize;
    let (to, from) = inbound.dialog.local_and_remote();
    let offer = format!(
        "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=rtcp-mux\r\na=sendonly\r\n",
        if outbound.media().local_addr().port() == 45000 {
            45002
        } else {
            45000
        }
    );
    let request = sipx_sip::build::RequestBuilder::new(Method::Update, uri)
        .header(HeaderName::To, Bytes::from(to))
        .unwrap()
        .header(HeaderName::From, Bytes::from(from))
        .unwrap()
        .header(
            HeaderName::CallId,
            Bytes::from(inbound.dialog.id.call_id.clone()),
        )
        .unwrap()
        .cseq(99, &Method::Update)
        .unwrap()
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )
        .unwrap()
        .max_forwards(70)
        .body(Bytes::from(offer))
        .build();
    let mut responses = caller
        .send(request, Target::udp(peer.local_addr()))
        .await
        .unwrap();
    let handling = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let request = incoming.recv().await.unwrap();
            let result = inbound.handle(&request).await;
            if request.request.method == Method::Update {
                break result;
            }
            result.unwrap();
        }
    })
    .await
    .unwrap();
    let hold = inbound.is_on_hold();
    let same_media = original == std::ptr::from_ref(inbound.media()) as usize;
    let ended = inbound.is_ended();
    let response =
        tokio::time::timeout(Duration::from_millis(250), responses.final_response()).await;
    let status = response
        .ok()
        .flatten()
        .map(|response| response.status.code());
    inbound.media().shutdown().await;
    outbound.media().shutdown().await;
    drop((inbound, outbound));
    caller.shutdown().await;
    peer.shutdown().await;
    assert!(
        handling.is_ok() && !hold && same_media && !ended && status == Some(488),
        "capacity refusal must be 488 with unchanged call: handle={handling:?}, hold={hold}, same_media={same_media}, ended={ended}, status={status:?}"
    );
}
