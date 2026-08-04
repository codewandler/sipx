//! §11's vectors, one test per row.
//!
//! [`docs/specs/app-contract.md`](../../../docs/specs/app-contract.md) §11 says each row is a test
//! in this crate. These are those tests, named for their row, and they are the definition of
//! whether the interpreter implements the contract — `tests/spec_tables.rs` separately checks that
//! this file has a test for every row the section lists, so a vector added to the spec fails the
//! build until somebody writes it.
//!
//! Everything here runs with no I/O at all: no socket, no clock, no runtime. Time is a number
//! passed in, and the app is whatever the test says it answered.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_app_protocol::{
    CallSnapshot, CallState, Callback, DialOutcome, Direction, Document, Effect, EndCause,
    Envelope, EventKind, Failure, GatherReason, Input, Interpreter, OnFailure, Output, Policy,
    Response, Timer, Timestamp,
};

/// A fixed instant. The interpreter never asks what time it is, so the tests never have to move
/// it — which is itself the sans-IO property, observable.
fn now() -> Timestamp {
    Timestamp::from_unix_millis(1_772_270_104_221)
}

fn interpreter(policy: Policy) -> Interpreter {
    Interpreter::new(
        CallSnapshot::new("b7c1", Direction::Inbound)
            .between("sip:alice@example.com", "sip:support@example.net"),
        policy,
    )
}

/// The effects in a batch of outputs, in order.
fn effects(outputs: &[Output]) -> Vec<&Effect> {
    outputs
        .iter()
        .filter_map(|output| match output {
            Output::Effect(effect) => Some(effect),
            _ => None,
        })
        .collect()
}

/// The one delivery in a batch of outputs, and the token that answers it.
fn delivery(outputs: Vec<Output>) -> (Envelope, Callback) {
    let mut found = None;
    for output in outputs {
        if let Output::Deliver { envelope, callback } = output {
            assert!(found.is_none(), "§6.3: at most one callback outstanding");
            found = Some((*envelope, callback));
        }
    }
    found.expect("expected a delivery")
}

fn body(instructions: &str) -> String {
    format!(r#"{{"contract":"sipx.app.v1","instructions":[{instructions}]}}"#)
}

/// **AC-1** — `call.incoming` → app unreachable: after `timeout_ms`, the declared `on_unreachable`
/// effect and nothing else; no panic, no hang.
///
/// §9.2's last paragraph is what this pins: *a call with no program and no reachable app follows
/// the same declaration*. The failure mode it rules out is the obvious one — an interpreter with
/// no program loaded having nothing to do and therefore doing nothing, leaving a call up with
/// nobody driving it.
#[test]
fn ac_1_no_program_and_an_unreachable_app_takes_the_declared_effect() {
    let policy = Policy {
        on_unreachable: OnFailure::Hangup,
        ..Policy::default()
    };
    let mut interpreter = interpreter(policy);

    // The event arrives, the app is asked, and §9.2's clock is armed with the declared timeout.
    let outputs = interpreter.handle(now(), Input::Event(EventKind::Incoming));
    assert!(
        outputs.contains(&Output::SetTimer {
            timer: Timer::Callback,
            after_ms: 2_000,
        }),
        "the callback timer is armed from the policy: {outputs:?}"
    );
    let (envelope, callback) = delivery(outputs);
    assert_eq!(envelope.seq, 1, "§5.1: per-call `seq` starts at 1");
    assert_eq!(envelope.event, EventKind::Incoming);

    // The binding cannot reach the app at all.
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Failed(Failure::Unreachable),
        },
    );

    assert_eq!(
        effects(&outputs),
        vec![&Effect::HangUp {
            cause: EndCause::Hangup
        }],
        "§9.2: the declared `on_unreachable` effect, and nothing else"
    );
    assert!(
        !interpreter.awaiting_app(),
        "no hang: nothing is outstanding"
    );
    assert_eq!(interpreter.pending(), 0);
}

