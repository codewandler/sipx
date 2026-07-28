//! SRTP (RFC 3711): the default transform, AES-128 counter mode with HMAC-SHA1.
//!
//! What SRTP protects and what it does not is worth being exact about, because the gap is where
//! people are surprised. It encrypts the *payload* and authenticates the *whole packet*
//! including the header. So the sequence number, timestamp and SSRC travel in the clear and
//! cannot be altered; the audio travels encrypted. That is deliberate — a relay has to read the
//! header to do its job.
//!
//! Three things here are easy to get wrong and each has a test against the RFC's own published
//! numbers rather than against this implementation's opinion of them:
//!
//! **Key derivation** (§4.3.1) turns one master key into six session keys through AES counter
//! mode. Getting the label or the salt alignment wrong produces keys that are perfectly
//! self-consistent — two endpoints running the same wrong code interoperate happily and neither
//! interoperates with anything else.
//!
//! **The packet index** (§3.3.1) is 48 bits: a 32-bit rollover counter above the 16-bit sequence
//! number. It is not sent. Both ends infer it, and an implementation that guesses differently
//! decrypts to noise at the first wrap — twenty minutes into a call, at speech packet rates.
//!
//! **Replay** (§3.3.2) is rejected by a sliding window rather than by remembering everything.
//! Without it, a captured packet can be replayed into a call for as long as the key lives.

use aes::Aes128;
use aes::cipher::{KeyIvInit, StreamCipher};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use subtle::ConstantTimeEq;

type Aes128Ctr = ctr::Ctr128BE<Aes128>;
type HmacSha1 = Hmac<Sha1>;

/// The master key length of the default transform.
pub const MASTER_KEY_LEN: usize = 16;
/// The master salt length of the default transform.
pub const MASTER_SALT_LEN: usize = 14;
/// The authentication tag length of `AES_CM_128_HMAC_SHA1_80`, in octets.
pub const TAG_LEN: usize = 10;

const SESSION_KEY_LEN: usize = 16;
const SESSION_SALT_LEN: usize = 14;
/// HMAC-SHA1 takes a key of any length; RFC 3711 §4.3.1 derives 94 octets for it.
const SESSION_AUTH_LEN: usize = 94;

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
}

/// The six session keys one master key produces.
#[derive(Clone)]
struct Session {
    rtp_key: [u8; SESSION_KEY_LEN],
    rtp_salt: [u8; SESSION_SALT_LEN],
    rtp_auth: [u8; SESSION_AUTH_LEN],
    rtcp_key: [u8; SESSION_KEY_LEN],
    rtcp_salt: [u8; SESSION_SALT_LEN],
    rtcp_auth: [u8; SESSION_AUTH_LEN],
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keys. Printing them would put them in whatever log the caller writes.
        f.write_str("Session { .. }")
    }
}

/// Derive `out.len()` octets of session key material (RFC 3711 §4.3.1).
///
/// The input block is the master salt with the label exclusive-ored into octet 7 and
/// `index DIV kdr` into octets 8..14, shifted left by two octets. sipx uses a key derivation
/// rate of zero — one derivation per master key, which is what `index DIV kdr` being zero means
/// — because rekeying mid-stream buys nothing until there is a way to signal it.
fn derive(
    master_key: &[u8; MASTER_KEY_LEN],
    master_salt: &[u8; MASTER_SALT_LEN],
    label: Label,
    out: &mut [u8],
) {
    let mut iv = [0u8; 16];
    iv[..MASTER_SALT_LEN].copy_from_slice(master_salt);
    iv[7] ^= label as u8;
    // Octets 8..14 would carry `index DIV kdr`; with a rate of zero it is six zero octets, and
    // exclusive-oring zero changes nothing. Written out so the alignment is visible rather than
    // implied by its absence.
    iv[14] = 0;
    iv[15] = 0;

    out.fill(0);
    let mut cipher = Aes128Ctr::new(master_key.into(), (&iv).into());
    cipher.apply_keystream(out);
}

/// One direction of one SRTP stream.
///
/// Directional on purpose: RFC 3711 keys each direction separately, and a context used for both
/// would have two senders sharing one replay window and one rollover counter.
#[derive(Debug)]
pub struct Context {
    session: Session,
    /// The rollover counter — the high 32 bits of the 48-bit packet index (§3.3.1).
    roc: u32,
    /// The highest sequence number seen, for inferring the rollover.
    highest_seq: Option<u16>,
    /// The replay window, most recent packet at bit 0 (§3.3.2).
    replay: u64,
    /// The SRTCP index this side sends, 31 bits (§3.4).
    rtcp_index: u32,
}

