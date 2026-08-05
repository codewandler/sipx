//! The stand-in realtime peer, held to `docs/specs/openai-realtime.md`'s vectors.
//!
//! Every test here is named for the vector it belongs to, and the ones that matter most are the
//! negatives: a stand-in whose misbehaviour is a configuration flag nobody observes proves
//! nothing about the bridge that will be tested against it. So each negative mode is asserted
//! from the *client's* side of the socket — the frame really arrives malformed, the pong really
//! does not come, the upgrade really is refused — and, where a negative could pass vacuously,
//! the same script runs against a well-behaved peer in the same test as the control arm.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use sipx_testkit::realtime_peer::{
    CancelPolicy, ClientEvent, Emission, F_RAMP_BASE64, F_SILENCE, F_SILENCE_BASE64,
    FIXTURE_BEARER, FRAME_BYTES, Malformed, PeerConfig, RealtimePeer, StallPoint, UpgradeOutcome,
    Withhold, tone_bytes, tone_frame,
};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::client_async;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::{Bytes as WsBytes, Error as WsError, Message};

/// How long a test waits for a frame the peer is supposed to send.
///
/// A **bound on failure**: how long this side waits before concluding the frame is not coming.
/// Nothing is ordered by it — every positive assertion below completes on the frame itself.
const ARRIVAL: Duration = Duration::from_secs(10);

/// How long a hole has to be before "the peer sent nothing" is true.
///
/// A **definition of silence**, in `docs/designs/media.md`'s sense: the assertion underneath it is
/// negative, so a slower machine lengthens the real hole rather than shortening this window.
const QUIET: Duration = Duration::from_millis(250);

type Client = WebSocketStream<TcpStream>;

/// The upgrade request the bridge sends, per spec §2: `?model=`, a bearer, nothing else.
fn upgrade_request(url: &str, bearer: Option<&str>, extra: &[(&str, &str)]) -> Request<()> {
    let uri: tokio_tungstenite::tungstenite::http::Uri = url.parse().expect("a uri");
    let host = uri.authority().expect("an authority").to_string();
    let mut builder = Request::builder()
        .method("GET")
        .uri(url)
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key());
    if let Some(bearer) = bearer {
        builder = builder.header("Authorization", format!("Bearer {bearer}"));
    }
    for (name, value) in extra {
        builder = builder.header(*name, *value);
    }
    builder.body(()).expect("a request")
}

/// Open one connection to the peer, the way the bridge would.
async fn connect_with(
    peer: &RealtimePeer,
    bearer: Option<&str>,
    extra: &[(&str, &str)],
) -> Result<Client, WsError> {
    let target = format!("{}?model=gpt-realtime-2.1", peer.url());
    let stream = TcpStream::connect(peer.addr()).await.expect("connects");
    client_async(upgrade_request(&target, bearer, extra), stream)
        .await
        .map(|(socket, _response)| socket)
}

/// The happy-path connection: the bearer the peer expects, no extra headers.
async fn connect(peer: &RealtimePeer) -> Client {
    connect_with(peer, Some(FIXTURE_BEARER), &[])
        .await
        .expect("the upgrade is accepted")
}

/// The next text frame, or a failure naming what was expected instead.
async fn next_text(client: &mut Client, expected: &str) -> String {
    match tokio::time::timeout(ARRIVAL, client.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => text.to_string(),
        other => panic!("expected {expected}, got {other:?}"),
    }
}

/// The next frame of any kind.
async fn next_frame(client: &mut Client, expected: &str) -> Message {
    match tokio::time::timeout(ARRIVAL, client.next()).await {
        Ok(Some(Ok(message))) => message,
        other => panic!("expected {expected}, got {other:?}"),
    }
}

/// The next text frame, parsed.
async fn next_event(client: &mut Client, expected: &str) -> Value {
    let text = next_text(client, expected).await;
    serde_json::from_str(&text).expect("an event parses as JSON")
}

