//! A STUN Binding client, only as much of RFC 5389 as a keep-alive needs.
//!
//! RFC 5626 §4.4.2 makes STUN the keep-alive for UDP flows: "All SIP UAs MUST support the STUN
//! keep-alive technique for UDP flows." It is a better keep-alive than a SIP request because the
//! response carries the address the far end *sees*, so a UA learns that its NAT mapping changed
//! rather than only that the flow still works — §4.4.2 has a changed `XOR-MAPPED-ADDRESS` mean the
//! flow has failed.
//!
//! Scope, deliberately: a Binding Request with no attributes, and a Binding Response read for its
//! mapped address. No `MESSAGE-INTEGRITY`, no `FINGERPRINT`, no long-term credentials — §4.4.2's
//! keep-alive is unauthenticated, and RFC 5389 §10 does not require authentication for Binding
//! over an established flow. Anything that needs the full protocol (ICE, RFC 8445) needs a
//! different module, not more attributes bolted onto this one.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// RFC 5389 §6: the fixed cookie that distinguishes STUN from RFC 3489 and from other traffic.
pub const MAGIC_COOKIE: u32 = 0x2112_a442;

/// RFC 5389 §6: every STUN message begins with a 20-byte header.
pub const HEADER_LEN: usize = 20;

const BINDING_REQUEST: u16 = 0x0001;
const BINDING_RESPONSE: u16 = 0x0101;
const BINDING_ERROR: u16 = 0x0111;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;
const FAMILY_IPV4: u8 = 0x01;
const FAMILY_IPV6: u8 = 0x02;

/// The 96 bits that tie a response to its request (RFC 5389 §6).
pub type TransactionId = [u8; 12];

/// A fresh transaction ID.
///
/// §6 requires it to be "uniformly and randomly chosen ... cryptographically random": it is the
/// only thing preventing an off-path attacker from forging a response, and a forged response with
/// a different mapped address would make a UA declare a working flow dead (§4.4.2).
#[must_use]
pub fn new_transaction_id() -> TransactionId {
    use rand::Rng as _;
    let mut id = [0u8; 12];
    rand::rng().fill(&mut id);
    id
}

/// Encode a Binding Request with no attributes (RFC 5389 §6, §7.1).
#[must_use]
pub fn binding_request(id: &TransactionId) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&BINDING_REQUEST.to_be_bytes());
    // Length counts the attributes only, and there are none.
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    out.extend_from_slice(id);
    out
}

/// Whether a datagram is STUN rather than SIP (RFC 5389 §7.3).
///
/// Two checks, both from §7.3: "the most significant 2 bits of every STUN message MUST be zeroes",
/// and the magic cookie. Demultiplexing on the same socket is what §4.4.2's keep-alive requires —
/// the ping has to travel over the very flow it is testing — and this is the test the RFC gives
/// for doing it. A SIP message cannot collide: its first byte is a method letter or `S`, all of
/// which have a high bit set within the first two bits' meaning here (`0x53` is `0101…`, whose top
/// two bits are `01`), and the cookie makes a collision beyond that vanishingly unlikely.
#[must_use]
pub fn is_stun(datagram: &[u8]) -> bool {
    let Some(first) = datagram.first() else {
        return false;
    };
    if first & 0xc0 != 0 || datagram.len() < HEADER_LEN {
        return false;
    }
    datagram
        .get(4..8)
        .and_then(|cookie| <[u8; 4]>::try_from(cookie).ok())
        .is_some_and(|cookie| u32::from_be_bytes(cookie) == MAGIC_COOKIE)
}

/// What a datagram that is STUN turned out to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// A Binding Response, with the address the server saw — if it named one.
    Bound {
        /// The transaction this answers.
        id: TransactionId,
        /// The reflexive address, from `XOR-MAPPED-ADDRESS`.
        mapped: Option<SocketAddr>,
    },
    /// A Binding Error Response. §4.4.2: the flow "is considered failed".
    Failed {
        /// The transaction this answers.
        id: TransactionId,
    },
}

impl Reply {
    /// The transaction this reply belongs to.
    #[must_use]
    pub fn id(&self) -> TransactionId {
        match self {
            Self::Bound { id, .. } | Self::Failed { id } => *id,
        }
    }
}

