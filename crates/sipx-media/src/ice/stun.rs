//! STUN as ICE uses it: connectivity checks over the media port (RFC 5389, RFC 8445 §7).
//!
//! [`sipx_transport::stun`] is a Binding *client* with no attributes and no credentials, and its
//! own header says so: "Anything that needs the full protocol (ICE, RFC 8445) needs a different
//! module, not more attributes bolted onto this one." This is that module. What it takes from
//! there it takes unchanged — RFC 5389 §6's header layout, the magic cookie, §7.3's `is_stun`
//! test and the cryptographically random [`TransactionId`], which is a security decision that
//! should exist once — and it adds nothing there.
//!
//! What it does not take is the `XOR-MAPPED-ADDRESS` reader, for two reasons: that decoder is
//! reachable only through `parse_reply`, which reads Binding Responses and discards every
//! attribute ICE needs, and a connectivity check has to *write* the attribute as well as read it.
//! Exposing the helper would have been extending the module the story was told not to extend.
//!
//! Two things in here are worth reading twice, because both fail silently.
//!
//! **The order of the two integrity values.** `MESSAGE-INTEGRITY` is HMAC-SHA1 over the message
//! with the header's length field temporarily set as though the message ended just after it
//! (RFC 5389 §15.4); `FINGERPRINT` is computed last, over everything including
//! `MESSAGE-INTEGRITY`, with the length field again adjusted to include *it* (§15.5), and its
//! value is the CRC-32 XOR `0x5354554e`. Both adjustments are easy to skip and neither is
//! visible in a self-test: the message round-trips through this module perfectly and every real
//! peer rejects it. [`the_rfc_5769_sample_request_is_produced_byte_for_byte`] is the guard,
//! because the IETF computed that tag and not this crate.
//!
//! **The direction of `USERNAME`.** See [`Peering`].
//!
//! Everything here is handed unauthenticated datagrams from whoever can reach the media port
//! (spec §11.3): no `unwrap`, no raw indexing, no length arithmetic that can wrap. A malformed
//! message is an [`Error`] and a dropped datagram.
//!
//! [`the_rfc_5769_sample_request_is_produced_byte_for_byte`]: self#tests
//! [spec §11]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sipx_sdp::ice::{Credentials, Priority};
use subtle::ConstantTimeEq as _;

use sipx_transport::stun::{HEADER_LEN, MAGIC_COOKIE, is_stun};
pub use sipx_transport::stun::{TransactionId, new_transaction_id};

type HmacSha1 = Hmac<Sha1>;

/// RFC 5389 §6: the only method ICE uses. Binding is the whole protocol here.
const METHOD_BINDING: u16 = 0x0001;

const ATTR_USERNAME: u16 = 0x0006;
const ATTR_MESSAGE_INTEGRITY: u16 = 0x0008;
const ATTR_ERROR_CODE: u16 = 0x0009;
const ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;
const ATTR_PRIORITY: u16 = 0x0024;
const ATTR_USE_CANDIDATE: u16 = 0x0025;
const ATTR_SOFTWARE: u16 = 0x8022;
const ATTR_FINGERPRINT: u16 = 0x8028;
const ATTR_ICE_CONTROLLED: u16 = 0x8029;
const ATTR_ICE_CONTROLLING: u16 = 0x802a;

const FAMILY_IPV4: u8 = 0x01;
const FAMILY_IPV6: u8 = 0x02;

/// RFC 5389 §15.5: the value on the wire is the CRC-32 XOR this constant, "to avoid a fingerprint
/// of the STUN message being confused with the CRC of an enclosing protocol".
const FINGERPRINT_XOR: u32 = 0x5354_554e;

/// The top 16 bits of [`MAGIC_COOKIE`], which is what §15.2 XORs a port with. Checked against
/// the cookie itself by `the_port_key_is_the_top_half_of_the_cookie`.
const PORT_KEY: u16 = 0x2112;

/// `MESSAGE-INTEGRITY` with its 4-byte attribute header: HMAC-SHA1 is 20 octets (RFC 5389 §15.4).
const INTEGRITY_ATTR_LEN: usize = 24;
/// `FINGERPRINT` with its 4-byte attribute header.
const FINGERPRINT_ATTR_LEN: usize = 8;

/// RFC 8445 §7.3.1.1's error response code: 487 Role Conflict.
pub const ROLE_CONFLICT: u16 = 487;

/// The byte an attribute value is padded to a 32-bit boundary with.
///
/// RFC 5389 §15 says the padding "may be any value", so this is a free choice — but it is a
/// choice that shows up in the wire bytes, because `MESSAGE-INTEGRITY` is an HMAC over the
/// padding as well as the value. RFC 5769's vectors pad with `0x20`: §2.1's `USERNAME` is nine
/// bytes followed by `20 20 20`, and §2.2's `SOFTWARE` is eleven followed by `20`. An encoder
/// that pads with zeroes cannot reproduce either published tag, and reproducing them is the only
/// evidence available that this encoder is right.
const PAD: u8 = 0x20;

/// What a datagram that was not a STUN message this profile understands turned out to be.
///
/// Every variant is a dropped datagram. None of them is a panic, and none of them moves any
/// state: an off-path attacker who can reach the media port can produce all of them at will.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Not STUN at all — the first two bits or the magic cookie say so (RFC 5389 §7.3).
    #[error("not a STUN message")]
    NotStun,
    /// The message ends inside its own header, an attribute, or the length it claims.
    #[error("the STUN message ends inside what it claims to contain")]
    Truncated,
    /// A method other than Binding. ICE uses no others (RFC 8445 §7).
    #[error("STUN method {0:#06x} is not Binding")]
    UnsupportedMethod(u16),
    /// A known attribute whose value is the wrong length, not UTF-8, or out of range.
    #[error("STUN attribute {0:#06x} is malformed")]
    MalformedAttribute(u16),
    /// `FINGERPRINT` is present and does not match the message (RFC 5389 §15.5).
    #[error("FINGERPRINT does not match the message")]
    Fingerprint,
    /// More bytes than the 16-bit length field can describe. Unreachable from any credential
    /// [`Credentials`] admits; it exists so that no encoding path has to panic or truncate.
    #[error("the message is longer than the STUN length field can describe")]
    TooLong,
}

/// RFC 5389 §6's message class: the two bits that say request from response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    /// A Binding Request. A connectivity check is one.
    Request,
    /// A Binding Indication. Draws no response; ICE's keepalive is one (RFC 8445 §11).
    Indication,
    /// A success response.
    Success,
    /// An error response.
    Error,
}

impl Class {
    const fn bits(self) -> u16 {
        match self {
            Self::Request => 0,
            Self::Indication => 1,
            Self::Success => 2,
            Self::Error => 3,
        }
    }

    const fn from_bits(bits: u16) -> Self {
        match bits {
            1 => Self::Indication,
            2 => Self::Success,
            3 => Self::Error,
            _ => Self::Request,
        }
    }
}

/// RFC 5389 §6: the 14-bit message type interleaves the method with the class, `C1` at bit 8 and
/// `C0` at bit 4, "for backwards compatibility with RFC 3489".
const fn message_type(class: Class, method: u16) -> u16 {
    let class = class.bits();
    (method & 0x000f)
        | ((method & 0x0070) << 1)
        | ((method & 0x0f80) << 2)
        | ((class & 0x1) << 4)
        | ((class & 0x2) << 7)
}

/// The inverse of [`message_type`].
const fn split_type(raw: u16) -> (Class, u16) {
    let class = ((raw & 0x0100) >> 7) | ((raw & 0x0010) >> 4);
    let method = (raw & 0x000f) | ((raw & 0x00e0) >> 1) | ((raw & 0x3e00) >> 2);
    (Class::from_bits(class), method)
}

