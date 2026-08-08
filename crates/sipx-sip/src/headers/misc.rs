//! The remaining headers the core needs: `CSeq`, `Call-ID`, the scalars, `Content-Type`,
//! `Date`, and the token-list headers.

use bytes::Bytes;

use crate::error::HeaderError;
use crate::headers::grammar::{self, HeaderParam, is_token_char, parse_u64, skip_ws, trim};
use crate::message::{Method, TypedHeader};
use crate::name::HeaderName;

/// The `CSeq` header (RFC 3261 §20.16): a sequence number and the method it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CSeq {
    /// The sequence number.
    pub sequence: u32,
    /// The method, which must match the request line (RFC 4475 §3.1.2.17).
    pub method: Method,
}

impl TypedHeader for CSeq {
    const NAME: HeaderName = HeaderName::CSeq;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let value = trim(value);
        let space = value
            .iter()
            .position(|&b| matches!(b, b' ' | b'\t'))
            .ok_or(HeaderError::Syntax { header: "CSeq" })?;
        let digits = value.get(..space).unwrap_or(&[]);
        let method_raw = trim(value.get(skip_ws(value, space)..).unwrap_or(&[]));

        if method_raw.is_empty() || !method_raw.iter().all(|&b| is_token_char(b)) {
            return Err(HeaderError::Syntax { header: "CSeq" });
        }

        // RFC 3261 §8.1.1.5 bounds the sequence number at 2^31-1, not 2^32-1, so that
        // incrementing it cannot overflow a 32-bit counter. RFC 4475 §3.1.2.4 sends one above
        // the limit and expects a 400.
        let sequence = parse_u64(digits, "CSeq")?;
        if sequence > u64::from(i32::MAX as u32) {
            return Err(HeaderError::OutOfRange { header: "CSeq" });
        }

        Ok(Self {
            sequence: u32::try_from(sequence)
                .map_err(|_| HeaderError::OutOfRange { header: "CSeq" })?,
            method: Method::parse(&Bytes::copy_from_slice(method_raw)),
        })
    }
}

/// The `Call-ID` header (RFC 3261 §20.8), an opaque identifier compared byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallId(pub Vec<u8>);

impl TypedHeader for CallId {
    const NAME: HeaderName = HeaderName::CallId;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let value = trim(value);
        if value.is_empty() {
            return Err(HeaderError::Syntax { header: "Call-ID" });
        }
        Ok(Self(value.to_vec()))
    }
}

/// The `Max-Forwards` header (RFC 3261 §20.22).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxForwards(pub u8);

impl TypedHeader for MaxForwards {
    const NAME: HeaderName = HeaderName::MaxForwards;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let n = parse_u64(trim(value), "Max-Forwards")?;
        u8::try_from(n)
            .map(Self)
            .map_err(|_| HeaderError::OutOfRange {
                header: "Max-Forwards",
            })
    }
}

/// The `Expires` header (RFC 3261 §20.19), in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expires(pub u32);

impl TypedHeader for Expires {
    const NAME: HeaderName = HeaderName::Expires;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let n = parse_u64(trim(value), "Expires")?;
        u32::try_from(n)
            .map(Self)
            .map_err(|_| HeaderError::OutOfRange { header: "Expires" })
    }
}

/// The `Content-Length` header (RFC 3261 §20.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentLength(pub u64);

impl TypedHeader for ContentLength {
    const NAME: HeaderName = HeaderName::ContentLength;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        parse_u64(trim(value), "Content-Length").map(Self)
    }
}

/// The `Content-Type` header (RFC 3261 §20.15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentType {
    /// The type, lowercased — `application`.
    pub media_type: Vec<u8>,
    /// The subtype, lowercased — `sdp`.
    pub subtype: Vec<u8>,
    /// Any parameters, such as a multipart `boundary`.
    pub params: Vec<HeaderParam>,
}

impl ContentType {
    /// Whether this is the given type and subtype, compared case-insensitively.
    #[must_use]
    pub fn is(&self, media_type: &str, subtype: &str) -> bool {
        self.media_type == media_type.as_bytes() && self.subtype == subtype.as_bytes()
    }

    /// A parameter value by name.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&[u8]> {
        grammar::param(&self.params, name).and_then(|p| p.value.as_deref())
    }
}

