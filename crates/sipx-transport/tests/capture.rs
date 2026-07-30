//! Recording what the stack sent, and not recording the secrets in it.
//!
//! Vectors X12, X14, X15 and X16 of `docs/specs/sip-transport.md` §11. The redaction *rules* are
//! unit-tested next to the code in `src/capture.rs`; these tests are about the whole path — a real
//! endpoint, a real socket, a real file — because the rule being right is no use if the bytes reach
//! the file by a route that bypasses it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{
    CaptureConfig, Config, Handle, Incoming, Target, TransportKind, bind, new_branch,
};
use tokio::sync::mpsc::Receiver;

/// A bound on failure, not a window to measure in (`X-29`). The writer is on its own thread, so the
/// file appears when it appears; nothing here sleeps and then asserts.
const WRITING_BOUND: Duration = Duration::from_secs(10);

async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A path in a fresh directory, so concurrent tests never share a file.
fn capture_path(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "sipx-capture-{}-{name}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    directory.join("signalling.pcapng")
}

async fn recording(path: &Path) -> (Handle, Receiver<Incoming>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.capture = Some(CaptureConfig::new(path));
    bind(config).await.expect("binds with a capture")
}

async fn plain() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn request(sender: &Handle, method: &Method, call_id: &'static str) -> sipx_sip::Request {
    RequestBuilder::new(
        method.clone(),
        Uri::sip(Host::Name(HostName::new("callee.example").expect("valid"))),
    )
    .header(
        HeaderName::Via,
        Bytes::from(format!(
            "SIP/2.0/UDP {};rport;branch={}",
            sender.sent_by_for(TransportKind::Udp),
            new_branch()
        )),
    )
    .expect("via")
    .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
    .expect("to")
    .header(
        HeaderName::From,
        Bytes::from_static(b"<sip:caller@example.net>;tag=abc"),
    )
    .expect("from")
    .header(HeaderName::CallId, Bytes::from_static(call_id.as_bytes()))
    .expect("call-id")
    .cseq(1, method)
    .expect("cseq")
    .max_forwards(70)
    .build()
}

// ---------------------------------------------------------------------------------------------
// A pcapng reader, deliberately written from §13.1 and the format's own block structure rather
// than from a library: a capture nothing can read is the failure being tested for, so the test
// must not share code with the writer.
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
struct Packet {
    data: Vec<u8>,
    comment: String,
}

#[derive(Debug)]
struct Capture {
    linktype: u16,
    packets: Vec<Packet>,
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_ne_bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    bytes
        .get(at..at + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_ne_bytes)
}

/// Walk the options trailing a block body, returning the first `opt_comment`.
fn comment_in(body: &[u8], from: usize) -> String {
    let mut cursor = from;
    while let (Some(code), Some(len)) = (u16_at(body, cursor), u16_at(body, cursor + 2)) {
        if code == 0 {
            break;
        }
        let value_at = cursor + 4;
        let value = body
            .get(value_at..value_at + usize::from(len))
            .unwrap_or(&[]);
        if code == 1 {
            return String::from_utf8_lossy(value).into_owned();
        }
        cursor = value_at + usize::from(len).next_multiple_of(4);
    }
    String::new()
}

fn read_capture(path: &Path) -> Capture {
    let bytes = std::fs::read(path).expect("the capture file is readable");
    let mut linktype = 0;
    let mut packets = Vec::new();
    let mut cursor = 0usize;

    while cursor + 12 <= bytes.len() {
        let kind = u32_at(&bytes, cursor).expect("a block type");
        let total = u32_at(&bytes, cursor + 4).expect("a block length") as usize;
        assert!(
            total >= 12 && cursor + total <= bytes.len(),
            "block at {cursor} has an impossible length {total}; the file is malformed"
        );
        // The trailing length must equal the leading one — that redundancy is what makes a
        // truncated capture readable, which is §13.1's third reason for choosing the format.
        assert_eq!(
            u32_at(&bytes, cursor + total - 4).map(|len| len as usize),
            Some(total),
            "block at {cursor} does not repeat its own length"
        );
        let body = bytes.get(cursor + 8..cursor + total - 4).unwrap_or(&[]);

        match kind {
            0x0A0D_0D0A => {
                assert_eq!(
                    u32_at(body, 0),
                    Some(0x1A2B_3C4D),
                    "the section header is missing its byte-order magic"
                );
            }
            0x0000_0001 => linktype = u16_at(body, 0).expect("a linktype"),
            0x0000_0006 => {
                let captured = u32_at(body, 16).expect("a captured length") as usize;
                let data = body.get(20..20 + captured).unwrap_or(&[]).to_vec();
                let comment = comment_in(body, 20 + captured.next_multiple_of(4));
                packets.push(Packet { data, comment });
            }
            _ => {}
        }
        cursor += total;
    }

    Capture { linktype, packets }
}

