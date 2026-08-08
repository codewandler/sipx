//! Privacy and isolation for the speech contracts (`docs/specs/speech-providers.md` §11).
//!
//! A sibling of `speech.rs` rather than more of it. That file is §10's `DIS`/`SEL`/`REC`/`SYN`/`LIF`
//! vectors — what the contract *does* — and it is already the largest test in this crate. These are
//! §11's `PRV` vectors: what the contract refuses to keep, refuses to say, and refuses to share
//! between two calls. Splitting them keeps each file readable and makes the privacy claims findable
//! by name instead of by scrolling.
//!
//! Everything here goes through `sipx_media::speech`'s public surface, for the same reason
//! `speech.rs` does: a property a downstream provider cannot reach is a property it cannot uphold.
//!
//! **Nothing here recognises or synthesises anything.** The providers below load no model, open no
//! device and reach no network. What they *do* carry is a marker taken from their own call's audio,
//! because "one call cannot receive another call's data" is only a test if the two calls' data are
//! distinguishable values.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use sipx_media::speech::{
    CancelReason, CancelScope, Conversion, DataClass, FailureCause, ForRecognition, LanguageRange,
    LanguageTag, ProviderDescriptor, ProviderDescriptorBuilder, ProviderId, ProviderKind,
    ProviderPrivacy, ProviderRegistry, RecognitionDriver, RecognitionFrame, RecognitionInput,
    RecognitionOutput, RecognitionSession, RefusalReason, RequestId, Retention, RetentionOptIn,
    SampleSpan, Secret, Selected, SelectionContext, SelectionDocument, SelectionError,
    SpeechBounds, SpeechPolicy, SpeechPrivacy, SynthesisChunk, SynthesisDriver, SynthesisInput,
    SynthesisOutput, SynthesisSession, Utterance, UtteranceId, Voice, VoiceToken,
};
use sipx_media::{
    AudioDirection, Codec, Config, MediaPort, MediaSession, Pcm, PcmEncoding, PcmFormat, PcmSamples,
};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

const SAMPLES_PER_PACKET: usize = 160;

/// A bound on failure for audio crossing loopback, orders of magnitude above the honest answer on
/// an idle machine. No assertion here is measured in a duration: every claim is asserted on an
/// event, on a value, or on a resource that was released.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

/// The transcript no log record may ever contain. Thirty-three octets, which is the one thing about
/// it a log may report.
const CONFIDENTIAL_SPEECH: &str = "the-caller-said-something-private";

/// The synthesis input no log record may ever contain.
const CONFIDENTIAL_TEXT: &str = "read-this-account-number-aloud";

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

fn id(token: &str) -> ProviderId {
    ProviderId::new(token).expect("a lowercase identity token")
}

fn tag(token: &str) -> LanguageTag {
    LanguageTag::new(token).expect("an RFC 5646 tag")
}

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

fn voice_token() -> VoiceToken {
    VoiceToken::new("flat").expect("a lowercase voice token")
}

/// One µ-law packet whose payload byte is the call's marker, so decoded audio is attributable.
fn packet(marker: u8, sequence: u16) -> Bytes {
    Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * SAMPLES_PER_PACKET as u32,
        0x2c54_0001,
        Bytes::from(vec![marker; SAMPLES_PER_PACKET]),
    )
    .encode()
}

async fn feed(peer: &UdpSocket, to: SocketAddr, marker: u8, packets: u16) {
    for sequence in 0..packets {
        peer.send_to(&packet(marker, sequence), to)
            .await
            .expect("sends");
    }
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

/// A local, offline recogniser that keeps nothing: the `PRV` vectors' baseline provider.
fn local_recogniser(token: &str) -> ProviderDescriptor {
    ProviderDescriptor::recognition(id(token), "0")
        .language(tag("en"))
        .accepted_format(narrowband())
        .streaming(true)
        .build()
}

/// A local, offline synthesiser.
fn local_synthesiser(token: &str) -> ProviderDescriptor {
    ProviderDescriptor::synthesis(id(token), "0")
        .language(tag("en"))
        .emitted_format(narrowband())
        .voice(Voice::new(voice_token()).language(tag("en")))
        .build()
}

fn recognition_document(provider: &str) -> SelectionDocument {
    SelectionDocument::new()
        .with_provider(id(provider))
        .with_language(LanguageRange::wildcard())
        .with_conversion(Conversion::Deny)
}

fn synthesis_document(provider: &str) -> SelectionDocument {
    recognition_document(provider).with_voice(voice_token())
}

/// Select one recognition provider under `privacy`.
fn select_recognition(
    registry: &ProviderRegistry,
    provider: &str,
    privacy: SpeechPrivacy,
) -> Result<Selected, SelectionError> {
    let context = SelectionContext::new(registry, 8_000)
        .expect("the call clock is a PCM rate")
        .with_privacy(privacy);
    SpeechPolicy::new()
        .with_default(ProviderKind::Recognition, recognition_document(provider))
        .select(ProviderKind::Recognition, None, &context)
}

/// Select the one local synthesiser.
fn select_synthesis(registry: &ProviderRegistry, provider: &str) -> Selected {
    let context = SelectionContext::new(registry, 8_000)
        .expect("the call clock is a PCM rate")
        .with_privacy(SpeechPrivacy::LOCAL_NO_RETENTION);
    SpeechPolicy::new()
        .with_default(ProviderKind::Synthesis, synthesis_document(provider))
        .select(ProviderKind::Synthesis, None, &context)
        .expect("a local synthesiser needs no opt-in")
}

// ---------------------------------------------------------------------------------------------
// The providers. Neither recognises nor synthesises; both are attributable to one call.
// ---------------------------------------------------------------------------------------------

/// What one session did with its resources, so the test can inspect cleanup instead of waiting.
///
/// Shared between the test and *one* provider. Two providers never share one of these — that is
/// the fact under test, and a witness that spanned two sessions could not observe it.
#[derive(Debug, Default)]
struct Witness {
    released: AtomicBool,
}

impl Witness {
    fn released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }
}

