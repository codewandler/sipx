//! The two bounds on `--record`, and what went wrong when they were one (`X-40`).
//!
//! `X-40` was filed as a test-hygiene story: `cli.rs`'s recording assertion looked like a test that
//! asserted on a real-time side effect after waiting for a different event. The shape was right and
//! the location was wrong — the defect was in production, in `crate::record`'s two predecessors:
//!
//! ```text
//! // crates/sipx-cli/src/answer.rs, before
//! tokio::time::timeout(duration, media.record_until_idle(Duration::from_millis(500)))
//!     .await
//!     .unwrap_or_default()
//! ```
//!
//! That is two defects on one line, and each of them turns a call that carried audio into a recording
//! of **zero** samples. Each has a test here, because each is reachable on its own.
//!
//! 1. **One window for two questions.** `record_until_idle` spends its 500 ms both on "how long
//!    until the stream starts" and on "how long a gap means it ended". A first frame delayed past it
//!    — two jitter buffers filling on a loaded machine — leaves the loop before its first iteration.
//!    `MediaSession::record_at_least`'s "Why this exists (`X-28`)" predicted this exactly: "a
//!    recording of zero samples — not a degraded one". `X-28` fixed the library and left its only
//!    production caller on the old primitive.
//! 2. **`unwrap_or_default` on the cap.** A far end still talking when the call's time is up makes
//!    the outer `timeout` fire, and `unwrap_or_default` then replaced everything recorded so far with
//!    silence — losing the whole recording rather than returning a short one.
//!
//! Both tests place the call from the library rather than from a second `sipx` process, for one
//! reason: what they have to control is the *timing* of the audio — when it starts, and that it is
//! still going when the cap fires — and the command line has no flag for either. Nothing else about
//! the calls is unusual. They connect, negotiate, carry audio and hang up.

// `answered`/`answerer` are the words this domain uses, as `cli.rs` says at more length.
#![allow(clippy::similar_names)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]

use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::read_wav;
use sipx_call::{DialOptions, dial};
use sipx_sip::Uri;
use sipx_transport::{Config as TransportConfig, Target, bind};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

#[path = "support/uplift.rs"]
mod uplift;

/// 8 kHz mono, which is what G.711 carries and what `--record` writes.
const SAMPLE_RATE: usize = 8000;

fn loopback() -> std::net::IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// A 440 Hz tone with an envelope, so a recording of silence cannot pass for it.
fn tone(milliseconds: usize) -> Vec<i16> {
    (0..milliseconds * SAMPLE_RATE / 1000)
        .map(|i| {
            let t = f64::from(u32::try_from(i).unwrap_or(0)) / SAMPLE_RATE as f64;
            let envelope = (t * 4.0).min(1.0);
            let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
            i16::try_from(value.round() as i32).unwrap_or(0)
        })
        .collect()
}

/// What the answerer recorded, and the line it reported.
struct Heard {
    samples: usize,
    report: String,
}

