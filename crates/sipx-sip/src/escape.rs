//! Percent-encoding, per RFC 3986 §2 and the comparison rule in RFC 3261 §19.1.4.
//!
//! sipx never decodes escapes while parsing (see `docs/specs/sip-parser.md` §4.6). `%00` has
//! to survive a round trip, and decoding into a Rust string type would either panic or
//! lossily replace it. Decoding is an explicit operation, and it yields bytes.

/// The RFC 2396 "reserved" set, which RFC 3261 §19.1.4 refers to when it says that
/// characters *outside* this set are equivalent to their escaped form.
///
/// Reserved characters are delimiters: escaping one changes what the URI means, so `%2F` and
/// `/` are genuinely different and must not be folded together when comparing.
const RESERVED: &[u8] = b";/?:@&=+$,";

#[must_use]
fn is_reserved(b: u8) -> bool {
    RESERVED.contains(&b)
}

#[must_use]
fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Whether every `%` in `input` is followed by two hex digits.
#[must_use]
pub(crate) fn escapes_are_well_formed(input: &[u8]) -> bool {
    let mut i = 0;
    while i < input.len() {
        let Some(&b) = input.get(i) else { break };
        if b == b'%' {
            let ok = input
                .get(i + 1)
                .and_then(|&h| hex_value(h))
                .and(input.get(i + 2).and_then(|&h| hex_value(h)))
                .is_some();
            if !ok {
                return false;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    true
}

/// Fully decode percent escapes, yielding raw bytes.
///
/// Returns `None` if an escape is malformed. The result is bytes, not a string: `%00` and
/// invalid UTF-8 are both legal in a SIP URI and must survive.
#[must_use]
pub(crate) fn decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while let Some(&b) = input.get(i) {
        if b == b'%' {
            let hi = hex_value(*input.get(i + 1)?)?;
            let lo = hex_value(*input.get(i + 2)?)?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    Some(out)
}

/// Decode only those escapes whose decoded octet is **not** reserved, leaving reserved
/// characters in their escaped form.
///
/// This is the normalization RFC 3261 §19.1.4 requires before comparing two URIs:
/// `sip:%61lice@atlanta.com` and `sip:alice@atlanta.com` are the same URI, while `%2F` and
/// `/` are not, because `/` is a delimiter.
///
/// A malformed escape is left verbatim rather than rejected — comparison is not the place to
/// diagnose syntax, and the parser has already refused genuinely malformed URIs.
#[must_use]
pub(crate) fn normalize_for_comparison(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while let Some(&b) = input.get(i) {
        if b == b'%'
            && let Some(decoded) = input
                .get(i + 1)
                .and_then(|&h| hex_value(h))
                .zip(input.get(i + 2).and_then(|&h| hex_value(h)))
                .map(|(hi, lo)| (hi << 4) | lo)
        {
            if is_reserved(decoded) {
                // Keep the escape, but canonicalize its spelling so %2F and %2f compare
                // equal.
                out.push(b'%');
                out.extend_from_slice(&upper_hex(decoded));
            } else {
                out.push(decoded);
            }
            i += 3;
            continue;
        }
        out.push(b);
        i += 1;
    }
    out
}

#[must_use]
fn upper_hex(b: u8) -> [u8; 2] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let hi = HEX.get(usize::from(b >> 4)).copied().unwrap_or(b'0');
    let lo = HEX.get(usize::from(b & 0x0f)).copied().unwrap_or(b'0');
    [hi, lo]
}

/// ASCII-case-insensitive comparison of two byte strings.
#[must_use]
pub(crate) fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
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
    fn decodes_escaped_null() {
        // RFC 4475 3.1.1.4: %00 is legal in a URI and must decode to an actual NUL, which is
        // exactly why decoding yields bytes and not a string.
        assert_eq!(decode(b"null-%00-null").unwrap(), b"null-\x00-null");
    }

    #[test]
    fn rejects_truncated_escape() {
        assert!(decode(b"abc%4").is_none());
        assert!(decode(b"abc%").is_none());
        assert!(decode(b"abc%zz").is_none());
        assert!(!escapes_are_well_formed(b"abc%4"));
        assert!(escapes_are_well_formed(b"abc%41"));
    }

    #[test]
    fn normalization_decodes_unreserved_and_keeps_reserved() {
        // %61 is 'a', unreserved, so it folds.
        assert_eq!(normalize_for_comparison(b"%61lice"), b"alice");
        // %40 is '@', reserved, so it stays escaped — with canonical case.
        assert_eq!(normalize_for_comparison(b"bob%40biloxi"), b"bob%40biloxi");
        assert_eq!(
            normalize_for_comparison(b"bob%40x"),
            normalize_for_comparison(b"bob%40x")
        );
        // %2f and %2F are the same escape of the same reserved character.
        assert_eq!(
            normalize_for_comparison(b"a%2fb"),
            normalize_for_comparison(b"a%2Fb")
        );
        // ...and neither equals a literal slash, because '/' is a delimiter.
        assert_ne!(
            normalize_for_comparison(b"a%2fb"),
            normalize_for_comparison(b"a/b")
        );
    }

    #[test]
    fn normalization_leaves_malformed_escapes_alone() {
        assert_eq!(normalize_for_comparison(b"100%"), b"100%");
        assert_eq!(normalize_for_comparison(b"%zz"), b"%zz");
    }
}
