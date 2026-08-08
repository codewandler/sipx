//! Discovery: what a registered provider says about itself (`docs/specs/speech-providers.md` §3).
//!
//! Everything here is data. A descriptor is gathered by the provider at registration time, behind
//! the leaf-driver boundary, and read afterwards without probing, model load or network I/O — so
//! two consecutive discovery reads of an unchanged registry are identical.
//!
//! **Locality is a property, not a name.** *Local/offline* is the conjunction `off_host = false`
//! and `network = false`, and nothing in this crate infers it from an identity token. **Execution
//! devices are described by capability** for the same reason: selection by marketing name is
//! neither portable nor checkable, so a device states what it is, how much memory it offers, and
//! how many concurrent real-time sessions the provider declares for it.

use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use sipx_audio::PcmFormat;

use super::privacy::{ProviderPrivacy, RetentionOptIn};

/// A token that is not in the shape its field requires.
///
/// One type for identity, voice, property and language tokens: the caller already knows which
/// field it wrote, and `expected` names the shape rather than repeating the field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{token}` is not {expected}")]
pub struct InvalidToken {
    token: String,
    expected: &'static str,
}

impl InvalidToken {
    /// The token as it was written.
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The shape the field requires.
    #[must_use]
    pub const fn expected(&self) -> &'static str {
        self.expected
    }
}

/// Whether every character is one this project's stable tokens allow.
///
/// Lowercase is checked rather than applied: §3 asks for a *stable lowercase* identity, and
/// silently folding `Inert` into `inert` would make two host configurations that read differently
/// register as one provider.
fn is_stable_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '-' | '.' | '_')
        })
}

