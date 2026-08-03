//! `received` and `rport` — what makes SIP work through a NAT (RFC 3261 §18.2.1, RFC 3581).
//!
//! A client behind a NAT puts its private address in the `Via` sent-by, because that is all it
//! knows. A response sent there goes nowhere. The server therefore records where the request
//! *actually* came from, and the client's NAT pinhole is still open on that address and port.
//!
//! The edits here are surgical: only the topmost hop is touched, and within it only the two
//! parameters in question. The hops below belong to other elements and go back out exactly as
//! they arrived.

use std::net::SocketAddr;

use bytes::Bytes;
use sipx_sip::headers::{Via, first_hop_end};
use sipx_sip::{Header, HeaderName, Host, Request};

/// Add `received` and, if asked, `rport` to the topmost `Via` of a received request.
///
/// Returns whether anything changed.
pub fn apply_received_and_rport(request: &mut Request, source: SocketAddr) -> bool {
    let Some(header) = request.headers.get(&HeaderName::Via) else {
        return false;
    };
    let value = header.value().into_owned();
    let hop_end = first_hop_end(&value);
    let hop = value.get(..hop_end).unwrap_or(&value);
    let Ok(via) = Via::parse_one(hop) else {
        return false;
    };

    let mut updated_hop = hop.to_vec();
    let mut changed = false;

    // RFC 3581: an empty `rport` is the client asking which port we saw. The answer replaces
    // the empty parameter — appending a second `rport` would leave the first one, and a
    // reader taking the first occurrence would learn nothing.
    if matches!(via.rport(), Some(None))
        && let Some((start, end)) = param_span(&updated_hop, b"rport")
    {
        let replacement = format!(";rport={}", source.port());
        updated_hop.splice(start..end, replacement.into_bytes());
        changed = true;
    }

    // RFC 3261 §18.2.1: record the source when it differs from what the sender claimed. A
    // hostname sent-by always counts as differing, because comparing it would mean resolving
    // it — and the whole point is that the sender may be wrong about where it is.
    //
    // RFC 3581 §4 goes further for a sender that asked for `rport`: `received` is added
    // "even if it is identical to the value of the sent-by component". Without it the
    // response is routed by sent-by, at the sent-by port — which is the very thing `rport`
    // was sent to correct.
    let sent_by_matches = match &via.host {
        Host::Ip(ip) => *ip == source.ip(),
        Host::Name(_) => false,
    };
    let asked_for_rport = matches!(via.rport(), Some(None));
    if (asked_for_rport || !sent_by_matches) && via.received().is_none() {
        let addition = format!(";received={}", source.ip());
        // Before the parameters, not after: `received` conventionally sits next to the
        // sent-by, and inserting at a parameter boundary keeps the value well-formed however
        // many parameters follow.
        let insert_at =
            param_span(&updated_hop, b"branch").map_or(updated_hop.len(), |(start, _)| start);
        updated_hop.splice(insert_at..insert_at, addition.into_bytes());
        changed = true;
    }

    if !changed {
        return false;
    }

    let mut rebuilt = Vec::with_capacity(value.len() + 32);
    rebuilt.extend_from_slice(&updated_hop);
    rebuilt.extend_from_slice(value.get(hop_end..).unwrap_or(&[]));
    replace_top_via(request, Bytes::from(rebuilt));
    true
}

/// The span of `;name` or `;name=value` within one via-parm, quote-aware.
pub(crate) fn param_span(hop: &[u8], name: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0usize;
    while i < hop.len() {
        match hop.get(i) {
            Some(b'"') => {
                i = quoted_end(hop, i)?;
            }
            Some(b';') => {
                let start = i;
                let mut j = i + 1;
                while matches!(hop.get(j), Some(b' ' | b'\t')) {
                    j += 1;
                }
                let name_start = j;
                while hop
                    .get(j)
                    .is_some_and(|b| !matches!(b, b';' | b'=' | b' ' | b'\t'))
                {
                    j += 1;
                }
                let found = hop.get(name_start..j).unwrap_or(&[]);
                // Skip an `=value`, which may itself be quoted.
                let mut end = j;
                while matches!(hop.get(end), Some(b' ' | b'\t')) {
                    end += 1;
                }
                if hop.get(end) == Some(&b'=') {
                    end += 1;
                    while matches!(hop.get(end), Some(b' ' | b'\t')) {
                        end += 1;
                    }
                    if hop.get(end) == Some(&b'"') {
                        end = quoted_end(hop, end)?;
                    } else {
                        while hop.get(end).is_some_and(|&b| b != b';') {
                            end += 1;
                        }
                    }
                }
                if found.eq_ignore_ascii_case(name) {
                    return Some((start, end));
                }
                i = end;
            }
            Some(_) => i += 1,
            None => break,
        }
    }
    None
}

