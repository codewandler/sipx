//! What `parse` + `to_string_sdp` does not preserve (`S-52`).
//!
//! These are not aspirations. Each assertion pins a byte the round trip is *known* to drop, so a
//! parser change that silently starts preserving one — or starts dropping a different one — fails
//! here rather than surprising the next caller who forwards someone else's description.

use sipx_sdp::parse;

/// A description that exercises every field the round trip is documented to lose.
const RECEIVED: &str = concat!(
    "v=0\r\n",
    "o=alice 2890844526 2890842807 IN IP4 198.51.100.1\r\n",
    "s=-\r\n",
    "c=IN IP4 233.252.0.1/127/3\r\n",
    "t=0 0\r\n",
    "m=audio 49170/2 RTP/AVP 0\r\n",
    "a=rtpmap:0 PCMU/8000\r\n",
);

#[test]
fn the_round_trip_drops_the_multicast_ttl_and_address_count() {
    let rendered = parse(RECEIVED).expect("the fixture parses").to_string_sdp();
    assert!(
        RECEIVED.contains("233.252.0.1/127/3"),
        "the fixture must carry a multicast ttl and count to be worth asserting on"
    );
    assert!(
        !rendered.contains("/127/3"),
        "to_string_sdp preserved the multicast ttl and count; the documented loss set on \
         to_string_sdp is now wrong and callers who forward a description need telling: {rendered}"
    );
}

#[test]
fn the_round_trip_drops_the_media_port_count() {
    let rendered = parse(RECEIVED).expect("the fixture parses").to_string_sdp();
    assert!(
        !rendered.contains("49170/2"),
        "to_string_sdp preserved the m= port count; the documented loss set is now wrong: \
         {rendered}"
    );
}

#[test]
fn the_rendered_description_is_not_the_bytes_that_arrived() {
    let rendered = parse(RECEIVED).expect("the fixture parses").to_string_sdp();
    assert_ne!(
        RECEIVED, rendered,
        "if the round trip ever becomes lossless, to_string_sdp's documentation and the relay \
         module's reason for existing both need revisiting"
    );
}
