//! Two calls a host owns, connected so each hears the other (`C-6`).
//!
//! `two_calls_bridge_and_pass_audio` is the failing-first test the story names. The rest cover the
//! properties its acceptance calls out: that a keypress arriving while bridged has a declared
//! behaviour selected when the bridge is made, that unbridging returns both calls to independent
//! operation, that ending either call ends the bridge and says so on the other's `C-3` event
//! stream, and that the same public path reaches `M-12`'s conference.
//!
//! The shape is four calls, not two. A host in the middle owns two of them — `left` and `right` —
//! and the two far ends, `alice` and `bob`, are what the audio is asserted from. A bridge that
//! mixed the two legs up would still pass with two.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{
    Call, CallBridge, CallConference, CallEvent, CallEvents, DialOptions, DtmfBridging,
    UnbridgeCause, answer, dial,
};
use sipx_media::{Config as MediaConfig, MediaPort, MediaSession};
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

/// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
///
/// A bound on failure, not a window to measure in: every clip below is well under a second, so
/// this is more than twenty times the honest answer. What it buys is that a machine with four
/// other gates compiling on it produces the same verdict as an idle one.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// The same, for one event on a call's stream. A test that is wrong about the wiring fails here
/// instead of hanging.
const EVENT_BOUND: Duration = Duration::from_secs(10);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// One connected pair: the dialling side and the answering side of the same call.
async fn connected(identity: &'static str) -> (Call, Call) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        let call = answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers");
        // Held so the endpoint is not dropped while the call runs.
        (call, callee_incoming, callee_endpoint)
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &DialOptions::new(identity, loopback()),
    )
    .await
    .expect("the call connects");

    let (callee, _incoming, _endpoint) = answering.await.expect("the answering side finishes");
    (caller, callee)
}

/// One connected pair whose answering endpoint remains driveable by this test.
///
/// The ordinary helper can discard both signalling receivers because most bridge tests never send
/// another request. A renegotiation proof needs the answering call's receiver so the raw peer's
/// re-INVITE reaches [`Call::handle`].
async fn connected_for_reinvite(
    identity: &'static str,
) -> (Call, Call, Handle, Receiver<Incoming>, std::net::SocketAddr) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        let mut call = answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers");
        let ack = tokio::time::timeout(EVENT_BOUND, callee_incoming.recv())
            .await
            .expect("the initial ACK arrival is bounded")
            .expect("the initial ACK arrives");
        assert_eq!(ack.request.method, Method::Ack);
        assert!(call.handle(&ack).await.expect("the call handles its ACK"));
        (call, callee_incoming)
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &DialOptions::new(identity, loopback()),
    )
    .await
    .expect("the call connects");
    let (callee, callee_incoming) = answering.await.expect("the answering side finishes");
    (
        caller,
        callee,
        caller_endpoint,
        callee_incoming,
        callee_addr,
    )
}

fn in_dialog(call: &Call, method: &Method, cseq: u32, body: Option<String>) -> sipx_sip::Request {
    let (local, remote) = call.dialog.local_and_remote();
    let mut request = sipx_sip::build::RequestBuilder::new(method.clone(), callee_uri())
        // The raw peer originates this request, so the receiving call's local party is To.
        .header(HeaderName::To, Bytes::from(local))
        .expect("to")
        .header(HeaderName::From, Bytes::from(remote))
        .expect("from")
        .header(
            HeaderName::CallId,
            Bytes::from(call.dialog.id.call_id.clone()),
        )
        .expect("call-id")
        .cseq(cseq, method)
        .expect("cseq")
        .max_forwards(70);
    if let Some(body) = body {
        request = request
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )
            .expect("content-type")
            .body(Bytes::from(body));
    }
    request.build()
}

