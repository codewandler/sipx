//! `docs/specs/browser-sdk.md` §9.2 to §9.4: the byte, framing and entropy vectors.
//!
//! Every vector is pinned by SHA-256 in §9.1's table, and this file checks the hash as well as
//! the bytes. That is the difference between "the kernel emits something plausible" and "the
//! kernel emits the contract": a vector whose length drifted by one octet still reads correctly
//! and is still wrong.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use sha2::{Digest as _, Sha256};
use support::{
    BSDK_CFG_1, BSDK_CMD_1, BSDK_EVT_1, BSDK_EVT_2, BSDK_EVT_3, Host, Out, decode, ent_1_tape,
    header,
};

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// §9.1's table, for the control-plane documents this crate must produce or consume verbatim.
#[test]
fn bsdk_control_plane_vectors_match_their_pinned_hashes() {
    for (id, bytes, octets, digest) in [
        (
            "BSDK-CFG-1",
            BSDK_CFG_1,
            178,
            "018dc212a2ff5646bc36a9737e28f9403407251d26eb23d77e1a1d11f7d20249",
        ),
        (
            "BSDK-CMD-1",
            BSDK_CMD_1,
            45,
            "73f99097e0a7dd0d96276ddf13c723cdb8e0e4da696d4d2440f98b0b6c5b26e0",
        ),
        (
            "BSDK-EVT-1",
            BSDK_EVT_1,
            37,
            "f2e4ac91f369ca513024f07b09135a0adf279b82b76eb8252877cc78c1614037",
        ),
        (
            "BSDK-EVT-2",
            BSDK_EVT_2,
            63,
            "c500a036d7ccef02c9f27834703a26be2db6398808fad8b0c428d767d78799c7",
        ),
        (
            "BSDK-EVT-3",
            BSDK_EVT_3,
            99,
            "63ece231c4c76af024d701aca7558611a99cfaf3e6c504c2eebbbbe070d4ed4a",
        ),
    ] {
        assert_eq!(bytes.len(), octets, "{id} is {octets} octets");
        assert_eq!(sha256(bytes), digest, "{id} hash");
    }
}

/// `BSDK-EVT-1`: a fresh kernel with an empty pool asks for entropy in exactly these bytes.
#[test]
fn bsdk_evt_1_is_what_an_empty_pool_emits() {
    let mut host = Host::new();
    // One octet is enough to make the kernel look at the pool, and the pool is still below the
    // low-water mark afterwards.
    assert_eq!(host.entropy(&[0x00]), 0);
    let events = host.events();
    assert_eq!(events.len(), 1, "one event: {events:?}");
    assert_eq!(events[0].as_bytes(), BSDK_EVT_1);
    assert_eq!(sha256(events[0].as_bytes()), sha256(BSDK_EVT_1));
}

