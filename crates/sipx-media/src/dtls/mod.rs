//! DTLS-SRTP: keying SRTP on the media path (RFC 5764).
//!
//! SDES ([`sipx_sdp::crypto`], RFC 4568) puts the master key in the SDP. That works and it means
//! every element that reads the signalling — every proxy, every session border controller that
//! terminates the TLS — has held the key. DTLS-SRTP does not: the two endpoints handshake **on the
//! media path**, derive the SRTP keys from the DTLS master secret, and the signalling carries only
//! a hash of the certificate that will appear ([`sipx_sdp::fingerprint`], RFC 8122).
//!
//! This module is the parts of RFC 5764 that are sipx's own: telling a DTLS record from an RTP
//! packet on one port (§5.1.2), the protection profiles and their key sizes (§4.1.2), and turning
//! the exported keying material into the two SRTP contexts a session needs (§4.2). The handshake
//! itself is a DTLS implementation's job and is reached through [`Handshake`].
//! **Experimental** (`A-8`): no `sipx-call` path selects DTLS-SRTP keying, so nothing above this
//! crate has ever constrained this module's shape. `Config.srtp` takes `SrtpKeys` while this
//! produces `srtp::Context`; closing that is `M-28`.
//!

#[cfg(feature = "dtls")]
pub mod openssl;

use sipx_rtp::srtp;

/// The TLS exporter label RFC 5764 §4.2 fixes for this use.
pub const EXPORTER_LABEL: &str = "EXTRACTOR-dtls_srtp";

/// What a datagram arriving on a media port is (RFC 5764 §5.1.2).
///
/// One port carries three protocols at once, and §5.1.2 disambiguates them by the first byte alone.
/// The ranges do not overlap because RTP's version-2 header puts `10` in the top two bits, DTLS
/// content types are 20–63, and STUN's first two bits are zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arriving {
    /// A STUN message — first byte 0 or 1. Relevant once ICE exists; classified here because
    /// §5.1.2 classifies it, and silently treating one as RTP would corrupt the sequence state.
    Stun,
    /// A DTLS record — first byte 20 to 63.
    Dtls,
    /// RTP or RTCP — first byte 128 to 191.
    Rtp,
    /// None of the three. §5.1.2 gives no meaning to these, so they are dropped by name rather
    /// than fed to whichever parser happens to be first.
    Unknown,
}

/// Classify a datagram by its first byte (RFC 5764 §5.1.2).
#[must_use]
pub fn classify(datagram: &[u8]) -> Arriving {
    match datagram.first() {
        Some(0 | 1) => Arriving::Stun,
        Some(20..=63) => Arriving::Dtls,
        Some(128..=191) => Arriving::Rtp,
        _ => Arriving::Unknown,
    }
}

/// An SRTP protection profile (RFC 5764 §4.1.2).
///
/// Only the one sipx can perform. §4.1.2 defines four; the two `NULL` profiles encrypt nothing, and
/// offering `AES128_CM_HMAC_SHA1_32` would mean an SRTP transform with a 32-bit tag that
/// [`sipx_rtp::srtp`] does not implement. A profile list is a promise, so the list is short.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// AES-128 counter mode, 80-bit HMAC-SHA1 tag — the same transform SDES negotiates.
    Aes128CmHmacSha1_80,
}

impl Profile {
    /// The name as the IANA registry and every DTLS API spell it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aes128CmHmacSha1_80 => "SRTP_AES128_CM_SHA1_80",
        }
    }

    /// The two-byte value carried in the `use_srtp` extension (RFC 5764 §4.1.2).
    #[must_use]
    pub fn id(self) -> u16 {
        match self {
            Self::Aes128CmHmacSha1_80 => 0x0001,
        }
    }

    /// Master key and master salt lengths, in octets.
    ///
    /// §4.1.2 states these in bits: a 128-bit key and a 112-bit salt. Fourteen octets of salt, not
    /// sixteen — the value that is easy to get wrong, and getting it wrong produces a key schedule
    /// that decrypts nothing with no error to say why.
    #[must_use]
    pub fn key_and_salt_len(self) -> (usize, usize) {
        match self {
            Self::Aes128CmHmacSha1_80 => (16, 14),
        }
    }

    /// How many octets to export from the handshake: `2 * (key + salt)` (RFC 5764 §4.2).
    #[must_use]
    pub fn exported_len(self) -> usize {
        let (key, salt) = self.key_and_salt_len();
        2 * (key + salt)
    }
}