/// A recognition session whose transcript names its own call's audio.
///
/// It "recognises" one thing: the marker byte the far end sent. A transcript carrying the other
/// call's marker would therefore be a visible value rather than a missing assertion.
#[derive(Debug)]
struct Attributed {
    label: &'static str,
    witness: Arc<Witness>,
    outputs: VecDeque<RecognitionOutput>,
    open: Option<(UtteranceId, u32, u64, u64)>,
    next: UtteranceId,
    consumed: u64,
    marker: Option<i16>,
    stopped: bool,
}

impl Attributed {
    fn new(label: &'static str, witness: &Arc<Witness>) -> Self {
        Self {
            label,
            witness: Arc::clone(witness),
            outputs: VecDeque::from([RecognitionOutput::Warming, RecognitionOutput::Ready]),
            open: None,
            next: UtteranceId::FIRST,
            consumed: 0,
            marker: None,
            stopped: false,
        }
    }

    /// The text of every revision: the call's label and the marker its own audio carried.
    fn text(&self) -> String {
        match self.marker {
            Some(marker) => format!("{}:{marker}:{CONFIDENTIAL_SPEECH}", self.label),
            None => format!("{}:silent:{CONFIDENTIAL_SPEECH}", self.label),
        }
    }

    fn revise(&mut self, samples: u64) {
        self.consumed = self.consumed.saturating_add(samples);
        let text = self.text();
        match self.open.as_mut() {
            None => {
                let open = (self.next, 1, self.consumed.saturating_sub(samples), samples);
                self.open = Some(open);
                self.outputs
                    .push_back(RecognitionOutput::Partial(Utterance::new(
                        open.0,
                        open.1,
                        text,
                        SampleSpan::new(open.2, open.3),
                    )));
            }
            Some(open) => {
                open.1 = open.1.saturating_add(1);
                open.3 = open.3.saturating_add(samples);
                let open = *open;
                self.outputs
                    .push_back(RecognitionOutput::Replacement(Utterance::new(
                        open.0,
                        open.1,
                        text,
                        SampleSpan::new(open.2, open.3),
                    )));
            }
        }
    }

    fn stop(&mut self, reason: CancelReason) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        if let Some((utterance, ..)) = self.open.take() {
            self.outputs
                .push_back(RecognitionOutput::Cancelled { utterance, reason });
            self.next = utterance.next();
        }
        self.outputs
            .push_back(RecognitionOutput::Stopped { aborted: false });
    }
}

impl Drop for Attributed {
    fn drop(&mut self) {
        self.witness.released.store(true, Ordering::SeqCst);
    }
}

impl RecognitionSession for Attributed {
    fn deliver(&mut self, input: RecognitionInput) {
        if self.stopped {
            return;
        }
        match input {
            RecognitionInput::Frame(frame) => {
                if self.marker.is_none()
                    && let PcmSamples::Signed16(samples) = frame.pcm().samples()
                    && let Some(first) = samples.first()
                {
                    self.marker = Some(*first);
                }
                let samples = frame.pcm().samples().len() as u64;
                self.revise(samples);
            }
            RecognitionInput::Discontinuity { samples, .. } => {
                self.consumed = self.consumed.saturating_add(samples);
            }
            RecognitionInput::Flush => self.stop(CancelReason::Application),
            RecognitionInput::Cancel(reason) => self.stop(reason),
            _ => {}
        }
    }

    fn poll_output(&mut self) -> Option<RecognitionOutput> {
        self.outputs.pop_front()
    }
}

/// A synthesis session that produces silent chunks and can be scripted to fail mid-request.
#[derive(Debug)]
struct Voicebox {
    witness: Arc<Witness>,
    outputs: VecDeque<SynthesisOutput>,
    chunks: u64,
    fail_after: Option<u64>,
    produced: u64,
    request: Option<RequestId>,
    stopped: bool,
}

