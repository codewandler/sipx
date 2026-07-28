//! Address headers: `From`, `To`, `Contact`, `Route`, `Record-Route`, `Refer-To`.
//!
//! All share one grammar (RFC 3261 §20.10, §20.20, §20.39):
//!
//! ```abnf
//! ( name-addr / addr-spec ) *( SEMI generic-param )
//! name-addr = [ display-name ] LAQUOT addr-spec RAQUOT
//! ```
//!
//! The trap is the bare `addr-spec` form: without angle brackets, a semicolon starts a
//! *header* parameter, not a URI parameter, so `sip:a@b;tag=1` is one URI and one header
//! parameter — while `<sip:a@b;tag=1>` is one URI with a URI parameter and no header
//! parameters. The two mean entirely different things and differ by two characters.

use bytes::Bytes;

use crate::error::HeaderError;
use crate::headers::grammar::{
    self, HeaderParam, find_param_start, is_token_char, quoted_string_end, skip_ws, trim,
};
use crate::message::TypedHeader;
use crate::name::HeaderName;
use crate::uri::Uri;

/// A display name with a URI and header parameters.
#[derive(Debug, Clone)]
pub struct Address {
    /// The display name, unquoted and unescaped, if there was one.
    pub display_name: Option<Vec<u8>>,
    /// The URI.
    pub uri: Uri,
    /// The header parameters — those after the URI, outside any angle brackets.
    pub params: Vec<HeaderParam>,
}

impl Address {
    /// Parse one address.
    pub fn parse(value: &[u8], header: &'static str) -> Result<Self, HeaderError> {
        let value = trim(value);
        let mut i = skip_ws(value, 0);

        // A display name is either a quoted string or a run of tokens; either way it ends at
        // the '<' that opens the URI.
        let mut display_name = None;
        if value.get(i) == Some(&b'"') {
            let end = quoted_string_end(value, i)
                .ok_or(HeaderError::UnterminatedQuotedString { header })?;
            let raw = value.get(i + 1..end.saturating_sub(1)).unwrap_or(&[]);
            display_name = Some(unescape(raw));
            i = skip_ws(value, end);
            if value.get(i) != Some(&b'<') {
                return Err(HeaderError::Syntax { header });
            }
        } else if let Some(angle) = find_angle(value, i) {
            let raw = trim(value.get(i..angle).unwrap_or(&[]));
            if !raw.is_empty() {
                // An unquoted display name is a sequence of tokens separated by whitespace.
                // A comma here is not a token character, which is what makes
                // `From: Bell, Alexander <sip:…>` invalid (RFC 4475 §3.1.2.15).
                if !raw
                    .iter()
                    .all(|&b| is_token_char(b) || matches!(b, b' ' | b'\t'))
                {
                    return Err(HeaderError::Syntax { header });
                }
                display_name = Some(raw.to_vec());
            }
            i = angle;
        }

        let (uri_bytes, params_tail) = if value.get(i) == Some(&b'<') {
            let close = find_closing_angle(value, i).ok_or(HeaderError::Syntax { header })?;
            let uri = value.get(i + 1..close).unwrap_or(&[]);
            (uri, value.get(close + 1..).unwrap_or(&[]))
        } else {
            // Bare addr-spec: the URI runs to the first header-parameter semicolon.
            let rest = value.get(i..).unwrap_or(&[]);
            match find_param_start(rest) {
                Some(semi) => (
                    trim(rest.get(..semi).unwrap_or(&[])),
                    rest.get(semi..).unwrap_or(&[]),
                ),
                None => (trim(rest), &[][..]),
            }
        };

        if uri_bytes.is_empty() {
            return Err(HeaderError::Syntax { header });
        }
        let uri = Uri::parse(Bytes::copy_from_slice(uri_bytes))
            .map_err(|source| HeaderError::Uri { header, source })?;

        // RFC 3261 §20: a URI carrying headers, or parameters, must be enclosed in angle
        // brackets, because otherwise there is no way to tell where the URI stops and the
        // header's own parameters start. RFC 4475 §3.1.2.13 is exactly this mistake.
        if value.get(i) != Some(&b'<') && uri.has_headers() {
            return Err(HeaderError::Syntax { header });
        }

        let params = grammar::parse_params(trim(params_tail), header)?;

        Ok(Self {
            display_name,
            uri,
            params,
        })
    }

