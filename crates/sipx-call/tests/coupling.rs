//! Two dialogs driven as one call (`C-1`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::future::{Future, poll_fn};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{
    Calls, Coupling, CouplingEnd, DialOptions, Dispatched, Dispatcher, EarlyCoupling, Error, Leg,
    dial, dial_early, dial_early_without_offer, ring, ring_early, ring_offer_early,
};
use sipx_sdp::{Capabilities, Direction};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::rel::RSeq;
use sipx_sip::{HeaderName, Host, HostName, Method, Request, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, Receiver};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

struct Pumped {
    surfaced: Receiver<Dispatched>,
    calls: Calls,
}

fn pump(endpoint: &Handle, incoming: Receiver<Incoming>) -> Pumped {
    let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
    let calls = dispatcher.calls();
    let (tx, surfaced) = mpsc::channel(4);
    tokio::spawn(async move {
        while let Some(dispatched) = dispatcher.next().await {
            if tx.send(dispatched).await.is_err() {
                return;
            }
        }
    });
    Pumped { surfaced, calls }
}

impl Pumped {
    async fn invitation(&mut self) -> sipx_call::Invitation {
        match tokio::time::timeout(Duration::from_secs(10), self.surfaced.recv())
            .await
            .expect("an invitation arrives")
            .expect("the dispatcher remains running")
        {
            Dispatched::Invitation(invitation) => invitation,
            other => panic!("expected an invitation, got {other:?}"),
        }
    }
}

async fn leg(
    edge: &Handle,
    pumped: &mut Pumped,
    identity: &'static str,
) -> (
    sipx_call::Call,
    Receiver<Incoming>,
    sipx_call::Call,
    Receiver<Incoming>,
    Handle,
) {
    let (peer_endpoint, peer_incoming) = endpoint().await;
    let peer_handle = peer_endpoint.clone();
    let target = Target::udp(edge.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("edge.example").expect("valid")));
    let dialling = tokio::spawn(async move {
        let call = dial(
            &peer_endpoint,
            target,
            &to,
            &DialOptions::new(identity, loopback()),
        )
        .await
        .expect("the edge answers");
        (call, peer_incoming)
    });

    let invitation = pumped.invitation().await;
    let edge_call = invitation
        .answer(edge, loopback())
        .await
        .expect("answers the leg");
    let (_invite, edge_incoming) = invitation.into_parts();
    let (peer_call, peer_incoming) = dialling.await.expect("the dial task finishes");
    (
        edge_call,
        edge_incoming,
        peer_call,
        peer_incoming,
        peer_handle,
    )
}

fn offer_sdp(payload: u8, encoding: &str) -> String {
    format!(
        "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio 45000 RTP/AVP {payload}\r\na=rtpmap:{payload} {encoding}/8000\r\na=sendrecv\r\n"
    )
}

fn peer_request(
    peer: &sipx_call::Call,
    endpoint: &Handle,
    method: &Method,
    cseq: u32,
    body: Option<&str>,
    wrong_local_tag: bool,
) -> Request {
    let (from, mut to) = peer.dialog.local_and_remote();
    if wrong_local_tag {
        "<sip:edge.example>;tag=not-this-dialog".clone_into(&mut to);
    }
    let via = Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ));
    let mut builder = RequestBuilder::new(method.clone(), peer.dialog.remote_target.clone())
        .header(HeaderName::Via, via)
        .expect("valid Via")
        .header(HeaderName::To, Bytes::from(to))
        .expect("valid To")
        .header(HeaderName::From, Bytes::from(from))
        .expect("valid From")
        .header(
            HeaderName::CallId,
            Bytes::from(peer.dialog.id.call_id.clone()),
        )
        .expect("valid Call-ID")
        .cseq(cseq, method)
        .expect("valid CSeq")
        .max_forwards(70);
    if let Some(body) = body {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )
            .expect("valid Content-Type")
            .body(Bytes::from(body.to_owned()));
    }
    builder.build()
}

fn offerless_invite(endpoint: &Handle, call_id: &'static str) -> Request {
    RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("edge.example").unwrap())),
    )
    .header(
        HeaderName::Via,
        Bytes::from(format!(
            "SIP/2.0/UDP {};rport;branch={}",
            endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
            sipx_transport::new_branch()
        )),
    )
    .unwrap()
    .header(HeaderName::To, "<sip:edge.example>")
    .unwrap()
    .header(HeaderName::From, "<sip:caller@example.net>;tag=source")
    .unwrap()
    .header(HeaderName::CallId, call_id)
    .unwrap()
    .cseq(1, &Method::Invite)
    .unwrap()
    .header(HeaderName::Supported, "100rel")
    .unwrap()
    .header(
        HeaderName::Contact,
        format!("<sip:caller@{}>", endpoint.local_addr()),
    )
    .unwrap()
    .max_forwards(70)
    .build()
}

