//! Hardening the DSP graph's real-time and failure isolation (`M-68`).
//!
//! `M-64` and `M-102` built the execution and failure policy of
//! [`docs/specs/call-dsp-graph.md`](../../../docs/specs/call-dsp-graph.md) and proved it on
//! well-behaved inputs. These tests are the adversarial half: hostile audio, hostile processors and
//! hostile lifecycles, driven against a live `MediaSession` and asked whether anything panics,
//! grows without bound, crosses between two calls or two directions, or keeps a call alive after it
//! ended.
//!
//! **What this file does not do is widen a claim.** Only proven-inline and supervised-isolated may
//! say that over-budget work cannot stall RTP; `TrustedCooperativeNative` says nothing about
//! containment, and a chain holding one such stage says nothing at all
//! ([`docs/specs/custom-call-dsp.md`](../../../docs/specs/custom-call-dsp.md) §7.3). Several tests
//! below run hostile code on that profile precisely because it is the uncontained one, and none of
//! them is evidence that it is contained.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    // An adversarial case is one call driven through one hostile sequence, and splitting it into
    // helpers would put the setup, the driving and the assertion in three places for a reader who
    // has to hold all three at once to know what was proved.
    clippy::too_many_lines
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use bytes::Bytes;
use sipx_media::dsp::{
    DeadlineAction, DspCapability, DspFrame, DspObservation, DspResetCause, ExecutionPolicy,
    ExecutionProfile, FailureAction, FormatError, FrameProcessor, FrameSink, GraphBounds,
    GraphPlan, GraphTransition, OverrunContainment, Parameter, ParameterError, ProcessError,
    Scratch, StreamFormat, TeardownCause, WorkerProcess,
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

/// The reference worker of `docs/specs/call-dsp-graph.md` §7.5.
const WORKER: &str = env!("CARGO_BIN_EXE_sipx-dsp-worker");

/// A sample value that appears nowhere else in this stack, so finding it in a diagnostic is proof
/// that the diagnostic carried call audio rather than a coincidence.
const SENTINEL: i16 = 30_011;

// ------------------------------------------------------------------------- fixtures ----

/// The identity, on the profile an application may actually select.
struct Ident {
    id: &'static str,
}

impl Ident {
    fn boxed(id: &'static str) -> Box<dyn FrameProcessor + Send> {
        Box::new(Self { id })
    }
}

impl FrameProcessor for Ident {
    fn capability(&self) -> DspCapability {
        DspCapability::new(self.id).with_execution(ExecutionPolicy::new(
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
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        sink.write(frame.samples())
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

/// What one stage saw, so "no call and no direction reached another" is a fact rather than a hope.
///
/// Shared by handle rather than by static, because two calls sharing a static is the very thing
/// this is here to detect.
#[derive(Debug, Default)]
struct Ledger {
    frames: AtomicU64,
    breaks: AtomicU64,
    foreign: AtomicU64,
    extremes: AtomicU64,
}

impl Ledger {
    fn frames(&self) -> u64 {
        self.frames.load(Ordering::SeqCst)
    }
    fn breaks(&self) -> u64 {
        self.breaks.load(Ordering::SeqCst)
    }
    fn foreign(&self) -> u64 {
        self.foreign.load(Ordering::SeqCst)
    }
    fn extremes(&self) -> u64 {
        self.extremes.load(Ordering::SeqCst)
    }
}

/// A pass-through that records what it was handed and refuses nothing.
///
/// `forbidden` is a value this stage must never see: another call's sentinel, or the other
/// direction's. Counting rather than asserting inside `process`, because a panic on the media
/// worker under `TrustedCooperativeNative` is precisely what the stack cannot contain — a fixture
/// that panicked there would be testing the harness's luck.
struct Witness {
    id: &'static str,
    ledger: Arc<Ledger>,
    forbidden: i16,
}

impl Witness {
    fn boxed(
        id: &'static str,
        ledger: &Arc<Ledger>,
        forbidden: i16,
    ) -> Box<dyn FrameProcessor + Send> {
        Box::new(Self {
            id,
            ledger: Arc::clone(ledger),
            forbidden,
        })
    }
}

impl FrameProcessor for Witness {
    fn capability(&self) -> DspCapability {
        DspCapability::new(self.id).with_execution(ExecutionPolicy::new(
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
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.ledger.frames.fetch_add(1, Ordering::SeqCst);
        if frame.discontinuity().is_some() {
            self.ledger.breaks.fetch_add(1, Ordering::SeqCst);
        }
        for sample in frame.samples() {
            if *sample == self.forbidden {
                self.ledger.foreign.fetch_add(1, Ordering::SeqCst);
            }
            if *sample == i16::MIN || *sample == i16::MAX {
                self.ledger.extremes.fetch_add(1, Ordering::SeqCst);
            }
        }
        sink.write(frame.samples())
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

/// A stage that refuses every frame carrying `i16::MIN`, and writes one position short of what it
/// consumed on every frame carrying `i16::MAX`.
///
/// Two of §7.2's four misses from one fixture, chosen by the audio itself, so a hostile stream is
/// what reaches the failure policy rather than a switch a test flipped.
struct Hostile {
    id: &'static str,
}

impl Hostile {
    fn boxed(id: &'static str) -> Box<dyn FrameProcessor + Send> {
        Box::new(Self { id })
    }
}

impl FrameProcessor for Hostile {
    fn capability(&self) -> DspCapability {
        DspCapability::new(self.id).with_execution(
            ExecutionPolicy::new(ExecutionProfile::TrustedCooperativeNative)
                .with_max_consecutive_misses(64),
        )
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
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        if frame.samples().contains(&i16::MIN) {
            return Err(ProcessError::Rejected {
                reason: "this fixture refuses full-scale negative audio",
            });
        }
        let short = frame
            .samples()
            .len()
            .saturating_sub(usize::from(frame.samples().contains(&i16::MAX)));
        sink.write(frame.samples().get(..short).unwrap_or_default())
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

/// A stage that says it is holding audio and ignores cancellation — the graph's own `ZombieCancel`.
///
/// `docs/specs/custom-call-dsp.md` §8.4 makes cancellation the processor's half of the teardown
/// barrier, and §11.3's `ZombieCancel` fixture is what proves the *conformance harness* catches a
/// processor that reneges on it. This asks the other question: whether a graph that attached one
/// anyway can still be torn down.
struct ZombieCancel {
    cancelled: Arc<AtomicU64>,
    after_cancel: Arc<AtomicU64>,
}

impl FrameProcessor for ZombieCancel {
    fn capability(&self) -> DspCapability {
        DspCapability::new("zombie-cancel").with_execution(ExecutionPolicy::new(
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
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        if self.cancelled.load(Ordering::SeqCst) > 0 {
            self.after_cancel.fetch_add(1, Ordering::SeqCst);
        }
        sink.write(frame.samples())
    }
    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        Ok(())
    }
    fn reset(&mut self, _cause: DspResetCause) {}
    /// Terminal and idempotent is what the contract asks for. This records the call and then
    /// carries on regardless, which is the whole point of the fixture.
    fn cancel(&mut self) {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
    }
    /// A lie: this processor holds nothing at all and never did.
    fn retained(&self) -> u32 {
        7
    }
}

/// A supervised worker that answers correctly, so a real process boundary is in the chain.
fn supervised_gain2() -> WorkerProcess {
    WorkerProcess::new(
        DspCapability::new("supervised-gain2")
            .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated)),
        WORKER,
    )
    .arg("--mode")
    .arg("gain")
    .arg("--gain")
    .arg("1")
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

/// A frame of one sentinel value, so any rendering of it is recognisable by eye and by assertion.
fn sentinel_tone() -> Vec<i16> {
    vec![SENTINEL; SAMPLES_PER_PACKET]
}

/// The hostile programs `M-68`'s second Acceptance row names, each one frame long.
///
/// Every one of them is exactly the packetisation, because a live call's frames are: the
/// wrong-length case is a *processor* producing one, not a producer sending one, and it has its own
/// test.
fn hostile_programs() -> Vec<(&'static str, Vec<i16>)> {
    let mut impulse = vec![0; SAMPLES_PER_PACKET];
    impulse[0] = i16::MAX;
    impulse[SAMPLES_PER_PACKET / 2] = i16::MIN;
    let alternating = (0..SAMPLES_PER_PACKET)
        .map(|n| if n % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();
    let ramp = (0..SAMPLES_PER_PACKET)
        .map(|n| i16::MIN.saturating_add(i16::try_from(n).unwrap_or(0).saturating_mul(400)))
        .collect();
    let mut discontinuity = vec![i16::MAX; SAMPLES_PER_PACKET];
    discontinuity[SAMPLES_PER_PACKET - 1] = i16::MIN;
    vec![
        ("impulse", impulse),
        ("alternating full scale", alternating),
        (
            "dc at full negative scale",
            vec![i16::MIN; SAMPLES_PER_PACKET],
        ),
        (
            "dc at full positive scale",
            vec![i16::MAX; SAMPLES_PER_PACKET],
        ),
        ("silence", vec![0; SAMPLES_PER_PACKET]),
        ("a step discontinuity", discontinuity),
        ("a ramp across the whole domain", ramp),
    ]
}

// ----------------------------------------------------------------------------- tests ----

/// Acceptance row 2, the diagnostics half: nothing a graph renders may carry call audio.
///
/// The rule is `M-107`'s, already held by the processor contract's own types — `DspFrame`,
/// `FrameSink` and `Scratch` each report shape rather than content. This asks the same question of
/// the *attachment*: a `DspGraph` is public and `Debug`, so a `tracing` field, a panic message or a
/// failing assertion that names one must not put the call's audio into a record whose length is the
/// frame's.
#[tokio::test]
async fn a_graph_diagnostic_carries_no_call_audio() {
    let (session, peer, session_addr) = session_and_peer().await;

    let outbound = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Ident::boxed("ident"))
                .with_supervised(supervised_gain2()),
        )
        .expect("a validated outbound graph activates");
    let inbound = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_processor(Ident::boxed("ident")),
        )
        .expect("a validated inbound graph activates");

    // Drive both directions, so every buffer the graph owns holds audio rather than zeroes.
    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    assert!(session.send(sentinel_tone()).await, "queues outbound audio");
    let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("transmitted audio reaches the seam")
        .expect("a frame");
    assert!(!signed(sent.pcm().samples()).is_empty());

    feed(&peer, session_addr, 4, 0xFF).await;
    tokio::time::timeout(ARRIVAL_BOUND, session.recv())
        .await
        .expect("received audio reaches the application")
        .expect("a frame");

    for (direction, rendering) in [
        ("outbound", format!("{outbound:?}")),
        ("inbound", format!("{inbound:?}")),
    ] {
        assert!(
            !rendering.contains(&SENTINEL.to_string()),
            "the {direction} graph's diagnostic rendering carries a call's sample value: {rendering}"
        );
        // A shape's length rather than a frame's. These frames carry 160 positions and the
        // contract's ceiling is 65,536, so a rendering that scaled with the audio would be orders
        // of magnitude past this rather than near it.
        assert!(
            rendering.len() < 2_048,
            "the {direction} graph's diagnostic rendering is {} octets long, so its length is a \
             frame's rather than a shape's: {rendering}",
            rendering.len()
        );
    }

    session.shutdown().await;
}

/// Acceptance row 2: hostile audio reaches every stage of a live call and cannot panic, cross a
/// direction or cross a call.
///
/// The audio is injected **outbound**, where a caller's own samples reach the graph before
/// encoding, because that is the only path on which a test can state the exact hostile value it
/// means. Two calls run at once with different sentinels and each one's stages are told the other's
/// value is forbidden, so isolation is checked by the audio itself rather than by counting frames.
#[tokio::test]
async fn hostile_audio_cannot_panic_or_cross_a_direction_or_a_call() {
    const OTHER: i16 = 701;

    let (alpha, alpha_peer, alpha_addr) = session_and_peer().await;
    let (beta, _beta_peer, _beta_addr) = session_and_peer().await;

    let alpha_out = Arc::new(Ledger::default());
    let alpha_in = Arc::new(Ledger::default());
    let beta_out = Arc::new(Ledger::default());

    // Alpha's stages refuse to see beta's sentinel; beta's refuses to see alpha's. The inbound
    // stage of alpha additionally refuses to see anything alpha sent outbound.
    let alpha_outbound = alpha
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Witness::boxed("alpha-out", &alpha_out, OTHER)),
        )
        .expect("alpha's outbound graph activates");
    let alpha_inbound = alpha
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_processor(Witness::boxed("alpha-in", &alpha_in, SENTINEL)),
        )
        .expect("alpha's inbound graph activates");
    let beta_outbound = beta
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Witness::boxed("beta-out", &beta_out, SENTINEL)),
        )
        .expect("beta's outbound graph activates");

    let mut transmitted = alpha
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");

    // Alpha carries every hostile program, and one sentinel frame so the cross-call check has a
    // value to find. Each frame is awaited at the seam, so the sweep is driven by arrivals rather
    // than by a duration.
    let mut programs = hostile_programs();
    programs.push(("the alpha sentinel", sentinel_tone()));
    for (name, program) in &programs {
        assert!(alpha.send(program.clone()).await, "queues {name}");
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .unwrap_or_else(|_| panic!("{name} reaches the outbound seam"))
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            program.as_slice(),
            "{name} passed through the witness unchanged"
        );
    }

    // Beta carries its own sentinel, several times, while alpha is running. Awaited at beta's own
    // seam, so "beta ran" is an arrival rather than a queue depth.
    let mut beta_transmitted = beta
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to beta's transmitted audio");
    for _ in 0..4 {
        assert!(beta.send(vec![OTHER; SAMPLES_PER_PACKET]).await);
        tokio::time::timeout(ARRIVAL_BOUND, beta_transmitted.recv())
            .await
            .expect("beta's audio reaches beta's seam")
            .expect("a frame");
    }

    // Alpha's inbound is fed real RTP with a gap, so a declared break reaches the chain too. The
    // gap is concealed as silence the graph never sees and a `Loss` the graph is told about, so the
    // break lands on the next frame that *is* audio — which is why this reads until the witness has
    // seen one rather than reading a number somebody counted.
    for sequence in [0_u16, 1, 2, 3, 12, 13, 14, 15] {
        alpha_peer
            .send_to(&packet(sequence, 0xFF), alpha_addr)
            .await
            .expect("sends");
    }
    while alpha_in.breaks() == 0 {
        tokio::time::timeout(ARRIVAL_BOUND, alpha.recv())
            .await
            .expect("the break reaches the inbound chain")
            .expect("a frame");
    }

    assert_eq!(
        alpha_out.frames(),
        programs.len() as u64,
        "alpha's outbound stage saw every frame it was sent"
    );
    assert!(
        alpha_out.extremes() > 0,
        "the hostile programs reached the stage as full-scale audio rather than as something \
         quantised on the way"
    );
    assert_eq!(alpha_out.foreign(), 0, "beta's audio reached alpha's chain");
    assert_eq!(
        beta_out.foreign(),
        0,
        "alpha's audio reached beta's chain — two calls share DSP state"
    );
    assert!(beta_out.frames() >= 4, "beta's own chain still ran");
    assert_eq!(
        alpha_in.foreign(),
        0,
        "outbound audio reached the inbound chain of the same call"
    );
    assert!(
        alpha_in.frames() > 0 && alpha_in.breaks() > 0,
        "the inbound chain saw frames and was told about the gap: {} frames, {} breaks",
        alpha_in.frames(),
        alpha_in.breaks()
    );

    // Nothing refused, nothing missed, nothing bypassed: hostile *audio* is not a failure.
    for (name, graph) in [
        ("alpha outbound", &alpha_outbound),
        ("alpha inbound", &alpha_inbound),
        ("beta outbound", &beta_outbound),
    ] {
        let counters = graph.counters();
        assert_eq!(counters.misses(), 0, "{name} counted a miss on valid audio");
        assert_eq!(counters.bypasses(), 0, "{name} bypassed a stage");
        assert_eq!(counters.teardowns(), 0, "{name} tore a graph down");
    }
    assert_ne!(
        alpha_outbound.counters().frames(),
        alpha_inbound.counters().frames(),
        "the two directions' counters are one shared tally"
    );

    alpha.shutdown().await;
    beta.shutdown().await;
}

/// Acceptance rows 2 and 3: a processor that refuses a frame and one that answers with the wrong
/// number of positions are two different findings, counted apart, and neither stalls the call.
#[tokio::test]
async fn a_refusal_and_an_invalid_output_length_are_counted_apart_and_cost_the_call_nothing() {
    let (session, _peer, _addr) = session_and_peer().await;

    let downstream = Arc::new(Ledger::default());
    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Hostile::boxed("hostile"))
                .with_processor(Witness::boxed("downstream", &downstream, 0)),
        )
        .expect("a validated outbound graph activates");

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");

    // Full-scale negative is refused; full-scale positive produces one position short; the plain
    // tone is neither. Every frame still reaches RTP.
    let program = [
        ("refused", vec![i16::MIN; SAMPLES_PER_PACKET], 1_u64, 0_u64),
        ("short", vec![i16::MAX; SAMPLES_PER_PACKET], 1, 1),
        ("clean", sentinel_tone(), 1, 1),
    ];
    for (name, samples, refusals, malformed) in program {
        assert!(session.send(samples.clone()).await, "queues {name}");
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .unwrap_or_else(|_| panic!("{name} still reaches RTP"))
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            samples.as_slice(),
            "{name}: a stage that contributed nothing passes its input on unchanged"
        );
        let counters = graph.counters();
        assert_eq!(counters.refusals(), refusals, "{name}: refusals");
        assert_eq!(
            counters.malformed_results(),
            malformed,
            "{name}: malformed results"
        );
        assert_eq!(counters.deadline_misses(), 0, "{name}: deadline misses");
        assert_eq!(counters.workers_lost(), 0, "{name}: workers lost");
    }

    // §6.2: the downstream stage was told each time, and the stage never reached its 64-miss
    // budget, so nothing was bypassed and nothing was torn down.
    assert_eq!(downstream.frames(), 3);
    assert_eq!(downstream.breaks(), 2, "the two misses reached downstream");
    let counters = graph.counters();
    assert_eq!(counters.misses(), 2);
    assert_eq!(counters.bypasses(), 0);
    assert_eq!(counters.frames(), 3);
    assert_eq!(counters.frames_passed_through(), 0);

    assert!(graph.detach().await.is_clear());
    session.shutdown().await;
}

