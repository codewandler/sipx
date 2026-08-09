//! The host's DSP door: §6.2's three `dsp` verbs, and §5.3's five `call.dsp.*` rows (`M-67`).
//!
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §6.6 is the wire, and
//! [`docs/specs/call-dsp-graph.md`](../../../docs/specs/call-dsp-graph.md) §10 is the graph. This
//! module is the join: it resolves the **identifiers** an application named against this
//! workspace's own registry, assembles a plan out of them, and turns the graph's typed transitions
//! back into contract events.
//!
//! It is where the five `call.dsp.*` rows are composed, because nothing else can be: a graph's
//! transitions are `sipx-media`'s facts, no `sipx-call` event carries one, and
//! [`sipx_app_protocol::event_from_call`] therefore has no arm for them. `spec_tables.rs` holds
//! this file to that claim by name.
//!
//! # What an application reaches through here, and what it does not
//!
//! **Reaches.** Registered processor identifiers, in an order, on one direction; a shape where a
//! processor declares one; finite parameter values; a parameter set moved at a sample boundary
//! against a named generation; a removal; and every transition the graph reports.
//!
//! **Does not reach.** [`GraphPlan::with_processor`] and [`GraphPlan::with_supervised`] are not
//! called from this module and cannot be: the wire carries no processor and no program. So every
//! stage an application assembles arrives through [`GraphPlan::with_built_in`] — one stage at a
//! time, which is what keeps provenance a property of the stage. Nothing here sets a bound, a
//! profile, a deadline or a failure action, and `contains_overrun` is **read off the live graph**
//! and reported; there is no path by which an application supplies one.

use std::collections::VecDeque;

use sipx_app_protocol::{
    AudioDirection, DspBypassCause, DspParameter, DspRefusal, DspStage, DspTeardownCause, DspValue,
    EventKind,
};
use sipx_media::MediaSession;
use sipx_media::dsp::{
    BuiltIn, BypassCause, DspGraph, GraphBounds, GraphError, GraphPlan, GraphTransition, Parameter,
    ParameterSpec, ParameterValue, TeardownCause,
};

/// One direction's live chain, and the registry entries it was assembled from.
///
/// The entries are kept because the contract's parameter identifiers are `&'static str` — they are
/// the processor's own, from its published schema — and the wire's are strings a peer chose. Every
/// wire identifier is therefore **resolved against the schema of the stage it names** before it can
/// become a parameter at all, so an identifier the processor does not declare is refused here,
/// where the application wrote it, and never becomes a `Parameter` with a borrowed lifetime nobody
/// could give it.
#[derive(Debug)]
struct Attached {
    graph: DspGraph,
    stages: Vec<BuiltIn>,
}

/// The graphs one call owns, at most one per direction, and the events they have produced.
///
/// Held by the call's actor for the life of the call. Dropping it detaches every graph, because
/// [`DspGraph`]'s own `Drop` cancels each stage — a call that ended does not leave a chain running.
#[derive(Debug, Default)]
pub(crate) struct CallGraphs {
    inbound: Option<Attached>,
    outbound: Option<Attached>,
    /// Events drained from a graph and not yet handed to the interpreter.
    ///
    /// The buffer is what makes [`Self::next_event`] safe to cancel inside a `select!`: a drain and
    /// the push that follows it happen in one poll, with no suspension between them, so a losing
    /// branch is one that never drained.
    pending: VecDeque<EventKind>,
}

