//! The contract's own vector set, as scenarios.
//!
//! [`specs/app-contract.md`](../../../../docs/specs/app-contract.md) §11 lists `AC-1` … `AC-9` and
//! says each row is a test. Here they are, expressed against the harness — which is
//! [story `A-7`](../../../../docs/stories/A-7-deterministic-harness.md)'s point: the vectors are
//! runnable **before** `C-3` and `C-5` land, so the contract's meaning is pinned before the code
//! that has to honour it exists.
//!
//! Below them, [`failure_knobs`] gives every §9.2 knob a scenario per declared action. Those are
//! the ones `A-2` and `A-4` are meant to *share* rather than restate: a binding proves it honours
//! the declaration by running these through [`Vector::check_against`], not by writing its own.

use std::collections::BTreeMap;
use std::time::Duration;

use super::binding::{Outcome, Reply};
use super::contract::{
    DialOutcome, Document, Effect, EndCause, EventKind, Gather, GatherReason, Instruction, Source,
    Verb,
};
use super::policy::{Failure, FailurePolicy, OnFailure};
use super::scenario::{Conclusion, Expectation, Scenario, Step, Vector};

/// `answer, play(p1), gather(g1)` — AC-2's program, and the one AC-3 starts from.
fn answer_play_gather() -> Document {
    Document::new(vec![
        Instruction::new("a1", Verb::Answer),
        Instruction::new(
            "p1",
            Verb::Play {
                source: Source::File("welcome.wav".to_owned()),
                interruptible: true,
            },
        ),
        Instruction::new(
            "g1",
            Verb::GatherDigits(Gather {
                min: 0,
                max: Some(4),
                terminators: "#".to_owned(),
                digit_timeout_ms: None,
                timeout_ms: Some(10_000),
                prompt: None,
            }),
        ),
    ])
}

fn keep_going() -> Reply {
    Reply::now(Document::keep_going())
}

fn play(id: &str, source: &str) -> Instruction {
    Instruction::new(
        id,
        Verb::Play {
            source: Source::File(source.to_owned()),
            interruptible: true,
        },
    )
}

fn dtmf(digit: char) -> EventKind {
    EventKind::Dtmf {
        digit,
        duration_ms: 100,
    }
}

fn played(id: &str) -> EventKind {
    EventKind::PlaybackFinished {
        instruction_id: id.to_owned(),
        completed: true,
    }
}

/// AC-1 — `call.incoming` → app unreachable.
///
/// *"After `timeout_ms`, the declared `on_unreachable` effect and nothing else; no panic, no
/// hang."* The declaration is `hangup` here so the effect is observable at all; under §9.2's
/// default of `continue` the correct output is no effect whatever, which [`failure_knobs`] covers.
///
/// One note on the wording. The harness answers `Unreachable` immediately rather than after
/// `timeout_ms`, because §9.2 gives "absent" and "slow" separate knobs and a host that could not
/// tell them apart could not honour both. Which one a given connect failure reports is `A-2`'s to
/// decide; the vector's real claim — the declared effect, nothing else, and termination — is what
/// is checked here.
#[must_use]
pub fn ac_1() -> Vector {
    Vector::new(
        Scenario::new("AC-1 unreachable app")
            .policy(FailurePolicy::declared().on_unreachable(OnFailure::Hangup))
            .then(Reply::unreachable())
            .steps(vec![Step::event(0, EventKind::Incoming)]),
        Expectation::new()
            .effects(vec![Effect::Hangup {
                cause: EndCause::Hangup,
            }])
            .failures(vec![Failure::Unreachable])
            .conclusion(Conclusion::Ended(EndCause::Hangup))
            .app_saw_ended(true),
    )
}

/// AC-2 — `call.incoming` → ← `answer, play(p1), gather(g1)`.
///
/// *"Effects in order; `call.gather.finished` carries `instruction_id: "g1"`."* The gather is
/// reached only because the play blocked the queue until its completion event arrived (§6.1), and
/// the digits then resolve it by reaching `max`.
#[must_use]
pub fn ac_2() -> Vector {
    Vector::new(
        Scenario::new("AC-2 answer, play, gather")
            .script(vec![Reply::now(answer_play_gather())])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, EventKind::Answered),
                Step::event(1000, played("p1")),
                Step::event(1500, dtmf('1')),
                Step::event(1600, dtmf('2')),
                Step::event(1700, dtmf('3')),
                Step::event(1800, dtmf('4')),
            ]),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::StartPlay {
                    id: "p1".to_owned(),
                    source: "welcome.wav".to_owned(),
                },
                Effect::StartGather {
                    id: "g1".to_owned(),
                },
            ])
            .delivered_event(EventKind::GatherFinished {
                instruction_id: "g1".to_owned(),
                digits: "1234".to_owned(),
                reason: GatherReason::Max,
            })
            .conclusion(Conclusion::Live),
    )
}