/// The SIP message inside a synthesised IPv4/UDP packet: twenty bytes of IP, eight of UDP.
fn payload(packet: &Packet) -> String {
    String::from_utf8_lossy(packet.data.get(28..).unwrap_or(&[])).into_owned()
}

fn source_port(packet: &Packet) -> u16 {
    u16::from_be_bytes([packet.data[20], packet.data[21]])
}

fn destination_port(packet: &Packet) -> u16 {
    u16::from_be_bytes([packet.data[22], packet.data[23]])
}

// ---------------------------------------------------------------------------------------------

/// **Vector X12.** A request out and a response in, both in the file, with the real ports.
#[tokio::test]
async fn a_loopback_exchange_is_recorded_with_its_real_addresses() {
    let path = capture_path("exchange");
    let (caller, _caller_incoming) = recording(&path).await;
    let (answerer, mut answerer_incoming) = plain().await;
    let answerer_addr = answerer.local_addr();

    // The callee answers, so there is a response to record as well as a request.
    let responder = tokio::spawn(async move {
        let incoming = answerer_incoming.recv().await.expect("a request arrives");
        let response = ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .build();
        answerer
            .respond(&incoming.key, response)
            .await
            .expect("responds");
    });

    let mut responses = caller
        .send(
            request(&caller, &Method::Options, "capture-x12@sipx"),
            Target::udp(answerer_addr),
        )
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
        .await
        .expect("the exchange completes")
        .expect("a final response");
    assert_eq!(response.status.code(), 200);
    responder.await.ok();

    until(
        WRITING_BOUND,
        "the capture never got two records",
        async || path.exists() && read_capture(&path).packets.len() >= 2,
    )
    .await;

    let capture = read_capture(&path);
    assert_eq!(capture.linktype, 101, "LINKTYPE_RAW, per §13.1");
    assert!(capture.packets.len() >= 2, "{:?}", capture.packets.len());

    let out = &capture.packets[0];
    let back = &capture.packets[1];
    assert!(payload(out).starts_with("OPTIONS "), "{}", payload(out));
    assert!(
        payload(back).starts_with("SIP/2.0 200"),
        "{}",
        payload(back)
    );

    // Real ports in the synthetic UDP header, and the direction reversed between the two.
    assert_eq!(source_port(out), caller.local_addr().port());
    assert_eq!(destination_port(out), answerer_addr.port());
    assert_eq!(source_port(back), answerer_addr.port());
    assert_eq!(destination_port(back), caller.local_addr().port());

    // The comment is the authoritative statement of the transport (§13.1), because the packet's own
    // UDP header is synthetic for every transport.
    assert!(out.comment.contains("dir=out"), "{}", out.comment);
    assert!(out.comment.contains("transport=UDP"), "{}", out.comment);
    assert!(out.comment.contains("seq=1"), "{}", out.comment);
    assert!(back.comment.contains("dir=in"), "{}", back.comment);

    let counters = caller.counters();
    assert!(counters.capture.records >= 2, "{counters:?}");
    assert_eq!(
        counters.capture.dropped, 0,
        "nothing should have been dropped"
    );
    assert_eq!(counters.capture.errors, 0, "and nothing should have failed");
}

/// **Vector X16.** Off by default: no file, and the counters stay at zero.
#[tokio::test]
async fn capture_off_opens_nothing_and_counts_nothing() {
    let path = capture_path("never-written");
    let (endpoint, _incoming) = plain().await;
    let (sender, _sender_incoming) = plain().await;

    for _ in 0..3u32 {
        let _ = sender
            .send_directly(
                request(&sender, &Method::Options, "capture-off@sipx"),
                Target::udp(endpoint.local_addr()),
            )
            .await;
    }
    until(WRITING_BOUND, "no request ever arrived", async || {
        endpoint
            .counters()
            .transport(TransportKind::Udp)
            .requests_in
            > 0
    })
    .await;

    assert!(!path.exists(), "a capture nobody asked for created a file");
    let counters = endpoint.counters();
    assert_eq!(
        counters.capture,
        sipx_transport::CaptureCounts::default(),
        "capture is off, so every capture counter must be zero: {counters:?}"
    );
}

