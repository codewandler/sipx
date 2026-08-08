//! Reliable provisional responses (RFC 3262): 100rel, `RSeq`, `RAck` and PRACK.
//!
//! An ordinary `180 Ringing` is fire-and-forget. Over UDP it is simply lost sometimes, and the
//! caller hears nothing while the callee's phone rings — or, worse, an early-media answer never
//! arrives and the call connects to silence. RFC 3262 makes a provisional response an
//! acknowledged message: the UAS numbers it, retransmits it until a PRACK comes back, and gives
//! up on the whole invitation if none ever does.
//!
//! Some carriers will not accept a call without it, which is the practical reason this exists.
//!
//! Everything here is pure: sequence numbers, the ordering rule, and the decision about whether
//! a request may or must be answered reliably. The retransmission clock lives a layer up.

use std::fmt;

use crate::error::HeaderError;
use crate::headers::grammar::{parse_u64, trim};
use crate::message::TypedHeader;
use crate::name::HeaderName;

/// The option tag, in `Supported`, `Require` and `Unsupported` (RFC 3262 §8.1).
pub const OPTION_TAG: &str = "100rel";

/// The largest first sequence number the RFC allows.
///
/// §3: the first `RSeq` "MUST be between 1 and 2**31 - 1". The ceiling is not decoration — the
/// field is 32 bits and "`RSeq` numbers MUST NOT wrap around", so starting in the lower half
/// leaves 2^31 responses of headroom before the rule could be broken.
pub const MAX_FIRST_RSEQ: u32 = i32::MAX as u32;

/// The `RSeq` header (RFC 3262 §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RSeq(pub u32);

impl TypedHeader for RSeq {
    const NAME: HeaderName = HeaderName::RSeq;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let n = parse_u64(trim(value), "RSeq")?;
        u32::try_from(n)
            .map(Self)
            .map_err(|_| HeaderError::OutOfRange { header: "RSeq" })
    }
}

/// The `RAck` header (RFC 3262 §7.2): `response-num CSeq-num Method`.
///
/// All three parts are load-bearing. The response number says *which* provisional is being
/// acknowledged, and the `CSeq` pair says which request it belonged to — without them a PRACK
/// for a re-INVITE could be matched against a provisional from the original INVITE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RAck {
    /// The `RSeq` of the response being acknowledged.
    pub rseq: u32,
    /// The `CSeq` number of the request that response answered.
    pub cseq: u32,
    /// The method of that request.
    pub method: Vec<u8>,
}

impl TypedHeader for RAck {
    const NAME: HeaderName = HeaderName::RAck;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let mut parts = trim(value)
            .split(u8::is_ascii_whitespace)
            .filter(|p| !p.is_empty());
        let bad = || HeaderError::Syntax { header: "RAck" };
        let rseq = parse_u64(parts.next().ok_or_else(bad)?, "RAck")?;
        let cseq = parse_u64(parts.next().ok_or_else(bad)?, "RAck")?;
        let method = parts.next().ok_or_else(bad)?.to_vec();
        // Three fields exactly. A fourth means the value is not what this grammar describes,
        // and guessing which three were meant is how a PRACK gets matched to the wrong
        // response.
        if parts.next().is_some() || method.is_empty() {
            return Err(bad());
        }
        Ok(Self {
            rseq: u32::try_from(rseq).map_err(|_| HeaderError::OutOfRange { header: "RAck" })?,
            cseq: u32::try_from(cseq).map_err(|_| HeaderError::OutOfRange { header: "RAck" })?,
            method,
        })
    }
}

impl fmt::Display for RAck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            self.rseq,
            self.cseq,
            String::from_utf8_lossy(&self.method)
        )
    }
}

/// What a UAS must do about reliability for an incoming request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reliability {
    /// The peer said nothing about 100rel. §3: the UAS "MUST NOT send the provisional response
    /// reliably" — a peer that has not asked for `RSeq` will not send PRACK, and the response
    /// would be retransmitted for 32 seconds and then fail the invitation.
    Forbidden,
    /// The peer supports it. Reliable provisionals are allowed, not required.
    Permitted,
    /// The peer put it in `Require`. Provisionals must be reliable.
    Required,
    /// The peer requires it and this side will not: refuse with `420 Bad Extension` and an
    /// `Unsupported: 100rel` (§3).
    Refuse,
}

