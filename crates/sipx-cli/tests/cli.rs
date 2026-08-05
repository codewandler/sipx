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
use sipx_sip::{HeaderName, Method, StatusCode};
use sipx_testkit::certs::Ca;
use sipx_transport::{Config as TransportConfig, bind};
use sipx_ua::{Authenticator, Presented, Verdict};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{Semaphore, SemaphorePermit};

/// One real command-line scenario at a time in this test binary.
///
/// A scenario may and usually does run several `sipx` processes concurrently. Running every
/// scenario concurrently as well multiplies that into dozens of media workers, then makes a
/// short clip's delivery depend on whether its worker is scheduled before the command's real call
/// duration expires. The permit is a capacity/readiness barrier, not a delay: the next scenario
/// starts when the previous one's processes have exited.
static PROCESS_SCENARIOS: Semaphore = Semaphore::const_new(1);

async fn process_scenario() -> SemaphorePermit<'static> {
    PROCESS_SCENARIOS
        .acquire()
        .await
        .expect("the CLI process-scenario semaphore remains open")
}

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
    tone_at(8_000, milliseconds, 440.0)
}

/// A deterministic signal at a media clock, with an onset envelope so silence cannot pass.
fn tone_at(sample_rate: u32, milliseconds: usize, frequency: f64) -> Wav {
    let samples = milliseconds * usize::try_from(sample_rate).unwrap_or(0) / 1_000;
    Wav {
        sample_rate,
        samples: (0..samples)
            .map(|i| {
                let t = f64::from(u32::try_from(i).unwrap_or(0)) / f64::from(sample_rate);
                let envelope = (t * 4.0).min(1.0);
                let value = (t * frequency * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
                i16::try_from(value.round() as i32).unwrap_or(0)
            })
            .collect(),
    }
}

/// Squared projection on one frequency, used to distinguish the two ends of a real call.
#[cfg(feature = "opus")]
fn spectral_power(wav: &Wav, frequency: f64) -> f64 {
    let angular = 2.0 * std::f64::consts::PI * frequency / f64::from(wav.sample_rate);
    let (sine, cosine) =
        wav.samples
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(sine, cosine), (index, sample)| {
                let phase = angular * f64::from(u32::try_from(index).unwrap_or(0));
                let sample = f64::from(*sample);
                (sine + sample * phase.sin(), cosine + sample * phase.cos())
            });
    sine.mul_add(sine, cosine * cosine)
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
    start_answerer_in(None, extra).await
}

/// As [`start_answerer`], but running the answerer in a directory of the test's choosing.
///
/// Only [`no_capture_flag_means_no_file`] needs this, and it needs it for a specific reason: an
/// assertion that *no* file was written has to know where a file could have appeared, and a file
/// nobody named can only land at a path compiled into the binary — which is a relative one. Giving
/// the process an empty directory of its own turns "no file at this path" into "no file at all".
async fn start_answerer_in(
    dir: Option<&std::path::Path>,
    extra: &[&str],
) -> (
    tokio::process::Child,
    String,
    tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
) {
    let mut args = vec!["answer", "--local", "127.0.0.1:0", "--json", "--wait", "20"];
    args.extend_from_slice(extra);

    let mut command = sipx();
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let mut child = command
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

/// Wait for the answerer to exit, and hold it to a clean exit.
///
/// Every one of these tests used to end `let _ = answerer.wait().await`, which threw the exit
/// status away and made every assertion after it ambiguous (`X-40`): "the callee recorded nothing"
/// could not distinguish media that never flowed from an answerer that died before it could record
/// anything, and the second is a different defect with a different fix. Whatever the assertions
/// below are about, they are about a process that ran to completion — so that is asserted rather
/// than assumed.
///
/// The wait is bounded, so an answerer that never exits is a named failure instead of a suite that
/// hangs until the harness kills it. Thirty seconds is a bound on failure and not a measurement:
/// every answerer here is started with a `--wait`/`--duration` well inside it.
///
/// Its stderr comes along because a status on its own says a process failed without saying why, and
/// the whole point of reading the status is diagnosis.
async fn answerer_exits_cleanly(answerer: &mut tokio::process::Child) {
    let complaint = drain_stderr(answerer).await;
    exits_cleanly(answerer, &complaint).await;
}

/// Everything the answerer has written to stderr, read to end of stream.
async fn drain_stderr(answerer: &mut tokio::process::Child) -> String {
    let mut complaint = Vec::new();
    if let Some(mut stderr) = answerer.stderr.take() {
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut complaint).await;
    }
    String::from_utf8_lossy(&complaint).into_owned()
}

/// The waiting half of [`answerer_exits_cleanly`], for callers that have already read stderr.
///
/// It is split out because a caller that wants *both* of the answerer's streams has to read them
/// concurrently — a process whose stderr goes unread while its stdout is drained to end of stream
/// can block on a full pipe and reach neither — so it cannot let this function do the reading.
async fn exits_cleanly(
    answerer: &mut tokio::process::Child,
    complaint: &str,
) -> std::process::ExitStatus {
    let status = tokio::time::timeout(Duration::from_secs(30), answerer.wait())
        .await
        .expect("the answerer exits rather than hanging")
        .expect("waits");
    assert!(
        status.success(),
        "the answerer exited with {status}, so anything asserted about what it recorded, heard or \
         captured describes a process that failed: {complaint}"
    );
    status
}

/// Certificate material written where the CLI can consume it through its public file flags.
struct TlsFixture {
    ca: std::path::PathBuf,
    cert: std::path::PathBuf,
    key: std::path::PathBuf,
}

fn tls_fixture(name: &str) -> TlsFixture {
    let dir = scratch(name);
    let authority = Ca::new();
    let (cert, key) = authority.issue_for("sipx.test");
    let ca = dir.join("ca.pem");
    let cert_path = dir.join("server.pem");
    let key_path = dir.join("server.key");
    std::fs::write(&ca, authority.pem()).expect("writes the CA");
    std::fs::write(&cert_path, cert).expect("writes the certificate");
    std::fs::write(&key_path, key).expect("writes the private key");
    TlsFixture {
        ca,
        cert: cert_path,
        key: key_path,
    }
}

/// A finite fake STUN server that reports the source address it actually observed.
async fn start_stun_server() -> (
    std::net::SocketAddr,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("STUN server binds");
    let address = socket.local_addr().expect("STUN address");
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        let (stop_relays, _) = tokio::sync::watch::channel(false);
        let mut relays = tokio::task::JoinSet::new();
        let mut stopped = std::pin::pin!(stopped);
        let mut datagram = [0u8; 1_500];
        loop {
            let received = tokio::select! {
                _ = &mut stopped => break,
                received = socket.recv_from(&mut datagram) => received,
            };
            let Ok((length, source)) = received else {
                break;
            };
            let Some(transaction) = datagram
                .get(..length)
                .and_then(|packet| packet.get(8..20))
                .and_then(|bytes| <[u8; 12]>::try_from(bytes).ok())
            else {
                continue;
            };
            // A loopback source address is already a host candidate and would be deduplicated.
            // Give it a distinct, functioning mapped port: the relay below is the fixture's
            // finite stand-in for the address/port mapping a STUN client is trying to discover.
            let mapped = tokio::net::UdpSocket::bind("127.0.0.1:0")
                .await
                .expect("mapped port binds");
            let mapped_address = mapped.local_addr().expect("mapped address");
            relays.spawn(mapped_relay(mapped, source, stop_relays.subscribe()));
            let response = stun_binding_response(transaction, mapped_address);
            let _ = socket.send_to(&response, source).await;
        }
        let _ = stop_relays.send(true);
        while relays.join_next().await.is_some() {}
    });
    (address, stop, serving)
}

/// Forward one finite fake address mapping in both directions until its STUN server stops.
async fn mapped_relay(
    socket: tokio::net::UdpSocket,
    internal: std::net::SocketAddr,
    mut stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut peer = None;
    let mut datagram = vec![0u8; 65_535];
    loop {
        let received = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return;
                }
                continue;
            }
            received = socket.recv_from(&mut datagram) => received,
        };
        let Ok((length, source)) = received else {
            return;
        };
        let destination = if source == internal {
            let Some(peer) = peer else {
                continue;
            };
            peer
        } else {
            peer = Some(source);
            internal
        };
        let _ = socket.send_to(&datagram[..length], destination).await;
    }
}

/// RFC 5389 §15.2's XOR-MAPPED-ADDRESS in a Binding success response.
fn stun_binding_response(transaction: [u8; 12], mapped: std::net::SocketAddr) -> Vec<u8> {
    let std::net::SocketAddr::V4(mapped) = mapped else {
        panic!("the loopback fixture is IPv4");
    };
    let cookie = sipx_transport::stun::MAGIC_COOKIE;
    let mut value = vec![0u8, 0x01];
    value.extend_from_slice(
        &(mapped.port() ^ u16::try_from(cookie >> 16).expect("cookie half")).to_be_bytes(),
    );
    value.extend_from_slice(&(u32::from(*mapped.ip()) ^ cookie).to_be_bytes());

    let mut message = vec![0x01, 0x01];
    message.extend_from_slice(
        &u16::try_from(value.len() + 4)
            .expect("small attribute")
            .to_be_bytes(),
    );
    message.extend_from_slice(&cookie.to_be_bytes());
    message.extend_from_slice(&transaction);
    message.extend_from_slice(&0x0020u16.to_be_bytes());
    message.extend_from_slice(
        &u16::try_from(value.len())
            .expect("small address")
            .to_be_bytes(),
    );
    message.extend_from_slice(&value);
    message
}

async fn dead_media_path() -> (tokio::net::UdpSocket, std::net::SocketAddr) {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("dead media socket binds");
    let address = socket.local_addr().expect("dead media address");
    (socket, address)
}

/// Replace the high-priority/default host path with a bound socket nobody reads while retaining
/// the STUN-discovered server-reflexive candidate. Component two is omitted so the scenario has
/// one selected path and one fact to assert.
fn silence_host_path(message: &[u8], dead: std::net::SocketAddr) -> Vec<u8> {
    let text = String::from_utf8_lossy(message);
    let (headers, body) = text.split_once("\r\n\r\n").expect("SIP has a body");
    assert!(
        body.contains("typ srflx"),
        "STUN produced a candidate:\n{body}"
    );

    let mut rewritten = Vec::new();
    for line in body.lines() {
        if line.starts_with("c=IN IP") {
            rewritten.push(format!("c=IN IP4 {}", dead.ip()));
        } else if let Some(rest) = line.strip_prefix("m=audio ") {
            let (_, tail) = rest.split_once(' ').expect("media line fields");
            rewritten.push(format!("m=audio {} {tail}", dead.port()));
        } else if let Some(candidate) = line.strip_prefix("a=candidate:") {
            let fields = candidate.split_whitespace().collect::<Vec<_>>();
            if fields.get(1) == Some(&"1") && fields.get(7) == Some(&"srflx") {
                rewritten.push(line.to_owned());
            }
        } else if !line.is_empty() {
            rewritten.push(line.to_owned());
        }
    }
    rewritten.push(format!(
        "a=candidate:dead 1 UDP 2130706431 {} {} typ host",
        dead.ip(),
        dead.port()
    ));
    let body = format!("{}\r\n", rewritten.join("\r\n"));
    let headers = headers
        .lines()
        .map(|line| {
            if line
                .split_once(':')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            {
                format!("Content-Length: {}", body.len())
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    format!("{headers}\r\n\r\n{body}").into_bytes()
}

/// `DPH-1`, plus the cleartext transports around it: every released signalling transport is
/// selected through the command line and carries a complete, bounded call. The assertions are on
/// both processes' terminal reports so a flag accepted and then ignored cannot pass.
#[tokio::test]
async fn dph_1_every_released_transport_carries_a_loopback_command_call() {
    let _scenario = process_scenario().await;
    let tls = tls_fixture("dph-1");
    let ca = tls.ca.to_string_lossy().into_owned();
    let cert = tls.cert.to_string_lossy().into_owned();
    let key = tls.key.to_string_lossy().into_owned();

    for transport in ["udp", "tcp", "tls", "ws", "wss"] {
        let mut answer_args = vec!["--transport", transport, "--duration", "1"];
        if matches!(transport, "tls" | "wss") {
            answer_args.extend_from_slice(&["--tls-cert", &cert, "--tls-key", &key]);
        }
        let (mut answerer, address, mut lines) = start_answerer(&answer_args).await;

        let uri = format!("sip:bob@{address}");
        let mut dialer = sipx();
        dialer.args([
            "dial",
            &uri,
            "--transport",
            transport,
            "--duration",
            "1",
            "--timeout",
            "5",
            "--json",
        ]);
        if matches!(transport, "tls" | "wss") {
            dialer.args(["--tls-ca", &ca, "--tls-server-name", "sipx.test"]);
        }
        let output = tokio::time::timeout(Duration::from_secs(15), dialer.output())
            .await
            .unwrap_or_else(|_| panic!("{transport} dial is bounded"))
            .expect("dial runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{transport} dial failed: {stdout} / {stderr}"
        );
        assert!(
            stdout.contains(&format!("\"requested_transport\":\"{transport}\"")),
            "{transport}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("\"negotiated_transport\":\"{transport}\"")),
            "{transport}: {stdout}"
        );

        let answered = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .unwrap_or_else(|_| panic!("{transport} answer is bounded"))
            .expect("reads answer report")
            .expect("answer report exists");
        assert!(
            answered.contains(&format!("\"requested_transport\":\"{transport}\"")),
            "{transport}: {answered}"
        );
        assert!(
            answered.contains(&format!("\"negotiated_transport\":\"{transport}\"")),
            "{transport}: {answered}"
        );
        answerer_exits_cleanly(&mut answerer).await;
    }
}

/// `DPH-2`: trusting the issuer is insufficient when the requested identity is wrong. WSS must
/// return a typed TLS failure and must not retry over WS, TCP or UDP.
#[tokio::test]
async fn dph_2_wss_name_mismatch_fails_without_downgrade() {
    let _scenario = process_scenario().await;
    let tls = tls_fixture("dph-2");
    let ca = tls.ca.to_string_lossy().into_owned();
    let cert = tls.cert.to_string_lossy().into_owned();
    let key = tls.key.to_string_lossy().into_owned();
    let (mut answerer, address, _lines) =
        start_answerer(&["--transport", "wss", "--tls-cert", &cert, "--tls-key", &key]).await;

    let uri = format!("sip:bob@{address}");
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        sipx()
            .args([
                "dial",
                &uri,
                "--transport",
                "wss",
                "--tls-ca",
                &ca,
                "--tls-server-name",
                "wrong.test",
                "--timeout",
                "5",
                "--json",
            ])
            .output(),
    )
    .await
    .expect("the refused dial is bounded")
    .expect("dial runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "name mismatch connected: {stdout} / {stderr}"
    );
    assert!(stderr.contains("\"status\":\"failed\""), "{stderr}");
    assert!(
        stderr.contains("certificate") || stderr.contains("tls handshake"),
        "the typed failure names TLS verification: {stderr}"
    );
    assert!(
        !stdout.contains("\"negotiated_transport\"")
            && !stderr.contains("\"negotiated_transport\""),
        "{stdout} / {stderr}"
    );

    answerer.kill().await.expect("stops the answerer");
    let _ = answerer.wait().await;
}

