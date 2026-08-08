//! Selection: defaults, overrides, precedence (`docs/specs/speech-providers.md` §4).
//!
//! A selection document is the unit of speech policy, and its fields are **total**: every field is
//! required, forbidden for the kind, or optional with the absence meaning stated in §4 — no
//! field's absence is ever filled from another document. That is why [`SpeechPolicy::select`] does
//! not merge an override into an endpoint default: an effective policy assembled from two
//! documents cannot be read from either, and the first mis-merged voice would be a silent policy
//! change nobody could see in either file.
//!
//! Evaluation stops at the first failing step and reports that step's reason together with the
//! provider identity, the requested value and the descriptor facts consulted. **A refusal never
//! selects something else.** The one path past a refusal is the explicit `fallback` chain, and a
//! chain carries provider identities only — so engaging a candidate cannot change the language,
//! voice, format, device or conversion policy the host wrote.
//!
//! Nothing here touches the call. [`Selected::processing`] is the only bridge to the media path,
//! and it produces a request for the `M-54` seam — after selection has succeeded, never before.

use sipx_audio::{LinearResampler, PcmEncoding, PcmError, PcmFormat};

use super::bounds::SpeechBounds;
use super::descriptor::{
    Device, DeviceRequirement, LanguageRange, LanguageTag, ProviderDescriptor, ProviderId,
    ProviderKind, VoiceToken,
};
use super::privacy::{SpeechAdmission, SpeechPrivacy};
use super::registry::ProviderRegistry;
use crate::processing::{AudioDirection, Processing};

/// Whether a session may run in a format the call clock has to be converted to (§4 `conversion`).
///
/// Two values and no more: `allow` and `deny` exhaust the question, so this is deliberately closed
/// where the reason and output enums around it are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Conversion {
    /// A conversion between the operating format and the call clock may be used. The absence
    /// meaning of the field.
    #[default]
    Allow,
    /// The session must run in the seam's call-clock format itself.
    Deny,
}

/// Why a document is not well formed (§4).
///
/// Well-formedness is a property of the document **alone**, so every one of these is decided
/// before the registry is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Malformed {
    /// No `provider`.
    MissingProvider,
    /// No `language`. The wildcard `*` is a value, not an omission.
    MissingLanguage,
    /// A synthesis document with no `voice`.
    ///
    /// Required precisely so that no machinery — neither a provider's preferred ordering nor a
    /// fallback step — ever chooses a voice: the host wrote it, or selection refuses.
    MissingVoice,
    /// A recognition document carrying a `voice`, which its kind forbids.
    VoiceNotForRecognition,
}

/// The host's admission policy for a provider's declared locality (§4 step 2).
///
/// The default refuses both, which is the epic's stated default of local and offline execution.
/// It is the locality half of [`SpeechPrivacy`], which is what §4 step 2 reads as a whole: the
/// retention half ([`SpeechPrivacy::admits_retention`]) is checked immediately after this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct LocalityPolicy {
    off_host: bool,
    network: bool,
}

impl LocalityPolicy {
    /// Admit only providers that declare neither off-host processing nor network egress.
    pub const LOCAL_ONLY: Self = Self {
        off_host: false,
        network: false,
    };

    /// Also admit providers that move audio, text or derived data off this machine.
    #[must_use]
    pub const fn allowing_off_host(mut self) -> Self {
        self.off_host = true;
        self
    }

    /// Also admit providers that require network egress.
    #[must_use]
    pub const fn allowing_network(mut self) -> Self {
        self.network = true;
        self
    }

    /// Whether off-host processing is opted in to.
    #[must_use]
    pub const fn off_host(self) -> bool {
        self.off_host
    }

    /// Whether network egress is opted in to.
    #[must_use]
    pub const fn network(self) -> bool {
        self.network
    }

    /// Whether this policy admits a descriptor's declared locality.
    #[must_use]
    pub const fn admits(self, off_host: bool, network: bool) -> bool {
        (self.off_host || !off_host) && (self.network || !network)
    }
}

