//! RFC 7714's own AES-GCM vectors, against the numbers the RFC publishes.
//!
//! This is the test that matters most for the AEAD profiles, and the reason is the one
//! [`super`] gives for RFC 3711 §B.3: an IV formation or an associated-data boundary that is
//! *wrong but self-consistent* produces two endpoints that protect and unprotect each other's
//! packets perfectly and interoperate with nothing else in the world. Every round-trip test in
//! this crate would pass. Only numbers this implementation did not produce can tell them apart.
//!
//! The vectors are **not transcribed**. `scripts/import-rfc7714-corpus.sh` slices sections 16 and
//! 17 out of the RFC editor's own text into `crates/sipx-testkit/corpus/rfc7714/`, and its
//! `--check` mode — a gate step — re-slices and diffs, which is what stops a fixture being nudged
//! into agreement with an implementation that disagreed with it. What follows reads the RFC's own
//! labelled lines (`Key:`, `salt:`, `AAD:`) straight out of those files.
//!
//! It lives beside the transform rather than in `tests/` because RFC 7714 §16 publishes *session*
//! keys and no master key at all: a context built the way a call builds one would derive different
//! keys and the vectors would say nothing. Reaching past key derivation is a thing a test may do
//! and an API may not, so the reaching happens here, through private fields, and adds no public
//! surface anybody could key a real stream through.

use super::{Context, Label, Profile, Session, SrtpError, derive};

use std::path::PathBuf;

fn corpus(name: &str) -> String {
    let path = PathBuf::from(format!(
        "{}/../sipx-testkit/corpus/rfc7714/{name}",
        env!("CARGO_MANIFEST_DIR")
    ));
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{}: {error} — run scripts/import-rfc7714-corpus.sh",
            path.display()
        )
    })
}

/// One numbered subsection of the RFC, as its own lines.
fn section<'a>(text: &'a str, heading: &str) -> Vec<&'a str> {
    let mut lines = text.lines().skip_while(|line| !line.starts_with(heading));
    let first = lines
        .next()
        .unwrap_or_else(|| panic!("RFC 7714 has no section starting {heading:?}"));
    let rest = lines.take_while(|line| !is_heading(line));
    std::iter::once(first).chain(rest).collect()
}

/// A numbered section heading — `16.1.2.  …`. Recognised by shape, so a slice ends at the next
/// one whatever it happens to be called.
fn is_heading(line: &str) -> bool {
    line.chars().next().is_some_and(|c| c.is_ascii_digit())
        && line
            .split_whitespace()
            .next()
            .is_some_and(|first| first.ends_with('.'))
}

/// The hex following a `Label:` in one section, including any continuation lines.
///
/// The RFC writes hex two ways — `8040f17b 8041f8d3` and `00 01 02 03` — and wraps long values
/// onto indented lines with no label of their own. Both forms and the wrapping appear inside a
/// single block, sometimes for one field, so both are handled here.
fn field(section: &[&str], label: &str) -> Vec<u8> {
    let marker = format!("{label}:");
    let start = section
        .iter()
        .position(|line| line.trim_start().starts_with(&marker))
        .unwrap_or_else(|| panic!("no {marker} in this section"));

    let mut hex = section[start].trim_start()[marker.len()..]
        .trim()
        .to_owned();
    for line in &section[start + 1..] {
        if !is_bare_hex(line) {
            break;
        }
        hex.push(' ');
        hex.push_str(line.trim());
    }
    unhex(&hex)
}

/// The hex block that follows an introductory sentence — "Encrypting the following packet:".
fn block(section: &[&str], after: &str) -> Vec<u8> {
    let start = section
        .iter()
        .position(|line| line.contains(after))
        .unwrap_or_else(|| panic!("no line containing {after:?} in this section"));
    let mut hex = String::new();
    for line in &section[start + 1..] {
        if line.trim().is_empty() {
            if hex.is_empty() {
                continue;
            }
            break;
        }
        if !is_bare_hex(line) {
            break;
        }
        hex.push(' ');
        hex.push_str(line.trim());
    }
    unhex(&hex)
}

fn is_bare_hex(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_hexdigit() || c == ' ')
}

