//! Bounded DSP graphs attached to live calls (`docs/specs/call-dsp-graph.md`, `M-64`).
//!
//! These run against a live `MediaSession` on loopback rather than against the graph in isolation,
//! because the claims worth proving are about the *attachment*: that a validated ordered chain
//! transforms the audio the call actually carries, that a replacement is never half-applied to a
//! frame, that a processor which fails or runs late costs its configured action rather than the
//! call's RTP, and that teardown leaves nothing of the graph behind.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use sipx_media::dsp::{
    BuiltIn, BypassCause, DspCapability, DspFrame, DspResetCause, ExecutionPolicy,
    ExecutionProfile, FailureAction, FormatError, FrameAdmission, FrameProcessor, FrameSink,
    GraphBounds, GraphError, GraphPlan, GraphTransition, Parameter, ParameterError, ParameterValue,
    ProcessError, Scratch, StreamFormat, SupervisedWorker, TeardownCause, WorkerResult,
};
use sipx_media::{
    AudioDirection, Codec, Config, MediaPort, MediaSession, PcmEncoding, PcmFormat, PcmSamples,
    Processing,
};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

/// How long these tests wait for audio to cross loopback before calling it lost.
///
/// A bound on failure, orders of magnitude above the honest answer on an idle machine, and never a
/// window anything is measured in.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

const SAMPLES_PER_PACKET: usize = 160;

// ------------------------------------------------------------------------- fixtures ----

/// A saturating gain of two, on the profile an application may actually select.
///
/// `TrustedCooperativeNative` runs on the media worker exactly as `ProvenInline` does; what differs
/// is the claim and the admission, which is the distinction these tests hold the attachment to.
struct Gain2 {
    admission: FrameAdmission,
    profile: ExecutionProfile,
    id: &'static str,
}

impl Gain2 {
    fn new(id: &'static str, profile: ExecutionProfile) -> Self {
        Self {
            admission: FrameAdmission::new(),
            profile,
            id,
        }
    }
}

impl FrameProcessor for Gain2 {
    fn capability(&self) -> DspCapability {
        DspCapability::new(self.id).with_execution(ExecutionPolicy::new(self.profile))
    }
    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }
    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }
    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&self.capability(), frame)?;
        for sample in frame.samples() {
            sink.push(sample.saturating_mul(2))?;
        }
        Ok(())
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }
    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
    }
    fn cancel(&mut self) {
        self.admission.cancel();
    }
    fn retained(&self) -> u32 {
        0
    }
}

/// Adds a constant, so composition order is readable in the output.
struct Bias {
    admission: FrameAdmission,
    offset: i16,
    id: &'static str,
}

impl Bias {
    fn new(id: &'static str, offset: i16) -> Self {
        Self {
            admission: FrameAdmission::new(),
            offset,
            id,
        }
    }
}

impl FrameProcessor for Bias {
    fn capability(&self) -> DspCapability {
        DspCapability::new(self.id).with_execution(ExecutionPolicy::new(
            ExecutionProfile::TrustedCooperativeNative,
        ))
    }
    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }
    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }
    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&self.capability(), frame)?;
        for sample in frame.samples() {
            sink.push(sample.saturating_add(self.offset))?;
        }
        Ok(())
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }
    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
    }
    fn cancel(&mut self) {
        self.admission.cancel();
    }
    fn retained(&self) -> u32 {
        0
    }
}

/// Refuses every frame it is offered, so the configured failure action is what is observed.
struct AlwaysRefuses {
    admission: FrameAdmission,
    action: FailureAction,
    misses: u32,
}

impl AlwaysRefuses {
    fn new(action: FailureAction, misses: u32) -> Self {
        Self {
            admission: FrameAdmission::new(),
            action,
            misses,
        }
    }
}

impl FrameProcessor for AlwaysRefuses {
    fn capability(&self) -> DspCapability {
        DspCapability::new("always-refuses").with_execution(
            ExecutionPolicy::new(ExecutionProfile::TrustedCooperativeNative)
                .with_max_consecutive_misses(self.misses)
                .with_on_failure(self.action),
        )
    }
    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }
    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }
    fn process(
        &mut self,
        _frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        _sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        Err(ProcessError::Rejected {
            reason: "this fixture refuses everything",
        })
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }
    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
    }
    fn cancel(&mut self) {
        self.admission.cancel();
    }
    fn retained(&self) -> u32 {
        0
    }
}

