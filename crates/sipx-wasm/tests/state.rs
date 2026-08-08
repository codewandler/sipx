//! `docs/specs/browser-sdk.md` §9.6: the state and lifecycle vectors, plus §4.9's create/free
//! memory bound.
//!
//! `BSDK-JS-1` and `BSDK-JS-2` are deliberately absent: §11 assigns §6's lifecycle — promise
//! settlement order and post-`close()` delivery — to `A-17`'s handwritten JavaScript layer, and a
//! Rust test cannot observe a JavaScript listener. `BSDK-STATE-1` to `BSDK-STATE-8` are the
//! kernel's own, and they are all here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use support::{BA_SDP_A1, BA_SDP_O1, BSDK_CMD_1, BSDK_CMD_2, Host, Out, header, respond_to, tape};

/// A kernel with a full pool.
fn ready() -> Host {
    let mut host = Host::new();
    host.entropy(&tape(0x80));
    host.clear_log();
    host
}

fn local_media(id: u64, call: u32, kind: &str, sdp: &str) -> Vec<u8> {
    serde_json::json!({"v":1,"cmd":"local-media","id":id,"call":call,"kind":kind,"sdp":sdp})
        .to_string()
        .into_bytes()
}

/// Drive an outbound call to `AnswerDelivered`: dial, offer, 180, 200 with `BA-SDP-A1`.
fn dial_to_answer_delivered(host: &mut Host) -> String {
    assert_eq!(host.command(BSDK_CMD_2), 0);
    assert_eq!(host.command(&local_media(4, 1, "offer", BA_SDP_O1)), 0);
    let invite = host.wires().last().copied().expect("an INVITE").to_owned();

    host.tick(10);
    assert_eq!(
        host.receive(&respond_to(&invite, "180 Ringing", &[], None)),
        0
    );
    host.tick(10);
    assert_eq!(
        host.receive(&respond_to(
            &invite,
            "200 OK",
            &[
                "Contact: <sip:bob@192.0.2.99;transport=ws>",
                "To: <sip:bob@example.net>;tag=remote1",
            ],
            Some(BA_SDP_A1),
        )),
        0
    );
    invite
}