fn prack_answer(endpoint: &Handle, invite: &Request, provisional: &sipx_sip::Response) -> Request {
    let rseq = provisional.headers.typed::<RSeq>().unwrap().unwrap().0;
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(provisional.body())).unwrap();
    let answer = sipx_sdp::answer(&offer, &Capabilities::g711(loopback(), 46_000));
    RequestBuilder::new(Method::Prack, invite.uri.clone())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
                sipx_transport::new_branch()
            )),
        )
        .unwrap()
        .header(
            HeaderName::To,
            Bytes::from(
                provisional
                    .headers
                    .value(&HeaderName::To)
                    .unwrap()
                    .into_owned(),
            ),
        )
        .unwrap()
        .header(
            HeaderName::From,
            Bytes::from(
                invite
                    .headers
                    .value(&HeaderName::From)
                    .unwrap()
                    .into_owned(),
            ),
        )
        .unwrap()
        .header(
            HeaderName::CallId,
            Bytes::from(
                invite
                    .headers
                    .value(&HeaderName::CallId)
                    .unwrap()
                    .into_owned(),
            ),
        )
        .unwrap()
        .cseq(2, &Method::Prack)
        .unwrap()
        .header(HeaderName::RAck, format!("{rseq} 1 INVITE"))
        .unwrap()
        .header(HeaderName::ContentType, "application/sdp")
        .unwrap()
        .body(Bytes::from(answer.to_string_sdp()))
        .max_forwards(70)
        .build()
}

fn cancel_for(invite: &Request) -> Request {
    let value = |name: &HeaderName| Bytes::from(invite.headers.value(name).unwrap().into_owned());
    RequestBuilder::new(Method::Cancel, invite.uri.clone())
        .header(HeaderName::Via, value(&HeaderName::Via))
        .unwrap()
        .header(HeaderName::To, value(&HeaderName::To))
        .unwrap()
        .header(HeaderName::From, value(&HeaderName::From))
        .unwrap()
        .header(HeaderName::CallId, value(&HeaderName::CallId))
        .unwrap()
        .cseq(1, &Method::Cancel)
        .unwrap()
        .max_forwards(70)
        .build()
}

async fn next_reliable_provisional(
    responses: &mut sipx_transport::Responses,
) -> sipx_sip::Response {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(sipx_sip::transaction::TuEvent::Response(response)) = responses.next().await
                && response.status.is_provisional()
                && response.headers.typed::<RSeq>().is_some()
            {
                return *response;
            }
        }
    })
    .await
    .expect("a reliable provisional arrives")
}

async fn delayed_relay(
    call_id: &'static str,
) -> (
    Handle,
    SocketAddr,
    Request,
    sipx_transport::Responses,
    sipx_call::Ringing,
    Receiver<Incoming>,
    sipx_call::CallEvents,
    EarlyCoupling,
) {
    let (edge, edge_incoming) = endpoint().await;
    let edge_addr = edge.local_addr();
    let mut edge_pumped = pump(&edge, edge_incoming);
    let (source, _source_incoming) = endpoint().await;
    let source_invite = offerless_invite(&source, call_id);
    let source_responses = source
        .send(source_invite.clone(), Target::udp(edge_addr))
        .await
        .expect("the offerless source INVITE leaves");
    let inbound = edge_pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let edge_for_coupling = edge.clone();
    let calls = edge_pumped.calls.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let outbound_to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(EarlyCoupling::dial(
            inbound,
            &calls,
            &edge_for_coupling,
            target,
            &outbound_to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
            loopback(),
        ))
        .await
        .expect("the coupling reaches the target early dialog")
    });

    let mut target_invitation = target_pumped.invitation().await;
    assert!(target_invitation.request().request.body().is_empty());
    let target_ringing = ring_offer_early(
        &target_endpoint,
        target_invitation.request(),
        183,
        "Session Progress",
        loopback(),
        Direction::SendRecv,
    )
    .await
    .expect("the target sends its offer reliably");
    let target_events = target_invitation
        .events()
        .expect("the target observes source cancellation");
    let (_target_invite, target_requests) = target_invitation.into_parts();
    let early = coupling_task.await.expect("the coupling task finishes");

    (
        source,
        edge_addr,
        source_invite,
        source_responses,
        target_ringing,
        target_requests,
        target_events,
        early,
    )
}

fn accepted_target_response(
    incoming: &Incoming,
    target_endpoint: &Handle,
    tag: &str,
) -> sipx_sip::Response {
    let original_to = incoming
        .request
        .headers
        .value(&HeaderName::To)
        .expect("the target INVITE has To");
    ResponseBuilder::to_request(
        &incoming.request,
        StatusCode::new(200).expect("valid status"),
        "OK",
    )
    .expect("the target final response builds")
    .set_header(
        &HeaderName::To,
        Bytes::from(format!(
            "{};tag={tag}",
            String::from_utf8_lossy(&original_to)
        )),
    )
    .expect("the target dialog tag is valid")
    .header(
        HeaderName::Contact,
        Bytes::from(format!("<sip:callee@{}>", target_endpoint.local_addr())),
    )
    .expect("the target Contact is valid")
    .build()
}

