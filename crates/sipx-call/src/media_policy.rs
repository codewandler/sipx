//! Application choices for an initial call's media.
//!
//! This module names policy only. SDP construction, offer/answer matching, ICE gathering and
//! media startup stay in [`crate::call`], so a command-line caller and a library caller cannot
//! acquire two implementations of negotiation.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use sipx_media::Codec;
use sipx_media::ice::{Gathering, Relay, RelayCredentialError};
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
#[non_exhaustive]
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

/// Maximum encoded TURN username length accepted by the Supported call configuration.
///
/// RFC 8489 §14.3 requires a USERNAME value to contain fewer than 509 bytes. Validation belongs
/// here, before the media port is bound, rather than in the later allocation encoder.
pub const MAX_TURN_USERNAME_BYTES: usize = 508;

/// Why a configured TURN relay cannot enter a call policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TurnPolicyError {
    /// The long-term username is absent.
    #[error("TURN username must not be empty")]
    EmptyUsername,
    /// The long-term username cannot fit the RFC 8489 USERNAME profile.
    #[error("TURN username exceeds {MAX_TURN_USERNAME_BYTES} bytes")]
    UsernameTooLong,
    /// The long-term password is absent.
    #[error("TURN password must not be empty")]
    EmptyPassword,
    /// The username is not valid under RFC 8265's `OpaqueString` profile.
    #[error("TURN username is not a valid OpaqueString")]
    InvalidUsername,
    /// The password is not valid under RFC 8265's `OpaqueString` profile.
    #[error("TURN password is not a valid OpaqueString")]
    InvalidPassword,
    /// A future credential-profile refusal is not understood by this call crate version.
    #[error("TURN credentials are not valid under the required profile")]
    InvalidCredentialProfile,
}

/// One caller-configured TURN relay and its long-term credentials.
///
/// The password is retained only by the private ICE relay value and is never exposed by an
/// accessor or `Debug`. Construction performs every validation possible without consulting the
/// server, before a call can bind media or spawn a worker.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TurnPolicy {
    relay: Arc<Relay>,
}

impl TurnPolicy {
    /// Validate a UDP TURN server and long-term credentials.
    ///
    /// # Errors
    ///
    /// Empty credentials and a username too long for RFC 8489's USERNAME attribute are refused.
    pub fn new(
        server: SocketAddr,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> std::result::Result<Self, TurnPolicyError> {
        let username = username.into();
        let password = password.into();
        if username.is_empty() {
            return Err(TurnPolicyError::EmptyUsername);
        }
        if password.is_empty() {
            return Err(TurnPolicyError::EmptyPassword);
        }
        let relay = Relay::new(server, username, password).map_err(|error| match error {
            RelayCredentialError::InvalidUsername => TurnPolicyError::InvalidUsername,
            RelayCredentialError::InvalidPassword => TurnPolicyError::InvalidPassword,
            _ => TurnPolicyError::InvalidCredentialProfile,
        })?;
        if relay.username().len() > MAX_TURN_USERNAME_BYTES {
            return Err(TurnPolicyError::UsernameTooLong);
        }
        Ok(Self {
            relay: Arc::new(relay),
        })
    }

    /// The configured TURN server.
    #[must_use]
    pub fn server(&self) -> SocketAddr {
        self.relay.server()
    }

    /// The long-term username. The password is intentionally not exposed.
    #[must_use]
    pub fn username(&self) -> &str {
        self.relay.username()
    }

    pub(crate) fn relay(&self) -> Relay {
        (*self.relay).clone()
    }
}

impl std::fmt::Debug for TurnPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TurnPolicy")
            .field("server", &self.server())
            .field("username", &self.username())
            .field("password", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Whether an initial call exchange uses ICE (`docs/specs/ice.md` §13.4).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum IcePolicy {
    /// Emit no ICE attributes and start no connectivity-check worker.
    #[default]
    Disabled,
    /// Gather host candidates from the bound media sockets.
    Host,
    /// Gather host candidates and ask this STUN server for server-reflexive candidates.
    Stun(SocketAddr),
    /// Gather host candidates and allocate one configured relayed candidate.
    ///
    /// Failure to allocate leaves the host candidates available; it does not weaken an explicit
    /// media-security policy or turn a viable direct pair into a call failure.
    Turn(TurnPolicy),
}

/// How the initial audio stream is keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum NegotiatedKeying {
    /// Plain RTP, without media encryption.
    Plain,
    /// SDES-SRTP (RFC 4568).
    Sdes,
    /// DTLS-SRTP (RFC 5763 and RFC 5764).
    DtlsSrtp,
}

/// One exact SRTP protection suite an application may require for a call.
///
/// With no requirement, both SDES and DTLS-SRTP offer every suite sipx can perform in
/// strongest-first order. [`MediaPolicy::with_srtp_suite`] changes that list to exactly one
/// value. It is a requirement, not a preference: use it only with [`Keying::Sdes`] or
/// [`Keying::DtlsSrtp`], and a peer which cannot agree on that suite gets no media stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SrtpSuite {
    /// AES-128 counter mode with an 80-bit HMAC-SHA1 tag (RFC 3711, RFC 4568 §6.2).
    AesCm128HmacSha1_80,
    /// AES-128 in Galois/Counter Mode with a 128-bit tag (RFC 7714).
    AeadAes128Gcm,
    /// AES-256 in Galois/Counter Mode with a 128-bit tag (RFC 7714).
    AeadAes256Gcm,
}