impl Voicebox {
    fn new(chunks: u64, witness: &Arc<Witness>) -> Self {
        Self {
            witness: Arc::clone(witness),
            outputs: VecDeque::from([SynthesisOutput::Warming, SynthesisOutput::Ready]),
            chunks,
            fail_after: None,
            produced: 0,
            request: None,
            stopped: false,
        }
    }

    /// Fail the session after `chunks` chunks, with audio still unconsumed behind it.
    fn failing_after(mut self, chunks: u64) -> Self {
        self.fail_after = Some(chunks);
        self
    }

    fn produce(&mut self) {
        let Some(request) = self.request else {
            return;
        };
        while self.produced < self.chunks {
            let offset = self.produced * SAMPLES_PER_PACKET as u64;
            self.outputs
                .push_back(SynthesisOutput::Chunk(SynthesisChunk::new(
                    request,
                    self.produced,
                    offset,
                    Pcm::from_i16(narrowband(), vec![0; SAMPLES_PER_PACKET]),
                )));
            self.produced = self.produced.saturating_add(1);
            if self.fail_after == Some(self.produced) {
                self.request = None;
                self.stopped = true;
                self.outputs.push_back(SynthesisOutput::Failed {
                    request: Some(request),
                    cause: FailureCause::EngineFailed,
                });
                self.outputs.push_back(SynthesisOutput::Failed {
                    request: None,
                    cause: FailureCause::EngineFailed,
                });
                self.outputs
                    .push_back(SynthesisOutput::Stopped { aborted: false });
                return;
            }
        }
        self.request = None;
        self.outputs.push_back(SynthesisOutput::Completed {
            request,
            samples: self.chunks * SAMPLES_PER_PACKET as u64,
        });
    }
}

impl Drop for Voicebox {
    fn drop(&mut self) {
        self.witness.released.store(true, Ordering::SeqCst);
    }
}

impl SynthesisSession for Voicebox {
    fn deliver(&mut self, input: SynthesisInput) {
        if self.stopped {
            return;
        }
        match input {
            SynthesisInput::Enqueue { request, .. } => {
                self.outputs.push_back(SynthesisOutput::Accepted {
                    request,
                    position: 0,
                });
                self.outputs.push_back(SynthesisOutput::Started { request });
                self.request = Some(request);
                self.produce();
            }
            SynthesisInput::Cancel { reason, .. } => {
                self.stopped = true;
                if let Some(request) = self.request.take() {
                    self.outputs
                        .push_back(SynthesisOutput::Cancelled { request, reason });
                }
                self.outputs
                    .push_back(SynthesisOutput::Stopped { aborted: false });
            }
            _ => {}
        }
    }

    fn poll_output(&mut self) -> Option<SynthesisOutput> {
        self.outputs.pop_front()
    }
}

/// Read a driver's outputs until it closes its stream.
async fn drain(driver: &mut RecognitionDriver) -> Vec<RecognitionOutput> {
    let mut seen = Vec::new();
    // A bound on failure, never a definition of silence: the loop leaves on the closed output
    // stream and on nothing else, so no assertion below depends on how long a gap was.
    while let Some(output) = tokio::time::timeout(ARRIVAL_BOUND, driver.recv())
        .await
        .expect("the driver reaches its terminal output")
    {
        seen.push(output);
    }
    seen
}

/// The same, for a synthesis driver.
async fn drain_synthesis(driver: &mut SynthesisDriver) -> Vec<SynthesisOutput> {
    let mut seen = Vec::new();
    // A bound on failure, on the same terms as `drain` above.
    while let Some(output) = tokio::time::timeout(ARRIVAL_BOUND, driver.recv())
        .await
        .expect("the driver reaches its terminal output")
    {
        seen.push(output);
    }
    seen
}

/// Every transcript an ordered output stream carried.
fn transcripts(outputs: &[RecognitionOutput]) -> Vec<&str> {
    outputs
        .iter()
        .filter_map(|output| match output {
            RecognitionOutput::Partial(utterance)
            | RecognitionOutput::Replacement(utterance)
            | RecognitionOutput::Final(utterance) => Some(utterance.text()),
            _ => None,
        })
        .collect()
}

/// The first revision of a stream's first utterance.
///
/// `Partial` or `Replacement`: §5 coalesces revisions in the slot the `Partial` opened, so a
/// consumer that read nothing sees the newest revision *in the first revision's place*. Its span
/// still starts where the utterance opened, which is the fact the assertions below are about.
fn first_revision(outputs: &[RecognitionOutput]) -> &Utterance {
    outputs
        .iter()
        .find_map(|output| match output {
            RecognitionOutput::Partial(utterance) | RecognitionOutput::Replacement(utterance) => {
                Some(utterance)
            }
            _ => None,
        })
        .expect("an utterance opened")
}

// ---------------------------------------------------------------------------------------------
// PRV-1: the classification, and the default that follows from it.
// ---------------------------------------------------------------------------------------------

