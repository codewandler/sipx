//! Transfer: REFER (RFC 3515) and the implicit subscription it creates.
//!
//! The shape of a blind transfer, and the part that is easy to get wrong:
//!
//! 1. The **transferor** sends REFER inside the existing dialog, naming where to go in
//!    `Refer-To`.
//! 2. The **transferee** answers `202 Accepted` — which means *"I will try"*, and nothing more.
//! 3. The transferee places the new call and reports back with NOTIFY.
//!
//! Step 2 is where implementations go wrong. A 202 is not success: treating it as success
//! reports a completed transfer to a user whose call may have been refused, gone to voicemail
//! or rung out. RFC 3515 §2.4.4 exists precisely so the transferor can tell those apart, and
//! that is why this module models the outcome as something that arrives *later*.
//!
//! The subscription is implicit — REFER creates one without a SUBSCRIBE — and it must end. A
//! transferee that reports the outcome and then says nothing leaves the transferor holding a
//! subscription that never terminates, which is a leak on both sides and, on a real network, a
//! dialog a proxy keeps state for.

use sipx_sip::{HeaderName, Request, Uri};

/// What a REFER asked of us.
#[derive(Debug, Clone)]
pub struct Referral {
    /// Where the transferor wants the call sent.
    pub target: Uri,
    /// Who asked, from `Referred-By` (RFC 3892). `None` if they did not say.
    ///
    /// Worth surfacing rather than swallowing: a transfer is a request to call somebody on
    /// another party's say-so, and who said so is the only basis for deciding whether to.
    pub referred_by: Option<String>,
    /// The REFER's sequence number, which identifies the subscription it created
    /// (RFC 3515 §2.4.4: `Event: refer;id=<CSeq>`).
    pub(crate) event_id: u32,
    /// The transaction to answer, and the request to answer it with. Both are kept because a
    /// response is built from the request it answers, and the REFER is gone from the incoming
    /// queue by the time the application decides.
    pub(crate) key: sipx_sip::transaction::TransactionKey,
    pub(crate) request: Request,
}

/// The dialog an INVITE asks to take the place of (RFC 3891).
///
/// **All three fields are load-bearing, and the two tags are the security.** A `Call-ID` is
/// carried in every message of a dialog and is visible to every element on the path — a proxy,
/// a load balancer, anything that logged a header. The tags are random and known only to the
/// two parties. Matching on the `Call-ID` alone would turn this header into a call-hijack
/// primitive: anyone who had seen one message of a call could ask to be put in the middle of it.
///
/// RFC 3891 §5 says as much, and it is the reason this type has no constructor that takes fewer
/// than three fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaces {
    /// The `Call-ID` of the dialog to replace.
    pub call_id: Vec<u8>,
    /// The `To` tag, from the point of view of whoever built this header.
    pub to_tag: Vec<u8>,
    /// The `From` tag, likewise.
    pub from_tag: Vec<u8>,
    /// Whether the sender will only replace a dialog that has not been answered yet.
    pub early_only: bool,
}

impl Replaces {
    /// Read a `Replaces` header out of a request.
    ///
    /// `None` when there is none, and also when there is one that is unusable — a header
    /// missing either tag names no dialog, and treating it as though it named one with an empty
    /// tag is how the tags stop being a secret.
    #[must_use]
    pub fn of(request: &Request) -> Option<Self> {
        let value = request.headers.value(&HeaderName::Replaces)?;
        Self::parse(&value)
    }

    /// Parse a header value: `call-id;to-tag=x;from-tag=y[;early-only]`.
    #[must_use]
    pub fn parse(value: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(value).ok()?;
        let mut parts = text.split(';');
        let call_id = parts.next()?.trim();
        if call_id.is_empty() {
            return None;
        }

        let (mut to_tag, mut from_tag, mut early_only) = (None, None, false);
        for part in parts {
            let part = part.trim();
            // Parameter names are case-insensitive (RFC 3261 §7.3.1); the values are not.
            let (name, value) = match part.split_once('=') {
                Some((name, value)) => (name.trim(), Some(value.trim())),
                None => (part, None),
            };
            match (name.to_ascii_lowercase().as_str(), value) {
                ("to-tag", Some(value)) if !value.is_empty() => {
                    to_tag = Some(value.as_bytes().to_vec());
                }
                ("from-tag", Some(value)) if !value.is_empty() => {
                    from_tag = Some(value.as_bytes().to_vec());
                }
                ("early-only", _) => early_only = true,
                _ => {}
            }
        }

        Some(Self {
            call_id: call_id.as_bytes().to_vec(),
            to_tag: to_tag?,
            from_tag: from_tag?,
            early_only,
        })
    }

