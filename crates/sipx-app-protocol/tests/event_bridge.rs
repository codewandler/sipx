//! Every arm of [`event_from_call`], the one function that maps a call's events into the
//! application contract's (`M-98`).
//!
//! It had **no tests at all** until this file, and that is the more important half of what `M-98`
//! is about. `M-59` specified `call.signal.metrics` and `call.signal.silence`, gave them
//! [`EventKind`] variants and a wire round trip, and never gave them an arm here — so for two
//! releases the contract named two events no host could emit, and every derived test the crate has
//! still passed. An untested seam between two vocabularies is how that happens.
//!
//! The reason it had no tests is worth stating, because it was not laziness: `sipx-call`'s payload
//! types had private fields and no constructors, so nothing outside that crate could build the
//! *input*. `M-98` closed that — [`VoiceActivity::new`], [`VoiceThresholds::new`],
//! [`SignalMetrics::new`], [`sipx_call::signal_metrics::measure`] and
//! [`sipx_media::PlaybackId::new`] — and the metric values below are not typed here at all: they
//! come out of the same analyser and reducer a live call runs, over samples this file states.
//!
//! One `CallEvent` variant is deliberately absent from these tests and cannot be added.
//! `CallEvent::ApplicationRequest` owns a live server transaction's response capability, so a
//! forged one would be a lie about a transaction that does not exist. Its arm binds nothing and
//! yields `None`; the only way it could be wrong is a misspelled variant, which the compiler
//! checks. `tests/spec_tables.rs` covers the direction that actually went wrong — a §5.3 row with
//! no arm at all.
//!
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §5.3.

#![cfg(feature = "call")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use sipx_app_protocol::json::Json;
use sipx_app_protocol::{
    AudioDirection, CallSnapshot, Direction, EndCause, EventKind, Input, Interpreter, Output,
    Policy, Timestamp, TransferState, VoiceEndCause, event_from_call,
};
use sipx_call::signal_metrics::{
    ResetCause, SignalObservation, SignalReportProfile, measure as measure_signal,
};
use sipx_call::voice::{
    AnalysisProfile, AudioDirection as CallAudioDirection, CalibrationProfile,
    VoiceEndCause as CallVoiceEndCause,
};
use sipx_call::{
    CallEvent, EndCause as CallEndCause, SignalMetrics, TransferState as CallTransferState,
    UnbridgeCause, VoiceActivity, VoiceThresholds,
};

/// This call's `Call-ID`, which no contract event repeats: §5.2's snapshot names the call every
/// envelope is about.
const CALL_ID: &str = "b7c1";

/// A fixed instant. The interpreter never asks what time it is (§2).
fn now() -> Timestamp {
    Timestamp::from_unix_millis(1_772_270_104_221)
}

fn interpreter() -> Interpreter {
    Interpreter::new(
        CallSnapshot::new(CALL_ID, Direction::Inbound)
            .between("sip:alice@example.com", "sip:support@example.net"),
        Policy::default(),
    )
}

/// One call event, as the JSON an app-protocol client is actually handed.
///
/// Deliberately the whole path — bridge, interpreter, envelope, text — rather than a comparison
/// against an [`EventKind`] value. What a remote app receives is the JSON; a test that stopped at
/// the Rust variant would pass for a client that never sees one.
fn received(event: &CallEvent, instruction_id: &str, bridged_leg: Option<&str>) -> Json {
    let kind = event_from_call(event, instruction_id, bridged_leg)
        .unwrap_or_else(|| panic!("§5.3 has a row for {event:?} and the bridge produced nothing"));
    for output in interpreter().handle(now(), Input::Event(kind)) {
        if let Output::Deliver { envelope, .. } = output {
            let envelope = Json::parse(&envelope.to_text()).expect("an envelope is JSON");
            return envelope
                .get("event")
                .expect("every envelope carries its event")
                .clone();
        }
    }
    panic!("expected a delivery");
}

/// §11.1's reference analysis profile at 8 kHz, inbound: a 20 ms window, so 160 samples.
fn analysis() -> AnalysisProfile {
    AnalysisProfile::new(CallAudioDirection::Inbound, 8_000)
}

