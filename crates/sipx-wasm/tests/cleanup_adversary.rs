//! ABI conformance for cleanup ownership across entropy and event ordering.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use support::{BA_SDP_A1, BA_SDP_O1, Host, Out, header, respond_to, tape};

fn authenticated_invite(host: &mut Host, call: u32) -> String {
    let id = u64::from(call) * 10;
    assert_eq!(
        host.command(
            serde_json::json!({
                "v":1,"cmd":"dial","id":id,"target":"sip:bob@example.net"
            })
            .to_string()
            .as_bytes()
        ),
        0
    );
    assert_eq!(
        host.command(
            serde_json::json!({
                "v":1,"cmd":"local-media","id":id+1,"call":call,"kind":"offer","sdp":BA_SDP_O1
            })
            .to_string()
            .as_bytes()
        ),
        0
    );
    let first = host.wires().last().unwrap().to_string();
    assert_eq!(host.receive(&respond_to(&first, "401 Unauthorized", &[
        "WWW-Authenticate: Digest realm=\"example.net\", nonce=\"review-nonce\", qop=\"auth\", algorithm=SHA-256"
    ], None)), 0);
    let retry = host.wires().last().unwrap().to_string();
    assert!(retry.starts_with("INVITE "));
    assert!(header(&retry, "Authorization").is_some());
    retry
}

fn success(invite: &str) -> String {
    respond_to(
        invite,
        "200 OK",
        &["Contact: <sip:bob@example.net>"],
        Some(BA_SDP_A1),
    )
    .replace(
        "\r\nTo: <sip:bob@example.net>\r\n",
        "\r\nTo: <sip:bob@example.net>;tag=review-remote\r\n",
    )
}

fn hangup(host: &mut Host, call: u32) -> i32 {
    host.command(
        serde_json::json!({"v":1,"cmd":"hangup","id":100+call,"call":call})
            .to_string()
            .as_bytes(),
    )
}

fn assert_cleanup_pair(host: &Host) -> String {
    let wires = host.wires();
    assert_eq!(wires.len(), 2);
    assert!(wires[0].starts_with("ACK "));
    assert!(wires[1].starts_with("BYE "));
    assert_eq!(header(wires[0], "Call-ID"), header(wires[1], "Call-ID"));
    assert_eq!(header(wires[0], "CSeq"), Some("2 ACK".to_owned()));
    assert_eq!(header(wires[1], "CSeq"), Some("3 BYE".to_owned()));
    assert_ne!(header(wires[0], "Via"), header(wires[1], "Via"));
    let bye = host
        .position(|record| matches!(record, Out::Wire(wire) if wire.starts_with("BYE ")))
        .unwrap();
    let ended = host.position(|record| matches!(record, Out::Event(event) if event.contains("\"evt\":\"call-ended\""))).unwrap();
    assert!(bye < ended, "terminal event must follow owed wire cleanup");
    assert_eq!(host.events_of("call-ended").len(), 1);
    assert!(host.events_of("negotiated-media").is_empty());
    header(wires[0], "Call-ID").unwrap()
}

#[test]
fn two_pending_dialogs_keep_separate_cleanup_through_partial_refills() {
    let mut host = Host::new();
    assert_eq!(host.entropy(&tape(0x80)[..112]), 0);
    let first = authenticated_invite(&mut host, 1);
    let second = authenticated_invite(&mut host, 2);
    assert_eq!(hangup(&mut host, 1), 0);
    assert_eq!(hangup(&mut host, 2), 0);
    let timers: Vec<_> = host
        .log
        .iter()
        .filter_map(|record| match record {
            Out::TimerSet { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    host.clear_log();
    for invite in [&first, &second] {
        assert_eq!(host.receive(&success(invite)), 0);
    }
    assert!(host.wires().is_empty());
    assert!(host.events_of("call-ended").is_empty());
    host.tick(64000);
    for timer in timers {
        assert_eq!(host.fire(timer), 0);
    }
    assert!(host.events_of("call-ended").is_empty());
    assert_eq!(host.entropy(&tape(0x20)[..15]), 0);
    assert!(host.wires().is_empty());
    assert!(host.events_of("call-ended").is_empty());
    host.clear_log();
    assert_eq!(host.entropy(&tape(0x40)[..1]), 0);
    let first_cleaned = assert_cleanup_pair(&host);
    host.clear_log();
    assert_eq!(host.entropy(&tape(0x60)[..16]), 0);
    let second_cleaned = assert_cleanup_pair(&host);
    assert_ne!(first_cleaned, second_cleaned);
    assert!(
        [
            header(&first, "Call-ID").unwrap(),
            header(&second, "Call-ID").unwrap()
        ]
        .contains(&first_cleaned)
    );
    assert!(
        [
            header(&first, "Call-ID").unwrap(),
            header(&second, "Call-ID").unwrap()
        ]
        .contains(&second_cleaned)
    );
    host.clear_log();
    assert_eq!(host.entropy(&tape(0)[..32]), 0);
    assert_eq!(host.receive(&success(&first)), 0);
    assert_eq!(host.receive(&success(&second)), 0);
    assert!(host.wires().is_empty());
    assert!(host.events_of("call-ended").is_empty());
    let snapshot: serde_json::Value = serde_json::from_str(&host.snapshot()).unwrap();
    assert_eq!(snapshot["calls"], serde_json::json!({}));
    assert_eq!(snapshot["pendingTimers"], 0);
}

#[test]
fn every_insufficient_pair_budget_is_atomic_and_the_same_command_can_retry() {
    for remaining in 0..16 {
        for verb in ["hangup", "media-failed"] {
            let mut host = Host::new();
            assert_eq!(host.entropy(&tape(0x80)[..56 + remaining]), 0);
            let invite = authenticated_invite(&mut host, 1);
            assert_eq!(host.receive(&success(&invite)), 0);
            let command = serde_json::json!({
                "v":1,"cmd":verb,"id":90,"call":1,"reason":"local media unavailable"
            })
            .to_string();
            host.clear_log();
            assert_eq!(
                host.command(command.as_bytes()),
                -8,
                "{verb}, budget={remaining}"
            );
            assert!(host.wires().is_empty());
            assert!(host.events_of("call-ended").is_empty());
            assert!(host.events_of("outcome").is_empty());
            let snapshot: serde_json::Value = serde_json::from_str(&host.snapshot()).unwrap();
            assert_eq!(snapshot["entropy"], remaining);
            assert_eq!(snapshot["calls"]["1"], "answerDelivered");
            assert_eq!(host.entropy(&tape(0x20)[..16 - remaining]), 0);
            assert!(
                host.wires().is_empty(),
                "refused commands do not become deferred commands"
            );
            assert_eq!(host.command(command.as_bytes()), 0);
            assert_cleanup_pair(&host);
            let outcomes = host.events_of("outcome");
            assert_eq!(
                outcomes
                    .iter()
                    .filter(|event| {
                        let value: serde_json::Value = serde_json::from_str(event).unwrap();
                        value["id"] == 90 && value["ok"] == true
                    })
                    .count(),
                1
            );
            let snapshot: serde_json::Value = serde_json::from_str(&host.snapshot()).unwrap();
            assert_eq!(snapshot["entropy"], 0);
            assert_eq!(snapshot["calls"], serde_json::json!({}));
            assert_eq!(snapshot["pendingTimers"], 0);
        }
    }
}
