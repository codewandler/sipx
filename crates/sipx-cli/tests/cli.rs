//! `sipx` as an actual process.
//!
//! These run the built binary rather than calling into the library: the point of a command
//! line tool is what a shell sees, and exit codes, stream separation and JSON on stdout are
//! all invisible from inside the process.

// `caller`/`callee` and `answered`/`answerer` are the words this domain uses. Renaming them to
// satisfy a similarity heuristic would make the test harder to read, not easier.
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

use sipx_audio::{Wav, read_wav, write_wav};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

fn sipx() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sipx"))
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("sipx-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// A 440 Hz tone with an envelope, so a recording of silence cannot pass for it.
fn tone(milliseconds: usize) -> Wav {
    let samples = milliseconds * 8;
    Wav::narrowband(
        (0..samples)
            .map(|i| {
                let t = f64::from(u32::try_from(i).unwrap_or(0)) / 8000.0;
                let envelope = (t * 4.0).min(1.0);
                let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
                i16::try_from(value.round() as i32).unwrap_or(0)
            })
            .collect(),
    )
}

/// Start `sipx answer` and wait for it to announce the port it bound.
///
/// Announcing rather than guessing is what makes these tests race-free: the caller starts only
/// once the answerer is listening, and on a port the OS chose.
async fn start_answerer(
    extra: &[&str],
) -> (
    tokio::process::Child,
    String,
    tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) {
    let mut args = vec!["answer", "--local", "127.0.0.1:0", "--json", "--wait", "20"];
    args.extend_from_slice(extra);

    let mut child = sipx()
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawns");

    let stdout = child.stdout.take().expect("piped");
    let mut lines = BufReader::new(stdout).lines();

    let listening = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("no timeout")
        .expect("a line")
        .expect("the address line");
    assert!(
        listening.contains("\"status\":\"listening\""),
        "{listening}"
    );

    let address = listening
        .split("\"address\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an address")
        .to_owned();

    (child, address, lines)
}

#[tokio::test]
async fn version_and_help_succeed() {
    let output = sipx().arg("version").output().await.expect("runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("sipx"));

    let output = sipx().arg("help").output().await.expect("runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("USAGE"));
}

/// An unknown command is a usage error with its own exit code, and the complaint goes to
/// stderr where it will not be parsed as a result.
#[tokio::test]
async fn an_unknown_command_is_a_usage_error_on_stderr() {
    let output = sipx()
        .args(["frobnicate", "--json"])
        .output()
        .await
        .expect("runs");

    assert_eq!(output.status.code(), Some(2), "usage");
    assert!(
        String::from_utf8_lossy(&output.stdout).is_empty(),
        "nothing on stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("frobnicate"), "{stderr}");
    assert!(stderr.contains("\"status\":\"usage\""), "{stderr}");
}

#[tokio::test]
async fn dial_without_a_uri_is_a_usage_error() {
    let output = sipx()
        .args(["dial", "--json"])
        .output()
        .await
        .expect("runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("\"status\":\"usage\""));
}

/// A name with no resolver behind it says what to do about it, rather than failing later in a
/// way that looks like a network problem.
#[tokio::test]
async fn dialling_a_name_explains_what_is_missing() {
    let output = sipx()
        .args(["dial", "sip:bob@example.com", "--json"])
        .output()
        .await
        .expect("runs");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("address and port"), "{stderr}");
}

/// The acceptance test for P-3 and P-4: two `sipx` processes, a real call, and a recording
/// that contains the audio that was played.
#[tokio::test]
async fn dial_plays_a_file_and_records_the_far_end() {
    let dir = scratch("call");
    let from_caller = dir.join("caller.wav");
    let from_callee = dir.join("callee.wav");
    let heard_by_callee_path = dir.join("heard-by-callee.wav");

    write_wav(
        std::fs::File::create(&from_caller).expect("creates"),
        &tone(400),
    )
    .expect("writes");
    write_wav(
        std::fs::File::create(&from_callee).expect("creates"),
        &Wav::narrowband(tone(400).samples.iter().map(|s| -s).collect()),
    )
    .expect("writes");

    let (mut answerer, address, mut lines) = start_answerer(&[
        "--duration",
        "10",
        "--play",
        from_callee.to_str().expect("a path"),
        "--record",
        heard_by_callee_path.to_str().expect("a path"),
    ])
    .await;

    let caller = tokio::time::timeout(
        Duration::from_secs(40),
        sipx()
            .args([
                "dial",
                &format!("sip:answer@{address}"),
                "--local",
                "127.0.0.1:0",
                "--json",
                "--duration",
                "6",
                "--play",
                from_caller.to_str().expect("a path"),
            ])
            .output(),
    )
    .await
    .expect("the caller finishes")
    .expect("runs");

    let caller_out = String::from_utf8_lossy(&caller.stdout);
    assert!(
        caller.status.success(),
        "dial failed: {caller_out} / {}",
        String::from_utf8_lossy(&caller.stderr)
    );
    assert!(
        caller_out.contains("\"status\":\"answered\""),
        "{caller_out}"
    );

    let answered = tokio::time::timeout(Duration::from_secs(25), lines.next_line())
        .await
        .expect("no timeout")
        .expect("a line")
        .expect("the result line");
    assert!(answered.contains("\"status\":\"answered\""), "{answered}");
    let _ = answerer.wait().await;

    // The recording contains the tone, not silence of the right length.
    let heard =
        read_wav(std::fs::File::open(&heard_by_callee_path).expect("opens")).expect("reads");
    assert!(!heard.samples.is_empty(), "the callee recorded nothing");
    let peak = heard
        .samples
        .iter()
        .map(|s| i32::from(s.abs()))
        .max()
        .unwrap_or(0);
    assert!(
        peak > 6000,
        "the recording is too quiet to be the tone: peak {peak}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A refused call gets its own exit code, so a script can tell busy from no-answer without
/// matching on English.
#[tokio::test]
async fn a_busy_answer_gives_the_caller_the_busy_exit_code() {
    let (mut answerer, address, _lines) = start_answerer(&["--busy"]).await;

    let caller = tokio::time::timeout(
        Duration::from_secs(30),
        sipx()
            .args([
                "dial",
                &format!("sip:busy@{address}"),
                "--local",
                "127.0.0.1:0",
                "--json",
                "--duration",
                "5",
            ])
            .output(),
    )
    .await
    .expect("the caller finishes")
    .expect("runs");

    assert_eq!(caller.status.code(), Some(6), "busy has its own exit code");
    let stderr = String::from_utf8_lossy(&caller.stderr);
    assert!(stderr.contains("\"status\":\"busy\""), "{stderr}");
    assert!(
        String::from_utf8_lossy(&caller.stdout).is_empty(),
        "a failure must not land on stdout"
    );

    let _ = answerer.wait().await;
}

/// Logging must never reach stdout, or one `-vv` turns every JSON result into a parse error at
/// the far end of a pipe.
#[tokio::test]
async fn verbose_logging_stays_off_stdout() {
    let output = sipx()
        .args(["dial", "sip:bob@example.com", "--json", "-vv"])
        .output()
        .await
        .expect("runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.is_empty(),
        "stdout must carry results only, got: {stdout}"
    );
}

/// DTMF sent by the caller is reported by the answering side.
#[tokio::test]
async fn digits_sent_by_the_caller_are_reported_by_the_answerer() {
    let (mut answerer, address, mut lines) = start_answerer(&["--duration", "10"]).await;

    let caller = tokio::time::timeout(
        Duration::from_secs(40),
        sipx()
            .args([
                "dial",
                &format!("sip:menu@{address}"),
                "--local",
                "127.0.0.1:0",
                "--json",
                "--duration",
                "8",
                "--dtmf",
                "1234",
            ])
            .output(),
    )
    .await
    .expect("the caller finishes")
    .expect("runs");
    assert!(
        caller.status.success(),
        "{}",
        String::from_utf8_lossy(&caller.stderr)
    );

    let answered = tokio::time::timeout(Duration::from_secs(25), lines.next_line())
        .await
        .expect("no timeout")
        .expect("a line")
        .expect("the result line");
    assert!(
        answered.contains("\"dtmf\":\"1234\""),
        "the keypresses must be reported: {answered}"
    );
    let _ = answerer.wait().await;
}
