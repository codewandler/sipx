//! A realtime binding through the real host, SIP call, RTP media and stand-in peer (`A-22`).
//!
//! The trait fixture in `realtime_bridge.rs` proves queue arithmetic. This file proves the product
//! seam it cannot: a routed INVITE is answered by [`Host`], the resulting [`Call`] is switched to
//! encoded relay, and distinct payloads cross both real RTP directions without a transcode.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::{IpAddr, SocketAddr};
use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;
use serde_json::Value;
use sipx_app::host::{Host, RealtimeCallReport};
use sipx_app::realtime::BridgeOutcome;
use sipx_call::{DialOptions, dial};
use sipx_media::Encoded;
use sipx_sip::{Host as UriHost, HostName, Uri};
use sipx_testkit::realtime_peer::{FIXTURE_BEARER, PeerConfig, RealtimePeer, tone_frame};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Receiver;

/// A bound on failure. Positive assertions below complete on the network event itself.
const ARRIVAL: Duration = Duration::from_secs(10);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("loopback")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("an address")))
        .await
        .expect("the endpoint binds")
}

fn callee() -> Uri {
    Uri::sip(UriHost::Name(
        HostName::new("agent.example").expect("a host"),
    ))
}

fn document(peer: &RealtimePeer) -> String {
    format!(
        r#"
[listener.calls]
protocol  = "sip"
transport = "udp"
bind      = "127.0.0.1:5060"
app       = "agent"

[app.agent]
binding        = "realtime"
endpoint       = "{}"
model          = "gpt-realtime-2.1"
instructions   = "answer with the test tone"
api_key_secret = "openai-api-key"
"#,
        peer.url()
    )
}

struct RunningHost {
    address: SocketAddr,
    handle: Handle,
    reports: Receiver<RealtimeCallReport>,
    task: tokio::task::JoinHandle<Result<(), sipx_app::host::HostError>>,
}

impl RunningHost {
    async fn start(peer: &RealtimePeer, bearer: &[u8]) -> Self {
        let (handle, incoming) = endpoint().await;
        let address = handle.local_addr();
        let shutdown = handle.clone();
        let mut host = Host::start_with_secrets(&document(peer), loopback(), |name| {
            (name == "openai-api-key").then(|| bearer.to_vec())
        })
        .expect("the realtime host starts");
        let reports = host.take_realtime_reports().expect("one report stream");
        let task = tokio::spawn(async move { host.serve(handle, incoming).await });
        Self {
            address,
            handle: shutdown,
            reports,
            task,
        }
    }

    async fn report(&mut self) -> RealtimeCallReport {
        tokio::time::timeout(ARRIVAL, self.reports.recv())
            .await
            .expect("the call reports within its failure bound")
            .expect("the report stream stays open")
    }

    async fn stop(self) {
        self.handle.shutdown().await;
        tokio::time::timeout(ARRIVAL, self.task)
            .await
            .expect("the host joins after endpoint shutdown")
            .expect("the host task joins")
            .expect("the host serves without error");
    }
}

async fn dial_host(address: SocketAddr) -> (sipx_call::Call, Receiver<Incoming>, Handle) {
    let (caller, incoming) = endpoint().await;
    let call = tokio::time::timeout(
        ARRIVAL,
        dial(
            &caller,
            Target::udp(address),
            &callee(),
            &DialOptions::new("<sip:caller@test.example>", loopback()),
        ),
    )
    .await
    .expect("the host answers within the failure bound")
    .expect("the realtime binding answers");
    (call, incoming, caller)
}

#[tokio::test]
async fn a_routed_call_crosses_the_realtime_bridge_in_both_rtp_directions() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut host = RunningHost::start(&peer, FIXTURE_BEARER.as_bytes()).await;
    let (mut call, _inbox, _caller) = dial_host(host.address).await;
    call.media().set_relay(true);
    assert_eq!(
        call.negotiated_payload_type(),
        0,
        "the product path chose PCMU"
    );

    let uplink = tone_frame(7);
    let downlink = tone_frame(19);
    assert_ne!(uplink, downlink, "correlation uses two distinct signals");
    assert!(
        call.media()
            .send_encoded(Encoded {
                payload_type: 0,
                payload: Bytes::copy_from_slice(&uplink),
            })
            .await,
        "the caller sends one encoded RTP payload"
    );
    let record = peer
        .await_appends(1)
        .await
        .expect("the append reaches the peer");
    assert_eq!(
        record.appended_audio,
        uplink.to_vec(),
        "uplink byte identity"
    );

    peer.send_delta("resp_product", &downlink)
        .await
        .expect("the peer sends the distinct reply");
    let heard = tokio::time::timeout(ARRIVAL, call.media().recv_encoded())
        .await
        .expect("the reply reaches RTP within the failure bound")
        .expect("the media path stays open");
    assert_eq!(heard.payload_type, 0);
    assert_eq!(
        heard.payload.to_vec(),
        downlink.to_vec(),
        "downlink byte identity"
    );

    call.hang_up().await.expect("the caller hangs up cleanly");
    let report = host.report().await;
    assert_eq!(report.codec, "PCMU");
    assert_eq!(report.packet_duration_ms, 20);
    assert_eq!(report.bridge.outcome, BridgeOutcome::CallEnded);
    let json: Value = serde_json::from_str(&report.to_json()).expect("the CLI record is JSON");
    assert_eq!(json["codec"], "PCMU");
    assert_eq!(json["packet_duration_ms"], 20);
    assert_eq!(json["session_outcome"], "call_ended");
    host.stop().await;
}