/// `DPH-3`: a known codec that this binary cannot run is rejected before even one signalling
/// datagram leaves. The socket is read only after the process exits, so an empty queue is a causal
/// assertion rather than a sleep standing in for one.
#[cfg(not(feature = "opus"))]
#[tokio::test]
async fn dph_3_opus_without_the_feature_fails_before_network_io() {
    let observer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("observer binds");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:bob@{address}"),
            "--codec",
            "opus",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{complaint}");
    assert!(complaint.contains("`opus` feature"), "{complaint}");
    let mut datagram = [0u8; 1];
    assert!(
        observer.try_recv_from(&mut datagram).is_err(),
        "an unsupported codec reached signalling"
    );
}

/// The positive command-process Opus claim: two distinguishable 48 kHz signals cross in opposite
/// directions. Rate, duration and identity are all asserted, so an 8 kHz header, a 160-sample
/// answer frame, a one-way path or a merely non-empty recording cannot satisfy this case.
#[cfg(feature = "opus")]
#[tokio::test(flavor = "multi_thread")]
async fn diagnostic_phone_opus_is_rate_and_direction_correct() {
    const CALLER_HZ: f64 = 431.0;
    const ANSWER_HZ: f64 = 947.0;

    let _scenario = process_scenario().await;
    let dir = scratch("opus");
    let caller_input = dir.join("caller-input.wav");
    let answer_input = dir.join("answer-input.wav");
    let heard_by_answer = dir.join("heard-by-answer.wav");
    let heard_by_dial = dir.join("heard-by-dial.wav");
    write_wav(
        std::fs::File::create(&caller_input).expect("creates caller input"),
        &tone_at(48_000, 1_000, CALLER_HZ),
    )
    .expect("writes caller input");
    write_wav(
        std::fs::File::create(&answer_input).expect("creates answer input"),
        &tone_at(48_000, 1_000, ANSWER_HZ),
    )
    .expect("writes answer input");

    let (mut answerer, address, mut lines) = start_answerer(&[
        "--codec",
        "opus",
        "--duration",
        "3",
        "--play",
        answer_input.to_str().expect("answer input path"),
        "--record",
        heard_by_answer.to_str().expect("answer recording path"),
    ])
    .await;
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:answer@{address}"),
                "--codec",
                "opus",
                "--duration",
                "3",
                "--timeout",
                "8",
                "--play",
                caller_input.to_str().expect("caller input path"),
                "--record",
                heard_by_dial.to_str().expect("dial recording path"),
                "--json",
            ])
            .output(),
    )
    .await
    .expect("Opus call is bounded")
    .expect("dial runs");
    let dial_report = String::from_utf8_lossy(&output.stdout);
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{dial_report} / {complaint}");
    assert!(
        dial_report.contains("\"requested_codecs\":\"opus\"")
            && dial_report.contains("\"negotiated_codec\":\"opus\""),
        "{dial_report}"
    );

    let answer_report = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("answer report is bounded")
        .expect("reads answer report")
        .expect("answer report exists");
    assert!(
        answer_report.contains("\"requested_codecs\":\"opus\"")
            && answer_report.contains("\"negotiated_codec\":\"opus\"")
            && answer_report.contains("\"heard_audio\":true"),
        "{answer_report}"
    );
    answerer_exits_cleanly(&mut answerer).await;
    for (path, expected_hz, local_hz, direction) in [
        (&heard_by_answer, CALLER_HZ, ANSWER_HZ, "dial to answer"),
        (&heard_by_dial, ANSWER_HZ, CALLER_HZ, "answer to dial"),
    ] {
        let heard =
            read_wav(std::fs::File::open(path).expect("opens recording")).expect("reads recording");
        assert_eq!(heard.sample_rate, 48_000, "{direction}: WAV media clock");
        assert!(
            (44_160..=48_000).contains(&heard.samples.len()),
            "{direction}: expected 920-1000 ms, got {} samples ({:.1} ms)",
            heard.samples.len(),
            f64::from(u32::try_from(heard.samples.len()).unwrap_or(u32::MAX)) * 1_000.0
                / f64::from(heard.sample_rate)
        );
        let expected = spectral_power(&heard, expected_hz);
        let local = spectral_power(&heard, local_hz);
        assert!(
            expected > local * 20.0,
            "{direction}: recording does not identify the far-end signal ({expected} versus {local})"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The executable, not only the call library, consumes a reliable provisional answer and records
/// its media before the final response. The fixture sends that final response only after playback
/// completes, so the reported early samples are a causal assertion with no sleep standing in for
/// ordering.
#[tokio::test(flavor = "multi_thread")]
async fn diagnostic_phone_records_reliable_provisional_audio_before_final_answer() {
    let _scenario = process_scenario().await;
    let dir = scratch("early-media");
    let recorded = dir.join("recorded.wav");
    let (callee, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("loopback address"),
    ))
    .await
    .expect("callee binds");
    let address = callee.local_addr();

    let answering = tokio::spawn(async move {
        let invite = incoming.recv().await.expect("INVITE arrives");
        let mut ringing = sipx_call::ring_early(
            &callee,
            &invite,
            183,
            "Session Progress",
            "127.0.0.1".parse().expect("loopback"),
        )
        .await
        .expect("starts reliable provisional media");
        let prack = incoming.recv().await.expect("PRACK arrives");
        assert!(
            ringing.on_prack(&prack).await.expect("handles PRACK"),
            "the diagnostic phone acknowledged the provisional answer"
        );
        let media = ringing.media().expect("early media is running");
        let clip = tone(1_200);
        assert!(
            media.play(&clip.samples, media.samples_per_packet()).await,
            "early announcement completes before the final answer"
        );
        sipx_call::answer_early(&callee, &invite, &mut ringing)
            .await
            .expect("sends final answer after early playback")
    });

    let output = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:early@{address}"),
                "--early-media",
                "--duration",
                "1",
                "--timeout",
                "8",
                "--record",
                recorded.to_str().expect("recording path"),
                "--json",
            ])
            .output(),
    )
    .await
    .expect("early-media call is bounded")
    .expect("dial runs");
    let report = String::from_utf8_lossy(&output.stdout);
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{report} / {complaint}");
    assert!(report.contains("\"early_media\":true"), "{report}");
    assert!(
        report.contains("\"early_samples_recorded\":")
            && !report.contains("\"early_samples_recorded\":0"),
        "{report}"
    );
    assert!(report.contains("\"heard_audio\":true"), "{report}");
    let _callee_call = answering.await.expect("answering task joins");
    let heard = read_wav(std::fs::File::open(&recorded).expect("opens recording"))
        .expect("reads recording");
    assert!(!heard.samples.is_empty(), "WAV contains early media");
}

/// `DPH-4`: SDES carries its master key in SDP, so selecting it over UDP is a setup error and no
/// INVITE may be emitted.
#[tokio::test]
async fn dph_4_explicit_sdes_over_udp_fails_before_network_io() {
    let observer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("observer binds");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:bob@{address}"),
            "--media-security",
            "sdes",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{complaint}");
    assert!(complaint.contains("requires protected"), "{complaint}");
    let mut datagram = [0u8; 1];
    assert!(
        observer.try_recv_from(&mut datagram).is_err(),
        "an unsafe keying selection reached signalling"
    );
}

/// Strict `plain` and `sdes` remain distinct even on the same protected signalling path. This is
/// a real call assertion on `Call::is_encrypted`, surfaced through the terminal result, not an SDP
/// string check in the command layer.
#[tokio::test(flavor = "multi_thread")]
async fn explicit_plain_and_sdes_report_what_the_tls_calls_actually_negotiated() {
    let _scenario = process_scenario().await;
    let tls = tls_fixture("media-security");
    let ca = tls.ca.to_string_lossy().into_owned();
    let cert = tls.cert.to_string_lossy().into_owned();
    let key = tls.key.to_string_lossy().into_owned();

    for (selected, negotiated) in [("plain", "plain"), ("sdes", "sdes")] {
        let (mut answerer, address, mut lines) = start_answerer(&[
            "--transport",
            "tls",
            "--tls-cert",
            &cert,
            "--tls-key",
            &key,
            "--media-security",
            selected,
            "--duration",
            "1",
        ])
        .await;
        let output = sipx()
            .args([
                "dial",
                &format!("sip:answer@{address}"),
                "--transport",
                "tls",
                "--tls-ca",
                &ca,
                "--tls-server-name",
                "sipx.test",
                "--media-security",
                selected,
                "--duration",
                "1",
                "--timeout",
                "5",
                "--json",
            ])
            .output()
            .await
            .expect("dial runs");
        let dial_report = String::from_utf8_lossy(&output.stdout);
        let complaint = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{dial_report} / {complaint}");
        assert!(
            dial_report.contains(&format!("\"negotiated_media_security\":\"{negotiated}\"")),
            "{dial_report}"
        );
        let answer_report = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await
            .expect("answer report is bounded")
            .expect("reads answer report")
            .expect("answer report exists");
        assert!(
            answer_report.contains(&format!("\"negotiated_media_security\":\"{negotiated}\"")),
            "{answer_report}"
        );
        answerer_exits_cleanly(&mut answerer).await;
    }
}

/// `DPH-5`: both command processes select DTLS-SRTP, report it from their running calls, and
/// carry a real clip through the encrypted media session.
#[cfg(feature = "dtls")]
#[tokio::test(flavor = "multi_thread")]
async fn dph_5_explicit_dtls_srtp_negotiates_and_carries_audio() {
    let _scenario = process_scenario().await;
    let dir = scratch("dph-5");
    let played = dir.join("played.wav");
    let recorded = dir.join("recorded.wav");
    let clip = tone(1_500);
    write_wav(
        std::fs::File::create(&played).expect("creates input"),
        &clip,
    )
    .expect("writes input");

    let (mut answerer, address, mut lines) = start_answerer(&[
        "--media-security",
        "dtls-srtp",
        "--duration",
        "3",
        "--record",
        recorded.to_str().expect("recording path"),
    ])
    .await;
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:answer@{address}"),
                "--media-security",
                "dtls-srtp",
                "--duration",
                "3",
                "--timeout",
                "8",
                "--play",
                played.to_str().expect("input path"),
                "--json",
            ])
            .output(),
    )
    .await
    .expect("DTLS call is bounded")
    .expect("dial runs");
    let dial_report = String::from_utf8_lossy(&output.stdout);
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{dial_report} / {complaint}");
    assert!(
        dial_report.contains("\"requested_media_security\":\"dtls-srtp\"")
            && dial_report.contains("\"negotiated_media_security\":\"dtls-srtp\""),
        "{dial_report}"
    );

    let answer_report = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("answer report is bounded")
        .expect("reads answer report")
        .expect("answer report exists");
    assert!(
        answer_report.contains("\"requested_media_security\":\"dtls-srtp\"")
            && answer_report.contains("\"negotiated_media_security\":\"dtls-srtp\""),
        "{answer_report}"
    );
    assert!(
        answer_report.contains("\"heard_audio\":true"),
        "{answer_report}"
    );
    answerer_exits_cleanly(&mut answerer).await;
    let heard = read_wav(std::fs::File::open(&recorded).expect("opens recording"))
        .expect("reads recording");
    assert!(!heard.samples.is_empty(), "encrypted media carried audio");
}

/// `M-49`: the public diagnostic commands are the executable offerer and answerer proof roles.
/// Their JSON is built from established `Call` and selected-component facts, not requested flags.
#[cfg(all(feature = "dtls", feature = "opus"))]
#[tokio::test(flavor = "multi_thread")]
async fn browser_audio_profile_runs_both_cli_roles_and_reports_nominated_facts() {
    let _scenario = process_scenario().await;
    let tls = tls_fixture("browser-audio-profile");
    let ca = tls.ca.to_string_lossy().into_owned();
    let cert = tls.cert.to_string_lossy().into_owned();
    let key = tls.key.to_string_lossy().into_owned();

    let (mut answerer, address, mut lines) = start_answerer(&[
        "--transport",
        "wss",
        "--tls-cert",
        &cert,
        "--tls-key",
        &key,
        "--profile",
        "browser-audio",
        "--duration",
        "1",
    ])
    .await;
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:answer@{address}"),
                "--transport",
                "wss",
                "--tls-ca",
                &ca,
                "--tls-server-name",
                "sipx.test",
                "--profile",
                "browser-audio",
                "--duration",
                "1",
                "--timeout",
                "8",
                "--json",
            ])
            .output(),
    )
    .await
    .expect("browser-audio CLI proof is bounded")
    .expect("offerer runs");
    let offerer = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{offerer} / {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let answerer_report = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("answerer report is bounded")
        .expect("reads answerer report")
        .expect("answerer report exists");

    for (report, role) in [
        (&*offerer, "browser-offerer"),
        (&answerer_report, "browser-answerer"),
    ] {
        let value: serde_json::Value = serde_json::from_str(report).expect("terminal JSON parses");
        assert_eq!(value["status"], "answered", "{report}");
        assert_eq!(value["media_profile"], "browser-audio", "{report}");
        assert_eq!(value["negotiated_codec"], "opus", "{report}");
        assert_eq!(value["negotiated_keying"], "dtls-srtp", "{report}");
        assert_eq!(value["browser_role"], role, "{report}");
        assert_eq!(value["ice_component"], 1, "{report}");
        assert!(value["nominated_local"].as_str().is_some(), "{report}");
        assert!(value["nominated_remote"].as_str().is_some(), "{report}");
        assert_eq!(value["ice_generation"], 0, "{report}");
        assert_eq!(value["media_state"], "running", "{report}");
        assert!(
            value["negotiated_payload_type"].as_u64().is_some(),
            "{report}"
        );
        assert_eq!(value["negotiated_clock_rate"], 48_000, "{report}");
        assert_eq!(value["local_candidate_type"], "host", "{report}");
        assert_eq!(value["remote_candidate_type"], "host", "{report}");
        assert!(value["ingress_drops_total"].as_u64().is_some(), "{report}");
    }
    answerer_exits_cleanly(&mut answerer).await;
}

