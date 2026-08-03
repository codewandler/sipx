//! Fixtures the crate's own tests and the derived spec-table tests both need.
//!
//! Public rather than `#[cfg(test)]` because the tests that matter most here live in `tests/`,
//! where a `#[cfg(test)]` module of the library is not visible. Nothing in this module is part of
//! the contract; it exists so that "the crate covers §5.3's table" can be a test that enumerates
//! rather than a promise that a reviewer checks by eye.

use crate::document::{DtmfMode, Gather, Instruction, Source, TransferTarget, Verb};
use crate::event::{
    CallSnapshot, CallState, DialOutcome, Direction, EndCause, EventKind, GatherReason, Leg,
    TransferState,
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
        Instruction::new(
            "i20",
            Verb::Hangup {
                cause: EndCause::Hangup,
            },
        ),
    ]
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
