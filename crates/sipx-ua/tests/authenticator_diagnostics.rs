//! The signing key never reaches a diagnostic (`M-110`'s adjacent finding, `M-117`).
//!
//! `Authenticator` derived `Debug` over `secret: [u8; 32]`. That is not a credential leak in the
//! ordinary sense — it is the key every self-describing nonce is `MACed` with, so a reader of that
//! record can mint nonces this authenticator accepts as its own, which is the whole of the replay
//! protection. Nothing logged one, which is what made it latent rather than live: exactly the state
//! `PcmFrame` was in for one release before `M-107`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sipx_ua::Authenticator;

#[test]
fn the_signing_key_never_reaches_a_diagnostic() {
    let secret = [0xA7_u8; 32];
    let authenticator = Authenticator::new("example.net", secret);
    let rendered = format!("{authenticator:?}");

    // Every spelling the key could survive as: the byte's own decimal, its hex in both cases, and
    // the printable run a `Bytes`-style renderer would produce.
    for spelling in ["167", "a7", "A7", "\\xa7"] {
        assert!(
            !rendered.contains(spelling),
            "the nonce signing key survived as {spelling:?} in: {rendered}"
        );
    }
    assert!(rendered.contains("redacted"), "{rendered}");
    // What a reader actually needs is still there.
    assert!(rendered.contains("example.net"), "{rendered}");
}
