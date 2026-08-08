//! SRTP (RFC 3711, RFC 7714): the counter-mode transform and the two AEAD ones.
//!
//! What SRTP protects and what it does not is worth being exact about, because the gap is where
//! people are surprised. It encrypts the *payload* and authenticates the *whole packet*
//! including the header. So the sequence number, timestamp and SSRC travel in the clear and
//! cannot be altered; the audio travels encrypted. That is deliberate — a relay has to read the
//! header to do its job.
//!
//! Which transform does that is a [`Profile`], and it is an argument rather than an assumption.
//! Three are implemented: RFC 3711's `AES_CM_128_HMAC_SHA1_80`, which RFC 5764 §4.1.2 makes the
//! mandatory interoperability floor, and RFC 7714's `AEAD_AES_128_GCM` and `AEAD_AES_256_GCM`.
//! Every length that differs between them — master key, master salt, authentication tag — is
//! **read off the profile**, and [`Context::new`] refuses a key or salt that is not the length the
//! named profile requires. That refusal is the point: a second set of implicit per-profile
//! constants is how the wrong cipher gets installed under the right negotiated name.
//!
//! Three things here are easy to get wrong and each has a test against the RFC's own published
//! numbers rather than against this implementation's opinion of them:
//!
//! **Key derivation** (§4.3.1) turns one master key into session keys through AES counter mode.
//! Getting the label or the salt alignment wrong produces keys that are perfectly self-consistent
//! — two endpoints running the same wrong code interoperate happily and neither interoperates
//! with anything else. RFC 7714 §11 keeps this KDF for the AEAD profiles and only changes the
//! cipher it is built on: AES-128 for `AEAD_AES_128_GCM`, and RFC 6188's AES-256 for
//! `AEAD_AES_256_GCM`.
//!
//! **The packet index** (§3.3.1) is 48 bits: a 32-bit rollover counter above the 16-bit sequence
//! number. It is not sent. Both ends infer it, and an implementation that guesses differently
//! decrypts to noise at the first wrap — twenty minutes into a call, at speech packet rates. For
//! AEAD it is worse than noise: RFC 7714 §8.1 puts the index straight into the IV, and §8.4 says
//! plainly that reusing one "compromises the authentication mechanism".
//!
//! **Replay** (§3.3.2) is rejected by a sliding window rather than by remembering everything.
//! Without it, a captured packet can be replayed into a call for as long as the key lives. The
//! window, the rollover inference and the rule that nothing touches this context's state until
//! the packet has authenticated are profile-independent, and the tests below run them over every
//! profile rather than over the one they were written for.

/// RFC 7714 §16 and §17's published vectors, read out of the imported corpus.
///
/// A sibling module rather than a `tests/` integration test because §16 publishes *session* keys
/// and no master key, so reaching the RFC's numbers means stepping over key derivation — which is
/// a thing a test may do and a public API may not.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod rfc7714_vectors;

use aes::cipher::{KeyIvInit, StreamCipher};
use aes::{Aes128, Aes256};
use aes_gcm::aead::{AeadInPlace, KeyInit as AeadKeyInit};
use aes_gcm::{Aes128Gcm, Aes256Gcm};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

type Aes128Ctr = ctr::Ctr128BE<Aes128>;
type Aes256Ctr = ctr::Ctr128BE<Aes256>;
type HmacSha1 = Hmac<Sha1>;

/// The master key length of `AES_CM_128_HMAC_SHA1_80`.
///
/// Prefer [`Profile::master_key_len`]: this constant describes one profile, and the transform in
/// force is a negotiated value.
pub const MASTER_KEY_LEN: usize = 16;
/// The master salt length of `AES_CM_128_HMAC_SHA1_80`.
///
/// Prefer [`Profile::master_salt_len`], for the reason given on [`MASTER_KEY_LEN`].
pub const MASTER_SALT_LEN: usize = 14;
/// The authentication tag length of `AES_CM_128_HMAC_SHA1_80`, in octets.
///
/// Prefer [`Profile::tag_len`]: RFC 7714 §13.2 fixes the AEAD tag at **16** octets and refuses to
/// allow it to be truncated, so this number is not the SRTP tag length in general.
pub const TAG_LEN: usize = 10;

/// The longest tag any implemented profile produces, in octets.
///
/// The AEAD tag RFC 7714 §13.2 fixes, which is longer than RFC 3711's 80-bit one. Exported so a
/// caller sizing a buffer for "an SRTP packet of at most N payload octets" has one number to add
/// rather than a table to consult.
pub const MAX_TAG_LEN: usize = 16;

const SESSION_SALT_LEN: usize = 14;
/// `n_a`, the session authentication key length: 160 bits (RFC 3711 §5.2, §8.2).
///
/// §4.3.1 derives `n = n_a` octets under label 0x01 and fixes no length of its own; §5.2 fixes
/// `n_a` at 160 bits for the pre-defined HMAC-SHA1 transform, and §8.2's table lists it as both
/// mandatory-to-support and the default. §B.3's worked example derives **94** octets because that
/// appendix posits an authentication function needing 94, in order to walk the PRF through six AES
/// blocks — a property of the example, not of the transform. HMAC accepts a key of any length,
/// which is what lets the two be confused without any error to say so.
///
/// AEAD has no counterpart: RFC 7714 §7.1 makes the AEAD tag "the primary message authentication
/// mechanism", so labels 0x01 and 0x04 derive nothing at all under those profiles.
const SESSION_AUTH_LEN: usize = 20;

/// The AEAD initialization vector length: 12 octets (RFC 7714 §10, `N_MIN` = `N_MAX`).
const AEAD_IV_LEN: usize = 12;

/// An SRTP protection profile — the transform in force, as a value rather than an assumption.
///
/// Every length that differs between the three is derived from this type, and nothing else in
/// this module carries a per-profile constant. The negotiation-truth rule
/// (`docs/designs/media-runtime-safety.md`) is why: a cipher installed under a name that means a
/// different cipher is a stream both ends believe is protected by something it is not.
///
/// Ordering is by [`Profile::strength`] and is what the keying paths rank on. It is deliberately
/// not `Ord`: "stronger" and "later in the enum" are the same thing only by accident today, and a
/// derived comparison would keep agreeing after they stop being.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Profile {
    /// RFC 3711's default transform: AES-128 counter mode with an 80-bit HMAC-SHA1 tag.
    ///
    /// RFC 5764 §4.1.2 makes it the mandatory-to-implement profile, so it stays offered however
    /// good the alternatives are — an endpoint that dropped it would fail to call most of the
    /// telephone network.
    AesCm128HmacSha1_80,
    /// RFC 7714's `AEAD_AES_128_GCM`: AES-128 in Galois/Counter Mode with a 128-bit tag.
    AeadAes128Gcm,
    /// RFC 7714's `AEAD_AES_256_GCM`: AES-256 in Galois/Counter Mode with a 128-bit tag.
    AeadAes256Gcm,
}

impl Profile {
    /// Every implemented profile, **strongest first**.
    ///
    /// One list, so the two keying paths cannot drift into offering different sets — RFC 4568
    /// SDES and RFC 5764 DTLS-SRTP name the same transforms differently, and a peer's reachable
    /// cipher should not depend on how it happened to key.
    pub const STRONGEST_FIRST: [Self; 3] = [
        Self::AeadAes256Gcm,
        Self::AeadAes128Gcm,
        Self::AesCm128HmacSha1_80,
    ];

    /// How strong this profile is, for ranking offers. Higher is stronger.
    ///
    /// Only the order means anything; the numbers are ranks and not a measure of anything. The
    /// two AEAD profiles outrank counter-mode-plus-HMAC because the tag and the ciphertext come
    /// from one construction, which removes the encrypt-then-MAC ordering question rather than
    /// answering it, and 256 outranks 128 on key size alone.
    #[must_use]
    pub fn strength(self) -> u8 {
        match self {
            Self::AesCm128HmacSha1_80 => 1,
            Self::AeadAes128Gcm => 2,
            Self::AeadAes256Gcm => 3,
        }
    }

    /// The master key length in octets (RFC 3711 §8.2; RFC 7714 §12, Tables 2 and 3).
    #[must_use]
    pub fn master_key_len(self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 | Self::AeadAes128Gcm => 16,
            Self::AeadAes256Gcm => 32,
        }
    }

    /// The master salt length in octets.
    ///
    /// **Fourteen for counter mode and twelve for AEAD**, which is the easiest number here to get
    /// wrong: RFC 3711 §8.2 fixes 112 bits and RFC 7714 §12 fixes 96. A salt of the other length
    /// produces a key schedule that decrypts nothing, with no error anywhere to say why.
    #[must_use]
    pub fn master_salt_len(self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 => 14,
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => AEAD_IV_LEN,
        }
    }

    /// Master key and master salt lengths together, in octets — the shape both keying paths want.
    #[must_use]
    pub fn key_and_salt_len(self) -> (usize, usize) {
        (self.master_key_len(), self.master_salt_len())
    }

    /// The authentication tag this profile appends, in octets.
    ///
    /// RFC 3711 §5.2's `n_tag` is 80 bits; RFC 7714 §13.2 fixes the AEAD tag at 16 octets and
    /// refuses truncation outright — "the risks associated with using truncated AES-GCM tags are
    /// deemed too high".
    #[must_use]
    pub fn tag_len(self) -> usize {
        match self {
            Self::AesCm128HmacSha1_80 => TAG_LEN,
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => MAX_TAG_LEN,
        }
    }

    /// Whether the transform is an AEAD one, so the tag and the ciphertext are one construction.
    #[must_use]
    pub fn is_aead(self) -> bool {
        match self {
            Self::AesCm128HmacSha1_80 => false,
            Self::AeadAes128Gcm | Self::AeadAes256Gcm => true,
        }
    }
}

