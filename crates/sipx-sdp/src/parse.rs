//! Reading SDP (RFC 8866 §5).
//!
//! SDP is line-oriented and strict about order, and forgiving about almost nothing else. What
//! this parser is deliberately lenient about: line endings (CRLF is required, LF is what half
//! the world sends) and unknown line types, which are kept rather than rejected.

use std::net::IpAddr;

use crate::session::{
    Address, Attribute, Connection, MediaDescription, Origin, SessionDescription, Timing,
};
use crate::{Result, SdpError};

/// Parse a session description.
pub fn parse(input: &str) -> Result<SessionDescription> {
    let mut origin = None;
    let mut session_name = None;
    let mut connection = None;
    let mut timing = Vec::new();
    let mut attributes = Vec::new();
    let mut media: Vec<MediaDescription> = Vec::new();
    let mut other = Vec::new();

    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        let mut chars = line.chars();
        let kind = chars
            .next()
            .ok_or_else(|| SdpError::MalformedLine(line.to_owned()))?;
        if chars.next() != Some('=') {
            return Err(SdpError::MalformedLine(line.to_owned()));
        }
        let value = line.get(2..).unwrap_or("");

        match kind {
            'v' => {
                // The version is always 0 and carries no information; rejecting a different
                // one would be pedantry with no upside.
            }
            'o' => origin = Some(parse_origin(value)?),
            's' => {
                // RFC 8866 §5.3: the line MUST NOT be empty — `s=-` is the spelling for "no
                // name" — so an empty one takes the same `-` a missing one does.
                if !value.is_empty() {
                    session_name = Some(value.to_owned());
                }
            }
            'c' => {
                let parsed = parse_connection(value)?;
                // A `c=` after an `m=` belongs to that stream, not the session.
                match media.last_mut() {
                    Some(stream) => stream.connection = Some(parsed),
                    None => connection = Some(parsed),
                }
            }
            't' => timing.push(parse_timing(value)),
            'm' => media.push(parse_media(value)?),
            'a' => {
                let attribute = parse_attribute(value);
                match media.last_mut() {
                    Some(stream) => stream.attributes.push(attribute),
                    None => attributes.push(attribute),
                }
            }
            // Everything else is kept verbatim. Dropping unknown lines is how an element
            // breaks features it has never heard of. RFC 8866 §5 places `i=`, `b=` and `k=`
            // inside each media description too, so a line after an `m=` belongs to that
            // stream, not the session.
            _ => match media.last_mut() {
                Some(stream) => stream.other.push((kind, value.to_owned())),
                None => other.push((kind, value.to_owned())),
            },
        }
    }

    Ok(SessionDescription {
        origin: origin.ok_or(SdpError::Missing("o="))?,
        session_name: session_name.unwrap_or_else(|| "-".to_owned()),
        connection,
        timing,
        attributes,
        media,
        other,
    })
}

fn parse_origin(value: &str) -> Result<Origin> {
    let mut parts = value.split_whitespace();
    let username = parts.next().unwrap_or("-").to_owned();
    let session_id = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("origin session id", value))?;
    let session_version = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| invalid("origin session version", value))?;
    let _net_type = parts.next();
    let _addr_type = parts.next();
    let address = parts
        .next()
        .map(parse_address)
        .ok_or_else(|| invalid("origin address", value))?;

    Ok(Origin {
        username,
        session_id,
        session_version,
        address,
    })
}

fn parse_connection(value: &str) -> Result<Connection> {
    let address = value
        .split_whitespace()
        .nth(2)
        .and_then(|raw| {
            // A multicast address may carry `/ttl` or `/ttl/count`, which is not part of the
            // address.
            raw.split('/').next().filter(|v| !v.is_empty())
        })
        .map(parse_address)
        .ok_or_else(|| invalid("connection address", value))?;
    Ok(Connection { address })
}

fn parse_address(raw: &str) -> Address {
    // RFC 8866 §5.2 and §5.7 allow a fully-qualified domain name where a literal is more
    // common. A token that is not a literal is kept as a name rather than rejected, because
    // refusing the address refuses the call.
    raw.parse::<IpAddr>()
        .map_or_else(|_| Address::Host(raw.to_owned()), Address::Ip)
}

