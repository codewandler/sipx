//! A deterministic scenario driver for the contract interpreter.
//!
//! A scenario **is data**: call events, binding replies, timer firings and expectations. The
//! runner advances [`Virtual`] time to the next scripted event or interpreter timer and never
//! sleeps. Instruction semantics are not implemented here: every event and response is fed to
//! [`sipx_app_protocol::Interpreter`], and this module only performs its outputs.

use std::collections::VecDeque;

use sipx_app_protocol::{
    CallSnapshot, Callback, Direction, Input, Interpreter, Output, Response, Timer, Timestamp,
};
use sipx_transport::timers::TimerQueue;

use super::binding::{Binding, Outcome, Reply, ScriptedApp};
use super::contract::{Effect, EndCause, Event, EventKind, Source};
use super::policy::{Failure, FailurePolicy};
use super::time::Virtual;

/// The interpreter's declared bound for events waiting behind one callback.
pub const EVENT_QUEUE: usize = sipx_app_protocol::MAX_QUEUED_EVENTS;

/// One thing the scenario makes happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A call event occurs.
    Event {
        /// When.
        at: Virtual,
        /// What.
        kind: EventKind,
    },
    /// A binding redelivers an event with the same sequence number (§5.1).
    Redeliver {
        /// When.
        at: Virtual,
        /// Which delivery.
        seq: u64,
    },
}

impl Step {
    /// When this step happens.
    #[must_use]
    pub fn at(&self) -> Virtual {
        match self {
            Self::Event { at, .. } | Self::Redeliver { at, .. } => *at,
        }
    }

    /// An event at this many milliseconds in.
    #[must_use]
    pub fn event(millis: u64, kind: EventKind) -> Self {
        Self::Event {
            at: Virtual::at_millis(millis),
            kind,
        }
    }

    /// A redelivery at this many milliseconds in.
    #[must_use]
    pub fn redeliver(millis: u64, seq: u64) -> Self {
        Self::Redeliver {
            at: Virtual::at_millis(millis),
            seq,
        }
    }
}

/// How a call finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conclusion {
    /// Still up when the scenario ended.
    Live,
    /// Over, for this reason.
    Ended(EndCause),
}

/// A scenario: everything that happens, and nothing that depends on the machine running it.
#[derive(Debug)]
pub struct Scenario {
    /// A vector id or descriptive name.
    pub name: String,
    /// The app's declared failure semantics.
    pub policy: FailurePolicy,
    /// What the app answers, in order.
    pub script: Vec<Reply>,
    /// What it answers after the script is exhausted.
    pub then: Option<Reply>,
    /// Call events and redeliveries, in time order.
    pub steps: Vec<Step>,
    /// The virtual-time bound for this run.
    pub until: Virtual,
}

impl Scenario {
    /// A scenario with the contract defaults and a one-minute virtual bound.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            policy: FailurePolicy::declared(),
            script: Vec::new(),
            then: None,
            steps: Vec::new(),
            until: Virtual::at_millis(60_000),
        }
    }

    /// With this failure declaration.
    #[must_use]
    pub fn policy(mut self, policy: FailurePolicy) -> Self {
        self.policy = policy;
        self
    }

    /// With this binding script.
    #[must_use]
    pub fn script(mut self, script: Vec<Reply>) -> Self {
        self.script = script;
        self
    }

    /// Answering this after the script runs out.
    #[must_use]
    pub fn then(mut self, reply: Reply) -> Self {
        self.then = Some(reply);
        self
    }

    /// With these call steps.
    #[must_use]
    pub fn steps(mut self, steps: Vec<Step>) -> Self {
        self.steps = steps;
        self
    }

    /// Running until this many milliseconds in.
    #[must_use]
    pub fn until(mut self, millis: u64) -> Self {
        self.until = Virtual::at_millis(millis);
        self
    }

    /// Run against the built-in scripted binding.
    #[must_use]
    pub fn run(self) -> Run {
        let mut app = ScriptedApp::new(self.script.clone());
        if let Some(reply) = self.then.clone() {
            app = app.then(reply);
        }
        self.run_against(&mut app)
    }

    /// Run against another deterministic binding.
    #[must_use]
    pub fn run_against<B: Binding>(&self, app: &mut B) -> Run {
        Actor::new(self).drive(app)
    }
}

