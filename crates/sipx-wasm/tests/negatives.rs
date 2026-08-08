//! `docs/specs/browser-sdk.md` §9.5: the thirteen ABI negative vectors.
//!
//! Each row is one call against an otherwise healthy kernel, and the required result includes
//! "and kernel state is unchanged" in every case. The state check is not decoration: an ABI that
//! returns the right code while half-applying the call is worse than one that returns the wrong
//! code, because the page has no way to see it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use sipx_wasm::{Abi, Error};
use support::{BA_SDP_O1, BSDK_CFG_1, BSDK_CMD_1, BSDK_CMD_2, Host, tape};

/// A kernel with enough entropy that no vector below is refused for the wrong reason.
fn healthy() -> Host {
    let mut host = Host::new();
    host.entropy(&tape(0x10));
    host.clear_log();
    host
}

/// `BSDK-NEG-1`: any entry with handle `0` or an unallocated handle is `E_INVALID_HANDLE`.
#[test]
fn bsdk_neg_1_unknown_handles() {
    let mut host = healthy();
    let live = host.handle;
    let abi = host.abi();
    let ptr = abi.alloc_with(BSDK_CMD_1);
    let len = support::len(BSDK_CMD_1);

    for handle in [0, -1, live + 1, 9999] {
        assert_eq!(
            abi.command(handle, ptr, len, 0),
            Error::InvalidHandle.code()
        );
        assert_eq!(
            abi.input_bytes(handle, ptr, len, 0),
            Error::InvalidHandle.code()
        );
        assert_eq!(
            abi.input_entropy(handle, ptr, len),
            Error::InvalidHandle.code()
        );
        assert_eq!(abi.input_timer(handle, 1, 0), Error::InvalidHandle.code());
        assert_eq!(abi.kernel_free(handle), Error::InvalidHandle.code());
        // §4.2's packed error form: pointer `0`, magnitude in the length half.
        assert_eq!(
            abi.next_output(handle),
            u64::from(Error::InvalidHandle.magnitude())
        );
        assert_eq!(
            abi.snapshot(handle),
            u64::from(Error::InvalidHandle.magnitude())
        );
    }

    assert!(host.wires().is_empty());
    assert!(host.snapshot().contains(r#""registration":"unregistered""#));
}

/// `BSDK-NEG-2`: a pointer/length pair leaving linear memory is `E_BAD_POINTER`.
#[test]
fn bsdk_neg_2_a_pointer_that_was_never_allocated() {
    let mut host = healthy();
    let before = host.snapshot();

    assert_eq!(host.raw_command(0, 16), Error::BadPointer.code());
    assert_eq!(host.raw_command(0xdead_beef, 16), Error::BadPointer.code());

    // A live allocation with a length that runs past it is the same refusal.
    let short: &[u8] = b"{}";
    let ptr = host.abi().alloc_with(short);
    assert_eq!(host.raw_command(ptr, 4096), Error::BadPointer.code());

    assert_eq!(
        strip_rejections(&host.snapshot()),
        strip_rejections(&before),
        "kernel state is unchanged"
    );
}

/// `BSDK-NEG-3`: a command buffer holding invalid UTF-8 is `E_UTF8`.
#[test]
fn bsdk_neg_3_invalid_utf8() {
    let mut host = healthy();
    assert_eq!(host.command(&[0x7b, 0xff, 0x22, 0x7d]), Error::Utf8.code());
    assert!(host.wires().is_empty());
}

/// `BSDK-NEG-4`: `{"v":1,"cmd":` is `E_JSON`.
#[test]
fn bsdk_neg_4_truncated_json() {
    let mut host = healthy();
    assert_eq!(host.command(br#"{"v":1,"cmd":"#), Error::Json.code());
    assert!(host.wires().is_empty());
}

/// `BSDK-NEG-5`: `{"v":1,"cmd":"transfer","id":9}` is `E_SCHEMA` — the verb is not in §5.2.
#[test]
fn bsdk_neg_5_a_verb_outside_the_vocabulary() {
    let mut host = healthy();
    assert_eq!(
        host.command(br#"{"v":1,"cmd":"transfer","id":9}"#),
        Error::Schema.code()
    );
    assert!(host.wires().is_empty(), "{:?}", host.wires());
}

/// `BSDK-NEG-6`: `"answer"` naming a call in `Dialing` is `E_STATE`.
#[test]
fn bsdk_neg_6_answer_on_a_dialing_call() {
    let mut host = healthy();
    assert_eq!(host.command(BSDK_CMD_2), 0);
    assert!(
        host.snapshot().contains(r#""1":"dialing""#),
        "{}",
        host.snapshot()
    );
    host.clear_log();

    assert_eq!(
        host.command(br#"{"v":1,"cmd":"answer","id":3,"call":1}"#),
        Error::State.code()
    );
    assert!(
        host.snapshot().contains(r#""1":"dialing""#),
        "the call did not move: {}",
        host.snapshot()
    );
    assert!(host.wires().is_empty());
}

/// `BSDK-NEG-7`: a 32769-octet command document is `E_BOUNDS` **before JSON parsing**.
#[test]
fn bsdk_neg_7_an_oversize_command_document() {
    let mut host = healthy();
    // Well-formed JSON, so a kernel that parsed first would accept the verb and only then notice
    // the size. The padding lives in a field §5.1 requires both sides to ignore.
    let mut document = br#"{"v":1,"cmd":"unregister","id":9,"pad":""#.to_vec();
    document.resize(32_768, b'x');
    document.extend_from_slice(br#""}"#);
    assert!(document.len() > 32_768, "{}", document.len());

    assert_eq!(host.command(&document), Error::Bounds.code());
    assert!(host.wires().is_empty());

    // One octet under the bound the same document is merely refused for its state, which is what
    // shows the bound and not the content did the refusing above.
    let mut ok = br#"{"v":1,"cmd":"unregister","id":9,"pad":""#.to_vec();
    ok.resize(32_766 - 2, b'x');
    ok.extend_from_slice(br#""}"#);
    assert!(ok.len() <= 32_768);
    assert_eq!(host.command(&ok), Error::State.code(), "not registered yet");
}

/// `BSDK-NEG-8`: any entry after `sipx_kernel_free` is `E_INVALID_HANDLE` — handles are never
/// reused.
#[test]
fn bsdk_neg_8_use_after_free() {
    let mut host = healthy();
    let handle = host.handle;
    assert_eq!(host.abi().kernel_free(handle), 0);
    assert_eq!(host.abi().kernel_free(handle), Error::InvalidHandle.code());

    let abi = host.abi();
    let ptr = abi.alloc_with(BSDK_CMD_1);
    assert_eq!(
        abi.command(handle, ptr, support::len(BSDK_CMD_1), 10),
        Error::InvalidHandle.code()
    );
    assert_eq!(
        abi.next_output(handle),
        u64::from(Error::InvalidHandle.magnitude())
    );

    // A kernel created afterwards gets a *different* handle.
    let cfg = abi.alloc_with(BSDK_CFG_1);
    let next = abi.kernel_new(cfg, support::len(BSDK_CFG_1));
    assert!(next > 0);
    assert_ne!(next, handle, "handles are never reused");
}

/// `BSDK-NEG-9`: a ninth concurrent `"dial"` is an outcome failure `call-limit`, and the eight
/// live calls are untouched.
#[test]
fn bsdk_neg_9_the_ninth_dial() {
    let mut host = Host::new();
    // Twenty-four octets per dial, refilled generously so entropy is never the refusal.
    host.entropy(&tape(0x20));
    for id in 1..=8u64 {
        host.entropy(&tape(0x30));
        let document =
            format!(r#"{{"v":1,"cmd":"dial","id":{id},"target":"sip:bob@example.net"}}"#);
        assert_eq!(host.command(document.as_bytes()), 0, "dial {id}");
    }
    let before = host.snapshot();
    assert!(before.contains(r#""8":"dialing""#), "{before}");
    host.clear_log();

    host.entropy(&tape(0x40));
    host.clear_log();
    assert_eq!(
        host.command(br#"{"v":1,"cmd":"dial","id":9,"target":"sip:bob@example.net"}"#),
        0,
        "a typed refusal is an outcome, not an ABI error"
    );
    let outcomes = host.events_of("outcome");
    assert_eq!(outcomes.len(), 1, "{:?}", host.events());
    assert!(
        outcomes[0].contains(r#""ok":false"#) && outcomes[0].contains(r#""code":"call-limit""#),
        "{}",
        outcomes[0]
    );
    assert_eq!(
        strip_rejections(&host.snapshot()),
        strip_rejections(&before),
        "the eight live calls are untouched"
    );
}

/// `BSDK-NEG-10`: overflowing the 1024-octet pool is `E_BOUNDS` with the pool unchanged.
#[test]
fn bsdk_neg_10_entropy_overflow() {
    let mut host = Host::new();
    assert_eq!(host.entropy(&vec![0x11; 1024]), 0);
    assert!(
        host.snapshot().contains(r#""entropy":1024"#),
        "{}",
        host.snapshot()
    );

    assert_eq!(host.entropy(&[0x22]), Error::Bounds.code());
    assert!(
        host.snapshot().contains(r#""entropy":1024"#),
        "the pool is unchanged: {}",
        host.snapshot()
    );
}

/// `BSDK-NEG-11`: `now_ms` lower than the previous call's is `E_TIME`.
#[test]
fn bsdk_neg_11_a_clock_that_went_backwards() {
    let mut host = healthy();
    assert_eq!(host.command_at(BSDK_CMD_1, 5_000), 0);
    let before = host.snapshot();
    host.clear_log();

    assert_eq!(
        host.command_at(br#"{"v":1,"cmd":"unregister","id":9}"#, 4_999),
        Error::Time.code()
    );
    assert!(
        host.wires().is_empty(),
        "nothing was sent: {:?}",
        host.wires()
    );
    assert_eq!(
        strip_rejections(&host.snapshot()),
        strip_rejections(&before),
        "kernel state is unchanged"
    );

    // The same instant is not a regression. A `dial` rather than an `unregister`, because
    // `unregister` is separately illegal here: the REGISTER is still in flight.
    assert_eq!(
        host.command_at(
            br#"{"v":1,"cmd":"dial","id":10,"target":"sip:bob@example.net"}"#,
            5_000
        ),
        0
    );
}

/// `BSDK-NEG-12`: `sipx_input_bytes` carrying 64 KiB + 1 is `E_BOUNDS`, and nothing is parsed.
#[test]
fn bsdk_neg_12_an_oversize_sip_message() {
    let mut host = healthy();
    let oversize = vec![b'A'; 64 * 1024 + 1];
    assert_eq!(host.receive_bytes(&oversize), Error::Bounds.code());
    assert!(
        host.snapshot().contains(r#""parse_errors":0"#),
        "nothing was parsed, so nothing failed to parse: {}",
        host.snapshot()
    );

    // A message inside the bound reaches the parser and is counted there instead.
    assert_eq!(host.receive_bytes(&vec![b'A'; 64 * 1024]), 0);
    assert!(
        host.snapshot().contains(r#""parse_errors":1"#),
        "{}",
        host.snapshot()
    );
}

/// `BSDK-NEG-13`: garbage bytes return `0`, `parse_errors` increments, and no event invents a
/// call.
#[test]
fn bsdk_neg_13_garbage_is_a_value_not_an_error() {
    let mut host = healthy();
    for garbage in [
        &b"not a sip message at all"[..],
        &[0x00, 0xff, 0x01, 0x80][..],
        b"INVITE\r\n\r\n",
        b"SIP/2.0 \r\n",
    ] {
        assert_eq!(
            host.receive_bytes(garbage),
            0,
            "hostile network input is a value, not a host-contract violation"
        );
    }
    assert!(
        host.snapshot().contains(r#""parse_errors":4"#),
        "{}",
        host.snapshot()
    );
    assert!(
        host.snapshot().contains(r#""calls":{}"#),
        "{}",
        host.snapshot()
    );
    assert!(
        host.events().is_empty(),
        "no event invents a call: {:?}",
        host.events()
    );
}

/// §4.9's handle cap: a seventeenth `sipx_kernel_new` is `E_LIMIT`.
#[test]
fn the_seventeenth_handle_is_refused() {
    let mut abi = Abi::new();
    for index in 1..=16 {
        let ptr = abi.alloc_with(BSDK_CFG_1);
        let handle = abi.kernel_new(ptr, support::len(BSDK_CFG_1));
        abi.free(ptr, support::len(BSDK_CFG_1));
        assert!(handle > 0, "handle {index}");
    }
    let ptr = abi.alloc_with(BSDK_CFG_1);
    assert_eq!(
        abi.kernel_new(ptr, support::len(BSDK_CFG_1)),
        Error::Limit.code()
    );
    assert_eq!(abi.live_handles(), 16);
}

/// §4.9's SDP bound: a description over 16 KiB is a typed refusal in the outcome, not an ABI
/// error — the command was well-formed, the description was not carriable.
#[test]
fn an_oversize_description_is_a_typed_refusal() {
    let mut host = healthy();
    assert_eq!(host.command(BSDK_CMD_2), 0);
    host.clear_log();

    let padded = format!("{BA_SDP_O1}a=pad:{}\r\n", "x".repeat(17_000));
    let document = serde_json::json!({
        "v": 1, "cmd": "local-media", "id": 4, "call": 1, "kind": "offer", "sdp": padded,
    })
    .to_string();
    assert_eq!(host.command(document.as_bytes()), 0);

    let outcomes = host.events_of("outcome");
    assert_eq!(outcomes.len(), 1, "{:?}", host.events());
    assert!(
        outcomes[0].contains(r#""code":"sdp-too-large""#),
        "{}",
        outcomes[0]
    );
    assert!(
        host.wires().is_empty(),
        "no INVITE was sent: {:?}",
        host.wires()
    );
}

/// §5.1: a command id already in flight is refused rather than producing two indistinguishable
/// outcomes.
#[test]
fn a_reused_command_id_is_refused_while_the_first_is_unfinished() {
    let mut host = healthy();
    assert_eq!(host.command(BSDK_CMD_1), 0);
    // `register`'s outcome follows the final response to REGISTER, so id 1 is still unfinished.
    assert_eq!(host.command(BSDK_CMD_1), Error::State.code());
}

/// The rejection counters move, and only the counters move, when a call is refused. Used by the
/// "state is unchanged" assertions above.
fn strip_rejections(snapshot: &str) -> String {
    let Some(start) = snapshot.find(r#""counters":{"#) else {
        return snapshot.to_owned();
    };
    snapshot[..start].to_owned()
}