fn unhex(text: &str) -> Vec<u8> {
    let digits: Vec<char> = text.chars().filter(char::is_ascii_hexdigit).collect();
    assert!(!digits.is_empty(), "no hex digits in {text:?}");
    assert!(
        digits.len().is_multiple_of(2),
        "odd number of hex digits in {text:?}"
    );
    digits
        .chunks(2)
        .map(|pair| {
            let byte: String = pair.iter().collect();
            u8::from_str_radix(&byte, 16).expect("hex")
        })
        .collect()
}

/// A context whose **session** keys are the RFC's, with key derivation stepped over.
///
/// §16 states a session key and a session salt and no master key, so there is no input that would
/// make `Context::new` produce them. Both directions of the transform are the same code either
/// way; what this skips is §11's KDF, which RFC 7714 publishes no vector for and which
/// `key_derivation_matches_the_rfc` pins separately for the counter-mode profile.
fn keyed(profile: Profile, key: &[u8], salt: &[u8]) -> Context {
    Context {
        profile,
        session: Session {
            rtp_key: key.to_vec(),
            rtp_salt: salt.to_vec(),
            rtp_auth: Vec::new(),
            rtcp_key: key.to_vec(),
            rtcp_salt: salt.to_vec(),
            rtcp_auth: Vec::new(),
        },
        roc: 0,
        highest_seq: None,
        replay: 0,
        rtcp_index: 0,
        highest_rtcp_index: None,
        rtcp_replay: 0,
    }
}

/// The key and salt every vector in §16 and §17 shares, stated in §16's preamble: "The 16-octet
/// (128-bit) key is 00 01 02 ... 0f, and the 32-octet (256-bit) key is 00 01 02 ... 1f. … The
/// salt used (51756964 2070726f 2071756f) comes from the ASCII string 'Quid pro quo'."
fn material(profile: Profile) -> (Vec<u8>, Vec<u8>) {
    let key: Vec<u8> = (0..profile.master_key_len())
        .map(|n| u8::try_from(n).unwrap_or(0))
        .collect();
    (key, b"Quid pro quo".to_vec())
}

fn profile_of(heading: &str) -> Profile {
    if heading.contains("128") {
        Profile::AeadAes128Gcm
    } else {
        Profile::AeadAes256Gcm
    }
}

/// §16.1.1 and §16.2.1: protecting the RFC's packet must produce the RFC's octets.
///
/// Compared as the **whole** protected packet rather than as the tag alone, so the header left in
/// the clear, the ciphertext and the tag are all pinned by one assertion and none of the three can
/// be right for the wrong reason.
#[test]
fn the_srtp_encryption_vectors_are_reproduced() {
    let text = corpus("rtp-vectors.txt");
    for heading in [
        "16.1.1.  SRTP AEAD_AES_128_GCM Encryption",
        "16.2.1.  SRTP AEAD_AES_256_GCM Encryption",
    ] {
        let vector = section(&text, heading);
        let profile = profile_of(heading);
        let (key, salt) = material(profile);
        assert_eq!(field(&vector, "Key"), key, "{heading}: §16's stated key");
        assert_eq!(field(&vector, "salt"), salt, "{heading}: §16's stated salt");

        let plain = block(&vector, "Encrypting the following packet");
        let expected = block(&vector, "Encrypted and tagged packet");

        let protected = keyed(profile, &key, &salt)
            .protect(&plain)
            .expect("protects");
        assert_eq!(
            protected, expected,
            "{heading}: the protected packet is not the one the RFC publishes"
        );
        assert_eq!(
            field(&vector, "AAD"),
            plain[..12],
            "{heading}: §8.2 makes the RTP header the Associated Data"
        );
        assert_eq!(
            field(&vector, "IV"),
            super::aead_rtp_iv(&salt, 0x5501_a0b2, 0, 0xf17b),
            "{heading}: §8.1's IV, formed from the packet rather than drawn"
        );
    }
}