fn parse_timing(value: &str) -> Timing {
    let mut parts = value.split_whitespace();
    // A malformed t= is treated as unbounded rather than fatal: it carries no information
    // sipx uses, and rejecting a whole description over it would refuse calls that work.
    Timing {
        start: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
        stop: parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
    }
}

fn parse_media(value: &str) -> Result<MediaDescription> {
    let mut parts = value.split_whitespace();
    let media = parts
        .next()
        .ok_or_else(|| invalid("media type", value))?
        .to_owned();
    let port = parts
        .next()
        .and_then(|raw| {
            // `port/count` is legal; the count is for hierarchical encoding and sipx uses the
            // base port.
            raw.split('/').next().and_then(|v| v.parse::<u16>().ok())
        })
        .ok_or_else(|| invalid("media port", value))?;
    let protocol = parts
        .next()
        .ok_or_else(|| invalid("media protocol", value))?
        .to_owned();
    let formats: Vec<String> = parts.map(str::to_owned).collect();
    // RFC 8866 §5.14: the ABNF requires at least one format. A bare `m=` line cannot even be
    // rejected in a well-formed answer, so it is refused here rather than propagated.
    if formats.is_empty() {
        return Err(invalid("media formats", value));
    }

    Ok(MediaDescription {
        media,
        port,
        protocol,
        formats,
        connection: None,
        attributes: Vec::new(),
        other: Vec::new(),
    })
}

fn parse_attribute(value: &str) -> Attribute {
    match value.split_once(':') {
        Some((name, rest)) => Attribute::valued(name, rest),
        None => Attribute::flag(value),
    }
}

