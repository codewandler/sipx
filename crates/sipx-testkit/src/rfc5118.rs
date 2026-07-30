//! The RFC 5118 IPv6 SIP torture-test corpus.
//!
//! RFC 5118 is the IPv6 twin of RFC 4475: ten sections of messages built to break parsers that
//! guess at where an IPv6 reference ends. Almost all of them are *valid*, which is the point —
//! the document exists because implementations were rejecting messages they were obliged to
//! accept, and because a colon means two different things inside `[...]` and after it.
//!
//! The messages are recovered from the bit-exact archive in that RFC's Appendix A by
//! `scripts/import-rfc5118-corpus.sh`, not retyped. Retyping is a worse idea here than for
//! RFC 4475: every case turns on the exact placement of `:`, `[` and `]`, and two of the
//! messages are wrapped across lines in the RFC's body text with an `<allOneLine>` convention
//! that a transcriber has to unwrap by hand. Run the script with `--check` to verify the
//! committed corpus still matches the RFC.
//!
//! # The archive is not wire bytes
//!
//! One difference from RFC 4475 has to be handled rather than admired. The files in RFC 5118's
//! archive are terminated with **bare LF**, not CRLF — there is not one CR octet in any of the
//! twelve — and the two §4.10 files carry no terminating blank line at all. SIP requires CRLF
//! (RFC 3261 §7), so the archived bytes are not a legal SIP message as shipped.
//!
//! The corpus on disk is kept bit-exact anyway, because that is what `--check` verifies against
//! the RFC. [`Case::wire`] performs the one documented transformation needed to get on-the-wire
//! bytes, and [`Case::bytes`] remains the archive's own content. The transformation touches
//! only line terminators; every octet of every IPv6 reference is the RFC's.
//!
//! # What the classification is for
//!
//! Each case carries an [`Expect`] naming which layer must object to it, and the tests assert
//! against that rather than a bare pass/fail. The vocabulary is [`crate::rfc4475`]'s, imported
//! rather than redefined: a reader comparing the two corpora is then comparing like with like,
//! and there is exactly one definition of what `ParseOk` claims.
//!
//! Unlike RFC 4475, this corpus is almost entirely `ParseOk`. Only §4.2 is invalid, and the RFC
//! says so in its title. The other nine sections are demonstrations that a parser must *accept*
//! things it may not expect, so the converse assertion — that nothing valid is rejected — is
//! where the value of this corpus lies.

use bytes::Bytes;

pub use crate::rfc4475::{Expect, Fault};

/// One message from the corpus.
#[derive(Debug, Clone, Copy)]
pub struct Case {
    /// The RFC's own name for the message, e.g. `ipv6-good`. RFC 5118 labels each message with
    /// this name ("Message Details: ipv6-good"), and the archive's file names match, so the
    /// name is the link from a fixture back to the prose that describes it.
    pub name: &'static str,
    /// The section of RFC 5118 that describes it, e.g. `4.1`. Two sections describe two
    /// messages each, so this is not unique across cases.
    pub section: &'static str,
    /// That section's title, verbatim.
    pub title: &'static str,
    /// Which layer must object, and how.
    pub expect: Expect,
    /// The message exactly as Appendix A's archive holds it — LF-terminated, and for the §4.10
    /// pair without a terminating blank line. Use [`Case::wire`] to get bytes a SIP parser is
    /// meant to see.
    pub bytes: &'static [u8],
}

impl Case {
    /// The archive bytes turned into on-the-wire SIP.
    ///
    /// Two transformations, both forced by the archive rather than chosen:
    ///
    /// 1. **LF becomes CRLF.** RFC 3261 §7 terminates every start line and header field with
    ///    CRLF. The archive holds none, so without this every message is one unterminated
    ///    header line and the corpus would measure nothing but that fact.
    /// 2. **The header section is terminated** if the archive left it open, which it does for
    ///    §4.10's two files. An unterminated header section is indistinguishable from a
    ///    truncated message, so a parser is right to refuse it — see how RFC 4475's `baddn` is
    ///    classified in [`crate::rfc4475`]. Refusing it here would test the archive's
    ///    formatting, not sipx's IPv6 handling.
    ///
    /// Neither touches an octet inside an IPv6 reference, a URI, or a body. What the corpus is
    /// for survives intact.
    #[must_use]
    pub fn wire(&self) -> Bytes {
        let mut out = Vec::with_capacity(self.bytes.len() + self.bytes.len() / 8 + 2);
        for &b in self.bytes {
            if b == b'\n' && out.last() != Some(&b'\r') {
                out.push(b'\r');
            }
            out.push(b);
        }
        if !out.windows(4).any(|w| w == b"\r\n\r\n") {
            out.extend_from_slice(b"\r\n");
        }
        Bytes::from(out)
    }