/// The `type` member of a text frame, which is how every event in §5 identifies itself.
fn event_type(text: &str) -> String {
    let value: Value = serde_json::from_str(text).expect("an event parses as JSON");
    value["type"].as_str().expect("a string type").to_owned()
}

async fn send(client: &mut Client, event: &Value) {
    client
        .send(Message::Text(event.to_string().into()))
        .await
        .expect("the client writes");
}

/// Walk setup: `session.created` in, `session.update` out, `session.updated` in (§3).
async fn establish(peer: &RealtimePeer, client: &mut Client) {
    let created = next_text(client, "session.created").await;
    assert_eq!(event_type(&created), "session.created");
    send(
        client,
        &json!({"type": "session.update", "session": {"type": "realtime"}}),
    )
    .await;
    let updated = next_text(client, "session.updated").await;
    assert_eq!(event_type(&updated), "session.updated");
    peer.await_session_update()
        .await
        .expect("the peer observed the session.update");
}

// ---------------------------------------------------------------------------- the vectors ----

/// ORB-1 (A-20 owns the client side): the peer records what the upgrade carried, so the bridge's
/// half of the vector — `?model=`, the bearer's bytes, no retired beta header — is assertable.
#[tokio::test]
async fn orb_1_the_peer_records_the_upgrade_target_and_its_headers() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let _client = connect(&peer).await;
    peer.await_upgrade().await.expect("an upgrade");

    let record = peer.record();
    assert_eq!(record.upgrades.len(), 1);
    let upgrade = &record.upgrades[0];
    assert!(
        upgrade.target.contains("model=gpt-realtime-2.1"),
        "the model is selected by the query parameter: {}",
        upgrade.target
    );
    assert_eq!(
        upgrade.authorization.as_deref(),
        Some(&format!("Bearer {FIXTURE_BEARER}")[..]),
        "the upgrade carries the resolved bearer bytes"
    );
    assert!(
        !upgrade
            .header_names
            .iter()
            .any(|name| name == "openai-beta"),
        "the retired beta header must not be sent: {:?}",
        upgrade.header_names
    );
    assert_eq!(upgrade.outcome, UpgradeOutcome::Accepted);

    // Non-vacuity: the absence above is only evidence if the peer would have seen the header.
    let _beta = connect_with(
        &peer,
        Some(FIXTURE_BEARER),
        &[("OpenAI-Beta", "realtime=v1")],
    )
    .await
    .expect("the upgrade is accepted");
    let record = peer
        .observe("a second upgrade", |record| record.upgrades.len() == 2)
        .await
        .expect("a second upgrade");
    assert!(
        record.upgrades[1]
            .header_names
            .iter()
            .any(|name| name == "openai-beta"),
        "the peer can see the header it just reported absent: {:?}",
        record.upgrades[1].header_names
    );
}

/// ORB-5: over a full scripted conversation the peer observes **only** the three client events of
/// §5.1 — and would have noticed a fourth.
#[tokio::test]
async fn orb_5_the_peer_observes_only_the_three_client_events() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    send(
        &mut client,
        &json!({"type": "input_audio_buffer.append", "audio": F_SILENCE_BASE64}),
    )
    .await;
    send(
        &mut client,
        &json!({"type": "input_audio_buffer.append", "audio": BASE64.encode(tone_frame(0))}),
    )
    .await;
    send(&mut client, &json!({"type": "response.cancel"})).await;
    let record = peer
        .observe("two appends and a cancel", |record| {
            record.appends() == 2 && record.cancels() == 1
        })
        .await
        .expect("the scripted conversation");

    assert!(
        record.events_outside_the_client_subset().is_empty(),
        "only §5.1's three events may reach the peer: {:?}",
        record.client_events
    );
    assert!(matches!(
        record.client_events.as_slice(),
        [
            ClientEvent::SessionUpdate(_),
            ClientEvent::Append { .. },
            ClientEvent::Append { .. },
            ClientEvent::Cancel,
        ]
    ));

    // ORB-3's uplink bytes, from the peer's side: F-silence's 216-char literal decodes to the
    // 160 bytes the call put on the wire, and the peer keeps them for the bridge test to assert.
    let mut expected = F_SILENCE.to_vec();
    expected.extend_from_slice(&tone_frame(0));
    assert_eq!(record.appended_audio, expected);

    // Non-vacuity: a client event outside §5.1 is reported rather than silently absorbed.
    let mut stray = connect(&peer).await;
    establish(&peer, &mut stray).await;
    send(&mut stray, &json!({"type": "input_audio_buffer.commit"})).await;
    let record = peer
        .observe("a stray client event", |record| {
            !record.events_outside_the_client_subset().is_empty()
        })
        .await
        .expect("the stray event");
    assert_eq!(
        record.events_outside_the_client_subset(),
        vec!["input_audio_buffer.commit".to_owned()]
    );
}

