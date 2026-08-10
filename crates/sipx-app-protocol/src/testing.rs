//! Fixtures the crate's own tests and the derived spec-table tests both need.
//!
//! Public rather than `#[cfg(test)]` because the tests that matter most here live in `tests/`,
//! where a `#[cfg(test)]` module of the library is not visible. Nothing in this module is part of
//! the contract; it exists so that "the crate covers §5.3's table" can be a test that enumerates
//! rather than a promise that a reviewer checks by eye.

use crate::document::{DtmfMode, Gather, Instruction, Source, TransferTarget, Verb};
use crate::dsp::{DspBypassCause, DspRefusal, DspStage, DspTeardownCause, DspValue};
use crate::event::{
    AudioDirection, CallSnapshot, CallState, DialOutcome, Direction, EndCause, EventKind,
    GatherReason, Leg, LegEndCause, TransferState, VoiceEndCause, VoiceThresholds,
};
use crate::interpreter::Callback;

/// One value of every event type §5.3 defines, in the section's order.
///
/// The order is load-bearing: `tests/spec_tables.rs` reads §5.3's rows out of the spec and lines
/// them up against this list, so a row added to the table with no variant here fails that test.
#[must_use]
pub fn one_of_every_event() -> Vec<EventKind> {
    vec![
        EventKind::Incoming,
        EventKind::Ringing { reliable: true },
        EventKind::EarlyMediaStarted,
        EventKind::Answered,
        EventKind::Dtmf {
            digit: '5',
            duration_ms: 160,
        },
        EventKind::VoiceStarted {
            direction: AudioDirection::Inbound,
            sequence: 0,
            sample_time: 0,
            sample_rate: 8_000,
        },
        EventKind::VoiceEnded {
            direction: AudioDirection::Inbound,
            sequence: 1,
            sample_time: 160,
            sample_rate: 8_000,
            cause: VoiceEndCause::Hangover,
        },
        EventKind::VoiceThresholds {
            sample_time: 1_600,
            thresholds: reference_thresholds(),
        },
        EventKind::SignalMetrics {
            direction: AudioDirection::Inbound,
            epoch: 0,
            sequence: 3,
            sample_time: 480,
            sample_rate: 8_000,
            samples: 160,
            windows: 1,
            peak: 32_767,
            rms: 32_767,
            clipped_samples: 160,
            clipping_windows: 1,
            active_windows: 0,
            silent_windows: 0,
        },
        EventKind::SignalSilence {
            direction: AudioDirection::Inbound,
            epoch: 0,
            sample_time: 640,
            sample_rate: 8_000,
        },
        EventKind::PlaybackFinished {
            instruction_id: "p1".to_owned(),
            completed: true,
        },
        EventKind::GatherFinished {
            instruction_id: "g1".to_owned(),
            digits: "1234".to_owned(),
            reason: GatherReason::Terminator,
        },
        EventKind::RecordingFinished {
            instruction_id: "r1".to_owned(),
            duration_ms: 4_200,
        },
        EventKind::DialFinished {
            instruction_id: "d1".to_owned(),
            leg: "b".to_owned(),
            outcome: DialOutcome::Rejected { status: 603 },
        },
        EventKind::LegEnded {
            leg: "b".to_owned(),
            cause: LegEndCause::Remote,
        },
        EventKind::TransferRequested {
            target: "sip:carol@example.net".to_owned(),
            attended: false,
        },
        EventKind::TransferProgress {
            state: TransferState::Failed { status: 480 },
        },
        EventKind::Bridged {
            leg: "b".to_owned(),
        },
        EventKind::Unbridged {
            leg: "b".to_owned(),
        },
    ]
    .into_iter()
    .chain(dsp_events())
    .chain(tail_events())
    .collect()
}

/// §5.3's five `call.dsp.*` rows, one of each (`M-67`).
///
/// A function of its own because five rows sharing four members push [`one_of_every_event`] past
/// this workspace's function-length limit on their own. The split is where §5.3 puts them, so the
/// section's order is still the list's order.
fn dsp_events() -> Vec<EventKind> {
    vec![
        EventKind::DspActivated {
            instruction_id: "x1".to_owned(),
            direction: AudioDirection::Outbound,
            generation: 2,
            previous: Some(1),
            processors: 2,
            contains_overrun: true,
        },
        EventKind::DspConfigured {
            instruction_id: "x2".to_owned(),
            direction: AudioDirection::Outbound,
            generation: 2,
            at_position: 320,
            processor: "sipx.gain".to_owned(),
        },
        EventKind::DspBypassed {
            direction: AudioDirection::Inbound,
            generation: 2,
            at_position: 480,
            processor: "sipx.low_pass".to_owned(),
            cause: DspBypassCause::DeadlineMissed,
        },
        EventKind::DspRemoved {
            instruction_id: Some("x3".to_owned()),
            direction: AudioDirection::Outbound,
            generation: 2,
            at_position: 640,
            processor: None,
            cause: DspTeardownCause::Requested,
        },
        EventKind::DspRefused {
            instruction_id: "x4".to_owned(),
            direction: AudioDirection::Inbound,
            reason: DspRefusal::UnknownProcessor,
        },
    ]
}