impl TypedHeader for ContentType {
    const NAME: HeaderName = HeaderName::ContentType;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        const LABEL: &str = "Content-Type";
        let value = trim(value);
        let (before_params, tail) = match grammar::find_param_start(value) {
            Some(semi) => (
                value.get(..semi).unwrap_or(&[]),
                value.get(semi..).unwrap_or(&[]),
            ),
            None => (value, &[][..]),
        };
        let slash = before_params
            .iter()
            .position(|&b| b == b'/')
            .ok_or(HeaderError::Syntax { header: LABEL })?;
        let media_type = trim(before_params.get(..slash).unwrap_or(&[]));
        let subtype = trim(before_params.get(slash + 1..).unwrap_or(&[]));

        if media_type.is_empty()
            || subtype.is_empty()
            || !media_type.iter().all(|&b| is_token_char(b))
            || !subtype.iter().all(|&b| is_token_char(b))
        {
            return Err(HeaderError::Syntax { header: LABEL });
        }

        Ok(Self {
            media_type: media_type.to_ascii_lowercase(),
            subtype: subtype.to_ascii_lowercase(),
            params: grammar::parse_params(trim(tail), LABEL)?,
        })
    }
}

/// The `Date` header (RFC 3261 §20.17).
///
/// SIP narrows HTTP's three date formats to one: RFC 1123 with the zone spelled `GMT` and
/// nothing else. RFC 4475 §3.1.2.12 sends `EST` and expects it to be refused, so the zone is
/// not cosmetic — accepting it would mean accepting a time that is wrong by hours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Date(pub Vec<u8>);

impl TypedHeader for Date {
    const NAME: HeaderName = HeaderName::Date;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        const LABEL: &str = "Date";
        // The alphabets are fixed by the grammar, and matched case-sensitively: rfc1123-date
        // is case-sensitive (RFC 2616 §3.3.1), and its dates are what SIP-date narrows down
        // to (RFC 3261 §20.17).
        const WKDAYS: [&[u8]; 7] = [b"Mon", b"Tue", b"Wed", b"Thu", b"Fri", b"Sat", b"Sun"];
        const MONTHS: [&[u8]; 12] = [
            b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
            b"Dec",
        ];
        let value = trim(value);

        // SIP-date = wkday "," SP date1 SP time SP "GMT". Every field is of fixed width, so
        // "Mon, 01 Jan 2010 16:00:00 GMT" is 29 octets and each field sits at a fixed offset.
        if value.len() != 29 {
            return Err(HeaderError::Syntax { header: LABEL });
        }
        let field = |from: usize, to: usize| value.get(from..to).unwrap_or(&[]);
        if field(3, 5) != b", "
            || field(7, 8) != b" "
            || field(11, 12) != b" "
            || field(16, 17) != b" "
            || field(19, 20) != b":"
            || field(22, 23) != b":"
            || field(25, 26) != b" "
            || field(26, 29) != b"GMT"
        {
            return Err(HeaderError::Syntax { header: LABEL });
        }
        if !WKDAYS.contains(&field(0, 3))
            || !MONTHS.contains(&field(8, 11))
            || !field(12, 16).iter().all(u8::is_ascii_digit)
        {
            return Err(HeaderError::Syntax { header: LABEL });
        }

        let day = two_digits(value, 5).ok_or(HeaderError::Syntax { header: LABEL })?;
        let hour = two_digits(value, 17).ok_or(HeaderError::Syntax { header: LABEL })?;
        let minute = two_digits(value, 20).ok_or(HeaderError::Syntax { header: LABEL })?;
        let second = two_digits(value, 23).ok_or(HeaderError::Syntax { header: LABEL })?;
        // The grammar's own bounds: date1 has at most 31 days, and the comment on `time`
        // stops it at 23:59:59 (RFC 3261 §25.1) — no leap-second allowance.
        if !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 59 {
            return Err(HeaderError::OutOfRange { header: LABEL });
        }

        Ok(Self(value.to_vec()))
    }
}

/// The `2DIGIT` field starting at `at`, or `None` if either octet is not a digit.
#[must_use]
fn two_digits(value: &[u8], at: usize) -> Option<u8> {
    let hi = value.get(at)?.checked_sub(b'0')?;
    let lo = value.get(at + 1)?.checked_sub(b'0')?;
    (hi <= 9 && lo <= 9).then_some(hi * 10 + lo)
}

/// A header whose value is a comma-separated list of tokens: `Allow`, `Supported`,
/// `Require`, `Proxy-Require`, `Unsupported`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenList(pub Vec<Vec<u8>>);