/// One unit of speech policy (§4).
///
/// Built field by field, and read the same way. Absent fields mean what §4's table says they mean
/// and never what another document says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectionDocument {
    provider: Option<ProviderId>,
    language: Option<LanguageRange>,
    voice: Option<VoiceToken>,
    format: Option<PcmFormat>,
    device: Option<DeviceRequirement>,
    conversion: Option<Conversion>,
    fallback: Vec<ProviderId>,
}

impl SelectionDocument {
    /// An empty document. It is malformed until it names a provider and a language.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            provider: None,
            language: None,
            voice: None,
            format: None,
            device: None,
            conversion: None,
            fallback: Vec::new(),
        }
    }

    /// Name the provider.
    #[must_use]
    pub fn with_provider(mut self, provider: ProviderId) -> Self {
        self.provider = Some(provider);
        self
    }

    /// Name the language range the policy asks for.
    #[must_use]
    pub fn with_language(mut self, language: LanguageRange) -> Self {
        self.language = Some(language);
        self
    }

    /// Name the voice. Required for synthesis, forbidden for recognition.
    #[must_use]
    pub fn with_voice(mut self, voice: VoiceToken) -> Self {
        self.voice = Some(voice);
        self
    }

    /// Pin the provider-side operating format.
    #[must_use]
    pub fn with_format(mut self, format: PcmFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// Require a device capability.
    #[must_use]
    pub fn with_device(mut self, device: DeviceRequirement) -> Self {
        self.device = Some(device);
        self
    }

    /// State the conversion policy. Absent means [`Conversion::Allow`].
    #[must_use]
    pub fn with_conversion(mut self, conversion: Conversion) -> Self {
        self.conversion = Some(conversion);
        self
    }

    /// Give the ordered fallback chain: provider identities only.
    #[must_use]
    pub fn with_fallback(mut self, chain: impl IntoIterator<Item = ProviderId>) -> Self {
        self.fallback = chain.into_iter().collect();
        self
    }

    /// The provider named, if any.
    #[must_use]
    pub const fn provider(&self) -> Option<&ProviderId> {
        self.provider.as_ref()
    }

    /// The language range asked for, if any.
    #[must_use]
    pub const fn language(&self) -> Option<&LanguageRange> {
        self.language.as_ref()
    }

    /// The voice asked for, if any.
    #[must_use]
    pub const fn voice(&self) -> Option<&VoiceToken> {
        self.voice.as_ref()
    }

    /// The pinned operating format, if any.
    #[must_use]
    pub const fn format(&self) -> Option<PcmFormat> {
        self.format
    }

    /// The required device capability, if any.
    #[must_use]
    pub const fn device(&self) -> Option<DeviceRequirement> {
        self.device
    }

    /// The conversion policy in force: the field, or its absence meaning.
    #[must_use]
    pub fn conversion(&self) -> Conversion {
        self.conversion.unwrap_or_default()
    }

    /// The ordered fallback chain. Empty means a refusal is final.
    #[must_use]
    pub fn fallback(&self) -> &[ProviderId] {
        &self.fallback
    }

    /// The document a host re-runs §4 with after the selected provider is lost (§7).
    ///
    /// Only `provider` moves: the head of the chain becomes the provider and the tail becomes the
    /// new chain. Every constraint field is carried across untouched, which is why engaging a
    /// successor cannot change the language, voice, format, device or conversion policy — the
    /// invariant is structural here rather than a discipline the caller has to keep.
    ///
    /// `None` when the chain is empty, and that is what makes a loss final in the same way an
    /// absent chain makes a refusal final. Starting the successor session is the host's action;
    /// this only says what it would be selected from.
    #[must_use]
    pub fn after_loss(&self) -> Option<Self> {
        let (next, rest) = self.fallback.split_first()?;
        let mut successor = self.clone();
        successor.provider = Some(next.clone());
        successor.fallback = rest.to_vec();
        Some(successor)
    }

    /// Whether this document is well formed for `kind` (§4).
    ///
    /// # Errors
    ///
    /// Returns the first [`Malformed`] reason, decided from the document alone.
    pub fn validate(&self, kind: ProviderKind) -> Result<(), Malformed> {
        if self.provider.is_none() {
            return Err(Malformed::MissingProvider);
        }
        if self.language.is_none() {
            return Err(Malformed::MissingLanguage);
        }
        match kind {
            ProviderKind::Recognition if self.voice.is_some() => {
                Err(Malformed::VoiceNotForRecognition)
            }
            ProviderKind::Synthesis if self.voice.is_none() => Err(Malformed::MissingVoice),
            _ => Ok(()),
        }
    }
}