/// **Vectors X14 and X15 end to end.** A credential that goes over a real socket does not reach the
/// file. This is the assertion the whole redaction design exists for: a capture is written to be
/// attached to a bug report, which is to say handed to someone outside the trust boundary.
#[tokio::test]
async fn credentials_do_not_reach_the_file() {
    let path = capture_path("redacted");
    let (endpoint, _incoming) = recording(&path).await;

    // Sent as raw bytes from a plain socket, so the message reaches the capture by the receive path
    // exactly as a peer's would — before parsing, per §13.2.
    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    let message = "REGISTER sip:example.net SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:9;branch=z9hG4bKredact\r\n\
         To: <sip:alice@example.net>\r\n\
         From: <sip:alice@example.net>;tag=r\r\n\
         Call-ID: redaction@sipx\r\n\
         CSeq: 1 REGISTER\r\n\
         Authorization: Digest username=\"alice\", realm=\"example.net\", nonce=\"n0nce\", \
         response=\"0123456789abcdef\"\r\n\
         Contact: <sip:alice@127.0.0.1>;pn-provider=apns;pn-prid=PUSHSECRET\r\n\
         Content-Type: application/sdp\r\n\
         Content-Length: 96\r\n\r\n\
         v=0\r\n\
         a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj|2^20|1\r\n";
    peer.send_to(message.as_bytes(), endpoint.local_addr())
        .await
        .expect("sends");

    until(
        WRITING_BOUND,
        "the capture never got the message",
        async || path.exists() && !read_capture(&path).packets.is_empty(),
    )
    .await;

    let raw = std::fs::read(&path).expect("readable");
    let whole = String::from_utf8_lossy(&raw);

    // The three secrets, checked against the *entire file* rather than a parsed field: a redaction
    // that missed a copy elsewhere in the record would pass a narrower assertion.
    for secret in [
        "0123456789abcdef",
        "PUSHSECRET",
        "d0RmdmcmVCspeEc3QGZiNWpVLFJhQX1cfHAwJSoj",
    ] {
        assert!(
            !whole.contains(secret),
            "the capture file contains the secret {secret}"
        );
    }

    // And the diagnosable parts survived, or the capture would be useless for the bug report it
    // exists to be attached to.
    for kept in [
        "z9hG4bKredact",
        "redaction@sipx",
        "realm=\"example.net\"",
        "nonce=\"n0nce\"",
        "pn-provider=apns",
        "AES_CM_128_HMAC_SHA1_80",
    ] {
        assert!(
            whole.contains(kept),
            "redaction removed {kept}, which it needs"
        );
    }

    let recorded = read_capture(&path);
    let first = &recorded.packets[0];
    assert!(first.comment.contains("redacted=yes"), "{}", first.comment);
    // Length preserved in the body, so `Content-Length: 96` is still true of what was written.
    assert!(
        payload(first).contains("Content-Length: 96"),
        "{}",
        payload(first)
    );
    assert!(
        payload(first).contains("inline:REDACTED"),
        "{}",
        payload(first)
    );
}

/// A capture whose file cannot be created is refused at `bind`, not discovered later.
///
/// The alternative is an endpoint that starts, looks like it is recording, and writes nothing — the
/// §13.2 failure one level up, and the reason `Error::Capture` exists.
#[tokio::test]
async fn a_capture_that_cannot_be_opened_fails_the_bind() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.capture = Some(CaptureConfig::new(
        Path::new("/nonexistent-directory-for-sipx").join("capture.pcapng"),
    ));

    let error = bind(config).await.expect_err("binding must fail");
    let said = error.to_string();
    assert!(
        said.contains("capture") && said.contains("capture.pcapng"),
        "the error should name the capture and its path: {said}"
    );
}

/// A malformed datagram is captured malformed (§13.2) and counted as a parse failure, not a message.
///
/// The bytes a peer actually sent are the whole point: a capture that only holds what parsed cannot
/// show why something did not.
#[tokio::test]
async fn a_datagram_that_does_not_parse_is_still_captured() {
    let path = capture_path("malformed");
    let (endpoint, _incoming) = recording(&path).await;

    let peer = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("binds");
    peer.send_to(b"THIS IS NOT SIP\r\n\r\n", endpoint.local_addr())
        .await
        .expect("sends");

    until(
        WRITING_BOUND,
        "the malformed datagram was not captured",
        async || path.exists() && !read_capture(&path).packets.is_empty(),
    )
    .await;

    let capture = read_capture(&path);
    assert!(
        payload(&capture.packets[0]).starts_with("THIS IS NOT SIP"),
        "{}",
        payload(&capture.packets[0])
    );
    until(
        WRITING_BOUND,
        "the parse failure was not counted",
        async || {
            endpoint
                .counters()
                .transport(TransportKind::Udp)
                .parse_failures
                > 0
        },
    )
    .await;
    let udp = endpoint.counters().transport(TransportKind::Udp);
    assert_eq!(udp.requests_in, 0, "a malformed datagram is not a request");
}