impl CallGraphs {
    /// Set one direction's chain: attach when there is none, replace when there is (§6.2's `dsp`).
    ///
    /// Returns the instruction's **one** terminal outcome. A refusal leaves the direction running
    /// exactly as it was, with the previous chain in force if there was one — the plan is validated
    /// whole before anything is prepared, so there is no half-applied graph to recover from.
    pub(crate) fn set(
        &mut self,
        session: &MediaSession,
        instruction_id: String,
        direction: AudioDirection,
        processors: &[DspStage],
    ) -> EventKind {
        let (plan, stages) = match plan_for(direction, processors) {
            Ok(built) => built,
            Err(reason) => return refused(instruction_id, direction, reason),
        };
        let replaced = self.slot(direction).is_some();
        let outcome = match self.slot(direction).as_mut() {
            Some(attached) => attached.graph.replace(plan).inspect(|_| {
                attached.stages = stages;
            }),
            None => session.attach_dsp(plan).map(|graph| {
                let generation = graph.generation();
                *self.slot(direction) = Some(Attached { graph, stages });
                generation
            }),
        };
        let generation = match outcome {
            Ok(generation) => generation,
            Err(error) => return refused(instruction_id, direction, refusal_for(error)),
        };
        let Some(attached) = self.slot(direction).as_ref() else {
            return refused(instruction_id, direction, DspRefusal::Rejected);
        };
        let contains_overrun = attached.graph.contains_overrun();
        let processors = u32::try_from(processors.len()).unwrap_or(u32::MAX);
        // discard: the queue now holds the activation this call is about to report, and nothing
        // older than it is about a generation that still exists. Reporting from the return value
        // rather than from the queue is what gives the instruction its correlation.
        drop(attached.graph.transitions());
        EventKind::DspActivated {
            instruction_id,
            direction,
            generation,
            previous: replaced.then(|| generation.saturating_sub(1)),
            processors,
            contains_overrun,
        }
    }

    /// Move one live stage's parameters at a sample boundary (§6.2's `dsp_param`).
    ///
    /// The outcome is the graph's own, correlated here rather than read back out of the transition
    /// queue: that queue coalesces superseded parameter state (`call-dsp-graph.md` §5.3), and an
    /// outcome an application is waiting on may not be something a later update absorbs.
    pub(crate) fn configure(
        &self,
        instruction_id: String,
        direction: AudioDirection,
        generation: u64,
        processor: u32,
        parameters: &[DspParameter],
    ) -> EventKind {
        let Some(attached) = self.peek(direction) else {
            return refused(instruction_id, direction, DspRefusal::NoGraph);
        };
        let Some(stage) = attached.stages.get(processor as usize) else {
            return refused(instruction_id, direction, DspRefusal::UnknownProcessor);
        };
        let set = match resolve(stage.parameters(), parameters) {
            Ok(set) => set,
            Err(reason) => return refused(instruction_id, direction, reason),
        };
        match attached
            .graph
            .configure(generation, processor as usize, &set)
        {
            Ok(update) => EventKind::DspConfigured {
                instruction_id,
                direction,
                generation: update.generation(),
                at_position: update.at_position(),
                processor: update.processor().to_owned(),
            },
            Err(error) => refused(instruction_id, direction, refusal_for(error)),
        }
    }

    /// Remove one direction's chain and wait for its barrier (§6.2's `dsp_remove`).
    ///
    /// The wait is the barrier and never a duration: what is reported is a graph that holds nothing
    /// rather than one that has been asked to.
    pub(crate) async fn remove(
        &mut self,
        instruction_id: String,
        direction: AudioDirection,
    ) -> EventKind {
        let Some(attached) = self.slot(direction).take() else {
            return refused(instruction_id, direction, DspRefusal::NoGraph);
        };
        let generation = attached.graph.generation();
        attached.graph.detach().await;
        EventKind::DspRemoved {
            instruction_id: Some(instruction_id),
            direction,
            generation,
            at_position: 0,
            processor: None,
            cause: DspTeardownCause::Requested,
        }
    }