/// Acceptance row 2, the format half: a renegotiation that changes the audio format or the
/// packetisation tears the graph down instead of handing its stages audio they never agreed to.
///
/// A graph belongs to the call, so an ordinary re-INVITE carries it over — that is `M-64`'s promise
/// and the first half of this test holds it. What it cannot survive is a change to the two things
/// it was validated against. Every stage was prepared for one `StreamFormat` and
/// `docs/specs/custom-call-dsp.md` §8.3 makes a rate change a `prepare` and never a frame, so a
/// carried-over chain would be told 8 kHz while carrying 16 kHz; every buffer was sized from the
/// old packetisation, and §4 forbids growing one, so a longer frame would pass through untouched
/// for the rest of the call with nothing saying so.
#[tokio::test]
async fn a_renegotiation_that_changes_the_format_tears_the_graph_down_rather_than_lying_to_it() {
    let (mut session, peer, _addr) = session_and_peer().await;
    let peer_addr = peer.local_addr().expect("has an address");
    let ledger = Arc::new(Ledger::default());

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Witness::boxed("witness", &ledger, 0))
                .with_supervised(supervised_gain2()),
        )
        .expect("a validated outbound graph activates");
    assert_eq!(graph.worker_pids().len(), 1);

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    assert!(session.send(sentinel_tone()).await);
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the first frame reaches the seam")
        .expect("a frame");
    assert_eq!(ledger.frames(), 1);

    // A renegotiation to the other G.711 codec: 8 kHz, 160 positions, exactly what the chain was
    // validated for. The graph is carried over, as `M-64` says it is.
    let mut same = Config::new(peer_addr, Codec::Pcma);
    same.rtcp_interval = None;
    assert!(session.reconfigure(same).await.expect("renegotiates"));
    assert_eq!(
        graph.generation(),
        1,
        "an unchanged format carries the graph over"
    );
    assert_eq!(graph.barrier().workers(), 1, "and its worker with it");
    assert!(graph.counters().resets() > 0, "the epoch was re-anchored");

    // A renegotiation to G.722: 16 kHz audio over an 8000 RTP clock, so both the format and the
    // frame the chain was sized for change.
    let mut wide = Config::new(peer_addr, Codec::G722);
    wide.rtcp_interval = None;
    assert!(session.reconfigure(wide).await.expect("renegotiates"));

    assert_eq!(graph.generation(), 0, "the graph did not survive it");
    let barrier = graph.settled().await;
    assert!(
        barrier.is_clear(),
        "the teardown left the graph holding something: {barrier:?}"
    );
    assert!(
        graph.worker_pids().is_empty(),
        "a torn-down graph still names a worker process"
    );
    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::TornDown {
                cause: TeardownCause::FormatChanged,
                ..
            }
        )),
        "no typed transition said why the graph ended: {transitions:?}"
    );
    let counters = graph.counters();
    assert_eq!(counters.teardowns(), 1);
    assert_eq!(
        counters.terminal_failures(),
        0,
        "a format change is not a stage failing closed"
    );
    assert_eq!(
        counters.frames_passed_through(),
        0,
        "no frame was carried at a rate the chain never agreed to"
    );

    // Nothing reaches the old chain at the new rate, and nothing is re-attached behind the
    // application's back.
    let seen = ledger.frames();
    let mut transmitted = session
        .attach_processor(Processing::new(
            AudioDirection::Outbound,
            PcmFormat::new(16_000, PcmEncoding::Signed16).expect("a supported format"),
        ))
        .expect("attaches to transmitted audio");
    assert!(session.send(vec![SENTINEL; 320]).await);
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the call keeps carrying audio")
        .expect("a frame");
    assert_eq!(
        ledger.frames(),
        seen,
        "a stage prepared for 8 kHz was handed a 16 kHz frame"
    );

    // Recovery is the application's: a graph for the format the call is now carrying attaches, and
    // is sized for it rather than for the one that ended.
    let fresh = Arc::new(Ledger::default());
    let regrown = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Witness::boxed("regrown", &fresh, 0)),
        )
        .expect("a graph for the new format attaches");
    assert!(session.send(vec![SENTINEL; 320]).await);
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the call keeps carrying audio")
        .expect("a frame");
    assert_eq!(fresh.frames(), 1, "the new chain sees the new frames whole");
    assert_eq!(
        regrown.counters().frames_passed_through(),
        0,
        "the new graph was sized for the packetisation the call is running"
    );

    session.shutdown().await;
}

