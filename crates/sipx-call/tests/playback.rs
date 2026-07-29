//! Playback control (story `M-17`): a handle, a queue, a stop, and interrupt-on-digit.
//!
//! `a_digit_interrupts_playback` is the failing-first test the story names. The rest cover the
//! properties its acceptance calls out: that a stop lands within a stated number of packet
//! intervals rather than "promptly", that clips queue rather than replace, that a clip queued
//! while another is stopping still plays, and that every playback resolves on the `C-3` event
//! stream saying which one it was and whether it ran out or was cut.
//!
//! Everything about the outbound audio is asserted **from the receiving side**, as
//! `tests/mute.rs` does. What this side believes it sent is not evidence; what the far end
//! decodes is. The one exception is the bound, which is a statement about packets that reached
//! the wire — `packets_sent` counts datagrams the socket accepted, and the far end's
//! `packets_received` is asserted to agree with it.

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
use sipx_media::{Interrupt, Playback, PlaybackEnd};
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

/// A connected pair.
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

/// A clip loud enough that anything but silence at the far end is unmistakable.
fn loud(samples: usize) -> Vec<i16> {
    vec![12_000i16; samples]
}

/// Wait until the far end has actually decoded some of what is being played into it.
///
/// Every test below that stops or interrupts a playback needs this first: without it the test
/// races the first packet, and a stop that arrived before anything went out would pass for the
/// wrong reason.
async fn hearing(call: &Call) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while call.media().packets_received() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the far end never heard the playback start"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The story's failing-first test: a keypress from the far end cuts the prompt short, and the
/// digit that cut it is **not** swallowed — it still reaches whoever is collecting digits.
///
/// This is the primitive under the contract's `gather{prompt, interruptible}`
/// (`docs/specs/app-contract.md` §6.2). A gather that consumed the digit that stopped its own
/// prompt would collect every digit of a PIN except the first, which is worse than not
/// supporting barge-in at all.
#[tokio::test]
async fn a_digit_interrupts_playback() {
    let (caller, callee) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    // Five seconds of prompt. Nothing here waits that long, so a playback that is *not*
    // interrupted fails on the timeout rather than by quietly running out.
    let clip = loud(per_packet * 250);

    let prompt: Playback = caller.start_playback(clip, Interrupt::OnDigit);
    hearing(&callee).await;

    callee.send_digits("5", Duration::from_millis(80)).await;

    let end = tokio::time::timeout(Duration::from_secs(2), prompt.finished())
        .await
        .expect("the prompt must be cut short, not play to its end");
    assert_eq!(
        end,
        PlaybackEnd::Interrupted,
        "a keypress must cut an interruptible prompt short"
    );

    let digit = tokio::time::timeout(Duration::from_secs(2), caller.recv_digit())
        .await
        .expect("no timeout waiting for the interrupting keypress")
        .expect("a digit arrives");
    assert_eq!(
        digit.as_char(),
        '5',
        "the digit that interrupted the prompt was lost"
    );
}

/// The bound, stated as a number and measured: after a playback is stopped, at most
/// [`Playback::STOP_BOUND_PACKETS`] more of its packets reach the wire.
///
/// The reference point is inside the system rather than on the test's clock: `finished()`
/// resolves the moment the stop is decided, before the send loop has drained anything, so the
/// count read straight after it is what "at the stop" means. Everything the send queue was still
/// holding for this clip — dozens of packets — must be discarded rather than played out.
#[tokio::test]
async fn stopping_a_playback_lands_within_the_stated_bound() {
    let (caller, callee) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    // Five seconds. The send queue runs well ahead of the wire, so by the time this is stopped it
    // is holding a backlog an unbounded stop would go on playing.
    let clip_packets = 250u64;

    let playback = caller.start_playback(
        loud(per_packet * usize::try_from(clip_packets).expect("fits")),
        Interrupt::Never,
    );
    hearing(&callee).await;

    playback.stop();
    let end = tokio::time::timeout(Duration::from_secs(2), playback.finished())
        .await
        .expect("a stop resolves the playback rather than leaving it running");
    assert_eq!(
        end,
        PlaybackEnd::Stopped,
        "stop must report itself as a stop"
    );

    let at_stop = caller.media().packets_sent();
    // Far longer than the bound: if the backlog were being played out, it would show up here.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let settled = caller.media().packets_sent();

    assert!(
        settled - at_stop <= Playback::STOP_BOUND_PACKETS,
        "a stop must land within {} packets, but {} more went out",
        Playback::STOP_BOUND_PACKETS,
        settled - at_stop
    );
    assert!(
        settled < clip_packets,
        "the whole clip went out despite being stopped: {settled} of {clip_packets} packets"
    );
    assert_eq!(
        callee.media().packets_received(),
        settled,
        "what this side counted as sent must be what the far end actually received"
    );
}

