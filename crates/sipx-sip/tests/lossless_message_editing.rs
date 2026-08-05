//! Byte vectors for parser-owned request-line and address-field URI editing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::{
    AddressEditError, Header, HeaderName, Host, HostName, Limits, Message, Method, Request, Uri,
    parse_datagram,
};

fn uri(value: &'static [u8]) -> Uri {
    Uri::parse(Bytes::from_static(value)).unwrap()
}

fn request(input: &'static [u8]) -> Request {
    match parse_datagram(Bytes::from_static(input), &Limits::datagram()).unwrap() {
        Message::Request(request) => request,
        Message::Response(_) => panic!("expected request"),
    }
}

fn request_bytes(request: &Request) -> Bytes {
    let mut out = Vec::new();
    request.write_to(&mut out);
    Bytes::from(out)
}

#[test]
fn lm_1_and_2_request_uri_replacement_retains_the_start_line_twice() {
    let mut request = request(b"iNvItE sip:old@EXAMPLE.test SiP/2.0\r\nContent-Length: 0\r\n\r\n");
    request.set_uri(uri(b"sips:new@example.net")).unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(b"iNvItE sips:new@example.net SiP/2.0\r\nContent-Length: 0\r\n\r\n")
    );

    request.set_uri(uri(b"tel:+12025550123")).unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(b"iNvItE tel:+12025550123 SiP/2.0\r\nContent-Length: 0\r\n\r\n")
    );
}

#[test]
fn lm_3_built_request_replacement_is_deterministic() {
    let host = Host::Name(HostName::new("b").unwrap());
    let mut request = Request::new(Method::Options, Uri::sip(host));
    request.set_uri(uri(b"sip:c@d")).unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(b"OPTIONS sip:c@d SIP/2.0\r\n\r\n")
    );
}

#[test]
fn lm_4_ambiguous_display_text_is_not_mistaken_for_the_uri() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nt : \"sip:old@h\" <sip:old@h>;tag=x\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_address_uri(&HeaderName::To, 0, &uri(b"sip:new@h"))
        .unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nt : \"sip:old@h\" <sip:new@h>;tag=x\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn lm_5_fold_outside_the_uri_is_preserved() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nTo:\tAlice\r\n \t<sip:old@h> ; tag=x\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_address_uri(&HeaderName::To, 0, &uri(b"sip:new@h"))
        .unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nTo:\tAlice\r\n \t<sip:new@h> ; tag=x\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn lm_6_and_7_indices_cover_comma_lists_and_repeated_rows() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:a@h>,  <sip:b@h>,<sip:c@h>\r\nP-Asserted-Identity: <sip:first@h>, <tel:+12025550101>\r\nP-Asserted-Identity:\t\"third\" <sip:third@h>\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_address_uri(&HeaderName::Route, 1, &uri(b"sips:b@n"))
        .unwrap();
    request
        .headers
        .replace_address_uri(&HeaderName::PAssertedIdentity, 2, &uri(b"sip:changed@n"))
        .unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:a@h>,  <sips:b@n>,<sip:c@h>\r\nP-Asserted-Identity: <sip:first@h>, <tel:+12025550101>\r\nP-Asserted-Identity:\t\"third\" <sip:changed@n>\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn lm_8_and_9_remove_middle_then_last_without_rebuilding_survivors() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:a@h>,  <sip:b@h>, <sip:c@h>\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .remove_address_value(&HeaderName::Route, 1)
        .unwrap();
    assert_eq!(
        request.headers.value(&HeaderName::Route).as_deref(),
        Some(&b"<sip:a@h>,  <sip:c@h>"[..])
    );
    request
        .headers
        .remove_address_value(&HeaderName::Route, 1)
        .unwrap();
    assert_eq!(
        request.headers.value(&HeaderName::Route).as_deref(),
        Some(&b"<sip:a@h>"[..])
    );
}

