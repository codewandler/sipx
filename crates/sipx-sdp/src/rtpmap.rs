//! `a=rtpmap` values: the format one names, and whether two of them name the same format.
//!
//! **This module is the single authority for RFC 8866 §6.6 format identity.** The question it
//! answers used to be answered twice. [`mod@crate::answer`] asked it to decide which offered
//! formats go into the answer, `sipx-call` asked it to decide which codec to build the media
//! session with, and the two disagreed: one compared the clock rate as text where the other
//! parsed it to a number. `08000` and `8000` are numerically equal and textually different, so an
//! offer spelling the rate that way settled on µ-law while the answer named only A-law. sipx then
//! sent on a payload type the answer never offered, and decoded the peer's A-law through a µ-law
//! session — audible garbage rather than silence, with nothing in the stack reporting an error
//! (`M-31`).
//!
//! The rule lives *here*, in the lower crate, because the dependency only runs one way:
//! `sipx-call` can call down, and [`mod@crate::answer`] cannot call up. Nothing that belongs to the
//! layer above comes with it — this module knows the grammar and what makes two values equal, and
//! has no concept of a codec set or of which format to prefer. Choosing among the values that
//! match stays where it belongs, above.
//!
//! `docs/specs/sdp-format-identity.md` is normative.

/// Why an `a=rtpmap` value names no format.
///
/// A value that names nothing is not an error the stack reports to anyone: both callers turn it
/// into a non-match, which is the conservative reading of hostile input — a format sipx cannot
/// identify is a format sipx does not agree to. The variants exist so the reason is available to
/// a caller that wants to log it, and so a malformed clock rate is a typed outcome rather than a
/// panic ([AGENTS.md](../../../AGENTS.md) non-negotiable 3).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RtpmapError {
    /// Nothing before the first `/`.
    #[error("no encoding name")]
    MissingEncoding,
    /// No `/` at all. RFC 8866 §6.6 makes the clock rate part of the format's identity, so a
    /// value without one identifies nothing.
    #[error("no clock rate")]
    MissingClockRate,
    /// The clock rate is not a decimal number that fits in 32 bits.
    #[error("clock rate is not a decimal number: {0}")]
    ClockRate(String),
    /// The encoding parameter is not a decimal number that fits in 32 bits. A value carrying more
    /// fields than the grammar has arrives here, because the extra `/` is not a digit.
    #[error("encoding parameter is not a decimal number: {0}")]
    EncodingParameter(String),
}

/// The format an `a=rtpmap` value names.
///
/// `<encoding name>/<clock rate>[/<encoding parameters>]` (RFC 8866 §6.6). For audio the encoding
/// parameter is the channel count, and an omitted one means one channel.
///
/// **Deliberately not `PartialEq`.** Derived equality would compare the encoding name as bytes,
/// and the name compares case-insensitively — so `==` would be a second, wrong answer to the
/// question this module exists to answer once. [`Rtpmap::same_format_as`] is the only comparison.
#[derive(Debug, Clone, Copy)]
pub struct Rtpmap<'a> {
    encoding: &'a str,
    clock_rate: u32,
    channels: u32,
}

impl<'a> Rtpmap<'a> {
    /// Read the format a value names.
    ///
    /// The value is the part of the attribute after the payload type — `PCMU/8000`, not
    /// `0 PCMU/8000` — which is what [`crate::MediaDescription::rtpmap`] returns.
    ///
    /// # Errors
    ///
    /// [`RtpmapError`] when the value is not RFC 8866 §6.6's grammar. This is a parser for data
    /// that arrived from the network, so every rejection is a returned error: an empty rate, a
    /// rate that is not digits, a rate too large for a `u32`, and a value carrying a field the
    /// grammar does not have.
    pub fn parse(value: &'a str) -> Result<Self, RtpmapError> {
        let (encoding, rest) = value.split_once('/').ok_or(RtpmapError::MissingClockRate)?;
        if encoding.is_empty() {
            return Err(RtpmapError::MissingEncoding);
        }

        // Everything after the second `/` stays with the encoding parameter on purpose. A value
        // with a fourth field is outside the grammar, and letting it through as "the parameter,
        // plus some text nobody read" is how a stack agrees to a format it did not understand.
        let (clock_rate, parameter) = match rest.split_once('/') {
            Some((clock_rate, parameter)) => (clock_rate, Some(parameter)),
            None => (rest, None),
        };

        Ok(Self {
            encoding,
            clock_rate: decimal(clock_rate)
                .map_err(|field| RtpmapError::ClockRate(field.to_owned()))?,
            channels: match parameter {
                // RFC 8866 §6.6: an omitted encoding parameter means one channel.
                None => 1,
                Some(parameter) => decimal(parameter)
                    .map_err(|field| RtpmapError::EncodingParameter(field.to_owned()))?,
            },
        })
    }