/// The observations a call with `Call::report_signal_metrics` running would have reported over
/// `samples`, measured by the same analyser and reducer that call uses.
fn measured(profile: SignalReportProfile, samples: &[i16]) -> Vec<SignalMetrics> {
    measure_signal(CALL_ID, profile, samples).expect("§11.1's reference profile is in domain")
}

/// The one silence transition in 16,000 samples of digital silence at 8 kHz — the profile's own
/// 2,000 ms timeout, reached exactly once and re-armed only by a non-silent window or a reset.
fn silence_transition() -> SignalMetrics {
    let profile = SignalReportProfile::new(analysis().with_silence_timeout_ms(Some(2_000)));
    measured(profile, &vec![0i16; 16_000])
        .into_iter()
        .find(|metrics| {
            matches!(
                metrics.observation(),
                SignalObservation::SilenceElapsed { .. }
            )
        })
        .expect("unbroken silence reaching the configured timeout is reported once")
}

/// **The defect this story exists for.** A call with `Call::report_signal_metrics` running produces
/// `call.signal.metrics`, and until `M-98` no host could emit one.
///
/// The numbers are not typed here. One 20 ms window of full-scale audio goes through the real
/// analyser and the real reducer, and what the client reads back is that measurement: the whole
/// window clipped, nothing silent, and the coverage named in samples at the rate it was counted at.
#[test]
fn an_app_protocol_client_receives_the_metrics_of_a_call_with_reporting_running() {
    let reports = measured(SignalReportProfile::new(analysis()), &[32_767i16; 160]);
    assert_eq!(reports.len(), 1, "one window, one report: {reports:?}");

    let event = received(&CallEvent::SignalMetrics(reports[0].clone()), "i09", None);
    assert_eq!(
        event.get("type").and_then(Json::as_str),
        Some("call.signal.metrics"),
        "the row `M-59` specified, reachable from a call at last"
    );
    assert_eq!(
        event.get("direction").and_then(Json::as_str),
        Some("inbound")
    );
    assert_eq!(event.get("epoch").and_then(Json::as_i64), Some(0));
    assert_eq!(event.get("sequence").and_then(Json::as_i64), Some(0));
    assert_eq!(event.get("sample_time").and_then(Json::as_i64), Some(0));
    assert_eq!(event.get("sample_rate").and_then(Json::as_i64), Some(8_000));
    assert_eq!(event.get("samples").and_then(Json::as_i64), Some(160));
    assert_eq!(event.get("windows").and_then(Json::as_i64), Some(1));
    assert_eq!(
        event.get("peak").and_then(Json::as_i64),
        Some(32_767),
        "the largest magnitude over the coverage, in sample-amplitude units"
    );
    assert_eq!(event.get("rms").and_then(Json::as_i64), Some(32_767));
    assert_eq!(
        event.get("clipped_samples").and_then(Json::as_i64),
        Some(160)
    );
    assert_eq!(
        event.get("clipping_windows").and_then(Json::as_i64),
        Some(1)
    );
    assert_eq!(event.get("silent_windows").and_then(Json::as_i64), Some(0));
    assert_eq!(event.get("active_windows").and_then(Json::as_i64), Some(0));
}

/// The other half of the same defect: `call.signal.silence`.
///
/// 16,000 samples of digital silence at 8 kHz is the profile's own 2,000 ms timeout, and the
/// transition is named at the run's *first* sample rather than at the position the timer expired —
/// the same rule §5.3 states for a voice ending.
#[test]
fn an_app_protocol_client_receives_the_silence_of_a_call_with_reporting_running() {
    let event = received(&CallEvent::SignalMetrics(silence_transition()), "i09", None);
    assert_eq!(
        event.get("type").and_then(Json::as_str),
        Some("call.signal.silence")
    );
    assert_eq!(
        event.get("direction").and_then(Json::as_str),
        Some("inbound")
    );
    assert_eq!(event.get("epoch").and_then(Json::as_i64), Some(0));
    assert_eq!(
        event.get("sample_time").and_then(Json::as_i64),
        Some(0),
        "the run's first sample, not where the timer expired"
    );
    assert_eq!(event.get("sample_rate").and_then(Json::as_i64), Some(8_000));
}