/// The rows §5.3 lists after the DSP ones, in the section's order.
fn tail_events() -> Vec<EventKind> {
    vec![
        EventKind::Hold,
        EventKind::Resumed,
        EventKind::Ended {
            cause: EndCause::Rejected { status: 486 },
        },
    ]
}

/// One instruction of every verb §6.2 defines, in the section's order.
///
/// Every optional field is populated, so a round trip through the wire exercises the whole row
/// rather than the two fields a hand-written fixture would have remembered.
#[must_use]
pub fn one_of_every_verb() -> Vec<Instruction> {
    let mut headers = std::collections::BTreeMap::new();
    headers.insert("x-campaign".to_owned(), "renewal".to_owned());
    vec![
        Instruction::new("i01", Verb::Answer),
        Instruction::new("i02", Verb::Ring { reliable: true }),
        Instruction::new(
            "i03",
            Verb::Reject {
                status: 486,
                reason: Some("Busy Here".to_owned()),
            },
        ),
        Instruction::new(
            "i04",
            Verb::Play {
                source: Source::Inline(vec![0x00, 0xff, 0x7f, 0x80]),
                interruptible: true,
            },
        ),
        Instruction::new(
            "i05",
            Verb::GatherDigits(Gather {
                min: 1,
                max: Some(4),
                terminators: "#".to_owned(),
                digit_timeout_ms: Some(4_000),
                timeout_ms: Some(10_000),
                prompt: Some(Source::File("menu.wav".to_owned())),
            }),
        ),
        Instruction::new(
            "i06",
            Verb::Record {
                max_ms: Some(30_000),
                idle_stop_ms: Some(2_000),
            },
        ),
        Instruction::new(
            "i07",
            Verb::SendDtmf {
                digits: "*72".to_owned(),
                duration_ms: Some(120),
            },
        ),
        Instruction::new(
            "i08",
            Verb::Dial {
                target: "sip:bob@example.net".to_owned(),
                from: Some("sip:support@example.net".to_owned()),
                timeout_ms: Some(20_000),
                headers,
            },
        ),
        Instruction::new(
            "i09",
            Verb::Bridge {
                leg: "b".to_owned(),
                dtmf: DtmfMode::Consume,
            },
        ),
        Instruction::new("i10", Verb::Unbridge),
        Instruction::new("i11", Verb::Hold),
        Instruction::new("i12", Verb::Resume),
        Instruction::new("i13", Verb::Mute),
        Instruction::new("i14", Verb::Unmute),
        Instruction::new(
            "i15",
            Verb::Transfer {
                target: TransferTarget::Blind {
                    target: "sip:carol@example.net".to_owned(),
                },
            },
        ),
        Instruction::new("i16", Verb::AcceptTransfer),
        Instruction::new("i17", Verb::RefuseTransfer { status: 603 }),
        Instruction::new("i18", Verb::Pause { ms: 500 }),
        Instruction::new(
            "i19",
            Verb::Tag {
                key: "campaign".to_owned(),
                value: "renewal".to_owned(),
            },
        ),
    ]
    .into_iter()
    .chain(dsp_verbs())
    .chain(vec![Instruction::new(
        "i23",
        Verb::Hangup {
            cause: EndCause::Hangup,
        },
    )])
    .collect()
}