/// What the peer said about 100rel in its request.
///
/// A struct rather than two `bool` arguments, because `supported` and `required` are the same
/// type and mean nearly opposite things, and a caller that transposes them turns "may send
/// reliably" into "must". That is not hypothetical: writing this module's own tests transposed
/// them on the first attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offered {
    /// `100rel` appeared in `Supported`.
    pub supported: bool,
    /// `100rel` appeared in `Require`.
    pub required: bool,
}

impl Offered {
    /// What a request's `Supported` and `Require` say about 100rel.
    #[must_use]
    pub fn in_request(request: &crate::message::Request) -> Self {
        let has = |name: &HeaderName| {
            request
                .headers
                .get_all(name)
                .any(|header| contains_tag(&header.value()))
        };
        Self {
            supported: has(&HeaderName::Supported),
            required: has(&HeaderName::Require),
        }
    }
}

/// Whether a comma-separated option-tag list contains `100rel`.
fn contains_tag(value: &[u8]) -> bool {
    value
        .split(|&b| b == b',')
        .any(|tag| trim(tag).eq_ignore_ascii_case(OPTION_TAG.as_bytes()))
}

/// Decide the UAS side (RFC 3262 §3).
///
/// `enabled` is local policy. Refusing outright is better than accepting and then not honouring
/// it: a caller that put 100rel in `Require` is waiting for an `RSeq` that will never come, and
/// silence looks the same as a network fault.
#[must_use]
pub fn reliability(peer: Offered, enabled: bool) -> Reliability {
    match (peer.required, peer.supported, enabled) {
        (true, _, true) => Reliability::Required,
        (true, _, false) => Reliability::Refuse,
        (false, true, true) => Reliability::Permitted,
        // Not offered, or offered to a side that has it switched off. Either way an ordinary
        // unreliable provisional is the only correct thing to send.
        (false, _, _) => Reliability::Forbidden,
    }
}

/// The UAS's numbering of reliable provisionals within one transaction (RFC 3262 §3).
#[derive(Debug, Clone, Copy)]
pub struct Numbering {
    next: u32,
    outstanding: Option<u32>,
}

impl Numbering {
    /// Start numbering at `first`, which must be in `1..=MAX_FIRST_RSEQ`.
    ///
    /// The value is supplied rather than generated here because this crate has no randomness
    /// and wants none — a sans-IO core that reaches for an entropy source has stopped being
    /// one. Out-of-range values are clamped into the legal window rather than rejected: the
    /// caller cannot usefully handle a failure to pick a number, and a number outside the
    /// window is a protocol violation this side would be committing.
    #[must_use]
    pub fn starting_at(first: u32) -> Self {
        Self {
            next: first.clamp(1, MAX_FIRST_RSEQ),
            outstanding: None,
        }
    }

    /// The number for the next reliable provisional, or `None` if one is still unacknowledged.
    ///
    /// §3: "The UAS MUST NOT send a second reliable provisional response until the first is
    /// acknowledged." Enforced by returning nothing rather than by trusting the caller, because
    /// the guarantee the mechanism sells — that the peer received these *in order* — is exactly
    /// what a second unacknowledged response destroys.
    pub fn allocate(&mut self) -> Option<u32> {
        if self.outstanding.is_some() {
            return None;
        }
        let rseq = self.next;
        self.next = self.next.saturating_add(1);
        self.outstanding = Some(rseq);
        Some(rseq)
    }

    /// The response still waiting for a PRACK.
    #[must_use]
    pub fn outstanding(&self) -> Option<u32> {
        self.outstanding
    }

