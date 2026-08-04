//! Application-owned SIP fields accepted by the command line.
//!
//! Validation is deliberately complete before any command binds a transport. A malformed value
//! must be a usage error, not a call that starts and then discovers it cannot build its INVITE.

use bytes::Bytes;
use sipx_sip::{Header, HeaderName};

/// Parse every repeatable `--header 'Name: value'` occurrence in order.
pub(crate) fn from_args(args: &crate::Args<'_>) -> Result<Vec<Header>, String> {
    args.values("header").map(parse).collect()
}

pub(crate) fn parse(raw: &str) -> Result<Header, String> {
    let Some((name, value)) = raw.split_once(':') else {
        return Err("--header must be written 'Name: value'".to_owned());
    };
    let name = name.trim();
    if name.is_empty() {
        return Err("--header has an empty field name".to_owned());
    }
    let name = HeaderName::parse(&Bytes::copy_from_slice(name.as_bytes()));
    if stack_owned(&name) {
        return Err(format!(
            "--header cannot set stack-owned field {}",
            String::from_utf8_lossy(name.canonical())
        ));
    }
    Header::build(name, Bytes::copy_from_slice(value.trim().as_bytes()))
        .map_err(|error| format!("invalid --header: {error}"))
}

fn stack_owned(name: &HeaderName) -> bool {
    matches!(
        name,
        HeaderName::Via
            | HeaderName::Route
            | HeaderName::RecordRoute
            | HeaderName::MaxForwards
            | HeaderName::CallId
            | HeaderName::CSeq
            | HeaderName::From
            | HeaderName::To
            | HeaderName::Contact
            | HeaderName::ContentLength
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_custom_supported_field_is_validated_and_retained() {
        let header = parse("Supported: path, outbound").expect("valid field");
        assert_eq!(header.name(), &HeaderName::Supported);
        assert_eq!(header.raw_value(), b"path, outbound");
    }

    #[test]
    fn stack_owned_names_and_their_compact_forms_are_refused() {
        for raw in [
            "Via: injected",
            "v: injected",
            "Route: <sip:elsewhere>",
            "Record-Route: <sip:elsewhere>",
            "Max-Forwards: 1",
            "Call-ID: chosen",
            "i: chosen",
            "CSeq: 1 INVITE",
            "From: <sip:a@b>",
            "f: <sip:a@b>",
            "To: <sip:b@b>",
            "t: <sip:b@b>",
            "Contact: <sip:a@b>",
            "m: <sip:a@b>",
            "Content-Length: 0",
            "l: 0",
        ] {
            assert!(parse(raw).is_err(), "accepted {raw}");
        }
    }

    #[test]
    fn line_injection_and_malformed_names_are_refused() {
        assert!(parse("X-Test: ok\r\nVia: injected").is_err());
        assert!(parse("Bad Name: value").is_err());
        assert!(parse("missing-colon").is_err());
    }
}
