//! A scenario, and the decision logic it drives.
//!
//! A scenario **is data**: when call events occur, what the app answers and after how long, and
//! what the host should have done. Running one is a discrete-event loop over
//! [`Virtual`] time — the earliest of "the next scripted event" and "the next
//! timer" wins, the clock jumps to it, and nothing sleeps. A scenario asserting on a two-second
//! callback timeout costs no wall-clock time at all.
//!
//! What is under test here is the **actor's** logic, not the interpreter's: delivery and the
//! alternation rule (§6.3), `seq` and redelivery (§5.1), the bounded event queue (AC-9), the
//! instruction queue's blocking discipline (§6.1), and the declared failure semantics (§9.2). When
//! `C-5` lands, its interpreter replaces the instruction-execution half of [`Run`]; the scenarios
//! and their expectations do not change, which is the point of writing them first.
//!
//! ## Two readings the spec leaves implicit
//!
//! Recorded here rather than buried, because a vector's outcome depends on each and `C-5` must
//! either agree or correct them:
//!
//! 1. **An empty document does not replace the program.** §6.3 says a response "is the *entire* new
//!    program", and also that "an empty `instructions` array is valid and means 'keep going'".
//!    Those only reconcile if empty is the one document that changes nothing — otherwise "keep
//!    going" would mean "discard everything queued", which is the opposite.
//! 2. **A digit consumed by a running `gather` is not also delivered as `call.dtmf`.** The gather
//!    is the abstraction over collecting digits; delivering both would have the app see every
//!    keypress it explicitly asked the host to collect for it. AC-3's barge-in is a digit arriving
//!    during a `play`, where no gather is running, and that case is unaffected.

use std::collections::VecDeque;

use sipx_transport::timers::TimerQueue;

use super::binding::{Binding, Outcome, Reply, ScriptedApp};
use super::contract::{
    DialOutcome, Document, Effect, EndCause, Event, EventKind, GatherReason, Instruction, Verb,
};
use super::policy::{Failure, FailurePolicy, OnFailure};
use super::time::Virtual;

/// How many events may queue behind an outstanding callback before the host starts dropping.
///
/// One slot of this is reserved for `call.ended` (AC-9), so ordinary events compete for
/// `EVENT_QUEUE - 1`. The same reasoning as `sipx-call`'s own stream: every other event carries a
/// snapshot and a consumer that missed one resynchronises from the next, but a consumer that never
/// learns the call ended waits forever.
pub const EVENT_QUEUE: usize = 8;

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
    /// An event already delivered is delivered again with the same `seq` (§5.1) — a document-mode
    /// retry or a session reconnect replay. The app may answer differently; the host may not care.
    Redeliver {
        /// When.
        at: Virtual,
        /// Which event.
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
    /// What this scenario is called — a vector id like `AC-3`, or a knob name.
    pub name: String,
    /// The app's declared failure semantics (§9.2).
    pub policy: FailurePolicy,
    /// What the app answers, in order, one per delivered event.
    pub script: Vec<Reply>,
    /// What it answers once the script runs out. `None` means "keep going".
    pub then: Option<Reply>,
    /// What happens to the call, in time order.
    pub steps: Vec<Step>,
    /// How long to run for. The loop stops here even if timers remain, so a scenario cannot hang.
    pub until: Virtual,
}

impl Scenario {
    /// A scenario with the §9.2 defaults, running for one minute of virtual time.
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

    /// With this app script.
    #[must_use]
    pub fn script(mut self, script: Vec<Reply>) -> Self {
        self.script = script;
        self
    }

    /// Answering this once the script runs out.
    #[must_use]
    pub fn then(mut self, reply: Reply) -> Self {
        self.then = Some(reply);
        self
    }

    /// With these steps.
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

    /// Run it.
    #[must_use]
    pub fn run(self) -> Run {
        let mut app = ScriptedApp::new(self.script.clone());
        if let Some(reply) = self.then.clone() {
            app = app.then(reply);
        }
        self.run_against(&mut app)
    }

