//! Anything that parses must re-serialize to a prefix of its input, and that output must
//! parse again to the same thing.
//!
//! This is the strongest property the parser has, and the one a proxy depends on: if some
//! input parses to a message that serializes to *different* bytes, sipx has silently rewritten
//! a message it was asked to forward.

#![no_main]

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use sipx_sip::{Limits, Message, parse_datagram};

fuzz_target!(|data: &[u8]| {
    let limits = Limits::datagram();
    let Ok(message) = parse_datagram(Bytes::copy_from_slice(data), &limits) else {
        return;
    };

    let out = message.to_bytes();
    assert!(
        data.starts_with(&out),
        "a parsed message must serialize to a prefix of its input"
    );

    let reparsed = parse_datagram(out.clone(), &limits)
        .expect("output of a successful parse must itself parse");
    assert_eq!(
        Message::to_bytes(&reparsed),
        out,
        "parse/serialize must be a fixed point"
    );
});
