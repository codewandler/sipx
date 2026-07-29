//! The contract's vector set, run under the deterministic harness (story `A-7`).
//!
//! The story's point in one line: these run **today**, before `C-3`'s event stream and `C-5`'s
//! interpreter exist, because the harness needs only the contract. When those land, the same
//! scenarios keep their expectations and the runner's instruction half is replaced underneath them.
//!
//! Nothing here touches a socket, a runtime or a clock — there is no `#[tokio::test]` in the file
//! and no way to write one that would help.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use sipx_app::harness::binding::{Binding, Outcome, Reply};
use sipx_app::harness::vectors;
use sipx_app::harness::{
    Conclusion, Document, EVENT_QUEUE, Effect, EndCause, Event, EventKind, Failure, FailurePolicy,
    Instruction, OnFailure, Scenario, Step, Verb, Virtual,
};

/// §11, every row. Each vector carries its own expectation, so a failure names itself.
#[test]
fn every_contract_vector_holds() {
    for vector in vectors::all() {
        if let Err(mismatch) = vector.check() {
            panic!("{mismatch}");
        }
    }
}

/// All nine, and not eight — a vector quietly dropped from `all()` would make the test above pass
/// by covering less.
#[test]
fn the_whole_vector_set_is_present() {
    assert_eq!(vectors::all().len(), 9, "AC-1 through AC-9");
}

/// Acceptance point 3: every §9.2 knob has a scenario, for every action it can declare.
#[test]
fn every_failure_knob_has_a_scenario_for_every_action() {
    let knobs = vectors::failure_knobs();
    assert_eq!(knobs.len(), 4 * 3, "four knobs, three actions each");

    for vector in knobs {
        if let Err(mismatch) = vector.check() {
            panic!("{mismatch}");
        }
    }
}

/// The four knobs are covered by name, so adding a fifth to §9.2 without a scenario fails here
/// rather than silently going untested.
#[test]
fn the_knob_scenarios_name_every_knob_the_policy_declares() {
    let names: Vec<String> = vectors::failure_knobs()
        .iter()
        .map(|vector| vector.scenario.name.clone())
        .collect();

    for failure in Failure::all() {
        assert!(
            names.iter().any(|name| name.contains(failure.knob())),
            "no scenario for {}: {names:?}",
            failure.knob()
        );
    }
}

/// A binding of the caller's own, which is how `A-2` and `A-4` reuse these rather than restating
/// them. This one is deliberately dumb — it always says the same thing — and the point is only
/// that the seam exists and carries a verdict.
struct AlwaysUnreachable;

impl Binding for AlwaysUnreachable {
    fn respond(&mut self, _event: &Event) -> Reply {
        Reply::unreachable()
    }
}

#[test]
fn a_foreign_binding_can_be_held_to_the_same_scenarios() {
    let vector = vectors::knob(Failure::Unreachable, &OnFailure::Hangup);
    let mut app = AlwaysUnreachable;
    if let Err(mismatch) = vector.check_against(&mut app) {
        panic!("{mismatch}");
    }

    // And the seam reports disagreement rather than swallowing it: this app does not do what
    // `on_5xx` scenarios expect, and saying so is the whole value of sharing them.
    let five = vectors::knob(Failure::ServerError, &OnFailure::Hangup);
    let mut app = AlwaysUnreachable;
    assert!(
        five.check_against(&mut app).is_err(),
        "a binding that fails differently than declared must not pass"
    );
}

/// The story's headline claim, stated as its own test: a two-second callback timeout costs no
/// wall-clock time. If any part of the harness ever reached for a real clock, this is what would
/// start taking seconds.
#[test]
fn a_scenario_spanning_a_minute_of_virtual_time_runs_instantly() {
    let started = std::time::Instant::now();

    let run = Scenario::new("a long, idle call")
        .policy(FailurePolicy::declared().with_timeout(Duration::from_secs(2)))
        .then(Reply::silent())
        .steps(vec![
            Step::event(0, EventKind::Incoming),
            Step::event(
                59_000,
                EventKind::Ended {
                    cause: EndCause::Remote,
                },
            ),
        ])
        .until(60_000)
        .run();

    assert_eq!(run.conclusion, Conclusion::Ended(EndCause::Remote));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "a minute of virtual time must not cost real time: {:?}",
        started.elapsed()
    );
}