/// **AC-1**, the other half: under §9.2's *defaults* the same call degrades rather than dying.
///
/// `on_unreachable: continue` with no program is the case that most invites a panic — there is
/// nothing to continue *to* — so it is asserted rather than assumed.
#[test]
fn ac_1_the_default_declaration_continues_without_a_program_and_without_a_panic() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Failed(Failure::Unreachable),
        },
    );
    assert!(
        effects(&outputs).is_empty(),
        "`continue` keeps the program: {outputs:?}"
    );
    assert!(!interpreter.awaiting_app());
}

/// **AC-1**, the timeout path: `timeout_ms` elapsing is the same declaration by another name.
#[test]
fn ac_1_a_callback_that_never_returns_takes_the_declared_timeout_effect() {
    let policy = Policy {
        timeout_ms: 750,
        on_timeout: OnFailure::Reject { status: 503 },
        ..Policy::default()
    };
    let mut interpreter = interpreter(policy);
    let outputs = interpreter.handle(now(), Input::Event(EventKind::Incoming));
    assert!(outputs.contains(&Output::SetTimer {
        timer: Timer::Callback,
        after_ms: 750,
    }));
    let (_, _callback) = delivery(outputs);

    let outputs = interpreter.handle(now(), Input::TimerFired(Timer::Callback));
    assert_eq!(
        effects(&outputs),
        vec![&Effect::Reject {
            status: 503,
            reason: None
        }],
        "the call is still `incoming`, so a refusal is what `reject` can mean"
    );
}

/// **AC-2** — `call.incoming` → `answer, play(p1), gather(g1)`: effects in order;
/// `call.gather.finished` carries `instruction_id: "g1"`.
#[test]
fn ac_2_a_program_runs_in_order_and_its_gather_reports_its_own_id() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));

    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r##"{"id":"a1","do":"answer"},
                   {"id":"p1","do":"play","source":{"file":"welcome.wav"},"interruptible":true},
                   {"id":"g1","do":"gather","max":4,"terminators":"#","timeout_ms":10000}"##,
            )),
        },
    );
    // §6.1: strictly in order, and `answer` blocks until `call.answered`. So the first batch is
    // the answer alone — the play has not been reached yet.
    assert_eq!(effects(&outputs), vec![&Effect::Answer]);
    assert_eq!(interpreter.running(), Some("a1"));

    let outputs = interpreter.handle(now(), Input::Event(EventKind::Answered));
    assert_eq!(
        effects(&outputs),
        vec![&Effect::Play {
            instruction_id: "p1".to_owned(),
            source: sipx_app_protocol::Source::File("welcome.wav".to_owned()),
            interruptible: true,
        }]
    );
    let (envelope, callback) = delivery(outputs);
    assert_eq!(envelope.event, EventKind::Answered);
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(String::new()),
        },
    );
    assert!(effects(&outputs).is_empty(), "an empty body keeps going");

    // The playback runs out; the gather starts and arms its own timeout.
    let outputs = interpreter.handle(
        now(),
        Input::Event(EventKind::PlaybackFinished {
            instruction_id: "p1".to_owned(),
            completed: true,
        }),
    );
    assert!(outputs.contains(&Output::SetTimer {
        timer: Timer::GatherOverall,
        after_ms: 10_000,
    }));
    let (_, callback) = delivery(outputs);
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(String::new()),
        },
    );
    assert_eq!(interpreter.running(), Some("g1"));

    // Four digits, then the terminator. The gather resolves under its own id.
    let mut finished = None;
    for digit in ['1', '2', '3', '#'] {
        let outputs = interpreter.handle(
            now(),
            Input::Event(EventKind::Dtmf {
                digit,
                duration_ms: 160,
            }),
        );
        let (_, callback) = delivery(outputs);
        let outputs = interpreter.handle(
            now(),
            Input::Response {
                callback,
                response: Response::Body(String::new()),
            },
        );
        if let Some(Output::Deliver { envelope, .. }) = outputs
            .into_iter()
            .find(|o| matches!(o, Output::Deliver { .. }))
        {
            finished = Some(*envelope);
        }
    }
    let finished = finished.expect("the gather resolved");
    assert_eq!(
        finished.event,
        EventKind::GatherFinished {
            instruction_id: "g1".to_owned(),
            digits: "123".to_owned(),
            reason: GatherReason::Terminator,
        }
    );
}

