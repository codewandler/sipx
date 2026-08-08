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
//!
//! **Supported**: `sipx-call` now selects this protocol, key-derivation and handshake surface for
//! explicit DTLS-SRTP call policy (`M-28`), so an upper-layer caller has constrained its shape. The
//! optional `dtls::openssl` implementation remains experimental; enabling that feature only
//! makes the explicit selection available and never changes a call's default.
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

/// An SRTP protection profile (RFC 5764 §4.1.2, RFC 7714 §14.2).
///
/// Only the ones sipx can perform. RFC 5764 §4.1.2 registers four and RFC 7714 §14.2 adds two;
/// the two `NULL` profiles encrypt nothing, and offering `AES128_CM_HMAC_SHA1_32` would mean an
/// SRTP transform with a 32-bit tag that [`sipx_rtp::srtp`] does not implement. A profile list is
/// a promise, so it holds exactly what [`sipx_rtp::srtp::Profile`] can key — and it is that type
/// this maps onto, so the DTLS name and the transform cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Profile {
    /// AES-128 counter mode, 80-bit HMAC-SHA1 tag — the same transform SDES negotiates.
    ///
    /// RFC 5764 §4.1.2 makes it mandatory to implement, so it is always in the offered list.
    Aes128CmHmacSha1_80,
    /// RFC 7714's `AEAD_AES_128_GCM`, registered for DTLS-SRTP by §14.2.
    AeadAes128Gcm,
    /// RFC 7714's `AEAD_AES_256_GCM`, registered for DTLS-SRTP by §14.2.
    AeadAes256Gcm,
}

impl Profile {
    /// The `use_srtp` profile list this endpoint offers, **strongest first** (RFC 5764 §4.1.1).
    ///
    /// §4.1.1 has the client send its profiles "in preference order", and the server picks. The
    /// order is by strength for the reason `sipx_sdp::crypto` gives for the SDES list: the
    /// alternative is letting whatever ordering arrives decide the cipher.
    ///
    /// The list is derived from [`sipx_rtp::srtp::Profile::STRONGEST_FIRST`] rather than written
    /// out, so the two keying paths cannot come to offer different transforms.
    #[must_use]
    pub fn strongest_first() -> Vec<Self> {
        srtp::Profile::STRONGEST_FIRST
            .into_iter()
            .filter_map(Self::for_transform)
            .collect()
    }

    /// The transform this profile names.
    ///
    /// The single point where a DTLS-SRTP profile becomes a cipher. Keeping it one function is
    /// what makes "never install a different cipher under a negotiated identifier"
    /// (`docs/designs/media-runtime-safety.md`) checkable rather than a habit.
    #[must_use]
    pub fn transform(self) -> srtp::Profile {
        match self {
            Self::Aes128CmHmacSha1_80 => srtp::Profile::AesCm128HmacSha1_80,
            Self::AeadAes128Gcm => srtp::Profile::AeadAes128Gcm,
            Self::AeadAes256Gcm => srtp::Profile::AeadAes256Gcm,
        }
    }

    /// The DTLS-SRTP profile that names a transform, if one does.
    #[must_use]
    pub fn for_transform(transform: srtp::Profile) -> Option<Self> {
        [
            Self::Aes128CmHmacSha1_80,
            Self::AeadAes128Gcm,
            Self::AeadAes256Gcm,
        ]
        .into_iter()
        .find(|profile| profile.transform() == transform)
    }

    /// The name as the IANA *DTLS-SRTP Protection Profiles* registry spells it.
    ///
    /// One spelling, from one source: RFC 5764 §4.1.2 for `0x0001` and RFC 7714 §14.2 for the two
    /// AEAD identifiers, which is what the registry carries for all three. A DTLS library's own
    /// option syntax is a different namespace and need not agree — one does not, for `0x0001` —
    /// and reconciling it is that library's boundary's job (the `openssl` module here), not this
    /// type's. The alternative is what `docs/specs/srtp.md` §12.4 recorded: a name an implementor
    /// of [`Handshake`] cannot look up, because it belongs to a library they may not be using.
    ///
    /// Nothing here reaches the wire. RFC 5764 §4.1.1 carries [`Self::id`]'s two octets; the name
    /// exists for APIs and for diagnostics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aes128CmHmacSha1_80 => "SRTP_AES128_CM_HMAC_SHA1_80",
            Self::AeadAes128Gcm => "SRTP_AEAD_AES_128_GCM",
            Self::AeadAes256Gcm => "SRTP_AEAD_AES_256_GCM",
        }
    }

    /// The profile a registered `use_srtp` name selects, if it is one sipx can perform.
    ///
    /// Registered names only, and [`Self::as_str`]'s spelling exactly: a library's own name for a
    /// profile is `None` here. That refusal is the safe direction — it surfaces as
    /// `openssl::DtlsError::NoProfile` and keys nothing, where a lenient match would be one more
    /// place a name and a transform could be paired by guesswork.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::strongest_first()
            .into_iter()
            .find(|profile| profile.as_str() == name)
    }

    /// The two-byte value carried in the `use_srtp` extension (RFC 5764 §4.1.2, RFC 7714 §14.2).
    #[must_use]
    pub fn id(self) -> u16 {
        match self {
            Self::Aes128CmHmacSha1_80 => 0x0001,
            Self::AeadAes128Gcm => 0x0007,
            Self::AeadAes256Gcm => 0x0008,
        }
    }

    /// Master key and master salt lengths, in octets.
    ///
    /// Read off the transform rather than restated, because restating them is how the two come to
    /// disagree. RFC 5764 §4.1.2 states the counter-mode pair in bits — a 128-bit key and a
    /// **112**-bit salt, so fourteen octets and not sixteen — and RFC 7714 §14.2 states the AEAD
    /// ones as a 128- or 256-bit key with a **96**-bit salt. Getting a salt length wrong produces
    /// a key schedule that decrypts nothing with no error to say why.
    #[must_use]
    pub fn key_and_salt_len(self) -> (usize, usize) {
        self.transform().key_and_salt_len()
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
    material: crate::SrtpKeys,
}

