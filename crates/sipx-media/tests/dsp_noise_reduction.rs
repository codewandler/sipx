//! Noise reduction seen from the call layer (`docs/specs/call-dsp-noise-reduction.md`, `M-66`).
//!
//! `crates/sipx-audio/tests/dsp_noise_reduction.rs` proves the arithmetic and the conformance run.
//! What is left to prove is the sentence the story turns on — that **a different reducer can be
//! substituted without the call layer knowing** — and that is a claim about an attachment, so it is
//! tested against a live `MediaSession` on loopback rather than against a graph in isolation.
//!
//! Two facts, and they are separate. One driver attaches either reducer and reads back what the
//! call carried, and the only thing that differs between the two runs is which plan it was handed.
//! And provenance is the *door* rather than the type: the workspace baseline reaches
//! `ProvenInline` through the registry, and the very same processor offered at the public plan door
//! is refused exactly as an application's reducer is.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_media::dsp::{
    ActivityInput, BuiltIn, DspCapability, DspFrame, DspResetCause, ExecutionPolicy,
    ExecutionProfile, FormatError, FrameAdmission, FrameProcessor, FrameSink, GraphBounds,
    GraphError, GraphPlan, GraphTransition, NoiseReducer, NoiseReduction, Parameter,
    ParameterError, ProcessError, Scratch, StreamFormat, SubbandSuppressor,
};
use sipx_media::{
    AudioDirection, Codec, Config, MediaPort, MediaSession, PcmEncoding, PcmFormat, PcmSamples,
    Processing,
};
use tokio::net::UdpSocket;

/// How long these tests wait for audio to cross loopback before calling it lost.
///
/// A bound on failure, orders of magnitude above the honest answer on an idle machine, and never a
/// window anything is measured in.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

const SAMPLES_PER_PACKET: usize = 160;

/// Enough packets to leave the baseline's declared 1,024-position warm-up behind.
const PACKETS: usize = 12;

// -------------------------------------------------------------------------- harness ----

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

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

fn signed(samples: &PcmSamples) -> &[i16] {
    match samples {
        PcmSamples::Signed16(samples) => samples,
        other => panic!("expected signed samples, got {other:?}"),
    }
}

/// A packet of alternating full-scale-fraction samples: at 8,000 Hz that is the folding frequency,
/// where the baseline's whole signal lands in its high band and its gain is legible in the output.
fn alternating(amplitude: i16) -> Vec<i16> {
    (0..SAMPLES_PER_PACKET)
        .map(|n| if n % 2 == 0 { amplitude } else { -amplitude })
        .collect()
}