/// **AC-3** — during AC-2's play, `call.dtmf` → `dial(d1)`: pending `gather` discarded, play
/// stopped, dial effect issued — **replacement, not append**.
#[test]
fn ac_3_a_document_replaces_the_program_rather_than_appending_to_it() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"p1","do":"play","source":{"file":"welcome.wav"},"interruptible":true},
                   {"id":"g1","do":"gather","max":4}"#,
            )),
        },
    );
    assert_eq!(interpreter.running(), Some("p1"));
    assert_eq!(
        interpreter.pending(),
        1,
        "the gather is queued behind the play"
    );

    // A keypress during the prompt: program-level barge-in.
    let outputs = interpreter.handle(
        now(),
        Input::Event(EventKind::Dtmf {
            digit: '5',
            duration_ms: 160,
        }),
    );
    let (_, callback) = delivery(outputs);
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"d1","do":"dial","target":"sip:bob@example.net"}"#,
            )),
        },
    );

    assert_eq!(
        effects(&outputs),
        vec![
            &Effect::StopPlayback,
            &Effect::Dial {
                instruction_id: "d1".to_owned(),
                leg: "b".to_owned(),
                target: "sip:bob@example.net".to_owned(),
                from: None,
                timeout_ms: None,
                headers: std::collections::BTreeMap::new(),
            },
        ],
        "the play is stopped and the dial issued"
    );
    assert_eq!(
        interpreter.pending(),
        0,
        "the queued gather is gone — replacement, not append"
    );
    assert_eq!(interpreter.running(), Some("d1"));
}

/// **AC-4** — redelivery of `seq: 3` answered differently: the second response is ignored and the
/// program is unchanged.
///
/// §7 says a document-mode retry repeats `seq`, so an app may legitimately answer the same
/// delivery twice. What the interpreter must never do is apply both. A correct driver cannot even
/// present the second, because [`Callback`] is not [`Clone`] and has no public constructor — so
/// the token here is *forged* through `testing::forge_callback`, which is the only way to write
/// this test at all and is itself the evidence that the rule is structural.
#[test]
fn ac_4_a_second_answer_to_one_delivery_is_ignored() {
    let mut interpreter = interpreter(Policy::default());
    // Get to seq 3: incoming (1), ringing (2), answered (3).
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(r#"{"id":"a1","do":"answer"}"#)),
        },
    );
    let (_, callback) =
        delivery(interpreter.handle(now(), Input::Event(EventKind::Ringing { reliable: false })));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(String::new()),
        },
    );
    let (envelope, callback) =
        delivery(interpreter.handle(now(), Input::Event(EventKind::Answered)));
    assert_eq!(envelope.seq, 3);
    assert_eq!(callback.seq(), 3);

    // The first answer is applied.
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"p1","do":"play","source":{"file":"one.wav"}}"#,
            )),
        },
    );
    assert_eq!(interpreter.running(), Some("p1"));

    // The redelivery is answered differently. It changes nothing.
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback: sipx_app_protocol::testing::forge_callback(3),
            response: Response::Body(body(r#"{"id":"h1","do":"hangup"}"#)),
        },
    );
    assert!(
        outputs.is_empty(),
        "the second response does nothing: {outputs:?}"
    );
    assert_eq!(interpreter.running(), Some("p1"), "program unchanged");
    assert_eq!(interpreter.pending(), 0);
}

