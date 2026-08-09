//! One WebSocket message is one SIP message: `docs/specs/sip-tls.md` §4 at `sipx_input_bytes`.
//!
//! > Not `Content-Length` framing — the frame boundary *is* the message boundary. A message split
//! > across frames is malformed, and two messages in one frame likewise. Both close the connection
//! > rather than being patched up.
//!
//! The kernel cannot close anything; it has no socket. What it owes the host is a verdict the host
//! can act on, and §4.11's `parse_errors` is that verdict — the same one a fragment already earns,
//! which is what makes `docs/specs/browser-signalling.md` §5's rule ("a raised count closes the
//! connection with `framing`") cover both halves of the RFC 7118 §5 rule rather than one.
//!
//! The half these cases are about is the one that was silent: a coalesced frame used to be parsed
//! down to its first message, acted on, and the rest discarded with no counter and no event.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use support::{Host, tape};

/// A kernel with enough entropy that nothing below is refused for the wrong reason.
fn healthy() -> Host {
    let mut host = Host::new();
    host.entropy(&tape(0x10));
    host.clear_log();
    host
}

/// An inbound request the kernel answers: outside §5.2's vocabulary, so RFC 3261 §8.2.1's `405`
/// goes back over the transaction. Answering is the point — it is what makes "the first message
/// of a coalesced frame was acted on" observable as a `WIRE` record.
fn options(tag: &str) -> String {
    format!(
        "OPTIONS sip:alice@example.net SIP/2.0\r\n\
         Via: SIP/2.0/WSS df7jal23ls0d.invalid;branch=z9hG4bK{tag}\r\n\
         To: <sip:alice@example.net>\r\n\
         From: <sip:peer@example.org>;tag={tag}\r\n\
         Call-ID: {tag}@example.org\r\n\
         CSeq: 1 OPTIONS\r\n\
         Content-Length: 0\r\n\r\n"
    )
}

