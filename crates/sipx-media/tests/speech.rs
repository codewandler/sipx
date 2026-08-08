//! `docs/specs/speech-providers.md` §10's conformance vectors, run against an inert provider.
//!
//! Two things make this file the executable form of that specification rather than a unit test of
//! one module.
//!
//! **It is outside the crate.** Everything here reaches the contract through `sipx_media::speech`'s
//! public surface, exactly as a downstream provider would (§9: "the provider traits are public and
//! implementable downstream — that is the point of the contract"). An item this file cannot reach
//! is an item a replacement provider cannot reach either.
//!
//! **The provider recognises nothing and synthesises nothing.** It loads no model, opens no device,
//! reaches no network, and every "transcript" it emits is the empty string. That is deliberate: the
//! vectors prove the contract is executable, and a contract that needed a working recogniser to be
//! checked would be a capability claim rather than a contract.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_media::speech::{
    CancelReason, CancelScope, Conversion, DeadlineKind, Device, DeviceKind, DeviceRequirement,
    FailureCause, FallbackEngaged, LanguageRange, LanguageTag, LocalityPolicy, LossCause,
    Malformed, ProviderDescriptor, ProviderId, ProviderKind, ProviderRegistry, RecognitionDriver,
    RecognitionFrame, RecognitionInput, RecognitionOutput, RecognitionSession, RefusalReason,
    RequestId, Resources, SampleSpan, Selected, SelectionContext, SelectionDocument,
    SelectionError, SpeechBounds, SpeechPolicy, SynthesisChunk, SynthesisDriver, SynthesisInput,
    SynthesisOutput, SynthesisRefusal, SynthesisSession, Utterance, UtteranceId, Voice, VoiceToken,
    recognition_inputs,
};
use sipx_media::{
    AudioDirection, Codec, Config, DiscontinuityKind, MediaPort, MediaSession, Pcm, PcmEncoding,
    PcmFormat, Processing,
};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

const SAMPLES_PER_PACKET: usize = 160;

/// A bound on failure for audio crossing loopback, orders of magnitude above the honest answer on
/// an idle machine. Nothing in these vectors is *measured* in a duration: every ordering claim is
/// asserted on event order, which is what §8 requires.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

/// One µ-law packet, so a decoded frame is recognisable by value.
fn packet(sequence: u16) -> Bytes {
    Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * SAMPLES_PER_PACKET as u32,
        0x2c54_0001,
        Bytes::from(vec![0xFFu8; SAMPLES_PER_PACKET]),
    )
    .encode()
}

async fn feed(peer: &UdpSocket, to: SocketAddr, packets: u16) {
    for sequence in 0..packets {
        peer.send_to(&packet(sequence), to).await.expect("sends");
    }
}

fn id(token: &str) -> ProviderId {
    ProviderId::new(token).expect("a lowercase identity token")
}

fn tag(token: &str) -> LanguageTag {
    LanguageTag::new(token).expect("an RFC 5646 tag")
}

fn range(token: &str) -> LanguageRange {
    LanguageRange::new(token).expect("an RFC 4647 range")
}

fn voice_token(token: &str) -> VoiceToken {
    VoiceToken::new(token).expect("a lowercase voice token")
}

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

fn wideband() -> PcmFormat {
    PcmFormat::new(16_000, PcmEncoding::Signed16).expect("a supported format")
}

/// The inert providers need nothing to run, and say so.
fn estimates() -> Resources {
    Resources::new()
}

/// The inert recogniser: offline, wideband and narrowband, `en` and `de`, CPU only.
fn inert_recognition() -> ProviderDescriptor {
    ProviderDescriptor::recognition(id("inert"), "0")
        .language(tag("en"))
        .language(tag("de"))
        .accepted_format(wideband())
        .accepted_format(narrowband())
        .streaming(true)
        .device(Device::new(DeviceKind::Cpu).with_concurrent_sessions(1))
        .resources(estimates())
        .build()
}

/// The inert synthesiser: offline, one voice, narrowband only.
fn inert_synthesis() -> ProviderDescriptor {
    ProviderDescriptor::synthesis(id("inert-voice"), "0")
        .language(tag("en"))
        .voice(
            Voice::new(voice_token("flat"))
                .language(tag("en"))
                .property("monotone"),
        )
        .emitted_format(narrowband())
        .streaming(true)
        .device(Device::new(DeviceKind::Cpu).with_concurrent_sessions(1))
        .resources(estimates())
        .build()
}

fn registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry
        .register(inert_recognition())
        .expect("a fresh identity registers");
    registry
        .register(inert_synthesis())
        .expect("a fresh identity registers");
    registry
}

fn recognition_document(provider: &str, language: &str) -> SelectionDocument {
    SelectionDocument::new()
        .with_provider(id(provider))
        .with_language(range(language))
}

fn synthesis_document(provider: &str, language: &str, voice: &str) -> SelectionDocument {
    recognition_document(provider, language).with_voice(voice_token(voice))
}

/// A session on loopback, and a raw socket standing in for the far end.
async fn session_and_peer() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");
    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();
    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

/// Run one document as an endpoint default and return the single candidate's refusal reason.
fn refuse(document: SelectionDocument, context: &SelectionContext<'_>) -> RefusalReason {
    let kind = if document.voice().is_some() {
        ProviderKind::Synthesis
    } else {
        ProviderKind::Recognition
    };
    let policy = SpeechPolicy::new().with_default(kind, document);
    match policy.select(kind, None, context) {
        Err(SelectionError::Refused(refusals)) => {
            assert_eq!(refusals.len(), 1, "an absent chain makes a refusal final");
            refusals[0].reason().clone()
        }
        other => panic!("expected a refused candidate, got {other:?}"),
    }
}

/// A-39 acceptance row 3, and §4's evaluation order.
///
/// Every step of the table refuses with its own reason, carrying the provider id, the requested
/// value and the descriptor facts consulted — so a host can tell "the provider is not registered"
/// from "the provider will not speak that language" without parsing a string.
///
/// The second half is the part that has to be a test rather than a reading: **a refusal takes no
/// call resource.** The seam bounds a session at eight attachments
/// (`docs/specs/call-audio-seam.md` §5), so after every refusal above, all eight are still
/// available. Had any refusal reached `attach_processor` first, the eighth would be refused
/// `TooManyProcessors` and this would be red.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn every_selection_refusal_is_distinguishable_and_takes_no_call_resource() {
    let registry = registry();
    let (session, _peer, _addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");

    // SEL-7: nothing configured. Rank 3 of the precedence table.
    let empty = SpeechPolicy::new();
    assert_eq!(
        empty.select(ProviderKind::Recognition, None, &context),
        Err(SelectionError::NoProviderConfigured)
    );

    // SEL-10: well-formedness is a property of the document alone, checked before any registry read.
    for (document, expected) in [
        (
            SelectionDocument::new().with_language(range("en")),
            Malformed::MissingProvider,
        ),
        (
            SelectionDocument::new().with_provider(id("inert")),
            Malformed::MissingLanguage,
        ),
    ] {
        let policy = SpeechPolicy::new().with_default(ProviderKind::Recognition, document);
        assert_eq!(
            policy.select(ProviderKind::Recognition, None, &context),
            Err(SelectionError::Malformed(expected))
        );
    }
    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Synthesis,
        recognition_document("inert-voice", "en"),
    );
    assert_eq!(
        policy.select(ProviderKind::Synthesis, None, &context),
        Err(SelectionError::Malformed(Malformed::MissingVoice)),
        "SEL-10: `voice` is required for synthesis so that no machinery ever chooses one"
    );
    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        synthesis_document("inert", "en", "flat"),
    );
    assert_eq!(
        policy.select(ProviderKind::Recognition, None, &context),
        Err(SelectionError::Malformed(Malformed::VoiceNotForRecognition))
    );

    // Steps 1 to 6 of §4's evaluation table, each refusing with its own reason.
    let step_1 = refuse(recognition_document("absent", "en"), &context);
    assert!(matches!(step_1, RefusalReason::UnknownProvider { .. }));

    let off_host = ProviderDescriptor::recognition(id("elsewhere"), "0")
        .off_host(true)
        .network(true)
        .language(tag("en"))
        .accepted_format(narrowband())
        .resources(estimates())
        .build();
    let mut wider = registry.clone();
    wider.register(off_host).expect("DIS-3: it is registerable");
    let elsewhere = SelectionContext::new(&wider, 8_000).expect("valid");
    let step_2 = refuse(recognition_document("elsewhere", "en"), &elsewhere);
    assert!(
        matches!(
            step_2,
            RefusalReason::LocalityRefused {
                off_host: true,
                network: true
            }
        ),
        "DIS-3: an off-host provider is selectable only past the explicit host opt-in"
    );

    let step_3 = refuse(recognition_document("inert", "fr"), &context);
    let RefusalReason::UnsupportedLanguage {
        requested,
        declared,
    } = &step_3
    else {
        panic!("SEL-3 expects UnsupportedLanguage, got {step_3:?}");
    };
    assert_eq!(requested, &range("fr"));
    assert_eq!(declared.as_slice(), &[tag("en"), tag("de")]);

    let step_4 = refuse(synthesis_document("inert-voice", "en", "absent"), &context);
    assert!(matches!(step_4, RefusalReason::UnsupportedVoice { .. }));

    let step_5 = refuse(
        recognition_document("inert", "en")
            .with_format(wideband())
            .with_conversion(Conversion::Deny),
        &context,
    );
    assert!(
        matches!(step_5, RefusalReason::UnsupportedFormat { .. }),
        "SEL-5: under `deny` the pin must be the call clock's own format"
    );

    let step_6 = refuse(
        recognition_document("inert", "en")
            .with_device(DeviceRequirement::new(DeviceKind::Accelerator)),
        &context,
    );
    assert!(
        matches!(step_6, RefusalReason::UnsupportedDevice { .. }),
        "SEL-6: no silent CPU substitution"
    );

    // Each reason is distinguishable from every other, which is the property a host branches on.
    let reasons = [step_1, step_2, step_3, step_4, step_5, step_6];
    for (first, second) in reasons.iter().zip(reasons.iter().skip(1)) {
        assert_ne!(
            std::mem::discriminant(first),
            std::mem::discriminant(second)
        );
    }

    // The call never heard about any of it: all eight seam attachments are still available.
    let held: Vec<_> = (0..8)
        .map(|slot| {
            session
                .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
                .unwrap_or_else(|error| {
                    panic!("a refusal consumed seam attachment {slot}: {error}")
                })
        })
        .collect();
    assert_eq!(held.len(), 8);

    session.shutdown().await;
}

// ---------------------------------------------------------------------------------------------
// The inert providers.
//
// §10's deterministic test provider, for both contract kinds: an explicit script keyed to input
// counts, no accelerator, no model, no network, no runtime. Every transcript is the empty string
// and every synthesized sample is silence, so the vectors below check the *contract* and can never
// be read as evidence that this crate can transcribe or speak.
// ---------------------------------------------------------------------------------------------