/// One STUN attribute, in the profile spec §11.1 lists.
///
/// `MESSAGE-INTEGRITY` and `FINGERPRINT` are deliberately not variants. They are not attributes a
/// caller chooses to add: they are computed over whatever else is present and must come last, in
/// that order, so [`Message::encode`] appends them and nothing else can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attribute {
    /// `USERNAME` (RFC 5389 §15.3), in the direction [`Peering`] fixes.
    Username(String),
    /// `PRIORITY` (RFC 8445 §7.1.1) — and note §7.1.1's rule that this is *not* the candidate's
    /// own priority but the one it would have as a peer-reflexive candidate (spec §4).
    ///
    /// Carried as [`Priority`] so the RFC 8839 §5.1 range check applies to a value read off the
    /// wire as much as to one read out of SDP: spec §6.2 shows that an unchecked priority is what
    /// overflows the pair-priority arithmetic, and a check is a place a peer can put one.
    Priority(Priority),
    /// `USE-CANDIDATE` (RFC 8445 §7.1.2): a flag, so a zero-length value.
    UseCandidate,
    /// `ICE-CONTROLLED` (§7.1.3) carrying the sender's tiebreaker.
    IceControlled(u64),
    /// `ICE-CONTROLLING` (§7.1.3) carrying the sender's tiebreaker.
    IceControlling(u64),
    /// `ERROR-CODE` (RFC 5389 §15.6). 487 is the role conflict of §7.3.1.1.
    ErrorCode {
        /// The three-digit code, reassembled from §15.6's class and number.
        code: u16,
        /// The reason phrase, which is advisory and may be empty.
        reason: String,
    },
    /// `XOR-MAPPED-ADDRESS` (RFC 5389 §15.2): where the responder saw the request come from.
    XorMappedAddress(SocketAddr),
    /// `SOFTWARE` (RFC 5389 §15.10).
    ///
    /// sipx does not put one on its own checks — spec §11.1 lists what a check carries and this
    /// is not on it, and a version string on every check is bytes on the wire and a gift to a
    /// scanner. It is here because a peer may send one and because RFC 5769's vectors carry one,
    /// and a vector that cannot be encoded is a vector that cannot test the encoder.
    Software(String),
    /// An attribute this profile has no meaning for, kept so the caller can decide.
    ///
    /// RFC 5389 §7.3.1 wants a comprehension-required unknown attribute in a *request* answered
    /// with a 420; that is the agent's decision, not the codec's, so the bytes survive to it.
    Unknown {
        /// The attribute type.
        kind: u16,
        /// Its value, unpadded.
        value: Vec<u8>,
    },
}

/// Which role attribute a check carries, and — for the controlling agent only — whether it
/// nominates (RFC 8445 §7.1.2, §7.1.3).
///
/// The controlled arm has no `nominate`, and that is the point: §7.1.2 says "the controlled agent
/// MUST NOT include the USE-CANDIDATE attribute in a Binding request", and a shape that cannot
/// express it cannot send it by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoleAttribute {
    /// `ICE-CONTROLLING`.
    Controlling {
        /// The 64-bit value chosen per ICE session (§7.1.3), regenerated on a role switch.
        tiebreaker: u64,
        /// Whether this check nominates the pair (§8.1.1's regular nomination).
        nominate: bool,
    },
    /// `ICE-CONTROLLED`.
    Controlled {
        /// The 64-bit value chosen per ICE session (§7.1.3).
        tiebreaker: u64,
    },
}

impl RoleAttribute {
    const fn attribute(self) -> Attribute {
        match self {
            Self::Controlling { tiebreaker, .. } => Attribute::IceControlling(tiebreaker),
            Self::Controlled { tiebreaker } => Attribute::IceControlled(tiebreaker),
        }
    }

    /// The tiebreaker the attribute carries.
    #[must_use]
    pub const fn tiebreaker(self) -> u64 {
        match self {
            Self::Controlling { tiebreaker, .. } | Self::Controlled { tiebreaker } => tiebreaker,
        }
    }
}

/// Our short-term credentials and the peer's, and the two usernames they make (spec §11.2).
///
/// This type exists for one reason: the direction. A check sipx **sends** carries
/// `<peer-ufrag>:<our-ufrag>` and is keyed with the **peer's** password; a check sipx
/// **receives** carries `<our-ufrag>:<peer-ufrag>` and is keyed with **ours**, as is the response
/// sipx sends back to it. Reverse the two and every check sipx sends is rejected for a bad
/// credential and every check it receives goes unanswered — which on the wire is
/// indistinguishable from a blocked path, so it gets diagnosed as a network fault and not as the
/// four transposed characters it is. Naming the four values rather than formatting a username at
/// each call site is what makes the mistake reviewable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peering {
    local: Credentials,
    remote: Credentials,
}

impl Peering {
    /// Pair our credentials with the peer's.
    #[must_use]
    pub const fn new(local: Credentials, remote: Credentials) -> Self {
        Self { local, remote }
    }

    /// Our `a=ice-ufrag` and `a=ice-pwd`.
    #[must_use]
    pub const fn local(&self) -> &Credentials {
        &self.local
    }

    /// The peer's.
    #[must_use]
    pub const fn remote(&self) -> &Credentials {
        &self.remote
    }

    /// The `USERNAME` on a check sipx sends: `<peer-ufrag>:<our-ufrag>`.
    #[must_use]
    pub fn outbound_username(&self) -> String {
        format!("{}:{}", self.remote.ufrag(), self.local.ufrag())
    }

    /// The key for a check sipx sends, and for the response it expects back: the peer's password.
    #[must_use]
    pub fn outbound_key(&self) -> &str {
        self.remote.pwd()
    }

    /// The `USERNAME` a check sipx receives must carry: `<our-ufrag>:<peer-ufrag>`.
    #[must_use]
    pub fn inbound_username(&self) -> String {
        format!("{}:{}", self.local.ufrag(), self.remote.ufrag())
    }

    /// The key for a check sipx receives, and for the response sipx sends to it: our password.
    #[must_use]
    pub fn inbound_key(&self) -> &str {
        self.local.pwd()
    }
}

/// A STUN message: the header, the attributes, and — once decoded — what the two integrity
/// values said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    class: Class,
    transaction: TransactionId,
    attributes: Vec<Attribute>,
    integrity: Option<ReceivedIntegrity>,
    fingerprint: bool,
}

/// A `MESSAGE-INTEGRITY` read off the wire, with the bytes it claims to cover.
///
/// The prefix is kept rather than the offsets into the datagram because the key is not known
/// when the message is decoded — it depends on the `USERNAME` the message itself carries — so
/// verification happens later, and asking the caller to hand the original datagram back at that
/// point is an invitation to hand back a different one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceivedIntegrity {
    tag: [u8; 20],
    covered: Vec<u8>,
}

impl Message {
    /// An empty Binding message of this class.
    #[must_use]
    pub const fn new(class: Class, transaction: TransactionId) -> Self {
        Self {
            class,
            transaction,
            attributes: Vec::new(),
            integrity: None,
            fingerprint: false,
        }
    }

    /// Append an attribute.
    ///
    /// Order is preserved, and only two attributes have a normative position: the two
    /// [`Message::encode`] appends itself.
    #[must_use]
    pub fn with(mut self, attribute: Attribute) -> Self {
        self.attributes.push(attribute);
        self
    }

    /// The message class.
    #[must_use]
    pub const fn class(&self) -> Class {
        self.class
    }

    /// The transaction this message belongs to.
    #[must_use]
    pub const fn transaction(&self) -> TransactionId {
        self.transaction
    }

    /// The attributes, in the order they appeared.
    #[must_use]
    pub fn attributes(&self) -> &[Attribute] {
        &self.attributes
    }

    /// Encode, appending `MESSAGE-INTEGRITY` keyed with `key` when there is one, and then
    /// `FINGERPRINT`, in that order and always last (RFC 5389 §15.4, §15.5).
    ///
    /// `key` is `None` only for RFC 8445 §11's keepalive, which "MUST NOT utilize any
    /// authentication mechanism". `FINGERPRINT` is not optional: spec §11.1 puts it on every
    /// check, every response and every keepalive, and it is what lets the far end tell a check
    /// from RTP on the one port they share.
    pub fn encode(&self, key: Option<&str>) -> Result<Vec<u8>, Error> {
        let mut out = Vec::with_capacity(HEADER_LEN + 64);
        out.extend_from_slice(&message_type(self.class, METHOD_BINDING).to_be_bytes());
        // Patched below, once per integrity value, because both are computed over it.
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
        out.extend_from_slice(&self.transaction);
        for attribute in &self.attributes {
            attribute.encode_into(&mut out, &self.transaction)?;
        }
        if let Some(key) = key {
            // §15.4: the length must already count MESSAGE-INTEGRITY when the HMAC is taken.
            set_length(&mut out, INTEGRITY_ATTR_LEN)?;
            let tag = hmac_sha1(key.as_bytes(), &out);
            push_attribute(&mut out, ATTR_MESSAGE_INTEGRITY, &tag)?;
        }
        // §15.5, and the order is the whole point: the CRC covers MESSAGE-INTEGRITY, so it is
        // computed after it, with the length adjusted again to count FINGERPRINT itself.
        set_length(&mut out, FINGERPRINT_ATTR_LEN)?;
        let crc = crc32(&out) ^ FINGERPRINT_XOR;
        push_attribute(&mut out, ATTR_FINGERPRINT, &crc.to_be_bytes())?;
        Ok(out)
    }