/// A language subtag sequence: alphanumeric subtags joined by `-`, none of them empty.
fn is_subtag_sequence(token: &str) -> bool {
    !token.is_empty()
        && token
            .split('-')
            .all(|subtag| !subtag.is_empty() && subtag.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// A provider's stable identity (§3 `id`).
///
/// Unique in the registry, identical across restarts and versions, and never a marketing name —
/// the version is a separate field precisely so the identity does not carry one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderId(String);

impl ProviderId {
    /// Read an identity token.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidToken`] unless the token is one or more of `a`–`z`, `0`–`9`, `-`, `.`
    /// or `_`.
    pub fn new(token: impl Into<String>) -> Result<Self, InvalidToken> {
        let token = token.into();
        if is_stable_token(&token) {
            Ok(Self(token))
        } else {
            Err(InvalidToken {
                token,
                expected: "a stable lowercase identity token",
            })
        }
    }

    /// The token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A synthesis voice's stable token (§3 `voices`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VoiceToken(String);

impl VoiceToken {
    /// Read a voice token.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidToken`] on the same domain as [`ProviderId::new`].
    pub fn new(token: impl Into<String>) -> Result<Self, InvalidToken> {
        let token = token.into();
        if is_stable_token(&token) {
            Ok(Self(token))
        } else {
            Err(InvalidToken {
                token,
                expected: "a stable lowercase voice token",
            })
        }
    }

    /// The token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VoiceToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One declared property of a voice (§3 `voices`).
///
/// Declared tokens, opaque to selection: no step of §4 reads them, so a property can never quietly
/// become the reason one voice was chosen over another. They exist so discovery can report what a
/// provider says about a voice beyond the language it speaks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VoiceProperty(String);

impl VoiceProperty {
    /// Read a property token.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidToken`] on the same domain as [`ProviderId::new`].
    pub fn new(token: impl Into<String>) -> Result<Self, InvalidToken> {
        let token = token.into();
        if is_stable_token(&token) {
            Ok(Self(token))
        } else {
            Err(InvalidToken {
                token,
                expected: "a declared lowercase voice property",
            })
        }
    }

    /// The token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VoiceProperty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An RFC 5646 language tag a provider declares.
///
/// Held in the canonical lowercase form, because RFC 5646 §2.1.1 makes case insignificant and
/// RFC 4647 §3.3.1 matches case-insensitively: retaining the written case would give two values
/// that must compare equal and do not.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageTag(String);

impl LanguageTag {
    /// Read a language tag.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidToken`] unless the tag is one or more non-empty alphanumeric subtags
    /// joined by `-`.
    pub fn new(tag: impl AsRef<str>) -> Result<Self, InvalidToken> {
        let tag = tag.as_ref();
        if is_subtag_sequence(tag) {
            Ok(Self(tag.to_ascii_lowercase()))
        } else {
            Err(InvalidToken {
                token: tag.to_owned(),
                expected: "an RFC 5646 language tag",
            })
        }
    }

    /// The canonical lowercase tag.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An RFC 4647 basic language range, as a selection document asks for a language.
///
/// The wildcard `*` is permitted and is the host explicitly stating that any declared language is
/// acceptable — §4 is emphatic that it is not an omission.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageRange(String);

impl LanguageRange {
    /// The range that matches every declared tag.
    #[must_use]
    pub fn wildcard() -> Self {
        Self("*".to_owned())
    }

    /// Read a basic language range.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidToken`] unless the range is `*` or a sequence of non-empty alphanumeric
    /// subtags joined by `-`.
    pub fn new(range: impl AsRef<str>) -> Result<Self, InvalidToken> {
        let range = range.as_ref();
        if range == "*" || is_subtag_sequence(range) {
            Ok(Self(range.to_ascii_lowercase()))
        } else {
            Err(InvalidToken {
                token: range.to_owned(),
                expected: "an RFC 4647 basic language range",
            })
        }
    }

    /// Whether this range matches `tag`, per RFC 4647 §3.3.1 basic filtering.
    ///
    /// The wildcard matches everything; otherwise the tag matches when it equals the range or
    /// continues it at a subtag boundary — so `en` matches `en-GB` and does not match `english`.
    #[must_use]
    pub fn matches(&self, tag: &LanguageTag) -> bool {
        if self.0 == "*" {
            return true;
        }
        tag.0 == self.0
            || (tag.0.len() > self.0.len()
                && tag.0.starts_with(&self.0)
                && tag.0.as_bytes().get(self.0.len()) == Some(&b'-'))
    }

    /// The range as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which of the two contracts one descriptor describes (§3 `kind`).
///
/// Closed by design, and deliberately not `#[non_exhaustive]`: this specification defines two
/// substitutable contracts, and a third would be a different document rather than a new variant of
/// this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProviderKind {
    /// Speech recognition (§5).
    Recognition,
    /// Speech synthesis (§6).
    Synthesis,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recognition => f.write_str("recognition"),
            Self::Synthesis => f.write_str("synthesis"),
        }
    }
}

/// What class of hardware a declared execution device is (§3 `devices`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DeviceKind {
    /// The host's general-purpose processor.
    Cpu,
    /// A separate compute device.
    Accelerator,
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => f.write_str("cpu"),
            Self::Accelerator => f.write_str("accelerator"),
        }
    }
}

/// One execution device a provider declares it can use now (§3 `devices`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Device {
    kind: DeviceKind,
    memory_bytes: u64,
    concurrent_sessions: u32,
}

impl Device {
    /// Declare a device of this class, offering nothing else yet.
    ///
    /// Capabilities are added one at a time rather than passed positionally, so a future capability
    /// is a new method and not a new argument every caller has to be edited for (§9).
    #[must_use]
    pub const fn new(kind: DeviceKind) -> Self {
        Self {
            kind,
            memory_bytes: 0,
            concurrent_sessions: 0,
        }
    }

    /// Declare how much memory it offers.
    #[must_use]
    pub const fn with_memory_bytes(mut self, bytes: u64) -> Self {
        self.memory_bytes = bytes;
        self
    }

    /// Declare how many concurrent real-time sessions the provider claims for it.
    #[must_use]
    pub const fn with_concurrent_sessions(mut self, sessions: u32) -> Self {
        self.concurrent_sessions = sessions;
        self
    }

    /// What the device is.
    #[must_use]
    pub const fn kind(self) -> DeviceKind {
        self.kind
    }

