//! The contract, end to end, over a real call — with no host anywhere.
//!
//! ```text
//! cargo run --example canned_program --features call
//! tests/canned_program.sh          # the same run, with the outcome asserted
//! ```
//!
//! Two SIP endpoints on the loopback, a real INVITE, real RTP and real RFC 4733 digits. One side
//! is driven entirely by [`Interpreter`] against a canned program — **answer → play → gather →
//! hang up** — and the other side is a caller that presses two keys and hangs up when told to.
//!
//! What this demonstrates is the claim the [app-sdk](../../../docs/designs/app-sdk.md) design
//! makes for the whole epic: *every host is a thin driver over the same tested machine*. The
//! driver is the `perform` function below, and it is about forty lines. It owns the socket, the
//! clock and the runtime; the interpreter owns the decisions and has none of the three. The "app"
//! is `canned_app`, a pure function from an envelope to a document — swap it for an HTTP round
//! trip and this file becomes the webhook binding without the interpreter changing at all.
//!
//! The trace it prints is what `tests/canned_program.sh` asserts on, which is why the lines are
//! terse and prefixed rather than chatty.

// An example is read before it is run. These fire on code that is clearer written out.
#![allow(clippy::cast_possible_truncation, clippy::too_many_lines)]
// This is a harness, not library code: it drives two endpoints against each other and its job on
// any failure is to die loudly so `tests/canned_program.sh` sees a non-zero exit. The workspace
// bans `expect` because the *library* parses hostile input; nothing here does. Same for the
// indexing, which is over `chunks_exact(2)` and cannot be short. (`sipx-testkit`'s
// `issue-certs.rs` allows the same set, for the same reason.)
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_app_protocol::{
    CallSnapshot, Direction, Document, Effect, EndCause, EventKind, Input, Instruction,
    Interpreter, Output, Policy, Response, Source, Timer, Timestamp, Verb, event_from_call,
};
use sipx_call::{Call, CallEvent, answer, dial};
use sipx_media::Interrupt;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

/// The instruction ids the canned program uses, so the trace and the app agree on them.
const PLAY: &str = "p1";
const GATHER: &str = "g1";

/// The app: a pure function from one event to the program that answers it.
///
/// This is the whole of the application. In a webhook host the same function lives in somebody
/// else's process behind an HTTP request; in an embedded runtime it is a script. The interpreter
/// cannot tell the difference, which is the property the contract exists to have.
fn canned_app(event: &EventKind) -> Document {
    match event {
        // A new call: answer it, play a prompt, then collect two digits.
        EventKind::Incoming => Document::new(vec![
            Instruction::new("a1", Verb::Answer),
            Instruction::new(
                PLAY,
                Verb::Play {
                    // Inline audio rather than a file, so the example needs nothing on disk.
                    source: Source::Inline(prompt_pcm()),
                    interruptible: true,
                },
            ),
            Instruction::new(
                GATHER,
                Verb::GatherDigits(sipx_app_protocol::Gather {
                    min: 0,
                    max: Some(2),
                    terminators: "#".to_owned(),
                    digit_timeout_ms: Some(3_000),
                    timeout_ms: Some(8_000),
                    prompt: None,
                }),
            ),
        ]),
        // The digits are in. That is everything this app wanted, so end the call.
        EventKind::GatherFinished { digits, reason, .. } => {
            println!(
                "canned_program: gather digits={digits} reason={}",
                reason.as_str()
            );
            Document::new(vec![Instruction::new(
                "h1",
                Verb::Hangup {
                    cause: EndCause::Hangup,
                },
            )])
        }
        // Everything else: keep going. §6.3's empty document.
        _ => Document::keep_going(),
    }
}