/// **AC-5** — a document naming the unknown verb `spindle`: rejected whole, §9.2 applied as a
/// `5xx`, and the prior program still runs.
#[test]
fn ac_5_an_unknown_verb_rejects_the_document_whole_and_leaves_the_program_running() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"p1","do":"play","source":{"file":"one.wav"}},
                   {"id":"p2","do":"play","source":{"file":"two.wav"}}"#,
            )),
        },
    );
    assert_eq!(interpreter.running(), Some("p1"));
    assert_eq!(interpreter.pending(), 1);

    let (_, callback) = delivery(interpreter.handle(
        now(),
        Input::Event(EventKind::Dtmf {
            digit: '5',
            duration_ms: 160,
        }),
    ));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            // Two good instructions around one bad one: the good ones must not run either.
            response: Response::Body(body(
                r#"{"id":"a","do":"answer"},{"id":"s","do":"spindle"},{"id":"h","do":"hangup"}"#,
            )),
        },
    );

    assert!(
        effects(&outputs).is_empty(),
        "§6.4: no partial application — not even the `answer`: {outputs:?}"
    );
    assert_eq!(
        interpreter.running(),
        Some("p1"),
        "§9.2 default `on_5xx: continue` — the prior program still runs"
    );
    assert_eq!(
        interpreter.pending(),
        1,
        "and so does what was queued behind it"
    );
}

/// **AC-6** — a `gather` with no digits until `timeout_ms`:
/// `call.gather.finished{digits: "", reason: "timeout"}`.
#[test]
fn ac_6_a_silent_gather_times_out_with_no_digits() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r##"{"id":"g1","do":"gather","max":4,"terminators":"#","timeout_ms":10000}"##,
            )),
        },
    );
    assert!(outputs.contains(&Output::SetTimer {
        timer: Timer::GatherOverall,
        after_ms: 10_000,
    }));

    // Nothing is pressed; time enters as a fired timer and nothing else.
    let outputs = interpreter.handle(now(), Input::TimerFired(Timer::GatherOverall));
    let (envelope, _callback) = delivery(outputs);
    assert_eq!(
        envelope.event,
        EventKind::GatherFinished {
            instruction_id: "g1".to_owned(),
            digits: String::new(),
            reason: GatherReason::Timeout,
        }
    );
    assert_eq!(interpreter.running(), None);
}

/// **AC-7** — a `dial` refused with 486: `call.dial.finished{outcome: busy}`, and the snapshot's
/// `legs` no longer lists the leg.
#[test]
fn ac_7_a_busy_dial_removes_the_leg_from_the_snapshot() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"d1","do":"dial","target":"sip:bob@example.net"}"#,
            )),
        },
    );
    assert_eq!(
        interpreter.snapshot().legs.len(),
        1,
        "the dial put a leg in the snapshot"
    );
    assert_eq!(interpreter.snapshot().legs[0].leg, "b");

    let outputs = interpreter.handle(
        now(),
        Input::Event(EventKind::DialFinished {
            instruction_id: "d1".to_owned(),
            leg: "b".to_owned(),
            outcome: DialOutcome::Busy,
        }),
    );
    let (envelope, _callback) = delivery(outputs);
    assert!(
        envelope.call.legs.is_empty(),
        "the snapshot the app receives no longer lists the leg: {:?}",
        envelope.call.legs
    );
    assert!(interpreter.snapshot().legs.is_empty());
    assert_eq!(interpreter.running(), None, "the dial resolved");
}

/// **AC-8** — a `call.dtmf` firing while a callback is outstanding: delivered after the response
/// is applied, `seq` in order, snapshot current.
#[test]
fn ac_8_an_event_during_an_outstanding_callback_waits_its_turn() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(r#"{"id":"a1","do":"answer"}"#)),
        },
    );
    // `call.answered` goes out and the app has not answered yet.
    let (first, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Answered)));
    assert_eq!(first.seq, 2);
    assert!(interpreter.awaiting_app());

    // A keypress arrives meanwhile. Nothing is delivered — but the snapshot moves anyway.
    let outputs = interpreter.handle(now(), Input::Event(EventKind::Hold));
    assert!(
        !outputs.iter().any(|o| matches!(o, Output::Deliver { .. })),
        "§6.3: at most one callback outstanding: {outputs:?}"
    );
    assert!(
        interpreter.snapshot().media.on_hold,
        "§6.3: a snapshot always reflects *now*, not the queue's past"
    );

    // The response is applied, and only then is the queued event delivered.
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(String::new()),
        },
    );
    let (second, _callback) = delivery(outputs);
    assert_eq!(second.seq, 3, "`seq` in order");
    assert_eq!(second.event, EventKind::Hold);
    assert!(second.call.media.on_hold, "the snapshot is current");
}

