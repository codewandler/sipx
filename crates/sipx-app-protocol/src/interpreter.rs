//! The sans-IO instruction interpreter: §§5–6 of the contract as a state machine.
//!
//! [`docs/specs/app-contract.md`](../../../../docs/specs/app-contract.md) §2: *the interpreter is
//! sans-IO*. [`Input`] in, [`Output`] out; no socket, no clock, no async runtime. Time enters as
//! [`Input::TimerFired`] and leaves as [`Output::SetTimer`], the app is reached by handing an
//! [`Output::Deliver`] to a driver and bringing the answer back as [`Input::Response`], and the
//! call framework is reached by performing [`Effect`]s. Every binding §§7–8 names — a webhook
//! server, a WebSocket peer, an embedded runtime with no wire at all — is a driver over this one
//! machine, which is why the vectors in §11 can be run against it with no I/O in sight.
//!
//! # The continuation rule, by construction
//!
//! §6.3 is the section this module is shaped around, and both of its clauses are held by types
//! rather than by checks:
//!
//! - **At most one callback is outstanding per call.** Delivering an event produces exactly one
//!   [`Callback`], and a [`Callback`] is neither [`Clone`] nor [`Copy`] and has no public
//!   constructor. [`Input::Response`] takes it **by value**. So a driver cannot answer one
//!   delivery twice: the token it would need is gone, and there is no second one to be had.
//!   While one is outstanding the interpreter delivers nothing further — later events queue
//!   (§6.3, vector AC-8) — so a driver cannot be holding two either.
//! - **A document accepted in response to event E replaces the pending program.** The queue is a
//!   [`Program`], whose inner `VecDeque` is private to its own module and which exposes no
//!   `push`, no `extend` and no `append` — only [`Program::replace`], which takes a whole
//!   program, and [`Program::abandon`], which empties it. A second document therefore cannot
//!   leave a stale instruction behind, because appending is not an operation the type has, and
//!   no edit to *this* file can give it one.
//!
//! The one exception §6.3 names is spelled out where it is implemented: an **empty**
//! `instructions` array means "keep going" and is the one document that does *not* replace.

use std::collections::{BTreeMap, VecDeque};

use crate::document::{Document, DtmfMode, Gather, Instruction, Source, TransferTarget, Verb};
use crate::error::Error;
use crate::event::{CallSnapshot, CallState, EndCause, Envelope, EventKind, GatherReason, Leg};
use crate::policy::{Failure, OnFailure, Policy};
use crate::program::Program;
use crate::time::Timestamp;

/// How many events may queue behind an outstanding callback before the queue is full.
///
/// §8 requires backpressure to be *declared* rather than discovered, and a queue with no bound is
/// a memory leak wearing an app's slowness as a disguise. What overflow may never drop is
/// `call.ended` (§5.3, vector AC-9): an app that missed the last event of a call would wait for it
/// forever, so a full queue makes room for that one by evicting its oldest member instead.
pub const MAX_QUEUED_EVENTS: usize = 32;

/// A timer the driver arms on the interpreter's behalf.
///
/// The whole of this crate's relationship with time. §2: no clock is read here, so every duration
/// leaves as an [`Output::SetTimer`] and comes back as an [`Input::TimerFired`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    /// §9.2's `timeout_ms`: how long the outstanding callback may take.
    Callback,
    /// The `pause` verb's `ms`.
    Pause,
    /// A `gather`'s `timeout_ms` — the whole collection.
    GatherOverall,
    /// A `gather`'s `digit_timeout_ms` — the wait for the next key.
    GatherDigit,
}

/// The right to answer one delivery, once.
///
/// §6.3's "at most one callback is outstanding per call" as a value. Deliberately **not**
/// [`Clone`] and **not** [`Copy`], with no public constructor: a driver receives one in an
/// [`Output::Deliver`] and gives it back in an [`Input::Response`], and the compiler is what stops
/// it from doing that twice. A redelivery (§7: retries repeat `seq`) may produce two answers on
/// the wire, and only the first can be presented — which is vector AC-4, held by the type system
/// rather than by a sequence-number check the host has to remember to write.
#[derive(Debug, PartialEq, Eq)]
#[must_use = "an unanswered callback stalls the call until its timer fires"]
pub struct Callback {
    seq: u64,
}

impl Callback {
    /// Which delivery this answers — the envelope's `seq` (§5.1).
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn new(seq: u64) -> Self {
        Self { seq }
    }
}