async fn rejected_offer_does_not_reach_peer(
    body: &str,
    wrong_local_tag: bool,
    expected_status: u16,
) {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let (edge_one, mut edge_one_incoming, peer_one, _peer_one_incoming, peer_one_endpoint) =
        leg(&edge, &mut pumped, "<sip:one@example.net>").await;
    let (edge_two, mut edge_two_incoming, mut peer_two, mut peer_two_incoming, _peer_two_endpoint) =
        leg(&edge, &mut pumped, "<sip:two@example.net>").await;
    let mut coupling = Coupling::new(edge_one, edge_two);

    let probe = peer_request(
        &peer_one,
        &peer_one_endpoint,
        &Method::Invite,
        99,
        Some(body),
        wrong_local_tag,
    );
    let bye = peer_request(
        &peer_one,
        &peer_one_endpoint,
        &Method::Bye,
        100,
        None,
        false,
    );
    let edge_addr = edge.local_addr();

    let (coupled, ()) = tokio::join!(
        coupling.run(&mut edge_one_incoming, &mut edge_two_incoming),
        async {
            let mut responses = peer_one_endpoint
                .send(probe, Target::udp(edge_addr))
                .await
                .expect("sends the source offer");
            // A bound on failure: the wire response is the barrier, not the duration.
            let refused = tokio::time::timeout(Duration::from_secs(10), responses.final_response())
                .await
                .expect("the source offer reaches a final response within the failure bound")
                .expect("the source offer receives a final response");
            assert_eq!(refused.status.code(), expected_status);
            assert!(
                matches!(
                    peer_two_incoming.try_recv(),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                ),
                "the rejected source offer changed the far leg"
            );

            let mut bye_responses = peer_one_endpoint
                .send(bye, Target::udp(edge_addr))
                .await
                .expect("sends the source BYE");
            let (bye_answer, peer_bye) =
                tokio::join!(bye_responses.final_response(), peer_two_incoming.recv(),);
            assert_eq!(
                bye_answer
                    .expect("the source BYE is answered")
                    .status
                    .code(),
                200
            );
            let peer_bye = peer_bye.expect("the far leg receives its BYE");
            assert_eq!(peer_bye.request.method, Method::Bye);
            assert!(
                peer_two
                    .handle(&peer_bye)
                    .await
                    .expect("handles the far-leg BYE")
            );
        }
    );

    assert_eq!(
        coupled.expect("the coupling ends cleanly"),
        CouplingEnd::Bye(Leg::One)
    );
}

#[tokio::test]
async fn a_wrong_dialog_offer_never_reaches_the_far_leg() {
    Box::pin(rejected_offer_does_not_reach_peer(
        &offer_sdp(0, "PCMU"),
        true,
        481,
    ))
    .await;
}

#[tokio::test]
async fn a_malformed_offer_never_reaches_the_far_leg() {
    Box::pin(rejected_offer_does_not_reach_peer(
        "this is not SDP\r\n",
        false,
        488,
    ))
    .await;
}

#[tokio::test]
async fn a_valid_unnegotiable_offer_never_reaches_the_far_leg() {
    Box::pin(rejected_offer_does_not_reach_peer(
        &offer_sdp(9, "G722"),
        false,
        488,
    ))
    .await;
}

async fn handle_early_prack_and_update(
    mut ringing: sipx_call::Ringing,
    mut requests: Receiver<Incoming>,
) {
    loop {
        let incoming = requests.recv().await.expect("the PRACK or UPDATE arrives");
        match incoming.request.method {
            Method::Prack => {
                assert!(
                    ringing
                        .on_prack(&incoming)
                        .await
                        .expect("answers the PRACK")
                );
            }
            Method::Update => {
                assert!(
                    String::from_utf8_lossy(incoming.request.body()).contains("a=sendonly"),
                    "the source send-only flow is preserved on the target leg"
                );
                assert!(
                    ringing
                        .on_update(&incoming)
                        .await
                        .expect("answers the early UPDATE")
                );
                return;
            }
            ref method => panic!("unexpected early method {method}"),
        }
    }
}

