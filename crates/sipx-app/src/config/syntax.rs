//! The concrete syntax — §2 of [`specs/host-config.md`](../../../../docs/specs/host-config.md).
//!
//! A subset of TOML, hand-read rather than delegated, because **the subset is the syntax**: a
//! document accepted here and nowhere else would be a document an operator cannot read in someone
//! else's editor, and a parser more permissive than the spec page would silently make the page
//! wrong. Everything the subset omits is refused with the physical line that caused it (N1).
//!
//! This layer knows nothing about listeners, apps or knobs. It produces tables of typed values and
//! the lines they came from; deciding what a table may contain is [`super::schema`]'s.

use super::ConfigError;

/// A value, in the four shapes §2 defines plus the inline table its actions need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    /// A basic string.
    String(String),
    /// A decimal integer.
    Integer(i64),
    /// `true` or `false`.
    Boolean(bool),
    /// A list, possibly spanning lines.
    Array(Vec<Value>),
    /// An inline table, possibly spanning lines. Keys in the order they were written.
    Table(Vec<(String, Value)>),
}

impl Value {
    /// What to call this in a message about the wrong type.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::String(_) => "a string",
            Self::Integer(_) => "an integer",
            Self::Boolean(_) => "a boolean",
            Self::Array(_) => "a list",
            Self::Table(_) => "an inline table",
        }
    }
}

/// One `key = value`, and where it was written.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The key.
    pub(crate) key: String,
    /// Its value.
    pub(crate) value: Value,
    /// The line the entry started on.
    pub(crate) line: usize,
}

/// One `[a.b.c]` table and everything set in it.
#[derive(Debug, Clone)]
pub(crate) struct Table {
    /// The header's dotted path, split.
    pub(crate) path: Vec<String>,
    /// The line the header was on.
    pub(crate) line: usize,
    /// Its entries, in the order they were written.
    pub(crate) entries: Vec<Entry>,
}

impl Table {
    /// The header as it was written, for messages.
    pub(crate) fn header(&self) -> String {
        self.path.join(".")
    }
}

/// A whole document: tables, in the order they were opened.
#[derive(Debug, Clone)]
pub(crate) struct Document {
    /// The tables.
    pub(crate) tables: Vec<Table>,
}

/// A logical line: one or more physical lines joined, because an array or an inline table may span
/// them and nothing else may.
struct Logical {
    text: String,
    line: usize,
}

/// Read a document, or say which line stopped it.
pub(crate) fn parse(text: &str) -> Result<Document, ConfigError> {
    if text.starts_with('\u{feff}') {
        return Err(ConfigError::syntax(
            1,
            "the document must have no byte-order mark",
        ));
    }

    let mut tables: Vec<Table> = Vec::new();
    for logical in logical_lines(text)? {
        let mut cursor = Cursor::new(&logical.text, logical.line);
        cursor.skip_spaces();
        if cursor.peek() == Some('[') {
            let path = cursor.header()?;
            if tables.iter().any(|table| table.path == path) {
                return Err(ConfigError::duplicate_table(logical.line, &path.join(".")));
            }
            tables.push(Table {
                path,
                line: logical.line,
                entries: Vec::new(),
            });
            continue;
        }

        let entry = cursor.entry(logical.line)?;
        let Some(table) = tables.last_mut() else {
            return Err(ConfigError::syntax(
                logical.line,
                "a key before any [table] header",
            ));
        };
        if table.entries.iter().any(|held| held.key == entry.key) {
            let path = format!("{}.{}", table.header(), entry.key);
            return Err(ConfigError::duplicate_key(logical.line, &path));
        }
        table.entries.push(entry);
    }

    Ok(Document { tables })
}

/// Physical lines, comments removed, joined while an array or inline table is open.
fn logical_lines(text: &str) -> Result<Vec<Logical>, ConfigError> {
    let mut lines = Vec::new();
    let mut buffer = String::new();
    let mut start = 1;
    let mut depth = 0i32;
    let mut last = 1;

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        last = line;
        let (code, delta) = strip(raw, line)?;
        let trimmed = code.trim();
        if depth == 0 && trimmed.is_empty() {
            continue;
        }
        if buffer.is_empty() {
            start = line;
        } else {
            buffer.push(' ');
        }
        buffer.push_str(trimmed);
        depth += delta;
        if depth < 0 {
            return Err(ConfigError::syntax(line, "a ] or } that opens nothing"));
        }
        if depth == 0 {
            lines.push(Logical {
                text: std::mem::take(&mut buffer),
                line: start,
            });
        }
    }

    if depth != 0 {
        return Err(ConfigError::syntax(
            last,
            "an array or inline table left open at the end of the document",
        ));
    }
    Ok(lines)
}

