//! Parse a SIP message and read its headers, with no runtime and no sockets.
//!
//! `sipx-sip` is usable entirely on its own — this example depends on no async runtime at all.
//!
//! ```text
//! cargo run --example parse_a_message
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use bytes::Bytes;
use sipx_sip::{HeaderName, Limits, Message, parse_datagram};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let wire = b"INVITE sip:bob@example.com SIP/2.0\r\n\
        Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bK776asdhds\r\n\
        Max-Forwards: 70\r\n\
        To: Bob <sip:bob@example.com>\r\n\
        From: Alice <sip:alice@example.net>;tag=1928301774\r\n\
        Call-ID: a84b4c76e66710@example.net\r\n\
        CSeq: 314159 INVITE\r\n\
        X-Vendor-Thing: preserved verbatim\r\n\
        Content-Length: 0\r\n\r\n";

    let message = parse_datagram(Bytes::from_static(wire), &Limits::datagram())?;

    let Message::Request(request) = message else {
        return Err("expected a request".into());
    };
    println!("{:?} {}", request.method, request.uri);

    // Typed access is lazy: the header is parsed when it is asked for, not when the message is.
    let via = request.headers.typed::<sipx_sip::headers::Via>();
    if let Some(Ok(via)) = via {
        println!("came from {}", via.host);
    }

    // A header sipx has no behaviour for still survives intact. That is why "parse-only" is a
    // status in the compliance table rather than a gap in it.
    if let Some(value) = request
        .headers
        .value(&HeaderName::Other("X-Vendor-Thing".into()))
    {
        println!("unknown header kept: {}", String::from_utf8_lossy(&value));
    }

    // And it re-serializes byte for byte.
    assert_eq!(
        sipx_sip::Message::Request(request).to_bytes().as_ref(),
        wire.as_slice()
    );
    println!("round-tripped byte for byte");
    Ok(())
}