    /// The message as a lossy string, for assertion messages.
    #[must_use]
    pub fn lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(self.bytes)
    }

    /// Whether this case is asserted on by the parser tests. Every RFC 5118 case is: unlike
    /// RFC 4475's archive, this one carries no file that no section references.
    #[must_use]
    pub fn is_classified(&self) -> bool {
        self.expect != Expect::Unreferenced
    }

    /// Whether the message carries an SDP body, and so belongs to the `sipx-sdp` half of the
    /// harness as well as the `sipx-sip` half.
    #[must_use]
    pub fn has_sdp(&self) -> bool {
        matches!(self.name, "ipv6-in-sdp" | "mult-ip-in-sdp" | "ipv4-mapped-ipv6")
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
                bytes: include_bytes!(concat!("../corpus/rfc5118/", $name)),
            },
        )*];
    };
}

use Expect::{ParseErr, ParseOk};
use Fault::StartLine;

corpus! {
    // ---- 4.1 ------------------------------------------------------------------------
    // An IPv6 reference in the R-URI, the Via and the Contact, all correctly delimited.
    // "well-formatted according to the grammar in [RFC3261]".
    "ipv6-good"  => "4.1", "Valid SIP Message with an IPv6 Reference", ParseOk;

    // ---- 4.2 ------------------------------------------------------------------------
    // The only invalid message in the corpus, and the RFC's title says so. The R-URI is
    // `sip:2001:db8::10` — an IPv6 address with the mandated "[" "]" stripped off. The RFC:
    // "A SIP implementation receiving this request should respond with a 400 Bad Request".
    //
    // Classified as a start-line fault rather than a header one because that is where the
    // undelimited reference is: the Request-URI. The same treatment RFC 4475 gives `lwsruri`.
    "ipv6-bad"   => "4.2", "Invalid SIP Message with an IPv6 Reference", ParseErr(StartLine);

    // ---- 4.3 ------------------------------------------------------------------------
    // `sip:[2001:db8::10:5070]` — the sender meant port 5070 and put it inside the "]". The RFC
    // is explicit that this is not a parse error: "From a parsing perspective, the request below
    // is well-formed. However, from a semantic point of view, it will not yield the desired
    // result." So the parser must accept it, and what it decides the host and port *are* is a
    // choice this corpus exists to pin down. See the harness test
    // `port_ambiguous_is_decided_the_way_the_rfc_predicts`.
    "port-ambiguous"   => "4.3", "Port Ambiguous in a SIP URI", ParseOk;

    // ---- 4.4 ------------------------------------------------------------------------
    // The contrast to 4.3: `sip:[2001:db8::10]:5070`, where the port is outside the "]".
    "port-unambiguous" => "4.4", "Port Unambiguous in a SIP URI", ParseOk;

    // ---- 4.5 ------------------------------------------------------------------------
    // Two messages for one section, and the pair is the test. RFC 3261's `via-received`
    // production takes a bare `IPv6address`, with no "[" "]" — while `sent-by` takes an
    // `IPv6reference`, which has them. Implementations split roughly 50/50 on what they sent,
    // so the RFC's instruction is the Robustness Principle: "implementations must follow the
    // Robustness Principle [RFC1122] and be liberal in accepting a 'received' parameter with or
    // without the delimiting '[' and ']' tokens", and "A SIP implementation receiving either of
    // these messages must parse them successfully."
    //
    // So `with-delim` is ParseOk *despite* being invalid under a strict reading of the grammar.
    // That is not sloppiness in the classification; it is what the RFC requires, and it is the
    // one place in either corpus where "must accept" and "matches the ABNF" come apart.
    "via-received-param-with-delim" => "4.5", "IPv6 Reference Delimiters in Via Header", ParseOk;
    "via-received-param-no-delim"   => "4.5", "IPv6 Reference Delimiters in Via Header", ParseOk;

    // ---- 4.6 ------------------------------------------------------------------------
    // "valid and well-formed". Carries SDP whose `o=` and `c=` lines hold IPv6 addresses
    // *without* "[" "]" — SDP has its own grammar (RFC 4566/8866) and never adopted the
    // brackets. A stack that reuses its SIP host parser for `c=` lines fails here.
    "ipv6-in-sdp" => "4.6", "SIP Request with IPv6 Addresses in Session Description Protocol (SDP) Body", ParseOk;

    // ---- 4.7 ------------------------------------------------------------------------
    // Three Via headers mixing IPv4 and IPv6, one with a port inside the reference's "]" and
    // one with a `received` IPv4 parameter.
    "mult-ip-in-header" => "4.7", "Multiple IP Addresses in SIP Headers", ParseOk;

    // ---- 4.8 ------------------------------------------------------------------------
    // Per-media `c=` lines, one IPv4 and one IPv6, overriding an `o=` line that names a
    // hostname rather than an address. The session has no session-level `c=` at all.
    "mult-ip-in-sdp" => "4.8", "Multiple IP Addresses in SDP", ParseOk;

    // ---- 4.9 ------------------------------------------------------------------------
    // IPv4-mapped addresses (`::ffff:192.0.2.2`) in two Vias, a Contact, and the SDP. "A SIP
    // implementation receiving a message that contains such a mapped address must be prepared
    // to parse it successfully."
    "ipv4-mapped-ipv6" => "4.9", "IPv4-Mapped IPv6 Addresses", ParseOk;

    // ---- 4.10 -----------------------------------------------------------------------
    // Another contrast pair. RFC 3261's ABNF, inherited from the obsolete RFC 2373, permits
    // `[2001:db8:::192.0.2.1]` — three colons before the embedded IPv4 address. RFC 4291
    // fixed the grammar; RFC 5118's instruction is to tolerate both: "following the Robustness
    // Principle [RFC1122], an implementation must tolerate both of the above constructs."
    //
    // The RFC permits, but does not require, re-serializing the three-colon form as two. Which
    // sipx does is asserted in `abnf_bug_reference_is_tolerated`.
    "ipv6-bug-abnf-3-colons"     => "4.10", "IPv6 Reference Bug in RFC 3261 ABNF", ParseOk;
    "ipv6-correct-abnf-2-colons" => "4.10", "IPv6 Reference Bug in RFC 3261 ABNF", ParseOk;
}