    /// The next event no instruction asked for: a bypass, or a teardown this call did not request.
    ///
    /// Waits for one. The wait is an event — the graph wakes a reader where it records a transition
    /// — so a driver never polls a call's graph and never times a look at it (§10.3). Resolves
    /// never on a direction with no graph, which is what makes it usable as a `select!` arm for the
    /// whole life of a call.
    pub(crate) async fn next_event(&mut self) -> EventKind {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return event;
            }
            let drained = match (self.inbound.as_ref(), self.outbound.as_ref()) {
                (Some(inbound), Some(outbound)) => tokio::select! {
                    transitions = inbound.graph.next_transitions() => {
                        unsolicited(AudioDirection::Inbound, &transitions)
                    }
                    transitions = outbound.graph.next_transitions() => {
                        unsolicited(AudioDirection::Outbound, &transitions)
                    }
                },
                (Some(inbound), None) => unsolicited(
                    AudioDirection::Inbound,
                    &inbound.graph.next_transitions().await,
                ),
                (None, Some(outbound)) => unsolicited(
                    AudioDirection::Outbound,
                    &outbound.graph.next_transitions().await,
                ),
                (None, None) => std::future::pending().await,
            };
            self.pending.extend(drained);
        }
    }

    /// Detach every graph this call owns, without waiting.
    ///
    /// The call is ending, so the barrier is the session's to reach: `MediaSession::shutdown`
    /// cancels and joins every stage of every direction. Dropping the handles here is what releases
    /// them; it is spelled out rather than left to the struct's drop so that teardown reads in one
    /// place.
    pub(crate) fn detach_all(&mut self) {
        self.inbound = None;
        self.outbound = None;
        self.pending.clear();
    }

    /// Queue an instruction's terminal outcome behind whatever a graph has already reported.
    ///
    /// Every `call.dsp.*` row reaches the interpreter through [`Self::next_event`], solicited or
    /// not, so that one ordering decides them all: an outcome never overtakes a bypass the same
    /// call already recorded.
    pub(crate) fn enqueue(&mut self, event: EventKind) {
        self.pending.push_back(event);
    }

    fn slot(&mut self, direction: AudioDirection) -> &mut Option<Attached> {
        match direction {
            AudioDirection::Inbound => &mut self.inbound,
            AudioDirection::Outbound => &mut self.outbound,
        }
    }

    fn peek(&self, direction: AudioDirection) -> Option<&Attached> {
        match direction {
            AudioDirection::Inbound => self.inbound.as_ref(),
            AudioDirection::Outbound => self.outbound.as_ref(),
        }
    }
}

/// Which transitions are facts nobody asked for, as §5.3 rows.
///
/// `Activated`, `Replaced` and `Configured` are each the outcome of an instruction and are reported
/// from that instruction's own return value, with its `instruction_id` on them. Composing them here
/// as well would send the application two events for one thing it asked for once.
fn unsolicited(direction: AudioDirection, transitions: &[GraphTransition]) -> Vec<EventKind> {
    transitions
        .iter()
        .filter_map(|transition| match *transition {
            GraphTransition::Bypassed {
                generation,
                at_position,
                processor,
                cause,
            } => Some(EventKind::DspBypassed {
                direction,
                generation,
                at_position,
                processor: processor.to_owned(),
                cause: bypass_cause(cause),
            }),
            GraphTransition::TornDown {
                generation,
                at_position,
                cause,
            } => Some(EventKind::DspRemoved {
                instruction_id: None,
                direction,
                generation,
                at_position,
                processor: match cause {
                    TeardownCause::FailedClosed { processor } => Some(processor.to_owned()),
                    _ => None,
                },
                cause: teardown_cause(cause),
            }),
            _ => None,
        })
        .collect()
}

