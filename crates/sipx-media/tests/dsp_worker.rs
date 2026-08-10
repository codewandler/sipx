//! The supervised worker's process boundary (`docs/specs/call-dsp-graph.md` §7, `M-102`).
//!
//! `M-64` proved everything about the supervised profile that is a protocol: the bounded channels,
//! the deadline counted in frames, the miss budget and the declared action at its expiry. What it
//! could not prove is the boundary those run across, because its worker ran in a thread. These
//! tests are about the boundary itself — that the worker is an operating-system process with a pid
//! of its own and that the audio came from *that* process, that killing it is an ending the
//! declared action handles, that one which will not stop is terminated regardless, and that every
//! ending is followed by a `wait` rather than by a kill anyone hoped landed.
//!
//! They run against the reference worker of §7.5, spawned as a real child process, because a
//! process boundary asserted against a stub is not a process boundary.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::time::Duration;

use sipx_media::dsp::{
    BypassCause, DspCapability, ExecutionPolicy, ExecutionProfile, FailureAction, GraphBounds,
    GraphPlan, GraphTransition, WorkerProcess,
};
use sipx_media::{
    AudioDirection, Codec, Config, MediaPort, MediaSession, PcmEncoding, PcmFormat, PcmSamples,
    Processing,
};
use tokio::net::UdpSocket;

/// How long these tests wait for something to happen before calling it lost.
///
/// A bound on failure, orders of magnitude above the honest answer on an idle machine, and never a
/// window anything is measured in.
const ARRIVAL_BOUND: Duration = Duration::from_secs(10);

const SAMPLES_PER_PACKET: usize = 160;

/// The reference worker of §7.5.
const WORKER: &str = env!("CARGO_BIN_EXE_sipx-dsp-worker");

// ------------------------------------------------------------------------- fixtures ----

/// A supervised stage running the reference worker in the named mode.
fn worker(id: &'static str, mode: &str) -> WorkerProcess {
    WorkerProcess::new(
        DspCapability::new(id)
            .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated)),
        WORKER,
    )
    .arg("--mode")
    .arg(mode)
}

/// The same, with the miss budget and failure action a failure case needs to reach its action.
fn failing(id: &'static str, mode: &str, misses: u32, action: FailureAction) -> WorkerProcess {
    WorkerProcess::new(
        DspCapability::new(id).with_execution(
            ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated)
                .with_max_consecutive_misses(misses)
                .with_on_failure(action),
        ),
        WORKER,
    )
    .arg("--mode")
    .arg(mode)
}

async fn session_and_peer() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

/// A valid session whose packetisation is one frame beyond the DSP contract's largest frame.
#[cfg(target_os = "linux")]
async fn oversized_session_and_peer() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.clock_rate = 384_000;
    config.packet_duration = Duration::from_millis(172);
    config.rtcp_interval = None;
    assert_eq!(config.samples_per_packet(), 66_048);
    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

fn signed(samples: &PcmSamples) -> &[i16] {
    match samples {
        PcmSamples::Signed16(samples) => samples,
        other => panic!("expected signed samples, got {other:?}"),
    }
}

/// A tone whose every sample doubles without saturating, so a gain of two is visible by value.
fn tone() -> Vec<i16> {
    (0..i16::try_from(SAMPLES_PER_PACKET).expect("a packet fits an i16"))
        .map(|n| n * 16)
        .collect()
}

fn doubled() -> Vec<i16> {
    tone().iter().map(|n| n.saturating_mul(2)).collect()
}

/// Whether the operating system still has an entry for this process.
///
/// A reaped process has none: §7.3's fourth step is a `wait`, and a worker that was killed but not
/// waited for is a zombie, which is a worker that still exists.
#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// Wait until the operating system has no entry for this process.
///
/// An event and never a duration. It is what makes a failure case deterministic without measuring
/// anything: a worker's ending reaches the stage when its pipe closes, and a test that went on
/// driving frames while the operating system was still tearing the process down would be counting
/// deadlines against the machine's speed rather than against the ending it means to assert.
#[cfg(target_os = "linux")]
async fn reaped(pid: u32) {
    while alive(pid) {
        tokio::task::yield_now().await;
    }
}

// ----------------------------------------------------------------------------- tests ----