/// `BSDK-OUT-1`, §9.3: the §4.6 record carrying `BSDK-EVT-1` — type `4`, length `37`, payload.
#[test]
fn bsdk_out_1_frames_the_entropy_demand() {
    let mut host = Host::new();
    assert_eq!(host.raw_entropy(0, 0), -2);
    // A pointer the table does not know is `E_BAD_POINTER` and changes nothing, so the pool is
    // still empty and the demand still has to be provoked by a legal feed.
    host.clear_log();
    assert_eq!(host.entropy(&[0x00]), 0);

    let mut framed = Vec::new();
    framed.extend_from_slice(&4u32.to_le_bytes());
    framed.extend_from_slice(&37u32.to_le_bytes());
    framed.extend_from_slice(BSDK_EVT_1);
    assert_eq!(framed.len(), 45);
    assert_eq!(
        sha256(&framed),
        "e94b52e04f1ee6991926e77805024290f88906c7a1c027c32afea07ac85975e6"
    );
    assert_eq!(
        framed[..8],
        [0x04, 0x00, 0x00, 0x00, 0x25, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        decode(&framed),
        Out::Event(String::from_utf8(BSDK_EVT_1.to_vec()).unwrap())
    );

    assert_eq!(
        host.events()[0].as_bytes(),
        BSDK_EVT_1,
        "the kernel's own record carries the same payload"
    );
}

/// `BSDK-OUT-2`, §9.3: a complete `TIMER_SET`, id `1`, `fire_at_ms` `500`.
#[test]
fn bsdk_out_2_frames_a_timer_set() {
    let expected: [u8; 24] = [
        0x02, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xf4, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    assert_eq!(
        sha256(&expected),
        "5f35832f0b2d782d3da8d35a53f98a21f0ddb6fac1520e8cdbb0280297fc8ac2"
    );
    assert_eq!(
        decode(&expected),
        Out::TimerSet {
            id: 1,
            fire_at_ms: 500
        }
    );
    assert_eq!(
        sipx_wasm::Record::TimerSet {
            id: 1,
            fire_at_ms: 500
        }
        .encode(),
        expected.to_vec(),
        "the kernel's encoder produces the vector, not merely something decodable"
    );
}

/// `BSDK-ENT-1`, §9.4: feed `00 01 … 1f`, submit `BSDK-CMD-1`, and the REGISTER carries exactly
/// the pinned Call-ID, From tag and Via branch. The pool then holds zero octets, so the outputs
/// include a `"need-entropy"` event.
#[test]
fn bsdk_ent_1_pins_every_identifier_the_register_carries() {
    let mut host = Host::new();
    host.clear_log();
    assert_eq!(host.entropy(&ent_1_tape()), 0);
    // Thirty-two octets is already below the low-water mark of sixty-four, so the feed itself
    // draws a demand. That is §4.7 working, not a confounder: the vector's claim is about what
    // the pool holds *after* the REGISTER.
    host.clear_log();
    assert_eq!(host.command(BSDK_CMD_1), 0);

    let wires = host.wires();
    assert_eq!(wires.len(), 1, "one REGISTER: {wires:?}");
    let register = wires[0];
    assert!(register.starts_with("REGISTER "), "{register}");
    assert_eq!(
        header(register, "Call-ID").as_deref(),
        Some("000102030405060708090a0b0c0d0e0f")
    );
    let from = header(register, "From").expect("a From header");
    assert!(from.ends_with(";tag=1011121314151617"), "{from}");
    let via = header(register, "Via").expect("a Via header");
    assert!(via.ends_with(";branch=z9hG4bK18191a1b1c1d1e1f"), "{via}");

    // "and the pool then holds 0 octets, so the outputs include a `need-entropy` event"
    let demands = host.events_of("need-entropy");
    assert_eq!(demands.len(), 1, "{:?}", host.events());
    assert_eq!(demands[0].as_bytes(), BSDK_EVT_1);
    assert!(
        host.snapshot().contains(r#""entropy":0"#),
        "{}",
        host.snapshot()
    );
}

/// The other half of `BSDK-ENT-1`: "submitting a command that needs another identifier before
/// more entropy arrives fails `E_ENTROPY` with nothing consumed".
#[test]
fn bsdk_ent_1_refuses_a_second_identifier_from_an_empty_pool() {
    let mut host = Host::new();
    host.entropy(&ent_1_tape());
    assert_eq!(host.command(BSDK_CMD_1), 0);
    host.clear_log();

    let dial = br#"{"v":1,"cmd":"dial","id":2,"target":"sip:bob@example.net"}"#;
    assert_eq!(host.command(dial), -8, "E_ENTROPY");
    assert!(
        host.wires().is_empty(),
        "nothing was serialised: {:?}",
        host.wires()
    );
    assert!(
        host.snapshot().contains(r#""entropy":0"#),
        "nothing consumed: {}",
        host.snapshot()
    );
    // No call object was invented for the refused dial.
    assert!(
        host.snapshot().contains(r#""calls":{}"#),
        "{}",
        host.snapshot()
    );
}

/// §4.7's table, exercised through the ABI rather than through the pool directly: the four
/// identifiers consume 16, 8, 8 and 16 octets, in the order they are first required.
#[test]
fn the_derivation_tape_consumes_the_stated_widths() {
    let mut host = Host::new();
    // Exactly the 32 octets a first REGISTER needs: Call-ID, From tag, branch.
    host.entropy(&ent_1_tape());
    assert_eq!(host.command(BSDK_CMD_1), 0);
    assert!(host.snapshot().contains(r#""entropy":0"#));

    // A challenge needs a cnonce (16) and the retry's branch (8): 24 more.
    let register = host.wires()[0].to_owned();
    host.clear_log();
    host.entropy(&(0x40u8..0x58).collect::<Vec<u8>>());
    let challenge = support::respond_to(
        &register,
        "401 Unauthorized",
        &[r#"WWW-Authenticate: Digest realm="example.net", nonce="abc", qop="auth""#],
        None,
    );
    assert_eq!(host.receive(&challenge), 0);

    let retry = host.wires().last().copied().expect("a second REGISTER");
    let authorization = header(retry, "Authorization").expect("an Authorization header");
    assert!(
        authorization.contains(r#"cnonce="404142434445464748494a4b4c4d4e4f""#),
        "the cnonce is the next sixteen octets of the tape: {authorization}"
    );
    let via = header(retry, "Via").expect("a Via header");
    assert!(
        via.ends_with(";branch=z9hG4bK5051525354555657"),
        "the retry's branch is the eight octets after the cnonce: {via}"
    );
    assert!(
        host.snapshot().contains(r#""entropy":0"#),
        "{}",
        host.snapshot()
    );
}