/// Acceptance row 4: a processor that reneges on cancellation cannot keep the graph, or the call,
/// alive.
///
/// `docs/specs/custom-call-dsp.md` §8.4 makes cancellation terminal and idempotent, and §11.3's
/// `ZombieCancel` fixture is what proves the conformance harness catches a processor that ignores
/// it. This is the other question: a graph that attached one anyway must still reach a clear
/// barrier, because §8's barrier is read off the graph's own structure rather than off the
/// processor's word.
#[tokio::test]
async fn a_processor_that_ignores_cancellation_cannot_keep_the_graph_alive() {
    let (session, _peer, _addr) = session_and_peer().await;
    let cancelled = Arc::new(AtomicU64::new(0));
    let after_cancel = Arc::new(AtomicU64::new(0));

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                ZombieCancel {
                    cancelled: Arc::clone(&cancelled),
                    after_cancel: Arc::clone(&after_cancel),
                },
            )),
        )
        .expect("a validated outbound graph activates");

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    assert!(session.send(sentinel_tone()).await);
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the first frame reaches the seam")
        .expect("a frame");

    // While the graph is live the barrier reports what the stage claims, because that is what
    // `retained()` is for and the graph has no way to check it.
    assert_eq!(graph.barrier().retained_positions(), 7);

    let barrier = graph.detach().await;
    assert!(
        barrier.is_clear(),
        "a processor's claim to be holding audio outlived the graph that held it: {barrier:?}"
    );
    assert!(cancelled.load(Ordering::SeqCst) >= 1, "it was cancelled");

    // And no frame reaches it afterwards, whatever it thinks: the generation is gone.
    assert!(session.send(sentinel_tone()).await);
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the call keeps carrying audio")
        .expect("a frame");
    assert_eq!(
        after_cancel.load(Ordering::SeqCst),
        0,
        "a cancelled stage went on seeing this call's audio"
    );

    session.shutdown().await;
}