/// Every §5.3 row the bridge produces, reached from the call event that produces it.
///
/// The type name is all this asserts; the arms whose *mapping* has a decision in it are asserted
/// one by one below. Together they are the coverage `tests/spec_tables.rs` cannot give: that test
/// reads the bridge's source, and source containing a variant name is not the same claim as a call
/// event arriving at it.
#[test]
fn every_row_the_bridge_produces_is_reached_by_a_call_event() {
    let reports = measured(SignalReportProfile::new(analysis()), &[32_767i16; 160]);
    let cases: Vec<(CallEvent, &str)> = vec![
        (CallEvent::Ringing { reliable: true }, "call.ringing"),
        (CallEvent::EarlyMediaStarted, "call.early_media.started"),
        (CallEvent::Answered, "call.answered"),
        (
            CallEvent::Dtmf {
                digit: sipx_rtp::Digit::Number(5),
                duration: Duration::from_millis(160),
            },
            "call.dtmf",
        ),
        (
            CallEvent::VoiceStarted(activity(0, 320)),
            "call.voice.started",
        ),
        (
            CallEvent::VoiceEnded {
                activity: activity(1, 1_920),
                cause: CallVoiceEndCause::Hangover,
            },
            "call.voice.ended",
        ),
        (
            CallEvent::VoiceThresholds(thresholds(analysis())),
            "call.voice.thresholds",
        ),
        (
            CallEvent::SignalMetrics(reports[0].clone()),
            "call.signal.metrics",
        ),
        (
            CallEvent::SignalMetrics(silence_transition()),
            "call.signal.silence",
        ),
        (
            CallEvent::PlaybackFinished {
                playback: sipx_media::PlaybackId::new(7),
                completed: true,
            },
            "call.playback.finished",
        ),
        (
            CallEvent::RecordingFinished {
                duration: Duration::from_millis(4_200),
            },
            "call.recording.finished",
        ),
        (
            CallEvent::TransferRequested {
                target: target(),
                attended: false,
            },
            "call.transfer.requested",
        ),
        (
            CallEvent::TransferProgress(CallTransferState::Ringing),
            "call.transfer.progress",
        ),
        (CallEvent::Bridged, "call.bridged"),
        (
            CallEvent::Unbridged {
                cause: UnbridgeCause::Released,
            },
            "call.unbridged",
        ),
        (CallEvent::Hold, "call.hold"),
        (CallEvent::Resumed, "call.resumed"),
        (CallEvent::Ended(CallEndCause::LocalHangup), "call.ended"),
    ];

    // Every case is offered a coupled leg, including the fifteen rows that have no use for one:
    // the name belongs to two arms, and an arm that started reading it would be putting a field on
    // a row §5.3 does not give one.
    for (event, expected) in cases {
        let kind = event_from_call(&event, "i09", Some("b"))
            .unwrap_or_else(|| panic!("no host can emit {expected}: {event:?} reaches no arm"));
        assert_eq!(kind.type_name(), expected, "for {event:?}");
        assert_eq!(
            kind.to_json().get("leg").and_then(Json::as_str).is_some(),
            matches!(
                kind,
                EventKind::Bridged { .. } | EventKind::Unbridged { .. }
            ),
            "only §5.3's coupling rows carry a leg: {expected}"
        );
    }
}

/// A voice transition carries where it sat and where it sits in this call's ordered observations,
/// and drops the call identity `sipx-call` puts on it.
///
/// §5.2's snapshot already names the call every envelope is about, and a second spelling of it in
/// the event body is a second thing that can disagree.
#[test]
fn a_voice_transition_carries_its_position_and_not_a_second_call_identity() {
    let started = received(&CallEvent::VoiceStarted(activity(4, 3_200)), "i06", None);
    assert_eq!(
        started.get("type").and_then(Json::as_str),
        Some("call.voice.started")
    );
    assert_eq!(started.get("sequence").and_then(Json::as_i64), Some(4));
    assert_eq!(
        started.get("sample_time").and_then(Json::as_i64),
        Some(3_200)
    );
    assert_eq!(
        started.get("sample_rate").and_then(Json::as_i64),
        Some(8_000)
    );
    assert_eq!(
        started.get("direction").and_then(Json::as_str),
        Some("inbound")
    );
    assert!(
        started.get("call_id").is_none(),
        "the call is the envelope's, not the event body's: {}",
        started.to_text()
    );

    // The analyser's vocabulary is `#[non_exhaustive]`; §5.3's is closed. A hangover is
    // `hangover`, and every reset of any kind is `cut`.
    assert_eq!(
        event_from_call(
            &CallEvent::VoiceEnded {
                activity: activity(5, 4_800),
                cause: CallVoiceEndCause::Hangover,
            },
            "i06",
            None,
        ),
        Some(EventKind::VoiceEnded {
            direction: AudioDirection::Inbound,
            sequence: 5,
            sample_time: 4_800,
            sample_rate: 8_000,
            cause: VoiceEndCause::Hangover,
        })
    );
}

