//! Public proofs for the S-44 URI rewriting contract.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use sipx_sip::{Param, Uri, UriError};

fn uri(raw: &'static [u8]) -> Uri {
    Uri::parse(Bytes::from_static(raw)).expect("vector URI parses")
}

#[test]
fn ur_u_1_replaces_only_the_structured_sip_user() {
    let mut value = uri(b"sip:old:secret@example.com:5070;transport=tcp?subject=x");

    assert_eq!(
        value.replace_user(Bytes::from_static(b"new%2Buser")),
        Ok(true)
    );
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(b"sip:new%2Buser:secret@example.com:5070;transport=tcp?subject=x")
    );
    assert_eq!(value.password(), Some(&b"secret"[..]));
    assert_eq!(value.port(), Some(5070));
}

#[test]
fn ur_u_2_accepts_every_literal_user_delimiter() {
    let mut value = uri(b"sips:old@example.com");

    assert_eq!(
        value.replace_user(Bytes::from_static(b"7042;isub=9?x/y")),
        Ok(true)
    );
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(b"sips:7042;isub=9?x/y@example.com")
    );
    assert_eq!(value.user(), Some(&b"7042;isub=9?x/y"[..]));
}

#[test]
fn ur_u_3_illegal_users_are_typed_and_atomic() {
    for replacement in [
        &b""[..],
        &b"a@b"[..],
        &b"a:b"[..],
        &b"a b"[..],
        &b"a\r\nb"[..],
        &[0xff][..],
    ] {
        let original = Bytes::from_static(b"SIP:old@[2001:0DB8:0:0:0:0:0:1]");
        let mut value = Uri::parse(original.clone()).expect("vector URI parses");

        assert_eq!(
            value.replace_user(Bytes::copy_from_slice(replacement)),
            Err(UriError::User),
            "replacement {replacement:?}"
        );
        assert_eq!(
            value.to_bytes(),
            original,
            "failure must retain verbatim bytes"
        );
        assert_eq!(value.user(), Some(&b"old"[..]));
    }
}

#[test]
fn ur_u_4_malformed_escapes_are_typed_and_atomic() {
    for replacement in [&b"bad%2"[..], &b"bad%xx"[..], &b"bad%"[..]] {
        let original = Bytes::from_static(b"SIPS:old@example.com");
        let mut value = Uri::parse(original.clone()).expect("vector URI parses");

        assert_eq!(
            value.replace_user(Bytes::copy_from_slice(replacement)),
            Err(UriError::PercentEscape),
            "replacement {replacement:?}"
        );
        assert_eq!(
            value.to_bytes(),
            original,
            "failure must retain verbatim bytes"
        );
    }
}

#[test]
fn ur_u_5_opaque_schemes_are_unchanged_without_validating_a_sip_user() {
    let original = Bytes::from_static(b"TEL:+1-201-555-0123;ext=9");
    let mut value = Uri::parse(original.clone()).expect("vector URI parses");

    assert_eq!(
        value.replace_user(Bytes::from_static(b"bad\r\nuser")),
        Ok(false)
    );
    assert_eq!(value.to_bytes(), original);
}

#[test]
fn ur_u_6_success_rewrites_only_the_parser_owned_user_span() {
    let mut value =
        uri(b"SiPs:old:p%61ss@[2001:0DB8:0:0:0:0:0:1]:05061;Transport=TCP;foo=%2f?Subject=X&x=%2F");

    assert_eq!(value.replace_user(Bytes::from_static(b"n%65w")), Ok(true));
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(
            b"SiPs:n%65w:p%61ss@[2001:0DB8:0:0:0:0:0:1]:05061;Transport=TCP;foo=%2f?Subject=X&x=%2F"
        )
    );

    // The retained span moves with a different-length replacement; a second edit must still
    // leave every byte outside that span exact.
    assert_eq!(value.replace_user(Bytes::from_static(b"x")), Ok(true));
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(
            b"SiPs:x:p%61ss@[2001:0DB8:0:0:0:0:0:1]:05061;Transport=TCP;foo=%2f?Subject=X&x=%2F"
        )
    );
}

#[test]
fn ur_u_7_a_uri_without_userinfo_is_unchanged() {
    let original = Bytes::from_static(b"SIP:ExAmPlE.COM:05060;Transport=UDP?Subject=X");
    let mut value = Uri::parse(original.clone()).expect("vector URI parses");

    assert_eq!(
        value.replace_user(Bytes::from_static(b"bad\r\nuser")),
        Ok(false)
    );
    assert_eq!(value.to_bytes(), original);
}

#[test]
fn ur_u_8_replacement_after_general_mutation_uses_structured_serialization() {
    let mut value = uri(b"SIP:old@[2001:0DB8:0:0:0:0:0:1]");
    value.push_param(Param::flag(Bytes::from_static(b"lr")));

    assert_eq!(value.replace_user(Bytes::from_static(b"new")), Ok(true));
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(b"sip:new@[2001:db8::1];lr")
    );
}

