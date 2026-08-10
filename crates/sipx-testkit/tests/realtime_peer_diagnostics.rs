//! What the peer *records* is a summary, never the uplink it heard (`M-117`).
//!
//! [`Record::appended_audio`] is "every appended audio byte, concatenated in arrival order — the
//! uplink as the far end heard it", and [`ClientEvent::Append`] holds "one 20 ms G.711 frame". Both
//! derived their `Debug`, and this crate's own `realtime_peer.rs` already formats the event list
//! into an assertion message — so a failing ORB-5 run printed a call's audio into a CI log, which
//! is `M-107`'s defect live rather than latent.
//!
//! A test fixture is not an excuse. The peer stands in for the far end of a *real* call: the
//! bridge tests drive audio from a real `sipx-media` session through it, and a soak run's failure
//! output is a file somebody pastes into a ticket. The record is also the last place the audio
//! comes to rest — it is kept for the whole life of the peer, by construction.
//!
//! What a test failure actually needs is on the other side of this: which events arrived, in what
//! order, and how much audio there was. Every assertion below keeps all three.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use serde_json::json;
use sipx_testkit::realtime_peer::{ClientEvent, Record, Upgrade, UpgradeOutcome};

/// The longest record either of these types may write, and `M-110`'s bound at `Packet`.
///
/// A constant rather than a multiple of anything: the whole property is that the length of a
/// record does not follow the length of the thing it describes.
const BOUND: usize = 300;

/// Assert that an octet appears in a record in none of the spellings a `Debug` can write it in.
///
/// `M-110`'s helper, needed here for the same reason and not shareable across a crate boundary.
/// A `Vec<u8>` renders as `[167, …]` and a `Bytes` as `b"\xa7…"`, so a check for one spelling
/// passes against the other; both hex cases, because `{:x}` and `{:X}` are one formatter apart.
/// The printable run is the spelling the first version of that helper missed — a graphic octet is
/// written as **itself**, so a `0x7E` frame rendered `b"~~~~…"` past a check for `126` and `7e`.
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

/// The failing-first test for `M-117`'s second carrier: an append is rendered as a frame length.
///
/// `0xA7` is 167 in decimal and `a7` in hex, and neither spelling is a length or a count this
/// fixture produces — so finding either can only mean the frame itself was rendered.
#[test]
fn an_append_event_renders_its_length_and_not_its_frame() {
    let event = ClientEvent::Append {
        audio: vec![0xA7_u8; 160],
    };
    let record = format!("{event:?}");

    carries_no_octet(&record, 0xA7, "an uplink audio octet");
    assert!(
        record.contains("Append"),
        "which §5.1 event arrived is what the record is for: {record}"
    );
    assert!(
        record.contains("160"),
        "and how much audio it carried: {record}"
    );
}

/// The graphic spelling, on its own fixture: `0x7E` is `~`, and a renderer that writes printable
/// octets as themselves leaks the frame without writing `126` or `7e` anywhere.
#[test]
fn an_append_event_leaks_no_printable_frame_either() {
    let event = ClientEvent::Append {
        audio: vec![0x7E_u8; 160],
    };
    carries_no_octet(&format!("{event:?}"), 0x7E, "a printable uplink octet");
}

/// The failing-first test for the third carrier: the accumulated uplink is a byte count.
///
/// This is the one that was already reaching a log. `realtime_peer.rs`'s ORB-5 assertion formats
/// `record.client_events` on failure, and a soak run formats the record itself.
#[test]
fn a_record_renders_counts_and_not_the_uplink() {
    let record = Record {
        client_events: vec![
            ClientEvent::SessionUpdate(json!({"type": "session.update", "session": {}})),
            ClientEvent::Append {
                audio: vec![0xA7_u8; 160],
            },
            ClientEvent::Cancel,
        ],
        appended_audio: vec![0xA7_u8; 160],
        pings: 3,
        deltas_sent: 7,
        ..Record::default()
    };
    let rendered = format!("{record:?}");

    carries_no_octet(&rendered, 0xA7, "an uplink audio octet");
    assert!(
        rendered.contains("160"),
        "how much uplink the far end heard is what the record is for: {rendered}"
    );
    assert!(
        rendered.contains("pings: 3") && rendered.contains("deltas_sent: 7"),
        "and the counts a liveness or pacing test asserts on: {rendered}"
    );
}