/// Everything selection reads that is not a document.
///
/// The call clock is derived rather than accepted: the seam carries signed 16-bit samples at the
/// negotiated media clock (`docs/specs/linear-pcm.md` §3), so a caller supplies the rate and
/// cannot supply a call-clock format the seam would never produce.
#[derive(Debug, Clone, Copy)]
pub struct SelectionContext<'a> {
    registry: &'a ProviderRegistry,
    privacy: SpeechPrivacy,
    call_clock: PcmFormat,
}

impl<'a> SelectionContext<'a> {
    /// Read selection against `registry` for a call whose media clock runs at `media_clock` Hz.
    ///
    /// The host privacy policy starts at [`SpeechPrivacy::LOCAL_NO_RETENTION`] — local, offline and
    /// nothing kept past the operation. Widen it with [`Self::with_privacy`], or its locality half
    /// alone with [`Self::with_locality`].
    ///
    /// # Errors
    ///
    /// Returns [`PcmError::UnsupportedSampleRate`] when the rate is outside the linear-PCM
    /// boundary — reused rather than re-minted, so an impossible clock refuses with the type that
    /// boundary already defines.
    pub fn new(registry: &'a ProviderRegistry, media_clock: u32) -> Result<Self, PcmError> {
        Ok(Self {
            registry,
            privacy: SpeechPrivacy::LOCAL_NO_RETENTION,
            call_clock: PcmFormat::new(media_clock, PcmEncoding::Signed16)?,
        })
    }

    /// Use a wider host locality policy, leaving the retention opt-ins where they are.
    #[must_use]
    pub const fn with_locality(mut self, locality: LocalityPolicy) -> Self {
        self.privacy = self.privacy.with_locality(locality);
        self
    }

    /// Use a wider host privacy policy: locality and retention together (§11.3).
    #[must_use]
    pub const fn with_privacy(mut self, privacy: SpeechPrivacy) -> Self {
        self.privacy = privacy;
        self
    }

    /// The registry selection reads.
    #[must_use]
    pub const fn registry(&self) -> &'a ProviderRegistry {
        self.registry
    }

    /// The host locality policy in force.
    #[must_use]
    pub const fn locality(&self) -> LocalityPolicy {
        self.privacy.locality()
    }

    /// The whole host privacy policy in force.
    #[must_use]
    pub const fn privacy(&self) -> SpeechPrivacy {
        self.privacy
    }

    /// The seam's call-clock format.
    #[must_use]
    pub const fn call_clock(&self) -> PcmFormat {
        self.call_clock
    }
}