/// §6.1: a verb with a completion event blocks the queue. Without this, `play` then `gather` would
/// start both at once and the gather would collect digits meant for nothing.
#[test]
fn a_blocking_verb_holds_the_queue_until_its_completion_event() {
    let run = Scenario::new("play blocks the gather behind it")
        .script(vec![Reply::now(Document::of(vec![
            Instruction::new(
                "p1",
                Verb::Play {
                    source: "welcome.wav".to_owned(),
                    interruptible: true,
                },
            ),
            Instruction::new(
                "g1",
                Verb::Gather {
                    max: 1,
                    terminators: String::new(),
                    timeout: Duration::from_secs(5),
                },
            ),
        ]))])
        .then(Reply::now(Document::keep_going()))
        .steps(vec![Step::event(0, EventKind::Incoming)])
        .until(1000)
        .run();

    assert_eq!(
        run.effects,
        vec![Effect::StartPlay {
            id: "p1".to_owned(),
            source: "welcome.wav".to_owned(),
        }],
        "the gather must not start while the play is running"
    );
}

/// §6.3's empty document: "keep going" changes nothing, rather than discarding what is queued.
/// The reading is recorded in the scenario module's docs; this is the test that pins it.
#[test]
fn an_empty_document_keeps_the_program_rather_than_clearing_it() {
    let run = Scenario::new("keep going")
        .script(vec![
            Reply::now(Document::of(vec![
                Instruction::new(
                    "p1",
                    Verb::Play {
                        source: "one.wav".to_owned(),
                        interruptible: true,
                    },
                ),
                Instruction::new(
                    "p2",
                    Verb::Play {
                        source: "two.wav".to_owned(),
                        interruptible: true,
                    },
                ),
            ])),
            // The answer to p1 finishing: keep going.
            Reply::now(Document::keep_going()),
        ])
        .then(Reply::now(Document::keep_going()))
        .steps(vec![
            Step::event(0, EventKind::Incoming),
            Step::event(
                500,
                EventKind::PlaybackFinished {
                    instruction_id: "p1".to_owned(),
                    completed: true,
                },
            ),
        ])
        .until(2000)
        .run();

    assert!(
        run.effects.contains(&Effect::StartPlay {
            id: "p2".to_owned(),
            source: "two.wav".to_owned(),
        }),
        "the queued p2 survived a keep-going: {:?}",
        run.effects
    );
}

/// The overflow policy's bound is real: an app that never answers cannot make the queue grow
/// without limit, which is what would turn a slow app into a memory leak.
#[test]
fn the_event_queue_is_bounded_rather_than_growing() {
    let mut steps = vec![Step::event(0, EventKind::Incoming)];
    for i in 0..50u64 {
        steps.push(Step::event(
            10 + i,
            EventKind::Dtmf {
                digit: '1',
                duration_ms: 80,
            },
        ));
    }

    let run = Scenario::new("a silent app under load")
        .policy(FailurePolicy::declared().with_timeout(Duration::from_secs(30)))
        .then(Reply::silent())
        .steps(steps)
        .until(1000)
        .run();

    assert!(
        run.dropped.len() >= 50 - EVENT_QUEUE,
        "the queue must be bounded, dropped {}",
        run.dropped.len()
    );
    assert!(
        !run.dropped.iter().any(EventKind::is_ended),
        "and never at the cost of call.ended"
    );
}

/// The clock has no `now()`, and that is the enforcement rather than a convention. This test is
/// really a claim about the API surface: `Virtual` is constructible only from the epoch or from an
/// explicit offset, so a scenario cannot smuggle real time in.
#[test]
fn the_only_clock_is_one_a_scenario_states() {
    assert_eq!(Virtual::epoch().millis(), 0);
    assert_eq!(
        Virtual::at_millis(1500).since(Virtual::epoch()),
        Duration::from_millis(1500)
    );
}

/// A 4xx is not a 5xx. §9.2 gives them separate knobs because they mean different things — the
/// request was wrong, versus the app was — and a host that collapsed them would make one
/// undeclarable.
#[test]
fn a_client_error_and_a_server_error_consult_different_knobs() {
    let policy = FailurePolicy::declared()
        .on_4xx(OnFailure::Hangup)
        .on_5xx(OnFailure::Continue);

    let four = Scenario::new("4xx")
        .policy(policy.clone())
        .then(Reply::failing(
            Duration::ZERO,
            Outcome::ClientError { status: 400 },
        ))
        .steps(vec![Step::event(0, EventKind::Incoming)])
        .until(1000)
        .run();

    let five = Scenario::new("5xx")
        .policy(policy)
        .then(Reply::failing(
            Duration::ZERO,
            Outcome::ServerError { status: 500 },
        ))
        .steps(vec![Step::event(0, EventKind::Incoming)])
        .until(1000)
        .run();

    assert_eq!(four.failures, vec![Failure::ClientError]);
    assert_eq!(four.conclusion, Conclusion::Ended(EndCause::Hangup));
    assert_eq!(five.failures, vec![Failure::ServerError]);
    assert_eq!(five.conclusion, Conclusion::Live, "5xx declared continue");
}