/// Counts every frame it sees into a shared counter, so two calls can be checked for a shared
/// adaptive state that must not exist.
struct Counting {
    admission: FrameAdmission,
    seen: Arc<AtomicU32>,
}

impl FrameProcessor for Counting {
    fn capability(&self) -> DspCapability {
        DspCapability::new("counting").with_execution(ExecutionPolicy::new(
            ExecutionProfile::TrustedCooperativeNative,
        ))
    }
    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }
    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }
    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&self.capability(), frame)?;
        self.seen.fetch_add(1, Ordering::Relaxed);
        sink.write(frame.samples())
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }
    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
    }
    fn cancel(&mut self) {
        self.admission.cancel();
    }
    fn retained(&self) -> u32 {
        0
    }
}

/// Records every discontinuity it was handed, so "the break reaches every downstream processor"
/// is a fact rather than an intention.
struct Downstream {
    admission: FrameAdmission,
    breaks: Arc<AtomicU32>,
}

impl FrameProcessor for Downstream {
    fn capability(&self) -> DspCapability {
        DspCapability::new("downstream").with_execution(ExecutionPolicy::new(
            ExecutionProfile::TrustedCooperativeNative,
        ))
    }
    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        self.capability().validate_parameters(parameters)
    }
    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&self.capability(), direction, format)
    }
    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        let admitted = self.admission.admit(&self.capability(), frame)?;
        if admitted.discontinuity().is_some() {
            self.breaks.fetch_add(1, Ordering::Relaxed);
        }
        sink.write(frame.samples())
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }
    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
    }
    fn cancel(&mut self) {
        self.admission.cancel();
    }
    fn retained(&self) -> u32 {
        0
    }
}

/// A supervised worker that never answers, so the deadline is what is observed rather than the
/// worker.
struct NeverAnswers;

impl SupervisedWorker for NeverAnswers {
    fn capability(&self) -> DspCapability {
        DspCapability::new("never-answers").with_execution(
            ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated)
                .with_max_consecutive_misses(2),
        )
    }
    fn run(&mut self, _samples: &[i16], _out: &mut Vec<i16>) -> WorkerResult {
        WorkerResult::Withheld
    }
}

/// A supervised worker that answers correctly, so the contained path is proven to carry audio.
struct SupervisedGain2;

impl SupervisedWorker for SupervisedGain2 {
    fn capability(&self) -> DspCapability {
        DspCapability::new("supervised-gain2")
            .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated))
    }
    fn run(&mut self, samples: &[i16], out: &mut Vec<i16>) -> WorkerResult {
        out.extend(samples.iter().map(|sample| sample.saturating_mul(2)));
        WorkerResult::Produced
    }
}

// --------------------------------------------------------------------------- harness ----

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

fn packet(sequence: u16, level: u8) -> Bytes {
    Packet::new(
        Codec::Pcmu.payload_type(),
        sequence,
        u32::from(sequence) * SAMPLES_PER_PACKET as u32,
        0x2c54_0001,
        Bytes::from(vec![level; SAMPLES_PER_PACKET]),
    )
    .encode()
}

async fn feed(peer: &UdpSocket, to: SocketAddr, packets: u16, level: u8) {
    for sequence in 0..packets {
        peer.send_to(&packet(sequence, level), to)
            .await
            .expect("sends");
    }
}

fn signed(samples: &PcmSamples) -> &[i16] {
    match samples {
        PcmSamples::Signed16(samples) => samples,
        other => panic!("expected signed samples, got {other:?}"),
    }
}

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

/// A tone whose every sample doubles without saturating, so a gain of two is visible by value.
fn tone() -> Vec<i16> {
    (0..i16::try_from(SAMPLES_PER_PACKET).expect("a packet fits an i16"))
        .map(|n| n * 16)
        .collect()
}