/// How far through stopping a session is.
///
/// One value rather than two booleans, because "draining" and "stopped" are states of one
/// transition and a pair of flags can represent a fourth thing that does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// Still consuming inputs.
    Running,
    /// Everything terminal has been emitted; `Stopped` follows the drain.
    Draining,
    /// `Stopped` has been emitted. Nothing follows it.
    Stopped,
}

/// One scripted action, performed after the recogniser consumes one frame.
///
/// A script shorter than the frame count simply runs out: a frame with no action is consumed and
/// emits nothing, which is how these vectors feed audio without provoking an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recognise {
    /// Open an utterance at revision 1.
    Open,
    /// Revise the open utterance.
    Revise,
    /// Terminate the open utterance with a result.
    Finish,
    /// Fail the engine mid-utterance.
    Fail,
    /// Lose the engine or its device.
    Lose(LossCause),
}

/// The open utterance's accumulating state.
#[derive(Debug)]
struct Open {
    id: UtteranceId,
    revision: u32,
    start: u64,
    samples: u64,
    discontinuities: u32,
}

/// A recognition session that recognises nothing, deterministically.
#[derive(Debug)]
struct InertRecognition {
    script: VecDeque<Recognise>,
    outputs: VecDeque<RecognitionOutput>,
    generation: u64,
    ready: bool,
    flushed: bool,
    stop: Stop,
    /// Never emit `Stopped`, so the drain has nothing to complete on. LIF-6's wedged provider.
    wedged: bool,
    open: Option<Open>,
    next_id: UtteranceId,
    consumed: u64,
}

impl InertRecognition {
    /// Start a session. `Warming` is its first output, as §7 requires.
    fn new(script: impl IntoIterator<Item = Recognise>) -> Self {
        Self {
            script: script.into_iter().collect(),
            outputs: VecDeque::from([RecognitionOutput::Warming]),
            generation: 1,
            ready: false,
            flushed: false,
            stop: Stop::Running,
            wedged: false,
            open: None,
            next_id: UtteranceId::FIRST,
            consumed: 0,
        }
    }

    /// Wedge the session: everything terminal is emitted, and `Stopped` never follows.
    ///
    /// This is LIF-6's subject. It is a provider defect by construction — §5 makes `Stopped` the
    /// session's last output and requires it — and the point of the vector is that a defective
    /// provider becomes a reported abort rather than a driver that never finishes.
    fn wedged(mut self) -> Self {
        self.wedged = true;
        self
    }

    /// Signal readiness. Result outputs are allowed only after this.
    fn warm(&mut self) {
        if !self.ready && self.stop == Stop::Running {
            self.ready = true;
            self.outputs.push_back(RecognitionOutput::Ready);
        }
    }

    /// Begin stopping: `Stopped` is emitted once everything already queued has been drained.
    fn drain(&mut self) {
        if self.stop == Stop::Running {
            self.stop = Stop::Draining;
        }
    }

    /// The generation the driver must quote when it fires a deadline.
    fn generation(&self) -> u64 {
        self.generation
    }

    /// Resolve the open utterance, if any, with a result.
    fn finish(&mut self) {
        if let Some(open) = self.open.take() {
            self.outputs
                .push_back(RecognitionOutput::Final(utterance(&open)));
            self.next_id = open.id.next();
        }
    }

    /// Resolve the open utterance, if any, as cancelled.
    fn cancel_open(&mut self, reason: CancelReason) {
        if let Some(open) = self.open.take() {
            self.outputs.push_back(RecognitionOutput::Cancelled {
                utterance: open.id,
                reason,
            });
            self.next_id = open.id.next();
        }
    }

    fn act(&mut self, action: Recognise, samples: u64) {
        match action {
            Recognise::Open if self.ready && self.open.is_none() => {
                let open = Open {
                    id: self.next_id,
                    revision: 1,
                    start: self.consumed.saturating_sub(samples),
                    samples,
                    discontinuities: 0,
                };
                self.outputs
                    .push_back(RecognitionOutput::Partial(utterance(&open)));
                self.open = Some(open);
            }
            Recognise::Revise if self.ready => {
                if let Some(open) = self.open.as_mut() {
                    open.revision += 1;
                }
                if let Some(open) = self.open.as_ref() {
                    self.outputs
                        .push_back(RecognitionOutput::Replacement(utterance(open)));
                }
            }
            Recognise::Finish if self.ready => self.finish(),
            Recognise::Fail => {
                self.cancel_open(CancelReason::SessionFailed);
                self.outputs
                    .push_back(RecognitionOutput::Failed(FailureCause::EngineFailed));
                self.drain();
            }
            Recognise::Lose(cause) => {
                self.cancel_open(CancelReason::ProviderLost);
                self.outputs.push_back(RecognitionOutput::Lost(cause));
                self.drain();
            }
            // A result action before `Ready` would put a result ahead of readiness, which §7
            // forbids; the fixture drops it rather than emitting something the contract refuses.
            Recognise::Open | Recognise::Revise | Recognise::Finish => {}
        }
    }
}

/// One revision of the open utterance, as the inert provider reports it: empty text, always.
fn utterance(open: &Open) -> Utterance {
    Utterance::new(
        open.id,
        open.revision,
        String::new(),
        SampleSpan::new(open.start, open.samples),
    )
    .with_discontinuities(open.discontinuities)
}

impl RecognitionSession for InertRecognition {
    fn deliver(&mut self, input: RecognitionInput) {
        if self.stop == Stop::Stopped {
            return;
        }
        match input {
            RecognitionInput::Frame(frame) => {
                if self.flushed {
                    // §5: a `Frame` after `Flush` is a driver defect.
                    self.cancel_open(CancelReason::SessionFailed);
                    self.outputs
                        .push_back(RecognitionOutput::Failed(FailureCause::ProtocolViolation));
                    self.drain();
                    return;
                }
                let samples = frame.pcm().samples().len() as u64;
                self.consumed += samples;
                if let Some(open) = self.open.as_mut() {
                    open.samples += samples;
                }
                if let Some(action) = self.script.pop_front() {
                    self.act(action, samples);
                }
            }
            RecognitionInput::Discontinuity { samples, .. } => {
                // §10: the deterministic provider terminates its open utterance at every
                // discontinuity, which is what makes these vectors exact.
                if let Some(open) = self.open.as_mut() {
                    open.discontinuities += 1;
                }
                self.finish();
                self.consumed += samples;
            }
            RecognitionInput::Flush => {
                self.flushed = true;
                self.finish();
                self.drain();
            }
            RecognitionInput::Cancel(reason) => {
                self.cancel_open(reason);
                self.drain();
            }
            RecognitionInput::DeadlineFired { kind, generation } => {
                if generation == self.generation && kind == DeadlineKind::Warmup && !self.ready {
                    self.outputs
                        .push_back(RecognitionOutput::Failed(FailureCause::WarmupTimeout));
                    self.drain();
                }
            }
            _ => {
                // §9: an input a provider does not recognise fails the session rather than being
                // guessed at.
                self.outputs
                    .push_back(RecognitionOutput::Failed(FailureCause::ProtocolViolation));
                self.drain();
            }
        }
    }

    fn poll_output(&mut self) -> Option<RecognitionOutput> {
        if let Some(output) = self.outputs.pop_front() {
            return Some(output);
        }
        if self.stop == Stop::Draining && !self.wedged {
            self.stop = Stop::Stopped;
            return Some(RecognitionOutput::Stopped { aborted: false });
        }
        None
    }
}

/// A synthesis session that synthesizes silence, deterministically.
#[derive(Debug)]
struct InertSynthesis {
    outputs: VecDeque<SynthesisOutput>,
    bounds: SpeechBounds,
    format: PcmFormat,
    /// Chunks each accepted request produces.
    chunks: usize,
    /// Emit a production gap after this many chunks of the started request.
    gap_after: Option<usize>,
    /// Fail the engine after this many chunks of the started request.
    fail_after: Option<usize>,
    generation: u64,
    ready: bool,
    stop: Stop,
    /// Never emit `Stopped`. LIF-6's wedged provider, on the synthesis side.
    wedged: bool,
    queued: VecDeque<RequestId>,
    started: Option<(RequestId, usize, u64)>,
    outstanding: usize,
}

impl InertSynthesis {
    const CHUNK_SAMPLES: u64 = 160;

    fn new(chunks: usize) -> Self {
        Self {
            outputs: VecDeque::from([SynthesisOutput::Warming]),
            bounds: SpeechBounds::DEFAULTS,
            format: narrowband(),
            chunks,
            gap_after: None,
            fail_after: None,
            generation: 1,
            ready: false,
            stop: Stop::Running,
            wedged: false,
            queued: VecDeque::new(),
            started: None,
            outstanding: 0,
        }
    }

    /// Wedge the session: `Stopped` never follows the drain. LIF-6's subject.
    fn wedged(mut self) -> Self {
        self.wedged = true;
        self
    }

    fn with_bounds(mut self, bounds: SpeechBounds) -> Self {
        self.bounds = bounds;
        self
    }

    fn with_gap_after(mut self, chunks: usize) -> Self {
        self.gap_after = Some(chunks);
        self
    }

    fn with_failure_after(mut self, chunks: usize) -> Self {
        self.fail_after = Some(chunks);
        self
    }

    fn warm(&mut self) {
        if !self.ready && self.stop == Stop::Running {
            self.ready = true;
            self.outputs.push_back(SynthesisOutput::Ready);
            self.start_next();
        }
    }

    /// Begin stopping: `Stopped` is emitted once everything already queued has been drained.
    fn drain(&mut self) {
        if self.stop == Stop::Running {
            self.stop = Stop::Draining;
        }
    }