    /// Read a datagram the media port's demultiplexer called STUN (RFC 5764 §5.1.2,
    /// [`crate::dtls::classify`]).
    ///
    /// Nothing here trusts the datagram. A `FINGERPRINT` that does not match is an error, per
    /// RFC 5389 §15.5, because a message whose CRC is wrong is not addressed to us however well
    /// formed it is; a `MESSAGE-INTEGRITY` cannot be checked yet, because which key applies
    /// depends on the `USERNAME` this message carries — see [`Message::verify_integrity`].
    pub fn decode(datagram: &[u8]) -> Result<Self, Error> {
        if !is_stun(datagram) {
            return Err(Error::NotStun);
        }
        let (class, method) = split_type(read_u16(datagram, 0).ok_or(Error::Truncated)?);
        if method != METHOD_BINDING {
            return Err(Error::UnsupportedMethod(method));
        }
        let transaction: TransactionId = datagram
            .get(8..HEADER_LEN)
            .and_then(|bytes| <[u8; 12]>::try_from(bytes).ok())
            .ok_or(Error::Truncated)?;
        // The stated length is authoritative; a datagram carrying extra bytes is not licence to
        // read them.
        let stated = usize::from(read_u16(datagram, 2).ok_or(Error::Truncated)?);
        let end = HEADER_LEN.checked_add(stated).ok_or(Error::Truncated)?;
        let body = datagram.get(HEADER_LEN..end).ok_or(Error::Truncated)?;

        let mut message = Self::new(class, transaction);
        let mut offset = 0usize;
        while offset < body.len() {
            let kind = read_u16(body, offset).ok_or(Error::Truncated)?;
            let length = usize::from(
                read_u16(body, offset.checked_add(2).ok_or(Error::Truncated)?)
                    .ok_or(Error::Truncated)?,
            );
            let start = offset.checked_add(4).ok_or(Error::Truncated)?;
            let value = start
                .checked_add(length)
                .and_then(|end| body.get(start..end))
                .ok_or(Error::Truncated)?;

            match kind {
                ATTR_MESSAGE_INTEGRITY if message.integrity.is_none() => {
                    message.integrity = Some(ReceivedIntegrity {
                        tag: <[u8; 20]>::try_from(value)
                            .map_err(|_| Error::MalformedAttribute(kind))?,
                        covered: covered_prefix(datagram, offset, INTEGRITY_ATTR_LEN)?,
                    });
                }
                ATTR_FINGERPRINT => {
                    if value.len() != 4 {
                        return Err(Error::MalformedAttribute(kind));
                    }
                    let stated_crc = read_u32(value, 0).ok_or(Error::MalformedAttribute(kind))?;
                    let prefix = covered_prefix(datagram, offset, FINGERPRINT_ATTR_LEN)?;
                    if crc32(&prefix) ^ FINGERPRINT_XOR != stated_crc {
                        return Err(Error::Fingerprint);
                    }
                    message.fingerprint = true;
                    // §15.5: FINGERPRINT "MUST be the last attribute in the message". Whatever
                    // follows it is not part of the message and is not read.
                    break;
                }
                _ if message.integrity.is_some() => {
                    // §15.4: with the exception of FINGERPRINT, "agents MUST ignore all other
                    // attributes that follow MESSAGE-INTEGRITY". They fall outside the tag, so
                    // anyone on the path can append them; honouring one would mean honouring an
                    // unauthenticated instruction.
                }
                _ => message
                    .attributes
                    .push(Attribute::decode(kind, value, &transaction)?),
            }

            // §15 aligns attributes on 32-bit boundaries. Skipping without the padding walks
            // into the middle of the next attribute.
            let padded = length.checked_add(3).ok_or(Error::Truncated)? & !3;
            offset = start.checked_add(padded).ok_or(Error::Truncated)?;
        }
        Ok(message)
    }

    /// Whether this message's `MESSAGE-INTEGRITY` was computed with `key`.
    ///
    /// `false` when there is none at all: an unauthenticated check is not a check that happens
    /// to verify, and spec §11.3 requires that it move no state.
    ///
    /// Which key to pass is the direction rule — [`Peering::inbound_key`] for a check that
    /// arrived, [`Peering::outbound_key`] for the response to one sipx sent.
    #[must_use]
    pub fn verify_integrity(&self, key: &str) -> bool {
        let Some(integrity) = self.integrity.as_ref() else {
            return false;
        };
        let computed = hmac_sha1(key.as_bytes(), &integrity.covered);
        // Constant time. An `==` here is a byte-at-a-time oracle for the tag, offered to anyone
        // who can reach the media port, and the tag is the only thing between an off-path
        // attacker and a state change (spec §11.2, §11.3) — the same reason
        // `sipx_sdp::fingerprint` and `sipx_rtp::srtp` compare this way.
        computed.ct_eq(&integrity.tag).into()
    }

    /// Whether a `MESSAGE-INTEGRITY` was present at all.
    #[must_use]
    pub const fn has_integrity(&self) -> bool {
        self.integrity.is_some()
    }

    /// Whether a `FINGERPRINT` was present. If it was, it matched — [`Message::decode`] rejects
    /// one that does not.
    #[must_use]
    pub const fn has_fingerprint(&self) -> bool {
        self.fingerprint
    }

    /// The `USERNAME`, if there is one.
    #[must_use]
    pub fn username(&self) -> Option<&str> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Username(name) => Some(name.as_str()),
                _ => None,
            })
    }

    /// The `PRIORITY` a check claims for the candidate the peer would learn from it (§7.1.1).
    #[must_use]
    pub fn priority(&self) -> Option<Priority> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::Priority(priority) => Some(*priority),
                _ => None,
            })
    }

    /// Whether `USE-CANDIDATE` is set (§7.1.2).
    #[must_use]
    pub fn use_candidate(&self) -> bool {
        self.attributes
            .iter()
            .any(|attribute| matches!(attribute, Attribute::UseCandidate))
    }

    /// The role attribute and its tiebreaker, if the peer sent one (§7.1.3).
    ///
    /// A peer that sends neither is not doing role signalling, which spec §7.3's last row says
    /// is not a conflict. `nominate` reports `USE-CANDIDATE` alongside `ICE-CONTROLLING`; a
    /// controlled peer that sets it anyway is violating §7.1.2, and the caller sees that as a
    /// `Controlled` role with [`Message::use_candidate`] true.
    #[must_use]
    pub fn role(&self) -> Option<RoleAttribute> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::IceControlling(tiebreaker) => Some(RoleAttribute::Controlling {
                    tiebreaker: *tiebreaker,
                    nominate: self.use_candidate(),
                }),
                Attribute::IceControlled(tiebreaker) => Some(RoleAttribute::Controlled {
                    tiebreaker: *tiebreaker,
                }),
                _ => None,
            })
    }

    /// The `ERROR-CODE`, if this is an error response. 487 is the role conflict of §7.3.1.1.
    #[must_use]
    pub fn error_code(&self) -> Option<u16> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::ErrorCode { code, .. } => Some(*code),
                _ => None,
            })
    }

    /// The `XOR-MAPPED-ADDRESS`, unobfuscated.
    #[must_use]
    pub fn mapped_address(&self) -> Option<SocketAddr> {
        self.attributes
            .iter()
            .find_map(|attribute| match attribute {
                Attribute::XorMappedAddress(address) => Some(*address),
                _ => None,
            })
    }
}