/// **AC-9** — `call.ended` under a full event queue: still delivered. Whatever the overflow policy
/// drops, it is never `call.ended`.
#[test]
fn ac_9_call_ended_survives_a_full_event_queue() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(r#"{"id":"a1","do":"answer"}"#)),
        },
    );
    // One delivery goes out and is left unanswered, so everything after it queues.
    let (_, held) = delivery(interpreter.handle(now(), Input::Event(EventKind::Answered)));

    // Flood the queue well past its bound.
    for _ in 0..(sipx_app_protocol::MAX_QUEUED_EVENTS * 4) {
        interpreter.handle(
            now(),
            Input::Event(EventKind::Dtmf {
                digit: '1',
                duration_ms: 160,
            }),
        );
    }
    // And then the one event that may never be dropped.
    let terminal = interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Remote,
        }),
    );
    assert!(
        !terminal
            .iter()
            .any(|output| matches!(output, Output::Deliver { .. }))
    );
    let state = interpreter.snapshot().state;
    interpreter.handle(now(), Input::Event(EventKind::Answered));
    assert_eq!(
        interpreter.snapshot().state,
        state,
        "late events cannot resurrect the snapshot"
    );
    let terminal = interpreter.handle(
        now(),
        Input::Response {
            callback: held,
            response: Response::Body(String::new()),
        },
    );
    let (ended, _callback) = delivery(terminal);
    assert!(
        matches!(
            ended.event,
            EventKind::Ended {
                cause: EndCause::Remote
            }
        ),
        "`call.ended` supersedes the full queue"
    );
}

/// A response to `call.ended` cannot act on the call again. The deterministic host harness used
/// to carry this guard in its second interpreter; keeping the proof here makes the sole
/// interpreter own it for every binding.
#[test]
fn a_failed_answer_to_call_ended_cannot_hang_up_twice() {
    let policy = Policy {
        on_unreachable: OnFailure::Hangup,
        ..Policy::default()
    };
    let mut interpreter = interpreter(policy);
    let (_, incoming) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback: incoming,
            response: Response::Failed(Failure::Unreachable),
        },
    );
    assert_eq!(
        effects(&outputs),
        vec![&Effect::HangUp {
            cause: EndCause::Hangup
        }]
    );

    let (ended, callback) = delivery(interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Hangup,
        }),
    ));
    assert!(matches!(ended.event, EventKind::Ended { .. }));

    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Failed(Failure::Unreachable),
        },
    );
    assert!(
        effects(&outputs).is_empty(),
        "an ended call has no second failure action: {outputs:?}"
    );
}

#[test]
fn a_timeout_waiting_for_call_ended_s_answer_cannot_hang_up_twice() {
    let policy = Policy {
        on_timeout: OnFailure::Hangup,
        ..Policy::default()
    };
    let mut interpreter = interpreter(policy);
    let (_ended, _callback) = delivery(interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Remote,
        }),
    ));

    let outputs = interpreter.handle(now(), Input::TimerFired(Timer::Callback));
    assert!(
        effects(&outputs).is_empty(),
        "an ended call has no timeout action: {outputs:?}"
    );
}

#[test]
fn ended_spends_the_prior_callback_before_a_failure_can_tear_down_again() {
    let policy = Policy {
        on_unreachable: OnFailure::Hangup,
        ..Policy::default()
    };
    let mut interpreter = interpreter(policy);
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let terminal = interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Remote,
        }),
    );
    assert!(
        !terminal
            .iter()
            .any(|output| matches!(output, Output::Deliver { .. }))
    );

    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Failed(Failure::Unreachable),
        },
    );
    assert!(
        effects(&outputs).is_empty(),
        "the ended snapshot suppresses teardown: {outputs:?}"
    );
    let (ended, _terminal_callback) = delivery(outputs);
    assert!(matches!(ended.event, EventKind::Ended { .. }));
}

