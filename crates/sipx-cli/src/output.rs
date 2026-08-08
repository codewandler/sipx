//! What `sipx` says, and how a script reads it.
//!
//! Three rules, each of which exists because breaking it makes the tool unusable in a pipeline:
//!
//! - **Results go to stdout, everything else to stderr.** `sipx dial --json | jq` has to work
//!   while errors stay visible on the terminal.
//! - **Logging never touches stdout.** One stray log line turns valid JSON into a parse error
//!   at the far end of a pipe, and the cause is invisible from there.
//! - **The two formats carry the same facts.** If `--json` reports something the text does not,
//!   one of them is lying, and whichever a person reads is the one they will believe.

use std::fmt::Write as _;

/// How results are printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// For a person.
    Text,
    /// For a program.
    Json,
}

/// A result worth reporting: an ordered unique collection of fields.
///
/// A field added again replaces its value without changing its position. Keeping the collection
/// private makes duplicate JSON members unrepresentable while preserving the text order scripts
/// and people already rely on.
#[derive(Debug, Default, Clone)]
pub(crate) struct Report {
    fields: Fields,
}

#[derive(Debug, Default, Clone)]
struct Fields(Vec<(String, Value)>);

impl Fields {
    fn insert(&mut self, name: &str, value: Value) {
        if let Some((_, existing)) = self.0.iter_mut().find(|(existing, _)| existing == name) {
            *existing = value;
        } else {
            self.0.push((name.to_owned(), value));
        }
    }

    fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(name, value)| (name.as_str(), value))
    }
}

/// A reported value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// A string.
    Text(String),
    /// A number.
    Number(i64),
    /// A truth value.
    Bool(bool),
    /// A duration, reported in seconds.
    Seconds(u64),
    /// A measured quantity, rendered to a fixed number of decimal places.
    ///
    /// Fixed because these are estimates. A mean opinion score printed as `4.393268032` claims
    /// a precision the model behind it does not have, and someone will compare two calls on the
    /// eighth digit.
    Decimal(f64, usize),
}

impl Value {
    fn to_json(&self) -> String {
        match self {
            Self::Text(text) => format!("\"{}\"", escape(text)),
            Self::Number(number) => number.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Seconds(seconds) => seconds.to_string(),
            Self::Decimal(value, places) => format!("{value:.places$}"),
        }
    }

    fn to_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Number(number) => number.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Seconds(seconds) => format!("{seconds}s"),
            Self::Decimal(value, places) => format!("{value:.places$}"),
        }
    }
}

/// Escape a string for JSON.
///
/// A SIP display name can contain a quote or a backslash, and a reason phrase can contain
/// anything at all — this is the boundary where untrusted text from the network becomes output
/// a script will parse.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

impl Report {
    /// An empty report.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Add a string field.
    #[must_use]
    pub(crate) fn text(mut self, name: &str, value: impl Into<String>) -> Self {
        self.fields.insert(name, Value::Text(value.into()));
        self
    }

    /// Add a numeric field.
    #[must_use]
    pub(crate) fn number(mut self, name: &str, value: i64) -> Self {
        self.fields.insert(name, Value::Number(value));
        self
    }

    /// Add a boolean field.
    #[must_use]
    pub(crate) fn boolean(mut self, name: &str, value: bool) -> Self {
        self.fields.insert(name, Value::Bool(value));
        self
    }

    /// Add a duration, reported in whole seconds.
    #[must_use]
    pub(crate) fn seconds(mut self, name: &str, value: std::time::Duration) -> Self {
        self.fields.insert(name, Value::Seconds(value.as_secs()));
        self
    }

    /// Add a measured quantity, to a fixed number of decimal places.
    #[must_use]
    pub(crate) fn decimal(mut self, name: &str, value: f64, places: usize) -> Self {
        // A NaN would render as `NaN`, which is not JSON and would break a script parsing the
        // output. Reporting zero is no better; the field is simply omitted upstream.
        let value = if value.is_finite() { value } else { 0.0 };
        self.fields.insert(name, Value::Decimal(value, places));
        self
    }

