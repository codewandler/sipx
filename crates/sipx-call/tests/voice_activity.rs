//! Voice activity as typed call events, on live calls (`M-58`).
//!
//! The deterministic half of this story is proved by fixtures — `sipx-audio`'s `CAP-*` corpus for
//! the analyser, and `sipx-call`'s own `voice` unit tests for the transition and coalescing policy.
//! What those cannot prove is that the wiring exists: that the audio reaches the analyser through
//! `M-54`'s one seam on a call nobody is polling, that what an application is told names *its* call,
//! and that ending a call closes activity before the stream's last word.
//!
//! Two simultaneous calls are the shape of the crossing risk. A shared analyser, a shared
//! observation counter or a shared attachment would all pass a single-call test and fail here, so
//! both calls carry voice at the same time and each is asserted to hear only its own.
//!
//! Nothing here waits a fixed duration to establish an ordering. Every wait is a bound on failure
//! around an event: an application event arriving, or a playback resolving.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::voice::{AnalysisProfile, AudioDirection, VoiceEndCause};
use sipx_call::{Call, CallEvent, CallEvents, answer, dial};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

/// How long a test here waits for an event before calling the wiring broken.
///
/// A bound on failure, not a window to measure in: every assertion below is about *which* event
/// arrived, never about when.
const EVENT_BOUND: Duration = Duration::from_secs(20);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

/// A connected pair over loopback, as `tests/playback.rs` builds one.
async fn connected() -> (Call, Call) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the call connects");

    let callee = answering.await.expect("the answering side finishes");
    (caller, callee)
}

fn call_id(call: &Call) -> String {
    String::from_utf8_lossy(&call.dialog.id.call_id).into_owned()
}

/// The reference profile of `docs/specs/call-audio-processing.md` §11.1, on received audio.
fn profile() -> AnalysisProfile {
    AnalysisProfile::new(AudioDirection::Inbound, 8_000)
}

/// A second of 1 kHz modulation at 8 kHz: loud, and — unlike a constant — genuinely varying, which
/// is what the contract's variance predicate calls voice.
fn speech() -> Vec<i16> {
    (0..8_000)
        .map(|index| if (index / 4) % 2 == 0 { 8_000 } else { -8_000 })
        .collect()
}

/// The next event satisfying `want`, bounded so a test that is wrong about the wiring fails
/// instead of hanging.
async fn next_matching(events: &mut CallEvents, want: impl Fn(&CallEvent) -> bool) -> CallEvent {
    let found = tokio::time::timeout(EVENT_BOUND, async {
        loop {
            let event = events
                .recv()
                .await
                .expect("the stream ended before the event arrived");
            if want(&event) {
                return event;
            }
        }
    })
    .await;
    found.expect("no timeout waiting for a call event")
}

/// Two calls carrying voice at the same time: each application is told about its own call, and only
/// about its own call.
#[tokio::test]
async fn two_simultaneous_calls_report_their_own_voice_and_never_cross() {
    let (caller_one, mut callee_one) = connected().await;
    let (caller_two, mut callee_two) = connected().await;

    let id_one = call_id(&callee_one);
    let id_two = call_id(&callee_two);
    assert_ne!(id_one, id_two, "two calls, two identities");

    callee_one
        .detect_voice_activity(profile())
        .await
        .expect("the reference profile is accepted");
    callee_two
        .detect_voice_activity(profile())
        .await
        .expect("the reference profile is accepted");

    let mut events_one = callee_one.events().expect("the first receiver");
    let mut events_two = callee_two.events().expect("the first receiver");

    // Both far ends speak at once. Nothing is polling either callee: the transitions arrive on the
    // event stream on their own.
    let clip = speech();
    let (played_one, played_two) = tokio::join!(caller_one.play(&clip), caller_two.play(&clip));
    assert!(played_one && played_two, "both clips ran to the end");

    for (events, id) in [(&mut events_one, &id_one), (&mut events_two, &id_two)] {
        let event =
            next_matching(events, |event| matches!(event, CallEvent::VoiceStarted(_))).await;
        let CallEvent::VoiceStarted(activity) = event else {
            unreachable!("the predicate above admits nothing else");
        };
        assert_eq!(activity.call_id(), id, "an event named the wrong call");
        assert_eq!(activity.direction(), AudioDirection::Inbound);
        assert_eq!(
            activity.sequence(),
            0,
            "each call numbers its own observations from zero"
        );
        assert_eq!(activity.sample_rate(), 8_000);
    }
}

/// Ending a call cannot leave activity latched, and cannot put an event after the call's last word.
#[tokio::test]
async fn ending_a_call_cuts_open_voice_before_ended() {
    let (caller, mut callee) = connected().await;

    callee
        .detect_voice_activity(profile())
        .await
        .expect("the reference profile is accepted");
    let mut events = callee.events().expect("the first receiver");

    assert!(caller.play(&speech()).await, "the clip ran to the end");
    let started = next_matching(&mut events, |event| {
        matches!(event, CallEvent::VoiceStarted(_))
    })
    .await;
    assert!(matches!(started, CallEvent::VoiceStarted(_)));

    callee.hang_up().await.expect("the call ends");

    // Everything the stream has left, in order. The cut has to be in it, and `Ended` has to be
    // last — the one ordering guarantee `CallEvents` makes.
    let mut tail = Vec::new();
    while let Some(event) = events.try_recv() {
        tail.push(event);
    }
    let cut = tail.iter().position(|event| {
        matches!(
            event,
            CallEvent::VoiceEnded {
                cause: VoiceEndCause::Cut,
                ..
            }
        )
    });
    let ended = tail
        .iter()
        .position(|event| matches!(event, CallEvent::Ended(_)));
    assert!(
        cut.is_some(),
        "voice was open and the call ended without closing it: {tail:?}"
    );
    assert_eq!(
        ended,
        Some(tail.len() - 1),
        "`Ended` must still be the last event: {tail:?}"
    );
    assert!(cut < ended, "the cut must precede the call's last word");
}

/// A profile outside the contract's domains is refused before anything is attached, and the call
/// carries on.
#[tokio::test]
async fn a_refused_profile_leaves_the_call_untouched() {
    let (caller, mut callee) = connected().await;

    let refusal = callee
        .detect_voice_activity(profile().with_queue_capacity(1))
        .await
        .expect_err("a queue of one is outside the seam's domain");
    assert!(
        refusal.to_string().contains("queue_capacity"),
        "the refusal names the field: {refusal}"
    );

    // The call is unchanged: audio still flows, and detection can still be asked for properly.
    assert!(caller.play(&[0i16; 160]).await);
    callee
        .detect_voice_activity(profile())
        .await
        .expect("the reference profile is accepted");
}