    /// Whether this `RAck` acknowledges the outstanding response, and clear it if so.
    ///
    /// §3 defines a match as same dialog, and `RAck`'s three fields equal to the response's
    /// `RSeq` and the request's `CSeq` number and method. The dialog is the caller's to check;
    /// everything else is here.
    pub fn acknowledge(&mut self, ack: &RAck, cseq: u32, method: &[u8]) -> bool {
        let matches = self.outstanding == Some(ack.rseq)
            && ack.cseq == cseq
            && ack.method.eq_ignore_ascii_case(method);
        if matches {
            self.outstanding = None;
        }
        matches
    }
}

/// The UAC's view of the reliable provisionals arriving for one request (RFC 3262 §4).
#[derive(Debug, Clone, Copy, Default)]
pub struct Sequence {
    last: Option<u32>,
}

/// What the UAC should do with a reliable provisional it has just received.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Received {
    /// In order. Acknowledge it with a PRACK.
    Acknowledge,
    /// A retransmission of one already seen. §4: "retransmissions of that response MUST be
    /// discarded" — and notably *not* re-PRACKed, so a lossy path does not turn one ringing
    /// response into a stream of PRACKs.
    Duplicate,
    /// Out of order: a gap, so an earlier response has not arrived. §4 says such a response
    /// "MUST NOT be acknowledged with a PRACK, and MUST NOT be processed further".
    OutOfOrder,
}

impl Sequence {
    /// Classify a reliable provisional.
    pub fn accept(&mut self, rseq: u32) -> Received {
        match self.last {
            // §4: the sequence "MUST be initialized to the RSeq header field in the first
            // reliable provisional response received", whatever that value happens to be.
            None => {
                self.last = Some(rseq);
                Received::Acknowledge
            }
            Some(last) if rseq == last => Received::Duplicate,
            Some(last) if rseq == last.saturating_add(1) => {
                self.last = Some(rseq);
                Received::Acknowledge
            }
            Some(_) => Received::OutOfOrder,
        }
    }