/// Move `call` to a newly bound far-end media session with an accepted in-dialog offer.
///
/// The returned session is the far end after the move. Assertions use only this session and the
/// other call's far end; the stopped middle generation is never inspected.
async fn move_far_end(
    call: &mut Call,
    peer: &Handle,
    incoming: &mut Receiver<Incoming>,
    call_address: std::net::SocketAddr,
) -> MediaSession {
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("the replacement far-end port binds");
    let cseq = call.dialog.remote_cseq.unwrap_or(1).saturating_add(1);
    let rtcp = if call.media().rtcp_mode() == sipx_sdp::RtcpMode::Mux {
        "a=rtcp-mux\r\n"
    } else {
        ""
    };
    let offer = format!(
        "v=0\r\no=- 2 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio {} RTP/AVP 0 8 101\r\na=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\na=rtpmap:101 telephone-event/8000\r\na=sendrecv\r\n{rtcp}",
        port.local_addr().port(),
    );
    let request = in_dialog(call, &Method::Invite, cseq, Some(offer));
    let mut responses = peer
        .send(request, Target::udp(call_address))
        .await
        .expect("the re-INVITE leaves");
    let arrived = tokio::time::timeout(EVENT_BOUND, incoming.recv())
        .await
        .expect("the re-INVITE arrival is bounded")
        .expect("the re-INVITE arrives");
    assert!(call.handle(&arrived).await.expect("the call handles it"));
    let accepted = tokio::time::timeout(EVENT_BOUND, responses.final_response())
        .await
        .expect("the re-INVITE response is bounded")
        .expect("the re-INVITE receives a final response");
    assert_eq!(
        accepted.status.code(),
        200,
        "the media move is accepted (codec {:?}, keying {:?}, RTCP {:?})",
        call.media().codec(),
        call.negotiated_keying(),
        call.media().rtcp_mode()
    );

    peer.send_directly(
        in_dialog(call, &Method::Ack, cseq, None),
        Target::udp(call_address),
    )
    .await
    .expect("the re-INVITE is acknowledged");

    let mut config = MediaConfig::new(call.media().local_addr(), call.media().codec());
    config.rtcp_interval = None;
    config.rtcp_mode = call.media().rtcp_mode();
    port.start(config)
        .expect("the replacement far-end session starts")
}

/// The host's two calls, and the two far ends they reach.
struct Parties {
    alice: Call,
    left: Call,
    right: Call,
    bob: Call,
}

async fn parties() -> Parties {
    let (alice, left) = connected("<sip:alice@example.net>").await;
    let (bob, right) = connected("<sip:bob@example.net>").await;
    Parties {
        alice,
        left,
        right,
        bob,
    }
}

/// A clip loud enough that anything but silence at the far end is unmistakable, and shaped so a
/// test that recorded its own silence could not pass.
fn tone(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|index| if index % 8 < 4 { 12_000 } else { -12_000 })
        .collect()
}

/// Whether a recording carries the clip rather than silence.
fn carries_audio(samples: &[i16]) -> bool {
    samples.iter().any(|sample| sample.abs() > 2_000)
}

async fn next_event(events: &mut CallEvents) -> CallEvent {
    tokio::time::timeout(EVENT_BOUND, events.recv())
        .await
        .expect("no timeout waiting for a call event")
        .expect("the stream ended before this event arrived")
}

/// The next event satisfying `want`, skipping whatever construction queued ahead of it.
async fn next_matching(events: &mut CallEvents, want: impl Fn(&CallEvent) -> bool) -> CallEvent {
    loop {
        let event = next_event(events).await;
        if want(&event) {
            return event;
        }
    }
}

/// The story's failing-first test: two calls the host owns are connected through the calls' own
/// public API, and audio played into one far end comes out at the other.
#[tokio::test]
async fn two_calls_bridge_and_pass_audio() {
    let Parties {
        alice,
        mut left,
        mut right,
        bob,
    } = Box::pin(parties()).await;

    let bridge = CallBridge::connect(&mut left, &mut right);
    assert!(
        bridge.is_connected(),
        "the bridge must be forwarding once it is made"
    );
    assert!(
        !bridge.is_transcoding(),
        "two legs that negotiated the same codec must not be transcoded"
    );
    assert!(left.is_bridged() && right.is_bridged());

    let clip = tone(alice.media().samples_per_packet() * 10);
    let (_played, heard) = tokio::join!(alice.play(&clip), async {
        bob.media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    });

    assert!(
        carries_audio(&heard),
        "audio played into one bridged call was not heard on the other: \
         {} samples recorded, all of them quiet",
        heard.len()
    );
}

/// The declared DTMF behaviour, half one: by default a keypress stays on the call it arrived on.
#[tokio::test]
async fn a_keypress_is_delivered_to_the_host_by_default() {
    let Parties {
        alice,
        mut left,
        mut right,
        bob,
    } = Box::pin(parties()).await;

    let bridge = CallBridge::connect(&mut left, &mut right);
    assert_eq!(bridge.dtmf(), DtmfBridging::Deliver);

    alice.send_digits("5", Duration::from_millis(80)).await;

    let digit = tokio::time::timeout(EVENT_BOUND, left.recv_digit())
        .await
        .expect("the keypress reaches the call it arrived on")
        .expect("the media session is still running");
    assert_eq!(digit.as_char(), '5');

    // And it was not regenerated on the other leg. A definition of silence: the assertion under
    // it is negative, and it is only asked once the keypress has already been observed arriving
    // at `left` above — so load lengthens the window and can only make this fail.
    assert!(
        tokio::time::timeout(Duration::from_millis(300), bob.recv_digit())
            .await
            .is_err(),
        "a delivered keypress must not also cross the bridge"
    );
}