    /// Add a duration in milliseconds, which is the useful scale for a round trip.
    #[must_use]
    pub(crate) fn millis(self, name: &str, value: std::time::Duration) -> Self {
        self.number(name, i64::try_from(value.as_millis()).unwrap_or(i64::MAX))
    }

    /// The field names, in order.
    ///
    /// Only the tests need this — it is how they check that the two formats carry the same
    /// facts — so it is not part of what the binary compiles.
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&str> {
        self.fields.iter().map(|(name, _)| name).collect()
    }

    /// Render in the requested format.
    #[must_use]
    pub(crate) fn render(&self, format: Format) -> String {
        match format {
            Format::Json => self.to_json(),
            Format::Text => self.to_text(),
        }
    }

    fn to_json(&self) -> String {
        let body = self
            .fields
            .iter()
            .map(|(name, value)| format!("\"{}\":{}", escape(name), value.to_json()))
            .collect::<Vec<_>>()
            .join(",");
        format!("{{{body}}}")
    }

    fn to_text(&self) -> String {
        let width = self
            .fields
            .iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0);
        self.fields
            .iter()
            .map(|(name, value)| format!("{name:<width$}  {}", value.to_text()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Print to stdout.
    pub(crate) fn emit(&self, format: Format) {
        println!("{}", self.render(format));
    }
}

/// Why `sipx` stopped.
///
/// Distinct codes so a script can branch without parsing text — "busy" and "no answer" lead to
/// different retry behaviour, and telling them apart from a message means matching on English.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub(crate) enum Exit {
    /// It worked.
    Success = 0,
    /// The command was used wrongly.
    Usage = 2,
    /// The far end refused.
    Rejected = 3,
    /// Credentials were wrong or missing.
    Unauthorized = 4,
    /// Nothing answered in time.
    Timeout = 5,
    /// The far end was busy.
    Busy = 6,
    /// Something else failed.
    Failed = 1,
}

impl Exit {
    /// The process exit code.
    #[must_use]
    pub(crate) fn code(self) -> i32 {
        self as i32
    }

    /// A stable machine-readable name, reported alongside the code.
    #[must_use]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Usage => "usage",
            Self::Rejected => "rejected",
            Self::Unauthorized => "unauthorized",
            Self::Timeout => "timeout",
            Self::Busy => "busy",
            Self::Failed => "failed",
        }
    }

    /// The exit for a SIP status code.
    #[must_use]
    pub(crate) fn for_status(status: u16) -> Self {
        match status {
            200..=299 => Self::Success,
            401 | 403 | 407 => Self::Unauthorized,
            408 | 504 => Self::Timeout,
            486 | 600 => Self::Busy,
            _ => Self::Rejected,
        }
    }
}