    /// Whether the session has ended, so an `Enqueue` is refused rather than queued.
    fn ended(&self) -> bool {
        self.stop != Stop::Running
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn silence(&self) -> Pcm {
        Pcm::from_i16(self.format, vec![0; Self::CHUNK_SAMPLES as usize])
    }

    fn start_next(&mut self) {
        if self.started.is_some() || !self.ready || self.ended() {
            return;
        }
        if let Some(request) = self.queued.pop_front() {
            self.outputs.push_back(SynthesisOutput::Started { request });
            self.started = Some((request, 0, 0));
            self.produce();
        }
    }

    /// Produce chunks up to the granted window; §6's production bound.
    fn produce(&mut self) {
        while let Some((request, produced, offset)) = self.started {
            if produced >= self.chunks {
                self.outputs.push_back(SynthesisOutput::Completed {
                    request,
                    samples: offset,
                });
                self.started = None;
                self.start_next();
                return;
            }
            if self.fail_after == Some(produced) {
                self.outputs.push_back(SynthesisOutput::Failed {
                    request: Some(request),
                    cause: FailureCause::EngineFailed,
                });
                self.started = None;
                self.fail_session();
                return;
            }
            if self.outstanding >= self.bounds.chunk_window() {
                return;
            }
            let mut offset = offset;
            if self.gap_after == Some(produced) {
                self.outputs.push_back(SynthesisOutput::Discontinuity {
                    request,
                    samples: Self::CHUNK_SAMPLES,
                });
                offset += Self::CHUNK_SAMPLES;
                self.gap_after = None;
            }
            self.outputs
                .push_back(SynthesisOutput::Chunk(SynthesisChunk::new(
                    request,
                    produced as u64,
                    offset,
                    self.silence(),
                )));
            self.outstanding += 1;
            self.started = Some((request, produced + 1, offset + Self::CHUNK_SAMPLES));
        }
    }

    /// §6: a session that cannot continue cancels every queued request in order, then fails.
    fn fail_session(&mut self) {
        while let Some(request) = self.queued.pop_front() {
            self.outputs.push_back(SynthesisOutput::Cancelled {
                request,
                reason: CancelReason::SessionFailed,
            });
        }
        self.outputs.push_back(SynthesisOutput::Failed {
            request: None,
            cause: FailureCause::EngineFailed,
        });
        self.drain();
    }
}

impl SynthesisSession for InertSynthesis {
    // §6 has one arm per input and each is short; splitting them would put the request state
    // machine in four places to satisfy a line count.
    #[allow(clippy::too_many_lines)]
    fn deliver(&mut self, input: SynthesisInput) {
        if self.stop == Stop::Stopped {
            return;
        }
        match input {
            SynthesisInput::Enqueue {
                request,
                text,
                replace,
            } => {
                if self.ended() {
                    self.outputs.push_back(SynthesisOutput::Refused {
                        request,
                        reason: SynthesisRefusal::SessionEnded,
                    });
                    return;
                }
                if replace {
                    if let Some((started, ..)) = self.started.take() {
                        self.outputs.push_back(SynthesisOutput::Cancelled {
                            request: started,
                            reason: CancelReason::Replaced,
                        });
                        self.outstanding = 0;
                    }
                    while let Some(queued) = self.queued.pop_front() {
                        self.outputs.push_back(SynthesisOutput::Cancelled {
                            request: queued,
                            reason: CancelReason::Replaced,
                        });
                    }
                }
                if text.len() > self.bounds.request_text_octets() {
                    self.outputs.push_back(SynthesisOutput::Refused {
                        request,
                        reason: SynthesisRefusal::TextTooLarge,
                    });
                    return;
                }
                if self.queued.len() >= self.bounds.queued_requests() {
                    self.outputs.push_back(SynthesisOutput::Refused {
                        request,
                        reason: SynthesisRefusal::QueueFull,
                    });
                    return;
                }
                self.outputs.push_back(SynthesisOutput::Accepted {
                    request,
                    position: self.queued.len(),
                });
                self.queued.push_back(request);
                self.start_next();
            }
            SynthesisInput::Cancel { scope, reason } => match scope {
                CancelScope::Request(target) => {
                    if self.started.map(|(request, ..)| request) == Some(target) {
                        self.started = None;
                        self.outstanding = 0;
                        self.outputs.push_back(SynthesisOutput::Cancelled {
                            request: target,
                            reason,
                        });
                        self.start_next();
                    } else if let Some(position) =
                        self.queued.iter().position(|queued| *queued == target)
                    {
                        self.queued.remove(position);
                        self.outputs.push_back(SynthesisOutput::Cancelled {
                            request: target,
                            reason,
                        });
                    }
                    // Cancelling a request already terminal, unknown or refused is ignored.
                }
                CancelScope::Session => {
                    if let Some((request, ..)) = self.started.take() {
                        self.outputs
                            .push_back(SynthesisOutput::Cancelled { request, reason });
                    }
                    while let Some(request) = self.queued.pop_front() {
                        self.outputs
                            .push_back(SynthesisOutput::Cancelled { request, reason });
                    }
                    self.drain();
                }
                _ => {}
            },
            SynthesisInput::Drained { chunks, .. } => {
                self.outstanding = self.outstanding.saturating_sub(chunks as usize);
                self.produce();
            }
            SynthesisInput::DeadlineFired { kind, generation } => {
                if generation == self.generation && kind == DeadlineKind::Warmup && !self.ready {
                    while let Some(request) = self.queued.pop_front() {
                        self.outputs.push_back(SynthesisOutput::Cancelled {
                            request,
                            reason: CancelReason::SessionFailed,
                        });
                    }
                    self.outputs.push_back(SynthesisOutput::Failed {
                        request: None,
                        cause: FailureCause::WarmupTimeout,
                    });
                    self.drain();
                }
            }
            _ => {
                self.outputs.push_back(SynthesisOutput::Failed {
                    request: None,
                    cause: FailureCause::ProtocolViolation,
                });
                self.drain();
            }
        }
    }

    fn poll_output(&mut self) -> Option<SynthesisOutput> {
        if let Some(output) = self.outputs.pop_front() {
            return Some(output);
        }
        if self.stop == Stop::Draining && !self.wedged {
            self.stop = Stop::Stopped;
            return Some(SynthesisOutput::Stopped { aborted: false });
        }
        None
    }
}

fn drain_recognition(session: &mut InertRecognition) -> Vec<RecognitionOutput> {
    let mut seen = Vec::new();
    while let Some(output) = session.poll_output() {
        seen.push(output);
    }
    seen
}

fn drain_synthesis(session: &mut InertSynthesis) -> Vec<SynthesisOutput> {
    let mut seen = Vec::new();
    while let Some(output) = session.poll_output() {
        seen.push(output);
    }
    seen
}

/// One frame of silence for the pure-contract vectors, at the operating format.
fn silent_frame(sequence: u64, sample_time: u64) -> RecognitionInput {
    RecognitionInput::Frame(RecognitionFrame::new(
        AudioDirection::Inbound,
        Pcm::from_i16(narrowband(), vec![0; SAMPLES_PER_PACKET]),
        sample_time,
        sequence,
    ))
}

/// Deliver `count` consecutive silent frames.
fn feed_frames(session: &mut InertRecognition, count: u64) {
    for sequence in 0..count {
        session.deliver(silent_frame(sequence, sequence * SAMPLES_PER_PACKET as u64));
    }
}

// ---------------------------------------------------------------------------------------------
// Discovery — DIS-1 to DIS-3.
// ---------------------------------------------------------------------------------------------

/// DIS-1: every §3 field is reported, local/offline holds by property, and two reads agree.
#[test]
fn dis_1_discovery_reports_every_field_and_repeats_itself() {
    let registry = registry();
    assert_eq!(registry.discover(), registry.discover());
    assert_eq!(registry.len(), 2);

    let recogniser = registry
        .resolve(&id("inert"), ProviderKind::Recognition)
        .expect("registered");
    assert_eq!(recogniser.id(), &id("inert"));
    assert_eq!(recogniser.version(), "0");
    assert_eq!(recogniser.kind(), ProviderKind::Recognition);
    assert!(!recogniser.off_host());
    assert!(!recogniser.network());
    assert!(
        recogniser.is_local_offline(),
        "local/offline is the conjunction of two declared properties, never an identity"
    );
    assert_eq!(recogniser.languages(), &[tag("en"), tag("de")]);
    assert_eq!(recogniser.accepted_formats(), &[wideband(), narrowband()]);
    assert!(recogniser.emitted_formats().is_empty());
    assert!(recogniser.voices().is_empty());
    assert!(recogniser.streaming());
    assert_eq!(
        recogniser.devices(),
        &[Device::new(DeviceKind::Cpu).with_concurrent_sessions(1)]
    );
    assert_eq!(recogniser.resources(), estimates());

    let synthesiser = registry
        .resolve(&id("inert-voice"), ProviderKind::Synthesis)
        .expect("registered");
    assert_eq!(synthesiser.kind(), ProviderKind::Synthesis);
    assert_eq!(synthesiser.emitted_formats(), &[narrowband()]);
    assert!(synthesiser.accepted_formats().is_empty());
    let voice = &synthesiser.voices()[0];
    assert_eq!(voice.token(), &voice_token("flat"));
    assert_eq!(voice.languages(), &[tag("en")]);
    assert_eq!(voice.properties().len(), 1);
}

/// DIS-2: permuting the registration order changes neither discovery nor any selection outcome.
#[test]
fn dis_2_registration_order_carries_no_meaning() {
    let mut reversed = ProviderRegistry::new();
    reversed.register(inert_synthesis()).expect("registers");
    reversed.register(inert_recognition()).expect("registers");
    let forwards = registry();
    assert_eq!(forwards.discover(), reversed.discover());

    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        recognition_document("inert", "en"),
    );
    let one = SelectionContext::new(&forwards, 8_000).expect("valid");
    let other = SelectionContext::new(&reversed, 8_000).expect("valid");
    assert_eq!(
        policy.select(ProviderKind::Recognition, None, &one),
        policy.select(ProviderKind::Recognition, None, &other)
    );
}

/// DIS-3: an off-host provider is discoverable, and selectable only past the explicit opt-in.
#[test]
fn dis_3_an_off_host_provider_needs_the_host_opt_in() {
    let mut registry = registry();
    registry
        .register(
            ProviderDescriptor::recognition(id("elsewhere"), "0")
                .off_host(true)
                .network(true)
                .language(tag("en"))
                .accepted_format(narrowband())
                .resources(estimates())
                .build(),
        )
        .expect("registers");
    let descriptor = registry
        .resolve(&id("elsewhere"), ProviderKind::Recognition)
        .expect("visible in discovery");
    assert!(!descriptor.is_local_offline());

    let document = recognition_document("elsewhere", "en");
    let policy = SpeechPolicy::new().with_default(ProviderKind::Recognition, document);
    let refused = SelectionContext::new(&registry, 8_000).expect("valid");
    assert!(matches!(
        policy.select(ProviderKind::Recognition, None, &refused),
        Err(SelectionError::Refused(_))
    ));

    let admitted = refused.with_locality(
        LocalityPolicy::LOCAL_ONLY
            .allowing_off_host()
            .allowing_network(),
    );
    let selected = policy
        .select(ProviderKind::Recognition, None, &admitted)
        .expect("the explicit opt-in admits it");
    assert_eq!(selected.provider(), &id("elsewhere"));
}

// ---------------------------------------------------------------------------------------------
// Selection and precedence — SEL-1, SEL-2, SEL-5, SEL-8, SEL-9. SEL-3, SEL-4, SEL-6, SEL-7 and
// SEL-10 are the refusal-order test above.
// ---------------------------------------------------------------------------------------------

