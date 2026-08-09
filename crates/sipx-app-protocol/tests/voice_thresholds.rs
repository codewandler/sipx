//! What a call's voice-activity detection is measuring against, as an app-protocol client sees it
//! (`M-84`).
//!
//! `M-60` made a call's effective thresholds readable from Rust — `Call::voice_thresholds` — and
//! stopped there. These tests are the wire half of the same question, and they are written **in
//! wire terms on purpose**: an app-protocol client is not a Rust caller, it is whatever reads the
//! JSON, so the assertions go through [`Envelope`]'s text rather than through a field of this
//! crate's types. A test that read a Rust field would pass for an SDK that never sees one.
//!
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5.2 (the `voice` member of
//! the call snapshot) and §5.3 (`call.voice.thresholds`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_app_protocol::json::Json;
use sipx_app_protocol::{
    CallSnapshot, Direction, EventKind, Input, Interpreter, Output, Policy, Timestamp,
};

/// A fixed instant. The interpreter never asks what time it is (§2).
fn now() -> Timestamp {
    Timestamp::from_unix_millis(1_772_270_104_221)
}

fn interpreter() -> Interpreter {
    Interpreter::new(
        CallSnapshot::new("b7c1", Direction::Inbound)
            .between("sip:alice@example.com", "sip:support@example.net"),
        Policy::default(),
    )
}

/// The envelope the app is handed, as the JSON text it is handed.
fn delivered(outputs: Vec<Output>) -> Json {
    for output in outputs {
        if let Output::Deliver { envelope, .. } = output {
            return Json::parse(&envelope.to_text()).expect("an envelope is JSON");
        }
    }
    panic!("expected a delivery");
}

/// One event read off the wire, exactly as a host that speaks `sipx.app.v1` would write it.
///
/// Built from text rather than from a variant so that the test states the *contract's* vocabulary
/// and not this crate's spelling of it.
fn event(text: &str) -> EventKind {
    EventKind::from_json(&Json::parse(text).expect("the fixture is JSON")).expect("a known event")
}

/// A host with calibration running announces the threshold; the client reads it off the snapshot.
///
/// §12.9 of the processing contract lists what an analyser lets an application read, and every
/// field of it is a count or an amplitude. This asserts the same set reached an app that never
/// links `sipx-audio` at all — which is the whole of `M-84`.
#[test]
fn an_app_protocol_client_reads_the_thresholds_of_a_call_with_detection_running() {
    let mut interpreter = interpreter();
    let outputs = interpreter.handle(
        now(),
        Input::Event(event(
            r#"{"type":"call.voice.thresholds","sample_time":1600,
                "thresholds":{"direction":"inbound","sample_rate":8000,
                              "activation_amplitude":1536,"window_samples":160,
                              "hangover_samples":1600,"silence_timeout_samples":null,
                              "calibration_samples":1600,"update_samples":800,
                              "freeze_limit_samples":240000}}"#,
        )),
    );

    let envelope = delivered(outputs);
    let voice = envelope
        .get("call")
        .and_then(|call| call.get("voice"))
        .expect("a call with detection running says what it is measuring against");
    assert_eq!(
        voice.get("activation_amplitude").and_then(Json::as_i64),
        Some(1_536),
        "the effective threshold §5.3's `active` predicate is comparing against"
    );
    assert_eq!(
        voice.get("window_samples").and_then(Json::as_i64),
        Some(160),
        "and the window it is compared over, in samples"
    );
    assert_eq!(
        voice.get("hangover_samples").and_then(Json::as_i64),
        Some(1_600)
    );
    assert_eq!(
        voice.get("direction").and_then(Json::as_str),
        Some("inbound")
    );
    assert_eq!(
        voice.get("calibration_samples").and_then(Json::as_i64),
        Some(1_600),
        "a threshold that can move says so, so an app knows to expect the announcement"
    );

    // The move itself is on the event, in samples from the start of the epoch — never a clock
    // reading, which is what makes a recorded call reproduce the same position.
    let moved = envelope
        .get("event")
        .expect("an envelope carries its event");
    assert_eq!(
        moved.get("type").and_then(Json::as_str),
        Some("call.voice.thresholds")
    );
    assert_eq!(moved.get("sample_time").and_then(Json::as_i64), Some(1_600));
}

/// The same read, for a client that *is* holding this crate: the envelope goes out as text and
/// comes back as a value with the record on it.
///
/// The wire test above is the contract; this is the SDK ergonomics of it, and having both is what
/// says the JSON and the type describe the same thing.
#[test]
fn the_record_survives_the_wire_as_a_value() {
    let mut interpreter = interpreter();
    let announced = EventKind::VoiceThresholds {
        sample_time: 1_600,
        thresholds: sipx_app_protocol::testing::reference_thresholds(),
    };
    let outputs = interpreter.handle(now(), Input::Event(announced.clone()));

    let text = match outputs.into_iter().find_map(|output| match output {
        Output::Deliver { envelope, .. } => Some(envelope),
        _ => None,
    }) {
        Some(envelope) => envelope.to_text(),
        None => panic!("expected a delivery"),
    };
    let back = sipx_app_protocol::Envelope::from_text(&text).expect("an app-protocol client reads");
    assert_eq!(back.event, announced, "the announcement round trips");
    assert_eq!(
        back.call.voice,
        Some(sipx_app_protocol::testing::reference_thresholds()),
        "and the snapshot on it is the read"
    );
    let voice = back.call.voice.expect("just asserted present");
    assert!(voice.is_calibrated(), "this one moves, and says so");
    assert_eq!(voice.activation_amplitude, 1_536);
    assert_eq!(voice.window_samples, 160);
}

/// A call nobody asked for detection on says nothing about thresholds.
///
/// The member is absent rather than null: §5.3's own rule is that a host emitting no voice
/// analysis is conformant, so "this call is not measuring anything" has to be representable
/// without inventing a threshold for it.
#[test]
fn a_call_without_detection_carries_no_thresholds() {
    let mut interpreter = interpreter();
    let outputs = interpreter.handle(now(), Input::Event(event(r#"{"type":"call.answered"}"#)));

    let envelope = delivered(outputs);
    let call = envelope
        .get("call")
        .expect("every event carries a snapshot");
    assert!(
        call.get("voice").is_none(),
        "a call with no detection running has no thresholds to report: {}",
        envelope.to_text()
    );
}