/// Read a STUN reply, or `None` if the datagram is not one.
///
/// Requests are not decoded: sipx is a STUN *client* here. A Binding Request arriving on a SIP
/// socket is something else's business, and answering it would make sipx a STUN server by
/// accident.
#[must_use]
pub fn parse_reply(datagram: &[u8]) -> Option<Reply> {
    if !is_stun(datagram) {
        return None;
    }
    let kind = u16::from_be_bytes(<[u8; 2]>::try_from(datagram.get(0..2)?).ok()?);
    let length = usize::from(u16::from_be_bytes(
        <[u8; 2]>::try_from(datagram.get(2..4)?).ok()?,
    ));
    let id: TransactionId = <[u8; 12]>::try_from(datagram.get(8..20)?).ok()?;

    match kind {
        BINDING_ERROR => Some(Reply::Failed { id }),
        BINDING_RESPONSE => {
            // The stated length is authoritative; a datagram carrying extra bytes is not licence
            // to read them.
            let body = datagram.get(HEADER_LEN..HEADER_LEN.checked_add(length)?)?;
            Some(Reply::Bound {
                id,
                mapped: mapped_address(body, &id),
            })
        }
        _ => None,
    }
}

/// Walk the attributes for `XOR-MAPPED-ADDRESS` (RFC 5389 §15.2).
fn mapped_address(mut body: &[u8], id: &TransactionId) -> Option<SocketAddr> {
    while body.len() >= 4 {
        let kind = u16::from_be_bytes(<[u8; 2]>::try_from(body.get(0..2)?).ok()?);
        let length = usize::from(u16::from_be_bytes(
            <[u8; 2]>::try_from(body.get(2..4)?).ok()?,
        ));
        let value = body.get(4..4usize.checked_add(length)?)?;
        if kind == XOR_MAPPED_ADDRESS {
            return decode_xor_mapped(value, id);
        }
        // §15: "the value in the length field MUST contain the length of the Value part ...
        // Since STUN aligns attributes on 32-bit boundaries, attributes whose content is not a
        // multiple of 4 bytes are padded". Skipping without the padding walks into the middle of
        // the next attribute — the SOFTWARE attribute in RFC 5769's own vector is 11 bytes, so a
        // decoder that forgets this fails on the RFC's example.
        let padded = length.checked_add(3)? & !3;
        body = body.get(4usize.checked_add(padded)?..).unwrap_or(&[]);
    }
    None
}