/// ORB-10: a wrong bearer and an absent bearer are both refused before the upgrade — and the same
/// peer accepts the bearer it was configured with, so the refusal is a decision and not a peer
/// that never works.
#[tokio::test]
async fn orb_10_a_wrong_or_absent_bearer_is_refused_before_the_upgrade() {
    let peer = PeerConfig::new()
        .expecting_bearer("the-configured-key")
        .start()
        .await
        .expect("the peer binds");

    for bearer in [Some("the-wrong-key"), None] {
        match connect_with(&peer, bearer, &[]).await {
            Err(WsError::Http(response)) => {
                assert_eq!(response.status().as_u16(), 401);
                let body = String::from_utf8_lossy(response.body().as_deref().unwrap_or(b""));
                assert!(
                    !body.contains("the-wrong-key") && !body.contains("the-configured-key"),
                    "a refusal never carries the credential: {body}"
                );
            }
            other => panic!("expected a 401 before the upgrade, got {other:?}"),
        }
    }

    let record = peer
        .observe("two refusals", |record| record.upgrades.len() == 2)
        .await
        .expect("two refusals");
    assert_eq!(record.accepted(), 0, "no session may exist");
    assert_eq!(record.refused(), 2);
    assert!(
        record
            .upgrades
            .iter()
            .all(|upgrade| upgrade.outcome == UpgradeOutcome::Refused(401))
    );

    let mut client = connect_with(&peer, Some("the-configured-key"), &[])
        .await
        .expect("the configured bearer is accepted");
    assert_eq!(
        event_type(&next_text(&mut client, "session.created").await),
        "session.created"
    );
}

/// ORB-8's truncation half, and the story's Acceptance: after the client's `response.cancel` the
/// peer sends no further delta for that response, so a bridge test can assert truncation as a
/// fact rather than as a hope.
#[tokio::test]
async fn orb_8_a_cancelled_response_gets_no_further_deltas() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    assert_eq!(
        peer.send_delta("resp_001", &tone_frame(0))
            .await
            .expect("a delta"),
        Emission::Sent
    );
    assert_eq!(
        event_type(&next_text(&mut client, "a delta").await),
        "response.output_audio.delta"
    );

    send(&mut client, &json!({"type": "response.cancel"})).await;
    peer.await_cancel().await.expect("the cancel is observed");

    assert_eq!(
        peer.send_delta("resp_001", &tone_frame(1))
            .await
            .expect("a directed delta"),
        Emission::SuppressedByCancel,
        "the peer honours the cancel"
    );
    assert_eq!(
        peer.send_delta("resp_001", &tone_frame(2))
            .await
            .expect("a directed delta"),
        Emission::SuppressedByCancel
    );
    peer.send_response_done("resp_001", "cancelled")
        .await
        .expect("the response ends");

    // The next frame the client sees is the end of the response, never a fourth delta: the two
    // suppressed deltas are a fact on the wire, not a counter the peer keeps to itself.
    assert_eq!(
        event_type(&next_text(&mut client, "response.done").await),
        "response.done"
    );
    let record = peer.record();
    assert_eq!(record.deltas_sent, 1);
    assert_eq!(record.deltas_suppressed, 2);
}

