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
    let mut command = Command::new(env!("CARGO_BIN_EXE_sipx"));
    // If an assertion fires while a child is running, the future is dropped but the process is
    // not — and a sipx that goes on retransmitting outlives the test binary. On CI that reads
    // as a hung job on top of whatever actually failed.
    command.kill_on_drop(true);
    command
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

/// The first header line with this name, without its terminator.
fn header_line<'a>(message: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name}:");
    message
        .split("\r\n")
        .find(|line| {
            line.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(&prefix))
        })
        .unwrap_or_else(|| panic!("no {name} header in:\n{message}"))
}

/// A REGISTER must name *this* client in its Via and Contact. The Via sent-by is where the
/// sender expects responses (RFC 3261 §18.1.1), and the Contact is the binding the registrar
/// stores and routes calls to (RFC 3261 §10.2.6) — an unspecified address in either sends
/// traffic nowhere.
#[tokio::test]
async fn register_advertises_this_client_in_via_and_contact() {
    let registrar = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let address = registrar.local_addr().expect("has an address");

    let mut child = sipx()
        .args([
            "register",
            "sip:alice@example.com",
            "--target",
            &address.to_string(),
            "--json",
            "--expires",
            "60",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawns");

    let mut buf = vec![0u8; 65_535];
    let (length, source) =
        tokio::time::timeout(Duration::from_secs(10), registrar.recv_from(&mut buf))
            .await
            .expect("a REGISTER arrives")
            .expect("reads");
    let request = String::from_utf8_lossy(&buf[..length]).into_owned();
    let _ = child.kill().await;

    assert!(request.starts_with("REGISTER"), "{request}");

    let contact = header_line(&request, "Contact");
    assert!(
        !contact.contains("0.0.0.0"),
        "a binding at the unspecified address routes inbound calls nowhere: {contact}"
    );
    assert!(
        contact.contains(&source.to_string()),
        "the Contact must be where this client listens ({source}): {contact}"
    );

    let via = header_line(&request, "Via");
    assert!(
        via.contains(&source.to_string()),
        "the Via sent-by must name the sender ({source}), not anyone else: {via}"
    );
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
                "--timeout",
                "15",
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
                "--timeout",
                "10",
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

/// A final response to an INVITE must carry a To tag (RFC 3261 §8.2.6.2): the tag is what
/// lets a caller behind a forking proxy tell one branch's refusal from another's.
#[tokio::test]
async fn a_refusal_carries_a_to_tag() {
    let (mut answerer, address, _lines) = start_answerer(&["--busy"]).await;

    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let port = socket.local_addr().expect("has an address").port();
    let unique = std::process::id();
    let invite = format!(
        "INVITE sip:answer@{address} SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{port};branch=z9hG4bKrefusal{unique};rport\r\n\
         Max-Forwards: 70\r\n\
         From: <sip:caller@127.0.0.1:{port}>;tag=refusal{unique}\r\n\
         To: <sip:answer@{address}>\r\n\
         Call-ID: refusal-{unique}@127.0.0.1\r\n\
         CSeq: 1 INVITE\r\n\
         Contact: <sip:caller@127.0.0.1:{port}>\r\n\
         Content-Length: 0\r\n\
         \r\n"
    );
    socket
        .send_to(invite.as_bytes(), &address)
        .await
        .expect("sends");

    // Provisional responses are the one place a To tag is optional, so read past them to the
    // final one.
    let mut buf = vec![0u8; 65_535];
    let response = loop {
        let (length, _) = tokio::time::timeout(Duration::from_secs(10), socket.recv_from(&mut buf))
            .await
            .expect("a response arrives")
            .expect("reads");
        let response = String::from_utf8_lossy(&buf[..length]).into_owned();
        if !response.starts_with("SIP/2.0 1") {
            break response;
        }
    };

    assert!(response.starts_with("SIP/2.0 486"), "{response}");
    let to = header_line(&response, "To");
    assert!(
        to.contains("tag="),
        "a final response needs a To tag to identify its branch: {to}"
    );

    let _ = answerer.kill().await;
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
                "--timeout",
                "15",
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

/// Calling something that never answers gives up on the caller's schedule rather than on the
/// transaction layer's. 64*T1 is 32 seconds — correct for SIP, and far too long for a script
/// that wanted either an answer or an error.
#[tokio::test]
async fn a_call_that_is_never_answered_times_out_on_schedule() {
    // A UDP socket that accepts packets and never replies.
    let black_hole = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let address = black_hole.local_addr().expect("has an address");

    let started = std::time::Instant::now();
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:nobody@{address}"),
                "--local",
                "127.0.0.1:0",
                "--json",
                "--timeout",
                "3",
            ])
            .output(),
    )
    .await
    .expect("must not wait for the transaction timeout")
    .expect("runs");

    assert_eq!(
        output.status.code(),
        Some(5),
        "timeout has its own exit code"
    );
    assert!(
        started.elapsed() < Duration::from_secs(12),
        "gave up after {:?}, which is the transaction's schedule rather than ours",
        started.elapsed()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"status\":\"timeout\""), "{stderr}");
}

