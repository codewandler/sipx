//! The SDK's door onto a call's DSP graph (`docs/specs/call-dsp-graph.md` §10, `M-67`).
//!
//! `M-64` built the graph and `M-65` filled the registry. What is proved here is the *door*: that
//! an application can discover which processors exist and what each one's closed parameter schema
//! is, name them in order on a direction, move their parameters at a declared sample boundary with
//! one terminal outcome, and read every transition — while none of that lets it declare a profile,
//! borrow an audio-thread object or assemble a containment claim.
//!
//! These run against a live `MediaSession` on loopback, for the reason `dsp_graph.rs` gives: the
//! claims worth proving are about the attachment rather than about the graph in isolation.

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
    BuiltIn, DspCapability, DspFrame, DspResetCause, ExecutionPolicy, ExecutionProfile,
    FailureAction, FormatError, FrameAdmission, FrameProcessor, FrameSink, GraphBounds, GraphError,
    GraphPlan, GraphTransition, Parameter, ParameterDomain, ParameterError, ParameterValue,
    ProcessError, Scratch, StreamFormat,
};
use sipx_media::{AudioDirection, Codec, Config, MediaPort, MediaSession, PcmEncoding, PcmFormat};
use tokio::net::UdpSocket;

/// A bound on failure and never a window anything is measured in.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

// ------------------------------------------------------------------------- fixtures ----

/// An application-supplied processor: the only profile the public door admits.
struct AppProcessor {
    admission: FrameAdmission,
    profile: ExecutionProfile,
}

impl AppProcessor {
    fn new(profile: ExecutionProfile) -> Self {
        Self {
            admission: FrameAdmission::new(),
            profile,
        }
    }
}