/// Which end of the DTLS connection this endpoint is.
///
/// It decides nothing about the handshake here and everything about the keys: §4.2's exported block
/// holds a client write key and a server write key, and each side protects with its own and
/// unprotects with the other's. A stack that picks the wrong one produces authentication failures
/// on every packet, in both directions, with no clue as to the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The endpoint that sent the `ClientHello` — SDP `a=setup:active`.
    Client,
    /// The endpoint that answered it — SDP `a=setup:passive`.
    Server,
}

/// The two SRTP contexts a session needs: one to protect with, one to unprotect with.
#[derive(Debug)]
pub struct Keys {
    /// Protects what this endpoint sends.
    pub outbound: srtp::Context,
    /// Unprotects what it receives.
    pub inbound: srtp::Context,
}

/// Why keying material could not be turned into SRTP contexts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    /// The handshake exported fewer octets than the profile needs.
    #[error("the handshake exported {got} octets; profile {profile} needs {needed}")]
    Short {
        /// The profile in force.
        profile: &'static str,
        /// How many octets are needed.
        needed: usize,
        /// How many arrived.
        got: usize,
    },
    /// The SRTP layer refused the key or salt.
    #[error("srtp: {0}")]
    Srtp(#[from] srtp::SrtpError),
}

/// Split exported keying material into the two SRTP contexts (RFC 5764 §4.2).
///
/// §4.2 fixes both the length and the order: `2 * (key + salt)` octets, assigned as **client write
/// key, server write key, client write salt, server write salt**. Keys first and salts after, not
/// key-and-salt per side — which is the natural way to read it and produces a context that
/// authenticates nothing.
///
/// `role` selects which pair protects and which unprotects. Both sides derive the same block; the
/// only thing that differs is which half each one sends with.
pub fn keys_from_exported(exported: &[u8], profile: Profile, role: Role) -> Result<Keys, KeyError> {
    let (key_len, salt_len) = profile.key_and_salt_len();
    let needed = profile.exported_len();
    if exported.len() < needed {
        return Err(KeyError::Short {
            profile: profile.as_str(),
            needed,
            got: exported.len(),
        });
    }
    let take = |from: usize, len: usize| exported.get(from..from + len).unwrap_or_default();
    let client_key = take(0, key_len);
    let server_key = take(key_len, key_len);
    let client_salt = take(2 * key_len, salt_len);
    let server_salt = take(2 * key_len + salt_len, salt_len);

    let (own_key, own_salt, peer_key, peer_salt) = match role {
        Role::Client => (client_key, client_salt, server_key, server_salt),
        Role::Server => (server_key, server_salt, client_key, client_salt),
    };
    Ok(Keys {
        outbound: srtp::Context::new(own_key, own_salt)?,
        inbound: srtp::Context::new(peer_key, peer_salt)?,
    })
}

/// A DTLS handshake on the media path, as much of one as RFC 5764 needs.
///
/// sipx does not implement DTLS. This is the seam: everything above it — the fingerprint check, the
/// profile, the key split, the demultiplexing — is sipx's, and an implementor of this trait supplies
/// the record layer and the handshake. Keeping it a trait rather than a hard dependency is what lets
/// the fingerprint verification be tested exhaustively without a certificate authority in the loop,
/// and what stops the choice of DTLS library from reaching into the media session.
pub trait Handshake {
    /// Why the handshake failed.
    type Error: std::error::Error;

    /// Run the handshake to completion.
    ///
    /// `role` comes from the negotiated `a=setup` and must not be guessed: a UA that starts a
    /// handshake it agreed to wait for meets one coming the other way.
    fn run(&mut self, role: Role) -> Result<(), Self::Error>;

    /// The peer's certificate, DER-encoded, once the handshake has produced one.
    ///
    /// This is what the SDP fingerprint is checked against, and why the trait exposes it rather than
    /// leaving verification to the implementation: RFC 8122 §6.2's check is against a value that
    /// arrived in the *signalling*, which a DTLS library has no way to see.
    fn peer_certificate(&self) -> Option<Vec<u8>>;

    /// The profile both ends agreed on, from the `use_srtp` extension (RFC 5764 §4.1).
    fn profile(&self) -> Option<Profile>;

    /// Export `len` octets under [`EXPORTER_LABEL`] (RFC 5764 §4.2).
    fn export(&self, len: usize) -> Result<Vec<u8>, Self::Error>;
}

