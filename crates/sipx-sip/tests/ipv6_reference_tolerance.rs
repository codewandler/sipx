//! The exact boundary of RFC 5118 §4.10's tolerance, held mechanically.
//!
//! `docs/specs/sip-parser.md` §4.8 says sipx accepts one construct RFC 4291 §2.2 forbids —
//! `:::` immediately before an embedded IPv4 address, the one shape the `IPv6address` production
//! RFC 3261 §25.1 inherited from the obsoleted RFC 2373 can derive — and nothing else. That is a
//! claim about a *language*, so it is tested as one:
//!
//! - `three_colon_table_rows_parse_exactly_as_the_spec_says` pins every row of §4.8's table to the
//!   exact address or the exact `UriError` variant, so the table cannot drift from the parser.
//! - `the_tolerance_admits_nothing_but_the_rfc2373_derivation` enumerates a grid of references and
//!   asserts the property over all of them: every input sipx accepts and `std` rejects has the
//!   shape `hexseq ":::" IPv4address`, and every input sipx rejects, `std` rejects too.
//!
//! The second is here rather than in `fuzz/` on purpose. `fuzz/fuzz_targets/parse_uri.rs` already
//! hunts crashes on arbitrary URI bytes, which is the question fuzzing answers well. The question
//! here is the opposite kind: not "does anything panic" but "is the accepted set exactly this set",
//! which is decidable over a fixed grid, cheap to run, and only useful if it runs on every commit.
//! A property that is checked once in review is a property that rots — which is the whole reason
//! §4.10 needed a story at all.

// A test that cannot read its own fixtures should fail loudly — AGENTS.md non-negotiable 3.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use bytes::Bytes;
use sipx_sip::error::UriError;
use sipx_sip::uri::Host;

/// Parse a `host [ ":" port ]` the way a Request-URI or a `Via` sent-by does.
fn hostport(text: &str) -> Result<Host, UriError> {
    Host::parse_hostport(&Bytes::from(text.to_owned())).map(|(host, _port)| host)
}

/// The address a `host` resolved to, or `None` if it parsed as a name rather than a literal.
fn address(text: &str) -> Result<Option<IpAddr>, UriError> {
    hostport(text).map(|host| match host {
        Host::Ip(ip) => Some(ip),
        Host::Name(_) => None,
    })
}

fn literal(text: &str) -> IpAddr {
    IpAddr::V6(text.parse::<Ipv6Addr>().expect("a valid RFC 4291 address"))
}

/// Every row of `docs/specs/sip-parser.md` §4.8's table, transcribed and pinned.
///
/// The spec states nine checkable outcomes. Before this test existed the suite asserted only that
/// the rejections *were* rejections, and the table's claim about the unbracketed form named the
/// wrong variant for two releases' worth of nothing noticing. `UriError::Port` is not a detail:
/// the transaction layer chooses a response code from the variant, and the spec is where that
/// mapping is read from.
#[test]
fn three_colon_table_rows_parse_exactly_as_the_spec_says() {
    // Accepts: the bracketed reference, and the address it must name.
    for (input, expected) in [
        // RFC 4291, the correct construct — unchanged by the carve-out.
        ("[2001:db8::192.0.2.1]", "2001:db8::192.0.2.1"),
        // The carve-out: `hexpart = hexseq "::"`, then `":" IPv4address`.
        ("[2001:db8:::192.0.2.1]", "2001:db8::192.0.2.1"),
        // The same production at full width — five groups before the `::`.
        ("[1:2:3:4:5:::192.0.2.1]", "1:2:3:4:5::192.0.2.1"),
        // The other production RFC 2373 offers: `hexpart = "::"`, empty `hexseq`.
        ("[:::192.0.2.1]", "::192.0.2.1"),
    ] {
        assert_eq!(
            address(input),
            Ok(Some(literal(expected))),
            "§4.8 says {input} is {expected}"
        );
    }

    // Rejections, each with the variant the spec names.
    for (input, expected) in [
        // Bracketed, but not the derivation.
        ("[2001:db8:::10]", UriError::Host),
        ("[2001:db8::::192.0.2.1]", UriError::Host),
        ("[2001:db8::1:::192.0.2.1]", UriError::Host),
        // Unbracketed, and the point of the pair: both fail at the *port* rule, because an
        // unbracketed host is split at its first `:` and never reaches an address parser. The
        // second is a perfectly valid RFC 4291 address and fails identically, which is what makes
        // this a statement about RFC 3261 §19.1.1's brackets rather than about the carve-out.
        ("2001:db8:::192.0.2.1", UriError::Port),
        ("2001:db8::192.0.2.1", UriError::Port),
    ] {
        assert_eq!(
            hostport(input).err(),
            Some(expected.clone()),
            "§4.8 says {input} is {expected:?}"
        );
    }
}

