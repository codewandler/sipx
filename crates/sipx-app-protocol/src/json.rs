//! JSON (RFC 8259), just enough of it, written here because the contract needs a wire and this
//! workspace has no serialization framework to borrow one from.
//!
//! Two properties matter more than completeness. The first is that **this parser eats hostile
//! input**: an app's response document arrives over a socket from somewhere else, so every path
//! here returns a typed [`JsonError`] rather than panicking, and nesting is bounded by
//! [`MAX_DEPTH`] so a document made of ten thousand open brackets cannot exhaust the stack. The
//! second is that it is *small*: the contract's vocabulary is objects, arrays, strings, integers,
//! booleans and null, and a general-purpose value tree is all the interpreter above it wants.
//!
//! Numbers are split into [`Json::Int`] and [`Json::Float`] rather than being kept as `f64`. Every
//! number the contract names is an integer — a `seq`, a `status`, a `duration_ms`, a `max` — and
//! rounding one of those through a binary float to serialize it back is the kind of quiet
//! infidelity a wire format should not have.

use std::collections::BTreeMap;
use std::fmt;

/// How deeply arrays and objects may nest before a document is refused.
///
/// The contract's own documents nest three deep (document → instructions → source), so this is
/// an order of magnitude of headroom and still nowhere near a stack a recursive parser could
/// run out of. It exists because the alternative to a limit is a crash on input the sender
/// chose.
pub const MAX_DEPTH: usize = 32;

/// A JSON value.
///
/// Objects keep their keys sorted ([`BTreeMap`]) rather than in document order. The contract
/// never gives meaning to member order (RFC 8259 §4 says objects are unordered), and a sorted
/// map makes serialization deterministic — which is what lets a test compare two documents as
/// text instead of walking them.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number with no fraction and no exponent.
    Int(i64),
    /// Any other number.
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Json>),
    /// An object.
    Object(BTreeMap<String, Json>),
}

/// Why a document could not be read as JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum JsonError {
    /// The document ended in the middle of a value.
    Truncated,
    /// A byte that cannot start or continue the value being read, and where it was.
    Unexpected {
        /// The offending byte.
        byte: u8,
        /// Its offset from the start of the document.
        at: usize,
    },
    /// Nesting past [`MAX_DEPTH`].
    TooDeep,
    /// A number literal that is not a number.
    BadNumber,
    /// A `\u` escape that is not a scalar value, or a lone surrogate.
    BadEscape,
    /// The document was not UTF-8, or a string contained a raw control character.
    NotUtf8,
    /// Something followed the top-level value.
    Trailing {
        /// Where the extra input starts.
        at: usize,
    },
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "the document ended mid-value"),
            Self::Unexpected { byte, at } => {
                write!(f, "unexpected byte {byte:#04x} at offset {at}")
            }
            Self::TooDeep => write!(f, "nested deeper than {MAX_DEPTH}"),
            Self::BadNumber => write!(f, "not a number"),
            Self::BadEscape => write!(f, "not a valid string escape"),
            Self::NotUtf8 => write!(f, "not valid UTF-8, or a raw control character in a string"),
            Self::Trailing { at } => write!(f, "trailing input at offset {at}"),
        }
    }
}

impl std::error::Error for JsonError {}

impl Json {
    /// Read one JSON value, and require it to be the whole document.
    ///
    /// # Errors
    ///
    /// [`JsonError`] for anything that is not exactly one well-formed value. Never panics, on any
    /// input at all — this is the crate's outermost boundary against a peer.
    pub fn parse(input: &str) -> Result<Self, JsonError> {
        let mut parser = Parser {
            bytes: input.as_bytes(),
            at: 0,
        };
        let value = parser.value(0)?;
        parser.skip_whitespace();
        if parser.at < parser.bytes.len() {
            return Err(JsonError::Trailing { at: parser.at });
        }
        Ok(value)
    }

    /// The value as a string, if it is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as an integer, if it is one.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// The value as a boolean, if it is one.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as an array, if it is one.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The value as an object, if it is one.
    #[must_use]
    pub fn as_object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(members) => Some(members),
            _ => None,
        }
    }

    /// One member of an object, if this is an object and it has that member.
    ///
    /// `null` is reported as absent. §4 of the contract says unknown fields are ignored and
    /// nothing in the vocabulary distinguishes "absent" from "explicitly null", so collapsing the
    /// two here keeps every reader above from having to.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self.as_object()?.get(key) {
            Some(Self::Null) | None => None,
            Some(value) => Some(value),
        }
    }

    /// Build an object from members, dropping the ones whose value is `None`.
    ///
    /// Omitting an absent field rather than writing `null` is what makes the serialized envelopes
    /// match the spec's examples, which never show a null.
    #[must_use]
    pub fn object<I>(members: I) -> Self
    where
        I: IntoIterator<Item = (&'static str, Option<Self>)>,
    {
        Self::Object(
            members
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value)))
                .collect(),
        )
    }

    /// The value as compact JSON text: UTF-8, no BOM, no insignificant whitespace.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Int(n) => out.push_str(&n.to_string()),
            Self::Float(n) => {
                // A non-finite float has no JSON spelling (RFC 8259 §6). Nothing in the contract
                // produces one; writing `null` rather than inventing `NaN` keeps the output
                // parseable if something ever does.
                if n.is_finite() {
                    out.push_str(&n.to_string());
                } else {
                    out.push_str("null");
                }
            }
            Self::Str(s) => write_string(s, out),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Self::Str(value.to_owned())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Self::Str(value)
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<u64> for Json {
    fn from(value: u64) -> Self {
        // Saturating rather than wrapping: a `seq` past `i64::MAX` is not reachable in a call's
        // lifetime, and a silently negative one would be worse than a clamped one.
        Self::Int(i64::try_from(value).unwrap_or(i64::MAX))
    }
}