impl FrameProcessor for AppProcessor {
    fn capability(&self) -> DspCapability {
        DspCapability::new("app.processor").with_execution(ExecutionPolicy::new(self.profile))
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

/// A plan an application could have written: registered identifiers and closed parameter values.
fn gain_plan(gain: i32) -> Result<GraphPlan, GraphError> {
    GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_built_in(
        BuiltIn::Gain,
        &[Parameter::new("gain", ParameterValue::Ratio(gain))],
    )
}

// ----------------------------------------------------------------------------- tests ----

/// Acceptance row 1: the SDK exposes processor discovery and each processor's closed parameter
/// schema.
///
/// `M-65` left the registry list behind `#[cfg(test)]` and said in as many words that `M-67` is
/// where an SDK gets a reason to publish one. Discovery is that list plus, per entry, the schema an
/// application is held to — which has to come from the processor's own declaration rather than from
/// a second table beside it.
#[test]
fn the_registry_is_discoverable_with_every_processor_s_closed_parameter_schema() {
    let registered = BuiltIn::registered();
    assert!(
        registered.len() >= 10,
        "the registry names this workspace's nine effects and its noise reducer"
    );

    for entry in registered {
        let id = entry.id();
        assert!(
            id.starts_with("sipx."),
            "{id} is not a workspace identifier"
        );
        assert_eq!(
            BuiltIn::from_id(id, 1),
            Some(*entry),
            "{id} does not resolve back to the entry that publishes it"
        );

        // The schema is closed: every parameter has an identifier and a domain with an end.
        for spec in entry.parameters() {
            assert!(!spec.id().is_empty(), "{id} declares a nameless parameter");
            match spec.domain() {
                ParameterDomain::Integer { min, max } => assert!(min <= max, "{id}/{}", spec.id()),
                ParameterDomain::Ratio { min, max } => assert!(min <= max, "{id}/{}", spec.id()),
                // `Flag`, and a domain a later revision adds: still a domain with ends, because
                // the vocabulary is closed by construction (`docs/specs/custom-call-dsp.md`
                // §3.6). There is nothing here to check that the type has not already decided.
                _ => {}
            }
        }
    }

    // A name nothing registered is not a processor, however plausible it looks.
    assert_eq!(BuiltIn::from_id("sipx.reverb", 1), None);
    assert_eq!(BuiltIn::from_id("gain", 1), None);

    // The one thing a parameter cannot express is a processor's shape, so the resolver takes it.
    assert_eq!(
        BuiltIn::from_id(
            sipx_media::dsp::BuiltIn::Stutter { delay_positions: 7 }.id(),
            7
        ),
        Some(BuiltIn::Stutter { delay_positions: 7 })
    );

    // Discovery is the schema the graph actually holds a stage to, not a copy of it.
    let gain = BuiltIn::Gain.parameters();
    assert!(gain.iter().any(|spec| spec.id() == "gain"));
    assert!(
        gain_plan(2_000).is_ok(),
        "a value inside the published domain is admitted"
    );
    assert!(
        matches!(gain_plan(99_000), Err(GraphError::Parameter { .. })),
        "a value outside it is refused where the caller wrote it"
    );
}

/// Acceptance row 2: a parameter update is validated off the media path, applied at a declared
/// sample boundary, and has exactly one terminal outcome carrying generation identity.
#[tokio::test]
async fn a_parameter_update_applies_at_a_declared_sample_boundary_with_one_terminal_outcome() {
    let (session, _peer, _addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(gain_plan(1_000).expect("a registered plan"))
        .expect("a validated graph activates");
    let generation = graph.generation();
    assert_eq!(generation, 1);

    let update = graph
        .configure(
            generation,
            0,
            &[Parameter::new("gain", ParameterValue::Ratio(2_000))],
        )
        .expect("a value inside the published domain applies");

    assert_eq!(update.generation(), generation, "generation identity");
    assert_eq!(update.processor(), BuiltIn::Gain.id());
    // The boundary is a position in the graph's own epoch and never a clock reading: nothing has
    // been sent, so the change takes effect at the very first position.
    assert_eq!(update.at_position(), 0);

    // Exactly one terminal transition, and it names the same boundary the caller was told.
    let transitions = graph.transitions();
    let configured: Vec<&GraphTransition> = transitions
        .iter()
        .filter(|transition| matches!(transition, GraphTransition::Configured { .. }))
        .collect();
    assert_eq!(
        configured.len(),
        1,
        "one update, one outcome: {transitions:?}"
    );
    assert_eq!(
        configured[0],
        &GraphTransition::Configured {
            generation,
            at_position: update.at_position(),
            processor: BuiltIn::Gain.id(),
        }
    );
}

/// Acceptance row 4: unknown processors and parameters and stale generations are refused, and the
/// active graph is not changed by any of them.
#[tokio::test]
async fn an_unknown_processor_a_bad_parameter_or_a_stale_generation_changes_nothing() {
    let (session, _peer, _addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(gain_plan(1_000).expect("a registered plan"))
        .expect("a validated graph activates");
    let live = graph.generation();
    let good = [Parameter::new("gain", ParameterValue::Ratio(2_000))];

    // A stage index the chain does not have.
    assert!(matches!(
        graph.configure(live, 7, &good),
        Err(GraphError::UnknownProcessor { .. })
    ));
    // A parameter the published schema does not declare.
    assert!(matches!(
        graph.configure(
            live,
            0,
            &[Parameter::new("reverb", ParameterValue::Ratio(1))]
        ),
        Err(GraphError::Parameter { .. })
    ));
    // A value outside the published domain.
    assert!(matches!(
        graph.configure(
            live,
            0,
            &[Parameter::new("gain", ParameterValue::Ratio(99_000))]
        ),
        Err(GraphError::Parameter { .. })
    ));
    // A generation that is no longer the live one.
    assert!(matches!(
        graph.configure(live + 1, 0, &good),
        Err(GraphError::StaleGeneration { .. })
    ));
    assert!(matches!(
        graph.configure(0, 0, &good),
        Err(GraphError::StaleGeneration { .. })
    ));

    assert_eq!(graph.generation(), live, "nothing replaced the graph");
    assert!(
        graph
            .transitions()
            .iter()
            .all(|transition| !matches!(transition, GraphTransition::Configured { .. })),
        "a refused update is not an outcome"
    );
}

/// Acceptance row 5: the bounded transition queue coalesces superseded parameter state and keeps
/// every activation, bypass, failure and terminal transition.
#[tokio::test]
async fn the_transition_queue_coalesces_superseded_parameter_state_and_keeps_the_rest() {
    let (session, _peer, _addr) = session_and_peer().await;
    let bounds = GraphBounds::new().with_observation_capacity(4);
    let plan = GraphPlan::new(AudioDirection::Outbound, bounds)
        .with_built_in(
            BuiltIn::Gain,
            &[Parameter::new("gain", ParameterValue::Ratio(1_000))],
        )
        .expect("a registered plan");
    let graph = session.attach_dsp(plan).expect("a validated graph");
    let generation = graph.generation();

    // Far more parameter updates than the queue can hold. Under drop-oldest alone the activation
    // would be gone; superseded parameter state is what gives way instead.
    for step in 1..=32 {
        graph
            .configure(
                generation,
                0,
                &[Parameter::new("gain", ParameterValue::Ratio(step * 100))],
            )
            .expect("each value is inside the published domain");
    }

    let transitions = graph.transitions();
    assert!(
        transitions
            .iter()
            .any(|transition| matches!(transition, GraphTransition::Activated { .. })),
        "the activation survived a burst of parameter updates: {transitions:?}"
    );
    let configured = transitions
        .iter()
        .filter(|transition| matches!(transition, GraphTransition::Configured { .. }))
        .count();
    assert_eq!(
        configured, 1,
        "one stage's parameter state coalesces to its latest: {transitions:?}"
    );
}

/// Acceptance row 3: an application selects registered identifiers and reads bounded events. It
/// never borrows an audio-thread object, and the wait for the next event is an event.
#[tokio::test]
async fn a_caller_reads_bounded_transitions_without_polling_the_media_worker() {
    let (session, _peer, _addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(gain_plan(1_000).expect("a registered plan"))
        .expect("a validated graph activates");

    let first = tokio::time::timeout(ARRIVAL_BOUND, graph.next_transitions())
        .await
        .expect("the activation is already queued");
    assert!(matches!(
        first.first(),
        Some(GraphTransition::Activated { .. })
    ));

    // Nothing is queued now, so this resolves only once something happens — never on a timer.
    let graph = std::sync::Arc::new(graph);
    let waiting = tokio::spawn({
        let graph = std::sync::Arc::clone(&graph);
        async move { graph.next_transitions().await }
    });
    graph
        .configure(
            graph.generation(),
            0,
            &[Parameter::new("gain", ParameterValue::Ratio(1_500))],
        )
        .expect("applies");
    let woken = tokio::time::timeout(ARRIVAL_BOUND, waiting)
        .await
        .expect("the wait is woken by the change and not by a clock")
        .expect("the task did not panic");
    assert!(matches!(
        woken.first(),
        Some(GraphTransition::Configured { .. })
    ));
}

/// The epic's central rule, held against a plan an application assembled: **provenance is a
/// property of the stage, not of the plan.**
///
/// Naming a built-in beside an application-supplied processor must not lend that processor the
/// proven profile, and the public door must refuse exactly what it refused before this story.
#[tokio::test]
async fn naming_a_built_in_lends_an_application_s_processor_no_provenance() {
    let (session, _peer, _addr) = session_and_peer().await;
    let refused = GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
        .with_built_in(
            BuiltIn::Gain,
            &[Parameter::new("gain", ParameterValue::Ratio(1_000))],
        )
        .expect("a registered stage")
        .with_processor(Box::new(AppProcessor::new(ExecutionProfile::ProvenInline)));

    // The refusal names the stage rather than the plan, and the plan is refused whole.
    let error = session
        .attach_dsp(refused)
        .expect_err("proven is not for the asking");
    assert!(
        matches!(
            error,
            GraphError::ProfileNotAdmissible {
                processor: "app.processor",
                profile: ExecutionProfile::ProvenInline,
            }
        ),
        "{error:?}"
    );
}

/// The other half of the same rule: **an application that assembles a graph must not assemble a
/// claim.**
///
/// `contains_overrun` is the conjunction over the stages' profiles and is read back from the live
/// graph. There is no door through which a caller states it.
#[tokio::test]
async fn an_assembled_graph_reports_its_containment_and_never_accepts_one() {
    let (session, _peer, _addr) = session_and_peer().await;

    // Every stage an application can name through the registry is proven-inline, so a chain of
    // them contains its own overruns — because of what the stages are, not because it said so.
    let registered = session
        .attach_dsp(gain_plan(1_000).expect("a registered plan"))
        .expect("a validated graph activates");
    assert!(registered.contains_overrun());
    registered.detach().await;

    // The same application supplying its own processor gets the honest answer instead, and no
    // configuration, parameter or built-in beside it changes that.
    let cooperative = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_built_in(
                    BuiltIn::Gain,
                    &[Parameter::new("gain", ParameterValue::Ratio(1_000))],
                )
                .expect("a registered stage")
                .with_processor(Box::new(AppProcessor::new(
                    ExecutionProfile::TrustedCooperativeNative,
                ))),
        )
        .expect("a cooperative-native chain is admissible, it just claims nothing");
    assert!(
        !cooperative.contains_overrun(),
        "one cooperative-native stage makes the whole chain uncontained"
    );
    cooperative.detach().await;
}

/// A supervised stage has no wire for a parameter set, so the door says so rather than pretending.
#[tokio::test]
async fn a_stage_with_no_parameter_wire_is_refused_rather_than_silently_ignored() {
    let (session, _peer, _addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_processor(Box::new(
                AppProcessor::new(ExecutionProfile::TrustedCooperativeNative),
            )),
        )
        .expect("a validated graph activates");

    // An application-supplied processor declares no parameters at all, so every identifier is
    // outside its closed schema and the whole set is refused.
    assert!(matches!(
        graph.configure(
            graph.generation(),
            0,
            &[Parameter::new("gain", ParameterValue::Ratio(1_000))]
        ),
        Err(GraphError::Parameter { .. })
    ));
}

/// A processor that misses every frame, so a chain of them bypasses on the first one.
struct AlwaysMisses;

impl FrameProcessor for AlwaysMisses {
    fn capability(&self) -> DspCapability {
        DspCapability::new("app.misses").with_execution(
            ExecutionPolicy::new(ExecutionProfile::TrustedCooperativeNative)
                .with_max_consecutive_misses(1)
                .with_on_failure(FailureAction::BypassOpen),
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
        _frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        _sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        Err(ProcessError::MalformedFrame)
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

/// The seam `M-67` and `M-68` meet on: **a bypass the media worker journals wakes a reader, and
/// two stages missing on one frame are two events rather than one.**
///
/// Neither story proves this on its own. `M-67`'s wake test drives a `configure` from the caller's
/// own thread, where the signal and the wait are obviously the same lock; `M-68`'s bypass tests
/// drain the queue rather than wait on it. The combination is the thing that could regress
/// silently: `Generation::run` journals a bypass from the media worker, mid-frame, with no
/// knowledge that anybody is waiting — and `Journal::push` is what has to trip the signal, which is
/// why the signal lives on the journal and not on the call sites.
#[tokio::test]
async fn a_bypass_the_media_worker_journals_wakes_a_waiting_reader() {
    let (session, _peer, _addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_processor(Box::new(AlwaysMisses))
                .with_processor(Box::new(AlwaysMisses)),
        )
        .expect("a validated chain activates");
    let mut transmitted = session
        .attach_processor(sipx_media::Processing::new(
            AudioDirection::Outbound,
            PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format"),
        ))
        .expect("attaches");

    // Take the activation, so the queue is empty and the next wait is on something still to come.
    let first = tokio::time::timeout(ARRIVAL_BOUND, graph.next_transitions())
        .await
        .expect("the activation is already queued");
    assert!(matches!(
        first.first(),
        Some(GraphTransition::Activated { .. })
    ));

    let graph = std::sync::Arc::new(graph);
    let waiting = tokio::spawn({
        let graph = std::sync::Arc::clone(&graph);
        async move { graph.next_transitions().await }
    });

    // One frame, which both stages miss. Nothing on the caller's side touches the journal.
    assert!(session.send(vec![1_000_i16; 160]).await, "queues audio");
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the call keeps carrying audio")
        .expect("a frame");

    let woken = tokio::time::timeout(ARRIVAL_BOUND, waiting)
        .await
        .expect("the media worker's own journal write woke the reader, not a clock")
        .expect("the task did not panic");

    // Two stages spent their budget on the same frame, so there are two entries: an `Option` on
    // the frame's report would have kept only the last (`M-68`).
    let bypasses = woken
        .iter()
        .filter(|transition| matches!(transition, GraphTransition::Bypassed { .. }))
        .count();
    assert_eq!(bypasses, 2, "{woken:?}");
    assert_eq!(
        graph.counters().bypasses(),
        2,
        "and the counters moved for the same two"
    );
}
