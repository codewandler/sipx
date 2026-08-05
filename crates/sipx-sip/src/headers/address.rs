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

use std::ops::Range;

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
        parse_spanned(value, header).map(|parsed| parsed.address)
    }

    /// Parse a header value carrying one or more comma-separated addresses.
    ///
    /// RFC 3261 §7.3: for `Contact`, `Route` and `Record-Route` a comma-joined row is
    /// exactly equivalent to the same values on separate rows, so the row must be split
    /// before the address grammar applies.
    pub fn parse_list(value: &[u8], header: &'static str) -> Result<Vec<Self>, HeaderError> {
        grammar::split_list(value, header)?
            .into_iter()
            .map(|part| Self::parse(part, header))
            .collect()
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

#[derive(Debug)]
struct ParsedAddress {
    address: Address,
    uri: Range<usize>,
}

fn parse_spanned(value: &[u8], header: &'static str) -> Result<ParsedAddress, HeaderError> {
    let outer = trimmed_range(value);
    let value = value.get(outer.clone()).unwrap_or(&[]);
    let mut i = skip_ws(value, 0);

    // A display name is either a quoted string or a run of tokens; either way it ends at
    // the '<' that opens the URI.
    let mut display_name = None;
    if value.get(i) == Some(&b'"') {
        let end =
            quoted_string_end(value, i).ok_or(HeaderError::UnterminatedQuotedString { header })?;
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

    let (uri_span, params_tail) = if value.get(i) == Some(&b'<') {
        let close = find_closing_angle(value, i).ok_or(HeaderError::Syntax { header })?;
        (i + 1..close, value.get(close + 1..).unwrap_or(&[]))
    } else {
        // Bare addr-spec: the URI runs to the first header-parameter semicolon.
        let rest = value.get(i..).unwrap_or(&[]);
        let param_start = find_param_start(rest);
        let candidate = match param_start {
            Some(semi) => i..i.checked_add(semi).ok_or(HeaderError::Syntax { header })?,
            None => i..value.len(),
        };
        let candidate_bytes = value.get(candidate.clone()).unwrap_or(&[]);
        let trimmed = trimmed_range(candidate_bytes);
        let uri_span = candidate
            .start
            .checked_add(trimmed.start)
            .zip(candidate.start.checked_add(trimmed.end))
            .map(|(start, end)| start..end)
            .ok_or(HeaderError::Syntax { header })?;
        let params_tail = if let Some(semi) = param_start {
            rest.get(semi..).unwrap_or(&[])
        } else {
            &[][..]
        };
        (uri_span, params_tail)
    };

    let uri_bytes = value.get(uri_span.clone()).unwrap_or(&[]);
    if uri_bytes.is_empty() {
        return Err(HeaderError::Syntax { header });
    }

    // RFC 8217 applies to every `(name-addr / addr-spec)` choice, independent of URI scheme.
    // A bare question mark is therefore malformed before scheme-specific URI parsing decides
    // whether it represents a structured SIP header component or belongs to an opaque body.
    if value.get(i) != Some(&b'<') && uri_bytes.contains(&b'?') {
        return Err(HeaderError::Syntax { header });
    }
    let parsed_uri = Uri::parse(Bytes::copy_from_slice(uri_bytes))
        .map_err(|source| HeaderError::Uri { header, source })?;

    let params = grammar::parse_params(trim(params_tail), header)?;
    let uri_span = add_offset(uri_span, outer.start, header)?;

    Ok(ParsedAddress {
        address: Address {
            display_name,
            uri: parsed_uri,
            params,
        },
        uri: uri_span,
    })
}

/// Parser-owned ranges for one address-list value in an unfolded field value.
#[derive(Debug, Clone)]
pub(crate) struct AddressValueSpan {
    /// The complete comma-delimited segment, including surrounding linear whitespace.
    pub(crate) part: Range<usize>,
    /// The address itself, excluding surrounding linear whitespace.
    pub(crate) item: Range<usize>,
    /// The nested URI.
    pub(crate) uri: Range<usize>,
}

/// Parse address values and retain their grammatical byte ranges.
///
/// The ordinary parser returns these ranges from the same pass that constructs [`Address`], so the
/// editor cannot drift into a second permissive delimiter implementation.
pub(crate) fn value_spans(
    value: &[u8],
    header: &'static str,
    is_list: bool,
) -> Result<Vec<AddressValueSpan>, HeaderError> {
    let parts = if is_list {
        grammar::split_list_spans(value, header)?
    } else {
        std::iter::once(0..value.len()).collect()
    };

    parts
        .into_iter()
        .map(|part| {
            let bytes = value.get(part.clone()).unwrap_or(&[]);
            let parsed = parse_spanned(bytes, header)?;
            let item = trimmed_range(bytes);
            Ok(AddressValueSpan {
                part: part.clone(),
                item: add_offset(item, part.start, header)?,
                uri: add_offset(parsed.uri, part.start, header)?,
            })
        })
        .collect()
}

fn trimmed_range(value: &[u8]) -> Range<usize> {
    let mut start = 0usize;
    while matches!(value.get(start), Some(b' ' | b'\t')) {
        start += 1;
    }
    let mut end = value.len();
    while end > start && matches!(value.get(end - 1), Some(b' ' | b'\t')) {
        end -= 1;
    }
    start..end
}

fn add_offset(
    range: Range<usize>,
    offset: usize,
    header: &'static str,
) -> Result<Range<usize>, HeaderError> {
    offset
        .checked_add(range.start)
        .zip(offset.checked_add(range.end))
        .map(|(start, end)| start..end)
        .ok_or(HeaderError::Syntax { header })
}

/// The index of the `<` that opens a URI, if the value uses the `name-addr` form.
///
/// Quoted strings are skipped, escapes and all: `qdtext` includes `%x3C` (RFC 3261 §25.1),
/// so the `<` in `sip:a@b;x="<y>"` is parameter text, not the start of a name-addr.
#[must_use]
fn find_angle(value: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < value.len() {
        match value.get(i) {
            Some(b'"') => i = quoted_string_end(value, i)?,
            Some(b'<') => return Some(i),
            Some(_) => i += 1,
            None => break,
        }
    }
    None
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

macro_rules! address_type {
    ($(#[$meta:meta])* $type:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $type(pub Address);

        impl std::ops::Deref for $type {
            type Target = Address;
            fn deref(&self) -> &Address {
                &self.0
            }
        }
    };
}

/// A header holding exactly one address per row. A comma in the value is a fault, not a
/// separator: `From` and `To` are single-value (RFC 3261 §20.20, §20.39).
macro_rules! single_address_header {
    ($(#[$meta:meta])* $type:ident => $variant:ident, $label:literal) => {
        address_type!($(#[$meta])* $type);

        impl TypedHeader for $type {
            const NAME: HeaderName = HeaderName::$variant;

            fn decode(value: &[u8]) -> Result<Self, HeaderError> {
                Address::parse(value, $label).map(Self)
            }
        }
    };
}

/// A header whose row may carry several comma-separated addresses (RFC 3261 §7.3).
macro_rules! address_list_header {
    ($(#[$meta:meta])* $type:ident => $variant:ident, $label:literal) => {
        address_type!($(#[$meta])* $type);

        impl $type {
            /// Parse a header value that may carry several comma-separated addresses.
            pub fn parse_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
                Address::parse_list(value, $label).map(|list| list.into_iter().map(Self).collect())
            }
        }

        impl TypedHeader for $type {
            const NAME: HeaderName = HeaderName::$variant;

            /// Decodes the **first** address in the value; use [`Self::parse_list`] or
            /// [`crate::message::Headers::typed_all`] when every one is needed.
            fn decode(value: &[u8]) -> Result<Self, HeaderError> {
                let parts = grammar::split_list(value, $label)?;
                let first = parts.first().copied().unwrap_or(&[]);
                Address::parse(first, $label).map(Self)
            }

            fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
                Self::parse_list(value)
            }
        }
    };
}

single_address_header!(
    /// The `From` header (RFC 3261 §20.20).
    From => From, "From"
);
single_address_header!(
    /// The `To` header (RFC 3261 §20.39).
    To => To, "To"
);
address_list_header!(
    /// The `Contact` header (RFC 3261 §20.10).
    ///
    /// A `Contact` of `*` is legal in a REGISTER and is *not* an address; parse it with
    /// [`ContactValue`] rather than this type.
    Contact => Contact, "Contact"
);
address_list_header!(
    /// The `Route` header (RFC 3261 §20.34).
    Route => Route, "Route"
);
address_list_header!(
    /// The `Record-Route` header (RFC 3261 §20.30).
    RecordRoute => RecordRoute, "Record-Route"
);
address_list_header!(
    /// The `Path` header (RFC 3327 §4).
    ///
    /// A route header, not a `Contact`-shaped one, and it has to be parsed with list semantics
    /// for the same reason `Record-Route` does: proxies each add their own value, and RFC 3261
    /// §7.3 lets a comma-joined row stand for the same values on separate rows. Read a line at
    /// a time, a two-proxy path becomes one opaque string and the order — which is the entire
    /// content of a path vector — is lost.
    Path => Path, "Path"
);
address_list_header!(
    /// The `Service-Route` header (RFC 3608 §5).
    ///
    /// `sr-value = name-addr *( SEMI rr-param )`, comma-separated — the same list grammar the
    /// other route headers have, and list semantics for the same reason: RFC 3608 §6.1 requires
    /// a UA that exercises a service route to "preserve the order", and order is exactly what is
    /// lost when a comma-joined row is read as one opaque value.
    ServiceRoute => ServiceRoute, "Service-Route"
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

    /// Decodes the wildcard, or the **first** address in the value; use
    /// [`TypedHeader::decode_list`] when every one is needed.
    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        if trim(value) == b"*" {
            return Ok(Self::Wildcard);
        }
        Contact::decode(value).map(|c| Self::Address(c.0))
    }

    fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
        // The wildcard is the entire value: `Contact: *, <sip:a@b>` is not in the grammar,
        // which has `STAR` as an alternative to the whole contact-param list (RFC 3261 §25.1).
        if trim(value) == b"*" {
            return Ok(vec![Self::Wildcard]);
        }
        Address::parse_list(value, "Contact")
            .map(|list| list.into_iter().map(Self::Address).collect())
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

    /// RFC 3261 §25.1: `gen-value` may be a quoted string, and `qdtext` includes `<`
    /// (%x3C), so an angle bracket inside a quoted parameter value never opens a name-addr.
    #[test]
    fn a_quoted_parameter_value_may_contain_an_angle_bracket() {
        let a = addr(br#"sip:a@b.com;x="<y>""#);
        assert_eq!(a.uri.to_bytes(), Bytes::from_static(b"sip:a@b.com"));
        assert_eq!(a.param("x"), Some(&b"<y>"[..]));

        let a = addr(br#"sip:a@b.com;note="hi <there>""#);
        assert_eq!(a.uri.to_bytes(), Bytes::from_static(b"sip:a@b.com"));
        assert_eq!(a.param("note"), Some(&b"hi <there>"[..]));

        // An escaped quote does not end the string early.
        let a = addr(br#"sip:a@b.com;x="a\"<b""#);
        assert_eq!(a.param("x"), Some(&br#"a"<b"#[..]));
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

    /// RFC 8217: a question mark requires name-addr for every URI scheme, or there is no way to
    /// tell where the URI ends.
    #[test]
    fn rejects_an_unbracketed_question_mark_for_every_uri_scheme() {
        for bare in [
            b"sip:user@example.com?Route=%3Csip:sip.example.com%3E".as_slice(),
            b"tel:+12015550123?x=y",
            b"mailto:alice@example.com?subject=hello",
        ] {
            assert!(matches!(
                Address::parse(bare, "Contact"),
                Err(HeaderError::Syntax { header: "Contact" })
            ));
        }

        // In brackets syntactically valid SIP and opaque URIs are fine.
        for bracketed in [
            b"<sip:user@example.com?Route=%3Csip:sip.example.com%3E>".as_slice(),
            b"<mailto:alice@example.com?subject=hello>",
        ] {
            assert!(Address::parse(bracketed, "Contact").is_ok());
        }
    }

    /// RFC 3261 §7.3: a comma-joined row is exactly equivalent to the same values on
    /// separate rows, so the typed readers must accept a list.
    #[test]
    fn decodes_the_first_address_of_a_comma_separated_row() {
        let c = Contact::decode(b"<sip:a@b.com>, <sip:c@d.com>").unwrap();
        assert_eq!(c.uri.to_bytes(), Bytes::from_static(b"sip:a@b.com"));

        let r = Route::decode(b"<sip:p1.example.com;lr>,<sip:p2.example.com;lr>").unwrap();
        assert_eq!(
            r.uri.to_bytes(),
            Bytes::from_static(b"sip:p1.example.com;lr")
        );
    }

    #[test]
    fn parses_every_address_of_a_comma_separated_row() {
        let list = Address::parse_list(
            br#""Bell, Alexander" <sip:a@b.com>;q=0.7, <sip:c@d.com>"#,
            "Contact",
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[0].display_name.as_deref(),
            Some(&b"Bell, Alexander"[..])
        );
        assert_eq!(list[0].param("q"), Some(&b"0.7"[..]));
        assert_eq!(list[1].uri.to_bytes(), Bytes::from_static(b"sip:c@d.com"));

        // One bad element spoils the row: the list is only as good as its members.
        assert!(Address::parse_list(b"<sip:a@b.com>, not a uri", "Contact").is_err());
    }

    #[test]
    fn wildcard_and_addresses_both_come_out_of_a_contact_list() {
        let all = ContactValue::decode_list(b"*").unwrap();
        assert!(matches!(all.as_slice(), [ContactValue::Wildcard]));

        let all = ContactValue::decode_list(b"<sip:a@b.com>, <sip:c@d.com>").unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|v| matches!(v, ContactValue::Address(_))));
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
