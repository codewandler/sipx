//! What one SDP description settles: codec, payload numbers, RTCP mode and SRTP keys.
//!
//! The offer/answer vocabulary (RFC 3264) shared by call establishment and every in-dialog
//! renegotiation: building an offer from capabilities, settling the peer's answer against it,
//! and reading a description into the [`Negotiated`] facts a media session is built from.

use super::{
    Capabilities, Codec, Codecs, Direction, Error, Result, SessionDescription, SocketAddr,
};

/// Refuse a socket-ownership change until renegotiation can replace the media session atomically.
pub(super) fn preserve_rtcp_mode(
    current: sipx_sdp::RtcpMode,
    proposed: sipx_sdp::RtcpMode,
) -> Result<()> {
    if current == proposed {
        Ok(())
    } else {
        Err(Error::RtcpModeChange { current, proposed })
    }
}

/// The mode one answer selected for its corresponding offered audio section.
pub(super) fn exchanged_rtcp_mode(
    offer: &SessionDescription,
    answer: &SessionDescription,
) -> sipx_sdp::RtcpMode {
    offer
        .media
        .iter()
        .zip(&answer.media)
        .find(|(offered, _)| offered.media == "audio")
        .map_or(sipx_sdp::RtcpMode::Separate, |(offered, answered)| {
            sipx_sdp::RtcpMode::from_exchange(offered, answered)
        })
}

/// The RTCP shape this implementation will select when answering `offer`.
pub(super) fn answering_rtcp_mode(offer: &SessionDescription) -> sipx_sdp::RtcpMode {
    offer
        .media
        .iter()
        .find(|media| media.media == "audio" && !media.is_rejected())
        .filter(|media| media.rtcp_mux())
        .map_or(sipx_sdp::RtcpMode::Separate, |_| sipx_sdp::RtcpMode::Mux)
}

/// What the far end's answer to *our* offer settles.
///
/// The calling side's counterpart of [`Early::settle`], and the reason it is a function is that
/// an answer can now reach us in two places: the 200 that [`establish`] reads, and — once
/// [`dial_early`] exists — the reliable provisional that makes an early dialog renegotiable at
/// all (RFC 3262 §5). There is no port to bind on either path, because ours was bound before the
/// INVITE named it.
pub(super) fn settle_answer(
    offered: &Capabilities,
    answer: &SessionDescription,
    codecs: Codecs,
) -> Result<Settled> {
    // Both halves or neither, *and* the two halves have to be the ones the two ends agreed on:
    // a stream keyed at one end only is a call that connects and carries silence, and one keyed
    // on an answer that echoed a tag nobody sent is a call encrypted to nothing. Neither is
    // worth having, so both come back as `Error::Sdp` rather than as a quietly plain call.
    let answered = answered_crypto(answer);
    let mut negotiated = negotiated(answer, codecs)?;
    let local_offer = offer_from(offered);
    let local_audio = local_offer
        .media
        .iter()
        .find(|media| media.media == "audio" && !media.is_rejected())
        .ok_or(Error::NoCommonCodec)?;
    negotiated.receive_payload_type = local_audio
        .formats
        .iter()
        .find_map(|format| {
            let (codec, payload_type, clock_rate) = codec_of(local_audio, format)?;
            (codec == negotiated.codec && clock_rate == negotiated.clock_rate)
                .then_some(payload_type)
        })
        .ok_or(Error::NoCommonCodec)?;
    let answered_mux = answer
        .media
        .iter()
        .find(|media| media.media == "audio" && !media.is_rejected())
        .is_some_and(sipx_sdp::MediaDescription::rtcp_mux);
    negotiated.rtcp_mode = if offered.rtcp_mux && answered_mux {
        sipx_sdp::RtcpMode::Mux
    } else {
        sipx_sdp::RtcpMode::Separate
    };
    Ok(Settled {
        negotiated,
        srtp: srtp_keys(offered.crypto.as_slice(), answered.as_ref())?,
    })
}

pub(crate) fn offer_from(capabilities: &Capabilities) -> SessionDescription {
    let mut sdp = SessionDescription::new(
        capabilities.address,
        capabilities.session_id,
        capabilities.session_version,
    );
    let mut audio = sipx_sdp::MediaDescription::audio(
        capabilities.audio_port,
        capabilities.audio_formats.clone(),
    );
    for (payload, mapping) in &capabilities.rtpmaps {
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "rtpmap",
            format!("{payload} {mapping}"),
        ));
    }
    // The key, and the protocol that matches it. Offering `a=crypto` under `RTP/AVP` asks for a
    // stream that is encrypted and declared not to be; offering `RTP/SAVP` with no key asks for
    // encryption with nothing to key it. Both come from the same place, so neither can drift.
    if !capabilities.crypto.is_empty() {
        capabilities.protocol().clone_into(&mut audio.protocol);
        // One line per suite, in the order the capabilities hold them — which is strongest first.
        // RFC 4568 §5.1.1 reads that order as preference, and it agrees with the rule the answer
        // applies whether or not the far end honours it.
        for crypto in &capabilities.crypto {
            audio
                .attributes
                .push(sipx_sdp::Attribute::valued("crypto", crypto.to_value()));
        }
    }
    // The same rule for DTLS-SRTP, with the fingerprint in place of the key: `UDP/TLS/RTP/SAVP`
    // and an `a=fingerprint` come from one place so a stream cannot claim one and carry the
    // other. RFC 5763 §5 requires the *offerer* to say `actpass` and let the answerer choose.
    if let Some(fingerprint) = capabilities.dtls() {
        capabilities.protocol().clone_into(&mut audio.protocol);
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "fingerprint",
            fingerprint.to_value(),
        ));
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "setup",
            sipx_sdp::fingerprint::Setup::ActPass.as_str().to_owned(),
        ));
    }
    if capabilities.rtcp_mux {
        audio.attributes.push(sipx_sdp::Attribute::flag("rtcp-mux"));
    }
    audio.set_direction(capabilities.direction);
    sdp.media.push(audio);
    sdp
}