/// Why one candidate was refused (§4's evaluation table).
///
/// Every variant carries the requested value and the descriptor facts consulted, so a host can act
/// on a refusal without parsing a message.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefusalReason {
    /// Step 1: no provider of the requested kind is registered under that identity.
    UnknownProvider {
        /// The kind the document was evaluated for.
        kind: ProviderKind,
    },
    /// Step 2: host locality policy does not admit what the descriptor declares.
    LocalityRefused {
        /// The descriptor's declared off-host property.
        off_host: bool,
        /// The descriptor's declared network property.
        network: bool,
    },
    /// Step 2: host retention policy does not admit what the descriptor declares (§11.3).
    ///
    /// Separate from [`Self::LocalityRefused`] because they are separate host decisions and lead to
    /// separate configuration changes: one is "this provider would send the call somewhere else",
    /// the other is "this provider would keep it".
    RetentionRefused {
        /// The descriptor's declared debug-capture property.
        debug_capture: bool,
        /// The descriptor's declared derived-cache property.
        derived_cache: bool,
    },
    /// Step 3: no declared tag matches the requested range under RFC 4647 §3.3.1.
    UnsupportedLanguage {
        /// The range the document asked for.
        requested: LanguageRange,
        /// The tags the descriptor declares, in declared order.
        declared: Vec<LanguageTag>,
    },
    /// Step 4: the named voice does not exist, or speaks no tag matching the range.
    UnsupportedVoice {
        /// The voice the document named.
        requested: VoiceToken,
        /// The range the document asked for.
        range: LanguageRange,
        /// The voices the descriptor declares, in declared order.
        declared: Vec<VoiceToken>,
    },
    /// Step 5: the operating-format rule yields no format.
    UnsupportedFormat {
        /// The pinned format, if the document pinned one.
        pinned: Option<PcmFormat>,
        /// The conversion policy in force.
        conversion: Conversion,
        /// The seam's call-clock format.
        call_clock: PcmFormat,
        /// The formats the descriptor declares for its kind, in declared order.
        declared: Vec<PcmFormat>,
    },
    /// Step 6: no declared device satisfies the required capability.
    UnsupportedDevice {
        /// The capability the document required.
        required: DeviceRequirement,
        /// The devices the descriptor declares, in declared order.
        declared: Vec<Device>,
    },
}

/// One candidate's refusal, with the identity it was refused for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    provider: ProviderId,
    reason: RefusalReason,
}

impl Refusal {
    /// The candidate that was refused.
    #[must_use]
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Why it was refused.
    #[must_use]
    pub const fn reason(&self) -> &RefusalReason {
        &self.reason
    }
}

/// Why an operation has no provider (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SelectionError {
    /// The document is not well formed. Decided before the registry is read.
    Malformed(Malformed),
    /// Rank 3 of the precedence table: nothing is configured, so nothing is selected. No provider
    /// is ever chosen implicitly, and discovery order never implies a default.
    NoProviderConfigured,
    /// Every candidate was refused, in configured order.
    ///
    /// The first entry is the document's own `provider`; the rest are its `fallback` chain. An
    /// absent chain therefore yields exactly one entry, which is what "a refusal is final" means.
    Refused(Vec<Refusal>),
}

impl std::fmt::Display for SelectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(reason) => write!(f, "the selection document is malformed: {reason:?}"),
            Self::NoProviderConfigured => f.write_str("no speech provider is configured"),
            Self::Refused(refusals) => {
                write!(
                    f,
                    "every candidate was refused ({} of them)",
                    refusals.len()
                )
            }
        }
    }
}

impl std::error::Error for SelectionError {}

/// One resolved selection: what the session will run as, decided once and not re-decided (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    provider: ProviderId,
    kind: ProviderKind,
    language: LanguageTag,
    voice: Option<VoiceToken>,
    format: PcmFormat,
    device: Option<DeviceRequirement>,
    conversion: Conversion,
    position: usize,
    passed_over: Vec<Refusal>,
    admission: SpeechAdmission,
}

impl Selected {
    /// The provider that satisfied the document.
    #[must_use]
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Which contract it implements.
    #[must_use]
    pub const fn kind(&self) -> ProviderKind {
        self.kind
    }

    /// The effective language tag: the first declared tag matching the requested range.
    ///
    /// Derived and deterministic. The *policy* is the range, which no selection step can alter.
    #[must_use]
    pub const fn language(&self) -> &LanguageTag {
        &self.language
    }

    /// The voice, for a synthesis selection.
    #[must_use]
    pub const fn voice(&self) -> Option<&VoiceToken> {
        self.voice.as_ref()
    }