impl SrtpSuite {
    /// The stable configuration and result token.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::AesCm128HmacSha1_80 => "AES_CM_128_HMAC_SHA1_80",
            Self::AeadAes128Gcm => "AEAD_AES_128_GCM",
            Self::AeadAes256Gcm => "AEAD_AES_256_GCM",
        }
    }

    pub(crate) const fn sdes(self) -> sipx_sdp::crypto::Suite {
        match self {
            Self::AesCm128HmacSha1_80 => sipx_sdp::crypto::Suite::AesCm128HmacSha1_80,
            Self::AeadAes128Gcm => sipx_sdp::crypto::Suite::AeadAes128Gcm,
            Self::AeadAes256Gcm => sipx_sdp::crypto::Suite::AeadAes256Gcm,
        }
    }

    #[cfg(feature = "dtls")]
    pub(crate) const fn dtls(self) -> sipx_media::dtls::Profile {
        match self {
            Self::AesCm128HmacSha1_80 => sipx_media::dtls::Profile::Aes128CmHmacSha1_80,
            Self::AeadAes128Gcm => sipx_media::dtls::Profile::AeadAes128Gcm,
            Self::AeadAes256Gcm => sipx_media::dtls::Profile::AeadAes256Gcm,
        }
    }
}

/// A named composition of call/media requirements.
///
/// `Standard` preserves the independently selectable SIP media policies. `BrowserAudio` is the
/// fail-closed one-stream profile in `docs/specs/webrtc-audio.md`; selecting it never authorizes
/// fallback to the standard policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum MediaProfile {
    /// Ordinary SIP audio with independently selected codec, ICE, and keying policies.
    #[default]
    Standard,
    /// Opus-first audio over WSS, ICE, DTLS-SRTP, and multiplexed RTCP.
    BrowserAudio,
}