/// SEL-1: with only an endpoint default, that default is selected for both kinds.
#[test]
fn sel_1_the_endpoint_default_is_selected_for_both_kinds() {
    let registry = registry();
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let policy = SpeechPolicy::new()
        .with_default(
            ProviderKind::Recognition,
            recognition_document("inert", "en"),
        )
        .with_default(
            ProviderKind::Synthesis,
            synthesis_document("inert-voice", "en", "flat"),
        );

    let heard = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("selects");
    assert_eq!(heard.provider(), &id("inert"));
    assert_eq!(heard.language(), &tag("en"));
    assert_eq!(heard.voice(), None);
    assert_eq!(heard.position(), 0);
    assert!(heard.passed_over().is_empty());

    let spoken = policy
        .select(ProviderKind::Synthesis, None, &context)
        .expect("selects");
    assert_eq!(spoken.provider(), &id("inert-voice"));
    assert_eq!(spoken.voice(), Some(&voice_token("flat")));
    assert_eq!(spoken.format(), narrowband());
}

/// SEL-2: a per-call override replaces the endpoint default for that call and leaves the default
/// in place for every other one.
#[test]
fn sel_2_an_override_is_per_call() {
    let mut registry = registry();
    registry
        .register(
            ProviderDescriptor::recognition(id("spare"), "0")
                .language(tag("en"))
                .accepted_format(narrowband())
                .resources(estimates())
                .build(),
        )
        .expect("registers");
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        recognition_document("inert", "en"),
    );

    let overridden = policy
        .select(
            ProviderKind::Recognition,
            Some(&recognition_document("spare", "en")),
            &context,
        )
        .expect("selects");
    assert_eq!(overridden.provider(), &id("spare"));

    let other_call = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("selects");
    assert_eq!(
        other_call.provider(),
        &id("inert"),
        "the endpoint default is untouched for every other call"
    );
}

/// SEL-5: a pin outside the declared list refuses; the same pin with a conversion succeeds.
#[test]
fn sel_5_a_pin_is_checked_against_the_declared_list_and_the_conversion_policy() {
    let registry = registry();
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        recognition_document("inert", "en").with_format(wideband()),
    );
    let selected = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("`allow` plus an M-43 conversion selects");
    assert_eq!(selected.format(), wideband());
    assert_eq!(selected.conversion(), Conversion::Allow);

    let unlisted = PcmFormat::new(44_100, PcmEncoding::Signed16).expect("valid");
    let refusing = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        recognition_document("inert", "en").with_format(unlisted),
    );
    let refusal = refusing
        .select(ProviderKind::Recognition, None, &context)
        .expect_err("44.1 kHz is not in the declared list");
    let SelectionError::Refused(refusals) = &refusal else {
        panic!("expected a candidate refusal, got {refusal:?}");
    };
    let RefusalReason::UnsupportedFormat {
        pinned,
        conversion,
        call_clock,
        declared,
    } = refusals[0].reason()
    else {
        panic!("expected UnsupportedFormat, got {:?}", refusals[0].reason());
    };
    assert_eq!(*pinned, Some(unlisted));
    assert_eq!(*conversion, Conversion::Allow);
    assert_eq!(*call_clock, narrowband());
    assert_eq!(declared.as_slice(), &[wideband(), narrowband()]);
}

/// SEL-8: the first candidate's typed refusal is recorded, the second is selected, and every
/// constraint field is still the top-level document's.
#[test]
fn sel_8_a_fallback_chain_records_refusals_and_changes_no_policy_field() {
    let mut registry = ProviderRegistry::new();
    registry
        .register(
            ProviderDescriptor::recognition(id("german"), "0")
                .language(tag("de"))
                .accepted_format(narrowband())
                .resources(estimates())
                .build(),
        )
        .expect("registers");
    registry
        .register(
            ProviderDescriptor::recognition(id("english"), "0")
                .language(tag("en-GB"))
                .accepted_format(narrowband())
                .device(Device::new(DeviceKind::Cpu).with_concurrent_sessions(1))
                .resources(estimates())
                .build(),
        )
        .expect("registers");
    let context = SelectionContext::new(&registry, 8_000).expect("valid");

    let document = recognition_document("german", "en")
        .with_device(DeviceRequirement::new(DeviceKind::Cpu))
        .with_fallback([id("english")]);
    let policy = SpeechPolicy::new().with_default(ProviderKind::Recognition, document.clone());
    let selected = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("the second candidate satisfies");

    assert_eq!(selected.provider(), &id("english"));
    assert_eq!(selected.position(), 1);
    assert_eq!(selected.passed_over().len(), 1);
    assert_eq!(selected.passed_over()[0].provider(), &id("german"));
    assert!(matches!(
        selected.passed_over()[0].reason(),
        RefusalReason::UnsupportedLanguage { .. }
    ));

    assert_eq!(selected.language(), &tag("en-GB"));
    assert_eq!(selected.format(), narrowband());
    assert_eq!(selected.conversion(), document.conversion());
    assert_eq!(selected.device(), document.device());
    assert_eq!(selected.voice(), None);
}

/// SEL-9: an absent optional field means what §4's table says, never what the endpoint default
/// said. Field-level merging is what this refuses to do.
#[test]
fn sel_9_an_override_never_inherits_a_field_from_the_default() {
    let registry = registry();
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let strict = recognition_document("inert", "en")
        .with_conversion(Conversion::Deny)
        .with_device(DeviceRequirement::new(DeviceKind::Accelerator));
    let policy = SpeechPolicy::new().with_default(ProviderKind::Recognition, strict);

    let selected = policy
        .select(
            ProviderKind::Recognition,
            Some(&recognition_document("inert", "en")),
            &context,
        )
        .expect("the override omits both fields, so neither constrains it");
    assert_eq!(selected.conversion(), Conversion::Allow);
    assert_eq!(selected.device(), None);
    assert_eq!(
        selected.format(),
        wideband(),
        "`allow` takes the first declared format, which `deny` would have refused"
    );
}

// ---------------------------------------------------------------------------------------------
// Recognition — REC-1, REC-2, REC-4, REC-5, REC-6, REC-8 against the contract; REC-3 against the
// seam that carries it. REC-7 is a driver obligation with no driver in this story; see the
// module note at the end of this file.
// ---------------------------------------------------------------------------------------------

/// REC-1: identical inputs produce identical ordered outputs, twice.
#[test]
fn rec_1_the_provider_is_deterministic() {
    let run = || {
        let mut session =
            InertRecognition::new([Recognise::Open, Recognise::Revise, Recognise::Finish]);
        session.warm();
        feed_frames(&mut session, 3);
        drain_recognition(&mut session)
    };
    let first = run();
    assert_eq!(first, run());

    assert_eq!(first[0], RecognitionOutput::Warming);
    assert_eq!(first[1], RecognitionOutput::Ready);
    assert!(matches!(first[2], RecognitionOutput::Partial(_)));
    assert!(matches!(first[3], RecognitionOutput::Replacement(_)));
    assert!(matches!(first[4], RecognitionOutput::Final(_)));
    assert_eq!(first.len(), 5, "nothing follows the terminal: {first:?}");
}

/// REC-2: revisions increment by exactly one, every event carries the complete text, nothing
/// follows a terminal, and utterance identities strictly increase.
#[test]
fn rec_2_the_utterance_state_machine_holds() {
    let mut session = InertRecognition::new([
        Recognise::Open,
        Recognise::Revise,
        Recognise::Revise,
        Recognise::Finish,
        Recognise::Open,
        Recognise::Finish,
    ]);
    session.warm();
    feed_frames(&mut session, 6);
    let outputs = drain_recognition(&mut session);

    let mut revisions: Vec<(UtteranceId, u32)> = Vec::new();
    let mut terminated: Vec<UtteranceId> = Vec::new();
    for output in &outputs {
        let (utterance, terminal) = match output {
            RecognitionOutput::Partial(u) | RecognitionOutput::Replacement(u) => (u, false),
            RecognitionOutput::Final(u) => (u, true),
            _ => continue,
        };
        assert!(
            !terminated.contains(&utterance.id()),
            "an output followed a terminal for {:?}",
            utterance.id()
        );
        assert!(
            utterance.text().is_empty(),
            "the inert provider recognises nothing, and says so with complete text"
        );
        if let Some((previous, revision)) = revisions.last().copied()
            && previous == utterance.id()
        {
            if terminal {
                assert_eq!(
                    utterance.revision(),
                    revision,
                    "a terminal reports the utterance as it stands, it does not revise it"
                );
            } else {
                assert_eq!(
                    utterance.revision(),
                    revision + 1,
                    "revisions increment by one"
                );
            }
        } else {
            assert_eq!(utterance.revision(), 1, "an utterance opens at revision 1");
        }
        revisions.push((utterance.id(), utterance.revision()));
        if terminal {
            terminated.push(utterance.id());
        }
    }
    assert_eq!(terminated, vec![UtteranceId::FIRST, UtteranceId::new(1)]);
    assert!(
        terminated.windows(2).all(|pair| pair[1] > pair[0]),
        "identities strictly increase"
    );
}

/// REC-4: the open utterance terminates at the gap, every pre-gap output precedes every post-gap
/// one, and the next utterance carries a new identity.
#[test]
fn rec_4_a_discontinuity_terminates_the_open_utterance() {
    let mut session = InertRecognition::new([Recognise::Open, Recognise::Open, Recognise::Finish]);
    session.warm();
    feed_frames(&mut session, 1);
    session.deliver(RecognitionInput::Discontinuity {
        kind: DiscontinuityKind::Loss,
        frames: 2,
        samples: 320,
    });
    feed_frames(&mut session, 2);
    session.deliver(RecognitionInput::Flush);
    let outputs = drain_recognition(&mut session);

    let identities: Vec<UtteranceId> = outputs
        .iter()
        .filter_map(|output| match output {
            RecognitionOutput::Partial(u)
            | RecognitionOutput::Replacement(u)
            | RecognitionOutput::Final(u) => Some(u.id()),
            _ => None,
        })
        .collect();
    assert_eq!(
        identities,
        vec![
            UtteranceId::FIRST,
            UtteranceId::FIRST,
            UtteranceId::new(1),
            UtteranceId::new(1)
        ],
        "pre-gap outputs all precede post-gap outputs: {outputs:?}"
    );

    let terminal = outputs
        .iter()
        .find_map(|output| match output {
            RecognitionOutput::Final(u) if u.id() == UtteranceId::FIRST => Some(u.clone()),
            _ => None,
        })
        .expect("the open utterance terminated at the gap");
    assert_eq!(terminal.discontinuities(), 1);
    assert_eq!(
        outputs.last(),
        Some(&RecognitionOutput::Stopped { aborted: false })
    );
}

