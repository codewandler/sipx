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
//! value is the CRC-32 XORed with `0x5354554e`. Both adjustments are easy to skip and neither is
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

pub use sipx_transport::stun::{TransactionId, new_transaction_id};
use sipx_transport::stun::{HEADER_LEN, MAGIC_COOKIE, is_stun};

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

/// RFC 5389 §15.5: the CRC-32 is XORed with this before it goes on the wire, "to avoid a
/// fingerprint of the STUN message being confused with the CRC of an enclosing protocol".
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
        self.attributes.iter().find_map(|attribute| match attribute {
            Attribute::Username(name) => Some(name.as_str()),
            _ => None,
        })
    }

    /// The `PRIORITY` a check claims for the candidate the peer would learn from it (§7.1.1).
    #[must_use]
    pub fn priority(&self) -> Option<Priority> {
        self.attributes.iter().find_map(|attribute| match attribute {
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
        self.attributes.iter().find_map(|attribute| match attribute {
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
        self.attributes.iter().find_map(|attribute| match attribute {
            Attribute::ErrorCode { code, .. } => Some(*code),
            _ => None,
        })
    }

    /// The `XOR-MAPPED-ADDRESS`, unobfuscated.
    #[must_use]
    pub fn mapped_address(&self) -> Option<SocketAddr> {
        self.attributes.iter().find_map(|attribute| match attribute {
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
    /// is `evtj`, and the peer's password is the one the RFC states.
    fn sample_sender() -> Peering {
        Peering::new(
            Credentials::new(SAMPLE_UFRAG_SENDER, OTHER_PASSWORD).expect("valid credentials"),
            Credentials::received(SAMPLE_UFRAG_RECEIVER, SAMPLE_PASSWORD)
                .expect("valid credentials"),
        )
    }

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
        let priority = Priority::new(SAMPLE_PRIORITY).expect("in range");

        let message = Message::new(Class::Request, SAMPLE_ID)
            .with(Attribute::Software(SAMPLE_SOFTWARE.to_owned()))
            .with(Attribute::Priority(priority))
            .with(Attribute::IceControlled(SAMPLE_TIEBREAKER))
            .with(Attribute::Username(peering.outbound_username()));

        assert_eq!(
            message.encode(Some(peering.outbound_key())).expect("encodes"),
            hex(SAMPLE_REQUEST),
            "the encoder does not reproduce RFC 5769 §2.1"
        );
    }
}
