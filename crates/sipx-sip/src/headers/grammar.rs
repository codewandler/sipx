//! Shared pieces of the header grammar (RFC 3261 §25.1).
//!
//! Header values look simple until you notice that commas, semicolons and angle brackets are
//! all delimiters that can also appear *inside* a value — in a quoted display name, in a URI,
//! in a comment. Every split in this module is aware of that, which is why none of them is a
//! call to `split`.

use std::ops::Range;

use crate::error::HeaderError;

/// RFC 3261 §25.1 `token`.
#[must_use]
pub(crate) fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
        )
}

/// Skip spaces and tabs.
#[must_use]
pub(crate) fn skip_ws(input: &[u8], mut at: usize) -> usize {
    while matches!(input.get(at), Some(b' ' | b'\t')) {
        at += 1;
    }
    at
}

/// Trim spaces and tabs from both ends.
#[must_use]
pub(crate) fn trim(mut b: &[u8]) -> &[u8] {
    while let Some((f, rest)) = b.split_first() {
        if matches!(f, b' ' | b'\t') {
            b = rest;
        } else {
            break;
        }
    }
    while let Some((l, rest)) = b.split_last() {
        if matches!(l, b' ' | b'\t') {
            b = rest;
        } else {
            break;
        }
    }
    b
}

/// Where a quoted string starting at `at` ends, as the index just past its closing quote.
///
/// Returns `None` if it is never closed — RFC 4475 §3.1.2.6 is precisely this case, and it
/// matters because an unterminated quote would otherwise swallow the rest of the message.
#[must_use]
pub(crate) fn quoted_string_end(input: &[u8], at: usize) -> Option<usize> {
    if input.get(at) != Some(&b'"') {
        return None;
    }
    let mut i = at + 1;
    while let Some(&b) = input.get(i) {
        match b {
            // A backslash quotes the next octet, including another backslash or a quote.
            b'\\' if i + 1 < input.len() => i += 2,
            b'"' => return Some(i + 1),
            _ => i += 1,
        }
    }
    None
}

/// Split a header value on commas that are actual list separators.
///
/// Commas inside quoted strings, angle brackets and parenthesized comments belong to the
/// value. Getting this wrong is how `From: "Bell, Alexander" <sip:…>` turns into two
/// mangled addresses.
pub(crate) fn split_list<'a>(
    value: &'a [u8],
    header: &'static str,
) -> Result<Vec<&'a [u8]>, HeaderError> {
    split_list_spans(value, header).map(|spans| {
        spans
            .into_iter()
            .map(|span| value.get(span).unwrap_or(&[]))
            .collect()
    })
}

/// Split a list while retaining each value's half-open range in the grammar input.
///
/// Range ownership is needed by lossless editors: returning only decoded values would force a
/// caller to search for their bytes, which is ambiguous when display text repeats a URI.
pub(crate) fn split_list_spans(
    value: &[u8],
    header: &'static str,
) -> Result<Vec<Range<usize>>, HeaderError> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut angle = 0usize;
    let mut paren = 0usize;

    while i < value.len() {
        match value.get(i) {
            Some(b'"') => {
                i = quoted_string_end(value, i)
                    .ok_or(HeaderError::UnterminatedQuotedString { header })?;
            }
            Some(b'<') => {
                angle += 1;
                i += 1;
            }
            Some(b'>') => {
                angle = angle.saturating_sub(1);
                i += 1;
            }
            Some(b'(') => {
                paren += 1;
                i += 1;
            }
            Some(b')') => {
                paren = paren.saturating_sub(1);
                i += 1;
            }
            Some(b',') if angle == 0 && paren == 0 => {
                parts.push(start..i);
                i += 1;
                start = i;
            }
            Some(_) => i += 1,
            None => break,
        }
    }
    parts.push(start..value.len());
    Ok(parts)
}

/// Index of the first semicolon that separates parameters, skipping quoted strings, angle
/// brackets and comments.
#[must_use]
pub(crate) fn find_param_start(value: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    let mut angle = 0usize;
    while i < value.len() {
        match value.get(i) {
            Some(b'"') => i = quoted_string_end(value, i)?,
            Some(b'<') => {
                angle += 1;
                i += 1;
            }
            Some(b'>') => {
                angle = angle.saturating_sub(1);
                i += 1;
            }
            Some(b';') if angle == 0 => return Some(i),
            Some(_) => i += 1,
            None => break,
        }
    }
    None
}

/// One header parameter: `name` or `name=value`, where the value may be quoted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderParam {
    /// The parameter name, lowercased for comparison.
    pub name: Vec<u8>,
    /// The value, with any surrounding quotes removed and escapes resolved.
    pub value: Option<Vec<u8>>,
}

impl HeaderParam {
    /// Whether the name matches, case-insensitively.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name == name.as_bytes()
    }
}

