//! Coupling two dialogs while staying off the media path (`C-7`, RFC 7092 §3.1.3).
//!
//! The peers here are deliberately asymmetric. The source is a raw socket, so the exact bytes it
//! offers are known and can be compared against what the target receives. The target is an
//! ordinary sipx call with a real media session, so the RTP it sends is real and has somewhere to
//! go. Nothing in between binds a media port: what proves that is where the packets arrive.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::future::poll_fn;
use std::net::IpAddr;
use std::task::Poll;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{
    Calls, CouplingEnd, DialOptions, Dispatched, Dispatcher, EarlyCoupling, Leg, OffMediaCoupling,
    OffMediaOptions,
};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::net::UdpSocket;
use tokio::sync::mpsc::{self, Receiver};

/// A bound on failure, not a measurement window: every wait below is for an event that has
/// already been caused, so reaching this means the causal chain is broken.
const BOUND: Duration = Duration::from_secs(10);

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
        match tokio::time::timeout(BOUND, self.surfaced.recv())
            .await
            .expect("an invitation arrives")
            .expect("the dispatcher remains running")
        {
            Dispatched::Invitation(invitation) => invitation,
            other => panic!("expected an invitation, got {other:?}"),
        }
    }
}

/// The description a real endpoint sends: its own address, its own port, its own origin.
fn endpoint_offer(port: u16, version: u64) -> String {
    format!(
        "v=0\r\no=source 4001 {version} IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {port} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n"
    )
}

/// Everything in a description except the line whose ownership the coupling takes.
fn without_origin(sdp: &str) -> String {
    sdp.lines()
        .filter(|line| !line.starts_with("o="))
        .fold(String::new(), |mut kept, line| {
            kept.push_str(line);
            kept.push('\n');
            kept
        })
}

fn origin_of(sdp: &str) -> String {
    sdp.lines()
        .find(|line| line.starts_with("o="))
        .expect("a description has an origin")
        .trim_end()
        .to_owned()
}

fn via(endpoint: &Handle) -> Bytes {
    Bytes::from(format!(
        "SIP/2.0/UDP {};rport;branch={}",
        endpoint.sent_by_for(sipx_transport::TransportKind::Udp),
        sipx_transport::new_branch()
    ))
}

fn source_invite(source: &Handle, call_id: &'static str, body: &str) -> Request {
    RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("edge.example").unwrap())),
    )
    .header(HeaderName::Via, via(source))
    .unwrap()
    .header(HeaderName::To, "<sip:edge.example>")
    .unwrap()
    .header(HeaderName::From, "<sip:caller@example.net>;tag=source")
    .unwrap()
    .header(HeaderName::CallId, call_id)
    .unwrap()
    .cseq(1, &Method::Invite)
    .unwrap()
    .header(
        HeaderName::Contact,
        format!("<sip:caller@{}>", source.local_addr()),
    )
    .unwrap()
    .header(HeaderName::ContentType, "application/sdp")
    .unwrap()
    .max_forwards(70)
    .body(Bytes::from(body.to_owned()))
    .build()
}