/// A supervised worker in the named mode, with the miss budget and action a failure case needs.
fn failing(id: &'static str, mode: &str, misses: u32, action: FailureAction) -> WorkerProcess {
    WorkerProcess::new(
        DspCapability::new(id).with_execution(
            ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated)
                .with_max_consecutive_misses(misses)
                .with_on_failure(action),
        ),
        WORKER,
    )
    .arg("--mode")
    .arg(mode)
}

/// Acceptance row 5: RTP stays serviced through a worker that hangs, one that answers with the
/// wrong number of positions and one that leaves mid-call, and every owned thing drains through the
/// barrier.
///
/// Three isolated endings in one chain, driven for a bounded number of frames, with every frame
/// awaited at the seam rather than timed. Nothing here waits a duration: the arrivals are the
/// clock, and `ARRIVAL_BOUND` is only what turns a hang into a failure instead of a hang. The
/// fourth ending §7.2 tabulates — a worker that crashes — is `M-102`'s own test, which waits for
/// the operating system to finish tearing a core-dumping process down; repeating it inside a
/// twelve-frame sweep would measure that teardown rather than this one.
#[tokio::test]
async fn rtp_stays_serviced_through_worker_hangs_malformed_results_and_lost_workers() {
    const FRAMES: usize = 12;

    let (session, _peer, _addr) = session_and_peer().await;

    // A deliberately tiny transition queue, so the drop-oldest end of §5.3 is exercised and the
    // counters have to be what survives it.
    let bounds = GraphBounds::new()
        .with_observation_capacity(2)
        .with_worker_queue_capacity(2);
    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, bounds)
                .with_supervised(failing("hangs", "hang", 2, FailureAction::BypassOpen))
                .with_supervised(failing("short", "short", 2, FailureAction::BypassOpen))
                // Answers two frames and then leaves, with a budget long enough to keep being
                // offered frames afterwards — which is how the *ending* rather than the deadline
                // is what this stage records.
                .with_supervised(
                    failing("leaves", "exit", 6, FailureAction::BypassOpen)
                        .arg("--after")
                        .arg("2"),
                ),
        )
        .expect("a validated outbound graph activates");
    assert_eq!(
        graph.worker_pids().len(),
        3,
        "three stages, three processes"
    );
    assert!(
        graph.contains_overrun(),
        "a chain of supervised stages may claim that over-budget work cannot stall RTP"
    );

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    for frame in 0..FRAMES {
        assert!(session.send(sentinel_tone()).await, "queues frame {frame}");
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .unwrap_or_else(|_| panic!("frame {frame} reached RTP while three workers misbehaved"))
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()).len(),
            SAMPLES_PER_PACKET,
            "frame {frame} kept the session's packetisation"
        );
        // Bounded, per frame, and never cumulative: what the graph lends its workers is the
        // channel depth it was configured with and nothing more.
        assert!(
            graph.barrier().frames_in_flight() <= 3 * 2,
            "frame {frame}: the graph holds more of this call's audio than its channels admit"
        );
    }

    let counters = graph.counters();
    assert_eq!(counters.frames(), FRAMES as u64);
    assert!(counters.deadline_misses() > 0, "{counters:?}");
    assert!(counters.malformed_results() > 0, "{counters:?}");
    assert!(counters.workers_lost() > 0, "{counters:?}");
    assert_eq!(counters.bypasses(), 3, "every stage spent its budget");
    assert_eq!(counters.frames_passed_through(), 0);
    assert!(
        counters.transitions_dropped() > 0,
        "a two-slot queue took every transition a twelve-frame run produced: {counters:?}"
    );
    // The queue itself never grew past what it was configured for, which is the other half of the
    // same sentence: a bounded queue that dropped nothing would be a queue that grew.
    assert!(graph.transitions().len() <= 2);

    let barrier = graph.detach().await;
    assert!(
        barrier.is_clear(),
        "a hung worker, a malformed one and a departed one left something behind: {barrier:?}"
    );
    assert!(graph.worker_pids().is_empty());
    assert_eq!(
        graph.counters().teardowns(),
        1,
        "the detach is the one teardown, and no worker was respawned into another"
    );

    session.shutdown().await;
}