/// The media choices shared by dialing and answering a call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MediaPolicy {
    /// Named composition policy.
    pub(crate) profile: MediaProfile,
    /// Which codecs are offered and accepted.
    pub(crate) codecs: Codecs,
    /// Whether and how the initial exchange gathers ICE candidates.
    pub(crate) ice: IcePolicy,
    /// Which media keying mechanism the application selected.
    pub(crate) keying: Keying,
    /// An exact SRTP suite requirement, or every supported suite in strongest-first order.
    ///
    /// A value is valid only with an explicit [`Keying::Sdes`] or [`Keying::DtlsSrtp`]. Calls
    /// refuse contradictory `Auto` and `Plain` combinations before binding media or sending SIP.
    pub(crate) srtp_suite: Option<SrtpSuite>,
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
            srtp_suite: None,
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
    pub fn with_ice(mut self, ice: IcePolicy) -> Self {
        self.ice = ice;
        self
    }

    /// Select the media keying while retaining the codec and ICE choices.
    #[must_use]
    pub const fn with_keying(mut self, keying: Keying) -> Self {
        self.keying = keying;
        self
    }

    /// Require exactly one SRTP suite for the selected explicit SRTP keying.
    ///
    /// This narrows SDES's `a=crypto` lines and DTLS-SRTP's `use_srtp` extension alike. A peer
    /// that cannot perform the suite must refuse the stream; no weaker suite or clear RTP is
    /// permitted. Combining this with [`Keying::Auto`] or [`Keying::Plain`] is refused by call
    /// preflight because those policies may select clear RTP.
    #[must_use]
    pub const fn with_srtp_suite(mut self, suite: SrtpSuite) -> Self {
        self.srtp_suite = Some(suite);
        self
    }

    /// The named media profile.
    #[must_use]
    pub const fn profile(&self) -> MediaProfile {
        self.profile
    }

    /// The ordered codec policy.
    #[must_use]
    pub const fn codecs(&self) -> Codecs {
        self.codecs
    }

    /// The ICE policy, including caller-owned TURN credentials when selected.
    #[must_use]
    pub const fn ice(&self) -> &IcePolicy {
        &self.ice
    }

    /// The requested media-keying mechanism.
    #[must_use]
    pub const fn keying(&self) -> Keying {
        self.keying
    }

    /// The exact SRTP suite requirement, when one was selected.
    #[must_use]
    pub const fn srtp_suite(&self) -> Option<SrtpSuite> {
        self.srtp_suite
    }

    /// Build fresh per-call gathering state when ICE was selected.
    pub(crate) fn gathering(&self, offerer: bool) -> Result<Option<Gathering>> {
        if self.ice == IcePolicy::Disabled {
            return Ok(None);
        }
        let credentials = IceCredentials::new(token(), format!("{}{}", token(), token()))
            .ok_or_else(|| Error::Sdp("could not generate valid ICE credentials".to_owned()))?;
        let mut gathering = Gathering::new(credentials, offerer);
        match &self.ice {
            IcePolicy::Stun(server) => gathering.stun_server = Some(*server),
            IcePolicy::Turn(turn) => gathering.relay = Some(turn.relay()),
            IcePolicy::Disabled | IcePolicy::Host => {}
        }
        Ok(Some(gathering))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn turn_policy_rejects_invalid_credentials_and_redacts_the_password() {
        let server = "192.0.2.20:3478".parse().unwrap();
        assert_eq!(
            TurnPolicy::new(server, "", "secret"),
            Err(TurnPolicyError::EmptyUsername)
        );
        assert_eq!(
            TurnPolicy::new(server, "1000", ""),
            Err(TurnPolicyError::EmptyPassword)
        );
        assert_eq!(
            TurnPolicy::new(server, "x".repeat(MAX_TURN_USERNAME_BYTES + 1), "secret"),
            Err(TurnPolicyError::UsernameTooLong)
        );
        assert_eq!(
            TurnPolicy::new(server, "contains-\0-control", "secret"),
            Err(TurnPolicyError::InvalidUsername)
        );
        assert_eq!(
            TurnPolicy::new(server, "1000", "contains-\0-control"),
            Err(TurnPolicyError::InvalidPassword)
        );
        let normalized = TurnPolicy::new(server, "cafe\u{301}", "secret").unwrap();
        assert_eq!(normalized.username(), "caf\u{e9}");
        let boundary = TurnPolicy::new(server, "e\u{301}".repeat(254), "secret").unwrap();
        assert_eq!(boundary.username().len(), MAX_TURN_USERNAME_BYTES);
        let configured = TurnPolicy::new(server, "1000", "relay-password").unwrap();
        let debug = format!("{configured:?}");
        assert!(debug.contains("1000"));
        assert!(!debug.contains("relay-password"));
    }

    #[test]
    fn turn_policy_enters_fresh_gathering_without_changing_defaults() {
        let defaults = MediaPolicy::default();
        assert_eq!(defaults.codecs(), Codecs::G711);
        assert_eq!(defaults.ice(), &IcePolicy::Disabled);
        assert_eq!(defaults.keying(), Keying::Auto);

        let relay =
            TurnPolicy::new("192.0.2.20:3478".parse().unwrap(), "1000", "relay-password").unwrap();
        let policy = defaults.with_ice(IcePolicy::Turn(relay));
        let gathering = policy.gathering(true).unwrap().unwrap();
        assert!(gathering.relay.is_some());
        assert!(gathering.stun_server.is_none());
    }

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