/// ORB-8's other half: the vector's script has the peer send two more deltas *after* the cancel,
/// because `bridge_cancelled_deltas` counts events the peer chose to send. Truncation is the
/// default, so the lagging peer is configuration too.
#[tokio::test]
async fn orb_8_a_lagging_peer_keeps_streaming_after_the_cancel() {
    let peer = PeerConfig::new()
        .on_cancel(CancelPolicy::KeepStreaming)
        .start()
        .await
        .expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta");
    assert_eq!(
        event_type(&next_text(&mut client, "a delta").await),
        "response.output_audio.delta"
    );
    send(&mut client, &json!({"type": "response.cancel"})).await;
    peer.await_cancel().await.expect("the cancel is observed");

    for frame in 1..3 {
        assert_eq!(
            peer.send_delta("resp_001", &tone_frame(frame))
                .await
                .expect("a delta"),
            Emission::Sent
        );
        assert_eq!(
            event_type(&next_text(&mut client, "a late delta").await),
            "response.output_audio.delta"
        );
    }
    peer.send_response_done("resp_001", "completed")
        .await
        .expect("the response ends");
    assert_eq!(
        event_type(&next_text(&mut client, "response.done").await),
        "response.done"
    );
    assert_eq!(peer.record().deltas_suppressed, 0);
}

/// The tone the peer speaks is distinct, known, and tied to the spec: its first frame *is* §4.2's
/// F-ramp, whose base64 is a literal in this file's imports rather than a value computed by the
/// assertion (the WB-8 discipline §4.2 cites).
#[tokio::test]
async fn the_tone_begins_with_the_specs_f_ramp_vector() {
    assert_eq!(tone_frame(0).len(), FRAME_BYTES);
    assert_eq!(BASE64.encode(tone_frame(0)), F_RAMP_BASE64);
    assert_eq!(BASE64.encode(F_SILENCE), F_SILENCE_BASE64);
    assert_eq!(
        BASE64.decode(F_RAMP_BASE64).expect("the literal decodes"),
        (0u8..=0x9F).collect::<Vec<u8>>()
    );
    assert_ne!(
        tone_frame(1),
        tone_frame(0),
        "successive frames must be distinguishable at the far end"
    );
    assert_eq!(
        tone_bytes(2),
        [tone_frame(0), tone_frame(1)].concat(),
        "what a bridge should receive is what the peer speaks"
    );

    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;
    assert_eq!(
        peer.speak_tone("resp_001", 2).await.expect("the tone"),
        2,
        "both frames were emitted"
    );
    for frame in 0..2 {
        let text = next_text(&mut client, "a tone delta").await;
        let event: Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(event["type"], "response.output_audio.delta");
        assert_eq!(event["response_id"], "resp_001");
        assert_eq!(
            event["delta"].as_str().expect("a string delta"),
            BASE64.encode(tone_frame(frame))
        );
    }
}

/// ORB-11: the peer really puts more than a mebibyte on the wire, so the client's bound is tested
/// against a frame and not against a mock.
#[tokio::test]
async fn orb_11_the_peer_sends_a_frame_over_the_one_mebibyte_bound() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    peer.send_oversize(2 << 20)
        .await
        .expect("an oversize frame");
    let text = next_text(&mut client, "an oversize frame").await;
    assert!(
        text.len() >= 2 << 20,
        "the frame must exceed §5.3's 1 MiB bound: {} bytes",
        text.len()
    );
    assert_eq!(event_type(&text), "response.output_audio.delta");
}