/// What negotiation settled on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Negotiated {
    pub(crate) remote: SocketAddr,
    pub(crate) codec: Codec,
    /// RTP clock rate of this exact format.
    ///
    /// Usually fixed by the codec. L16 can be negotiated at more than one rate, so retaining the
    /// format's rate is what keeps packet sizing, resampling, and RTP timestamps in agreement.
    pub(crate) clock_rate: u32,
    /// The payload type to send `codec` with, when the description gave it a number.
    ///
    /// `None` only for a bare static type matched by number. Anything an rtpmap touched —
    /// Opus always, a remapped static possibly — has no number of its own that means anything:
    /// 111 is convention, and what the far end listens for is the number *it* assigned.
    pub(crate) payload_type: Option<u8>,
    /// The payload type our description assigned to packets arriving for this codec.
    ///
    /// Separate from [`Self::payload_type`], which belongs to the peer's description and is the
    /// number used for sending. They are usually equal but dynamic assignments are directional.
    pub(crate) receive_payload_type: Option<u8>,
    /// The payload type the far end uses for `telephone-event`, if it offered one.
    ///
    /// Taken from the description rather than assumed, because it is a *dynamic* type: 101 is
    /// what sipx offers, not what everyone uses, and assuming it would send keypresses on
    /// whatever the far end put that number to.
    pub(crate) dtmf: Option<u8>,
    /// Whether RTCP shares the RTP port or uses its adjacent control port.
    pub(crate) rtcp_mode: sipx_sdp::RtcpMode,
}

/// What negotiation settled on, plus the keys — which are not `Copy` and do not belong in a
/// type that is.
#[derive(Debug, Clone)]
pub(crate) struct Settled {
    pub(crate) negotiated: Negotiated,
    pub(super) srtp: Option<sipx_media::SrtpKeys>,
}

impl Negotiated {
    /// The number this codec actually goes out with: the one the description assigned, or the
    /// codec's own when it is a static type nothing remapped.
    ///
    /// Mirrors [`sipx_media::Config::wire_payload_type`], which is what the session reads — so this
    /// is the value to compare when asking whether the wire changed. The raw [`Self::payload_type`]
    /// is not: `Some(0)` and `None` are two descriptions of PCMU and the same byte on the wire.
    pub(super) fn wire_payload_type(&self) -> u8 {
        self.payload_type
            .unwrap_or_else(|| self.codec.payload_type())
    }

    pub(super) fn receive_wire_payload_type(&self) -> u8 {
        self.receive_payload_type
            .unwrap_or_else(|| self.codec.payload_type())
    }

    pub(super) fn media_config(self) -> sipx_media::Config {
        let mut config = sipx_media::Config::new(self.remote, self.codec);
        config.clock_rate = self.clock_rate;
        config.payload_type = self.payload_type;
        config.receive_payload_type = self.receive_payload_type;
        config.dtmf_payload_type = self.dtmf;
        config.rtcp_mode = self.rtcp_mode;
        config
    }
}

impl Settled {
    /// Whether both halves of the keying are present, so the media is actually encrypted.
    pub(crate) fn is_encrypted(&self) -> bool {
        self.srtp.is_some()
    }

    pub(crate) fn media_config(&self) -> sipx_media::Config {
        let mut config = self.negotiated.media_config();
        config.srtp.clone_from(&self.srtp);
        config
    }
}

/// The keys an answer to *our* offer settles on, once it has been checked against what we sent.
///
/// RFC 4568 §5.1.3 makes the check a MUST on the offerer, and this is the only place a call can
/// run it: [`sipx_media::SrtpKeys::from_answer`] is the sole route from an answer to keys, and it
/// returns which of *our* offers the answer accepted, so the half we key with is the half we sent
/// rather than whichever one happened to be first. `docs/specs/srtp.md` §5.4.
///
/// `offered` is a slice and not one attribute because that is what the check takes. sipx offers
/// exactly one today, and a function that quietly assumed so would have to be found again the day
/// it offers two.
///
/// `Ok(None)` means this side offered no key at all — a plain call, which is the only case where
/// the absence of an `a=crypto` in the answer is not a failure. When we did offer, an answer
/// carrying nothing usable is refused: that is the shape "a suite that was never offered" arrives
/// in, since [`sipx_sdp::crypto::Crypto::parse`] refuses a suite sipx cannot key.
///
/// # Errors
///
/// [`Error::Sdp`] when the answer accepted a tag and suite this side never offered, or carried no
/// key. Not `None`: dropping to an unencrypted call would hand the user an insecure call presented
/// as a secure one, and dropping the stream would end the call with nothing anyone can act on.
pub(crate) fn srtp_keys(
    offered: &[sipx_sdp::crypto::Crypto],
    answered: Option<&sipx_sdp::crypto::Crypto>,
) -> Result<Option<sipx_media::SrtpKeys>> {
    if offered.is_empty() {
        // Nothing was offered, so there is nothing to verify and no local half to key with. An
        // answer cannot introduce SDES the offer did not ask for (RFC 4568 §5.1.2).
        return Ok(None);
    }
    sipx_media::SrtpKeys::from_answer(offered, answered)
        .map(Some)
        .map_err(|error| Error::Sdp(error.to_string()))
}