/// An in-dialog request from the raw source, addressed at the coupling's own `Contact`.
fn in_dialog(
    source: &Handle,
    invite: &Request,
    accepted: &Response,
    method: &Method,
    cseq: u32,
    body: Option<&str>,
) -> Request {
    let value = |name: &HeaderName| {
        Bytes::from(
            accepted
                .headers
                .value(name)
                .unwrap_or_else(|| panic!("the acceptance carries {name:?}"))
                .into_owned(),
        )
    };
    let contact = String::from_utf8_lossy(&value(&HeaderName::Contact)).into_owned();
    let uri = contact
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned();
    let uri = Uri::parse(Bytes::from(uri)).expect("a target URI");
    let mut builder = RequestBuilder::new(method.clone(), uri)
        .header(HeaderName::Via, via(source))
        .unwrap()
        .header(HeaderName::To, value(&HeaderName::To))
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
        .cseq(cseq, method)
        .unwrap()
        .header(
            HeaderName::Contact,
            format!("<sip:caller@{}>", source.local_addr()),
        )
        .unwrap()
        .max_forwards(70);
    if let Some(body) = body {
        builder = builder
            .header(HeaderName::ContentType, "application/sdp")
            .unwrap()
            .body(Bytes::from(body.to_owned()));
    }
    builder.build()
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

async fn final_response(responses: &mut sipx_transport::Responses, what: &str) -> Response {
    tokio::time::timeout(BOUND, responses.final_response())
        .await
        .unwrap_or_else(|_| panic!("{what} receives a final response"))
        .unwrap_or_else(|| panic!("{what} transaction stays open until its final response"))
}

async fn next_request(inbox: &mut Receiver<Incoming>, what: &str) -> Incoming {
    tokio::time::timeout(BOUND, inbox.recv())
        .await
        .unwrap_or_else(|_| panic!("{what} arrives"))
        .unwrap_or_else(|| panic!("{what} inbox stays open"))
}

/// The first RTP packet to reach a socket the endpoint owns and sipx never bound.
async fn first_rtp(socket: &UdpSocket, what: &str) -> sipx_rtp::Packet {
    let mut datagram = vec![0_u8; 2048];
    let (len, _) = tokio::time::timeout(BOUND, socket.recv_from(&mut datagram))
        .await
        .unwrap_or_else(|_| panic!("{what}"))
        .expect("the endpoint media socket stays readable");
    sipx_rtp::Packet::decode(&Bytes::copy_from_slice(&datagram[..len])).expect("a valid RTP packet")
}

fn body_of(request: &Incoming) -> String {
    String::from_utf8_lossy(request.request.body()).into_owned()
}

/// `C-7`'s failing-first acceptance test.
///
/// Two endpoints keep their own media addresses across an initial offer/answer and a relayed
/// re-INVITE that moves one of them. The proof that sipx is off the media path is causal rather
/// than structural: the packets arrive on a socket this test bound itself, at the port the
/// relayed description named, and they arrive there *after* the negotiation that moved it.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the whole two-endpoint exchange, media included, is one causal vector"
)]
async fn media_addresses_stay_endpoint_owned_across_a_relayed_reinvite() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let edge_addr = edge.local_addr();

    // The source endpoint owns this socket. Its port is the only audio destination sipx may
    // put in front of the target.
    let source_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let source_port = source_media.local_addr().expect("bound").port();
    let offer = endpoint_offer(source_port, 1);

    let (source, _source_incoming) = endpoint().await;
    let invite = source_invite(&source, "off-media-reinvite", &offer);
    let mut source_responses = source
        .send(invite.clone(), Target::udp(edge_addr))
        .await
        .expect("the source INVITE leaves");
    let invitation = pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let calls = pumped.calls.clone();
    let edge_for_coupling = edge.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(OffMediaCoupling::dial(
            invitation,
            &calls,
            &edge_for_coupling,
            target,
            &to,
            &OffMediaOptions::new("<sip:edge@example.net>", loopback()),
        ))
        .await
        .expect("the off-media coupling reaches the target")
    });

    let target_invitation = target_pumped.invitation().await;
    let relayed = String::from_utf8_lossy(target_invitation.request().request.body()).into_owned();
    assert_eq!(
        without_origin(&relayed),
        without_origin(&offer),
        "the description reaches the target endpoint unchanged apart from its origin"
    );
    assert_ne!(
        origin_of(&relayed),
        origin_of(&offer),
        "each dialog's description carries the coupling's own origin, not the far endpoint's"
    );
    assert!(
        relayed.contains(&format!("m=audio {source_port} RTP/AVP 0")),
        "the target is told to send audio to the source endpoint's own port: {relayed}"
    );

    let mut target_call = target_invitation
        .answer(&target_endpoint, loopback())
        .await
        .expect("the target answers the relayed INVITE");
    let (_target_invite, mut target_inbox) = target_invitation.into_parts();
    let mut coupling = coupling_task.await.expect("the coupling task finishes");
    assert_eq!(
        next_request(&mut target_inbox, "the target's acceptance is acknowledged")
            .await
            .request
            .method,
        Method::Ack
    );

    let accepted = final_response(&mut source_responses, "the source INVITE").await;
    assert_eq!(accepted.status.code(), 200);
    let answer = String::from_utf8_lossy(accepted.body()).into_owned();
    assert!(
        answer.contains(&format!(
            "m=audio {}",
            target_call.media().local_addr().port()
        )),
        "the source is told to send audio to the target endpoint's own port: {answer}"
    );
    source
        .send_directly(
            in_dialog(&source, &invite, &accepted, &Method::Ack, 1, None),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source acknowledges");

    let running = tokio::spawn(async move {
        let end = coupling.run().await;
        (end, coupling)
    });

    // Audio the target sends arrives at the source endpoint's own socket. Nothing forwarded it.
    let samples = vec![8_000_i16; 1_600];
    let (played, heard) = tokio::join!(
        target_call.media().play(&samples, 160),
        first_rtp(
            &source_media,
            "the target's audio reaches the source endpoint directly"
        ),
    );
    assert!(played, "the target endpoint finishes its audio");
    assert_eq!(heard.payload_type, 0);
    assert!(
        heard.payload.iter().any(|byte| *byte != 0xFF),
        "what arrived is the audio the target played, not µ-law silence"
    );

    // The source moves its media to a second socket it owns, and says so in a re-INVITE.
    let moved_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let moved_port = moved_media.local_addr().expect("bound").port();
    let moved_offer = endpoint_offer(moved_port, 2);
    let mut reinvite_responses = source
        .send(
            in_dialog(
                &source,
                &invite,
                &accepted,
                &Method::Invite,
                2,
                Some(&moved_offer),
            ),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source re-INVITE leaves");

    let relayed_reinvite = next_request(&mut target_inbox, "the relayed re-INVITE").await;
    assert_eq!(relayed_reinvite.request.method, Method::Invite);
    let relayed_body = body_of(&relayed_reinvite);
    assert_eq!(
        without_origin(&relayed_body),
        without_origin(&moved_offer),
        "the moved media description crosses unchanged apart from its origin"
    );
    assert!(
        origin_of(&relayed_body) > origin_of(&relayed),
        "the coupling's own session version increases with the description it changed: {} then {}",
        origin_of(&relayed),
        origin_of(&relayed_body)
    );
    assert!(
        target_call
            .handle(&relayed_reinvite)
            .await
            .expect("the target answers the relayed re-INVITE")
    );

    let accepted_reinvite = final_response(&mut reinvite_responses, "the source re-INVITE").await;
    assert_eq!(accepted_reinvite.status.code(), 200);
    source
        .send_directly(
            in_dialog(&source, &invite, &accepted, &Method::Ack, 2, None),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source acknowledges the renegotiation");
    assert_eq!(
        next_request(
            &mut target_inbox,
            "the relayed renegotiation is acknowledged"
        )
        .await
        .request
        .method,
        Method::Ack,
        "the coupling acknowledges the target's 2xx on the re-INVITE axis too"
    );

    // The causal claim: audio now arrives at the port the relayed re-INVITE named, and it got
    // there without sipx ever holding a socket on either path.
    let (played, moved) = tokio::join!(
        target_call.media().play(&samples, 160),
        first_rtp(
            &moved_media,
            "audio follows the relayed re-INVITE to the endpoint's new port"
        ),
    );
    assert!(played, "the target endpoint finishes its second burst");
    assert_eq!(moved.payload_type, 0);
    assert!(
        moved.payload.iter().any(|byte| *byte != 0xFF),
        "the endpoint's new port carries the audio, not µ-law silence"
    );

    // The UPDATE carrier, on the same two dialogs: the description moves back to the first
    // socket and the audio follows it there.
    let mut update_responses = source
        .send(
            in_dialog(
                &source,
                &invite,
                &accepted,
                &Method::Update,
                3,
                Some(&endpoint_offer(source_port, 3)),
            ),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source UPDATE leaves");
    let relayed_update = next_request(&mut target_inbox, "the relayed UPDATE").await;
    assert_eq!(relayed_update.request.method, Method::Update);
    assert_eq!(
        without_origin(&body_of(&relayed_update)),
        without_origin(&endpoint_offer(source_port, 3)),
        "the UPDATE carries the endpoint's description unchanged apart from its origin"
    );
    assert!(
        target_call
            .handle(&relayed_update)
            .await
            .expect("the target answers the relayed UPDATE")
    );
    let updated = final_response(&mut update_responses, "the source UPDATE").await;
    assert_eq!(updated.status.code(), 200);
    assert!(
        String::from_utf8_lossy(updated.body()).contains("m=audio"),
        "the UPDATE's answer is the target endpoint's own description"
    );
    let (played, back) = tokio::join!(
        target_call.media().play(&samples, 160),
        first_rtp(
            &source_media,
            "audio follows the relayed UPDATE back to the first port"
        ),
    );
    assert!(played, "the target endpoint finishes its third burst");
    assert_eq!(back.payload_type, 0);

    let mut bye_responses = source
        .send(
            in_dialog(&source, &invite, &accepted, &Method::Bye, 4, None),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source BYE leaves");
    let relayed_bye = next_request(&mut target_inbox, "the relayed BYE").await;
    assert_eq!(relayed_bye.request.method, Method::Bye);
    assert!(
        target_call
            .handle(&relayed_bye)
            .await
            .expect("the target answers the relayed BYE")
    );
    assert_eq!(
        final_response(&mut bye_responses, "the source BYE")
            .await
            .status
            .code(),
        200
    );

    let (end, _coupling) = running.await.expect("the coupling driver finishes");
    assert_eq!(
        end.expect("the coupling ends cleanly"),
        CouplingEnd::Bye(Leg::One)
    );
    assert!(target_call.is_ended(), "the target dialog received a BYE");
}

/// The acceptance's negative control. `C-1`'s coupling with no bridge attached still terminates
/// media on both legs, so the address the far endpoint is told to use is sipx's own. If this
/// stops being true, the off-media role above has become indistinguishable from omitting
/// `bridge_media`, and one of the two claims is wrong.
#[tokio::test]
async fn a_media_terminating_coupling_replaces_the_endpoint_address() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let edge_addr = edge.local_addr();

    let source_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let source_port = source_media.local_addr().expect("bound").port();
    let offer = endpoint_offer(source_port, 1);

    let (source, _source_incoming) = endpoint().await;
    let invite = source_invite(&source, "media-terminating", &offer);
    let _responses = source
        .send(invite, Target::udp(edge_addr))
        .await
        .expect("the source INVITE leaves");
    let invitation = pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let calls = pumped.calls.clone();
    let edge_for_coupling = edge.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(EarlyCoupling::dial(
            invitation,
            &calls,
            &edge_for_coupling,
            target,
            &to,
            &DialOptions::new("<sip:edge@example.net>", loopback()),
            loopback(),
        ))
        .await
    });

    let target_invitation = target_pumped.invitation().await;
    let relayed = String::from_utf8_lossy(target_invitation.request().request.body()).into_owned();
    assert!(
        !relayed.contains(&format!("m=audio {source_port} ")),
        "a media-terminating coupling advertises its own port, not the source endpoint's: {relayed}"
    );
    drop(coupling_task);
}

/// A description sipx cannot map is refused where it arrived. The far leg must not learn that
/// anything happened, and the coupling must still be usable afterwards.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "establishing the coupling is the setup the refusal is asserted against"
)]
async fn unmappable_sdp_is_refused_before_the_peer_leg_is_told() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let edge_addr = edge.local_addr();

    let source_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let source_port = source_media.local_addr().expect("bound").port();
    let offer = endpoint_offer(source_port, 1);

    let (source, _source_incoming) = endpoint().await;
    let invite = source_invite(&source, "off-media-refusal", &offer);
    let mut source_responses = source
        .send(invite.clone(), Target::udp(edge_addr))
        .await
        .expect("the source INVITE leaves");
    let invitation = pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let calls = pumped.calls.clone();
    let edge_for_coupling = edge.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(OffMediaCoupling::dial(
            invitation,
            &calls,
            &edge_for_coupling,
            target,
            &to,
            &OffMediaOptions::new("<sip:edge@example.net>", loopback()),
        ))
        .await
        .expect("the off-media coupling reaches the target")
    });

    let target_invitation = target_pumped.invitation().await;
    let mut target_call = target_invitation
        .answer(&target_endpoint, loopback())
        .await
        .expect("the target answers the relayed INVITE");
    let (_target_invite, mut target_inbox) = target_invitation.into_parts();
    let mut coupling = coupling_task.await.expect("the coupling task finishes");
    assert_eq!(
        next_request(&mut target_inbox, "the target's acceptance is acknowledged")
            .await
            .request
            .method,
        Method::Ack
    );
    let accepted = final_response(&mut source_responses, "the source INVITE").await;
    source
        .send_directly(
            in_dialog(&source, &invite, &accepted, &Method::Ack, 1, None),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source acknowledges");

    let running = tokio::spawn(async move {
        let end = coupling.run().await;
        (end, coupling)
    });

    let mut refused = source
        .send(
            in_dialog(
                &source,
                &invite,
                &accepted,
                &Method::Invite,
                2,
                Some("v=0\r\nthis is not a session description\r\n"),
            ),
            Target::udp(edge_addr),
        )
        .await
        .expect("the malformed re-INVITE leaves");
    assert_eq!(
        final_response(&mut refused, "the malformed re-INVITE")
            .await
            .status
            .code(),
        488,
        "an unmappable description is refused on the leg it arrived on"
    );
    assert!(
        poll_fn(|cx| Poll::Ready(target_inbox.poll_recv(cx).is_pending())).await,
        "the peer leg is told nothing about a description that never mapped"
    );

    // The dialog is unchanged, so the next offer still relays.
    let moved_offer = endpoint_offer(source_port, 2);
    let mut retried = source
        .send(
            in_dialog(
                &source,
                &invite,
                &accepted,
                &Method::Invite,
                3,
                Some(&moved_offer),
            ),
            Target::udp(edge_addr),
        )
        .await
        .expect("the retried re-INVITE leaves");
    let relayed_reinvite = next_request(&mut target_inbox, "the relayed re-INVITE").await;
    assert!(
        target_call
            .handle(&relayed_reinvite)
            .await
            .expect("the target answers the relayed re-INVITE")
    );
    assert_eq!(
        final_response(&mut retried, "the retried re-INVITE")
            .await
            .status
            .code(),
        200
    );

    running.abort();
}