/// What the app said, or failed to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// A `2xx` and its body, as bytes off the wire. The interpreter parses it, so a binding needs
    /// no reader of its own and §6.4's "rejected whole" is enforced in one place for every
    /// binding. An empty or whitespace-only body is §6.3's "keep going" (§7 says as much).
    Body(String),
    /// A document a caller already has in hand — what an embedded runtime (§8) produces, having
    /// never put anything on a wire.
    Document(Document),
    /// The callback failed; §9.2's declaration applies.
    Failed(Failure),
}

/// Something to do (§3: every instruction resolves to exactly one call-framework operation).
///
/// One variant per row of §3's table, which is what "the contract may not name a verb that has no
/// operation" looks like when it is checked by a compiler. `pause` and `tag` are absent on
/// purpose — §3's last row calls them interpreter-internal, and they are: one is a timer, the
/// other writes the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Effect {
    /// Accept the invitation.
    Answer,
    /// Send a provisional.
    Ring {
        /// Reliably (RFC 3262)?
        reliable: bool,
    },
    /// Refuse the invitation.
    Reject {
        /// The status.
        status: u16,
        /// The reason phrase, if the app chose one.
        reason: Option<String>,
    },
    /// Start a playback. The `instruction_id` is what its `call.playback.finished` must carry.
    Play {
        /// The instruction this belongs to.
        instruction_id: String,
        /// Where the audio comes from.
        source: Source,
        /// Whether a keypress cuts it short.
        interruptible: bool,
    },
    /// Stop the running playback (`M-17`). Issued when a replacement program supersedes it.
    StopPlayback,
    /// Start a recording.
    Record {
        /// The instruction this belongs to.
        instruction_id: String,
        /// The longest recording to make.
        max_ms: Option<u32>,
        /// How much trailing silence ends it.
        idle_stop_ms: Option<u32>,
    },
    /// Send digits to the far end.
    SendDigits {
        /// Which keys.
        digits: String,
        /// How long to hold each.
        duration_ms: Option<u32>,
    },
    /// Place a new leg of this call.
    Dial {
        /// The instruction this belongs to.
        instruction_id: String,
        /// The name the interpreter gave the leg; its `call.dial.finished` must carry it.
        leg: String,
        /// Who to call.
        target: String,
        /// What to call as.
        from: Option<String>,
        /// How long to let it ring.
        timeout_ms: Option<u32>,
        /// Header fields to set — already filtered against the host's allowlist (§6.5).
        headers: BTreeMap<String, String>,
    },
    /// Couple this leg's media to another's.
    Bridge {
        /// The other leg.
        leg: String,
        /// What to do with digits.
        dtmf: DtmfMode,
    },
    /// Break the coupling.
    Unbridge,
    /// Put the far end on hold.
    Hold,
    /// Take it off hold.
    Resume,
    /// Gate this side's own audio.
    Mute,
    /// Let it through again.
    Unmute,
    /// Transfer the call away.
    Transfer {
        /// Where to.
        target: TransferTarget,
    },
    /// Accept a transfer the far end asked for.
    AcceptTransfer,
    /// Refuse one.
    RefuseTransfer {
        /// The status.
        status: u16,
    },
    /// End the call.
    HangUp {
        /// Why.
        cause: EndCause,
    },
}

/// What the interpreter wants done.
///
/// Deliberately **not** `#[non_exhaustive]`, unlike [`Effect`] and [`Input`]. A driver's whole job
/// is to carry out every output; one it silently ignored would be a call that quietly stopped
/// working, not a driver that still compiles. So a new kind of output is a breaking change every
/// driver is made to look at — whereas a new *verb* ([`Effect`]) or a new kind of input is
/// something a driver can reasonably meet with a default arm.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    /// Perform this call-framework operation.
    Effect(Effect),
    /// Deliver this envelope to the app, and bring its answer back as [`Input::Response`] with
    /// this callback.
    ///
    /// The envelope is boxed because it is much the largest thing this enum carries, and every
    /// other variant would otherwise pay for it.
    Deliver {
        /// What to send.
        envelope: Box<Envelope>,
        /// The right to answer it, once.
        callback: Callback,
    },
    /// Arrange for this timer to fire after this long.
    SetTimer {
        /// Which timer.
        timer: Timer,
        /// How long from now, in milliseconds.
        after_ms: u32,
    },
    /// Cancel a timer that has not fired.
    ClearTimer(Timer),
}