/// Pair the key we are *answering* with against the far end's offered one.
///
/// The other side of [`srtp_keys`], and deliberately not the same function. §5.1.3's check is the
/// offerer's: here this side chose the attribute and echoed its tag ([`sipx_sdp::answer`], RFC
/// 4568 §5.1.2), so there is nothing to verify — only two halves to put together.
///
/// `None` unless *both* are present. One key is not a session: a stream keyed at one end only
/// is a stream the other end cannot read, and treating a half-offer as success would produce a
/// call that connects and carries silence.
///
/// `ours` is the whole capability list, and the half taken from it is the one whose **suite**
/// matches what [`sipx_sdp::answer`] accepted — not the first entry. Since `M-41` this side
/// offers several, and keying with the wrong one produces a well-formed stream nobody can read.
pub(crate) fn srtp_keys_answering(
    ours: &[sipx_sdp::crypto::Crypto],
    theirs: Option<sipx_sdp::crypto::Crypto>,
) -> Option<sipx_media::SrtpKeys> {
    let theirs = theirs?;
    let ours = ours.iter().find(|mine| mine.suite == theirs.suite)?;
    Some(sipx_media::SrtpKeys {
        profile: sipx_media::transform_of(theirs.suite),
        local: (ours.master_key().to_vec(), ours.master_salt().to_vec()),
        remote: (theirs.master_key().to_vec(), theirs.master_salt().to_vec()),
    })
}

/// The keying the far end offered, from its description. Same shape as the answered one; named
/// separately because reading it from an *offer* and from an *answer* are different moments.
pub(crate) fn offer_crypto(sdp: &SessionDescription) -> Option<sipx_sdp::crypto::Crypto> {
    answered_crypto(sdp)
}

/// The keying the far end answered with, from its description.
fn answered_crypto(sdp: &SessionDescription) -> Option<sipx_sdp::crypto::Crypto> {
    sdp.media
        .iter()
        .find(|m| m.media == "audio" && !m.is_rejected())?
        .crypto()
}

/// The payload type carrying `telephone-event`, per the description's own rtpmaps.
fn telephone_event_payload_type(audio: &sipx_sdp::MediaDescription) -> Option<u8> {
    audio.formats.iter().find_map(|format| {
        let mapping = audio.rtpmap(format)?;
        let encoding = mapping.split('/').next().unwrap_or(mapping);
        encoding
            .eq_ignore_ascii_case("telephone-event")
            .then(|| format.parse::<u8>().ok())
            .flatten()
    })
}

/// Where to send media, and in what codec, from a description.
///
/// `codecs` is the set this side offered or answered from: negotiation may only settle on a
/// codec the application selected, so an Opus offer answered from a G.711 set settles on
/// G.711, not on a codec the answer never named.
pub(crate) fn negotiated(sdp: &SessionDescription, codecs: Codecs) -> Result<Negotiated> {
    let audio = sdp
        .media
        .iter()
        .find(|m| m.media == "audio" && !m.is_rejected())
        .ok_or(Error::NoCommonCodec)?;

    // A stream marked `inactive` carries nothing in either direction. Treating it as a working
    // call means holding a media session open for audio that will never come.
    if audio.direction() == Direction::Inactive {
        return Err(Error::NoCommonCodec);
    }

    let address = sdp.address_for(audio).ok_or(Error::NoCommonCodec)?;

    // The first format both sides can carry. The list is already in the offerer's preference
    // order, so the first playable one is the one to use. Playable is judged by what the
    // format's rtpmap says, never by a dynamic number alone — which is also the reason
    // `Codec::from_payload_type` deliberately never returns Opus: 111 is Opus here only because
    // this description said so.
    //
    // `sipx_sdp::answer` decides the same question when it builds the answer that goes on the
    // wire, and the two *must* agree: this settles what the session sends, and the answer is what
    // the far end was told to expect. They now agree by construction — both ask
    // `sipx_sdp::rtpmap::same_format` whether an offered rtpmap names a format this side has, so
    // there is one rule rather than two readings of it (`M-31`). What is left of the difference is
    // deliberate and one-directional: the answer also names `telephone-event`, which is not a
    // codec to settle on. `the_answer_and_the_negotiated_codec_agree` holds the agreement over a
    // table of offers, so this paragraph is a claim with a test under it rather than a hope.
    //
    // `carries` is part of the search and not a test applied to its result. Rejecting afterwards
    // would stop at the offerer's first choice and refuse the whole description if that one
    // format is outside our set — so an Opus-first offer reaching a G.711 call would come back
    // `NoCommonCodec` while the answer this side builds happily names the PCMU further down the
    // same list.
    let (codec, payload_type, clock_rate) = audio
        .formats
        .iter()
        .find_map(|format| {
            codec_of(audio, format)
                .filter(|(codec, _, clock_rate)| codecs.carries_format(*codec, *clock_rate))
        })
        .ok_or(Error::NoCommonCodec)?;

    Ok(Negotiated {
        remote: SocketAddr::new(address, audio.port),
        codec,
        clock_rate,
        payload_type,
        receive_payload_type: payload_type,
        dtmf: telephone_event_payload_type(audio),
        // On the answering side this is the offer's request, which sipx accepts. On the offering
        // side `settle_answer` additionally requires that this side actually offered the flag.
        rtcp_mode: if audio.rtcp_mux() {
            sipx_sdp::RtcpMode::Mux
        } else {
            sipx_sdp::RtcpMode::Separate
        },
    })
}

/// The codec a format names, and the payload type to put on the wire for it.
///
/// A format with an rtpmap is matched by the map: RFC 8866 §6.6 makes it authoritative even
/// for a static number, which is how an offer of `8` meaning iLBC is not read as PCMA. The
/// number is then *dynamic in meaning* — the map could have hung any name on it — so it goes
/// home with the codec rather than being reassumed from [`Codec::payload_type`]. Only a bare
/// static type, with no map at all, is matched by number.
fn codec_of(audio: &sipx_sdp::MediaDescription, format: &str) -> Option<(Codec, Option<u8>, u32)> {
    let payload = format.parse::<u8>().ok()?;
    if let Some(rtpmap) = audio.rtpmap(format) {
        return codec_format(rtpmap).map(|(codec, clock_rate)| (codec, Some(payload), clock_rate));
    }
    Codec::from_payload_type(payload).map(|codec| (codec, None, codec.clock_rate()))
}