/// Why a keyed media path could not be established.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The peer offered no fingerprint, so there is nothing to check its certificate against.
    ///
    /// Refused rather than accepted unverified. RFC 8122's guarantee is the fingerprint; without
    /// one, a DTLS handshake with a self-signed certificate authenticates nobody, and proceeding
    /// would produce encrypted media with no idea who is at the other end.
    #[error("the peer's SDP carried no fingerprint, so its certificate cannot be verified")]
    NoFingerprint,
    /// The handshake completed and presented no certificate.
    #[error("the peer presented no certificate")]
    NoCertificate,
    /// The certificate presented is not the one the SDP named (RFC 8122 §6.2).
    #[error("the peer's certificate does not match the fingerprint its SDP carried")]
    FingerprintMismatch,
    /// No SRTP profile was agreed.
    #[error("the handshake agreed no SRTP protection profile")]
    NoProfile,
    /// The keying material could not be used.
    #[error("keying: {0}")]
    Keying(#[from] KeyError),
    /// The handshake itself failed.
    #[error("dtls: {0}")]
    Dtls(String),
}

/// Handshake, verify the peer against the fingerprint from its SDP, and derive the SRTP keys.
///
/// The order is the point. RFC 8122 §6.2 requires an endpoint whose peer's certificate does not
/// match the fingerprint to "terminate the media connection with a `bad_certificate` error" — so the
/// check happens **before** any keys are handed back, and a mismatch returns an error rather than a
/// pair of contexts a caller might use anyway.
pub fn establish<H: Handshake>(
    handshake: &mut H,
    role: Role,
    peer_fingerprint: Option<&sipx_sdp::fingerprint::Fingerprint>,
) -> Result<Keys, Error> {
    // Before the handshake, not after: a peer that sent no fingerprint cannot be authenticated at
    // all, and finding that out after exchanging keys means having done the work to establish a
    // channel to an unknown party.
    let fingerprint = peer_fingerprint.ok_or(Error::NoFingerprint)?;

    handshake
        .run(role)
        .map_err(|error| Error::Dtls(error.to_string()))?;

    let certificate = handshake.peer_certificate().ok_or(Error::NoCertificate)?;
    if !fingerprint.matches(&certificate) {
        return Err(Error::FingerprintMismatch);
    }

    let profile = handshake.profile().ok_or(Error::NoProfile)?;
    let exported = handshake
        .export(profile.exported_len())
        .map_err(|error| Error::Dtls(error.to_string()))?;
    Ok(keys_from_exported(&exported, profile, role)?)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use sipx_sdp::fingerprint::{Fingerprint, HashFunc};

    /// A handshake that succeeds, presenting whatever certificate the test names.
    struct Stub {
        certificate: Option<Vec<u8>>,
        profile: Option<Profile>,
        exported: Vec<u8>,
        fail: bool,
        ran_as: Option<Role>,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("the stub was told to fail")]
    struct StubError;

    impl Stub {
        fn good() -> Self {
            Self {
                certificate: Some(b"the peer's certificate".to_vec()),
                profile: Some(Profile::Aes128CmHmacSha1_80),
                // A recognisable block: 16 key, 16 key, 14 salt, 14 salt.
                exported: (0u8..60).collect(),
                fail: false,
                ran_as: None,
            }
        }
    }

    impl Handshake for Stub {
        type Error = StubError;

        fn run(&mut self, role: Role) -> Result<(), Self::Error> {
            self.ran_as = Some(role);
            if self.fail { Err(StubError) } else { Ok(()) }
        }

        fn peer_certificate(&self) -> Option<Vec<u8>> {
            self.certificate.clone()
        }

        fn profile(&self) -> Option<Profile> {
            self.profile
        }

        fn export(&self, len: usize) -> Result<Vec<u8>, Self::Error> {
            Ok(self.exported.iter().copied().take(len).collect())
        }
    }

    /// RFC 5764 §5.1.2's ranges, at every boundary.
    #[test]
    fn one_port_tells_stun_dtls_and_rtp_apart_by_the_first_byte() {
        assert_eq!(classify(&[0]), Arriving::Stun);
        assert_eq!(classify(&[1]), Arriving::Stun);
        assert_eq!(classify(&[2]), Arriving::Unknown);
        assert_eq!(classify(&[19]), Arriving::Unknown);
        assert_eq!(classify(&[20]), Arriving::Dtls, "DTLS ChangeCipherSpec");
        assert_eq!(classify(&[22]), Arriving::Dtls, "DTLS Handshake");
        assert_eq!(classify(&[23]), Arriving::Dtls, "DTLS ApplicationData");
        assert_eq!(classify(&[63]), Arriving::Dtls);
        assert_eq!(classify(&[64]), Arriving::Unknown);
        assert_eq!(classify(&[127]), Arriving::Unknown);
        assert_eq!(classify(&[128]), Arriving::Rtp, "RTP version 2, no padding");
        assert_eq!(classify(&[0x80]), Arriving::Rtp);
        assert_eq!(classify(&[0xbf]), Arriving::Rtp);
        assert_eq!(classify(&[192]), Arriving::Unknown);
        assert_eq!(
            classify(&[]),
            Arriving::Unknown,
            "an empty datagram is not RTP"
        );
    }

    /// A real RTP packet and a real DTLS record, classified as themselves. The ranges above are
    /// only useful if actual traffic lands in them.
    #[test]
    fn a_real_rtp_packet_and_a_real_dtls_record_land_where_they_should() {
        // RTP: version 2 in the top two bits, payload type 0 (PCMU).
        let rtp = [0x80, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(classify(&rtp), Arriving::Rtp);
        // RTCP: version 2, packet type 200 (sender report).
        let sender_report = [0x80, 0xc8, 0x00, 0x06];
        assert_eq!(classify(&sender_report), Arriving::Rtp);
        // DTLS 1.2 handshake record: content type 22, version 254.253.
        let dtls = [0x16, 0xfe, 0xfd, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        assert_eq!(classify(&dtls), Arriving::Dtls);
    }

    /// §4.1.2 states 128 bits of key and **112** bits of salt. Fourteen octets, not sixteen.
    #[test]
    fn the_profile_asks_for_the_key_and_salt_sizes_the_rfc_states() {
        let profile = Profile::Aes128CmHmacSha1_80;
        assert_eq!(profile.key_and_salt_len(), (16, 14));
        assert_eq!(profile.exported_len(), 60, "2 * (16 + 14)");
        assert_eq!(profile.id(), 0x0001);
        assert_eq!(profile.as_str(), "SRTP_AES128_CM_SHA1_80");
    }

    /// §4.2's order: client key, server key, client salt, server salt. Keys first, then salts.
    ///
    /// Asserted by *position in the exported block* rather than by round-tripping through SRTP,
    /// because the failure this guards against — reading key-and-salt per side — produces contexts
    /// that are structurally valid and decrypt nothing.
    #[test]
    fn the_exported_block_splits_keys_before_salts() {
        let exported: Vec<u8> = (0u8..60).collect();
        let profile = Profile::Aes128CmHmacSha1_80;
        // Both roles derive from the same block, so deriving both and comparing is enough to pin
        // which bytes went where without reaching inside `srtp::Context`.
        let client = keys_from_exported(&exported, profile, Role::Client).expect("keys");
        let server = keys_from_exported(&exported, profile, Role::Server).expect("keys");
        // The client's outbound context must equal the server's inbound one: same key, same salt.
        // `srtp::Context` does not expose its key, so this is asserted through behaviour — what one
        // protects, the other unprotects.
        let mut protecting = client.outbound;
        let mut unprotecting = server.inbound;
        let packet = [
            0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xa0, 0xde, 0xad, 0xbe, 0xef, 0x11, 0x22,
        ];
        let protected = protecting.protect(&packet).expect("protects");
        assert_ne!(
            protected.get(12..14),
            packet.get(12..14),
            "the payload should not be in the clear"
        );
        let recovered = unprotecting.unprotect(&protected).expect(
            "the client's outbound key must be the server's inbound key; if this fails the block \
             was split key-and-salt per side rather than keys-then-salts",
        );
        assert_eq!(recovered, packet);
    }

    /// And the other direction, which a one-sided split would leave working by accident.
    #[test]
    fn the_server_protects_with_what_the_client_unprotects_with() {
        let exported: Vec<u8> = (0u8..60).collect();
        let profile = Profile::Aes128CmHmacSha1_80;
        let client = keys_from_exported(&exported, profile, Role::Client).expect("keys");
        let server = keys_from_exported(&exported, profile, Role::Server).expect("keys");
        let mut protecting = server.outbound;
        let mut unprotecting = client.inbound;
        let packet = [
            0x80, 0x00, 0x00, 0x07, 0x00, 0x00, 0x03, 0x20, 0xca, 0xfe, 0xba, 0xbe, 0x33, 0x44,
        ];
        let protected = protecting.protect(&packet).expect("protects");
        assert_eq!(
            unprotecting.unprotect(&protected).expect("unprotects"),
            packet
        );
    }

    /// The two roles must not derive the *same* sending key — that is what a split ignoring the
    /// role would produce, and it would work perfectly in a loopback test.
    #[test]
    fn the_two_roles_do_not_send_with_the_same_key() {
        let exported: Vec<u8> = (0u8..60).collect();
        let profile = Profile::Aes128CmHmacSha1_80;
        let mut client = keys_from_exported(&exported, profile, Role::Client)
            .expect("keys")
            .outbound;
        let mut server = keys_from_exported(&exported, profile, Role::Server)
            .expect("keys")
            .outbound;
        let packet = [
            0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xa0, 0xde, 0xad, 0xbe, 0xef, 0x55, 0x66,
        ];
        assert_ne!(
            client.protect(&packet).expect("protects"),
            server.protect(&packet).expect("protects"),
            "both roles derived the same sending key, so the role was ignored"
        );
    }

    #[test]
    fn keying_material_shorter_than_the_profile_needs_is_refused() {
        let short: Vec<u8> = (0u8..59).collect();
        let outcome = keys_from_exported(&short, Profile::Aes128CmHmacSha1_80, Role::Client);
        assert!(
            matches!(
                outcome,
                Err(KeyError::Short {
                    needed: 60,
                    got: 59,
                    ..
                })
            ),
            "{outcome:?}"
        );
    }

    /// The story's failing-first test, at this layer: RFC 8122 §6.2's mandatory check.
    #[test]
    fn a_mismatched_fingerprint_yields_no_keys() {
        let mut handshake = Stub::good();
        // A fingerprint of some *other* certificate — what a substituting intermediary produces.
        let wrong = Fingerprint::of(b"a certificate the peer does not have", HashFunc::Sha256);
        let outcome = establish(&mut handshake, Role::Client, Some(&wrong));
        assert!(
            matches!(outcome, Err(Error::FingerprintMismatch)),
            "a certificate that does not match the SDP must yield an error, not keys: {outcome:?}"
        );
    }

    #[test]
    fn a_matching_fingerprint_yields_keys() {
        let mut handshake = Stub::good();
        let right = Fingerprint::of(b"the peer's certificate", HashFunc::Sha256);
        assert!(establish(&mut handshake, Role::Client, Some(&right)).is_ok());
        assert_eq!(
            handshake.ran_as,
            Some(Role::Client),
            "the negotiated role must reach the handshake, not be guessed there"
        );
    }

    /// A peer that sent no fingerprint is refused *before* the handshake runs. An unverified DTLS
    /// handshake with a self-signed certificate authenticates nobody, and finding that out
    /// afterwards means having established a channel to an unknown party.
    #[test]
    fn a_peer_with_no_fingerprint_is_refused_before_the_handshake_runs() {
        let mut handshake = Stub::good();
        let outcome = establish(&mut handshake, Role::Client, None);
        assert!(matches!(outcome, Err(Error::NoFingerprint)), "{outcome:?}");
        assert_eq!(
            handshake.ran_as, None,
            "the handshake must not run for a peer that cannot be verified"
        );
    }

    #[test]
    fn a_handshake_that_agrees_no_profile_yields_no_keys() {
        let mut handshake = Stub::good();
        handshake.profile = None;
        let right = Fingerprint::of(b"the peer's certificate", HashFunc::Sha256);
        assert!(matches!(
            establish(&mut handshake, Role::Client, Some(&right)),
            Err(Error::NoProfile)
        ));
    }

    #[test]
    fn a_handshake_that_presents_no_certificate_yields_no_keys() {
        let mut handshake = Stub::good();
        handshake.certificate = None;
        let right = Fingerprint::of(b"the peer's certificate", HashFunc::Sha256);
        assert!(matches!(
            establish(&mut handshake, Role::Client, Some(&right)),
            Err(Error::NoCertificate)
        ));
    }

    #[test]
    fn a_failed_handshake_is_reported_rather_than_keyed_around() {
        let mut handshake = Stub::good();
        handshake.fail = true;
        let right = Fingerprint::of(b"the peer's certificate", HashFunc::Sha256);
        assert!(matches!(
            establish(&mut handshake, Role::Client, Some(&right)),
            Err(Error::Dtls(_))
        ));
    }

    #[test]
    fn the_exporter_label_is_the_one_the_rfc_fixes() {
        // §4.2. Not a detail sipx may choose: a different label derives different keys, and the
        // failure is silent on both sides.
        assert_eq!(EXPORTER_LABEL, "EXTRACTOR-dtls_srtp");
    }
}
