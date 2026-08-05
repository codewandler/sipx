//! RFC 3325 §§9.1–9.2 asserted and preferred identity value-list vectors.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::headers::{
    IgnoredIdentity, IgnoredIdentityReason, PAssertedIdentity, PAssertedIdentityList,
    PPreferredIdentity, PPreferredIdentityList,
};
use sipx_sip::{
    Address, AddressEditError, Header, HeaderError, HeaderName, Headers, Scheme, TypedHeader,
    UriError,
};

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
fn ih_14_to_17_enforce_rfc_8217_name_addr_delimiters() {
    for row in [
        &b"sip:alice@example.com;user=phone"[..],
        &b"sip:alice@example.com?subject=hello"[..],
        &b"tel:+12015550123;ext=7"[..],
    ] {
        assert!(matches!(
            asserted(&[row]),
            Err(HeaderError::Syntax {
                header: "P-Asserted-Identity"
            })
        ));
    }

    let value = asserted(&[b"<sip:alice@example.com;user=phone?subject=hello>"]).unwrap();
    assert_eq!(
        value[0].uri.to_bytes(),
        Bytes::from_static(b"sip:alice@example.com;user=phone?subject=hello")
    );
    assert!(matches!(
        asserted(&[b"<sip:alice@example.com>;tag=x"]),
        Err(HeaderError::Syntax {
            header: "P-Asserted-Identity"
        })
    ));
}

#[test]
fn ih_18_receive_filter_preserves_valid_values_and_reports_unexpected_schemes() {
    let received = PAssertedIdentityList::from_headers(&headers(
        &HeaderName::PAssertedIdentity,
        &[b"<mailto:alice@example.com>, <sip:alice@example.com>, <tel:+12015550123>"],
    ))
    .unwrap()
    .unwrap();

    assert_eq!(received.values().len(), 2);
    assert!(matches!(received.values()[0].uri.scheme(), Scheme::Sip));
    assert!(matches!(received.values()[1].uri.scheme(), Scheme::Tel));
    assert_eq!(received.ignored().len(), 1);
    assert_eq!(received.ignored()[0].index(), 0);
    assert_eq!(
        received.ignored()[0].reason(),
        IgnoredIdentityReason::UnexpectedScheme
    );
    assert!(matches!(
        received.ignored()[0].address().uri.scheme(),
        Scheme::Other(_)
    ));
    assert!(received.requires_rewrite());
    assert_eq!(
        received.to_bytes(),
        Some(Bytes::from_static(
            b"<sip:alice@example.com>, <tel:+12015550123>"
        ))
    );
}

#[test]
fn ih_19_receive_filter_is_message_wide_and_indices_follow_wire_order() {
    let received = PAssertedIdentityList::from_headers(&headers(
        &HeaderName::PAssertedIdentity,
        &[
            b"<sip:a@example.com>, <sips:b@example.com>",
            b"<sip:c@example.com>, <tel:+12015550123>, <tel:+12015550124>",
        ],
    ))
    .unwrap()
    .unwrap();

    assert_eq!(received.values().len(), 2);
    assert_eq!(
        received
            .ignored()
            .iter()
            .map(|ignored| (ignored.index(), ignored.reason()))
            .collect::<Vec<_>>(),
        vec![
            (1, IgnoredIdentityReason::SipsAfterSip),
            (2, IgnoredIdentityReason::DuplicateSip),
            (4, IgnoredIdentityReason::DuplicateTel),
        ]
    );
}

#[test]
fn ih_20_preferred_receive_filter_handles_the_reverse_sip_family_order() {
    let received = PPreferredIdentityList::from_headers(&headers(
        &HeaderName::PPreferredIdentity,
        &[b"<sips:a@example.com>, <sip:b@example.com>"],
    ))
    .unwrap()
    .unwrap();

    assert_eq!(received.values().len(), 1);
    assert!(matches!(received.values()[0].uri.scheme(), Scheme::Sips));
    assert_eq!(received.ignored()[0].index(), 1);
    assert_eq!(
        received.ignored()[0].reason(),
        IgnoredIdentityReason::SipAfterSips
    );
}

#[test]
fn ih_21_receive_distinguishes_absence_all_ignored_and_malformed() {
    assert!(
        PAssertedIdentityList::from_headers(&Headers::new())
            .unwrap()
            .is_none()
    );

    let ignored = PAssertedIdentityList::from_headers(&headers(
        &HeaderName::PAssertedIdentity,
        &[b"<mailto:alice@example.com>"],
    ))
    .unwrap()
    .unwrap();
    assert!(ignored.values().is_empty());
    assert_eq!(ignored.ignored().len(), 1);
    assert_eq!(ignored.to_bytes(), None);

    assert!(matches!(
        PAssertedIdentityList::from_headers(&headers(
            &HeaderName::PAssertedIdentity,
            &[b"<sip:a%GG@example.com>"],
        )),
        Err(HeaderError::Uri {
            header: "P-Asserted-Identity",
            source: UriError::PercentEscape
        })
    ));
}