/// AC-3 — during AC-2's play: `call.dtmf` → ← `dial(d1)`.
///
/// *"Pending `gather` discarded, play stopped, dial effect issued — replacement, not append."* The
/// digit arrives while the play is running and no gather is, so it reaches the app as `call.dtmf`;
/// the document it answers with replaces the whole program (§6.3). `StartGather` never happening is
/// the assertion that matters most, and is stated as an explicit absence rather than left to be
/// inferred from the effect list.
#[must_use]
pub fn ac_3() -> Vector {
    Vector::new(
        Scenario::new("AC-3 barge-in replaces the program")
            .script(vec![
                Reply::now(answer_play_gather()),
                keep_going(),
                Reply::now(Document::new(vec![Instruction::new(
                    "d1",
                    Verb::Dial {
                        target: "sip:bob@example.net".to_owned(),
                        from: None,
                        timeout_ms: Some(30_000),
                        headers: BTreeMap::default(),
                    },
                )])),
            ])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, EventKind::Answered),
                Step::event(500, dtmf('9')),
            ]),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::StartPlay {
                    id: "p1".to_owned(),
                    source: "welcome.wav".to_owned(),
                },
                Effect::StopPlay {
                    id: "p1".to_owned(),
                },
                Effect::Dial {
                    id: "d1".to_owned(),
                    target: "sip:bob@example.net".to_owned(),
                },
            ])
            .without(vec![Effect::StartGather {
                id: "g1".to_owned(),
            }])
            .conclusion(Conclusion::Live),
    )
}

/// AC-4 — redelivery of `seq: 3` answered differently.
///
/// *"Second response ignored; program unchanged."* The app is entitled to answer a retry
/// differently — it may have restarted and forgotten what it said. The host is required not to
/// care: `seq` 3 was settled the first time, so the `hangup` in the second answer changes nothing
/// and the call is still up.
#[must_use]
pub fn ac_4() -> Vector {
    Vector::new(
        Scenario::new("AC-4 a redelivery answered differently")
            .script(vec![
                Reply::now(Document::new(vec![
                    Instruction::new("a1", Verb::Answer),
                    play("p1", "welcome.wav"),
                ])),
                keep_going(),
                keep_going(),
                Reply::now(Document::new(vec![play("p2", "menu.wav")])),
                // The answer to the redelivery: a different program entirely, and inert.
                Reply::now(Document::new(vec![Instruction::new(
                    "h1",
                    Verb::Hangup {
                        cause: EndCause::Hangup,
                    },
                )])),
            ])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, EventKind::Answered),
                Step::event(1000, played("p1")),
                Step::event(2000, dtmf('5')),
                Step::redeliver(3000, 3),
            ]),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::StartPlay {
                    id: "p1".to_owned(),
                    source: "welcome.wav".to_owned(),
                },
                Effect::StartPlay {
                    id: "p2".to_owned(),
                    source: "menu.wav".to_owned(),
                },
            ])
            .without(vec![Effect::Hangup {
                cause: EndCause::Hangup,
            }])
            .conclusion(Conclusion::Live),
    )
}

/// AC-5 — document names unknown verb `spindle`.
///
/// *"Rejected whole; §9.2 as 5xx; prior program still runs."* The prior program has a second play
/// queued behind the first precisely so "still runs" is observable: `p2` starting is the proof that
/// the rejection tore nothing down.
#[must_use]
pub fn ac_5() -> Vector {
    Vector::new(
        Scenario::new("AC-5 unknown verb rejects the document whole")
            .script(vec![
                Reply::now(Document::new(vec![
                    Instruction::new("a1", Verb::Answer),
                    play("p1", "welcome.wav"),
                    play("p2", "menu.wav"),
                ])),
                keep_going(),
                Reply::body(
                    r#"{"contract":"sipx.app.v1","instructions":[{"id":"x1","do":"spindle"}]}"#,
                ),
            ])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, EventKind::Answered),
                Step::event(1000, played("p1")),
            ]),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::StartPlay {
                    id: "p1".to_owned(),
                    source: "welcome.wav".to_owned(),
                },
                Effect::StartPlay {
                    id: "p2".to_owned(),
                    source: "menu.wav".to_owned(),
                },
            ])
            .conclusion(Conclusion::Live),
    )
}

