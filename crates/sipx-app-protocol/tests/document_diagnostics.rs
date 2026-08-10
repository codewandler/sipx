//! A diagnostic record of a document says what it plays, never what the audio was (`M-117`).
//!
//! [`Source::Inline`] is spec §6.5's second source: PCM carried in the document itself, base64
//! (RFC 4648 §4) on the wire and a `Vec<u8>` once parsed. A derived `Debug` renders every one of
//! those bytes, so an `expect` message, a `tracing` field or a test failure carrying an
//! instruction puts the audio an app sent into a record whose length is the audio's — `M-107`'s
//! defect at `PcmFrame` and `M-110`'s at `Packet`, one crate further out.
//!
//! It is the same defect and not a smaller one. The relay path's copy is the conversation in
//! flight; this copy is the conversation **at rest**, held in a document a host may well have
//! logged on arrival, and it stays in that record for as long as the record does.
//!
//! What a log of a `play` is actually for is on the other side of this: which source, and how much
//! audio there was. Both survive every assertion below.

#![allow(clippy::expect_used, clippy::indexing_slicing)]

use sipx_app_protocol::{Document, Instruction, Source, Verb};

/// The longest record a `Source` may write, and `M-110`'s bound at `Packet`.
///
/// A constant rather than a multiple of anything: the whole property is that the length of a
/// record does not follow the length of the thing it describes.
const BOUND: usize = 200;

/// Assert that an octet appears in a record in none of the spellings a `Debug` can write it in.
///
/// `M-110`'s helper, which this crate needs for the same reason and cannot share across a crate
/// boundary. A `Vec<u8>` renders as `[167, …]` and a `Bytes` as `b"\xa7…"`, so a check for one
/// spelling passes against the other; both hex cases, because `{:x}` and `{:X}` are one formatter
/// apart. The printable run is the spelling the first version of that helper missed — a graphic
/// octet is written as **itself** and neither numeric spelling appears at all, so a `0x7E` payload
/// rendered `b"~~~~…"` past a check that looked only for `126` and `7e`.
fn carries_no_octet(record: &str, octet: u8, what: &str) {
    let mut spellings = vec![
        format!("{octet}"),
        format!("{octet:02x}"),
        format!("{octet:02X}"),
    ];
    if octet.is_ascii_graphic() {
        // Four in a row: one graphic character is a field name away from a false failure, and no
        // fixture here is one octet long.
        spellings.push(char::from(octet).to_string().repeat(4));
    }
    for spelling in spellings {
        assert!(
            !record.contains(&spelling),
            "{what} survived as `{spelling}` in: {record}"
        );
    }
}

/// The failing-first test for `M-117`'s first carrier: inline audio is rendered as a length.
///
/// `0xA7` is 167 in decimal and `a7` in hex, and neither spelling is a length, a count or any
/// other number this fixture produces — so finding either in the record can only mean the audio
/// itself was rendered.
#[test]
fn an_inline_source_renders_its_length_and_not_its_audio() {
    let source = Source::Inline(vec![0xA7_u8; 320]);
    let record = format!("{source:?}");

    carries_no_octet(&record, 0xA7, "an inline audio octet");
    assert!(
        record.contains("320"),
        "how much audio the document carried is what a `play` log is for: {record}"
    );
    assert!(
        record.contains("Inline"),
        "and which of §6.5's two sources it was: {record}"
    );
}

/// The graphic spelling, on its own fixture: `0x7E` is `~`, and a renderer that writes printable
/// octets as themselves leaks the audio without writing `126` or `7e` anywhere.
#[test]
fn an_inline_source_leaks_no_printable_audio_either() {
    let source = Source::Inline(vec![0x7E_u8; 160]);
    carries_no_octet(
        &format!("{source:?}"),
        0x7E,
        "a printable inline audio octet",
    );
}

/// The other half of the defect: a redaction that still grew with the audio would fix nothing.
///
/// A document may carry a whole prompt inline, and §6.5 sets no size on it. A rendering that names
/// a length is bounded by this implementation; one that names the bytes is bounded by whatever the
/// app sent.
#[test]
fn an_inline_source_record_is_bounded_independently_of_the_audio() {
    let source = Source::Inline(vec![0xA7_u8; 100_000]);
    let record = format!("{source:?}");

    assert!(
        record.len() < BOUND,
        "a record whose length is the audio's is an unbounded diagnostic: {} octets",
        record.len()
    );
    carries_no_octet(&record, 0xA7, "an inline audio octet");
}

/// What a `play` log is for stays readable: a host-local file is named in full.
///
/// §6.5's first source is a name the host resolves, so it is a fact about the program rather than
/// about the conversation, and redacting it would cost a reader the one thing the record is for.
/// This pins that decision so a later generalisation of the redaction above does not take it.
#[test]
fn a_file_source_still_names_its_file() {
    let source = Source::File("greetings/welcome.wav".to_owned());
    let record = format!("{source:?}");
    assert!(
        record.contains("greetings/welcome.wav"),
        "which file a `play` names is what the record is for: {record}"
    );
}

/// The same, through the document that carries the source — a redaction one type up is not one.
///
/// A `Document` is what an interpreter actually holds and what an error path actually renders, so
/// the property has to hold at the type a caller formats rather than only at the leaf.
#[test]
fn a_document_renders_no_inline_audio() {
    let document = Document::new(vec![
        Instruction::new("greet", Verb::Answer),
        Instruction::new(
            "prompt",
            Verb::Play {
                source: Source::Inline(vec![0xA7_u8; 320]),
                interruptible: true,
            },
        ),
    ]);
    let record = format!("{document:?}");

    carries_no_octet(&record, 0xA7, "an inline audio octet");
    assert!(
        record.contains("prompt"),
        "which instruction it was still belongs in the record: {record}"
    );
}
