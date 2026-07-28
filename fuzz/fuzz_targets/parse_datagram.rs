//! Datagram parsing must not panic, hang, or allocate unboundedly, whatever arrives.

#![no_main]

use bytes::Bytes;
use libfuzzer_sys::fuzz_target;
use sipx_sip::{Limits, parse_datagram};

fuzz_target!(|data: &[u8]| {
    let _ = parse_datagram(Bytes::copy_from_slice(data), &Limits::datagram());
});