    /// Whether this names that dialog.
    ///
    /// The tags swap sides. A `Replaces` is built by the party that *observed* the dialog from
    /// outside it — in an attended transfer, the transferor describing its own call to the
    /// transferee — so the `to-tag` is the tag of the party receiving this INVITE, which is
    /// that party's *local* tag. Getting the orientation wrong makes every legitimate transfer
    /// fail while leaving the hijack case wide open, because a mismatch is a mismatch either
    /// way and only the successful case would have shown it up.
    #[must_use]
    pub fn matches(&self, dialog: &crate::dialog::Dialog) -> bool {
        // Constant-time comparison is not called for: these are not secrets an attacker can
        // learn by timing, they are values that are either known or guessed, and a guess has
        // 2^128 of tag space to find.
        dialog.id.call_id == self.call_id
            && dialog.id.local_tag == self.to_tag
            && dialog.id.remote_tag == self.from_tag
    }

    /// Render as a header value, for the INVITE that asks for the replacement.
    #[must_use]
    pub fn to_header(&self) -> String {
        let mut out = format!(
            "{};to-tag={};from-tag={}",
            String::from_utf8_lossy(&self.call_id),
            String::from_utf8_lossy(&self.to_tag),
            String::from_utf8_lossy(&self.from_tag),
        );
        if self.early_only {
            out.push_str(";early-only");
        }
        out
    }
}

/// How far a transfer has got, as the transferor learns it from NOTIFY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferState {
    /// The transferee has taken it on and is trying.
    Trying,
    /// The target is ringing.
    Ringing,
    /// The target answered. The transfer worked.
    Succeeded,
    /// It did not, and this is what the target said.
    Failed {
        /// The status the target gave.
        status: u16,
        /// Its reason phrase.
        reason: String,
    },
}

impl TransferState {
    /// Read a state out of a `message/sipfrag` status line.
    #[must_use]
    pub fn from_status(status: u16, reason: &str) -> Self {
        match status {
            100..=199 if status == 180 || status == 183 => Self::Ringing,
            100..=199 => Self::Trying,
            200..=299 => Self::Succeeded,
            _ => Self::Failed {
                status,
                reason: reason.to_owned(),
            },
        }
    }

    /// Whether the transfer is over, either way.
    #[must_use]
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed { .. })
    }
}

/// A transfer this side asked for, and what has become of it.
#[derive(Debug, Clone)]
pub struct Transfer {
    /// The last thing the transferee reported.
    pub state: TransferState,
    /// Whether the implicit subscription has ended.
    ///
    /// Separate from `state.is_final()` on purpose. A transferee may report a final status and
    /// still owe a terminating NOTIFY; until this is true the subscription is open, and a
    /// transferor that stopped listening would miss it.
    pub finished: bool,
}

/// The body of a NOTIFY about a transfer: a status line and nothing else.
///
/// RFC 3515 §2.4.5 asks for `message/sipfrag` (RFC 3420) — a fragment of a SIP message. Only
/// the status line is required, and only the status line is useful, so that is what sipx sends.
#[must_use]
pub fn sipfrag(status: u16, reason: &str) -> String {
    format!("SIP/2.0 {status} {reason}\r\n")
}

/// The status line out of a `message/sipfrag` body.
///
/// Tolerant about what follows: a fragment may legally carry headers after the status line, and
/// a transferee that sends them is not wrong. Strict about the line itself — anything that is
/// not a SIP status line means the body is not what its `Content-Type` claimed.
#[must_use]
pub fn parse_sipfrag(body: &[u8]) -> Option<(u16, String)> {
    let text = std::str::from_utf8(body).ok()?;
    let line = text.lines().next()?.trim();
    let rest = line.strip_prefix("SIP/2.0 ")?;
    let (code, reason) = rest.split_once(' ').unwrap_or((rest, ""));
    let status: u16 = code.trim().parse().ok()?;
    if !(100..=699).contains(&status) {
        return None;
    }
    Some((status, reason.trim().to_owned()))
}