    /// How much memory it offers.
    #[must_use]
    pub const fn memory_bytes(self) -> u64 {
        self.memory_bytes
    }

    /// Concurrent real-time sessions the provider declares for it.
    #[must_use]
    pub const fn concurrent_sessions(self) -> u32 {
        self.concurrent_sessions
    }
}

/// A device capability a selection document requires (§4 `device`).
///
/// A requirement names a class and, optionally, floors on the numeric capabilities. It never names
/// a device by identity: §3 puts device description on a capability footing precisely so a policy
/// written on one machine means the same thing on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceRequirement {
    kind: DeviceKind,
    min_memory_bytes: u64,
    min_concurrent_sessions: u32,
}

impl DeviceRequirement {
    /// Require a device of this class.
    #[must_use]
    pub const fn new(kind: DeviceKind) -> Self {
        Self {
            kind,
            min_memory_bytes: 0,
            min_concurrent_sessions: 0,
        }
    }

    /// Also require at least this much device memory.
    #[must_use]
    pub const fn with_min_memory_bytes(mut self, bytes: u64) -> Self {
        self.min_memory_bytes = bytes;
        self
    }

    /// Also require at least this many declared concurrent real-time sessions.
    #[must_use]
    pub const fn with_min_concurrent_sessions(mut self, sessions: u32) -> Self {
        self.min_concurrent_sessions = sessions;
        self
    }

    /// The class required.
    #[must_use]
    pub const fn kind(self) -> DeviceKind {
        self.kind
    }

    /// The memory floor.
    #[must_use]
    pub const fn min_memory_bytes(self) -> u64 {
        self.min_memory_bytes
    }

    /// The concurrency floor.
    #[must_use]
    pub const fn min_concurrent_sessions(self) -> u32 {
        self.min_concurrent_sessions
    }

    /// Whether one declared device satisfies this requirement.
    #[must_use]
    pub const fn satisfied_by(self, device: Device) -> bool {
        matches!(
            (self.kind, device.kind),
            (DeviceKind::Cpu, DeviceKind::Cpu) | (DeviceKind::Accelerator, DeviceKind::Accelerator)
        ) && device.memory_bytes >= self.min_memory_bytes
            && device.concurrent_sessions >= self.min_concurrent_sessions
    }
}

/// A provider's own capacity-planning estimates (§3 `resources`).
///
/// Estimates, and stated as such: the binding guarantee is behavioural, and a provider that cannot
/// meet its declared real-time profile refuses setup with the measured requirement (`M-55`/`M-56`)
/// rather than degrading silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Resources {
    warm_bytes: u64,
    session_bytes: u64,
    warmup: Duration,
}

impl Resources {
    /// No estimate declared for anything yet.
    ///
    /// Estimates are added one at a time for the reason [`Device::new`] gives: a new estimate must
    /// be a new method rather than a new argument (§9).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            warm_bytes: 0,
            session_bytes: 0,
            warmup: Duration::ZERO,
        }
    }

    /// Estimate the bytes needed to warm the provider.
    #[must_use]
    pub const fn with_warm_bytes(mut self, bytes: u64) -> Self {
        self.warm_bytes = bytes;
        self
    }

    /// Estimate the resident bytes per session.
    #[must_use]
    pub const fn with_session_bytes(mut self, bytes: u64) -> Self {
        self.session_bytes = bytes;
        self
    }

    /// Estimate how long warming takes.
    #[must_use]
    pub const fn with_warmup(mut self, warmup: Duration) -> Self {
        self.warmup = warmup;
        self
    }

    /// Bytes to warm the provider.
    #[must_use]
    pub const fn warm_bytes(self) -> u64 {
        self.warm_bytes
    }

    /// Resident bytes per session.
    #[must_use]
    pub const fn session_bytes(self) -> u64 {
        self.session_bytes
    }

    /// How long warming is expected to take.
    ///
    /// An estimate for planning. Warm-up is *bounded* by the driver-fired deadline of §8, which is
    /// a different number owned by the host.
    #[must_use]
    pub const fn warmup(self) -> Duration {
        self.warmup
    }
}