    /// Run it against a binding of the caller's own.
    ///
    /// This is what acceptance point 3 means by *shared*: `A-2` and `A-4` run the same scenarios
    /// through their own binding adapter rather than restating the failure semantics per binding.
    #[must_use]
    pub fn run_against<B: Binding>(&self, app: &mut B) -> Run {
        Actor::new(self).drive(app)
    }
}

/// What a scenario did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// The name of the scenario that produced it.
    pub name: String,
    /// What the host executed, in order.
    pub effects: Vec<Effect>,
    /// Every event the app was actually given, in delivery order.
    pub delivered: Vec<Event>,
    /// Events dropped by the queue's overflow policy. Never contains `call.ended` (AC-9).
    pub dropped: Vec<EventKind>,
    /// Which §9.2 knob was consulted, in order. Empty when the app never failed.
    pub failures: Vec<Failure>,
    /// The legs the snapshot would list (§5.2).
    pub legs: Vec<String>,
    /// How it finished.
    pub conclusion: Conclusion,
}

impl Run {
    /// The `seq` of each delivered event, which is what §5.1's ordering claims are about.
    #[must_use]
    pub fn delivered_seqs(&self) -> Vec<u64> {
        self.delivered.iter().map(|event| event.seq).collect()
    }

    /// The first delivered event of this type, if any.
    #[must_use]
    pub fn first(&self, type_name: &str) -> Option<&EventKind> {
        self.delivered
            .iter()
            .map(|event| &event.kind)
            .find(|kind| kind.type_name() == type_name)
    }

