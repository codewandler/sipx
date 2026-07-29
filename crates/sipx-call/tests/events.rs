//! The call event stream (story `C-3`): a `Call` reports what happens to it on a channel,
//! instead of only being inspectable by calling methods on it at the right moment.
//!
//! `hanging_up_emits_ended_with_cause` is the failing-first test the story names; the rest cover
//! the properties its acceptance calls out by name: the overflow policy, `Ended` surviving a
//! full queue, and a consumer that never reads at all not being able to stall a call's own
//! teardown.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{Call, CallEvent, CallEvents, EndCause, answer, dial, serve};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

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

/// The next event, bounded so a test that is wrong about wiring fails instead of hanging.
async fn next_event(events: &mut CallEvents) -> CallEvent {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("no timeout waiting for a call event")
        .expect("the stream ended before this event arrived")
}

/// The next `Ended`, skipping whatever construction already queued ahead of it (`Answered`, and
/// `Ringing` when the dial rang first) — this test suite is about what happens *after* a call
/// connects, not about the order those two arrive in, which has its own test below.
async fn next_ended(events: &mut CallEvents) -> CallEvent {
    loop {
        let event = next_event(events).await;
        if matches!(event, CallEvent::Ended(_)) {
            return event;
        }
    }
}

/// The next event satisfying `want`, skipping whatever construction queued ahead of it.
async fn next_matching(events: &mut CallEvents, want: impl Fn(&CallEvent) -> bool) -> CallEvent {
    loop {
        let event = next_event(events).await;
        if want(&event) {
            return event;
        }
    }
}

/// A caller and a callee, connected, with nothing further driving either side's incoming
/// requests — enough for tests that only hang up or only inspect construction-time events.
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

/// The story's failing-first test: hanging up must emit `Ended` with a cause that says *this
/// side* decided to end it, not the far end or a timeout.
#[tokio::test]
async fn hanging_up_emits_ended_with_cause() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream has not been taken yet");

    caller.hang_up().await.expect("hangs up");

    let event = next_ended(&mut events).await;
    assert!(
        matches!(event, CallEvent::Ended(EndCause::LocalHangup)),
        "expected Ended(LocalHangup), got {event:?}"
    );
}

/// `Call::events` is a one-shot handle, per the vision's "own it, don't share it": a second call
/// gets nothing to take.
#[tokio::test]
async fn the_event_stream_can_only_be_taken_once() {
    let (mut caller, _callee) = connected().await;
    assert!(caller.events().is_some(), "the first call gets the stream");
    assert!(caller.events().is_none(), "the second gets nothing");
}

/// The far end's BYE is a different cause from hanging up locally — the whole point of naming
/// causes rather than just reporting `Ended`.
#[tokio::test]
async fn a_remote_bye_emits_ended_with_remote_cause() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("connects");
    let mut callee = answering.await.expect("the answering side finishes");

    let mut events = caller.events().expect("not yet taken");

    callee.hang_up().await.expect("the callee hangs up");
    let bye = tokio::time::timeout(Duration::from_secs(2), caller_incoming.recv())
        .await
        .expect("no timeout")
        .expect("a BYE arrives");
    assert!(
        caller.handle(&bye).await.expect("handles"),
        "the BYE belongs to this call"
    );

    let event = next_ended(&mut events).await;
    assert!(
        matches!(event, CallEvent::Ended(EndCause::RemoteBye)),
        "expected Ended(RemoteBye), got {event:?}"
    );
}

/// A consumer that never reads a single event — worse than merely slow — must not be able to
/// stall the call's own teardown. This is what the reserved `Ended` slot and the `try_send`
/// overflow policy in `sipx_call::event` exist to guarantee: nothing about ending a call ever
/// awaits on the event channel having room.
#[tokio::test]
async fn a_consumer_that_never_reads_does_not_stall_hanging_up() {
    let (mut caller, _callee) = connected().await;
    // Taken and immediately ignored: the worst case for a "slow" consumer is one that is not
    // reading at all.
    let _events = caller.events().expect("the stream has not been taken yet");

    let result = tokio::time::timeout(Duration::from_secs(2), caller.hang_up()).await;
    assert!(
        result.is_ok(),
        "hang_up must not block on an event stream nobody is reading"
    );
    result.expect("no timeout").expect("hangs up");
}