/// ORB-12: the peer emits events outside §5.2 verbatim — the forward-compatibility half of the
/// claim ORB-5 closes from the other side.
#[tokio::test]
async fn orb_12_unknown_events_reach_the_client_verbatim() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    peer.send_unknown("rate_limits.updated")
        .await
        .expect("an ignorable event");
    peer.send_unknown("response.output_audio_transcript.delta")
        .await
        .expect("a future event");
    assert_eq!(
        event_type(&next_text(&mut client, "rate_limits.updated").await),
        "rate_limits.updated"
    );
    assert_eq!(
        event_type(&next_text(&mut client, "a future event").await),
        "response.output_audio_transcript.delta"
    );
}

/// ORB-13: `not json{`, a frame with no `type`, and a binary frame all really leave the peer.
#[tokio::test]
async fn orb_13_the_peer_sends_frames_that_cannot_be_read_as_events() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    peer.send_malformed(Malformed::NotJson)
        .await
        .expect("a malformed frame");
    let text = next_text(&mut client, "unparseable text").await;
    assert_eq!(text, "not json{");
    assert!(serde_json::from_str::<Value>(&text).is_err());

    peer.send_malformed(Malformed::NoType)
        .await
        .expect("a typeless frame");
    let text = next_text(&mut client, "a typeless event").await;
    let event: Value = serde_json::from_str(&text).expect("it parses");
    assert!(event.get("type").is_none(), "no `type` member: {text}");

    peer.send_malformed(Malformed::Binary)
        .await
        .expect("a binary frame");
    match next_frame(&mut client, "a binary frame").await {
        Message::Binary(bytes) => assert!(!bytes.is_empty()),
        other => panic!("expected a binary frame, got {other:?}"),
    }
}

/// ORB-18: the read-set violations of §5.3, each a real frame — an unusable `delta`, an absent
/// `delta`, and a `response.output_audio.done` with no `response_id`.
#[tokio::test]
async fn orb_18_the_peer_sends_deltas_that_fail_the_read_set() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    peer.send_malformed(Malformed::DeltaNotBase64 {
        response: "resp_001".to_owned(),
    })
    .await
    .expect("a delta that is not base64");
    let event: Value = serde_json::from_str(&next_text(&mut client, "a bad delta").await)
        .expect("the event parses");
    assert_eq!(event["type"], "response.output_audio.delta");
    assert_eq!(event["delta"], "not base64!!");
    assert!(
        BASE64.decode("not base64!!").is_err(),
        "the member must really fail RFC 4648 §4"
    );

    peer.send_malformed(Malformed::DeltaMissing {
        response: "resp_001".to_owned(),
    })
    .await
    .expect("a delta with no delta member");
    let event: Value = serde_json::from_str(&next_text(&mut client, "a memberless delta").await)
        .expect("the event parses");
    assert_eq!(event["type"], "response.output_audio.delta");
    assert!(event.get("delta").is_none(), "no `delta` member: {event}");

    peer.send_malformed(Malformed::AudioDoneWithoutResponseId)
        .await
        .expect("a done with no response_id");
    let event: Value = serde_json::from_str(&next_text(&mut client, "a done event").await)
        .expect("the event parses");
    assert_eq!(event["type"], "response.output_audio.done");
    assert!(event.get("response_id").is_none());
}