/// One second of a quiet tone, as 8 kHz signed 16-bit PCM.
fn prompt_pcm() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8_000 * 2);
    for n in 0..8_000 {
        let sample = ((f64::from(n) * 0.1).sin() * 6_000.0) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// PCM back out of the contract's byte-oriented [`Source`], for the media layer.
fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// What woke the driver's wait. Three sources, one value, so the loop below has one shape.
enum Woke {
    /// The call reported something on its `C-3` event stream.
    Event(CallEvent),
    /// A full RFC 4733 keypress arrived over RTP.
    Digit(sipx_rtp::Digit, Duration),
    /// A timer the interpreter asked for has elapsed.
    Timeout,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let loopback: IpAddr = "127.0.0.1".parse()?;
    let (answerer, mut inbound) = bind(Config::new("127.0.0.1:0".parse()?)).await?;
    let (originator, _unused_inbound) = bind(Config::new("127.0.0.1:0".parse()?)).await?;
    let callee_addr = answerer.local_addr();

    // The caller: dial, press 4 and 2 and then #, and wait for the far end to hang up.
    let caller = tokio::spawn(async move {
        let call = dial(
            &originator,
            Target::udp(callee_addr),
            &Uri::sip(Host::Name(HostName::new("callee.example").expect("valid"))),
            &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback),
        )
        .await
        .expect("the call connects");
        println!("canned_program: caller connected");
        // Let the one-second prompt play out, so the digits land while the `gather` is armed
        // rather than racing the playback that precedes it.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        for digit in ["4", "2", "#"] {
            call.send_digits(digit, Duration::from_millis(100)).await;
            tokio::time::sleep(Duration::from_millis(120)).await;
        }
        println!("canned_program: caller sent 42#");
        call
    });

    let incoming = inbound.recv().await.ok_or("no INVITE arrived")?;

    // ---- everything below is the driver ----

    let mut interpreter = Interpreter::new(
        CallSnapshot::new("example-call", Direction::Inbound)
            .between("sip:caller@example.net", "sip:callee.example"),
        Policy::default(),
    );
    let mut call: Option<Call> = None;
    let mut events = None;
    let mut gather_deadline: Option<Duration> = None;
    let mut hung_up = false;

    // The first event is not a `CallEvent` — there is no `Call` yet, only an invitation.
    let mut batch = interpreter.handle(now(), Input::Event(EventKind::Incoming));

    loop {
        // Perform whatever the interpreter asked for, and feed back whatever the app says.
        let mut next: Vec<Input> = Vec::new();
        for output in batch {
            match output {
                Output::Effect(effect) => {
                    if !perform(&effect, &mut call, &incoming, &answerer, loopback).await? {
                        println!("canned_program: call ended");
                        hung_up = true;
                    }
                    if call.is_some() && events.is_none() {
                        events = call.as_mut().and_then(Call::events);
                    }
                }
                Output::Deliver { envelope, callback } => {
                    println!(
                        "canned_program: deliver seq={} event={}",
                        envelope.seq,
                        envelope.event.type_name()
                    );
                    // The "callback": in this binding it returns before it was even sent. A
                    // webhook host would await an HTTP response here and hand back
                    // `Response::Failed` on a timeout — §9.2 is the driver's only other duty.
                    let document = canned_app(&envelope.event);
                    next.push(Input::Response {
                        callback,
                        response: Response::Document(document),
                    });
                }
                Output::SetTimer { timer, after_ms } => {
                    if timer == Timer::GatherOverall {
                        gather_deadline = Some(Duration::from_millis(u64::from(after_ms)));
                    }
                }
                Output::ClearTimer(Timer::GatherOverall) => gather_deadline = None,
                Output::ClearTimer(_) => {}
            }
        }

        if let Some(input) = next.pop() {
            batch = interpreter.handle(now(), input);
            continue;
        }

        // Nothing outstanding: wait for the call to say something. This `select!` is the only
        // clock and the only socket in the whole program, and both are on this side of the line.
        let (Some(stream), Some(live)) = (events.as_mut(), call.as_ref()) else {
            return Err("the interpreter never asked for the call to be answered".into());
        };
        let waited = if hung_up {
            // The BYE has gone. All that is left is the call's last event — §5.3 says
            // `call.ended` is always the last one and is never dropped — so this waits for that
            // alone. Selecting on the media session here would race it: a closed session reports
            // "no more digits" the instant the call goes down, which is not the call's end and
            // must not be mistaken for it.
            tokio::time::timeout(Duration::from_secs(5), stream.recv())
                .await
                .ok()
                .flatten()
                .map(Woke::Event)
        } else {
            tokio::select! {
                event = stream.recv() => event.map(Woke::Event),
                // DTMF arrives over RTP rather than signalling, so nothing on the event stream
                // ever sees it until somebody reads the media session. `sipx_call::serve` is the
                // loop that normally does this; a driver running its own loop owes the same read.
                digit = live.media().recv_digit() => {
                    digit.map(|(digit, held)| Woke::Digit(digit, held))
                }
                () = tokio::time::sleep(gather_deadline.unwrap_or(Duration::from_secs(10))) => {
                    Some(Woke::Timeout)
                }
            }
        };
        batch = match waited {
            Some(Woke::Digit(digit, held)) => {
                println!("canned_program: call said call.dtmf {}", digit.as_char());
                interpreter.handle(
                    now(),
                    Input::Event(EventKind::Dtmf {
                        digit: digit.as_char(),
                        duration_ms: u32::try_from(held.as_millis()).unwrap_or(u32::MAX),
                    }),
                )
            }
            Some(Woke::Event(event)) => {
                let id = match &event {
                    CallEvent::PlaybackFinished { .. } => PLAY,
                    _ => GATHER,
                };
                match event_from_call(&event, id) {
                    Some(EventKind::Ended { cause }) => {
                        println!("canned_program: ended cause={}", tag(cause));
                        break;
                    }
                    Some(contract_event) => {
                        println!("canned_program: call said {}", contract_event.type_name());
                        interpreter.handle(now(), Input::Event(contract_event))
                    }
                    // Mute is §5.2, not §5.3 — it lands in the next snapshot, not on the wire.
                    None => match event {
                        CallEvent::Muted => {
                            interpreter.handle(now(), Input::MediaGate { muted: true })
                        }
                        CallEvent::Unmuted => {
                            interpreter.handle(now(), Input::MediaGate { muted: false })
                        }
                        _ => Vec::new(),
                    },
                }
            }
            None => break,
            // Time entering the machine, which is the only way it ever does.
            Some(Woke::Timeout) => {
                interpreter.handle(now(), Input::TimerFired(Timer::GatherOverall))
            }
        };
    }

    let mut caller = caller.await?;
    let _ = caller.hang_up().await;
    println!("canned_program: OK");
    Ok(())
}

