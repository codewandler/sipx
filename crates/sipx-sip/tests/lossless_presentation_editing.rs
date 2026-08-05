//! Public lossless presentation-editing vectors from `docs/specs/lossless-presentation-editing.md`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::{
    AddressEditError, Header, HeaderName, Headers, Limits, Message, Request, Uri, WarningEditError,
    parse_datagram,
};

fn uri(value: &'static [u8]) -> Uri {
    Uri::parse(Bytes::from_static(value)).expect("vector URI parses")
}

fn request(input: &'static [u8]) -> Request {
    match parse_datagram(Bytes::from_static(input), &Limits::datagram()).expect("request parses") {
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
fn anonymous_presentation_retains_tag_exactly() {
    let mut headers = Headers::new();
    headers.push(
        Header::build(HeaderName::From, "\"Anna\" <sip:a@old.example>;tag=a1").expect("header"),
    );
    headers
        .replace_address_presentation(
            &HeaderName::From,
            0,
            Some("Anonymous"),
            &uri(b"sip:anonymous@anonymous.invalid"),
        )
        .expect("edit");
    assert_eq!(
        headers.value(&HeaderName::From).as_deref(),
        Some(&b"\"Anonymous\" <sip:anonymous@anonymous.invalid>;tag=a1"[..])
    );
}

#[test]
fn fold_and_escaped_parameter_tail_survive_presentation_edit() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nFrom: \"A\\\" B\"<sip:a@old.example>\r\n \t;TaG=a1;note=\"x\\\";y\"\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_address_presentation(
            &HeaderName::From,
            0,
            Some("Anonymous"),
            &uri(b"sip:anonymous@anonymous.invalid"),
        )
        .expect("edit");
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nFrom: \"Anonymous\" <sip:anonymous@anonymous.invalid>\r\n \t;TaG=a1;note=\"x\\\";y\"\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn bare_address_becomes_name_address_without_rebuilding_parameters() {
    let mut header = Header::build(
        HeaderName::From,
        "sip:a@old.example ;tag=a1;opaque=\"a\\\\b\"",
    )
    .expect("header");
    header
        .replace_address_presentation(
            0,
            Some("Anonymous"),
            &uri(b"sip:anonymous@anonymous.invalid"),
        )
        .expect("edit");
    assert_eq!(
        header.raw_value(),
        b"\"Anonymous\" <sip:anonymous@anonymous.invalid> ;tag=a1;opaque=\"a\\\\b\""
    );
}

#[test]
fn flattened_list_edit_quotes_replacement_display_name() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nRoute: <sip:a@h>,\r\n \t\"old\" <sip:b@h>;x=\"q\\\"r\"\r\nRoute: <sip:c@h>\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_address_presentation(&HeaderName::Route, 1, Some("A \"B\\C"), &uri(b"sips:b@n"))
        .expect("edit");
    assert_eq!(
        request
            .headers
            .get(&HeaderName::Route)
            .expect("row")
            .raw_value(),
        b"<sip:a@h>,\r\n \t\"A \\\"B\\\\C\" <sips:b@n>;x=\"q\\\"r\""
    );
}

#[test]
fn malformed_later_row_makes_collection_edit_atomic() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nFrom: <sip:first@h>;tag=x\r\nFrom: \"unterminated <sip:bad@h>\r\nContent-Length: 0\r\n\r\n",
    );
    let before = request_bytes(&request);
    assert!(matches!(
        request.headers.replace_address_presentation(
            &HeaderName::From,
            0,
            Some("Anonymous"),
            &uri(b"sip:anonymous@anonymous.invalid"),
        ),
        Err(AddressEditError::Malformed(_))
    ));
    assert_eq!(request_bytes(&request), before);
}

#[test]
fn unterminated_old_display_name_is_typed_and_atomic() {
    let mut header =
        Header::build(HeaderName::From, "\"unterminated <sip:a@h>;tag=x").expect("raw field");
    let before = header.raw_value().to_vec();
    assert!(matches!(
        header.replace_address_presentation(
            0,
            Some("Anonymous"),
            &uri(b"sip:anonymous@anonymous.invalid"),
        ),
        Err(AddressEditError::Malformed(_))
    ));
    assert_eq!(header.raw_value(), before);
}

#[test]
fn hostile_replacement_display_names_are_refused_atomically() {
    for display_name in ["line\r\nbreak", "nul\0byte", "delete\u{7f}", "tab\tbyte"] {
        let mut header = Header::build(HeaderName::From, "<sip:a@h>;tag=x").expect("header");
        let before = header.raw_value().to_vec();
        assert!(matches!(
            header.replace_address_presentation(
                0,
                Some(display_name),
                &uri(b"sip:anonymous@anonymous.invalid"),
            ),
            Err(AddressEditError::InvalidDisplayName)
        ));
        assert_eq!(header.raw_value(), before);
    }
}

#[test]
fn delimiter_rich_uri_reparses_inside_name_address() {
    let mut header = Header::build(HeaderName::From, "sip:old@h ;tag=x").expect("header");
    header
        .replace_address_presentation(0, None, &uri(b"sip:new,part@h;transport=tcp?subject=x"))
        .expect("edit");
    assert_eq!(
        header.raw_value(),
        b"<sip:new,part@h;transport=tcp?subject=x> ;tag=x"
    );
}