/// Which session key is being derived (RFC 3711 §4.3.1).
#[derive(Debug, Clone, Copy)]
enum Label {
    RtpEncryption = 0x00,
    RtpAuthentication = 0x01,
    RtpSalt = 0x02,
    RtcpEncryption = 0x03,
    RtcpAuthentication = 0x04,
    RtcpSalt = 0x05,
}

/// What can go wrong protecting or unprotecting a packet.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SrtpError {
    /// The key or salt was not the length the transform requires.
    #[error("{what} must be {expected} octets, not {actual}")]
    KeyLength {
        /// Which one.
        what: &'static str,
        /// How long it should be.
        expected: usize,
        /// How long it was.
        actual: usize,
    },
    /// Too short to be a packet of this kind at all.
    #[error("packet is {0} octets; too short to be authenticated")]
    TooShort(usize),
    /// The authentication tag did not match.
    ///
    /// Deliberately says nothing about *why*. A caller that could tell "wrong key" from
    /// "altered packet" would be an oracle.
    #[error("authentication failed")]
    NotAuthentic,
    /// The packet has been seen before, or is too old to judge (RFC 3711 §3.3.2).
    #[error("replayed or too old: sequence {0}")]
    Replayed(u16),
    /// The SRTCP packet has been seen before, or its 31-bit index is too old to judge.
    #[error("replayed or too old SRTCP index {0}")]
    ReplayedRtcp(u32),
}

/// The session keys one master key produces.
///
/// `Vec` rather than fixed arrays because the lengths belong to the profile: 16 or 32 octets of
/// key, 14 or 12 of salt. The authentication halves are **empty under an AEAD profile** — RFC 7714
/// §7.1 makes the AEAD tag the primary message authentication mechanism, so labels 0x01 and 0x04
/// derive nothing at all and a key that is never used is a key that cannot leak.
#[derive(Clone)]
struct Session {
    rtp_key: Vec<u8>,
    rtp_salt: Vec<u8>,
    rtp_auth: Vec<u8>,
    rtcp_key: Vec<u8>,
    rtcp_salt: Vec<u8>,
    rtcp_auth: Vec<u8>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keys. Printing them would put them in whatever log the caller writes.
        f.write_str("Session { .. }")
    }
}

/// Derive `out.len()` octets of session key material (RFC 3711 §4.3.1, §4.3.3).
///
/// The input block is the master salt with the label exclusive-ored into octet 7 and
/// `index DIV kdr` into octets 8..14, shifted left by two octets. sipx uses a key derivation
/// rate of zero — one derivation per master key, which is what `index DIV kdr` being zero means
/// — because rekeying mid-stream buys nothing until there is a way to signal it.
///
/// **The cipher under the PRF follows the master key length**, which is what RFC 7714 §11 asks
/// for: `AEAD_AES_128_GCM` "MUST use the (128-bit) `AES_CM` PRF KDF described in [RFC3711]", and
/// `AEAD_AES_256_GCM` "MUST use the `AES_256_CM_PRF` KDF described in [RFC6188]". RFC 6188 §2 makes
/// that the same construction with AES-256 in place of AES-128, so the only thing that changes
/// here is which key schedule the counter runs through.
///
/// **The 96-bit master salt is left-aligned in the 128-bit input block**, with the label still at
/// octet 7. RFC 3711 §4.3.1 words the label placement as right-alignment against a 112-bit salt,
/// which puts it at octet 7 of the block; RFC 7714 shortens the salt to 96 bits and says nothing
/// about moving anything, and no RFC publishes a KDF vector for the AEAD profiles. Keeping the
/// block layout fixed and the salt where it already was is the reading that changes one thing at
/// a time. `docs/specs/srtp.md` §4.3 records it as the one AEAD parameter nothing external pins.
/// # Errors
///
/// [`SrtpError::KeyLength`] when the master key or salt is not a length this PRF can run at.
/// `Context::new` has already measured both against the profile, so this cannot be reached from
/// there — but it returns an error rather than degrading, because the two ways of degrading are
/// both silent. Falling back to a zero key would derive session keys from nothing and protect
/// every stream with the same ones; leaving `out` zeroed would do the same by another route. A
/// length check in another function is not a guarantee this one may rely on.
fn derive(
    master_key: &[u8],
    master_salt: &[u8],
    label: Label,
    out: &mut [u8],
) -> Result<(), SrtpError> {
    let mut iv = [0u8; 16];
    let slot = iv
        .get_mut(..master_salt.len())
        .ok_or(SrtpError::KeyLength {
            what: "master salt",
            expected: MASTER_SALT_LEN,
            actual: master_salt.len(),
        })?;
    slot.copy_from_slice(master_salt);
    // The label lands on octet 7 of the block for every profile — §4.3 of `docs/specs/srtp.md`.
    *iv.get_mut(7).ok_or(SrtpError::TooShort(16))? ^= label as u8;
    // Octets 8..14 would carry `index DIV kdr`; with a rate of zero it is six zero octets, and
    // exclusive-oring zero changes nothing. Written out so the alignment is visible rather than
    // implied by its absence. For the 96-bit AEAD salt they are zero already.
    iv[14] = 0;
    iv[15] = 0;

    out.fill(0);
    match master_key.len() {
        32 => {
            let key = <&[u8; 32]>::try_from(master_key).map_err(|_| SrtpError::KeyLength {
                what: "master key",
                expected: 32,
                actual: master_key.len(),
            })?;
            Aes256Ctr::new(key.into(), (&iv).into()).apply_keystream(out);
        }
        16 => {
            let key = <&[u8; 16]>::try_from(master_key).map_err(|_| SrtpError::KeyLength {
                what: "master key",
                expected: 16,
                actual: master_key.len(),
            })?;
            Aes128Ctr::new(key.into(), (&iv).into()).apply_keystream(out);
        }
        actual => {
            return Err(SrtpError::KeyLength {
                what: "master key",
                expected: MASTER_KEY_LEN,
                actual,
            });
        }
    }
    Ok(())
}

/// One direction of one SRTP stream.
///
/// Directional on purpose: RFC 3711 keys each direction separately, and a context used for both
/// would have two senders sharing one replay window and one rollover counter.
#[derive(Debug)]
pub struct Context {
    /// The transform in force. Every length this context uses is read off it.
    profile: Profile,
    session: Session,
    /// The rollover counter — the high 32 bits of the 48-bit packet index (§3.3.1).
    roc: u32,
    /// The highest sequence number seen, for inferring the rollover.
    highest_seq: Option<u16>,
    /// The replay window, most recent packet at bit 0 (§3.3.2).
    replay: u64,
    /// The SRTCP index this side sends, 31 bits (§3.4).
    rtcp_index: u32,
    /// The highest authenticated SRTCP index received, separate from the SRTP sequence/ROC (§3.4).
    highest_rtcp_index: Option<u32>,
    /// The SRTCP replay window, most recent authenticated index at bit 0 (§3.4, §3.3.2).
    rtcp_replay: u64,
}