/// The driver's other half: one call-framework operation per [`Effect`].
///
/// Returns whether the call is still up. This example implements the four effects its canned
/// program can produce; a real host implements all of §3's table the same way.
async fn perform(
    effect: &Effect,
    call: &mut Option<Call>,
    incoming: &sipx_transport::Incoming,
    endpoint: &sipx_transport::Handle,
    media_address: IpAddr,
) -> Result<bool, Box<dyn std::error::Error>> {
    match effect {
        Effect::Answer => {
            println!("canned_program: effect answer");
            *call = Some(answer(endpoint, incoming, media_address).await?);
        }
        Effect::Play {
            instruction_id,
            source,
            interruptible,
        } => {
            println!("canned_program: effect play {instruction_id}");
            let Source::Inline(bytes) = source else {
                return Err("this example's program only plays inline audio".into());
            };
            let Some(call) = call.as_ref() else {
                return Err("a play before an answer".into());
            };
            // Fire and forget: the playback reports itself on the event stream (`M-17`), which is
            // what turns into the contract's `call.playback.finished`.
            let interrupt = if *interruptible {
                Interrupt::OnDigit
            } else {
                Interrupt::Never
            };
            drop(call.start_playback(samples(bytes), interrupt));
        }
        Effect::StopPlayback => println!("canned_program: effect stop_playback"),
        Effect::HangUp { cause } => {
            println!("canned_program: effect hangup cause={}", tag(*cause));
            if let Some(call) = call.as_mut() {
                call.hang_up().await?;
            }
            return Ok(false);
        }
        other => println!("canned_program: effect {other:?} (not implemented in this example)"),
    }
    Ok(true)
}

/// An end cause's wire name, for the trace.
fn tag(cause: EndCause) -> String {
    let json = cause.to_json();
    json.as_str()
        .map(str::to_owned)
        .or_else(|| json.get("name").and_then(|n| n.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The driver's clock, and the only one in the program.
///
/// §2 of the contract: the interpreter reads no clock, so every timestamp it stamps on an envelope
/// is one the driver handed it. This is that function, and it is deliberately the only place in
/// this file that asks what time it is.
fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    Timestamp::from_unix_millis(millis)
}