/// What a scenario did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The scenario name.
    pub name: String,
    /// Call operations performed, in order.
    pub effects: Vec<Effect>,
    /// Every event delivered to the binding, including redeliveries.
    pub delivered: Vec<Event>,
    /// Scripted call events the interpreter did not deliver because its queue overflowed.
    pub dropped: Vec<EventKind>,
    /// Transport/timer failures presented to the interpreter.
    pub failures: Vec<Failure>,
    /// Legs in the interpreter's final snapshot.
    pub legs: Vec<String>,
    /// How the call finished.
    pub conclusion: Conclusion,
}

impl Run {
    /// The sequence number of every delivery.
    #[must_use]
    pub fn delivered_seqs(&self) -> Vec<u64> {
        self.delivered.iter().map(|event| event.seq).collect()
    }

    /// The first delivered event of this type.
    #[must_use]
    pub fn first(&self, type_name: &str) -> Option<&EventKind> {
        self.delivered
            .iter()
            .map(|event| &event.kind)
            .find(|kind| kind.type_name() == type_name)
    }

    /// Whether the app saw the terminal event.
    #[must_use]
    pub fn app_saw_ended(&self) -> bool {
        self.delivered.iter().any(Event::is_ended)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum InterpreterTimer {
    Callback,
    Pause,
    GatherOverall,
    GatherDigit,
}

impl InterpreterTimer {
    fn from_protocol(timer: Timer) -> Self {
        match timer {
            Timer::Callback => Self::Callback,
            Timer::Pause => Self::Pause,
            Timer::GatherOverall => Self::GatherOverall,
            Timer::GatherDigit => Self::GatherDigit,
        }
    }

    fn protocol(self) -> Timer {
        match self {
            Self::Callback => Timer::Callback,
            Self::Pause => Timer::Pause,
            Self::GatherOverall => Timer::GatherOverall,
            Self::GatherDigit => Timer::GatherDigit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TimerKey {
    Interpreter(InterpreterTimer),
    Reply(u64),
}

struct PendingReply {
    seq: u64,
    callback: Callback,
    outcome: Outcome,
}

/// A virtual binding and call-framework driver around the sole interpreter.
struct Actor<'a> {
    scenario: &'a Scenario,
    now: Virtual,
    interpreter: Interpreter,
    timers: TimerQueue<TimerKey, Virtual>,
    pending: Vec<PendingReply>,
    history: Vec<Event>,
    undelivered_inputs: Vec<EventKind>,
    active_playback: Option<String>,
    gather_started: Option<String>,
    run: Run,
}

impl<'a> Actor<'a> {
    fn new(scenario: &'a Scenario) -> Self {
        Self {
            scenario,
            now: Virtual::epoch(),
            interpreter: Interpreter::new(
                CallSnapshot::new("harness-call", Direction::Inbound),
                scenario.policy.protocol(),
            ),
            timers: TimerQueue::new(),
            pending: Vec::new(),
            history: Vec::new(),
            undelivered_inputs: Vec::new(),
            active_playback: None,
            gather_started: None,
            run: Run {
                name: scenario.name.clone(),
                effects: Vec::new(),
                delivered: Vec::new(),
                dropped: Vec::new(),
                failures: Vec::new(),
                legs: Vec::new(),
                conclusion: Conclusion::Live,
            },
        }
    }

    fn drive<B: Binding>(mut self, app: &mut B) -> Run {
        let mut steps: VecDeque<Step> = self.scenario.steps.iter().cloned().collect();
        loop {
            let next_step = steps.front().map(Step::at);
            let next_timer = self.timers.next_deadline();
            let now = match (next_step, next_timer) {
                (Some(step), Some(timer)) if step <= timer => step,
                (Some(step), None) => step,
                (_, Some(timer)) => timer,
                (None, None) => break,
            };
            if now > self.scenario.until {
                break;
            }
            self.now = now;
            if next_step == Some(now) {
                if let Some(step) = steps.pop_front() {
                    self.step(step, app);
                }
            } else {
                for key in self.timers.take_due(now) {
                    self.fire(key, app);
                }
            }
        }

        self.run.dropped = self.undelivered_inputs;
        self.run.legs = self
            .interpreter
            .snapshot()
            .legs
            .iter()
            .map(|leg| leg.leg.clone())
            .collect();
        self.run
    }

    fn step<B: Binding>(&mut self, step: Step, app: &mut B) {
        match step {
            Step::Event { kind, .. } => self.event(kind, app),
            Step::Redeliver { seq, .. } => {
                if let Some(event) = self.history.iter().find(|event| event.seq == seq).cloned() {
                    let _discarded = app.respond(&event);
                    self.run.delivered.push(event);
                }
            }
        }
    }

    fn event<B: Binding>(&mut self, kind: EventKind, app: &mut B) {
        if let EventKind::PlaybackFinished { instruction_id, .. } = &kind
            && self.active_playback.as_deref() == Some(instruction_id)
        {
            self.active_playback = None;
        }
        if let EventKind::Ended { cause } = &kind {
            self.run.conclusion = Conclusion::Ended(*cause);
        }
        self.undelivered_inputs.push(kind.clone());
        self.input(Input::Event(kind), app);
    }

    fn input<B: Binding>(&mut self, input: Input, app: &mut B) {
        let at = Timestamp::from_unix_millis(self.now_millis());
        let outputs = self.interpreter.handle(at, input);
        self.outputs(outputs, app);
    }

    fn outputs<B: Binding>(&mut self, outputs: Vec<Output>, app: &mut B) {
        let mut terminal = None;
        for output in outputs {
            match output {
                Output::Effect(effect) => {
                    if let Some(cause) = self.effect(effect) {
                        terminal = Some(cause);
                    }
                }
                Output::Deliver { envelope, callback } => {
                    self.deliver(envelope.seq, envelope.event.clone(), callback, app);
                }
                Output::SetTimer { timer, after_ms } => {
                    if timer == Timer::GatherOverall
                        && let Some(id) = self.interpreter.running().map(str::to_owned)
                        && self.gather_started.as_deref() != Some(id.as_str())
                    {
                        self.run
                            .effects
                            .push(Effect::StartGather { id: id.clone() });
                        self.gather_started = Some(id);
                    }
                    self.timers.set(
                        TimerKey::Interpreter(InterpreterTimer::from_protocol(timer)),
                        self.now,
                        std::time::Duration::from_millis(u64::from(after_ms)),
                    );
                }
                Output::ClearTimer(timer) => {
                    self.timers
                        .clear(&TimerKey::Interpreter(InterpreterTimer::from_protocol(
                            timer,
                        )));
                    if matches!(timer, Timer::GatherOverall | Timer::GatherDigit) {
                        self.gather_started = None;
                    }
                }
            }
        }
        if let Some(cause) = terminal
            && !matches!(
                self.interpreter.snapshot().state,
                sipx_app_protocol::CallState::Ended
            )
        {
            self.event(EventKind::Ended { cause }, app);
        }
    }

    fn effect(&mut self, effect: sipx_app_protocol::Effect) -> Option<EndCause> {
        match effect {
            sipx_app_protocol::Effect::Answer => self.run.effects.push(Effect::Answer),
            sipx_app_protocol::Effect::Play {
                instruction_id,
                source,
                ..
            } => {
                self.active_playback = Some(instruction_id.clone());
                self.run.effects.push(Effect::StartPlay {
                    id: instruction_id,
                    source: match source {
                        Source::File(path) => path,
                        Source::Inline(_) => "<inline>".to_owned(),
                    },
                });
            }
            sipx_app_protocol::Effect::StopPlayback => {
                if let Some(id) = self.active_playback.take() {
                    self.run.effects.push(Effect::StopPlay { id });
                }
            }
            sipx_app_protocol::Effect::Dial {
                instruction_id,
                target,
                ..
            } => self.run.effects.push(Effect::Dial {
                id: instruction_id,
                target,
            }),
            sipx_app_protocol::Effect::HangUp { cause } => {
                self.run.effects.push(Effect::Hangup { cause });
                self.run.conclusion = Conclusion::Ended(cause);
                return Some(cause);
            }
            sipx_app_protocol::Effect::Reject { status, .. } => {
                self.run.effects.push(Effect::Reject { status });
                let cause = EndCause::Rejected { status };
                self.run.conclusion = Conclusion::Ended(cause);
                return Some(cause);
            }
            other => self.run.effects.push(Effect::Other {
                verb: effect_name(&other).to_owned(),
            }),
        }
        None
    }

    fn deliver<B: Binding>(&mut self, seq: u64, kind: EventKind, callback: Callback, app: &mut B) {
        if let Some(index) = self
            .undelivered_inputs
            .iter()
            .position(|candidate| candidate == &kind)
        {
            self.undelivered_inputs.remove(index);
        }
        let event = Event { seq, kind };
        let reply = app.respond(&event);
        self.history.push(event.clone());
        self.run.delivered.push(event);
        if !matches!(reply.outcome, Outcome::Silent) {
            self.pending.push(PendingReply {
                seq,
                callback,
                outcome: reply.outcome,
            });
            self.timers.set(TimerKey::Reply(seq), self.now, reply.after);
        }
    }

    fn fire<B: Binding>(&mut self, key: TimerKey, app: &mut B) {
        match key {
            TimerKey::Interpreter(timer) => {
                if timer == InterpreterTimer::Callback && self.run.conclusion == Conclusion::Live {
                    self.run.failures.push(Failure::Timeout);
                    self.pending.clear();
                }
                self.input(Input::TimerFired(timer.protocol()), app);
            }
            TimerKey::Reply(seq) => {
                let Some(index) = self.pending.iter().position(|reply| reply.seq == seq) else {
                    return;
                };
                let reply = self.pending.remove(index);
                self.timers
                    .clear(&TimerKey::Interpreter(InterpreterTimer::Callback));
                let response = match reply.outcome {
                    Outcome::Document(document) => Response::Document(document),
                    Outcome::Body(body) => Response::Body(body),
                    Outcome::ClientError { .. } => {
                        self.record_failure(Failure::ClientError);
                        Response::Failed(sipx_app_protocol::Failure::ClientError)
                    }
                    Outcome::ServerError { .. } => {
                        self.record_failure(Failure::ServerError);
                        Response::Failed(sipx_app_protocol::Failure::ServerError)
                    }
                    Outcome::Unreachable => {
                        self.record_failure(Failure::Unreachable);
                        Response::Failed(sipx_app_protocol::Failure::Unreachable)
                    }
                    Outcome::Silent => return,
                };
                self.input(
                    Input::Response {
                        callback: reply.callback,
                        response,
                    },
                    app,
                );
            }
        }
    }

    fn record_failure(&mut self, failure: Failure) {
        if self.run.conclusion == Conclusion::Live {
            self.run.failures.push(failure);
        }
    }

    fn now_millis(&self) -> i64 {
        i64::try_from(self.now.millis()).unwrap_or(i64::MAX)
    }
}

fn effect_name(effect: &sipx_app_protocol::Effect) -> &'static str {
    match effect {
        sipx_app_protocol::Effect::Ring { .. } => "ring",
        sipx_app_protocol::Effect::Record { .. } => "record",
        sipx_app_protocol::Effect::SendDigits { .. } => "send_dtmf",
        sipx_app_protocol::Effect::Bridge { .. } => "bridge",
        sipx_app_protocol::Effect::Unbridge => "unbridge",
        sipx_app_protocol::Effect::Hold => "hold",
        sipx_app_protocol::Effect::Resume => "resume",
        sipx_app_protocol::Effect::Mute => "mute",
        sipx_app_protocol::Effect::Unmute => "unmute",
        sipx_app_protocol::Effect::Transfer { .. } => "transfer",
        sipx_app_protocol::Effect::AcceptTransfer => "accept_transfer",
        sipx_app_protocol::Effect::RefuseTransfer { .. } => "refuse_transfer",
        _ => "contract-effect",
    }
}

/// What a scenario should have produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expectation {
    /// Exact effects, when specified.
    pub effects: Option<Vec<Effect>>,
    /// Effects that must be absent.
    pub without: Vec<Effect>,
    /// Expected conclusion.
    pub conclusion: Option<Conclusion>,
    /// Exact delivered sequence numbers.
    pub delivered_seqs: Option<Vec<u64>>,
    /// Exact binding failures.
    pub failures: Option<Vec<Failure>>,
    /// Final leg names.
    pub legs: Option<Vec<String>>,
    /// Whether the terminal event reached the binding.
    pub app_saw_ended: Option<bool>,
    /// An exact event that must have been delivered.
    pub delivered_event: Option<EventKind>,
    /// Whether any event was dropped.
    pub dropped_any: Option<bool>,
}

impl Expectation {
    /// Expecting nothing in particular.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Exactly these effects.
    #[must_use]
    pub fn effects(mut self, effects: Vec<Effect>) -> Self {
        self.effects = Some(effects);
        self
    }

    /// And none of these effects.
    #[must_use]
    pub fn without(mut self, effects: Vec<Effect>) -> Self {
        self.without = effects;
        self
    }

    /// Finishing this way.
    #[must_use]
    pub fn conclusion(mut self, conclusion: Conclusion) -> Self {
        self.conclusion = Some(conclusion);
        self
    }

    /// Delivering these sequence numbers.
    #[must_use]
    pub fn delivered_seqs(mut self, seqs: Vec<u64>) -> Self {
        self.delivered_seqs = Some(seqs);
        self
    }

    /// Presenting these failures to the interpreter.
    #[must_use]
    pub fn failures(mut self, failures: Vec<Failure>) -> Self {
        self.failures = Some(failures);
        self
    }

    /// Leaving these legs in the snapshot.
    #[must_use]
    pub fn legs(mut self, legs: Vec<String>) -> Self {
        self.legs = Some(legs);
        self
    }

    /// With the terminal event seen or unseen.
    #[must_use]
    pub fn app_saw_ended(mut self, saw: bool) -> Self {
        self.app_saw_ended = Some(saw);
        self
    }

    /// Having delivered this event.
    #[must_use]
    pub fn delivered_event(mut self, kind: EventKind) -> Self {
        self.delivered_event = Some(kind);
        self
    }

    /// Having dropped something, or nothing.
    #[must_use]
    pub fn dropped_any(mut self, dropped: bool) -> Self {
        self.dropped_any = Some(dropped);
        self
    }

    /// Check a run, returning the first useful mismatch.
    pub fn check(&self, run: &Run) -> Result<(), String> {
        let at = &run.name;
        if let Some(expected) = &self.effects
            && &run.effects != expected
        {
            return Err(format!(
                "{at}: effects\n  expected {expected:?}\n  got      {:?}",
                run.effects
            ));
        }
        for unwanted in &self.without {
            if run.effects.contains(unwanted) {
                return Err(format!("{at}: {unwanted:?} must not have happened"));
            }
        }
        if let Some(expected) = &self.conclusion
            && &run.conclusion != expected
        {
            return Err(format!(
                "{at}: conclusion expected {expected:?}, got {:?}",
                run.conclusion
            ));
        }
        if let Some(expected) = &self.delivered_seqs
            && &run.delivered_seqs() != expected
        {
            return Err(format!(
                "{at}: delivered seqs expected {expected:?}, got {:?}",
                run.delivered_seqs()
            ));
        }
        if let Some(expected) = &self.failures
            && &run.failures != expected
        {
            return Err(format!(
                "{at}: failure knobs expected {expected:?}, got {:?}",
                run.failures
            ));
        }
        if let Some(expected) = &self.legs
            && &run.legs != expected
        {
            return Err(format!(
                "{at}: legs expected {expected:?}, got {:?}",
                run.legs
            ));
        }
        if let Some(expected) = self.app_saw_ended
            && run.app_saw_ended() != expected
        {
            return Err(format!(
                "{at}: call.ended reaching the app expected {expected}, got {}",
                run.app_saw_ended()
            ));
        }
        if let Some(expected) = &self.delivered_event
            && !run.delivered.iter().any(|event| &event.kind == expected)
        {
            return Err(format!(
                "{at}: expected {expected:?} among delivered, got {:?}",
                run.delivered
                    .iter()
                    .map(|event| &event.kind)
                    .collect::<Vec<_>>()
            ));
        }
        if let Some(expected) = self.dropped_any
            && run.dropped.is_empty() == expected
        {
            return Err(format!(
                "{at}: dropped-anything expected {expected}, dropped {:?}",
                run.dropped
            ));
        }
        if run
            .dropped
            .iter()
            .any(|event| matches!(event, EventKind::Ended { .. }))
        {
            return Err(format!("{at}: call.ended was dropped"));
        }
        Ok(())
    }
}

/// A scenario paired with its expected result.
#[derive(Debug)]
pub struct Vector {
    /// What happens.
    pub scenario: Scenario,
    /// What must result.
    pub expect: Expectation,
}

impl Vector {
    /// Pair data and expectation.
    #[must_use]
    pub fn new(scenario: Scenario, expect: Expectation) -> Self {
        Self { scenario, expect }
    }

    /// Check against the built-in scripted binding.
    pub fn check(self) -> Result<(), String> {
        let run = self.scenario.run();
        self.expect.check(&run)
    }

    /// Check against another deterministic binding.
    pub fn check_against<B: Binding>(&self, app: &mut B) -> Result<(), String> {
        let run = self.scenario.run_against(app);
        self.expect.check(&run)
    }
}