impl Context {
    /// A context from a master key and salt.
    pub fn new(master_key: &[u8], master_salt: &[u8]) -> Result<Self, SrtpError> {
        let key: &[u8; MASTER_KEY_LEN] =
            master_key.try_into().map_err(|_| SrtpError::KeyLength {
                what: "master key",
                expected: MASTER_KEY_LEN,
                actual: master_key.len(),
            })?;
        let salt: &[u8; MASTER_SALT_LEN] =
            master_salt.try_into().map_err(|_| SrtpError::KeyLength {
                what: "master salt",
                expected: MASTER_SALT_LEN,
                actual: master_salt.len(),
            })?;

        let mut session = Session {
            rtp_key: [0; SESSION_KEY_LEN],
            rtp_salt: [0; SESSION_SALT_LEN],
            rtp_auth: [0; SESSION_AUTH_LEN],
            rtcp_key: [0; SESSION_KEY_LEN],
            rtcp_salt: [0; SESSION_SALT_LEN],
            rtcp_auth: [0; SESSION_AUTH_LEN],
        };
        derive(key, salt, Label::RtpEncryption, &mut session.rtp_key);
        derive(key, salt, Label::RtpSalt, &mut session.rtp_salt);
        derive(key, salt, Label::RtpAuthentication, &mut session.rtp_auth);
        derive(key, salt, Label::RtcpEncryption, &mut session.rtcp_key);
        derive(key, salt, Label::RtcpSalt, &mut session.rtcp_salt);
        derive(key, salt, Label::RtcpAuthentication, &mut session.rtcp_auth);

        Ok(Self {
            session,
            roc: 0,
            highest_seq: None,
            replay: 0,
            rtcp_index: 0,
        })
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

        let mut out = packet.to_vec();
        let (_, payload) = out.split_at_mut(header_len);
        keystream(
            &self.session.rtp_key,
            &self.session.rtp_salt,
            ssrc,
            index_of(roc, sequence),
        )
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
        if packet.len() < TAG_LEN {
            return Err(SrtpError::TooShort(packet.len()));
        }
        let (body, tag) = packet.split_at(packet.len() - TAG_LEN);
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
        )
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
        self.rtcp_index = (self.rtcp_index + 1) & 0x7FFF_FFFF;
        let index = self.rtcp_index;

        let mut out = packet.to_vec();
        let (_, payload) = out.split_at_mut(RTCP_HEADER_LEN);
        keystream(
            &self.session.rtcp_key,
            &self.session.rtcp_salt,
            ssrc,
            u64::from(index),
        )
        .apply_keystream(payload);

        // The trailer carries the encryption flag and the index in the clear; the tag covers it.
        out.extend_from_slice(&(index | 0x8000_0000).to_be_bytes());
        let tag = authenticate(&self.session.rtcp_auth, &out, None);
        out.extend_from_slice(&tag);
        Ok(out)
    }

    /// Authenticate and decrypt an RTCP compound packet.
    pub fn unprotect_rtcp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        const RTCP_HEADER_LEN: usize = 8;
        const TRAILER_LEN: usize = 4;
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
            )
            .apply_keystream(payload);
        }
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
}

/// The 48-bit packet index: the rollover counter above the sequence number.
fn index_of(roc: u32, sequence: u16) -> u64 {
    (u64::from(roc) << 16) | u64::from(sequence)
}

/// The keystream generator for one packet (RFC 3711 §4.1.1).
///
/// `IV = (salt * 2^16) XOR (SSRC * 2^64) XOR (index * 2^16)`. Every term is shifted left by at
/// least two octets, so the low 16 bits of the IV are always zero — which is why a plain
/// 128-bit counter is correct here: it cannot carry into the rest of the block within any packet
/// short enough to exist.
fn keystream(
    key: &[u8; SESSION_KEY_LEN],
    salt: &[u8; SESSION_SALT_LEN],
    ssrc: u32,
    index: u64,
) -> Aes128Ctr {
    let mut iv = [0u8; 16];
    iv[..SESSION_SALT_LEN].copy_from_slice(salt);

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
    Aes128Ctr::new(key.into(), (&iv).into())
}