fn quoted_end(input: &[u8], at: usize) -> Option<usize> {
    let mut i = at + 1;
    while let Some(&b) = input.get(i) {
        match b {
            b'\\' if i + 1 < input.len() => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Replace the first `Via` header, keeping every other header where it was.
///
/// Was a rebuild of the whole collection — a fresh `Headers` and a clone of every header, to
/// change one — because that was the only thing the API allowed. `remove_first` plus `push_front`
/// says the same thing in two operations and clones nothing.
fn replace_top_via(request: &mut Request, value: Bytes) {
    let Ok(header) = Header::build(HeaderName::Via, value) else {
        return;
    };
    // Only when there is one to replace. The one caller today has already read the top `Via`, so
    // this cannot fire from there — it is the function's contract rather than a live guard, and it
    // is here because "replace" and "add" differ in a way that matters: the topmost `Via` is where
    // the response goes, so adding one to a request that had none redirects the answer.
    if request.headers.remove_first(&HeaderName::Via).is_some() {
        request.headers.push_front(header);
    }
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
    use sipx_sip::{Limits, Message, parse_datagram};

    fn request_with(via: &str) -> Request {
        let text = format!(
            "OPTIONS sip:a@b.com SIP/2.0\r\n\
             Via: {via}\r\n\
             To: <sip:a@b.com>\r\n\
             From: <sip:c@d.net>;tag=1\r\n\
             Call-ID: x@y\r\n\
             CSeq: 1 OPTIONS\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n"
        );
        match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses") {
            Message::Request(r) => r,
            Message::Response(_) => panic!("a request"),
        }
    }

    fn source() -> SocketAddr {
        "203.0.113.9:41234".parse().expect("valid")
    }

    fn top_via(request: &Request) -> String {
        String::from_utf8_lossy(&request.headers.value(&HeaderName::Via).expect("a Via"))
            .into_owned()
    }

    fn parsed_via(request: &Request) -> Via {
        request
            .headers
            .typed::<Via>()
            .expect("a Via")
            .expect("it parses")
    }

    #[test]
    fn received_is_added_when_the_source_differs_from_the_sent_by() {
        let mut request = request_with("SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKx");
        assert!(apply_received_and_rport(&mut request, source()));
        assert_eq!(
            parsed_via(&request).received().map(<[u8]>::to_vec),
            Some(b"203.0.113.9".to_vec())
        );
    }

    #[test]
    fn received_is_not_added_when_the_sent_by_is_already_right() {
        let mut request = request_with("SIP/2.0/UDP 203.0.113.9:41234;branch=z9hG4bKx");
        assert!(!apply_received_and_rport(&mut request, source()));
        assert!(!top_via(&request).contains("received"));
    }

    /// RFC 3581: an empty `rport` is a question, and the answer replaces it. Appending a
    /// second `rport` instead would leave the empty one first, where every reader looks.
    #[test]
    fn an_empty_rport_is_replaced_not_duplicated() {
        let mut request = request_with("SIP/2.0/UDP 10.0.0.5:5060;rport;branch=z9hG4bKx");
        assert!(apply_received_and_rport(&mut request, source()));

        let via = top_via(&request);
        assert_eq!(via.matches("rport").count(), 1, "exactly one rport: {via}");
        assert_eq!(
            parsed_via(&request).rport().flatten().map(<[u8]>::to_vec),
            Some(b"41234".to_vec())
        );
        assert_eq!(
            parsed_via(&request).branch().map(<[u8]>::to_vec),
            Some(b"z9hG4bKx".to_vec()),
            "the branch must survive the surgery"
        );
    }

    /// RFC 3581 §4 is explicit that asking for `rport` also asks for `received`: the server
    /// "MUST insert a `received` parameter containing the source IP address that the request
    /// came from, even if it is identical to the value of the `sent-by` component". Omitting
    /// it when the addresses agree leaves the response to be routed by sent-by, which is the
    /// port `rport` exists to correct.
    #[test]
    fn rport_brings_received_with_it_even_when_the_sent_by_matches() {
        let mut request = request_with("SIP/2.0/UDP 203.0.113.9:41234;rport;branch=z9hG4bKx");
        assert!(apply_received_and_rport(&mut request, source()));
        assert_eq!(
            parsed_via(&request).rport().flatten().map(<[u8]>::to_vec),
            Some(b"41234".to_vec())
        );
        assert_eq!(
            parsed_via(&request).received().map(<[u8]>::to_vec),
            Some(b"203.0.113.9".to_vec())
        );
    }

    #[test]
    fn an_absent_rport_is_not_invented() {
        let mut request = request_with("SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKx");
        apply_received_and_rport(&mut request, source());
        assert!(!top_via(&request).contains("rport"));
    }

    #[test]
    fn an_rport_that_already_has_a_value_is_left_alone() {
        let mut request = request_with("SIP/2.0/UDP 203.0.113.9:5060;rport=9999;branch=z9hG4bKx");
        apply_received_and_rport(&mut request, source());
        assert_eq!(
            parsed_via(&request).rport().flatten().map(<[u8]>::to_vec),
            Some(b"9999".to_vec())
        );
    }

    /// The hops below the top one belong to other elements and must be left exactly alone.
    #[test]
    fn only_the_topmost_hop_is_touched() {
        let mut request = request_with(
            "SIP/2.0/UDP 10.0.0.5:5060;rport;branch=z9hG4bK1, SIP/2.0/UDP 192.0.2.7:5060;branch=z9hG4bK2",
        );
        assert!(apply_received_and_rport(&mut request, source()));
        let via = top_via(&request);
        assert!(
            via.ends_with("SIP/2.0/UDP 192.0.2.7:5060;branch=z9hG4bK2"),
            "the second hop is untouched: {via}"
        );
        assert_eq!(via.matches("received").count(), 1);
        assert_eq!(
            parsed_via(&request).rport().flatten().map(<[u8]>::to_vec),
            Some(b"41234".to_vec()),
            "the top hop is the one that got the answer"
        );
    }

    #[test]
    fn every_other_header_keeps_its_place() {
        let mut request = request_with("SIP/2.0/UDP 10.0.0.5:5060;branch=z9hG4bKx");
        let before: Vec<String> = request
            .headers
            .iter()
            .map(|h| h.name().to_string())
            .collect();
        apply_received_and_rport(&mut request, source());
        let after: Vec<String> = request
            .headers
            .iter()
            .map(|h| h.name().to_string())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn parameter_spans_are_found_regardless_of_position_and_quoting() {
        let hop = br#"SIP/2.0/UDP h;a=1;rport;note="x;y";branch=z"#;
        let (start, end) = param_span(hop, b"rport").expect("rport is there");
        assert_eq!(&hop[start..end], b";rport");
        let (start, end) = param_span(hop, b"branch").expect("branch is there");
        assert_eq!(&hop[start..end], b";branch=z");
        let (start, end) = param_span(hop, b"note").expect("note is there");
        assert_eq!(&hop[start..end], br#";note="x;y""#);
        assert!(param_span(hop, b"absent").is_none());
    }

    /// `replace_top_via` replaces and does not add.
    ///
    /// Its one caller today has already read a `Via`, so it cannot reach this — which is exactly
    /// why the property is worth pinning here rather than left to that caller's shape. Adding a
    /// `Via` to a request that had none redirects the response, and the second caller is how that
    /// bug arrives.
    #[test]
    fn replacing_the_top_via_on_a_request_without_one_adds_nothing() {
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\n\
             To: <sip:a@b.com>\r\n\
             From: <sip:c@d.net>;tag=1\r\n\
             Call-ID: x@y\r\n\
             CSeq: 1 OPTIONS\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n";
        let mut request = match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram())
            .expect("parses")
        {
            Message::Request(r) => r,
            Message::Response(_) => panic!("a request"),
        };
        assert_eq!(request.headers.count(&HeaderName::Via), 0);

        replace_top_via(
            &mut request,
            Bytes::from_static(b"SIP/2.0/UDP invented.example"),
        );

        assert_eq!(
            request.headers.count(&HeaderName::Via),
            0,
            "a request with no Via must not acquire one; the topmost Via is where the response goes"
        );
    }
}