/// Parse a `;`-separated parameter list from the tail of a header value.
///
/// Empty segments are rejected. The ABNF has `*( SEMI generic-param )` with a non-empty name,
/// so `;;` is not a quirky spelling of `;` — RFC 4475 §3.1.2.1 turns a message invalid on
/// exactly that, in a `Via`.
pub(crate) fn parse_params(
    tail: &[u8],
    header: &'static str,
) -> Result<Vec<HeaderParam>, HeaderError> {
    let mut params = Vec::new();
    let mut i = 0usize;

    while i < tail.len() {
        // Each iteration must start at a semicolon.
        i = skip_ws(tail, i);
        if tail.get(i) != Some(&b';') {
            return Err(HeaderError::Syntax { header });
        }
        i = skip_ws(tail, i + 1);

        let name_start = i;
        while tail.get(i).is_some_and(|&b| is_token_char(b)) {
            i += 1;
        }
        let name = tail.get(name_start..i).unwrap_or(&[]);
        if name.is_empty() {
            return Err(HeaderError::Syntax { header });
        }

        i = skip_ws(tail, i);
        let value = if tail.get(i) == Some(&b'=') {
            i = skip_ws(tail, i + 1);
            if tail.get(i) == Some(&b'"') {
                let end = quoted_string_end(tail, i)
                    .ok_or(HeaderError::UnterminatedQuotedString { header })?;
                let raw = tail.get(i + 1..end.saturating_sub(1)).unwrap_or(&[]);
                let unescaped = unescape_quoted(raw);
                i = end;
                Some(unescaped)
            } else {
                let start = i;
                // A bare parameter value is a token, but hosts and IPv6 references appear as
                // values too (maddr, received), so `[`, `]`, `:` and `/` are accepted here.
                while tail
                    .get(i)
                    .is_some_and(|&b| is_token_char(b) || matches!(b, b'[' | b']' | b':' | b'/'))
                {
                    i += 1;
                }
                if i == start {
                    return Err(HeaderError::Syntax { header });
                }
                Some(tail.get(start..i).unwrap_or(&[]).to_vec())
            }
        } else {
            None
        };

        params.push(HeaderParam {
            name: name.to_ascii_lowercase(),
            value,
        });
        i = skip_ws(tail, i);
    }

    Ok(params)
}

/// Resolve backslash escapes inside a quoted string.
#[must_use]
fn unescape_quoted(raw: &[u8]) -> Vec<u8> {
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

/// Find a parameter by name.
#[must_use]
pub(crate) fn param<'a>(params: &'a [HeaderParam], name: &str) -> Option<&'a HeaderParam> {
    params.iter().find(|p| p.is(name))
}

/// Parse an unsigned decimal, rejecting anything that is not entirely digits.
///
/// Leading zeros are fine — RFC 4475 §3.1.1.1 writes `Max-Forwards: 0068` and `CSeq: 0009`.
/// A sign character is not: it never reaches a conversion that could produce a negative
/// number.
pub(crate) fn parse_u64(value: &[u8], header: &'static str) -> Result<u64, HeaderError> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(HeaderError::Syntax { header });
    }
    let mut n: u64 = 0;
    for &b in value {
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(u64::from(b - b'0')))
            .ok_or(HeaderError::OutOfRange { header })?;
    }
    Ok(n)
}

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
    fn list_splitting_respects_quotes_and_brackets() {
        // The comma belongs to the display name, not to the list.
        let parts = split_list(br#""Bell, Alexander" <sip:a@b>, <sip:c@d>"#, "From").unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(trim(parts[0]), br#""Bell, Alexander" <sip:a@b>"#);
        assert_eq!(trim(parts[1]), b"<sip:c@d>");
    }

    #[test]
    fn list_splitting_reports_an_unterminated_quote() {
        // RFC 4475 3.1.2.6. Without this the open quote swallows the rest of the message.
        let err = split_list(br#""Mr. J. User <sip:j@example.com>"#, "To").unwrap_err();
        assert!(matches!(
            err,
            HeaderError::UnterminatedQuotedString { header: "To" }
        ));
    }

    #[test]
    fn parameters_reject_empty_segments() {
        // RFC 4475 3.1.2.1: `;;,;,,` is not a quirky spelling of nothing.
        assert!(parse_params(b";;", "Via").is_err());
        assert!(parse_params(b";a=1;;", "Via").is_err());
        assert!(parse_params(b";", "Via").is_err());
    }

    #[test]
    fn parameters_parse_flags_quoted_and_bare_values() {
        let params = parse_params(br#";lr;tag=abc;text="a;b\"c""#, "To").unwrap();
        assert_eq!(params.len(), 3);
        assert!(params[0].is("lr") && params[0].value.is_none());
        assert_eq!(params[1].value.as_deref(), Some(&b"abc"[..]));
        // The quoted value keeps its semicolon and its escaped quote.
        assert_eq!(params[2].value.as_deref(), Some(&br#"a;b"c"#[..]));
    }

    #[test]
    fn parameter_names_compare_case_insensitively() {
        let params = parse_params(b";Transport=TCP", "Via").unwrap();
        assert!(param(&params, "transport").is_some());
    }

    #[test]
    fn parameter_values_may_be_hosts_and_ipv6_references() {
        let params = parse_params(b";received=192.0.2.1;maddr=[2001:db8::1]", "Via").unwrap();
        assert_eq!(
            param(&params, "received").and_then(|p| p.value.as_deref()),
            Some(&b"192.0.2.1"[..])
        );
        assert_eq!(
            param(&params, "maddr").and_then(|p| p.value.as_deref()),
            Some(&b"[2001:db8::1]"[..])
        );
    }

    #[test]
    fn numbers_keep_leading_zeros_and_reject_signs() {
        // RFC 4475 3.1.1.1 sends `Max-Forwards: 0068`.
        assert_eq!(parse_u64(b"0068", "Max-Forwards").unwrap(), 68);
        assert!(parse_u64(b"-1", "Max-Forwards").is_err());
        assert!(parse_u64(b"+1", "Max-Forwards").is_err());
        assert!(parse_u64(b"", "Max-Forwards").is_err());
        assert!(parse_u64(b"12x", "Max-Forwards").is_err());
        // Overflow is an out-of-range error, not a wrap.
        assert!(matches!(
            parse_u64(b"99999999999999999999999", "CSeq"),
            Err(HeaderError::OutOfRange { .. })
        ));
    }
}
