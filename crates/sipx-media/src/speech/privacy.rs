//! Data classes, retention and redaction (`docs/specs/speech-providers.md` §11).
//!
//! §11 answers three questions the rest of the contract deliberately leaves open, and this module
//! is the answer in types.
//!
//! **What is user data?** [`DataClass`] names the six things a speech session can hold, and says
//! for each one whether it belongs to the call or to the provider. The four that belong to the call
//! — call audio, transcripts, synthesis input and anything derived from them — default to
//! [`Retention::LiveOperation`]: they exist for the operation that produced them and for nothing
//! after it. That default is a *value*, [`SpeechPrivacy::LOCAL_NO_RETENTION`], and it is what
//! [`SpeechPrivacy::default`] gives, so a host that configures nothing has opted in to nothing.
//!
//! **What can change that?** Four named opt-ins and no others ([`RetentionOptIn`]). Each is host
//! configuration, each is refused at selection (§4 step 2) when the provider needs it and the host
//! has not written it down, each is visible in discovery through [`ProviderPrivacy`], and each is
//! named on the session's [`SpeechAdmission`] — so "why is this call's audio being written to
//! disk?" is answerable from the event stream rather than from a configuration file nobody has.
//!
//! **What may a log say?** The identity, the kind, the lifecycle, the limits and the typed cause.
//! Never the content. [`Redacted`] renders a class and a size where the value would have gone, and
//! [`Secret`] does the same for a credential or a model path, which is why the redaction survives a
//! caller who writes `?value` without thinking about it. That is the only kind of redaction that
//! holds: one you cannot forget.
//!
//! Erasure itself is the driver's, not this module's — see [`SynthesisDriver::retained_audio`]
//! (`crate::speech::SynthesisDriver`) and §11.5.

use std::fmt;

use super::descriptor::{ProviderId, ProviderKind};
use super::selection::LocalityPolicy;

/// How long a class of data may live (§11.1).
///
/// Two values, because there are two owners. Call data lives for the operation that produced it;
/// provider data lives with the provider. Nothing in this contract has a third lifetime, and a
/// class that wanted one would be a specification change rather than a configuration option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Retention {
    /// Until the operation that produced it ends. For a recognition result that is the utterance's
    /// terminal; for a synthesis chunk it is the hand-off to the call; for the session as a whole
    /// it is `Stopped`.
    LiveOperation,
    /// For as long as the provider is loaded. Model weights and credentials are the provider's own
    /// and outlive no call because they were never a call's to begin with.
    ProcessLifetime,
}

impl fmt::Display for Retention {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::LiveOperation => "the live operation",
            Self::ProcessLifetime => "the provider's lifetime",
        })
    }
}

/// One host opt-in (§11.3).
///
/// Four, one per property a provider can declare. They are separate because they are separate
/// decisions: a host that accepts a diagnostic capture has not thereby accepted a cache, and a host
/// that accepts a network-backed provider has not thereby accepted one that keeps what it heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum RetentionOptIn {
    /// Call audio, transcripts or synthesis input may be written somewhere durable for diagnosis.
    DebugCapture,
    /// Data derived from call audio or text may outlive the session.
    PersistentDerivedData,
    /// Audio, text or data derived from them may leave this machine.
    OffHostProcessing,
    /// The provider may require network egress to run at all.
    NetworkEgress,
}

impl RetentionOptIn {
    /// Every opt-in, in the order refusals and admissions report them.
    pub const ALL: [Self; 4] = [
        Self::DebugCapture,
        Self::PersistentDerivedData,
        Self::OffHostProcessing,
        Self::NetworkEgress,
    ];

    /// This opt-in's place in a [`ProviderPrivacy`] or [`SpeechPrivacy`] set.
    const fn bit(self) -> u8 {
        match self {
            Self::DebugCapture => 1,
            Self::PersistentDerivedData => 1 << 1,
            Self::OffHostProcessing => 1 << 2,
            Self::NetworkEgress => 1 << 3,
        }
    }
}

impl fmt::Display for RetentionOptIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DebugCapture => "debug-capture",
            Self::PersistentDerivedData => "persistent-derived-data",
            Self::OffHostProcessing => "off-host-processing",
            Self::NetworkEgress => "network-egress",
        })
    }
}

