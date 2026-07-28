//! The RFC 4475 SIP torture-test corpus.
//!
//! RFC 4475 collects messages designed to break naive parsers: legal ones that look illegal,
//! illegal ones that look legal, and a long tail of whitespace, escaping and scalar-range
//! edge cases. It is the standard correctness bar for a SIP implementation, so sipx runs it
//! continuously rather than at the end.
//!
//! The messages are recovered from the bit-exact archive in that RFC's Appendix A by
//! `scripts/import-rfc4475-corpus.sh`, not retyped: several cases hinge on octets that do not
//! survive transcription (escaped NULs, UTF-8 display names, trailing whitespace). Run the
//! script with `--check` to verify the committed corpus still matches the RFC.
//!
//! # What the classification is for
//!
//! The RFC groups its messages by the *layer* that should object to them, and that grouping
//! is the useful part. A negative `Content-Length` is a framing failure — the byte stream
//! cannot be cut into messages at all. An overlarge `CSeq` is nothing of the sort: the message
//! frames and forwards perfectly well, and the fault appears only when something reads
//! `CSeq`. An implementation that conflates the two either drops messages it could have
//! forwarded, or accepts messages it cannot frame.
//!
//! So each case carries an [`Expect`] naming which layer must object, and the parser tests
//! assert against that rather than against a bare pass/fail.

/// Which layer must object to a message, and how.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    /// Parses, and re-serializes byte-identically. Anything else about the message —
    /// unknown schemes, unknown methods, a `Max-Forwards` of zero — is a concern for a layer
    /// above the parser.
    ParseOk,
    /// The structural parser must reject it: the bytes cannot be turned into a message.
    ParseErr(Fault),
    /// Parses. Reading the named header must yield a `HeaderError`.
    HeaderErr(&'static str),
    /// Parses, and every header parses. Request/response validation must reject it, with the
    /// stated reason.
    ValidateErr(&'static str),
    /// Present in the Appendix A archive but referenced by no section of the RFC. Carried so
    /// the corpus is a faithful copy of the archive, but asserted on by nothing.
    Unreferenced,
}

/// The structural fault a [`Expect::ParseErr`] case must produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The request or status line is malformed.
    StartLine,
    /// A header line is malformed — bad field name, stray separator.
    HeaderSyntax,
    /// The body cannot be delimited: `Content-Length` absent, repeated, negative,
    /// non-numeric, or larger than the datagram.
    Framing,
}

/// One message from the corpus.
#[derive(Debug, Clone, Copy)]
pub struct Case {
    /// The RFC's own name for the message, e.g. `wsinv`.
    pub name: &'static str,
    /// The section of RFC 4475 that describes it, e.g. `3.1.1.1`.
    pub section: &'static str,
    /// That section's title.
    pub title: &'static str,
    /// Which layer must object, and how.
    pub expect: Expect,
    /// The message, bit-exact.
    pub bytes: &'static [u8],
}

impl Case {
    /// The message as a lossy string, for assertion messages. Several cases are not UTF-8;
    /// never use this for parsing.
    #[must_use]
    pub fn lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.bytes)
    }

    /// Whether this case is asserted on by the parser tests.
    #[must_use]
    pub fn is_classified(&self) -> bool {
        self.expect != Expect::Unreferenced
    }
}

macro_rules! corpus {
    ($($name:literal => $section:literal, $title:literal, $expect:expr;)*) => {
        /// Every message in the corpus, in RFC section order.
        pub static CASES: &[Case] = &[$(
            Case {
                name: $name,
                section: $section,
                title: $title,
                expect: $expect,
                bytes: include_bytes!(concat!("../corpus/rfc4475/", $name, ".dat")),
            },
        )*];
    };
}

use Expect::{HeaderErr, ParseErr, ParseOk, Unreferenced, ValidateErr};
use Fault::{Framing, HeaderSyntax, StartLine};