/// `BSDK-STATE-1`: new kernel → entropy → `"register"` → 401 challenge → 200.
///
/// Required: exactly two WIRE REGISTERs (the second with digest, cnonce from the tape), a refresh
/// `TIMER_SET`, events `"registration"` (`registered`) then `"outcome"` ok, in that order.
#[test]
fn bsdk_state_1_registration_with_a_digest_challenge() {
    let mut host = ready();

    assert_eq!(host.command(BSDK_CMD_1), 0);
    let first = host.wires().last().copied().expect("a REGISTER").to_owned();
    assert!(
        first.starts_with("REGISTER sip:example.net SIP/2.0\r\n"),
        "{first}"
    );
    assert!(
        header(&first, "Authorization").is_none(),
        "the first REGISTER is unauthenticated"
    );

    host.tick(20);
    assert_eq!(
        host.receive(&respond_to(
            &first,
            "401 Unauthorized",
            &[r#"WWW-Authenticate: Digest realm="example.net", nonce="dcd98b7102dd2f0e", qop="auth", algorithm=SHA-256"#],
            None,
        )),
        0
    );

    let second = host.wires().last().copied().expect("a retry").to_owned();
    let authorization = header(&second, "Authorization").expect("a digest response");
    assert!(authorization.starts_with("Digest "), "{authorization}");
    assert!(authorization.contains(r#"cnonce=""#), "{authorization}");
    assert!(
        authorization.contains(r#"realm="example.net""#),
        "{authorization}"
    );
    assert!(
        authorization.contains("algorithm=SHA-256"),
        "{authorization}"
    );
    assert!(
        !authorization.contains("secret"),
        "the password never appears on the wire: {authorization}"
    );

    host.tick(20);
    assert_eq!(
        host.receive(&respond_to(&second, "200 OK", &["Expires: 600"], None)),
        0
    );

    // "exactly two WIRE REGISTERs"
    let registers: Vec<&str> = host
        .wires()
        .into_iter()
        .filter(|wire| wire.starts_with("REGISTER "))
        .collect();
    assert_eq!(registers.len(), 2, "{registers:#?}");

    // "a refresh TIMER_SET, events `registration` (registered) then `outcome` ok, in that order"
    let refresh = host
        .position(
            |record| matches!(record, Out::TimerSet { fire_at_ms, .. } if *fire_at_ms >= 540_000),
        )
        .expect("a refresh timer at nine tenths of six hundred seconds");
    let registered = host
        .position(|record| {
            record.as_event().is_some_and(|event| {
                event.contains(r#""evt":"registration""#)
                    && event.contains(r#""state":"registered""#)
            })
        })
        .expect("a registration event");
    let outcome = host
        .position(|record| {
            record.as_event().is_some_and(|event| {
                event.contains(r#""evt":"outcome""#) && event.contains(r#""id":1"#)
            })
        })
        .expect("the register command's outcome");

    assert!(
        refresh < registered,
        "TIMER_SET precedes the registration event"
    );
    assert!(registered < outcome, "the state event precedes the outcome");

    // The registration event is `BSDK-EVT-2`, byte for byte.
    assert_eq!(
        host.events_of("registration")
            .into_iter()
            .find(|event| event.contains(r#""state":"registered""#)),
        Some(r#"{"v":1,"evt":"registration","state":"registered","expires":600}"#)
    );
    let outcomes = host.events_of("outcome");
    assert_eq!(
        outcomes,
        vec![r#"{"v":1,"evt":"outcome","id":1,"ok":true}"#]
    );
}

/// `BSDK-STATE-2`: `"dial"` → `"local-media"` offer → 180 → 200 answer → `"media-applied"`.
///
/// Required: `"need-local-media"` before any WIRE; INVITE only after the offer validates; ACK only
/// after `"media-applied"`; kernel state `sipEstablished`.
#[test]
fn bsdk_state_2_an_outbound_call_reaches_sip_established() {
    let mut host = ready();

    assert_eq!(host.command(BSDK_CMD_2), 0);
    // "`need-local-media` before any WIRE" — and there is no WIRE at all yet.
    assert!(host.wires().is_empty(), "no SIP yet: {:?}", host.wires());
    let demands = host.events_of("need-local-media");
    assert_eq!(demands.len(), 1, "{:?}", host.events());
    assert_eq!(
        demands[0],
        r#"{"v":1,"evt":"need-local-media","call":1,"kind":"offer","constraints":{"audio":true,"video":false}}"#,
        "BSDK-EVT-3, byte for byte"
    );

    assert_eq!(host.command(&local_media(4, 1, "offer", BA_SDP_O1)), 0);
    let invite = host.wires().last().copied().expect("an INVITE").to_owned();
    assert!(
        invite.starts_with("INVITE sip:bob@example.net SIP/2.0\r\n"),
        "{invite}"
    );
    assert!(invite.ends_with(BA_SDP_O1), "the offer is the body");

    host.tick(10);
    assert_eq!(
        host.receive(&respond_to(&invite, "180 Ringing", &[], None)),
        0
    );
    assert!(
        host.snapshot().contains(r#""1":"ringing""#),
        "{}",
        host.snapshot()
    );

    host.clear_log();
    host.tick(10);
    assert_eq!(
        host.receive(&respond_to(
            &invite,
            "200 OK",
            &[
                "Contact: <sip:bob@192.0.2.99;transport=ws>",
                "To: <sip:bob@example.net>;tag=remote1",
            ],
            Some(BA_SDP_A1),
        )),
        0
    );
    // "ACK only after `media-applied`" — the answer has been delivered and the ACK is held.
    assert!(
        host.wires().iter().all(|wire| !wire.starts_with("ACK ")),
        "the ACK is held: {:?}",
        host.wires()
    );
    let remote = host.events_of("remote-media");
    assert_eq!(remote.len(), 1, "{:?}", host.events());
    assert!(remote[0].contains(r#""kind":"answer""#), "{}", remote[0]);
    assert!(
        host.snapshot().contains(r#""1":"answerDelivered""#),
        "{}",
        host.snapshot()
    );

    host.clear_log();
    host.tick(5);
    assert_eq!(
        host.command(br#"{"v":1,"cmd":"media-applied","id":5,"call":1}"#),
        0
    );
    let acks: Vec<&str> = host
        .wires()
        .into_iter()
        .filter(|wire| wire.starts_with("ACK "))
        .collect();
    assert_eq!(acks.len(), 1, "exactly one ACK: {:?}", host.wires());
    assert!(
        acks[0].contains("Call-ID: "),
        "the ACK is in the INVITE's dialog: {}",
        acks[0]
    );
    assert!(
        host.snapshot().contains(r#""1":"sipEstablished""#),
        "{}",
        host.snapshot()
    );
    // The `dial`'s own promise settles here, not at acceptance (§5.2).
    assert!(
        host.events_of("outcome")
            .iter()
            .any(|event| event.contains(r#""id":2"#) && event.contains(r#""ok":true"#)),
        "{:?}",
        host.events()
    );
}

/// `BSDK-STATE-3`: a profile-valid INVITE in → `"ring"` → `"answer"` → `"local-media"` answer →
/// ACK in.
#[test]
fn bsdk_state_3_an_inbound_call_reaches_sip_established() {
    let mut host = ready();
    assert_eq!(host.receive(&inbound_invite(BA_SDP_O1)), 0);

    // "events `call` (incoming) then `remote-media`"
    let call_event = host
        .position(|record| {
            record
                .as_event()
                .is_some_and(|e| e.contains(r#""state":"incoming""#))
        })
        .expect("a call event");
    let remote = host
        .position(|record| {
            record
                .as_event()
                .is_some_and(|e| e.contains(r#""evt":"remote-media""#))
        })
        .expect("a remote-media event");
    assert!(call_event < remote, "the call event comes first");
    assert!(
        host.events_of("remote-media")[0].contains(r#""kind":"offer""#),
        "{:?}",
        host.events_of("remote-media")
    );

    host.clear_log();
    assert_eq!(host.command(br#"{"v":1,"cmd":"ring","id":6,"call":1}"#), 0);
    let ringing = host.wires().last().copied().expect("a 180").to_owned();
    assert!(ringing.starts_with("SIP/2.0 180 Ringing\r\n"), "{ringing}");
    assert!(
        header(&ringing, "To").is_some_and(|to| to.contains(";tag=")),
        "a dialog-forming provisional carries this endpoint's tag: {ringing}"
    );

    host.clear_log();
    assert_eq!(
        host.command(br#"{"v":1,"cmd":"answer","id":7,"call":1}"#),
        0
    );
    assert!(
        host.wires().is_empty(),
        "no 200 before the answer exists: {:?}",
        host.wires()
    );
    assert_eq!(
        host.events_of("need-local-media"),
        vec![
            r#"{"v":1,"evt":"need-local-media","call":1,"kind":"answer","constraints":{"audio":true,"video":false}}"#
        ]
    );

    host.clear_log();
    assert_eq!(host.command(&local_media(8, 1, "answer", BA_SDP_A1)), 0);
    let ok = host.wires().last().copied().expect("a 200").to_owned();
    assert!(ok.starts_with("SIP/2.0 200 OK\r\n"), "{ok}");
    assert!(ok.ends_with(BA_SDP_A1), "the answer is the body");
    assert!(
        host.snapshot().contains(r#""1":"answerSent""#),
        "{}",
        host.snapshot()
    );

    host.clear_log();
    host.tick(5);
    assert_eq!(host.receive(&inbound_ack(&ok)), 0);
    assert!(
        host.snapshot().contains(r#""1":"sipEstablished""#),
        "{}",
        host.snapshot()
    );
}

/// `BSDK-STATE-4`: `"dial"` → abort before `"local-media"`.
///
/// Required at the kernel: no WIRE ever emitted, and `"call-ended"` cause `local`. (Stopping the
/// tracks acquired for the call is the JavaScript layer's half of the row, per §6.3.)
#[test]
fn bsdk_state_4_a_dial_aborted_before_any_media() {
    let mut host = ready();
    assert_eq!(host.command(BSDK_CMD_2), 0);
    assert_eq!(
        host.command(br#"{"v":1,"cmd":"hangup","id":3,"call":1}"#),
        0
    );

    assert!(
        host.wires().is_empty(),
        "no SIP was ever emitted: {:?}",
        host.wires()
    );
    let ended = host.events_of("call-ended");
    assert_eq!(ended.len(), 1, "{:?}", host.events());
    assert_eq!(
        ended[0],
        r#"{"v":1,"evt":"call-ended","call":1,"cause":{"class":"local"}}"#
    );
    assert!(
        host.snapshot().contains(r#""calls":{}"#),
        "{}",
        host.snapshot()
    );
}

/// `BSDK-STATE-5`: `"dial"` → INVITE sent → `"hangup"`.
///
/// Required: CANCEL emitted; on the 487 exchange, `"call-ended"` cause `local`; every timer the
/// call set is cancelled by `TIMER_CANCEL`.
#[test]
fn bsdk_state_5_cancelling_an_invite_in_flight() {
    let mut host = ready();
    assert_eq!(host.command(BSDK_CMD_2), 0);
    assert_eq!(host.command(&local_media(4, 1, "offer", BA_SDP_O1)), 0);
    let invite = host.wires().last().copied().expect("an INVITE").to_owned();
    let invite_branch = header(&invite, "Via").expect("a Via");

    let set: Vec<u64> = host
        .log
        .iter()
        .filter_map(|record| match record {
            Out::TimerSet { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert!(
        !set.is_empty(),
        "the INVITE transaction set at least one timer"
    );

    host.clear_log();
    host.tick(10);
    assert_eq!(
        host.command(br#"{"v":1,"cmd":"hangup","id":3,"call":1}"#),
        0
    );
    let cancel = host
        .wires()
        .into_iter()
        .find(|wire| wire.starts_with("CANCEL "))
        .expect("a CANCEL")
        .to_owned();
    assert_eq!(
        header(&cancel, "Via").as_deref(),
        Some(invite_branch.as_str()),
        "RFC 3261 §9.1: the CANCEL reuses the INVITE's branch"
    );

    host.tick(10);
    assert_eq!(host.receive(&respond_to(&cancel, "200 OK", &[], None)), 0);
    assert_eq!(
        host.receive(&respond_to(
            &invite,
            "487 Request Terminated",
            &["To: <sip:bob@example.net>;tag=remote1"],
            None
        )),
        0
    );

    let ended = host.events_of("call-ended");
    assert_eq!(ended.len(), 1, "{:?}", host.events());
    assert!(ended[0].contains(r#""class":"local""#), "{}", ended[0]);

    let cancelled: Vec<u64> = host
        .log
        .iter()
        .filter_map(|record| match record {
            Out::TimerCancel(id) => Some(*id),
            _ => None,
        })
        .collect();
    for id in set {
        assert!(
            cancelled.contains(&id),
            "timer {id} was set for the call and never cancelled; cancelled: {cancelled:?}"
        );
    }
}

/// `BSDK-STATE-6`: mid-call `sipx_kernel_free`.
///
/// Required: returns `0`; every subsequent entry on the handle is `E_INVALID_HANDLE`; no output
/// record survives the free.
#[test]
fn bsdk_state_6_freeing_a_kernel_mid_call() {
    let mut host = ready();
    dial_to_answer_delivered(&mut host);
    assert!(host.snapshot().contains(r#""1":"answerDelivered""#));

    // Leave records queued so "no output record survives" has something to be true about.
    host.tick(5);
    let handle = host.handle;
    let applied: &[u8] = br#"{"v":1,"cmd":"media-applied","id":5,"call":1}"#;
    let len = support::len(applied);
    let ptr = host.abi().alloc_with(applied);
    assert_eq!(host.abi().command(handle, ptr, len, 100), 0);
    assert_ne!(
        host.abi().next_output(handle),
        0,
        "records are waiting to be drained"
    );

    assert_eq!(host.abi().kernel_free(handle), 0);
    assert_eq!(
        host.abi().next_output(handle),
        u64::from(sipx_wasm::Error::InvalidHandle.magnitude()),
        "no output record survives the free"
    );
    assert_eq!(
        host.abi().command(handle, ptr, len, 200),
        sipx_wasm::Error::InvalidHandle.code()
    );
    assert_eq!(
        host.abi().input_timer(handle, 1, 200),
        sipx_wasm::Error::InvalidHandle.code()
    );
    assert_eq!(host.abi().live_handles(), 0);

    // §6.5 step 4: the glue clears every host timer the kernel owned.
    assert!(
        !host.abi().last_teardown_cancellations().is_empty(),
        "the teardown names the timers the host still holds"
    );
}

/// An SDES master key, in the shape RFC 4568 §6.1 carries one. `BSDK-STATE-7` requires that
/// these bytes never reach an event, so they are named here to be searched for.
const SDES_KEY: &str = "PS1uQCVeeCFCanVmcjkpPywjNWhcYD0mXXX1Zj4/";

/// `BSDK-STATE-7`: a 200 answer carrying `a=crypto` (weaker media).
///
/// Required: the kernel refuses per profile — ACK then BYE, `"call-ended"` cause `media` — and the
/// SDES key bytes never appear in any event.
#[test]
fn bsdk_state_7_an_answer_that_weakens_the_media() {
    let mut host = ready();
    assert_eq!(host.command(BSDK_CMD_2), 0);
    assert_eq!(host.command(&local_media(4, 1, "offer", BA_SDP_O1)), 0);
    let invite = host.wires().last().copied().expect("an INVITE").to_owned();

    let weaker = BA_SDP_A1.replace(
        "a=setup:active\r\n",
        &format!("a=setup:active\r\na=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:{SDES_KEY}\r\n"),
    );

    host.clear_log();
    host.tick(10);
    assert_eq!(
        host.receive(&respond_to(
            &invite,
            "200 OK",
            &[
                "Contact: <sip:bob@192.0.2.99;transport=ws>",
                "To: <sip:bob@example.net>;tag=remote1",
            ],
            Some(&weaker),
        )),
        0
    );

    let wires = host.wires();
    let ack = wires.iter().position(|wire| wire.starts_with("ACK "));
    let bye = wires.iter().position(|wire| wire.starts_with("BYE "));
    assert!(ack.is_some(), "an ACK is owed for the 2xx: {wires:?}");
    assert!(bye.is_some(), "then a BYE: {wires:?}");
    assert!(ack < bye, "ACK then BYE, in that order: {wires:?}");

    let ended = host.events_of("call-ended");
    assert_eq!(ended.len(), 1, "{:?}", host.events());
    assert!(ended[0].contains(r#""class":"media""#), "{}", ended[0]);

    // The refusal names the rule, never the description.
    for event in host.events() {
        assert!(
            !event.contains(SDES_KEY),
            "an SDES key reached an event: {event}"
        );
        assert!(
            !event.contains("a=crypto"),
            "the refused description reached an event: {event}"
        );
    }
    assert!(
        host.events_of("remote-media").is_empty(),
        "an off-profile answer never reaches setRemoteDescription"
    );
}

/// `BSDK-STATE-8`: an INVITE whose offer has a video section.
///
/// Required: automatic 488; `refused_incoming` increments; no call object, no `"remote-media"`
/// event.
#[test]
fn bsdk_state_8_an_inbound_offer_with_video() {
    let mut host = ready();
    let with_video = format!(
        "{BA_SDP_O1}m=video 49180 UDP/TLS/RTP/SAVPF 96\r\nc=IN IP4 192.0.2.10\r\na=sendrecv\r\na=rtpmap:96 VP8/90000\r\n"
    );

    assert_eq!(host.receive(&inbound_invite(&with_video)), 0);

    let refusal = host.wires().last().copied().expect("a response").to_owned();
    assert!(
        refusal.starts_with("SIP/2.0 488 Not Acceptable Here\r\n"),
        "{refusal}"
    );
    assert!(
        host.snapshot().contains(r#""refused_incoming":1"#),
        "{}",
        host.snapshot()
    );
    assert!(
        host.snapshot().contains(r#""calls":{}"#),
        "no call object: {}",
        host.snapshot()
    );
    assert!(
        host.events_of("remote-media").is_empty(),
        "no remote-media event: {:?}",
        host.events()
    );
}

/// §3.3's other refusals arrive by the same door: a data-channel section is not a call either.
#[test]
fn an_inbound_offer_with_a_data_channel_is_refused_the_same_way() {
    let mut host = ready();
    let with_data = format!(
        "{BA_SDP_O1}m=application 49190 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 192.0.2.10\r\na=sctp-port:5000\r\n"
    );
    assert_eq!(host.receive(&inbound_invite(&with_data)), 0);
    assert!(
        host.wires()
            .last()
            .is_some_and(|wire| wire.starts_with("SIP/2.0 488 ")),
        "{:?}",
        host.wires()
    );
    assert!(
        host.snapshot().contains(r#""calls":{}"#),
        "{}",
        host.snapshot()
    );
}

/// §4.9's last row: "repeated `sipx_kernel_new`/`sipx_kernel_free` MUST return linear memory use
/// to its baseline (`S-41` proves this)".
///
/// On `wasm32-unknown-unknown` the strongest available statement is that the module's linear
/// memory does not grow across the cycles; everywhere the accounting the ABI itself keeps must
/// return to where it started, which is what catches a handle or an allocation that leaks.
#[test]
fn repeated_create_and_free_cycles_return_to_baseline() {
    let mut abi = sipx_wasm::Abi::new();
    let baseline_handles = abi.live_handles();
    let baseline_allocations = abi.live_allocations();

    #[cfg(target_arch = "wasm32")]
    let baseline_pages = core::arch::wasm32::memory_size(0);

    for _ in 0..1_000 {
        let ptr = abi.alloc_with(support::BSDK_CFG_1);
        let handle = abi.kernel_new(ptr, support::len(support::BSDK_CFG_1));
        abi.free(ptr, support::len(support::BSDK_CFG_1));
        assert!(handle > 0);

        let entropy = abi.alloc_with(&tape(0x11));
        assert_eq!(abi.input_entropy(handle, entropy, 256), 0);
        abi.free(entropy, 256);

        let command = abi.alloc_with(BSDK_CMD_2);
        assert_eq!(abi.command(handle, command, support::len(BSDK_CMD_2), 0), 0);
        abi.free(command, support::len(BSDK_CMD_2));
        while abi.next_output(handle) != 0 {}

        assert_eq!(abi.kernel_free(handle), 0);
    }

    assert_eq!(abi.live_handles(), baseline_handles, "handles leaked");
    assert_eq!(
        abi.live_allocations(),
        baseline_allocations,
        "host allocations leaked"
    );

    #[cfg(target_arch = "wasm32")]
    assert_eq!(
        core::arch::wasm32::memory_size(0),
        baseline_pages,
        "linear memory grew across a thousand create/free cycles"
    );
}

/// An INVITE arriving from the network, carrying `offer` as its body.
fn inbound_invite(offer: &str) -> String {
    format!(
        "INVITE sip:alice@example.net SIP/2.0\r\n\
         Via: SIP/2.0/WSS proxy.example.net;branch=z9hG4bKinbound0001\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:bob@example.net>;tag=remote2\r\n\
         To: <sip:alice@example.net>\r\n\
         Call-ID: inbound-call-1\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:bob@192.0.2.99;transport=ws>\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n{offer}",
        offer.len()
    )
}

/// The ACK that confirms a 200 this kernel sent.
fn inbound_ack(response: &str) -> String {
    let to = header(response, "To").unwrap_or_default();
    format!(
        "ACK sip:alice@example.net SIP/2.0\r\n\
         Via: SIP/2.0/WSS proxy.example.net;branch=z9hG4bKinbound0002\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:bob@example.net>;tag=remote2\r\n\
         To: {to}\r\n\
         Call-ID: inbound-call-1\r\n\
         CSeq: 1 ACK\r\n\
         Content-Length: 0\r\n\r\n"
    )
}