/// ORB-14: the peer answers the upgrade and then goes silent — no events, and no Pong, which is
/// what makes the client's liveness timer the only thing that can end the session. The control
/// arm proves the silence is the mode and not the fixture.
#[tokio::test]
async fn orb_14_a_stalled_peer_answers_the_upgrade_and_then_nothing() {
    let peer = PeerConfig::new()
        .stalling_at(StallPoint::Upgrade)
        .start()
        .await
        .expect("the peer binds");
    let mut client = connect(&peer).await;
    peer.await_upgrade().await.expect("an upgrade");
    assert_eq!(peer.record().accepted(), 1, "the upgrade completed");

    client
        .send(Message::Ping(WsBytes::from_static(b"live?")))
        .await
        .expect("the client pings");
    // QUIET defines silence: nothing may arrive at all, so a slower machine only makes the hole
    // longer than the window rather than shorter.
    let quiet = tokio::time::timeout(QUIET, client.next()).await;
    assert!(
        quiet.is_err(),
        "a stalled peer answers nothing, got {quiet:?}"
    );

    let lively = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&lively).await;
    assert_eq!(
        event_type(&next_text(&mut client, "session.created").await),
        "session.created"
    );
    client
        .send(Message::Ping(WsBytes::from_static(b"live?")))
        .await
        .expect("the client pings");
    assert!(
        matches!(next_frame(&mut client, "a pong").await, Message::Pong(_)),
        "an unstalled peer answers the ping, so the silence above is the mode"
    );
}

/// The mid-call stall: setup is answered, audio starts, and *then* the far end goes quiet without
/// closing. The difference from ORB-16's close is the whole point — nothing arrives, including the
/// EOF that would let the client end the session on its own, so only the liveness timer can.
#[tokio::test]
async fn a_mid_call_stall_reads_the_frame_and_then_answers_nothing() {
    let peer = PeerConfig::new()
        .stalling_at(StallPoint::Session)
        .start()
        .await
        .expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    send(
        &mut client,
        &json!({"type": "input_audio_buffer.append", "audio": F_SILENCE_BASE64}),
    )
    .await;
    let record = peer.await_appends(1).await.expect("the frame was read");
    assert_eq!(
        record.appended_audio,
        F_SILENCE.to_vec(),
        "the peer was alive when it went quiet"
    );

    client
        .send(Message::Ping(WsBytes::from_static(b"live?")))
        .await
        .expect("the client pings");
    // A definition of silence. The window has to be empty of *everything*: a Pong would mean the
    // peer is still serving, and a close or an EOF would mean it ended the session instead of
    // stalling. `Elapsed` is the only result that says the connection is open and unanswered.
    let quiet = tokio::time::timeout(QUIET, client.next()).await;
    assert!(
        quiet.is_err(),
        "a mid-call stall neither answers nor closes, got {quiet:?}"
    );
}

/// The peer's half of the barge-in and cancel-race scripts (§4.3, ORB-8 and ORB-9): every server
/// event the bridge consumes can be put on the wire, in order, with the members §5.2 names.
#[tokio::test]
async fn the_peer_scripts_every_server_event_the_bridge_consumes() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;

    peer.send_speech_started().await.expect("speech_started");
    let event = next_event(&mut client, "speech_started").await;
    assert_eq!(event["type"], "input_audio_buffer.speech_started");
    assert!(event.get("audio_start_ms").is_some());

    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta");
    assert_eq!(
        next_event(&mut client, "a delta").await["type"],
        "response.output_audio.delta"
    );

    peer.send_audio_done("resp_001").await.expect("audio done");
    let event = next_event(&mut client, "audio done").await;
    assert_eq!(event["type"], "response.output_audio.done");
    assert_eq!(event["response_id"], "resp_001");

    peer.send_error("response_cancel_not_active", "no active response")
        .await
        .expect("an error");
    let event = next_event(&mut client, "an error").await;
    assert_eq!(event["type"], "error");
    assert_eq!(event["error"]["code"], "response_cancel_not_active");

    peer.send_response_done("resp_001", "completed")
        .await
        .expect("the response ends");
    let event = next_event(&mut client, "response.done").await;
    assert_eq!(event["type"], "response.done");
    assert_eq!(event["response"]["status"], "completed");

    assert!(
        peer.record().events_outside_the_client_subset().is_empty(),
        "the script cost the client nothing outside §5.1"
    );
}