/// A named profile selected on clear signalling is rejected before any datagram leaves.
#[tokio::test]
async fn browser_audio_profile_refuses_non_wss_before_network_io() {
    let observer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("observer binds");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:bob@{address}"),
            "--profile",
            "browser-audio",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("requires --transport wss"));
    let mut datagram = [0_u8; 1];
    assert!(observer.try_recv_from(&mut datagram).is_err());
}

/// Browser audio starts media only after a final answer, ICE nomination and verified DTLS. The
/// diagnostic command refuses its reliable-provisional mode while it is still pure configuration.
#[cfg(all(feature = "dtls", feature = "opus"))]
#[tokio::test]
async fn browser_audio_profile_refuses_early_media_before_network_io() {
    let observer = std::net::TcpListener::bind("127.0.0.1:0").expect("observer binds");
    observer
        .set_nonblocking(true)
        .expect("observer is nonblocking");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:bob@{address}"),
            "--transport",
            "wss",
            "--profile",
            "browser-audio",
            "--early-media",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("does not support --early-media; wait for the final answer")
    );
    assert!(observer.accept().is_err());
}

/// Without the optional handshake implementation, `DPH-5` takes its other permitted result: a
/// typed refusal before signalling and no downgrade to SDES or plain RTP.
#[cfg(not(feature = "dtls"))]
#[tokio::test]
async fn dph_5_dtls_srtp_without_the_feature_is_a_typed_pre_io_failure() {
    let observer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("observer binds");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:bob@{address}"),
            "--media-security",
            "dtls-srtp",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{complaint}");
    assert!(complaint.contains("`dtls` feature"), "{complaint}");
    let mut datagram = [0u8; 1];
    assert!(observer.try_recv_from(&mut datagram).is_err());
}

/// `DPH-6`: both default/high-priority host destinations are silent. The only usable addresses
/// are the candidates learned from the selected STUN server, so the clip and terminal reports
/// prove a nominated server-reflexive pair replaced the defaults.
#[tokio::test(flavor = "multi_thread")]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end vector keeps the mapped-path fixture and both process reports together"
)]
async fn dph_6_stun_ice_reports_and_carries_audio_on_a_server_reflexive_pair() {
    let _scenario = process_scenario().await;
    let dir = scratch("dph-6");
    let played = dir.join("played.wav");
    let recorded = dir.join("recorded.wav");
    write_wav(
        std::fs::File::create(&played).expect("creates input"),
        &tone(4_000),
    )
    .expect("writes input");

    let (stun, stop_stun, serving_stun) = start_stun_server().await;
    let stun = stun.to_string();
    let (mut answerer, answer_address, mut lines) = start_answerer(&[
        "--codec",
        "pcma",
        "--ice",
        "stun",
        "--stun-server",
        &stun,
        "--duration",
        "6",
        "--record",
        recorded.to_str().expect("recording path"),
    ])
    .await;

    let proxy = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("proxy binds");
    let proxy_address = proxy.local_addr().expect("proxy address");
    let answer_address: std::net::SocketAddr = answer_address.parse().expect("answer address");
    let (_caller_dead_socket, caller_dead) = dead_media_path().await;
    let (_answer_dead_socket, answer_dead) = dead_media_path().await;
    let forwarding = tokio::spawn(async move {
        let mut datagram = vec![0u8; 65_535];
        let (length, caller_address) = proxy.recv_from(&mut datagram).await.expect("INVITE");
        let offer = silence_host_path(&datagram[..length], caller_dead);
        proxy
            .send_to(&offer, answer_address)
            .await
            .expect("forwards INVITE");

        let (length, _) = proxy.recv_from(&mut datagram).await.expect("final answer");
        let answer = silence_host_path(&datagram[..length], answer_dead);
        proxy
            .send_to(&answer, caller_address)
            .await
            .expect("forwards answer");
    });

    let output = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:answer@{proxy_address}"),
                "--codec",
                "pcma",
                "--ice",
                "stun",
                "--stun-server",
                &stun,
                "--duration",
                "6",
                "--timeout",
                "10",
                "--play",
                played.to_str().expect("input path"),
                "--json",
            ])
            .output(),
    )
    .await
    .expect("ICE call is bounded")
    .expect("dial runs");
    forwarding.await.expect("proxy task");
    let dial_report = String::from_utf8_lossy(&output.stdout);
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{dial_report} / {complaint}");
    assert!(
        dial_report.contains("\"requested_ice\":\"stun\"")
            && dial_report.contains("\"negotiated_ice\":\"server-reflexive\"")
            && dial_report.contains("\"requested_codecs\":\"pcma\"")
            && dial_report.contains("\"negotiated_codec\":\"pcma\""),
        "{dial_report}"
    );

    let answer_report = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("answer report is bounded")
        .expect("reads answer report")
        .expect("answer report exists");
    assert!(
        answer_report.contains("\"requested_ice\":\"stun\"")
            && answer_report.contains("\"negotiated_ice\":\"server-reflexive\"")
            && answer_report.contains("\"negotiated_codec\":\"pcma\"")
            && answer_report.contains("\"heard_audio\":true"),
        "{answer_report}"
    );
    answerer_exits_cleanly(&mut answerer).await;
    let heard = read_wav(std::fs::File::open(&recorded).expect("opens recording"))
        .expect("reads recording");
    assert!(
        !heard.samples.is_empty(),
        "the nominated pair carried audio"
    );

    let _ = stop_stun.send(());
    serving_stun.await.expect("STUN server stops");
}

/// `DPH-7`: an explicit stable identifier either opens that exact device or fails before the
/// signalling observer sees a byte. The observer is read only after the process exits, so this is
/// a causal assertion and needs no wall-clock sleep.
#[cfg(feature = "device-audio")]
#[tokio::test]
async fn dph_7_a_missing_requested_device_fails_before_network_io() {
    let observer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("observer binds");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:missing@{address}"),
            "--audio-input",
            "device:alsa:missing",
            "--duration",
            "0",
            "--timeout",
            "1",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");

    assert_eq!(output.status.code(), Some(1), "typed setup failure");
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("audio input"), "{complaint}");
    assert!(complaint.contains("alsa:missing"), "{complaint}");
    assert!(complaint.contains("not available"), "{complaint}");
    let mut datagram = [0u8; 1];
    assert!(
        observer.try_recv_from(&mut datagram).is_err(),
        "device validation happened after signalling I/O"
    );
}

