//! Offer/answer (RFC 3264).
//!
//! A pure function. The rules here are full of cases that are awkward to reach through a live
//! call — a stream with no common codec, an offer that reorders media, a `sendonly` that must
//! become `recvonly` — and they are all one function call away.
//!
//! The rule that shapes everything: **the answer has the same number of `m=` lines as the
//! offer, in the same order.** A stream that cannot be accepted is answered with port 0, not
//! omitted. Omitting it shifts every stream after it, so the two ends disagree about which
//! stream is which — and that is a call where video arrives on the audio port.

use std::net::IpAddr;

use crate::session::{
    Attribute, Connection, Direction, MediaDescription, Origin, SessionDescription, Timing,
};

/// What this side can do.
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// Where to receive media.
    pub address: IpAddr,
    /// The port to receive audio on. Zero rejects audio entirely.
    pub audio_port: u16,
    /// Payload types this side supports, as offered.
    pub audio_formats: Vec<String>,
    /// `rtpmap` values by payload type, for the formats above.
    pub rtpmaps: Vec<(String, String)>,
    /// The direction this side wants.
    pub direction: Direction,
    /// The session identifier to use.
    pub session_id: u64,
    /// The session version to use.
    pub session_version: u64,
}

impl Capabilities {
    /// The G.711 pair plus RFC 4733 DTMF, which is what practically every endpoint accepts.
    #[must_use]
    pub fn g711(address: IpAddr, audio_port: u16) -> Self {
        Self {
            address,
            audio_port,
            audio_formats: vec!["0".to_owned(), "8".to_owned(), "101".to_owned()],
            rtpmaps: vec![
                ("0".to_owned(), "PCMU/8000".to_owned()),
                ("8".to_owned(), "PCMA/8000".to_owned()),
                ("101".to_owned(), "telephone-event/8000".to_owned()),
            ],
            direction: Direction::SendRecv,
            session_id: 1,
            session_version: 1,
        }
    }

    fn rtpmap_for(&self, format: &str) -> Option<&str> {
        self.rtpmaps
            .iter()
            .find(|(payload, _)| payload == format)
            .map(|(_, value)| value.as_str())
    }
}

/// Build an answer to an offer.
///
/// Returns a description whose media lines correspond one to one with the offer's.
#[must_use]
pub fn answer(offer: &SessionDescription, capabilities: &Capabilities) -> SessionDescription {
    let mut media = Vec::with_capacity(offer.media.len());

    for offered in &offer.media {
        media.push(answer_stream(offer, offered, capabilities));
    }

    SessionDescription {
        origin: Origin::new(
            capabilities.address,
            capabilities.session_id,
            capabilities.session_version,
        ),
        session_name: "-".to_owned(),
        connection: Some(Connection::new(capabilities.address)),
        timing: vec![Timing::default()],
        attributes: Vec::new(),
        media,
        other: Vec::new(),
    }
}

fn answer_stream(
    offer: &SessionDescription,
    offered: &MediaDescription,
    capabilities: &Capabilities,
) -> MediaDescription {
    // An offer that already rejected the stream is answered with a rejection. Reviving it
    // would be answering a question that was not asked.
    if offered.is_rejected() {
        return rejected(offered);
    }

    // sipx handles audio. Anything else is declined rather than ignored — declining keeps the
    // media lines aligned, which is the whole point.
    if offered.media != "audio" || capabilities.audio_port == 0 {
        return rejected(offered);
    }

    // RFC 3264 §6.1: the answer lists the formats both sides support. The order is the
    // *offerer's*, because the offerer's first choice is the one it most wants used, and the
    // answerer expressing its own preference here is how two endpoints end up transcoding for
    // no reason.
    let common: Vec<String> = offered
        .formats
        .iter()
        .filter(|format| supports(capabilities, offered, format))
        .cloned()
        .collect();

    // A stream with nothing in common is rejected. Answering with an empty format list is not
    // an alternative: it is syntactically invalid and says nothing.
    if common.is_empty() || common.iter().all(|f| is_telephone_event(offered, f)) {
        return rejected(offered);
    }

    let mut attributes = Vec::new();
    for format in &common {
        // Prefer the offerer's own spelling of the rtpmap when it gave one: it is
        // authoritative for dynamic payload types, where the number means nothing on its own.
        let rtpmap = offered
            .rtpmap(format)
            .or_else(|| capabilities.rtpmap_for(format));
        if let Some(rtpmap) = rtpmap {
            attributes.push(Attribute::valued("rtpmap", format!("{format} {rtpmap}")));
        }
        // DTMF needs its fmtp echoed or the far end does not know which events we take.
        if is_telephone_event(offered, format) {
            let events = offered
                .attributes
                .iter()
                .find(|a| {
                    a.name == "fmtp"
                        && a.value
                            .as_deref()
                            .is_some_and(|v| v.starts_with(&format!("{format} ")))
                })
                .and_then(|a| a.value.clone())
                .unwrap_or_else(|| format!("{format} 0-15"));
            attributes.push(Attribute::valued("fmtp", events));
        }
    }

    // The direction is a negotiation, not a copy. The answer can only narrow what the offer
    // proposed: an offer of `sendonly` cannot be answered `sendrecv`.
    let direction = negotiate_direction(offered.direction(), capabilities.direction);
    attributes.push(Attribute::flag(direction.as_str()));

    let _ = offer;

    MediaDescription {
        media: offered.media.clone(),
        port: capabilities.audio_port,
        protocol: offered.protocol.clone(),
        formats: common,
        connection: None,
        attributes,
    }
}

