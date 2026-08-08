//! Message validation — the checks that come *after* parsing.
//!
//! A message can frame perfectly, have every header parse, and still be one no element may
//! act on: required headers missing, a `CSeq` naming a different method than the request
//! line, a version nobody speaks. RFC 4475 files these under the application layer, and so do
//! we, for a practical reason: answering `400` requires having parsed the message that is
//! wrong, and forwarding requires not caring.
//!
//! Validation therefore returns a *list* of findings rather than failing at the first. An
//! element picks a response from them; a proxy may ignore several of them entirely.

use crate::headers::{CSeq, CallId, From, MaxForwards, To, Via};
use crate::message::{Headers, Message, Request, Response, TypedHeader};
use crate::name::HeaderName;

/// Something wrong with a message that parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// A header RFC 3261 §8.1.1 requires is absent.
    MissingRequiredHeader(&'static str),
    /// A required header is present but its value does not parse.
    MalformedRequiredHeader(&'static str),
    /// A header the RFC permits at most once appears more than once (RFC 4475 §3.3.8).
    RepeatedSingleValueHeader(&'static str),
    /// The `CSeq` method does not match the request line (RFC 4475 §3.1.2.17, §3.1.2.18).
    CSeqMethodMismatch,
    /// The protocol version is one sipx does not speak; answer 505 (RFC 4475 §3.1.2.16).
    UnsupportedVersion,
    /// The Request-URI carries header components, which RFC 3261 §19.1.1 forbids
    /// (RFC 4475 §3.1.2.11).
    RequestUriHasHeaders,
}

impl Finding {
    /// The response status an element should send for this finding.
    #[must_use]
    pub fn status(&self) -> u16 {
        match self {
            Self::UnsupportedVersion => 505,
            _ => 400,
        }
    }

    /// Whether a proxy may reasonably forward the message anyway.
    ///
    /// A missing `Max-Forwards` is the one finding a proxy is explicitly allowed to repair
    /// rather than reject: RFC 3261 §16.6 step 3 says it MAY add the header itself. Every
    /// other finding means the message cannot be safely acted on.
    #[must_use]
    pub fn is_repairable(&self) -> bool {
        matches!(self, Self::MissingRequiredHeader("Max-Forwards"))
    }
}

fn check_required<H: TypedHeader>(headers: &Headers, label: &'static str, out: &mut Vec<Finding>) {
    match headers.typed::<H>() {
        None => out.push(Finding::MissingRequiredHeader(label)),
        Some(Err(_)) => out.push(Finding::MalformedRequiredHeader(label)),
        Some(Ok(_)) => {}
    }
}

fn check_single_value(
    headers: &Headers,
    name: &HeaderName,
    label: &'static str,
    out: &mut Vec<Finding>,
) {
    if headers.count(name) > 1 {
        out.push(Finding::RepeatedSingleValueHeader(label));
    }
}

/// Validate a request against RFC 3261 §8.1.1.
#[must_use]
pub fn validate_request(request: &Request) -> Vec<Finding> {
    let mut out = Vec::new();
    let headers = &request.headers;

    if !request.version.is_supported() {
        out.push(Finding::UnsupportedVersion);
    }
    if request.uri.has_headers() {
        out.push(Finding::RequestUriHasHeaders);
    }

    check_required::<To>(headers, "To", &mut out);
    check_required::<From>(headers, "From", &mut out);
    check_required::<CallId>(headers, "Call-ID", &mut out);
    check_required::<CSeq>(headers, "CSeq", &mut out);
    check_required::<MaxForwards>(headers, "Max-Forwards", &mut out);
    check_required::<Via>(headers, "Via", &mut out);

    check_single_value(headers, &HeaderName::To, "To", &mut out);
    check_single_value(headers, &HeaderName::From, "From", &mut out);
    check_single_value(headers, &HeaderName::CallId, "Call-ID", &mut out);
    check_single_value(headers, &HeaderName::CSeq, "CSeq", &mut out);
    check_single_value(headers, &HeaderName::MaxForwards, "Max-Forwards", &mut out);

    // The CSeq method must name the same method as the request line. A mismatch means one of
    // the two is a forgery or a bug, and either way the transaction it would create is not
    // the one the sender thinks.
    if let Some(Ok(cseq)) = headers.typed::<CSeq>()
        && cseq.method != request.method
    {
        out.push(Finding::CSeqMethodMismatch);
    }

    out
}

/// Validate a response.
///
/// A response has no Request-URI and no `Max-Forwards`, and its `CSeq` method names the
/// request it answers rather than anything on its own start line, so there is nothing to
/// cross-check there.
#[must_use]
pub fn validate_response(response: &Response) -> Vec<Finding> {
    let mut out = Vec::new();
    let headers = &response.headers;

    if !response.version.is_supported() {
        out.push(Finding::UnsupportedVersion);
    }

    check_required::<To>(headers, "To", &mut out);
    check_required::<From>(headers, "From", &mut out);
    check_required::<CallId>(headers, "Call-ID", &mut out);
    check_required::<CSeq>(headers, "CSeq", &mut out);
    check_required::<Via>(headers, "Via", &mut out);

    check_single_value(headers, &HeaderName::To, "To", &mut out);
    check_single_value(headers, &HeaderName::From, "From", &mut out);
    check_single_value(headers, &HeaderName::CallId, "Call-ID", &mut out);
    check_single_value(headers, &HeaderName::CSeq, "CSeq", &mut out);

    out
}

/// Validate whichever kind of message this is.
#[must_use]
pub fn validate(message: &Message) -> Vec<Finding> {
    match message {
        Message::Request(r) => validate_request(r),
        Message::Response(r) => validate_response(r),
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
    use crate::{Limits, parse_datagram};
    use bytes::Bytes;

    fn findings(text: &str) -> Vec<Finding> {
        let msg = parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram())
            .expect("should parse");
        validate(&msg)
    }

    const GOOD: &str = "OPTIONS sip:a@b.com SIP/2.0\r\n\
         Via: SIP/2.0/UDP h.example.com;branch=z9hG4bKx\r\n\
         To: <sip:a@b.com>\r\n\
         From: <sip:c@d.net>;tag=1\r\n\
         Call-ID: x@y\r\n\
         CSeq: 1 OPTIONS\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\r\n";

    #[test]
    fn a_well_formed_request_has_no_findings() {
        assert_eq!(findings(GOOD), Vec::new());
    }

    #[test]
    fn missing_required_headers_are_each_reported() {
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\nContent-Length: 0\r\n\r\n";
        let found = findings(text);
        for header in ["To", "From", "Call-ID", "CSeq", "Max-Forwards", "Via"] {
            assert!(
                found.contains(&Finding::MissingRequiredHeader(header)),
                "{header} should be reported missing"
            );
        }
    }

    #[test]
    fn a_cseq_method_mismatch_is_reported() {
        let text = GOOD.replace("CSeq: 1 OPTIONS", "CSeq: 1 INVITE");
        assert!(findings(&text).contains(&Finding::CSeqMethodMismatch));
    }

    #[test]
    fn an_unsupported_version_asks_for_505_not_400() {
        let text = GOOD.replace("SIP/2.0\r\nVia", "SIP/7.0\r\nVia");
        let found = findings(&text);
        assert!(found.contains(&Finding::UnsupportedVersion));
        assert_eq!(Finding::UnsupportedVersion.status(), 505);
    }

    #[test]
    fn a_repeated_single_value_header_is_reported() {
        let text = GOOD.replace(
            "To: <sip:a@b.com>",
            "To: <sip:a@b.com>\r\nTo: <sip:e@f.org>",
        );
        assert!(findings(&text).contains(&Finding::RepeatedSingleValueHeader("To")));
    }

    #[test]
    fn a_malformed_required_header_is_not_reported_as_missing() {
        // The distinction the whole layering exists to preserve: present-and-broken is not
        // the same as absent, and an element that conflates them answers the wrong thing.
        let text = GOOD.replace("CSeq: 1 OPTIONS", "CSeq: 99999999999 OPTIONS");
        let found = findings(&text);
        assert!(found.contains(&Finding::MalformedRequiredHeader("CSeq")));
        assert!(!found.contains(&Finding::MissingRequiredHeader("CSeq")));
    }

    #[test]
    fn a_request_uri_with_headers_is_reported() {
        let text = GOOD.replace(
            "OPTIONS sip:a@b.com SIP/2.0",
            "OPTIONS sip:a@b.com?Route=%3Csip:x%3E SIP/2.0",
        );
        assert!(findings(&text).contains(&Finding::RequestUriHasHeaders));
    }

    #[test]
    fn a_missing_max_forwards_is_the_one_repairable_finding() {
        assert!(Finding::MissingRequiredHeader("Max-Forwards").is_repairable());
        assert!(!Finding::MissingRequiredHeader("Via").is_repairable());
        assert!(!Finding::CSeqMethodMismatch.is_repairable());
    }
}