/// What a speech session can hold (§11.1).
///
/// The split that matters is [`Self::user_data`]: four of these came from a call and belong to it,
/// and two are the provider's own. Every retention rule in §11 follows from which side a class is
/// on, so classifying a new kind of data is the whole of deciding how long it may live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum DataClass {
    /// PCM from or for the call, on either side of the `M-54` seam.
    CallAudio,
    /// Recognition text: partial, replacement or final.
    Transcript,
    /// The text a host asked a synthesis session to speak.
    SynthesisInput,
    /// Model weights and warmed engine state.
    ModelState,
    /// A credential a provider needs to reach whatever it reaches.
    Credential,
    /// Anything derived from call audio or text — adaptation state, embeddings, caches.
    DerivedCache,
}

impl DataClass {
    /// Every class §11 classifies.
    pub const ALL: [Self; 6] = [
        Self::CallAudio,
        Self::Transcript,
        Self::SynthesisInput,
        Self::ModelState,
        Self::Credential,
        Self::DerivedCache,
    ];

    /// Whether this class came from a call, and so belongs to it.
    #[must_use]
    pub const fn user_data(self) -> bool {
        match self {
            Self::CallAudio | Self::Transcript | Self::SynthesisInput | Self::DerivedCache => true,
            Self::ModelState | Self::Credential => false,
        }
    }

    /// How long this class lives when the host has configured nothing (§11.2).
    #[must_use]
    pub const fn default_retention(self) -> Retention {
        if self.user_data() {
            Retention::LiveOperation
        } else {
            Retention::ProcessLifetime
        }
    }

    /// The one opt-in that lets this class outlive [`Self::default_retention`].
    ///
    /// `None` for the provider's own data, which no host configuration extends — it is already
    /// bounded by the provider being loaded. Note that this is about *outliving* the operation:
    /// whether a class may leave the machine at all is [`RetentionOptIn::OffHostProcessing`], and
    /// it applies to every user-data class at once.
    #[must_use]
    pub const fn opt_in(self) -> Option<RetentionOptIn> {
        match self {
            Self::CallAudio | Self::Transcript | Self::SynthesisInput => {
                Some(RetentionOptIn::DebugCapture)
            }
            Self::DerivedCache => Some(RetentionOptIn::PersistentDerivedData),
            Self::ModelState | Self::Credential => None,
        }
    }
}

impl fmt::Display for DataClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::CallAudio => "call audio",
            Self::Transcript => "transcript",
            Self::SynthesisInput => "synthesis input",
            Self::ModelState => "model state",
            Self::Credential => "credential",
            Self::DerivedCache => "derived cache",
        })
    }
}

/// What one provider declares about data leaving the machine or outliving the operation (§3, §11).
///
/// One type read from two places, deliberately: discovery reports it
/// ([`ProviderDescriptor::privacy`](super::ProviderDescriptor::privacy)) and the session's
/// admission event carries it ([`SpeechAdmission::declared`]). A host therefore cannot be told one
/// thing by discovery and another by the call.
///
/// It is *the set of opt-ins this provider needs* rather than four independent flags, because that
/// is what §4 step 2 asks it: the four §3 properties are read back through [`Self::off_host`] and
/// its siblings, and the question selection actually puts to a host policy is always "is everything
/// in this set configured?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ProviderPrivacy {
    required: u8,
}

impl ProviderPrivacy {
    /// The declaration that needs no host opt-in: nothing leaves, nothing is kept.
    pub const LOCAL_OFFLINE: Self = Self { required: 0 };

    /// Declare — or withdraw — one property.
    #[must_use]
    pub const fn declaring(self, opt_in: RetentionOptIn, declared: bool) -> Self {
        let bit = opt_in.bit();
        Self {
            required: if declared {
                self.required | bit
            } else {
                self.required & !bit
            },
        }
    }

    /// Whether audio, text or data derived from them ever leaves this machine.
    #[must_use]
    pub const fn off_host(self) -> bool {
        self.requires(RetentionOptIn::OffHostProcessing)
    }

    /// Whether runtime operation requires any network egress at all.
    #[must_use]
    pub const fn network(self) -> bool {
        self.requires(RetentionOptIn::NetworkEgress)
    }