impl Attribute {
    fn encode_into(&self, out: &mut Vec<u8>, transaction: &TransactionId) -> Result<(), Error> {
        let (kind, value) = match self {
            Self::Username(name) => (ATTR_USERNAME, name.as_bytes().to_vec()),
            Self::Priority(priority) => (ATTR_PRIORITY, priority.get().to_be_bytes().to_vec()),
            Self::UseCandidate => (ATTR_USE_CANDIDATE, Vec::new()),
            Self::IceControlled(tiebreaker) => {
                (ATTR_ICE_CONTROLLED, tiebreaker.to_be_bytes().to_vec())
            }
            Self::IceControlling(tiebreaker) => {
                (ATTR_ICE_CONTROLLING, tiebreaker.to_be_bytes().to_vec())
            }
            Self::ErrorCode { code, reason } => (ATTR_ERROR_CODE, encode_error_code(*code, reason)),
            Self::XorMappedAddress(address) => (
                ATTR_XOR_MAPPED_ADDRESS,
                encode_xor_mapped(*address, transaction),
            ),
            Self::Software(text) => (ATTR_SOFTWARE, text.as_bytes().to_vec()),
            Self::Unknown { kind, value } => (*kind, value.clone()),
        };
        push_attribute(out, kind, &value)
    }

    fn decode(kind: u16, value: &[u8], transaction: &TransactionId) -> Result<Self, Error> {
        let malformed = || Error::MalformedAttribute(kind);
        Ok(match kind {
            ATTR_USERNAME => Self::Username(text(value, kind)?),
            ATTR_SOFTWARE => Self::Software(text(value, kind)?),
            ATTR_PRIORITY => {
                let raw = fixed_u32(value, kind)?;
                // §5.1 of RFC 8839 bounds a priority at 2^31 − 1, and spec §6.2 shows what an
                // unchecked one does to the pair-priority arithmetic. A check is a place a peer
                // can put a ten-digit number, so the bound is enforced here too.
                Self::Priority(Priority::new(raw).ok_or_else(malformed)?)
            }
            ATTR_USE_CANDIDATE => {
                if !value.is_empty() {
                    return Err(malformed());
                }
                Self::UseCandidate
            }
            ATTR_ICE_CONTROLLED => Self::IceControlled(fixed_u64(value, kind)?),
            ATTR_ICE_CONTROLLING => Self::IceControlling(fixed_u64(value, kind)?),
            ATTR_ERROR_CODE => {
                let class = u16::from(*value.get(2).ok_or_else(malformed)? & 0x07);
                let number = u16::from(*value.get(3).ok_or_else(malformed)?);
                let code = class
                    .checked_mul(100)
                    .and_then(|hundreds| hundreds.checked_add(number))
                    .ok_or_else(malformed)?;
                Self::ErrorCode {
                    code,
                    reason: text(value.get(4..).unwrap_or_default(), kind)?,
                }
            }
            ATTR_XOR_MAPPED_ADDRESS => {
                Self::XorMappedAddress(decode_xor_mapped(value, transaction).ok_or_else(malformed)?)
            }
            _ => Self::Unknown {
                kind,
                value: value.to_vec(),
            },
        })
    }
}

/// A UTF-8 attribute value. RFC 5389 §15.3 and §15.10 are both `SASLprep`-able strings, so
/// anything that is not UTF-8 is malformed rather than lossily converted.
fn text(value: &[u8], kind: u16) -> Result<String, Error> {
    std::str::from_utf8(value)
        .map(str::to_owned)
        .map_err(|_| Error::MalformedAttribute(kind))
}

/// A four-byte attribute value, rejecting one that is merely long enough.
fn fixed_u32(value: &[u8], kind: u16) -> Result<u32, Error> {
    <[u8; 4]>::try_from(value)
        .map(u32::from_be_bytes)
        .map_err(|_| Error::MalformedAttribute(kind))
}

/// An eight-byte attribute value.
fn fixed_u64(value: &[u8], kind: u16) -> Result<u64, Error> {
    <[u8; 8]>::try_from(value)
        .map(u64::from_be_bytes)
        .map_err(|_| Error::MalformedAttribute(kind))
}

/// A big-endian `u16` at `at`, or `None` if the bytes are not there.
fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    <[u8; 2]>::try_from(bytes.get(at..end)?)
        .ok()
        .map(u16::from_be_bytes)
}

/// A big-endian `u32` at `at`, or `None` if the bytes are not there.
fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    <[u8; 4]>::try_from(bytes.get(at..end)?)
        .ok()
        .map(u32::from_be_bytes)
}

/// The bytes an integrity value covers: the header and every attribute before the one at
/// `offset`, with the length field rewritten as though the message ended just after it.
///
/// This is the receiving half of [`set_length`], and it has to make the same adjustment for the
/// same reason (RFC 5389 §15.4, §15.5). Getting it wrong here rejects every well-formed peer.
fn covered_prefix(datagram: &[u8], offset: usize, attr_len: usize) -> Result<Vec<u8>, Error> {
    let end = HEADER_LEN.checked_add(offset).ok_or(Error::Truncated)?;
    let mut prefix = datagram.get(..end).ok_or(Error::Truncated)?.to_vec();
    let body = offset.checked_add(attr_len).ok_or(Error::Truncated)?;
    let length = u16::try_from(body).map_err(|_| Error::Truncated)?;
    prefix
        .get_mut(2..4)
        .ok_or(Error::Truncated)?
        .copy_from_slice(&length.to_be_bytes());
    Ok(prefix)
}

/// Undo the obfuscation §15.2 applies to an address.
fn decode_xor_mapped(value: &[u8], transaction: &TransactionId) -> Option<SocketAddr> {
    let family = *value.get(1)?;
    let port = read_u16(value, 2)? ^ PORT_KEY;
    match family {
        FAMILY_IPV4 => {
            let raw = read_u32(value, 4)?;
            let address = Ipv4Addr::from(raw ^ MAGIC_COOKIE);
            Some(SocketAddr::new(IpAddr::V4(address), port))
        }
        FAMILY_IPV6 => {
            let raw = <[u8; 16]>::try_from(value.get(4..20)?).ok()?;
            let key = xor_key(transaction);
            let mut octets = [0u8; 16];
            for (slot, (byte, k)) in octets.iter_mut().zip(raw.into_iter().zip(key)) {
                *slot = byte ^ k;
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port))
        }
        _ => None,
    }
}

/// HMAC-SHA1, the `MESSAGE-INTEGRITY` transform (RFC 5389 §15.4).
///
/// RFC 8445 cites RFC 5389 and not RFC 8489, so there is no SHA-256 variant to negotiate here.
/// The key for short-term credentials is `SASLprep(password)`; every character RFC 8839 §5.4's
/// `ice-char` admits is ASCII alphanumeric, `+` or `/`, all of which `SASLprep` leaves alone, so
/// the password's own bytes are the key.
fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut mac = <HmacSha1 as Mac>::new_from_slice(key)
        .unwrap_or_else(|_| unreachable!("HMAC accepts a key of any length"));
    mac.update(data);
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; 20];
    for (slot, byte) in tag.iter_mut().zip(full) {
        *slot = byte;
    }
    tag
}