/// REC-5: one cancellation, one terminal, then `Stopped` — and nothing after it.
#[test]
fn rec_5_cancellation_resolves_the_open_utterance_exactly_once() {
    let mut session = InertRecognition::new([Recognise::Open]);
    session.warm();
    feed_frames(&mut session, 1);
    session.deliver(RecognitionInput::Cancel(CancelReason::Application));
    let outputs = drain_recognition(&mut session);

    let cancelled: Vec<&RecognitionOutput> = outputs
        .iter()
        .filter(|output| matches!(output, RecognitionOutput::Cancelled { .. }))
        .collect();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(
        cancelled[0],
        &RecognitionOutput::Cancelled {
            utterance: UtteranceId::FIRST,
            reason: CancelReason::Application
        }
    );
    assert_eq!(
        outputs.last(),
        Some(&RecognitionOutput::Stopped { aborted: false })
    );

    session.deliver(RecognitionInput::Flush);
    assert_eq!(session.poll_output(), None, "no output follows `Stopped`");
}

/// REC-6: an engine failure resolves the open utterance as cancelled, then fails the session.
#[test]
fn rec_6_a_failure_cancels_the_open_utterance_then_fails_the_session() {
    let mut session = InertRecognition::new([Recognise::Open, Recognise::Fail]);
    session.warm();
    feed_frames(&mut session, 2);
    let outputs = drain_recognition(&mut session);

    assert_eq!(
        &outputs[2..],
        &[
            RecognitionOutput::Partial(Utterance::new(
                UtteranceId::FIRST,
                1,
                String::new(),
                SampleSpan::new(0, 160)
            )),
            RecognitionOutput::Cancelled {
                utterance: UtteranceId::FIRST,
                reason: CancelReason::SessionFailed
            },
            RecognitionOutput::Failed(FailureCause::EngineFailed),
            RecognitionOutput::Stopped { aborted: false },
        ]
    );
}

/// REC-8: `Flush` resolves the utterance and stops; a `Frame` after it is a driver defect.
#[test]
fn rec_8_flush_drains_and_a_late_frame_is_a_protocol_violation() {
    let mut clean = InertRecognition::new([Recognise::Open]);
    clean.warm();
    feed_frames(&mut clean, 1);
    clean.deliver(RecognitionInput::Flush);
    let outputs = drain_recognition(&mut clean);
    assert!(matches!(outputs[3], RecognitionOutput::Final(_)));
    assert_eq!(outputs[4], RecognitionOutput::Stopped { aborted: false });

    let mut defective = InertRecognition::new([Recognise::Open]);
    defective.warm();
    feed_frames(&mut defective, 1);
    defective.deliver(RecognitionInput::Flush);
    defective.deliver(silent_frame(1, 160));
    let outputs = drain_recognition(&mut defective);
    assert!(
        outputs.contains(&RecognitionOutput::Failed(FailureCause::ProtocolViolation)),
        "a frame after flush fails the session: {outputs:?}"
    );
    assert_eq!(
        outputs.last(),
        Some(&RecognitionOutput::Stopped { aborted: false }),
        "`Stopped` is still the last output"
    );
}

// ---------------------------------------------------------------------------------------------
// Synthesis — SYN-1 to SYN-7.
// ---------------------------------------------------------------------------------------------

/// SYN-1: identical requests produce identical chunk payloads, in one contiguous stream.
#[test]
fn syn_1_a_request_produces_a_contiguous_chunk_stream() {
    let run = || {
        let mut session = InertSynthesis::new(3);
        session.warm();
        session.deliver(SynthesisInput::Enqueue {
            request: RequestId::new(0),
            text: String::new(),
            replace: false,
        });
        drain_synthesis(&mut session)
    };
    let first = run();
    assert_eq!(first, run(), "byte-identical outputs for identical inputs");

    assert_eq!(first[0], SynthesisOutput::Warming);
    assert_eq!(first[1], SynthesisOutput::Ready);
    assert_eq!(
        first[2],
        SynthesisOutput::Accepted {
            request: RequestId::new(0),
            position: 0
        }
    );
    assert_eq!(
        first[3],
        SynthesisOutput::Started {
            request: RequestId::new(0)
        }
    );
    let chunks: Vec<&SynthesisChunk> = first
        .iter()
        .filter_map(|output| match output {
            SynthesisOutput::Chunk(chunk) => Some(chunk),
            _ => None,
        })
        .collect();
    assert_eq!(chunks.len(), 3);
    for (index, pair) in chunks.windows(2).enumerate() {
        assert_eq!(pair[1].sequence(), pair[0].sequence() + 1, "chunk {index}");
        assert_eq!(
            pair[1].offset(),
            pair[0].offset() + pair[0].pcm().samples().len() as u64,
            "offsets are contiguous with no discontinuity between them"
        );
    }
    assert_eq!(
        first.last(),
        Some(&SynthesisOutput::Completed {
            request: RequestId::new(0),
            samples: 480
        })
    );
}

/// SYN-2: `replace` cancels the started request and every queued one, in queue order, before the
/// replacement is accepted.
#[test]
fn syn_2_replace_cancels_everything_before_the_new_request_is_accepted() {
    let bounds = SpeechBounds::DEFAULTS
        .with_chunk_window(1)
        .expect("one chunk is a window");
    let mut session = InertSynthesis::new(3).with_bounds(bounds);
    session.warm();
    for index in 0..2 {
        session.deliver(SynthesisInput::Enqueue {
            request: RequestId::new(index),
            text: String::new(),
            replace: false,
        });
    }
    session.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(2),
        text: String::new(),
        replace: true,
    });
    let outputs = drain_synthesis(&mut session);

    let ordered: Vec<&SynthesisOutput> = outputs
        .iter()
        .filter(|output| {
            matches!(
                output,
                SynthesisOutput::Cancelled { .. } | SynthesisOutput::Accepted { .. }
            )
        })
        .collect();
    let replaced: Vec<&SynthesisOutput> = ordered
        .iter()
        .copied()
        .skip_while(|output| !matches!(output, SynthesisOutput::Cancelled { .. }))
        .collect();
    assert_eq!(
        replaced[0],
        &SynthesisOutput::Cancelled {
            request: RequestId::new(0),
            reason: CancelReason::Replaced
        }
    );
    assert_eq!(
        replaced[1],
        &SynthesisOutput::Cancelled {
            request: RequestId::new(1),
            reason: CancelReason::Replaced
        }
    );
    assert_eq!(
        replaced[2],
        &SynthesisOutput::Accepted {
            request: RequestId::new(2),
            position: 0
        }
    );
    assert!(
        outputs.contains(&SynthesisOutput::Started {
            request: RequestId::new(2)
        }),
        "the replacement plays"
    );
}

/// SYN-3: the queue bound and the text bound each refuse with their own reason and disturb
/// nothing already queued.
#[test]
fn syn_3_the_request_bounds_refuse_by_type() {
    let bounds = SpeechBounds::DEFAULTS
        .with_chunk_window(1)
        .expect("valid")
        .with_queued_requests(2)
        .expect("valid")
        .with_request_text_octets(8)
        .expect("valid");
    let mut session = InertSynthesis::new(3).with_bounds(bounds);
    session.warm();
    for index in 0..3 {
        session.deliver(SynthesisInput::Enqueue {
            request: RequestId::new(index),
            text: String::new(),
            replace: false,
        });
    }
    session.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(3),
        text: String::new(),
        replace: false,
    });
    session.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(4),
        text: "far too much text".to_owned(),
        replace: false,
    });
    let outputs = drain_synthesis(&mut session);

    assert!(outputs.contains(&SynthesisOutput::Refused {
        request: RequestId::new(3),
        reason: SynthesisRefusal::QueueFull
    }));
    assert!(outputs.contains(&SynthesisOutput::Refused {
        request: RequestId::new(4),
        reason: SynthesisRefusal::TextTooLarge
    }));
    for index in 0..3 {
        assert!(
            outputs.iter().any(|output| matches!(
                output,
                SynthesisOutput::Accepted { request, .. } if *request == RequestId::new(index)
            )),
            "request {index} was queued and stays queued"
        );
    }
}

/// SYN-4: a production gap is named between chunks whose offsets are otherwise contiguous, and
/// the request still reaches exactly one terminal.
#[test]
fn syn_4_a_production_gap_is_named() {
    let mut session = InertSynthesis::new(3).with_gap_after(1);
    session.warm();
    session.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(0),
        text: String::new(),
        replace: false,
    });
    let outputs = drain_synthesis(&mut session);

    let gap = outputs
        .iter()
        .position(|output| matches!(output, SynthesisOutput::Discontinuity { .. }))
        .expect("the gap is named");
    assert_eq!(
        outputs[gap],
        SynthesisOutput::Discontinuity {
            request: RequestId::new(0),
            samples: 160
        }
    );

    let chunks: Vec<&SynthesisChunk> = outputs
        .iter()
        .filter_map(|output| match output {
            SynthesisOutput::Chunk(chunk) => Some(chunk),
            _ => None,
        })
        .collect();
    assert_eq!(
        chunks[1].offset(),
        chunks[0].offset() + 320,
        "the gap shows"
    );
    assert_eq!(chunks[2].offset(), chunks[1].offset() + 160, "and closes");

    let terminals = outputs
        .iter()
        .filter(|output| {
            matches!(
                output,
                SynthesisOutput::Completed { .. }
                    | SynthesisOutput::Cancelled { .. }
                    | SynthesisOutput::Failed { .. }
            )
        })
        .count();
    assert_eq!(terminals, 1);
}