    /// The operating format the session runs in, fixed here so nothing downstream re-decides it.
    #[must_use]
    pub const fn format(&self) -> PcmFormat {
        self.format
    }

    /// The device capability required, if the document required one.
    #[must_use]
    pub const fn device(&self) -> Option<DeviceRequirement> {
        self.device
    }

    /// The conversion policy in force.
    #[must_use]
    pub const fn conversion(&self) -> Conversion {
        self.conversion
    }

    /// Where in the chain this candidate sits: 0 is the document's own `provider`.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// The candidates refused ahead of this one, in configured order, each with its typed reason.
    #[must_use]
    pub fn passed_over(&self) -> &[Refusal] {
        &self.passed_over
    }

    /// What this session was admitted to do (§11.3).
    ///
    /// A host output on the speech event stream, and the only place a consumer has to look to
    /// answer "is this call's audio being kept, or leaving this machine?". It exists on `Selected`
    /// rather than beside it because step 2 is what produced it: an admission that could be built
    /// without a selection would be a claim rather than a record.
    #[must_use]
    pub const fn admission(&self) -> &SpeechAdmission {
        &self.admission
    }

    /// The `M-54` seam request this selection makes (`docs/specs/call-audio-seam.md` §5).
    ///
    /// This is the whole of the contract's reach into call media, and it is one call to the one
    /// tap: the attachment runs in the operating format decided above, with §8's input-frame bound
    /// as its queue depth, so the seam's drop-oldest policy *is* §5's input-bound obligation.
    ///
    /// It exists on `Selected` and nowhere else, which is what makes "an unknown or unavailable
    /// provider is refused before any call resource is taken" a property of the types rather than
    /// of the order somebody wrote two statements in.
    #[must_use]
    pub fn processing(&self, direction: AudioDirection, bounds: SpeechBounds) -> Processing {
        Processing::new(direction, self.format).with_queue_capacity(bounds.input_frames())
    }
}

/// The host output that reports a fallback candidate taking over from a lost session (§7).
///
/// A host output on the speech event stream, never a session output: the lost session's own last
/// output is `Stopped`, and the successor is a new session rather than a transition of the old one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackEngaged {
    lost: ProviderId,
    engaged: ProviderId,
    position: usize,
    language: LanguageTag,
}

impl FallbackEngaged {
    /// Report that `lost` was replaced by the provider `selected` names.
    #[must_use]
    pub fn new(lost: ProviderId, selected: &Selected) -> Self {
        Self {
            lost,
            engaged: selected.provider().clone(),
            position: selected.position(),
            language: selected.language().clone(),
        }
    }

    /// The provider that was lost.
    #[must_use]
    pub const fn lost(&self) -> &ProviderId {
        &self.lost
    }

    /// The provider that took over.
    #[must_use]
    pub const fn engaged(&self) -> &ProviderId {
        &self.engaged
    }

    /// Its position in the chain.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.position
    }

    /// The effective language tag of the successor selection.
    #[must_use]
    pub const fn language(&self) -> &LanguageTag {
        &self.language
    }
}

/// An endpoint's speech policy: at most one default document per contract kind (§4 rank 2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpeechPolicy {
    recognition: Option<SelectionDocument>,
    synthesis: Option<SelectionDocument>,
}