/// Attach `plan` to a fresh session, push `packets` frames of `signal`, and read back what the call
/// actually carried.
///
/// **This function is the test.** It names [`GraphPlan`] and nothing else: it cannot tell which
/// reducer it is driving, has no branch on one, and reads every size and expectation off the
/// attachment. Handing it two plans that differ only in which reducer they carry is what makes
/// "substitutable without the call layer knowing" a fact rather than a shape.
async fn carried(plan: GraphPlan, signal: &[i16], packets: usize) -> (Vec<i16>, GraphTransition) {
    let (session, _peer, _addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(plan)
        .expect("a validated graph activates");
    let activation = graph.transitions().first().copied().expect("an activation");

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");

    let mut heard = Vec::new();
    for _ in 0..packets {
        assert!(session.send(signal.to_vec()).await, "queues outbound audio");
    }
    for _ in 0..packets {
        let frame = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("transmitted audio reaches the seam")
            .expect("a frame");
        heard.extend_from_slice(signed(frame.pcm().samples()));
    }

    session.shutdown().await;
    (heard, activation)
}

// ------------------------------------------------------------------- an application's ----

/// An application-supplied noise reducer: a fixed-magnitude subtractor with no state at all.
///
/// It shares nothing with the baseline but the contract, and it declares
/// [`ExecutionProfile::TrustedCooperativeNative`] because an application-supplied processor may not
/// call itself proven. Its whole arithmetic is `y = sign(x)·max(0, |x| − 4_000)`.
#[derive(Debug, Default)]
struct FixedFloorSubtractor {
    admission: FrameAdmission,
}

impl FixedFloorSubtractor {
    const FLOOR: i32 = 4_000;

    fn declaration() -> DspCapability {
        DspCapability::new("fixture.fixed_floor").with_execution(ExecutionPolicy::new(
            ExecutionProfile::TrustedCooperativeNative,
        ))
    }
}

impl FrameProcessor for FixedFloorSubtractor {
    fn capability(&self) -> DspCapability {
        Self::declaration()
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
            let magnitude = (i32::from(*sample).abs() - Self::FLOOR).max(0);
            let signed = if *sample < 0 { -magnitude } else { magnitude };
            sink.push(i16::try_from(signed).unwrap_or(i16::MAX))?;
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

impl NoiseReducer for FixedFloorSubtractor {
    fn noise_reduction(&self) -> NoiseReduction {
        NoiseReduction::new(Self::declaration())
    }
}

// ---------------------------------------------------------------------------- tests ----

/// Acceptance row 2: one driver, two reducers, and the call layer cannot tell which it ran.
#[tokio::test]
async fn either_reducer_is_substituted_without_the_call_layer_knowing() {
    let signal = alternating(16_000);

    let workspace = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(BuiltIn::SubbandSuppressor, &[])
        .expect("the registry builds it");
    let (workspace_audio, workspace_activation) = carried(workspace, &signal, PACKETS).await;

    let application = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_processor(Box::new(FixedFloorSubtractor::default()));
    let (application_audio, application_activation) = carried(application, &signal, PACKETS).await;

    // Same driver, same shape of attachment: one stage, generation 1, at position 0.
    assert_eq!(workspace_activation, application_activation);
    assert_eq!(
        workspace_activation,
        GraphTransition::Activated {
            generation: 1,
            at_position: 0,
            processors: 1,
        }
    );

    // Both are genuinely in the path, and they are genuinely different implementations: the
    // fixture subtracts a constant magnitude from every sample from the first one, and the
    // baseline is the exact identity through its declared warm-up and attenuates after it.
    assert_eq!(workspace_audio.len(), application_audio.len());
    assert_eq!(
        &workspace_audio[..1_024],
        &signal.repeat(PACKETS)[..1_024],
        "the baseline's declared warm-up is the exact identity"
    );
    assert!(
        application_audio[..160]
            .iter()
            .all(|sample| sample.abs() == 16_000 - FixedFloorSubtractor::FLOOR as i16),
        "the fixture subtracts from the first sample and has no warm-up"
    );
    assert_ne!(
        workspace_audio, application_audio,
        "two reducers, two results — substituted, not aliased"
    );
    let tail = workspace_audio.len() - 160;
    assert!(
        workspace_audio[tail..]
            .iter()
            .all(|sample| sample.abs() < 16_000),
        "the baseline is attenuating once its warm-up is behind it"
    );
}

/// Acceptance row 2: provenance is the door and not the type.
#[tokio::test]
async fn the_registry_grants_the_profile_and_the_type_does_not() {
    let (session, _peer, _addr) = session_and_peer().await;

    // The same processor, constructed by hand and offered at the door an application reaches.
    let refused = session.attach_dsp(
        GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
            .with_processor(Box::new(SubbandSuppressor::new())),
    );
    assert!(
        matches!(refused, Err(GraphError::ProfileNotAdmissible { .. })),
        "the baseline built by hand is refused like anybody's: {refused:?}"
    );

    // Through the registry, it activates.
    let admitted = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_built_in(BuiltIn::SubbandSuppressor, &[])
                .expect("the registry builds it"),
        )
        .expect("the registry grants the profile");
    assert_eq!(admitted.direction(), AudioDirection::Outbound);

    session.shutdown().await;
}

/// Acceptance row 3: adaptive state is per call, and it is per call because it is per instance.
///
/// A second call driven hard through the same reducer kind changes nothing about the first, which
/// is what `docs/specs/call-dsp-graph.md` §8's ownership already implies and what a reducer with
/// adaptive state is the first processor in this workspace to actually depend on.
#[tokio::test]
async fn one_calls_estimate_never_reaches_another() {
    let quiet = alternating(2_000);
    let loud = alternating(30_000);

    let lone = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(BuiltIn::SubbandSuppressor, &[])
        .expect("the registry builds it");
    let (reference, _) = carried(lone, &quiet, PACKETS).await;

    // A second call, driven hard through the same reducer kind, first.
    let busy = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(BuiltIn::SubbandSuppressor, &[])
        .expect("the registry builds it");
    let (busy_audio, _) = carried(busy, &loud, PACKETS).await;
    assert_ne!(
        busy_audio.len(),
        0,
        "the loud call really did run its own reducer"
    );

    let beside = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(BuiltIn::SubbandSuppressor, &[])
        .expect("the registry builds it");
    let (beside_audio, _) = carried(beside, &quiet, PACKETS).await;

    assert_eq!(
        beside_audio, reference,
        "a call's output is what its own estimator makes of its own audio, and nothing else"
    );
}

/// Acceptance row 6: the declaration a call layer reads distinguishes this from its neighbours.
#[test]
fn the_declaration_says_what_it_is_and_what_it_consumes() {
    let declaration = SubbandSuppressor::new().noise_reduction();
    declaration.validate().expect("the shipped declaration");

    assert_eq!(declaration.warm_up_positions(), 1_024);
    assert_eq!(declaration.processor().latency_positions(), 0);
    assert_eq!(declaration.processor().tail_positions(), 0);
    assert_eq!(
        declaration.activity_input(),
        ActivityInput::Optional {
            parameter: "voice_active"
        },
        "it consumes an activity hint as a declared parameter and never as a call into a detector"
    );

    // A reducer is not a detector: the media crate's own voice-activity vocabulary is the analysis
    // contract's, and nothing in this declaration names it.
    assert_eq!(declaration.admits_host(&[]), Ok(()));
}
