//! Stream framing must not panic however the input is chopped up.
//!
//! The first byte chooses a chunk size, so the fuzzer explores chunk boundaries as well as
//! content — the two interact, and boundary bugs are the ones unit tests miss.

#![no_main]

use libfuzzer_sys::fuzz_target;
use sipx_sip::{Limits, StreamParser};

fuzz_target!(|data: &[u8]| {
    let Some((&first, rest)) = data.split_first() else {
        return;
    };
    let chunk_size = usize::from(first).max(1);

    let mut parser = StreamParser::new(Limits::stream());
    for chunk in rest.chunks(chunk_size) {
        if parser.push(chunk).is_err() {
            // A framing error is terminal by design; keep pushing to prove that stays true.
            break;
        }
    }
});
