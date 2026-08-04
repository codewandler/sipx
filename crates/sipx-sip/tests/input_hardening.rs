//! Named regression tests for the malformed-input properties in X-64.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::error::BuildError;
use sipx_sip::{
    Finding, HeaderName, Limits, Message, StatusCode, parse_datagram, validate_request,
};

const ANSWERABLE: &str = "OPTIONS sip:a@b.com SIP/2.0\r\n\
     Via: SIP/2.0/UDP h.example.com;branch=z9hG4bKx\r\n\
     To: <sip:a@b.com>\r\n\
     From: <sip:c@d.net>;tag=1\r\n\
     Call-ID: x@y\r\n\
     CSeq: 1 OPTIONS\r\n\
     Max-Forwards: 70\r\n\
     Content-Length: 0\r\n\r\n";

/// RFC 3261 §8.2.6.1 and §8.2.6.2: a response needs the complete Via stack, To, From,
/// Call-ID and `CSeq`. Every public response-construction entry reports a typed error when one is
/// absent; silently producing an unrouteable response is not a refusal.
#[test]
fn response_construction_refuses_each_missing_required_header_by_name() {
    let required = [
        (HeaderName::Via, "Via"),
        (HeaderName::To, "To"),
        (HeaderName::From, "From"),
        (HeaderName::CallId, "Call-ID"),
        (HeaderName::CSeq, "CSeq"),
    ];

    for (name, label) in required {
        let message = parse_datagram(
            Bytes::from_static(ANSWERABLE.as_bytes()),
            &Limits::datagram(),
        )
        .expect("the control request frames");
        let Message::Request(mut request) = message else {
            panic!("the control message is a request");
        };
        request.headers.remove_all(&name);

        assert!(
            validate_request(&request).contains(&Finding::MissingRequiredHeader(label)),
            "validation did not name missing {label}"
        );
        let error = ResponseBuilder::to_request(
            &request,
            StatusCode::new(400).expect("status"),
            "Bad Request",
        )
        .expect_err("response construction must refuse the missing header");
        assert_eq!(
            error,
            BuildError::MissingRequiredResponseHeader { header: label },
            "response construction did not name missing {label}"
        );
    }
}