/// A record renders no upgrade credential either, which is this redaction's second effect.
///
/// [`Upgrade::authorization`] is the `Authorization` header verbatim — ORB-1 asserts the resolved
/// secret's *bytes* reached the wire, so the field cannot be redacted at its own type without
/// making that assertion vacuous. Rendering the list as a count is what keeps that decision local
/// to `Upgrade`: a test that wants the header reads the field, and a record formatted into a
/// failure message no longer carries it.
#[test]
fn a_record_renders_no_upgrade_credential() {
    let record = Record {
        upgrades: vec![Upgrade {
            target: "/v1/realtime?model=fixture".to_owned(),
            authorization: Some("Bearer sk-fixture-do-not-print".to_owned()),
            header_names: vec!["authorization".to_owned()],
            outcome: UpgradeOutcome::Accepted,
        }],
        ..Record::default()
    };
    let rendered = format!("{record:?}");

    assert!(
        !rendered.contains("sk-fixture-do-not-print"),
        "an upgrade credential reached a record through the peer's own summary: {rendered}"
    );
    assert!(
        rendered.contains('1'),
        "how many upgrades were attempted is what ORB-16 asserts on: {rendered}"
    );
}

/// The other half of the defect: a redaction that still grew with the conversation fixes nothing.
///
/// Every part of a `Record` a call can make arbitrarily long is here at once — the accumulated
/// uplink, the event list, and one event's own frame. A rendering that names a count for each is
/// bounded by this implementation; one that names the values is bounded by however long the call
/// ran.
#[test]
fn a_record_is_bounded_independently_of_the_conversation() {
    let record = Record {
        client_events: vec![
            ClientEvent::Append {
                audio: vec![0xA7_u8; 160],
            };
            10_000
        ],
        appended_audio: vec![0xA7_u8; 1_600_000],
        upgrades: vec![
            Upgrade {
                target: "/v1/realtime".to_owned(),
                authorization: None,
                header_names: vec!["host".to_owned()],
                outcome: UpgradeOutcome::Refused(401),
            };
            500
        ],
        ..Record::default()
    };
    let rendered = format!("{record:?}");

    assert!(
        rendered.len() < BOUND,
        "a record whose length is the call's is an unbounded diagnostic: {} octets",
        rendered.len()
    );
    carries_no_octet(&rendered, 0xA7, "an uplink audio octet");
}

/// The same for one event, whose two other variants carry values a defective bridge chooses.
///
/// A `session.update` object is the app's own configuration and an `Outside` event's type is a
/// string off the wire; neither is the call, and both are unbounded. The bound is the property
/// here rather than the redaction — a diagnostic that a peer can be made to grow without limit is
/// the same defect wearing different bytes.
#[test]
fn an_event_record_is_bounded_independently_of_the_event() {
    let events = [
        ClientEvent::SessionUpdate(json!({"type": "session.update", "prompt": "x".repeat(50_000)})),
        ClientEvent::Outside {
            event_type: "y".repeat(50_000),
        },
        ClientEvent::Unreadable {
            reason: "z".repeat(50_000),
        },
    ];
    for event in events {
        let rendered = format!("{event:?}");
        assert!(
            rendered.len() < BOUND,
            "an event record grew with the event: {} octets",
            rendered.len()
        );
    }
}

/// What a failing ORB-5 run needs is still there: which events arrived, in order.
///
/// This pins the diagnostic the redaction had to preserve. The assertion in `realtime_peer.rs`
/// formats this exact list, and a summary that named no variants would have made the failure
/// unreadable rather than safe.
#[test]
fn an_event_list_still_names_its_events_in_order() {
    let events = vec![
        ClientEvent::SessionUpdate(json!({"type": "session.update"})),
        ClientEvent::Append {
            audio: vec![0xFF_u8; 160],
        },
        ClientEvent::Cancel,
        ClientEvent::Outside {
            event_type: "response.create".to_owned(),
        },
    ];
    let rendered = format!("{events:?}");

    let order = ["SessionUpdate", "Append", "Cancel", "Outside"];
    let mut at = 0;
    for name in order {
        let found = rendered[at..]
            .find(name)
            .unwrap_or_else(|| panic!("`{name}` is missing from: {rendered}"));
        at += found + name.len();
    }
    assert!(
        rendered.contains("response.create"),
        "the type of an event outside §5.1 is the whole point of ORB-5's failure message: \
         {rendered}"
    );
}