    /// The highest in-order number seen.
    #[must_use]
    pub fn last(&self) -> Option<u32> {
        self.last
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_option_tag_is_found_however_the_peer_spells_the_list() {
        use crate::{Limits, Message, parse_datagram};
        let request = |extra: &str| {
            let text = format!(
                "INVITE sip:b@example.com SIP/2.0\r\n\
                 Via: SIP/2.0/UDP 192.0.2.1;branch=z9hG4bKx\r\n\
                 To: <sip:b@example.com>\r\n\
                 From: <sip:a@example.net>;tag=1\r\n\
                 Call-ID: c\r\n\
                 CSeq: 1 INVITE\r\n\
                 {extra}\
                 Content-Length: 0\r\n\r\n"
            );
            match parse_datagram(bytes::Bytes::from(text), &Limits::datagram()).expect("parses") {
                Message::Request(r) => r,
                Message::Response(_) => panic!("a request"),
            }
        };
        // Comma-joined with other tags, on its own row, and in a different case: all the same
        // list as far as RFC 3261 §7.3.1 is concerned.
        assert!(Offered::in_request(&request("Supported: timer, 100rel, path\r\n")).supported);
        assert!(Offered::in_request(&request("Supported: 100REL\r\n")).supported);
        assert!(Offered::in_request(&request("Require: 100rel\r\n")).required);
        // Not a substring match: `100relx` is a different tag.
        assert!(!Offered::in_request(&request("Supported: 100relx\r\n")).supported);
        assert!(!Offered::in_request(&request("Supported: timer\r\n")).supported);
    }

    #[test]
    fn an_rack_carries_all_three_fields() {
        let ack = RAck::decode(b"9021 314159 INVITE").expect("parses");
        assert_eq!(ack.rseq, 9021);
        assert_eq!(ack.cseq, 314_159);
        assert_eq!(ack.method, b"INVITE");
        assert_eq!(ack.to_string(), "9021 314159 INVITE");
    }

    #[test]
    fn a_malformed_rack_is_rejected_rather_than_guessed_at() {
        // Two fields, four fields, and a missing method. Each could be "read generously" into
        // something, and each generous reading is a PRACK matched against a response it does
        // not acknowledge.
        for value in [
            &b"9021 314159"[..],
            &b"9021 314159 INVITE extra"[..],
            &b"9021"[..],
            &b""[..],
        ] {
            assert!(
                RAck::decode(value).is_err(),
                "{:?} should not parse",
                String::from_utf8_lossy(value)
            );
        }
    }

    #[test]
    fn the_uas_will_not_number_a_second_response_before_the_first_is_acknowledged() {
        let mut numbering = Numbering::starting_at(500);
        assert_eq!(numbering.allocate(), Some(500));
        // §3: not until the first is acknowledged. Otherwise the UAS cannot be sure the peer
        // received them in order, which is the entire guarantee.
        assert_eq!(numbering.allocate(), None);

        let ack = RAck {
            rseq: 500,
            cseq: 1,
            method: b"INVITE".to_vec(),
        };
        assert!(numbering.acknowledge(&ack, 1, b"INVITE"));
        // §3: "greater by exactly one".
        assert_eq!(numbering.allocate(), Some(501));
    }

    #[test]
    fn a_prack_for_another_request_does_not_acknowledge_this_one() {
        let mut numbering = Numbering::starting_at(500);
        numbering.allocate();
        // Right RSeq, wrong CSeq: a PRACK for a re-INVITE's provisional would otherwise stop
        // the retransmissions of the original INVITE's.
        let wrong_cseq = RAck {
            rseq: 500,
            cseq: 2,
            method: b"INVITE".to_vec(),
        };
        assert!(!numbering.acknowledge(&wrong_cseq, 1, b"INVITE"));
        let wrong_method = RAck {
            rseq: 500,
            cseq: 1,
            method: b"UPDATE".to_vec(),
        };
        assert!(!numbering.acknowledge(&wrong_method, 1, b"INVITE"));
        assert_eq!(numbering.outstanding(), Some(500));
    }

    #[test]
    fn a_first_rseq_outside_the_window_is_brought_into_it() {
        assert_eq!(Numbering::starting_at(0).allocate(), Some(1));
        assert_eq!(
            Numbering::starting_at(u32::MAX).allocate(),
            Some(MAX_FIRST_RSEQ)
        );
    }

    #[test]
    fn the_uac_acknowledges_in_order_and_discards_the_rest() {
        let mut seen = Sequence::default();
        // Whatever the first value is, it is the baseline — the RFC picks it at random.
        assert_eq!(seen.accept(9021), Received::Acknowledge);
        assert_eq!(seen.accept(9021), Received::Duplicate);
        assert_eq!(seen.accept(9022), Received::Acknowledge);
        // A gap means an earlier response is missing; PRACKing this one would tell the UAS
        // something arrived in order when it did not.
        assert_eq!(seen.accept(9024), Received::OutOfOrder);
        assert_eq!(seen.last(), Some(9022));
        // And the missing one, arriving late, is still in order.
        assert_eq!(seen.accept(9023), Received::Acknowledge);
    }

    #[test]
    fn a_peer_that_never_mentioned_100rel_gets_unreliable_provisionals() {
        // §3: "If the request did not include either a Supported or Require header field
        // indicating this feature, the UAS MUST NOT send the provisional response reliably."
        // Sending one anyway means retransmitting for 32 seconds at a peer that will never
        // PRACK, and then failing an invitation that was working.
        assert_eq!(
            reliability(
                Offered {
                    supported: false,
                    required: false
                },
                true
            ),
            Reliability::Forbidden
        );
    }

    #[test]
    fn a_requirement_this_side_will_not_meet_is_refused_rather_than_ignored() {
        let asked = Offered {
            supported: true,
            required: true,
        };
        let offered = Offered {
            supported: true,
            required: false,
        };
        assert_eq!(reliability(asked, false), Reliability::Refuse);
        assert_eq!(reliability(asked, true), Reliability::Required);
        assert_eq!(reliability(offered, true), Reliability::Permitted);
        // Switched off locally, and only offered: nothing to refuse, nothing to do.
        assert_eq!(reliability(offered, false), Reliability::Forbidden);
    }
}