/// Something that happened.
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Input {
    /// A call event, in the contract's own vocabulary. With the `call` feature,
    /// [`crate::event_from_call`] lifts `sipx-call`'s [`CallEvent`] (`C-3`) into it.
    ///
    /// [`CallEvent`]: https://docs.rs/sipx-call
    Event(EventKind),
    /// This side muted or unmuted its own audio (`M-18`).
    ///
    /// Not an event type: §5.3 has none for it on purpose, because mute is a decision one party
    /// makes about its own microphone and the far end is told nothing. What a remote app sees is
    /// `media.muted` on the next snapshot (§5.2), which is what this input updates.
    MediaGate {
        /// Whether this side's audio is now gated.
        muted: bool,
    },
    /// A timer the driver armed has fired.
    TimerFired(Timer),
    /// The app answered the outstanding callback — or failed to.
    Response {
        /// The token from the [`Output::Deliver`] this answers. Taken by value: that is §6.3's
        /// "at most one outstanding callback", enforced by move semantics.
        callback: Callback,
        /// What it said.
        response: Response,
    },
}

/// The instruction currently blocking the queue (§6.1).
#[derive(Debug)]
struct Running {
    instruction: Instruction,
    /// Digits a `gather` has collected so far.
    digits: String,
    /// Whether a playback this instruction started is still going, so that a replacement knows
    /// whether there is anything to stop.
    playing: bool,
}

/// The `sipx.app.v1` interpreter for one call.
///
/// §2: *one call, one program, one app*. An instance is a call, and it is the whole of what the
/// contract says about that call: its snapshot, its program, and whether its app owes an answer.
#[derive(Debug)]
pub struct Interpreter {
    snapshot: CallSnapshot,
    policy: Policy,
    /// §6.3's *pending program*. A [`Program`] rather than a bare queue precisely so that the
    /// replacement rule is the type's, not this file's — see that module's header.
    program: Program,
    running: Option<Running>,
    /// The `seq` of the outstanding callback, if one is out (§5.1, §6.3).
    outstanding: Option<u64>,
    /// §5.1: per-call, starts at 1, increments by 1 per event.
    next_seq: u64,
    /// Events that arrived while a callback was outstanding (§6.3, AC-8).
    queued: VecDeque<EventKind>,
    /// Names already given to legs, so a second `dial` does not reuse one.
    legs_named: usize,
    /// Whether `call.ended` has been delivered. After it there is nothing left to drive.
    ended: bool,
    /// A terminal event waiting behind the one callback already in flight (§6.3).
    terminal_pending: Option<(Timestamp, EventKind)>,
}

impl Interpreter {
    /// A new interpreter for a call, with the app's declared failure semantics (§9.2).
    #[must_use]
    pub fn new(snapshot: CallSnapshot, policy: Policy) -> Self {
        Self {
            snapshot,
            policy,
            program: Program::default(),
            running: None,
            outstanding: None,
            next_seq: 1,
            queued: VecDeque::new(),
            legs_named: 0,
            ended: false,
            terminal_pending: None,
        }
    }

    /// The call as it is *now* (§5.2, §6.3: a snapshot always reflects now, never the queue's
    /// past).
    #[must_use]
    pub fn snapshot(&self) -> &CallSnapshot {
        &self.snapshot
    }