/// GRAPH-28: the plan door owns the session frame size, so it refuses an out-of-contract sizing
/// before the supervised door can spawn a worker from it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_out_of_contract_frame_is_refused_before_a_worker_is_spawned() {
    let (session, _peer, _addr) = oversized_session_and_peer().await;
    let attempted = session.attach_dsp(
        GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_supervised(failing(
            "oversized-frame",
            "gain",
            3,
            FailureAction::BypassOpen,
        )),
    );

    let graph = match attempted {
        Err(error) => {
            assert_eq!(
                error,
                sipx_media::dsp::GraphError::FrameSamplesOutOfRange {
                    value: 66_048,
                    bound: 65_536,
                }
            );
            session.shutdown().await;
            return;
        }
        Ok(graph) => graph,
    };
    let pid = graph.worker_pids()[0];
    let _ = graph.transitions();

    let format = PcmFormat::new(384_000, PcmEncoding::Signed16).expect("a supported format");
    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, format))
        .expect("attaches");
    // The first offer lets the pump observe the worker's `Hello` refusal. Reaping is the event
    // which orders that refusal before the two frames that spend the rest of the miss budget.
    assert!(session.send(tone()).await);
    tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("the first miss never holds the call's RTP")
        .expect("a frame");
    reaped(pid).await;

    for _ in 0..2 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..2 {
        tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("a lost worker never holds the call's RTP")
            .expect("a frame");
    }

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                cause: BypassCause::WorkerLost,
                ..
            }
        )),
        "the worker's own Hello refusal is retained as defence in depth: {transitions:?}"
    );
    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("the refused worker is still reaped");
    assert!(barrier.is_clear(), "{barrier:?}");
    session.shutdown().await;
    panic!("an out-of-contract graph was admitted and failed later as WorkerLost");
}

/// Acceptance row 1: the worker is an operating-system process and not a thread, and the audio the
/// call carried came out of that process.
#[tokio::test]
async fn a_supervised_worker_runs_in_its_own_operating_system_process() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_supervised(worker("reports-its-pid", "pid")),
        )
        .expect("activates");

    let pids = graph.worker_pids();
    assert_eq!(pids.len(), 1, "one supervised stage, one process");
    let pid = pids[0];
    assert_ne!(
        pid,
        std::process::id(),
        "a supervised worker must run in an operating-system process of its own, not in the media \
         process: `docs/specs/custom-call-dsp.md` §7.2's containment claim is written against a \
         process that can be bounded and killed, and a thread is neither"
    );

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    for _ in 0..3 {
        assert!(session.send(tone()).await);
    }
    // The first frame fills the pipeline and passes through unprocessed; the second carries the
    // worker's own answer, and that answer says which process wrote it.
    let mut frames = Vec::new();
    for _ in 0..2 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("arrives")
            .expect("a frame");
        frames.push(signed(sent.pcm().samples()).to_vec());
    }
    assert_eq!(
        frames[0],
        tone(),
        "the pipeline's first frame is unprocessed"
    );
    let low = u16::from_ne_bytes(frames[1][0].to_ne_bytes());
    let high = u16::from_ne_bytes(frames[1][1].to_ne_bytes());
    assert_eq!(
        u32::from(low) | (u32::from(high) << 16),
        pid,
        "the process the runtime reports is the process the DSP ran in"
    );

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("detaching terminates and reaps the worker");
    assert!(barrier.is_clear(), "{barrier:?}");
    assert!(
        graph.worker_pids().is_empty(),
        "a reaped worker is not a process any more"
    );
    #[cfg(target_os = "linux")]
    assert!(!alive(pid), "the process was reaped, not merely killed");

    session.shutdown().await;
}