/// The threshold announcement is exactly §5.2.1's record — the same one the crate's own fixture and
/// the specification's example are held against.
///
/// Asserting equality with [`sipx_app_protocol::testing::reference_thresholds`] is what ties the
/// three together: `tests/spec_tables.rs` already holds that fixture against §5.2's example and
/// §5.2.1's table, so a bridge that built the record differently would now fail here rather than
/// quietly send a fourth shape.
#[test]
fn the_threshold_announcement_is_the_record_the_specification_shows() {
    let calibrating = analysis()
        .with_activation_amplitude(1_536)
        .with_silence_timeout_ms(None)
        .with_calibration(Some(CalibrationProfile::new()));
    assert_eq!(
        event_from_call(
            &CallEvent::VoiceThresholds(thresholds(calibrating)),
            "i08",
            None
        ),
        Some(EventKind::VoiceThresholds {
            sample_time: 1_600,
            thresholds: sipx_app_protocol::testing::reference_thresholds(),
        })
    );

    // A profile with no calibration: §12.3 derives the three calibration counts together or not at
    // all, so an analyser whose threshold cannot move says so by carrying none of them.
    let fixed = match event_from_call(
        &CallEvent::VoiceThresholds(thresholds(analysis())),
        "i08",
        None,
    ) {
        Some(EventKind::VoiceThresholds { thresholds, .. }) => thresholds,
        other => panic!("expected an announcement: {other:?}"),
    };
    assert!(
        !fixed.is_calibrated(),
        "a threshold that cannot move says so: {:?}",
        fixed.to_json().to_text()
    );
    assert_eq!(
        fixed.activation_amplitude, 2_048,
        "§11.1's reference activation amplitude, unmoved"
    );
    assert_eq!(fixed.window_samples, 160);
    assert_eq!(fixed.hangover_samples, 1_600);
    assert_eq!(
        fixed.silence_timeout_samples,
        Some(16_000),
        "the profile's 2,000 ms timeout, in samples at the rate it is counted at"
    );

    // And the same record on the wire, because what an app receives is the JSON.
    let event = received(
        &CallEvent::VoiceThresholds(thresholds(analysis())),
        "i08",
        None,
    );
    let record = event
        .get("thresholds")
        .expect("the announcement carries the record");
    assert_eq!(
        record.get("activation_amplitude").and_then(Json::as_i64),
        Some(2_048)
    );
    assert_eq!(
        record.get("window_samples").and_then(Json::as_i64),
        Some(160)
    );
    assert_eq!(
        record.get("hangover_samples").and_then(Json::as_i64),
        Some(1_600)
    );
}

/// The correlation ids §5.3 asks for are the driver's, not the call's.
///
/// `sipx-call` names a playback by its own id and a recording by nothing at all; the contract names
/// both by the **app's** instruction id (§6.1), so the id the driver passes in is the one that
/// comes out.
#[test]
fn a_completion_is_named_by_the_apps_instruction_id() {
    let playback = received(
        &CallEvent::PlaybackFinished {
            playback: sipx_media::PlaybackId::new(7),
            completed: false,
        },
        "i04",
        None,
    );
    assert_eq!(
        playback.get("instruction_id").and_then(Json::as_str),
        Some("i04"),
        "never the `PlaybackId`, which means nothing to an app"
    );
    assert_eq!(
        playback.get("completed").and_then(Json::as_bool),
        Some(false),
        "a clip that was cut short did not run to the end"
    );

    let recording = received(
        &CallEvent::RecordingFinished {
            duration: Duration::from_millis(4_200),
        },
        "i06",
        None,
    );
    assert_eq!(
        recording.get("instruction_id").and_then(Json::as_str),
        Some("i06")
    );
    assert_eq!(
        recording.get("duration_ms").and_then(Json::as_i64),
        Some(4_200)
    );
}

