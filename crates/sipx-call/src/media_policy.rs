//! Application choices for an initial call's media.
//!
//! This module names policy only. SDP construction, offer/answer matching, ICE gathering and
//! media startup stay in [`crate::call`], so a command-line caller and a library caller cannot
//! acquire two implementations of negotiation.

use std::net::{IpAddr, SocketAddr};

use sipx_media::Codec;
use sipx_media::ice::Gathering;
use sipx_sdp::Capabilities;
use sipx_sdp::ice::Credentials as IceCredentials;

use crate::call::token;
use crate::error::{Error, Result};

/// One codec an application may put in its ordered preference list.
///
/// `Opus` remains a value in builds without the feature so configuration can fail with a typed
/// setup error instead of treating a known codec name as an unknown string. It cannot enter a
/// [`Codecs`] value in that build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CodecPreference {
    /// G.711 µ-law (RFC 3551 §4.5.14).
    Pcmu,
    /// G.711 A-law (RFC 3551 §4.5.14).
    Pcma,
    /// G.722 wideband, static payload type 9 (RFC 3551 §4.5.2).
    G722,
    /// Opus (RFC 6716, carried per RFC 7587).
    Opus,
    /// Mono signed linear PCM (RFC 3551 §4.5.11).
    L16,
}

impl CodecPreference {
    /// The stable configuration and result token.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pcmu => "pcmu",
            Self::Pcma => "pcma",
            Self::G722 => "g722",
            Self::Opus => "opus",
            Self::L16 => "l16",
        }
    }
}

/// Why an ordered codec selection cannot be honoured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CodecSelectionError {
    /// At least one audio codec must be offered.
    #[error("at least one codec must be selected")]
    Empty,
    /// Repeating a codec does not express a second preference.
    #[error("codec `{0}` was selected more than once")]
    Duplicate(&'static str),
    /// The value is known, but this build cannot run it.
    #[error("codec `opus` requires a build with the `opus` feature")]
    OpusUnavailable,
}

/// Which codecs a call offers and accepts, in preference order (`M-30`, `P-9`).
///
/// The default is PCMU then PCMA. An explicit selection is exactly the ordered set supplied by
/// the application; negotiation may choose no codec outside it. RFC 4733 telephone events remain
/// alongside every non-empty audio set and are not themselves an audio codec.
///
/// # Where G.722 sits, and why (`M-44`)
///
/// In every named selection that includes it, G.722 is placed **below Opus and above G.711**,
/// and the rule is stated here rather than implied by list order: Opus is the better wideband
/// codec wherever both ends have it — lower bitrate for the same band, loss concealment, and a
/// negotiation that cannot be half-taken — and wideband G.722 beats narrowband G.711 wherever
/// they do not. An explicit [`Codecs::ordered`] list is the application's own and is used
/// exactly as given; nothing reorders it toward this rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Codecs {
    ordered: [Option<CodecPreference>; 5],
}

impl Default for Codecs {
    fn default() -> Self {
        Self::G711
    }
}

impl Codecs {
    /// The compatibility default: PCMU, then PCMA.
    pub const G711: Self = Self {
        ordered: [
            Some(CodecPreference::Pcmu),
            Some(CodecPreference::Pcma),
            None,
            None,
            None,
        ],
    };