impl SpeechPolicy {
    /// A policy with no default for either kind. Selection under it is `NoProviderConfigured`.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            recognition: None,
            synthesis: None,
        }
    }

    /// Set the endpoint default for one kind.
    #[must_use]
    pub fn with_default(mut self, kind: ProviderKind, document: SelectionDocument) -> Self {
        match kind {
            ProviderKind::Recognition => self.recognition = Some(document),
            ProviderKind::Synthesis => self.synthesis = Some(document),
        }
        self
    }

    /// The endpoint default for one kind, if there is one.
    #[must_use]
    pub const fn endpoint_default(&self, kind: ProviderKind) -> Option<&SelectionDocument> {
        match kind {
            ProviderKind::Recognition => self.recognition.as_ref(),
            ProviderKind::Synthesis => self.synthesis.as_ref(),
        }
    }

    /// Resolve one selection under §4's precedence.
    ///
    /// A per-call `over` replaces the endpoint default **entirely**: an absent optional field in it
    /// means what §4's table says, never what the endpoint default's field says. Field-level
    /// merging is deliberately excluded.
    ///
    /// # Errors
    ///
    /// Returns [`SelectionError::NoProviderConfigured`] when neither rank supplies a document,
    /// [`SelectionError::Malformed`] when the chosen document is not well formed, and
    /// [`SelectionError::Refused`] with one entry per candidate in configured order when no
    /// candidate satisfies the document.
    pub fn select(
        &self,
        kind: ProviderKind,
        over: Option<&SelectionDocument>,
        context: &SelectionContext<'_>,
    ) -> Result<Selected, SelectionError> {
        let Some(document) = over.or_else(|| self.endpoint_default(kind)) else {
            return Err(SelectionError::NoProviderConfigured);
        };
        select_document(document, kind, context)
    }
}

/// Evaluate one document against the registry, walking its fallback chain in configured order.
///
/// # Errors
///
/// As [`SpeechPolicy::select`], minus the precedence step.
pub(crate) fn select_document(
    document: &SelectionDocument,
    kind: ProviderKind,
    context: &SelectionContext<'_>,
) -> Result<Selected, SelectionError> {
    document.validate(kind).map_err(SelectionError::Malformed)?;
    // Both are present: `validate` refused a document missing either.
    let (Some(provider), Some(range)) = (document.provider(), document.language()) else {
        return Err(SelectionError::Malformed(Malformed::MissingProvider));
    };

    let mut refusals = Vec::new();
    let candidates = std::iter::once(provider).chain(document.fallback());
    for (position, candidate) in candidates.enumerate() {
        match evaluate(candidate, range, document, kind, context) {
            Ok(mut selected) => {
                selected.position = position;
                selected.passed_over = refusals;
                return Ok(selected);
            }
            Err(reason) => refusals.push(Refusal {
                provider: candidate.clone(),
                reason,
            }),
        }
    }
    Err(SelectionError::Refused(refusals))
}

/// §4's six evaluation steps, in order, stopping at the first failure.
fn evaluate(
    candidate: &ProviderId,
    range: &LanguageRange,
    document: &SelectionDocument,
    kind: ProviderKind,
    context: &SelectionContext<'_>,
) -> Result<Selected, RefusalReason> {
    // Step 1: registered with the requested kind.
    let descriptor = context
        .registry()
        .resolve(candidate, kind)
        .ok_or(RefusalReason::UnknownProvider { kind })?;

    // Step 2: host privacy policy admits what the descriptor declares — locality first, because
    // "this provider would send the call somewhere else" is the coarser fact, then retention.
    let declared = descriptor.privacy();
    if !context
        .locality()
        .admits(declared.off_host(), declared.network())
    {
        return Err(RefusalReason::LocalityRefused {
            off_host: declared.off_host(),
            network: declared.network(),
        });
    }
    if !context
        .privacy()
        .admits_retention(declared.debug_capture(), declared.derived_cache())
    {
        return Err(RefusalReason::RetentionRefused {
            debug_capture: declared.debug_capture(),
            derived_cache: declared.derived_cache(),
        });
    }

    // Step 3: the effective tag is the first declared tag matching the range.
    let language = descriptor
        .languages()
        .iter()
        .find(|tag| range.matches(tag))
        .ok_or_else(|| RefusalReason::UnsupportedLanguage {
            requested: range.clone(),
            declared: descriptor.languages().to_vec(),
        })?
        .clone();

    // Step 4: the named voice exists and speaks a tag matching the range.
    let voice = match document.voice() {
        None => None,
        Some(requested) => {
            let found = descriptor.voices().iter().find(|voice| {
                voice.token() == requested && voice.languages().iter().any(|tag| range.matches(tag))
            });
            if found.is_none() {
                return Err(RefusalReason::UnsupportedVoice {
                    requested: requested.clone(),
                    range: range.clone(),
                    declared: descriptor
                        .voices()
                        .iter()
                        .map(|voice| voice.token().clone())
                        .collect(),
                });
            }
            Some(requested.clone())
        }
    };

    // Step 5: the operating-format rule yields exactly one format.
    let conversion = document.conversion();
    let format = operating_format(descriptor, document.format(), conversion, context)?;

    // Step 6: the required device capability is present.
    if let Some(required) = document.device()
        && !descriptor
            .devices()
            .iter()
            .any(|device| required.satisfied_by(*device))
    {
        return Err(RefusalReason::UnsupportedDevice {
            required,
            declared: descriptor.devices().to_vec(),
        });
    }

    Ok(Selected {
        provider: candidate.clone(),
        kind,
        language,
        voice,
        format,
        device: document.device(),
        conversion,
        position: 0,
        passed_over: Vec::new(),
        admission: SpeechAdmission::new(candidate.clone(), kind, declared, context.privacy()),
    })
}