impl Context {
    /// A context from a negotiated profile and the master key and salt that were keyed for it.
    ///
    /// The profile comes first because it decides what the other two arguments must be. Both are
    /// measured **against it** rather than against a constant, so a 16-octet salt keyed for
    /// counter mode cannot be installed under an AEAD name, and a 128-bit key cannot be installed
    /// under `AEAD_AES_256_GCM`. That refusal is the whole reason the profile is an argument: the
    /// two would otherwise produce a context that is structurally valid and protects a stream
    /// with something other than what was negotiated.
    ///
    /// # Errors
    ///
    /// [`SrtpError::KeyLength`] naming which of the two was wrong and what the profile requires.
    pub fn new(profile: Profile, master_key: &[u8], master_salt: &[u8]) -> Result<Self, SrtpError> {
        let (key_len, salt_len) = profile.key_and_salt_len();
        if master_key.len() != key_len {
            return Err(SrtpError::KeyLength {
                what: "master key",
                expected: key_len,
                actual: master_key.len(),
            });
        }
        if master_salt.len() != salt_len {
            return Err(SrtpError::KeyLength {
                what: "master salt",
                expected: salt_len,
                actual: master_salt.len(),
            });
        }

        // RFC 7714 §15's note, read the other way round: the KDF's input key length is the session
        // encryption key length, so a 256-bit master key derives a 256-bit session key. The salt
        // keeps the master salt's length because that is what the transform's IV is built from —
        // 112 bits for RFC 3711's keystream offset, 96 for RFC 7714 §8.1's AEAD IV.
        let auth_len = if profile.is_aead() {
            0
        } else {
            SESSION_AUTH_LEN
        };
        let mut session = Session {
            rtp_key: vec![0; key_len],
            rtp_salt: vec![0; salt_len],
            rtp_auth: vec![0; auth_len],
            rtcp_key: vec![0; key_len],
            rtcp_salt: vec![0; salt_len],
            rtcp_auth: vec![0; auth_len],
        };
        derive(
            master_key,
            master_salt,
            Label::RtpEncryption,
            &mut session.rtp_key,
        )?;
        derive(
            master_key,
            master_salt,
            Label::RtpSalt,
            &mut session.rtp_salt,
        )?;
        derive(
            master_key,
            master_salt,
            Label::RtpAuthentication,
            &mut session.rtp_auth,
        )?;
        derive(
            master_key,
            master_salt,
            Label::RtcpEncryption,
            &mut session.rtcp_key,
        )?;
        derive(
            master_key,
            master_salt,
            Label::RtcpSalt,
            &mut session.rtcp_salt,
        )?;
        derive(
            master_key,
            master_salt,
            Label::RtcpAuthentication,
            &mut session.rtcp_auth,
        )?;

        Ok(Self {
            profile,
            session,
            roc: 0,
            highest_seq: None,
            replay: 0,
            rtcp_index: 0,
            highest_rtcp_index: None,
            rtcp_replay: 0,
        })
    }

