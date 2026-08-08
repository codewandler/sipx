//! Interchangeable speech providers (`docs/specs/speech-providers.md`, `A-25`).
//!
//! **Experimental** (`A-8`): this is the contract and nothing behind it. No recognition or
//! synthesis implementation, model, accelerator dependency or audio retention ships in this crate,
//! and nothing here makes a call able to transcribe or speak. What exists is the shape two
//! substitutable providers must have — the discovery descriptor, the registry, the selection state
//! machine and the two session contracts — so that `M-55`/`M-56` and downstream replacements can be
//! written against one document instead of against whichever implementation landed first.
//!
//! Three properties are worth knowing before reading further.
//!
//! **Nothing here does I/O.** Descriptors, documents, events, errors and the selection machine are
//! plain values (§2). There is no runtime, no socket, no device and no clock read: time enters as
//! sample counts on frames and as driver-fired deadlines carrying a generation. The one type here
//! that touches the call is [`Selected::processing`], and it *asks* the `M-54` seam for an
//! attachment rather than tapping media itself.
//!
//! **Call audio rides the one seam.** [`crate::processing`] is the only tap into call media
//! (`docs/specs/call-audio-seam.md`), and this contract consumes it: [`recognition_inputs`] turns
//! one [`PcmFrame`](crate::processing::PcmFrame) into the ordered
//! [`RecognitionInput`]s §5 requires, discontinuity first. Opening a second tap is forbidden by
//! both specs.
//!
//! **Selection happens before the call is touched.** [`SpeechPolicy::select`] reads documents and
//! descriptors and returns either a [`Selected`] or a typed refusal. Only a `Selected` can produce
//! the seam request, so an unknown, off-host or incompatible provider is refused before any queue
//! is allocated on the session.

mod bounds;
mod descriptor;
mod lifecycle;
mod recognition;
mod registry;
mod selection;
mod synthesis;

pub use bounds::{SpeechBounds, ZeroBound};
pub use descriptor::{
    Device, DeviceKind, DeviceRequirement, ForRecognition, ForSynthesis, InvalidToken,
    LanguageRange, LanguageTag, ProviderDescriptor, ProviderDescriptorBuilder, ProviderId,
    ProviderKind, Resources, Voice, VoiceProperty, VoiceToken,
};
pub use lifecycle::{CancelReason, DeadlineKind, FailureCause, LossCause};
pub use recognition::{
    FrameInputs, RecognitionFrame, RecognitionInput, RecognitionOutput, RecognitionSession,
    SampleSpan, Utterance, UtteranceId, recognition_inputs,
};
pub use registry::{ProviderRegistry, RegistrationError};
pub use selection::{
    Conversion, FallbackEngaged, LocalityPolicy, Malformed, Refusal, RefusalReason, Selected,
    SelectionContext, SelectionDocument, SelectionError, SpeechPolicy,
};
pub use synthesis::{
    CancelScope, RequestId, SynthesisChunk, SynthesisInput, SynthesisOutput, SynthesisRefusal,
    SynthesisSession,
};
