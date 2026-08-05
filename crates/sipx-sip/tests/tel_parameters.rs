//! Public RFC 3966 TEL-parameter vectors from `docs/specs/uri-rewriting.md` (S-49).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::{TelParameterErrorKind, Uri};

fn uri(raw: &'static [u8]) -> Uri {
    Uri::parse(Bytes::from_static(raw)).expect("vector URI parses")
}

fn exact_parameters(raw: &'static [u8]) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    uri(raw)
        .tel_parts()
        .expect("TEL parts")
        .parsed_parameters()
        .map(|parameter| {
            let parameter = parameter.expect("valid parameter tail");
            (
                parameter.name().to_vec(),
                parameter.value().map(<[u8]>::to_vec),
            )
        })
        .collect()
}

#[test]
fn ur_p_1_absent_tel_parameter_tail_is_an_empty_iterator() {
    assert!(exact_parameters(b"TEL:7042").is_empty());
}

#[test]
fn ur_p_2_phone_context_is_parser_owned_and_case_insensitive() {
    let value = uri(b"tel:7042;phone-context=example.com");
    let parameter = value
        .tel_parts()
        .expect("TEL parts")
        .parsed_parameters()
        .next()
        .expect("one parameter")
        .expect("valid parameter");

    assert_eq!(parameter.name(), b"phone-context");
    assert_eq!(parameter.value(), Some(&b"example.com"[..]));
    assert!(parameter.name_eq(b"PHONE-CONTEXT"));
    assert!(!parameter.name_eq(b""));
    assert!(!parameter.name_eq(b"phone_context"));
}

#[test]
fn ur_p_3_ext_alone_does_not_match_phone_context() {
    let value = uri(b"tel:7042;ext=9");
    let parameter = value
        .tel_parts()
        .expect("TEL parts")
        .parsed_parameters()
        .next()
        .expect("one parameter")
        .expect("valid parameter");

    assert!(parameter.name_eq(b"ext"));
    assert!(!parameter.name_eq(b"phone-context"));
}

#[test]
fn ur_p_4_reordered_parameters_retain_wire_order() {
    assert_eq!(
        exact_parameters(b"tel:7042;foo=x;phone-context=example.com;ext=9"),
        vec![
            (b"foo".to_vec(), Some(b"x".to_vec())),
            (b"phone-context".to_vec(), Some(b"example.com".to_vec())),
            (b"ext".to_vec(), Some(b"9".to_vec())),
        ]
    );
}

#[test]
fn ur_p_5_mixed_case_name_retains_bytes_and_compares_insensitively() {
    let value = uri(b"tel:7042;PhOnE-CoNtExT=example.com");
    let parameter = value
        .tel_parts()
        .expect("TEL parts")
        .parsed_parameters()
        .next()
        .expect("one parameter")
        .expect("valid parameter");

    assert_eq!(parameter.name(), b"PhOnE-CoNtExT");
    assert!(parameter.name_eq(b"phone-context"));
}

#[test]
fn ur_p_6_escaped_delimiters_remain_exact_value_bytes() {
    assert_eq!(
        exact_parameters(b"tel:7042;foo=a%3Bb%3Dc"),
        vec![(b"foo".to_vec(), Some(b"a%3Bb%3Dc".to_vec()))]
    );
}

#[test]
fn ur_p_7_duplicate_names_remain_separate_and_match_case_insensitively() {
    let value = uri(b"tel:7042;foo=one;FOO=two");
    let parameters: Vec<_> = value
        .tel_parts()
        .expect("TEL parts")
        .parsed_parameters()
        .map(|parameter| parameter.expect("valid parameter"))
        .collect();

    assert_eq!(parameters.len(), 2);
    assert!(parameters.iter().all(|parameter| parameter.name_eq(b"foo")));
    assert_eq!(parameters[0].value(), Some(&b"one"[..]));
    assert_eq!(parameters[1].value(), Some(&b"two"[..]));
}

#[test]
fn valueless_and_complete_paramchar_parameters_remain_exact() {
    assert_eq!(
        exact_parameters(b"tel:7042;flag;value=-._!~*'()[]/:&+$%3B"),
        vec![
            (b"flag".to_vec(), None),
            (b"value".to_vec(), Some(b"-._!~*'()[]/:&+$%3B".to_vec())),
        ]
    );
}

#[test]
fn ur_p_8_malformed_tail_yields_one_typed_error_then_fuses() {
    for (raw, expected_offset, expected_kind) in [
        (b"tel:7042;".as_slice(), 0, TelParameterErrorKind::Empty),
        (b"tel:7042;;ext=9", 0, TelParameterErrorKind::Empty),
        (b"tel:7042;=x", 0, TelParameterErrorKind::Name),
        (b"tel:7042;foo=", 4, TelParameterErrorKind::Value),
        (b"tel:7042;foo?=x", 3, TelParameterErrorKind::Name),
        (b"tel:7042;foo=a?", 5, TelParameterErrorKind::Value),
        (b"tel:7042;foo=a=b", 5, TelParameterErrorKind::Value),
    ] {
        let value = Uri::parse(Bytes::copy_from_slice(raw)).expect("TEL URI shell parses");
        let mut parameters = value.tel_parts().expect("TEL parts").parsed_parameters();
        let error = parameters
            .next()
            .expect("one error")
            .expect_err("malformed tail is rejected");

        assert_eq!(error.offset(), expected_offset, "tail {raw:?}");
        assert_eq!(error.kind(), expected_kind, "tail {raw:?}");
        assert!(
            parameters.next().is_none(),
            "iterator must fuse for {raw:?}"
        );
        assert!(
            parameters.next().is_none(),
            "iterator remains fused for {raw:?}"
        );
    }
}
