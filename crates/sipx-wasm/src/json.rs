//! Canonical JSON emission (`docs/specs/browser-sdk.md` §5.1).
//!
//! > For vector determinism the kernel emits canonical JSON: UTF-8, no insignificant whitespace,
//! > fields in the order this section defines them.
//!
//! That is why events are written by hand here instead of being derived. A serialiser that is
//! free to choose field order cannot be held to a byte vector, and §9's `BSDK-EVT-*` hashes are
//! byte vectors. Parsing runs the other way — host documents arrive in any order and are read
//! with `serde_json` in [`crate::command`].

/// A JSON object being written, field by field, in the order the caller writes them.
#[derive(Debug)]
pub(crate) struct Writer {
    out: String,
    empty: bool,
}

impl Writer {
    /// Open an object.
    pub(crate) fn object() -> Self {
        Self {
            out: String::from("{"),
            empty: true,
        }
    }

    fn separate(&mut self) {
        if self.empty {
            self.empty = false;
        } else {
            self.out.push(',');
        }
    }

    fn key(&mut self, name: &str) {
        self.separate();
        escape_into(name, &mut self.out);
        self.out.push(':');
    }

    /// A string-valued field.
    pub(crate) fn string(&mut self, name: &str, value: &str) -> &mut Self {
        self.key(name);
        escape_into(value, &mut self.out);
        self
    }

    /// An unsigned-integer-valued field.
    pub(crate) fn number(&mut self, name: &str, value: u64) -> &mut Self {
        self.key(name);
        self.out.push_str(&value.to_string());
        self
    }

    /// A boolean-valued field.
    pub(crate) fn boolean(&mut self, name: &str, value: bool) -> &mut Self {
        self.key(name);
        self.out.push_str(if value { "true" } else { "false" });
        self
    }

    /// A field whose value is a nested object, written by `build`.
    pub(crate) fn object_field(&mut self, name: &str, build: impl FnOnce(&mut Self)) -> &mut Self {
        self.key(name);
        let mut nested = Self::object();
        build(&mut nested);
        self.out.push_str(&nested.finish());
        self
    }

    /// A string field written only when the value is present, which is how §5.3's `"field"?`
    /// columns are spelled.
    pub(crate) fn string_opt(&mut self, name: &str, value: Option<&str>) -> &mut Self {
        if let Some(value) = value {
            self.string(name, value);
        }
        self
    }

    /// A numeric field written only when the value is present.
    pub(crate) fn number_opt(&mut self, name: &str, value: Option<u64>) -> &mut Self {
        if let Some(value) = value {
            self.number(name, value);
        }
        self
    }

    /// Close the object and take the document.
    pub(crate) fn finish(mut self) -> String {
        self.out.push('}');
        self.out
    }
}

/// Lowercase hex digits, for the `\u00XX` escape form.
const DIGITS: [u8; 16] = *b"0123456789abcdef";

/// RFC 8259 §7 string escaping.
///
/// Control characters below `0x20` take the `\u00XX` form except for the five with short
/// escapes. Nothing else is escaped: `/` is not, and non-ASCII stays as UTF-8, because the
/// encoding convention in §9.1 is "exactly the displayed characters".
fn escape_into(value: &str, out: &mut String) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            control if control < ' ' => {
                out.push_str("\\u");
                let code = control as u32;
                for shift in [12u32, 8, 4, 0] {
                    let nibble = usize::try_from((code >> shift) & 0xf).unwrap_or(0);
                    out.push(char::from(DIGITS.get(nibble).copied().unwrap_or(b'0')));
                }
            }
            other => out.push(other),
        }
    }
    out.push('"');
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
    fn fields_keep_the_order_they_were_written_in() {
        let mut writer = Writer::object();
        writer
            .number("v", 1)
            .string("evt", "need-entropy")
            .number("min", 64);
        assert_eq!(writer.finish(), r#"{"v":1,"evt":"need-entropy","min":64}"#);
    }

    #[test]
    fn nested_objects_are_written_inline() {
        let mut writer = Writer::object();
        writer
            .number("v", 1)
            .string("evt", "need-local-media")
            .number("call", 1)
            .string("kind", "offer")
            .object_field("constraints", |constraints| {
                constraints.boolean("audio", true).boolean("video", false);
            });
        assert_eq!(
            writer.finish(),
            r#"{"v":1,"evt":"need-local-media","call":1,"kind":"offer","constraints":{"audio":true,"video":false}}"#
        );
    }

    #[test]
    fn absent_optional_fields_are_omitted_entirely() {
        let mut writer = Writer::object();
        writer
            .number("v", 1)
            .string("evt", "registration")
            .string("state", "registered")
            .number_opt("expires", Some(600))
            .number_opt("status", None)
            .string_opt("reason", None);
        assert_eq!(
            writer.finish(),
            r#"{"v":1,"evt":"registration","state":"registered","expires":600}"#
        );
    }

    #[test]
    fn a_quote_in_a_value_cannot_end_the_string() {
        let mut writer = Writer::object();
        writer.string("reason", "he said \"no\"\n");
        assert_eq!(writer.finish(), r#"{"reason":"he said \"no\"\n"}"#);
    }

    #[test]
    fn control_characters_take_the_short_unicode_escape() {
        let mut writer = Writer::object();
        writer.string("reason", "a\u{1}b");
        assert_eq!(writer.finish(), r#"{"reason":"a\u0001b"}"#);
    }
}