fn invalid(field: &'static str, value: &str) -> SdpError {
    SdpError::Invalid {
        field,
        value: value.to_owned(),
    }
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
    use crate::session::Direction;

    const OFFER: &str = "v=0\r\n\
        o=alice 2890844526 2890844526 IN IP4 192.0.2.10\r\n\
        s=A call\r\n\
        c=IN IP4 192.0.2.10\r\n\
        t=0 0\r\n\
        m=audio 49170 RTP/AVP 0 8 101\r\n\
        a=rtpmap:0 PCMU/8000\r\n\
        a=rtpmap:8 PCMA/8000\r\n\
        a=rtpmap:101 telephone-event/8000\r\n\
        a=fmtp:101 0-15\r\n\
        a=sendrecv\r\n\
        a=ptime:20\r\n";

    #[test]
    fn a_session_description_parses_into_its_parts() {
        let sdp = parse(OFFER).expect("parses");
        assert_eq!(sdp.origin.username, "alice");
        assert_eq!(sdp.origin.session_id, 2_890_844_526);
        assert_eq!(sdp.session_name, "A call");
        assert_eq!(
            sdp.connection.expect("a connection").address.to_string(),
            "192.0.2.10"
        );
        assert_eq!(sdp.media.len(), 1);

        let audio = &sdp.media[0];
        assert_eq!(audio.media, "audio");
        assert_eq!(audio.port, 49170);
        assert_eq!(audio.protocol, "RTP/AVP");
        assert_eq!(audio.formats, vec!["0", "8", "101"]);
        assert_eq!(audio.rtpmap("0"), Some("PCMU/8000"));
        assert_eq!(audio.rtpmap("8"), Some("PCMA/8000"));
        assert_eq!(audio.direction(), Direction::SendRecv);
    }

    /// Attributes after an `m=` belong to that stream, not the session. Attaching them to the
    /// session instead makes a two-stream description nonsense.
    #[test]
    fn attributes_attach_to_the_media_line_they_follow() {
        let sdp = parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 192.0.2.1\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.1\r\n\
             t=0 0\r\n\
             a=session-level\r\n\
             m=audio 5000 RTP/AVP 0\r\n\
             a=sendonly\r\n\
             m=video 5002 RTP/AVP 96\r\n\
             a=recvonly\r\n",
        )
        .expect("parses");

        assert_eq!(sdp.attributes.len(), 1);
        assert_eq!(sdp.attributes[0].name, "session-level");
        assert_eq!(sdp.media[0].direction(), Direction::SendOnly);
        assert_eq!(sdp.media[1].direction(), Direction::RecvOnly);
    }

    /// A `c=` under an `m=` overrides the session's for that stream only.
    #[test]
    fn a_media_connection_overrides_the_session_one() {
        let sdp = parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 192.0.2.1\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.1\r\n\
             t=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n\
             c=IN IP4 198.51.100.7\r\n\
             m=video 5002 RTP/AVP 96\r\n",
        )
        .expect("parses");

        assert_eq!(
            sdp.address_for(&sdp.media[0])
                .expect("an address")
                .to_string(),
            "198.51.100.7"
        );
        assert_eq!(
            sdp.address_for(&sdp.media[1])
                .expect("an address")
                .to_string(),
            "192.0.2.1",
            "a stream with no c= falls back to the session's"
        );
    }

    /// An absent direction attribute means `sendrecv` (RFC 4566 §6). Defaulting to anything
    /// else silences calls that would otherwise work.
    #[test]
    fn an_absent_direction_means_sendrecv() {
        let sdp = parse(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n",
        )
        .expect("parses");
        assert_eq!(sdp.media[0].direction(), Direction::SendRecv);
    }

    /// Half the world sends bare LF. Rejecting it would be correct and useless.
    #[test]
    fn bare_line_feeds_are_accepted() {
        let sdp = parse(
            "v=0\no=- 1 1 IN IP4 192.0.2.1\ns=-\nc=IN IP4 192.0.2.1\nt=0 0\nm=audio 5000 RTP/AVP 0\n",
        )
        .expect("parses");
        assert_eq!(sdp.media[0].port, 5000);
    }

    /// Unknown lines survive, and they come back out where RFC 8866 §5 puts them: `i=`
    /// before `c=`, `b=` before the timing lines, `z=` after them — even when the input had
    /// them elsewhere. Receivers enforce the grammar's order, so emitting a kept line in the
    /// wrong slot turns tolerance into a parse error at the far end.
    #[test]
    fn unknown_lines_survive_a_round_trip() {
        let input = "v=0\r\n\
             o=- 1 1 IN IP4 192.0.2.1\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.1\r\n\
             t=0 0\r\n\
             b=AS:64\r\n\
             i=session info\r\n\
             z=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n";
        let sdp = parse(input).expect("parses");
        let out = sdp.to_string_sdp();
        let position = |needle: &str| {
            out.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing from {out}"))
        };
        assert!(position("i=session info") < position("c=IN"), "{out}");
        assert!(position("c=IN") < position("b=AS:64"), "{out}");
        assert!(position("b=AS:64") < position("t=0 0"), "{out}");
        assert!(position("t=0 0") < position("z=0 0"), "{out}");
        assert!(position("z=0 0") < position("m=audio"), "{out}");
    }

    /// RFC 8866 §5 places `i=`, `b=` and `k=` inside each media description, and §5.8 says a
    /// media-level `b=` is that stream's bandwidth. Hoisting them to session level collapses
    /// two streams' bandwidths into one meaningless pair.
    #[test]
    fn media_level_lines_stay_with_their_stream() {
        let sdp = parse(
            "v=0\r\n\
             o=- 1 1 IN IP4 192.0.2.1\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.1\r\n\
             t=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n\
             b=TIAS:64000\r\n\
             m=video 5002 RTP/AVP 96\r\n\
             b=TIAS:256000\r\n",
        )
        .expect("parses");

        assert!(sdp.other.is_empty(), "nothing here is session-level");

        let out = sdp.to_string_sdp();
        let position = |needle: &str| {
            out.find(needle)
                .unwrap_or_else(|| panic!("{needle} missing from {out}"))
        };
        assert!(position("m=audio") < position("b=TIAS:64000"), "{out}");
        assert!(position("b=TIAS:64000") < position("m=video"), "{out}");
        assert!(position("m=video") < position("b=TIAS:256000"), "{out}");
    }

    #[test]
    fn a_parsed_description_reserializes_to_the_same_meaning() {
        let sdp = parse(OFFER).expect("parses");
        let again = parse(&sdp.to_string_sdp()).expect("reparses");
        assert_eq!(sdp, again);
    }

    /// A rejected stream is present with port 0, not absent. This is what keeps an answer's
    /// media lines aligned with the offer's.
    #[test]
    fn port_zero_is_a_rejected_stream_not_a_parse_error() {
        let sdp = parse(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
             m=audio 0 RTP/AVP 0\r\n",
        )
        .expect("parses");
        assert!(sdp.media[0].is_rejected());
    }

    /// RFC 8866 §5.14: the ABNF requires at least one format on an `m=` line. Accepting a
    /// bare one means later emitting a format-less `m=` in the answer's rejection, which the
    /// far end cannot parse.
    #[test]
    fn a_media_line_without_formats_is_rejected() {
        assert!(matches!(
            parse(
                "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
                 m=application 49174 udp\r\n",
            ),
            Err(SdpError::Invalid {
                field: "media formats",
                ..
            })
        ));
    }

    #[test]
    fn a_line_without_an_equals_sign_is_rejected() {
        assert!(matches!(
            parse("v=0\r\nthis is not sdp\r\n"),
            Err(SdpError::MalformedLine(_))
        ));
    }

    /// RFC 8866 §5.3: the `s=` line MUST NOT be empty; `s=-` is the spelling for "no name".
    /// An empty one is taken the same way a missing one is, so it is never re-emitted
    /// invalid.
    #[test]
    fn an_empty_session_name_becomes_a_dash() {
        let sdp = parse(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n",
        )
        .expect("parses");
        assert_eq!(sdp.session_name, "-");
        assert!(sdp.to_string_sdp().contains("s=-\r\n"));
    }

    #[test]
    fn a_description_without_an_origin_is_rejected() {
        assert!(matches!(
            parse("v=0\r\ns=-\r\nt=0 0\r\n"),
            Err(SdpError::Missing("o="))
        ));
    }

    #[test]
    fn ipv6_addresses_round_trip() {
        let sdp = parse(
            "v=0\r\no=- 1 1 IN IP6 2001:db8::1\r\ns=-\r\nc=IN IP6 2001:db8::1\r\nt=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n",
        )
        .expect("parses");
        let out = sdp.to_string_sdp();
        assert!(out.contains("c=IN IP6 2001:db8::1"), "{out}");
        assert!(out.contains("o=- 1 1 IN IP6 2001:db8::1"), "{out}");
    }

    /// RFC 8866 §5.2 and §5.7 allow a fully-qualified domain name as the unicast address,
    /// and offers carrying one exist (RFC 3264 §10.1 opens with one). Rejecting the name
    /// rejects the whole description, and with it the call.
    #[test]
    fn a_domain_name_address_is_kept_not_rejected() {
        let sdp = parse(
            "v=0\r\n\
             o=alice 2890844526 2890844526 IN IP4 host.anywhere.com\r\n\
             s=-\r\n\
             c=IN IP4 host.anywhere.com\r\n\
             t=0 0\r\n\
             m=audio 49170 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        )
        .expect("parses");

        assert_eq!(sdp.origin.address.to_string(), "host.anywhere.com");
        assert_eq!(
            sdp.address_for(&sdp.media[0]),
            None,
            "a name is not an address until someone resolves it, and this crate does no I/O"
        );

        let out = sdp.to_string_sdp();
        assert!(
            out.contains("o=alice 2890844526 2890844526 IN IP4 host.anywhere.com"),
            "{out}"
        );
        assert!(out.contains("c=IN IP4 host.anywhere.com"), "{out}");
    }

    /// A multicast `c=` carries `/ttl`, which is not part of the address.
    #[test]
    fn a_multicast_connection_drops_its_ttl_suffix() {
        let sdp = parse(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 224.0.1.1/127\r\nt=0 0\r\n\
             m=audio 5000 RTP/AVP 0\r\n",
        )
        .expect("parses");
        assert_eq!(
            sdp.connection.expect("a connection").address.to_string(),
            "224.0.1.1"
        );
    }
}