impl TokenList {
    fn decode_named(
        value: &[u8],
        header: &'static str,
        may_be_empty: bool,
    ) -> Result<Self, HeaderError> {
        let mut tokens = Vec::new();
        for part in grammar::split_list(value, header)? {
            let token = trim(part);
            // An entirely empty value is a legitimate "none of them" for `Allow` and
            // `Supported`, whose ABNF brackets the whole value — but `Require`,
            // `Proxy-Require` and `Unsupported` are `option-tag *(COMMA option-tag)` and
            // demand at least one tag (RFC 3261 §25.1).
            if token.is_empty() {
                if may_be_empty && grammar::split_list(value, header)?.len() == 1 {
                    return Ok(Self(Vec::new()));
                }
                return Err(HeaderError::Syntax { header });
            }
            if !token.iter().all(|&b| is_token_char(b)) {
                return Err(HeaderError::Syntax { header });
            }
            tokens.push(token.to_vec());
        }
        Ok(Self(tokens))
    }

    /// Whether the list contains this token, compared case-insensitively.
    #[must_use]
    pub fn contains(&self, token: &str) -> bool {
        self.0
            .iter()
            .any(|t| t.eq_ignore_ascii_case(token.as_bytes()))
    }
}

macro_rules! token_list_header {
    ($(#[$meta:meta])* $type:ident => $variant:ident, $label:literal, $may_be_empty:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $type(pub TokenList);

        impl std::ops::Deref for $type {
            type Target = TokenList;
            fn deref(&self) -> &TokenList {
                &self.0
            }
        }

        impl TypedHeader for $type {
            const NAME: HeaderName = HeaderName::$variant;

            fn decode(value: &[u8]) -> Result<Self, HeaderError> {
                TokenList::decode_named(value, $label, $may_be_empty).map(Self)
            }
        }
    };
}

token_list_header!(
    /// The `Allow` header (RFC 3261 §20.5).
    Allow => Allow, "Allow", true
);
token_list_header!(
    /// The `Supported` header (RFC 3261 §20.37).
    Supported => Supported, "Supported", true
);
token_list_header!(
    /// The `Require` header (RFC 3261 §20.32).
    Require => Require, "Require", false
);
token_list_header!(
    /// The `Proxy-Require` header (RFC 3261 §20.29).
    ProxyRequire => ProxyRequire, "Proxy-Require", false
);
token_list_header!(
    /// The `Unsupported` header (RFC 3261 §20.40).
    Unsupported => Unsupported, "Unsupported", false
);

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

    #[test]
    fn cseq_parses_number_and_method() {
        let c = CSeq::decode(b"8 INVITE").unwrap();
        assert_eq!(c.sequence, 8);
        assert_eq!(c.method, Method::Invite);
    }

    /// RFC 4475 3.1.1.1 sends `CSeq: 0009` folded onto the next line; once unfolded the
    /// leading zeros are still legal.
    #[test]
    fn cseq_accepts_leading_zeros_and_extra_whitespace() {
        let c = CSeq::decode(b"0009    INVITE").unwrap();
        assert_eq!(c.sequence, 9);
    }

    /// RFC 4475 3.1.2.4 and 3.1.2.5: above 2^31-1, which is where RFC 3261 8.1.1.5 stops.
    #[test]
    fn cseq_rejects_overlarge_sequence_numbers() {
        assert!(matches!(
            CSeq::decode(b"2147483648 INVITE"),
            Err(HeaderError::OutOfRange { header: "CSeq" })
        ));
        assert!(matches!(
            CSeq::decode(b"9292394834772304023312 OPTIONS"),
            Err(HeaderError::OutOfRange { header: "CSeq" })
        ));
        // The largest legal value still works.
        assert_eq!(
            CSeq::decode(b"2147483647 INVITE").unwrap().sequence,
            i32::MAX as u32
        );
    }

    #[test]
    fn cseq_rejects_a_missing_or_non_token_method() {
        assert!(CSeq::decode(b"8").is_err());
        assert!(CSeq::decode(b"8 IN VITE").is_err());
        assert!(CSeq::decode(b"x INVITE").is_err());
    }

    #[test]
    fn max_forwards_is_bounded_at_255() {
        assert_eq!(MaxForwards::decode(b"0068").unwrap().0, 68);
        assert_eq!(MaxForwards::decode(b"0").unwrap().0, 0);
        assert!(matches!(
            MaxForwards::decode(b"256"),
            Err(HeaderError::OutOfRange { .. })
        ));
    }

    #[test]
    fn content_type_lowercases_and_keeps_parameters() {
        let ct = ContentType::decode(b"Application/SDP").unwrap();
        assert!(ct.is("application", "sdp"));

        let ct = ContentType::decode(b"multipart/mixed;boundary=unique-boundary-1").unwrap();
        assert_eq!(ct.param("boundary"), Some(&b"unique-boundary-1"[..]));
    }

    #[test]
    fn content_type_rejects_a_missing_subtype() {
        assert!(ContentType::decode(b"application").is_err());
        assert!(ContentType::decode(b"application/").is_err());
        assert!(ContentType::decode(b"/sdp").is_err());
    }

    /// RFC 4475 3.1.2.12: SIP allows exactly one date format and exactly one zone.
    #[test]
    fn date_requires_gmt() {
        assert!(Date::decode(b"Fri, 01 Jan 2010 16:00:00 GMT").is_ok());
        assert!(Date::decode(b"Fri, 01 Jan 2010 16:00:00 EST").is_err());
        assert!(Date::decode(b"Fri, 01 Jan 2010 16:00:00").is_err());
        assert!(Date::decode(b"nonsense GMT").is_err());
    }

    /// RFC 3261 §25.1: `wkday` and `month` are fixed alphabets, the numeric fields are
    /// `2DIGIT`/`4DIGIT`, and the time comment bounds them at 23:59:59. The right shape with
    /// the wrong contents is not a date.
    #[test]
    fn date_validates_every_field_not_just_the_shape() {
        for bad in [
            &b"aaa, aaaaaaaaaaaaaaaaaaaa GMT"[..],
            b"Fri, 32 Jan 2010 25:99:99 GMT",
            b"Xyz, 01 Jan 2010 16:00:00 GMT",
            b"Fri, 00 Jan 2010 16:00:00 GMT",
            b"Fri, 01 Foo 2010 16:00:00 GMT",
            b"Fri, 01 Jan 2010 24:00:00 GMT",
            b"Fri, 01 Jan 2010 16:60:00 GMT",
            b"Fri, 01 Jan 2010 16:00:60 GMT",
            b"Fri, 01 Jan x010 16:00:00 GMT",
            b"Fri, 01 Jan 2010 16.00.00 GMT",
        ] {
            assert!(
                Date::decode(bad).is_err(),
                "{:?} should be rejected",
                String::from_utf8_lossy(bad)
            );
        }
        for good in [
            &b"Mon, 01 Jan 2010 00:00:00 GMT"[..],
            b"Sat, 13 Nov 2010 23:29:00 GMT",
            b"Sun, 31 Dec 2699 23:59:59 GMT",
        ] {
            assert!(
                Date::decode(good).is_ok(),
                "{:?} should parse",
                String::from_utf8_lossy(good)
            );
        }
    }

    #[test]
    fn token_lists_split_and_compare_case_insensitively() {
        let allow = Allow::decode(b"INVITE, ACK, OPTIONS, CANCEL, BYE").unwrap();
        assert_eq!(allow.0.0.len(), 5);
        assert!(allow.contains("invite"));
        assert!(!allow.contains("REFER"));

        // An empty value says "none", which is different from the header being absent.
        assert_eq!(Supported::decode(b"").unwrap().0.0.len(), 0);
        // But a stray comma is a malformed list, not an empty one.
        assert!(Supported::decode(b"100rel,,timer").is_err());
    }

    /// RFC 3261 §25.1 brackets the value of `Allow` and `Supported`, so those may be empty;
    /// `Require`, `Proxy-Require` and `Unsupported` are `option-tag *(COMMA option-tag)` and
    /// demand at least one tag.
    #[test]
    fn only_allow_and_supported_may_be_empty() {
        assert!(Allow::decode(b"").is_ok());
        assert!(Supported::decode(b"").is_ok());

        assert!(Require::decode(b"").is_err());
        assert!(ProxyRequire::decode(b"").is_err());
        assert!(Unsupported::decode(b"").is_err());

        // With a tag present all five parse alike.
        assert!(Require::decode(b"100rel").is_ok());
        assert!(ProxyRequire::decode(b"sec-agree").is_ok());
        assert!(Unsupported::decode(b"foo, bar").is_ok());
    }

    #[test]
    fn call_id_is_opaque_but_not_empty() {
        assert_eq!(
            CallId::decode(b"wsinv.ndaksdj@192.0.2.1").unwrap().0,
            b"wsinv.ndaksdj@192.0.2.1"
        );
        assert!(CallId::decode(b"   ").is_err());
    }
}