/// The feature-off half of the same boundary: accepting a device selector and then silently using
/// the null endpoint would make the small binary look successful while carrying no microphone.
#[cfg(not(feature = "device-audio"))]
#[tokio::test]
async fn a_device_endpoint_without_the_feature_fails_before_network_io() {
    let observer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("observer binds");
    let address = observer.local_addr().expect("observer address");
    let output = sipx()
        .args([
            "dial",
            &format!("sip:feature@{address}"),
            "--audio-input",
            "device:alsa:anything",
            "--duration",
            "0",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    assert_eq!(output.status.code(), Some(1));
    let complaint = String::from_utf8_lossy(&output.stderr);
    assert!(complaint.contains("device-audio"), "{complaint}");
    let mut datagram = [0u8; 1];
    assert!(observer.try_recv_from(&mut datagram).is_err());
}

#[cfg(not(feature = "device-audio"))]
#[test]
fn listing_devices_without_the_feature_is_a_typed_failure() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_sipx"))
        .args(["devices", "--json"])
        .output()
        .expect("devices command runs");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("device-audio"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `DPH-12`: the Linux file-backed virtual microphone contains the same deterministic clip as the
/// WAV run, so two independent calls hold the callback path against the existing file path. The
/// 48 kHz conversion arithmetic is pinned separately at the converter boundary, where it can be
/// exact rather than dependent on which formats the machine's virtual PCM advertises.
#[cfg(all(feature = "device-audio", target_os = "linux"))]
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the two complete process calls stay together so their fixture and comparison cannot drift"
)]
async fn dph_12_wav_and_virtual_device_carry_the_same_clip() {
    let _scenario = process_scenario().await;
    let dir = scratch("dph-12");
    let source = tone(500);
    let wav_path = dir.join("source.wav");
    write_wav(
        std::fs::File::create(&wav_path).expect("creates WAV input"),
        &source,
    )
    .expect("writes WAV input");

    let raw_path = dir.join("virtual-mic.raw");
    let mut raw = Vec::with_capacity(source.samples.len() * 2);
    for sample in &source.samples {
        raw.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&raw_path, raw).expect("writes virtual microphone PCM");

    let sink_path = dir.join("virtual-speaker.raw");
    let alsa_path = dir.join("alsa.conf");
    let alsa = format!(
        "</usr/share/alsa/alsa.conf>\n\
         pcm.sipx_dph12 {{\n\
           type file\n\
           hint {{\n\
             show on\n\
             description \"sipx DPH-12 virtual microphone\"\n\
           }}\n\
           slave.pcm \"null\"\n\
           file \"{}\"\n\
           infile \"{}\"\n\
           format raw\n\
         }}\n",
        sink_path.display(),
        raw_path.display(),
    );
    std::fs::write(&alsa_path, alsa).expect("writes the virtual-device configuration");

    let mut listing = sipx();
    listing
        .env("ALSA_CONFIG_PATH", &alsa_path)
        .args(["devices", "--json"]);
    let listing = tokio::time::timeout(Duration::from_secs(10), listing.output())
        .await
        .expect("device enumeration is bounded")
        .expect("device enumeration runs");
    assert!(
        listing.status.success(),
        "{} / {}",
        String::from_utf8_lossy(&listing.stdout),
        String::from_utf8_lossy(&listing.stderr)
    );
    let listing: serde_json::Value =
        serde_json::from_slice(&listing.stdout).expect("device inventory is JSON");
    let listed = listing["devices"]
        .as_array()
        .expect("device inventory contains an array")
        .iter()
        .find(|device| device["id"] == "alsa:sipx_dph12")
        .expect("the stable virtual-device identifier is listed");
    assert_eq!(listed["input"], true);
    assert_eq!(listed["output"], true);

    let wav_recording = dir.join("wav-heard.wav");
    let (mut wav_answerer, wav_address, mut wav_lines) = start_answerer(&[
        "--duration",
        "2",
        "--record",
        wav_recording.to_str().expect("WAV recording path"),
    ])
    .await;
    let wav_output = tokio::time::timeout(
        Duration::from_secs(15),
        sipx()
            .args([
                "dial",
                &format!("sip:wav@{wav_address}"),
                "--audio-input",
                &format!("wav:{}", wav_path.display()),
                "--duration",
                "1",
                "--timeout",
                "5",
                "--json",
            ])
            .output(),
    )
    .await
    .expect("WAV call is bounded")
    .expect("WAV dial runs");
    assert!(
        wav_output.status.success(),
        "{} / {}",
        String::from_utf8_lossy(&wav_output.stdout),
        String::from_utf8_lossy(&wav_output.stderr)
    );
    let _ = tokio::time::timeout(Duration::from_secs(10), wav_lines.next_line())
        .await
        .expect("WAV answer report is bounded")
        .expect("reads WAV answer report")
        .expect("WAV answer report exists");
    answerer_exits_cleanly(&mut wav_answerer).await;

    let device_recording = dir.join("device-heard.wav");
    let (mut device_answerer, device_address, mut device_lines) = start_answerer(&[
        "--duration",
        "2",
        "--record",
        device_recording.to_str().expect("device recording path"),
    ])
    .await;
    let mut command = sipx();
    command.env("ALSA_CONFIG_PATH", &alsa_path).args([
        "dial",
        &format!("sip:device@{device_address}"),
        "--audio-input",
        "device:alsa:sipx_dph12",
        "--duration",
        "1",
        "--timeout",
        "5",
        "--json",
    ]);
    let device_output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("device call is bounded")
        .expect("device dial runs");
    let device_report = String::from_utf8_lossy(&device_output.stdout);
    assert!(
        device_output.status.success(),
        "{device_report} / {}",
        String::from_utf8_lossy(&device_output.stderr)
    );
    assert!(
        device_report.contains("\"audio_input_device\":\"alsa:sipx_dph12\""),
        "{device_report}"
    );
    for counter in [
        "device_input_dropped_samples",
        "device_output_dropped_samples",
        "device_output_silence_samples",
    ] {
        assert!(
            device_report.contains(counter),
            "{counter}: {device_report}"
        );
    }
    let _ = tokio::time::timeout(Duration::from_secs(10), device_lines.next_line())
        .await
        .expect("device answer report is bounded")
        .expect("reads device answer report")
        .expect("device answer report exists");
    answerer_exits_cleanly(&mut device_answerer).await;

    let wav_heard = read_wav(std::fs::File::open(&wav_recording).expect("opens WAV result"))
        .expect("reads WAV result");
    let device_heard =
        read_wav(std::fs::File::open(&device_recording).expect("opens virtual-device result"))
            .expect("reads virtual-device result");
    let compared = wav_heard
        .samples
        .len()
        .min(device_heard.samples.len())
        .min(source.samples.len());
    assert!(compared >= 3_200, "both paths carry most of the clip");
    let mean_difference = wav_heard
        .samples
        .iter()
        .zip(&device_heard.samples)
        .take(compared)
        .map(|(wav, device)| i64::from((i32::from(*wav) - i32::from(*device)).abs()))
        .sum::<i64>()
        / i64::try_from(compared).expect("positive comparison length");
    assert!(
        mean_difference < 600,
        "device conversion diverged from WAV by {mean_difference} mean sample units"
    );

    // The input run above proves callback conversion with real samples. A zero-duration second
    // call proves that the same exact identifier opens as an output and is causally stopped,
    // without letting the file-backed null sink spin merely to simulate elapsed playback.
    let (mut output_answerer, output_address, mut output_lines) =
        start_answerer(&["--duration", "1"]).await;
    let mut command = sipx();
    command.env("ALSA_CONFIG_PATH", &alsa_path).args([
        "dial",
        &format!("sip:output@{output_address}"),
        "--audio-output",
        "device:alsa:sipx_dph12",
        "--duration",
        "0",
        "--timeout",
        "5",
        "--json",
    ]);
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("output-device call is bounded")
        .expect("output-device dial runs");
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{report} / {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        report.contains("\"audio_output_device\":\"alsa:sipx_dph12\""),
        "{report}"
    );
    let _ = tokio::time::timeout(Duration::from_secs(10), output_lines.next_line())
        .await
        .expect("output answer report is bounded")
        .expect("reads output answer report")
        .expect("output answer report exists");
    answerer_exits_cleanly(&mut output_answerer).await;

    let _ = std::fs::remove_dir_all(&dir);
}

/// Registration uses the same selection and certificate policy as calls. This is deliberately a
/// real endpoint rather than a mock byte sink: TCP framing, TLS and both WebSocket handshakes must
/// complete before the REGISTER reaches the registrar.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one matrix test keeps identical assertions visible for every released transport"
)]
async fn register_selects_every_released_transport() {
    let _scenario = process_scenario().await;
    let tls = tls_fixture("register-transports");
    let ca = tls.ca.to_string_lossy().into_owned();
    let cert = std::fs::read(&tls.cert).expect("reads certificate");
    let key = std::fs::read(&tls.key).expect("reads key");

    for (kind, name) in [
        (sipx_transport::TransportKind::Udp, "udp"),
        (sipx_transport::TransportKind::Tcp, "tcp"),
        (sipx_transport::TransportKind::Tls, "tls"),
        (sipx_transport::TransportKind::Ws, "ws"),
        (sipx_transport::TransportKind::Wss, "wss"),
    ] {
        let local: std::net::SocketAddr = "127.0.0.1:0".parse().expect("an address");
        let mut config = sipx_transport::Config::new(local);
        config.tcp = kind == sipx_transport::TransportKind::Tcp;
        if kind == sipx_transport::TransportKind::Tls {
            let identity = sipx_transport::tls::Identity::from_pem(&cert, &key).expect("identity");
            config.tls_server = Some((
                sipx_transport::tls::ServerTls::new(identity).expect("TLS server"),
                0,
            ));
        }
        if kind == sipx_transport::TransportKind::Ws {
            config.ws_server = Some(0);
        }
        if kind == sipx_transport::TransportKind::Wss {
            let identity = sipx_transport::tls::Identity::from_pem(&cert, &key).expect("identity");
            config.wss_server = Some((
                sipx_transport::tls::ServerTls::new(identity).expect("WSS server"),
                0,
            ));
        }
        let (handle, mut incoming) = sipx_transport::bind(config).await.expect("registrar binds");
        let address = match kind {
            sipx_transport::TransportKind::Udp | sipx_transport::TransportKind::Tcp => {
                handle.local_addr()
            }
            sipx_transport::TransportKind::Tls => handle.tls_addr().expect("TLS address"),
            sipx_transport::TransportKind::Ws => handle.ws_addr().expect("WS address"),
            sipx_transport::TransportKind::Wss => handle.wss_addr().expect("WSS address"),
            sipx_transport::TransportKind::Quic => {
                panic!("QUIC is not part of this five-transport command-line matrix")
            }
        };
        let registrar = handle.clone();
        let serving = tokio::spawn(async move {
            let request = tokio::time::timeout(Duration::from_secs(10), incoming.recv())
                .await
                .expect("REGISTER is bounded")
                .expect("REGISTER arrives");
            assert_eq!(request.transport, kind);
            let contact = request
                .request
                .headers
                .value(&sipx_sip::HeaderName::Contact)
                .expect("REGISTER carries Contact");
            let response = sipx_sip::build::ResponseBuilder::to_request(
                &request.request,
                sipx_sip::StatusCode::new(200).expect("status"),
                "OK",
            )
            .expect("response")
            .header(
                sipx_sip::HeaderName::Contact,
                bytes::Bytes::from(format!("{};expires=60", String::from_utf8_lossy(&contact))),
            )
            .expect("Contact")
            .build();
            registrar
                .respond(&request.key, response)
                .await
                .expect("REGISTER answered");
        });

        let target = address.to_string();
        let mut command = sipx();
        command.args([
            "register",
            "sip:alice@example.com",
            "--target",
            &target,
            "--transport",
            name,
            "--expires",
            "60",
            "--json",
        ]);
        if matches!(
            kind,
            sipx_transport::TransportKind::Tls | sipx_transport::TransportKind::Wss
        ) {
            command.args(["--tls-ca", &ca, "--tls-server-name", "sipx.test"]);
        }
        let output = tokio::time::timeout(Duration::from_secs(15), command.output())
            .await
            .unwrap_or_else(|_| panic!("{name} registration is bounded"))
            .expect("register runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{name} register failed: {stdout} / {stderr}"
        );
        assert!(
            stdout.contains(&format!("\"requested_transport\":\"{name}\"")),
            "{name}: {stdout}"
        );
        assert!(
            stdout.contains(&format!("\"negotiated_transport\":\"{name}\"")),
            "{name}: {stdout}"
        );
        serving.await.expect("registrar task");
        handle.shutdown().await;
    }
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
    let _scenario = process_scenario().await;
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

/// Answer a REGISTER the way a registrar that implements RFC 5626 and RFC 8599 does: the
/// option tag in `Require` says an outbound registration was performed (§6), the `Feature-Caps`
/// name the push service the client asked for and assign the binding a PURR (§8.2).
async fn answer_register(
    registrar: &tokio::net::UdpSocket,
    request: &str,
    source: std::net::SocketAddr,
) {
    let field = |name: &str| header_line(request, name);
    let response = format!(
        "SIP/2.0 200 OK\r\n{}\r\n{}\r\n{}\r\n{}\r\n{}\r\nRequire: outbound\r\nFlow-Timer: 30\r\n\
         {};expires=60\r\nFeature-Caps: *;+sip.pns=\"webpush\";+sip.pnspurr=\"opaque-purr-1\"\r\n\
         Content-Length: 0\r\n\r\n",
        field("Via"),
        field("To"),
        field("From"),
        field("Call-ID"),
        field("CSeq"),
        field("Contact"),
    );
    registrar
        .send_to(response.as_bytes(), source)
        .await
        .expect("answers");
}

/// Wait for a REGISTER, returning it with the address it came from.
///
/// The registration and the binding refresh a push triggers are the same wait, so it is written
/// once. The timeout is what turns a REGISTER that never arrives into a named failure instead of
/// a test that hangs until the harness kills it.
async fn next_register(
    registrar: &tokio::net::UdpSocket,
    expected: &str,
) -> (String, std::net::SocketAddr) {
    let mut buf = vec![0u8; 65_535];
    let (length, source) =
        tokio::time::timeout(Duration::from_secs(10), registrar.recv_from(&mut buf))
            .await
            .unwrap_or_else(|_| panic!("{expected}"))
            .expect("reads");
    let request = String::from_utf8_lossy(&buf[..length]).into_owned();
    assert!(request.starts_with("REGISTER"), "{expected}: {request}");
    (request, source)
}

/// What `--outbound` with the push flags must put on the wire: RFC 5626's flow identity and RFC
/// 8599's push parameters, on the one `Contact` a registrar will store.
///
/// This is the assertion that fails when nothing above `sipx-ua` builds the config — a plain
/// REGISTER carries none of it.
fn assert_outbound_push_register(request: &str) {
    let contact = header_line(request, "Contact");
    assert!(
        contact.contains(";reg-id=1"),
        "RFC 5626 §4.2's flow number is missing — nothing built the Outbound config: {contact}"
    );
    assert!(
        contact.contains("+sip.instance=\"<urn:uuid:"),
        "RFC 5626 §4.1's device identity: {contact}"
    );
    for param in ["pn-provider=webpush", "pn-prid=c1a5b3e7d9f2"] {
        assert!(
            contact.contains(param),
            "RFC 8599 §4.1.2's {param} is missing: {contact}"
        );
    }
    // §8.7 registers the pn-* parameters as *URI* parameters: inside the angle brackets, where a
    // registrar's URI parser looks. Outside them a `;` starts a header parameter.
    assert!(
        contact.find("pn-provider=") < contact.rfind('>'),
        "the push parameters belong inside the Contact's angle brackets: {contact}"
    );
    let supported = header_line(request, "Supported");
    assert!(
        supported.contains("outbound"),
        "§4.2 makes offering the option tag a MUST: {supported}"
    );
}

/// The acceptance test for S-29: a registration placed over an Outbound flow, and woken.
///
/// `--outbound` must put RFC 5626's `reg-id` and `+sip.instance` on the `Contact` and the
/// `outbound` option tag in `Supported`; `--push-provider`/`--push-prid` must put RFC 8599's
/// `pn-*` parameters inside the `Contact` URI's angle brackets; and `--wake` must send §4.1.3's
/// binding-refresh REGISTER — the thing `UserAgent::woken` exists to do. Until S-29 no caller
/// above `sipx-ua`'s own tests built this config, which is why `X-37` demoted both RFCs to no
/// roles: this test fails on a plain REGISTER, which is all the CLI could send before.
#[tokio::test]
async fn register_over_a_flow_keeps_it_and_a_push_wakes_it() {
    let _scenario = process_scenario().await;
    let registrar = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let address = registrar.local_addr().expect("has an address");

    let child = sipx()
        .args([
            "register",
            "sip:alice@example.com",
            "--target",
            &address.to_string(),
            "--outbound",
            "--push-provider",
            "webpush",
            "--push-prid",
            "c1a5b3e7d9f2",
            "--wake",
            "--json",
            "--expires",
            "60",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawns");

    // The registration itself: one flow, and the push parameters in the Contact URI.
    let (first, source) = next_register(&registrar, "a REGISTER arrives").await;
    assert_outbound_push_register(&first);

    answer_register(&registrar, &first, source).await;

    // `--wake`: the push arrived, so §4.1.3's binding-refresh REGISTER must follow — same flow,
    // same push parameters, a later CSeq.
    let (second, source) = next_register(
        &registrar,
        "§4.1.3's answer to a push is a binding-refresh REGISTER",
    )
    .await;
    let refreshed = header_line(&second, "Contact");
    assert!(
        refreshed.contains(";reg-id=1"),
        "the refresh replaces the flow's binding rather than adding a second one: {refreshed}"
    );
    assert!(
        refreshed.contains("pn-prid=c1a5b3e7d9f2"),
        "the refresh keeps the push parameters: {refreshed}"
    );
    let cseq = header_line(&second, "CSeq");
    assert!(
        cseq.contains("2 REGISTER"),
        "a refresh advances the sequence inside the same Call-ID: {cseq}"
    );

    answer_register(&registrar, &second, source).await;

    // What a script reading stdout learns: the flow was accepted (§6), the registrar named our
    // push service (§8.2), and the wake reported the PURR the binding was assigned.
    let output = child.wait_with_output().await.expect("reports");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "register failed: {stdout} / {stderr}"
    );
    let mut lines = stdout.lines();
    let registered = lines.next().expect("the registration report");
    assert!(
        registered.contains("\"status\":\"registered\""),
        "{registered}"
    );
    assert!(
        registered.contains("\"flow\":true"),
        "§6: the registrar said it performed an outbound registration: {registered}"
    );
    assert!(
        registered.contains("\"push\":true"),
        "§8.2: the registrar named the push service this client registered: {registered}"
    );
    let woken = lines.next().expect("the wake report");
    assert!(woken.contains("\"status\":\"woken\""), "{woken}");
    assert!(
        woken.contains("\"purr\":\"opaque-purr-1\""),
        "the PURR the registrar assigned travels with the wake: {woken}"
    );
}

#[tokio::test]
async fn version_and_help_succeed() {
    let _scenario = process_scenario().await;
    let output = sipx().arg("version").output().await.expect("runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("sipx"));

    let output = sipx().arg("help").output().await.expect("runs");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("USAGE"));

    let output = sipx()
        .args(["devices", "--help"])
        .output()
        .await
        .expect("runs");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("stable audio device identifiers"), "{help}");
    assert!(help.contains("opens no stream"), "{help}");
}

/// An unknown command is a usage error with its own exit code, and the complaint goes to
/// stderr where it will not be parsed as a result.
#[tokio::test]
async fn an_unknown_command_is_a_usage_error_on_stderr() {
    let _scenario = process_scenario().await;
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
    let _scenario = process_scenario().await;
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
    let _scenario = process_scenario().await;
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
    let _scenario = process_scenario().await;
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

    // The receiver must outlive the caller's complete causal bound: six seconds of exchange plus
    // Call::hang_up's five-second media-queue flush. Twelve seconds leaves that relation true
    // under load; it is a bound on failure, while the BYE remains the ordinary completion event.
    let (mut answerer, address, mut lines) = start_answerer(&[
        "--duration",
        "12",
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

    // The answerer's own account of the audio, read before the file, because it is what tells the
    // two failures apart (`X-40`). `heard_audio` is false when the media path delivered nothing to
    // record; a false here with a non-empty file, or a true here with an empty one, is a defect in
    // the writing rather than in the carrying. "The callee recorded nothing" said neither.
    let heard_audio = answered.contains("\"heard_audio\":true");
    answerer_exits_cleanly(&mut answerer).await;

    // The recording contains the tone, not silence of the right length.
    let heard =
        read_wav(std::fs::File::open(&heard_by_callee_path).expect("opens")).expect("reads");
    assert!(
        heard_audio,
        "the answerer reports it heard no audio at all during the call, so the recording has \
         nothing in it to assert on: {answered}"
    );
    assert!(
        !heard.samples.is_empty(),
        "the answerer reported audio and then wrote an empty recording: {answered}"
    );
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

/// Silence is a property of the media received, not proof that signalling failed.
///
/// Neither endpoint plays audio. Both still establish and complete the call, report the zero
/// samples honestly, and use the success exit code. This is deliberately a binary test: the
/// contract belongs to the status a shell sees, not to an internal return value (`S-33`).
#[tokio::test]
async fn a_completed_silent_call_is_success_for_dial_and_answer() {
    let _scenario = process_scenario().await;
    let dir = scratch("silent-call");
    let heard_by_answer = dir.join("answer.wav");
    let heard_by_dial = dir.join("dial.wav");
    let (mut answerer, address, mut lines) = start_answerer(&[
        "--duration",
        "1",
        "--advertise",
        "127.0.0.1",
        "--record",
        heard_by_answer.to_str().expect("an answer recording path"),
    ])
    .await;

    let caller = tokio::time::timeout(
        Duration::from_secs(20),
        sipx()
            .args([
                "dial",
                &format!("sip:silence@{address}"),
                "--local",
                "127.0.0.1:0",
                "--advertise",
                "127.0.0.1",
                "--json",
                "--duration",
                "1",
                "--timeout",
                "10",
                "--record",
                heard_by_dial.to_str().expect("a dial recording path"),
            ])
            .output(),
    )
    .await
    .expect("the silent call is bounded")
    .expect("dial runs");
    let caller_report = String::from_utf8_lossy(&caller.stdout);
    assert_eq!(
        caller.status.code(),
        Some(0),
        "dial completed a call but did not exit successfully: {caller_report} / {}",
        String::from_utf8_lossy(&caller.stderr)
    );
    let caller_json: serde_json::Value =
        serde_json::from_str(caller_report.trim()).expect("dial emits one JSON report");
    assert_eq!(caller_json["status"], "answered", "{caller_report}");
    assert_eq!(caller_json["samples_recorded"], 0, "{caller_report}");
    assert_eq!(caller_json["heard_audio"], false, "{caller_report}");

    let answer_report = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("the answer report is bounded")
        .expect("reads answer stdout")
        .expect("the answerer emits its terminal report");
    let answer_json: serde_json::Value =
        serde_json::from_str(&answer_report).expect("answer emits one JSON report");
    assert_eq!(answer_json["status"], "answered", "{answer_report}");
    assert_eq!(answer_json["samples_recorded"], 0, "{answer_report}");
    assert_eq!(answer_json["heard_audio"], false, "{answer_report}");
    for (side, report) in [("dial", &caller_json), ("answer", &answer_json)] {
        assert_eq!(
            report["media_advertised"], "127.0.0.1",
            "{side} must report the selected advertised media address: {report}"
        );
        let bound: std::net::SocketAddr = report["media_bound"]
            .as_str()
            .unwrap_or_else(|| panic!("{side} media_bound must be a string: {report}"))
            .parse()
            .unwrap_or_else(|error| panic!("{side} media_bound must be a socket: {error}"));
        assert_eq!(bound.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
        assert_ne!(bound.port(), 0, "{side} must report the allocated RTP port");
    }
    let complaint = drain_stderr(&mut answerer).await;
    let answer_status = exits_cleanly(&mut answerer, &complaint).await;
    assert_eq!(
        answer_status.code(),
        Some(0),
        "answer completed a silent call but chose another outcome: {complaint}"
    );

    for recording in [&heard_by_answer, &heard_by_dial] {
        let heard = read_wav(std::fs::File::open(recording).expect("opens silent recording"))
            .expect("reads silent recording");
        assert_eq!(
            heard.sample_rate, 8_000,
            "the default G.711 recording keeps its 8 kHz clock: {recording:?}"
        );
        assert!(
            heard.samples.is_empty(),
            "the test must not pass by accidentally carrying audio: {recording:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Keeping exit 0 for silence is a decision, and the decision must be discoverable to a script
/// author rather than inferred from a test or one run of the binary.
#[test]
fn the_silent_call_exit_contract_is_documented_for_both_commands() {
    let reference = include_str!("../../../website/docs/reference/cli.md");
    assert!(
        reference.contains(
            "Both `dial` and `answer` exit 0 after a completed call that received no audio"
        ),
        "the CLI reference does not state the shared dial/answer exit rule for a silent call"
    );
    assert!(
        reference.contains("`heard_audio: false`") && reference.contains("Silence is not"),
        "the CLI reference must say where silence is reported and why it is not an exit failure"
    );
}

/// A refused call gets its own exit code, so a script can tell busy from no-answer without
/// matching on English.
#[tokio::test]
async fn a_busy_answer_gives_the_caller_the_busy_exit_code() {
    let _scenario = process_scenario().await;
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

    answerer_exits_cleanly(&mut answerer).await;
}

/// `S-28`: the shell-facing credential option reaches the call retry, rather than merely parsing.
#[tokio::test]
async fn dial_password_answers_a_proxy_challenge_and_connects() {
    let _scenario = process_scenario().await;
    authenticated_dial(false).await;
}

/// The environment is the documented credential route because argv is visible to other users.
#[tokio::test]
async fn sipx_password_answers_a_proxy_challenge_and_connects() {
    let _scenario = process_scenario().await;
    authenticated_dial(true).await;
}

async fn authenticated_dial(from_environment: bool) {
    const PASSWORD: &str = "Circle Of Life";
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("a local address"),
    ))
    .await
    .expect("binds");
    let address = handle.local_addr();
    let serving = tokio::spawn(async move {
        let first = incoming.recv().await.expect("the first INVITE arrives");
        assert_eq!(first.request.method, Method::Invite);
        let mut authenticator = Authenticator::new("proxy.example", [9; 32]);
        let challenge = sipx_sip::build::ResponseBuilder::to_request(
            &first.request,
            StatusCode::new(407).expect("valid"),
            "Proxy Authentication Required",
        )
        .expect("builds")
        .set_header(
            &HeaderName::To,
            bytes::Bytes::from_static(b"<sip:bob@sipx.test>;tag=challenge"),
        )
        .expect("valid")
        .header(
            HeaderName::ProxyAuthenticate,
            bytes::Bytes::from(authenticator.challenge(false)),
        )
        .expect("valid")
        .build();
        handle
            .respond(&first.key, challenge)
            .await
            .expect("challenges");

        let retry = incoming
            .recv()
            .await
            .expect("the authenticated retry arrives");
        let presented = Presented::from_request(&retry.request, true)
            .expect("the retry carries Proxy-Authorization");
        assert_eq!(presented.username, "alice");
        assert_eq!(
            authenticator.verify(&presented, "INVITE", PASSWORD),
            Verdict::Authenticated
        );
        sipx_call::answer(&handle, &retry, "127.0.0.1".parse().expect("loopback"))
            .await
            .expect("answers")
    });

    let mut command = sipx();
    command.args([
        "dial",
        &format!("sip:bob@{address}"),
        "--from",
        "sip:alice@example.net",
        "--duration",
        "0",
        "--timeout",
        "5",
        "--json",
    ]);
    if from_environment {
        command.env("SIPX_PASSWORD", PASSWORD);
    } else {
        command.args(["--password", PASSWORD]);
    }
    let output = tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("the authenticated dial is bounded")
        .expect("dial runs");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{} / {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _callee = serving.await.expect("the challenge server finishes");
}

/// A challenge with no credential is a named authentication outcome, not a transaction timeout.
#[tokio::test]
async fn a_challenged_dial_without_a_password_exits_unauthorized() {
    let _scenario = process_scenario().await;
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("a local address"),
    ))
    .await
    .expect("binds");
    let address = handle.local_addr();
    let serving = tokio::spawn(async move {
        let invite = incoming.recv().await.expect("the INVITE arrives");
        let authenticator = Authenticator::new("proxy.example", [11; 32]);
        let challenge = sipx_sip::build::ResponseBuilder::to_request(
            &invite.request,
            StatusCode::new(407).expect("valid"),
            "Proxy Authentication Required",
        )
        .expect("builds")
        .header(
            HeaderName::ProxyAuthenticate,
            bytes::Bytes::from(authenticator.challenge(false)),
        )
        .expect("valid")
        .build();
        handle
            .respond(&invite.key, challenge)
            .await
            .expect("challenges");
    });

    let output = tokio::time::timeout(
        Duration::from_secs(10),
        sipx()
            .args([
                "dial",
                &format!("sip:bob@{address}"),
                "--timeout",
                "5",
                "--json",
            ])
            .output(),
    )
    .await
    .expect("the rejection is bounded")
    .expect("dial runs");
    serving.await.expect("the challenge server finishes");
    assert_eq!(
        output.status.code(),
        Some(4),
        "a missing credential must be Unauthorized, not timeout: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// DPH-10 through the shipped process: the call bound is exact and the stable summary keeps
/// rejection causes and response codes separate. The peer counts INVITEs itself, so a command that
/// merely printed the requested count without placing that many calls cannot pass.
#[tokio::test]
async fn bounded_load_stops_at_the_call_limit_and_emits_one_stable_summary() {
    let _scenario = process_scenario().await;
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("a local address"),
    ))
    .await
    .expect("binds");
    let address = handle.local_addr();
    let serving = tokio::spawn(async move {
        let mut invitations = 0usize;
        while invitations < 3 {
            let request = incoming.recv().await.expect("the load request arrives");
            if request.request.method != Method::Invite {
                continue;
            }
            invitations += 1;
            let refusal = sipx_sip::build::ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(486).expect("valid"),
                "Busy Here",
            )
            .expect("builds")
            .set_header(
                &HeaderName::To,
                bytes::Bytes::from(format!("<sip:load@sipx.test>;tag=load{invitations}")),
            )
            .expect("valid")
            .build();
            handle
                .respond(&request.key, refusal)
                .await
                .expect("refuses");
        }
        invitations
    });

    let output = tokio::time::timeout(
        Duration::from_secs(15),
        sipx()
            .args([
                "load",
                &format!("sip:load@{address}"),
                "--rate",
                "100",
                "--concurrency",
                "3",
                "--calls",
                "3",
                "--seed",
                "41",
                "--timeout",
                "5",
                "--json",
            ])
            .output(),
    )
    .await
    .expect("the bounded run finishes")
    .expect("load runs");
    assert_eq!(serving.await.expect("the peer finishes"), 3);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 summary");
    assert_eq!(stdout.lines().count(), 1, "one final record: {stdout}");
    let summary: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON summary");
    assert_eq!(summary["schema"], "sipx.load.v1");
    assert_eq!(summary["seed"], 41);
    assert_eq!(summary["outcomes"]["attempted"], 3);
    let rejected = summary["outcomes"]["rejected"].as_u64().unwrap_or(0);
    let stopped = summary["outcomes"]["timed_out"].as_u64().unwrap_or(0);
    assert_eq!(rejected + stopped, 3, "every admitted call is classified");
    assert_eq!(
        summary["response_codes"]["486"].as_u64().unwrap_or(0),
        rejected,
        "only responses that arrived are counted"
    );
}

/// P-15 through the shipped process: readiness is the start barrier, then the exact SDP-free
/// INVITE/2xx/ACK/BYE/2xx flow drains every owned route, transaction and task before the terminal
/// record appears.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn bounded_load_responder_drives_readiness_through_zero_state() {
    let _scenario = process_scenario().await;
    let mut command = sipx();
    command
        .args([
            "load-responder",
            "--max-active",
            "2",
            "--calls",
            "1",
            "--cleanup",
            "5",
            "--dialog-duration",
            "5",
            "--seed",
            "41",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("responder starts");
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    let ready_line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("readiness is bounded")
        .expect("readiness can be read")
        .expect("readiness line exists");
    let ready: serde_json::Value = serde_json::from_str(&ready_line).expect("readiness JSON");
    assert_eq!(ready["schema"], "sipx.comparative-load.ready.v1");
    assert_eq!(ready["role"], "responder");
    let address: std::net::SocketAddr = ready["address"]
        .as_str()
        .expect("readiness address")
        .parse()
        .expect("IP socket address");

    let (peer, _incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("a local address"),
    ))
    .await
    .expect("peer binds");
    let call_id = "cl-0123456789abcdef0123456789abcdef-0@driver.invalid";
    let request_uri = sipx_sip::Uri::parse(bytes::Bytes::from(format!("sip:load@{address}")))
        .expect("request URI");
    let from = bytes::Bytes::from_static(b"<sip:driver@driver.invalid>;tag=f-fixed");
    let to = bytes::Bytes::from(format!("<sip:load@{address}>"));
    let contact = bytes::Bytes::from(format!("<sip:driver@{}>", peer.local_addr()));
    let invite = sipx_sip::build::RequestBuilder::new(Method::Invite, request_uri.clone())
        .header(HeaderName::To, to)
        .expect("To")
        .header(HeaderName::From, from.clone())
        .expect("From")
        .header(
            HeaderName::CallId,
            bytes::Bytes::from_static(call_id.as_bytes()),
        )
        .expect("Call-ID")
        .cseq(1, &Method::Invite)
        .expect("CSeq")
        .header(HeaderName::Contact, contact.clone())
        .expect("Contact")
        .max_forwards(70)
        .build();
    let mut invite_responses = peer
        .send(invite, sipx_transport::Target::udp(address))
        .await
        .expect("INVITE sends");
    let accepted = tokio::time::timeout(Duration::from_secs(5), invite_responses.final_response())
        .await
        .expect("answer is bounded")
        .expect("INVITE final response");
    assert_eq!(accepted.status.code(), 200);
    assert!(accepted.body().is_empty(), "signalling mode creates no SDP");
    let tagged_to = bytes::Bytes::copy_from_slice(
        &accepted
            .headers
            .value(&HeaderName::To)
            .expect("accepted To tag"),
    );
    assert!(
        String::from_utf8_lossy(&tagged_to).contains(";tag=t-"),
        "deterministic load tag: {}",
        String::from_utf8_lossy(&tagged_to)
    );

    let in_dialog = |method: Method, cseq: u32| {
        sipx_sip::build::RequestBuilder::new(method.clone(), request_uri.clone())
            .header(
                HeaderName::Via,
                bytes::Bytes::from(format!(
                    "SIP/2.0/UDP {};rport;branch={}",
                    peer.sent_by_for(sipx_transport::TransportKind::Udp),
                    sipx_transport::new_branch()
                )),
            )
            .expect("Via")
            .header(HeaderName::To, tagged_to.clone())
            .expect("To")
            .header(HeaderName::From, from.clone())
            .expect("From")
            .header(
                HeaderName::CallId,
                bytes::Bytes::from_static(call_id.as_bytes()),
            )
            .expect("Call-ID")
            .cseq(cseq, &method)
            .expect("CSeq")
            .header(HeaderName::Contact, contact.clone())
            .expect("Contact")
            .max_forwards(70)
            .build()
    };
    peer.send_directly(
        in_dialog(Method::Ack, 1),
        sipx_transport::Target::udp(address),
    )
    .await
    .expect("ACK sends");
    let mut bye_responses = peer
        .send(
            in_dialog(Method::Bye, 2),
            sipx_transport::Target::udp(address),
        )
        .await
        .expect("BYE sends");
    let ended = tokio::time::timeout(Duration::from_secs(5), bye_responses.final_response())
        .await
        .expect("teardown is bounded")
        .expect("BYE final response");
    assert_eq!(ended.status.code(), 200);

    let summary_line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("summary follows cleanup")
        .expect("summary can be read")
        .expect("summary line exists");
    let summary: serde_json::Value = serde_json::from_str(&summary_line).expect("summary JSON");
    assert_eq!(summary["schema"], "sipx.load-responder.v1");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["counts"]["invitations"], 1);
    assert_eq!(summary["counts"]["established"], 1);
    assert_eq!(summary["counts"]["completed"], 1);
    assert_eq!(summary["counts"]["active_high_water"], 1);
    assert_eq!(summary["post_drain"]["active_dialogs"], 0);
    assert_eq!(summary["post_drain"]["dispatcher_routes"], 0);
    assert_eq!(summary["post_drain"]["endpoint_transactions"], 0);
    assert_eq!(summary["post_drain"]["owned_tasks"], 0);

    let complaint = drain_stderr(&mut child).await;
    exits_cleanly(&mut child, &complaint).await;
    peer.shutdown().await;
}

fn load_invite(
    peer: &sipx_transport::Handle,
    address: std::net::SocketAddr,
    index: usize,
) -> sipx_sip::Request {
    let request_uri = sipx_sip::Uri::parse(bytes::Bytes::from(format!("sip:load@{address}")))
        .expect("request URI");
    sipx_sip::build::RequestBuilder::new(Method::Invite, request_uri)
        .header(
            HeaderName::To,
            bytes::Bytes::from(format!("<sip:load@{address}>")),
        )
        .expect("To")
        .header(
            HeaderName::From,
            bytes::Bytes::from(format!("<sip:driver@driver.invalid>;tag=f-{index}")),
        )
        .expect("From")
        .header(
            HeaderName::CallId,
            bytes::Bytes::from(format!(
                "cl-0123456789abcdef0123456789abcdef-{index}@driver.invalid"
            )),
        )
        .expect("Call-ID")
        .cseq(1, &Method::Invite)
        .expect("CSeq")
        .header(
            HeaderName::Contact,
            bytes::Bytes::from(format!("<sip:driver@{}>", peer.local_addr())),
        )
        .expect("Contact")
        .max_forwards(70)
        .build()
}

fn load_dialog_request(
    peer: &sipx_transport::Handle,
    address: std::net::SocketAddr,
    tagged_to: bytes::Bytes,
    method: &Method,
    cseq: u32,
) -> sipx_sip::Request {
    let request_uri = sipx_sip::Uri::parse(bytes::Bytes::from(format!("sip:load@{address}")))
        .expect("request URI");
    sipx_sip::build::RequestBuilder::new(method.clone(), request_uri)
        .header(
            HeaderName::Via,
            bytes::Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                peer.sent_by_for(sipx_transport::TransportKind::Udp),
                sipx_transport::new_branch()
            )),
        )
        .expect("Via")
        .header(HeaderName::To, tagged_to)
        .expect("To")
        .header(
            HeaderName::From,
            bytes::Bytes::from_static(b"<sip:driver@driver.invalid>;tag=f-0"),
        )
        .expect("From")
        .header(
            HeaderName::CallId,
            bytes::Bytes::from_static(b"cl-0123456789abcdef0123456789abcdef-0@driver.invalid"),
        )
        .expect("Call-ID")
        .cseq(cseq, method)
        .expect("CSeq")
        .header(
            HeaderName::Contact,
            bytes::Bytes::from(format!("<sip:driver@{}>", peer.local_addr())),
        )
        .expect("Contact")
        .max_forwards(70)
        .build()
}

/// P-15's active-dialog ceiling is admission, not a reporting hint: a second concurrent INVITE is
/// refused while the first remains live, and the admitted dialog can still complete and drain.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn load_responder_enforces_the_concurrent_dialog_ceiling() {
    let _scenario = process_scenario().await;
    let mut command = sipx();
    command
        .args([
            "load-responder",
            "--max-active",
            "1",
            "--calls",
            "2",
            "--cleanup",
            "5",
            "--dialog-duration",
            "5",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("responder starts");
    let stdout = child.stdout.take().expect("stdout is piped");
    let mut lines = BufReader::new(stdout).lines();
    let ready_line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("readiness is bounded")
        .expect("readiness can be read")
        .expect("readiness line exists");
    let ready: serde_json::Value = serde_json::from_str(&ready_line).expect("readiness JSON");
    let address: std::net::SocketAddr = ready["address"]
        .as_str()
        .expect("readiness address")
        .parse()
        .expect("IP socket address");
    let (peer, _incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("a local address"),
    ))
    .await
    .expect("peer binds");

    let mut first = peer
        .send(
            load_invite(&peer, address, 0),
            sipx_transport::Target::udp(address),
        )
        .await
        .expect("first INVITE sends");
    let accepted = tokio::time::timeout(Duration::from_secs(5), first.final_response())
        .await
        .expect("first answer is bounded")
        .expect("first final response");
    assert_eq!(accepted.status.code(), 200);
    let tagged_to =
        bytes::Bytes::copy_from_slice(&accepted.headers.value(&HeaderName::To).expect("tagged To"));

    let mut second = peer
        .send(
            load_invite(&peer, address, 1),
            sipx_transport::Target::udp(address),
        )
        .await
        .expect("second INVITE sends");
    let refused = tokio::time::timeout(Duration::from_secs(5), second.final_response())
        .await
        .expect("overload answer is bounded")
        .expect("overload final response");
    assert_eq!(refused.status.code(), 503);

    peer.send_directly(
        load_dialog_request(&peer, address, tagged_to.clone(), &Method::Ack, 1),
        sipx_transport::Target::udp(address),
    )
    .await
    .expect("ACK sends");
    let mut bye = peer
        .send(
            load_dialog_request(&peer, address, tagged_to, &Method::Bye, 2),
            sipx_transport::Target::udp(address),
        )
        .await
        .expect("BYE sends");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), bye.final_response())
            .await
            .expect("BYE response is bounded")
            .expect("BYE final response")
            .status
            .code(),
        200
    );

    let summary_line = tokio::time::timeout(Duration::from_secs(5), lines.next_line())
        .await
        .expect("summary follows drain")
        .expect("summary can be read")
        .expect("summary line exists");
    let summary: serde_json::Value = serde_json::from_str(&summary_line).expect("summary JSON");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["counts"]["invitations"], 2);
    assert_eq!(summary["counts"]["admitted"], 1);
    assert_eq!(summary["counts"]["rejected"], 1);
    assert_eq!(summary["counts"]["established"], 1);
    assert_eq!(summary["counts"]["completed"], 1);
    assert_eq!(summary["counts"]["active_high_water"], 1);
    assert_eq!(summary["post_drain"]["active_dialogs"], 0);
    assert_eq!(summary["post_drain"]["dispatcher_routes"], 0);
    assert_eq!(summary["post_drain"]["endpoint_transactions"], 0);
    assert_eq!(summary["post_drain"]["owned_tasks"], 0);

    let complaint = drain_stderr(&mut child).await;
    exits_cleanly(&mut child, &complaint).await;
    peer.shutdown().await;
}