/// SYN-5: cancelling the started request leaves the queue alone; a session cancel resolves it in
/// queue order.
#[test]
fn syn_5_cancellation_is_scoped() {
    let bounds = SpeechBounds::DEFAULTS.with_chunk_window(1).expect("valid");
    let mut session = InertSynthesis::new(3).with_bounds(bounds);
    session.warm();
    for index in 0..3 {
        session.deliver(SynthesisInput::Enqueue {
            request: RequestId::new(index),
            text: String::new(),
            replace: false,
        });
    }
    session.deliver(SynthesisInput::Cancel {
        scope: CancelScope::Request(RequestId::new(0)),
        reason: CancelReason::Application,
    });
    session.deliver(SynthesisInput::Cancel {
        scope: CancelScope::Session,
        reason: CancelReason::CallEnded,
    });
    let outputs = drain_synthesis(&mut session);

    let cancelled: Vec<(RequestId, CancelReason)> = outputs
        .iter()
        .filter_map(|output| match output {
            SynthesisOutput::Cancelled { request, reason } => Some((*request, *reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        cancelled,
        vec![
            (RequestId::new(0), CancelReason::Application),
            (RequestId::new(1), CancelReason::CallEnded),
            (RequestId::new(2), CancelReason::CallEnded),
        ]
    );
    assert_eq!(
        outputs.last(),
        Some(&SynthesisOutput::Stopped { aborted: false })
    );
}

/// SYN-6: a production failure fails the active request, cancels every queued one in order, then
/// fails the session.
#[test]
fn syn_6_a_production_failure_drains_the_queue_in_order() {
    let mut session = InertSynthesis::new(3).with_failure_after(1);
    for index in 0..3 {
        session.deliver(SynthesisInput::Enqueue {
            request: RequestId::new(index),
            text: String::new(),
            replace: false,
        });
    }
    session.warm();
    let outputs = drain_synthesis(&mut session);

    let tail: Vec<&SynthesisOutput> = outputs
        .iter()
        .skip_while(|output| !matches!(output, SynthesisOutput::Failed { .. }))
        .collect();
    assert_eq!(
        tail,
        vec![
            &SynthesisOutput::Failed {
                request: Some(RequestId::new(0)),
                cause: FailureCause::EngineFailed
            },
            &SynthesisOutput::Cancelled {
                request: RequestId::new(1),
                reason: CancelReason::SessionFailed
            },
            &SynthesisOutput::Cancelled {
                request: RequestId::new(2),
                reason: CancelReason::SessionFailed
            },
            &SynthesisOutput::Failed {
                request: None,
                cause: FailureCause::EngineFailed
            },
            &SynthesisOutput::Stopped { aborted: false },
        ]
    );
}

/// SYN-7: production stops at the chunk window and resumes only when credit returns.
#[test]
fn syn_7_production_never_runs_past_the_window() {
    let bounds = SpeechBounds::DEFAULTS.with_chunk_window(2).expect("valid");
    let mut session = InertSynthesis::new(5).with_bounds(bounds);
    session.warm();
    session.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(0),
        text: String::new(),
        replace: false,
    });

    let mut produced = 0usize;
    let mut rounds = 0usize;
    loop {
        let outputs = drain_synthesis(&mut session);
        let chunks = outputs
            .iter()
            .filter(|output| matches!(output, SynthesisOutput::Chunk(_)))
            .count();
        assert!(chunks <= 2, "outstanding chunks never exceed the window");
        produced += chunks;
        if outputs
            .iter()
            .any(|output| matches!(output, SynthesisOutput::Completed { .. }))
        {
            break;
        }
        assert!(chunks > 0, "credit was returned, so production resumed");
        session.deliver(SynthesisInput::Drained {
            request: RequestId::new(0),
            chunks: u32::try_from(chunks).expect("small"),
        });
        rounds += 1;
        assert!(
            rounds < 10,
            "the window releases production, it does not stall"
        );
    }
    assert_eq!(produced, 5);
}

// ---------------------------------------------------------------------------------------------
// Lifecycle — LIF-1 to LIF-5. LIF-6 is a driver obligation; see the note at the end of this file.
// ---------------------------------------------------------------------------------------------

/// LIF-1: `Warming` then `Ready` precede any result or `Started` output, for both kinds.
#[test]
fn lif_1_readiness_precedes_every_result() {
    let mut heard = InertRecognition::new([Recognise::Open]);
    heard.warm();
    feed_frames(&mut heard, 1);
    let outputs = drain_recognition(&mut heard);
    assert_eq!(outputs[0], RecognitionOutput::Warming);
    assert_eq!(outputs[1], RecognitionOutput::Ready);
    assert!(matches!(outputs[2], RecognitionOutput::Partial(_)));

    let mut spoken = InertSynthesis::new(1);
    spoken.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(0),
        text: String::new(),
        replace: false,
    });
    spoken.warm();
    let outputs = drain_synthesis(&mut spoken);
    let started = outputs
        .iter()
        .position(|output| matches!(output, SynthesisOutput::Started { .. }))
        .expect("it started");
    let ready = outputs
        .iter()
        .position(|output| *output == SynthesisOutput::Ready)
        .expect("it became ready");
    assert!(ready < started, "`Started` may not precede `Ready`");
}

/// LIF-2: the warm-up deadline resolves the queue in order, fails the session, and refuses a
/// later enqueue as ended rather than cancelling something that never existed. A stale generation
/// changes nothing.
#[test]
fn lif_2_the_warm_up_deadline_fails_the_session() {
    let mut session = InertSynthesis::new(1);
    for index in 0..2 {
        session.deliver(SynthesisInput::Enqueue {
            request: RequestId::new(index),
            text: String::new(),
            replace: false,
        });
    }
    session.deliver(SynthesisInput::DeadlineFired {
        kind: DeadlineKind::Warmup,
        generation: session.generation() + 1,
    });
    assert!(
        drain_synthesis(&mut session)
            .iter()
            .all(|output| !matches!(output, SynthesisOutput::Failed { .. })),
        "a fired deadline with a stale generation is ignored"
    );

    session.deliver(SynthesisInput::DeadlineFired {
        kind: DeadlineKind::Warmup,
        generation: session.generation(),
    });
    session.deliver(SynthesisInput::Enqueue {
        request: RequestId::new(2),
        text: String::new(),
        replace: false,
    });
    let outputs = drain_synthesis(&mut session);
    assert_eq!(
        outputs,
        vec![
            SynthesisOutput::Cancelled {
                request: RequestId::new(0),
                reason: CancelReason::SessionFailed
            },
            SynthesisOutput::Cancelled {
                request: RequestId::new(1),
                reason: CancelReason::SessionFailed
            },
            SynthesisOutput::Failed {
                request: None,
                cause: FailureCause::WarmupTimeout
            },
            SynthesisOutput::Refused {
                request: RequestId::new(2),
                reason: SynthesisRefusal::SessionEnded
            },
            SynthesisOutput::Stopped { aborted: false },
        ]
    );

    // The same deadline, on the other contract: no queue to resolve, so it is the session alone.
    let mut heard = InertRecognition::new([]);
    heard.deliver(RecognitionInput::DeadlineFired {
        kind: DeadlineKind::Warmup,
        generation: heard.generation() + 1,
    });
    assert_eq!(
        drain_recognition(&mut heard),
        vec![RecognitionOutput::Warming],
        "a stale generation is ignored on both contracts"
    );
    heard.deliver(RecognitionInput::DeadlineFired {
        kind: DeadlineKind::Warmup,
        generation: heard.generation(),
    });
    assert_eq!(
        drain_recognition(&mut heard),
        vec![
            RecognitionOutput::Failed(FailureCause::WarmupTimeout),
            RecognitionOutput::Stopped { aborted: false },
        ]
    );
}

/// LIF-3: loss with no chain resolves open work, reports the loss, stops — and starts nothing.
#[test]
fn lif_3_loss_without_a_chain_has_no_successor() {
    let mut session = InertRecognition::new([Recognise::Open, Recognise::Lose(LossCause::Device)]);
    session.warm();
    feed_frames(&mut session, 2);
    let outputs = drain_recognition(&mut session);

    assert_eq!(
        &outputs[3..],
        &[
            RecognitionOutput::Cancelled {
                utterance: UtteranceId::FIRST,
                reason: CancelReason::ProviderLost
            },
            RecognitionOutput::Lost(LossCause::Device),
            RecognitionOutput::Stopped { aborted: false },
        ]
    );
    assert_eq!(
        recognition_document("inert", "en").after_loss(),
        None,
        "an empty chain means a loss, like a refusal, is final"
    );
}

/// LIF-4: every output of the lost session precedes the successor's first, the fallback is
/// reported by identity, the policy fields are unchanged, and identities restart.
#[test]
fn lif_4_a_fallback_successor_is_a_new_session() {
    let mut registry = registry();
    registry
        .register(
            ProviderDescriptor::recognition(id("spare"), "0")
                .language(tag("en"))
                .accepted_format(wideband())
                .accepted_format(narrowband())
                .resources(estimates())
                .build(),
        )
        .expect("registers");
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let document = recognition_document("inert", "en").with_fallback([id("spare")]);
    let policy = SpeechPolicy::new().with_default(ProviderKind::Recognition, document.clone());
    let first = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("selects");
    assert_eq!(first.provider(), &id("inert"));

    let mut lost = InertRecognition::new([Recognise::Open, Recognise::Lose(LossCause::Engine)]);
    lost.warm();
    feed_frames(&mut lost, 2);
    let mut ordered = drain_recognition(&mut lost);
    assert_eq!(
        ordered.last(),
        Some(&RecognitionOutput::Stopped { aborted: false })
    );

    let successor_document = document.after_loss().expect("the chain has a candidate");
    let successor = policy
        .select(
            ProviderKind::Recognition,
            Some(&successor_document),
            &context,
        )
        .expect("the chain's first candidate satisfies");
    let engaged = FallbackEngaged::new(first.provider().clone(), &successor);
    assert_eq!(engaged.lost(), &id("inert"));
    assert_eq!(engaged.engaged(), &id("spare"));
    assert_eq!(engaged.language(), &tag("en"));

    assert_eq!(successor.language(), first.language());
    assert_eq!(successor.voice(), first.voice());
    assert_eq!(successor.conversion(), first.conversion());
    assert_eq!(successor.device(), first.device());
    assert_eq!(successor.format(), first.format());

    let mut next = InertRecognition::new([Recognise::Open, Recognise::Finish]);
    next.warm();
    feed_frames(&mut next, 2);
    let follow_on = drain_recognition(&mut next);
    let first_identity = follow_on.iter().find_map(|output| match output {
        RecognitionOutput::Partial(u) => Some(u.id()),
        _ => None,
    });
    assert_eq!(
        first_identity,
        Some(UtteranceId::FIRST),
        "identities do not carry across sessions"
    );
    // §7: every output of the lost session precedes the successor's first. In the two streams
    // concatenated, the boundary is the lost session's `Stopped` — it is the last thing that
    // session emits, and the first thing after it is the successor's `Warming`.
    let lost_len = ordered.len();
    ordered.extend(follow_on);
    assert_eq!(
        ordered.get(lost_len - 1),
        Some(&RecognitionOutput::Stopped { aborted: false }),
        "the lost session's last output is its `Stopped`"
    );
    assert_eq!(
        ordered.get(lost_len),
        Some(&RecognitionOutput::Warming),
        "and the successor's first output is the one immediately after it"
    );
    assert_eq!(
        ordered
            .iter()
            .filter(|output| matches!(output, RecognitionOutput::Stopped { .. }))
            .count(),
        1,
        "the successor is a new session, not a transition of the lost one: it has not stopped"
    );
}

/// LIF-5: call teardown is a cancellation, and it is distinguishable by type from every failure.
#[test]
fn lif_5_call_teardown_is_a_cancellation_not_a_failure() {
    let mut session = InertRecognition::new([Recognise::Open]);
    session.warm();
    feed_frames(&mut session, 1);
    session.deliver(RecognitionInput::Cancel(CancelReason::CallEnded));
    let outputs = drain_recognition(&mut session);

    assert!(outputs.contains(&RecognitionOutput::Cancelled {
        utterance: UtteranceId::FIRST,
        reason: CancelReason::CallEnded
    }));
    assert!(
        outputs
            .iter()
            .all(|output| !matches!(output, RecognitionOutput::Failed(_))),
        "SIP teardown is never reported as a provider failure: {outputs:?}"
    );
    assert_eq!(
        outputs.last(),
        Some(&RecognitionOutput::Stopped { aborted: false }),
        "the drain is observable as an event, never as an elapsed duration"
    );
}

