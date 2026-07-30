//! What `X-40` turned out to be about: `sipx answer --record` decides in its first half second
//! whether it will record anything at all.
//!
//! `X-40` was filed against `cli.rs`'s recording assertion, on the reasoning that the test asserted
//! on a real-time side effect after waiting for a different event. The reasoning was right and the
//! location was not. The assertion cannot be made load-proof from the test side, because the window
//! that decides it belongs to the answerer:
//!
//! ```text
//! // crates/sipx-cli/src/answer.rs
//! tokio::time::timeout(duration, media.record_until_idle(Duration::from_millis(500)))
//! ```
//!
//! `record_until_idle` spends one window on two different questions — how long to wait for the
//! stream to *start*, and how long a gap means it has *ended*. `MediaSession::record_at_least`
//! exists precisely because that is unsound, and its documentation (`sipx-media/src/session.rs`,
//! "Why this exists (`X-28`)") describes this failure exactly: "The observed result is a recording
//! of **zero** samples — not a degraded one — because once the first frame lands the rest follow at
//! the packet rate."
//!
//! So the answerer's `--duration 10` is not what bounds the recording. The 500 ms is. A first frame
//! that arrives later than it leaves the loop before its first iteration, and the answerer writes a
//! valid WAV with zero samples, reports `"status":"answered"` and exits **0** — which is what
//! `cli.rs`'s "the callee recorded nothing" was reporting, and why no exit-status assertion catches
//! it.
//!
//! This test pins that. It places the call from the library rather than from a second `sipx`
//! process, because the one thing it has to control is *when* the audio starts, and the command line
//! has no flag for it. Nothing about the call is broken: it connects, negotiates, carries audio and
//! is hung up normally. Only the audio's timing changes, which is the one variable load moves.
//!
//! **Ignored because it is red, and it is red because the defect is real.** The fix is in
//! `crates/sipx-cli/src/answer.rs`, which `X-40` was not scoped to touch; un-ignore it with that
//! change and it becomes the regression test for it.

// `answered`/`answerer` are the words this domain uses, as `cli.rs` says at more length.
#![allow(clippy::similar_names)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::read_wav;
use sipx_call::{DialOptions, dial};
use sipx_sip::Uri;
use sipx_transport::{Config as TransportConfig, Target, bind};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

fn loopback() -> std::net::IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// A 440 Hz tone with an envelope, so a recording of silence cannot pass for it.
fn tone(milliseconds: usize) -> Vec<i16> {
    let samples = milliseconds * 8;
    (0..samples)
        .map(|i| {
            let t = f64::from(u32::try_from(i).unwrap_or(0)) / 8000.0;
            let envelope = (t * 4.0).min(1.0);
            let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
            i16::try_from(value.round() as i32).unwrap_or(0)
        })
        .collect()
}

/// What the answerer recorded, and the line it reported, for a call whose audio starts after
/// `delay`.
///
/// The answerer is given `--duration 10`, far longer than anything here needs, so that a recording
/// which stops early cannot be blamed on the duration the test asked for.
async fn record_with_audio_starting_after(delay: Duration) -> (usize, String) {
    let dir = std::env::temp_dir().join(format!(
        "sipx-cli-{}-late-media-{}",
        std::process::id(),
        delay.as_millis()
    ));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let recording = dir.join("heard-by-callee.wav");

    let mut answerer = Command::new(env!("CARGO_BIN_EXE_sipx"))
        .args([
            "answer",
            "--local",
            "127.0.0.1:0",
            "--json",
            "--wait",
            "20",
            "--duration",
            "10",
            "--record",
            recording.to_str().expect("a path"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawns");

    let stdout = answerer.stdout.take().expect("piped");
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

    let mut config = TransportConfig::new("127.0.0.1:0".parse().expect("valid"));
    config.sent_by = loopback().to_string();
    let (handle, _incoming) = bind(config).await.expect("binds");

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

    // The only variable: when the audio starts. Everything else is an ordinary call.
    tokio::time::sleep(delay).await;
    call.media().play(&tone(400), 160).await;

    let answered = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("the answerer reports rather than hanging")
        .expect("a line")
        .expect("the result line");

    let _ = call.hang_up().await;
    let status = tokio::time::timeout(Duration::from_secs(30), answerer.wait())
        .await
        .expect("the answerer exits")
        .expect("waits");
    assert!(
        status.success(),
        "the answerer exited with {status}; this test is about what it recorded, not about it \
         crashing: {answered}"
    );

    let heard = read_wav(std::fs::File::open(&recording).expect("opens")).expect("reads");
    let samples = heard.samples.len();
    let _ = std::fs::remove_dir_all(&dir);
    (samples, answered)
}

/// The control. Audio that starts immediately is recorded, so the two cases below differ in one
/// thing only — and a failure here would mean the harness is wrong rather than the answerer.
#[tokio::test]
async fn audio_that_starts_at_once_is_recorded() {
    let (samples, answered) = record_with_audio_starting_after(Duration::ZERO).await;
    assert!(
        samples > 0,
        "the control case recorded nothing, so this file proves nothing about timing: {answered}"
    );
}

/// The defect. A caller whose audio starts 1.5 s into the call is a caller a loaded machine
/// produces, and every sample it sends is discarded.
///
/// The claim is deliberately the weakest one that still fails: not "all of it", not "loudly
/// enough" — just that *something* was recorded from a call that carried 400 ms of tone for 10 s.
#[tokio::test]
#[ignore = "red: pins X-40's root cause in src/answer.rs; un-ignore with the fix"]
async fn audio_that_starts_late_is_recorded_too() {
    let (samples, answered) = record_with_audio_starting_after(Duration::from_millis(1500)).await;
    assert!(
        samples > 0,
        "the call carried 400 ms of tone and the answerer recorded none of it, because its \
         500 ms idle window is also how long it waits for the first frame: {answered}"
    );
}