/// §16.1.2 and §16.2.2: unprotecting the RFC's packet must recover the RFC's plaintext.
#[test]
fn the_srtp_decryption_vectors_are_reproduced() {
    let text = corpus("rtp-vectors.txt");
    for heading in [
        "16.1.2.  SRTP AEAD_AES_128_GCM Decryption",
        "16.2.2.  SRTP AEAD_AES_256_GCM Decryption",
    ] {
        let vector = section(&text, heading);
        let profile = profile_of(heading);
        let (key, salt) = material(profile);

        let protected = block(&vector, "Decrypting the following packet");
        // The RFC prints the recovered *payload* under this heading; the header is unchanged and
        // already above it in the same block.
        let payload = block(&vector, "Verified and tagged packet");

        let recovered = keyed(profile, &key, &salt)
            .unprotect(&protected)
            .expect("unprotects");
        assert_eq!(
            &recovered[..12],
            &protected[..12],
            "{heading}: the header is carried through unchanged"
        );
        assert_eq!(
            &recovered[12..],
            payload.as_slice(),
            "{heading}: the recovered payload is not the RFC's plaintext"
        );
    }
}

/// One flipped bit in the RFC's own encrypted packet, and it no longer authenticates.
///
/// The vectors above prove agreement; this proves the tag is load-bearing. A transform that
/// ignored the tag entirely would satisfy every assertion in them.
#[test]
fn an_altered_rfc_packet_does_not_verify() {
    let text = corpus("rtp-vectors.txt");
    let vector = section(&text, "16.1.2.  SRTP AEAD_AES_128_GCM Decryption");
    let profile = Profile::AeadAes128Gcm;
    let (key, salt) = material(profile);
    let protected = block(&vector, "Decrypting the following packet");

    // The header, the first ciphertext octet, the middle, and the tag itself.
    for index in [3usize, 12, 30, protected.len() - 1] {
        let mut altered = protected.clone();
        altered[index] ^= 0x01;
        assert_eq!(
            keyed(profile, &key, &salt).unprotect(&altered),
            Err(SrtpError::NotAuthentic),
            "a flipped bit at octet {index} must not authenticate"
        );
    }
}

/// §17.1: an encrypted and tagged SRTCP packet, including where the ESRTCP word ends up.
///
/// The field order is what this pins. RFC 7714 §9.2 puts the cipher **before** the ESRTCP word,
/// the reverse of RFC 3711 §3.4's trailer-then-tag layout, and a transform that kept the older
/// order would round-trip against itself and against nothing else.
#[test]
fn the_srtcp_encryption_vector_is_reproduced() {
    let text = corpus("rtcp-vectors.txt");
    let vector = section(
        &text,
        "17.1.  SRTCP AEAD_AES_128_GCM Encryption and Tagging",
    );
    let profile = Profile::AeadAes128Gcm;
    let (key, salt) = material(profile);
    assert_eq!(field(&vector, "Key"), key);
    assert_eq!(field(&vector, "salt"), salt);

    let plain = block(&vector, "Encrypting the following packet");
    let expected = block(&vector, "Encrypted and tagged packet");

    let mut context = keyed(profile, &key, &salt);
    // §17: "with 32-bit SRTCP index 000005d4". The sender's index is its own state, so it is set
    // here the same way this module's other SRTCP tests reach a chosen index.
    context.rtcp_index = 0x0000_05d4;
    let protected = context.protect_rtcp(&plain).expect("protects");
    assert_eq!(
        protected, expected,
        "the protected SRTCP packet is not the one the RFC publishes"
    );
    assert_eq!(
        &protected[protected.len() - 4..],
        &[0x80, 0x00, 0x05, 0xd4],
        "§9.2: the ESRTCP word follows the cipher rather than preceding the tag"
    );
    assert_eq!(
        field(&vector, "IV"),
        super::aead_rtcp_iv(&salt, 0x4d61_7273, 0x0000_05d4),
        "§9.1's IV carries the index without the E-flag"
    );
}

/// §17.2: verifying and decrypting an SRTCP packet under `AEAD_AES_256_GCM`.
#[test]
fn the_srtcp_decryption_vector_is_reproduced() {
    let text = corpus("rtcp-vectors.txt");
    let vector = section(
        &text,
        "17.2.  SRTCP AEAD_AES_256_GCM Verification and Decryption",
    );
    let profile = Profile::AeadAes256Gcm;
    let (key, salt) = material(profile);

    let protected = block(&vector, "Decrypting the following packet");
    // §17.2 prints the whole recovered packet under this heading, where §16.1.2 printed only the
    // payload. Asserted as the RFC writes it rather than reshaped to match its sibling.
    let expected = block(&vector, "Verified and decrypted packet");

    let recovered = keyed(profile, &key, &salt)
        .unprotect_rtcp(&protected)
        .expect("unprotects");
    assert_eq!(
        &recovered[..8],
        &protected[..8],
        "the eight-octet RTCP header is carried through unchanged"
    );
    assert_eq!(
        recovered, expected,
        "the recovered SRTCP packet is not the RFC's plaintext"
    );
}