    /// The protection profile this context was keyed for.
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.profile
    }

    /// Encrypt and authenticate an RTP packet in place, returning it with the tag appended.
    ///
    /// `packet` is a complete serialized RTP packet. The header is left readable — a relay has
    /// to see the sequence number and SSRC to do its job — and authenticated, so it cannot be
    /// altered without detection.
    pub fn protect(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        let header_len = rtp_header_len(packet).ok_or(SrtpError::TooShort(packet.len()))?;
        let (sequence, ssrc) =
            sequence_and_ssrc(packet).ok_or(SrtpError::TooShort(packet.len()))?;

        // The sender's index simply follows its own sequence numbers.
        // A sender knows its own order, so a large step backwards is a wrap rather than a guess.
        let roc = match self.highest_seq {
            Some(previous) if i32::from(previous) - i32::from(sequence) > 32_768 => {
                self.roc = self.roc.wrapping_add(1);
                self.roc
            }
            _ => self.roc,
        };
        self.highest_seq = Some(sequence);

        if self.profile.is_aead() {
            // RFC 7714 §8.2: the header — through the CSRCs and any extension — is Associated
            // Data, the payload is Plaintext, and the tag lands where the payload ended. One
            // construction produces both, so there is no ordering question to get wrong.
            let (header, payload) = packet.split_at(header_len);
            let iv = aead_rtp_iv(&self.session.rtp_salt, ssrc, roc, sequence);
            let mut sealed = aead_seal(self.profile, &self.session.rtp_key, &iv, header, payload)?;
            let mut out = header.to_vec();
            out.append(&mut sealed);
            return Ok(out);
        }

        let mut out = packet.to_vec();
        let (_, payload) = out.split_at_mut(header_len);
        keystream(
            &self.session.rtp_key,
            &self.session.rtp_salt,
            ssrc,
            index_of(roc, sequence),
        )?
        .apply_keystream(payload);

        // The tag covers the whole packet *and* the rollover counter, which is not transmitted.
        // Without the ROC in the tag, a packet from before a wrap could be replayed after it.
        let tag = authenticate(&self.session.rtp_auth, &out, Some(roc));
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Authenticate and decrypt an RTP packet, returning the plaintext packet.
    ///
    /// Authentication happens **before** decryption and before the replay window is updated: a
    /// packet that fails it never touches this context's state, which is what stops an attacker
    /// advancing the window with forgeries.
    pub fn unprotect(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        let tag_len = self.profile.tag_len();
        if packet.len() < tag_len {
            return Err(SrtpError::TooShort(packet.len()));
        }

        if self.profile.is_aead() {
            let header_len = rtp_header_len(packet).ok_or(SrtpError::TooShort(packet.len()))?;
            let (sequence, ssrc) =
                sequence_and_ssrc(packet).ok_or(SrtpError::TooShort(packet.len()))?;
            let (header, sealed) = packet.split_at(header_len);
            let roc = self.guess_roc(sequence);
            let iv = aead_rtp_iv(&self.session.rtp_salt, ssrc, roc, sequence);
            // AEAD releases no plaintext to a caller until the tag has verified, so this is the
            // same "authenticate before decrypt" ordering the counter-mode branch spells out —
            // held by the construction rather than by the sequence of statements.
            let plain = aead_open(self.profile, &self.session.rtp_key, &iv, header, sealed)?;

            // Only now, with the packet proven genuine, is replay considered — and the plaintext
            // is still not returned until it has passed.
            self.check_replay(roc, sequence)?;
            let mut out = header.to_vec();
            out.extend_from_slice(&plain);
            self.accept(roc, sequence);
            return Ok(out);
        }

        let (body, tag) = packet.split_at(packet.len() - tag_len);
        let header_len = rtp_header_len(body).ok_or(SrtpError::TooShort(body.len()))?;
        let (sequence, ssrc) = sequence_and_ssrc(body).ok_or(SrtpError::TooShort(body.len()))?;

        let roc = self.guess_roc(sequence);
        let expected = authenticate(&self.session.rtp_auth, body, Some(roc));
        if expected.ct_eq(tag).unwrap_u8() != 1 {
            return Err(SrtpError::NotAuthentic);
        }

        // Only now, with the packet proven genuine, is replay considered.
        self.check_replay(roc, sequence)?;

        let mut out = body.to_vec();
        let (_, payload) = out.split_at_mut(header_len);
        keystream(
            &self.session.rtp_key,
            &self.session.rtp_salt,
            ssrc,
            index_of(roc, sequence),
        )?
        .apply_keystream(payload);

        self.accept(roc, sequence);
        Ok(out)
    }

    /// Encrypt and authenticate an RTCP compound packet (RFC 3711 §3.4).
    pub fn protect_rtcp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        // The first eight octets — header and sender SSRC — stay readable, as with RTP.
        const RTCP_HEADER_LEN: usize = 8;
        if packet.len() < RTCP_HEADER_LEN {
            return Err(SrtpError::TooShort(packet.len()));
        }
        let ssrc = u32::from_be_bytes(
            packet
                .get(4..8)
                .and_then(|s| s.try_into().ok())
                .ok_or(SrtpError::TooShort(packet.len()))?,
        );
        // §3.4: the index "MUST be set to zero before the first SRTCP packet is sent, and MUST be
        // incremented by one, modulo 2^31, *after* each SRTCP packet is sent". Read then advance,
        // so the first packet carries zero and no index is ever skipped.
        let index = self.rtcp_index;
        self.rtcp_index = self.rtcp_index.wrapping_add(1) & 0x7FFF_FFFF;

        if self.profile.is_aead() {
            return self.aead_protect_rtcp(packet, ssrc, index, true);
        }

        let mut out = packet.to_vec();
        let (_, payload) = out.split_at_mut(RTCP_HEADER_LEN);
        keystream(
            &self.session.rtcp_key,
            &self.session.rtcp_salt,
            ssrc,
            u64::from(index),
        )?
        .apply_keystream(payload);

        // The trailer carries the encryption flag and the index in the clear; the tag covers it.
        out.extend_from_slice(&(index | 0x8000_0000).to_be_bytes());
        let tag = authenticate(&self.session.rtcp_auth, &out, None);
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// One AEAD SRTCP packet (RFC 7714 §9).
    ///
    /// The field order is **not** RFC 3711's, and that is the whole subtlety of this function.
    /// §9.2 puts the cipher — payload and tag together — *before* the ESRTCP word, so the four
    /// octets carrying the encryption flag and the index are the last thing on the wire and are
    /// Associated Data rather than covered by a tag that follows them. Producing RFC 3711's order
    /// with an AEAD tag yields a packet no conformant peer authenticates.
    ///
    /// `encrypt` is §9's E-flag. When it is clear, §9.3 makes the *whole* packet Associated Data
    /// and the plaintext empty, so the cipher is exactly the tag and nothing is hidden. sipx only
    /// ever sends the encrypted form; the other branch exists because RFC 7714 §17.3 and §17.4
    /// publish vectors for it, and a receiver that could not read one would drop a peer's reports.
    fn aead_protect_rtcp(
        &self,
        packet: &[u8],
        ssrc: u32,
        index: u32,
        encrypt: bool,
    ) -> Result<Vec<u8>, SrtpError> {
        const RTCP_HEADER_LEN: usize = 8;
        let esrtcp = if encrypt { index | 0x8000_0000 } else { index };
        let esrtcp = esrtcp.to_be_bytes();
        let iv = aead_rtcp_iv(&self.session.rtcp_salt, ssrc, index);

        let split = if encrypt {
            RTCP_HEADER_LEN
        } else {
            packet.len()
        };
        let (clear, plaintext) = packet.split_at(split);
        let mut aad = clear.to_vec();
        aad.extend_from_slice(&esrtcp);

        let mut sealed = aead_seal(self.profile, &self.session.rtcp_key, &iv, &aad, plaintext)?;
        let mut out = clear.to_vec();
        out.append(&mut sealed);
        out.extend_from_slice(&esrtcp);
        Ok(out)
    }

    /// The inverse of [`Context::aead_protect_rtcp`], up to but not including the replay window.
    ///
    /// Returns the recovered RTCP packet and the authenticated index, so the caller can run
    /// §3.3.2's replay rule on an index that has already been proven rather than on one read out
    /// of a datagram anybody could have written.
    fn aead_unprotect_rtcp(&self, packet: &[u8]) -> Result<(Vec<u8>, u32), SrtpError> {
        const RTCP_HEADER_LEN: usize = 8;
        const TRAILER_LEN: usize = 4;
        let tag_len = self.profile.tag_len();
        if packet.len() < RTCP_HEADER_LEN + TRAILER_LEN + tag_len {
            return Err(SrtpError::TooShort(packet.len()));
        }
        let (body, trailer) = packet.split_at(packet.len() - TRAILER_LEN);
        let esrtcp = u32::from_be_bytes(
            trailer
                .try_into()
                .map_err(|_| SrtpError::TooShort(packet.len()))?,
        );
        let encrypted = esrtcp & 0x8000_0000 != 0;
        let index = esrtcp & SRTCP_INDEX_MASK;
        let ssrc = u32::from_be_bytes(
            body.get(4..8)
                .and_then(|s| s.try_into().ok())
                .ok_or(SrtpError::TooShort(body.len()))?,
        );

        // With the E-flag clear the cipher is the tag alone and everything before it is Associated
        // Data (§9.3); with it set the AAD stops after the eight-octet header (§9.2). Either way
        // the ESRTCP word is the last thing the AEAD authenticates and the first thing read.
        let split = if encrypted {
            RTCP_HEADER_LEN
        } else {
            body.len()
                .checked_sub(tag_len)
                .ok_or(SrtpError::TooShort(packet.len()))?
        };
        let (clear, sealed) = body.split_at(split);
        let mut aad = clear.to_vec();
        aad.extend_from_slice(trailer);

        let iv = aead_rtcp_iv(&self.session.rtcp_salt, ssrc, index);
        let plain = aead_open(self.profile, &self.session.rtcp_key, &iv, &aad, sealed)?;

        let mut out = clear.to_vec();
        out.extend_from_slice(&plain);
        Ok((out, index))
    }

    /// Authenticate and decrypt an RTCP compound packet.
    pub fn unprotect_rtcp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        const RTCP_HEADER_LEN: usize = 8;
        const TRAILER_LEN: usize = 4;

        if self.profile.is_aead() {
            // Authentication and decryption are one step, so the index this returns has been
            // proven before the window sees it — the same order the counter-mode branch keeps.
            let (out, index) = self.aead_unprotect_rtcp(packet)?;
            self.check_rtcp_replay(index)?;
            self.accept_rtcp(index);
            return Ok(out);
        }

        if packet.len() < RTCP_HEADER_LEN + TRAILER_LEN + TAG_LEN {
            return Err(SrtpError::TooShort(packet.len()));
        }
        let (body, tag) = packet.split_at(packet.len() - TAG_LEN);
        let expected = authenticate(&self.session.rtcp_auth, body, None);
        if expected.ct_eq(tag).unwrap_u8() != 1 {
            return Err(SrtpError::NotAuthentic);
        }

        let (payload_and_header, trailer) = body.split_at(body.len() - TRAILER_LEN);
        let trailer = u32::from_be_bytes(
            trailer
                .try_into()
                .map_err(|_| SrtpError::TooShort(packet.len()))?,
        );
        let encrypted = trailer & 0x8000_0000 != 0;
        let index = trailer & 0x7FFF_FFFF;
        // The explicit SRTCP index has authenticated by this point. Only now may it be compared
        // with the replay window; a forged high index therefore cannot move trusted state.
        self.check_rtcp_replay(index)?;
        let ssrc = u32::from_be_bytes(
            body.get(4..8)
                .and_then(|s| s.try_into().ok())
                .ok_or(SrtpError::TooShort(body.len()))?,
        );

        let mut out = payload_and_header.to_vec();
        if encrypted {
            let (_, payload) = out.split_at_mut(RTCP_HEADER_LEN);
            keystream(
                &self.session.rtcp_key,
                &self.session.rtcp_salt,
                ssrc,
                u64::from(index),
            )?
            .apply_keystream(payload);
        }
        self.accept_rtcp(index);
        Ok(out)
    }

    /// The rollover counter this sequence number most likely belongs to (RFC 3711 §3.3.1).
    ///
    /// The arithmetic is **signed**, and that is the whole subtlety. The RFC writes
    /// `if (SEQ - s_l > 32768)` over two 16-bit values, and it means ordinary subtraction that
    /// may go negative — not wrapping `u16` subtraction. Read as wrapping, a packet arriving one
    /// place out of order looks 65 535 ahead, is taken for the previous cycle, and fails
    /// authentication. Every out-of-order packet in a call, silently dropped.
    fn guess_roc(&self, sequence: u16) -> u32 {
        let Some(highest) = self.highest_seq else {
            return self.roc;
        };
        let (sequence, highest) = (i32::from(sequence), i32::from(highest));

        if highest < 32_768 {
            // Near the start of a cycle: a number far *above* us is from the previous one.
            if sequence - highest > 32_768 {
                return self.roc.wrapping_sub(1);
            }
        } else if highest - 32_768 > sequence {
            // Near the end of a cycle: a number far *below* us has already wrapped.
            return self.roc.wrapping_add(1);
        }
        self.roc
    }

    fn check_replay(&self, roc: u32, sequence: u16) -> Result<(), SrtpError> {
        let Some(highest) = self.highest_seq else {
            return Ok(());
        };
        let incoming = index_of(roc, sequence);
        let current = index_of(self.roc, highest);

        if incoming > current {
            return Ok(());
        }
        let behind = current - incoming;
        if behind >= 64 {
            // Older than the window can judge. Refused rather than accepted: accepting it would
            // mean a packet captured minutes ago could be replayed for as long as the key lives.
            return Err(SrtpError::Replayed(sequence));
        }
        if self.replay & (1 << behind) != 0 {
            return Err(SrtpError::Replayed(sequence));
        }
        Ok(())
    }

    fn accept(&mut self, roc: u32, sequence: u16) {
        let incoming = index_of(roc, sequence);
        let current = self
            .highest_seq
            .map_or(0, |highest| index_of(self.roc, highest));

        if self.highest_seq.is_none() || incoming > current {
            let advance = if self.highest_seq.is_none() {
                0
            } else {
                incoming - current
            };
            self.replay = if advance >= 64 {
                0
            } else {
                self.replay << advance
            };
            self.replay |= 1;
            self.roc = roc;
            self.highest_seq = Some(sequence);
        } else {
            let behind = current - incoming;
            if behind < 64 {
                self.replay |= 1 << behind;
            }
        }
    }

    fn check_rtcp_replay(&self, index: u32) -> Result<(), SrtpError> {
        let Some(highest) = self.highest_rtcp_index else {
            return Ok(());
        };
        if srtcp_forward_distance(highest, index).is_some() {
            return Ok(());
        }
        let behind = highest.wrapping_sub(index) & SRTCP_INDEX_MASK;
        if behind >= 64 || self.rtcp_replay & (1u64 << behind) != 0 {
            return Err(SrtpError::ReplayedRtcp(index));
        }
        Ok(())
    }

    fn accept_rtcp(&mut self, index: u32) {
        let Some(highest) = self.highest_rtcp_index else {
            self.highest_rtcp_index = Some(index);
            self.rtcp_replay = 1;
            return;
        };
        if let Some(advance) = srtcp_forward_distance(highest, index) {
            self.rtcp_replay = if advance >= 64 {
                0
            } else {
                self.rtcp_replay << advance
            };
            self.rtcp_replay |= 1;
            self.highest_rtcp_index = Some(index);
        } else {
            let behind = highest.wrapping_sub(index) & SRTCP_INDEX_MASK;
            if behind < 64 {
                self.rtcp_replay |= 1u64 << behind;
            }
        }
    }
}

const SRTCP_INDEX_MASK: u32 = 0x7FFF_FFFF;
const SRTCP_INDEX_HALF_RANGE: u32 = 0x4000_0000;