/// PRV-1: every class §11 names is classified, and no user data is retained past the operation.
///
/// The default is a value, not a convention: `SpeechPrivacy::LOCAL_NO_RETENTION` engages nothing,
/// and it is what `Default` gives, so a host that configures nothing has opted in to nothing.
#[test]
fn prv_1_user_data_is_not_retained_beyond_the_live_operation_by_default() {
    assert_eq!(DataClass::ALL.len(), 6);
    for class in DataClass::ALL {
        if class.user_data() {
            assert_eq!(
                class.default_retention(),
                Retention::LiveOperation,
                "{class} is user data, so its default retention is the live operation"
            );
            assert!(
                class.opt_in().is_some(),
                "{class} can only outlive the operation under a named host opt-in"
            );
        } else {
            assert_eq!(
                class.default_retention(),
                Retention::ProcessLifetime,
                "{class} is not user data: it belongs to the provider, not to the call"
            );
        }
    }

    let default = SpeechPrivacy::default();
    assert_eq!(default, SpeechPrivacy::LOCAL_NO_RETENTION);
    for opt_in in RetentionOptIn::ALL {
        assert!(
            !default.allows(opt_in),
            "the default engages no opt-in, and {opt_in} is one"
        );
    }
    assert!(
        default.admits(ProviderPrivacy::LOCAL_OFFLINE),
        "a local, offline provider that keeps nothing needs no opt-in at all"
    );
}

// ---------------------------------------------------------------------------------------------
// PRV-2: each opt-in is explicit, discoverable and named on the call event.
// ---------------------------------------------------------------------------------------------

/// One local provider and one for each thing a host has to decide about separately.
fn declaring_registry() -> ProviderRegistry {
    let declaring = |token: &str, declare: fn(ProviderDescriptorBuilder<ForRecognition>) -> _| {
        declare(
            ProviderDescriptor::recognition(id(token), "0")
                .language(tag("en"))
                .accepted_format(narrowband()),
        )
    };
    let mut registry = ProviderRegistry::new();
    registry.register(local_recogniser("local")).expect("fresh");
    for descriptor in [
        declaring("capturing", |builder| builder.debug_capture(true)),
        declaring("caching", |builder| builder.derived_cache(true)),
        declaring("elsewhere", |builder| builder.off_host(true)),
    ] {
        registry.register(descriptor.build()).expect("fresh");
    }
    registry
}

/// PRV-2: debug capture, persistent derived data and an off-host provider each need their own host
/// configuration, are visible in discovery, and are named on the session's admission event.
#[test]
fn prv_2_every_opt_in_is_explicit_visible_in_discovery_and_on_the_call_event() {
    let registry = declaring_registry();

    // Discovery reports each property, and the local provider declares none of them.
    let declared = |token: &str| -> ProviderPrivacy {
        registry
            .resolve(&id(token), ProviderKind::Recognition)
            .expect("registered")
            .privacy()
    };
    assert_eq!(declared("local"), ProviderPrivacy::LOCAL_OFFLINE);
    assert!(declared("capturing").debug_capture());
    assert!(declared("caching").derived_cache());
    assert!(declared("elsewhere").off_host());
    assert_eq!(
        declared("capturing").required_opt_ins().collect::<Vec<_>>(),
        vec![RetentionOptIn::DebugCapture]
    );
    assert_eq!(
        declared("local").required_opt_ins().count(),
        0,
        "a provider that keeps nothing and goes nowhere asks the host for nothing"
    );

    // With nothing configured, only the local provider is admitted.
    let refused = |token: &str| -> RefusalReason {
        match select_recognition(&registry, token, SpeechPrivacy::LOCAL_NO_RETENTION) {
            Err(SelectionError::Refused(refusals)) => refusals[0].reason().clone(),
            other => panic!("{token} was not refused: {other:?}"),
        }
    };
    assert_eq!(
        refused("capturing"),
        RefusalReason::RetentionRefused {
            debug_capture: true,
            derived_cache: false,
        }
    );
    assert_eq!(
        refused("caching"),
        RefusalReason::RetentionRefused {
            debug_capture: false,
            derived_cache: true,
        }
    );
    assert_eq!(
        refused("elsewhere"),
        RefusalReason::LocalityRefused {
            off_host: true,
            network: false,
        }
    );

    // One opt-in admits exactly one of them, and the admission event names what it engaged.
    let capture_only = SpeechPrivacy::LOCAL_NO_RETENTION.allowing(RetentionOptIn::DebugCapture);
    assert!(
        select_recognition(&registry, "caching", capture_only).is_err(),
        "one opt-in is not three"
    );
    let admitted = select_recognition(&registry, "capturing", capture_only)
        .expect("the explicit opt-in admits it");
    let admission = admitted.admission();
    assert_eq!(admission.provider(), &id("capturing"));
    assert_eq!(admission.kind(), ProviderKind::Recognition);
    assert!(admission.declared().debug_capture());
    assert_eq!(
        admission.engaged().collect::<Vec<_>>(),
        vec![RetentionOptIn::DebugCapture],
        "the call event names the opt-in this session runs under, and no other"
    );

    // A local provider engages nothing however wide the host's configuration is.
    let everything = RetentionOptIn::ALL
        .into_iter()
        .fold(SpeechPrivacy::LOCAL_NO_RETENTION, SpeechPrivacy::allowing);
    let local = select_recognition(&registry, "local", everything).expect("always admitted");
    assert_eq!(local.admission().declared(), ProviderPrivacy::LOCAL_OFFLINE);
    assert_eq!(
        local.admission().engaged().count(),
        0,
        "a permitted opt-in nothing uses is not an opt-in in force"
    );
    assert_eq!(local.admission().policy(), everything);
}