#[test]
fn ended_clears_the_prior_callback_timer_without_timeout_teardown() {
    let policy = Policy {
        on_timeout: OnFailure::Hangup,
        ..Policy::default()
    };
    let mut interpreter = interpreter(policy);
    let (_, _callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let terminal = interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Remote,
        }),
    );
    assert!(
        !terminal
            .iter()
            .any(|output| matches!(output, Output::Deliver { .. }))
    );

    let outputs = interpreter.handle(now(), Input::TimerFired(Timer::Callback));
    let (_ended, _callback) = delivery(outputs);
    assert!(
        interpreter.snapshot().state == CallState::Ended,
        "the ended snapshot suppresses timeout teardown"
    );
}

#[test]
fn ended_abandons_the_program_instead_of_advancing_hangup_to_play() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"h1","do":"hangup"},{"id":"p1","do":"play","source":{"file":"late.wav"}}"#,
            )),
        },
    );
    assert!(matches!(
        effects(&outputs).as_slice(),
        [Effect::HangUp { .. }]
    ));

    let outputs = interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Hangup,
        }),
    );
    assert!(
        !effects(&outputs)
            .iter()
            .any(|effect| matches!(effect, Effect::Play { .. })),
        "terminal input never advances to the queued play: {outputs:?}"
    );
    assert_eq!(interpreter.pending(), 0);
    assert!(interpreter.running().is_none());
}

#[test]
fn ended_clears_pause_and_all_later_inputs_are_effect_free() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"w1","do":"pause","ms":500},{"id":"h1","do":"hangup"}"#,
            )),
        },
    );
    let (_, held) = delivery(interpreter.handle(
        now(),
        Input::Event(EventKind::Dtmf {
            digit: '1',
            duration_ms: 80,
        }),
    ));

    let terminal = interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Remote,
        }),
    );
    assert!(terminal.contains(&Output::ClearTimer(Timer::Pause)));
    assert!(!terminal.contains(&Output::ClearTimer(Timer::Callback)));
    let terminal = interpreter.handle(
        now(),
        Input::Response {
            callback: held,
            response: Response::Body(String::new()),
        },
    );
    assert!(terminal.contains(&Output::ClearTimer(Timer::Callback)));
    let (_ended, callback) = delivery(terminal);
    for outputs in [
        interpreter.handle(now(), Input::TimerFired(Timer::Pause)),
        interpreter.handle(
            now(),
            Input::Event(EventKind::Dtmf {
                digit: '2',
                duration_ms: 80,
            }),
        ),
        interpreter.handle(
            now(),
            Input::Response {
                callback,
                response: Response::Body(body(r#"{"id":"h2","do":"hangup"}"#)),
            },
        ),
    ] {
        assert!(
            effects(&outputs).is_empty(),
            "no input after terminal state has a call effect: {outputs:?}"
        );
    }
}

#[test]
fn ended_clears_gather_and_callback_timers() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r##"{"id":"g1","do":"gather","max":4,"terminators":"#","timeout_ms":10000}"##,
            )),
        },
    );
    assert!(outputs.contains(&Output::SetTimer {
        timer: Timer::GatherOverall,
        after_ms: 10_000,
    }));
    let outputs = interpreter.handle(
        now(),
        Input::Event(EventKind::Dtmf {
            digit: '1',
            duration_ms: 80,
        }),
    );
    let (_, callback) = delivery(outputs);

    let terminal = interpreter.handle(
        now(),
        Input::Event(EventKind::Ended {
            cause: EndCause::Remote,
        }),
    );
    for timer in [Timer::GatherOverall, Timer::GatherDigit] {
        assert!(
            terminal.contains(&Output::ClearTimer(timer)),
            "terminal input clears {timer:?}: {terminal:?}"
        );
    }
    let terminal = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(String::new()),
        },
    );
    assert!(terminal.contains(&Output::ClearTimer(Timer::Callback)));
    let (_ended, _callback) = delivery(terminal);
}

