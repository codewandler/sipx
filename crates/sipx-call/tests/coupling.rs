//! Two dialogs driven as one call (`C-1`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use sipx_call::{
    Calls, Coupling, CouplingEnd, DialOptions, Dispatched, Dispatcher, EarlyCoupling, Error, Leg,
    dial, dial_early, ring, ring_early,
};
use sipx_sdp::Direction;
use sipx_sip::{Host, HostName, Method, Uri};
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
) {
    let (peer_endpoint, peer_incoming) = endpoint().await;
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
    (edge_call, edge_incoming, peer_call, peer_incoming)
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
                    String::from_utf8_lossy(incoming.request.body()).contains("a=recvonly"),
                    "the source send-only offer is inverted on the target leg"
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
    let (edge_one, mut edge_one_incoming, mut peer_one, _peer_one_incoming) =
        leg(&edge, &mut pumped, "<sip:one@example.net>").await;
    let (edge_two, mut edge_two_incoming, mut peer_two, mut peer_two_incoming) =
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
                        String::from_utf8_lossy(incoming.request.body()).contains("a=recvonly"),
                        "the first leg's send-only offer is inverted on the peer leg"
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
    let (edge_one, mut edge_one_incoming, mut peer_one, mut peer_one_incoming) =
        leg(&edge, &mut pumped, "<sip:one@example.net>").await;
    let (edge_two, mut edge_two_incoming, mut peer_two, mut peer_two_incoming) =
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
