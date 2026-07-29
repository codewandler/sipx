//! Mute and unmute (story `M-18`): a local media gate on a call's outbound audio.
//!
//! `a_muted_call_contributes_no_audio` is the failing-first test the story names. The rest cover
//! the properties its acceptance calls out: that muting is *not* hold (no re-INVITE, no change to
//! the SDP direction, no effect on the far end's hold state), that reception is untouched, that
//! the state is queryable, and that transitions land on the `C-3` event stream.
//!
//! Everything about the outbound audio is asserted **from the receiving side**. What this side
//! believes it sent is not evidence; what the far end decodes is.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{Call, CallEvent, CallEvents, answer, dial};
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

/// The next event, bounded so a test that is wrong about the wiring fails instead of hanging.
async fn next_event(events: &mut CallEvents) -> CallEvent {
    tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .expect("no timeout waiting for a call event")
        .expect("the stream ended before this event arrived")
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

/// A connected pair, with the callee's inbound request stream handed back rather than consumed —
/// which is what lets a test assert that muting sent *no* in-dialog request at all.
async fn connected() -> (Call, Call, Receiver<Incoming>) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        let call = answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers");
        (call, callee_incoming)
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the call connects");

    let (callee, callee_incoming) = answering.await.expect("the answering side finishes");
    (caller, callee, callee_incoming)
}

/// A clip loud enough that anything but silence at the far end is unmistakable.
fn loud(samples: usize) -> Vec<i16> {
    vec![12_000i16; samples]
}

/// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
/// A bound on failure, not a window to measure in — see `MediaSession::record_at_least`.
///
/// Note that a muted call still sends: mute substitutes silence packet for packet rather than
/// stopping the stream, which is the property half this file exists to assert. So counting
/// samples terminates here whether the caller is muted or not.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// The story's failing-first test: while a call is muted, the far end must decode nothing but
/// silence out of it, however loud what this side plays into it is.
#[tokio::test]
async fn a_muted_call_contributes_no_audio() {
    let (caller, callee, _incoming) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    let clip = loud(per_packet * 5);

    caller.mute();

    let heard = tokio::join!(async { caller.play(&clip).await }, async {
        callee
            .media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    })
    .1;

    assert!(
        heard.iter().all(|sample| *sample == 0),
        "the far end decoded audio out of a muted call: {} of {} samples were not silence",
        heard.iter().filter(|sample| **sample != 0).count(),
        heard.len()
    );
}

/// The design decision, asserted from the receiving side: mute substitutes silence for the audio
/// packet by packet, it does not stop the stream. The far end therefore receives exactly the
/// packets it would have received unmuted — no gap in the sequence space for RFC 3550 §6.4.1 to
/// score as loss — and this side's sender report counts packets that genuinely went out.
#[tokio::test]
async fn muting_substitutes_silence_rather_than_stopping_the_stream() {
    let (caller, callee, _incoming) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    let packets = 5usize;
    let clip = loud(per_packet * packets);

    caller.mute();
    let heard = tokio::join!(async { caller.play(&clip).await }, async {
        callee
            .media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    })
    .1;

    assert_eq!(
        heard.len(),
        clip.len(),
        "every packet must still arrive; mute is a gate on the audio, not on the stream"
    );
    assert!(
        heard.iter().all(|sample| *sample == 0),
        "and every one of them must decode to silence"
    );
    assert_eq!(
        caller.media().packets_sent(),
        callee.media().packets_received(),
        "the count this side reports as sent must be the count that actually arrived"
    );
    assert_eq!(
        callee.media().quality().await.cumulative_lost,
        0,
        "a muted stretch must not look like loss to the far end"
    );
}

/// Unmuting restores the audio, on the same session and without renegotiating anything.
#[tokio::test]
async fn unmuting_restores_the_audio() {
    let (caller, callee, _incoming) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    let clip = loud(per_packet * 3);

    caller.mute();
    let while_muted = tokio::join!(async { caller.play(&clip).await }, async {
        callee
            .media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    })
    .1;
    assert!(
        while_muted.iter().all(|sample| *sample == 0),
        "muted audio reached the far end"
    );

    caller.unmute();
    assert!(!caller.is_muted(), "unmute must clear the gate");
    let after = tokio::join!(async { caller.play(&clip).await }, async {
        callee
            .media()
            .record_at_least(clip.len(), DELIVERY_BOUND)
            .await
    })
    .1;

    assert!(
        after.iter().any(|sample| *sample != 0),
        "the far end heard nothing after unmuting"
    );
}