    /// Opus first, followed by the compatibility G.711 pair.
    #[cfg(feature = "opus")]
    #[allow(
        non_upper_case_globals,
        reason = "preserves the pre-P-9 public spelling"
    )]
    pub const Opus: Self = Self {
        ordered: [
            Some(CodecPreference::Opus),
            Some(CodecPreference::Pcmu),
            Some(CodecPreference::Pcma),
            None,
            None,
        ],
    };

    /// G.722 first, followed by the compatibility G.711 pair.
    ///
    /// The type documentation states the placement rule: below Opus, above G.711. This named
    /// set has no Opus entry only because it is constructible in every build; a wideband-first
    /// application with the `opus` feature wants `Codecs::ordered(&[Opus, G722, Pcmu, Pcma])`.
    pub const G722: Self = Self {
        ordered: [
            Some(CodecPreference::G722),
            Some(CodecPreference::Pcmu),
            Some(CodecPreference::Pcma),
            None,
            None,
        ],
    };

    /// Mono L16, in its static 44.1 kHz and dynamic 8 kHz forms.
    pub const L16: Self = Self {
        ordered: [Some(CodecPreference::L16), None, None, None, None],
    };

    /// Validate an application's exact ordered preference list.
    ///
    /// # Errors
    ///
    /// Empty lists, duplicates and Opus in a build without Opus are refused before a call can
    /// bind media or send signalling.
    pub fn ordered(
        preferences: &[CodecPreference],
    ) -> std::result::Result<Self, CodecSelectionError> {
        if preferences.is_empty() {
            return Err(CodecSelectionError::Empty);
        }
        let mut ordered = [None; 5];
        for (slot, preference) in ordered.iter_mut().zip(preferences.iter().copied()) {
            if preferences
                .iter()
                .filter(|candidate| **candidate == preference)
                .count()
                > 1
            {
                return Err(CodecSelectionError::Duplicate(preference.name()));
            }
            if preference == CodecPreference::Opus && !cfg!(feature = "opus") {
                return Err(CodecSelectionError::OpusUnavailable);
            }
            *slot = Some(preference);
        }
        // There are exactly five closed values, so a longer duplicate-free list cannot exist.
        Ok(Self { ordered })
    }

    /// The selected codecs, in preference order.
    pub fn preferences(self) -> impl Iterator<Item = CodecPreference> {
        self.ordered.into_iter().flatten()
    }

    /// What this side offers or answers with.
    pub(crate) fn capabilities(self, address: IpAddr, audio_port: u16) -> Capabilities {
        let mut capabilities = Capabilities::g711(address, audio_port);
        capabilities.audio_formats.clear();
        capabilities.rtpmaps.clear();
        for preference in self.preferences() {
            match preference {
                CodecPreference::Pcmu => {
                    capabilities.audio_formats.push("0".to_owned());
                    capabilities
                        .rtpmaps
                        .push(("0".to_owned(), "PCMU/8000".to_owned()));
                }
                CodecPreference::Pcma => {
                    capabilities.audio_formats.push("8".to_owned());
                    capabilities
                        .rtpmaps
                        .push(("8".to_owned(), "PCMA/8000".to_owned()));
                }
                CodecPreference::G722 => {
                    // RFC 3551 §4.5.2: the rtpmap says 8000 although the audio is 16 kHz —
                    // the historical clock is part of the format's identity on the wire.
                    capabilities.audio_formats.push("9".to_owned());
                    capabilities
                        .rtpmaps
                        .push(("9".to_owned(), "G722/8000".to_owned()));
                }
                CodecPreference::Opus => {
                    // `ordered` refuses this value when the codec implementation is absent.
                    capabilities.audio_formats.push("111".to_owned());
                    capabilities
                        .rtpmaps
                        .push(("111".to_owned(), "opus/48000/2".to_owned()));
                }
                CodecPreference::L16 => {
                    // RFC 3551 §6 assigns mono 44.1 kHz L16 statically to 11. The 8 kHz form is
                    // a different format and therefore receives a dynamic number.
                    capabilities.audio_formats.push("11".to_owned());
                    capabilities
                        .rtpmaps
                        .push(("11".to_owned(), "L16/44100/1".to_owned()));
                    capabilities.audio_formats.push("96".to_owned());
                    capabilities
                        .rtpmaps
                        .push(("96".to_owned(), "L16/8000/1".to_owned()));
                }
            }
        }
        capabilities.audio_formats.push("101".to_owned());
        capabilities
            .rtpmaps
            .push(("101".to_owned(), "telephone-event/8000".to_owned()));
        capabilities
    }

    /// Whether this set carries a codec, so negotiation cannot settle outside the selection.
    pub(crate) fn carries(self, codec: Codec) -> bool {
        self.preferences().any(|preference| match preference {
            CodecPreference::Pcmu => codec == Codec::Pcmu,
            CodecPreference::Pcma => codec == Codec::Pcma,
            CodecPreference::G722 => codec == Codec::G722,
            #[cfg(feature = "opus")]
            CodecPreference::Opus => codec == Codec::Opus,
            #[cfg(not(feature = "opus"))]
            CodecPreference::Opus => false,
            CodecPreference::L16 => codec == Codec::L16,
        })
    }

    /// Whether this selection carries the exact codec format, including its RTP clock.
    pub(crate) fn carries_format(self, codec: Codec, clock_rate: u32) -> bool {
        self.preferences().any(|preference| match preference {
            CodecPreference::Pcmu => codec == Codec::Pcmu && clock_rate == 8_000,
            CodecPreference::Pcma => codec == Codec::Pcma && clock_rate == 8_000,
            // The 8000 is the RTP clock RFC 3551 §4.5.2 preserves; the audio is 16 kHz.
            CodecPreference::G722 => codec == Codec::G722 && clock_rate == 8_000,
            #[cfg(feature = "opus")]
            CodecPreference::Opus => codec == Codec::Opus && clock_rate == 48_000,
            #[cfg(not(feature = "opus"))]
            CodecPreference::Opus => false,
            CodecPreference::L16 => codec == Codec::L16 && matches!(clock_rate, 8_000 | 44_100),
        })
    }
}