/// §6.3's other clause, on its own: **at most one callback is outstanding per call**.
///
/// The type system carries this — a [`Callback`] cannot be cloned and cannot be built — so what a
/// test can add is that the interpreter never *hands out* a second one while one is unanswered.
#[test]
fn the_interpreter_never_issues_a_second_callback_while_one_is_out() {
    let mut interpreter = interpreter(Policy::default());
    let (_, _held) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    for event in [
        EventKind::Ringing { reliable: true },
        EventKind::Answered,
        EventKind::Hold,
        EventKind::Resumed,
    ] {
        let outputs = interpreter.handle(now(), Input::Event(event));
        assert!(
            !outputs.iter().any(|o| matches!(o, Output::Deliver { .. })),
            "a second callback was issued: {outputs:?}"
        );
    }
}

/// §6.5: a `dial` may only set header fields the host allows, and one that does not is a document
/// rejected whole (§6.4) rather than a header quietly dropped.
#[test]
fn a_dial_header_outside_the_host_allowlist_rejects_the_document() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"d1","do":"dial","target":"sip:bob@example.net","headers":{"Route":"<sip:evil@example.net;lr>"}}"#,
            )),
        },
    );
    assert!(
        effects(&outputs).is_empty(),
        "no dial was issued: {outputs:?}"
    );
    assert_eq!(interpreter.pending(), 0);

    // With the field allowed, the same document runs.
    let policy = Policy {
        dial_headers: vec!["X-Campaign".to_owned()],
        ..Policy::default()
    };
    let mut allowed = Interpreter::new(CallSnapshot::new("b7c1", Direction::Inbound), policy);
    let (_, callback) = delivery(allowed.handle(now(), Input::Event(EventKind::Incoming)));
    let outputs = allowed.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"d1","do":"dial","target":"sip:bob@example.net","headers":{"x-campaign":"renewal"}}"#,
            )),
        },
    );
    assert_eq!(
        effects(&outputs).len(),
        1,
        "the allowed field is fine: {outputs:?}"
    );
}

/// AGENTS.md non-negotiable 3, at the interpreter's own boundary: an instruction document is input
/// this process did not produce, and no body may panic or hang the machine.
#[test]
fn nothing_an_app_can_answer_with_panics() {
    for hostile in sipx_app_protocol::testing::HOSTILE_BODIES {
        let mut interpreter = interpreter(Policy::default());
        let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
        let _ = interpreter.handle(
            now(),
            Input::Response {
                callback,
                response: Response::Body((*hostile).to_owned()),
            },
        );
        assert!(
            !interpreter.awaiting_app(),
            "no hang after {hostile:?}: the callback is always resolved"
        );
    }
}

/// §6.3: an empty `instructions` array is "keep going" — the one document that does not replace.
#[test]
fn an_empty_document_keeps_the_program_rather_than_clearing_it() {
    let mut interpreter = interpreter(Policy::default());
    let (_, callback) = delivery(interpreter.handle(now(), Input::Event(EventKind::Incoming)));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Body(body(
                r#"{"id":"p1","do":"play","source":{"file":"one.wav"}},
                   {"id":"p2","do":"play","source":{"file":"two.wav"}}"#,
            )),
        },
    );
    let (_, callback) = delivery(interpreter.handle(
        now(),
        Input::Event(EventKind::Dtmf {
            digit: '5',
            duration_ms: 160,
        }),
    ));
    interpreter.handle(
        now(),
        Input::Response {
            callback,
            response: Response::Document(Document::keep_going()),
        },
    );
    assert_eq!(
        interpreter.running(),
        Some("p1"),
        "the program is untouched"
    );
    assert_eq!(interpreter.pending(), 1);
}
