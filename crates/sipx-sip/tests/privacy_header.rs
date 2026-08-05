//! Normative RFC 3323 Privacy-header vectors, including verified erratum 5184.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_sip::{
    Header, HeaderError, HeaderName, Headers, HistoryInfo, Limits, Privacy, PrivacyList,
    PrivacyValue, TypedHeader, Uri, parse_datagram,
};

fn headers(rows: &[&[u8]]) -> Headers {
    let mut headers = Headers::new();
    for row in rows {
        headers.push(
            Header::build(HeaderName::Privacy, Bytes::copy_from_slice(row))
                .expect("Privacy fixture is a safe header value"),
        );
    }
    headers
}

fn privacy(rows: &[&[u8]]) -> Result<Vec<Privacy>, HeaderError> {
    headers(rows).typed_all::<Privacy>().collect()
}

fn serialize(values: &[Privacy]) -> Bytes {
    PrivacyList::new(values.iter().map(|value| value.value().clone()))
        .expect("decoded complete list remains constructible")
        .to_bytes()
}

#[test]
fn p1_to_p3_decode_and_serialize_the_corrected_comma_list() {
    let vectors: &[(&[u8], &[PrivacyValue], &[u8])] = &[
        (b"none", &[PrivacyValue::None], b"none"),
        (
            b"user,header,session,critical",
            &[
                PrivacyValue::User,
                PrivacyValue::Header,
                PrivacyValue::Session,
                PrivacyValue::Critical,
            ],
            b"user,header,session,critical",
        ),
        (
            b" ID , history , VendorX , CRITICAL ",
            &[
                PrivacyValue::Id,
                PrivacyValue::History,
                PrivacyValue::Extension(b"VendorX".to_vec()),
                PrivacyValue::Critical,
            ],
            b"id,history,VendorX,critical",
        ),
    ];

    for (wire, expected, serialized) in vectors {
        let values = privacy(&[wire]).expect("P1-P3 must decode");
        assert_eq!(
            values
                .iter()
                .map(|value| value.value().clone())
                .collect::<Vec<_>>(),
            *expected
        );
        assert_eq!(serialize(&values).as_ref(), *serialized);
    }
}

#[test]
fn p4_to_p12_reject_malformed_or_contradictory_same_row_values() {
    let vectors: &[&[u8]] = &[
        b"user,User",
        b"none,id",
        b"critical",
        b"critical,header",
        b"header,critical,session",
        b"header,,session",
        b"header;session",
        b"header,bad=value",
        b"",
    ];

    for wire in vectors {
        assert!(matches!(
            privacy(&[wire]),
            Err(HeaderError::Syntax { header: "Privacy" })
        ));
    }
}

#[test]
fn p13_and_p14_enforce_none_and_duplicates_across_repeated_rows() {
    for rows in [
        [&b"none"[..], &b"history"[..]],
        [&b"user"[..], &b"User"[..]],
    ] {
        assert!(matches!(
            privacy(&rows),
            Err(HeaderError::Syntax { header: "Privacy" })
        ));
    }
}

#[test]
fn p15_allows_critical_after_a_service_in_an_earlier_row() {
    let values = privacy(&[b"id", b"critical"]).expect("the rows form one ordered list");
    assert_eq!(
        values.iter().map(|value| value.value()).collect::<Vec<_>>(),
        vec![&PrivacyValue::Id, &PrivacyValue::Critical]
    );
    assert_eq!(serialize(&values).as_ref(), b"id,critical");
}

#[test]
fn checked_construction_enforces_the_complete_list_invariants() {
    let ordinary = PrivacyList::new([
        PrivacyValue::User,
        PrivacyValue::Header,
        PrivacyValue::Session,
        PrivacyValue::Critical,
    ])
    .expect("P2 must construct");
    assert_eq!(
        ordinary.to_bytes().as_ref(),
        b"user,header,session,critical"
    );

    let extended = PrivacyList::new([
        PrivacyValue::Id,
        PrivacyValue::History,
        PrivacyValue::Extension(b"VendorX".to_vec()),
        PrivacyValue::Critical,
    ])
    .expect("P3 must construct");
    assert_eq!(extended.to_bytes().as_ref(), b"id,history,VendorX,critical");
    assert!(extended.contains(&PrivacyValue::History));

    assert!(PrivacyList::new([PrivacyValue::User, PrivacyValue::User]).is_err());
    assert!(PrivacyList::new([PrivacyValue::None, PrivacyValue::Id]).is_err());
    assert!(PrivacyList::new([PrivacyValue::Critical]).is_err());
    assert!(Privacy::new(PrivacyValue::Extension(b"bad=value".to_vec())).is_err());
    assert!(Privacy::new(PrivacyValue::Extension(b"HiStOrY".to_vec())).is_err());
}

#[test]
fn p16_history_info_consumes_the_validated_message_wide_list() {
    let message = parse_datagram(
        Bytes::from_static(
            b"INVITE sip:bob@example.test SIP/2.0\r\n\
          Privacy: id\r\n\
          Privacy: history,critical\r\n\
          Content-Length: 0\r\n\r\n",
        ),
        &Limits::default(),
    )
    .expect("message must frame");
    let request = message.as_request().expect("fixture is a request");
    let values = request
        .headers
        .typed_all::<Privacy>()
        .collect::<Result<Vec<_>, _>>()
        .expect("the complete Privacy list is valid");
    assert_eq!(values.len(), 3);
    assert!(values[0].is(&PrivacyValue::Id));
    assert!(values[1].is(&PrivacyValue::History));
    assert!(values[2].is(&PrivacyValue::Critical));

    let target = Uri::parse(Bytes::from_static(b"sip:alice@example.test"))
        .expect("History-Info fixture URI is valid");
    let mut history = HistoryInfo::initial(target);
    history
        .apply_message_privacy(&request.headers)
        .expect("History-Info consumes the validated typed list");
    assert_eq!(
        history.0[0].target.to_bytes().as_ref(),
        b"sip:anonymous@anonymous.invalid"
    );
}

#[test]
fn privacy_is_recognized_as_a_comma_separated_list_header() {
    assert_eq!(Privacy::NAME, HeaderName::Privacy);
    assert!(HeaderName::Privacy.is_comma_separated_list());
}