// ---------------------------------------------------------------------------------------------
// PRV-3: two calls share no queue, no budget, no data and no provider state.
// ---------------------------------------------------------------------------------------------

/// PRV-3: one call cannot receive another call's data or events, and neither can spend the other's
/// budget.
///
/// Both calls run the same registered provider, which is the case §2 singles out: "two sessions
/// share no mutable state, including two sessions of the same provider on different calls". They
/// are given different input-frame bounds and different audio, and the assertion is on both — the
/// transcripts are attributable to one call's marker, and the sample spans show each call spending
/// only its own queue.
///
/// The arithmetic is exact rather than approximate, and nothing waits for it. Each driver's
/// unconsumed-output bound of two is reached at `Warming`/`Ready`, so neither consumes a frame
/// until its own consumer reads; twelve packets then go by with the first call keeping the last two
/// and the second keeping the last eight.
#[tokio::test]
async fn prv_3_two_calls_share_no_queue_budget_data_or_provider_state() {
    let mut registry = ProviderRegistry::new();
    registry.register(local_recogniser("local")).expect("fresh");
    let selected = select_recognition(&registry, "local", SpeechPrivacy::LOCAL_NO_RETENTION)
        .expect("a local provider needs no opt-in");

    let (one, one_peer, one_addr) = session_and_peer().await;
    let (two, two_peer, two_addr) = session_and_peer().await;
    let one_witness = Arc::new(Witness::default());
    let two_witness = Arc::new(Witness::default());

    let narrow = SpeechBounds::DEFAULTS
        .with_input_frames(2)
        .expect("two frames is a bound")
        .with_unconsumed_outputs(2)
        .expect("two outputs is a bound");
    let wide = SpeechBounds::DEFAULTS
        .with_input_frames(8)
        .expect("eight frames is a bound")
        .with_unconsumed_outputs(2)
        .expect("two outputs is a bound");

    let mut first = RecognitionDriver::attach(
        &one,
        &selected,
        AudioDirection::Inbound,
        narrow,
        Attributed::new("one", &one_witness),
    )
    .expect("attaches");
    let mut second = RecognitionDriver::attach(
        &two,
        &selected,
        AudioDirection::Inbound,
        wide,
        Attributed::new("two", &two_witness),
    )
    .expect("attaches");

    // Different audio down each call. The call side is consumed first, which is the happens-before
    // for "the seam was offered twelve frames" on each attachment.
    feed(&one_peer, one_addr, 0x10, 12).await;
    feed(&two_peer, two_addr, 0x70, 12).await;
    for _ in 0..12 {
        tokio::time::timeout(ARRIVAL_BOUND, one.recv())
            .await
            .expect("the first call carries its audio")
            .expect("a frame");
        tokio::time::timeout(ARRIVAL_BOUND, two.recv())
            .await
            .expect("the second call carries its audio")
            .expect("a frame");
    }
    one.shutdown().await;
    two.shutdown().await;

    let first_outputs = drain(&mut first).await;
    let second_outputs = drain(&mut second).await;

    // No transcript crosses. Each call's marker appears only in its own stream.
    let first_texts = transcripts(&first_outputs);
    let second_texts = transcripts(&second_outputs);
    assert!(!first_texts.is_empty() && !second_texts.is_empty());
    let first_marker = first_texts[0].to_owned();
    let second_marker = second_texts[0].to_owned();
    assert_ne!(
        first_marker, second_marker,
        "the two calls carried distinguishable audio"
    );
    for text in &first_texts {
        assert!(
            text.starts_with("one:"),
            "the first call received {text:?}, which is not its own"
        );
        assert_eq!(*text, first_marker, "and its own audio never changed");
    }
    for text in &second_texts {
        assert!(
            text.starts_with("two:"),
            "the second call received {text:?}, which is not its own"
        );
        assert_eq!(*text, second_marker);
    }

    // Neither call spent the other's queue: the narrow one dropped ten frames, the wide one four.
    let samples = SAMPLES_PER_PACKET as u64;
    assert_eq!(
        first_revision(&first_outputs).span().start(),
        10 * samples,
        "a two-frame queue kept the last two of twelve"
    );
    assert_eq!(
        first_revision(&second_outputs).span().start(),
        4 * samples,
        "an eight-frame queue kept the last eight, of the same twelve"
    );

    // Provider state does not carry between calls: both number their first utterance from zero.
    for outputs in [&first_outputs, &second_outputs] {
        assert_eq!(first_revision(outputs).id(), UtteranceId::FIRST);
    }

    // And both released everything they held.
    tokio::time::timeout(ARRIVAL_BOUND, first.join())
        .await
        .expect("joined");
    tokio::time::timeout(ARRIVAL_BOUND, second.join())
        .await
        .expect("joined");
    assert!(one_witness.released() && two_witness.released());
}