/// DPH-11 through the process boundary: signal only after the peer has observed the first INVITE,
/// then require the one final summary to follow cleanup. Concurrency one is load-bearing: no second
/// invitation can be admitted while the owned first call is still cleaning up.
#[cfg(unix)]
#[tokio::test]
async fn interrupted_load_stops_admission_and_summarizes_after_cleanup() {
    let _scenario = process_scenario().await;
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("peer binds");
    let address = peer.local_addr().expect("peer address");
    let mut command = sipx();
    command
        .args([
            "load",
            &format!("sip:load@{address}"),
            "--rate",
            "100",
            "--concurrency",
            "1",
            "--calls",
            "100",
            "--timeout",
            "20",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = command.spawn().expect("load starts");
    let process = child.id().expect("load process id");

    let mut packet = [0u8; 4096];
    let (length, _) = tokio::time::timeout(Duration::from_secs(5), peer.recv_from(&mut packet))
        .await
        .expect("the first admission is bounded")
        .expect("the first INVITE arrives");
    assert!(
        packet
            .get(..length)
            .is_some_and(|bytes| bytes.starts_with(b"INVITE ")),
        "the readiness event is an INVITE"
    );

    let signal = Command::new("kill")
        .args(["-INT", &process.to_string()])
        .status()
        .await
        .expect("sends SIGINT");
    assert!(signal.success(), "SIGINT reaches the load process");
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .expect("interrupted cleanup is bounded")
        .expect("load exits");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 summary");
    assert_eq!(
        stdout.lines().count(),
        1,
        "one summary after cleanup: {stdout}"
    );
    let summary: serde_json::Value = serde_json::from_str(stdout.trim()).expect("JSON summary");
    assert_eq!(summary["status"], "interrupted");
    assert_eq!(summary["outcomes"]["attempted"], 1);
    assert_eq!(summary["outcomes"]["timed_out"], 1);
}

/// A final response to an INVITE must carry a To tag (RFC 3261 §8.2.6.2): the tag is what
/// lets a caller behind a forking proxy tell one branch's refusal from another's.
#[tokio::test]
async fn a_refusal_carries_a_to_tag() {
    let _scenario = process_scenario().await;
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

/// DPH-8: an application-owned Supported field reaches the INVITE exactly as supplied, while a
/// stack-owned Via is refused by name before the command reaches bind or dial.
#[tokio::test]
async fn custom_supported_header_is_sent_and_stack_owned_via_is_refused_before_bind() {
    let _scenario = process_scenario().await;
    let (handle, mut incoming) = bind(TransportConfig::new(
        "127.0.0.1:0".parse().expect("a local address"),
    ))
    .await
    .expect("peer binds");
    let address = handle.local_addr();
    let peer = tokio::spawn(async move {
        let invite = incoming.recv().await.expect("the INVITE arrives");
        let supported: Vec<_> = invite
            .request
            .headers
            .get_all(&HeaderName::Supported)
            .map(|header| String::from_utf8_lossy(header.raw_value()).into_owned())
            .collect();
        assert!(
            supported.iter().any(|value| value == "dph-eight"),
            "custom field missing: {supported:?}"
        );
        let response = sipx_sip::build::ResponseBuilder::to_request(
            &invite.request,
            StatusCode::new(486).expect("valid"),
            "Busy Here",
        )
        .expect("response")
        .build();
        handle
            .respond(&invite.key, response)
            .await
            .expect("refuses");
    });
    let sent = sipx()
        .args([
            "dial",
            &format!("sip:header@{address}"),
            "--header",
            "Supported: dph-eight",
            "--timeout",
            "5",
            "--json",
        ])
        .output()
        .await
        .expect("dial runs");
    peer.await.expect("peer finishes");
    assert_eq!(sent.status.code(), Some(6));

    let refused = sipx()
        .args([
            "dial",
            "sip:header@127.0.0.1:9",
            "--header",
            "Via: SIP/2.0/UDP injected.invalid",
            "--local",
            "this-is-not-an-address",
            "--json",
        ])
        .output()
        .await
        .expect("refusal runs");
    assert_eq!(refused.status.code(), Some(2));
    let complaint = String::from_utf8_lossy(&refused.stderr);
    assert!(complaint.contains("stack-owned field Via"), "{complaint}");
    assert!(
        !complaint.contains("--local must"),
        "header validation must win: {complaint}"
    );
}

/// DPH-9: the real process reads a finite shell pipeline, waits for the answer event instead of a
/// delay, sends DTMF, hangs up, and correlates every completion in causal sequence.
#[tokio::test]
async fn scenario_waits_for_answer_then_sends_dtmf_and_hangs_up_in_causal_order() {
    use tokio::io::AsyncWriteExt as _;

    let _scenario = process_scenario().await;
    let (mut answerer, address, _lines) = start_answerer(&["--duration", "2"]).await;
    let mut child = sipx()
        .args(["scenario", "--local", "127.0.0.1:0"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("scenario starts");
    let script = format!(
        "{{\"id\":\"dial-1\",\"command\":\"dial\",\"uri\":\"sip:scenario@{address}\",\"timeout_ms\":5000}}\n\
         {{\"id\":\"wait-1\",\"command\":\"wait_for\",\"event\":\"call.answered\",\"timeout_ms\":5000}}\n\
         {{\"id\":\"digit-1\",\"command\":\"send_dtmf\",\"digits\":\"5\"}}\n\
         {{\"id\":\"hangup-1\",\"command\":\"hangup\"}}\n\
         {{\"id\":\"shutdown-1\",\"command\":\"shutdown\"}}\n"
    );
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(script.as_bytes())
        .await
        .expect("script writes");
    let output = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output())
        .await
        .expect("scenario is bounded")
        .expect("scenario exits");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .expect("UTF-8 NDJSON")
        .lines()
        .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
        .collect();
    assert!(lines.len() >= 8, "ready, events and completions: {lines:?}");
    for (index, line) in lines.iter().enumerate() {
        assert_eq!(line["contract"], "sipx.app.v1");
        assert_eq!(
            line["seq"].as_u64(),
            Some(u64::try_from(index + 1).expect("small index"))
        );
    }
    let position = |event: &str, id: Option<&str>| {
        lines
            .iter()
            .position(|line| {
                line["event"]["type"] == event && id.is_none_or(|id| line["event"]["id"] == id)
            })
            .unwrap_or_else(|| panic!("missing {event} {id:?}: {lines:?}"))
    };
    let answered = position("call.answered", None);
    let waited = position("scenario.command.completed", Some("wait-1"));
    let digit = position("scenario.command.completed", Some("digit-1"));
    let ended = position("call.ended", None);
    let hung_up = position("scenario.command.completed", Some("hangup-1"));
    assert!(answered < waited && waited < digit && digit < ended && ended < hung_up);

    answerer_exits_cleanly(&mut answerer).await;
}

/// Logging must never reach stdout, or one verbosity flag turns every JSON result into a parse
/// error at the far end of a pipe.
///
/// **This test used to be unable to observe its own name (`X-53`).** It ran
/// `dial sip:bob@example.com --json -vv`, an invocation refused as a usage error before any socket
/// is bound, so the process emitted no log record at all and `stdout.is_empty()` held exactly as
/// well with logging writing to stdout as to stderr. Making `init_logging` write to stdout — the
/// one defect it exists to catch — left it green. It is the same shape `X-45` fixed one story
/// earlier, and `X-36` before that: an assertion about the absence of a side effect, in a run that
/// never enters the code that would produce it.
///
/// So the subject is now a process that logs. That is the **answerer** rather than the caller: a
/// caller that completes a call emits no record at any verbosity, while the answerer does, and the
/// answerer's stdout carries JSON result lines — which is precisely the pipe this test protects.
/// A real call is placed against it, and:
///
/// - its **stderr carries log records**, which is what makes the rest mean anything. Without that
///   control, "no records on stdout" is equally consistent with logging being broken outright,
///   which is the failure a test named for logging can least afford to miss;
/// - the **same call run quietly carries none**, so those records are attributable to the verbosity
///   flag and not to something that would have been logged regardless;
/// - **stdout carries results only**, in both runs, asserted line by line as well as by record; and
/// - the **exit code is asserted**, which the old test never did. It silently depended on being
///   refused; an invocation that started or stopped being refused would have moved it to another
///   code path without failing.
///
/// The verbose spelling is the documented one, `-vv`, and its control is DEBUG specifically. It was
/// `-v -v` until `X-57`: verbosity was counted as the number of *arguments* beginning with `-v`, so
/// `-vv` counted once and got the INFO ceiling, under which the answerer's records — all DEBUG — were
/// filtered out. The undocumented spelling was the only one that could produce the control this test
/// needs, which is why it used it and reported the defect rather than fixing it.
#[tokio::test]
async fn verbose_logging_stays_off_stdout() {
    let _scenario = process_scenario().await;
    let dir = scratch("verbose-logging");

    let loud = place_a_call(&dir, &["-vv"], &[]).await;
    // The negative comes before its own control, deliberately: writing records to stdout empties
    // stderr as a side effect, so both assertions fire together on that one defect and the first to
    // panic is the one that gets to name it.
    let loud_stdout = loud.answerer_stdout.join("\n");
    let on_stdout = log_records(&loud_stdout);
    assert!(
        on_stdout.is_empty(),
        "stdout must carry results only, got the log records {on_stdout:?}"
    );
    let on_stderr = log_records(&loud.answerer_stderr);
    assert!(
        on_stderr.iter().any(|record| record.contains("DEBUG")),
        "`-vv` is documented as DEBUG and the answerer's records are DEBUG, so an absence of them \
         means the second `v` was not counted and the run was capped at INFO — and it also leaves \
         the clean stdout above equally consistent with logging being broken outright. Records \
         seen: {on_stderr:?}, whole stream: {}",
        loud.answerer_stderr
    );
    assert!(
        loud.answerer_stdout
            .iter()
            .all(|line| line.starts_with('{') && line.ends_with('}')),
        "every line on stdout has to be a JSON result a pipe can parse: {:?}",
        loud.answerer_stdout
    );
    assert_eq!(
        loud.answerer_status.code(),
        Some(0),
        "the verbose answerer has to have taken the path that answers a call: {}",
        loud.answerer_stderr
    );
    assert_eq!(
        loud.caller.status.code(),
        Some(0),
        "the verbose run has to have been a completed call: {}",
        String::from_utf8_lossy(&loud.caller.stderr)
    );

    let quiet = place_a_call(&dir, &[], &[]).await;
    assert!(
        !log_records(&quiet.answerer_stderr)
            .iter()
            .any(|record| record.contains("DEBUG")),
        "an answerer nobody asked for verbosity logged at DEBUG anyway, so the records above say \
         nothing about the flag: {}",
        quiet.answerer_stderr
    );
    let quiet_written = quiet.answerer_stdout.join("\n");
    let quiet_stdout = log_records(&quiet_written);
    assert!(
        quiet_stdout.is_empty(),
        "stdout must carry results only, got the log records {quiet_stdout:?}"
    );
    assert_eq!(
        quiet.answerer_status.code(),
        Some(0),
        "the quiet answerer has to have taken the same path as the verbose one: {}",
        quiet.answerer_stderr
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `-v` on its own reports the call, on both ends of it (`X-57`).
///
/// The flag was **accepted and inert**: the only two `tracing::info!` sites in the workspace are a
/// registration refresh and a transcoding bridge, neither of which a call goes anywhere near, so a
/// documented level produced nothing whatsoever on the path an operator reaches for it on. That is
/// the same shape as a capture that can only be switched on by editing code — the flag is there, and
/// the thing it promises is not. Either the help text had to stop promising it or the records had to
/// exist; `sipx` now logs the call's own lifecycle at INFO, so both ends have something to say.
///
/// Asserted on the **caller** as well as the answerer, because the caller was the worse half: it
/// emitted no record at *any* verbosity, so `sipx dial -v` was silent all the way through a call that
/// worked.
///
/// The absence of DEBUG here is what makes the neighbour above mean something: it establishes that
/// one `v` stops at INFO, so the DEBUG records that test demands of `-vv` are attributable to the
/// second `v` and not to a flag that switches everything on at once.
#[tokio::test]
async fn one_v_reports_the_call_on_both_ends_of_it() {
    let _scenario = process_scenario().await;
    let dir = scratch("verbosity-info");
    let placed = place_a_call(&dir, &["-v"], &["-v"]).await;

    let caller_stderr = String::from_utf8_lossy(&placed.caller.stderr).into_owned();
    for (who, stream) in [
        ("answerer", placed.answerer_stderr.as_str()),
        ("caller", caller_stderr.as_str()),
    ] {
        let records = log_records(stream);
        assert!(
            records.iter().any(|record| record.contains("INFO")),
            "`{who} -v` documents INFO and this call produced {records:?}, so the level is accepted \
             and inert — the operator asked for the call to be reported and got silence: {stream}"
        );
        assert!(
            !records.iter().any(|record| record.contains("DEBUG")),
            "one `v` is INFO, so DEBUG from the {who} means the ladder has no rung for `-vv` to \
             climb to: {records:?}"
        );
    }

    let written = placed.answerer_stdout.join("\n");
    let on_stdout = log_records(&written);
    assert!(
        on_stdout.is_empty(),
        "the records `-v` added must be on stderr like every other record, or one verbosity flag \
         turns every JSON result into a parse error: {on_stdout:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// DTMF sent by the caller is reported by the answering side.
#[tokio::test]
async fn digits_sent_by_the_caller_are_reported_by_the_answerer() {
    let _scenario = process_scenario().await;
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
    answerer_exits_cleanly(&mut answerer).await;
}

/// Calling something that never answers gives up on the caller's schedule rather than on the
/// transaction layer's. 64*T1 is 32 seconds — correct for SIP, and far too long for a script
/// that wanted either an answer or an error.
#[tokio::test]
async fn a_call_that_is_never_answered_times_out_on_schedule() {
    let _scenario = process_scenario().await;
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
    // `X-40`'s sweep left this clock deliberately, and this is the reason at the site that `X-29`
    // asks for. The elapsed time here is not a wait standing in for an arrival — it *is* the
    // measurement, which is `X-29`'s third category: the whole claim is *which* schedule fired, and
    // the only way to read that is the clock. There is nothing to poll for, because the thing under
    // test is which of two durations elapsed.
    //
    // What keeps it out of the flaky family is the width of the gap it has to resolve: it separates
    // our 3 s from 64*T1's 32 s, so anything comfortably between them does, and 12 s is four times
    // the schedule that should fire. Load can only push the number up, and the 20 s timeout above is
    // the next bound in the same direction — so a starved run fails here rather than passing wrongly.
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
    let _scenario = process_scenario().await;
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
    let _scenario = process_scenario().await;
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
    let _scenario = process_scenario().await;
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
    let _scenario = process_scenario().await;
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

/// Every documented seconds flag refuses a non-number before command-specific validation or I/O.
///
/// The cases come from each command's own help instead of repeating the current five names. A new
/// `<S>` flag therefore joins this assertion on the same change that documents it. The deliberately
/// invalid positional/address after the flag proves ordering too: before `S-32`, `answer --duration
/// notanumber` silently took 30 and failed on `--local` instead, while `register --expires` failed on
/// its address of record. A refusal that does not name the numeric flag is the old defect.
#[tokio::test]
async fn a_non_number_is_refused_by_every_numeric_flag() {
    let _scenario = process_scenario().await;
    let cases: [(&str, &[&str]); 3] = [
        ("dial", &["not-a-uri", "--json"]),
        ("answer", &["--local", "not-an-address", "--json"]),
        ("register", &["not-an-aor", "--json"]),
    ];

    for (command, invalid_after_arguments) in cases {
        let help = sipx()
            .args([command, "--help"])
            .output()
            .await
            .expect("help runs");
        let help = String::from_utf8_lossy(&help.stdout);
        let flags: Vec<String> = help
            .lines()
            .filter_map(|line| {
                let rest = line.trim_start().strip_prefix("--")?;
                let (flag, tail) = rest.split_once(char::is_whitespace)?;
                tail.trim_start()
                    .starts_with("<S>")
                    .then(|| format!("--{flag}"))
            })
            .collect();
        assert!(!flags.is_empty(), "{command} documents no seconds flags");

        for flag in flags {
            let mut arguments = vec![command, flag.as_str(), "notanumber"];
            arguments.extend(invalid_after_arguments.iter().copied());
            let output = sipx().args(&arguments).output().await.expect("runs");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.code(),
                Some(2),
                "`sipx {}` is a usage error: {stderr}",
                arguments.join(" ")
            );
            assert!(
                stderr.contains(flag.as_str()) && stderr.contains("whole number"),
                "the refusal must name {flag} and its required domain: {stderr}"
            );
        }
    }
}

/// The flags a help text documents as taking a value: a flag whose line shows a `<PLACEHOLDER>`
/// after it.
///
/// Derived from the binary's own `--help` rather than listed here, so a flag added later is swept
/// in without anyone remembering to add it. `main.rs`'s
/// `every_valued_flag_in_the_help_text_is_registered` holds the help text and `VALUED_FLAGS` to
/// each other, which is what makes "documented with a placeholder" and "registered as valued" the
/// same set.
fn documented_valued_flags(help: &str) -> Vec<String> {
    let mut flags = Vec::new();
    for line in help.lines() {
        let Some(rest) = line.trim_start().strip_prefix("--") else {
            continue;
        };
        let Some((flag, tail)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        if tail.trim_start().starts_with('<') {
            flags.push(format!("--{flag}"));
        }
    }
    flags
}

/// A valued flag that was given no value is a usage error naming the flag — for every such flag,
/// in every command, in both of the ways a value goes missing.
///
/// `Args::value` answered `None` both for "the flag was last, so nothing followed it" and for "the
/// flag was absent", so every caller took its absent-branch and the command ran on a default that
/// was never asked for: `sipx register sip:alice@example.com --outbound --instance` exited 0 having
/// generated an instance URN nobody typed (`S-30`). The empty right-hand side is the same mistake
/// wearing a shell's clothes — `--target "$ADDR"` with `ADDR` unset arrives as `--target ""`.
///
/// The extra arguments per command exist only to make the *old* behaviour cheap to observe: with
/// the flag honoured this exits before opening a socket, but a run against the defect places a real
/// call, and `--timeout 1`/`--wait 1` keep that to a second instead of the transaction layer's 32.
#[tokio::test]
async fn a_valued_flag_given_no_value_is_refused_by_every_command() {
    let _scenario = process_scenario().await;
    let dir = scratch("valueless-flags");
    let book = dir.join("peers");
    std::fs::write(&book, "alice sip:alice@192.0.2.17:5060\n").expect("writes");

    let cases: [(&str, &[&str]); 4] = [
        ("register", &["sip:alice@example.com", "--json"]),
        (
            "dial",
            &[
                "sip:bob@192.0.2.1:5060",
                "--local",
                "127.0.0.1:0",
                "--timeout",
                "1",
                "--json",
            ],
        ),
        (
            "answer",
            &["--local", "127.0.0.1:0", "--wait", "1", "--json"],
        ),
        ("peers", &["--json"]),
    ];

    for (command, extra) in cases {
        let help = sipx()
            .args([command, "--help"])
            .output()
            .await
            .expect("runs");
        let help = String::from_utf8_lossy(&help.stdout).into_owned();
        let flags = documented_valued_flags(&help);
        assert!(
            !flags.is_empty(),
            "{command} documents no valued flags, so this asserts nothing:\n{help}"
        );

        for flag in &flags {
            // Nothing after the flag at all, then an empty right-hand side.
            for trailing in [flag.clone(), format!("{flag}=")] {
                let mut args: Vec<&str> = vec![command];
                args.extend(extra.iter().copied());
                args.push(trailing.as_str());

                let output = sipx()
                    .args(&args)
                    // `peers` falls back through the environment when `--book` is absent, and
                    // whether this machine has a peer book must not decide the result.
                    .env("SIPX_PEERS", &book)
                    .output()
                    .await
                    .expect("runs");

                let rendered = args.join(" ");
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert_eq!(
                    output.status.code(),
                    Some(2),
                    "`sipx {rendered}` must be a usage error, not a run on a default: {stderr}"
                );
                assert!(
                    stderr.contains(flag.as_str()),
                    "`sipx {rendered}` must name {flag} in its refusal: {stderr}"
                );
                assert!(
                    String::from_utf8_lossy(&output.stdout).is_empty(),
                    "`sipx {rendered}` refused, so nothing may reach stdout where it would be \
                     parsed as a result: {:?}",
                    String::from_utf8_lossy(&output.stdout)
                );
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// What one completed call left behind, for the tests that assert about it afterwards.
struct Placed {
    /// Every line the answerer wrote to stdout after the one announcing its port.
    answerer_stdout: Vec<String>,
    /// Everything the answerer wrote to stderr.
    answerer_stderr: String,
    /// How the answerer exited. [`place_a_call`] has already held it to success; it is carried so a
    /// test can say which code path it depends on rather than inheriting it silently (`X-53`).
    answerer_status: std::process::ExitStatus,
    /// The caller's streams and status, likewise already held to success.
    caller: std::process::Output,
}

/// Place one real call against a fresh answerer, both processes running in `dir`, and return only
/// once the answerer has exited cleanly.
///
/// The three tests below need exactly the same thing and must not disagree about what it is. A
/// capture records signalling, so a test about capture that places no call has no subject; a process
/// that is refused before it binds a socket logs nothing, so a test about logging that never places
/// a call has none either (`X-45`, `X-53`); and the answerer's *exit* is what flushes both the
/// capture writer and its streams, so nothing may be read before it. `X-45` factored this out of the
/// positive capture test rather than give the negative one a harness of its own, because two
/// harnesses drift and a pair that has drifted is no longer a pair.
///
/// Every wait here is causal — the answerer announces its port, the caller exits, the answerer's
/// streams reach end of stream, the answerer exits — and the durations are bounds on failure, not
/// measurements (`X-28`, `X-29`). The exception is `--duration 1`, which is neither: it is how long
/// the call lasts before either end hangs up.
///
/// Both of the answerer's streams are read concurrently and to the end, rather than one line of
/// stdout being taken and the rest discarded: a caller can only assert that stdout carries results
/// *only* if it has all of stdout, and a process left writing into an unread pipe while the other is
/// drained can block forever.
///
/// **Both ends take flags**, because both ends are subjects: `X-57` asserts that a verbosity flag
/// reports the call on the *caller's* path as well as the answerer's, and a helper that could only
/// configure the answerer would have left that half to a second harness. They are separate lists
/// rather than one shared list on purpose — `--capture` names a file, and two processes handed the
/// same path would be writing over each other.
async fn place_a_call(
    dir: &std::path::Path,
    answerer_flags: &[&str],
    caller_flags: &[&str],
) -> Placed {
    let mut args = vec!["--duration", "1"];
    args.extend_from_slice(answerer_flags);
    let (mut answerer, address, mut lines) = start_answerer_in(Some(dir), &args).await;

    let target = format!("sip:answer@{address}");
    let mut dial = vec![
        "dial",
        target.as_str(),
        "--local",
        "127.0.0.1:0",
        "--json",
        "--duration",
        "1",
        "--timeout",
        "15",
    ];
    dial.extend_from_slice(caller_flags);
    let caller = tokio::time::timeout(
        Duration::from_secs(40),
        sipx().current_dir(dir).args(&dial).output(),
    )
    .await
    .expect("the caller finishes")
    .expect("runs");
    assert!(
        caller.status.success(),
        "dial failed: {} / {}",
        String::from_utf8_lossy(&caller.stdout),
        String::from_utf8_lossy(&caller.stderr)
    );

    let mut stderr = answerer.stderr.take();
    let (answerer_stdout, answerer_stderr) = tokio::time::timeout(Duration::from_secs(25), async {
        tokio::join!(
            async {
                let mut written = Vec::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    written.push(line);
                }
                written
            },
            async {
                let mut complaint = Vec::new();
                if let Some(stderr) = stderr.as_mut() {
                    let _ = tokio::io::AsyncReadExt::read_to_end(stderr, &mut complaint).await;
                }
                String::from_utf8_lossy(&complaint).into_owned()
            }
        )
    })
    .await
    .expect("the answerer closes its streams rather than holding them open");

    // Scanned rather than taken from the head of the list: a sabotaged build that writes log records
    // to stdout would put one ahead of the result line, and that must fail the test that is *about*
    // records on stdout, with the diagnosis that test gives — not this one.
    assert!(
        answerer_stdout
            .iter()
            .any(|line| line.contains("\"status\":\"answered\"")),
        "the call has to have happened for anything below to be about a call: {answerer_stdout:?}"
    );

    let answerer_status = exits_cleanly(&mut answerer, &answerer_stderr).await;

    Placed {
        answerer_stdout,
        answerer_stderr,
        answerer_status,
        caller,
    }
}

/// The lines of `stream` that are `tracing` log records.
///
/// Recognised by shape — a level word beside one of our own crate targets — rather than by message
/// text. What is under test is which stream a record lands on, not what any subsystem chose to say,
/// and matching a message would turn a reworded log line into a logging regression.
///
/// Both target spellings count. A library record is targeted at its crate — `sipx_call::call` — while
/// the binary's own records are targeted at its module path, and the binary is named `sipx`, so those
/// read `sipx::dial`. Matching only the first spelling made the records `X-57` put on the call's path
/// invisible to a test written to look for them, which is a test that cannot observe its own subject.
/// Neither spelling is a bare `sipx`: that appears in `sip:sipx@…` on every result line.
fn log_records(stream: &str) -> Vec<&str> {
    stream
        .lines()
        .filter(|line| {
            (line.contains("sipx_") || line.contains("sipx::"))
                && ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"]
                    .iter()
                    .any(|level| line.contains(level))
        })
        .collect()
}

/// **`X-18`'s command-line half.** `--capture <path>` records the signalling of a real call.
///
/// The story's reason for wanting this on the command line rather than only in a test is the
/// vision's "testable from a shell": a capture that can only be switched on by editing code is
/// unavailable in the incident it exists for. So this runs the built binary and reads the file a
/// shell would be left holding.
///
/// The assertion is a substring search over the whole file rather than a parsed pcapng, and that is
/// deliberate here: `sipx-transport`'s own tests parse the format block by block, so what is left to
/// establish at this layer is only that the flag reached `Config::capture` at all. Duplicating the
/// reader would test the reader twice and the flag once.
#[tokio::test]
async fn the_capture_flag_records_the_signalling_of_a_call() {
    let _scenario = process_scenario().await;
    let dir = scratch("capture-flag");
    let capture = dir.join("signalling.pcapng");

    // The capture is read only after the call has run its course and the answerer has exited, which
    // is what flushes it — `place_a_call` will not return before then, and it asserts the exit
    // rather than discarding it, so an empty capture cannot be an answerer that died holding it
    // (`X-40`).
    place_a_call(&dir, &["--capture", capture.to_str().expect("a path")], &[]).await;

    let bytes = std::fs::read(&capture).expect("the capture the flag asked for exists");
    assert!(
        bytes.len() > 100,
        "the capture is {} bytes, so nothing was written to it",
        bytes.len()
    );
    let whole = String::from_utf8_lossy(&bytes);
    // The signalling of the call that just happened, in both directions.
    assert!(whole.contains("INVITE sip:"), "no INVITE in the capture");
    assert!(
        whole.contains("SIP/2.0 200"),
        "no answer in the capture, so only one direction was recorded"
    );
    // pcapng, not a text log: the Section Header Block's own comment.
    assert!(
        whole.contains("sipx signalling capture"),
        "the file is not a pcapng section"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **`X-54`'s command-line half.** The counters come out of the process beside the capture.
///
/// M12's third clause asks for every discard in the signalling path to be counted **and exportable
/// next to** a capture of the traffic that caused it. `X-18` built both halves and `X-51` found
/// that nothing joined them: `Handle::counters` and `Calls::counts` were read by each crate's own
/// tests and by nothing else, so from a shell — the [vision](../../../docs/vision.md)'s own measure
/// of usable — the capture and the numbers were two features that existed separately.
///
/// So `--capture` implies the counters file rather than requiring a second flag: whoever took a
/// capture is assembling a bug report, and the numbers explaining it belong in the same bundle.
/// The run names the file it wrote, which is what keeps an implied file from being a surprise.
#[tokio::test]
async fn the_capture_flag_leaves_the_counters_beside_the_capture() {
    let _scenario = process_scenario().await;
    let dir = scratch("counters-beside-capture");
    let capture = dir.join("signalling.pcapng");
    let counters = dir.join("signalling.pcapng.counters.json");

    let placed = place_a_call(&dir, &["--capture", capture.to_str().expect("a path")], &[]).await;

    let body = std::fs::read_to_string(&counters)
        .expect("the counters file --capture implies exists beside the capture");

    // Real traffic happened, so the transport half is populated rather than a zeroed template.
    assert!(
        body.contains("\"messages_in\""),
        "the transport's own numbers are missing: {body}"
    );
    assert!(
        !body.contains("\"messages_in\": 0,") && !body.contains("\"messages_in\":0,"),
        "a call was placed, so messages_in cannot be zero: {body}"
    );
    // §12.1's fields are what the clause is about, and they are named even at zero — a discard
    // counter that only appears once it fires cannot be used to rule a cause out.
    for field in ["unsent_bye", "unsent_cancel", "discard_send_failures"] {
        assert!(body.contains(field), "{field} is missing from {body}");
    }
    // §12.2 applied to the export: no `Dispatcher` runs in these commands, so the dialog layer's
    // refusals are *unmeasured* here and the file says so rather than reporting zeros for them.
    assert!(
        body.contains("\"dispatch_measured\": false")
            || body.contains("\"dispatch_measured\":false"),
        "an unasked question must not be exported as a negative answer: {body}"
    );
    assert!(
        !body.contains("dispatch_acks"),
        "a dispatcher that never ran must not contribute counts: {body}"
    );

    // The run said where it put it, so nothing appeared that the command did not mention.
    let said = placed.answerer_stdout.join("\n");
    assert!(
        said.contains("signalling.pcapng.counters.json"),
        "the answerer's report must name the counters file it wrote: {said}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **The run that fails is the run that needs the numbers.**
///
/// `X-54`'s first version wrote the counters only after the call had already succeeded, so a dial
/// that timed out produced the capture and no counters at all — inverting Acceptance item 3's own
/// words on precisely the run a bug report is about, and contradicting the claim in
/// `crates/sipx-cli/src/counters.rs` that a counters file which silently did not appear is the
/// §13.2 failure one level up. The export is armed straight after `bind` now, so every `return
/// fail(…)` takes the file with it.
///
/// Dialling a discard port that nothing answers, with a short timeout, is the cheapest honest
/// failure: no peer process to manage, and the outcome does not depend on anything answering.
#[tokio::test]
async fn a_failed_run_still_exports_its_counters() {
    let _scenario = process_scenario().await;
    let dir = scratch("counters-on-failure");
    let capture = dir.join("sig.pcapng");
    let counters = dir.join("sig.pcapng.counters.json");

    let output = sipx()
        .current_dir(&dir)
        .args([
            "dial",
            "--capture",
            capture.to_str().expect("a path"),
            // A bound on failure, not a measurement: the peer never answers, so this only decides
            // how long the test waits to find that out.
            "--timeout",
            "3",
            "sip:bob@127.0.0.1:9",
        ])
        .output()
        .await
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "this test is about the failing path, and the dial succeeded"
    );
    // The capture was always written on this path. The counters are the half that was missing.
    assert!(
        capture.exists(),
        "the capture is written on a failed run, which is what made the missing counters a gap"
    );
    let body = std::fs::read_to_string(&counters)
        .expect("a failed run must still export its counters beside the capture");
    assert!(
        body.contains("\"unsent_bye\"") && body.contains("\"any_loss\""),
        "the export is a full snapshot even on the failing path: {body}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Off unless asked for, at the command line as well as in the library: a real call, placed with no
/// `--capture`, leaves nothing on disk.
///
/// **This test used to be unable to observe either half of its own name (`X-45`).** It killed the
/// answerer the instant it announced its port, and then asserted that one path did not exist. Both
/// halves were vacuous. A capture is written while signalling flows, so a run with no call cannot
/// see a capture being written; and the path it watched was one nothing would ever write to, because
/// a capture nobody asked for is given no path and can only fall back to a name compiled into the
/// binary. Sabotaging `apply_capture` to capture unconditionally — the exact defect this guards —
/// left the old test green.
///
/// So both halves are fixed here. A **real call** happens, because signalling crossing the wire is
/// the only thing a capture could record. And the assertion is over the **directory** the two
/// processes ran in rather than a path chosen in advance: an unconditional capture has to put its
/// file somewhere, and absent a flag that somewhere is a relative default, so an empty directory
/// catches it whatever it is called. (A compiled-in *absolute* default would still escape. That is
/// the known edge of this assertion, and a far less likely regression than a bare file name.)
///
/// The **positive control comes first and is what makes the negative mean anything**: without it,
/// "no file appeared" is equally consistent with capture being broken outright, which is the failure
/// mode a test named for the flag being off is least able to notice. The neighbour above asserts
/// what a capture *contains*; the control here asserts only that this same call, in this same
/// directory, does produce one when asked — which is the claim the absence below is measured
/// against.
#[tokio::test]
async fn no_capture_flag_means_no_file() {
    let _scenario = process_scenario().await;
    let dir = scratch("capture-absent");
    // A directory each, so neither run can see the other's files.
    let asked = dir.join("asked");
    let unasked = dir.join("unasked");
    std::fs::create_dir_all(&asked).expect("a directory");
    std::fs::create_dir_all(&unasked).expect("a directory");

    let wanted = asked.join("signalling.pcapng");
    place_a_call(
        &asked,
        &["--capture", wanted.to_str().expect("a path")],
        &[],
    )
    .await;
    let control = std::fs::read(&wanted).expect("the control capture exists");
    assert!(
        String::from_utf8_lossy(&control).contains("INVITE sip:"),
        "the control captured no signalling, so an absence below would prove nothing about the flag"
    );

    place_a_call(&unasked, &[], &[]).await;
    let left_behind: Vec<std::path::PathBuf> = std::fs::read_dir(&unasked)
        .expect("the directory the call ran in")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    assert!(
        left_behind.is_empty(),
        "the same call, with no --capture, wrote {left_behind:?} — and the control above proves \
         this run would have produced a capture had one been asked for"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