/// A final failure on the target leg is the source leg's final response, with the same status —
/// `C-1`'s lifecycle policy, unchanged by the media role.
#[tokio::test]
async fn an_outbound_final_failure_is_the_inbound_final_response() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let edge_addr = edge.local_addr();

    let source_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let source_port = source_media.local_addr().expect("bound").port();
    let (source, _source_incoming) = endpoint().await;
    let invite = source_invite(
        &source,
        "off-media-failure",
        &endpoint_offer(source_port, 1),
    );
    let mut source_responses = source
        .send(invite, Target::udp(edge_addr))
        .await
        .expect("the source INVITE leaves");
    let invitation = pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let calls = pumped.calls.clone();
    let edge_for_coupling = edge.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(OffMediaCoupling::dial(
            invitation,
            &calls,
            &edge_for_coupling,
            target,
            &to,
            &OffMediaOptions::new("<sip:edge@example.net>", loopback()),
        ))
        .await
    });

    let target_invitation = target_pumped.invitation().await;
    let refusal = ResponseBuilder::to_request(
        &target_invitation.request().request,
        StatusCode::new(486).expect("valid status"),
        "Busy Here",
    )
    .expect("the refusal builds")
    .set_header(
        &HeaderName::To,
        Bytes::from(format!(
            "{};tag=busy",
            String::from_utf8_lossy(
                &target_invitation
                    .request()
                    .request
                    .headers
                    .value(&HeaderName::To)
                    .expect("the relayed INVITE has To")
            )
        )),
    )
    .expect("the refusal dialog tag is valid")
    .build();
    target_endpoint
        .respond(&target_invitation.request().key, refusal)
        .await
        .expect("the target refuses");

    assert_eq!(
        final_response(&mut source_responses, "the source INVITE")
            .await
            .status
            .code(),
        486,
        "the outbound refusal is the inbound final response, with its own status"
    );
    assert!(
        coupling_task
            .await
            .expect("the coupling task finishes")
            .is_err(),
        "a refused target leg is not a coupling"
    );
}