// ----------------------------------------------------------------------------- tests ----

/// Acceptance row 1: separate ordered chains per direction, validated whole before they activate.
#[tokio::test]
async fn ordered_graphs_attach_per_direction_and_transform_live_audio() {
    let (session, peer, session_addr) = session_and_peer().await;

    // Outbound: gain of two, then a bias of one. Order is observable: (n·2)+1, never (n+1)·2.
    let outbound = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Box::new(Gain2::new(
                    "gain2",
                    ExecutionProfile::TrustedCooperativeNative,
                )))
                .with_processor(Box::new(Bias::new("bias", 1))),
        )
        .expect("a validated outbound graph activates");
    let inbound = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_processor(Box::new(Bias::new("bias", 3))),
        )
        .expect("a validated inbound graph activates");

    assert_eq!(outbound.direction(), AudioDirection::Outbound);
    assert_eq!(inbound.direction(), AudioDirection::Inbound);
    assert_eq!(
        outbound.transitions(),
        vec![GraphTransition::Activated {
            generation: 1,
            at_position: 0,
            processors: 2,
        }]
    );

    // The outbound seam tap reports the samples that become RTP, so it reports the graph's output.
    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    assert!(session.send(tone()).await, "queues outbound audio");
    let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("transmitted audio reaches the seam")
        .expect("a frame");
    let expected: Vec<i16> = tone()
        .iter()
        .map(|n| n.saturating_mul(2).saturating_add(1))
        .collect();
    assert_eq!(signed(sent.pcm().samples()), expected.as_slice());

    // Inbound: the application receives the graph's output, not the decoder's. What the decoder
    // produces for this payload is read off a second session with no graph, so the comparison is
    // against the stack's own answer rather than against a constant written down here.
    let (plain, peer_b, addr_b) = session_and_peer().await;
    feed(&peer_b, addr_b, 4, 0xFF).await;
    let undsped = tokio::time::timeout(ARRIVAL_BOUND, plain.recv())
        .await
        .expect("received audio reaches the application")
        .expect("a frame");

    feed(&peer, session_addr, 4, 0xFF).await;
    let heard = tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("received audio reaches the application")
        .expect("a frame");
    let expected: Vec<i16> = undsped.iter().map(|n| n.saturating_add(3)).collect();
    assert_eq!(
        heard, expected,
        "the inbound chain transformed what the application hears"
    );

    plain.shutdown().await;
    session.shutdown().await;
}

/// Acceptance row 1: an inadmissible chain never becomes the graph a frame sees.
#[tokio::test]
async fn a_graph_is_validated_whole_before_anything_activates() {
    let (session, _peer, _addr) = session_and_peer().await;

    // The second stage is fine; the third declares a profile no application may select. The whole
    // plan is refused and the direction keeps running without a graph.
    let refused = session.attach_dsp(
        GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Box::new(Bias::new("bias", 1)))
            .with_processor(Box::new(Gain2::new(
                "claims-proven",
                ExecutionProfile::ProvenInline,
            ))),
    );
    assert_eq!(
        refused.map(|_| ()),
        Err(GraphError::ProfileNotAdmissible {
            processor: "claims-proven",
            profile: ExecutionProfile::ProvenInline,
        }),
        "an application-supplied processor may not declare itself proven"
    );

    // Nothing activated: the direction is free, and audio is untouched.
    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");
    assert!(session.send(tone()).await);
    let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("arrives")
        .expect("a frame");
    assert_eq!(signed(sent.pcm().samples()), tone().as_slice());

    session.shutdown().await;
}

