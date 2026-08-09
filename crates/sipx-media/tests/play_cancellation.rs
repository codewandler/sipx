//! Stopping a task that is playing into a session, or reading from one (`M-93`).
//!
//! `JoinHandle::abort()` is the ordinary way to stop background work in this runtime, so a media
//! API that cannot survive one will be found the hard way by whoever writes the first real
//! application. `M-93` was filed from a measurement at the call layer — ten calls whose tone tasks
//! were stopped by `abort()` had not finished tearing down after ninety seconds, against four for
//! the same run carrying no audio — and its hypothesis was that a `play` future dropped mid-clip
//! leaves a resource the teardown then waits on.
//!
//! The measurement stands and the hypothesis does not. Aborting a task parked *inside* `play` works
//! and always has: the clip is stopped, its queued packets are discarded, and the session tears
//! down in milliseconds. What wedges is the other order — the **call ending underneath a loop that
//! is still playing**. Every long-running entry point here answers immediately once the session has
//! stopped, and answers without ever suspending, so `loop { play().await }` stops being paced and
//! becomes a bare busy loop. A task that never reaches a suspension point cannot be cancelled: its
//! `abort()` never lands, `JoinHandle::await` never returns, and even dropping the runtime blocks,
//! because a running task is not a task the scheduler can drop. That is the resource — **a runtime
//! worker**, held indefinitely — and it is what the teardown path is really waiting on.
//!
//! Each test therefore forces its runtime down rather than dropping it. An ordinary drop would
//! *hang* on the defect instead of reporting it, and a test that hangs says nothing.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::sync::Arc;
use std::time::Duration;

use sipx_media::{Codec, Config, MediaPort, MediaSession};
use tokio::net::UdpSocket;

/// What a call's teardown gives the send queue before it gives up on it — the bound
/// `Call::begin_end` passes to [`MediaSession::flush`]. Restated rather than imported, so this file
/// states the contract instead of restating whatever the code currently computes.
const CALL_FLUSH_BOUND: Duration = Duration::from_secs(5);

/// How long an aborted task may still be running.
///
/// A bound on failure and not a measurement (`X-29`): the honest answer is one scheduler hop, and
/// the behaviour this bounds is unbounded rather than slow. Two seconds is far enough above the
/// former to be quiet on a loaded machine and far enough below the latter to be decisive.
const REAP_BOUND: Duration = Duration::from_secs(2);

/// What tearing one session down may cost once nothing is left that has to be waited for.
///
/// The send queue holds at most 64 frames of 20 ms, so 1.28 s is the most that pacing every one of
/// them onto the wire could cost — and the point of stopping a playback is that none of them is
/// paced at all.
const TEARDOWN_BOUND: Duration = Duration::from_secs(2);

/// How long a test waits for audio to reach the wire before calling the playback broken. A bound on
/// failure, orders of magnitude above the packetisation interval.
const AUDIO_BOUND: Duration = Duration::from_secs(10);

/// One second of 8 kHz audio: fifty 20 ms packets, so a task aborted at an arbitrary moment is
/// overwhelmingly likely to be parked inside a clip rather than between two.
fn tone() -> Vec<i16> {
    (0..8_000)
        .map(|i| {
            let t = f64::from(i) / 8_000.0;
            ((t * 440.0 * std::f64::consts::TAU).sin() * 8_000.0) as i16
        })
        .collect()
}

/// Run `body` on a runtime that is then forced down rather than dropped.
///
/// The force is the point. A task spinning in a loop that never suspends cannot be dropped by the
/// scheduler, so an ordinary `Runtime` drop blocks in the destructor and the test never reports
/// anything at all. [`tokio::runtime::Runtime::shutdown_timeout`] gives up on such a task and
/// leaks its thread for the few seconds the test binary has left, which is what turns this defect
/// into a failed assertion instead of a hung run. Nothing is leaked once the defect is fixed:
/// every task here ends at the first suspension point after its `abort()`.
fn on_a_runtime<T>(body: impl std::future::Future<Output = T>) -> T {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime");
    let outcome = runtime.block_on(body);
    runtime.shutdown_timeout(Duration::from_secs(1));
    outcome
}

/// A session sending to a socket nobody reads, which is all the send path needs.
async fn session_and_peer() -> (Arc<MediaSession>, UdpSocket) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    // Nothing here reads RTCP, and a report loop is one more worker competing for the runtime
    // while the teardown under test is in flight.
    config.rtcp_interval = None;
    (
        Arc::new(port.start(config).expect("valid media setup")),
        peer,
    )
}