#[tokio::test]
async fn a_wrong_bearer_never_bridges_on_the_real_call_path() {
    const WRONG: &[u8] = b"wrong-product-bearer";
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let mut host = RunningHost::start(&peer, WRONG).await;
    let (mut call, mut inbox, _caller) = dial_host(host.address).await;

    let (served, report) = tokio::time::timeout(ARRIVAL, async {
        tokio::join!(sipx_call::serve(&mut call, &mut inbox), host.report())
    })
    .await
    .expect("the refusal releases both owners within the failure bound");
    served.expect("the caller processes the host's BYE");
    assert_eq!(
        report.bridge.outcome,
        BridgeOutcome::AuthRefused {
            secret: "openai-api-key".to_owned(),
            status: Some(401),
        }
    );
    assert!(call.is_ended(), "the refused session releases the SIP call");
    assert_eq!(peer.record().appends(), 0, "no caller audio was admitted");
    let printed = format!("{report:?} {}", report.to_json());
    assert!(!printed.contains(std::str::from_utf8(WRONG).expect("test text")));
    assert!(printed.contains("openai-api-key"));
    host.stop().await;
}

/// The shipped one-command path: `sipx-host <document>` answers the call and prints its terminal
/// negotiated facts as one JSON line.
#[tokio::test]
async fn sipx_host_is_the_one_command_realtime_path_and_prints_json() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let path = std::env::temp_dir().join(format!(
        "sipx-a22-host-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time moves forward")
            .as_nanos()
    ));
    let cli_document = document(&peer).replace("127.0.0.1:5060", "127.0.0.1:0");
    std::fs::write(&path, cli_document).expect("the temporary document is written");

    let mut host = Command::new(env!("CARGO_BIN_EXE_sipx-host"))
        .arg(&path)
        .env("SIPX_SECRET_openai-api-key", FIXTURE_BEARER)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("sipx-host starts");
    let stdout = host.stdout.take().expect("stdout is piped");
    let stderr = host.stderr.take().expect("stderr is piped");
    let mut output = BufReader::new(stdout).lines();
    let mut diagnostics = BufReader::new(stderr).lines();
    let ready = tokio::time::timeout(ARRIVAL, diagnostics.next_line())
        .await
        .expect("the command reaches readiness within the failure bound")
        .expect("readiness is readable")
        .expect("the command names its listener");
    let address: SocketAddr = ready
        .split_whitespace()
        .nth(3)
        .unwrap_or_else(|| panic!("readiness has no address: {ready}"))
        .parse()
        .unwrap_or_else(|error| panic!("readiness has an invalid address: {ready}: {error}"));

    let (mut call, _inbox, _caller) = dial_host(address).await;
    call.media().set_relay(true);
    let uplink = tone_frame(31);
    assert!(
        call.media()
            .send_encoded(Encoded {
                payload_type: 0,
                payload: Bytes::copy_from_slice(&uplink),
            })
            .await
    );
    assert_eq!(
        peer.await_appends(1).await.expect("uplink").appended_audio,
        uplink.to_vec()
    );
    let downlink = tone_frame(47);
    peer.send_delta("resp_cli", &downlink)
        .await
        .expect("downlink");
    let heard = tokio::time::timeout(ARRIVAL, call.media().recv_encoded())
        .await
        .expect("downlink reaches the call")
        .expect("the call is live");
    assert_eq!(heard.payload.to_vec(), downlink.to_vec());
    call.hang_up().await.expect("the CLI-owned call hangs up");

    let line = tokio::time::timeout(ARRIVAL, output.next_line())
        .await
        .expect("the command reports within the failure bound")
        .expect("the JSON line is readable")
        .expect("one terminal record");
    let json: Value = serde_json::from_str(&line).expect("the command emits JSON");
    assert_eq!(json["contract"], "sipx.realtime.v1");
    assert_eq!(json["codec"], "PCMU");
    assert_eq!(json["packet_duration_ms"], 20);
    assert_eq!(json["session_outcome"], "call_ended");

    host.start_kill().expect("the bounded test stops the host");
    tokio::time::timeout(ARRIVAL, host.wait())
        .await
        .expect("the stopped host is reaped")
        .expect("the host process can be waited");
    std::fs::remove_file(&path).expect("the temporary document is removed");
}