/// C-1's failing-first acceptance test: ending one dialog ends the peer dialog, not only its
/// media worker. The audio assertion before it is the positive control for the optional bridge.
#[tokio::test]
async fn a_bye_on_one_leg_ends_the_other() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let (edge_one, mut edge_one_incoming, mut peer_one, _peer_one_incoming, _peer_one_endpoint) =
        leg(&edge, &mut pumped, "<sip:one@example.net>").await;
    let (edge_two, mut edge_two_incoming, mut peer_two, mut peer_two_incoming, _peer_two_endpoint) =
        leg(&edge, &mut pumped, "<sip:two@example.net>").await;

    let mut coupling = Coupling::new(edge_one, edge_two);
    assert!(
        !coupling.has_media_bridge(),
        "signalling coupling does not silently put itself on the media path"
    );
    assert!(
        !coupling.bridge_media(),
        "both default legs negotiated PCMU"
    );

    let samples = vec![8_000; 1_600];
    let heard = tokio::join!(
        peer_one.media().play(&samples, 160),
        peer_two
            .media()
            .record_at_least(samples.len(), Duration::from_secs(10))
    )
    .1;
    assert_eq!(
        heard.len(),
        samples.len(),
        "the attached bridge carries audio"
    );

    let (coupled_end, peer_one_end, peer_two_end) = tokio::join!(
        coupling.run(&mut edge_one_incoming, &mut edge_two_incoming),
        async {
            peer_one
                .reinvite(Direction::SendOnly)
                .await
                .expect("the relayed answer returns on the re-INVITE axis");
            assert!(
                peer_one.media().play(&vec![1_700; 8_000], 160).await,
                "the directional source finishes its audio"
            );
            peer_one.hang_up().await
        },
        async {
            let mut saw_reinvite = false;
            loop {
                let incoming =
                    tokio::time::timeout(Duration::from_secs(10), peer_two_incoming.recv())
                        .await
                        .expect("the peer BYE arrives")
                        .expect("the peer inbox remains open");
                if incoming.request.method == Method::Invite {
                    assert!(
                        String::from_utf8_lossy(incoming.request.body()).contains("a=sendonly"),
                        "the first leg's send-only flow is preserved on the peer leg"
                    );
                    saw_reinvite = true;
                }
                if incoming.request.method == Method::Bye {
                    assert!(
                        peer_two
                            .handle(&incoming)
                            .await
                            .expect("handles the peer BYE")
                    );
                    return saw_reinvite;
                }
                assert!(
                    peer_two
                        .handle(&incoming)
                        .await
                        .expect("handles its dialog request")
                );
                if incoming.request.method == Method::Invite {
                    let heard = peer_two
                        .media()
                        .record_at_least(1_600, Duration::from_secs(10))
                        .await;
                    assert!(
                        heard.len() >= 1_600,
                        "audio follows the preserved send-only direction through the bridge"
                    );
                }
            }
        }
    );

    peer_one_end.expect("the first BYE is answered");
    assert_eq!(
        coupled_end.expect("the coupling ends cleanly"),
        CouplingEnd::Bye(Leg::One)
    );
    assert!(peer_two.is_ended(), "the second dialog received a BYE");
    assert!(peer_two_end, "the offer crossed to the peer before the BYE");
}

/// The confirmed driver must keep polling the leg on which its relayed offer is outstanding. If
/// it awaited inline, the crossed request would sit in the inbox until there was no glare left and
/// would incorrectly cross instead of receiving 491.
#[tokio::test]
async fn glare_gets_a_live_491_and_the_peers_fresh_retry_is_relayed() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let (edge_one, mut edge_one_incoming, mut peer_one, mut peer_one_incoming, _peer_one_endpoint) =
        leg(&edge, &mut pumped, "<sip:one@example.net>").await;
    let (edge_two, mut edge_two_incoming, mut peer_two, mut peer_two_incoming, _peer_two_endpoint) =
        leg(&edge, &mut pumped, "<sip:two@example.net>").await;
    let mut coupling = Coupling::new(edge_one, edge_two);
    let first_exchange_settled = Arc::new(Notify::new());

    let (coupled_end, peer_one_end, peer_two_end) = tokio::join!(
        coupling.run(&mut edge_one_incoming, &mut edge_two_incoming),
        {
            let first_exchange_settled = Arc::clone(&first_exchange_settled);
            async move {
                let first = tokio::time::timeout(Duration::from_secs(10), peer_one_incoming.recv())
                    .await
                    .expect("the first relayed offer arrives")
                    .expect("the first peer inbox remains open");
                assert_eq!(first.request.method, Method::Invite);

                let crossed = peer_one.reinvite(Direction::RecvOnly).await;
                assert!(
                    matches!(crossed, Err(Error::Rejected { status: 491, .. })),
                    "the crossed offer receives 491 while the first is still outstanding: {crossed:?}"
                );

                assert!(
                    peer_one
                        .handle(&first)
                        .await
                        .expect("answers the first relayed offer")
                );
                first_exchange_settled.notified().await;

                peer_one
                    .reinvite(Direction::RecvOnly)
                    .await
                    .expect("the UAC's fresh retry crosses after settlement");
                peer_one.hang_up().await
            }
        },
        {
            let first_exchange_settled = Arc::clone(&first_exchange_settled);
            async move {
                peer_two
                    .reinvite(Direction::SendOnly)
                    .await
                    .expect("the first exchange completes");
                first_exchange_settled.notify_one();

                let mut saw_retry = false;
                loop {
                    let incoming =
                        tokio::time::timeout(Duration::from_secs(10), peer_two_incoming.recv())
                            .await
                            .expect("the retried offer or BYE arrives")
                            .expect("the second peer inbox remains open");
                    if incoming.request.method == Method::Invite {
                        saw_retry = true;
                    }
                    assert!(
                        peer_two
                            .handle(&incoming)
                            .await
                            .expect("handles the relayed request")
                    );
                    if incoming.request.method == Method::Bye {
                        return saw_retry;
                    }
                }
            }
        }
    );

    peer_one_end.expect("the retry completes and the first peer ends");
    assert_eq!(
        coupled_end.expect("the coupling ends cleanly"),
        CouplingEnd::Bye(Leg::One)
    );
    assert!(peer_two_end, "the fresh retry was relayed before the BYE");
}