/// Place a call at a `sipx answer --record`, play a `clip`-millisecond tone starting `after`, and
/// report what the answerer wrote to disk.
///
/// `hang_up_after` is the answerer's `--duration`, which is the cap on its recording.
async fn record_a_call(case: &str, hang_up_after: u64, after: Duration, clip: usize) -> Heard {
    // Every assertion below is about what reached a WAV file. A binary built without this run's
    // features answers those with silence, which is what this file is here to distinguish (`X-121`).
    uplift::assert_binary_matches_this_build();
    let dir = std::env::temp_dir().join(format!("sipx-cli-{}-{case}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let recording = dir.join("heard-by-callee.wav");

    // Bind the caller before starting the answerer's wait-for-call clock. On the reported flake,
    // this setup competed with every other workspace suite after the answerer had already announced
    // its address, so `--wait` stood in for the caller becoming ready.
    let mut config = TransportConfig::new("127.0.0.1:0".parse().expect("valid"));
    config.sent_by = loopback().to_string();
    let (handle, _incoming) = bind(config).await.expect("binds");

    let mut answerer = Command::new(env!("CARGO_BIN_EXE_sipx"))
        .args([
            "answer",
            "--local",
            "127.0.0.1:0",
            "--json",
            "--wait",
            // A bound on failure: if a caller that is already bound cannot send an INVITE in five
            // minutes, the harness is stuck. This is deliberately orders of magnitude above the
            // honest loopback answer and never stands in for readiness.
            "300",
            "--duration",
            &hang_up_after.to_string(),
            "--record",
            recording.to_str().expect("a path"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawns");

    let stdout = answerer.stdout.take().expect("piped");
    let mut stderr = answerer.stderr.take().expect("piped");
    let mut lines = BufReader::new(stdout).lines();
    let listening = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("no timeout")
        .expect("a line")
        .expect("the address line");
    let address = listening
        .split("\"address\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an address")
        .to_owned();

    let to = Uri::parse(Bytes::from(format!("sip:answer@{address}"))).expect("a SIP URI");
    let options = DialOptions::new("<sip:caller@127.0.0.1>", loopback())
        .with_timeout(Duration::from_secs(15));
    let mut call = dial(
        &handle,
        Target::udp(address.parse().expect("an address")),
        &to,
        &options,
    )
    .await
    .expect("the answerer accepts the call");

    // Played concurrently with reading the report, because the point of the last case is that the
    // audio is still going when the answerer's cap fires.
    let media = call.media();
    let (_, report) = tokio::join!(
        async {
            tokio::time::sleep(after).await;
            media.play(&tone(clip), 160).await
        },
        async {
            tokio::time::timeout(Duration::from_secs(40), lines.next_line())
                .await
                .expect("the answerer reports rather than hanging")
                .expect("reads the answerer's stdout")
        }
    );

    let _ = call.hang_up().await;
    let status = tokio::time::timeout(Duration::from_secs(30), answerer.wait())
        .await
        .expect("the answerer exits")
        .expect("waits");
    let mut error = String::new();
    stderr
        .read_to_string(&mut error)
        .await
        .expect("reads stderr");
    let report = report.unwrap_or_else(|| {
        let exit_path = if status.code() == Some(5) {
            "the wait-for-call bound expired before an INVITE arrived"
        } else if status.success() {
            "the answerer exited successfully without emitting its result"
        } else if status.code().is_none() {
            "the answerer was terminated by a signal"
        } else {
            "the answerer failed before emitting its result"
        };
        panic!(
            "answerer stdout closed before the result line: {exit_path}; exit={status}; stderr={error:?}"
        );
    });
    assert!(
        status.success(),
        "the answerer exited with {status}; these tests are about what it recorded, not about it \
         crashing: report={report}; stderr={error:?}"
    );

    let heard = read_wav(std::fs::File::open(&recording).expect("opens")).expect("reads");
    let samples = heard.samples.len();
    let _ = std::fs::remove_dir_all(&dir);
    Heard { samples, report }
}

/// The control for the two cases below: audio that starts at once is recorded. A failure here means
/// the harness is wrong rather than the answerer, and without it neither case below would mean much.
#[tokio::test]
async fn audio_that_starts_at_once_is_recorded() {
    let heard = record_a_call("at-once", 10, Duration::ZERO, 400).await;
    assert!(
        heard.samples > 0,
        "the control case recorded nothing, so nothing else in this file proves anything: {}",
        heard.report
    );
}

/// Defect 1: a caller whose audio starts 1.5 s into the call is a caller a loaded machine produces,
/// and every sample it sent used to be discarded.
///
/// The claim is deliberately the weakest one that fails against the defect: not "all of it", not
/// "loudly enough" — only that *something* was recorded from a call that carried 400 ms of tone with
/// ten seconds to do it in.
#[tokio::test]
async fn audio_that_starts_late_is_recorded_too() {
    let heard = record_a_call("late", 10, Duration::from_millis(1500), 400).await;
    assert!(
        heard.samples > 0,
        "the call carried 400 ms of tone and the answerer recorded none of it: waiting for the \
         first frame must not share a window with deciding the stream has ended: {}",
        heard.report
    );
}

/// Defect 2: a far end still talking when the call's time is up leaves a *partial* recording, and a
/// partial recording must survive.
///
/// Six seconds of tone into an answerer that hangs up after two. The cap has to fire — there is no
/// 500 ms gap anywhere in the clip for the idle window to find — so this is the timed-out path every
/// time, which is what `unwrap_or_default` used to turn into silence.
///
/// Both bounds are asserted, and the upper one is not decoration: without it this test would pass
/// just as well if the cap never fired at all, and it would then be asserting nothing about the path
/// it is named for.
#[tokio::test]
async fn a_recording_cut_short_by_the_cap_is_kept() {
    let cap = 2;
    let heard = record_a_call("cut-short", cap, Duration::ZERO, 6000).await;

    assert!(
        heard.samples > SAMPLE_RATE / 2,
        "the call's time ran out mid-stream and the answerer kept only {} samples; a recording the \
         cap cut short is still the audio the call carried, and must not be replaced by silence: {}",
        heard.samples,
        heard.report
    );
    let whole_clip = 6 * SAMPLE_RATE;
    assert!(
        heard.samples < whole_clip,
        "the answerer recorded {} samples of a {whole_clip}-sample clip despite being told to hang \
         up after {cap}s, so the cap never fired and this case did not exercise it: {}",
        heard.samples,
        heard.report
    );
}