/// Whether an initial call exchange uses ICE (`docs/specs/ice.md` §13.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IcePolicy {
    /// Emit no ICE attributes and start no connectivity-check worker.
    #[default]
    Disabled,
    /// Gather host candidates from the bound media sockets.
    Host,
    /// Gather host candidates and ask this STUN server for server-reflexive candidates.
    Stun(SocketAddr),
}

/// How the initial audio stream is keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Keying {
    /// Preserve the compatibility behavior: SDES over protected signalling, plain RTP otherwise.
    #[default]
    Auto,
    /// Require plain RTP, including when signalling is protected.
    Plain,
    /// Require SDES-SRTP and refuse an unprotected signalling path.
    Sdes,
    /// Require DTLS-SRTP; it never falls back to SDES or plain RTP.
    DtlsSrtp,
}

/// The keying mechanism an established call actually uses.
///
/// Unlike [`Keying`], this contains no `Auto`: the compatibility policy has resolved to either
/// plain RTP or SDES by the time a [`crate::Call`] exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NegotiatedKeying {
    /// Plain RTP, without media encryption.
    Plain,
    /// SDES-SRTP (RFC 4568).
    Sdes,
    /// DTLS-SRTP (RFC 5763 and RFC 5764).
    DtlsSrtp,
}

/// A named composition of call/media requirements.
///
/// `Standard` preserves the independently selectable SIP media policies. `BrowserAudio` is the
/// fail-closed one-stream profile in `docs/specs/webrtc-audio.md`; selecting it never authorizes
/// fallback to the standard policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaProfile {
    /// Ordinary SIP audio with independently selected codec, ICE, and keying policies.
    #[default]
    Standard,
    /// Opus-first audio over WSS, ICE, DTLS-SRTP, and multiplexed RTCP.
    BrowserAudio,
}

/// The media choices shared by dialing and answering a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MediaPolicy {
    /// Named composition policy.
    pub profile: MediaProfile,
    /// Which codecs are offered and accepted.
    pub codecs: Codecs,
    /// Whether and how the initial exchange gathers ICE candidates.
    pub ice: IcePolicy,
    /// Which media keying mechanism the application selected.
    pub keying: Keying,
}

impl MediaPolicy {
    /// The fail-closed browser-audio policy.
    ///
    /// Feature availability and WSS are checked before media binding or gathering. The value is
    /// constructible in every build so a missing Opus or DTLS feature produces a typed setup
    /// error instead of turning a known profile name into an unknown configuration value.
    #[must_use]
    pub const fn browser_audio() -> Self {
        #[cfg(feature = "opus")]
        let codecs = Codecs::Opus;
        #[cfg(not(feature = "opus"))]
        let codecs = Codecs::G711;
        Self {
            profile: MediaProfile::BrowserAudio,
            codecs,
            ice: IcePolicy::Host,
            keying: Keying::DtlsSrtp,
        }
    }