/// The declared DTMF behaviour, half two: selected when the bridge is made, a keypress is
/// regenerated on the other call instead.
#[tokio::test]
async fn a_keypress_passes_through_when_the_bridge_says_so() {
    let Parties {
        alice,
        mut left,
        mut right,
        bob,
    } = Box::pin(parties()).await;

    let bridge = CallBridge::connect_with(
        &mut left,
        &mut right,
        sipx_call::BridgeOptions::new().with_dtmf(DtmfBridging::PassThrough),
    );
    assert_eq!(bridge.dtmf(), DtmfBridging::PassThrough);

    alice.send_digits("7", Duration::from_millis(80)).await;

    let digit = tokio::time::timeout(EVENT_BOUND, bob.recv_digit())
        .await
        .expect("the keypress crosses the bridge")
        .expect("the media session is still running");
    assert_eq!(digit.as_char(), '7');
}

/// Unbridging returns both calls to independent operation: audio stops crossing, and each call
/// receives its own far end's audio again.
#[tokio::test]
async fn unbridging_returns_both_calls_to_independent_operation() {
    let Parties {
        alice,
        mut left,
        mut right,
        bob,
    } = Box::pin(parties()).await;

    let mut left_events = left.events().expect("one receiver");
    let mut right_events = right.events().expect("one receiver");

    let bridge = CallBridge::connect(&mut left, &mut right);
    bridge.unbridge();

    for events in [&mut left_events, &mut right_events] {
        match next_matching(events, |event| matches!(event, CallEvent::Unbridged { .. })).await {
            CallEvent::Unbridged { cause } => assert_eq!(cause, UnbridgeCause::Released),
            other => panic!("expected Unbridged, got {other:?}"),
        }
    }

    assert!(!left.is_bridged() && !right.is_bridged());

    // The host's own call hears its far end again, which is what "independent" means: while
    // bridged, the received audio went to the far leg instead of into this session's own queue.
    let clip = tone(alice.media().samples_per_packet() * 10);
    let (_played, heard, crossed) = tokio::join!(
        alice.play(&clip),
        async {
            left.media()
                .record_at_least(clip.len(), DELIVERY_BOUND)
                .await
        },
        async {
            // A definition of silence: the assertion under this is negative — nothing must reach
            // bob — so a longer wait can only collect more of what must not be there, and load
            // can only make this fail rather than pass.
            tokio::time::timeout(Duration::from_secs(1), async {
                bob.media()
                    .record_at_least(clip.len(), DELIVERY_BOUND)
                    .await
            })
            .await
        }
    );

    assert!(
        carries_audio(&heard),
        "an unbridged call must receive its own far end's audio again"
    );
    assert!(
        crossed.map_or(true, |samples| !carries_audio(&samples)),
        "audio must stop crossing an unbridged call"
    );
}

/// Ending one bridged call ends the bridge, and the other call is told on its event stream
/// (`C-3`) rather than having to poll for it.
#[tokio::test]
async fn ending_one_call_ends_the_bridge_on_the_other_stream() {
    let Parties {
        alice: _alice,
        mut left,
        mut right,
        bob: _bob,
    } = Box::pin(parties()).await;

    let mut right_events = right.events().expect("one receiver");
    let bridge = CallBridge::connect(&mut left, &mut right);

    left.hang_up().await.expect("hangs up");

    match next_matching(&mut right_events, |event| {
        matches!(event, CallEvent::Unbridged { .. })
    })
    .await
    {
        CallEvent::Unbridged { cause } => assert_eq!(cause, UnbridgeCause::PeerEnded),
        other => panic!("expected Unbridged, got {other:?}"),
    }

    assert!(
        !bridge.is_connected(),
        "ending either call must end the bridge"
    );
    assert!(!right.is_bridged());
}