#[test]
fn ih_22_ignored_indices_drive_the_proxy_must_not_forward_surgery() {
    let mut fields = headers(
        &HeaderName::PAssertedIdentity,
        &[
            b"<mailto:alice@example.com>, <sip:alice@example.com>",
            b"<tel:+12015550123>, <tel:+12015550124>",
        ],
    );
    let received = PAssertedIdentityList::from_headers(&fields)
        .unwrap()
        .unwrap();
    assert_eq!(
        received
            .ignored()
            .iter()
            .map(IgnoredIdentity::index)
            .collect::<Vec<_>>(),
        vec![0, 3]
    );

    // Removing in descending order keeps every earlier flattened index stable.
    for ignored in received.ignored().iter().rev() {
        fields
            .remove_address_value(&HeaderName::PAssertedIdentity, ignored.index())
            .unwrap();
    }

    let forwarded = PAssertedIdentityList::from_headers(&fields)
        .unwrap()
        .unwrap();
    assert!(!forwarded.requires_rewrite());
    assert_eq!(forwarded.values().len(), 2);
    assert_eq!(
        forwarded.to_bytes(),
        Some(Bytes::from_static(
            b"<sip:alice@example.com>, <tel:+12015550123>"
        ))
    );
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

#[test]
fn complete_construction_enforces_pairing_and_cardinality() {
    let sip = PAssertedIdentity::new(Address::parse(b"<sip:alice@example.com>", "test").unwrap())
        .unwrap();
    let second_sip =
        PAssertedIdentity::new(Address::parse(b"<sips:alice@example.com>", "test").unwrap())
            .unwrap();
    let tel =
        PAssertedIdentity::new(Address::parse(b"<tel:+12015550123>", "test").unwrap()).unwrap();

    assert!(PAssertedIdentityList::new([sip.clone(), second_sip.clone()]).is_err());
    assert!(PAssertedIdentityList::new([sip.clone(), tel.clone(), second_sip]).is_err());
    let list = PAssertedIdentityList::new([sip, tel]).unwrap();
    assert!(!list.requires_rewrite());
    assert_eq!(
        list.to_bytes(),
        Some(Bytes::from_static(
            b"<sip:alice@example.com>, <tel:+12015550123>"
        ))
    );
}

#[test]
fn construction_rejects_display_names_that_cannot_form_a_header_value() {
    for display_name in [b"Alice\r\nInjected: yes".to_vec(), vec![0xff]] {
        let mut address = Address::parse(b"<sip:alice@example.com>", "test").unwrap();
        address.display_name = Some(display_name);
        assert!(matches!(
            PAssertedIdentity::new(address),
            Err(HeaderError::Syntax {
                header: "P-Asserted-Identity"
            })
        ));
    }
}

#[test]
fn ih_26_only_sip_whitespace_may_surround_a_received_value() {
    for row in [
        &b"\x0b<sip:alice@example.com>"[..],
        &b"<sip:alice@example.com>\x0c"[..],
    ] {
        let mut fields = headers(&HeaderName::PAssertedIdentity, &[row]);
        assert!(matches!(
            PAssertedIdentityList::from_headers(&fields),
            Err(HeaderError::Syntax {
                header: "P-Asserted-Identity"
            })
        ));
        assert!(matches!(
            fields.remove_address_value(&HeaderName::PAssertedIdentity, 0),
            Err(AddressEditError::Malformed(HeaderError::Syntax {
                header: "P-Asserted-Identity"
            }))
        ));
    }
}

#[test]
fn ih_27_rfc_8217_question_mark_rule_is_independent_of_uri_scheme() {
    for row in [
        &b"tel:+12015550123?x=y"[..],
        &b"mailto:alice@example.com?subject=hello"[..],
    ] {
        assert!(matches!(
            PAssertedIdentityList::from_headers(&headers(&HeaderName::PAssertedIdentity, &[row])),
            Err(HeaderError::Syntax {
                header: "P-Asserted-Identity"
            })
        ));
    }

    let bracketed = PAssertedIdentityList::from_headers(&headers(
        &HeaderName::PAssertedIdentity,
        &[b"<mailto:alice@example.com?subject=hello>"],
    ))
    .unwrap()
    .unwrap();
    assert!(bracketed.values().is_empty());
    assert_eq!(bracketed.ignored().len(), 1);
    assert_eq!(
        bracketed.ignored()[0].reason(),
        IgnoredIdentityReason::UnexpectedScheme
    );
}

#[test]
fn ih_28_malformed_tel_and_opaque_uris_are_not_received_as_identities() {
    for row in [&b"<tel:>"[..], &b"<tel:+>"[..]] {
        assert!(matches!(
            PAssertedIdentityList::from_headers(&headers(&HeaderName::PAssertedIdentity, &[row])),
            Err(HeaderError::Uri {
                header: "P-Asserted-Identity",
                source: UriError::TelephoneSubscriber
            })
        ));
    }

    assert!(matches!(
        PAssertedIdentityList::from_headers(&headers(
            &HeaderName::PAssertedIdentity,
            &[b"<mailto:%GG>"]
        )),
        Err(HeaderError::Uri {
            header: "P-Asserted-Identity",
            source: UriError::PercentEscape
        })
    ));

    let valid = PAssertedIdentityList::from_headers(&headers(
        &HeaderName::PAssertedIdentity,
        &[b"<mailto:alice@example.com>"],
    ))
    .unwrap()
    .unwrap();
    assert!(valid.values().is_empty());
    assert_eq!(valid.ignored().len(), 1);
    assert_eq!(
        valid.ignored()[0].reason(),
        IgnoredIdentityReason::UnexpectedScheme
    );
}