    /// The encoding name, as the description spelled it.
    #[must_use]
    pub fn encoding(&self) -> &'a str {
        self.encoding
    }

    /// The RTP clock rate, which for several codecs is not the sample rate.
    #[must_use]
    pub fn clock_rate(&self) -> u32 {
        self.clock_rate
    }

    /// The channel count, with RFC 8866 §6.6's default of one already applied.
    #[must_use]
    pub fn channels(&self) -> u32 {
        self.channels
    }

    /// Whether two values name the same format.
    ///
    /// RFC 8866 §6.6: the encoding name compares case-insensitively, and the clock rate and
    /// channel count are part of the format's identity — the same codec at two rates is two
    /// formats.
    ///
    /// The rate and the count compare **by value**, because they are numbers and the identity of a
    /// number is numeric. Comparing them as text answers a different question — whether they are
    /// *spelled* the same — and answering that one by accident is exactly the defect `M-31` fixed.
    #[must_use]
    pub fn same_format_as(&self, other: &Rtpmap<'_>) -> bool {
        self.encoding.eq_ignore_ascii_case(other.encoding)
            && self.clock_rate == other.clock_rate
            && self.channels == other.channels
    }
}

/// Whether two `a=rtpmap` values name the same format, reading both.
///
/// The predicate both callers use, so neither has a rule of its own to drift from the other's.
///
/// `false` when either value names no format at all. A value that identifies nothing matches
/// nothing — including another value that identifies nothing, since `PCMU` and `G729` are not the
/// same format merely because neither carries a clock rate.
#[must_use]
pub fn same_format(offered: &str, local: &str) -> bool {
    match (Rtpmap::parse(offered), Rtpmap::parse(local)) {
        (Ok(offered), Ok(local)) => offered.same_format_as(&local),
        _ => false,
    }
}