#[test]
fn every_supported_address_field_uses_the_presentation_span() {
    for name in [
        HeaderName::From,
        HeaderName::To,
        HeaderName::Contact,
        HeaderName::Route,
        HeaderName::RecordRoute,
        HeaderName::Path,
        HeaderName::ServiceRoute,
        HeaderName::PAssertedIdentity,
        HeaderName::PPreferredIdentity,
    ] {
        let mut header = Header::build(name, "old <sip:old@h>;x=y").expect("header");
        header
            .replace_address_presentation(0, Some("Néw"), &uri(b"sips:new@n"))
            .expect("edit");
        assert_eq!(header.raw_value(), "\"Néw\" <sips:new@n>;x=y".as_bytes());
    }
}

#[test]
fn warning_agent_becomes_anonymous_without_touching_code_or_text() {
    let mut headers = Headers::new();
    headers.push(
        Header::build(
            HeaderName::Warning,
            "399 pbx.acme.example \"Media downgraded\"",
        )
        .expect("header"),
    );
    headers
        .replace_warning_agent_with_pseudonym(0, b"anonymous")
        .expect("edit");
    assert_eq!(
        headers.value(&HeaderName::Warning).as_deref(),
        Some(&b"399 anonymous \"Media downgraded\""[..])
    );
}

#[test]
fn warning_agent_edit_retains_fold_and_escaped_text() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 399 pbx.example:5060\r\n \t\"Media \\\"downgraded\\\"\"\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_warning_agent_with_pseudonym(0, b"anonymous")
        .expect("edit");
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 399 anonymous\r\n \t\"Media \\\"downgraded\\\"\"\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn flattened_warning_list_edit_handles_quoted_commas_and_ipv6() {
    let mut request = request(
        b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 399 old.example \"comma, and \\\"quote\\\"\", 301 [2001:db8::1]:5060 \"Second\"\r\nWarning: 307 other.example \"Third\"\r\nContent-Length: 0\r\n\r\n",
    );
    request
        .headers
        .replace_warning_agent_with_pseudonym(1, b"anonymous")
        .expect("edit");
    assert_eq!(
        request_bytes(&request),
        Bytes::from_static(
            b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 399 old.example \"comma, and \\\"quote\\\"\", 301 anonymous \"Second\"\r\nWarning: 307 other.example \"Third\"\r\nContent-Length: 0\r\n\r\n"
        )
    );
}

#[test]
fn already_anonymous_warning_is_byte_identical() {
    let mut header =
        Header::build(HeaderName::Warning, "399 anonymous \"already private\"").expect("header");
    let before = header.raw_value().to_vec();
    header
        .replace_warning_agent_with_pseudonym(0, b"anonymous")
        .expect("edit");
    assert_eq!(header.raw_value(), before);
}

#[test]
fn agentless_warning_is_malformed_not_already_anonymous() {
    let mut headers = Headers::new();
    headers.push(Header::build(HeaderName::Warning, "399 \"missing agent\"").expect("raw header"));
    let before = headers.value(&HeaderName::Warning).expect("value").to_vec();
    assert!(matches!(
        headers.replace_warning_agent_with_pseudonym(0, b"anonymous"),
        Err(WarningEditError::Malformed(_))
    ));
    assert_eq!(
        headers.value(&HeaderName::Warning).as_deref(),
        Some(before.as_slice())
    );
}

#[test]
fn bad_code_text_or_later_row_makes_edit_atomic() {
    let cases: &[&[u8]] = &[
        b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 39A old.example \"bad code\"\r\nContent-Length: 0\r\n\r\n",
        b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 399 old.example \"unterminated\r\nContent-Length: 0\r\n\r\n",
        b"OPTIONS sip:x@h SIP/2.0\r\nWarning: 399 good.example \"good\"\r\nWarning: 399 \"missing agent\"\r\nContent-Length: 0\r\n\r\n",
    ];
    for &input in cases {
        let mut request = request(input);
        let before = request_bytes(&request);
        assert!(matches!(
            request
                .headers
                .replace_warning_agent_with_pseudonym(0, b"anonymous"),
            Err(WarningEditError::Malformed(_))
        ));
        assert_eq!(request_bytes(&request), before);
    }
}

#[test]
fn hostile_pseudonyms_are_refused_atomically() {
    let pseudonyms: &[&[u8]] = &[
        b"",
        b"not anonymous",
        b",",
        b"line\r\nbreak",
        b"nul\0byte",
        b"(",
        b"non\xfftoken",
    ];
    for &pseudonym in pseudonyms {
        let mut header = Header::build(HeaderName::Warning, "399 old.example \"Media downgraded\"")
            .expect("header");
        let before = header.raw_value().to_vec();
        assert_eq!(
            header.replace_warning_agent_with_pseudonym(0, pseudonym),
            Err(WarningEditError::InvalidPseudonym)
        );
        assert_eq!(header.raw_value(), before);
    }
}

#[test]
fn warning_index_past_complete_field_is_typed() {
    let mut headers = Headers::new();
    headers.push(
        Header::build(HeaderName::Warning, "399 old.example \"Media downgraded\"").expect("header"),
    );
    let before = headers.value(&HeaderName::Warning).expect("value").to_vec();
    assert_eq!(
        headers.replace_warning_agent_with_pseudonym(1, b"anonymous"),
        Err(WarningEditError::IndexOutOfRange { index: 1 })
    );
    assert_eq!(
        headers.value(&HeaderName::Warning).as_deref(),
        Some(before.as_slice())
    );
}