/// Whether a `Subscription-State` says the subscription is over (RFC 6665 §4.1.3).
#[must_use]
pub fn is_terminated(subscription_state: &[u8]) -> bool {
    let text = String::from_utf8_lossy(subscription_state);
    let state = text.split(';').next().unwrap_or("").trim();
    state.eq_ignore_ascii_case("terminated")
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

    #[test]
    fn a_status_line_round_trips() {
        let (status, reason) = parse_sipfrag(sipfrag(200, "OK").as_bytes()).expect("parses");
        assert_eq!((status, reason.as_str()), (200, "OK"));
    }

    /// A fragment may carry headers after the status line. Rejecting one that does would refuse
    /// a transferee that is following the RFC more closely than we do.
    #[test]
    fn headers_after_the_status_line_are_ignored() {
        let body = b"SIP/2.0 486 Busy Here\r\nContact: <sip:a@b>\r\n\r\n";
        assert_eq!(
            parse_sipfrag(body).expect("parses"),
            (486, "Busy Here".to_owned())
        );
    }

    #[test]
    fn a_reason_phrase_may_have_spaces_or_be_absent() {
        assert_eq!(
            parse_sipfrag(b"SIP/2.0 480 Temporarily Unavailable\r\n")
                .expect("parses")
                .1,
            "Temporarily Unavailable"
        );
        assert_eq!(
            parse_sipfrag(b"SIP/2.0 200\r\n").expect("parses"),
            (200, String::new())
        );
    }

    #[test]
    fn something_that_is_not_a_status_line_is_refused() {
        assert!(parse_sipfrag(b"200 OK\r\n").is_none(), "no SIP version");
        assert!(parse_sipfrag(b"SIP/2.0 wat\r\n").is_none(), "not a number");
        assert!(parse_sipfrag(b"SIP/2.0 99 Too Low\r\n").is_none());
        assert!(parse_sipfrag(b"SIP/2.0 700 Too High\r\n").is_none());
        assert!(parse_sipfrag(b"").is_none());
    }

    /// A 202 is not one of these. It answers the REFER, not the call the REFER asked for, and a
    /// transferor that read it as success would report a transfer that may have been refused.
    #[test]
    fn a_status_becomes_the_state_it_means() {
        assert_eq!(
            TransferState::from_status(100, "Trying"),
            TransferState::Trying
        );
        assert_eq!(
            TransferState::from_status(180, "Ringing"),
            TransferState::Ringing
        );
        assert_eq!(
            TransferState::from_status(200, "OK"),
            TransferState::Succeeded
        );
        assert_eq!(
            TransferState::from_status(486, "Busy Here"),
            TransferState::Failed {
                status: 486,
                reason: "Busy Here".to_owned()
            }
        );
    }

    #[test]
    fn only_a_final_state_is_final() {
        assert!(!TransferState::Trying.is_final());
        assert!(!TransferState::Ringing.is_final());
        assert!(TransferState::Succeeded.is_final());
    }

    fn dialog(call_id: &str, local: &str, remote: &str) -> crate::dialog::Dialog {
        crate::dialog::Dialog {
            role: crate::dialog::Role::Callee,
            id: crate::dialog::DialogId {
                call_id: call_id.as_bytes().to_vec(),
                local_tag: local.as_bytes().to_vec(),
                remote_tag: remote.as_bytes().to_vec(),
            },
            local_uri: "<sip:a@b>".to_owned(),
            remote_uri: "<sip:c@d>".to_owned(),
            remote_target: Uri::parse(bytes::Bytes::from_static(b"sip:c@d")).expect("valid"),
            local_cseq: 1,
            remote_cseq: None,
            route_set: Vec::new(),
        }
    }

    #[test]
    fn a_replaces_header_round_trips() {
        let replaces = Replaces {
            call_id: b"abc@host".to_vec(),
            to_tag: b"tttt".to_vec(),
            from_tag: b"ffff".to_vec(),
            early_only: false,
        };
        let parsed = Replaces::parse(replaces.to_header().as_bytes()).expect("parses");
        assert_eq!(parsed, replaces);
    }

    #[test]
    fn early_only_survives_the_round_trip() {
        let replaces = Replaces {
            call_id: b"abc@host".to_vec(),
            to_tag: b"t".to_vec(),
            from_tag: b"f".to_vec(),
            early_only: true,
        };
        assert!(replaces.to_header().contains(";early-only"));
        assert!(
            Replaces::parse(replaces.to_header().as_bytes())
                .expect("parses")
                .early_only
        );
    }

    /// A header missing either tag names no dialog. Accepting it with an empty tag is exactly
    /// how the tags stop being the thing that makes this safe.
    #[test]
    fn a_header_missing_a_tag_is_not_a_replaces() {
        assert!(
            Replaces::parse(b"abc@host;to-tag=t").is_none(),
            "no from-tag"
        );
        assert!(
            Replaces::parse(b"abc@host;from-tag=f").is_none(),
            "no to-tag"
        );
        assert!(Replaces::parse(b"abc@host").is_none(), "neither");
        assert!(
            Replaces::parse(b"abc@host;to-tag=;from-tag=f").is_none(),
            "empty"
        );
        assert!(
            Replaces::parse(b";to-tag=t;from-tag=f").is_none(),
            "no Call-ID"
        );
        assert!(Replaces::parse(b"").is_none());
    }

    #[test]
    fn parameter_names_are_case_insensitive_and_values_are_not() {
        let parsed = Replaces::parse(b"abc@host;To-Tag=Abc;FROM-TAG=Def").expect("parses");
        assert_eq!(parsed.to_tag, b"Abc".to_vec(), "the value keeps its case");
        assert_eq!(parsed.from_tag, b"Def".to_vec());
    }

    /// The orientation. The `to-tag` is the *local* tag of whoever receives the INVITE, because
    /// the header was written by a party looking at that dialog from the other side.
    #[test]
    fn the_tags_match_the_dialog_from_the_receivers_point_of_view() {
        let dialog = dialog("call-1", "mine", "theirs");
        let replaces = Replaces {
            call_id: b"call-1".to_vec(),
            to_tag: b"mine".to_vec(),
            from_tag: b"theirs".to_vec(),
            early_only: false,
        };
        assert!(replaces.matches(&dialog));

        // Swapped, which is the mistake that makes every legitimate transfer fail.
        let swapped = Replaces {
            to_tag: b"theirs".to_vec(),
            from_tag: b"mine".to_vec(),
            ..replaces.clone()
        };
        assert!(!swapped.matches(&dialog));
    }

    /// The security case. A `Call-ID` is visible to everything on the path; the tags are not.
    /// Matching on the `Call-ID` alone would let anyone who had seen one message of a call ask
    /// to be put in the middle of it.
    #[test]
    fn a_matching_call_id_with_wrong_tags_does_not_match() {
        let dialog = dialog("call-1", "mine", "theirs");
        for (to, from) in [
            ("guessed", "theirs"),
            ("mine", "guessed"),
            ("guessed", "guessed"),
            ("", ""),
        ] {
            let attempt = Replaces {
                call_id: b"call-1".to_vec(),
                to_tag: to.as_bytes().to_vec(),
                from_tag: from.as_bytes().to_vec(),
                early_only: false,
            };
            assert!(
                !attempt.matches(&dialog),
                "the Call-ID alone must not be enough: to={to} from={from}"
            );
        }
    }

    #[test]
    fn a_different_call_does_not_match_however_right_the_tags_look() {
        let dialog = dialog("call-1", "mine", "theirs");
        let other = Replaces {
            call_id: b"call-2".to_vec(),
            to_tag: b"mine".to_vec(),
            from_tag: b"theirs".to_vec(),
            early_only: false,
        };
        assert!(!other.matches(&dialog));
    }

    #[test]
    fn a_terminated_subscription_is_recognised_however_it_is_spelled() {
        assert!(is_terminated(b"terminated;reason=noresource"));
        assert!(is_terminated(b"Terminated"));
        assert!(is_terminated(b"  terminated  ;reason=timeout"));
        assert!(!is_terminated(b"active;expires=60"));
        assert!(!is_terminated(b"pending"));
    }
}