/// §17.3 and §17.4: the unencrypted form, where the cipher is the tag and nothing else.
///
/// sipx never *sends* this — every SRTCP packet it protects sets the E-flag — but a peer may, and
/// §9.3's associated-data rule for it is different enough from §9.2's (the whole packet, not the
/// eight-octet header) that a receiver which guessed would drop every report that arrived.
#[test]
fn the_srtcp_tagging_only_vectors_are_reproduced() {
    let text = corpus("rtcp-vectors.txt");
    let vector = section(&text, "17.3.  SRTCP AEAD_AES_128_GCM Tagging Only");
    let profile = Profile::AeadAes128Gcm;
    let (key, salt) = material(profile);

    let plain = block(&vector, "Tagging the following packet");
    let expected = block(&vector, "Tagged packet");

    let context = keyed(profile, &key, &salt);
    let ssrc = u32::from_be_bytes([plain[4], plain[5], plain[6], plain[7]]);
    let tagged = context
        .aead_protect_rtcp(&plain, ssrc, 0x0000_05d4, false)
        .expect("tags");
    assert_eq!(
        tagged, expected,
        "the tagged SRTCP packet is not the one the RFC publishes"
    );
    assert_eq!(
        &tagged[tagged.len() - 4..],
        &[0x00, 0x00, 0x05, 0xd4],
        "the E-flag is clear when the packet was tagged and not encrypted"
    );

    // §17.4's verification, in the direction a receiver runs it: the same packet back through
    // `unprotect_rtcp` comes out whole, because nothing in it was encrypted.
    assert_eq!(
        keyed(profile, &key, &salt)
            .unprotect_rtcp(&tagged)
            .expect("verifies"),
        plain,
        "an unencrypted SRTCP packet verifies and is returned unchanged"
    );
}

/// §12's parameter tables, as the type states them.
#[test]
fn the_aead_profiles_carry_the_lengths_the_rfc_tabulates() {
    assert_eq!(
        Profile::AeadAes128Gcm.key_and_salt_len(),
        (16, 12),
        "§12 Table 2: 128-bit master key, 96-bit master salt"
    );
    assert_eq!(
        Profile::AeadAes256Gcm.key_and_salt_len(),
        (32, 12),
        "§12 Table 3: 256-bit master key, 96-bit master salt"
    );
    for profile in [Profile::AeadAes128Gcm, Profile::AeadAes256Gcm] {
        assert_eq!(
            profile.tag_len(),
            16,
            "§13.2 refuses a truncated AEAD tag outright"
        );
        assert!(profile.is_aead(), "{profile:?}");
    }
}

/// §11: `AEAD_AES_256_GCM` derives through AES-256, not AES-128.
///
/// No RFC publishes a KDF vector for the AEAD profiles, so this cannot be checked against a
/// published number. What it *can* pin is that the two 256-bit halves of a master key both reach
/// the key schedule: a KDF that silently used the first sixteen octets would derive identical
/// session keys for two master keys that differ only after octet 16, and every round-trip test
/// would still pass. `docs/specs/srtp.md` §4.3 records this as the one AEAD parameter under test
/// here rather than against the RFC.
#[test]
fn the_256_bit_kdf_reads_the_whole_master_key() {
    let salt = vec![0x11u8; 12];
    let mut first = [0u8; 32];
    let mut second = [0u8; 32];

    let mut key = vec![0u8; 32];
    derive(&key, &salt, Label::RtpEncryption, &mut first);
    key[31] ^= 0xFF;
    derive(&key, &salt, Label::RtpEncryption, &mut second);

    assert_ne!(
        first, second,
        "the last octet of a 256-bit master key must reach the key schedule; if it does not, the \
         PRF is AES-128 over the first half and RFC 6188's AES_256_CM_PRF is not what ran"
    );
}