/// Heads: candidate `hexpart` prefixes, valid and not.
const HEADS: &[&str] = &[
    "",
    ":",
    "::",
    "0",
    "2001",
    "12345",
    "g",
    "abcd:ef01",
    "2001:db8",
    "2001:db8:",
    "2001:db8::",
    "fe80",
    "fe80::",
    "::1",
    "1::",
    "0:0:0:0:0:0",
    "1:2:3:4:5",
    "1:2:3:4:5:6",
    "1:2:3:4:5:6:7",
    "1:2:3:4:5:6:7:8",
    "1:2:3:4:5:6:7:8:9",
];

/// Separators: one colon through six.
const SEPARATORS: &[&str] = &[":", "::", ":::", "::::", ":::::", "::::::"];

/// Tails: embedded IPv4 addresses, near-misses, and things that are not addresses at all.
const TAILS: &[&str] = &[
    "",
    "192.0.2.1",
    "0.0.0.0",
    "255.255.255.255",
    "256.0.0.1",
    "192.0.2",
    "192.0.2.1.5",
    "0192.0.2.1",
    "192.0.2.01",
    "10",
    "ffff",
    "1:2",
    "192.0.2.1:5060",
    "%eth0",
];

/// Overlapping count, so `::::` counts two `:::` and not one.
fn three_colon_runs(text: &str) -> usize {
    text.as_bytes().windows(3).filter(|w| *w == b":::").count()
}

/// Assert that `inner` — accepted by sipx and rejected by RFC 4291 — is RFC 2373's derivation.
///
/// `IPv6address = hexpart [ ":" IPv4address ]` with `hexpart` ending in `"::"`. Every clause below
/// is one conjunct of that: one `:::` and no more, an `IPv4address` filling the tail, and a head
/// that is a plain `hexseq` — because RFC 2373's `hexpart` supplies the `"::"` itself, so a head
/// carrying its own `::` is a different (and non-)derivation. The address must finally agree with
/// the two-colon form, which is the statement that the extra colon was *tolerated* rather than
/// absorbed into some other group.
fn assert_is_the_rfc2373_derivation(reference: &str, inner: &str, got: Ipv6Addr) {
    let runs = three_colon_runs(inner);
    assert_eq!(
        runs, 1,
        "{reference}: accepted beyond RFC 4291 with {runs} ':::' runs; the derivation \
         produces exactly one"
    );
    assert!(
        !inner.contains("::::"),
        "{reference}: accepted beyond RFC 4291 with four colons, which no RFC 2373 \
         derivation produces"
    );

    let (hexseq, embedded) = inner
        .split_once(":::")
        .expect("just asserted there is exactly one ':::'");
    assert!(
        embedded.parse::<Ipv4Addr>().is_ok(),
        "{reference}: accepted beyond RFC 4291 with {embedded:?} after the ':::'; only an \
         embedded IPv4address is tolerated"
    );
    assert!(
        !hexseq.contains("::"),
        "{reference}: accepted beyond RFC 4291 with {hexseq:?} before the ':::'; RFC 2373's \
         hexpart supplies the '::' itself, so the head must be a plain hexseq"
    );

    let rewritten = format!("{hexseq}::{embedded}");
    let want = rewritten.parse::<Ipv6Addr>().unwrap_or_else(|e| {
        panic!(
            "{reference}: accepted beyond RFC 4291, but its two-colon form {rewritten:?} is \
             not an address either: {e}"
        )
    });
    assert_eq!(
        got, want,
        "{reference}: must mean exactly what its two-colon twin means"
    );
}