/// AC-6 — `gather` with no digits until `timeout_ms`.
///
/// *"`call.gather.finished{digits: "", reason: "timeout"}`."* Nothing in the scenario produces that
/// event: the harness's own timer does, five virtual seconds in and no real ones, which is what
/// makes a timeout an ordinary assertion rather than a slow test.
#[must_use]
pub fn ac_6() -> Vector {
    Vector::new(
        Scenario::new("AC-6 a gather that times out")
            .script(vec![
                Reply::now(Document::new(vec![
                    Instruction::new("a1", Verb::Answer),
                    Instruction::new(
                        "g1",
                        Verb::GatherDigits(Gather {
                            min: 0,
                            max: Some(4),
                            terminators: "#".to_owned(),
                            digit_timeout_ms: None,
                            timeout_ms: Some(5_000),
                            prompt: None,
                        }),
                    ),
                ])),
                keep_going(),
            ])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, EventKind::Answered),
            ])
            .until(10_000),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::StartGather {
                    id: "g1".to_owned(),
                },
            ])
            .delivered_event(EventKind::GatherFinished {
                instruction_id: "g1".to_owned(),
                digits: String::new(),
                reason: GatherReason::Timeout,
            })
            .conclusion(Conclusion::Live),
    )
}

/// AC-7 — `dial` refused with 486.
///
/// *"`call.dial.finished{outcome: busy}`; snapshot's `legs` no longer lists the leg."* The second
/// half is the one worth a test: an app that read `legs` after a refusal and still found the leg
/// there would bridge to something that does not exist.
#[must_use]
pub fn ac_7() -> Vector {
    Vector::new(
        Scenario::new("AC-7 a dial refused with 486")
            .script(vec![
                Reply::now(Document::new(vec![
                    Instruction::new("a1", Verb::Answer),
                    Instruction::new(
                        "d1",
                        Verb::Dial {
                            target: "sip:bob@example.net".to_owned(),
                            from: None,
                            timeout_ms: Some(30_000),
                            headers: BTreeMap::default(),
                        },
                    ),
                ])),
                keep_going(),
            ])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, EventKind::Answered),
                Step::event(
                    2000,
                    EventKind::DialFinished {
                        instruction_id: "d1".to_owned(),
                        leg: "b".to_owned(),
                        outcome: DialOutcome::Busy,
                    },
                ),
            ]),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::Dial {
                    id: "d1".to_owned(),
                    target: "sip:bob@example.net".to_owned(),
                },
            ])
            .legs(vec![])
            .delivered_event(EventKind::DialFinished {
                instruction_id: "d1".to_owned(),
                leg: "b".to_owned(),
                outcome: DialOutcome::Busy,
            })
            .conclusion(Conclusion::Live),
    )
}

/// AC-8 — `call.dtmf` fires while AC-2's callback is outstanding.
///
/// *"Delivered after the response is applied, `seq` in order."* The app takes 300 ms to answer the
/// incoming event; the digit arrives at 100 ms and has to wait, because §6.3 allows at most one
/// outstanding callback per call. Both are delivered, in `seq` order, and neither is lost.
#[must_use]
pub fn ac_8() -> Vector {
    Vector::new(
        Scenario::new("AC-8 an event arriving mid-callback")
            .script(vec![
                Reply::after(
                    Duration::from_millis(300),
                    Document::new(vec![
                        Instruction::new("a1", Verb::Answer),
                        play("p1", "welcome.wav"),
                    ]),
                ),
                keep_going(),
                keep_going(),
            ])
            .then(keep_going())
            .steps(vec![
                Step::event(0, EventKind::Incoming),
                Step::event(100, dtmf('7')),
                Step::event(400, EventKind::Answered),
            ]),
        Expectation::new()
            .effects(vec![
                Effect::Answer,
                Effect::StartPlay {
                    id: "p1".to_owned(),
                    source: "welcome.wav".to_owned(),
                },
            ])
            .delivered_seqs(vec![1, 2, 3])
            .conclusion(Conclusion::Live),
    )
}