/// Acceptance row 6: nothing upgrades the cooperative-native profile to the containment claim.
///
/// `docs/specs/custom-call-dsp.md` §7.3: `contains_overrun()` is `false` for that profile
/// unconditionally, and no harness result, measurement or configuration may set it true. §3.3 of
/// the graph spec extends that to a chain — a graph is only as contained as its least contained
/// stage. This drives all three doors and then tries, on purpose, the two things that most look
/// like they might change the answer: a clean run in which no counter moves at all, and the
/// strictest policy a capability can declare.
#[tokio::test]
async fn nothing_upgrades_the_cooperative_native_profile_to_the_containment_claim() {
    // The profile's own answer, before any graph exists.
    let native = ExecutionProfile::TrustedCooperativeNative;
    assert!(!native.contains_overrun());
    assert_eq!(native.containment(), OverrunContainment::None);
    assert_eq!(native.deadline_action(), DeadlineAction::AccountAfterReturn);
    assert!(
        native.runs_on_media_worker(),
        "the profile that claims nothing is the one that runs where a stall would land"
    );
    assert!(ExecutionProfile::SupervisedIsolated.contains_overrun());
    assert!(!ExecutionProfile::SupervisedIsolated.runs_on_media_worker());

    let (session, _peer, _addr) = session_and_peer().await;

    // A supervised-only chain may claim it.
    let contained = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_supervised(supervised_gain2()),
        )
        .expect("activates");
    assert!(contained.contains_overrun());

    // One cooperative stage beside it and the whole chain claims nothing — including under the
    // strictest policy the vocabulary can express, which is the configuration that most looks like
    // it should buy something and buys nothing.
    let mixed = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_supervised(supervised_gain2())
                .with_processor(Box::new(Strict)),
        )
        .expect("activates");
    assert!(
        !mixed.contains_overrun(),
        "one cooperative-native stage left the chain claiming containment"
    );

    // A clean run changes nothing. This is the shape of the upgrade the spec forbids: every frame
    // behaved, no counter moved, and the answer is what it was before the call started.
    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    for _ in 0..8 {
        assert!(session.send(sentinel_tone()).await);
        tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("audio reaches the seam")
            .expect("a frame");
    }
    let counters = mixed.counters();
    assert!(counters.frames() >= 8);
    // §7.1: the supervised stage's pipeline has to fill, and the one frame that has no result yet
    // is a miss like any other rather than a special case. Nothing else went wrong.
    assert_eq!(counters.deadline_misses(), 1, "{counters:?}");
    assert_eq!(counters.misses(), 1, "{counters:?}");
    assert_eq!(counters.bypasses(), 0, "no stage failed: {counters:?}");
    assert!(
        !mixed.contains_overrun(),
        "a run in which nothing went wrong upgraded the profile's claim"
    );
    assert!(
        contained.contains_overrun(),
        "and the supervised chain's own claim is untouched by the other direction's"
    );

    session.shutdown().await;
}