/// The ending call's own stream keeps `Ended` last: the bridge reports to the peer, never back
/// onto the stream that is about to close.
#[tokio::test]
async fn the_ending_call_stream_still_ends_with_ended() {
    let Parties {
        alice: _alice,
        mut left,
        mut right,
        bob: _bob,
    } = Box::pin(parties()).await;

    let mut left_events = left.events().expect("one receiver");
    let _bridge = CallBridge::connect(&mut left, &mut right);

    left.hang_up().await.expect("hangs up");

    // Read the whole stream up to its last word, rather than waiting out a window and hoping
    // nothing else was coming: `Ended` is guaranteed to arrive, so it is the thing to wait for.
    let mut seen = Vec::new();
    loop {
        let event = next_event(&mut left_events).await;
        let ended = matches!(event, CallEvent::Ended(_));
        seen.push(event);
        if ended {
            break;
        }
    }
    assert!(
        !seen
            .iter()
            .any(|event| matches!(event, CallEvent::Unbridged { .. })),
        "the ending call's own stream must carry no Unbridged at all — its bridge ending is \
         implied by Ended, which is last: {seen:?}"
    );
}

/// The conference half of the same public path: a `Call` joins and leaves `M-12`'s mixer without
/// the host ever holding a media session or a raw port.
#[tokio::test]
async fn calls_join_and_leave_the_conference() {
    let Parties {
        alice,
        mut left,
        mut right,
        bob,
    } = Box::pin(parties()).await;

    let conference = CallConference::narrowband().expect("a narrowband conference starts");
    assert!(conference.is_empty().await);

    let one = conference.join(&mut left).await;
    let two = conference.join(&mut right).await;
    assert_eq!(conference.len().await, 2);

    // What alice says reaches the other participant's far end through the mix: into `left`, out
    // of the mixer to `right`, and down `right`'s own leg to bob. Asserted at bob, because what
    // the host's own session believes it sent is not evidence.
    let clip = tone(alice.media().samples_per_packet() * 25);
    let (_played, heard) = tokio::join!(alice.play(&clip), async {
        bob.media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    });
    assert!(
        carries_audio(&heard),
        "a conference participant heard nothing of what another said"
    );

    conference.leave(one).await;
    assert_eq!(conference.len().await, 1);
    conference.leave(two).await;
    assert!(conference.is_empty().await);
    conference.close().await;
}

/// B1: replacing one bridged call's media generation keeps the same live bridge.
#[tokio::test]
async fn a_bridge_carries_audio_both_ways_after_a_reinvite_moves_one_leg() {
    let (_alice, mut left, _original_peer, mut left_incoming, left_address) =
        connected_for_reinvite("<sip:alice@example.net>").await;
    let (peer, _peer_incoming) = endpoint().await;
    let (bob, mut right) = connected("<sip:bob@example.net>").await;
    let bridge = CallBridge::connect(&mut left, &mut right);

    let moved_alice = move_far_end(&mut left, &peer, &mut left_incoming, left_address).await;
    assert!(bridge.is_connected(), "the bridge follows the replacement");
    assert!(left.is_bridged() && right.is_bridged());

    let clip = tone(moved_alice.samples_per_packet() * 20);
    let (_played, heard) = tokio::join!(moved_alice.play(&clip, 160), async {
        bob.media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    });
    assert!(carries_audio(&heard), "Bob did not hear the moved far end");

    let (_played, heard) = tokio::join!(bob.play(&clip), async {
        moved_alice
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    });
    assert!(carries_audio(&heard), "the moved far end did not hear Bob");
}

/// C5: media replacement changes neither the conference identity nor either audio direction.
#[tokio::test]
async fn a_conference_participant_keeps_hearing_and_contributing_after_a_reinvite() {
    let (_alice, mut left, _original_peer, mut left_incoming, left_address) =
        connected_for_reinvite("<sip:alice@example.net>").await;
    let (peer, _peer_incoming) = endpoint().await;
    let (bob, mut right) = connected("<sip:bob@example.net>").await;
    let conference = CallConference::narrowband().expect("a conference starts");
    let alice_id = conference.join(&mut left).await;
    let bob_id = conference.join(&mut right).await;

    let moved_alice = move_far_end(&mut left, &peer, &mut left_incoming, left_address).await;
    assert_eq!(conference.len().await, 2, "neither participant rejoined");

    let clip = tone(moved_alice.samples_per_packet() * 30);
    let (_played, heard) = tokio::join!(moved_alice.play(&clip, 160), async {
        bob.media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    });
    assert!(
        carries_audio(&heard),
        "Bob did not hear the moved participant"
    );

    let (_played, heard) = tokio::join!(bob.play(&clip), async {
        moved_alice
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    });
    assert!(
        carries_audio(&heard),
        "the moved participant did not hear the mix"
    );

    conference.leave(alice_id).await;
    conference.leave(bob_id).await;
    conference.close().await;
}