#[test]
fn ur_u_9_empty_userinfo_is_rejected_before_it_can_become_a_span() {
    for raw in [b"sip:@example.com".as_slice(), b"sip::password@example.com"] {
        let parsed = Uri::parse(Bytes::copy_from_slice(raw));
        assert_eq!(
            parsed.as_ref().err(),
            Some(&UriError::User),
            "empty userinfo in {raw:?}"
        );
    }
}

#[test]
fn ur_t_1_tel_parts_borrow_exact_subscriber_and_parameter_bytes() {
    let value = uri(b"tel:+1-201-555-0123;ext=9;Phone-Context=+1-201");
    let parts = value.tel_parts().expect("TEL parts");

    assert_eq!(parts.subscriber(), b"+1-201-555-0123");
    assert_eq!(parts.parameters(), Some(&b"ext=9;Phone-Context=+1-201"[..]));
}

#[test]
fn ur_t_2_tel_without_parameters_has_no_tail() {
    let value = uri(b"TEL:7042");
    let parts = value.tel_parts().expect("TEL parts");

    assert_eq!(parts.subscriber(), b"7042");
    assert_eq!(parts.parameters(), None);
}

#[test]
fn ur_t_3_tel_trailing_separator_retains_an_empty_tail() {
    let value = uri(b"tel:7042;");
    let parts = value.tel_parts().expect("TEL parts");

    assert_eq!(parts.subscriber(), b"7042");
    assert_eq!(parts.parameters(), Some(&b""[..]));
}

#[test]
fn ur_t_4_non_tel_schemes_have_no_tel_parts() {
    for raw in [
        b"sip:7042@example.com;user=phone".as_slice(),
        b"urn:7042;ext=9",
    ] {
        assert!(
            Uri::parse(Bytes::copy_from_slice(raw))
                .expect("vector URI parses")
                .tel_parts()
                .is_none()
        );
    }
}

#[test]
fn ur_t_5_tel_replacement_splices_only_the_parser_owned_subscriber_span() {
    let mut value = uri(b"TeL:+1-(201)-555-0123;Ext=9;Phone-Context=+1-201");

    assert_eq!(
        value.replace_tel_subscriber(Bytes::from_static(b"+49-30-123456")),
        Ok(true)
    );
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(b"TeL:+49-30-123456;Ext=9;Phone-Context=+1-201")
    );

    assert_eq!(
        value.replace_tel_subscriber(Bytes::from_static(b"7042")),
        Ok(true)
    );
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(b"TeL:7042;Ext=9;Phone-Context=+1-201")
    );
    let parts = value.tel_parts().expect("TEL parts after replacement");
    assert_eq!(parts.subscriber(), b"7042");
    assert_eq!(parts.parameters(), Some(&b"Ext=9;Phone-Context=+1-201"[..]));

    assert_eq!(
        value.replace_tel_subscriber(Bytes::from_static(b"*#")),
        Ok(true)
    );
    assert_eq!(
        value.to_bytes(),
        Bytes::from_static(b"TeL:*#;Ext=9;Phone-Context=+1-201")
    );
}

#[test]
fn ur_t_6_invalid_tel_subscribers_are_typed_and_atomic() {
    for replacement in [
        &b""[..],
        &b"+"[..],
        &b"+12A"[..],
        &b"12G"[..],
        &b"12:34"[..],
        &b"12 34"[..],
        &b"12\r\n34"[..],
        &[0xff][..],
    ] {
        let original = Bytes::from_static(b"TEL:+1-201-555-0123;Ext=9");
        let mut value = Uri::parse(original.clone()).expect("vector URI parses");

        assert_eq!(
            value.replace_tel_subscriber(Bytes::copy_from_slice(replacement)),
            Err(UriError::TelephoneSubscriber),
            "replacement {replacement:?}"
        );
        assert_eq!(value.to_bytes(), original, "failure must be atomic");
        assert_eq!(
            value.tel_parts().map(|parts| parts.subscriber()),
            Some(&b"+1-201-555-0123"[..])
        );
    }
}

#[test]
fn ur_t_7_non_tel_schemes_are_unchanged_without_validating_a_subscriber() {
    for original in [
        Bytes::from_static(b"SIP:7042@example.com"),
        Bytes::from_static(b"UrN:example:7042;Ext=9"),
    ] {
        let mut value = Uri::parse(original.clone()).expect("vector URI parses");

        assert_eq!(
            value.replace_tel_subscriber(Bytes::from_static(b"bad\r\nsubscriber")),
            Ok(false)
        );
        assert_eq!(value.to_bytes(), original);
    }
}

#[test]
fn ur_t_8_parsing_rejects_an_invalid_tel_subscriber() {
    for raw in [b"tel:".as_slice(), b"tel:+"] {
        assert_eq!(
            Uri::parse(Bytes::copy_from_slice(raw)).err(),
            Some(UriError::TelephoneSubscriber),
            "subscriber in {raw:?}"
        );
    }
}

#[test]
fn ur_o_1_opaque_uris_validate_percent_escape_shape() {
    assert_eq!(
        Uri::parse(Bytes::from_static(b"mailto:%GG")).err(),
        Some(UriError::PercentEscape)
    );

    let valid = Bytes::from_static(b"mailto:alice%40example.com");
    assert_eq!(
        Uri::parse(valid.clone()).map(|uri| uri.to_bytes()),
        Ok(valid)
    );
}