/// §5.3's four transfer states, and the reason phrase that does not travel.
#[test]
fn a_transfer_reports_the_four_states_the_contract_spells() {
    let cases = [
        (CallTransferState::Trying, TransferState::Trying),
        (CallTransferState::Ringing, TransferState::Ringing),
        (CallTransferState::Succeeded, TransferState::Succeeded),
        (
            CallTransferState::Failed {
                status: 480,
                reason: "Temporarily Unavailable".to_owned(),
            },
            TransferState::Failed { status: 480 },
        ),
    ];
    for (from, expected) in cases {
        assert_eq!(
            event_from_call(&CallEvent::TransferProgress(from.clone()), "i10", None),
            Some(EventKind::TransferProgress { state: expected }),
            "for {from:?}"
        );
    }

    let requested = received(
        &CallEvent::TransferRequested {
            target: target(),
            attended: true,
        },
        "i10",
        None,
    );
    assert_eq!(
        requested.get("target").and_then(Json::as_str),
        Some("sip:198.51.100.7")
    );
    assert_eq!(
        requested.get("attended").and_then(Json::as_bool),
        Some(true)
    );
}

/// §5.3's five end causes, from the five `sipx-call` spells.
///
/// The pair worth stating is `RemoteBye` and `RemoteCancel`. `sipx-call` keeps them apart because a
/// host matching on that enum has to tell "stop ringing" from "hang up", and its own documentation
/// says both are the contract's `remote`. They were not: `RemoteCancel` fell through to `error`,
/// which tells an application the host could not go on when the far end simply changed its mind
/// (`M-98`).
#[test]
fn both_far_end_endings_are_the_contracts_remote_cause() {
    let cases = [
        (CallEndCause::LocalHangup, EndCause::Hangup),
        (CallEndCause::RemoteBye, EndCause::Remote),
        (CallEndCause::RemoteCancel, EndCause::Remote),
        (
            CallEndCause::Rejected { status: 486 },
            EndCause::Rejected { status: 486 },
        ),
        (CallEndCause::Timeout, EndCause::Timeout),
    ];
    for (from, expected) in cases {
        assert_eq!(
            event_from_call(&CallEvent::Ended(from), "i11", None),
            Some(EventKind::Ended { cause: expected }),
            "for {from:?}"
        );
    }
}

/// The events that are deliberately not delivered, each for the reason the bridge states.
///
/// This is the arm `M-98` is about, asserted from the other side: it used to be a wildcard that
/// swallowed `CallEvent::SignalMetrics` too. What is left in it now is a closed set, and a §5.3 row
/// falling back into it is a red build in `tests/spec_tables.rs`.
#[test]
fn the_events_the_contract_does_not_carry_are_not_delivered() {
    // §5.2, not §5.3: mute is a local decision the far end was told nothing about, and a remote app
    // sees it as `media.muted` on the next snapshot.
    assert_eq!(event_from_call(&CallEvent::Muted, "i12", None), None);
    assert_eq!(event_from_call(&CallEvent::Unmuted, "i12", None), None);

    // The one case here where §5.3 *does* have a row: a coupling the caller did not name. `C-6`'s
    // events say the media started or stopped crossing and not what it crossed to, and the row
    // requires the other leg — so a host that coupled media outside §6.2's `bridge` verb has no
    // contract name to put on it. This is a `None` the caller can turn into an event by answering
    // the question, which is what makes it different from the two above; the test below is that
    // same pair with an answer (`M-99`).
    assert_eq!(event_from_call(&CallEvent::Bridged, "i12", None), None);
    assert_eq!(
        event_from_call(
            &CallEvent::Unbridged {
                cause: UnbridgeCause::Released,
            },
            "i12",
            None,
        ),
        None
    );

    // A reset and the marker for observations the analyser's bounded queue coalesced away are its
    // own bookkeeping. An application learns of a reset from the `epoch` the next report names.
    for observation in [
        SignalObservation::Reset {
            cause: ResetCause::Requested,
            epoch: 1,
        },
        SignalObservation::Lost { count: 3 },
    ] {
        assert_eq!(
            event_from_call(
                &CallEvent::SignalMetrics(SignalMetrics::new(
                    CALL_ID,
                    CallAudioDirection::Inbound,
                    observation,
                )),
                "i12",
                None,
            ),
            None,
            "for {observation:?}"
        );
    }
}