/// PRV-3, second half: cancelling one call is not cancelling the other.
///
/// The two sessions run the same provider on two calls. One is cancelled and drained to its
/// `Stopped`; the other then carries audio and produces its own results, which is the only way to
/// say "the cancellation was scoped to one call" without waiting for the other to fail.
#[tokio::test]
async fn prv_3_cancelling_one_call_leaves_the_other_running() {
    let mut registry = ProviderRegistry::new();
    registry.register(local_recogniser("local")).expect("fresh");
    let selected = select_recognition(&registry, "local", SpeechPrivacy::LOCAL_NO_RETENTION)
        .expect("admitted");

    let (one, _one_peer, _one_addr) = session_and_peer().await;
    let (two, two_peer, two_addr) = session_and_peer().await;
    let one_witness = Arc::new(Witness::default());
    let two_witness = Arc::new(Witness::default());

    let mut cancelled = RecognitionDriver::attach(
        &one,
        &selected,
        AudioDirection::Inbound,
        SpeechBounds::DEFAULTS,
        Attributed::new("one", &one_witness),
    )
    .expect("attaches");
    let mut running = RecognitionDriver::attach(
        &two,
        &selected,
        AudioDirection::Inbound,
        SpeechBounds::DEFAULTS,
        Attributed::new("two", &two_witness),
    )
    .expect("attaches");

    cancelled.cancel(CancelReason::Application);
    let stopped = drain(&mut cancelled).await;
    assert_eq!(
        stopped.last(),
        Some(&RecognitionOutput::Stopped { aborted: false })
    );
    tokio::time::timeout(ARRIVAL_BOUND, cancelled.join())
        .await
        .expect("joined");
    assert!(one_witness.released());
    assert!(
        !two_witness.released(),
        "the other call's provider was untouched by a cancellation it never received"
    );

    feed(&two_peer, two_addr, 0x70, 1).await;
    tokio::time::timeout(ARRIVAL_BOUND, two.recv())
        .await
        .expect("the second call still carries audio")
        .expect("a frame");
    two.shutdown().await;

    let outputs = drain(&mut running).await;
    let texts = transcripts(&outputs);
    assert!(
        !texts.is_empty() && texts.iter().all(|text| text.starts_with("two:")),
        "the surviving call produced its own results: {outputs:?}"
    );
    tokio::time::timeout(ARRIVAL_BOUND, running.join())
        .await
        .expect("joined");
}

// ---------------------------------------------------------------------------------------------
// PRV-4: what an ordinary log says, and what it never says.
// ---------------------------------------------------------------------------------------------

/// Everything the driver logged, in one buffer.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<String>>);

impl Captured {
    fn text(&self) -> String {
        self.0.lock().expect("not poisoned").clone()
    }

    fn clear(&self) {
        self.0.lock().expect("not poisoned").clear();
    }
}

/// One record's fields, rendered the way a subscriber would render them.
struct Line(String);