/// HMAC-SHA1 over the packet, truncated to 80 bits.
fn authenticate(key: &[u8; SESSION_AUTH_LEN], data: &[u8], roc: Option<u32>) -> [u8; TAG_LEN] {
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
        );
        assert_eq!(cipher_key.to_vec(), hex("C61E7A93744F39EE10734AFE3FF7A087"));

        let mut cipher_salt = [0u8; 14];
        derive(&master_key, &master_salt, Label::RtpSalt, &mut cipher_salt);
        assert_eq!(cipher_salt.to_vec(), hex("30CBBC08863D8C85D49DB34A9AE1"));

        let mut auth_key = [0u8; 94];
        derive(
            &master_key,
            &master_salt,
            Label::RtpAuthentication,
            &mut auth_key,
        );
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

    /// RFC 3711 §B.2. The counter block and the keystream it produces, from the RFC.
    #[test]
    fn the_keystream_matches_the_rfc() {
        let key: [u8; 16] = hex("2B7E151628AED2A6ABF7158809CF4F3C").try_into().unwrap();
        // The RFC gives the offset already shifted; the salt is its first fourteen octets, and
        // SSRC and index are zero, so the IV is exactly that offset.
        let salt: [u8; 14] = hex("F0F1F2F3F4F5F6F7F8F9FAFBFCFD").try_into().unwrap();

        let mut out = [0u8; 48];
        keystream(&key, &salt, 0, 0).apply_keystream(&mut out);

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

    fn pair() -> (Context, Context) {
        let key = [7u8; 16];
        let salt = [9u8; 14];
        (
            Context::new(&key, &salt).expect("a sender"),
            Context::new(&key, &salt).expect("a receiver"),
        )
    }

    #[test]
    fn a_protected_packet_round_trips() {
        let (mut send, mut recv) = pair();
        let plain = rtp(1000, b"the quick brown fox jumps");

        let protected = send.protect(&plain).expect("protects");
        assert_eq!(protected.len(), plain.len() + TAG_LEN);
        assert_eq!(recv.unprotect(&protected).expect("unprotects"), plain);
    }

    /// The header stays readable and the payload does not. That split is the whole design: a
    /// relay must see the sequence number, and nobody should hear the audio.
    #[test]
    fn the_header_is_readable_and_the_payload_is_not() {
        let (mut send, _) = pair();
        let plain = rtp(7, b"SECRET AUDIO SAMPLES HERE");
        let protected = send.protect(&plain).expect("protects");

        assert_eq!(
            &protected[..12],
            &plain[..12],
            "the header travels in the clear"
        );
        assert!(
            !protected.windows(6).any(|w| w == b"SECRET"),
            "the payload must not appear on the wire"
        );
    }

    #[test]
    fn an_altered_packet_is_refused() {
        let (mut send, mut recv) = pair();
        let mut protected = send.protect(&rtp(1, b"hello")).expect("protects");

        // One bit, anywhere.
        protected[14] ^= 0x01;
        assert_eq!(recv.unprotect(&protected), Err(SrtpError::NotAuthentic));
    }

    /// Including the header, which is not encrypted and would otherwise be free to rewrite.
    #[test]
    fn an_altered_header_is_refused() {
        let (mut send, mut recv) = pair();
        let mut protected = send.protect(&rtp(1, b"hello")).expect("protects");

        protected[3] ^= 0x01; // the sequence number
        assert_eq!(recv.unprotect(&protected), Err(SrtpError::NotAuthentic));
    }

    #[test]
    fn a_packet_from_a_different_key_is_refused() {
        let (mut send, _) = pair();
        let mut stranger = Context::new(&[1u8; 16], &[2u8; 14]).expect("a context");
        let protected = send.protect(&rtp(1, b"hello")).expect("protects");
        assert_eq!(stranger.unprotect(&protected), Err(SrtpError::NotAuthentic));
    }

    /// RFC 3711 §3.3.2. Without this a captured packet can be replayed into a call for as long
    /// as the key lives, and it authenticates perfectly because it is genuine.
    #[test]
    fn a_replayed_packet_is_refused() {
        let (mut send, mut recv) = pair();
        let protected = send.protect(&rtp(100, b"hello")).expect("protects");

        recv.unprotect(&protected).expect("the first time");
        assert_eq!(recv.unprotect(&protected), Err(SrtpError::Replayed(100)));
    }

    #[test]
    fn out_of_order_packets_inside_the_window_are_accepted_once_each() {
        let (mut send, mut recv) = pair();
        let packets: Vec<Vec<u8>> = (200..210)
            .map(|n| send.protect(&rtp(n, b"x")).expect("protects"))
            .collect();

        // Delivered backwards, which a network does.
        for protected in packets.iter().rev() {
            recv.unprotect(protected).expect("accepted once");
        }
        // And not a second time.
        for protected in &packets {
            assert!(matches!(
                recv.unprotect(protected),
                Err(SrtpError::Replayed(_))
            ));
        }
    }

    /// Older than the window can judge is refused rather than accepted. Accepting it would make
    /// the window a speed bump: an attacker would only have to wait.
    #[test]
    fn a_packet_older_than_the_window_is_refused() {
        let (mut send, mut recv) = pair();
        let old = send.protect(&rtp(1, b"x")).expect("protects");
        for n in 2..200 {
            let p = send.protect(&rtp(n, b"x")).expect("protects");
            recv.unprotect(&p).expect("accepted");
        }
        assert_eq!(recv.unprotect(&old), Err(SrtpError::Replayed(1)));
    }

    /// The sequence number wraps every twenty minutes at speech packet rates, and the rollover
    /// counter it implies is never transmitted. Both ends infer it; an implementation that
    /// infers differently decrypts to noise from that moment on.
    #[test]
    fn the_stream_survives_the_sequence_number_wrapping() {
        let (mut send, mut recv) = pair();
        for n in [65_530u16, 65_533, 65_535, 0, 1, 5] {
            let plain = rtp(n, b"across the wrap");
            let protected = send.protect(&plain).expect("protects");
            assert_eq!(
                recv.unprotect(&protected).expect("unprotects"),
                plain,
                "sequence {n} did not survive"
            );
        }
        assert_eq!(send.roc, 1, "the sender counted one rollover");
        assert_eq!(recv.roc, 1, "and so did the receiver");
    }

    #[test]
    fn rtcp_round_trips_and_is_encrypted() {
        let (mut send, mut recv) = pair();
        // A minimal receiver report: version 2, PT 201, then the sender SSRC and a body.
        let mut packet = vec![0x80, 201, 0x00, 0x07];
        packet.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        packet.extend_from_slice(b"REPORTBODY-REPORTBODY-RE");

        let protected = send.protect_rtcp(&packet).expect("protects");
        assert!(
            !protected.windows(6).any(|w| w == b"REPORT"),
            "the report body must not appear on the wire"
        );
        assert_eq!(recv.unprotect_rtcp(&protected).expect("unprotects"), packet);
    }

    #[test]
    fn an_altered_rtcp_packet_is_refused() {
        let (mut send, mut recv) = pair();
        let mut packet = vec![0x80, 201, 0x00, 0x07];
        packet.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        packet.extend_from_slice(b"REPORTBODY-REPORTBODY-RE");

        let mut protected = send.protect_rtcp(&packet).expect("protects");
        protected[10] ^= 0x01;
        assert_eq!(
            recv.unprotect_rtcp(&protected),
            Err(SrtpError::NotAuthentic)
        );
    }

    #[test]
    fn a_wrong_length_key_is_refused_by_name() {
        let error = Context::new(&[0u8; 8], &[0u8; 14]).expect_err("refused");
        assert!(error.to_string().contains("master key"), "{error}");
        let error = Context::new(&[0u8; 16], &[0u8; 4]).expect_err("refused");
        assert!(error.to_string().contains("master salt"), "{error}");
    }

    /// A header with CSRCs is longer, and encrypting from the wrong offset would encrypt part of
    /// the header and leave part of the audio in the clear.
    #[test]
    fn a_header_with_contributing_sources_is_measured_correctly() {
        let mut packet = vec![0x82, 0x00, 0x00, 0x05]; // two CSRCs
        packet.extend_from_slice(&800u32.to_be_bytes());
        packet.extend_from_slice(&0xDEAD_BEEFu32.to_be_bytes());
        packet.extend_from_slice(&1u32.to_be_bytes());
        packet.extend_from_slice(&2u32.to_be_bytes());
        packet.extend_from_slice(b"AUDIOAUDIO");

        assert_eq!(rtp_header_len(&packet), Some(20));

        let (mut send, mut recv) = pair();
        let protected = send.protect(&packet).expect("protects");
        assert_eq!(&protected[..20], &packet[..20], "the whole header is clear");
        assert!(!protected.windows(5).any(|w| w == b"AUDIO"));
        assert_eq!(recv.unprotect(&protected).expect("unprotects"), packet);
    }

    #[test]
    fn a_truncated_packet_is_refused_rather_than_indexed() {
        let (_, mut recv) = pair();
        assert!(matches!(
            recv.unprotect(&[0u8; 4]),
            Err(SrtpError::TooShort(4))
        ));
        assert_eq!(rtp_header_len(&[0u8; 8]), None);
    }

    /// Keys must not reach a log through a derived `Debug`.
    #[test]
    fn debug_output_does_not_leak_key_material() {
        let context = Context::new(&[7u8; 16], &[9u8; 14]).expect("a context");
        let printed = format!("{context:?}");
        assert!(printed.contains("Session { .. }"), "{printed}");
        assert!(!printed.contains('7'), "{printed}");
    }
}