/// Mute is a *local* gate. The far end is told nothing: no re-INVITE goes out, so the SDP
/// direction it holds is the one it already had, and its own hold state is untouched. This is the
/// whole difference from `reinvite(Direction::SendOnly)`, which signals and which the far end
/// sees as hold.
#[tokio::test]
async fn muting_sends_no_re_invite_and_leaves_the_far_end_off_hold() {
    let (caller, callee, mut callee_incoming) = connected().await;

    caller.mute();
    caller.unmute();
    caller.mute();

    // Long enough that a re-INVITE sent by any of the three would have arrived. The only thing
    // the callee's inbound stream carries on a call nobody is renegotiating is the ACK that
    // established it.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    while let Ok(Some(incoming)) = tokio::time::timeout_at(deadline, callee_incoming.recv()).await {
        assert_ne!(
            incoming.request.method,
            sipx_sip::Method::Invite,
            "muting must not renegotiate the session"
        );
    }
    assert!(
        !callee.is_on_hold(),
        "the far end's hold state must be untouched by a local mute"
    );
}

/// Reception is not part of the gate: a muted call still hears the far end, still collects its
/// keypresses, and still reports on how the call is going.
#[tokio::test]
async fn a_muted_call_still_receives_audio_digits_and_statistics() {
    let (caller, callee, _incoming) = connected().await;
    let per_packet = caller.media().samples_per_packet();

    caller.mute();

    let heard = tokio::join!(
        async { callee.media().play(&loud(per_packet * 3), per_packet).await },
        async { caller.record_at_least(per_packet * 3, DELIVERY_BOUND).await }
    )
    .1;
    assert!(
        heard.iter().any(|sample| *sample != 0),
        "a muted call must still hear the far end"
    );

    callee.send_digits("7", Duration::from_millis(80)).await;
    let digit = tokio::time::timeout(Duration::from_secs(2), caller.recv_digit())
        .await
        .expect("no timeout waiting for the keypress")
        .expect("a digit arrives");
    assert_eq!(digit.as_char(), '7', "a muted call must still collect DTMF");

    let quality = caller.media().quality().await;
    assert_eq!(
        quality.cumulative_lost, 0,
        "quality statistics must keep describing the inbound stream while muted"
    );
    assert!(
        caller.media().packets_received() > 0,
        "the receive path must have kept counting while muted"
    );
}

/// The state is queryable and its transitions land on the `C-3` stream — and only the
/// transitions: muting a call that is already muted is not something that happened.
#[tokio::test]
async fn mute_and_unmute_are_queryable_and_reported_as_events() {
    let (mut caller, _callee, _incoming) = connected().await;
    let mut events = caller.events().expect("the stream has not been taken yet");

    assert!(!caller.is_muted(), "a fresh call is not muted");
    caller.mute();
    assert!(caller.is_muted(), "mute must be visible without an event");
    caller.mute();
    caller.unmute();

    let muted = next_matching(&mut events, |event| {
        matches!(event, CallEvent::Muted | CallEvent::Unmuted)
    })
    .await;
    assert!(
        matches!(muted, CallEvent::Muted),
        "expected Muted, got {muted:?}"
    );

    let next = next_matching(&mut events, |event| {
        matches!(event, CallEvent::Muted | CallEvent::Unmuted)
    })
    .await;
    assert!(
        matches!(next, CallEvent::Unmuted),
        "a repeated mute is not a transition and must not be reported: expected Unmuted, got {next:?}"
    );
}

/// A keypress is not audio. Mute gates what the call *says*; an RFC 4733 event is generated by
/// this endpoint on purpose, the way a keypad tone is on a handset, and it still goes out.
#[tokio::test]
async fn a_muted_call_can_still_send_a_keypress() {
    let (caller, callee, _incoming) = connected().await;

    caller.mute();
    caller.send_digits("4", Duration::from_millis(80)).await;

    let digit = tokio::time::timeout(Duration::from_secs(2), callee.recv_digit())
        .await
        .expect("no timeout waiting for the keypress")
        .expect("a digit arrives");
    assert_eq!(digit.as_char(), '4');
}