/// Construction emits in the order things actually happened: `Answered` is queued as soon as
/// the call exists, and — when the far end rang first — `Ringing` comes before it, carrying
/// whether that provisional was reliable, from what was observed while waiting for the 200, not
/// recomputed from anything stored on the `Call` afterwards.
#[tokio::test]
async fn dialing_through_a_provisional_reports_ringing_then_answered() {
    use tokio::net::UdpSocket;

    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let (caller_endpoint, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("peer.example").expect("valid")));
    let dialing = tokio::spawn(async move {
        dial(
            &caller_endpoint,
            Target::udp(peer_addr),
            &to,
            &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
    });

    let mut buf = vec![0u8; 8192];
    let (len, from) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
        .await
        .expect("no timeout")
        .expect("an INVITE");
    let invite = String::from_utf8_lossy(&buf[..len]).into_owned();
    let header = |name: &str| {
        invite
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with(&name.to_ascii_lowercase())
            })
            .unwrap_or_default()
            .to_owned()
    };

    let ringing = format!(
        "SIP/2.0 180 Ringing\r\n{}\r\n{}\r\n{};tag=peertag\r\n{}\r\n{}\r\nContent-Length: 0\r\n\r\n",
        header("Via:"),
        header("From:"),
        header("To:"),
        header("Call-ID:"),
        header("CSeq:")
    );
    peer.send_to(ringing.as_bytes(), from).await.expect("sends");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 41000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n";
    let ok = format!(
        "SIP/2.0 200 OK\r\n{}\r\n{}\r\n{};tag=peertag\r\n{}\r\n{}\r\n\
         Contact: <sip:peer@127.0.0.1:{}>\r\nContent-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n{sdp}",
        header("Via:"),
        header("From:"),
        header("To:"),
        header("Call-ID:"),
        header("CSeq:"),
        peer_addr.port(),
        sdp.len()
    );
    peer.send_to(ok.as_bytes(), from).await.expect("sends");

    let mut caller = tokio::time::timeout(Duration::from_secs(2), dialing)
        .await
        .expect("no timeout")
        .expect("the dialing task completes")
        .expect("the call connects");

    let mut events = caller.events().expect("not yet taken");
    let first = next_event(&mut events).await;
    assert!(
        matches!(first, CallEvent::Ringing { reliable: false }),
        "expected an unreliable Ringing first, got {first:?}"
    );
    let second = next_event(&mut events).await;
    assert!(
        matches!(second, CallEvent::Answered),
        "expected Answered next, got {second:?}"
    );

    let _ = caller.hang_up().await;
}

/// Hold and resume, driven the way this story means a call to be driven from now on:
/// `sipx_call::serve` owns the `Call` outright while it runs, rather than the
/// `Arc<Mutex<Call>>` workaround the story's own notes call out.
#[tokio::test]
async fn hold_and_resume_by_the_far_end_are_reported() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("connects");
    let mut callee = answering.await.expect("the answering side finishes");

    let mut events = caller.events().expect("not yet taken");
    let first = next_event(&mut events).await;
    assert!(
        matches!(first, CallEvent::Answered),
        "expected Answered first, got {first:?}"
    );

    let serving = tokio::spawn(async move {
        let _ = serve(&mut caller, &mut caller_incoming).await;
    });

    callee
        .reinvite(sipx_sdp::Direction::SendOnly)
        .await
        .expect("hold is accepted");
    let held = next_event(&mut events).await;
    assert!(
        matches!(held, CallEvent::Hold),
        "expected Hold, got {held:?}"
    );

    callee
        .reinvite(sipx_sdp::Direction::SendRecv)
        .await
        .expect("resume is accepted");
    let resumed = next_event(&mut events).await;
    assert!(
        matches!(resumed, CallEvent::Resumed),
        "expected Resumed, got {resumed:?}"
    );

    serving.abort();
}

/// A digit the far end presses arrives over RTP, never through signalling — `serve` is what
/// bridges it onto the event stream, which is why this is the one place the story's third
/// acceptance item (`Call::handle` and `serve` emit through the same path) has to name `serve`
/// explicitly.
#[tokio::test]
async fn serve_reports_dtmf_as_an_event() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("connects");
    let callee = answering.await.expect("the answering side finishes");

    let mut events = caller.events().expect("not yet taken");
    let first = next_event(&mut events).await;
    assert!(
        matches!(first, CallEvent::Answered),
        "expected Answered first, got {first:?}"
    );

    let serving = tokio::spawn(async move {
        let _ = serve(&mut caller, &mut caller_incoming).await;
    });

    callee.send_digits("5", Duration::from_millis(80)).await;

    let event = next_event(&mut events).await;
    match event {
        CallEvent::Dtmf { digit, .. } => assert_eq!(digit.as_char(), '5'),
        other => panic!("expected a Dtmf event, got {other:?}"),
    }

    serving.abort();
}