    /// The value of a header parameter.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<&[u8]> {
        grammar::param(&self.params, name).and_then(|p| p.value.as_deref())
    }

    /// The `tag` parameter, which identifies a dialog participant.
    #[must_use]
    pub fn tag(&self) -> Option<&[u8]> {
        self.param("tag")
    }
}

/// The index of the `<` that opens a URI, if the value uses the `name-addr` form.
#[must_use]
fn find_angle(value: &[u8], from: usize) -> Option<usize> {
    value
        .get(from..)?
        .iter()
        .position(|&b| b == b'<')
        .map(|p| p + from)
}

#[must_use]
fn find_closing_angle(value: &[u8], open: usize) -> Option<usize> {
    value
        .get(open + 1..)?
        .iter()
        .position(|&b| b == b'>')
        .map(|p| p + open + 1)
}

#[must_use]
fn unescape(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while let Some(&b) = raw.get(i) {
        if b == b'\\'
            && let Some(&next) = raw.get(i + 1)
        {
            out.push(next);
            i += 2;
            continue;
        }
        out.push(b);
        i += 1;
    }
    out
}

macro_rules! address_header {
    ($(#[$meta:meta])* $type:ident => $variant:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $type(pub Address);

        impl std::ops::Deref for $type {
            type Target = Address;
            fn deref(&self) -> &Address {
                &self.0
            }
        }

        impl TypedHeader for $type {
            const NAME: HeaderName = HeaderName::$variant;

            fn decode(value: &[u8]) -> Result<Self, HeaderError> {
                Address::parse(value, $label).map(Self)
            }
        }
    };
}

address_header!(
    /// The `From` header (RFC 3261 §20.20).
    From => From, "From"
);
address_header!(
    /// The `To` header (RFC 3261 §20.39).
    To => To, "To"
);
address_header!(
    /// One `Contact` value (RFC 3261 §20.10).
    ///
    /// A `Contact` of `*` is legal in a REGISTER and is *not* an address; parse it with
    /// [`ContactValue`] rather than this type.
    Contact => Contact, "Contact"
);
address_header!(
    /// One `Route` value (RFC 3261 §20.34).
    Route => Route, "Route"
);
address_header!(
    /// One `Record-Route` value (RFC 3261 §20.30).
    RecordRoute => RecordRoute, "Record-Route"
);

/// A `Contact` value, which may be the wildcard `*`.
///
/// RFC 3261 §10.2.2: `Contact: *` with `Expires: 0` deregisters everything. It is the one
/// place in the grammar where a header that otherwise holds addresses holds a single asterisk
/// instead, and a parser that expects an address there will reject a legal deregistration.
#[derive(Debug, Clone)]
pub enum ContactValue {
    /// `*` — every registration.
    Wildcard,
    /// An ordinary address.
    Address(Address),
}

impl TypedHeader for ContactValue {
    const NAME: HeaderName = HeaderName::Contact;

    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        if trim(value) == b"*" {
            return Ok(Self::Wildcard);
        }
        Address::parse(value, "Contact").map(Self::Address)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn addr(value: &[u8]) -> Address {
        Address::parse(value, "To").unwrap_or_else(|e| panic!("{value:?} should parse: {e}"))
    }

    #[test]
    fn parses_a_bare_addr_spec() {
        let a = addr(b"sip:j.user@example.com");
        assert!(a.display_name.is_none());
        assert_eq!(
            a.uri.to_bytes(),
            Bytes::from_static(b"sip:j.user@example.com")
        );
        assert!(a.params.is_empty());
    }

    #[test]
    fn parses_a_quoted_display_name_with_escapes() {
        // RFC 4475 3.1.1.1 carries exactly this: an escaped backslash and an escaped quote.
        let a = addr(br#""J Rosenberg \\\"" <sip:jdrosen@example.com>;tag=98asjd8"#);
        assert_eq!(a.display_name.as_deref(), Some(&br#"J Rosenberg \""#[..]));
        assert_eq!(a.tag(), Some(&b"98asjd8"[..]));
    }

    #[test]
    fn parses_an_unquoted_token_display_name() {
        let a = addr(b"J Rosenberg <sip:jdrosen@example.com>");
        assert_eq!(a.display_name.as_deref(), Some(&b"J Rosenberg"[..]));
    }

    /// RFC 4475 3.1.1.6: no whitespace between the display name and the `<`.
    #[test]
    fn parses_a_display_name_abutting_the_angle_bracket() {
        let a = addr(br#""caller"<sip:caller@example.com>;tag=323"#);
        assert_eq!(a.display_name.as_deref(), Some(&b"caller"[..]));
        assert_eq!(a.tag(), Some(&b"323"[..]));
    }

    /// The distinction that costs two characters and changes everything.
    #[test]
    fn semicolons_bind_to_the_header_without_brackets_and_to_the_uri_within_them() {
        let bare = addr(b"sip:a@b.com;tag=1");
        assert_eq!(bare.uri.to_bytes(), Bytes::from_static(b"sip:a@b.com"));
        assert_eq!(bare.tag(), Some(&b"1"[..]));

        let bracketed = addr(b"<sip:a@b.com;tag=1>");
        assert_eq!(
            bracketed.uri.to_bytes(),
            Bytes::from_static(b"sip:a@b.com;tag=1")
        );
        assert!(bracketed.tag().is_none());
    }

    /// RFC 4475 3.1.2.15. The archive file for this case is unterminated so the corpus test
    /// cannot reach it; this is the hand-built version.
    #[test]
    fn rejects_an_unquoted_comma_in_a_display_name() {
        let err = Address::parse(b"Bell, Alexander <sip:a.g.bell@example.com>", "From");
        assert!(matches!(err, Err(HeaderError::Syntax { header: "From" })));
    }

    /// RFC 4475 3.1.2.6.
    #[test]
    fn rejects_an_unterminated_quoted_display_name() {
        let err = Address::parse(br#""Mr. J. User <sip:j.user@example.com>"#, "To");
        assert!(matches!(
            err,
            Err(HeaderError::UnterminatedQuotedString { header: "To" })
        ));
    }

    /// RFC 4475 3.1.2.14: spaces inside the angle brackets are not part of any URI.
    #[test]
    fn rejects_spaces_within_the_addr_spec() {
        let err = Address::parse(br#""Watson, Thomas" < sip:t.watson@example.org >"#, "To");
        assert!(matches!(err, Err(HeaderError::Uri { .. })));
    }

    /// RFC 4475 3.1.2.13: a URI with headers must be in angle brackets, or there is no way to
    /// tell where it ends.
    #[test]
    fn rejects_an_unbracketed_uri_carrying_headers() {
        let err = Address::parse(
            b"sip:user@example.com?Route=%3Csip:sip.example.com%3E",
            "Contact",
        );
        assert!(matches!(
            err,
            Err(HeaderError::Syntax { header: "Contact" })
        ));

        // In brackets the same URI is fine.
        assert!(
            Address::parse(
                b"<sip:user@example.com?Route=%3Csip:sip.example.com%3E>",
                "Contact"
            )
            .is_ok()
        );
    }

    #[test]
    fn parses_the_wildcard_contact() {
        assert!(matches!(
            ContactValue::decode(b"*"),
            Ok(ContactValue::Wildcard)
        ));
        assert!(matches!(
            ContactValue::decode(b"<sip:a@b>"),
            Ok(ContactValue::Address(_))
        ));
    }
}