    /// Select a named profile while retaining explicit lower-level choices.
    ///
    /// The browser-audio preflight still refuses any incompatible retained choice; this method
    /// does not silently rewrite it.
    #[must_use]
    pub const fn with_profile(mut self, profile: MediaProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Select a codec set while retaining the other media choices.
    #[must_use]
    pub const fn with_codecs(mut self, codecs: Codecs) -> Self {
        self.codecs = codecs;
        self
    }

    /// Select an ICE policy while retaining the other media choices.
    #[must_use]
    pub const fn with_ice(mut self, ice: IcePolicy) -> Self {
        self.ice = ice;
        self
    }

    /// Select the media keying while retaining the codec and ICE choices.
    #[must_use]
    pub const fn with_keying(mut self, keying: Keying) -> Self {
        self.keying = keying;
        self
    }

    /// Build fresh per-call gathering state when ICE was selected.
    pub(crate) fn gathering(self, offerer: bool) -> Result<Option<Gathering>> {
        if self.ice == IcePolicy::Disabled {
            return Ok(None);
        }
        let credentials = IceCredentials::new(token(), format!("{}{}", token(), token()))
            .ok_or_else(|| Error::Sdp("could not generate valid ICE credentials".to_owned()))?;
        let mut gathering = Gathering::new(credentials, offerer);
        if let IcePolicy::Stun(server) = self.ice {
            gathering.stun_server = Some(server);
        }
        Ok(Some(gathering))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn the_default_remains_the_g711_pair_in_wire_order() {
        assert_eq!(Codecs::default(), Codecs::G711);
        assert_eq!(
            Codecs::default().preferences().collect::<Vec<_>>(),
            vec![CodecPreference::Pcmu, CodecPreference::Pcma]
        );
        let capabilities = Codecs::default().capabilities("192.0.2.9".parse().unwrap(), 40_000);
        assert_eq!(capabilities.audio_formats, ["0", "8", "101"]);
    }

    #[test]
    fn an_explicit_order_is_the_order_put_in_an_offer() {
        let codecs = Codecs::ordered(&[CodecPreference::Pcma, CodecPreference::Pcmu]).unwrap();
        let capabilities = codecs.capabilities("192.0.2.9".parse().unwrap(), 40_000);
        assert_eq!(capabilities.audio_formats, ["8", "0", "101"]);
        assert!(codecs.carries(Codec::Pcma));
        assert!(codecs.carries(Codec::Pcmu));
    }

    /// M-43: one L16 preference names both mono formats sipx implements. Payload 11 is RFC
    /// 3551's static 44.1 kHz assignment; the 8 kHz form needs an explicit dynamic mapping.
    #[test]
    fn l16_offers_the_static_and_dynamic_mono_formats() {
        let codecs = Codecs::ordered(&[CodecPreference::L16]).unwrap();
        let capabilities = codecs.capabilities("192.0.2.9".parse().unwrap(), 40_000);
        assert_eq!(capabilities.audio_formats, ["11", "96", "101"]);
        assert!(
            capabilities
                .rtpmaps
                .contains(&("11".to_owned(), "L16/44100/1".to_owned()))
        );
        assert!(
            capabilities
                .rtpmaps
                .contains(&("96".to_owned(), "L16/8000/1".to_owned()))
        );
        assert!(codecs.carries(Codec::L16));
    }

    /// M-44: the named G.722 set leads with static type 9 and keeps the G.711 pair behind it,
    /// exactly the below-Opus-above-G.711 placement the type documentation states.
    #[test]
    fn g722_offers_static_type_9_ahead_of_g711() {
        let capabilities = Codecs::G722.capabilities("192.0.2.9".parse().unwrap(), 40_000);
        assert_eq!(capabilities.audio_formats, ["9", "0", "8", "101"]);
        assert!(
            capabilities
                .rtpmaps
                .contains(&("9".to_owned(), "G722/8000".to_owned())),
            "the rtpmap keeps RFC 3551 §4.5.2's historical 8000 clock"
        );
        assert!(Codecs::G722.carries(Codec::G722));
        assert!(Codecs::G722.carries_format(Codec::G722, 8_000));
        assert!(
            !Codecs::G722.carries_format(Codec::G722, 16_000),
            "G722/16000 names a format nobody has"
        );
        assert!(
            !Codecs::default().carries(Codec::G722),
            "the compatibility default stays the G.711 pair"
        );
    }

    #[test]
    fn an_empty_or_duplicate_selection_is_refused() {
        assert_eq!(Codecs::ordered(&[]), Err(CodecSelectionError::Empty));
        assert_eq!(
            Codecs::ordered(&[CodecPreference::Pcmu, CodecPreference::Pcmu]),
            Err(CodecSelectionError::Duplicate("pcmu"))
        );
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn opus_is_a_known_but_unavailable_value_without_the_feature() {
        assert_eq!(
            Codecs::ordered(&[CodecPreference::Opus]),
            Err(CodecSelectionError::OpusUnavailable)
        );
    }

    #[cfg(feature = "opus")]
    #[test]
    fn opus_can_be_placed_anywhere_in_the_order() {
        let codecs = Codecs::ordered(&[
            CodecPreference::Pcma,
            CodecPreference::Opus,
            CodecPreference::Pcmu,
        ])
        .unwrap();
        let capabilities = codecs.capabilities("192.0.2.9".parse().unwrap(), 40_000);
        assert_eq!(capabilities.audio_formats, ["8", "111", "0", "101"]);
        assert!(codecs.carries(Codec::Opus));
    }

    #[test]
    fn keying_default_and_explicit_values_are_distinct() {
        assert_eq!(Keying::default(), Keying::Auto);
        assert_ne!(Keying::Plain, Keying::Sdes);
        assert_ne!(Keying::Auto, Keying::Sdes);
        assert_ne!(Keying::DtlsSrtp, Keying::Plain);
    }
}
