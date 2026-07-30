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
use sipx_testkit::certs::Ca;
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
    let mut complaint = Vec::new();
    if let Some(mut stderr) = answerer.stderr.take() {
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut complaint).await;
    }
    let status = tokio::time::timeout(Duration::from_secs(30), answerer.wait())
        .await
        .expect("the answerer exits rather than hanging")
        .expect("waits");
    assert!(
        status.success(),
        "the answerer exited with {status}, so anything asserted about what it recorded, heard or \
         captured describes a process that failed: {}",
        String::from_utf8_lossy(&complaint)
    );
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

/// `DPH-1`, plus the cleartext transports around it: every released signalling transport is
/// selected through the command line and carries a complete, bounded call. The assertions are on
/// both processes' terminal reports so a flag accepted and then ignored cannot pass.
#[tokio::test]
async fn dph_1_every_released_transport_carries_a_loopback_command_call() {
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

/// Registration uses the same selection and certificate policy as calls. This is deliberately a
/// real endpoint rather than a mock byte sink: TCP framing, TLS and both WebSocket handshakes must
/// complete before the REGISTER reaches the registrar.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one matrix test keeps identical assertions visible for every released transport"
)]
async fn register_selects_every_released_transport() {
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

    answerer_exits_cleanly(&mut answerer).await;
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
    answerer_exits_cleanly(&mut answerer).await;
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

/// Place one real call against a fresh answerer, both processes running in `dir`, and return only
/// once the answerer has exited cleanly.
///
/// The two capture tests below need exactly the same thing and must not disagree about what it is.
/// A capture records signalling, so a test about capture that places no call has no subject; and the
/// answerer's *exit* is what flushes the writer, so nothing may be read before it. `X-45` factored
/// this out of the positive test rather than give the negative one a harness of its own, because two
/// harnesses drift and a pair that has drifted is no longer a pair.
///
/// Every wait here is causal — the answerer announces its port, the caller exits, the answerer
/// prints its result line, the answerer exits — and the durations are bounds on failure, not
/// measurements (`X-28`, `X-29`). The exception is `--duration 1`, which is neither: it is how long
/// the call lasts before either end hangs up.
async fn place_a_call(dir: &std::path::Path, answerer_flags: &[&str]) {
    let mut args = vec!["--duration", "1"];
    args.extend_from_slice(answerer_flags);
    let (mut answerer, address, mut lines) = start_answerer_in(Some(dir), &args).await;

    let caller = tokio::time::timeout(
        Duration::from_secs(40),
        sipx()
            .current_dir(dir)
            .args([
                "dial",
                &format!("sip:answer@{address}"),
                "--local",
                "127.0.0.1:0",
                "--json",
                "--duration",
                "1",
                "--timeout",
                "15",
            ])
            .output(),
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

    let answered = tokio::time::timeout(Duration::from_secs(25), lines.next_line())
        .await
        .expect("no timeout")
        .expect("a line")
        .expect("the result line");
    assert!(
        answered.contains("\"status\":\"answered\""),
        "the call has to have happened for anything below to be about a call: {answered}"
    );

    answerer_exits_cleanly(&mut answerer).await;
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
    let dir = scratch("capture-flag");
    let capture = dir.join("signalling.pcapng");

    // The capture is read only after the call has run its course and the answerer has exited, which
    // is what flushes it — `place_a_call` will not return before then, and it asserts the exit
    // rather than discarding it, so an empty capture cannot be an answerer that died holding it
    // (`X-40`).
    place_a_call(&dir, &["--capture", capture.to_str().expect("a path")]).await;

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
    let dir = scratch("capture-absent");
    // A directory each, so neither run can see the other's files.
    let asked = dir.join("asked");
    let unasked = dir.join("unasked");
    std::fs::create_dir_all(&asked).expect("a directory");
    std::fs::create_dir_all(&unasked).expect("a directory");

    let wanted = asked.join("signalling.pcapng");
    place_a_call(&asked, &["--capture", wanted.to_str().expect("a path")]).await;
    let control = std::fs::read(&wanted).expect("the control capture exists");
    assert!(
        String::from_utf8_lossy(&control).contains("INVITE sip:"),
        "the control captured no signalling, so an absence below would prove nothing about the flag"
    );

    place_a_call(&unasked, &[]).await;
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