/// Every case the parser tests assert on.
pub fn classified() -> impl Iterator<Item = &'static Case> {
    CASES.iter().filter(|c| c.is_classified())
}

/// Cases matching a given expectation.
pub fn expecting(expect: Expect) -> impl Iterator<Item = &'static Case> {
    CASES.iter().filter(move |c| c.expect == expect)
}

/// Cases carrying an SDP body.
pub fn with_sdp() -> impl Iterator<Item = &'static Case> {
    CASES.iter().filter(|c| c.has_sdp())
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

    /// The corpus is only a correctness bar if it is complete. RFC 5118 §4 runs 4.1 to 4.10,
    /// and two of those sections carry a contrast pair — drop either half and the section stops
    /// testing the thing it was written to test.
    #[test]
    fn corpus_is_complete() {
        assert_eq!(CASES.len(), 12, "Appendix A's archive holds 12 files");
        assert_eq!(classified().count(), 12, "every file is referenced by a section");

        let sections: HashSet<_> = CASES.iter().map(|c| c.section).collect();
        assert_eq!(sections.len(), 10, "RFC 5118 section 4 has ten subsections");

        for n in 1..=10 {
            let section = if n == 10 { "4.10".to_owned() } else { format!("4.{n}") };
            assert!(
                CASES.iter().any(|c| c.section == section),
                "no case for RFC 5118 section {section}"
            );
        }

        // The two contrast pairs, named so a reader knows the duplication is deliberate.
        for section in ["4.5", "4.10"] {
            assert_eq!(
                CASES.iter().filter(|c| c.section == section).count(),
                2,
                "section {section} contrasts two messages"
            );
        }
    }

    /// Only §4.2 is invalid, and this corpus is worth running because of that imbalance rather
    /// than in spite of it. If a later edit quietly reclassified a valid message as a rejection,
    /// the corpus would start asserting the opposite of what the RFC says while staying green.
    #[test]
    fn only_section_4_2_is_a_rejection() {
        let rejected: Vec<_> = CASES
            .iter()
            .filter(|c| matches!(c.expect, Expect::ParseErr(_)))
            .map(|c| c.name)
            .collect();
        assert_eq!(
            rejected,
            vec!["ipv6-bad"],
            "RFC 5118 titles exactly one message invalid (§4.2)"
        );
        assert_eq!(
            expecting(ParseOk).count(),
            11,
            "the other eleven are demonstrations a parser must accept"
        );
    }

    #[test]
    fn case_names_are_unique() {
        let names: HashSet<_> = CASES.iter().map(|c| c.name).collect();
        assert_eq!(names.len(), CASES.len(), "duplicate case name");
    }

    /// The table is hand-written; the directory is generated by the import script. If they
    /// drift, the table is silently ignoring a message.
    #[test]
    fn table_matches_the_imported_directory() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus/rfc5118");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .expect("corpus directory")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "README.md")
            .collect();
        on_disk.sort();

        let mut in_table: Vec<String> = CASES.iter().map(|c| c.name.to_owned()).collect();
        in_table.sort();

        assert_eq!(on_disk, in_table, "corpus directory and case table disagree");
    }

    /// The archive's own shape, asserted so the [`Case::wire`] transformation stays justified.
    /// If a future re-import brought CRLF files, `wire` would become a no-op and this test
    /// would say so rather than leaving a transformation nobody could explain.
    #[test]
    fn the_archive_is_lf_terminated_which_is_why_wire_exists() {
        for c in CASES {
            assert!(!c.bytes.is_empty(), "{} is empty", c.name);
            assert!(
                !c.bytes.contains(&b'\r'),
                "{} carries a CR; RFC 5118's archive has none, so `wire` needs revisiting",
                c.name
            );
        }
    }

    /// `wire` has to produce something a SIP parser can be asked about: every line terminated
    /// with CRLF, and a terminated header section.
    #[test]
    fn wire_terminates_every_line_and_the_header_section() {
        for c in CASES {
            let wire = c.wire();
            assert_eq!(
                wire.iter().filter(|&&b| b == b'\n').count(),
                c.bytes.iter().filter(|&&b| b == b'\n').count()
                    + usize::from(!c.bytes.windows(2).any(|w| w == b"\n\n")),
                "{}: wire must not invent or lose lines",
                c.name
            );
            // No bare LF survives: every LF is preceded by CR.
            for (i, &b) in wire.iter().enumerate() {
                if b == b'\n' {
                    assert_eq!(
                        i.checked_sub(1).and_then(|j| wire.get(j)),
                        Some(&b'\r'),
                        "{}: bare LF at offset {i} in the wire form",
                        c.name
                    );
                }
            }
            assert!(
                wire.windows(4).any(|w| w == b"\r\n\r\n"),
                "{}: wire must terminate the header section",
                c.name
            );
        }
    }

    /// The transformation must not touch anything the corpus is *for*. Strip line terminators
    /// from both forms and they must be identical — which is exactly the claim that no IPv6
    /// reference, URI or body octet was altered.
    #[test]
    fn wire_changes_only_line_terminators() {
        for c in CASES {
            let strip = |b: &[u8]| -> Vec<u8> {
                b.iter().copied().filter(|&b| b != b'\r' && b != b'\n').collect()
            };
            assert_eq!(
                strip(&c.wire()),
                strip(c.bytes),
                "{}: wire altered more than line terminators",
                c.name
            );
        }
    }
}