#[tokio::test]
async fn an_early_cancel_withdraws_the_owned_outbound_invitation() {
    let (edge, edge_incoming) = endpoint().await;
    let mut edge_pumped = pump(&edge, edge_incoming);
    let (source_endpoint, source_incoming) = endpoint().await;
    let edge_target = Target::udp(edge.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("edge.example").expect("valid")));
    let caller_task = tokio::spawn(async move {
        let dialing = dial_early(
            &source_endpoint,
            edge_target,
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
        .expect("the coupled inbound leg rings");
        dialing.cancel().await;
        source_incoming
    });
    let inbound = edge_pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut callee_pumped = pump(&target_endpoint, target_incoming);
    let callee_target = Target::udp(target_endpoint.local_addr());
    let outbound_to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let edge_for_dial = edge.clone();
    let outbound_task = tokio::spawn(async move {
        dial_early(
            &edge_for_dial,
            callee_target,
            &outbound_to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
        )
        .await
        .expect("the outbound leg rings")
    });
    let mut outbound_invitation = callee_pumped.invitation().await;
    let _outbound_ringing = ring(
        &target_endpoint,
        outbound_invitation.request(),
        180,
        "Ringing",
        false,
    )
    .await
    .expect("sends an unreliable provisional");
    let mut outbound_events = outbound_invitation
        .events()
        .expect("the cancellation stream is available");
    let outbound = outbound_task.await.expect("the outbound task finishes");
    let outbound_incoming = edge_pumped.calls.register(
        outbound
            .dialog()
            .expect("the provisional established an early dialog"),
    );

    let inbound_ringing = ring(&edge, inbound.request(), 180, "Ringing", true)
        .await
        .expect("sends a reliable inbound provisional");
    let early = EarlyCoupling::new(
        inbound,
        inbound_ringing,
        outbound,
        outbound_incoming,
        &edge,
        loopback(),
    );

    let (coupled, caller_done, callee_cancelled) =
        tokio::join!(early.confirmed(), caller_task, outbound_events.recv());
    let _caller_incoming = caller_done.expect("the caller task finishes");
    assert!(matches!(coupled, Err(Error::InvitationCancelled)));
    assert!(matches!(
        callee_cancelled,
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert!(
        outbound_invitation.is_cancelled(),
        "the coupled outbound INVITE received CANCEL"
    );
}

#[tokio::test]
async fn an_outbound_final_failure_is_the_inbound_final_response() {
    let (edge, edge_incoming) = endpoint().await;
    let mut edge_pumped = pump(&edge, edge_incoming);
    let (source_endpoint, _source_incoming) = endpoint().await;
    let edge_target = Target::udp(edge.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("edge.example").expect("valid")));
    let caller_task = tokio::spawn(async move {
        dial(
            &source_endpoint,
            edge_target,
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
    });
    let inbound = edge_pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut callee_pumped = pump(&target_endpoint, target_incoming);
    let callee_target = Target::udp(target_endpoint.local_addr());
    let outbound_to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let edge_for_dial = edge.clone();
    let outbound_task = tokio::spawn(async move {
        dial_early(
            &edge_for_dial,
            callee_target,
            &outbound_to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
        )
        .await
        .expect("the outbound leg rings")
    });
    let outbound_invitation = callee_pumped.invitation().await;
    let _outbound_ringing = ring(
        &target_endpoint,
        outbound_invitation.request(),
        180,
        "Ringing",
        false,
    )
    .await
    .expect("sends an unreliable provisional");
    let outbound = outbound_task.await.expect("the outbound task finishes");
    let outbound_incoming = edge_pumped.calls.register(
        outbound
            .dialog()
            .expect("the provisional established an early dialog"),
    );
    let inbound_ringing = ring(&edge, inbound.request(), 180, "Ringing", false)
        .await
        .expect("rings the inbound leg");
    let early = EarlyCoupling::new(
        inbound,
        inbound_ringing,
        outbound,
        outbound_incoming,
        &edge,
        loopback(),
    );

    let (coupled, refused, caller_result) = tokio::join!(
        early.confirmed(),
        outbound_invitation.refuse(&target_endpoint, 486, "Busy Here"),
        caller_task,
    );
    refused.expect("the outbound refusal is sent");
    assert!(matches!(coupled, Err(Error::Rejected { status: 486, .. })));
    assert!(matches!(
        caller_result.expect("the caller task finishes"),
        Err(Error::Rejected { status: 486, .. })
    ));
}

#[tokio::test]
async fn an_early_update_offer_crosses_after_both_reliable_provisionals_are_pracked() {
    let (edge, edge_incoming) = endpoint().await;
    let mut edge_pumped = pump(&edge, edge_incoming);
    let (source_endpoint, _source_incoming) = endpoint().await;
    let edge_target = Target::udp(edge.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("edge.example").expect("valid")));
    let source_task = tokio::spawn(async move {
        let mut dialing = dial_early(
            &source_endpoint,
            edge_target,
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
        .expect("the inbound reliable provisional is PRACKed");
        assert!(dialing.has_early_session());
        dialing
            .update(Direction::SendOnly)
            .await
            .expect("the early UPDATE answer returns through the coupling");
        dialing.cancel().await;
    });
    let inbound = edge_pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let target = Target::udp(target_endpoint.local_addr());
    let outbound_to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let edge_for_dial = edge.clone();
    let outbound_task = tokio::spawn(async move {
        dial_early(
            &edge_for_dial,
            target,
            &outbound_to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
        )
        .await
        .expect("the outbound reliable provisional is PRACKed")
    });
    let outbound_invitation = target_pumped.invitation().await;
    let target_ringing = ring_early(
        &target_endpoint,
        outbound_invitation.request(),
        183,
        "Session Progress",
        loopback(),
    )
    .await
    .expect("answers the outbound offer reliably");
    let (_outbound_invite, target_requests) = outbound_invitation.into_parts();
    let target_handler = tokio::spawn(handle_early_prack_and_update(
        target_ringing,
        target_requests,
    ));
    let outbound = outbound_task.await.expect("the outbound task finishes");
    assert!(outbound.has_early_session());
    let outbound_incoming = edge_pumped.calls.register(
        outbound
            .dialog()
            .expect("the provisional established an early dialog"),
    );
    let inbound_ringing = ring_early(
        &edge,
        inbound.request(),
        183,
        "Session Progress",
        loopback(),
    )
    .await
    .expect("answers the inbound offer reliably");
    let early = EarlyCoupling::new(
        inbound,
        inbound_ringing,
        outbound,
        outbound_incoming,
        &edge,
        loopback(),
    );

    let (coupled, source_done, target_done) =
        tokio::join!(early.confirmed(), source_task, target_handler);
    source_done.expect("the source task finishes");
    target_done.expect("the target handles PRACK and UPDATE");
    assert!(matches!(coupled, Err(Error::InvitationCancelled)));
}

/// C-1 E2: ownership starts before the target INVITE leaves. The target sees a fresh offer whose
/// direction maps the source offer, then source cancellation is still translated to the pending
/// target leg by that same owner.
#[tokio::test]
async fn the_owning_coupling_relays_the_initial_invite_offer() {
    let (edge, edge_incoming) = endpoint().await;
    let mut edge_pumped = pump(&edge, edge_incoming);
    let (source_endpoint, _source_incoming) = endpoint().await;
    let edge_target = Target::udp(edge.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("edge.example").expect("valid")));
    let source_task = tokio::spawn(async move {
        dial_early(
            &source_endpoint,
            edge_target,
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback())
                .with_initial_direction(Direction::SendOnly),
        )
        .await
        .expect("the owning coupling rings the source")
    });
    let inbound = edge_pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let target = Target::udp(target_endpoint.local_addr());
    let outbound_to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let edge_for_coupling = edge.clone();
    let calls = edge_pumped.calls.clone();
    let coupling_task = tokio::spawn(async move {
        Box::pin(EarlyCoupling::dial(
            inbound,
            &calls,
            &edge_for_coupling,
            target,
            &outbound_to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
            loopback(),
        ))
        .await
        .expect("the coupling owns both pending legs")
    });

    let mut outbound_invitation = target_pumped.invitation().await;
    assert!(
        String::from_utf8_lossy(outbound_invitation.request().request.body())
            .contains("a=sendonly"),
        "the source send-only flow is mapped onto the target initial leg"
    );
    let _outbound_ringing = ring(
        &target_endpoint,
        outbound_invitation.request(),
        180,
        "Ringing",
        false,
    )
    .await
    .expect("the target establishes its early dialog");
    let mut outbound_events = outbound_invitation
        .events()
        .expect("the target observes cancellation");
    let early = coupling_task.await.expect("the constructor finishes");
    let coupled = tokio::spawn(early.confirmed());
    let source = source_task.await.expect("the source task finishes");

    let (ended, target_end) = tokio::join!(coupled, async move {
        source.cancel().await;
        outbound_events.recv().await
    });
    assert!(matches!(
        ended.expect("the coupling task finishes"),
        Err(Error::InvitationCancelled)
    ));
    assert!(matches!(
        target_end,
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert!(outbound_invitation.is_cancelled());
}

/// C-1 E3: a reliable provisional originates the offer and the answer returns in PRACK. Both
/// public early-dialog owners retain the negotiated media rather than merely moving SDP bytes.
#[tokio::test]
async fn a_reliable_provisional_offer_is_answered_in_prack() {
    let (answering_endpoint, mut answering_incoming) = endpoint().await;
    let (offering_endpoint, _offering_incoming) = endpoint().await;
    let target = Target::udp(answering_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let calling = tokio::spawn(async move {
        dial_early_without_offer(
            &offering_endpoint,
            target,
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
        .expect("the provisional offer is answered")
    });

    let invite = answering_incoming
        .recv()
        .await
        .expect("the offerless INVITE");
    assert!(invite.request.body().is_empty());
    let mut ringing = ring_offer_early(
        &answering_endpoint,
        &invite,
        183,
        "Session Progress",
        loopback(),
        Direction::SendRecv,
    )
    .await
    .expect("puts an offer in the reliable provisional");
    let prack = answering_incoming
        .recv()
        .await
        .expect("the answering PRACK");
    assert_eq!(prack.request.method, Method::Prack);
    assert!(
        !prack.request.body().is_empty(),
        "the answer to the provisional offer travels in PRACK"
    );
    assert!(
        ringing
            .on_prack(&prack)
            .await
            .expect("adopts the PRACK answer")
    );
    assert!(ringing.has_early_session());

    let dialing = calling.await.expect("the caller task finishes");
    assert!(dialing.has_early_session());
    assert!(dialing.media().is_some());
    assert!(ringing.media().is_some());
    dialing.cancel().await;
}

/// C-1 E3: neither leg may invent the answer to a delayed offer. The target's reliable
/// provisional offer is held at the coupling until the source answers it in PRACK; only then may
/// the corresponding target PRACK leave.
#[tokio::test]
async fn a_target_provisional_offer_crosses_both_legs_before_its_prack_leaves() {
    let (
        source,
        edge_addr,
        source_invite,
        mut source_responses,
        mut target_ringing,
        mut target_requests,
        mut target_events,
        early,
    ) = delayed_relay("coupled-delayed-offer@sipx").await;
    let coupled = tokio::spawn(early.confirmed());

    let source_provisional = next_reliable_provisional(&mut source_responses).await;
    assert!(
        !source_provisional.body().is_empty(),
        "the target offer reaches the source reliable provisional"
    );
    assert!(
        matches!(
            target_requests.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "the target PRACK waits for the source's answer"
    );

    let source_prack = prack_answer(&source, &source_invite, &source_provisional);
    let mut source_prack_responses = source
        .send(source_prack, Target::udp(edge_addr))
        .await
        .expect("the source PRACK leaves");
    let target_prack = tokio::time::timeout(Duration::from_secs(10), target_requests.recv())
        .await
        .expect("the target PRACK arrives")
        .expect("the target request inbox remains open");
    assert_eq!(target_prack.request.method, Method::Prack);
    assert!(
        !target_prack.request.body().is_empty(),
        "the source answer reaches the target PRACK"
    );
    assert!(
        target_ringing
            .on_prack(&target_prack)
            .await
            .expect("the target adopts the relayed answer")
    );
    assert!(target_ringing.has_early_session());
    let source_prack_response = tokio::time::timeout(
        Duration::from_secs(10),
        source_prack_responses.final_response(),
    )
    .await
    .expect("the source PRACK is answered")
    .expect("the source PRACK has a final response");
    assert_eq!(source_prack_response.status.code(), 200);

    let mut cancel_responses = source
        .send(cancel_for(&source_invite), Target::udp(edge_addr))
        .await
        .expect("the source CANCEL leaves");
    let (coupled_result, cancelled, cancel_response) = tokio::join!(
        coupled,
        target_events.recv(),
        cancel_responses.final_response()
    );
    assert!(matches!(
        coupled_result.expect("the coupling task finishes"),
        Err(Error::InvitationCancelled)
    ));
    assert!(matches!(
        cancelled,
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert_eq!(
        cancel_response
            .expect("the CANCEL has a final response")
            .status
            .code(),
        200
    );
}

/// With the answer still outstanding, source cancellation owns the target cleanup and cannot
/// leak the held target PRACK.
#[tokio::test]
async fn cancelling_a_delayed_offer_cancels_the_target_without_pracking_it() {
    let (
        source,
        edge_addr,
        source_invite,
        mut source_responses,
        _target_ringing,
        mut target_requests,
        mut target_events,
        early,
    ) = delayed_relay("coupled-delayed-cancel@sipx").await;
    let source_provisional = next_reliable_provisional(&mut source_responses).await;
    assert!(!source_provisional.body().is_empty());
    let coupled = tokio::spawn(early.confirmed());
    assert!(matches!(
        target_requests.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let mut cancel_responses = source
        .send(cancel_for(&source_invite), Target::udp(edge_addr))
        .await
        .expect("the source CANCEL leaves");
    let (coupled_result, target_end, cancel_response, invite_response) = tokio::join!(
        coupled,
        target_events.recv(),
        cancel_responses.final_response(),
        source_responses.final_response(),
    );
    assert!(matches!(
        coupled_result.expect("the coupling task finishes"),
        Err(Error::InvitationCancelled)
    ));
    assert!(matches!(
        target_end,
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert_eq!(cancel_response.unwrap().status.code(), 200);
    assert_eq!(invite_response.unwrap().status.code(), 487);
    assert!(
        !matches!(target_requests.try_recv(), Ok(request) if request.request.method == Method::Prack),
        "cancellation must not release the held target PRACK"
    );
}

/// A malformed source answer is refused on its own leg and tears down the still-pending target
/// invitation. The invalid bytes never become a target PRACK body.
#[tokio::test]
async fn a_malformed_source_prack_answer_refuses_and_cleans_both_pending_legs() {
    let (
        source,
        edge_addr,
        source_invite,
        mut source_responses,
        _target_ringing,
        mut target_requests,
        mut target_events,
        early,
    ) = delayed_relay("coupled-delayed-malformed@sipx").await;
    let source_provisional = next_reliable_provisional(&mut source_responses).await;
    let mut source_prack = prack_answer(&source, &source_invite, &source_provisional);
    let declared_length = source_prack.body().len();
    source_prack.set_body(Bytes::from(vec![b'x'; declared_length]));
    let coupled = tokio::spawn(early.confirmed());
    let mut prack_responses = source
        .send(source_prack, Target::udp(edge_addr))
        .await
        .expect("the malformed source PRACK leaves");

    let prack_response =
        tokio::time::timeout(Duration::from_secs(10), prack_responses.final_response())
            .await
            .expect("the malformed PRACK is answered");
    let invite_response =
        tokio::time::timeout(Duration::from_secs(10), source_responses.final_response())
            .await
            .expect("the source INVITE is refused");
    let target_end = tokio::time::timeout(Duration::from_secs(10), target_events.recv())
        .await
        .expect("the target invitation is cancelled");
    let coupled_result = tokio::time::timeout(Duration::from_secs(10), coupled)
        .await
        .expect("the coupling finishes after malformed SDP");
    assert!(matches!(
        coupled_result.expect("the coupling task finishes"),
        Err(Error::Sdp(_))
    ));
    assert!(matches!(
        target_end,
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert_eq!(prack_response.unwrap().status.code(), 488);
    assert_eq!(invite_response.unwrap().status.code(), 488);
    assert!(
        !matches!(target_requests.try_recv(), Ok(request) if request.request.method == Method::Prack),
        "malformed source SDP must not cross into a target PRACK"
    );
}

/// A target 2xx can cross the held PRACK. If the source answer then fails, the coupling still
/// owns the now-confirmed target dialog and must ACK and BYE it rather than sending a stale CANCEL
/// or abandoning the accepted response.
#[tokio::test]
async fn a_crossed_target_final_is_acked_and_ended_when_the_source_answer_fails() {
    let (edge, edge_incoming) = endpoint().await;
    let edge_addr = edge.local_addr();
    let mut edge_pumped = pump(&edge, edge_incoming);
    let (source, _source_incoming) = endpoint().await;
    let source_invite = offerless_invite(&source, "coupled-delayed-crossed-final@sipx");
    let mut source_responses = source
        .send(source_invite.clone(), Target::udp(edge_addr))
        .await
        .expect("the offerless source INVITE leaves");
    let inbound = edge_pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let edge_for_coupling = edge.clone();
    let calls = edge_pumped.calls.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let outbound_to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(EarlyCoupling::dial(
            inbound,
            &calls,
            &edge_for_coupling,
            target,
            &outbound_to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
            loopback(),
        ))
        .await
        .expect("the coupling reaches the target early dialog")
    });

    let target_invitation = target_pumped.invitation().await;
    let target_ringing = ring_offer_early(
        &target_endpoint,
        target_invitation.request(),
        183,
        "Session Progress",
        loopback(),
        Direction::SendRecv,
    )
    .await
    .expect("the target sends its offer reliably");
    let (target_invite, mut target_requests) = target_invitation.into_parts();
    let crossed_final =
        accepted_target_response(&target_invite, &target_endpoint, target_ringing.tag());
    target_endpoint
        .respond(&target_invite.key, crossed_final)
        .await
        .expect("the target final crosses the held PRACK");

    let early = coupling_task.await.expect("the coupling task finishes");
    let source_provisional = next_reliable_provisional(&mut source_responses).await;
    let mut confirmed = Box::pin(early.confirmed());
    let first_poll = poll_fn(|context| Poll::Ready(confirmed.as_mut().poll(context))).await;
    assert!(
        first_poll.is_pending(),
        "the target final is held until the source answers"
    );

    let mut source_prack = prack_answer(&source, &source_invite, &source_provisional);
    let declared_length = source_prack.body().len();
    source_prack.set_body(Bytes::from(vec![b'x'; declared_length]));
    let mut prack_responses = source
        .send(source_prack, Target::udp(edge_addr))
        .await
        .expect("the malformed source PRACK leaves");
    let coupled_result = tokio::time::timeout(Duration::from_secs(10), confirmed.as_mut())
        .await
        .expect("crossed-final cleanup finishes");
    assert!(matches!(coupled_result, Err(Error::Sdp(_))));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), prack_responses.final_response())
            .await
            .expect("the malformed PRACK is answered")
            .expect("the malformed PRACK has a final response")
            .status
            .code(),
        488
    );
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), source_responses.final_response())
            .await
            .expect("the source INVITE is refused")
            .expect("the source INVITE has a final response")
            .status
            .code(),
        488
    );

    let mut methods = Vec::new();
    while methods.len() < 2 {
        let request = tokio::time::timeout(Duration::from_secs(10), target_requests.recv())
            .await
            .expect("the target cleanup request arrives")
            .expect("the target request inbox remains open");
        methods.push(request.request.method);
    }
    assert_eq!(methods, vec![Method::Ack, Method::Bye]);
    assert!(
        !methods.contains(&Method::Prack),
        "the failed source answer never releases the target PRACK"
    );
}