/// ORB-15: `session.created` withheld, and separately `session.updated` — each the peer being
/// deliberately quiet while demonstrably alive, which is what separates this from a dead socket.
#[tokio::test]
async fn orb_15_the_peer_withholds_the_setup_acknowledgements() {
    let peer = PeerConfig::new()
        .withholding(Withhold::SessionCreated)
        .start()
        .await
        .expect("the peer binds");
    let mut client = connect(&peer).await;
    peer.await_upgrade().await.expect("an upgrade");
    let quiet = tokio::time::timeout(QUIET, client.next()).await; // a definition of silence
    assert!(quiet.is_err(), "no session.created may arrive: {quiet:?}");
    // Alive, not dead: the same socket still reads and still answers a session.update.
    send(
        &mut client,
        &json!({"type": "session.update", "session": {"type": "realtime"}}),
    )
    .await;
    assert_eq!(
        event_type(&next_text(&mut client, "session.updated").await),
        "session.updated"
    );

    let peer = PeerConfig::new()
        .withholding(Withhold::SessionUpdated)
        .start()
        .await
        .expect("the peer binds");
    let mut client = connect(&peer).await;
    assert_eq!(
        event_type(&next_text(&mut client, "session.created").await),
        "session.created"
    );
    send(
        &mut client,
        &json!({"type": "session.update", "session": {"type": "realtime"}}),
    )
    .await;
    peer.await_session_update()
        .await
        .expect("the peer read the update");
    let quiet = tokio::time::timeout(QUIET, client.next()).await; // a definition of silence
    assert!(
        quiet.is_err(),
        "the peer read the update and answered nothing: {quiet:?}"
    );
}

/// ORB-16: a normal close carrying 1000, and an abrupt reset that ends the connection with no
/// close handshake at all. Both leave exactly one upgrade behind, which is the observation a
/// bridge test uses to prove it did not reconnect.
#[tokio::test]
async fn orb_16_the_peer_closes_normally_and_resets() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;
    peer.close_normally().await.expect("a close");
    match next_frame(&mut client, "a close frame").await {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), 1000),
        other => panic!("expected a 1000 close, got {other:?}"),
    }
    // The peer does not drop the socket the instant it writes the close: it waits for the echo,
    // which is what keeps a client's own close frame from landing on a socket that has gone. One
    // more read is what sends that echo, and the session ends on it — the handshake, not a timer.
    let echoed = tokio::time::timeout(ARRIVAL, client.next()).await; // a bound on failure
    assert!(
        matches!(echoed, Ok(None)),
        "the close handshake completes, got {echoed:?}"
    );
    let record = peer
        .observe("the session ending", |record| record.sessions_ended == 1)
        .await
        .expect("the session ends");
    assert_eq!(record.upgrades.len(), 1, "no second upgrade");

    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;
    peer.reset().await.expect("a reset");
    let ending = tokio::time::timeout(ARRIVAL, client.next()).await; // a bound on failure
    assert!(
        !matches!(ending, Ok(Some(Ok(Message::Close(_))))),
        "an abrupt reset gives the client no close handshake, got {ending:?}"
    );
    assert_eq!(peer.record().upgrades.len(), 1, "no second upgrade");
}

/// The peer is bounded by its own handle: dropping it ends the session and frees the port, so a
/// test that forgets to shut it down cannot leave a listener behind.
#[tokio::test]
async fn the_peer_stops_serving_when_it_is_dropped() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let addr = peer.addr();
    let mut client = connect(&peer).await;
    establish(&peer, &mut client).await;
    drop(peer);

    let ending = tokio::time::timeout(ARRIVAL, client.next()).await; // a bound on failure
    assert!(
        matches!(ending, Ok(None | Some(Err(_) | Ok(Message::Close(_))))),
        "the session ends when the peer is dropped, got {ending:?}"
    );

    // The listener goes with it. The loop completes on the refused connection, never on the
    // interval: the wait is only how often the question is asked.
    let closed = tokio::time::timeout(ARRIVAL, async {
        loop {
            if TcpStream::connect(addr).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(closed.is_ok(), "the port is still accepting connections");
}