/// The direction an answer may carry, given what was offered and what this side wants.
///
/// The mirror of the offer is the *most* the answer may claim; the local preference can only
/// narrow it further.
#[must_use]
pub fn negotiate_direction(offered: Direction, wanted: Direction) -> Direction {
    let allowed = offered.mirrored();
    let sends = allowed.sends() && wanted.sends();
    let receives = allowed.receives() && wanted.receives();
    match (sends, receives) {
        (true, true) => Direction::SendRecv,
        (true, false) => Direction::SendOnly,
        (false, true) => Direction::RecvOnly,
        (false, false) => Direction::Inactive,
    }
}

fn rejected(offered: &MediaDescription) -> MediaDescription {
    MediaDescription {
        media: offered.media.clone(),
        port: 0,
        protocol: offered.protocol.clone(),
        // A rejected stream keeps a format so the line stays well-formed; the RFC allows any
        // single one, and echoing the offer's first is the least surprising.
        formats: offered.formats.first().cloned().into_iter().collect(),
        connection: None,
        attributes: Vec::new(),
    }
}

/// Whether this side supports a payload type.
///
/// Static payload types (0–95) mean the same thing everywhere, so the number is enough.
/// Dynamic ones (96–127) mean whatever the `rtpmap` says, so the encoding names must match —
/// comparing the numbers alone is how a stack agrees to a codec it cannot decode.
fn supports(capabilities: &Capabilities, offered: &MediaDescription, format: &str) -> bool {
    let is_dynamic = format
        .parse::<u8>()
        .is_ok_and(|payload| (96..=127).contains(&payload));

    if !is_dynamic {
        return capabilities.audio_formats.iter().any(|f| f == format);
    }

    let Some(offered_map) = offered.rtpmap(format) else {
        // A dynamic type with no rtpmap is uninterpretable, whatever the number.
        return false;
    };
    let offered_encoding = encoding_of(offered_map);
    capabilities
        .rtpmaps
        .iter()
        .any(|(_, mapping)| encoding_of(mapping).eq_ignore_ascii_case(offered_encoding))
}

fn encoding_of(rtpmap: &str) -> &str {
    rtpmap.split('/').next().unwrap_or(rtpmap)
}