/// The `parse_errors` count in a §4.11 snapshot.
fn parse_errors(host: &mut Host) -> u64 {
    let snapshot = host.snapshot();
    let needle = r#""parse_errors":"#;
    let start = snapshot
        .find(needle)
        .map(|at| at + needle.len())
        .expect("the snapshot always carries parse_errors");
    let rest = &snapshot[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().expect("a decimal count")
}

/// Assert that a frame was refused whole: counted, and with nothing at all done about it.
fn assert_refused(host: &mut Host, frame: &str, what: &str) {
    let before = parse_errors(host);
    assert_eq!(
        host.receive(frame),
        0,
        "{what}: hostile framing is a value, not a host-contract violation (§4.10)"
    );
    assert_eq!(
        parse_errors(host),
        before + 1,
        "{what}: the host learns about it the only way it can, through §4.11's parse_errors"
    );
    assert!(
        host.log.is_empty(),
        "{what}: nothing may be answered, sent or scheduled from a frame that was refused: {:?}",
        host.log
    );
    let snapshot = host.snapshot();
    assert!(
        snapshot.contains(r#""calls":{}"#),
        "{what}: no dialog: {snapshot}"
    );
    assert!(
        snapshot.contains(r#""pendingTimers":0"#),
        "{what}: no transaction was created, so no transaction timer was set: {snapshot}"
    );
}

/// Two complete messages in one frame are refused, and the first of them is not acted on.
///
/// This is the case `T-33` measured against the compiled module: the kernel used to answer the
/// first `OPTIONS` with a `405` and set its transaction's timer, then drop the second request on
/// the floor without counting it. A peer that frames wrongly has revealed it disagrees about where
/// messages end, so nothing further from it can be trusted — including the part that parsed.
#[test]
fn two_messages_in_one_frame_are_refused() {
    let mut host = healthy();
    let frame = format!("{}{}", options("first"), options("second"));
    assert_refused(&mut host, &frame, "two complete requests in one frame");
}

/// The rule is "the frame is the message", not "the frame starts with a message".
#[test]
fn octets_after_a_message_are_refused() {
    let mut host = healthy();
    let frame = format!("{}garbage", options("trailing"));
    assert_refused(&mut host, &frame, "a request with trailing octets");
}

/// A leading CRLF is RFC 5626 §4.4.1's keep-alive and RFC 3261 §7.5 says to ignore it; it does not
/// turn one message into two.
#[test]
fn a_keepalive_before_the_message_is_not_a_second_message() {
    let mut host = healthy();
    let frame = format!("\r\n{}", options("keepalive"));
    assert_eq!(host.receive(&frame), 0);
    assert_eq!(parse_errors(&mut host), 0, "one message, framed correctly");
    assert_eq!(host.wires().len(), 1, "and answered: {:?}", host.wires());
}

/// One message, with and without a body, is untouched by any of this.
#[test]
fn exactly_one_message_is_unaffected() {
    for (what, frame) in [
        ("without a body", options("plain")),
        (
            "with a body",
            options("bodied").replace(
                "Content-Length: 0\r\n\r\n",
                "Content-Type: application/sdp\r\nContent-Length: 12\r\n\r\nv=0\r\no=- 1 1",
            ),
        ),
    ] {
        let mut host = healthy();
        assert_eq!(host.receive(&frame), 0, "{what}");
        assert_eq!(
            parse_errors(&mut host),
            0,
            "{what}: nothing failed to parse"
        );
        let wires = host.wires();
        assert_eq!(wires.len(), 1, "{what}: answered once: {wires:?}");
        assert!(
            wires[0].starts_with("SIP/2.0 405 "),
            "{what}: RFC 3261 §8.2.1's answer: {}",
            wires[0]
        );
    }
}

/// `docs/specs/sip-tls.md` §4: **`Content-Length` is optional here**, unlike on a stream. The
/// frame says where the message ends, so a body simply runs to the end of it — and refusing a
/// message that omits the header would reject traffic this transport frames perfectly well.
///
/// Which is why a frame whose *body* happens to look like a second message is one message and not
/// two: with no `Content-Length` on the first, RFC 3261 §20.14 already spent those octets.
#[test]
fn a_message_without_content_length_runs_to_the_end_of_the_frame() {
    let mut host = healthy();
    let frame = options("nolength").replace("Content-Length: 0\r\n", "");
    assert_eq!(host.receive(&frame), 0);
    assert_eq!(parse_errors(&mut host), 0, "the frame says where it ends");
    assert_eq!(host.wires().len(), 1, "answered: {:?}", host.wires());
}

// ------------------------------------------------------ the shared vectors, this side of them

/// The four shapes that make §4's rule a rule of its own rather than one of its neighbours: two
/// separate it from RFC 3261 §18.3's datagram tolerance, and two from the stream framer.
///
/// Named rather than counted, because a table that quietly lost one of them would still be a
/// table and this test would still pass over whatever was left.
const SEPARATING_SHAPES: [&str; 4] = [
    "two-complete-messages",
    "one-message-then-octets",
    "no-content-length",
    "leading-keepalive",
];

/// `docs/specs/sip-tls.md` §6.1's framing vectors, read out of the spec rather than transcribed.
///
/// A verdict (`one` or `refused`) and a name begin a case; the frame's lines follow, indented four
/// spaces, each contributing its own text and a CRLF, with `<CRLF>` alone meaning an empty line.
/// The frame is those octets and nothing else.
///
/// `sipx-transport`'s `ws.rs` carries the same reader over the same table. That is deliberate: the
/// duplication that matters is of the *rule*, which now has one implementation, and this is what
/// stops a caller quietly acquiring a second one — a corpus neither caller owns and both answer.
fn spec_framing_vectors() -> Vec<(String, bool, Vec<u8>)> {
    const SPEC: &str = include_str!("../../../docs/specs/sip-tls.md");
    const BEGIN: &str = "# BEGIN ws-framing-vectors";
    const END: &str = "# END ws-framing-vectors";

    let table = SPEC
        .split_once(BEGIN)
        .and_then(|(_, rest)| rest.split_once(END))
        .map(|(table, _)| table)
        .unwrap_or_default();

    let mut vectors: Vec<(String, bool, Vec<u8>)> = Vec::new();
    for line in table.lines() {
        if let Some(text) = line.strip_prefix("    ") {
            let (name, _, frame) = vectors.last_mut().expect("a frame line inside a case");
            assert!(!text.is_empty(), "{name}: an empty line is written <CRLF>");
            if text != "<CRLF>" {
                frame.extend_from_slice(text.as_bytes());
            }
            frame.extend_from_slice(b"\r\n");
        } else if !line.trim().is_empty() {
            let mut fields = line.split_whitespace();
            let verdict = fields.next().expect("a verdict");
            let name = fields.next().expect("a case name").to_owned();
            assert!(
                matches!(verdict, "one" | "refused"),
                "{name}: unknown verdict {verdict}"
            );
            vectors.push((name, verdict == "one", Vec::new()));
        }
    }
    vectors
}

/// Every §6.1 vector, through `sipx_input_bytes`.
///
/// `sipx-transport` runs the same table through the WebSocket transport. Two callers, one rule,
/// one corpus: after `S-54` the rule has a single implementation in `sipx_sip::parse_frame`, and
/// this is the check that says so out loud — a caller that reimplements it fails the row it
/// disagrees with rather than agreeing today and drifting later.
#[test]
fn every_spec_framing_vector_holds_for_the_kernel() {
    let vectors = spec_framing_vectors();
    for shape in SEPARATING_SHAPES {
        assert!(
            vectors.iter().any(|(name, ..)| name == shape),
            "docs/specs/sip-tls.md §6.1 has no `{shape}` vector; found {:?}",
            vectors.iter().map(|(name, ..)| name).collect::<Vec<_>>()
        );
    }

    for (name, one, frame) in &vectors {
        let mut host = healthy();
        let before = parse_errors(&mut host);
        assert_eq!(
            host.receive_bytes(frame),
            0,
            "{name}: hostile framing is a value, not a host-contract violation (§4.10)"
        );
        if *one {
            assert_eq!(
                parse_errors(&mut host),
                before,
                "{name}: §6.1 calls this one whole message"
            );
            assert_eq!(
                host.wires().len(),
                1,
                "{name}: and one message is answered once: {:?}",
                host.wires()
            );
        } else {
            assert_eq!(
                parse_errors(&mut host),
                before + 1,
                "{name}: §6.1 refuses this frame, and §4.11's count is how the host learns"
            );
            assert!(
                host.log.is_empty(),
                "{name}: nothing may be answered, sent or scheduled from a refused frame: {:?}",
                host.log
            );
        }
    }
}