    /// Whether the provider can write call audio, transcripts or synthesis input somewhere durable.
    #[must_use]
    pub const fn debug_capture(self) -> bool {
        self.requires(RetentionOptIn::DebugCapture)
    }

    /// Whether the provider keeps data derived from call audio or text past the session.
    #[must_use]
    pub const fn derived_cache(self) -> bool {
        self.requires(RetentionOptIn::PersistentDerivedData)
    }

    /// Whether this is the local, offline declaration: nothing leaves the machine.
    #[must_use]
    pub const fn is_local_offline(self) -> bool {
        !self.off_host() && !self.network()
    }

    /// The opt-ins a host must have configured before this provider can be selected, in
    /// [`RetentionOptIn::ALL`] order.
    ///
    /// Empty for [`Self::LOCAL_OFFLINE`], which is the point: the default configuration admits the
    /// default provider and refuses every other one.
    pub fn required_opt_ins(self) -> impl Iterator<Item = RetentionOptIn> {
        RetentionOptIn::ALL
            .into_iter()
            .filter(move |opt_in| self.requires(*opt_in))
    }

    /// Whether this declaration needs one named opt-in.
    #[must_use]
    pub const fn requires(self, opt_in: RetentionOptIn) -> bool {
        self.required & opt_in.bit() != 0
    }
}

impl fmt::Display for ProviderPrivacy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", Engagements(*self))
    }
}

/// The host's speech privacy configuration (§11.3).
///
/// It carries §4 step 2's [`LocalityPolicy`] and the two retention opt-ins together, because they
/// are one decision from the host's point of view — "what may a speech provider do with this call?"
/// — and because selection has to answer all of them at one step or admit something on the way to
/// refusing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SpeechPrivacy {
    locality: LocalityPolicy,
    debug_capture: bool,
    persistent_derived_data: bool,
}

impl SpeechPrivacy {
    /// The default: local, offline, and nothing kept past the operation.
    pub const LOCAL_NO_RETENTION: Self = Self {
        locality: LocalityPolicy::LOCAL_ONLY,
        debug_capture: false,
        persistent_derived_data: false,
    };

    /// Configure one opt-in.
    ///
    /// Takes and returns the whole policy, so widening is a chain a reader can read as a list of
    /// decisions rather than a struct literal whose `false`s are invisible.
    #[must_use]
    pub const fn allowing(self, opt_in: RetentionOptIn) -> Self {
        match opt_in {
            RetentionOptIn::DebugCapture => Self {
                debug_capture: true,
                ..self
            },
            RetentionOptIn::PersistentDerivedData => Self {
                persistent_derived_data: true,
                ..self
            },
            RetentionOptIn::OffHostProcessing => Self {
                locality: self.locality.allowing_off_host(),
                ..self
            },
            RetentionOptIn::NetworkEgress => Self {
                locality: self.locality.allowing_network(),
                ..self
            },
        }
    }

    /// Use a locality policy built directly, leaving the retention opt-ins alone.
    #[must_use]
    pub const fn with_locality(mut self, locality: LocalityPolicy) -> Self {
        self.locality = locality;
        self
    }

    /// The locality half, which §4 step 2 reads first.
    #[must_use]
    pub const fn locality(self) -> LocalityPolicy {
        self.locality
    }

    /// Whether one opt-in is configured.
    #[must_use]
    pub const fn allows(self, opt_in: RetentionOptIn) -> bool {
        match opt_in {
            RetentionOptIn::DebugCapture => self.debug_capture,
            RetentionOptIn::PersistentDerivedData => self.persistent_derived_data,
            RetentionOptIn::OffHostProcessing => self.locality.off_host(),
            RetentionOptIn::NetworkEgress => self.locality.network(),
        }
    }

    /// Whether the retention half admits a declaration's two retention properties.
    #[must_use]
    pub const fn admits_retention(self, debug_capture: bool, derived_cache: bool) -> bool {
        (self.debug_capture || !debug_capture) && (self.persistent_derived_data || !derived_cache)
    }