/// §4's operating-format rule: every session runs in exactly one provider-side format.
fn operating_format(
    descriptor: &ProviderDescriptor,
    pinned: Option<PcmFormat>,
    conversion: Conversion,
    context: &SelectionContext<'_>,
) -> Result<PcmFormat, RefusalReason> {
    let declared = descriptor.operating_formats();
    let call_clock = context.call_clock();
    let refuse = || RefusalReason::UnsupportedFormat {
        pinned,
        conversion,
        call_clock,
        declared: declared.to_vec(),
    };

    match (pinned, conversion) {
        (Some(pin), Conversion::Allow) => {
            if declared.contains(&pin) && convertible(pin, call_clock) {
                Ok(pin)
            } else {
                Err(refuse())
            }
        }
        (Some(pin), Conversion::Deny) => {
            if declared.contains(&pin) && pin == call_clock {
                Ok(pin)
            } else {
                Err(refuse())
            }
        }
        (None, Conversion::Allow) => declared
            .iter()
            .copied()
            .find(|format| convertible(*format, call_clock))
            .ok_or_else(refuse),
        (None, Conversion::Deny) => {
            if declared.contains(&call_clock) {
                Ok(call_clock)
            } else {
                Err(refuse())
            }
        }
    }
}

/// Whether `M-43` can convert between a provider-side format and the call clock, both ways.
///
/// Asked of the shared boundary rather than assumed, so the rule reads off §4. The boundary
/// validates rates when a `PcmFormat` is built, so today this is total for two constructed
/// formats — and it is written as a question because that is a property of the boundary's domain
/// and not of this rule.
fn convertible(format: PcmFormat, call_clock: PcmFormat) -> bool {
    LinearResampler::new(format.sample_rate(), call_clock.sample_rate()).is_ok()
        && LinearResampler::new(call_clock.sample_rate(), format.sample_rate()).is_ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::speech::descriptor::{DeviceKind, ProviderDescriptor};

    fn id(token: &str) -> ProviderId {
        ProviderId::new(token).unwrap()
    }

    fn range(token: &str) -> LanguageRange {
        LanguageRange::new(token).unwrap()
    }

    fn tag(token: &str) -> LanguageTag {
        LanguageTag::new(token).unwrap()
    }

    fn narrowband() -> PcmFormat {
        PcmFormat::new(8_000, PcmEncoding::Signed16).unwrap()
    }

    fn wideband() -> PcmFormat {
        PcmFormat::new(16_000, PcmEncoding::Signed16).unwrap()
    }

    fn registry() -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry
            .register(
                ProviderDescriptor::recognition(id("one"), "0")
                    .language(tag("en-GB"))
                    .language(tag("en"))
                    .accepted_format(wideband())
                    .accepted_format(narrowband())
                    .device(
                        Device::new(DeviceKind::Cpu)
                            .with_memory_bytes(1_024)
                            .with_concurrent_sessions(2),
                    )
                    .build(),
            )
            .unwrap();
        registry
    }

    fn document() -> SelectionDocument {
        SelectionDocument::new()
            .with_provider(id("one"))
            .with_language(range("en"))
    }

    /// §4: the effective tag is the *first* declared tag matching the range, so declaration order
    /// is the only thing that decides it.
    #[test]
    fn the_effective_tag_is_the_first_declared_match() {
        let registry = registry();
        let context = SelectionContext::new(&registry, 8_000).unwrap();
        let selected =
            select_document(&document(), ProviderKind::Recognition, &context).expect("selects");
        assert_eq!(selected.language(), &tag("en-GB"));
    }

    /// §4: with no pin and `conversion = allow`, the operating format is the first declared format
    /// a conversion exists for — the provider's preference, not the call's.
    #[test]
    fn an_unpinned_allowing_selection_takes_the_first_declared_format() {
        let registry = registry();
        let context = SelectionContext::new(&registry, 8_000).unwrap();
        let selected =
            select_document(&document(), ProviderKind::Recognition, &context).expect("selects");
        assert_eq!(selected.format(), wideband());
        assert_eq!(selected.conversion(), Conversion::Allow);
    }

    /// §4: with no pin and `conversion = deny`, the operating format is the call clock's own,
    /// which must be declared.
    #[test]
    fn an_unpinned_denying_selection_takes_the_call_clock() {
        let registry = registry();
        let context = SelectionContext::new(&registry, 8_000).unwrap();
        let selected = select_document(
            &document().with_conversion(Conversion::Deny),
            ProviderKind::Recognition,
            &context,
        )
        .expect("selects");
        assert_eq!(selected.format(), narrowband());

        let elsewhere = SelectionContext::new(&registry, 44_100).unwrap();
        let refused = select_document(
            &document().with_conversion(Conversion::Deny),
            ProviderKind::Recognition,
            &elsewhere,
        )
        .expect_err("44.1 kHz is not declared");
        assert!(matches!(
            refused,
            SelectionError::Refused(ref refusals)
                if matches!(refusals[0].reason(), RefusalReason::UnsupportedFormat { .. })
        ));
    }

    /// §4 step 6: a floor on a numeric capability is part of the requirement.
    #[test]
    fn a_device_floor_is_part_of_the_requirement() {
        let registry = registry();
        let context = SelectionContext::new(&registry, 8_000).unwrap();
        let met = document().with_device(
            DeviceRequirement::new(DeviceKind::Cpu)
                .with_min_memory_bytes(1_024)
                .with_min_concurrent_sessions(2),
        );
        assert!(select_document(&met, ProviderKind::Recognition, &context).is_ok());

        let unmet = document()
            .with_device(DeviceRequirement::new(DeviceKind::Cpu).with_min_memory_bytes(4_096));
        let refused = select_document(&unmet, ProviderKind::Recognition, &context)
            .expect_err("the floor is not met");
        assert!(matches!(
            refused,
            SelectionError::Refused(ref refusals)
                if matches!(refusals[0].reason(), RefusalReason::UnsupportedDevice { .. })
        ));
    }

    /// §4: selection is the only producer of a seam request, and it carries §8's input bound.
    #[test]
    fn a_selection_asks_the_seam_for_its_operating_format() {
        let registry = registry();
        let context = SelectionContext::new(&registry, 8_000).unwrap();
        let selected =
            select_document(&document(), ProviderKind::Recognition, &context).expect("selects");
        let request = selected.processing(AudioDirection::Inbound, SpeechBounds::DEFAULTS);
        assert_eq!(request.direction(), AudioDirection::Inbound);
        assert_eq!(request.format(), selected.format());
        assert_eq!(
            request.queue_capacity(),
            SpeechBounds::DEFAULTS.input_frames()
        );
    }
}