/// Undo the obfuscation §15.2 applies to the address.
///
/// The port is `XOR`ed with the top 16 bits of the cookie and the address with the cookie itself,
/// extended by the transaction ID for IPv6. §15.2 explains the point: some NATs rewrite anything
/// that looks like an address in a payload, and obfuscating it stops them corrupting the very
/// value the mechanism exists to report.
fn decode_xor_mapped(value: &[u8], id: &TransactionId) -> Option<SocketAddr> {
    let family = *value.get(1)?;
    let port = u16::from_be_bytes(<[u8; 2]>::try_from(value.get(2..4)?).ok()?)
        ^ u16::try_from(MAGIC_COOKIE >> 16).ok()?;
    match family {
        FAMILY_IPV4 => {
            let raw = u32::from_be_bytes(<[u8; 4]>::try_from(value.get(4..8)?).ok()?);
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::from(raw ^ MAGIC_COOKIE)),
                port,
            ))
        }
        FAMILY_IPV6 => {
            let raw = <[u8; 16]>::try_from(value.get(4..20)?).ok()?;
            let mut key = [0u8; 16];
            key.get_mut(..4)?
                .copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
            key.get_mut(4..)?.copy_from_slice(id);
            let mut out = [0u8; 16];
            for (index, byte) in out.iter_mut().enumerate() {
                *byte = raw.get(index)? ^ key.get(index)?;
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(out)), port))
        }
        _ => None,
    }
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

    /// Decode the hex listings RFC 5769 prints, so the test input is the RFC's bytes rather than
    /// something transcribed by hand into a different shape.
    fn hex(text: &str) -> Vec<u8> {
        text.split_whitespace()
            .map(|byte| u8::from_str_radix(byte, 16).expect("a hex byte"))
            .collect()
    }

    /// RFC 5769 §2.1, the sample request. Its transaction ID is what §2.2's response answers.
    const SAMPLE_REQUEST: &str = "
        00 01 00 58  21 12 a4 42  b7 e7 a7 01  bc 34 d6 86
        fa 87 df ae  80 22 00 10  53 54 55 4e  20 74 65 73
        74 20 63 6c  69 65 6e 74  00 24 00 04  6e 00 01 ff
        80 29 00 08  93 2f f9 b1  51 26 3b 36  00 06 00 09
        65 76 74 6a  3a 68 36 76  59 20 20 20  00 08 00 14
        9a ea a7 0c  bf d8 cb 56  78 1e f2 b5  b2 d3 f2 49
        c1 b5 71 a2  80 28 00 04  e5 7a 3b cf";

    /// RFC 5769 §2.2, the sample IPv4 response. The RFC states the decoded address itself:
    /// 192.0.2.1 port 32853.
    const SAMPLE_RESPONSE: &str = "
        01 01 00 3c  21 12 a4 42  b7 e7 a7 01  bc 34 d6 86
        fa 87 df ae  80 22 00 0b  74 65 73 74  20 76 65 63
        74 6f 72 20  00 20 00 08  00 01 a1 47  e1 12 a6 43
        00 08 00 14  2b 91 f5 99  fd 9e 90 c3  8c 74 89 f9
        2a f9 ba 53  f0 6b e7 d7  80 28 00 04  c0 7d 4c 96";

    const SAMPLE_ID: TransactionId = [
        0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae,
    ];

    /// The address RFC 5769 §2.2 says its own vector decodes to. Not computed here — the point of
    /// a published vector is that the expected value comes from the publisher.
    #[test]
    fn the_rfc_5769_ipv4_response_decodes_to_the_address_the_rfc_states() {
        let reply = parse_reply(&hex(SAMPLE_RESPONSE)).expect("a STUN reply");
        assert_eq!(
            reply,
            Reply::Bound {
                id: SAMPLE_ID,
                mapped: Some("192.0.2.1:32853".parse().expect("valid")),
            }
        );
    }

    /// The `SOFTWARE` attribute in that vector is 11 bytes, so its 32-bit padding has to be
    /// skipped to reach `XOR-MAPPED-ADDRESS` at all. A decoder that ignores §15's padding rule
    /// walks into the middle of the next attribute and finds nothing — which is why the vector is
    /// worth using rather than a hand-built two-attribute message.
    #[test]
    fn an_attribute_whose_length_is_not_a_multiple_of_four_is_padded_past() {
        let bytes = hex(SAMPLE_RESPONSE);
        let software_length = u16::from_be_bytes([bytes[22], bytes[23]]);
        assert_eq!(software_length, 11, "the vector's SOFTWARE attribute");
        assert!(
            matches!(
                parse_reply(&bytes),
                Some(Reply::Bound {
                    mapped: Some(_),
                    ..
                })
            ),
            "the padded attribute was not skipped correctly"
        );
    }

    #[test]
    fn the_rfc_5769_request_is_recognised_as_stun_but_not_as_a_reply() {
        let bytes = hex(SAMPLE_REQUEST);
        assert!(is_stun(&bytes));
        assert!(
            parse_reply(&bytes).is_none(),
            "sipx is a STUN client; answering a Binding Request would make it a server by accident"
        );
    }

    #[test]
    fn our_binding_request_has_the_header_the_rfc_specifies() {
        let request = binding_request(&SAMPLE_ID);
        assert_eq!(request.len(), HEADER_LEN, "no attributes");
        assert_eq!(&request[0..2], &[0x00, 0x01], "Binding Request");
        assert_eq!(&request[2..4], &[0x00, 0x00], "length counts attributes");
        assert_eq!(&request[4..8], &MAGIC_COOKIE.to_be_bytes());
        assert_eq!(&request[8..20], &SAMPLE_ID);
        assert!(is_stun(&request), "our own request must pass §7.3's test");
    }

    #[test]
    fn a_sip_message_is_not_mistaken_for_stun() {
        // The demultiplexing has to be safe in the direction that matters: a SIP request read as
        // STUN would be dropped, and the far end would see a request vanish.
        for message in [
            &b"INVITE sip:bob@example.com SIP/2.0\r\n\r\n"[..],
            &b"SIP/2.0 200 OK\r\n\r\n"[..],
            &b"REGISTER sip:example.com SIP/2.0\r\n\r\n"[..],
            &b"\r\n\r\n"[..],
            &b""[..],
        ] {
            assert!(
                !is_stun(message),
                "{:?} was taken for STUN",
                String::from_utf8_lossy(message)
            );
        }
    }

    #[test]
    fn a_truncated_or_cookieless_datagram_is_not_stun() {
        let mut short = binding_request(&SAMPLE_ID);
        short.truncate(HEADER_LEN - 1);
        assert!(!is_stun(&short), "a header must be complete to be one");

        let mut wrong_cookie = binding_request(&SAMPLE_ID);
        wrong_cookie[4] = 0x00;
        assert!(!is_stun(&wrong_cookie), "§7.3's cookie check");
    }

    #[test]
    fn a_binding_error_response_reads_as_a_failed_flow() {
        // §4.4.2: "If a STUN Binding Error Response is received ... the UA considers the flow
        // failed."
        let mut bytes = binding_request(&SAMPLE_ID);
        bytes[0] = 0x01;
        bytes[1] = 0x11;
        assert_eq!(
            parse_reply(&bytes),
            Some(Reply::Failed { id: SAMPLE_ID }),
            "an error response is a failed flow, not an absent answer"
        );
    }

    #[test]
    fn a_response_with_no_mapped_address_still_answers_the_transaction() {
        // A server is not obliged to be useful. What matters is that the flow is proven alive;
        // the mapped address is extra, and treating its absence as a parse failure would declare
        // a working flow dead.
        let mut bytes = binding_request(&SAMPLE_ID);
        bytes[0] = 0x01;
        bytes[1] = 0x01;
        assert_eq!(
            parse_reply(&bytes),
            Some(Reply::Bound {
                id: SAMPLE_ID,
                mapped: None
            })
        );
    }

    #[test]
    fn an_ipv6_mapped_address_is_unxored_with_the_transaction_id() {
        // §15.2 extends the XOR key with the transaction ID for IPv6. Built by XORing a known
        // address *with the rule the RFC states*, then asserting it decodes back — the closest
        // thing to a vector available, since RFC 5769 §2.3's IPv6 response is for a different
        // transaction ID than §2.1's.
        let addr: Ipv6Addr = "2001:db8::1".parse().expect("valid");
        let port: u16 = 32853;
        let mut key = [0u8; 16];
        key[..4].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
        key[4..].copy_from_slice(&SAMPLE_ID);
        let xored: Vec<u8> = addr
            .octets()
            .iter()
            .zip(key.iter())
            .map(|(a, k)| a ^ k)
            .collect();

        let mut bytes = binding_request(&SAMPLE_ID);
        bytes[0] = 0x01;
        bytes[1] = 0x01;
        bytes[2] = 0x00;
        bytes[3] = 24; // one attribute: 4 header + 20 value
        bytes.extend_from_slice(&XOR_MAPPED_ADDRESS.to_be_bytes());
        bytes.extend_from_slice(&20u16.to_be_bytes());
        bytes.push(0);
        bytes.push(FAMILY_IPV6);
        bytes.extend_from_slice(
            &(port ^ u16::try_from(MAGIC_COOKIE >> 16).expect("fits")).to_be_bytes(),
        );
        bytes.extend_from_slice(&xored);

        assert_eq!(
            parse_reply(&bytes),
            Some(Reply::Bound {
                id: SAMPLE_ID,
                mapped: Some(SocketAddr::new(IpAddr::V6(addr), port)),
            })
        );
    }

    #[test]
    fn two_transaction_ids_differ() {
        assert_ne!(new_transaction_id(), new_transaction_id());
    }
}