impl From<u32> for Json {
    fn from(value: u32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<u16> for Json {
    fn from(value: u16) -> Self {
        Self::Int(i64::from(value))
    }
}

/// RFC 8259 §7: the two mandatory escapes, the control range, and nothing else.
///
/// Solidus and the non-ASCII range are deliberately left alone — escaping them is permitted and
/// pointless, and the output is UTF-8 by construction.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                for shift in [12u32, 8, 4, 0] {
                    let nibble = ((c as u32) >> shift) & 0xf;
                    out.push(char::from_digit(nibble, 16).unwrap_or('0'));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        match self.peek() {
            Some(found) if found == byte => {
                self.at += 1;
                Ok(())
            }
            Some(found) => Err(JsonError::Unexpected {
                byte: found,
                at: self.at,
            }),
            None => Err(JsonError::Truncated),
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, JsonError> {
        let end = self.at + word.len();
        match self.bytes.get(self.at..end) {
            Some(found) if found == word.as_bytes() => {
                self.at = end;
                Ok(value)
            }
            Some(_) => Err(JsonError::Unexpected {
                byte: self.peek().unwrap_or(b'?'),
                at: self.at,
            }),
            None => Err(JsonError::Truncated),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::TooDeep);
        }
        self.skip_whitespace();
        match self.peek().ok_or(JsonError::Truncated)? {
            b'n' => self.literal("null", Json::Null),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'"' => self.string().map(Json::Str),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number(),
            byte => Err(JsonError::Unexpected { byte, at: self.at }),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.skip_whitespace();
            match self.bump().ok_or(JsonError::Truncated)? {
                b',' => {}
                b']' => return Ok(Json::Array(items)),
                byte => {
                    return Err(JsonError::Unexpected {
                        byte,
                        at: self.at - 1,
                    });
                }
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.expect(b'{')?;
        let mut members = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(members));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let value = self.value(depth + 1)?;
            // Last member wins, which is one of the two readings RFC 8259 §4 leaves open and the
            // only one that cannot silently drop the value a sender wrote most recently.
            members.insert(key, value);
            self.skip_whitespace();
            match self.bump().ok_or(JsonError::Truncated)? {
                b',' => {}
                b'}' => return Ok(Json::Object(members)),
                byte => {
                    return Err(JsonError::Unexpected {
                        byte,
                        at: self.at - 1,
                    });
                }
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = String::new();
        // Raw bytes are accumulated so a multi-byte character split across the loop is decoded
        // once, at the end, rather than a byte at a time.
        let mut raw: Vec<u8> = Vec::new();
        loop {
            match self.bump().ok_or(JsonError::Truncated)? {
                b'"' => {
                    push_utf8(&raw, &mut out)?;
                    return Ok(out);
                }
                b'\\' => {
                    push_utf8(&raw, &mut out)?;
                    raw.clear();
                    self.escape(&mut out)?;
                }
                // RFC 8259 §7: unescaped control characters are not allowed in a string.
                byte if byte < 0x20 => return Err(JsonError::NotUtf8),
                byte => raw.push(byte),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let ch = match self.bump().ok_or(JsonError::Truncated)? {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{08}',
            b'f' => '\u{0c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => return self.unicode_escape(out),
            _ => return Err(JsonError::BadEscape),
        };
        out.push(ch);
        Ok(())
    }

    /// `\uXXXX`, including the surrogate pair that RFC 8259 §7 requires for anything above the
    /// basic plane. A lone surrogate is refused rather than replaced: the contract's strings are
    /// URIs, digits and tags, and a silent U+FFFD in one of those is a bug that travels.
    fn unicode_escape(&mut self, out: &mut String) -> Result<(), JsonError> {
        let first = self.hex4()?;
        let scalar = if (0xd800..0xdc00).contains(&first) {
            if self.bump() != Some(b'\\') || self.bump() != Some(b'u') {
                return Err(JsonError::BadEscape);
            }
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return Err(JsonError::BadEscape);
            }
            0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00)
        } else {
            first
        };
        out.push(char::from_u32(scalar).ok_or(JsonError::BadEscape)?);
        Ok(())
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self.bump().ok_or(JsonError::Truncated)?;
            let digit = char::from(byte)
                .to_digit(16)
                .ok_or(JsonError::BadEscape)?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let mut exact = true;
        while let Some(byte) = self.peek() {
            match byte {
                b'0'..=b'9' => self.at += 1,
                b'.' | b'e' | b'E' | b'+' | b'-' => {
                    exact = false;
                    self.at += 1;
                }
                _ => break,
            }
        }
        let text = self
            .bytes
            .get(start..self.at)
            .and_then(|slice| std::str::from_utf8(slice).ok())
            .ok_or(JsonError::BadNumber)?;
        if exact {
            // An integer too large for `i64` is not rounded into one — a `seq` or a `duration_ms`
            // that does not fit is a document this contract has no reading for.
            text.parse::<i64>().map(Json::Int).map_err(|_| JsonError::BadNumber)
        } else {
            text.parse::<f64>()
                .map(Json::Float)
                .map_err(|_| JsonError::BadNumber)
        }
    }
}

fn push_utf8(raw: &[u8], out: &mut String) -> Result<(), JsonError> {
    if raw.is_empty() {
        return Ok(());
    }
    out.push_str(std::str::from_utf8(raw).map_err(|_| JsonError::NotUtf8)?);
    Ok(())
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

    #[test]
    fn the_spec_s_own_envelope_example_round_trips() {
        let text = r#"{"contract":"sipx.app.v1","seq":4,"event":{"type":"call.dtmf","digit":"5","duration_ms":160}}"#;
        let value = Json::parse(text).unwrap();
        assert_eq!(value.get("seq").unwrap().as_i64(), Some(4));
        assert_eq!(
            value.get("event").unwrap().get("digit").unwrap().as_str(),
            Some("5")
        );
        // The map is sorted, so the text is not byte-identical; re-parsing is the round trip that
        // matters.
        assert_eq!(Json::parse(&value.to_text()).unwrap(), value);
    }

    #[test]
    fn escapes_survive_both_directions() {
        let value = Json::Str("a\"b\\c\nd\u{1}e\u{1f600}".to_owned());
        let text = value.to_text();
        assert!(text.contains("\\u0001"), "control characters are escaped: {text}");
        assert_eq!(Json::parse(&text).unwrap(), value);
    }

    #[test]
    fn a_surrogate_pair_decodes_and_a_lone_surrogate_is_refused() {
        assert_eq!(
            Json::parse(r#""😀""#).unwrap(),
            Json::Str("\u{1f600}".to_owned())
        );
        assert_eq!(Json::parse(r#""\ud83d""#), Err(JsonError::BadEscape));
    }

    /// The property AGENTS.md non-negotiable 3 asks for, on the one type in this crate that reads
    /// bytes a peer chose: every input is an answer, never a panic.
    #[test]
    fn nothing_a_peer_can_send_panics() {
        let hostile = [
            "",
            " ",
            "{",
            "[",
            "\"",
            "\"\\",
            "\"\\u",
            "\"\\uZZZZ",
            "{\"a\"",
            "{\"a\":",
            "{\"a\":1,",
            "[1,",
            "-",
            "-.",
            "1e",
            "999999999999999999999999999",
            "nul",
            "tru",
            "fals",
            "{}{}",
            "\u{0}",
            "[\"\u{1}\"]",
        ];
        for input in hostile {
            let _ = Json::parse(input);
        }
        // Deeper than `MAX_DEPTH`, which is the input a recursive parser without a limit dies on.
        let deep = "[".repeat(10_000) + &"]".repeat(10_000);
        assert_eq!(Json::parse(&deep), Err(JsonError::TooDeep));
    }

    #[test]
    fn nesting_is_allowed_right_up_to_the_limit() {
        let ok = "[".repeat(MAX_DEPTH) + "1" + &"]".repeat(MAX_DEPTH);
        assert!(Json::parse(&ok).is_ok(), "{MAX_DEPTH} deep must parse");
        let over = "[".repeat(MAX_DEPTH + 2) + "1" + &"]".repeat(MAX_DEPTH + 2);
        assert_eq!(Json::parse(&over), Err(JsonError::TooDeep));
    }

    #[test]
    fn an_explicit_null_reads_as_an_absent_member() {
        let value = Json::parse(r#"{"a":null,"b":1}"#).unwrap();
        assert!(value.get("a").is_none());
        assert!(value.get("b").is_some());
    }

    #[test]
    fn integers_stay_integers() {
        assert_eq!(Json::parse("4000").unwrap(), Json::Int(4000));
        assert_eq!(Json::parse("4000").unwrap().to_text(), "4000");
        assert!(matches!(Json::parse("4.5").unwrap(), Json::Float(_)));
    }
}