    /// Whether `call.ended` reached the app. AC-9's whole question.
    #[must_use]
    pub fn app_saw_ended(&self) -> bool {
        self.delivered.iter().any(|event| event.kind.is_ended())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum TimerKey {
    /// The app's answer to this event arrives.
    Reply(u64),
    /// The callback for this event has taken too long (§9.2 `timeout_ms`).
    Callback(u64),
    /// A `gather` ran out of time.
    Gather(String),
    /// A `dial` ran out of time.
    Dial(String),
}

/// One call's decision logic — the thing every behaviour claim about the host rests on.
struct Actor<'a> {
    scenario: &'a Scenario,
    now: Virtual,
    timers: TimerQueue<TimerKey, Virtual>,
    /// Events not yet delivered, oldest first.
    queue: VecDeque<Event>,
    /// The `seq` of the callback currently outstanding, if any. §6.3: at most one per call.
    outstanding: Option<u64>,
    /// Every event created, by `seq`, so a redelivery can repeat one.
    history: Vec<Event>,
    /// The highest `seq` whose response has already been applied. A response for one at or below
    /// this is a redelivery's, and is ignored (AC-4).
    applied: Option<u64>,
    next_seq: u64,
    /// The program: instructions not yet started.
    program: VecDeque<Instruction>,
    /// The instruction blocking the queue, if any (§6.1).
    running: Option<Instruction>,
    /// Digits a running `gather` has collected.
    collected: String,
    /// Answers the app has given but whose arrival time has not come yet. Held rather than applied
    /// on the spot, because "the app answered after 300 ms" has to be able to lose a race with a
    /// 200 ms callback timeout — which is the whole of the slow-app case.
    answers: Vec<(u64, Outcome)>,
    run: Run,
}

impl<'a> Actor<'a> {
    fn new(scenario: &'a Scenario) -> Self {
        Self {
            scenario,
            now: Virtual::epoch(),
            timers: TimerQueue::new(),
            queue: VecDeque::new(),
            outstanding: None,
            history: Vec::new(),
            applied: None,
            next_seq: 1,
            program: VecDeque::new(),
            running: None,
            collected: String::new(),
            answers: Vec::new(),
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
            // Earliest wins; a step and a timer at the same instant let the step go first, so a
            // scenario that says "the digit arrives exactly as the gather expires" is the digit
            // arriving in time rather than a coin toss.
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
        self.run
    }

    fn step<B: Binding>(&mut self, step: Step, app: &mut B) {
        match step {
            Step::Event { kind, .. } => self.occur(kind, app),
            Step::Redeliver { seq, .. } => {
                if let Some(event) = self.history.iter().find(|e| e.seq == seq).cloned() {
                    self.ask(event, app);
                }
            }
        }
    }

    /// Something happened to the call.
    fn occur<B: Binding>(&mut self, kind: EventKind, app: &mut B) {
        // A digit a running `gather` asked for belongs to the gather, not to the app (see the
        // module docs).
        if let (EventKind::Dtmf { digit, .. }, Some(instruction)) = (&kind, self.running.clone())
            && let Verb::Gather {
                max, terminators, ..
            } = &instruction.verb
        {
            if terminators.contains(*digit) {
                self.resolve_gather(&instruction.id, GatherReason::Terminator, app);
            } else {
                self.collected.push(*digit);
                if self.collected.chars().count() >= *max {
                    self.resolve_gather(&instruction.id, GatherReason::Max, app);
                }
            }
            return;
        }

        if self.run.conclusion == Conclusion::Live
            && let EventKind::Ended { cause } = &kind
        {
            self.run.conclusion = Conclusion::Ended(cause.clone());
        }
        // A completion event resolves whatever it names, whether the harness or the scenario
        // produced it.
        self.settle(&kind);

        let event = Event {
            seq: self.next_seq,
            kind,
        };
        self.next_seq += 1;
        self.history.push(event.clone());
        self.enqueue(event);
        self.pump(app);
    }

    /// A completion event clears the instruction it names, and updates the leg list.
    fn settle(&mut self, kind: &EventKind) {
        let finished = match kind {
            EventKind::PlaybackFinished { instruction_id, .. }
            | EventKind::GatherFinished { instruction_id, .. } => Some(instruction_id.clone()),
            EventKind::DialFinished {
                instruction_id,
                leg,
                outcome,
            } => {
                // §5.2: a leg that did not answer is no longer listed. AC-7 asks exactly this.
                if *outcome != DialOutcome::Answered {
                    self.run.legs.retain(|held| held != leg);
                }
                Some(instruction_id.clone())
            }
            _ => None,
        };
        if let Some(id) = finished
            && self.running.as_ref().is_some_and(|i| i.id == id)
        {
            self.running = None;
            self.collected.clear();
            self.timers.clear(&TimerKey::Gather(id.clone()));
            self.timers.clear(&TimerKey::Dial(id));
        }
    }

    fn enqueue(&mut self, event: Event) {
        if event.kind.is_ended() {
            // The reserved slot. `call.ended` is a call's last word and is never what an overflow
            // discards, so it does not consult the bound at all.
            self.queue.push_back(event);
            return;
        }
        if self.queue.len() + 1 >= EVENT_QUEUE {
            self.run.dropped.push(event.kind);
            return;
        }
        self.queue.push_back(event);
    }

    /// Deliver the next queued event, if the app is free to take one (§6.3: one at a time).
    fn pump<B: Binding>(&mut self, app: &mut B) {
        if self.outstanding.is_some() {
            return;
        }
        let Some(event) = self.queue.pop_front() else {
            return;
        };
        self.ask(event, app);
    }

    fn ask<B: Binding>(&mut self, event: Event, app: &mut B) {
        let seq = event.seq;
        let reply = app.respond(&event);
        self.run.delivered.push(event);
        self.outstanding = Some(seq);
        if self.pending_reply(seq, reply) {
            // The app failed on the spot rather than after a delay, so nothing is outstanding and
            // the next queued event can go out now. Recursion terminates because every pass either
            // empties the queue or ends the call, and an ended call stops failing.
            self.pump(app);
        }
    }

    /// Returns whether the app failed synchronously, leaving nothing outstanding.
    fn pending_reply(&mut self, seq: u64, reply: Reply) -> bool {
        match reply.outcome {
            // An app that is not there has already failed; there is nothing to wait for.
            Outcome::Unreachable => {
                self.outstanding = None;
                self.fail(Failure::Unreachable);
                return true;
            }
            Outcome::Silent => {
                self.timers.set(
                    TimerKey::Callback(seq),
                    self.now,
                    self.scenario.policy.timeout,
                );
            }
            outcome => {
                self.answers.push((seq, outcome));
                self.timers.set(TimerKey::Reply(seq), self.now, reply.after);
                self.timers.set(
                    TimerKey::Callback(seq),
                    self.now,
                    self.scenario.policy.timeout,
                );
            }
        }
        false
    }

    fn fire<B: Binding>(&mut self, key: TimerKey, app: &mut B) {
        match key {
            TimerKey::Reply(seq) => {
                self.timers.clear(&TimerKey::Callback(seq));
                if self.outstanding == Some(seq) {
                    self.outstanding = None;
                }
                if let Some(index) = self.answers.iter().position(|(at, _)| *at == seq) {
                    let (_, outcome) = self.answers.remove(index);
                    self.apply(seq, outcome);
                }
                self.pump(app);
            }
            TimerKey::Callback(seq) => {
                // The app took too long. A reply that turns up later finds its `seq` already
                // settled and is ignored — which is the same rule redelivery uses.
                self.answers.retain(|(at, _)| *at != seq);
                if self.outstanding == Some(seq) {
                    self.outstanding = None;
                }
                self.applied = Some(self.applied.map_or(seq, |a| a.max(seq)));
                self.fail(Failure::Timeout);
                self.pump(app);
            }
            TimerKey::Gather(id) => {
                self.resolve_gather(&id, GatherReason::Timeout, app);
            }
            TimerKey::Dial(id) => {
                let leg = format!("leg-{id}");
                self.occur(
                    EventKind::DialFinished {
                        instruction_id: id,
                        leg,
                        outcome: DialOutcome::Timeout,
                    },
                    app,
                );
            }
        }
    }

    fn resolve_gather<B: Binding>(&mut self, id: &str, reason: GatherReason, app: &mut B) {
        if self.running.as_ref().is_none_or(|i| i.id != id) {
            return;
        }
        let digits = self.collected.clone();
        self.occur(
            EventKind::GatherFinished {
                instruction_id: id.to_owned(),
                digits,
                reason,
            },
            app,
        );
    }

    /// Apply what the app said.
    fn apply(&mut self, seq: u64, outcome: Outcome) {
        // AC-4: a response for a `seq` already settled is a redelivery's, and changes nothing.
        if self.applied.is_some_and(|applied| seq <= applied) {
            return;
        }
        self.applied = Some(seq);

        match outcome {
            Outcome::Document(document) => {
                if let Some(_verb) = document.is_rejected() {
                    // §6.4: rejected whole, and the app's declared policy applies as if the
                    // callback had failed with a 5xx. The prior program is untouched (AC-5).
                    self.fail(Failure::ServerError);
                    return;
                }
                self.adopt(document);
            }
            Outcome::ClientError { .. } => self.fail(Failure::ClientError),
            Outcome::ServerError { .. } => self.fail(Failure::ServerError),
            Outcome::Unreachable => self.fail(Failure::Unreachable),
            Outcome::Silent => {}
        }
    }

    /// Replace the program with this document, then run it (§6.3).
    fn adopt(&mut self, document: Document) {
        // An empty document means "keep going" — the one document that changes nothing. See the
        // module docs for why this is not "replace the program with nothing".
        if !document.instructions.is_empty() {
            if let Some(running) = self.running.take() {
                // Running interruptible work is stopped; a `play` is the case that shows.
                if matches!(running.verb, Verb::Play { .. }) {
                    self.run.effects.push(Effect::StopPlay {
                        id: running.id.clone(),
                    });
                }
                self.timers.clear(&TimerKey::Gather(running.id.clone()));
                self.timers.clear(&TimerKey::Dial(running.id));
                self.collected.clear();
            }
            // Whatever was still queued is discarded — replacement, not append (AC-3). Queued
            // instructions never started, so nothing is stopped for them.
            self.program.clear();
            self.program.extend(document.instructions);
        }
        self.advance();
    }

    /// Execute instructions in order until one blocks or the queue empties (§6.1).
    fn advance(&mut self) {
        while self.running.is_none() {
            let Some(instruction) = self.program.pop_front() else {
                return;
            };
            let blocks = instruction.verb.blocks();
            self.execute(&instruction);
            if blocks && self.run.conclusion == Conclusion::Live {
                self.running = Some(instruction);
                return;
            }
        }
    }

    fn execute(&mut self, instruction: &Instruction) {
        let id = instruction.id.clone();
        match &instruction.verb {
            Verb::Answer => self.run.effects.push(Effect::Answer),
            Verb::Play { source, .. } => self.run.effects.push(Effect::StartPlay {
                id,
                source: source.clone(),
            }),
            Verb::Gather { timeout, .. } => {
                self.collected.clear();
                self.run
                    .effects
                    .push(Effect::StartGather { id: id.clone() });
                self.timers.set(TimerKey::Gather(id), self.now, *timeout);
            }
            Verb::Dial { target, timeout } => {
                self.run.legs.push(format!("leg-{id}"));
                self.run.effects.push(Effect::Dial {
                    id: id.clone(),
                    target: target.clone(),
                });
                self.timers.set(TimerKey::Dial(id), self.now, *timeout);
            }
            Verb::Hangup { cause } => {
                self.run.effects.push(Effect::Hangup {
                    cause: cause.clone(),
                });
                self.end(cause.clone());
            }
            Verb::Reject { status } => {
                self.run.effects.push(Effect::Reject { status: *status });
                self.end(EndCause::Rejected { status: *status });
            }
            Verb::Other(name) => self.run.effects.push(Effect::Other {
                id,
                verb: name.clone(),
            }),
            // Never reached: §6.4 rejects the document before it is adopted.
            Verb::Unknown(_) => {}
        }
    }

    /// The declared response to a failure (§9.2). Never a hard-coded one — ground rule 3.
    fn fail(&mut self, failure: Failure) {
        // A call that is already over cannot be hung up twice. The app still receives `call.ended`
        // and may still answer it badly; that answer has nothing left to act on, and counting it
        // would report a second consultation of a knob that was only consulted once.
        if self.run.conclusion != Conclusion::Live {
            return;
        }
        self.run.failures.push(failure);
        match self.scenario.policy.action_for(failure).clone() {
            // "Keep program" is not "stop": whatever was queued behind the instruction that just
            // resolved carries on, which is what AC-5 means by the prior program still running.
            OnFailure::Continue => self.advance(),
            OnFailure::Hangup => {
                self.run.effects.push(Effect::Hangup {
                    cause: EndCause::Hangup,
                });
                self.end(EndCause::Hangup);
            }
            OnFailure::Reject { status } => {
                self.run.effects.push(Effect::Reject { status });
                self.end(EndCause::Rejected { status });
            }
        }
    }

    /// The call is over: stop the program and queue the last event the app will see.
    fn end(&mut self, cause: EndCause) {
        if self.run.conclusion != Conclusion::Live {
            return;
        }
        self.run.conclusion = Conclusion::Ended(cause.clone());
        self.program.clear();
        self.running = None;
        let event = Event {
            seq: self.next_seq,
            kind: EventKind::Ended { cause },
        };
        self.next_seq += 1;
        self.history.push(event.clone());
        self.enqueue(event);
    }
}

/// What a scenario should have produced.
///
/// Data, like the scenario itself — acceptance point 1's "plus the expected effects and outcomes".
/// Keeping it data rather than a closure is what lets `A-2` and `A-4` run the same vectors through
/// their own binding and get the same verdict, instead of each restating the assertions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expectation {
    /// The effects the host must have executed, in order. `None` asserts nothing about them.
    pub effects: Option<Vec<Effect>>,
    /// Effects that must **not** appear, whatever else does.
    pub without: Vec<Effect>,
    /// How the call must have finished.
    pub conclusion: Option<Conclusion>,
    /// The `seq` of every delivered event, in delivery order.
    pub delivered_seqs: Option<Vec<u64>>,
    /// The §9.2 knobs that must have been consulted, in order.
    pub failures: Option<Vec<Failure>>,
    /// The legs the snapshot must list.
    pub legs: Option<Vec<String>>,
    /// Whether `call.ended` must have reached the app.
    pub app_saw_ended: Option<bool>,
    /// An event of this type must have been delivered, equal to this.
    pub delivered_event: Option<EventKind>,
    /// Whether anything must have been dropped by the queue's overflow policy.
    pub dropped_any: Option<bool>,
}

impl Expectation {
    /// Expecting nothing in particular.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Exactly these effects, in this order.
    #[must_use]
    pub fn effects(mut self, effects: Vec<Effect>) -> Self {
        self.effects = Some(effects);
        self
    }

