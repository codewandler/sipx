//! RFC 3325 §§9.1–9.2 asserted and preferred identity value-list vectors.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::headers::{PAssertedIdentity, PPreferredIdentity};
use sipx_sip::{Header, HeaderError, HeaderName, Headers, Scheme, TypedHeader, UriError};

fn headers(name: &HeaderName, rows: &[&[u8]]) -> Headers {
    let mut headers = Headers::new();
    for row in rows {
        headers.push(Header::build(name.clone(), Bytes::copy_from_slice(row)).unwrap());
    }
    headers
}

fn asserted(rows: &[&[u8]]) -> Result<Vec<PAssertedIdentity>, HeaderError> {
    headers(&HeaderName::PAssertedIdentity, rows)
        .typed_all::<PAssertedIdentity>()
        .collect()
}

#[test]
fn ih_1_to_3_accept_each_permitted_scheme_and_serialize() {
    for (row, expected, family) in [
        (
            &b"<sip:alice@example.com>"[..],
            &b"<sip:alice@example.com>"[..],
            "sip",
        ),
        (
            &b"<sips:alice@example.com>"[..],
            &b"<sips:alice@example.com>"[..],
            "sip",
        ),
        (&b"tel:+12015550123"[..], &b"<tel:+12015550123>"[..], "tel"),
    ] {
        let values = asserted(&[row]).unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].to_bytes().as_ref(), expected);
        assert_eq!(
            matches!(values[0].uri.scheme(), Scheme::Tel),
            family == "tel"
        );
    }
}

#[test]
fn ih_4_to_6_comma_and_repeated_rows_are_one_ordered_list() {
    let comma = asserted(&[br#""Alice, A" <sip:alice@example.com>, <tel:+12015550123>"#]).unwrap();
    let repeated = asserted(&[b"<sip:alice@example.com>", b"<tel:+12015550123>"]).unwrap();
    let reversed = asserted(&[b"<tel:+12015550123>, <sips:alice@example.com>"]).unwrap();

    assert_eq!(comma.len(), 2);
    assert_eq!(comma[0].display_name.as_deref(), Some(&b"Alice, A"[..]));
    assert_eq!(
        comma[0].to_bytes(),
        Bytes::from_static(br#""Alice, A" <sip:alice@example.com>"#)
    );
    assert_eq!(
        repeated
            .iter()
            .map(PAssertedIdentity::to_bytes)
            .collect::<Vec<_>>(),
        vec![
            Bytes::from_static(b"<sip:alice@example.com>"),
            Bytes::from_static(b"<tel:+12015550123>"),
        ]
    );
    assert!(matches!(reversed[0].uri.scheme(), Scheme::Tel));
    assert!(matches!(reversed[1].uri.scheme(), Scheme::Sips));
}

#[test]
fn ih_7_preferred_identity_has_the_same_pairing_contract() {
    let values: Vec<PPreferredIdentity> = headers(
        &HeaderName::PPreferredIdentity,
        &[b"<sip:alice@example.com>, <tel:+12015550123>"],
    )
    .typed_all::<PPreferredIdentity>()
    .collect::<Result<_, _>>()
    .unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(
        values[1].to_bytes(),
        Bytes::from_static(b"<tel:+12015550123>")
    );
}

#[test]
fn ih_8_to_11_reject_scheme_pairing_and_message_wide_cardinality() {
    for rows in [
        vec![&b"<mailto:alice@example.com>"[..]],
        vec![&b"<sip:a@example.com>, <sips:b@example.com>"[..]],
        vec![&b"<tel:+12015550123>"[..], &b"<tel:+12015550124>"[..]],
        vec![
            &b"<sip:a@example.com>"[..],
            &b"<tel:+12015550123>, <sip:b@example.com>"[..],
        ],
    ] {
        assert!(matches!(
            asserted(&rows),
            Err(HeaderError::Syntax {
                header: "P-Asserted-Identity"
            })
        ));
    }
}

#[test]
fn ih_12_and_13_preserve_common_address_and_uri_errors() {
    assert!(matches!(
        asserted(&[b"<sip:a%GG@example.com>"]),
        Err(HeaderError::Uri {
            header: "P-Asserted-Identity",
            source: UriError::PercentEscape
        })
    ));
    assert!(matches!(
        asserted(&[br#""Alice <sip:alice@example.com>"#]),
        Err(HeaderError::UnterminatedQuotedString {
            header: "P-Asserted-Identity"
        })
    ));
}

#[test]
fn ih_14_and_15_keep_bare_uri_parameters_but_reject_header_parameters() {
    let value = asserted(&[b"sip:alice@example.com;user=phone"]).unwrap();
    assert_eq!(
        value[0].uri.to_bytes(),
        Bytes::from_static(b"sip:alice@example.com;user=phone")
    );
    assert!(matches!(
        asserted(&[b"<sip:alice@example.com>;tag=x"]),
        Err(HeaderError::Syntax {
            header: "P-Asserted-Identity"
        })
    ));
}

#[test]
fn ih_16_names_are_recognized_and_classified_as_lists() {
    assert_eq!(
        HeaderName::parse(&Bytes::from_static(b"p-AsSeRtEd-IdEnTiTy")),
        HeaderName::PAssertedIdentity
    );
    assert_eq!(
        HeaderName::parse(&Bytes::from_static(b"P-preferred-identity")),
        HeaderName::PPreferredIdentity
    );
    assert!(HeaderName::PAssertedIdentity.is_comma_separated_list());
    assert!(HeaderName::PPreferredIdentity.is_comma_separated_list());
    assert_eq!(PAssertedIdentity::NAME, HeaderName::PAssertedIdentity);
    assert_eq!(PPreferredIdentity::NAME, HeaderName::PPreferredIdentity);
}

#[test]
fn construction_rejects_a_scheme_rfc_3325_does_not_permit() {
    let address = sipx_sip::Address::parse(b"<mailto:alice@example.com>", "test").unwrap();
    assert!(matches!(
        PAssertedIdentity::new(address),
        Err(HeaderError::Syntax {
            header: "P-Asserted-Identity"
        })
    ));
}

#[test]
fn construction_rejects_an_address_with_header_parameters() {
    let address = sipx_sip::Address::parse(b"<sip:alice@example.com>;tag=x", "test").unwrap();
    assert!(!address.params.is_empty());
    assert!(matches!(
        PPreferredIdentity::new(address),
        Err(HeaderError::Syntax {
            header: "P-Preferred-Identity"
        })
    ));
}
