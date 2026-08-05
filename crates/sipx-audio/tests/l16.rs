//! RFC 3551 §4.5.11 L16 byte-order vectors (`M-43`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

/// M-43 / `linear-pcm.md` L16-1: L16 is signed 16-bit network byte order, not native or little
/// endian. Exact bytes are the interoperability proof; a round trip alone could mirror one bug.
#[test]
fn l16_uses_signed_network_byte_order() {
    let samples = [-32_768, 0, 32_767];
    let encoded = sipx_audio::l16::encode(&samples);
    assert_eq!(encoded, [0x80, 0x00, 0x00, 0x00, 0x7f, 0xff]);
    assert_eq!(
        sipx_audio::l16::decode(&encoded).expect("whole samples"),
        samples
    );
}

/// A partial network word cannot become a sample whose missing byte was guessed.
#[test]
fn l16_refuses_an_odd_payload() {
    assert!(sipx_audio::l16::decode(&[0x12]).is_err());
}