/// Glare is `C-1`'s decision, taken by `C-1`'s state table, before anything is forwarded. The
/// crossed offer receives a live 491 while the first exchange is still outstanding, and the
/// peer's own fresh retry is relayed once it settles.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the collision and the retry after it are one sequence"
)]
async fn glare_gets_a_live_491_and_the_retry_is_relayed() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let edge_addr = edge.local_addr();

    let source_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let source_port = source_media.local_addr().expect("bound").port();
    let (source, mut source_inbox) = endpoint().await;
    let invite = source_invite(&source, "off-media-glare", &endpoint_offer(source_port, 1));
    let mut source_responses = source
        .send(invite.clone(), Target::udp(edge_addr))
        .await
        .expect("the source INVITE leaves");
    let invitation = pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let calls = pumped.calls.clone();
    let edge_for_coupling = edge.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(OffMediaCoupling::dial(
            invitation,
            &calls,
            &edge_for_coupling,
            target,
            &to,
            &OffMediaOptions::new("<sip:edge@example.net>", loopback()),
        ))
        .await
        .expect("the off-media coupling reaches the target")
    });

    let target_invitation = target_pumped.invitation().await;
    let mut target_call = target_invitation
        .answer(&target_endpoint, loopback())
        .await
        .expect("the target answers the relayed INVITE");
    let (_target_invite, mut target_inbox) = target_invitation.into_parts();
    let mut coupling = coupling_task.await.expect("the coupling task finishes");
    assert_eq!(
        next_request(&mut target_inbox, "the target's acceptance is acknowledged")
            .await
            .request
            .method,
        Method::Ack
    );
    let accepted = final_response(&mut source_responses, "the source INVITE").await;
    source
        .send_directly(
            in_dialog(&source, &invite, &accepted, &Method::Ack, 1, None),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source acknowledges");

    let running = tokio::spawn(async move {
        let end = coupling.run().await;
        (end, coupling)
    });

    let mut reinvite_responses = source
        .send(
            in_dialog(
                &source,
                &invite,
                &accepted,
                &Method::Invite,
                2,
                Some(&endpoint_offer(source_port, 2)),
            ),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source re-INVITE leaves");
    let relayed = next_request(&mut target_inbox, "the relayed re-INVITE").await;
    assert_eq!(relayed.request.method, Method::Invite);

    // Crossed while the relayed offer is still unanswered on this leg.
    let crossed = target_call.reinvite(sipx_sdp::Direction::SendRecv).await;
    assert!(
        matches!(crossed, Err(sipx_call::Error::Rejected { status: 491, .. })),
        "the crossed offer receives 491 while the first is outstanding: {crossed:?}"
    );

    assert!(
        target_call
            .handle(&relayed)
            .await
            .expect("the target answers the relayed offer")
    );
    assert_eq!(
        final_response(&mut reinvite_responses, "the source re-INVITE")
            .await
            .status
            .code(),
        200
    );
    source
        .send_directly(
            in_dialog(&source, &invite, &accepted, &Method::Ack, 2, None),
            Target::udp(edge_addr),
        )
        .await
        .expect("the source acknowledges the settled exchange");
    assert_eq!(
        next_request(&mut target_inbox, "the settled exchange is acknowledged")
            .await
            .request
            .method,
        Method::Ack
    );

    // The far end's own retry, after settlement, is a new transaction and is relayed.
    let retried = tokio::join!(target_call.reinvite(sipx_sdp::Direction::SendRecv), async {
        let relayed_retry = next_request(&mut source_inbox, "the relayed retry").await;
        assert_eq!(relayed_retry.request.method, Method::Invite);
        assert!(
            !body_of(&relayed_retry).is_empty(),
            "the retried offer carries the target endpoint's own description"
        );
        let answer = ResponseBuilder::to_request(
            &relayed_retry.request,
            StatusCode::new(200).expect("valid status"),
            "OK",
        )
        .expect("the source acceptance builds")
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:caller@{}>", source.local_addr())),
        )
        .expect("valid Contact")
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )
        .expect("valid Content-Type")
        .body(Bytes::from(endpoint_offer(source_port, 3)))
        .build();
        source
            .respond(&relayed_retry.key, answer)
            .await
            .expect("the source answers the relayed retry");
    })
    .0;
    retried.expect("the retry crosses after settlement");

    running.abort();
}