/// §5.3's two coupling rows, delivered to the app that asked for the coupling (`M-99`).
///
/// The split this asserts is that **the fact is the call's and the name is the driver's**, which is
/// the same split `instruction_id` has above: `sipx-call` knows the media started crossing and the
/// contract knows what the app called the far leg, and neither knows the other's half. Only the
/// coupling can report `UnbridgeCause::PeerEnded` — §6.2's "ended by `unbridge` or *either leg
/// ending*" — so a driver composing these from the instruction alone would have no way to say the
/// bridge ended because the other call hung up.
///
/// The input is real: `sipx-call`'s own `tests/bridge.rs` is what proves `CallBridge::connect`
/// emits [`CallEvent::Bridged`] on both calls and gives the survivor of an ended peer
/// [`CallEvent::Unbridged`]. What is asserted here is the half that had no producer at all — that
/// those events reach an app-protocol client as §5.3's rows.
#[test]
fn a_coupled_call_tells_its_app_which_leg_the_media_crossed_to() {
    let bridged = received(&CallEvent::Bridged, "i20", Some("b"));
    assert_eq!(
        bridged.get("type").and_then(Json::as_str),
        Some("call.bridged")
    );
    assert_eq!(bridged.get("leg").and_then(Json::as_str), Some("b"));

    for cause in [UnbridgeCause::Released, UnbridgeCause::PeerEnded] {
        let unbridged = received(&CallEvent::Unbridged { cause }, "i20", Some("b"));
        assert_eq!(
            unbridged.get("type").and_then(Json::as_str),
            Some("call.unbridged"),
            "for {cause:?}"
        );
        assert_eq!(
            unbridged.get("leg").and_then(Json::as_str),
            Some("b"),
            "for {cause:?}"
        );
        // §5.3's row carries the leg and nothing else. Why the coupling ended is `sipx-call`'s
        // vocabulary, and putting a word on the wire the contract has not defined is how a field
        // nobody agreed to becomes one somebody depends on.
        assert_eq!(unbridged.get("cause"), None, "for {cause:?}");
    }
}

/// §5.2's `bridged` member is written by exactly these two events, and by nothing else.
///
/// So it is the other half of the same defect: for as long as no host could emit §5.3's coupling
/// rows, no host could report a coupled call in its snapshot either, and dropping the rows from the
/// section would have moved the unreachable thing one layer down rather than removed it.
#[test]
fn the_snapshot_follows_the_coupling_the_app_was_told_about() {
    let mut interpreter = interpreter();
    assert!(!interpreter.snapshot().bridged, "a new call is not coupled");

    for (event, expected) in [
        (CallEvent::Bridged, true),
        (
            CallEvent::Unbridged {
                cause: UnbridgeCause::PeerEnded,
            },
            false,
        ),
    ] {
        let kind = event_from_call(&event, "i21", Some("b")).unwrap_or_else(|| {
            panic!("§5.3 has a row for {event:?} and the bridge produced nothing")
        });
        interpreter.handle(now(), Input::Event(kind));
        assert_eq!(
            interpreter.snapshot().bridged,
            expected,
            "after {event:?} the snapshot must say the call is coupled: {expected}"
        );
    }
}

/// One voice transition on this call's timeline.
fn activity(sequence: u64, at_sample: u64) -> VoiceActivity {
    VoiceActivity::new(
        CALL_ID,
        CallAudioDirection::Inbound,
        sequence,
        at_sample,
        8_000,
    )
}

/// What an analyser configured by `profile` measures against, announced 1,600 samples in.
fn thresholds(profile: AnalysisProfile) -> VoiceThresholds {
    VoiceThresholds::new(CALL_ID, 1_600, profile).expect("the profile is in domain")
}

/// Where a transferor wants this call sent. A literal address, so nothing here needs a URI parser.
fn target() -> sipx_sip::Uri {
    sipx_sip::Uri::sip(sipx_sip::Host::Ip(IpAddr::V4(Ipv4Addr::new(
        198, 51, 100, 7,
    ))))
}