#[test]
fn lm_10_removing_a_rows_only_value_removes_only_that_row() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nP-Preferred-Identity : <sip:first@h>\r\nP-Preferred-Identity:\t\"second\" <sip:second@h>\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .remove_address_value(&HeaderName::PPreferredIdentity, 0)
        .unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nP-Preferred-Identity:\t\"second\" <sip:second@h>\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn lm_11_to_13_failures_are_typed_and_atomic() {
    let mut malformed = Header::build(HeaderName::To, "not an address").unwrap();
    let before = {
        let mut bytes = Vec::new();
        malformed.write_to(&mut bytes);
        bytes
    };
    assert!(matches!(
        malformed.replace_address_uri(0, &uri(b"sip:new@h")),
        Err(AddressEditError::Malformed(_))
    ));
    let mut after = Vec::new();
    malformed.write_to(&mut after);
    assert_eq!(after, before);

    let mut unsupported = Header::build(HeaderName::Subject, "sip:a@h").unwrap();
    assert!(matches!(
        unsupported.replace_address_uri(0, &uri(b"sip:new@h")),
        Err(AddressEditError::UnsupportedHeader)
    ));

    let mut out_of_range = Header::build(HeaderName::From, "<sip:a@h>").unwrap();
    assert!(matches!(
        out_of_range.replace_address_uri(1, &uri(b"sip:new@h")),
        Err(AddressEditError::IndexOutOfRange { index: 1 })
    ));
}

#[test]
fn lm_14_removal_projects_a_folded_separator_back_to_wire_bytes() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:a@h>,\r\n \t<sip:b@h>\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .remove_address_value(&HeaderName::Route, 0)
        .unwrap();
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:b@h>\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn every_existing_typed_address_field_uses_the_shared_surgery() {
    let names = [
        HeaderName::From,
        HeaderName::To,
        HeaderName::Contact,
        HeaderName::Route,
        HeaderName::RecordRoute,
        HeaderName::Path,
        HeaderName::ServiceRoute,
        HeaderName::PAssertedIdentity,
        HeaderName::PPreferredIdentity,
    ];

    for name in names {
        let mut header = Header::build(name, "\"sip:old@h\" <sip:old@h>;x=y").unwrap();
        header.replace_address_uri(0, &uri(b"sips:new@n")).unwrap();
        assert_eq!(
            header.raw_value(),
            b"\"sip:old@h\" <sips:new@n>;x=y",
            "{}",
            String::from_utf8_lossy(header.name().canonical())
        );
    }
}

#[test]
fn lm_15_bare_address_delimiters_are_rejected_atomically() {
    let cases = [
        (HeaderName::To, uri(b"sip:new@h;transport=tcp"), "semicolon"),
        (HeaderName::To, uri(b"sip:new@h?subject=x"), "query"),
        (HeaderName::Contact, uri(b"sip:new,part@h"), "comma"),
    ];

    for (name, replacement, delimiter) in cases {
        let mut header = Header::build(name, "sip:old@h").unwrap();
        let before = header.raw_value().to_vec();
        assert!(
            matches!(
                header.replace_address_uri(0, &replacement),
                Err(AddressEditError::Malformed(_))
            ),
            "bare {delimiter} replacement must be refused"
        );
        assert_eq!(header.raw_value(), before, "{delimiter} failure is atomic");
    }
}

#[test]
fn lm_16_name_addr_delimiters_are_replaced_exactly() {
    let cases = [
        (
            HeaderName::To,
            uri(b"sip:new@h;transport=tcp"),
            b"<sip:new@h;transport=tcp>".as_slice(),
        ),
        (
            HeaderName::To,
            uri(b"sip:new@h?subject=x"),
            b"<sip:new@h?subject=x>".as_slice(),
        ),
        (
            HeaderName::Contact,
            uri(b"sip:new,part@h"),
            b"<sip:new,part@h>".as_slice(),
        ),
    ];

    for (name, replacement, expected) in cases {
        let mut header = Header::build(name, "<sip:old@h>").unwrap();
        header.replace_address_uri(0, &replacement).unwrap();
        assert_eq!(header.raw_value(), expected);
    }
}

#[test]
fn lm_17_collection_preflight_makes_a_later_malformed_row_atomic() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:first@h>\r\nRoute: not-an-address\r\nContent-Length: 0\r\n\r\n",
    );
    let before = request_bytes(&request);
    assert!(matches!(
        request
            .headers
            .replace_address_uri(&HeaderName::Route, 0, &uri(b"sip:new@h")),
        Err(AddressEditError::Malformed(_))
    ));
    assert_eq!(request_bytes(&request), before);
}

#[test]
fn lm_18_final_removal_preserves_trailing_lws_and_fold() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:a@h>, <sip:b@h>\t \r\n \t\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .remove_address_value(&HeaderName::Route, 1)
        .unwrap();
    let route = request.headers.get(&HeaderName::Route).unwrap();
    assert_eq!(route.raw_value(), b"<sip:a@h>\t \r\n \t");
}