/// One physical line's code half, and how far it opens or closes brackets.
///
/// A string may not span a line in this syntax, so an unterminated one is refused here rather than
/// swallowing the rest of the document as text.
fn strip(raw: &str, line: usize) -> Result<(String, i32), ConfigError> {
    let mut code = String::new();
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;

    for c in raw.chars() {
        if in_string {
            code.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '#' => break,
            '"' => {
                in_string = true;
                code.push(c);
            }
            '[' | '{' => {
                depth += 1;
                code.push(c);
            }
            ']' | '}' => {
                depth -= 1;
                code.push(c);
            }
            _ => code.push(c),
        }
    }

    if in_string {
        return Err(ConfigError::syntax(
            line,
            "a string may not span lines; multi-line and literal strings are not in the syntax",
        ));
    }
    Ok((code, depth))
}

/// A position in one logical line.
struct Cursor {
    chars: Vec<char>,
    at: usize,
    line: usize,
}

impl Cursor {
    fn new(text: &str, line: usize) -> Self {
        Self {
            chars: text.chars().collect(),
            at: 0,
            line,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.at += 1;
        }
        c
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, want: char, what: &str) -> Result<(), ConfigError> {
        if self.peek() == Some(want) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.wrong(what))
        }
    }

    fn wrong(&self, what: &str) -> ConfigError {
        ConfigError::syntax(self.line, what)
    }

    /// `[a.b.c]`, and nothing after it.
    fn header(&mut self) -> Result<Vec<String>, ConfigError> {
        self.expect('[', "a table header starts with [")?;
        let mut path = vec![self.name()?];
        while self.peek() == Some('.') {
            self.at += 1;
            path.push(self.name()?);
        }
        self.expect(']', "a table header ends with ]")?;
        self.end()?;
        Ok(path)
    }

    /// `key = value`, and nothing after it.
    fn entry(&mut self, line: usize) -> Result<Entry, ConfigError> {
        let key = self.name()?;
        self.skip_spaces();
        self.expect('=', "a key is followed by =")?;
        self.skip_spaces();
        let value = self.value()?;
        self.end()?;
        Ok(Entry { key, value, line })
    }

    /// A bare name: lowercase, starting with a letter.
    fn name(&mut self) -> Result<String, ConfigError> {
        if !matches!(self.peek(), Some(c) if c.is_ascii_lowercase()) {
            return Err(self.wrong(
                "a name is lowercase and starts with a letter; quoted and dotted keys are not in \
                 the syntax",
            ));
        }
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                name.push(c);
                self.at += 1;
            } else {
                break;
            }
        }
        Ok(name)
    }

    /// Nothing but spaces left.
    fn end(&mut self) -> Result<(), ConfigError> {
        self.skip_spaces();
        if self.peek().is_some() {
            return Err(self.wrong("trailing characters after the value"));
        }
        Ok(())
    }

    fn value(&mut self) -> Result<Value, ConfigError> {
        match self.peek() {
            Some('"') => self.string(),
            Some('[') => self.array(),
            Some('{') => self.inline_table(),
            Some('t' | 'f') => self.boolean(),
            Some(c) if c == '-' || c.is_ascii_digit() => self.integer(),
            _ => Err(self.wrong("a value is a string, integer, boolean, list or inline table")),
        }
    }

    fn string(&mut self) -> Result<Value, ConfigError> {
        self.expect('"', "a string starts with \"")?;
        let mut text = String::new();
        loop {
            match self.bump() {
                Some('"') => return Ok(Value::String(text)),
                Some('\\') => match self.bump() {
                    Some('"') => text.push('"'),
                    Some('\\') => text.push('\\'),
                    Some('n') => text.push('\n'),
                    Some('r') => text.push('\r'),
                    Some('t') => text.push('\t'),
                    _ => {
                        return Err(self.wrong(
                            "the only escapes are \\\" \\\\ \\n \\r \\t; \\u is not in the syntax",
                        ));
                    }
                },
                Some(c) => text.push(c),
                None => return Err(self.wrong("a string that never closes")),
            }
        }
    }

    fn boolean(&mut self) -> Result<Value, ConfigError> {
        let mut word = String::new();
        while matches!(self.peek(), Some(c) if c.is_ascii_alphabetic()) {
            if let Some(c) = self.bump() {
                word.push(c);
            }
        }
        match word.as_str() {
            "true" => Ok(Value::Boolean(true)),
            "false" => Ok(Value::Boolean(false)),
            _ => Err(self.wrong("an unquoted word is only ever true or false")),
        }
    }

    fn integer(&mut self) -> Result<Value, ConfigError> {
        let mut text = String::new();
        if self.peek() == Some('-') {
            text.push('-');
            self.at += 1;
        }
        let mut digits = 0;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            if let Some(c) = self.bump() {
                text.push(c);
                digits += 1;
            }
        }
        if digits == 0 {
            return Err(self.wrong("a number needs a digit"));
        }
        if !matches!(self.peek(), None | Some(' ' | '\t' | ',' | ']' | '}')) {
            return Err(self.wrong(
                "a number ends at a delimiter; floats, dates, 0x and 1_000 are not in the syntax",
            ));
        }
        text.parse::<i64>()
            .map(Value::Integer)
            .map_err(|_| self.wrong("a number too large to represent"))
    }

    fn array(&mut self) -> Result<Value, ConfigError> {
        self.expect('[', "a list starts with [")?;
        let mut items = Vec::new();
        loop {
            self.skip_spaces();
            if self.peek() == Some(']') {
                self.at += 1;
                return Ok(Value::Array(items));
            }
            items.push(self.value()?);
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.at += 1,
                Some(']') => {}
                _ => return Err(self.wrong("a list separates its items with , and ends with ]")),
            }
        }
    }

    fn inline_table(&mut self) -> Result<Value, ConfigError> {
        self.expect('{', "an inline table starts with {")?;
        let mut pairs: Vec<(String, Value)> = Vec::new();
        loop {
            self.skip_spaces();
            if self.peek() == Some('}') {
                self.at += 1;
                return Ok(Value::Table(pairs));
            }
            let key = self.name()?;
            if pairs.iter().any(|(held, _)| held == &key) {
                return Err(ConfigError::duplicate_key(self.line, &key));
            }
            self.skip_spaces();
            self.expect('=', "a key is followed by =")?;
            self.skip_spaces();
            let value = self.value()?;
            pairs.push((key, value));
            self.skip_spaces();
            match self.peek() {
                Some(',') => self.at += 1,
                Some('}') => {}
                _ => {
                    return Err(
                        self.wrong("an inline table separates its keys with , and ends with }")
                    );
                }
            }
        }
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

    #[test]
    fn a_comment_is_not_part_of_a_value() {
        let document = parse("[a]\nb = \"x # y\" # and this is a comment\n").unwrap();
        assert_eq!(
            document.tables[0].entries[0].value,
            Value::String("x # y".to_owned())
        );
    }

    #[test]
    fn a_list_may_span_lines_and_keeps_the_line_it_started_on() {
        let document = parse("[a]\nb = [\n  \"one\",\n  \"two\",\n]\n").unwrap();
        let entry = &document.tables[0].entries[0];
        assert_eq!(entry.line, 2);
        assert_eq!(
            entry.value,
            Value::Array(vec![
                Value::String("one".to_owned()),
                Value::String("two".to_owned()),
            ])
        );
    }

    #[test]
    fn a_string_may_not() {
        let error = parse("[a]\nb = \"one\ntwo\"\n").unwrap_err();
        assert_eq!(error.code(), "syntax");
        assert_eq!(error.line(), Some(2));
    }

    #[test]
    fn the_omitted_constructs_are_refused_by_name() {
        for (document, line) in [
            ("[a]\nb = 1.5\n", 2),
            ("[a]\nb = 0xff\n", 2),
            ("[a]\nb = 1_000\n", 2),
            ("[a]\nb = 1979-05-27\n", 2),
            ("[a]\nb = 'literal'\n", 2),
            ("[a]\nb.c = 1\n", 2),
            ("[a]\nB = 1\n", 2),
            ("[[a]]\n", 1),
        ] {
            let error = parse(document).unwrap_err();
            assert_eq!(error.code(), "syntax", "{document:?}");
            assert_eq!(error.line(), Some(line), "{document:?}: {error}");
        }
    }

    #[test]
    fn an_escape_outside_the_five_is_refused() {
        assert_eq!(
            parse("[a]\nb = \"\\u0041\"\n").unwrap_err().code(),
            "syntax"
        );
    }

    #[test]
    fn a_negative_integer_is_a_value_but_a_leading_dash_is_not_a_name() {
        assert_eq!(
            parse("[a]\nb = -1\n").unwrap().tables[0].entries[0].value,
            Value::Integer(-1)
        );
        assert_eq!(parse("[a]\n-b = 1\n").unwrap_err().code(), "syntax");
    }
}