fn is_telephone_event(offered: &MediaDescription, format: &str) -> bool {
    offered
        .rtpmap(format)
        .is_some_and(|mapping| encoding_of(mapping).eq_ignore_ascii_case("telephone-event"))
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
    use crate::parse::parse;

    fn local() -> IpAddr {
        "192.0.2.20".parse().expect("valid")
    }

    fn offer(body: &str) -> SessionDescription {
        parse(body).expect("the offer parses")
    }

    const AUDIO_OFFER: &str = "v=0\r\n\
        o=alice 1 1 IN IP4 192.0.2.10\r\n\
        s=-\r\n\
        c=IN IP4 192.0.2.10\r\n\
        t=0 0\r\n\
        m=audio 49170 RTP/AVP 0 8 101\r\n\
        a=rtpmap:0 PCMU/8000\r\n\
        a=rtpmap:8 PCMA/8000\r\n\
        a=rtpmap:101 telephone-event/8000\r\n\
        a=fmtp:101 0-15\r\n\
        a=sendrecv\r\n";

    #[test]
    fn a_plain_audio_offer_is_answered_with_the_common_codecs() {
        let answered = answer(&offer(AUDIO_OFFER), &Capabilities::g711(local(), 40000));
        assert_eq!(answered.media.len(), 1);
        let audio = &answered.media[0];
        assert_eq!(audio.port, 40000);
        assert_eq!(audio.formats, vec!["0", "8", "101"]);
        assert_eq!(audio.rtpmap("0"), Some("PCMU/8000"));
        assert_eq!(audio.direction(), Direction::SendRecv);
    }

    /// The failing-first test for this story. A rejected stream stays in place with port 0;
    /// omitting it would shift every later stream and make the two ends disagree about which
    /// stream is which.
    #[test]
    fn an_answer_keeps_the_offers_media_order_and_rejects_with_port_zero() {
        let offered = offer(
            "v=0\r\n\
             o=alice 1 1 IN IP4 192.0.2.10\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.10\r\n\
             t=0 0\r\n\
             m=video 49172 RTP/AVP 96\r\n\
             a=rtpmap:96 H264/90000\r\n\
             m=audio 49170 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             m=application 49174 udp wb\r\n",
        );
        let answered = answer(&offered, &Capabilities::g711(local(), 40000));

        assert_eq!(answered.media.len(), 3, "one answer per offered stream");
        assert_eq!(answered.media[0].media, "video");
        assert_eq!(answered.media[0].port, 0, "video is declined");
        assert_eq!(answered.media[1].media, "audio");
        assert_eq!(answered.media[1].port, 40000, "audio is accepted, in place");
        assert_eq!(answered.media[2].media, "application");
        assert_eq!(answered.media[2].port, 0);
    }

    /// The order is the offerer's. An answerer that imposes its own preference is how two
    /// endpoints end up transcoding for no reason.
    #[test]
    fn the_codec_order_is_the_offerers_not_ours() {
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 8 0\r\n\
             a=rtpmap:8 PCMA/8000\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        );
        // Our own list prefers PCMU (0) first.
        let answered = answer(&offered, &Capabilities::g711(local(), 40000));
        assert_eq!(
            answered.media[0].formats,
            vec!["8", "0"],
            "the offerer asked for PCMA first, so PCMA comes first"
        );
    }

    #[test]
    fn a_codec_we_do_not_have_is_left_out_of_the_answer() {
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 0 9\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             a=rtpmap:9 G722/8000\r\n",
        );
        let answered = answer(&offered, &Capabilities::g711(local(), 40000));
        assert_eq!(answered.media[0].formats, vec!["0"], "G.722 is not ours");
    }

    /// Nothing in common is a rejection. An answer with an empty format list is not an
    /// alternative — it is invalid and says nothing.
    #[test]
    fn a_stream_with_no_common_codec_is_rejected() {
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 9\r\n\
             a=rtpmap:9 G722/8000\r\n",
        );
        let answered = answer(&offered, &Capabilities::g711(local(), 40000));
        assert!(answered.media[0].is_rejected());
        assert!(!answered.media[0].formats.is_empty(), "still well-formed");
    }

    /// DTMF alone is not a call. A stream offering only telephone-event has no audio codec, so
    /// accepting it would establish a session that can never carry speech.
    #[test]
    fn a_stream_offering_only_dtmf_is_rejected() {
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 101\r\n\
             a=rtpmap:101 telephone-event/8000\r\n",
        );
        assert!(answer(&offered, &Capabilities::g711(local(), 40000)).media[0].is_rejected());
    }

    /// The direction is mirrored, not copied. Copying produces a call where both ends wait for
    /// audio that never comes.
    #[test]
    fn directions_are_mirrored_rather_than_copied() {
        for (offered_direction, expected) in [
            ("sendrecv", Direction::SendRecv),
            ("sendonly", Direction::RecvOnly),
            ("recvonly", Direction::SendOnly),
            ("inactive", Direction::Inactive),
        ] {
            let offered = offer(&format!(
                "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
                 m=audio 49170 RTP/AVP 0\r\n\
                 a=rtpmap:0 PCMU/8000\r\n\
                 a={offered_direction}\r\n"
            ));
            assert_eq!(
                answer(&offered, &Capabilities::g711(local(), 40000)).media[0].direction(),
                expected,
                "offer of {offered_direction}"
            );
        }
    }

    /// The answer may only narrow what was offered. An offer of `sendonly` cannot be answered
    /// `sendrecv` however much this side would like to send.
    #[test]
    fn the_answer_cannot_widen_what_was_offered() {
        assert_eq!(
            negotiate_direction(Direction::SendOnly, Direction::SendRecv),
            Direction::RecvOnly
        );
        assert_eq!(
            negotiate_direction(Direction::SendRecv, Direction::RecvOnly),
            Direction::RecvOnly
        );
        assert_eq!(
            negotiate_direction(Direction::Inactive, Direction::SendRecv),
            Direction::Inactive
        );
        assert_eq!(
            negotiate_direction(Direction::RecvOnly, Direction::RecvOnly),
            Direction::Inactive,
            "the offerer will only receive and so will we: nothing flows"
        );
    }

    /// A dynamic payload type means whatever its `rtpmap` says. Matching on the number alone
    /// is how a stack agrees to a codec it cannot decode.
    #[test]
    fn dynamic_payload_types_are_matched_by_name_not_number() {
        let mut capabilities = Capabilities::g711(local(), 40000);
        capabilities.audio_formats.push("96".to_owned());
        capabilities
            .rtpmaps
            .push(("96".to_owned(), "opus/48000/2".to_owned()));

        // The far end uses 96 for something else entirely.
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 96 0\r\n\
             a=rtpmap:96 SPEEX/8000\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        );
        let answered = answer(&offered, &capabilities);
        assert_eq!(
            answered.media[0].formats,
            vec!["0"],
            "96 is Speex there and Opus here; the numbers agreeing means nothing"
        );

        // And when the names do match, it is accepted.
        let matching = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 96 0\r\n\
             a=rtpmap:96 opus/48000/2\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        );
        assert_eq!(
            answer(&matching, &capabilities).media[0].formats,
            vec!["96", "0"]
        );
    }

    #[test]
    fn a_dynamic_payload_type_without_an_rtpmap_is_not_accepted() {
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 49170 RTP/AVP 96 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        );
        assert_eq!(
            answer(&offered, &Capabilities::g711(local(), 40000)).media[0].formats,
            vec!["0"]
        );
    }

    /// An offer that already rejected a stream is answered with a rejection; reviving it would
    /// answer a question nobody asked.
    #[test]
    fn a_stream_the_offer_already_rejected_stays_rejected() {
        let offered = offer(
            "v=0\r\no=a 1 1 IN IP4 192.0.2.10\r\ns=-\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\n\
             m=audio 0 RTP/AVP 0\r\n\
             a=rtpmap:0 PCMU/8000\r\n",
        );
        assert!(answer(&offered, &Capabilities::g711(local(), 40000)).media[0].is_rejected());
    }

    #[test]
    fn the_dtmf_fmtp_is_echoed_so_the_far_end_knows_which_events_we_take() {
        let answered = answer(&offer(AUDIO_OFFER), &Capabilities::g711(local(), 40000));
        let fmtp = answered.media[0]
            .attributes
            .iter()
            .find(|a| a.name == "fmtp")
            .and_then(|a| a.value.clone())
            .expect("an fmtp for DTMF");
        assert_eq!(fmtp, "101 0-15");
    }

    #[test]
    fn the_answer_advertises_our_address_and_port() {
        let answered = answer(&offer(AUDIO_OFFER), &Capabilities::g711(local(), 40000));
        assert_eq!(answered.connection.expect("a connection").address, local());
        assert_eq!(answered.origin.address, local());
        assert_eq!(answered.media[0].port, 40000);
    }

    #[test]
    fn an_answer_reparses_to_itself() {
        let answered = answer(&offer(AUDIO_OFFER), &Capabilities::g711(local(), 40000));
        let round_tripped = parse(&answered.to_string_sdp()).expect("the answer parses");
        assert_eq!(answered, round_tripped);
    }
}