/// A CANCEL on the source leg withdraws the target invitation this coupling owns — the same
/// lifecycle mapping the media-terminating role applies, with no media session on either side.
#[tokio::test]
async fn a_source_cancel_withdraws_the_owned_target_invitation() {
    let (edge, edge_incoming) = endpoint().await;
    let mut pumped = pump(&edge, edge_incoming);
    let edge_addr = edge.local_addr();

    let source_media = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let source_port = source_media.local_addr().expect("bound").port();
    let (source, _source_inbox) = endpoint().await;
    let invite = source_invite(&source, "off-media-cancel", &endpoint_offer(source_port, 1));
    let mut source_responses = source
        .send(invite.clone(), Target::udp(edge_addr))
        .await
        .expect("the source INVITE leaves");
    let invitation = pumped.invitation().await;

    let (target_endpoint, target_incoming) = endpoint().await;
    let mut target_pumped = pump(&target_endpoint, target_incoming);
    let calls = pumped.calls.clone();
    let edge_for_coupling = edge.clone();
    let target = Target::udp(target_endpoint.local_addr());
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let coupling_task = tokio::spawn(async move {
        Box::pin(OffMediaCoupling::dial(
            invitation,
            &calls,
            &edge_for_coupling,
            target,
            &to,
            &OffMediaOptions::new("<sip:edge@example.net>", loopback()),
        ))
        .await
    });

    let mut target_invitation = target_pumped.invitation().await;
    assert!(
        !target_invitation.request().request.body().is_empty(),
        "the target leg was offered the source endpoint's own description"
    );
    let _ringing = sipx_call::ring(
        &target_endpoint,
        target_invitation.request(),
        180,
        "Ringing",
        false,
    )
    .await
    .expect("the target rings without binding media");
    let mut target_events = target_invitation
        .events()
        .expect("the cancellation stream is available");

    source
        .send_directly(cancel_for(&invite), Target::udp(edge_addr))
        .await
        .expect("the source CANCEL leaves");

    let (coupled, cancelled) = tokio::join!(coupling_task, target_events.recv());
    assert!(matches!(
        coupled.expect("the coupling task finishes"),
        Err(sipx_call::Error::InvitationCancelled)
    ));
    assert!(matches!(
        cancelled,
        Some(sipx_call::CallEvent::Ended(
            sipx_call::EndCause::RemoteCancel
        ))
    ));
    assert!(
        target_invitation.is_cancelled(),
        "the coupled target INVITE received CANCEL"
    );
    assert_eq!(
        final_response(&mut source_responses, "the cancelled source INVITE")
            .await
            .status
            .code(),
        487
    );
}