/// AC-9 — `call.ended` under a full event queue.
///
/// *"Still delivered; whatever the overflow policy drops, it is never `call.ended`."* The app holds
/// its callback for five virtual seconds while twenty digits arrive; the queue is bounded, so most
/// are discarded. The call then ends, and that one event is delivered regardless — it rides the
/// slot reserved for it before any ordinary event could claim it.
#[must_use]
pub fn ac_9() -> Vector {
    let mut steps = vec![Step::event(0, EventKind::Incoming)];
    for i in 0..u64::try_from(super::scenario::EVENT_QUEUE * 2).unwrap_or(u64::MAX) {
        steps.push(Step::event(100 + i * 10, dtmf('1')));
    }
    steps.push(Step::event(
        3000,
        EventKind::Ended {
            cause: EndCause::Remote,
        },
    ));

    Vector::new(
        Scenario::new("AC-9 call.ended under a full queue")
            .script(vec![Reply::after(
                Duration::from_secs(5),
                Document::new(vec![Instruction::new("a1", Verb::Answer)]),
            )])
            .then(keep_going())
            // A callback timeout longer than the app's own delay, so this vector is about the
            // queue rather than accidentally about `on_timeout`.
            .policy(FailurePolicy::declared().with_timeout(Duration::from_secs(30)))
            .steps(steps)
            .until(60_000),
        Expectation::new()
            .dropped_any(true)
            .app_saw_ended(true)
            .conclusion(Conclusion::Ended(EndCause::Remote)),
    )
}

/// Every vector in §11, in order.
#[must_use]
pub fn all() -> Vec<Vector> {
    vec![
        ac_1(),
        ac_2(),
        ac_3(),
        ac_4(),
        ac_5(),
        ac_6(),
        ac_7(),
        ac_8(),
        ac_9(),
    ]
}

/// One scenario per §9.2 knob per declared action — twelve in all.
///
/// **These are the ones `A-2` and `A-4` share.** A binding does not restate what `on_5xx: hangup`
/// means; it runs these through [`Vector::check_against`] with its own adapter and either agrees or
/// does not. Ground rule 3 says failure semantics are configuration rather than code, and a second
/// hand-written copy per binding is exactly how that stops being true.
#[must_use]
pub fn failure_knobs() -> Vec<Vector> {
    let mut vectors = Vec::new();
    for failure in Failure::all() {
        for action in [
            OnFailure::Continue,
            OnFailure::Hangup,
            OnFailure::Reject { status: 503 },
        ] {
            vectors.push(knob(failure, &action));
        }
    }
    vectors
}

/// One knob, one declared action.
#[must_use]
pub fn knob(failure: Failure, action: &OnFailure) -> Vector {
    let policy = FailurePolicy::declared().with_timeout(Duration::from_millis(200));
    let policy = match failure {
        Failure::Timeout => policy.on_timeout(action.clone()),
        Failure::ServerError => policy.on_5xx(action.clone()),
        Failure::Unreachable => policy.on_unreachable(action.clone()),
        Failure::ClientError => policy.on_4xx(action.clone()),
    };

    // How an app expresses each failure. The timeout case is the only one that needs the clock, and
    // it needs no real time: the callback timer fires 200 virtual milliseconds in.
    let reply = match failure {
        Failure::Timeout => Reply::silent(),
        Failure::ServerError => {
            Reply::failing(Duration::ZERO, Outcome::ServerError { status: 500 })
        }
        Failure::Unreachable => Reply::unreachable(),
        Failure::ClientError => {
            Reply::failing(Duration::ZERO, Outcome::ClientError { status: 400 })
        }
    };

    let (effects, conclusion) = match action {
        OnFailure::Continue => (vec![], Conclusion::Live),
        OnFailure::Hangup => (
            vec![Effect::Hangup {
                cause: EndCause::Hangup,
            }],
            Conclusion::Ended(EndCause::Hangup),
        ),
        OnFailure::Reject { status } => (
            vec![Effect::Reject { status: *status }],
            Conclusion::Ended(EndCause::Rejected { status: *status }),
        ),
    };

    Vector::new(
        Scenario::new(format!("§9.2 {} = {action:?}", failure.knob()))
            .policy(policy)
            .then(reply)
            .steps(vec![Step::event(0, EventKind::Incoming)])
            .until(5000),
        Expectation::new()
            .effects(effects)
            .failures(vec![failure])
            .conclusion(conclusion),
    )
}