// ---------------------------------------------------------------------------------------------
// The seam this contract rides — REC-3, and the disjointness §7 promises.
// ---------------------------------------------------------------------------------------------

/// REC-3: a stalled recognition session loses its oldest frames, is told exactly what it lost,
/// and never stalls the call.
///
/// This runs against a live `MediaSession` on purpose. §5 puts the input bound on the driver and
/// attributes the guarantee to `M-54`'s seam; the seam is where the bounded queue and the
/// `Overflow` discontinuity actually live, so proving it anywhere else would be proving it about a
/// second queue this contract does not have. [`recognition_inputs`] is the whole adaptation, and
/// the assertion is that it puts the break ahead of the audio that follows it.
#[tokio::test]
async fn rec_3_the_seam_bounds_the_input_and_names_what_it_dropped() {
    let registry = registry();
    let (session, peer, session_addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        recognition_document("inert", "en").with_conversion(Conversion::Deny),
    );
    let selected = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("selects");
    assert_eq!(selected.format(), narrowband());

    let bounds = SpeechBounds::DEFAULTS
        .with_input_frames(2)
        .expect("two frames is a bound");
    let mut processor = session
        .attach_processor(selected.processing(AudioDirection::Inbound, bounds))
        .expect("selection precedes attachment, and this one succeeded");

    feed(&peer, session_addr, 8).await;

    // The call is never blocked by a stalled recognition session.
    for _ in 0..8 {
        let frame = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
            .await
            .expect("the call keeps carrying audio")
            .expect("a frame");
        assert_eq!(frame.len(), SAMPLES_PER_PACKET);
    }

    let mut inputs = Vec::new();
    while let Some(frame) = processor.try_recv() {
        inputs.extend(recognition_inputs(frame));
    }

    let breaks: Vec<&RecognitionInput> = inputs
        .iter()
        .filter(|input| matches!(input, RecognitionInput::Discontinuity { .. }))
        .collect();
    assert_eq!(breaks.len(), 1, "exactly one discontinuity names the gap");
    assert_eq!(
        breaks[0],
        &RecognitionInput::Discontinuity {
            kind: DiscontinuityKind::Overflow,
            frames: 6,
            samples: 6 * SAMPLES_PER_PACKET as u64
        }
    );
    assert!(
        matches!(inputs[0], RecognitionInput::Discontinuity { .. }),
        "the break is delivered before the frame that follows it: {inputs:?}"
    );
    assert!(matches!(inputs[1], RecognitionInput::Frame(_)));
    assert_eq!(inputs.len(), 3, "two frames survived the bound: {inputs:?}");
    assert_eq!(processor.lost_frames(), 6);

    // And the session consumes them in that order without complaint.
    let mut provider = InertRecognition::new([Recognise::Open, Recognise::Finish]);
    provider.warm();
    for input in inputs {
        provider.deliver(input);
    }
    provider.deliver(RecognitionInput::Flush);
    let outputs = drain_recognition(&mut provider);
    assert!(
        outputs
            .iter()
            .all(|output| !matches!(output, RecognitionOutput::Failed(_))),
        "a bounded loss is not a protocol violation: {outputs:?}"
    );

    session.shutdown().await;
}