/// Wait until something has happened, rather than sleeping and assuming it has (`X-29`).
async fn until(within: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition() {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        // A polling interval, not a wait: the assertions are on the condition and the deadline.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// The teardown a call performs, in `Call::begin_end`'s order and with its bound.
async fn tear_down(session: &MediaSession) {
    session.flush(CALL_FLUSH_BOUND).await;
    session.stop();
    session.shutdown().await;
}

/// A task holding audio up for the life of a call, the way an application does it.
fn hold_audio_up(session: &Arc<MediaSession>) -> tokio::task::JoinHandle<()> {
    let session = Arc::clone(session);
    let clip = tone();
    tokio::spawn(async move {
        loop {
            session.play(&clip, session.samples_per_packet()).await;
        }
    })
}

/// The story's stated shape: a task parked inside `play` is aborted, and the session is then torn
/// down the way a call tears one down.
///
/// This half has always worked — [`sipx_media::Playback::play_out`] stops the clip when its wait is
/// dropped — and it is asserted here so that the fix for the half that did not cannot cost it.
#[test]
fn a_task_aborted_inside_play_is_reaped_and_its_session_torn_down_within_the_bound() {
    let (reaped, teardown) = on_a_runtime(async {
        let (session, _peer) = session_and_peer().await;
        let playing = hold_audio_up(&session);

        // Abort only once audio is demonstrably on the wire, so the task is certainly inside a
        // clip rather than still starting up.
        until(AUDIO_BOUND, "the session never sent a packet", || {
            session.packets_sent() > 0
        })
        .await;
        playing.abort();

        let started = tokio::time::Instant::now();
        tear_down(&session).await;
        let teardown = started.elapsed();
        let reaped = tokio::time::timeout(REAP_BOUND, playing).await.is_ok();
        (reaped, teardown)
    });

    assert!(reaped, "the task aborted inside `play` was never reaped");
    assert!(
        teardown < TEARDOWN_BOUND,
        "tearing down after an aborted play took {teardown:?}, over the {TEARDOWN_BOUND:?} bound"
    );
}

/// The shape that wedges: the call ends underneath a loop that is still playing, and only then is
/// the task told to stop.
///
/// This is the order a hangup actually happens in — `Call::begin_end` flushes and stops the media
/// before anything joins the application's own tasks — and it is the order the capacity measurement
/// hit. Once the session has stopped, [`MediaSession::play`] answers `false` on the first poll
/// without suspending, so the loop above stops being paced by the send queue and becomes a busy
/// loop. `abort()` on a task that never suspends never lands.
#[test]
fn a_task_playing_when_its_call_ended_can_still_be_aborted() {
    let reaped = on_a_runtime(async {
        let (session, _peer) = session_and_peer().await;
        let playing = hold_audio_up(&session);

        until(AUDIO_BOUND, "the session never sent a packet", || {
            session.packets_sent() > 0
        })
        .await;

        // The call ends under the playing task. `shutdown` is awaited so that the playback queue
        // is provably gone before the abort, which is what makes this deterministic rather than a
        // race against the teardown.
        tear_down(&session).await;

        playing.abort();
        tokio::time::timeout(REAP_BOUND, playing).await.is_ok()
    });

    assert!(
        reaped,
        "a task looping over `play` after its session stopped was still running {REAP_BOUND:?} \
         after `abort()`; it holds a runtime worker and nothing can take it back"
    );
}

/// The same question for the receiving side (`MediaSession::recv`), which
/// [`MediaSession::record_at_least`] and [`MediaSession::record_until_idle`] are both loops over.
///
/// A stopped session's inbound queue answers `None` immediately and, like `play`, without
/// suspending — so a task reading in a loop is the same busy loop by a different route and has to
/// be settled the same way.
#[test]
fn a_task_receiving_when_its_call_ended_can_still_be_aborted() {
    let reaped = on_a_runtime(async {
        let (session, _peer) = session_and_peer().await;
        let reading = tokio::spawn({
            let session = Arc::clone(&session);
            async move {
                loop {
                    session.recv().await;
                }
            }
        });

        // Parked on an empty queue is the only state this task can be in before the session stops,
        // and that state is reached by the first poll.
        until(AUDIO_BOUND, "the session never started", || {
            session.packets_sent() > 0 || !session.is_stopped()
        })
        .await;

        tear_down(&session).await;

        reading.abort();
        tokio::time::timeout(REAP_BOUND, reading).await.is_ok()
    });

    assert!(
        reaped,
        "a task looping over `recv` after its session stopped was still running {REAP_BOUND:?} \
         after `abort()`"
    );
}