/// Assemble a plan out of names, and nothing but names (§10.1).
///
/// Every stage goes through [`GraphPlan::with_built_in`] — the registry door — one at a time. There
/// is no branch here that reaches [`GraphPlan::with_processor`], because there is nothing on the
/// wire for one to be reached with.
#[allow(clippy::type_complexity)]
fn plan_for(
    direction: AudioDirection,
    processors: &[DspStage],
) -> Result<(GraphPlan, Vec<BuiltIn>), DspRefusal> {
    if processors.is_empty() {
        // An empty chain is not a removal. `dsp_remove` is that, and it has a teardown and a
        // barrier; reading an empty list as one would tear a call's audio down on a typo.
        return Err(DspRefusal::Rejected);
    }
    let mut plan = GraphPlan::new(media_direction(direction), GraphBounds::new());
    let mut stages = Vec::with_capacity(processors.len());
    for stage in processors {
        let built_in =
            BuiltIn::from_id(&stage.id, stage.shape).ok_or(DspRefusal::UnknownProcessor)?;
        let set = resolve(built_in.parameters(), &stage.parameters)?;
        plan = plan
            .with_built_in(built_in, &set)
            .map_err(refusal_for)
            .map_err(|reason| match reason {
                // A shape the effect does not admit arrives as a capability refusal; from the
                // application's side that is a value outside a declared range like any other.
                DspRefusal::Rejected => DspRefusal::OutOfRange,
                other => other,
            })?;
        stages.push(built_in);
    }
    Ok((plan, stages))
}

/// Resolve a wire parameter set against a processor's published schema.
///
/// The identifier a [`Parameter`] carries is the **processor's own** `&'static str`, so this is
/// where a string a peer chose becomes one — by matching it, or by being refused. An identifier
/// outside the schema refuses the whole set, which is `docs/specs/custom-call-dsp.md` §3.6's rule
/// applied one step earlier than the processor would apply it.
fn resolve(
    schema: &'static [ParameterSpec],
    parameters: &[DspParameter],
) -> Result<Vec<Parameter>, DspRefusal> {
    parameters
        .iter()
        .map(|parameter| {
            let spec = schema
                .iter()
                .find(|spec| spec.id() == parameter.id)
                .ok_or(DspRefusal::UnknownParameter)?;
            let value = match parameter.value {
                DspValue::Flag(flag) => ParameterValue::Flag(flag),
                DspValue::Integer(found) => ParameterValue::Integer(found),
                DspValue::Ratio(found) => ParameterValue::Ratio(found),
            };
            Ok(Parameter::new(spec.id(), value))
        })
        .collect()
}

/// Which of §5.3's words a graph refusal is.
///
/// Narrower than the graph's own vocabulary on purpose: `Rejected` is what a host diagnosis
/// collapses to, because a word per internal refusal would put the host's reasoning into a
/// vocabulary that otherwise says only what the application got wrong.
fn refusal_for(error: GraphError) -> DspRefusal {
    match error {
        GraphError::StaleGeneration { .. } | GraphError::Detached => DspRefusal::StaleGeneration,
        GraphError::UnknownProcessor { .. } => DspRefusal::UnknownProcessor,
        GraphError::NotConfigurable { .. } => DspRefusal::NotConfigurable,
        GraphError::TooManyProcessors { .. }
        | GraphError::FrameExceedsBound { .. }
        | GraphError::ScratchExceedsBound { .. }
        | GraphError::TailExceedsBound { .. } => DspRefusal::TooManyProcessors,
        GraphError::Parameter { source, .. } => match source {
            sipx_media::dsp::ParameterError::OutOfRange { .. } => DspRefusal::OutOfRange,
            _ => DspRefusal::UnknownParameter,
        },
        _ => DspRefusal::Rejected,
    }
}

fn bypass_cause(cause: BypassCause) -> DspBypassCause {
    match cause {
        BypassCause::Requested => DspBypassCause::Requested,
        BypassCause::DeadlineMissed => DspBypassCause::DeadlineMissed,
        BypassCause::MalformedResult => DspBypassCause::MalformedResult,
        BypassCause::WorkerLost => DspBypassCause::WorkerLost,
        // `Refused`, and the safe reading of a cause a later revision adds: a stage stopped
        // contributing and the audio it owed is not in the call, which is what `refused` says.
        _ => DspBypassCause::Refused,
    }
}