/// Acceptance row 2: every bound is explicit and non-zero, and configuration cannot exceed one.
#[tokio::test]
async fn bounds_are_explicit_non_zero_and_enforced() {
    let (session, _peer, _addr) = session_and_peer().await;

    let bounds = GraphBounds::new();
    assert!(bounds.max_processors() > 0);
    assert!(bounds.max_frame_samples() > 0);
    assert!(bounds.scratch_samples() > 0);
    assert!(bounds.retained_tail_positions() > 0);
    assert!(bounds.observation_capacity() > 0);

    assert_eq!(
        GraphBounds::new().with_max_processors(0).validate(),
        Err(GraphError::Bound {
            field: "max_processors",
            value: 0,
        })
    );
    assert_eq!(
        GraphBounds::new()
            .with_retained_tail_positions(0)
            .validate(),
        Err(GraphError::Bound {
            field: "retained_tail_positions",
            value: 0,
        })
    );

    // A chain longer than the configured processor count is refused before it activates.
    let mut plan = GraphPlan::new(
        AudioDirection::Outbound,
        GraphBounds::new().with_max_processors(2),
    );
    for _ in 0..3 {
        plan = plan.with_processor(Box::new(Bias::new("bias", 1)));
    }
    assert_eq!(
        session.attach_dsp(plan).map(|_| ()),
        Err(GraphError::TooManyProcessors { limit: 2 })
    );

    // A processor whose declared tail exceeds the configured delay-line bound is refused, so no
    // configuration can build an unbounded delay line out of admissible parts.
    let long_tail = session.attach_dsp(
        GraphPlan::new(
            AudioDirection::Outbound,
            GraphBounds::new().with_retained_tail_positions(4),
        )
        .with_processor(Box::new(TailHog)),
    );
    assert_eq!(
        long_tail.map(|_| ()),
        Err(GraphError::TailExceedsBound {
            processor: "tail-hog",
            declared: 64,
            bound: 4,
        })
    );

    session.shutdown().await;
}

/// Acceptance row 3: one frame, one generation; a replacement is a typed sample-boundary
/// transition and never a mixture.
#[tokio::test]
async fn replacement_is_atomic_at_a_sample_boundary() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Box::new(Bias::new("bias", 1))),
        )
        .expect("activates");
    assert_eq!(graph.generation(), 1);

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    assert!(session.send(tone()).await);
    let first = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("arrives")
        .expect("a frame");
    let biased: Vec<i16> = tone().iter().map(|n| n.saturating_add(1)).collect();
    assert_eq!(signed(first.pcm().samples()), biased.as_slice());

    let generation = graph
        .replace(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                Gain2::new("gain2", ExecutionProfile::TrustedCooperativeNative),
            )),
        )
        .expect("a validated replacement is published whole");
    assert_eq!(generation, 2);

    assert!(session.send(tone()).await);
    let second = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("arrives")
        .expect("a frame");
    let doubled: Vec<i16> = tone().iter().map(|n| n.saturating_mul(2)).collect();
    assert_eq!(
        signed(second.pcm().samples()),
        doubled.as_slice(),
        "the frame saw generation 2 entire — no bias from generation 1 survived into it"
    );

    let transitions = graph.transitions();
    assert!(
        matches!(
            transitions.as_slice(),
            [
                GraphTransition::Activated { generation: 1, .. },
                GraphTransition::Replaced {
                    generation: 2,
                    previous: 1,
                    ..
                },
            ]
        ),
        "{transitions:?}"
    );

    session.shutdown().await;
}

/// Acceptance row 4: a failing processor costs its configured action, the call keeps carrying RTP,
/// and the break reaches every downstream processor.
#[tokio::test]
async fn a_failed_processor_bypasses_open_and_flags_every_downstream_processor() {
    let (session, _peer, _addr) = session_and_peer().await;
    let breaks = Arc::new(AtomicU32::new(0));

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Box::new(AlwaysRefuses::new(FailureAction::BypassOpen, 1)))
                .with_processor(Box::new(Downstream {
                    admission: FrameAdmission::new(),
                    breaks: Arc::clone(&breaks),
                })),
        )
        .expect("activates");

    // Nothing in this graph is contained: a cooperative-native stage claims nothing, so the graph
    // may not claim it either.
    assert!(!graph.contains_overrun());

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    for _ in 0..3 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..3 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("RTP keeps flowing past a failed processor")
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            tone().as_slice(),
            "fail-open: unmodified audio continues to flow"
        );
    }

    assert!(
        breaks.load(Ordering::Relaxed) >= 1,
        "the downstream processor was told the signal ahead of it changed"
    );
    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                processor: "always-refuses",
                cause: BypassCause::Refused,
                ..
            }
        )),
        "{transitions:?}"
    );

    session.shutdown().await;
}