/// The codec and RTP clock an rtpmap names.
///
/// L16 is special only in having more than one supported rate. Its name and mono channel count
/// are still parsed by SDP's shared format reader; policy decides below whether the exact rate
/// was offered locally.
fn codec_format(rtpmap: &str) -> Option<(Codec, u32)> {
    let parsed = sipx_sdp::rtpmap::Rtpmap::parse(rtpmap).ok()?;
    if parsed.encoding().eq_ignore_ascii_case("L16") && parsed.channels() == 1 {
        return Some((Codec::L16, parsed.clock_rate()));
    }
    codec_named(rtpmap).map(|codec| (codec, codec.clock_rate()))
}

/// The codec an rtpmap value names, if it is one we carry.
///
/// **The matching rule is not written here.** [`sipx_sdp::rtpmap::same_format`] decides whether two
/// `a=rtpmap` values name the same format, and this asks it once per codec sipx can run, against
/// the value that codec is offered with. It used to be written out a second time in this function,
/// with the clock rate parsed to a `u32` where [`sipx_sdp::answer`] compared the same field as
/// text — so the answer on the wire and the codec the session was built with could name different
/// formats for one offer (`M-31`).
///
/// `sipx-sdp` is the authority and not this crate, because the dependency only runs one way:
/// [`sipx_sdp::answer`] builds the answer sipx sends and cannot call up into `sipx-call`, so the
/// only arrangement in which one implementation serves both is the lower crate holding it. What
/// stays here is the half `sipx-sdp` must not learn — which rtpmaps sipx has a codec for, and
/// which codecs the application selected.
///
/// The order of the search does not matter: the values in [`carried`] are distinct formats, so an
/// rtpmap matches at most one of them. Preference order is the *offerer's*, and it is applied by
/// [`negotiated`] walking `m=`'s format list.
fn codec_named(rtpmap: &str) -> Option<Codec> {
    carried()
        .iter()
        .copied()
        .find(|&codec| sipx_sdp::rtpmap::same_format(rtpmap, offered_rtpmap(codec)))
}

/// Every codec sipx can run, and can therefore read out of an rtpmap.
///
/// Omitting a new [`Codec`] variant here means it is simply never named by an offer, which is the
/// safe direction to fail in — the same reasoning as [`Codecs::carries`]. The exhaustive match in
/// [`offered_rtpmap`] is what forces someone to decide.
fn carried() -> &'static [Codec] {
    &[
        Codec::Pcmu,
        Codec::Pcma,
        Codec::G722,
        Codec::L16,
        #[cfg(feature = "opus")]
        Codec::Opus,
    ]
}