/// A playback that runs to the end reports itself, and says it completed.
///
/// The completion half of the contract's `play` instruction (§5.3 `call.playback.finished`).
/// Without it a host driving a call from its events has to guess when an announcement is over,
/// and the only way to guess is a timer that does not know the clip's length.
#[tokio::test]
async fn playing_a_clip_to_the_end_reports_it_as_completed() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");

    // Two packets' worth at the session's own rate, so this does not assume 8 kHz.
    let per_packet = caller.media().samples_per_packet();
    let completed = caller.play(&vec![0i16; per_packet * 2]).await;
    assert!(completed, "the send queue was open the whole way");

    let event = next_matching(&mut events, |event| {
        matches!(event, CallEvent::PlaybackFinished { .. })
    })
    .await;
    assert!(
        matches!(
            event,
            CallEvent::PlaybackFinished {
                completed: true,
                ..
            }
        ),
        "a clip that ran out must not be reported as cut short: {event:?}"
    );
}

/// A playback the call cuts off short is reported as *not* completed.
///
/// The distinction is the point of the flag: "the announcement finished" and "the caller hung up
/// during the announcement" lead a host to do different things next, and a `PlaybackFinished`
/// that always said `true` would collapse them into one.
#[tokio::test]
async fn a_playback_cut_short_is_not_reported_as_completed() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");
    let per_packet = caller.media().samples_per_packet();

    // Stopping the session makes its send loop exit and drop the queue — which is what the end
    // of a call does to a clip still going out. The loop is a task, so the close is not
    // instantaneous; wait for it rather than racing it, or this asserts on timing instead of on
    // behaviour.
    caller.media().stop();
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        while caller.media().send(vec![0i16; per_packet]).await {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        closed.is_ok(),
        "the send queue closed once the session stopped"
    );

    let completed = caller.play(&vec![0i16; per_packet * 4]).await;
    assert!(!completed, "the queue was closed, so nothing went out");

    let event = next_matching(&mut events, |event| {
        matches!(event, CallEvent::PlaybackFinished { .. })
    })
    .await;
    assert!(
        matches!(
            event,
            CallEvent::PlaybackFinished {
                completed: false,
                ..
            }
        ),
        "a clip the call cut off must say so: {event:?}"
    );
}

/// A recording reports how much audio it captured, measured from the samples themselves rather
/// than from how long this side sat waiting for them.
#[tokio::test]
async fn a_recording_reports_the_duration_of_what_it_captured() {
    let (caller, mut callee) = connected().await;
    let mut events = callee.events().expect("the stream is taken once");

    let per_packet = caller.media().samples_per_packet();
    let rate = u64::from(caller.media().codec().clock_rate());
    // Ten packets the far end actually sends — a duration distinct from the idle timeout that
    // detects the end of it, which is the thing being asserted about.
    let packets = 10usize;
    let spoken = Duration::from_micros((packets * per_packet) as u64 * 1_000_000 / rate);

    // Two seconds rather than the 500 ms this used to be (`X-28`). This test is *about*
    // `record_until_idle`, so it cannot be moved to a counted wait like the rest of that sweep
    // was — the idle window is the subject, not the transport. What it can do is stop treating
    // half a second of wall clock as the difference between "the far end stopped talking" and
    // "this machine is busy": at 20 ms a packet that gap is a hundred missed intervals. Both
    // assertions below survive the change — a duration that wrongly counted the window would be
    // `spoken + idle`, still comfortably over it.
    let idle = Duration::from_secs(2);
    let recorded = tokio::join!(
        async {
            caller
                .media()
                .play(&vec![64i16; per_packet * packets], per_packet)
                .await
        },
        async { callee.record_until_idle(idle).await }
    )
    .1;
    assert!(!recorded.is_empty(), "the callee heard nothing at all");

    let event = next_matching(&mut events, |event| {
        matches!(event, CallEvent::RecordingFinished { .. })
    })
    .await;
    let CallEvent::RecordingFinished { duration } = event else {
        panic!("a recording event, got {event:?}");
    };

    // The captured audio, not the half-second of silence that ended it. A duration that counted
    // the idle timeout would describe this side's patience rather than the recording, and would
    // grow if someone tuned the timeout.
    assert!(
        duration < idle,
        "the idle timeout must not be counted as recorded audio: {duration:?}"
    );
    assert!(
        duration >= spoken,
        "every sample that arrived must be reported: {duration:?} for {spoken:?} spoken"
    );
}