/// GRAPH-16: a worker killed from outside is an ending the declared action handles, and nothing is
/// respawned behind the application's back.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_worker_killed_from_outside_costs_its_declared_action_and_is_never_respawned() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph =
        session
            .attach_dsp(
                GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_supervised(
                    failing("killed-from-outside", "gain", 3, FailureAction::BypassOpen),
                ),
            )
            .expect("activates");
    let pid = graph.worker_pids()[0];

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    // Two frames first, so the stage is demonstrably carrying audio before it is killed.
    for _ in 0..2 {
        assert!(session.send(tone()).await);
    }
    for expected in [tone(), doubled()] {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("arrives")
            .expect("a frame");
        assert_eq!(signed(sent.pcm().samples()), expected.as_slice());
    }

    let killed = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status()
        .expect("the operating system can signal a process this test's runtime owns");
    assert!(killed.success(), "{killed:?}");

    // One frame into the dead process. The pump waits for the call's frames rather than for the
    // worker, so writing to a pipe with no reader is what carries the ending back — and it costs
    // this one deadline.
    assert!(session.send(tone()).await);
    let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
        .await
        .expect("arrives")
        .expect("a frame");
    // Whatever the pipeline still held for it: the stage lags its input by its deadline (§7.1), so
    // this frame's result was produced before the kill and is not a statement about it.
    let drained = signed(sent.pcm().samples()).to_vec();
    assert!(drained == tone() || drained == doubled(), "{drained:?}");
    tokio::time::timeout(ARRIVAL_BOUND, reaped(pid))
        .await
        .expect("a killed worker is reaped by the runtime that spawned it, and not left a zombie");

    // RTP keeps flowing, carrying unmodified audio, which is what `BypassOpen` costs.
    for _ in 0..4 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..4 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("a killed worker never holds the call's RTP")
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            tone().as_slice(),
            "fail-open: unprocessed audio keeps flowing, and it is audible rather than hidden"
        );
    }

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                cause: BypassCause::WorkerLost,
                ..
            }
        )),
        "a killed worker is `WorkerLost`, not a deadline: {transitions:?}"
    );
    assert_eq!(
        graph.worker_pids(),
        vec![pid],
        "the stage still names the process that died: a worker is never respawned behind the \
         application's back, because a respawned process has none of the state the last one built"
    );

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("a dead worker is still reaped");
    assert!(barrier.is_clear(), "{barrier:?}");
    session.shutdown().await;
}

/// GRAPH-17: a worker stuck inside one frame costs its frames and is then terminated regardless —
/// the reap does not wait for it to agree.
#[tokio::test]
async fn a_worker_that_will_not_stop_is_terminated_and_reaped_regardless() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_supervised(
                failing("hangs", "hang", 2, FailureAction::BypassOpen)
                    .arg("--after")
                    .arg("1"),
            ),
        )
        .expect("activates");
    let pid = graph.worker_pids()[0];

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    for _ in 0..6 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..6 {
        tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("a hung worker never holds the call's RTP")
            .expect("a frame");
    }

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                cause: BypassCause::DeadlineMissed,
                ..
            }
        )),
        "{transitions:?}"
    );

    // The worker is inside a frame it will never return from. Nothing it does can end it, so the
    // teardown's own kill is what does — and the barrier is clear on the far side of a `wait`.
    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("a worker that will not stop is killed rather than waited for");
    assert!(barrier.is_clear(), "{barrier:?}");
    assert_eq!(barrier.workers(), 0);
    #[cfg(target_os = "linux")]
    assert!(!alive(pid), "the process was reaped, not merely killed");
    let _ = pid;

    session.shutdown().await;
}

/// GRAPH-18: a well-formed answer carrying the wrong number of positions is a malformed result and
/// not audio.
#[tokio::test]
async fn a_worker_answering_with_the_wrong_position_count_is_malformed() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_supervised(failing(
                "one-sample-short",
                "short",
                // Two, so the pipeline-filling miss of the first frame is not itself the budget:
                // what must trip this is the malformed answer, not the frame before it (§7.1).
                2,
                FailureAction::BypassOpen,
            )),
        )
        .expect("activates");

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    for _ in 0..4 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..4 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("arrives")
            .expect("a frame");
        assert_eq!(
            signed(sent.pcm().samples()),
            tone().as_slice(),
            "a short answer is a miss, never a short frame on the wire"
        );
    }

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                cause: BypassCause::MalformedResult,
                ..
            }
        )),
        "{transitions:?}"
    );

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("reaped");
    assert!(barrier.is_clear(), "{barrier:?}");
    session.shutdown().await;
}

