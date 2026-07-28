//! URI parsing must not panic, and anything that parses must serialize.

#![no_main]

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use sipx_sip::Uri;

fuzz_target!(|data: &[u8]| {
    if let Ok(uri) = Uri::parse(Bytes::copy_from_slice(data)) {
        let _ = uri.to_bytes();
        let _ = uri.decoded_user();
        // Equivalence is reachable from protocol logic on attacker-supplied URIs.
        let _ = uri.equivalent(&uri);
    }
});