/// The same bound for interrupt-on-digit, measured the same way. Barge-in that took a second to
/// stop the prompt would be barge-in nobody would use.
#[tokio::test]
async fn an_interrupting_digit_lands_within_the_stated_bound() {
    let (caller, callee) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    let clip_packets = 250u64;

    let prompt = caller.start_playback(
        loud(per_packet * usize::try_from(clip_packets).expect("fits")),
        Interrupt::OnDigit,
    );
    hearing(&callee).await;
    callee.send_digits("9", Duration::from_millis(80)).await;

    let end = tokio::time::timeout(Duration::from_secs(2), prompt.finished())
        .await
        .expect("the keypress resolves the prompt");
    assert_eq!(end, PlaybackEnd::Interrupted);

    let at_interrupt = caller.media().packets_sent();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let settled = caller.media().packets_sent();

    assert!(
        settled - at_interrupt <= Playback::STOP_BOUND_PACKETS,
        "an interruption must land within {} packets, but {} more went out",
        Playback::STOP_BOUND_PACKETS,
        settled - at_interrupt
    );
    assert!(
        settled < clip_packets,
        "the whole prompt went out despite the keypress: {settled} of {clip_packets} packets"
    );
}

/// The decision this story had to make and record: a second clip started while one is playing
/// **queues** behind it. It does not replace it.
///
/// Asserted from the receiving side, and the two clips are told apart by sign — G.711 is lossy
/// about magnitude and exact about which side of zero a sample is on. Replacement would show up
/// as half as much audio, all of it negative; interleaving would show up as the signs alternating.
#[tokio::test]
async fn clips_queue_rather_than_replacing_one_another() {
    let (caller, callee) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    let packets = 3usize;

    let (ends, heard) = tokio::join!(
        async {
            let first =
                caller.start_playback(vec![8_000i16; per_packet * packets], Interrupt::Never);
            let second =
                caller.start_playback(vec![-8_000i16; per_packet * packets], Interrupt::Never);
            (first.finished().await, second.finished().await)
        },
        async {
            callee
                .media()
                .record_until_idle(Duration::from_millis(400))
                .await
        }
    );

    assert_eq!(
        ends,
        (PlaybackEnd::Completed, PlaybackEnd::Completed),
        "queueing must let both clips run to their end"
    );
    assert_eq!(
        heard.len(),
        per_packet * packets * 2,
        "the far end must hear both clips, not one of them"
    );
    let (first_half, second_half) = heard.split_at(per_packet * packets);
    assert!(
        first_half.iter().all(|sample| *sample > 0),
        "the clip started first must be heard first"
    );
    assert!(
        second_half.iter().all(|sample| *sample < 0),
        "the clip started second must follow it, whole and unmixed"
    );
}