    /// And none of these, anywhere.
    #[must_use]
    pub fn without(mut self, effects: Vec<Effect>) -> Self {
        self.without = effects;
        self
    }

    /// Finishing like this.
    #[must_use]
    pub fn conclusion(mut self, conclusion: Conclusion) -> Self {
        self.conclusion = Some(conclusion);
        self
    }

    /// Delivering these `seq`s, in this order.
    #[must_use]
    pub fn delivered_seqs(mut self, seqs: Vec<u64>) -> Self {
        self.delivered_seqs = Some(seqs);
        self
    }

    /// Consulting these knobs, in this order.
    #[must_use]
    pub fn failures(mut self, failures: Vec<Failure>) -> Self {
        self.failures = Some(failures);
        self
    }

    /// Leaving the snapshot listing these legs.
    #[must_use]
    pub fn legs(mut self, legs: Vec<String>) -> Self {
        self.legs = Some(legs);
        self
    }

    /// With `call.ended` reaching the app, or not.
    #[must_use]
    pub fn app_saw_ended(mut self, saw: bool) -> Self {
        self.app_saw_ended = Some(saw);
        self
    }

    /// Having delivered this exact event.
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

    /// Check a run against this, naming the first thing that disagrees.
    ///
    /// # Errors
    /// The mismatch, described well enough to act on without re-reading the scenario.
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
            && !run.delivered.iter().any(|e| &e.kind == expected)
        {
            return Err(format!(
                "{at}: expected {expected:?} among delivered, got {:?}",
                run.delivered.iter().map(|e| &e.kind).collect::<Vec<_>>()
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
        // AC-9's invariant, checked on every vector rather than only the one that poses it: an
        // overflow may discard anything except the call's last word.
        if run.dropped.iter().any(EventKind::is_ended) {
            return Err(format!(
                "{at}: call.ended was dropped, which must never happen"
            ));
        }
        Ok(())
    }
}

/// A scenario and what it should produce.
#[derive(Debug)]
pub struct Vector {
    /// What happens.
    pub scenario: Scenario,
    /// What should come of it.
    pub expect: Expectation,
}

impl Vector {
    /// Pair a scenario with its expectation.
    #[must_use]
    pub fn new(scenario: Scenario, expect: Expectation) -> Self {
        Self { scenario, expect }
    }

    /// Run it against the harness's own scripted app.
    ///
    /// # Errors
    /// Whatever disagreed.
    pub fn check(self) -> Result<(), String> {
        let run = self.scenario.run();
        self.expect.check(&run)
    }

    /// Run it against a binding of the caller's own — `A-2`'s HTTP client, `A-4`'s session.
    ///
    /// # Errors
    /// Whatever disagreed.
    pub fn check_against<B: Binding>(&self, app: &mut B) -> Result<(), String> {
        let run = self.scenario.run_against(app);
        self.expect.check(&run)
    }
}