    /// How many instructions are still queued. The queue itself is not exposed: §6.3's replacement
    /// rule is only enforceable if nothing outside this type can write it.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.program.len()
    }

    /// The id of the instruction currently blocking the queue, if one is (§6.1).
    #[must_use]
    pub fn running(&self) -> Option<&str> {
        self.running.as_ref().map(|r| r.instruction.id.as_str())
    }

    /// Whether a callback is outstanding — §6.3's alternation, observable.
    #[must_use]
    pub fn awaiting_app(&self) -> bool {
        self.outstanding.is_some()
    }

    /// Drive the machine one step.
    ///
    /// `at` is the driver's clock reading, used for the envelope's `at` (§5.1) and for nothing
    /// else. It is a parameter rather than a field because a field could be stale and a clock read
    /// would break §2; passing it in makes "this crate never asks what time it is" a property of
    /// the signature.
    pub fn handle(&mut self, at: Timestamp, input: Input) -> Vec<Output> {
        let mut out = Vec::new();
        match input {
            Input::Event(event) => {
                if self.ended || self.terminal_pending.is_some() {
                    return out;
                }
                // The snapshot is updated even when the event queues: §6.3 says a snapshot always
                // reflects *now*, not the queue's past.
                self.apply_to_snapshot(&event);
                if matches!(event, EventKind::Ended { .. }) {
                    self.finish(at, event, &mut out);
                } else if self.outstanding.is_some() {
                    self.enqueue(event);
                } else {
                    self.process(at, event, &mut out);
                }
            }
            Input::MediaGate { muted } if !self.ended && self.terminal_pending.is_none() => {
                self.snapshot.media.muted = muted;
            }
            Input::MediaGate { .. } => {}
            Input::TimerFired(timer) => self.fire(at, timer, &mut out),
            Input::Response { callback, response } => {
                self.respond(at, &callback, response, &mut out);
            }
        }
        out
    }

    /// Enter the one-way terminal state and deliver its one event.
    fn finish(&mut self, at: Timestamp, event: EventKind, out: &mut Vec<Output>) {
        if self.ended {
            return;
        }
        self.queued.clear();
        self.program.abandon();
        if let Some(running) = self.running.take() {
            if running.playing {
                out.push(Output::Effect(Effect::StopPlayback));
            }
            match running.instruction.verb {
                Verb::Pause { .. } => out.push(Output::ClearTimer(Timer::Pause)),
                Verb::GatherDigits(_) => {
                    out.push(Output::ClearTimer(Timer::GatherOverall));
                    out.push(Output::ClearTimer(Timer::GatherDigit));
                }
                _ => {}
            }
        }
        if self.outstanding.is_some() {
            self.terminal_pending = Some((at, event));
        } else {
            self.deliver(at, event, out);
        }
    }

    /// §6.3's alternation: an event arriving while a callback is outstanding waits its turn.
    ///
    /// The queue is bounded (see [`MAX_QUEUED_EVENTS`]). What overflow may not drop is
    /// `call.ended` — vector AC-9 — so if the queue is full when it arrives, the oldest member
    /// makes room for it rather than the other way round.
    fn enqueue(&mut self, event: EventKind) {
        if self.queued.len() < MAX_QUEUED_EVENTS {
            self.queued.push_back(event);
            return;
        }
        if matches!(event, EventKind::Ended { .. }) {
            self.queued.pop_front();
            self.queued.push_back(event);
        }
        // Otherwise the new event is what is dropped. §2 says events are authoritative and carry a
        // full snapshot, so the app resynchronises from the next one it does receive.
    }

    /// One event, all the way through: the running instruction reacts, the queue advances, and the
    /// app is asked what to do next.
    fn process(&mut self, at: Timestamp, event: EventKind, out: &mut Vec<Output>) {
        let completion = self.feed_running(&event, out);
        self.advance(out);
        self.deliver(at, event, out);
        // A `gather` that resolved on this event produces its own `call.gather.finished` (§5.3),
        // which is an event like any other and therefore queues behind the delivery just made.
        if let Some(finished) = completion {
            self.apply_to_snapshot(&finished);
            self.enqueue(finished);
        }
    }

    /// Give the event to whatever is blocking the queue. Returns a completion event the
    /// interpreter itself produced, if this resolved a `gather`.
    fn feed_running(&mut self, event: &EventKind, out: &mut Vec<Output>) -> Option<EventKind> {
        let running = self.running.as_mut()?;
        let id = running.instruction.id.clone();
        match (&mut running.instruction.verb, event) {
            (Verb::Answer, EventKind::Answered)
            | (Verb::Reject { .. } | Verb::Hangup { .. }, EventKind::Ended { .. }) => {
                self.running = None;
            }
            (Verb::Play { .. }, EventKind::PlaybackFinished { instruction_id, .. })
                if *instruction_id == id =>
            {
                self.running = None;
            }
            (Verb::Record { .. }, EventKind::RecordingFinished { instruction_id, .. })
                if *instruction_id == id =>
            {
                self.running = None;
            }
            (Verb::Dial { .. }, EventKind::DialFinished { instruction_id, .. })
                if *instruction_id == id =>
            {
                self.running = None;
            }
            // A `gather`'s prompt finishing does not resolve the gather — the gather is collecting
            // digits, and the prompt was only ever something to listen to while doing it.
            (Verb::GatherDigits(_), EventKind::PlaybackFinished { instruction_id, .. })
                if *instruction_id == id =>
            {
                running.playing = false;
            }
            (Verb::GatherDigits(gather), EventKind::Dtmf { digit, .. }) => {
                let gather = gather.clone();
                return self.collect(&gather, *digit, out);
            }
            _ => {}
        }
        None
    }

    /// A keypress, offered to a running `gather` (§6.2).
    fn collect(
        &mut self,
        gather: &Gather,
        digit: char,
        out: &mut Vec<Output>,
    ) -> Option<EventKind> {
        let running = self.running.as_mut()?;
        // A terminator ends collection — but only once `min` digits are in hand, so that a `#`
        // typed early is a keypress rather than an empty answer.
        if gather.terminators.contains(digit) {
            let long_enough = running.digits.chars().count() >= gather.min as usize;
            if long_enough {
                return self.resolve_gather(GatherReason::Terminator, out);
            }
            return None;
        }
        running.digits.push(digit);
        if gather
            .max
            .is_some_and(|max| running.digits.chars().count() >= max as usize)
        {
            return self.resolve_gather(GatherReason::Max, out);
        }
        // Re-arm the inter-digit wait; it measures the gap between keys, so it starts over on
        // every one of them.
        if let Some(ms) = gather.digit_timeout_ms {
            out.push(Output::SetTimer {
                timer: Timer::GatherDigit,
                after_ms: ms,
            });
        }
        None
    }

    /// End the running `gather` and produce its `call.gather.finished` (§5.3).
    fn resolve_gather(&mut self, reason: GatherReason, out: &mut Vec<Output>) -> Option<EventKind> {
        let running = self.running.take()?;
        out.push(Output::ClearTimer(Timer::GatherOverall));
        out.push(Output::ClearTimer(Timer::GatherDigit));
        if running.playing {
            out.push(Output::Effect(Effect::StopPlayback));
        }
        Some(EventKind::GatherFinished {
            instruction_id: running.instruction.id,
            digits: running.digits,
            reason,
        })
    }

    /// Run instructions until one blocks or the program is empty (§6.1: strictly in order, and a
    /// verb with a completion event blocks the queue until it resolves).
    fn advance(&mut self, out: &mut Vec<Output>) {
        while self.running.is_none() {
            let Some(instruction) = self.program.take_next() else {
                return;
            };
            self.start(instruction, out);
        }
    }

    /// Issue one instruction's effects, and record it as running if it blocks.
    /// The verbs that are one instruction in and one effect out, with no interpreter state to
    /// touch on the way. Split from [`Self::start`], which keeps the arms that write the snapshot,
    /// arm a timer or record that audio is playing — those are the interesting ones, and having
    /// them alone in `start` is the point of the split as much as the length limit is.
    fn simple_effect(verb: &Verb, id: &str) -> Option<Effect> {
        Some(match verb {
            Verb::Answer => Effect::Answer,
            Verb::Ring { reliable } => Effect::Ring {
                reliable: *reliable,
            },
            Verb::Reject { status, reason } => Effect::Reject {
                status: *status,
                reason: reason.clone(),
            },
            Verb::Record {
                max_ms,
                idle_stop_ms,
            } => Effect::Record {
                instruction_id: id.to_owned(),
                max_ms: *max_ms,
                idle_stop_ms: *idle_stop_ms,
            },
            Verb::SendDtmf {
                digits,
                duration_ms,
            } => Effect::SendDigits {
                digits: digits.clone(),
                duration_ms: *duration_ms,
            },
            Verb::Bridge { leg, dtmf } => Effect::Bridge {
                leg: leg.clone(),
                dtmf: *dtmf,
            },
            Verb::Unbridge => Effect::Unbridge,
            Verb::Hold => Effect::Hold,
            Verb::Resume => Effect::Resume,
            Verb::Mute => Effect::Mute,
            Verb::Unmute => Effect::Unmute,
            Verb::Transfer { target } => Effect::Transfer {
                target: target.clone(),
            },
            Verb::AcceptTransfer => Effect::AcceptTransfer,
            Verb::RefuseTransfer { status } => Effect::RefuseTransfer { status: *status },
            Verb::Hangup { cause } => Effect::HangUp { cause: *cause },
            // The rest need more than an effect — a timer, a snapshot write, or a note that
            // audio is playing — and `start` has them. Spelled out rather than left to `_` so
            // that a verb added to §6.2 is a compile error here until someone classifies it.
            Verb::Play { .. }
            | Verb::GatherDigits(_)
            | Verb::Dial { .. }
            | Verb::Pause { .. }
            | Verb::Tag { .. } => return None,
        })
    }

    /// Issue one instruction's effects, and record it as running if it blocks.
    fn start(&mut self, instruction: Instruction, out: &mut Vec<Output>) {
        let id = instruction.id.clone();
        let mut playing = false;
        if let Some(effect) = Self::simple_effect(&instruction.verb, &id) {
            out.push(Output::Effect(effect));
            if instruction.verb.blocks() {
                self.running = Some(Running {
                    instruction,
                    digits: String::new(),
                    playing,
                });
            }
            return;
        }
        match &instruction.verb {
            Verb::Play {
                source,
                interruptible,
            } => {
                playing = true;
                out.push(Output::Effect(Effect::Play {
                    instruction_id: id.clone(),
                    source: source.clone(),
                    interruptible: *interruptible,
                }));
            }
            Verb::GatherDigits(gather) => {
                if let Some(prompt) = &gather.prompt {
                    playing = true;
                    // §6.2: a gather's prompt is interruptible by definition. A prompt that
                    // swallowed the digit that stopped it would lose the first digit of every PIN.
                    out.push(Output::Effect(Effect::Play {
                        instruction_id: id.clone(),
                        source: prompt.clone(),
                        interruptible: true,
                    }));
                }
                if let Some(ms) = gather.timeout_ms {
                    out.push(Output::SetTimer {
                        timer: Timer::GatherOverall,
                        after_ms: ms,
                    });
                }
            }
            Verb::Dial {
                target,
                from,
                timeout_ms,
                headers,
            } => {
                let leg = self.name_a_leg();
                self.snapshot.legs.push(Leg {
                    leg: leg.clone(),
                    state: CallState::Ringing,
                    to: target.clone(),
                });
                out.push(Output::Effect(Effect::Dial {
                    instruction_id: id.clone(),
                    leg,
                    target: target.clone(),
                    from: from.clone(),
                    timeout_ms: *timeout_ms,
                    headers: headers.clone(),
                }));
            }
            // §3: `pause` and `tag` are interpreter-internal. One is a timer and the other writes
            // the snapshot; neither is anything for the call framework to do.
            Verb::Pause { ms } => out.push(Output::SetTimer {
                timer: Timer::Pause,
                after_ms: *ms,
            }),
            Verb::Tag { key, value } => {
                self.snapshot.tags.insert(key.clone(), value.clone());
            }
            // Everything else is `simple_effect`'s, and returned above. Reachable only if a new
            // verb is added to that function's `return None` group without being added here.
            _ => {}
        }
        if instruction.verb.blocks() {
            self.running = Some(Running {
                instruction,
                digits: String::new(),
                playing,
            });
        }
    }

    /// §5.2's `legs`: `a` is this call, so the ones a `dial` creates start at `b`.
    fn name_a_leg(&mut self) -> String {
        self.legs_named += 1;
        let offset = u32::try_from(self.legs_named).unwrap_or(u32::MAX);
        char::from_u32('a' as u32 + offset)
            .filter(char::is_ascii_lowercase)
            .map_or_else(|| format!("leg{offset}"), |c| c.to_string())
    }

    /// Hand the event to the app and start §9.2's clock (§5.1, §6.3).
    fn deliver(&mut self, at: Timestamp, event: EventKind, out: &mut Vec<Output>) {
        if self.ended {
            return;
        }
        if matches!(event, EventKind::Ended { .. }) {
            self.ended = true;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.outstanding = Some(seq);
        out.push(Output::Deliver {
            envelope: Box::new(Envelope {
                seq,
                at,
                call: self.snapshot.clone(),
                event,
            }),
            callback: Callback::new(seq),
        });
        out.push(Output::SetTimer {
            timer: Timer::Callback,
            after_ms: self.policy.timeout_ms,
        });
    }

    fn fire(&mut self, at: Timestamp, timer: Timer, out: &mut Vec<Output>) {
        if (self.ended || self.terminal_pending.is_some()) && timer != Timer::Callback {
            return;
        }
        match timer {
            Timer::Callback => {
                // §9.2: the callback did not return in time. The token the driver holds is now
                // stale; a response presented with it is ignored below.
                if self.outstanding.take().is_some() && !self.ended {
                    if let Some((terminal_at, event)) = self.terminal_pending.take() {
                        self.deliver(terminal_at, event, out);
                        return;
                    }
                    if self.snapshot.state == CallState::Ended {
                        // The terminal event arrived while this callback was outstanding. Its
                        // snapshot is authoritative even though its envelope is still queued: do
                        // not tear down an already-ended call; release the queue so `call.ended`
                        // can become the last delivery.
                        self.drain_ended(at, out);
                    } else {
                        self.fail(at, Failure::Timeout, out);
                    }
                }
            }
            Timer::Pause => {
                if matches!(
                    self.running.as_ref().map(|r| &r.instruction.verb),
                    Some(Verb::Pause { .. })
                ) {
                    self.running = None;
                    self.advance(out);
                }
            }
            Timer::GatherOverall | Timer::GatherDigit => {
                if matches!(
                    self.running.as_ref().map(|r| &r.instruction.verb),
                    Some(Verb::GatherDigits(_))
                ) && let Some(finished) = self.resolve_gather(GatherReason::Timeout, out)
                {
                    self.after_internal_event(at, finished, out);
                }
            }
        }
    }

    /// An event the interpreter produced itself, rather than one the call framework reported.
    fn after_internal_event(&mut self, at: Timestamp, event: EventKind, out: &mut Vec<Output>) {
        self.apply_to_snapshot(&event);
        if self.outstanding.is_some() {
            self.enqueue(event);
        } else {
            self.advance(out);
            self.deliver(at, event, out);
        }
    }

    /// The app answered — or the binding says it did not.
    fn respond(
        &mut self,
        at: Timestamp,
        callback: &Callback,
        response: Response,
        out: &mut Vec<Output>,
    ) {
        // A token whose delivery is no longer the outstanding one is spent: its callback already
        // returned, or its timer already fired and §9.2 was already applied. Move semantics make
        // the first case unreachable for a driver — see this module's header — and this is what
        // answers the second. Either way the response is ignored, which is vector AC-4.
        if self.outstanding != Some(callback.seq) {
            return;
        }
        self.outstanding = None;
        out.push(Output::ClearTimer(Timer::Callback));
        if let Some((terminal_at, event)) = self.terminal_pending.take() {
            self.deliver(terminal_at, event, out);
            return;
        }
        // `call.ended` is the app's last observation, not an opportunity to act on a call that no
        // longer exists. A failed or non-empty answer to that delivery is therefore acknowledged
        // and ignored; applying its policy could issue a second hangup or refusal after teardown.
        if self.ended {
            return;
        }
        if self.snapshot.state == CallState::Ended {
            // `call.ended` may be queued behind the callback being answered. The snapshot already
            // says there is no live call on which a failure policy or document can act; draining
            // is still required so the terminal envelope itself reaches the app.
            self.drain_ended(at, out);
            return;
        }
        match response {
            Response::Failed(failure) => self.fail(at, failure, out),
            Response::Document(document) => self.accept(document, out),
            Response::Body(body) => {
                // §7: an empty response body is "keep going".
                if body.trim().is_empty() {
                    self.accept(Document::keep_going(), out);
                } else {
                    match Document::parse(&body) {
                        Ok(document) => self.accept(document, out),
                        // §6.4: a document that fails to parse, names an unknown verb or carries
                        // an illegal field value is rejected **whole**, and the app's declared
                        // failure policy applies as if the callback had failed with a 5xx.
                        Err(_) => self.fail(at, Failure::ServerError, out),
                    }
                }
            }
        }
        self.drain(at, out);
    }

    /// §6.3: the document is the entire new program.
    fn accept(&mut self, document: Document, out: &mut Vec<Output>) {
        // §6.5's allowlist is host configuration, so it is checked here rather than in the parser
        // — and a document that breaks it is rejected whole like any other (§6.4). It cannot be
        // applied in part, because the only way instructions reach `self.program` is the single
        // whole-program `replace` below, which this return skips.
        if let Err(Error::BadField { .. }) = self.check_allowlist(&document) {
            self.apply_failure(self.policy.on(Failure::ServerError), out);
            return;
        }
        // §6.3: an **empty** `instructions` array means "keep going" — the one document that does
        // not replace. Treating it as a replacement would make "keep going" a synonym for "throw
        // the program away", which is the opposite of what the section says it means.
        if document.instructions.is_empty() {
            return;
        }
        // Everything below is §6.3's replacement. `Program::replace` takes the whole document,
        // so whatever was queued is discarded by construction rather than by care taken here;
        // running interruptible work is stopped; `bridge` state and `tag`s persist, because
        // neither lives in the queue.
        if let Some(running) = self.running.take() {
            let interruptible = match &running.instruction.verb {
                Verb::Play { interruptible, .. } => *interruptible,
                // A gather's prompt is interruptible by definition (§6.2).
                Verb::GatherDigits(_) => true,
                _ => false,
            };
            if running.playing && interruptible {
                out.push(Output::Effect(Effect::StopPlayback));
            }
            if matches!(running.instruction.verb, Verb::GatherDigits(_)) {
                out.push(Output::ClearTimer(Timer::GatherOverall));
                out.push(Output::ClearTimer(Timer::GatherDigit));
            }
            if matches!(running.instruction.verb, Verb::Pause { .. }) {
                out.push(Output::ClearTimer(Timer::Pause));
            }
        }
        self.program.replace(document.instructions);
        self.advance(out);
    }

    /// §6.5: `dial.headers` may only set fields on a host-configured allowlist.
    fn check_allowlist(&self, document: &Document) -> Result<(), Error> {
        for instruction in &document.instructions {
            if let Verb::Dial { headers, .. } = &instruction.verb {
                for name in headers.keys() {
                    if !self
                        .policy
                        .dial_headers
                        .iter()
                        .any(|allowed| allowed.eq_ignore_ascii_case(name))
                    {
                        return Err(Error::BadField { field: "headers" });
                    }
                }
            }
        }
        Ok(())
    }

    /// §9.2, and the only place the interpreter decides anything about a failing app.
    fn fail(&mut self, at: Timestamp, failure: Failure, out: &mut Vec<Output>) {
        self.outstanding = None;
        self.apply_failure(self.policy.on(failure), out);
        self.drain(at, out);
    }

    fn apply_failure(&mut self, action: OnFailure, out: &mut Vec<Output>) {
        match action {
            // "keep program": the program goes on running, which is why nothing happens here.
            // A flapping app degrades a call it has already scripted; it does not kill it.
            OnFailure::Continue => self.advance(out),
            OnFailure::Hangup => self.tear_down(EndCause::Hangup, None, out),
            OnFailure::Reject { status } => {
                // A refusal only exists before the call is answered. Once it is, there is no
                // invitation left to refuse and the same declaration can only mean "end it".
                if matches!(
                    self.snapshot.state,
                    CallState::Incoming | CallState::Ringing
                ) {
                    self.tear_down(EndCause::Rejected { status }, Some(status), out);
                } else {
                    self.tear_down(EndCause::Hangup, None, out);
                }
            }
        }
    }

    /// End the call on the host's own initiative, discarding whatever was queued.
    fn tear_down(&mut self, cause: EndCause, reject: Option<u16>, out: &mut Vec<Output>) {
        self.program.abandon();
        if let Some(running) = self.running.take()
            && running.playing
        {
            out.push(Output::Effect(Effect::StopPlayback));
        }
        match reject {
            Some(status) => out.push(Output::Effect(Effect::Reject {
                status,
                reason: None,
            })),
            None => out.push(Output::Effect(Effect::HangUp { cause })),
        }
    }

    /// §6.3: an event that queued while a callback was outstanding is delivered once the response
    /// has been applied — and not before, so it meets the program the app just wrote rather than
    /// the one it replaced.
    fn drain(&mut self, at: Timestamp, out: &mut Vec<Output>) {
        while self.outstanding.is_none() {
            let Some(event) = self.queued.pop_front() else {
                return;
            };
            self.process(at, event, out);
        }
    }

    /// Deliver the terminal event that made the snapshot ended, dropping superseded queued events.
    ///
    /// Every event carries the current snapshot, so once that snapshot is ended an older queued
    /// event cannot be acted on usefully. Running it through [`Self::process`] could advance a
    /// program on a call that no longer exists. `call.ended` is the one event that may never be
    /// dropped, and becomes the next and final delivery.
    fn drain_ended(&mut self, at: Timestamp, out: &mut Vec<Output>) {
        while let Some(event) = self.queued.pop_front() {
            if matches!(event, EventKind::Ended { .. }) {
                self.deliver(at, event, out);
                return;
            }
        }
    }

    /// §2: events are authoritative, so the snapshot is rebuilt from each one as it arrives.
    fn apply_to_snapshot(&mut self, event: &EventKind) {
        match event {
            EventKind::Incoming => self.snapshot.state = CallState::Incoming,
            // Media began, but the INVITE is still provisional: the snapshot remains ringing
            // until the separate 2xx/ACK event confirms it.
            EventKind::Ringing { .. } | EventKind::EarlyMediaStarted => {
                self.snapshot.state = CallState::Ringing;
            }
            EventKind::Answered => self.snapshot.state = CallState::Answered,
            EventKind::Hold => self.snapshot.media.on_hold = true,
            EventKind::Resumed => self.snapshot.media.on_hold = false,
            EventKind::Bridged { .. } => self.snapshot.bridged = true,
            EventKind::Unbridged { .. } => self.snapshot.bridged = false,
            EventKind::Ended { .. } => self.snapshot.state = CallState::Ended,
            EventKind::DialFinished { leg, outcome, .. } => {
                // A leg that did not answer is not a leg of this call any more (vector AC-7). It
                // is removed rather than left in some `failed` state, because §5.2's `legs` is
                // "the other legs of this call" and a busy target is not one.
                if matches!(outcome, crate::event::DialOutcome::Answered) {
                    for existing in &mut self.snapshot.legs {
                        if existing.leg == *leg {
                            existing.state = CallState::Answered;
                        }
                    }
                } else {
                    self.snapshot.legs.retain(|existing| existing.leg != *leg);
                }
            }
            _ => {}
        }
    }
}