/// The edge the story names: queueing while stopping.
///
/// This is barge-in's exact shape — cut the prompt, say something else — and it is the case a
/// queue gets wrong. The clip being stopped must release the queue at once and its unsent backlog
/// must be discarded, so the new clip starts within the bound rather than after the far end has
/// sat through the rest of a prompt the application already abandoned.
#[tokio::test]
async fn a_clip_queued_while_another_is_stopping_starts_within_the_bound() {
    let (caller, callee) = connected().await;
    let per_packet = caller.media().samples_per_packet();
    let reply_packets = 3u64;

    let ((prompt_end, reply_end, at_stop), heard) = tokio::join!(
        async {
            let prompt = caller.start_playback(vec![8_000i16; per_packet * 250], Interrupt::Never);
            hearing(&callee).await;
            prompt.stop();
            let reply = caller.start_playback(
                vec![-8_000i16; per_packet * usize::try_from(reply_packets).expect("fits")],
                Interrupt::Never,
            );
            let prompt_end = prompt.finished().await;
            // Read once the prompt has resolved and before the reply can have gone out, so it
            // measures the prompt alone.
            let at_stop = caller.media().packets_sent();
            (prompt_end, reply.finished().await, at_stop)
        },
        async {
            callee
                .media()
                .record_until_idle(Duration::from_millis(400))
                .await
        }
    );

    assert_eq!(prompt_end, PlaybackEnd::Stopped);
    assert_eq!(
        reply_end,
        PlaybackEnd::Completed,
        "a clip queued behind a stopping one must still play"
    );

    let tail = per_packet * usize::try_from(reply_packets).expect("fits");
    assert!(
        heard.len() >= tail,
        "the far end heard less than the reply: {} samples",
        heard.len()
    );
    assert!(
        heard[heard.len() - tail..].iter().all(|sample| *sample < 0),
        "the reply must be the last thing the far end hears, whole"
    );
    assert!(
        heard[..heard.len() - tail].iter().any(|sample| *sample > 0),
        "the prompt must actually have been playing when it was stopped"
    );

    // The prompt's backlog must not have been played out ahead of the reply: everything the far
    // end heard is what went out before the stop, plus the bound, plus the reply.
    let ceiling = usize::try_from(at_stop + Playback::STOP_BOUND_PACKETS + reply_packets)
        .expect("fits")
        * per_packet;
    assert!(
        heard.len() <= ceiling,
        "the stopped prompt's queued audio played out before the reply: {} samples, ceiling {ceiling}",
        heard.len()
    );
}

/// Every playback resolves on the event stream saying **which** playback it was — a call may have
/// several queued at once, and "a playback finished" on its own does not say which one to move on
/// from.
#[tokio::test]
async fn every_playback_reports_its_own_end_by_id() {
    let (mut caller, _callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");
    let per_packet = caller.media().samples_per_packet();

    let first = caller.start_playback(vec![4_000i16; per_packet * 2], Interrupt::Never);
    let second = caller.start_playback(vec![4_000i16; per_packet * 2], Interrupt::Never);
    assert_ne!(
        first.id(),
        second.id(),
        "two playbacks on one call must be two playbacks"
    );

    assert_eq!(first.finished().await, PlaybackEnd::Completed);
    assert_eq!(second.finished().await, PlaybackEnd::Completed);

    let mut reported = Vec::new();
    while reported.len() < 2 {
        let event = next_matching(&mut events, |event| {
            matches!(event, CallEvent::PlaybackFinished { .. })
        })
        .await;
        if let CallEvent::PlaybackFinished {
            playback,
            completed,
        } = event
        {
            reported.push((playback, completed));
        }
    }

    assert_eq!(
        reported,
        vec![(first.id(), true), (second.id(), true)],
        "each playback must report its own end, in the order they ran"
    );
}

/// Belt and braces on the event half, so the failing-first test above has a sibling that keeps
/// the `C-3` reporting honest once it passes.
#[tokio::test]
async fn an_interrupted_playback_reports_itself_as_cut() {
    let (mut caller, callee) = connected().await;
    let mut events = caller.events().expect("the stream is taken once");
    let per_packet = caller.media().samples_per_packet();

    let prompt = caller.start_playback(loud(per_packet * 250), Interrupt::OnDigit);
    hearing(&callee).await;
    callee.send_digits("2", Duration::from_millis(80)).await;

    let event = next_matching(&mut events, |event| {
        matches!(event, CallEvent::PlaybackFinished { .. })
    })
    .await;
    assert!(
        matches!(
            event,
            CallEvent::PlaybackFinished {
                playback,
                completed: false
            } if playback == prompt.id()
        ),
        "an interrupted playback must report itself, by id, as not completed: {event:?}"
    );
}