/// A flag's value must never be read as the URI. `sipx dial --timeout 30 sip:bob@host` tried
/// to call "30" until `--timeout` was registered as taking a value.
#[tokio::test]
async fn a_valued_flag_before_the_uri_is_not_mistaken_for_it() {
    let output = sipx()
        .args([
            "dial",
            "--timeout",
            "1",
            "--local",
            "127.0.0.1:0",
            "--json",
            "sip:bob@192.0.2.1:5060",
        ])
        .output()
        .await
        .expect("runs");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("must name an address"),
        "the timeout value was read as the URI: {stderr}"
    );
    // 192.0.2.1 is TEST-NET-1 and answers nothing, so this is a timeout rather than usage.
    assert_eq!(output.status.code(), Some(5), "{stderr}");
}

/// The acceptance test for P-5: a name written into the peer book comes back out of
/// `sipx peers`, in both forms, carrying the source it came from.
#[tokio::test]
async fn a_peer_written_to_the_book_is_listed_by_name() {
    let dir = scratch("peers-list");
    let book = dir.join("peers");
    // Written the way a shell script would write it: append a line, no library, no escaping.
    std::fs::write(
        &book,
        "# who this phone knows about\nalice   sip:alice@192.0.2.17:5060\n",
    )
    .expect("writes");

    let json = sipx()
        .args(["peers", "--book", book.to_str().expect("a path"), "--json"])
        .output()
        .await
        .expect("runs");
    let stdout = String::from_utf8_lossy(&json.stdout);
    assert!(
        json.status.success(),
        "peers failed: {stdout} / {}",
        String::from_utf8_lossy(&json.stderr)
    );

    assert_eq!(
        stdout.lines().count(),
        1,
        "one line per peer, so a reader can split on newlines: {stdout}"
    );
    assert!(stdout.contains("\"name\":\"alice\""), "{stdout}");
    assert!(
        stdout.contains("\"uri\":\"sip:alice@192.0.2.17:5060\""),
        "an entry must carry enough to dial it: {stdout}"
    );
    assert!(
        stdout.contains("\"source\":\"book\""),
        "an entry must say which source it came from, or S-24 and T-24 cannot be merged in: \
         {stdout}"
    );

    // The human form carries the same facts.
    let text = sipx()
        .args(["peers", "--book", book.to_str().expect("a path")])
        .output()
        .await
        .expect("runs");
    let human = String::from_utf8_lossy(&text.stdout);
    assert!(text.status.success(), "{human}");
    for fact in ["alice", "sip:alice@192.0.2.17:5060", "book"] {
        assert!(human.contains(fact), "{fact} missing from {human}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// A source that cannot be read is a failure, not an empty list. An empty list on a fresh
/// machine reads as "nobody to call" when the truth is "you have not been told about anyone",
/// and the design is explicit that a partial list must never be presented as complete.
#[tokio::test]
async fn a_peer_book_that_cannot_be_read_is_an_error_not_an_empty_list() {
    let dir = scratch("peers-missing");
    let missing = dir.join("not-there");

    let output = sipx()
        .args([
            "peers",
            "--book",
            missing.to_str().expect("a path"),
            "--json",
        ])
        .output()
        .await
        .expect("runs");

    assert_eq!(output.status.code(), Some(1), "a read failure is not zero");
    assert!(
        String::from_utf8_lossy(&output.stdout).is_empty(),
        "a failure must not land on stdout where it would be parsed as a result"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\"status\":\"failed\""), "{stderr}");
    assert!(
        stderr.contains("not-there"),
        "the error must name the path it tried: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A non-numeric option is a usage error, not a silent fall back to the default — which would
/// restore exactly the behaviour the flag exists to prevent.
#[tokio::test]
async fn a_non_numeric_timeout_is_a_usage_error() {
    let output = sipx()
        .args([
            "dial",
            "sip:bob@192.0.2.1:5060",
            "--timeout",
            "3s",
            "--json",
        ])
        .output()
        .await
        .expect("runs");

    assert_eq!(output.status.code(), Some(2), "usage");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--timeout"), "{stderr}");
    assert!(stderr.contains("whole number"), "{stderr}");
}