    /// Whether this configuration admits everything a provider declares.
    #[must_use]
    pub const fn admits(self, declared: ProviderPrivacy) -> bool {
        self.locality
            .admits(declared.off_host(), declared.network())
            && self.admits_retention(declared.debug_capture(), declared.derived_cache())
    }

    /// The opt-ins configured, in [`RetentionOptIn::ALL`] order.
    pub fn configured(self) -> impl Iterator<Item = RetentionOptIn> {
        RetentionOptIn::ALL
            .into_iter()
            .filter(move |opt_in| self.allows(*opt_in))
    }
}

impl fmt::Display for SpeechPrivacy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for opt_in in self.configured() {
            if !first {
                f.write_str(",")?;
            }
            write!(f, "{opt_in}")?;
            first = false;
        }
        if first { f.write_str("none") } else { Ok(()) }
    }
}

/// The opt-ins a declaration requires, rendered for a log record.
struct Engagements(ProviderPrivacy);

impl fmt::Display for Engagements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for opt_in in self.0.required_opt_ins() {
            if !first {
                f.write_str(",")?;
            }
            write!(f, "{opt_in}")?;
            first = false;
        }
        if first { f.write_str("none") } else { Ok(()) }
    }
}

/// What one session was admitted to do (§11.3).
///
/// A host output on the speech event stream, on the same terms as
/// [`FallbackEngaged`](super::FallbackEngaged): it is produced by selection, before any queue is
/// allocated, and it names the provider, the contract kind, what the provider declared and what the
/// host had configured. [`Self::engaged`] is the answer a reader usually wants — the opt-ins this
/// session actually runs under, which is the intersection of the two and is exactly what the
/// provider declared, because selection refuses anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechAdmission {
    provider: ProviderId,
    kind: ProviderKind,
    declared: ProviderPrivacy,
    policy: SpeechPrivacy,
}

impl SpeechAdmission {
    /// Record that `provider` was admitted under `policy` with `declared`.
    #[must_use]
    pub const fn new(
        provider: ProviderId,
        kind: ProviderKind,
        declared: ProviderPrivacy,
        policy: SpeechPrivacy,
    ) -> Self {
        Self {
            provider,
            kind,
            declared,
            policy,
        }
    }

    /// The provider admitted.
    #[must_use]
    pub const fn provider(&self) -> &ProviderId {
        &self.provider
    }

    /// Which contract it implements.
    #[must_use]
    pub const fn kind(&self) -> ProviderKind {
        self.kind
    }

    /// What the provider declared in discovery.
    #[must_use]
    pub const fn declared(&self) -> ProviderPrivacy {
        self.declared
    }

    /// What the host had configured when this session was admitted.
    #[must_use]
    pub const fn policy(&self) -> SpeechPrivacy {
        self.policy
    }

    /// The opt-ins in force for this session, in [`RetentionOptIn::ALL`] order.
    ///
    /// A configured opt-in nothing uses is not in force: a host that permits debug capture and then
    /// runs a provider that captures nothing gets an empty list, which is the honest answer to
    /// "what is happening to this call's audio?".
    pub fn engaged(&self) -> impl Iterator<Item = RetentionOptIn> {
        self.declared.required_opt_ins()
    }
}

impl fmt::Display for SpeechAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} engaged=[{}] configured=[{}]",
            self.provider,
            self.kind,
            Engagements(self.declared),
            self.policy
        )
    }
}

/// A value whose only rendering is a redaction (§11.4).
///
/// The carrier the contract offers a provider for its credentials and model paths. Neither is
/// contract data — a session never receives either — which is exactly why they need a type: they
/// live in a provider's own configuration, where a derived `Debug` would put them in the first log
/// record anyone wrote.
///
/// [`Self::expose`] is the only way to read one, and it is deliberately a word you can grep for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wrap a value so it cannot be printed.
    #[must_use]
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Read the value.
    #[must_use]
    pub const fn expose(&self) -> &T {
        &self.0
    }

    /// Consume the wrapper and return the value.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// What a size is measured in when the value itself is not reportable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Unit {
    Octets,
    Samples,
}