/// One synthesis voice (§3 `voices`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    token: VoiceToken,
    languages: Vec<LanguageTag>,
    properties: Vec<VoiceProperty>,
}

impl Voice {
    /// Start declaring a voice.
    #[must_use]
    pub const fn new(token: VoiceToken) -> Self {
        Self {
            token,
            languages: Vec::new(),
            properties: Vec::new(),
        }
    }

    /// Declare another tag this voice can speak.
    #[must_use]
    pub fn language(mut self, tag: LanguageTag) -> Self {
        self.languages.push(tag);
        self
    }

    /// Declare another property token.
    ///
    /// An unreadable token is dropped rather than refused: a property is opaque to selection, so a
    /// malformed one can change no outcome, and making every declaration fallible would put a
    /// `Result` in the path of data nothing branches on. Use [`VoiceProperty::new`] to check one.
    #[must_use]
    pub fn property(mut self, property: impl Into<String>) -> Self {
        if let Ok(property) = VoiceProperty::new(property) {
            self.properties.push(property);
        }
        self
    }

    /// The voice's stable token.
    #[must_use]
    pub const fn token(&self) -> &VoiceToken {
        &self.token
    }

    /// The tags this voice can speak, in declared order.
    #[must_use]
    pub fn languages(&self) -> &[LanguageTag] {
        &self.languages
    }

    /// The declared property tokens, in declared order.
    #[must_use]
    pub fn properties(&self) -> &[VoiceProperty] {
        &self.properties
    }
}

/// Everything one registered provider reports about itself (§3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    id: ProviderId,
    version: String,
    kind: ProviderKind,
    /// §3's four privacy properties, kept as the one declaration §4 step 2 admits or refuses.
    privacy: ProviderPrivacy,
    languages: Vec<LanguageTag>,
    voices: Vec<Voice>,
    accepted_formats: Vec<PcmFormat>,
    emitted_formats: Vec<PcmFormat>,
    streaming: bool,
    devices: Vec<Device>,
    resources: Resources,
}

/// The builder state that produces a recognition descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForRecognition;

/// The builder state that produces a synthesis descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForSynthesis;

impl ProviderDescriptor {
    /// Start describing a recognition provider.
    #[must_use]
    pub fn recognition(
        id: ProviderId,
        version: impl Into<String>,
    ) -> ProviderDescriptorBuilder<ForRecognition> {
        ProviderDescriptorBuilder::start(id, version.into(), ProviderKind::Recognition)
    }

    /// Start describing a synthesis provider.
    #[must_use]
    pub fn synthesis(
        id: ProviderId,
        version: impl Into<String>,
    ) -> ProviderDescriptorBuilder<ForSynthesis> {
        ProviderDescriptorBuilder::start(id, version.into(), ProviderKind::Synthesis)
    }

    /// The provider's stable identity.
    #[must_use]
    pub const fn id(&self) -> &ProviderId {
        &self.id
    }

    /// The provider's version, which is not part of its identity.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Which contract this descriptor describes.
    #[must_use]
    pub const fn kind(&self) -> ProviderKind {
        self.kind
    }

    /// Whether audio, text or data derived from them ever leaves this machine.
    #[must_use]
    pub const fn off_host(&self) -> bool {
        self.privacy.off_host()
    }

    /// Whether runtime operation requires any network egress at all.
    #[must_use]
    pub const fn network(&self) -> bool {
        self.privacy.network()
    }

    /// Whether the provider can write call audio, transcripts or synthesis input somewhere durable
    /// for diagnosis (§11.3).
    ///
    /// Declaring it does not make it happen: the host has to configure
    /// [`RetentionOptIn::DebugCapture`](super::RetentionOptIn::DebugCapture) as well, or §4 step 2
    /// refuses the provider outright.
    #[must_use]
    pub const fn debug_capture(&self) -> bool {
        self.privacy.debug_capture()
    }