fn teardown_cause(cause: TeardownCause) -> DspTeardownCause {
    match cause {
        TeardownCause::Requested => DspTeardownCause::Requested,
        TeardownCause::Detached => DspTeardownCause::Detached,
        TeardownCause::FailedClosed { .. } => DspTeardownCause::FailedClosed,
        // `SessionStopped`, and the safe reading of a cause a later revision adds: the call's media
        // ended, which is the one of these that needs no application action.
        _ => DspTeardownCause::SessionStopped,
    }
}

fn media_direction(direction: AudioDirection) -> sipx_media::AudioDirection {
    match direction {
        AudioDirection::Inbound => sipx_media::AudioDirection::Inbound,
        AudioDirection::Outbound => sipx_media::AudioDirection::Outbound,
    }
}

fn refused(instruction_id: String, direction: AudioDirection, reason: DspRefusal) -> EventKind {
    EventKind::DspRefused {
        instruction_id,
        direction,
        reason,
    }
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
    use sipx_media::{Codec, Config, MediaPort};

    async fn session() -> MediaSession {
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let mut config = Config::new(peer.local_addr().unwrap(), Codec::Pcmu);
        config.rtcp_interval = None;
        port.start(config).unwrap()
    }

    fn gain(ratio: i32) -> Vec<DspStage> {
        vec![DspStage::new("sipx.gain").with_parameter("gain", DspValue::Ratio(ratio))]
    }

    /// Acceptance row 4: an unknown processor is refused and the active chain is not changed.
    #[tokio::test]
    async fn an_unknown_identifier_is_refused_and_the_active_chain_is_untouched() {
        let session = session().await;
        let mut graphs = CallGraphs::default();
        let activated = graphs.set(
            &session,
            "a".to_owned(),
            AudioDirection::Outbound,
            &gain(2_000),
        );
        let EventKind::DspActivated { generation, .. } = activated else {
            panic!("a registered chain activates: {activated:?}");
        };

        let refusal = graphs.set(
            &session,
            "b".to_owned(),
            AudioDirection::Outbound,
            &[DspStage::new("sipx.reverb")],
        );
        assert_eq!(
            refusal,
            EventKind::DspRefused {
                instruction_id: "b".to_owned(),
                direction: AudioDirection::Outbound,
                reason: DspRefusal::UnknownProcessor,
            }
        );
        // Still the chain that was there, at the generation it was there at.
        assert_eq!(
            graphs
                .peek(AudioDirection::Outbound)
                .unwrap()
                .graph
                .generation(),
            generation
        );
    }

    /// Acceptance rows 2 and 4: one terminal outcome per instruction, and every refusal changes
    /// nothing.
    #[tokio::test]
    async fn a_parameter_update_has_exactly_one_terminal_outcome() {
        let session = session().await;
        let mut graphs = CallGraphs::default();
        let EventKind::DspActivated { generation, .. } = graphs.set(
            &session,
            "a".to_owned(),
            AudioDirection::Outbound,
            &gain(1_000),
        ) else {
            panic!("a registered chain activates");
        };
        let applied = graphs.configure(
            "p".to_owned(),
            AudioDirection::Outbound,
            generation,
            0,
            &[DspParameter::new("gain", DspValue::Ratio(2_000))],
        );
        assert!(
            matches!(
                applied,
                EventKind::DspConfigured {
                    ref instruction_id, ..
                } if instruction_id == "p"
            ),
            "{applied:?}"
        );

        for (id, direction, generation, index, parameter, reason) in [
            (
                "q",
                AudioDirection::Outbound,
                generation + 1,
                0,
                DspParameter::new("gain", DspValue::Ratio(1_000)),
                DspRefusal::StaleGeneration,
            ),
            (
                "r",
                AudioDirection::Outbound,
                generation,
                4,
                DspParameter::new("gain", DspValue::Ratio(1_000)),
                DspRefusal::UnknownProcessor,
            ),
            (
                "s",
                AudioDirection::Outbound,
                generation,
                0,
                DspParameter::new("reverb", DspValue::Ratio(1_000)),
                DspRefusal::UnknownParameter,
            ),
            (
                "t",
                AudioDirection::Outbound,
                generation,
                0,
                DspParameter::new("gain", DspValue::Ratio(99_000)),
                DspRefusal::OutOfRange,
            ),
            (
                "u",
                AudioDirection::Inbound,
                generation,
                0,
                DspParameter::new("gain", DspValue::Ratio(1_000)),
                DspRefusal::NoGraph,
            ),
        ] {
            assert_eq!(
                graphs.configure(id.to_owned(), direction, generation, index, &[parameter]),
                EventKind::DspRefused {
                    instruction_id: id.to_owned(),
                    direction,
                    reason,
                },
                "{id}"
            );
        }
        assert_eq!(
            graphs
                .peek(AudioDirection::Outbound)
                .unwrap()
                .graph
                .generation(),
            generation,
            "no refusal replaced the chain"
        );
    }

    /// The epic's rule at the application door: containment is reported, never supplied.
    ///
    /// Every identifier this vocabulary can carry resolves to a workspace processor, so a chain an
    /// application assembles is contained *because of what its stages are*. There is no member of
    /// [`DspStage`] through which it could have said so.
    #[tokio::test]
    async fn an_application_assembled_chain_reports_a_containment_it_could_not_state() {
        let session = session().await;
        let mut graphs = CallGraphs::default();
        let activated = graphs.set(
            &session,
            "a".to_owned(),
            AudioDirection::Outbound,
            &gain(1_000),
        );
        assert!(
            matches!(
                activated,
                EventKind::DspActivated {
                    contains_overrun: true,
                    previous: None,
                    processors: 1,
                    ..
                }
            ),
            "{activated:?}"
        );
        // And the same fact for every registered identifier, which is the whole set an application
        // can name: there is no name it can write that produces an uncontained chain.
        for entry in BuiltIn::registered() {
            assert_eq!(
                BuiltIn::from_id(entry.id(), 1).map(|_| ()),
                Some(()),
                "{}",
                entry.id()
            );
        }
    }

    /// A teardown nobody asked for reaches the application, and carries no instruction to
    /// correlate against.
    #[tokio::test]
    async fn an_unsolicited_teardown_carries_no_instruction_id() {
        let events = unsolicited(
            AudioDirection::Inbound,
            &[
                GraphTransition::Activated {
                    generation: 1,
                    at_position: 0,
                    processors: 1,
                },
                GraphTransition::Configured {
                    generation: 1,
                    at_position: 160,
                    processor: "sipx.gain",
                },
                GraphTransition::Bypassed {
                    generation: 1,
                    at_position: 320,
                    processor: "sipx.gain",
                    cause: BypassCause::DeadlineMissed,
                },
                GraphTransition::TornDown {
                    generation: 1,
                    at_position: 480,
                    cause: TeardownCause::FailedClosed {
                        processor: "sipx.gain",
                    },
                },
            ],
        );
        // The activation and the parameter change were an instruction's outcome and were reported
        // with its id; composing them again here would be two events for one thing asked once.
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(
            events[0],
            EventKind::DspBypassed {
                direction: AudioDirection::Inbound,
                generation: 1,
                at_position: 320,
                processor: "sipx.gain".to_owned(),
                cause: DspBypassCause::DeadlineMissed,
            }
        );
        assert_eq!(
            events[1],
            EventKind::DspRemoved {
                instruction_id: None,
                direction: AudioDirection::Inbound,
                generation: 1,
                at_position: 480,
                processor: Some("sipx.gain".to_owned()),
                cause: DspTeardownCause::FailedClosed,
            }
        );
    }
}