/// The narrowness, enumerated.
///
/// For every reference in the grid, sipx's answer is compared with `std`'s RFC 4291 parser and one
/// of three things must hold:
///
/// 1. **sipx accepts, `std` accepts** — the addresses are identical. The carve-out must not change
///    the meaning of anything that already parsed.
/// 2. **sipx accepts, `std` rejects** — the input must be the RFC 2373 derivation and nothing else:
///    exactly one `:::`, no `::::`, an `IPv4address` after the `:::` running to the end, a genuine
///    `hexseq` before it (no `::` of its own), and a two-colon rewrite `std` accepts.
/// 3. **sipx rejects** — `std` rejects too, so the accepted set is a strict superset of RFC 4291
///    and the carve-out has taken nothing away.
#[test]
fn the_tolerance_admits_nothing_but_the_rfc2373_derivation() {
    let mut examined = 0_usize;
    let mut distinct = BTreeSet::new();
    let mut plain = 0_usize;
    let mut carve_outs = BTreeSet::new();

    for head in HEADS {
        for separator in SEPARATORS {
            for tail in TAILS {
                let inner = format!("{head}{separator}{tail}");
                let reference = format!("[{inner}]");
                examined += 1;
                distinct.insert(inner.clone());

                let std_says = inner.parse::<Ipv6Addr>().ok();
                let sipx_says = match address(&reference) {
                    Ok(Some(IpAddr::V6(ip))) => Some(ip),
                    Ok(other) => panic!(
                        "{reference}: a bracketed reference is an IPv6 literal, got {other:?}"
                    ),
                    Err(_) => None,
                };

                match (sipx_says, std_says) {
                    (Some(got), Some(want)) => {
                        plain += 1;
                        assert_eq!(
                            got, want,
                            "{reference}: already parsed under RFC 4291; the carve-out must not \
                             change what it means"
                        );
                    }
                    (Some(got), None) => {
                        carve_outs.insert(inner.clone());
                        assert_is_the_rfc2373_derivation(&reference, &inner, got);
                    }
                    (None, Some(want)) => panic!(
                        "{reference}: rejected, but RFC 4291 says it is {want} — the carve-out \
                         must only ever add to the accepted set"
                    ),
                    (None, None) => {}
                }
            }
        }
    }

    assert_eq!(
        examined,
        HEADS.len() * SEPARATORS.len() * TAILS.len(),
        "every combination is examined"
    );
    assert_eq!(examined, 1764, "21 heads x 6 separators x 14 tails");
    // Fewer distinct strings than combinations, because some (head, separator) pairs spell the same
    // reference — `"" + ":::"` and `":" + "::"` are both `:::`. Harmless for the property, but the
    // grid should not be described as 1764 different references.
    assert_eq!(
        distinct.len(),
        1428,
        "1428 of those are distinct references"
    );

    // Pin the two populations. Without these the test would still pass if the parser started
    // rejecting everything, or if the carve-out silently stopped firing — a property test that
    // never sees its interesting case is a test that passes for the wrong reason. Both counts are
    // measured over this grid, not derived from anything: change the grid and they change.
    assert_eq!(
        plain, 86,
        "the grid holds 86 combinations RFC 4291 already accepted, and the carve-out changed the \
         meaning of none of them"
    );

    // The whole accepted-beyond-RFC-4291 set, enumerated. This is the narrowness written out: 13
    // references, every one of them `hexseq ":::" IPv4address`. A diff here is a diff in what sipx
    // treats as an address on unauthenticated input, and it should be impossible to land quietly.
    let expected: BTreeSet<String> = [
        ":::0.0.0.0",
        ":::192.0.2.1",
        ":::255.255.255.255",
        "0:::0.0.0.0",
        "0:::192.0.2.1",
        "0:::255.255.255.255",
        "1:::0.0.0.0",
        "1:::192.0.2.1",
        "1:::255.255.255.255",
        "1:2:3:4:5:::0.0.0.0",
        "1:2:3:4:5:::192.0.2.1",
        "1:2:3:4:5:::255.255.255.255",
        "2001:::0.0.0.0",
        "2001:::192.0.2.1",
        "2001:::255.255.255.255",
        "2001:db8:::0.0.0.0",
        "2001:db8:::192.0.2.1",
        "2001:db8:::255.255.255.255",
        "abcd:ef01:::0.0.0.0",
        "abcd:ef01:::192.0.2.1",
        "abcd:ef01:::255.255.255.255",
        "fe80:::0.0.0.0",
        "fe80:::192.0.2.1",
        "fe80:::255.255.255.255",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(
        carve_outs, expected,
        "the set sipx accepts beyond RFC 4291 must be exactly RFC 2373's one derivation"
    );
}