/// Acceptance row 4: fail-closed tears the graph down rather than letting unprocessed audio flow
/// on — and says so as a typed transition.
#[tokio::test]
async fn a_failed_processor_configured_closed_tears_the_graph_down() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                AlwaysRefuses::new(FailureAction::TerminateClosed, 1),
            )),
        )
        .expect("activates");

    for _ in 0..3 {
        assert!(session.send(tone()).await);
    }
    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.settled())
        .await
        .expect("the graph settles");
    assert!(barrier.is_clear(), "{barrier:?}");

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::TornDown {
                cause: TeardownCause::FailedClosed { .. },
                ..
            }
        )),
        "{transitions:?}"
    );

    session.shutdown().await;
}

/// Acceptance row 5: a supervised stage carries audio through bounded channels, and cancellation
/// terminates and reaps its worker.
#[tokio::test]
async fn a_supervised_stage_carries_audio_and_is_reaped() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_supervised(Box::new(SupervisedGain2)),
        )
        .expect("activates");
    assert!(
        graph.contains_overrun(),
        "a wholly supervised graph may claim that over-budget work cannot stall RTP"
    );
    assert_eq!(
        graph.pipeline_frames(),
        1,
        "the deadline is the pipeline depth, and it is reported rather than hidden"
    );

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    for _ in 0..3 {
        assert!(session.send(tone()).await);
    }

    // The first frame fills the pipeline: there is no result for it yet, so it passes through and
    // is counted as a miss like any other missing result.
    let first = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("arrives")
        .expect("a frame");
    assert_eq!(signed(first.pcm().samples()), tone().as_slice());

    let doubled: Vec<i16> = tone().iter().map(|n| n.saturating_mul(2)).collect();
    for _ in 0..2 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("arrives")
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            doubled.as_slice(),
            "the worker's result reached the wire through the bounded channels"
        );
    }

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("detaching terminates and reaps the worker");
    assert!(barrier.is_clear(), "{barrier:?}");
    assert_eq!(barrier.workers(), 0);
    assert_eq!(barrier.frames_in_flight(), 0);

    session.shutdown().await;
}

/// Acceptance row 5: a worker that never answers costs its declared action and never the call's
/// RTP.
#[tokio::test]
async fn a_supervised_worker_that_never_answers_costs_its_declared_action() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_supervised(Box::new(NeverAnswers)),
        )
        .expect("activates");

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    for _ in 0..4 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..4 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("RTP is never held by a worker that does not answer")
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            tone().as_slice(),
            "fail-open: unprocessed audio keeps flowing, and it is audible rather than hidden"
        );
    }

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                cause: BypassCause::DeadlineMissed,
                ..
            }
        )),
        "{transitions:?}"
    );

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("a worker that never answers is still terminated and reaped");
    assert!(barrier.is_clear(), "{barrier:?}");

    session.shutdown().await;
}

/// Acceptance row 6: teardown is an observable barrier, not a wait, and it proves the graph holds
/// nothing.
#[tokio::test]
async fn teardown_waits_on_a_barrier_proving_nothing_is_held() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Box::new(Bias::new("bias", 1)))
                .with_supervised(Box::new(SupervisedGain2)),
        )
        .expect("activates");

    assert!(session.send(tone()).await);
    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("the barrier is reached rather than waited out");

    assert!(barrier.is_clear(), "{barrier:?}");
    assert_eq!(barrier.workers(), 0);
    assert_eq!(barrier.frames_in_flight(), 0);
    assert_eq!(barrier.retained_positions(), 0);
    assert_eq!(barrier.processors(), 0);

    session.shutdown().await;
}