    /// Whether the provider keeps data derived from call audio or text past the session (§11.3).
    #[must_use]
    pub const fn derived_cache(&self) -> bool {
        self.privacy.derived_cache()
    }

    /// The four properties §11 admits a provider on, as one value.
    ///
    /// The same type the session's [`SpeechAdmission`](super::SpeechAdmission) carries, so
    /// discovery and the call event cannot disagree about what a provider does.
    #[must_use]
    pub const fn privacy(&self) -> ProviderPrivacy {
        self.privacy
    }

    /// Whether this provider is local and offline: the conjunction of the two properties above.
    #[must_use]
    pub const fn is_local_offline(&self) -> bool {
        self.privacy.is_local_offline()
    }

    /// The RFC 5646 tags supported, in declared order.
    ///
    /// Declared order is load-bearing: §4's effective language tag is the first declared tag
    /// matching the requested range.
    #[must_use]
    pub fn languages(&self) -> &[LanguageTag] {
        &self.languages
    }

    /// The voices offered. Empty for a recognition provider.
    #[must_use]
    pub fn voices(&self) -> &[Voice] {
        &self.voices
    }

    /// The formats accepted as session input. Empty for a synthesis provider.
    #[must_use]
    pub fn accepted_formats(&self) -> &[PcmFormat] {
        &self.accepted_formats
    }

    /// The formats this provider can emit. Empty for a recognition provider.
    #[must_use]
    pub fn emitted_formats(&self) -> &[PcmFormat] {
        &self.emitted_formats
    }

    /// The formats §4's operating-format rule reads for this descriptor's kind.
    #[must_use]
    pub fn operating_formats(&self) -> &[PcmFormat] {
        match self.kind {
            ProviderKind::Recognition => &self.accepted_formats,
            ProviderKind::Synthesis => &self.emitted_formats,
        }
    }

    /// Recognition: whether revisions are emitted before `Flush`. Synthesis: whether chunks are
    /// emitted before the request completes.
    #[must_use]
    pub const fn streaming(&self) -> bool {
        self.streaming
    }

    /// The execution devices usable now.
    #[must_use]
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    /// The capacity-planning estimates.
    #[must_use]
    pub const fn resources(&self) -> Resources {
        self.resources
    }
}

/// Assembles a [`ProviderDescriptor`] one field at a time (§9).
///
/// The kind is a type parameter rather than a runtime check, so `accepted_format` does not exist
/// on a synthesis descriptor and `voice` does not exist on a recognition one: §3's "applies to"
/// column becomes something the compiler holds rather than something a validator has to refuse.
/// New descriptor fields arrive as new setters, which is why construction goes through a builder
/// at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptorBuilder<K> {
    descriptor: ProviderDescriptor,
    kind: PhantomData<K>,
}