/// The stand-in a user-data value renders as (§11.4).
///
/// It reports the class and the size and nothing else, which is enough to see that something was
/// said, how much of it there was, and which contract it belonged to — and not enough to learn a
/// word of it. Every `Debug` implementation on a user-data type in this module tree uses this, so
/// `?value` in a log record is safe by construction rather than by review.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Redacted {
    class: DataClass,
    unit: Unit,
    amount: usize,
}

impl Redacted {
    /// A redacted text value of `octets` octets.
    #[must_use]
    pub const fn octets(class: DataClass, octets: usize) -> Self {
        Self {
            class,
            unit: Unit::Octets,
            amount: octets,
        }
    }

    /// A redacted audio buffer of `samples` mono samples.
    #[must_use]
    pub const fn samples(class: DataClass, samples: usize) -> Self {
        Self {
            class,
            unit: Unit::Samples,
            amount: samples,
        }
    }

    /// Which class was redacted.
    #[must_use]
    pub const fn class(self) -> DataClass {
        self.class
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let unit = match self.unit {
            Unit::Octets => "octets",
            Unit::Samples => "samples",
        };
        write!(f, "<{}: {} {unit}>", self.class, self.amount)
    }
}

impl fmt::Debug for Redacted {
    /// The same as [`fmt::Display`], because a `Debug` that differed would be the one a log record
    /// actually reached.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// §11.2: every class is classified, and user data lives for the operation and no longer.
    #[test]
    fn user_data_defaults_to_the_live_operation() {
        for class in DataClass::ALL {
            let expected = if class.user_data() {
                Retention::LiveOperation
            } else {
                Retention::ProcessLifetime
            };
            assert_eq!(class.default_retention(), expected, "{class}");
            assert_eq!(class.opt_in().is_some(), class.user_data(), "{class}");
        }
    }

    /// §11.3: the default configures nothing, and admits exactly the provider that asks for
    /// nothing.
    #[test]
    fn the_default_admits_only_a_local_provider_that_keeps_nothing() {
        let default = SpeechPrivacy::default();
        assert_eq!(default, SpeechPrivacy::LOCAL_NO_RETENTION);
        assert_eq!(default.configured().count(), 0);
        assert!(default.admits(ProviderPrivacy::LOCAL_OFFLINE));
        for opt_in in RetentionOptIn::ALL {
            let declared = ProviderPrivacy::LOCAL_OFFLINE.declaring(opt_in, true);
            assert!(
                !default.admits(declared),
                "{opt_in} was admitted by default"
            );
            assert!(
                default.allowing(opt_in).admits(declared),
                "{opt_in} was not admitted by its own opt-in"
            );
            assert_eq!(
                declared.required_opt_ins().collect::<Vec<_>>(),
                vec![opt_in]
            );
        }
    }

    /// §11.4: a redaction reports the class and the size, and a secret reports neither.
    #[test]
    fn a_redaction_reports_the_shape_and_never_the_value() {
        assert_eq!(
            Redacted::octets(DataClass::Transcript, 12).to_string(),
            "<transcript: 12 octets>"
        );
        assert_eq!(
            format!("{:?}", Redacted::samples(DataClass::CallAudio, 160)),
            "<call audio: 160 samples>"
        );
        let secret = Secret::new("token");
        assert_eq!(format!("{secret:?}"), "[redacted]");
        assert_eq!(secret.to_string(), "[redacted]");
        assert_eq!(*secret.expose(), "token");
    }

    /// §11.3: an admission names what is in force, which is what the provider declared — a
    /// configured opt-in nothing uses is not one.
    #[test]
    fn an_admission_reports_what_is_in_force_not_what_was_permitted() {
        let everything = RetentionOptIn::ALL
            .into_iter()
            .fold(SpeechPrivacy::LOCAL_NO_RETENTION, SpeechPrivacy::allowing);
        let admission = SpeechAdmission::new(
            ProviderId::new("inert").unwrap(),
            ProviderKind::Recognition,
            ProviderPrivacy::LOCAL_OFFLINE,
            everything,
        );
        assert_eq!(admission.engaged().count(), 0);
        assert_eq!(admission.policy().configured().count(), 4);
        assert_eq!(
            admission.to_string(),
            "inert recognition engaged=[none] \
             configured=[debug-capture,persistent-derived-data,off-host-processing,network-egress]"
        );
    }
}