/// A decimal digit string, by value.
///
/// Strict about the spelling in every way but one. `u32::from_str` on its own would accept `+8000`
/// while rejecting ` 8000` and `8_000`, which is a different rule from any reader that looks at the
/// characters, so the digits are checked here rather than left to it: an empty field, a sign,
/// surrounding whitespace and a digit separator all name no rate.
///
/// **Leading zeros are tolerated on purpose.** RFC 8866 §9's `integer` rule starts at a non-zero
/// digit, so `08000` is strictly ungrammatical — but it is unambiguously eight thousand, it is
/// what a zero-padded field in somebody's config generator produces, and refusing it would decline
/// a format the peer plainly named. Tolerating it costs nothing precisely because there is now one
/// reader: the two rules cannot tolerate it differently.
///
/// Returns the offending field so the caller can name it in a typed error. A value too large for a
/// `u32` fails here rather than wrapping or panicking.
fn decimal(field: &str) -> Result<u32, &str> {
    if field.is_empty() || !field.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(field);
    }
    field.parse::<u32>().map_err(|_| field)
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

    /// §4.1's vectors: what the grammar admits, and what each field reads as.
    #[test]
    fn a_value_reads_as_its_three_fields() {
        let pcmu = Rtpmap::parse("PCMU/8000").expect("the grammar");
        assert_eq!(pcmu.encoding(), "PCMU");
        assert_eq!(pcmu.clock_rate(), 8_000);
        assert_eq!(pcmu.channels(), 1, "an omitted parameter is one channel");

        let opus = Rtpmap::parse("opus/48000/2").expect("the grammar");
        assert_eq!(opus.encoding(), "opus");
        assert_eq!(opus.clock_rate(), 48_000);
        assert_eq!(opus.channels(), 2);
    }

    /// The identity rule, field by field. A rate or a channel count that differs is a *different
    /// format*, not the same one spelled loosely — the same codec at two rates is two formats.
    #[test]
    fn identity_is_the_name_case_insensitively_and_the_numbers_by_value() {
        assert!(
            same_format("PCMU/8000", "pcmu/8000"),
            "the name is case-blind"
        );
        assert!(
            same_format("PCMU/8000", "PCMU/8000/1"),
            "one channel is the default"
        );
        assert!(!same_format("PCMU/8000", "PCMA/8000"), "a different name");
        assert!(!same_format("PCMU/16000", "PCMU/8000"), "a different rate");
        assert!(
            !same_format("PCMU/8000/2", "PCMU/8000"),
            "a different channel count"
        );
    }

    /// **The `M-31` class.** A spelling that is numerically equal and textually different is the
    /// same format, in either numeric field. These are the rows a text comparison got wrong.
    #[test]
    fn a_number_spelled_differently_is_the_same_number() {
        assert!(
            same_format("PCMU/08000", "PCMU/8000"),
            "a leading zero in the rate"
        );
        assert!(
            same_format("PCMU/8000/01", "PCMU/8000"),
            "a leading zero in the count"
        );
        assert!(
            same_format("PCMU/0008000/0001", "PCMU/8000/1"),
            "both, padded"
        );
        assert!(
            same_format("opus/048000/2", "opus/48000/2"),
            "the gated codec too"
        );
    }

    /// Hostile input from a peer is a typed error and a non-match, never a panic. §4.2's vectors.
    #[test]
    fn a_value_that_identifies_nothing_is_a_typed_error() {
        for (value, expected) in [
            ("PCMU", RtpmapError::MissingClockRate),
            ("", RtpmapError::MissingClockRate),
            ("/8000", RtpmapError::MissingEncoding),
            ("PCMU/", RtpmapError::ClockRate(String::new())),
            ("PCMU/+8000", RtpmapError::ClockRate("+8000".to_owned())),
            ("PCMU/-8000", RtpmapError::ClockRate("-8000".to_owned())),
            ("PCMU/ 8000", RtpmapError::ClockRate(" 8000".to_owned())),
            ("PCMU/8_000", RtpmapError::ClockRate("8_000".to_owned())),
            ("PCMU/eight", RtpmapError::ClockRate("eight".to_owned())),
            // Larger than u32::MAX. Parsed, not wrapped and not panicked on.
            (
                "PCMU/99999999999999",
                RtpmapError::ClockRate("99999999999999".to_owned()),
            ),
            ("PCMU/8000/", RtpmapError::EncodingParameter(String::new())),
            (
                "PCMU/8000/two",
                RtpmapError::EncodingParameter("two".to_owned()),
            ),
            // A fourth field is outside the grammar and stays with the parameter, so it is
            // rejected rather than silently ignored.
            (
                "PCMU/8000/1/9",
                RtpmapError::EncodingParameter("1/9".to_owned()),
            ),
        ] {
            assert_eq!(
                Rtpmap::parse(value).err(),
                Some(expected),
                "reading {value:?}"
            );
            assert!(
                !same_format(value, "PCMU/8000"),
                "{value:?} names no format, so it matches none"
            );
            assert!(
                !same_format("PCMU/8000", value),
                "{value:?} matches none from the other side either"
            );
        }
    }

    /// Two values that each name nothing are not thereby equal. Without this, an offer of `PCMU`
    /// with no rate would match a local `G729` with no rate.
    #[test]
    fn nothing_does_not_match_nothing() {
        assert!(!same_format("PCMU", "G729"));
        assert!(!same_format("PCMU", "PCMU"), "not even the identical value");
    }

    /// The largest rate the type holds is read, so the boundary is a value and not a panic.
    #[test]
    fn the_largest_representable_rate_reads() {
        let max = format!("X/{}", u32::MAX);
        assert_eq!(
            Rtpmap::parse(&max).expect("u32::MAX fits").clock_rate(),
            u32::MAX
        );

        let over = format!("X/{}", u64::from(u32::MAX) + 1);
        assert!(Rtpmap::parse(&over).is_err(), "one past the top is refused");
    }
}