/// The `a=rtpmap` value sipx offers a codec with.
///
/// The same strings [`sipx_sdp::Capabilities::g711`] and [`sipx_sdp::Capabilities::with_opus`] put
/// on the wire, and they have to be: a codec whose value here disagreed with the one offered would
/// be a codec negotiation settles on and no answer ever names, which is the whole of `M-31`.
/// `the_answer_and_the_negotiated_codec_agree` is what holds the two together, rather than a
/// comment asking them to match.
///
/// RFC 7587 §7 fixes Opus's RTP clock at 48000 and its rtpmap channel count at 2 whatever the
/// audio actually is, so `opus/16000` is nothing we have however it is numbered. G.722's rtpmap
/// spelling is `G722/8000` even though the audio is 16 kHz — RFC 3551 §4.5.2 preserves the
/// historical clock on the wire, and a `G722/16000` spelling would name a format nobody has.
const fn offered_rtpmap(codec: Codec) -> &'static str {
    match codec {
        Codec::Pcmu => "PCMU/8000",
        Codec::Pcma => "PCMA/8000",
        Codec::G722 => "G722/8000",
        Codec::L16 => "L16/44100/1",
        #[cfg(feature = "opus")]
        Codec::Opus => "opus/48000/2",
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
pub(crate) mod tests {
    use std::fmt::Write as _;
    use std::net::IpAddr;

    use super::*;

    /// An audio description with the given formats and rtpmaps, as a peer would send it.
    pub(crate) fn offered(formats: &str, rtpmaps: &[&str]) -> SessionDescription {
        let mut body = format!(
            "v=0\r\n\
             o=- 1 1 IN IP4 192.0.2.1\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP {formats}\r\n"
        );
        for rtpmap in rtpmaps {
            let _ = write!(body, "a=rtpmap:{rtpmap}\r\n");
        }
        sipx_sdp::parse(&body).expect("a description this test wrote")
    }

    /// M-44, and the RFC 3551 §6/§4.5.2 pair of facts in one place: static payload type 9 with
    /// **no** `a=rtpmap` line is G.722 — the field-reported failure is a stack rejecting exactly
    /// that offer — and the negotiated RTP clock is 8000 even though the audio is 16 kHz.
    #[test]
    fn a_bare_static_9_offer_negotiates_g722() {
        let bare = offered("9 0", &["0 PCMU/8000"]);
        let settled = negotiated(&bare, Codecs::G722).expect("bare static 9 is G.722");
        assert_eq!(settled.codec, Codec::G722);
        assert_eq!(settled.wire_payload_type(), 9);
        assert_eq!(settled.clock_rate, 8_000, "the RFC 3551 §4.5.2 clock");

        // And identically when the offer writes the rtpmap out.
        let mapped = offered("9 0", &["9 G722/8000", "0 PCMU/8000"]);
        let settled = negotiated(&mapped, Codecs::G722).expect("mapped 9 is G.722 too");
        assert_eq!(settled.codec, Codec::G722);
        assert_eq!(settled.wire_payload_type(), 9);
        assert_eq!(settled.clock_rate, 8_000);

        // A G722/16000 spelling names a format nobody has (the clock is part of the format's
        // identity), so the offer falls through to the PCMU beside it.
        let wrong_clock = offered("9 0", &["9 G722/16000", "0 PCMU/8000"]);
        let settled = negotiated(&wrong_clock, Codecs::G722).expect("PCMU remains usable");
        assert_eq!(settled.codec, Codec::Pcmu);
    }

    /// The default is the G.711 pair, in every build. The `opus` feature adds a variant to
    /// [`Codecs`]; it must never move which one `Default` produces, or turning the feature on to
    /// get the *option* of Opus would silently change what every existing call offers.
    #[test]
    fn the_default_codec_set_is_g711() {
        assert_eq!(Codecs::default(), Codecs::G711);
        let capabilities = Codecs::default().capabilities("192.0.2.9".parse().unwrap(), 40000);
        assert!(
            !capabilities
                .rtpmaps
                .iter()
                .any(|(_, value)| value.to_ascii_lowercase().contains("opus")),
            "the default offer names no Opus: {:?}",
            capabilities.rtpmaps
        );
    }

    /// RFC 8866 §6.6 makes the rtpmap authoritative even over a static number. This is the rule
    /// that lets an Opus offer arrive at all — 111 means Opus only because the description said
    /// so — and the same rule refuses to read an offer of `8` remapped to something else as PCMA.
    #[test]
    fn a_format_is_read_from_its_rtpmap_and_not_from_its_number() {
        let remapped = offered("8 0", &["8 iLBC/8000", "0 PCMU/8000"]);
        let settled = negotiated(&remapped, Codecs::G711).expect("PCMU is common");
        assert_eq!(settled.codec, Codec::Pcmu);
        assert_eq!(
            settled.payload_type,
            Some(0),
            "the number the far end assigned travels with the codec"
        );
    }

    /// A bare static type with no rtpmap at all is the one case matched by number, which is what
    /// keeps every G.711-only peer that sends `m=audio … 0 8` and nothing else working.
    #[test]
    fn a_bare_static_type_is_still_matched_by_number() {
        let settled = negotiated(&offered("0", &[]), Codecs::G711).expect("PCMU is static");
        assert_eq!(settled.codec, Codec::Pcmu);
        assert_eq!(
            settled.payload_type, None,
            "nothing named it, so nothing overrides `Codec::payload_type`"
        );
    }

    /// M-43: RFC 3551 assigns mono L16 at 44.1 kHz to static payload 11. The adjacent payload
    /// 10 is stereo, which sipx's mono media surface deliberately does not claim.
    #[test]
    fn l16_static_payload_is_mono_at_forty_four_point_one_kilohertz() {
        let l16 = Codecs::ordered(&[crate::CodecPreference::L16]).expect("L16 selection");
        let settled = negotiated(&offered("11", &[]), l16).expect("static mono L16");
        assert_eq!(settled.codec, Codec::L16);
        assert_eq!(settled.payload_type, None);
        assert_eq!(settled.clock_rate, 44_100);
        assert!(matches!(
            negotiated(&offered("10", &[]), l16),
            Err(Error::NoCommonCodec)
        ));
    }

    /// M-43: an L16 rate outside the static assignment is identified by rtpmap and its dynamic
    /// payload number travels with the session. Only rates this policy actually offers settle.
    #[test]
    fn l16_dynamic_payload_retains_its_explicit_clock_rate() {
        let l16 = Codecs::ordered(&[crate::CodecPreference::L16]).expect("L16 selection");
        let settled =
            negotiated(&offered("110", &["110 L16/8000/1"]), l16).expect("dynamic mono L16");
        assert_eq!(settled.codec, Codec::L16);
        assert_eq!(settled.payload_type, Some(110));
        assert_eq!(settled.clock_rate, 8_000);

        assert!(
            matches!(
                negotiated(&offered("96", &["96 L16/16000/1"]), l16),
                Err(Error::NoCommonCodec)
            ),
            "an unoffered rate is not silently accepted"
        );
        assert!(
            matches!(
                negotiated(&offered("96", &["96 L16/8000/2"]), l16),
                Err(Error::NoCommonCodec)
            ),
            "the PCM API is mono"
        );
    }

    /// Each SDP description owns its dynamic assignment. An 8 kHz L16 answer may send on 110
    /// while receiving on the 96 this side offered, with one shared negotiated clock.
    #[test]
    fn l16_answer_keeps_directional_dynamic_payload_assignments() {
        let l16 = Codecs::L16;
        let capabilities = l16.capabilities("192.0.2.9".parse().expect("address"), 40_000);
        let answer = offered("110", &["110 L16/8000/1"]);
        let settled = settle_answer(&capabilities, &answer, l16).expect("dynamic L16 answer");
        assert_eq!(settled.negotiated.codec, Codec::L16);
        assert_eq!(settled.negotiated.clock_rate, 8_000);
        assert_eq!(settled.negotiated.wire_payload_type(), 110);
        assert_eq!(settled.negotiated.receive_wire_payload_type(), 96);
    }

    /// The clock rate and channel count are part of a format's identity (RFC 8866 §6.6), so a
    /// name sipx knows at a rate it does not is not a match.
    #[test]
    fn a_known_name_at_an_unknown_clock_rate_is_not_a_match() {
        assert_eq!(codec_named("PCMU/16000"), None);
        assert_eq!(codec_named("opus/16000/2"), None);
        assert_eq!(codec_named("PCMU/8000"), Some(Codec::Pcmu));
        assert_eq!(codec_named("pcma/8000"), Some(Codec::Pcma));
    }

    /// The default build has no Opus, so an offer of it is not a codec that build can carry —
    /// and the offer is answered from what *is* common rather than refused. This is the promise
    /// the `opus` feature is off by default in order to make: `tests/opus.rs` is gated on the
    /// feature and cannot assert anything about the build that lacks it.
    #[cfg(not(feature = "opus"))]
    #[test]
    fn a_default_build_does_not_carry_an_offered_opus() {
        assert_eq!(codec_named("opus/48000/2"), None);
        let opus_first = offered("111 0", &["111 opus/48000/2", "0 PCMU/8000"]);
        let settled = negotiated(&opus_first, Codecs::G711).expect("G.711 is still offered");
        assert_eq!(settled.codec, Codec::Pcmu, "the first format sipx carries");
    }

    /// Selecting a set is what puts a codec on the table, and negotiation may not step outside
    /// it. An Opus offer answered from [`Codecs::G711`] settles on G.711 — not because Opus is
    /// absent from the build, but because the answer this side builds never named it, and a
    /// session started on a codec no answer named sends packets the far end cannot place.
    #[cfg(feature = "opus")]
    #[test]
    fn negotiation_does_not_settle_outside_the_selected_set() {
        assert_eq!(codec_named("opus/48000/2"), Some(Codec::Opus));
        let opus_first = offered("111 0", &["111 opus/48000/2", "0 PCMU/8000"]);

        let from_g711 = negotiated(&opus_first, Codecs::G711).expect("G.711 is still offered");
        assert_eq!(from_g711.codec, Codec::Pcmu);

        let from_opus = negotiated(&opus_first, Codecs::Opus).expect("Opus is on the table");
        assert_eq!(from_opus.codec, Codec::Opus);
        assert_eq!(
            from_opus.payload_type,
            Some(111),
            "on the number this offer assigned, not on a number 111 means by itself"
        );
    }

    /// A peer may spell a static type either way — `m=audio … 0` alone, or the same thing with a
    /// redundant `a=rtpmap:0 PCMU/8000` — and RFC 8866 §6.6 allows both for the same codec.
    ///
    /// So moving between the two spellings is not a *change*, and [`Call::move_media_if_changed`]
    /// must not rebuild the session for it: rebuilding costs an audible gap, and some peers
    /// re-INVITE every thirty seconds as a keep-alive. `negotiated` does record the difference —
    /// `Some(0)` against `None`, which is a true fact about what the description said — so the
    /// comparison is on [`Negotiated::wire_payload_type`], where the two collapse to the one byte
    /// that actually goes out.
    #[test]
    fn a_redundant_rtpmap_for_a_static_type_is_not_a_change() {
        let mapped = negotiated(&offered("0", &["0 PCMU/8000"]), Codecs::G711).expect("PCMU");
        let bare = negotiated(&offered("0", &[]), Codecs::G711).expect("PCMU");

        assert_eq!(mapped.codec, bare.codec);
        assert_eq!(mapped.payload_type, Some(0), "the rtpmap named it");
        assert_eq!(bare.payload_type, None, "nothing named it");
        assert_eq!(
            mapped.wire_payload_type(),
            bare.wire_payload_type(),
            "the same byte goes on the wire either way, so the session must not move",
        );
    }

    /// S-36 / RFC 3264 §6.1: the offer and answer may assign different dynamic numbers to the
    /// same format. The answer's number is what we send; the offer's remains what we receive.
    #[test]
    fn an_asymmetric_answer_keeps_each_directions_payload_number() {
        let address = "192.0.2.9".parse().expect("address");
        let mut capabilities = Capabilities::g711(address, 40_000);
        capabilities.audio_formats = vec!["111".to_owned()];
        capabilities.rtpmaps = vec![("111".to_owned(), "PCMU/8000".to_owned())];
        let answer = offered("96", &["96 PCMU/8000"]);

        let settled = settle_answer(&capabilities, &answer, Codecs::G711).expect("same format");
        assert_eq!(settled.negotiated.wire_payload_type(), 96);
        assert_eq!(settled.negotiated.receive_wire_payload_type(), 111);
        let config = settled.media_config();
        assert_eq!(
            config.wire_payload_type(),
            96,
            "send with the peer's number"
        );
        assert_eq!(
            config.receive_wire_payload_type(),
            111,
            "receive with our number"
        );
    }

    /// An *answer* naming a codec outside the selected set is refused, so nothing keys a session
    /// on it.
    ///
    /// Pinned separately from `negotiated` because of where the refusal lands rather than what it
    /// returns. It is a failure mode `M-30` adds to `settle_answer`, which had no codec opinion
    /// before; on this branch an early answer that trips it is swallowed by
    /// `Dialing::adopt_early_answer`, but that function propagates on `main` after `S-25`, so once
    /// the two are merged this same refusal ends the invitation over a CANCEL. That is a call
    /// termination neither branch produces alone, which is why the precondition is worth holding
    /// here rather than waiting for the merge to discover it.
    ///
    /// True in both feature configurations for two different reasons: with `opus` off no rtpmap can
    /// name Opus at all, and with it on `Codecs::G711` does not carry it.
    #[test]
    fn an_answer_outside_the_selected_set_is_refused() {
        let opus_only = offered("111", &["111 opus/48000/2"]);
        let capabilities =
            Capabilities::g711("127.0.0.1".parse().expect("loopback address"), 40_000);
        assert!(matches!(
            settle_answer(&capabilities, &opus_only, Codecs::G711),
            Err(Error::NoCommonCodec)
        ));
    }

    #[test]
    fn the_answer_settles_mux_or_the_separate_port_fallback_without_a_retry() {
        let capabilities =
            Capabilities::g711("127.0.0.1".parse().expect("loopback address"), 40_000)
                .with_rtcp_mux();
        let separate_answer = offered("0", &["0 PCMU/8000"]);
        let separate = settle_answer(&capabilities, &separate_answer, Codecs::G711)
            .expect("the answer remains usable");
        assert_eq!(separate.negotiated.rtcp_mode, sipx_sdp::RtcpMode::Separate);

        let mut mux_answer = separate_answer;
        mux_answer
            .media
            .first_mut()
            .expect("audio answer")
            .attributes
            .push(sipx_sdp::Attribute::flag("rtcp-mux"));
        let mux = settle_answer(&capabilities, &mux_answer, Codecs::G711)
            .expect("the muxed answer remains usable");
        assert_eq!(mux.negotiated.rtcp_mode, sipx_sdp::RtcpMode::Mux);

        let not_offered =
            Capabilities::g711("127.0.0.1".parse().expect("loopback address"), 40_000);
        let unasked = settle_answer(&not_offered, &mux_answer, Codecs::G711)
            .expect("an unasked attribute does not break the answer");
        assert_eq!(
            unasked.negotiated.rtcp_mode,
            sipx_sdp::RtcpMode::Separate,
            "an answer cannot negotiate a feature that was not offered"
        );
    }

    /// A running one-port session cannot accept an in-dialog offer that drops mux while retaining
    /// its old socket owner. The same typed guard is used before inbound state is applied.
    #[test]
    fn an_inbound_reoffer_cannot_remove_the_running_mux_mode() {
        let offered_without_mux = offered("0", &["0 PCMU/8000"]);
        let answered_without_mux = offered("0", &["0 PCMU/8000"]);
        let proposed = exchanged_rtcp_mode(&offered_without_mux, &answered_without_mux);

        assert!(matches!(
            preserve_rtcp_mode(sipx_sdp::RtcpMode::Mux, proposed),
            Err(Error::RtcpModeChange {
                current: sipx_sdp::RtcpMode::Mux,
                proposed: sipx_sdp::RtcpMode::Separate,
            })
        ));
    }

    /// The outbound mirror: omission in an answer to a later offer is an explicit failure and
    /// leaves the established mux session in place instead of binding an unadvertised replacement.
    #[test]
    fn an_outbound_reoffer_answer_cannot_remove_the_running_mux_mode() {
        let answer_without_mux = offered("0", &["0 PCMU/8000"]);
        let renegotiated = negotiated(&answer_without_mux, Codecs::G711).expect("PCMU answer");

        assert!(matches!(
            preserve_rtcp_mode(sipx_sdp::RtcpMode::Mux, renegotiated.rtcp_mode),
            Err(Error::RtcpModeChange {
                current: sipx_sdp::RtcpMode::Mux,
                proposed: sipx_sdp::RtcpMode::Separate,
            })
        ));
    }

    /// An offer with nothing sipx carries is refused rather than answered on a guess.
    #[test]
    fn an_offer_of_nothing_we_carry_has_no_common_codec() {
        let g729 = offered("18", &["18 G729/8000"]);
        assert!(matches!(
            negotiated(&g729, Codecs::G711),
            Err(Error::NoCommonCodec)
        ));
    }

    /// One row of the agreement table: an offer, and the set the application selected.
    ///
    /// The property is in [`tests::the_answer_and_the_negotiated_codec_agree`]. The rows exist so
    /// it is held against a *class* of rtpmap spellings rather than the one spelling that happened
    /// to be found — `M-31` was filed because a fix aimed at `08000` alone would leave the shape
    /// in place.
    struct Agreement {
        /// Why this row is in the table. Quoted in every failure, because a table-driven
        /// assertion that only prints the values makes the reader guess what was being tested.
        why: &'static str,
        /// The `m=audio` format list, in the offerer's preference order.
        formats: &'static str,
        /// The offer's `a=rtpmap` attribute values.
        rtpmaps: &'static [&'static str],
        /// The set the application selected for this call.
        codecs: Codecs,
    }

    /// The offers the agreement must hold over, in every build.
    ///
    /// Derived from `docs/specs/sdp-format-identity.md` §4.4's vectors. A `const` table rather than
    /// a function that builds one: it is data, and a hundred lines of data is not a hundred lines
    /// of control flow for anyone reading it — or for `clippy::too_many_lines`.
    const AGREEMENT_TABLE: &[Agreement] = &[
        Agreement {
            why: "a clock rate with a leading zero is the same rate — `08000` and `8000` are \
                      numerically equal and textually different, which is the split M-31 was \
                      filed for",
            formats: "0 8",
            rtpmaps: &["0 PCMU/08000", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "the same split in the *channel* field, so a fix aimed at the clock rate \
                      alone does not close the story",
            formats: "0 8",
            rtpmaps: &["0 PCMU/8000/01", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an offer that puts a codec sipx does not carry first: both rules must skip \
                      it and settle further down the list, not refuse the stream",
            formats: "18 0",
            rtpmaps: &["18 G729/8000", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a dynamic number carrying a codec sipx does have — 96 means PCMU here only \
                      because this offer said so (RFC 8866 §6.6), and both rules must read the \
                      map rather than the number",
            formats: "96 0",
            rtpmaps: &["96 PCMU/8000", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a bare static type, the one case with no rtpmap for either rule to read",
            formats: "0",
            rtpmaps: &[],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "mono spelled out where RFC 8866 §6.6 would have let it be implied",
            formats: "0",
            rtpmaps: &["0 PCMU/8000/1"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "stereo G.711 is a different format from mono G.711, and neither rule may \
                      settle on it",
            formats: "0 8",
            rtpmaps: &["0 PCMU/8000/2", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a signed clock rate is not a decimal digit string, so it identifies nothing for \
                  either rule. The *third* witness, and the one that was not predicted: \
                  `u32::from_str` accepts a leading `+`, so the parsing rule read `+8000` as 8000 \
                  while the textual one did not — the same split as a leading zero, arrived at from \
                  the other side. It is why the digits are checked in `sipx-sdp` rather than left \
                  to `from_str`, and note the single rule resolves it the *opposite* way from a \
                  leading zero: both callers decline it, and both settle on PCMA below",
            formats: "0 8",
            rtpmaps: &["0 PCMU/+8000", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a clock rate that overflows u32 — hostile input, and a non-match for both \
                      rules rather than a panic in either",
            formats: "0 8",
            rtpmaps: &["0 PCMU/99999999999999", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an rtpmap with no clock rate at all identifies nothing",
            formats: "0 8",
            rtpmaps: &["0 PCMU", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an empty clock rate is not zero and is not 8000",
            formats: "0 8",
            rtpmaps: &["0 PCMU/", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "whitespace inside the value: a rate neither rule can read, and both must \
                      fail to read it the same way",
            formats: "0 8",
            rtpmaps: &["0 PCMU/ 8000", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a fourth field is outside RFC 8866 §6.6's grammar, so the value identifies \
                      nothing — and must do so for both rules rather than one silently ignoring it",
            formats: "0 8",
            rtpmaps: &["0 PCMU/8000/1/9", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an Opus-first offer reaching a call that selected G.711: the M-30 case, and \
                      true in both feature configurations — with `opus` off no rtpmap can name it, \
                      with it on the set does not carry it",
            formats: "111 0",
            rtpmaps: &["111 opus/48000/2", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a dynamic number with no rtpmap is uninterpretable whatever the number, so \
                      the stream is refused rather than guessed at",
            formats: "111",
            rtpmaps: &[],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a stream offering only telephone-event is not a call: the answer rejects it \
                      and negotiation must refuse it too",
            formats: "101",
            rtpmaps: &["101 telephone-event/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an offer of nothing sipx carries at all — both rules refuse, and the \
                      agreement holds on the refusing side as well",
            formats: "18",
            rtpmaps: &["18 G729/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "M-44: bare static 9 with no rtpmap — the field-reported offer a stack \
                      must not reject when G.722 is selected",
            formats: "9 0",
            rtpmaps: &["0 PCMU/8000"],
            codecs: Codecs::G722,
        },
        Agreement {
            why: "M-44: the same offer with the rtpmap written out, and the historical \
                      G722/8000 clock spelling",
            formats: "9 0",
            rtpmaps: &["9 G722/8000", "0 PCMU/8000"],
            codecs: Codecs::G722,
        },
        Agreement {
            why: "M-44: G.722 on a dynamic number is matched by the rtpmap, not the number",
            formats: "96",
            rtpmaps: &["96 G722/8000"],
            codecs: Codecs::G722,
        },
        Agreement {
            why: "M-44: 9 remapped to another encoding is not taken on the number",
            formats: "9",
            rtpmaps: &["9 G729/8000"],
            codecs: Codecs::G722,
        },
        Agreement {
            why: "M-44: a G.722 offer reaching a call that selected only G.711 settles on \
                      neither more nor less than the selection allows",
            formats: "9 0",
            rtpmaps: &["9 G722/8000", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
    ];

    /// The rows that only exist when the `opus` feature is on, because [`Codecs::Opus`] does.
    ///
    /// Empty in the default build rather than absent, so the test body has no `cfg` in it and the
    /// two configurations run the same code over different data.
    #[cfg(feature = "opus")]
    const OPUS_AGREEMENT_TABLE: &[Agreement] = &[
        Agreement {
            why: "Opus on the set that carries it, on the number this offer assigned",
            formats: "111 0",
            rtpmaps: &["111 opus/48000/2", "0 PCMU/8000"],
            codecs: Codecs::Opus,
        },
        Agreement {
            why: "the leading-zero split on Opus's own clock rate, so the class is closed in the \
                  gated path too and not only for G.711",
            formats: "111 0",
            rtpmaps: &["111 opus/048000/2", "0 PCMU/8000"],
            codecs: Codecs::Opus,
        },
        Agreement {
            why: "Opus at a rate RFC 7587 §7 does not assign is nothing sipx has, whatever number \
                  is beside it",
            formats: "111 0",
            rtpmaps: &["111 opus/16000/2", "0 PCMU/8000"],
            codecs: Codecs::Opus,
        },
    ];

    /// No Opus in this build, so no Opus rows. See [`OPUS_AGREEMENT_TABLE`].
    #[cfg(not(feature = "opus"))]
    const OPUS_AGREEMENT_TABLE: &[Agreement] = &[];

    /// The answer sipx puts on the wire and the codec it configures the media session with must
    /// name the same format. **`M-31`'s failing-first test.**
    ///
    /// This is the assertion that fails while the two rules disagree: with the answer comparing an
    /// rtpmap clock rate as text and `codec_named` parsing it to `u32`, an offer of
    /// `a=rtpmap:0 PCMU/08000` settles on `Pcmu` at payload type 0 while the answer names only
    /// `8`. sipx would then send µ-law on a number the answer never offered *and* decode the
    /// peer's PCMA through a µ-law session — audible garbage rather than silence, with nothing in
    /// the stack reporting an error.
    ///
    /// The property is a biconditional, not a one-way check, because both halves are reachable
    /// defects: a codec the answer never named is a session the far end cannot read, and a stream
    /// the answer accepted while negotiation refused it is a call that fails after the 200 OK went
    /// out. `wire_payload_type` is the value compared because that is the byte that leaves —
    /// `Some(0)` and `None` are two descriptions of the same PCMU.
    #[test]
    fn the_answer_and_the_negotiated_codec_agree() {
        let local: IpAddr = "192.0.2.9".parse().expect("a literal address");

        for row in AGREEMENT_TABLE.iter().chain(OPUS_AGREEMENT_TABLE) {
            let offer = offered(row.formats, row.rtpmaps);
            let answered = sipx_sdp::answer(&offer, &row.codecs.capabilities(local, 40000));
            let audio = answered
                .media
                .iter()
                .find(|stream| stream.media == "audio")
                .expect("the answer has one m= line per offered stream");

            match negotiated(&offer, row.codecs) {
                Ok(settled) => {
                    assert!(
                        !audio.is_rejected(),
                        "{}: negotiation settled on {:?} while the answer rejected the stream",
                        row.why,
                        settled.codec,
                    );
                    let wire = settled.wire_payload_type().to_string();
                    assert!(
                        audio.formats.contains(&wire),
                        "{}: negotiation settled on {:?} at payload type {wire}, which the answer \
                         never named ({:?})",
                        row.why,
                        settled.codec,
                        audio.formats,
                    );
                }
                Err(error) => {
                    assert!(
                        audio.is_rejected(),
                        "{}: negotiation refused the stream ({error}) while the answer accepted it \
                         with formats {:?}",
                        row.why,
                        audio.formats,
                    );
                }
            }
        }
    }
}