impl tracing::field::Visit for Line {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl tracing::Subscriber for Captured {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = Line(String::new());
        event.record(&mut line);
        let mut held = self.0.lock().expect("not poisoned");
        held.push_str(&line.0);
        held.push('\n');
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// One recognition session on one call, from admission to a joined task.
async fn one_recognition_session() -> Vec<RecognitionOutput> {
    let mut registry = ProviderRegistry::new();
    registry.register(local_recogniser("local")).expect("fresh");
    let selected = select_recognition(&registry, "local", SpeechPrivacy::LOCAL_NO_RETENTION)
        .expect("admitted");
    let (session, peer, addr) = session_and_peer().await;
    let bounds = SpeechBounds::DEFAULTS
        .with_input_frames(4)
        .expect("four frames is a bound");
    let witness = Arc::new(Witness::default());
    let mut driver = RecognitionDriver::attach(
        &session,
        &selected,
        AudioDirection::Inbound,
        bounds,
        Attributed::new("one", &witness),
    )
    .expect("attaches");

    feed(&peer, addr, 0x10, 1).await;
    tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("the call carries the frame")
        .expect("a frame");
    session.shutdown().await;

    let outputs = drain(&mut driver).await;
    assert!(witness.released(), "the provider is released at the stop");
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("joined");
    outputs
}

/// PRV-4: an ordinary log record names the provider, the lifecycle, the limits and the typed cause
/// — and never the audio, the transcript, the synthesis input, a model path or a credential.
///
/// Not a claim about one `Debug` implementation: the driver is run under a subscriber that captures
/// every field of every record it emits, on a single-threaded runtime so the driver task's records
/// land in the same buffer. What the buffer holds is what an operator would see.
#[test]
fn prv_4_an_ordinary_log_reports_the_provider_and_the_cause_and_never_the_content() {
    let captured = Captured::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");

    tracing::subscriber::with_default(captured.clone(), || {
        // `tracing` decides once per record site whether anyone is listening, the first time that
        // site is reached — which in a test binary can be on another test's thread, before this
        // subscriber exists. One warm-up session reaches every site the driver has, and rebuilding
        // the cache then re-answers the question with this subscriber in place. Without it this
        // assertion would pass or fail depending on which test in the binary ran first.
        runtime.block_on(one_recognition_session());
        tracing::callsite::rebuild_interest_cache();
        captured.clear();

        let outputs = runtime.block_on(one_recognition_session());
        assert!(
            transcripts(&outputs)
                .iter()
                .any(|text| text.contains(CONFIDENTIAL_SPEECH)),
            "the transcript reached the consumer, which is where it belongs"
        );
    });

    let log = captured.text();
    assert!(
        !log.contains(CONFIDENTIAL_SPEECH),
        "a transcript reached an ordinary log record: {log}"
    );
    assert!(
        log.contains("local"),
        "the provider's typed identity is what a log is for: {log}"
    );
    assert!(log.contains("recognition"), "and the contract kind: {log}");
    assert!(
        log.contains("input_frames=4"),
        "the limits the session runs under are reportable: {log}"
    );
    assert!(log.contains("stopped"), "and its lifecycle: {log}");
}

/// PRV-4, second half: the redaction is in the types, so no caller has to remember it.
///
/// Every value that carries user data renders its class and its size and never its content, which
/// is what makes `?value` in a log record safe by construction rather than by review.
#[test]
fn prv_4_every_user_data_type_redacts_itself() {
    let utterance = Utterance::new(
        UtteranceId::FIRST,
        3,
        CONFIDENTIAL_SPEECH,
        SampleSpan::new(160, 320),
    );
    let rendered = format!("{utterance:?}");
    assert!(!rendered.contains(CONFIDENTIAL_SPEECH), "{rendered}");
    assert!(rendered.contains("transcript"), "{rendered}");
    assert!(
        rendered.contains("33 octets"),
        "the size survives, so a log can still show that something was said: {rendered}"
    );
    assert!(
        rendered.contains("revision: 3"),
        "and everything that is not the content: {rendered}"
    );

    let audio = Pcm::from_i16(narrowband(), vec![7; SAMPLES_PER_PACKET]);
    let frame = RecognitionFrame::new(AudioDirection::Inbound, audio.clone(), 0, 1);
    let rendered = format!("{frame:?}");
    assert!(
        !rendered.contains('7'),
        "a sample value survived: {rendered}"
    );
    assert!(rendered.contains("call audio"), "{rendered}");
    assert!(rendered.contains("160 samples"), "{rendered}");

    let chunk = SynthesisChunk::new(RequestId::new(0), 0, 0, audio);
    let rendered = format!("{chunk:?}");
    assert!(!rendered.contains('7'), "{rendered}");
    assert!(rendered.contains("call audio"), "{rendered}");

    let enqueue = SynthesisInput::Enqueue {
        request: RequestId::new(0),
        text: CONFIDENTIAL_TEXT.to_owned(),
        replace: false,
    };
    let rendered = format!("{enqueue:?}");
    assert!(!rendered.contains(CONFIDENTIAL_TEXT), "{rendered}");
    assert!(rendered.contains("synthesis input"), "{rendered}");

    // Credentials and model paths are the provider's own, and the contract gives them a carrier
    // whose only rendering is a redaction.
    let credential = Secret::new("s3cr3t-token".to_owned());
    assert_eq!(format!("{credential:?}"), "[redacted]");
    assert_eq!(format!("{credential}"), "[redacted]");
    assert_eq!(credential.expose(), "s3cr3t-token");
    let model = Secret::new("/var/lib/models/private/weights.bin".to_owned());
    assert!(!format!("{model:?}").contains("weights"));
}

// ---------------------------------------------------------------------------------------------
// PRV-5: cancellation and failure release what the session held.
// ---------------------------------------------------------------------------------------------

/// PRV-5: a cancelled session releases its provider and its seam attachment, and the release is
/// inspected rather than waited for.
///
/// The seam bounds one call at eight attachments (`docs/specs/call-audio-seam.md` §5). Nine
/// sessions are therefore started and cancelled on one call: had any of them leaked its attachment,
/// the ninth would be refused. The witnesses say the same thing about the engine and device
/// allocation a real provider would hold behind the same handle. Nothing here sleeps.
#[tokio::test]
async fn prv_5_cancellation_releases_the_provider_and_the_seam_attachment() {
    let mut registry = ProviderRegistry::new();
    registry.register(local_recogniser("local")).expect("fresh");
    let selected = select_recognition(&registry, "local", SpeechPrivacy::LOCAL_NO_RETENTION)
        .expect("admitted");
    let (session, _peer, _addr) = session_and_peer().await;

    for round in 0..9u32 {
        let witness = Arc::new(Witness::default());
        let mut driver = RecognitionDriver::attach(
            &session,
            &selected,
            AudioDirection::Inbound,
            SpeechBounds::DEFAULTS,
            Attributed::new("one", &witness),
        )
        .unwrap_or_else(|error| {
            panic!("round {round} could not attach, so an earlier round leaked one: {error}")
        });
        driver.cancel(CancelReason::Application);
        let outputs = drain(&mut driver).await;
        assert_eq!(
            outputs.last(),
            Some(&RecognitionOutput::Stopped { aborted: false })
        );
        assert!(
            witness.released(),
            "the provider is released when the driver's output stream closes, not later"
        );
        tokio::time::timeout(ARRIVAL_BOUND, driver.join())
            .await
            .expect("joined");
    }
    session.shutdown().await;
}

/// PRV-5, second half: a provider failure erases the audio the driver had not handed on.
///
/// §6 returns chunk-window credit when the driver *hands a chunk on*, so an unconsumed chunk is
/// audio that will never be played. When the session stops it is erased rather than left in a queue
/// for a consumer that has already gone: `Stopped` means nothing owned remains, and synthesized
/// speech is user data with no retention past the operation that produced it. Terminal and
/// lifecycle outputs stay, because they are the outcome and §6 never drops one.
#[tokio::test]
async fn prv_5_a_stopped_session_keeps_no_audio_it_never_handed_on() {
    let mut registry = ProviderRegistry::new();
    registry
        .register(local_synthesiser("voice"))
        .expect("fresh");
    let selected = select_synthesis(&registry, "voice");

    let witness = Arc::new(Witness::default());
    let provider = Voicebox::new(6, &witness).failing_after(4);
    let mut driver = SynthesisDriver::spawn(&selected, SpeechBounds::DEFAULTS, provider)
        .expect("a synthesis selection starts a synthesis driver");
    assert!(driver.enqueue(RequestId::new(0), CONFIDENTIAL_TEXT.to_owned(), false));

    let outputs = drain_synthesis(&mut driver).await;
    assert!(
        !outputs
            .iter()
            .any(|output| matches!(output, SynthesisOutput::Chunk(_))),
        "audio nobody consumed outlived the session: {outputs:?}"
    );
    assert_eq!(driver.retained_audio(), 0);
    assert!(
        outputs.contains(&SynthesisOutput::Failed {
            request: None,
            cause: FailureCause::EngineFailed,
        }),
        "the typed cause is the outcome, and it is never erased: {outputs:?}"
    );
    assert_eq!(
        outputs.last(),
        Some(&SynthesisOutput::Stopped { aborted: false })
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("joined");
    assert!(witness.released());
}

/// PRV-5, third half: call teardown erases exactly what a failure would.
///
/// The distinction matters because §7 makes call teardown a cancellation and never a failure: an
/// erasure rule only a failure triggered would leave the ordinary end of every call as the one path
/// that keeps audio.
#[tokio::test]
async fn prv_5_call_teardown_erases_the_same_audio_a_failure_would() {
    let mut registry = ProviderRegistry::new();
    registry
        .register(local_synthesiser("voice"))
        .expect("fresh");
    let selected = select_synthesis(&registry, "voice");

    let witness = Arc::new(Witness::default());
    let mut driver = SynthesisDriver::spawn(
        &selected,
        SpeechBounds::DEFAULTS,
        Voicebox::new(6, &witness),
    )
    .expect("starts");
    assert!(driver.enqueue(RequestId::new(0), CONFIDENTIAL_TEXT.to_owned(), false));
    driver.cancel(CancelScope::Session, CancelReason::CallEnded);

    let outputs = drain_synthesis(&mut driver).await;
    assert!(
        !outputs
            .iter()
            .any(|output| matches!(output, SynthesisOutput::Chunk(_))),
        "call teardown left audio behind: {outputs:?}"
    );
    assert_eq!(driver.retained_audio(), 0);
    assert!(
        outputs.contains(&SynthesisOutput::Completed {
            request: RequestId::new(0),
            samples: 6 * SAMPLES_PER_PACKET as u64,
        }),
        "the request's own terminal is the outcome, and it survives: {outputs:?}"
    );
    assert_eq!(
        outputs.last(),
        Some(&SynthesisOutput::Stopped { aborted: false })
    );
    tokio::time::timeout(ARRIVAL_BOUND, driver.join())
        .await
        .expect("joined");
    assert!(witness.released());
}