corpus! {
    // ---- 3.1.1 Valid messages -------------------------------------------------------
    "wsinv"      => "3.1.1.1",  "A Short Tortuous INVITE", ParseOk;
    "intmeth"    => "3.1.1.2",  "Wide Range of Valid Characters", ParseOk;
    "esc01"      => "3.1.1.3",  "Valid Use of the % Escaping Mechanism", ParseOk;
    "escnull"    => "3.1.1.4",  "Escaped Nulls in URIs", ParseOk;
    "esc02"      => "3.1.1.5",  "Use of % When It Is Not an Escape", ParseOk;
    "lwsdisp"    => "3.1.1.6",  "Message with No LWS between Display Name and <", ParseOk;
    "longreq"    => "3.1.1.7",  "Long Values in Header Fields", ParseOk;
    "dblreq"     => "3.1.1.8",  "Extra Trailing Octets in a UDP Datagram", ParseOk;
    "semiuri"    => "3.1.1.9",  "Semicolon-Separated Parameters in URI User Part", ParseOk;
    "transports" => "3.1.1.10", "Varied and Unknown Transport Types", ParseOk;
    "mpart01"    => "3.1.1.11", "Multipart MIME Message", ParseOk;
    "unreason"   => "3.1.1.12", "Unusual Reason Phrase", ParseOk;
    "noreason"   => "3.1.1.13", "Empty Reason Phrase", ParseOk;

    // ---- 3.1.2 Invalid messages -----------------------------------------------------
    // Structural: the bytes cannot be framed or the grammar is violated.
    "badinv01"   => "3.1.2.1",  "Extraneous Header Field Separators", ParseErr(HeaderSyntax);
    "clerr"      => "3.1.2.2",  "Content Length Larger Than Message", ParseErr(Framing);
    "ncl"        => "3.1.2.3",  "Negative Content-Length", ParseErr(Framing);
    "ltgtruri"   => "3.1.2.7",  "<> Enclosing Request-URI", ParseErr(StartLine);
    "lwsruri"    => "3.1.2.8",  "Malformed SIP Request-URI (embedded LWS)", ParseErr(StartLine);
    "lwsstart"   => "3.1.2.9",  "Multiple SP Separating Request-Line Elements", ParseErr(StartLine);
    "trws"       => "3.1.2.10", "SP Characters at End of Request-Line", ParseErr(StartLine);
    "bigcode"    => "3.1.2.19", "Overlarge Response Code", ParseErr(StartLine);

    // Value-level: the message frames, and one header's value is bad.
    "scalar02"   => "3.1.2.4",  "Request Scalar Fields with Overlarge Values", HeaderErr("CSeq");
    "scalarlg"   => "3.1.2.5",  "Response Scalar Fields with Overlarge Values", HeaderErr("CSeq");
    "quotbal"    => "3.1.2.6",  "Unterminated Quoted String in Display Name", HeaderErr("To");
    "baddate"    => "3.1.2.12", "Invalid Time Zone in Date Header Field", HeaderErr("Date");
    "regbadct"   => "3.1.2.13", "Failure to Enclose name-addr URI in <>", HeaderErr("Contact");
    "badaspec"   => "3.1.2.14", "Spaces within addr-spec", HeaderErr("To");
    "baddn"      => "3.1.2.15", "Non-token Characters in Display Name", HeaderErr("From");

    // Semantic: everything parses; the message is still not one we may act on.
    "escruri"    => "3.1.2.11", "Escaped Headers in SIP Request-URI",
                                ValidateErr("a Request-URI may not carry headers (RFC 3261 19.1.1)");
    "badvers"    => "3.1.2.16", "Unknown Protocol Version",
                                ValidateErr("unsupported SIP version; answer 505");
    "mismatch01" => "3.1.2.17", "Start Line and CSeq Method Mismatch",
                                ValidateErr("CSeq method must match the request line");
    "mismatch02" => "3.1.2.18", "Unknown Method with CSeq Method Mismatch",
                                ValidateErr("CSeq method must match the request line");

    // ---- 3.2 Transaction layer ------------------------------------------------------
    "badbranch"  => "3.2.1",    "Missing Transaction Identifier", ParseOk;

    // ---- 3.3 Application layer ------------------------------------------------------
    "insuf"      => "3.3.1",    "Missing Required Header Fields",
                                ValidateErr("To, From, Call-ID, CSeq and Via are required");
    "unkscm"     => "3.3.2",    "Request-URI with Unknown Scheme", ParseOk;
    "novelsc"    => "3.3.3",    "Request-URI with Known but Atypical Scheme", ParseOk;
    "unksm2"     => "3.3.4",    "Unknown URI Schemes in Header Fields", ParseOk;
    "bext01"     => "3.3.5",    "Proxy-Require and Require", ParseOk;
    "invut"      => "3.3.6",    "Unknown Content-Type", ParseOk;
    "regaut01"   => "3.3.7",    "Unknown Authorization Scheme", ParseOk;
    "multi01"    => "3.3.8",    "Multiple Values in Single Value Required Fields",
                                ValidateErr("single-value headers must not be repeated");
    // The RFC files this under the application layer, permitting a 400. sipx rejects it while
    // framing instead: two Content-Length values means the body's extent is unknown, which is
    // a framing question, not a semantic one. See docs/specs/sip-parser.md, section 4.4.
    "mcl01"      => "3.3.9",    "Multiple Content-Length Values", ParseErr(Framing);
    "bcast"      => "3.3.10",   "200 OK Response with Broadcast Via Header Field Value", ParseOk;
    "zeromf"     => "3.3.11",   "Max-Forwards of Zero", ParseOk;
    "cparam01"   => "3.3.12",   "REGISTER with a Contact Header Parameter", ParseOk;
    "cparam02"   => "3.3.13",   "REGISTER with a url-parameter", ParseOk;
    "regescrt"   => "3.3.14",   "REGISTER with a URL Escaped Header", ParseOk;
    "sdp01"      => "3.3.15",   "Unacceptable Accept Offering", ParseOk;

    // ---- 3.4 Backward compatibility -------------------------------------------------
    "inv2543"    => "3.4.1",    "INVITE with RFC 2543 Syntax", ParseOk;

    // ---- Present in the archive, referenced by no section ---------------------------
    "test"       => "-",        "(not referenced by RFC 4475)", Unreferenced;
}