/// The forward distance in the 31-bit SRTCP index space, or `None` when `incoming` is not newer.
///
/// RFC 3711 limits one key to the index space, but treating the modulo boundary normally keeps the
/// held replay window correct at the last packet while the caller arranges rekeying. Exactly half
/// the space is ambiguous and is deliberately not considered newer.
fn srtcp_forward_distance(current: u32, incoming: u32) -> Option<u32> {
    let distance = incoming.wrapping_sub(current) & SRTCP_INDEX_MASK;
    (distance != 0 && distance < SRTCP_INDEX_HALF_RANGE).then_some(distance)
}

/// The 48-bit packet index: the rollover counter above the sequence number.
fn index_of(roc: u32, sequence: u16) -> u64 {
    (u64::from(roc) << 16) | u64::from(sequence)
}

/// The AEAD SRTP initialization vector (RFC 7714 §8.1).
///
/// Two zero octets, the SSRC, the rollover counter and the sequence number — twelve octets —
/// exclusive-ored with the twelve-octet session salt. Nothing here is a nonce this stack chooses:
/// every field is already in the packet or already inferred, which is what makes §8.4's rule
/// ("the (ROC,SEQ,SSRC) triple is never used twice with the same master key") a property of the
/// sequence numbering rather than of a random draw.
fn aead_rtp_iv(salt: &[u8], ssrc: u32, roc: u32, sequence: u16) -> [u8; AEAD_IV_LEN] {
    let mut iv = [0u8; AEAD_IV_LEN];
    iv[2..6].copy_from_slice(&ssrc.to_be_bytes());
    iv[6..10].copy_from_slice(&roc.to_be_bytes());
    iv[10..12].copy_from_slice(&sequence.to_be_bytes());
    xor_salt(&mut iv, salt);
    iv
}

/// The AEAD SRTCP initialization vector (RFC 7714 §9.1).
///
/// Two zero octets, the SSRC, two more zero octets, then the 31-bit index right-justified in four
/// octets with a zero bit in front — **the index without the encryption flag**, even when the flag
/// is set. Folding the E-flag in here would give the encrypted and unencrypted forms of the same
/// index two different keystreams and neither would be the RFC's.
fn aead_rtcp_iv(salt: &[u8], ssrc: u32, index: u32) -> [u8; AEAD_IV_LEN] {
    let mut iv = [0u8; AEAD_IV_LEN];
    iv[2..6].copy_from_slice(&ssrc.to_be_bytes());
    iv[8..12].copy_from_slice(&(index & SRTCP_INDEX_MASK).to_be_bytes());
    xor_salt(&mut iv, salt);
    iv
}

fn xor_salt(iv: &mut [u8; AEAD_IV_LEN], salt: &[u8]) {
    for (slot, byte) in iv.iter_mut().zip(salt) {
        *slot ^= *byte;
    }
}

/// Encrypt `plaintext` and authenticate it together with `aad`, returning ciphertext then tag.
///
/// The two profiles differ only in the key schedule, so the branch is on the key length and
/// nothing else. A key that is not the length the profile requires cannot reach here —
/// [`Context::new`] refused it — but it is handled rather than indexed, because the alternative
/// is a panic on a path that carries network data.
fn aead_seal(
    profile: Profile,
    key: &[u8],
    iv: &[u8; AEAD_IV_LEN],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, SrtpError> {
    let mut buffer = plaintext.to_vec();
    let nonce = aes_gcm::Nonce::from_slice(iv);
    let sealed = match profile {
        Profile::AeadAes256Gcm => Aes256Gcm::new_from_slice(key)
            .map_err(|_| key_length(profile, key.len()))?
            .encrypt_in_place(nonce, aad, &mut buffer),
        _ => Aes128Gcm::new_from_slice(key)
            .map_err(|_| key_length(profile, key.len()))?
            .encrypt_in_place(nonce, aad, &mut buffer),
    };
    // The only documented failure is a plaintext beyond `P_MAX` (RFC 7714 §10: 2^36 - 32 octets),
    // which no datagram reaches. Reported rather than unwrapped all the same.
    sealed.map_err(|_| SrtpError::TooShort(plaintext.len()))?;
    Ok(buffer)
}

/// Verify `aad` and `sealed` together and return the plaintext.
///
/// A tag that does not verify yields [`SrtpError::NotAuthentic`] and **no plaintext at all** —
/// the AEAD construction does not release it, which is what makes "authenticate before decrypt"
/// a property here rather than an ordering this module has to remember to keep.
fn aead_open(
    profile: Profile,
    key: &[u8],
    iv: &[u8; AEAD_IV_LEN],
    aad: &[u8],
    sealed: &[u8],
) -> Result<Vec<u8>, SrtpError> {
    if sealed.len() < MAX_TAG_LEN {
        return Err(SrtpError::TooShort(sealed.len()));
    }
    let mut buffer = sealed.to_vec();
    let nonce = aes_gcm::Nonce::from_slice(iv);
    let opened = match profile {
        Profile::AeadAes256Gcm => Aes256Gcm::new_from_slice(key)
            .map_err(|_| key_length(profile, key.len()))?
            .decrypt_in_place(nonce, aad, &mut buffer),
        _ => Aes128Gcm::new_from_slice(key)
            .map_err(|_| key_length(profile, key.len()))?
            .decrypt_in_place(nonce, aad, &mut buffer),
    };
    // Deliberately says nothing about *why*, exactly as the counter-mode branch does not: a
    // caller that could tell "wrong key" from "altered packet" would be an oracle.
    opened.map_err(|_| SrtpError::NotAuthentic)?;
    Ok(buffer)
}

fn key_length(profile: Profile, actual: usize) -> SrtpError {
    SrtpError::KeyLength {
        what: "session key",
        expected: profile.master_key_len(),
        actual,
    }
}

/// The keystream generator for one packet (RFC 3711 §4.1.1).
///
/// `IV = (salt * 2^16) XOR (SSRC * 2^64) XOR (index * 2^16)`. Every term is shifted left by at
/// least two octets, so the low 16 bits of the IV are always zero — which is why a plain
/// 128-bit counter is correct here: it cannot carry into the rest of the block within any packet
/// short enough to exist.
/// # Errors
///
/// [`SrtpError::KeyLength`] when the session key or salt is not the counter-mode transform's.
/// This branch is only reached under `AES_CM_128_HMAC_SHA1_80`, whose session key and salt
/// `Context::new` sized itself, so it cannot fail from there — and it is fallible rather than
/// falling back for the same reason [`derive`] is. A zero key here would encrypt every stream on
/// the host with one keystream and raise nothing anywhere, which is the worst outcome available.
fn keystream(key: &[u8], salt: &[u8], ssrc: u32, index: u64) -> Result<Aes128Ctr, SrtpError> {
    let mut iv = [0u8; 16];
    let slot = iv
        .get_mut(..SESSION_SALT_LEN)
        .ok_or(SrtpError::TooShort(16))?;
    slot.copy_from_slice(salt.get(..SESSION_SALT_LEN).ok_or(SrtpError::KeyLength {
        what: "session salt",
        expected: SESSION_SALT_LEN,
        actual: salt.len(),
    })?);

    for (slot, byte) in iv.iter_mut().skip(4).zip(ssrc.to_be_bytes()) {
        *slot ^= byte;
    }
    // The low 48 bits of the index, which is what a packet index is.
    for (slot, byte) in iv
        .iter_mut()
        .skip(8)
        .zip(index.to_be_bytes().into_iter().skip(2))
    {
        *slot ^= byte;
    }
    let key: &[u8; 16] = key.try_into().map_err(|_| SrtpError::KeyLength {
        what: "session key",
        expected: 16,
        actual: key.len(),
    })?;
    Ok(Aes128Ctr::new(key.into(), (&iv).into()))
}

/// HMAC-SHA1 over the packet, truncated to `n_tag` = 80 bits (RFC 3711 §4.2.1).
///
/// `M` is the authenticated portion of the packet, followed by the rollover counter for SRTP and
/// by nothing for SRTCP (§4.2). The key is taken as a slice rather than as `[u8; SESSION_AUTH_LEN]`
/// so a test can hand it one the RFC published rather than one this module derived.
fn authenticate(key: &[u8], data: &[u8], roc: Option<u32>) -> [u8; TAG_LEN] {
    let mut mac = <HmacSha1 as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts a key of any length"));
    mac.update(data);
    if let Some(roc) = roc {
        mac.update(&roc.to_be_bytes());
    }
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; TAG_LEN];
    // SHA-1 is 20 octets and the tag is 10, so the slice is always there.
    tag.copy_from_slice(full.get(..TAG_LEN).unwrap_or(&[0u8; TAG_LEN]));
    tag
}

