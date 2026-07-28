//! Property tests for URI parsing.
//!
//! The unit tests cover the cases we thought of. These cover the ones we did not: proptest
//! generates URIs from the grammar's own character classes and asserts the invariants that
//! must hold for every URI, not just the interesting ones.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use proptest::prelude::*;
use sipx_sip::Uri;

/// Characters legal in a user part (RFC 3261 §25.1 `user-unreserved` plus `unreserved`),
/// minus the escape character, which is generated separately so escapes stay well-formed.
fn user_part() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-zA-Z0-9\\-_.!~*'()&=+$,;?/]{1,20}").unwrap()
}

fn hostname() -> impl Strategy<Value = String> {
    proptest::string::string_regex(
        "[a-zA-Z0-9][a-zA-Z0-9\\-]{0,10}(\\.[a-zA-Z][a-zA-Z0-9\\-]{0,10}){0,3}",
    )
    .unwrap()
}

fn token() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[a-zA-Z0-9\\-_.!~*'+`%]{1,10}").unwrap()
}

prop_compose! {
    fn arbitrary_uri()(
        secure in any::<bool>(),
        user in proptest::option::of(user_part()),
        host in hostname(),
        port in proptest::option::of(1u16..=65535),
        params in proptest::collection::vec((token(), proptest::option::of(token())), 0..4),
        headers in proptest::collection::vec((token(), token()), 0..3),
    ) -> String {
        let mut s = String::from(if secure { "sips:" } else { "sip:" });
        if let Some(u) = user {
            s.push_str(&u);
            s.push('@');
        }
        s.push_str(&host);
        if let Some(p) = port {
            s.push(':');
            s.push_str(&p.to_string());
        }
        for (name, value) in params {
            s.push(';');
            s.push_str(&name);
            if let Some(v) = value {
                s.push('=');
                s.push_str(&v);
            }
        }
        for (i, (name, value)) in headers.iter().enumerate() {
            s.push(if i == 0 { '?' } else { '&' });
            s.push_str(name);
            s.push('=');
            s.push_str(value);
        }
        s
    }
}

proptest! {
    /// Parsing and serializing is a fixed point: a URI that parses is written back exactly,
    /// and re-parsing that output yields the same bytes again. This is the property the
    /// verbatim-passthrough guarantee rests on.
    #[test]
    fn parse_serialize_is_a_fixed_point(text in arbitrary_uri()) {
        let raw = Bytes::from(text.clone());
        let Ok(uri) = Uri::parse(raw.clone()) else {
            // A generated string the parser rejects is a legitimate outcome — the generator
            // is looser than the grammar. It must simply not be accepted-then-mangled.
            return Ok(());
        };
        prop_assert_eq!(uri.to_bytes(), raw.clone(), "first serialization must be verbatim");

        let again = Uri::parse(uri.to_bytes()).expect("a serialized URI must re-parse");
        prop_assert_eq!(again.to_bytes(), raw, "round trip must be a fixed point");
    }

    /// Whatever else equivalence does, a URI is equivalent to itself. Reflexivity is the one
    /// property the RFC's non-transitive relation still has to satisfy, and it exercises
    /// every branch of the comparison against real generated input.
    #[test]
    fn equivalence_is_reflexive(text in arbitrary_uri()) {
        let raw = Bytes::from(text);
        if let Ok(uri) = Uri::parse(raw) {
            let twin = Uri::parse(uri.to_bytes()).expect("a serialized URI must re-parse");
            prop_assert!(uri.equivalent(&twin));
            prop_assert!(twin.equivalent(&uri), "equivalence must be symmetric");
        }
    }

    /// No input may panic. The parser is reachable from the network.
    #[test]
    fn parsing_arbitrary_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        let _ = Uri::parse(Bytes::from(bytes));
    }

    /// Nor may serializing whatever came out of a parse.
    #[test]
    fn serializing_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..200)) {
        if let Ok(uri) = Uri::parse(Bytes::from(bytes)) {
            let _ = uri.to_bytes();
            let _ = uri.to_string();
            let _ = uri.decoded_user();
        }
    }
}