/// REC-6 and §7's disjointness, on a live call: speech failing is not the call failing.
///
/// The provider fails terminally, the session stops, and the call carries audio through all of it.
/// Nothing in the speech outputs is representable as a SIP status, and nothing about the call
/// changed — which is the property `A-26`/`A-27` will surface and `A-28` will bound.
#[tokio::test]
async fn a_speech_failure_leaves_the_call_established() {
    let registry = registry();
    let (session, peer, session_addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("valid");
    let policy = SpeechPolicy::new().with_default(
        ProviderKind::Recognition,
        recognition_document("inert", "en"),
    );
    let selected = policy
        .select(ProviderKind::Recognition, None, &context)
        .expect("selects");
    let processor = session
        .attach_processor(selected.processing(AudioDirection::Inbound, SpeechBounds::DEFAULTS))
        .expect("attaches");

    let mut provider = InertRecognition::new([Recognise::Open, Recognise::Fail]);
    provider.warm();
    feed_frames(&mut provider, 2);
    let outputs = drain_recognition(&mut provider);
    assert!(outputs.contains(&RecognitionOutput::Failed(FailureCause::EngineFailed)));
    assert_eq!(
        outputs.last(),
        Some(&RecognitionOutput::Stopped { aborted: false })
    );
    processor.detach();

    feed(&peer, session_addr, 4).await;
    let frame = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("the call is established and stays established")
        .expect("a frame");
    assert_eq!(frame.len(), SAMPLES_PER_PACKET);
    assert!(
        session
            .attach_processor(Processing::new(AudioDirection::Inbound, narrowband()))
            .is_ok(),
        "the session is still running after speech failed"
    );

    session.shutdown().await;
}

// ---------------------------------------------------------------------------------------------
// The driver §2 asks for — REC-7 and LIF-6, which `A-39` had nothing to run against (`A-40`).
// ---------------------------------------------------------------------------------------------

/// Read a driver's outputs until it reaches its terminal one.
async fn drain_driver(driver: &mut RecognitionDriver) -> Vec<RecognitionOutput> {
    let mut seen = Vec::new();
    // A bound on failure, the same one on every pass and never a definition of silence: the loop
    // leaves on the driver closing its output stream and on nothing else, so no assertion here
    // depends on how long a gap between two outputs was. A driver that never stops is precisely
    // what LIF-6 is about, and this turns that into a named failure instead of a hung suite.
    while let Some(output) = tokio::time::timeout(ARRIVAL_BOUND, driver.recv())
        .await
        .expect("the driver reaches its terminal output")
    {
        seen.push(output);
    }
    seen
}

/// The same, for a synthesis driver.
async fn drain_synthesis_driver(driver: &mut SynthesisDriver) -> Vec<SynthesisOutput> {
    let mut seen = Vec::new();
    // A bound on failure, on the same terms as `drain_driver` above: the loop's exit is the closed
    // output stream, and the duration decides only how a wedged driver is reported.
    while let Some(output) = tokio::time::timeout(ARRIVAL_BOUND, driver.recv())
        .await
        .expect("the driver reaches its terminal output")
    {
        seen.push(output);
    }
    seen
}

/// One selected recognition provider, pinned to the call clock so the seam converts nothing.
fn selected_recognition(context: &SelectionContext<'_>) -> Selected {
    SpeechPolicy::new()
        .with_default(
            ProviderKind::Recognition,
            recognition_document("inert", "en").with_conversion(Conversion::Deny),
        )
        .select(ProviderKind::Recognition, None, context)
        .expect("the inert recogniser satisfies its own document")
}

/// One selected synthesis provider.
fn selected_synthesis(context: &SelectionContext<'_>) -> Selected {
    SpeechPolicy::new()
        .with_default(
            ProviderKind::Synthesis,
            synthesis_document("inert-voice", "en", "flat"),
        )
        .select(ProviderKind::Synthesis, None, context)
        .expect("the inert synthesiser satisfies its own document")
}

/// REC-7, and A-40 acceptance rows 1 and 2: a consumer that reads nothing coalesces revisions to
/// the newest and loses no terminal, while the call carries audio throughout.
///
/// Eight packets into an eight-frame seam queue, so nothing is dropped here and the script runs in
/// full; the loss half of REC-7 is the vector after this one. §8's bound of four unconsumed
/// terminal-and-lifecycle outputs is reached exactly at the last `Final` (`Warming`, `Ready`, and
/// one terminal per utterance), which is what makes the driver stop consuming provider output
/// instead of growing its queue.
///
/// The surviving revision of each utterance is its **third**, carrying the whole text and the whole
/// span — the `Partial` that opened it and the revision after that were coalesced away. §5 permits
/// exactly that ("a coalesced or missed revision cannot leave a consumer permanently wrong"),
/// because no event is ever a delta.
#[tokio::test]
async fn rec_7_unconsumed_output_coalesces_to_the_newest_revision() {
    let registry = registry();
    let (session, peer, session_addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");
    let selected = selected_recognition(&context);
    assert_eq!(selected.format(), narrowband());

    let bounds = SpeechBounds::DEFAULTS
        .with_input_frames(8)
        .expect("eight frames is a bound")
        .with_unconsumed_outputs(4)
        .expect("four outputs is a bound")
        .with_pending_revisions(1)
        .expect("one revision is a bound");

    let mut provider = InertRecognition::new([
        Recognise::Open,
        Recognise::Revise,
        Recognise::Revise,
        Recognise::Finish,
        Recognise::Open,
        Recognise::Revise,
        Recognise::Revise,
        Recognise::Finish,
    ]);
    provider.warm();
    let mut driver = RecognitionDriver::attach(
        &session,
        &selected,
        AudioDirection::Inbound,
        bounds,
        provider,
    )
    .expect("selection precedes attachment, and this one succeeded");

    feed(&peer, session_addr, 8).await;
    // The consumer reads nothing until the audio has gone by. The call is unaffected: every packet
    // is decoded and delivered while the driver's output queue sits at its bound.
    for _ in 0..8 {
        let frame = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
            .await
            .expect("the call keeps carrying audio past a stalled speech consumer")
            .expect("a frame");
        assert_eq!(frame.len(), SAMPLES_PER_PACKET);
    }
    session.shutdown().await;

    let outputs = drain_driver(&mut driver).await;
    let samples = SAMPLES_PER_PACKET as u64;
    assert_eq!(
        outputs,
        vec![
            RecognitionOutput::Warming,
            RecognitionOutput::Ready,
            RecognitionOutput::Replacement(Utterance::new(
                UtteranceId::FIRST,
                3,
                String::new(),
                SampleSpan::new(0, 3 * samples)
            )),
            RecognitionOutput::Final(Utterance::new(
                UtteranceId::FIRST,
                3,
                String::new(),
                SampleSpan::new(0, 4 * samples)
            )),
            RecognitionOutput::Replacement(Utterance::new(
                UtteranceId::new(1),
                3,
                String::new(),
                SampleSpan::new(4 * samples, 3 * samples)
            )),
            RecognitionOutput::Final(Utterance::new(
                UtteranceId::new(1),
                3,
                String::new(),
                SampleSpan::new(4 * samples, 4 * samples)
            )),
            RecognitionOutput::Stopped { aborted: false },
        ],
        "one revision per utterance survives, both terminals do, and `Stopped` is last"
    );
    assert_eq!(
        driver.pending(),
        0,
        "nothing is owned after the last output"
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("the driver's task is joined rather than abandoned");
}

/// REC-7, second half: the output bound stops frame consumption, and the input degrades by REC-3's
/// policy — bounded, named loss rather than unbounded memory.
///
/// `Warming` and `Ready` are lifecycle outputs, so a bound of two is reached before the first frame
/// is ever consumed. The driver therefore consumes **no** audio at all while twelve packets go by,
/// which makes the arithmetic exact rather than approximate: a two-frame seam queue keeps the last
/// two and drops the other ten, and the single `Discontinuity(Overflow)` that names them puts the
/// surviving audio at sample time `10 × 160`. That number is the whole assertion — it is the
/// accumulated loss, reported to the session rather than silently absorbed.
#[tokio::test]
async fn rec_7_the_output_bound_degrades_the_input_by_the_rec_3_policy() {
    let registry = registry();
    let (session, peer, session_addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");
    let selected = selected_recognition(&context);

    let bounds = SpeechBounds::DEFAULTS
        .with_input_frames(2)
        .expect("two frames is a bound")
        .with_unconsumed_outputs(2)
        .expect("two outputs is a bound");

    let mut provider = InertRecognition::new([Recognise::Open, Recognise::Finish]);
    provider.warm();
    let mut driver = RecognitionDriver::attach(
        &session,
        &selected,
        AudioDirection::Inbound,
        bounds,
        provider,
    )
    .expect("attaches");

    feed(&peer, session_addr, 12).await;
    // The seam is offered a frame before the call delivers it, so twelve frames on the call side is
    // the happens-before for twelve frames offered to the attachment.
    for _ in 0..12 {
        tokio::time::timeout(ARRIVAL_BOUND, session.recv())
            .await
            .expect("the call keeps carrying audio")
            .expect("a frame");
    }
    session.shutdown().await;

    let outputs = drain_driver(&mut driver).await;
    let samples = SAMPLES_PER_PACKET as u64;
    let lost = 10 * samples;
    assert_eq!(
        outputs,
        vec![
            RecognitionOutput::Warming,
            RecognitionOutput::Ready,
            RecognitionOutput::Partial(Utterance::new(
                UtteranceId::FIRST,
                1,
                String::new(),
                SampleSpan::new(lost, samples)
            )),
            RecognitionOutput::Final(Utterance::new(
                UtteranceId::FIRST,
                1,
                String::new(),
                SampleSpan::new(lost, 2 * samples)
            )),
            RecognitionOutput::Stopped { aborted: false },
        ],
        "ten frames were dropped while the consumer stalled, and the gap named all ten"
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("the driver's task is joined");
}

/// A-40 acceptance row 1, synthesis half: a synthesis session runs to completion off the driver,
/// and §6's chunk window is what keeps it from running ahead.
///
/// The provider is scripted for six chunks and the window grants four, so the last two can only be
/// produced once the driver has returned credit — which it does when it hands a chunk to the
/// consumer, and at no other time. A driver that never returned credit would stop at chunk four and
/// this vector would hang out at its bound.
#[tokio::test]
async fn the_driver_runs_a_synthesis_session_to_completion_within_the_chunk_window() {
    let registry = registry();
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");
    let selected = selected_synthesis(&context);

    let mut provider = InertSynthesis::new(6);
    provider.warm();
    let mut driver = SynthesisDriver::spawn(&selected, SpeechBounds::DEFAULTS, provider)
        .expect("a synthesis selection starts a synthesis driver");
    assert_eq!(driver.format(), narrowband());

    let request = RequestId::new(0);
    assert!(
        driver.enqueue(request, "unspoken".to_owned(), false),
        "the driver's input queue accepts the request"
    );

    let mut outputs = Vec::new();
    loop {
        let output = tokio::time::timeout(ARRIVAL_BOUND, driver.recv())
            .await
            .expect("the request reaches its terminal")
            .expect("the session has not stopped");
        let completed = matches!(output, SynthesisOutput::Completed { .. });
        outputs.push(output);
        if completed {
            break;
        }
    }
    driver.cancel(CancelScope::Session, CancelReason::Shutdown);
    outputs.extend(drain_synthesis_driver(&mut driver).await);

    let chunks: Vec<&SynthesisChunk> = outputs
        .iter()
        .filter_map(|output| match output {
            SynthesisOutput::Chunk(chunk) => Some(chunk),
            _ => None,
        })
        .collect();
    assert_eq!(
        chunks.len(),
        6,
        "every chunk crosses the window: {outputs:?}"
    );
    for (position, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.sequence(), position as u64);
        assert_eq!(chunk.offset(), position as u64 * SAMPLES_PER_PACKET as u64);
    }
    assert!(outputs.contains(&SynthesisOutput::Started { request }));
    assert!(outputs.contains(&SynthesisOutput::Completed {
        request,
        samples: 6 * SAMPLES_PER_PACKET as u64
    }));
    assert_eq!(
        outputs.last(),
        Some(&SynthesisOutput::Stopped { aborted: false }),
        "a session that stops itself is not an aborted stop"
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("the driver's task is joined");
}

/// LIF-6: the drain deadline fired against a wedged provider yields an aborted `Stopped`, and a
/// clean drain does not.
///
/// Both halves run against the same bound, because the difference between them is the whole
/// vector: `Stopped { aborted: true }` is a reportable provider defect, and reading it as "the
/// session stopped" would lose the only report there is. The deadline is lowered to bound the
/// failure — §8's own words for what a deadline is — and neither half asserts on an elapsed
/// duration: the obedient session stops before the deadline can be reached at all, and the wedged
/// one is observed through the `Stopped` the driver emits in its place.
#[tokio::test]
async fn lif_6_the_drain_deadline_aborts_a_wedged_session() {
    let registry = registry();
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");
    let selected = selected_synthesis(&context);
    let bounds = SpeechBounds::DEFAULTS
        .with_drain(Duration::from_millis(50))
        .expect("fifty milliseconds is a bound");

    let mut wedged = InertSynthesis::new(1).wedged();
    wedged.warm();
    let mut driver = SynthesisDriver::spawn(&selected, bounds, wedged).expect("starts");
    driver.cancel(CancelScope::Session, CancelReason::Shutdown);
    let outputs = drain_synthesis_driver(&mut driver).await;
    assert_eq!(
        outputs.last(),
        Some(&SynthesisOutput::Stopped { aborted: true }),
        "the drain deadline expired, so the driver stopped the session itself: {outputs:?}"
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("an aborted session still joins its task");

    let mut obedient = InertSynthesis::new(1);
    obedient.warm();
    let mut driver = SynthesisDriver::spawn(&selected, bounds, obedient).expect("starts");
    driver.cancel(CancelScope::Session, CancelReason::Shutdown);
    let outputs = drain_synthesis_driver(&mut driver).await;
    assert_eq!(
        outputs.last(),
        Some(&SynthesisOutput::Stopped { aborted: false }),
        "a session that stops within the deadline is never marked aborted: {outputs:?}"
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("the driver's task is joined");
}

/// LIF-6 on the recognition side, on a live call: the same drain rule, the same abort.
///
/// The drain deadline is lowered to bound the failure rather than to schedule anything — the
/// assertion is on the `Stopped` the driver emits, never on an elapsed duration — and the call is
/// still established afterwards, because a wedged speech provider is not a call failure (§7).
#[tokio::test]
async fn lif_6_a_wedged_recognition_session_is_aborted_and_leaves_the_call_established() {
    let registry = registry();
    let (session, peer, session_addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");
    let selected = selected_recognition(&context);

    let bounds = SpeechBounds::DEFAULTS
        .with_drain(Duration::from_millis(50))
        .expect("fifty milliseconds is a bound");
    let mut provider = InertRecognition::new([]).wedged();
    provider.warm();
    let mut driver = RecognitionDriver::attach(
        &session,
        &selected,
        AudioDirection::Inbound,
        bounds,
        provider,
    )
    .expect("attaches");

    driver.cancel(CancelReason::Application);
    let outputs = drain_driver(&mut driver).await;
    assert_eq!(
        outputs.last(),
        Some(&RecognitionOutput::Stopped { aborted: true }),
        "the driver aborts the provider it cannot drain: {outputs:?}"
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("the aborted task is joined, not leaked");

    feed(&peer, session_addr, 4).await;
    let frame = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("the call is established and stays established")
        .expect("a frame");
    assert_eq!(frame.len(), SAMPLES_PER_PACKET);
    session.shutdown().await;
}

/// A-40 acceptance row 4, and LIF-5 on a live call: cancellation and call teardown each drop and
/// join the driver's task, and each is observed as an event.
///
/// Teardown reaches the session as `Cancel(CallEnded)` and never as a failure (§7), and the seam
/// hands over everything it already holds before it reports completion — so the frame fed below is
/// consumed first and its utterance is resolved as a cancellation rather than lost.
#[tokio::test]
async fn cancellation_and_call_teardown_join_every_driver_task() {
    let registry = registry();
    let (session, peer, session_addr) = session_and_peer().await;
    let context = SelectionContext::new(&registry, 8_000).expect("the call clock is a PCM rate");
    let selected = selected_recognition(&context);
    let bounds = SpeechBounds::DEFAULTS;

    let mut idle = InertRecognition::new([]);
    idle.warm();
    let mut cancelled =
        RecognitionDriver::attach(&session, &selected, AudioDirection::Inbound, bounds, idle)
            .expect("attaches");
    cancelled.cancel(CancelReason::Application);
    assert_eq!(
        drain_driver(&mut cancelled).await,
        vec![
            RecognitionOutput::Warming,
            RecognitionOutput::Ready,
            RecognitionOutput::Stopped { aborted: false },
        ]
    );
    tokio::time::timeout(ARRIVAL_BOUND, cancelled.join())
        .await
        .expect("cancellation joins the driver's task");

    let mut speaking = InertRecognition::new([Recognise::Open]);
    speaking.warm();
    let mut torn_down = RecognitionDriver::attach(
        &session,
        &selected,
        AudioDirection::Inbound,
        bounds,
        speaking,
    )
    .expect("attaches");
    feed(&peer, session_addr, 1).await;
    tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("the call carries the frame")
        .expect("a frame");
    session.shutdown().await;

    let outputs = drain_driver(&mut torn_down).await;
    let samples = SAMPLES_PER_PACKET as u64;
    assert_eq!(
        outputs,
        vec![
            RecognitionOutput::Warming,
            RecognitionOutput::Ready,
            RecognitionOutput::Partial(Utterance::new(
                UtteranceId::FIRST,
                1,
                String::new(),
                SampleSpan::new(0, samples)
            )),
            RecognitionOutput::Cancelled {
                utterance: UtteranceId::FIRST,
                reason: CancelReason::CallEnded,
            },
            RecognitionOutput::Stopped { aborted: false },
        ],
        "SIP teardown is a cancellation with its own reason, never a failure"
    );
    tokio::time::timeout(ARRIVAL_BOUND, torn_down.join())
        .await
        .expect("call teardown joins the driver's task");
}