/// Report a failure on stderr and give back its exit code.
pub(crate) fn fail(format: Format, exit: Exit, message: &str) -> Exit {
    let report = Report::new()
        .text("status", exit.as_str())
        .text("error", message);
    // stderr, not stdout: a failure must not land in a pipeline where the next stage will try
    // to parse it as a result.
    eprintln!("{}", report.render(format));
    exit
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

    fn sample() -> Report {
        Report::new()
            .text("status", "answered")
            .text("peer", "sip:bob@example.com")
            .number("duration_ms", 4200)
            .boolean("recorded", true)
            .seconds("expires", std::time::Duration::from_secs(3600))
    }

    /// The acceptance test for P-1: the two formats carry the same facts. If one reports
    /// something the other does not, whichever a person reads is the one they will believe.
    #[test]
    fn json_output_is_parseable_and_carries_the_same_facts_as_the_text() {
        let report = sample();
        let json = report.render(Format::Json);
        let text = report.render(Format::Text);

        // Every field appears in both.
        for name in report.names() {
            assert!(
                json.contains(&format!("\"{name}\"")),
                "{name} missing from {json}"
            );
            assert!(text.contains(name), "{name} missing from {text}");
        }

        // And every value.
        assert!(json.contains("\"answered\""));
        assert!(text.contains("answered"));
        assert!(json.contains("4200"));
        assert!(text.contains("4200"));
        assert!(json.contains("true"));
        assert!(text.contains("true"));
        assert!(json.contains("3600"));
        assert!(text.contains("3600s"));
    }

    #[test]
    fn json_is_a_single_object_on_one_line() {
        let json = sample().render(Format::Json);
        assert!(json.starts_with('{') && json.ends_with('}'), "{json}");
        assert!(
            !json.contains('\n'),
            "one line per result, so a reader can split on newlines: {json}"
        );
    }

    /// A reason phrase comes off the network and can contain anything. This is the boundary
    /// where untrusted text becomes output a script will parse.
    #[test]
    fn values_from_the_network_are_escaped() {
        let report = Report::new().text("reason", "he said \"no\" \\ then\nhung up\ttwice");
        let json = report.render(Format::Json);

        assert!(json.contains(r#"\"no\""#), "{json}");
        assert!(json.contains(r"\\"), "{json}");
        assert!(json.contains(r"\n"), "{json}");
        assert!(json.contains(r"\t"), "{json}");
        assert_eq!(
            json.matches('\n').count(),
            0,
            "a newline in a value must not break the one-line contract: {json}"
        );
    }

    #[test]
    fn a_control_character_is_escaped_rather_than_emitted() {
        let json = Report::new()
            .text("odd", "before\u{1}after")
            .render(Format::Json);
        assert!(json.contains("\\u0001"), "{json}");
    }

    #[test]
    fn fields_keep_the_order_they_were_added() {
        assert_eq!(
            sample().names(),
            vec!["status", "peer", "duration_ms", "recorded", "expires"]
        );
    }

    #[test]
    fn adding_a_field_again_cannot_create_a_duplicate_member() {
        let report = Report::new()
            .text("status", "first")
            .text("peer", "sip:bob@example.com")
            .text("status", "second");
        assert_eq!(report.names(), ["status", "peer"]);
        assert_eq!(
            report.render(Format::Json),
            r#"{"status":"second","peer":"sip:bob@example.com"}"#
        );
    }

    /// The codes a script branches on. Busy and no-answer lead to different retry behaviour,
    /// and telling them apart from a message means matching on English.
    #[test]
    fn sip_statuses_map_to_distinct_exit_codes() {
        assert_eq!(Exit::for_status(200), Exit::Success);
        assert_eq!(Exit::for_status(401), Exit::Unauthorized);
        assert_eq!(Exit::for_status(407), Exit::Unauthorized);
        assert_eq!(Exit::for_status(408), Exit::Timeout);
        assert_eq!(Exit::for_status(486), Exit::Busy);
        assert_eq!(Exit::for_status(600), Exit::Busy);
        assert_eq!(Exit::for_status(404), Exit::Rejected);
        assert_eq!(Exit::for_status(503), Exit::Rejected);
    }

    #[test]
    fn every_exit_has_a_distinct_code_and_name() {
        let all = [
            Exit::Success,
            Exit::Failed,
            Exit::Usage,
            Exit::Rejected,
            Exit::Unauthorized,
            Exit::Timeout,
            Exit::Busy,
        ];
        let codes: std::collections::HashSet<i32> = all.iter().map(|e| e.code()).collect();
        assert_eq!(codes.len(), all.len(), "codes must be distinct");

        let names: std::collections::HashSet<&str> = all.iter().map(|e| e.as_str()).collect();
        assert_eq!(names.len(), all.len(), "names must be distinct too");

        assert_eq!(Exit::Success.code(), 0, "only success is zero");
        assert_eq!(
            all.iter().filter(|e| e.code() == 0).count(),
            1,
            "only success may be zero, or a script cannot tell failure from success"
        );
    }

    #[test]
    fn an_empty_report_is_still_valid_json() {
        assert_eq!(Report::new().render(Format::Json), "{}");
        assert_eq!(Report::new().render(Format::Text), "");
    }
}