/// Every case the parser tests assert on.
pub fn classified() -> impl Iterator<Item = &'static Case> {
    CASES.iter().filter(|c| c.is_classified())
}

/// Cases matching a given expectation.
pub fn expecting(expect: Expect) -> impl Iterator<Item = &'static Case> {
    CASES.iter().filter(move |c| c.expect == expect)
}

/// Look up a case by its RFC name.
#[must_use]
pub fn case(name: &str) -> Option<&'static Case> {
    CASES.iter().find(|c| c.name == name)
}

#[cfg(test)]
// The no-unwrap/no-panic rules exist because library code parses hostile input. A test that
// cannot read its own fixtures should fail loudly.
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The corpus is only a correctness bar if it is complete. A case that quietly goes
    /// missing — a renamed file, a botched import — would weaken the suite invisibly, so
    /// assert the counts the RFC itself states.
    #[test]
    fn corpus_is_complete() {
        assert_eq!(CASES.len(), 50, "archive holds 50 files");
        assert_eq!(classified().count(), 49, "49 are referenced by a section");

        let valid = CASES
            .iter()
            .filter(|c| c.section.starts_with("3.1.1"))
            .count();
        let invalid = CASES
            .iter()
            .filter(|c| c.section.starts_with("3.1.2"))
            .count();
        assert_eq!(valid, 13, "RFC 4475 3.1.1 defines 13 valid messages");
        assert_eq!(invalid, 19, "RFC 4475 3.1.2 defines 19 invalid messages");
        assert_eq!(
            CASES
                .iter()
                .filter(|c| c.section.starts_with("3.3"))
                .count(),
            15,
            "RFC 4475 3.3 defines 15 application-layer messages"
        );
    }

    #[test]
    fn case_names_and_sections_are_unique() {
        let names: HashSet<_> = CASES.iter().map(|c| c.name).collect();
        assert_eq!(names.len(), CASES.len(), "duplicate case name");

        let sections: HashSet<_> = classified().map(|c| c.section).collect();
        assert_eq!(sections.len(), 49, "duplicate section reference");
    }

    /// The table is hand-written; the directory is generated by the import script. If they
    /// drift, the table is silently ignoring a message.
    #[test]
    fn table_matches_the_imported_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/rfc4475");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("corpus directory")
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".dat").map(str::to_owned)
            })
            .collect();
        on_disk.sort();

        let mut in_table: Vec<String> = CASES.iter().map(|c| c.name.to_owned()).collect();
        in_table.sort();

        assert_eq!(
            on_disk, in_table,
            "corpus directory and case table disagree"
        );
    }

    #[test]
    fn every_case_has_content() {
        for c in CASES {
            assert!(!c.bytes.is_empty(), "{} is empty", c.name);
            assert!(
                c.bytes.windows(2).any(|w| w == b"\r\n"),
                "{} has no CRLF; the import may have mangled line endings",
                c.name
            );
        }
    }
}