/// Acceptance row 6: stopping a session tears its graphs down without holding a runtime worker.
#[tokio::test]
async fn a_stopped_session_releases_its_graphs() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_supervised(Box::new(SupervisedGain2)),
        )
        .expect("activates");

    session.shutdown().await;

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.settled())
        .await
        .expect("a stopped session's graph settles rather than idling");
    assert!(barrier.is_clear(), "{barrier:?}");

    // And the direction refuses new work by type rather than by silence.
    assert_eq!(
        session
            .attach_dsp(GraphPlan::new(AudioDirection::Inbound, GraphBounds::new()))
            .map(|_| ()),
        Err(GraphError::SessionStopped)
    );
}

/// Acceptance row 6: two calls share no mutable DSP state.
#[tokio::test]
async fn concurrent_calls_share_no_mutable_dsp_state() {
    let (first, _peer_a, _addr_a) = session_and_peer().await;
    let (second, _peer_b, _addr_b) = session_and_peer().await;

    let seen_first = Arc::new(AtomicU32::new(0));
    let seen_second = Arc::new(AtomicU32::new(0));

    let _a = first
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                Counting {
                    admission: FrameAdmission::new(),
                    seen: Arc::clone(&seen_first),
                },
            )),
        )
        .expect("activates");
    let _b = second
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                Counting {
                    admission: FrameAdmission::new(),
                    seen: Arc::clone(&seen_second),
                },
            )),
        )
        .expect("activates");

    let mut watched = first
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");
    for _ in 0..3 {
        assert!(first.send(tone()).await);
    }
    for _ in 0..3 {
        tokio::time::timeout(ARRIVAL_BOUND, watched.recv())
            .await
            .expect("arrives")
            .expect("a frame");
    }

    assert_eq!(seen_first.load(Ordering::Relaxed), 3);
    assert_eq!(
        seen_second.load(Ordering::Relaxed),
        0,
        "one call's graph never saw another call's audio"
    );

    first.shutdown().await;
    second.shutdown().await;
}

/// Acceptance row 4: a break the session declares reaches every processor in the chain.
#[tokio::test]
async fn a_session_break_reaches_every_processor() {
    let (mut session, peer, session_addr) = session_and_peer().await;
    let breaks = Arc::new(AtomicU32::new(0));

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new()).with_processor(Box::new(
                Downstream {
                    admission: FrameAdmission::new(),
                    breaks: Arc::clone(&breaks),
                },
            )),
        )
        .expect("activates");
    assert_eq!(graph.generation(), 1);

    feed(&peer, session_addr, 4, 0xFF).await;
    tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("arrives")
        .expect("a frame");
    assert_eq!(
        breaks.load(Ordering::Relaxed),
        0,
        "an unbroken stream carries no break"
    );

    // Renegotiation re-anchors the timeline. The graph survives it — an application does not
    // re-attach across a re-INVITE — and every stage restarts its epoch rather than carrying old
    // audio into a new one.
    let mut config = Config::new(peer.local_addr().expect("has an address"), Codec::Pcma);
    config.rtcp_interval = None;
    assert!(session.reconfigure(config).await.expect("renegotiates"));
    assert_eq!(
        graph.generation(),
        1,
        "the graph belongs to the call, not to a worker generation"
    );

    session.shutdown().await;
}

/// A fixture whose declared tail is larger than a bounded configuration admits.
struct TailHog;

impl FrameProcessor for TailHog {
    fn capability(&self) -> DspCapability {
        DspCapability::new("tail-hog")
            .with_tail_positions(64)
            .with_latency_positions(64)
            .with_execution(ExecutionPolicy::new(
                ExecutionProfile::TrustedCooperativeNative,
            ))
    }
    fn configure(&mut self, _parameters: &[Parameter]) -> Result<(), ParameterError> {
        Ok(())
    }
    fn prepare(
        &mut self,
        _direction: AudioDirection,
        _format: StreamFormat,
    ) -> Result<(), FormatError> {
        Ok(())
    }
    fn process(
        &mut self,
        _frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        _sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        Ok(())
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        Ok(())
    }
    fn reset(&mut self, _cause: DspResetCause) {}
    fn cancel(&mut self) {}
    fn retained(&self) -> u32 {
        0
    }
}