/// GRAPH-16, the self-inflicted half: a worker that crashes mid-call is the same ending as one
/// that is killed, and the media process outlives it.
///
/// The crash is a real `SIGABRT`, core dump and all, which is exactly why this waits for the
/// process to be gone before it counts anything: how long an operating system takes to tear a
/// crashing process down is a property of the machine, and the ending is not.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_crashing_worker_is_an_ending_the_media_process_survives() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new()).with_supervised(
                failing("crashes", "crash", 3, FailureAction::BypassOpen)
                    .arg("--after")
                    .arg("1"),
            ),
        )
        .expect("activates");
    let pid = graph.worker_pids()[0];

    let mut transmitted = session
        .attach_processor(Processing::new(AudioDirection::Outbound, narrowband()))
        .expect("attaches");

    // The worker gains the first frame and crashes inside the second.
    for _ in 0..2 {
        assert!(session.send(tone()).await);
    }
    for expected in [tone(), doubled()] {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("arrives")
            .expect("a frame");
        assert_eq!(signed(sent.pcm().samples()), expected.as_slice());
    }
    tokio::time::timeout(ARRIVAL_BOUND, reaped(pid))
        .await
        .expect("a crashed worker is reaped by the runtime that spawned it, and not left a zombie");

    for _ in 0..4 {
        assert!(session.send(tone()).await);
    }
    for _ in 0..4 {
        let sent = tokio::time::timeout(ARRIVAL_BOUND, transmitted.recv())
            .await
            .expect("a crashed worker never holds the call's RTP")
            .expect("a frame");
        assert_eq!(signed(sent.pcm().samples()), tone().as_slice());
    }

    let transitions = graph.transitions();
    assert!(
        transitions.iter().any(|transition| matches!(
            transition,
            GraphTransition::Bypassed {
                cause: BypassCause::WorkerLost,
                ..
            }
        )),
        "a crashed worker is lost, and it is reported: {transitions:?}"
    );

    let barrier = tokio::time::timeout(ARRIVAL_BOUND, graph.detach())
        .await
        .expect("a crashed worker is still reaped");
    assert!(barrier.is_clear(), "{barrier:?}");
    session.shutdown().await;
}

/// §7.3: a graph that is dropped rather than detached still reaps its worker process.
///
/// A leaked worker per call would be worse than the thread this replaced, so the reap is not
/// something only the polite teardown path does.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_dropped_graph_still_reaps_its_worker_process() {
    let (session, _peer, _addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_supervised(worker("dropped", "gain")),
        )
        .expect("activates");
    let pid = graph.worker_pids()[0];
    assert!(alive(pid));

    assert!(session.send(tone()).await);
    drop(graph);

    // The drop cannot await, so the reap happens on the thread that supervises the process. What is
    // waited for here is the fact, never a duration.
    let reaped = tokio::time::timeout(ARRIVAL_BOUND, async {
        while alive(pid) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(
        reaped.is_ok(),
        "a dropped graph left process {pid} behind, which is one leaked worker per call"
    );

    session.shutdown().await;
}

/// §7.4's third obligation, exercised against the reference worker directly and against the octets
/// §7.4 writes rather than against this crate's own encoder.
///
/// This is what bounds an *orphaned* worker: if the media process dies without running any of
/// §7.3's steps, the only thing left is the end of file the operating system delivers on the
/// worker's input — and a worker stuck inside a frame must still act on it.
#[tokio::test]
async fn a_worker_ends_at_end_of_file_even_inside_a_frame_it_will_never_return_from() {
    use std::io::Write;

    let mut child = Command::new(WORKER)
        .arg("--mode")
        .arg("hang")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the reference worker is a program");

    let mut input = child.stdin.take().expect("piped");
    // §7.4's `Hello`: magic, type, reserved, length; then version, direction, rate, channels and
    // the ceiling. Hand-written from the spec, so this asserts the implementation against the
    // document and not against itself.
    let mut hello = vec![0x53, 0x44, 0x01, 0x00];
    hello.extend_from_slice(&12_u32.to_be_bytes());
    hello.extend_from_slice(&1_u16.to_be_bytes());
    hello.push(2);
    hello.extend_from_slice(&8_000_u32.to_be_bytes());
    hello.push(1);
    hello.extend_from_slice(&4_u32.to_be_bytes());
    input.write_all(&hello).expect("writes");

    // §7.4's `Frame`: sequence, position, discontinuity, sample count, samples.
    let mut frame = vec![0x53, 0x44, 0x02, 0x00];
    frame.extend_from_slice(&25_u32.to_be_bytes());
    frame.extend_from_slice(&0_u64.to_be_bytes());
    frame.extend_from_slice(&0_u64.to_be_bytes());
    frame.push(0);
    frame.extend_from_slice(&2_u32.to_be_bytes());
    frame.extend_from_slice(&1_i16.to_be_bytes());
    frame.extend_from_slice(&2_i16.to_be_bytes());
    input.write_all(&frame).expect("writes");
    input.flush().expect("flushes");

    // The worker is now inside a frame it will never return from. Closing its input is the whole of
    // what an orphaned worker ever gets, and it must be enough.
    drop(input);

    let status = tokio::time::timeout(
        ARRIVAL_BOUND,
        tokio::task::spawn_blocking(move || child.wait()),
    )
    .await
    .expect("a conforming worker ends when its input ends, whatever its processing is doing")
    .expect("joins")
    .expect("waits");
    assert!(status.success(), "{status:?}");
}