/// §6.2's three `dsp` verbs, one of each (`M-67`).
///
/// Split from [`one_of_every_verb`] for the reason [`dsp_events`] is split from
/// [`one_of_every_event`]: three verbs carrying a nested chain and a parameter set between them
/// push that function past this workspace's function-length limit on their own.
fn dsp_verbs() -> Vec<Instruction> {
    vec![
        Instruction::new(
            "i20",
            Verb::Dsp {
                direction: AudioDirection::Outbound,
                processors: vec![
                    DspStage::new("sipx.gain")
                        .with_parameter("gain", DspValue::Ratio(2_000))
                        .with_parameter("smoothing_positions", DspValue::Integer(160)),
                    DspStage::new("sipx.stutter")
                        .with_shape(480)
                        .with_parameter("repeat", DspValue::Flag(true)),
                ],
            },
        ),
        Instruction::new(
            "i21",
            Verb::DspParam {
                direction: AudioDirection::Outbound,
                generation: 3,
                processor: 0,
                parameters: vec![crate::dsp::DspParameter::new(
                    "gain",
                    DspValue::Ratio(1_500),
                )],
            },
        ),
        Instruction::new(
            "i22",
            Verb::DspRemove {
                direction: AudioDirection::Inbound,
            },
        ),
    ]
}

/// The processing contract's reference profile `P8` with its reference calibration `K8` on it, as
/// §5.2's `voice` member (`M-84`).
///
/// The numbers are that spec's, not invented here: `W = 160` and a 1,600-sample hangover at 8 kHz
/// (§11.1), and `C = 1,600`, `U = 800`, `F = 240,000` from §12.12. `1,536` is where vector CAL-6's
/// 25-frame prefix leaves the threshold, which `sipx-call`'s own tests arrive at from the analyser
/// rather than from this constant — so an analyser change that moved it shows up as a disagreement
/// between two crates instead of being agreed with here.
#[must_use]
pub fn reference_thresholds() -> VoiceThresholds {
    VoiceThresholds::new(AudioDirection::Inbound, 8_000, 1_536, 160, 1_600).with_calibration(
        1_600,
        800,
        Some(240_000),
    )
}

/// A snapshot with every member of §5.2 populated, including the ones that are easy to forget.
#[must_use]
pub fn populated_snapshot() -> CallSnapshot {
    let mut call = CallSnapshot::new("b7c1", Direction::Inbound)
        .between("sip:alice@example.com", "sip:support@example.net");
    call.state = CallState::Answered;
    call.headers
        .set("P-Asserted-Identity", "\"Alice\" <sip:alice@example.com>");
    call.media.encrypted = true;
    call.legs.push(Leg {
        leg: "b".to_owned(),
        state: CallState::Ringing,
        to: "sip:bob@example.net".to_owned(),
    });
    call.voice = Some(reference_thresholds());
    call.tags
        .insert("campaign".to_owned(), "renewal".to_owned());
    call
}

/// A [`Callback`] for a delivery, forged.
///
/// **A driver cannot do this**, and that is the point of the function existing here rather than on
/// [`Callback`]: §6.3's "at most one callback outstanding" is held by [`Callback`] being neither
/// [`Clone`] nor [`Copy`] and having no public constructor, so the only way to *demonstrate* that
/// a second answer to one delivery is ignored is to forge the token that a correct driver can
/// never obtain. Vector AC-4 does exactly that, and it is the reason this is not `#[cfg(test)]`.
pub fn forge_callback(seq: u64) -> Callback {
    Callback::new(seq)
}

/// Bodies a peer could send that a parser must answer rather than die on.
///
/// AGENTS.md non-negotiable 3: no panics on input this process did not produce. An app's response
/// document is exactly that input, so every reader in this crate is run over this list.
pub const HOSTILE_BODIES: &[&str] = &[
    "",
    " ",
    "\0",
    "{",
    "}",
    "[",
    "]",
    "\"",
    "\"\\",
    "\"\\u",
    "\"\\uD800\"",
    "{\"contract\"",
    "{\"contract\":}",
    "{\"contract\":\"sipx.app.v1\"",
    "{\"contract\":\"sipx.app.v1\"}",
    "{\"contract\":\"sipx.app.v1\",\"instructions\":}",
    "{\"contract\":\"sipx.app.v1\",\"instructions\":[{}]}",
    "{\"contract\":\"sipx.app.v1\",\"instructions\":[{\"do\":\"play\"}]}",
    "{\"contract\":\"sipx.app.v1\",\"instructions\":[{\"id\":1,\"do\":2}]}",
    "{\"contract\":\"sipx.app.v1\",\"instructions\":{}}",
    "{\"contract\":\"sipx.app.v2\",\"instructions\":[]}",
    "{\"contract\":null,\"instructions\":[]}",
    "[1,2,3]",
    "null",
    "3.14",
    "-",
    "1e",
    "999999999999999999999999999999",
    "{\"a\":1,}",
    "{\"contract\":\"sipx.app.v1\",\"seq\":-1,\"at\":\"\",\"call\":{},\"event\":{}}",
];