/// DTLS-SRTP keys whose peer certificate and protection profile were verified.
///
/// Unlike [`Keys`], this value cannot be constructed from raw exporter bytes. The browser-audio
/// component accepts this type at its key-installation boundary so a caller cannot accidentally
/// advance media after key derivation while skipping RFC 8122's fingerprint check.
pub struct VerifiedKeys(Keys);

impl std::fmt::Debug for VerifiedKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedKeys { .. }")
    }
}

impl VerifiedKeys {
    pub(crate) fn into_srtp_keys(self) -> crate::SrtpKeys {
        self.0.into_srtp_keys()
    }
}

impl Keys {
    /// Move the same directional master key and salt pairs into a live media session.
    ///
    /// The contexts above exist for users that apply SRTP themselves. A [`crate::MediaSession`]
    /// constructs separate RTP and RTCP contexts, so it needs the master material instead; keeping
    /// it here closes that boundary without trying to recover secrets from an opaque context.
    #[must_use]
    pub fn into_srtp_keys(self) -> crate::SrtpKeys {
        self.material
    }
}

/// Why keying material could not be turned into SRTP contexts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
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
    // The profile travels with the material rather than being inferred downstream from how many
    // octets arrived. Two profiles can agree on a key length and disagree on the transform, and
    // a session that guessed would install a cipher the handshake never agreed to.
    let transform = profile.transform();
    let material = crate::SrtpKeys {
        profile: transform,
        local: (own_key.to_vec(), own_salt.to_vec()),
        remote: (peer_key.to_vec(), peer_salt.to_vec()),
    };
    Ok(Keys {
        outbound: srtp::Context::new(transform, own_key, own_salt)?,
        inbound: srtp::Context::new(transform, peer_key, peer_salt)?,
        material,
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
#[non_exhaustive]
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

/// Handshake, verify and return key material carrying proof that verification ran.
///
/// This has the same protocol behavior and errors as [`establish`]. Its distinct return type is
/// for composition boundaries that must make skipping fingerprint verification unrepresentable.
pub fn establish_verified<H: Handshake>(
    handshake: &mut H,
    role: Role,
    peer_fingerprint: Option<&sipx_sdp::fingerprint::Fingerprint>,
) -> Result<VerifiedKeys, Error> {
    establish(handshake, role, peer_fingerprint).map(VerifiedKeys)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
        assert_eq!(profile.as_str(), "SRTP_AES128_CM_HMAC_SHA1_80");
    }

    /// The counter-mode profile is spelled the way the registry spells it (RFC 5764 §4.1.2).
    ///
    /// IANA's *DTLS-SRTP Protection Profiles* registry and RFC 5764 §4.1.2 both spell `0x0001`
    /// `SRTP_AES128_CM_HMAC_SHA1_80` — with the `HMAC_`. A DTLS library's own list syntax may use
    /// a shorter spelling, and one does; that is a name for its API and not for this registry, so
    /// translating it belongs at that library's boundary rather than here. This assertion is what
    /// stops the library's spelling leaking back into the type an implementor of [`Handshake`]
    /// reads (`docs/specs/srtp.md` §12.4).
    ///
    /// The identifier is asserted beside it because that, not the name, is what §4.1.1 puts on the
    /// wire: the rename must not move it.
    #[test]
    fn the_counter_mode_profile_is_spelled_as_the_registry_spells_it() {
        assert_eq!(
            Profile::Aes128CmHmacSha1_80.as_str(),
            "SRTP_AES128_CM_HMAC_SHA1_80"
        );
        assert_eq!(
            Profile::parse("SRTP_AES128_CM_HMAC_SHA1_80"),
            Some(Profile::Aes128CmHmacSha1_80)
        );
        assert_eq!(Profile::Aes128CmHmacSha1_80.id(), 0x0001);
    }

    /// RFC 7714 §14.2's registrations, spelled and numbered as IANA has them.
    ///
    /// The identifiers are the two octets that go on the wire; a transposed pair would agree on
    /// nothing with a conformant peer and would look like a plain handshake failure.
    #[test]
    fn the_aead_profiles_carry_the_names_and_ids_rfc_7714_registers() {
        assert_eq!(Profile::AeadAes128Gcm.as_str(), "SRTP_AEAD_AES_128_GCM");
        assert_eq!(Profile::AeadAes128Gcm.id(), 0x0007);
        assert_eq!(Profile::AeadAes128Gcm.key_and_salt_len(), (16, 12));
        assert_eq!(Profile::AeadAes128Gcm.exported_len(), 56, "2 * (16 + 12)");

        assert_eq!(Profile::AeadAes256Gcm.as_str(), "SRTP_AEAD_AES_256_GCM");
        assert_eq!(Profile::AeadAes256Gcm.id(), 0x0008);
        assert_eq!(Profile::AeadAes256Gcm.key_and_salt_len(), (32, 12));
        assert_eq!(Profile::AeadAes256Gcm.exported_len(), 88, "2 * (32 + 12)");

        for profile in Profile::strongest_first() {
            assert_eq!(Profile::parse(profile.as_str()), Some(profile));
        }
        assert_eq!(
            Profile::parse("SRTP_AES128_CM_SHA1_32"),
            None,
            "not keyable"
        );
    }

    /// The offered list is strongest first **and** keeps RFC 5764 §4.1.2's mandatory profile.
    ///
    /// An endpoint that dropped `SRTP_AES128_CM_HMAC_SHA1_80` to look modern would fail to key with
    /// every peer that implements only what the RFC requires — which is most of them.
    #[test]
    fn the_offered_profile_list_is_strongest_first_and_keeps_the_floor() {
        assert_eq!(
            Profile::strongest_first(),
            vec![
                Profile::AeadAes256Gcm,
                Profile::AeadAes128Gcm,
                Profile::Aes128CmHmacSha1_80,
            ]
        );
        assert!(
            Profile::strongest_first().contains(&Profile::Aes128CmHmacSha1_80),
            "RFC 5764 §4.1.2 makes it mandatory to implement"
        );
    }

    /// Every offered profile names a transform `sipx-rtp` can key, and no two name the same one.
    ///
    /// A profile list is a promise. This is the promise checked rather than asserted: a name
    /// offered on the wire that maps to nothing would be a handshake this side agreed to and then
    /// could not honour, which is worse than not offering it.
    #[test]
    fn every_offered_profile_maps_to_a_transform_that_can_be_keyed() {
        let mut transforms = Vec::new();
        for profile in Profile::strongest_first() {
            let transform = profile.transform();
            assert_eq!(
                Profile::for_transform(transform),
                Some(profile),
                "{profile:?} does not round-trip through its transform"
            );
            assert_eq!(
                transform.key_and_salt_len(),
                profile.key_and_salt_len(),
                "{profile:?} states different lengths from the transform it names"
            );
            let (key_len, salt_len) = profile.key_and_salt_len();
            srtp::Context::new(transform, &vec![0u8; key_len], &vec![0u8; salt_len])
                .unwrap_or_else(|error| panic!("{profile:?} cannot be keyed: {error}"));
            assert!(
                !transforms.contains(&transform),
                "{profile:?} is a duplicate"
            );
            transforms.push(transform);
        }
    }

    /// The exported block splits and keys correctly under **every** profile, not only the one
    /// the split was written for. §4.2's order is the same; the lengths are not.
    #[test]
    fn the_exported_block_keys_every_offered_profile() {
        for profile in Profile::strongest_first() {
            let exported: Vec<u8> = (0..profile.exported_len())
                .map(|n| u8::try_from(n % 251).unwrap_or(0))
                .collect();
            let client = keys_from_exported(&exported, profile, Role::Client).expect("keys");
            let server = keys_from_exported(&exported, profile, Role::Server).expect("keys");
            assert_eq!(
                client.material.profile,
                profile.transform(),
                "{profile:?}: the negotiated transform must travel with the keys"
            );

            let packet = [
                0x80, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xa0, 0xde, 0xad, 0xbe, 0xef, 0x11, 0x22,
            ];
            let mut protecting = client.outbound;
            let mut unprotecting = server.inbound;
            let protected = protecting.protect(&packet).expect("protects");
            assert_eq!(
                unprotecting.unprotect(&protected).expect("unprotects"),
                packet,
                "{profile:?}: the client's outbound key must be the server's inbound key"
            );
        }
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