impl<K> ProviderDescriptorBuilder<K> {
    fn start(id: ProviderId, version: String, kind: ProviderKind) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                id,
                version,
                kind,
                privacy: ProviderPrivacy::LOCAL_OFFLINE,
                languages: Vec::new(),
                voices: Vec::new(),
                accepted_formats: Vec::new(),
                emitted_formats: Vec::new(),
                streaming: false,
                devices: Vec::new(),
                resources: Resources::default(),
            },
            kind: PhantomData,
        }
    }

    /// Declare that audio, text or data derived from them leaves this machine.
    #[must_use]
    pub fn off_host(mut self, off_host: bool) -> Self {
        self.descriptor.privacy = self
            .descriptor
            .privacy
            .declaring(RetentionOptIn::OffHostProcessing, off_host);
        self
    }

    /// Declare that runtime operation requires network egress.
    #[must_use]
    pub fn network(mut self, network: bool) -> Self {
        self.descriptor.privacy = self
            .descriptor
            .privacy
            .declaring(RetentionOptIn::NetworkEgress, network);
        self
    }

    /// Declare that the provider can write call audio, transcripts or synthesis input somewhere
    /// durable for diagnosis (§11.3).
    #[must_use]
    pub fn debug_capture(mut self, debug_capture: bool) -> Self {
        self.descriptor.privacy = self
            .descriptor
            .privacy
            .declaring(RetentionOptIn::DebugCapture, debug_capture);
        self
    }

    /// Declare that the provider keeps data derived from call audio or text past the session
    /// (§11.3).
    #[must_use]
    pub fn derived_cache(mut self, derived_cache: bool) -> Self {
        self.descriptor.privacy = self
            .descriptor
            .privacy
            .declaring(RetentionOptIn::PersistentDerivedData, derived_cache);
        self
    }

    /// Declare another supported language tag. Declaration order is the order §4 matches in.
    #[must_use]
    pub fn language(mut self, tag: LanguageTag) -> Self {
        self.descriptor.languages.push(tag);
        self
    }

    /// Declare whether output is produced before the input ends.
    #[must_use]
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.descriptor.streaming = streaming;
        self
    }

    /// Declare another usable execution device.
    #[must_use]
    pub fn device(mut self, device: Device) -> Self {
        self.descriptor.devices.push(device);
        self
    }

    /// Declare the capacity-planning estimates.
    #[must_use]
    pub fn resources(mut self, resources: Resources) -> Self {
        self.descriptor.resources = resources;
        self
    }

    /// Finish the descriptor.
    #[must_use]
    pub fn build(self) -> ProviderDescriptor {
        self.descriptor
    }
}

impl ProviderDescriptorBuilder<ForRecognition> {
    /// Declare another `PcmFormat` accepted as session input.
    #[must_use]
    pub fn accepted_format(mut self, format: PcmFormat) -> Self {
        self.descriptor.accepted_formats.push(format);
        self
    }
}

impl ProviderDescriptorBuilder<ForSynthesis> {
    /// Declare another voice.
    #[must_use]
    pub fn voice(mut self, voice: Voice) -> Self {
        self.descriptor.voices.push(voice);
        self
    }

    /// Declare another `PcmFormat` this provider can emit.
    #[must_use]
    pub fn emitted_format(mut self, format: PcmFormat) -> Self {
        self.descriptor.emitted_formats.push(format);
        self
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// RFC 4647 §3.3.1: a range matches a tag it equals or continues at a subtag boundary, and the
    /// wildcard matches everything.
    #[test]
    fn basic_filtering_matches_at_subtag_boundaries() {
        let range = LanguageRange::new("en").unwrap();
        assert!(range.matches(&LanguageTag::new("en").unwrap()));
        assert!(range.matches(&LanguageTag::new("en-GB").unwrap()));
        assert!(!range.matches(&LanguageTag::new("eng").unwrap()));
        assert!(!range.matches(&LanguageTag::new("de").unwrap()));

        let wildcard = LanguageRange::wildcard();
        assert!(wildcard.matches(&LanguageTag::new("de-CH-1901").unwrap()));
    }

    /// §3: case is insignificant, so the canonical form is what a descriptor retains.
    #[test]
    fn language_tags_are_case_insensitive() {
        assert_eq!(
            LanguageTag::new("EN-gb").unwrap(),
            LanguageTag::new("en-GB").unwrap()
        );
    }

    /// §3: an identity is a stable lowercase token, and the check refuses rather than folds.
    #[test]
    fn identities_are_lowercase_tokens() {
        assert!(ProviderId::new("inert-0.1_a").is_ok());
        for refused in ["", "Inert", "in ert", "inert/one"] {
            let error = ProviderId::new(refused).expect_err("refused");
            assert_eq!(error.token(), refused);
            assert_eq!(error.expected(), "a stable lowercase identity token");
        }
    }

    /// §3: *local/offline* is the conjunction of two declared properties and never an identity.
    #[test]
    fn locality_is_a_property() {
        let id = ProviderId::new("inert").unwrap();
        let local = ProviderDescriptor::recognition(id.clone(), "0").build();
        assert!(local.is_local_offline());
        assert!(
            !ProviderDescriptor::recognition(id, "0")
                .network(true)
                .build()
                .is_local_offline()
        );
    }
}