/// The sequence number and SSRC of an RTP packet.
///
/// Read fallibly rather than indexed. This function is handed whatever arrived on a UDP socket,
/// and a length check three lines above is not a guarantee a later reader will preserve.
fn sequence_and_ssrc(packet: &[u8]) -> Option<(u16, u32)> {
    let sequence = u16::from_be_bytes(packet.get(2..4)?.try_into().ok()?);
    let ssrc = u32::from_be_bytes(packet.get(8..12)?.try_into().ok()?);
    Some((sequence, ssrc))
}

/// How long the RTP header is, including CSRCs and any extension.
///
/// `None` when the buffer is too short to hold what it claims, which is the case a decoder that
/// trusts the length field turns into a panic.
fn rtp_header_len(packet: &[u8]) -> Option<usize> {
    let first = *packet.first()?;
    if packet.len() < 12 {
        return None;
    }
    let csrc_count = usize::from(first & 0x0F);
    let mut len = 12 + csrc_count * 4;
    if first & 0x10 != 0 {
        // An extension: four octets of header, then a length in 32-bit words.
        let words = usize::from(u16::from_be_bytes([
            *packet.get(len + 2)?,
            *packet.get(len + 3)?,
        ]));
        len += 4 + words * 4;
    }
    (len <= packet.len()).then_some(len)
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

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
            .collect()
    }

    /// RFC 3711 §B.3, checked against the numbers the RFC publishes rather than against this
    /// implementation's own arithmetic.
    ///
    /// This is the test that matters most in the file. A key derivation that is wrong but
    /// self-consistent produces two endpoints that interoperate perfectly with each other and
    /// with nothing else in the world — and every round-trip test in this module would pass.
    ///
    /// It exercises the PRF at the lengths §B.3 uses, which for the authentication label is 94
    /// octets — six AES blocks, enough to catch a counter that does not advance. That is a property
    /// of the appendix and **not** the transform's `n_a`; how many of these octets the default
    /// transform actually keys with is
    /// `the_session_authentication_key_is_the_160_bits_the_rfc_fixes`.
    #[test]
    fn key_derivation_matches_the_rfc() {
        let master_key: [u8; 16] = hex("E1F97A0D3E018BE0D64FA32C06DE4139").try_into().unwrap();
        let master_salt: [u8; 14] = hex("0EC675AD498AFEEBB6960B3AABE6").try_into().unwrap();

        let mut cipher_key = [0u8; 16];
        derive(
            &master_key,
            &master_salt,
            Label::RtpEncryption,
            &mut cipher_key,
        )
        .expect("the RFC's own master key and salt");
        assert_eq!(cipher_key.to_vec(), hex("C61E7A93744F39EE10734AFE3FF7A087"));

        let mut cipher_salt = [0u8; 14];
        derive(&master_key, &master_salt, Label::RtpSalt, &mut cipher_salt)
            .expect("the RFC's own master key and salt");
        assert_eq!(cipher_salt.to_vec(), hex("30CBBC08863D8C85D49DB34A9AE1"));

        let mut auth_key = [0u8; 94];
        derive(
            &master_key,
            &master_salt,
            Label::RtpAuthentication,
            &mut auth_key,
        )
        .expect("the RFC's own master key and salt");
        assert_eq!(
            auth_key.to_vec(),
            hex("CEBE321F6FF7716B6FD4AB49AF256A15\
                 6D38BAA48F0A0ACF3C34E2359E6CDBCE\
                 E049646C43D9327AD175578EF7227098\
                 6371C10C9A369AC2F94A8C5FBCDDDC25\
                 6D6E919A48B610EF17C2041E47403576\
                 6B68642C59BBFC2F34DB60DBDFB2")
        );
    }

    /// The session authentication key is 160 bits, not §B.3's 94 octets.
    ///
    /// RFC 3711 §5.2: "The default session authentication key-length (`n_a`) SHALL be 160 bits", and
    /// §8.2's table repeats it. §4.3.1 derives `n = n_a` octets under label 0x01 — it does not fix
    /// a length of its own. §B.3 walks through a **94**-octet derivation because that appendix
    /// posits "an authentication function which requires a 94-octet session authentication key" to
    /// exercise six AES blocks of the PRF; 94 is a property of the worked example, not of the
    /// default transform.
    ///
    /// Reading it the other way produces a stack whose HMAC key is a different length from every
    /// conformant peer's, so every packet fails authentication in both directions — and every
    /// round-trip test still passes, because both ends are wrong the same way.
    #[test]
    fn the_session_authentication_key_is_the_160_bits_the_rfc_fixes() {
        let context = Context::new(
            Profile::AesCm128HmacSha1_80,
            &hex("E1F97A0D3E018BE0D64FA32C06DE4139"),
            &hex("0EC675AD498AFEEBB6960B3AABE6"),
        )
        .expect("a context");

        assert_eq!(
            context.session.rtp_auth.len(),
            20,
            "n_a SHALL be 160 bits (RFC 3711 §5.2, §8.2)"
        );
        // The first 160 bits of §B.3's own derived block, which is what `n = n_a` selects.
        assert_eq!(
            context.session.rtp_auth.clone(),
            hex("CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4")
        );
        assert_eq!(context.session.rtcp_auth.len(), 20, "and for SRTCP too");
    }

    /// RFC 3711 §4.2.1's tag, over inputs the RFC publishes and against a value this stack did not
    /// produce.
    ///
    /// `k_a` is §B.3's derived authentication key truncated to `n_a`; `M` is §B.1's published RTP
    /// header and the ROC is §B.1's published rollover counter. The expected tags are HMAC-SHA1
    /// (RFC 2104) truncated to `n_tag` = 80 bits, computed with an implementation outside this
    /// repository — a tag that agrees only with [`authenticate`] proves nothing about either.
    ///
    /// Both forms of `M` are pinned, because they differ: §4.2 appends the ROC for SRTP and not for
    /// SRTCP, whose index travels in the packet instead.
    #[test]
    fn the_authentication_tag_is_hmac_sha1_over_the_packet_and_the_roc() {
        let k_a = hex("CEBE321F6FF7716B6FD4AB49AF256A156D38BAA4");
        let m = hex("806E5CBA50681DE55C621599");

        assert_eq!(
            authenticate(&k_a, &m, Some(0xD462_564A)).to_vec(),
            hex("2E19C5351B7F99278F33"),
            "SRTP: M = Authenticated Portion || ROC"
        );
        assert_eq!(
            authenticate(&k_a, &m, None).to_vec(),
            hex("66126DD7550B7E7C90A4"),
            "SRTCP: M = Authenticated Portion only"
        );
    }

    /// RFC 3711 §3.4: "The SRTCP index MUST be set to zero before the first SRTCP packet is sent,
    /// and MUST be incremented by one, modulo 2^31, **after** each SRTCP packet is sent."
    ///
    /// Incrementing first makes the first packet carry 1 and never emits index 0 at all. It is not
    /// an interoperability failure — the index is explicit in the trailer, so a receiver reads
    /// whatever arrives — but it is a stated MUST, and the index feeds the SRTCP keystream IV, so
    /// "which packet used which counter block" is not a free choice.
    #[test]
    fn the_first_srtcp_packet_carries_index_zero() {
        for profile in EVERY_PROFILE {
            let (mut send, _) = keyed(profile);
            let mut packet = vec![0x80, 201, 0x00, 0x07];
            packet.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
            packet.extend_from_slice(b"REPORTBODY-REPORTBODY-RE");

            let first = send.protect_rtcp(&packet).expect("protects");
            let trailer = esrtcp_of(profile, &first);
            assert_eq!(
                trailer & 0x8000_0000,
                0x8000_0000,
                "{profile:?}: the E flag is set"
            );
            assert_eq!(
                trailer & 0x7FFF_FFFF,
                0,
                "{profile:?}: the first index is zero"
            );

            let second = send.protect_rtcp(&packet).expect("protects");
            assert_eq!(
                esrtcp_of(profile, &second) & 0x7FFF_FFFF,
                1,
                "{profile:?}: and it increments after each packet, not before"
            );
        }
    }

    /// The ESRTCP word — the encryption flag and the 31-bit index — wherever the profile puts it.
    ///
    /// The two layouts genuinely differ and neither is a detail: RFC 3711 §3.4 puts the trailer
    /// before the authentication tag, and RFC 7714 §9.2 puts it **after** the cipher, because
    /// under AEAD the tag is the last of the ciphertext and the word is Associated Data. A test
    /// that read one offset for both would pass on the wrong four octets.
    fn esrtcp_of(profile: Profile, protected: &[u8]) -> u32 {
        let end = if profile.is_aead() {
            protected.len()
        } else {
            protected.len() - profile.tag_len()
        };
        u32::from_be_bytes(protected[end - 4..end].try_into().expect("four octets"))
    }

    /// RFC 3711 §B.2. The counter block and the keystream it produces, from the RFC.
    #[test]
    fn the_keystream_matches_the_rfc() {
        let key: [u8; 16] = hex("2B7E151628AED2A6ABF7158809CF4F3C").try_into().unwrap();
        // The RFC gives the offset already shifted; the salt is its first fourteen octets, and
        // SSRC and index are zero, so the IV is exactly that offset.
        let salt: [u8; 14] = hex("F0F1F2F3F4F5F6F7F8F9FAFBFCFD").try_into().unwrap();

        let mut out = [0u8; 48];
        keystream(&key, &salt, 0, 0)
            .expect("the RFC's own session key and salt")
            .apply_keystream(&mut out);

        assert_eq!(out[..16].to_vec(), hex("E03EAD0935C95E80E166B16DD92B4EB4"));
        assert_eq!(
            out[16..32].to_vec(),
            hex("D23513162B02D0F72A43A2FE4A5F97AB")
        );
        assert_eq!(out[32..].to_vec(), hex("41E95B3BB0A2E8DD477901E4FCA894C0"));
    }

    fn rtp(sequence: u16, payload: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x80, 0x00];
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(&(u32::from(sequence) * 160).to_be_bytes());
        packet.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    /// A sender and a receiver keyed alike for one profile.
    ///
    /// Key and salt are sized from the profile rather than written out, which is the same rule
    /// the production path follows — a test that spelled `[7u8; 16]` would stop compiling the day
    /// a profile changed its key length and would say nothing useful when it did.
    fn keyed(profile: Profile) -> (Context, Context) {
        let (key_len, salt_len) = profile.key_and_salt_len();
        let (key, salt) = (vec![7u8; key_len], vec![9u8; salt_len]);
        (
            Context::new(profile, &key, &salt).expect("a sender"),
            Context::new(profile, &key, &salt).expect("a receiver"),
        )
    }

    /// The behavioural tests below run over **every** profile rather than over the one they were
    /// written for. Replay, rollover inference and the refusal to touch context state before a
    /// packet has authenticated are properties of SRTP and not of a cipher, and a second copy of
    /// each test for AEAD would be two places for them to drift apart (`M-41`).
    const EVERY_PROFILE: [Profile; 3] = Profile::STRONGEST_FIRST;

    #[test]
    fn a_protected_packet_round_trips() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let plain = rtp(1000, b"the quick brown fox jumps");

            let protected = send.protect(&plain).expect("protects");
            assert_eq!(
                protected.len(),
                plain.len() + profile.tag_len(),
                "{profile:?} grows the packet by its own tag length and nothing else"
            );
            assert_eq!(
                recv.unprotect(&protected).expect("unprotects"),
                plain,
                "{profile:?}"
            );
        }
    }

    /// The header stays readable and the payload does not. That split is the whole design: a
    /// relay must see the sequence number, and nobody should hear the audio.
    #[test]
    fn the_header_is_readable_and_the_payload_is_not() {
        for profile in EVERY_PROFILE {
            let (mut send, _) = keyed(profile);
            let plain = rtp(7, b"SECRET AUDIO SAMPLES HERE");
            let protected = send.protect(&plain).expect("protects");

            assert_eq!(
                &protected[..12],
                &plain[..12],
                "{profile:?}: the header travels in the clear"
            );
            assert!(
                !protected.windows(6).any(|w| w == b"SECRET"),
                "{profile:?}: the payload must not appear on the wire"
            );
        }
    }

    #[test]
    fn an_altered_packet_is_refused() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let mut protected = send.protect(&rtp(1, b"hello")).expect("protects");

            // One bit, anywhere.
            protected[14] ^= 0x01;
            assert_eq!(
                recv.unprotect(&protected),
                Err(SrtpError::NotAuthentic),
                "{profile:?}"
            );
        }
    }

    /// Including the header, which is not encrypted and would otherwise be free to rewrite.
    ///
    /// For AEAD this is the associated-data boundary doing its job: RFC 7714 §8.2 authenticates
    /// the header without encrypting it, so a transform that passed the header to GCM as neither
    /// plaintext nor AAD would still round-trip and would let anyone rewrite the sequence number.
    #[test]
    fn an_altered_header_is_refused() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let mut protected = send.protect(&rtp(1, b"hello")).expect("protects");

            protected[3] ^= 0x01; // the sequence number
            assert_eq!(
                recv.unprotect(&protected),
                Err(SrtpError::NotAuthentic),
                "{profile:?}"
            );
        }
    }

    #[test]
    fn a_packet_from_a_different_key_is_refused() {
        for profile in EVERY_PROFILE {
            let (mut send, _) = keyed(profile);
            let (key_len, salt_len) = profile.key_and_salt_len();
            let mut stranger = Context::new(profile, &vec![1u8; key_len], &vec![2u8; salt_len])
                .expect("a context");
            let protected = send.protect(&rtp(1, b"hello")).expect("protects");
            assert_eq!(
                stranger.unprotect(&protected),
                Err(SrtpError::NotAuthentic),
                "{profile:?}"
            );
        }
    }

    /// RFC 3711 §3.3.2. Without this a captured packet can be replayed into a call for as long
    /// as the key lives, and it authenticates perfectly because it is genuine.
    #[test]
    fn a_replayed_packet_is_refused() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let protected = send.protect(&rtp(100, b"hello")).expect("protects");

            recv.unprotect(&protected).expect("the first time");
            assert_eq!(
                recv.unprotect(&protected),
                Err(SrtpError::Replayed(100)),
                "{profile:?}"
            );
        }
    }

    #[test]
    fn out_of_order_packets_inside_the_window_are_accepted_once_each() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let packets: Vec<Vec<u8>> = (200..210)
                .map(|n| send.protect(&rtp(n, b"x")).expect("protects"))
                .collect();

            // Delivered backwards, which a network does.
            for protected in packets.iter().rev() {
                recv.unprotect(protected).expect("accepted once");
            }
            // And not a second time.
            for protected in &packets {
                assert!(
                    matches!(recv.unprotect(protected), Err(SrtpError::Replayed(_))),
                    "{profile:?}"
                );
            }
        }
    }

    /// Older than the window can judge is refused rather than accepted. Accepting it would make
    /// the window a speed bump: an attacker would only have to wait.
    #[test]
    fn a_packet_older_than_the_window_is_refused() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let old = send.protect(&rtp(1, b"x")).expect("protects");
            for n in 2..200 {
                let p = send.protect(&rtp(n, b"x")).expect("protects");
                recv.unprotect(&p).expect("accepted");
            }
            assert_eq!(
                recv.unprotect(&old),
                Err(SrtpError::Replayed(1)),
                "{profile:?}"
            );
        }
    }

    /// The sequence number wraps every twenty minutes at speech packet rates, and the rollover
    /// counter it implies is never transmitted. Both ends infer it; an implementation that
    /// infers differently decrypts to noise from that moment on — and under AEAD it is worse than
    /// noise, because RFC 7714 §8.1 puts the counter straight into the IV.
    #[test]
    fn the_stream_survives_the_sequence_number_wrapping() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            for n in [65_530u16, 65_533, 65_535, 0, 1, 5] {
                let plain = rtp(n, b"across the wrap");
                let protected = send.protect(&plain).expect("protects");
                assert_eq!(
                    recv.unprotect(&protected).expect("unprotects"),
                    plain,
                    "{profile:?}: sequence {n} did not survive"
                );
            }
            assert_eq!(send.roc, 1, "{profile:?}: the sender counted one rollover");
            assert_eq!(recv.roc, 1, "{profile:?}: and so did the receiver");
        }
    }

    #[test]
    fn rtcp_round_trips_and_is_encrypted() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            // A minimal receiver report: version 2, PT 201, then the sender SSRC and a body.
            let mut packet = vec![0x80, 201, 0x00, 0x07];
            packet.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
            packet.extend_from_slice(b"REPORTBODY-REPORTBODY-RE");

            let protected = send.protect_rtcp(&packet).expect("protects");
            assert!(
                !protected.windows(6).any(|w| w == b"REPORT"),
                "{profile:?}: the report body must not appear on the wire"
            );
            assert_eq!(
                recv.unprotect_rtcp(&protected).expect("unprotects"),
                packet,
                "{profile:?}"
            );
        }
    }

    fn rtcp() -> Vec<u8> {
        let mut packet = vec![0x80, 201, 0x00, 0x07];
        packet.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        packet.extend_from_slice(b"REPORTBODY-REPORTBODY-RE");
        packet
    }

    /// RFC 3711 §3.4 applies §3.3.2's replay rule to the explicit SRTCP index. A genuine captured
    /// report authenticates forever, so authentication alone cannot reject its second delivery.
    #[test]
    fn an_authenticated_srtcp_packet_is_accepted_once() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let first = send.protect_rtcp(&rtcp()).expect("protects index zero");
            let second = send.protect_rtcp(&rtcp()).expect("protects index one");

            recv.unprotect_rtcp(&first).expect("accepted once");
            assert_eq!(
                recv.unprotect_rtcp(&first),
                Err(SrtpError::ReplayedRtcp(0)),
                "{profile:?}"
            );
            recv.unprotect_rtcp(&second)
                .expect("a distinct authenticated index remains acceptable");
        }
    }

    /// RFC 3711 §3.4 says the SRTCP replay list is separate from the SRTP list. Both streams begin
    /// at index zero, and advancing one must not consume the other's bit zero.
    #[test]
    fn srtp_and_srtcp_have_separate_replay_windows() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let media = send.protect(&rtp(0, b"audio")).expect("protects RTP zero");
            let control = send.protect_rtcp(&rtcp()).expect("protects RTCP zero");

            recv.unprotect(&media).expect("RTP zero is accepted");
            recv.unprotect_rtcp(&control)
                .expect("SRTCP zero is independently accepted");
            assert_eq!(
                recv.unprotect(&media),
                Err(SrtpError::Replayed(0)),
                "{profile:?}"
            );
            assert_eq!(
                recv.unprotect_rtcp(&control),
                Err(SrtpError::ReplayedRtcp(0)),
                "{profile:?}"
            );
        }
    }

    /// RFC 3711 §3.3 step 5 authenticates before touching replay state. Changing the explicit index
    /// without recomputing the tag is a forged high-index packet and cannot push the window ahead.
    #[test]
    fn a_forged_high_srtcp_index_does_not_advance_the_window() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let authentic = send.protect_rtcp(&rtcp()).expect("protects");
            let mut forged = authentic.clone();
            let trailer = if profile.is_aead() {
                forged.len() - 4
            } else {
                forged.len() - profile.tag_len() - 4
            };
            forged[trailer..trailer + 4].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());

            assert_eq!(
                recv.unprotect_rtcp(&forged),
                Err(SrtpError::NotAuthentic),
                "{profile:?}"
            );
            recv.unprotect_rtcp(&authentic)
                .expect("the authentic index zero was not made old");
        }
    }

    /// A 64-packet window holds distances zero through 63. An unseen packet exactly 63 behind is
    /// accepted once; one exactly 64 behind is too old. Testing both edges pins the comparison,
    /// rather than merely observing a packet comfortably outside the window.
    #[test]
    fn the_srtcp_replay_window_holds_exactly_sixty_four_indices() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let oldest_held = send.protect_rtcp(&rtcp()).expect("protects index zero");
            send.rtcp_index = 63;
            let newest = send.protect_rtcp(&rtcp()).expect("protects index 63");
            recv.unprotect_rtcp(&newest).expect("establishes index 63");
            recv.unprotect_rtcp(&oldest_held)
                .expect("an unseen packet 63 places behind remains held");
            assert_eq!(
                recv.unprotect_rtcp(&oldest_held),
                Err(SrtpError::ReplayedRtcp(0)),
                "{profile:?}"
            );

            let (mut send, mut recv) = keyed(profile);
            let too_old = send.protect_rtcp(&rtcp()).expect("protects index zero");
            send.rtcp_index = 64;
            let newest = send.protect_rtcp(&rtcp()).expect("protects index 64");
            recv.unprotect_rtcp(&newest).expect("establishes index 64");
            assert_eq!(
                recv.unprotect_rtcp(&too_old),
                Err(SrtpError::ReplayedRtcp(0)),
                "{profile:?}"
            );
        }
    }

    /// The SRTCP index is 31 bits. The window treats `0x7fff_ffff -> 0` as one forward step, not as
    /// an ancient packet, and still remembers the last pre-wrap packet after crossing the boundary.
    #[test]
    fn the_srtcp_replay_window_crosses_the_index_wrap() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            send.rtcp_index = 0x7FFF_FFFE;
            let before = send.protect_rtcp(&rtcp()).expect("protects max minus one");
            let last = send.protect_rtcp(&rtcp()).expect("protects max");
            let wrapped = send.protect_rtcp(&rtcp()).expect("protects zero");

            recv.unprotect_rtcp(&before).expect("accepts max minus one");
            recv.unprotect_rtcp(&last).expect("accepts max");
            recv.unprotect_rtcp(&wrapped).expect("accepts wrapped zero");
            assert_eq!(
                recv.unprotect_rtcp(&last),
                Err(SrtpError::ReplayedRtcp(0x7FFF_FFFF)),
                "{profile:?}"
            );
        }
    }

    #[test]
    fn an_altered_rtcp_packet_is_refused() {
        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let mut packet = vec![0x80, 201, 0x00, 0x07];
            packet.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
            packet.extend_from_slice(b"REPORTBODY-REPORTBODY-RE");

            let mut protected = send.protect_rtcp(&packet).expect("protects");
            protected[10] ^= 0x01;
            assert_eq!(
                recv.unprotect_rtcp(&protected),
                Err(SrtpError::NotAuthentic),
                "{profile:?}"
            );
        }
    }

    /// The lengths are the **profile's**, and a key or salt of the other profile's length is
    /// refused rather than stretched or truncated to fit.
    ///
    /// This is `M-41`'s first acceptance item as a test. The failure it guards against is not a
    /// crash: 16 octets of counter-mode master salt accepted under `AEAD_AES_128_GCM` produces a
    /// context that protects and unprotects its own traffic perfectly and interoperates with
    /// nothing, under a name that says it should.
    #[test]
    fn a_key_or_salt_of_another_profiles_length_is_refused_by_name() {
        for profile in EVERY_PROFILE {
            let (key_len, salt_len) = profile.key_and_salt_len();

            for wrong in EVERY_PROFILE
                .into_iter()
                .map(Profile::master_key_len)
                .filter(|len| *len != key_len)
            {
                let error = Context::new(profile, &vec![0u8; wrong], &vec![0u8; salt_len])
                    .expect_err("a key of another profile's length must be refused");
                assert!(
                    error.to_string().contains("master key"),
                    "{profile:?} with a {wrong}-octet key: {error}"
                );
            }

            for wrong in EVERY_PROFILE
                .into_iter()
                .map(Profile::master_salt_len)
                .filter(|len| *len != salt_len)
            {
                let error = Context::new(profile, &vec![0u8; key_len], &vec![0u8; wrong])
                    .expect_err("a salt of another profile's length must be refused");
                assert!(
                    error.to_string().contains("master salt"),
                    "{profile:?} with a {wrong}-octet salt: {error}"
                );
            }

            Context::new(profile, &vec![0u8; key_len], &vec![0u8; salt_len])
                .expect("its own lengths are accepted");
        }
    }

    #[test]
    fn a_wrong_length_key_is_refused_by_name() {
        let profile = Profile::AesCm128HmacSha1_80;
        let error = Context::new(profile, &[0u8; 8], &[0u8; 14]).expect_err("refused");
        assert!(error.to_string().contains("master key"), "{error}");
        let error = Context::new(profile, &[0u8; 16], &[0u8; 4]).expect_err("refused");
        assert!(error.to_string().contains("master salt"), "{error}");
    }

    /// A header with CSRCs is longer, and encrypting from the wrong offset would encrypt part of
    /// the header and leave part of the audio in the clear. Under AEAD the same offset is the
    /// associated-data boundary (RFC 7714 §8.2), so reading it short would hand a CSRC to GCM as
    /// plaintext and hide four octets a relay has to see.
    #[test]
    fn a_header_with_contributing_sources_is_measured_correctly() {
        let mut packet = vec![0x82, 0x00, 0x00, 0x05]; // two CSRCs
        packet.extend_from_slice(&800u32.to_be_bytes());
        packet.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        packet.extend_from_slice(&1u32.to_be_bytes());
        packet.extend_from_slice(&2u32.to_be_bytes());
        packet.extend_from_slice(b"AUDIOAUDIO");

        assert_eq!(rtp_header_len(&packet), Some(20));

        for profile in EVERY_PROFILE {
            let (mut send, mut recv) = keyed(profile);
            let protected = send.protect(&packet).expect("protects");
            assert_eq!(
                &protected[..20],
                &packet[..20],
                "{profile:?}: the whole header is clear"
            );
            assert!(!protected.windows(5).any(|w| w == b"AUDIO"), "{profile:?}");
            assert_eq!(
                recv.unprotect(&protected).expect("unprotects"),
                packet,
                "{profile:?}"
            );
        }
    }

    #[test]
    fn a_truncated_packet_is_refused_rather_than_indexed() {
        for profile in EVERY_PROFILE {
            let (_, mut recv) = keyed(profile);
            assert!(
                matches!(recv.unprotect(&[0u8; 4]), Err(SrtpError::TooShort(4))),
                "{profile:?}"
            );
            assert!(
                matches!(recv.unprotect_rtcp(&[0u8; 4]), Err(SrtpError::TooShort(4))),
                "{profile:?}"
            );
        }
        assert_eq!(rtp_header_len(&[0u8; 8]), None);
    }

    /// Keys must not reach a log through a derived `Debug`.
    #[test]
    fn debug_output_does_not_leak_key_material() {
        for profile in EVERY_PROFILE {
            let (context, _) = keyed(profile);
            let printed = format!("{context:?}");
            assert!(printed.contains("Session { .. }"), "{printed}");
            assert!(!printed.contains('7'), "{printed}");
        }
    }
}