/// A cooperative-native stage under the strictest policy the vocabulary can express.
struct Strict;

impl FrameProcessor for Strict {
    fn capability(&self) -> DspCapability {
        DspCapability::new("strict").with_execution(
            ExecutionPolicy::new(ExecutionProfile::TrustedCooperativeNative)
                .with_deadline_frames(1)
                .with_max_consecutive_misses(1)
                .with_on_failure(FailureAction::TerminateClosed),
        )
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
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        sink.write(frame.samples())
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

/// Acceptance row 3: an intentional glitch effect's observations are not the runtime's counters.
///
/// `docs/specs/custom-call-dsp.md` §6 keeps the two vocabularies apart on purpose — an effect that
/// is *supposed* to sound broken emits a [`DspObservation`] about the audio it made, and a
/// processor that is failing its budget moves a counter the runtime keeps. This stage emits one
/// observation per frame and one saturation report per frame, over full-scale hostile audio, and
/// nothing it says reaches a single counter.
#[tokio::test]
async fn an_intentional_glitch_effect_moves_no_runtime_counter() {
    /// A stage whose whole output vocabulary is observations: it saturates deliberately and says
    /// so, which is exactly what `M-65`'s glitch effects do.
    struct Glitch {
        emitted: Arc<AtomicU64>,
    }

    impl FrameProcessor for Glitch {
        fn capability(&self) -> DspCapability {
            DspCapability::new("glitch").with_execution(ExecutionPolicy::new(
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
            frame: &DspFrame<'_>,
            _scratch: &mut Scratch<'_>,
            sink: &mut FrameSink<'_>,
        ) -> Result<(), ProcessError> {
            let mut clipped = 0;
            for sample in frame.samples() {
                let loud = sample.saturating_mul(8);
                clipped += u32::from(loud == i16::MAX || loud == i16::MIN);
                sink.push(loud)?;
            }
            sink.emit(DspObservation::Saturated { positions: clipped });
            sink.emit(DspObservation::PassedThrough);
            self.emitted.fetch_add(2, Ordering::SeqCst);
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

    let (session, _peer, _addr) = session_and_peer().await;
    let emitted = Arc::new(AtomicU64::new(0));
    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                Glitch {
                    emitted: Arc::clone(&emitted),
                },
            )),
        )
        .expect("activates");

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches to transmitted audio");
    for (name, program) in hostile_programs() {
        assert!(session.send(program).await, "queues {name}");
        tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .unwrap_or_else(|_| panic!("{name} reaches the seam"))
            .expect("a frame");
    }

    assert!(
        emitted.load(Ordering::SeqCst) >= 2,
        "the effect never got to observe anything"
    );
    let counters = graph.counters();
    assert_eq!(
        counters.misses(),
        0,
        "a deliberate saturation was counted as a runtime failure: {counters:?}"
    );
    assert_eq!(counters.bypasses(), 0, "{counters:?}");
    assert_eq!(counters.teardowns(), 0, "{counters:?}");
    assert_eq!(counters.terminal_failures(), 0, "{counters:?}");
    assert_eq!(counters.transitions_dropped(), 0, "{counters:?}");
    assert_eq!(
        counters.frames(),
        hostile_programs().len() as u64,
        "the only counter a well-behaved effect moves is the frame count"
    );
    assert!(
        graph
            .transitions()
            .iter()
            .all(|transition| matches!(transition, GraphTransition::Activated { .. })),
        "an effect's own observation became a graph transition"
    );

    session.shutdown().await;
}