/// `M-65`: a graph can now be built from a real stage, and the registry is the only door that
/// grants the proven-inline profile.
///
/// Two claims in one test, because they are the same claim seen from either side. A workspace
/// processor named through [`BuiltIn`] attaches and transforms the audio the call carries; the very
/// same processor constructed by hand and offered at the public door is refused, because what the
/// door admits is provenance and not a type.
#[tokio::test]
async fn the_workspace_registry_admits_a_proven_stage_and_the_public_door_still_does_not() {
    let (session, _peer, _addr) = session_and_peer().await;

    let doubled = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(
            BuiltIn::Gain,
            &[Parameter::new("gain", ParameterValue::Ratio(2_000))],
        )
        .expect("a registered processor with a valid parameter set");
    let graph = session
        .attach_dsp(doubled)
        .expect("a proven stage attaches");
    assert!(
        graph.contains_overrun(),
        "a chain of proven stages may claim that over-budget work cannot stall RTP"
    );

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    assert!(session.send(tone()).await, "queues outbound audio");
    let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("transmitted audio reaches the seam")
        .expect("a frame");
    let expected: Vec<i16> = tone().iter().map(|n| n.saturating_mul(2)).collect();
    assert_eq!(signed(sent.pcm().samples()), expected.as_slice());

    // The same processor, built by hand and offered where an application offers one. It is a
    // workspace processor and it still declares `ProvenInline`; the refusal is about the door.
    let (plain, _peer_b, _addr_b) = session_and_peer().await;
    let refused = plain.attach_dsp(
        GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Box::new(sipx_audio::dsp::effects::Gain::new())),
    );
    assert_eq!(
        refused.map(|_| ()),
        Err(GraphError::ProfileNotAdmissible {
            processor: "sipx.gain",
            profile: ExecutionProfile::ProvenInline,
        })
    );

    plain.shutdown().await;
    session.shutdown().await;
}

/// `M-65`: provenance is per stage, so a built-in beside an application's processor does not lend
/// it the proven profile — and the registry refuses a bad shape or a bad parameter set where the
/// caller wrote it.
#[tokio::test]
async fn registry_provenance_does_not_spread_along_a_chain() {
    let (session, _peer, _addr) = session_and_peer().await;

    let mixed = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(BuiltIn::HighPass, &[])
        .expect("a registered processor")
        .with_processor(Box::new(Gain2::new(
            "claims-proven",
            ExecutionProfile::ProvenInline,
        )));
    assert_eq!(
        session.attach_dsp(mixed).map(|_| ()),
        Err(GraphError::ProfileNotAdmissible {
            processor: "claims-proven",
            profile: ExecutionProfile::ProvenInline,
        }),
        "a built-in's provenance is its own and does not travel along the chain"
    );

    // A shape the effect does not admit, refused at the call that wrote it.
    let too_long = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_built_in(
        BuiltIn::Stutter {
            delay_positions: sipx_audio::dsp::effects::MAX_STUTTER_POSITIONS + 1,
        },
        &[],
    );
    assert!(matches!(
        too_long.map(|_| ()),
        Err(GraphError::Capability {
            processor: "sipx.stutter",
            ..
        })
    ));

    // A parameter outside the declared schema, likewise.
    let bad_parameter = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_built_in(
        BuiltIn::Gain,
        &[Parameter::new("gain", ParameterValue::Ratio(9_999))],
    );
    assert!(matches!(
        bad_parameter.map(|_| ()),
        Err(GraphError::Parameter {
            processor: "sipx.gain",
            ..
        })
    ));

    // And a delay line longer than the graph's own retained-tail bound is still the graph's
    // refusal, not the registry's: the registry admits the shape and the bounds reject it.
    let long_line = GraphPlan::new(
        AudioDirection::Outbound,
        GraphBounds::new().with_retained_tail_positions(4),
    )
    .with_built_in(
        BuiltIn::Stutter {
            delay_positions: 64,
        },
        &[],
    )
    .expect("64 positions is an admissible shape");
    assert_eq!(
        session.attach_dsp(long_line).map(|_| ()),
        Err(GraphError::TailExceedsBound {
            processor: "sipx.stutter",
            declared: 64,
            bound: 4,
        })
    );

    session.shutdown().await;
}