/// CRC-32 as IEEE 802.3 defines it, which is the one RFC 5389 §15.5 means.
///
/// Bit-at-a-time rather than table-driven: a connectivity check is a hundred-odd bytes and one
/// leaves every 50 ms (spec §9), so a lookup table would cost a kilobyte of static data to save
/// microseconds nobody is waiting for. Pinned to the standard's own check value by
/// `the_crc_matches_the_published_check_value`, and to the IETF's by both RFC 5769 vectors.
fn crc32(data: &[u8]) -> u32 {
    /// The reversed representation of the IEEE polynomial.
    const POLYNOMIAL: u32 = 0xedb8_8320;
    let mut crc = 0xffff_ffff_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ POLYNOMIAL
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Write one attribute, padded to a 32-bit boundary (RFC 5389 §15).
fn push_attribute(out: &mut Vec<u8>, kind: u16, value: &[u8]) -> Result<(), Error> {
    let stated = u16::try_from(value.len()).map_err(|_| Error::TooLong)?;
    out.extend_from_slice(&kind.to_be_bytes());
    out.extend_from_slice(&stated.to_be_bytes());
    out.extend_from_slice(value);
    out.extend(std::iter::repeat_n(PAD, (4 - value.len() % 4) % 4));
    Ok(())
}

/// Rewrite the header's length field as though the message ended `extra` bytes past what has been
/// written.
///
/// RFC 5389 §15.4 and §15.5 both require exactly this before their value is computed: the length
/// must "point to the length of the message up to, and including, the attribute itself". Leave it
/// at the length of what has been written so far and both values are wrong — and wrong in a way
/// that round-trips through this module perfectly and that only a real peer rejects.
fn set_length(out: &mut [u8], extra: usize) -> Result<(), Error> {
    let body = out
        .len()
        .checked_sub(HEADER_LEN)
        .and_then(|written| written.checked_add(extra))
        .ok_or(Error::TooLong)?;
    let length = u16::try_from(body).map_err(|_| Error::TooLong)?;
    out.get_mut(2..4)
        .ok_or(Error::TooLong)?
        .copy_from_slice(&length.to_be_bytes());
    Ok(())
}

/// RFC 5389 §15.6: 21 reserved bits, a 3-bit class holding the hundreds digit, an 8-bit number
/// holding the rest, then the reason phrase.
fn encode_error_code(code: u16, reason: &str) -> Vec<u8> {
    let class = u8::try_from((code / 100) & 0x07).unwrap_or_default();
    let number = u8::try_from(code % 100).unwrap_or_default();
    let mut value = vec![0, 0, class, number];
    value.extend_from_slice(reason.as_bytes());
    value
}

/// The 16-byte key §15.2 XORs an IPv6 address with: the cookie followed by the transaction ID.
fn xor_key(transaction: &TransactionId) -> [u8; 16] {
    let mut key = [0u8; 16];
    let source = MAGIC_COOKIE
        .to_be_bytes()
        .into_iter()
        .chain(transaction.iter().copied());
    for (slot, byte) in key.iter_mut().zip(source) {
        *slot = byte;
    }
    key
}

/// Obfuscate an address the way RFC 5389 §15.2 requires.
///
/// §15.2's reason, not tidiness: some NATs rewrite anything in a payload that looks like an
/// address, and that would corrupt the very value the attribute exists to report.
fn encode_xor_mapped(address: SocketAddr, transaction: &TransactionId) -> Vec<u8> {
    let mut value = Vec::with_capacity(20);
    value.push(0);
    match address.ip() {
        IpAddr::V4(v4) => {
            value.push(FAMILY_IPV4);
            value.extend_from_slice(&(address.port() ^ PORT_KEY).to_be_bytes());
            let raw = u32::from_be_bytes(v4.octets());
            value.extend_from_slice(&(raw ^ MAGIC_COOKIE).to_be_bytes());
        }
        IpAddr::V6(v6) => {
            value.push(FAMILY_IPV6);
            value.extend_from_slice(&(address.port() ^ PORT_KEY).to_be_bytes());
            let key = xor_key(transaction);
            value.extend(v6.octets().into_iter().zip(key).map(|(byte, k)| byte ^ k));
        }
    }
    value
}

/// A connectivity check to send to the peer (RFC 8445 §7.1, spec §11.1).
///
/// The attribute order is RFC 5769 §2.1's own — `PRIORITY`, the role attribute, `USERNAME` — so
/// that what this produces lines up with the published vector attribute for attribute. Nothing
/// but the two integrity values has a normative position.
pub fn connectivity_check(
    transaction: TransactionId,
    peering: &Peering,
    priority: Priority,
    role: RoleAttribute,
) -> Result<Vec<u8>, Error> {
    let mut message = Message::new(Class::Request, transaction)
        .with(Attribute::Priority(priority))
        .with(role.attribute())
        .with(Attribute::Username(peering.outbound_username()));
    if matches!(role, RoleAttribute::Controlling { nominate: true, .. }) {
        message = message.with(Attribute::UseCandidate);
    }
    message.encode(Some(peering.outbound_key()))
}

/// The success response to a check sipx received (RFC 8445 §7.3.1.2, RFC 5389 §10.1.2).
///
/// `mapped` is the address the check arrived from — the peer-reflexive address the peer learns
/// itself by, so it must be the source of the datagram and not anything out of SDP.
///
/// No `USERNAME`: RFC 5389 §10.1.2 asks a short-term-credential response for `MESSAGE-INTEGRITY`
/// and nothing more, and RFC 5769 §2.2 — the IETF's own response to §2.1's request — carries
/// none. The key is **ours**, because it is our credential the peer's check was made with.
pub fn check_success(
    transaction: TransactionId,
    peering: &Peering,
    mapped: SocketAddr,
) -> Result<Vec<u8>, Error> {
    Message::new(Class::Success, transaction)
        .with(Attribute::XorMappedAddress(mapped))
        .encode(Some(peering.inbound_key()))
}

/// The 487 Role Conflict error response (RFC 8445 §7.3.1.1).
pub fn role_conflict(transaction: TransactionId, peering: &Peering) -> Result<Vec<u8>, Error> {
    Message::new(Class::Error, transaction)
        .with(Attribute::ErrorCode {
            code: ROLE_CONFLICT,
            reason: "Role Conflict".to_owned(),
        })
        .encode(Some(peering.inbound_key()))
}

/// A keepalive on a selected pair (RFC 8445 §11, RFC 8839 §6, spec §10).
///
/// A Binding **Indication** with `FINGERPRINT` and nothing else. §11 is unusually specific: it
/// "MUST NOT utilize any authentication mechanism", it SHOULD carry `FINGERPRINT` so the far end
/// can demultiplex it from media, and it SHOULD NOT carry anything more. An indication draws no
/// response, so this proves nothing about the path — it only holds the NAT binding open.
pub fn keepalive(transaction: TransactionId) -> Result<Vec<u8>, Error> {
    Message::new(Class::Indication, transaction).encode(None)
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

    /// Decode the hex listings RFC 5769 prints, so the test input is the RFC's bytes rather than
    /// something transcribed by hand into a different shape.
    fn hex(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).expect("a hex byte"))
            .collect()
    }

    /// RFC 5769 §2.1, the sample request — which §2.2 of the ICE spec notes is itself an ICE
    /// connectivity check.
    const SAMPLE_REQUEST: &str = "
        00 01 00 58  21 12 a4 42  b7 e7 a7 01  bc 34 d6 86
        fa 87 df ae  80 22 00 10  53 54 55 4e  20 74 65 73
        74 20 63 6c  69 65 6e 74  00 24 00 04  6e 00 01 ff
        80 29 00 08  93 2f f9 b1  51 26 3b 36  00 06 00 09
        65 76 74 6a  3a 68 36 76  59 20 20 20  00 08 00 14
        9a ea a7 0c  bf d8 cb 56  78 1e f2 b5  b2 d3 f2 49
        c1 b5 71 a2  80 28 00 04  e5 7a 3b cf";

    const SAMPLE_ID: TransactionId = [
        0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
    ];

    /// RFC 5769 §2.1's stated parameters.
    const SAMPLE_SOFTWARE: &str = "STUN test client";
    const SAMPLE_UFRAG_SENDER: &str = "h6vY";
    const SAMPLE_UFRAG_RECEIVER: &str = "evtj";
    const SAMPLE_PASSWORD: &str = "VOkJxbRl1RmTxUk/WvJxBt";
    const SAMPLE_PRIORITY: u32 = 0x6e00_01ff;
    const SAMPLE_TIEBREAKER: u64 = 0x932f_f9b1_5126_3b36;

    /// A password for the side of the vector RFC 5769 does not state one for. Never keys
    /// anything the vector asserts on; it is here because [`Credentials`] will not hold a
    /// half-populated pair.
    const OTHER_PASSWORD: &str = "aPasswordTheRfcNeverStates";

    /// The peering as the *sender* of §2.1's request sees it: its own ufrag is `h6vY`, the peer's
    /// is `evtj`, and the peer's password is the one the RFC states — because `USERNAME` is
    /// `<peer-ufrag>:<our-ufrag>` and the key is the peer's password.
    fn sample_sender() -> Peering {
        Peering::new(
            Credentials::new(SAMPLE_UFRAG_SENDER, OTHER_PASSWORD).expect("valid credentials"),
            Credentials::received(SAMPLE_UFRAG_RECEIVER, SAMPLE_PASSWORD)
                .expect("valid credentials"),
        )
    }

    /// The same two agents from the other side — the one that receives §2.1's request and sends
    /// §2.2's response. `evtj` is now ours and `VOkJxbRl1RmTxUk/WvJxBt` is our password, which is
    /// what makes the RFC's two vectors a test of both directions rather than one.
    fn sample_receiver() -> Peering {
        Peering::new(
            Credentials::new(SAMPLE_UFRAG_RECEIVER, SAMPLE_PASSWORD).expect("valid credentials"),
            Credentials::received(SAMPLE_UFRAG_SENDER, OTHER_PASSWORD).expect("valid credentials"),
        )
    }

    fn sample_priority() -> Priority {
        Priority::new(SAMPLE_PRIORITY).expect("in range")
    }

    /// Offsets into [`SAMPLE_REQUEST`]. Stated rather than searched for, so that a test asserting
    /// on a slice of the vector is asserting on the part of it that it names.
    const REQUEST_ICE_ATTRIBUTES: std::ops::Range<usize> = 40..76;
    const REQUEST_INTEGRITY_TAG: std::ops::Range<usize> = 80..100;
    /// Where `MESSAGE-INTEGRITY` starts, counted from the end of the 20-byte header.
    const REQUEST_INTEGRITY_BODY_OFFSET: u16 = 56;

    /// RFC 5769 §2.1's sample request, produced by this encoder from the parameters the RFC
    /// states and compared byte for byte.
    ///
    /// This is the assertion the whole module is built around, and it is the only one that is not
    /// self-confirming: the IETF computed `MESSAGE-INTEGRITY` and `FINGERPRINT` here, so matching
    /// them proves the length adjustments, the ordering, the attribute padding and the direction
    /// of `USERNAME` at once. A decoder tested against this encoder would prove none of it.
    #[test]
    fn a_connectivity_check_encodes_to_the_rfc_5769_sample_request() {
        let peering = sample_sender();

        let message = Message::new(Class::Request, SAMPLE_ID)
            .with(Attribute::Software(SAMPLE_SOFTWARE.to_owned()))
            .with(Attribute::Priority(sample_priority()))
            .with(Attribute::IceControlled(SAMPLE_TIEBREAKER))
            .with(Attribute::Username(peering.outbound_username()));

        assert_eq!(
            message
                .encode(Some(peering.outbound_key()))
                .expect("encodes"),
            hex(SAMPLE_REQUEST),
            "the encoder does not reproduce RFC 5769 §2.1"
        );

        // The vector carries a SOFTWARE attribute and a check sipx sends does not (spec §11.1),
        // so what ties the profile helper to the same bytes is the run of ICE attributes: the
        // vector's PRIORITY, ICE-CONTROLLED and USERNAME, in that order and with that padding.
        let check = connectivity_check(
            SAMPLE_ID,
            &peering,
            sample_priority(),
            RoleAttribute::Controlled {
                tiebreaker: SAMPLE_TIEBREAKER,
            },
        )
        .expect("encodes");
        let vector = hex(SAMPLE_REQUEST);
        assert_eq!(
            &check[HEADER_LEN..HEADER_LEN + REQUEST_ICE_ATTRIBUTES.len()],
            &vector[REQUEST_ICE_ATTRIBUTES],
        );
    }

    /// RFC 5769 §2.2, the sample IPv4 success response — the answer to §2.1's request, keyed with
    /// the same password and so, in ICE's terms, keyed with *ours*.
    ///
    /// A second published vector, and the only one that exercises `XOR-MAPPED-ADDRESS` in the
    /// encoding direction. Its `SOFTWARE` value is eleven bytes, so it also pins the padding byte
    /// a second time and independently.
    #[test]
    fn a_success_response_encodes_to_the_rfc_5769_sample_response() {
        const SAMPLE_RESPONSE: &str = "
            01 01 00 3c  21 12 a4 42  b7 e7 a7 01  bc 34 d6 86
            fa 87 df ae  80 22 00 0b  74 65 73 74  20 76 65 63
            74 6f 72 20  00 20 00 08  00 01 a1 47  e1 12 a6 43
            00 08 00 14  2b 91 f5 99  fd 9e 90 c3  8c 74 89 f9
            2a f9 ba 53  f0 6b e7 d7  80 28 00 04  c0 7d 4c 96";
        /// The address the RFC states its own vector carries. Not computed here — the point of a
        /// published vector is that the expected value comes from the publisher.
        const MAPPED: &str = "192.0.2.1:32853";
        const RESPONSE_MAPPED_ADDRESS: std::ops::Range<usize> = 36..48;

        let peering = sample_receiver();
        let mapped: SocketAddr = MAPPED.parse().expect("valid");

        let message = Message::new(Class::Success, SAMPLE_ID)
            .with(Attribute::Software("test vector".to_owned()))
            .with(Attribute::XorMappedAddress(mapped));

        assert_eq!(
            message
                .encode(Some(peering.inbound_key()))
                .expect("encodes"),
            hex(SAMPLE_RESPONSE),
            "the encoder does not reproduce RFC 5769 §2.2"
        );

        let response = check_success(SAMPLE_ID, &peering, mapped).expect("encodes");
        let vector = hex(SAMPLE_RESPONSE);
        assert_eq!(
            &response[HEADER_LEN..HEADER_LEN + RESPONSE_MAPPED_ADDRESS.len()],
            &vector[RESPONSE_MAPPED_ADDRESS],
        );
    }

    /// RFC 5389 §15.4's length adjustment, shown to be the difference between the IETF's tag and
    /// a wrong one.
    ///
    /// The vector's own length field says 88 — the whole message, `FINGERPRINT` included. The
    /// HMAC is taken over 80: everything up to and including `MESSAGE-INTEGRITY`. Skip the
    /// adjustment and the message still round-trips through this module; it is only a real peer
    /// that rejects it, which is why the assertion is on the published tag.
    #[test]
    fn the_integrity_is_taken_over_the_adjusted_length_and_not_the_real_one() {
        let vector = hex(SAMPLE_REQUEST);
        assert_eq!(vector.len(), 108, "RFC 5769 §2.1 is 108 bytes");
        assert_eq!(&vector[2..4], &[0x00, 0x58], "the vector's real length, 88");

        let mut covered = vector[..76].to_vec();
        let real = hmac_sha1(SAMPLE_PASSWORD.as_bytes(), &covered);
        assert_ne!(
            &real[..],
            &vector[REQUEST_INTEGRITY_TAG],
            "the real length must not produce the published tag"
        );

        let adjusted = REQUEST_INTEGRITY_BODY_OFFSET + 24;
        assert_eq!(adjusted, 80);
        covered[2..4].copy_from_slice(&adjusted.to_be_bytes());
        assert_eq!(
            &hmac_sha1(SAMPLE_PASSWORD.as_bytes(), &covered)[..],
            &vector[REQUEST_INTEGRITY_TAG],
            "the adjusted length must"
        );
    }

    /// RFC 5389 §15.5: `FINGERPRINT` is computed last and is last, over a message that already
    /// contains `MESSAGE-INTEGRITY`. Swap the two and neither value is right.
    #[test]
    fn the_integrity_comes_before_the_fingerprint_and_both_come_last() {
        let peering = sample_sender();
        let check = connectivity_check(
            SAMPLE_ID,
            &peering,
            sample_priority(),
            RoleAttribute::Controlling {
                tiebreaker: SAMPLE_TIEBREAKER,
                nominate: true,
            },
        )
        .expect("encodes");

        let integrity = check.len() - INTEGRITY_ATTR_LEN - FINGERPRINT_ATTR_LEN;
        let fingerprint = check.len() - FINGERPRINT_ATTR_LEN;
        assert_eq!(
            read_u16(&check, integrity),
            Some(ATTR_MESSAGE_INTEGRITY),
            "MESSAGE-INTEGRITY is second to last"
        );
        assert_eq!(
            read_u16(&check, fingerprint),
            Some(ATTR_FINGERPRINT),
            "FINGERPRINT is last"
        );

        // The CRC covers MESSAGE-INTEGRITY, so recomputing it over the message as sent must
        // reproduce the value on the wire.
        let expected = crc32(&check[..fingerprint]) ^ FINGERPRINT_XOR;
        assert_eq!(read_u32(&check, fingerprint + 4), Some(expected));

        let decoded = Message::decode(&check).expect("our own check decodes");
        assert!(decoded.has_integrity() && decoded.has_fingerprint());
        assert!(decoded.verify_integrity(peering.outbound_key()));
    }

    /// The direction rule of spec §11.2, against the IETF's own bytes in both directions.
    ///
    /// The two peerings are the same pair of agents seen from either end. `evtj` sends nothing in
    /// this test; it only reads. Reverse either accessor and one of these four assertions fails —
    /// which is the point, because on the wire the reversal looks like a blocked path.
    #[test]
    fn the_username_and_key_of_a_check_depend_on_which_way_it_travels() {
        let sender = sample_sender();
        let receiver = sample_receiver();

        assert_eq!(sender.outbound_username(), "evtj:h6vY");
        assert_eq!(sender.outbound_key(), SAMPLE_PASSWORD);
        assert_eq!(receiver.inbound_username(), "evtj:h6vY");
        assert_eq!(receiver.inbound_key(), SAMPLE_PASSWORD);
        assert_eq!(sender.inbound_username(), "h6vY:evtj");
        assert_eq!(sender.inbound_key(), OTHER_PASSWORD);
        assert_eq!(receiver.outbound_username(), "h6vY:evtj");
        assert_eq!(receiver.outbound_key(), OTHER_PASSWORD);

        // What the receiver of RFC 5769 §2.1's request must conclude about it.
        let arrived = Message::decode(&hex(SAMPLE_REQUEST)).expect("decodes");
        assert_eq!(
            arrived.username(),
            Some(receiver.inbound_username()).as_deref()
        );
        assert!(
            arrived.verify_integrity(receiver.inbound_key()),
            "a check that arrived is keyed with our password"
        );
        assert!(
            !arrived.verify_integrity(receiver.outbound_key()),
            "keying an inbound check with the peer's password answers nothing and looks like a \
             network fault"
        );
    }

    /// Every attribute spec §11.1 lists, out and back (RFC 5389 §15, RFC 8445 §7.1).
    #[test]
    fn every_profile_attribute_encodes_as_well_as_decodes() {
        let attributes = vec![
            Attribute::Username("evtj:h6vY".to_owned()),
            Attribute::Priority(sample_priority()),
            Attribute::UseCandidate,
            Attribute::IceControlling(SAMPLE_TIEBREAKER),
            Attribute::ErrorCode {
                code: ROLE_CONFLICT,
                reason: "Role Conflict".to_owned(),
            },
            Attribute::XorMappedAddress("192.0.2.1:32853".parse().expect("valid")),
            Attribute::Software(SAMPLE_SOFTWARE.to_owned()),
            Attribute::Unknown {
                kind: 0x8050,
                value: vec![1, 2, 3],
            },
        ];
        let message = attributes
            .iter()
            .cloned()
            .fold(Message::new(Class::Request, SAMPLE_ID), Message::with);
        let bytes = message.encode(Some(SAMPLE_PASSWORD)).expect("encodes");
        let decoded = Message::decode(&bytes).expect("decodes");

        assert_eq!(decoded.attributes(), attributes.as_slice());
        assert_eq!(decoded.class(), Class::Request);
        assert_eq!(decoded.transaction(), SAMPLE_ID);
        assert_eq!(decoded.error_code(), Some(487));
        assert_eq!(decoded.priority(), Some(sample_priority()));
        assert_eq!(
            decoded.mapped_address(),
            Some("192.0.2.1:32853".parse().expect("valid"))
        );
        assert_eq!(
            decoded.role(),
            Some(RoleAttribute::Controlling {
                tiebreaker: SAMPLE_TIEBREAKER,
                nominate: true,
            })
        );

        // ICE-CONTROLLED is the other half of §7.1.3, and the same message cannot carry both.
        let controlled = Message::new(Class::Request, SAMPLE_ID)
            .with(Attribute::IceControlled(SAMPLE_TIEBREAKER))
            .encode(None)
            .expect("encodes");
        assert_eq!(
            Message::decode(&controlled).expect("decodes").role(),
            Some(RoleAttribute::Controlled {
                tiebreaker: SAMPLE_TIEBREAKER
            })
        );
    }

    /// `XOR-MAPPED-ADDRESS` for IPv6 extends the key with the transaction ID (§15.2). RFC 5769
    /// §2.3's IPv6 response is for a different transaction ID than §2.1's, so this is a
    /// round-trip rather than a vector — the encoding direction is pinned by §2.2 above.
    #[test]
    fn an_ipv6_mapped_address_round_trips() {
        let address: SocketAddr = "[2001:db8::1]:32853".parse().expect("valid");
        let value = encode_xor_mapped(address, &SAMPLE_ID);
        assert_eq!(value.len(), 20);
        assert_ne!(&value[4..20], &[0u8; 16], "the address must be obfuscated");
        assert_eq!(decode_xor_mapped(&value, &SAMPLE_ID), Some(address));
        assert_ne!(
            decode_xor_mapped(&value, &[0u8; 12]),
            Some(address),
            "the transaction ID is part of the key"
        );
    }

    /// `USE-CANDIDATE` is a flag: RFC 8445 §7.1.2 gives it no value at all.
    #[test]
    fn use_candidate_is_a_zero_length_flag_only_the_controlling_agent_can_send() {
        let peering = sample_sender();
        let nominating = connectivity_check(
            SAMPLE_ID,
            &peering,
            sample_priority(),
            RoleAttribute::Controlling {
                tiebreaker: SAMPLE_TIEBREAKER,
                nominate: true,
            },
        )
        .expect("encodes");
        let decoded = Message::decode(&nominating).expect("decodes");
        assert!(decoded.use_candidate());
        assert!(decoded.attributes().contains(&Attribute::UseCandidate));

        // On the wire the flag is four bytes of attribute header and no value.
        let flag = Message::new(Class::Request, SAMPLE_ID)
            .with(Attribute::UseCandidate)
            .encode(None)
            .expect("encodes");
        assert_eq!(read_u16(&flag, HEADER_LEN), Some(ATTR_USE_CANDIDATE));
        assert_eq!(read_u16(&flag, HEADER_LEN + 2), Some(0), "zero length");

        // §7.1.2: the controlled agent MUST NOT send it, which `RoleAttribute::Controlled` has
        // no way to express.
        let controlled = connectivity_check(
            SAMPLE_ID,
            &peering,
            sample_priority(),
            RoleAttribute::Controlled {
                tiebreaker: SAMPLE_TIEBREAKER,
            },
        )
        .expect("encodes");
        assert!(
            !Message::decode(&controlled)
                .expect("decodes")
                .use_candidate()
        );
    }

    /// RFC 8445 §7.3.1.1's answer to a role conflict, and RFC 5389 §15.6's split of the code.
    #[test]
    fn a_role_conflict_is_a_487_error_response() {
        let peering = sample_receiver();
        let bytes = role_conflict(SAMPLE_ID, &peering).expect("encodes");
        let decoded = Message::decode(&bytes).expect("decodes");

        assert_eq!(decoded.class(), Class::Error);
        assert_eq!(decoded.error_code(), Some(ROLE_CONFLICT));
        assert!(
            decoded.verify_integrity(peering.inbound_key()),
            "a response to a check that arrived is keyed with our password"
        );
        // §15.6 puts the hundreds digit in a 3-bit class and the rest in a byte: 487 is 4 then 87.
        assert_eq!(&bytes[HEADER_LEN + 4..HEADER_LEN + 8], &[0, 0, 4, 87]);
    }

    /// Spec §10 and RFC 8445 §11: a keepalive is a Binding Indication with `FINGERPRINT`, no
    /// credential, and nothing else.
    #[test]
    fn a_keepalive_is_a_binding_indication_with_a_fingerprint_and_nothing_else() {
        let bytes = keepalive(SAMPLE_ID).expect("encodes");
        assert_eq!(
            bytes.len(),
            HEADER_LEN + FINGERPRINT_ATTR_LEN,
            "a header and one attribute"
        );
        assert_eq!(&bytes[0..2], &[0x00, 0x11], "Binding Indication");

        let decoded = Message::decode(&bytes).expect("decodes");
        assert_eq!(decoded.class(), Class::Indication);
        assert!(decoded.attributes().is_empty(), "§11: nothing else");
        assert!(decoded.has_fingerprint(), "§11 SHOULD, for demultiplexing");
        assert!(
            !decoded.has_integrity(),
            "§11: MUST NOT utilize any authentication mechanism"
        );
        assert!(
            !decoded.verify_integrity(SAMPLE_PASSWORD),
            "an unauthenticated message never verifies against any key"
        );
    }

    /// RFC 5389 §15.5: a message whose `FINGERPRINT` is wrong is not addressed to us, however
    /// well formed the rest of it is.
    #[test]
    fn a_fingerprint_that_does_not_match_is_a_dropped_datagram() {
        let mut bytes = hex(SAMPLE_REQUEST);
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert_eq!(Message::decode(&bytes), Err(Error::Fingerprint));
    }

    /// RFC 5389 §15.4: with the exception of `FINGERPRINT`, everything after `MESSAGE-INTEGRITY`
    /// is ignored. It falls outside the tag, so anyone on the path can put it there.
    #[test]
    fn an_attribute_appended_after_message_integrity_is_ignored() {
        let peering = sample_sender();
        let honest = connectivity_check(
            SAMPLE_ID,
            &peering,
            sample_priority(),
            RoleAttribute::Controlled {
                tiebreaker: SAMPLE_TIEBREAKER,
            },
        )
        .expect("encodes");

        // Splice USE-CANDIDATE in between MESSAGE-INTEGRITY and FINGERPRINT, and repair the
        // length and the CRC so that only the tag can tell.
        let split = honest.len() - FINGERPRINT_ATTR_LEN;
        let mut forged = honest[..split].to_vec();
        push_attribute(&mut forged, ATTR_USE_CANDIDATE, &[]).expect("encodes");
        set_length(&mut forged, FINGERPRINT_ATTR_LEN).expect("fits");
        let crc = crc32(&forged) ^ FINGERPRINT_XOR;
        push_attribute(&mut forged, ATTR_FINGERPRINT, &crc.to_be_bytes()).expect("encodes");

        let decoded = Message::decode(&forged).expect("decodes");
        assert!(decoded.has_fingerprint(), "the CRC was repaired");
        assert!(
            decoded.verify_integrity(peering.outbound_key()),
            "the bytes the tag covers are untouched"
        );
        assert!(
            !decoded.use_candidate(),
            "an unauthenticated USE-CANDIDATE must not nominate a pair"
        );
    }

    /// Spec §6.2: an unchecked priority is what overflows the pair-priority arithmetic, and a
    /// connectivity check is a place a peer can put one.
    #[test]
    fn a_priority_outside_rfc_8839s_range_is_rejected() {
        let mut bytes = Message::new(Class::Request, SAMPLE_ID)
            .with(Attribute::Priority(Priority::MAX))
            .encode(None)
            .expect("encodes");
        assert!(Message::decode(&bytes).is_ok());

        // Raise it one past 2^31 − 1 and repair the CRC, so it is the range check that rejects it.
        bytes[HEADER_LEN + 4..HEADER_LEN + 8].copy_from_slice(&0x8000_0000_u32.to_be_bytes());
        let split = bytes.len() - FINGERPRINT_ATTR_LEN;
        let crc = crc32(&bytes[..split]) ^ FINGERPRINT_XOR;
        bytes[split + 4..].copy_from_slice(&crc.to_be_bytes());
        assert_eq!(
            Message::decode(&bytes),
            Err(Error::MalformedAttribute(ATTR_PRIORITY))
        );
    }

    /// Acceptance's live invariant: this parser is handed unauthenticated datagrams by anyone who
    /// can reach the media port. Every prefix of a message the RFC itself publishes is a
    /// plausible truncation, and none of them may panic.
    #[test]
    fn no_prefix_of_a_real_message_panics() {
        let bytes = hex(SAMPLE_REQUEST);
        for length in 0..=bytes.len() {
            let _ = Message::decode(&bytes[..length]);
        }
    }

    /// Nor may any single-byte corruption of one — which reaches every length field, every
    /// attribute type and both integrity values.
    #[test]
    fn no_single_byte_corruption_of_a_real_message_panics() {
        let bytes = hex(SAMPLE_REQUEST);
        for index in 0..bytes.len() {
            for pattern in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                let mut corrupted = bytes.clone();
                corrupted[index] = pattern;
                let _ = Message::decode(&corrupted);
            }
        }
    }

    /// And nor may arbitrary bytes behind a well-formed header, which is what an attacker sends.
    ///
    /// Deterministic rather than randomised: a fuzz finding that cannot be reproduced from the
    /// test file is a fuzz finding nobody fixes.
    #[test]
    fn arbitrary_bytes_behind_a_valid_stun_header_never_panic() {
        let mut seed = 0x5354_554e_u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            u8::try_from(seed >> 56).unwrap_or_default()
        };
        for _ in 0..2_000 {
            let body_len = usize::from(next()) * 2;
            let mut datagram = Vec::with_capacity(HEADER_LEN + body_len);
            datagram.extend_from_slice(&[0x00, 0x01]);
            datagram.extend_from_slice(&u16::try_from(body_len).unwrap_or_default().to_be_bytes());
            datagram.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
            datagram.extend_from_slice(&SAMPLE_ID);
            datagram.extend((0..body_len).map(|_| next()));
            let _ = Message::decode(&datagram);
        }
    }

    /// A length field that claims more than arrived is the classic way into a panic.
    #[test]
    fn a_length_field_past_the_end_is_an_error() {
        let mut bytes = hex(SAMPLE_REQUEST);
        bytes[2..4].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(Message::decode(&bytes), Err(Error::Truncated));

        // The same one attribute down: an attribute claiming to run past the body.
        let mut bytes = hex(SAMPLE_REQUEST);
        bytes[22..24].copy_from_slice(&u16::MAX.to_be_bytes());
        assert_eq!(Message::decode(&bytes), Err(Error::Truncated));
    }

    /// Anything that is not STUN, and anything that is STUN but not Binding, is refused before a
    /// single attribute is read.
    #[test]
    fn a_datagram_that_is_not_a_binding_message_is_refused() {
        assert_eq!(Message::decode(&[]), Err(Error::NotStun));
        assert_eq!(
            Message::decode(b"INVITE sip:bob@example.com SIP/2.0\r\n\r\n"),
            Err(Error::NotStun)
        );

        let mut allocate = keepalive(SAMPLE_ID).expect("encodes");
        allocate[0..2].copy_from_slice(&0x0003_u16.to_be_bytes());
        assert_eq!(Message::decode(&allocate), Err(Error::UnsupportedMethod(3)));
    }

    /// Our own checks must pass the §7.3 test the transport crate already implements — that is
    /// what makes them demultiplexable at the far end.
    #[test]
    fn what_this_module_encodes_is_stun_by_the_transport_crates_own_test() {
        let peering = sample_sender();
        for bytes in [
            connectivity_check(
                SAMPLE_ID,
                &peering,
                sample_priority(),
                RoleAttribute::Controlled {
                    tiebreaker: SAMPLE_TIEBREAKER,
                },
            )
            .expect("encodes"),
            check_success(
                SAMPLE_ID,
                &peering,
                "192.0.2.1:32853".parse().expect("valid"),
            )
            .expect("encodes"),
            role_conflict(SAMPLE_ID, &peering).expect("encodes"),
            keepalive(SAMPLE_ID).expect("encodes"),
        ] {
            assert!(is_stun(&bytes), "{bytes:02x?}");
            assert_eq!(crate::dtls::classify(&bytes), crate::dtls::Arriving::Stun);
        }
    }

    /// The check value every description of CRC-32 publishes for the ASCII digits.
    #[test]
    fn the_crc_matches_the_published_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn the_port_key_is_the_top_half_of_the_cookie() {
        assert_eq!(u32::from(PORT_KEY) << 16, MAGIC_COOKIE & 0xffff_0000);
    }

    /// RFC 5389 §6's class and method interleave, over the four classes Binding has.
    #[test]
    fn the_message_type_round_trips_through_the_class_bits() {
        for (class, raw) in [
            (Class::Request, 0x0001),
            (Class::Indication, 0x0011),
            (Class::Success, 0x0101),
            (Class::Error, 0x0111),
        ] {
            assert_eq!(message_type(class, METHOD_BINDING), raw);
            assert_eq!(split_type(raw), (class, METHOD_BINDING));
        }
    }

    /// Two transaction IDs differ, because the one thing they must not do is repeat.
    #[test]
    fn a_transaction_id_is_fresh_each_time() {
        assert_ne!(new_transaction_id(), new_transaction_id());
    }
}
