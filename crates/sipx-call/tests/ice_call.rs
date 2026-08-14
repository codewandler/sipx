//! ICE selected through the call-layer policy (`M-27`, `docs/specs/ice.md` §13.4).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// `caller` and `callee` differ by two letters and are the names the RFCs, the industry and
// everyone reading this test already use. Renaming them to satisfy a similarity heuristic
// would make the test harder to read, not easier. Same allow, same reason, as `call.rs`.
#![allow(clippy::similar_names)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use hmac::{Hmac, Mac as _};
use sha2::{Digest as _, Sha256};
use sipx_call::{
    CallConfig, DialOptions, Dispatched, Dispatcher, IcePolicy, MediaAddress, MediaPolicy,
    TurnPolicy, answer_with_policy, dial,
};
use sipx_media::{IcePath, Interrupt, RtcpQualityHook};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("loopback")
}

/// How long a test here waits for audio it played to arrive before calling it lost.
///
/// A bound on **failure**, not a window to measure in — the same constant and the same reason as
/// `call.rs` and `secure_media.rs` (`X-28`). This file first carried three different ad-hoc values.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// How much audio each test requires to have arrived, out of a deliberately longer clip.
///
/// Every test here plays more than this and asserts on a prefix, which is not a convenience: RTP
/// is UDP, and a test that plays exactly what it requires is asserting **lossless delivery** on top
/// of everything else it is about. `an_unavailable_stun_server_degrades_to_host_candidates` played
/// exactly 1600 samples and failed with 1280 whenever the four runtimes in this binary contended —
/// and raising the timeout could not fix it, because a dropped packet does not arrive later. Two
/// of these tests additionally need the margin for a real reason: audio sent before nomination goes
/// to a default destination that is deliberately silent, so the early packets are *expected* to be
/// lost.
const REQUIRED: usize = 1_600;

fn ice() -> MediaPolicy {
    MediaPolicy::default().with_ice(IcePolicy::Host)
}

/// The `ice-ufrag` and `ice-pwd` a description states, as the pair RFC 8839 §4.4.1.1.1 compares.
///
/// Both, and always together: the rule is about the two of them changing, so a helper that
/// returned one would invite a test that asserts half of it.
fn credentials_in(description: &str) -> (String, String) {
    let value = |name: &str| {
        description
            .lines()
            .find_map(|line| line.trim().strip_prefix(name).map(str::to_owned))
            .unwrap_or_default()
    };
    (value("a=ice-ufrag:"), value("a=ice-pwd:"))
}

/// Serve the callee's in-dialog traffic until a re-offer has been answered, and return its body.
///
/// The loop is the point. The first request to arrive after `answer_with_policy` returns is the
/// **ACK** for the 200 it just sent, not the re-INVITE — a single `recv()` here consumed that ACK,
/// reported it as the re-offer, and then sat waiting while the real re-INVITE went unanswered and
/// the offering side blocked on a final response that nobody was going to send.
async fn serve_until_reoffer(
    callee: &mut sipx_call::Call,
    incoming: &mut tokio::sync::mpsc::Receiver<sipx_transport::Incoming>,
) -> (String, sipx_call::Result<bool>) {
    loop {
        let request = incoming.recv().await.expect("an in-dialog request");
        let is_reoffer = request.request.method == sipx_sip::Method::Invite;
        let body = String::from_utf8_lossy(request.request.body()).into_owned();
        let answered = callee.handle(&request).await;
        if is_reoffer {
            return (body, answered);
        }
    }
}

async fn dead_end() -> (UdpSocket, std::net::SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("dead end binds");
    let address = socket.local_addr().expect("dead end address");
    (socket, address)
}

/// Make the path a NAT-shaped one without depending on host firewall or namespace privileges.
/// The high-priority host/default destination becomes a bound socket nobody reads, while the
/// address the endpoint really bound is retained only as a lower-priority reflexive candidate.
fn behind_nat(message: &[u8], dead: std::net::SocketAddr) -> Vec<u8> {
    let text = String::from_utf8_lossy(message);
    let (headers, body) = text.split_once("\r\n\r\n").expect("SIP message has a body");
    let host = body
        .lines()
        .find_map(|line| {
            let value = line.strip_prefix("a=candidate:")?;
            let fields: Vec<&str> = value.split_whitespace().collect();
            (fields.get(1) == Some(&"1") && fields.get(7) == Some(&"host")).then(|| {
                format!(
                    "{}:{}",
                    fields.get(4).expect("candidate address"),
                    fields.get(5).expect("candidate port")
                )
                .parse::<std::net::SocketAddr>()
                .expect("candidate socket address")
            })
        })
        .expect("an RTP host candidate");

    let mut rewritten = Vec::new();
    for line in body.lines() {
        if line.starts_with("c=IN IP") {
            rewritten.push(format!("c=IN IP4 {}", dead.ip()));
        } else if let Some(rest) = line.strip_prefix("m=audio ") {
            let (_, tail) = rest.split_once(' ').expect("media line fields");
            rewritten.push(format!("m=audio {} {tail}", dead.port()));
        } else if line.starts_with("a=candidate:") {
            // Component 2 is omitted: both agents reduce the stream to RTP, and the test remains
            // about the one path that carries the asserted audio.
        } else if !line.is_empty() {
            rewritten.push(line.to_owned());
        }
    }
    rewritten.push(format!(
        "a=candidate:1 1 UDP 2130706431 {} {} typ host",
        dead.ip(),
        dead.port()
    ));
    rewritten.push(format!(
        "a=candidate:9 1 UDP 1694498815 {} {} typ srflx raddr {} rport {}",
        host.ip(),
        host.port(),
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

const STUN_COOKIE: u32 = 0x2112_a442;
const TURN_ALLOCATE: u16 = 0x0003;
const TURN_SEND: u16 = 0x0006;
const TURN_DATA: u16 = 0x0007;
const TURN_CREATE_PERMISSION: u16 = 0x0008;
const TURN_USERNAME: u16 = 0x0006;
const TURN_ERROR_CODE: u16 = 0x0009;
const TURN_LIFETIME: u16 = 0x000d;
const TURN_XOR_PEER: u16 = 0x0012;
const TURN_DATA_ATTRIBUTE: u16 = 0x0013;
const TURN_REALM: u16 = 0x0014;
const TURN_NONCE: u16 = 0x0015;
const TURN_XOR_RELAYED: u16 = 0x0016;
const TURN_MESSAGE_INTEGRITY_SHA256: u16 = 0x001c;
const TURN_PASSWORD_ALGORITHM: u16 = 0x001d;
const TURN_XOR_MAPPED: u16 = 0x0020;
const TURN_PASSWORD_ALGORITHMS: u16 = 0x8002;
const TURN_SHA256_NONCE: &str = "obMatJos2AAABcall-layer";
const TURN_REALM_VALUE: &str = "example.com";
const TURN_CREDENTIAL_MATERIAL: &[u8] = b"1000:example.com:relay-password";

type HmacSha256 = Hmac<Sha256>;

/// Rewrite only signalling facts: host/default destinations go silent, while component one's
/// relayed candidate remains reachable. Both roles receive this treatment, so no host-to-host
/// pair can succeed. Traffic addressed to the retained candidate enters an actual allocation
/// socket and returns to its client as a TURN Data indication.
fn behind_relay(message: &[u8], dead: SocketAddr) -> Vec<u8> {
    let text = String::from_utf8_lossy(message);
    let (headers, body) = text.split_once("\r\n\r\n").expect("SIP message has a body");
    let mut rewritten = Vec::new();
    let mut relay_seen = false;
    for line in body.lines() {
        if line.starts_with("c=IN IP") {
            rewritten.push(format!("c=IN IP4 {}", dead.ip()));
        } else if let Some(rest) = line.strip_prefix("m=audio ") {
            let (_, tail) = rest.split_once(' ').expect("media line fields");
            rewritten.push(format!("m=audio {} {tail}", dead.port()));
        } else if let Some(value) = line.strip_prefix("a=candidate:") {
            let fields: Vec<&str> = value.split_whitespace().collect();
            if fields.get(1) == Some(&"1") && fields.get(7) == Some(&"relay") {
                rewritten.push(line.to_owned());
                relay_seen = true;
            }
        } else if !line.is_empty() {
            rewritten.push(line.to_owned());
        }
    }
    assert!(
        relay_seen,
        "TURN gathering did not produce a relayed candidate: {body}"
    );
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

fn turn_message_type(class: u16, method: u16) -> u16 {
    (method & 0x000f)
        | ((method & 0x0070) << 1)
        | ((method & 0x0f80) << 2)
        | ((class & 0x1) << 4)
        | ((class & 0x2) << 7)
}

fn turn_class_and_method(raw: u16) -> (u16, u16) {
    let class = ((raw & 0x0100) >> 7) | ((raw & 0x0010) >> 4);
    let method = (raw & 0x000f) | ((raw & 0x00e0) >> 1) | ((raw & 0x3e00) >> 2);
    (class, method)
}

fn turn_transaction(datagram: &[u8]) -> Option<[u8; 12]> {
    datagram.get(8..20)?.try_into().ok()
}

fn turn_kind(datagram: &[u8]) -> Option<(u16, u16)> {
    let raw = u16::from_be_bytes(datagram.get(..2)?.try_into().ok()?);
    Some(turn_class_and_method(raw))
}

fn turn_attribute(datagram: &[u8], expected: u16) -> Option<&[u8]> {
    let stated = usize::from(u16::from_be_bytes(datagram.get(2..4)?.try_into().ok()?));
    let body = datagram.get(20..20usize.checked_add(stated)?)?;
    let mut offset = 0usize;
    while offset < body.len() {
        let kind = u16::from_be_bytes(body.get(offset..offset.checked_add(2)?)?.try_into().ok()?);
        let length_at = offset.checked_add(2)?;
        let length = usize::from(u16::from_be_bytes(
            body.get(length_at..length_at.checked_add(2)?)?
                .try_into()
                .ok()?,
        ));
        let start = offset.checked_add(4)?;
        let value = body.get(start..start.checked_add(length)?)?;
        if kind == expected {
            return Some(value);
        }
        offset = start.checked_add(length.checked_add(3)? & !3)?;
    }
    None
}

fn turn_header(class: u16, method: u16, transaction: [u8; 12]) -> Vec<u8> {
    let mut message = Vec::with_capacity(160);
    message.extend_from_slice(&turn_message_type(class, method).to_be_bytes());
    message.extend_from_slice(&0u16.to_be_bytes());
    message.extend_from_slice(&STUN_COOKIE.to_be_bytes());
    message.extend_from_slice(&transaction);
    message
}

fn turn_push_attribute(message: &mut Vec<u8>, kind: u16, value: &[u8]) {
    message.extend_from_slice(&kind.to_be_bytes());
    message.extend_from_slice(
        &u16::try_from(value.len())
            .expect("fixture attribute fits")
            .to_be_bytes(),
    );
    message.extend_from_slice(value);
    message.extend(std::iter::repeat_n(0, (4 - value.len() % 4) % 4));
}

fn turn_set_length(message: &mut [u8], extra: usize) {
    let body = message.len().checked_sub(20).expect("fixture header");
    let length = u16::try_from(body.checked_add(extra).expect("fixture length")).expect("fits");
    message[2..4].copy_from_slice(&length.to_be_bytes());
}

fn turn_xor_address(address: SocketAddr, transaction: [u8; 12]) -> Vec<u8> {
    let port = address.port() ^ u16::try_from(STUN_COOKIE >> 16).expect("cookie prefix");
    match address {
        SocketAddr::V4(address) => {
            let mut value = vec![0, 0x01];
            value.extend_from_slice(&port.to_be_bytes());
            value.extend_from_slice(&(u32::from(*address.ip()) ^ STUN_COOKIE).to_be_bytes());
            value
        }
        SocketAddr::V6(address) => {
            let mut value = vec![0, 0x02];
            value.extend_from_slice(&port.to_be_bytes());
            let mask: [u8; 16] = [
                STUN_COOKIE.to_be_bytes()[0],
                STUN_COOKIE.to_be_bytes()[1],
                STUN_COOKIE.to_be_bytes()[2],
                STUN_COOKIE.to_be_bytes()[3],
                transaction[0],
                transaction[1],
                transaction[2],
                transaction[3],
                transaction[4],
                transaction[5],
                transaction[6],
                transaction[7],
                transaction[8],
                transaction[9],
                transaction[10],
                transaction[11],
            ];
            value.extend(
                address
                    .ip()
                    .octets()
                    .iter()
                    .zip(mask)
                    .map(|(byte, mask)| byte ^ mask),
            );
            value
        }
    }
}

fn turn_decode_xor_address(value: &[u8], transaction: [u8; 12]) -> Option<SocketAddr> {
    let port = u16::from_be_bytes(value.get(2..4)?.try_into().ok()?)
        ^ u16::try_from(STUN_COOKIE >> 16).ok()?;
    match *value.get(1)? {
        0x01 => {
            let address = u32::from_be_bytes(value.get(4..8)?.try_into().ok()?) ^ STUN_COOKIE;
            Some(SocketAddr::new(
                std::net::Ipv4Addr::from(address).into(),
                port,
            ))
        }
        0x02 => {
            let encoded: [u8; 16] = value.get(4..20)?.try_into().ok()?;
            let cookie = STUN_COOKIE.to_be_bytes();
            let mut mask = [0u8; 16];
            mask[..4].copy_from_slice(&cookie);
            mask[4..].copy_from_slice(&transaction);
            let mut address = [0u8; 16];
            for (decoded, (byte, mask)) in address.iter_mut().zip(encoded.into_iter().zip(mask)) {
                *decoded = byte ^ mask;
            }
            Some(SocketAddr::new(address.into(), port))
        }
        _ => None,
    }
}

fn turn_finish_sha256(message: &mut Vec<u8>) {
    turn_set_length(message, 36);
    let key = Sha256::digest(TURN_CREDENTIAL_MATERIAL);
    let mut mac = <HmacSha256 as hmac::Mac>::new_from_slice(&key).expect("HMAC accepts SHA-256");
    mac.update(message);
    let tag = mac.finalize().into_bytes();
    turn_push_attribute(message, TURN_MESSAGE_INTEGRITY_SHA256, &tag);
}

fn turn_challenge(transaction: [u8; 12], code: u16) -> Vec<u8> {
    let mut message = turn_header(3, TURN_ALLOCATE, transaction);
    turn_push_attribute(
        &mut message,
        TURN_ERROR_CODE,
        &[
            0,
            0,
            u8::try_from(code / 100).expect("class"),
            u8::try_from(code % 100).expect("code"),
        ],
    );
    turn_push_attribute(&mut message, TURN_REALM, TURN_REALM_VALUE.as_bytes());
    turn_push_attribute(&mut message, TURN_NONCE, TURN_SHA256_NONCE.as_bytes());
    // SHA-256 first, MD5 second: RFC 8489's PASSWORD-ALGORITHMS entries have empty parameters.
    turn_push_attribute(
        &mut message,
        TURN_PASSWORD_ALGORITHMS,
        &[0, 2, 0, 0, 0, 1, 0, 0],
    );
    turn_set_length(&mut message, 0);
    message
}

fn turn_allocation_success(
    transaction: [u8; 12],
    relayed: SocketAddr,
    mapped: SocketAddr,
) -> Vec<u8> {
    let mut message = turn_header(2, TURN_ALLOCATE, transaction);
    turn_push_attribute(
        &mut message,
        TURN_XOR_RELAYED,
        &turn_xor_address(relayed, transaction),
    );
    turn_push_attribute(
        &mut message,
        TURN_XOR_MAPPED,
        &turn_xor_address(mapped, transaction),
    );
    turn_push_attribute(&mut message, TURN_LIFETIME, &600u32.to_be_bytes());
    turn_finish_sha256(&mut message);
    message
}

fn turn_permission_success(transaction: [u8; 12]) -> Vec<u8> {
    let mut message = turn_header(2, TURN_CREATE_PERMISSION, transaction);
    turn_finish_sha256(&mut message);
    message
}

fn turn_data_indication(peer: SocketAddr, data: &[u8]) -> Vec<u8> {
    let transaction = [0xa5; 12];
    let mut message = turn_header(1, TURN_DATA, transaction);
    turn_push_attribute(
        &mut message,
        TURN_XOR_PEER,
        &turn_xor_address(peer, transaction),
    );
    turn_push_attribute(&mut message, TURN_DATA_ATTRIBUTE, data);
    turn_set_length(&mut message, 0);
    message
}

struct RelayedDatagram {
    allocation: usize,
    from: SocketAddr,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct TurnCounters {
    send_indications: AtomicUsize,
    data_indications: AtomicUsize,
}

struct TurnFixture {
    address: SocketAddr,
    counters: Arc<TurnCounters>,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl TurnFixture {
    async fn shutdown(self) {
        let _ = self.shutdown.send(());
        self.task.await.expect("TURN fixture joins");
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the bounded fixture loop is the complete Allocate, permission, Send and Data peer"
)]
async fn serve_turn_fixture(
    control: UdpSocket,
    relays: Vec<Arc<UdpSocket>>,
    counters: Arc<TurnCounters>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let (relayed, mut received) = mpsc::channel::<RelayedDatagram>(64);
    let mut readers = JoinSet::new();
    for (allocation, relay) in relays.iter().cloned().enumerate() {
        let relayed = relayed.clone();
        readers.spawn(async move {
            let mut datagram = vec![0u8; 65_535];
            loop {
                let Ok((length, from)) = relay.recv_from(&mut datagram).await else {
                    break;
                };
                if relayed
                    .send(RelayedDatagram {
                        allocation,
                        from,
                        bytes: datagram[..length].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
    }
    drop(relayed);

    let mut clients = vec![None; relays.len()];
    let mut control_datagram = vec![0u8; 65_535];
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            incoming = control.recv_from(&mut control_datagram) => {
                let (length, from) = incoming.expect("TURN control socket receives");
                let datagram = &control_datagram[..length];
                let Some((class, method)) = turn_kind(datagram) else { continue };
                let Some(transaction) = turn_transaction(datagram) else { continue };
                match (class, method) {
                    (0, TURN_ALLOCATE) => {
                        let allocation = clients
                            .iter()
                            .position(|client| *client == Some(from))
                            .or_else(|| clients.iter().position(Option::is_none))
                            .expect("the fixture has one allocation socket per component");
                        clients[allocation] = Some(from);
                        let response = if turn_attribute(datagram, TURN_MESSAGE_INTEGRITY_SHA256).is_some() {
                            assert!(turn_attribute(datagram, TURN_USERNAME).is_some());
                            assert!(turn_attribute(datagram, TURN_PASSWORD_ALGORITHMS).is_some());
                            assert!(turn_attribute(datagram, TURN_PASSWORD_ALGORITHM).is_some());
                            turn_allocation_success(
                                transaction,
                                relays[allocation].local_addr().expect("relay address"),
                                from,
                            )
                        } else {
                            turn_challenge(transaction, 401)
                        };
                        control.send_to(&response, from).await.expect("TURN Allocate response sends");
                    }
                    (0, TURN_CREATE_PERMISSION) => {
                        assert!(turn_attribute(datagram, TURN_MESSAGE_INTEGRITY_SHA256).is_some());
                        let _peer = turn_attribute(datagram, TURN_XOR_PEER)
                            .and_then(|value| turn_decode_xor_address(value, transaction))
                            .expect("permission peer");
                        control
                            .send_to(&turn_permission_success(transaction), from)
                            .await
                            .expect("TURN permission response sends");
                    }
                    (1, TURN_SEND) => {
                        let Some(allocation) = clients.iter().position(|client| *client == Some(from)) else {
                            continue;
                        };
                        let peer = turn_attribute(datagram, TURN_XOR_PEER)
                            .and_then(|value| turn_decode_xor_address(value, transaction))
                            .expect("Send peer");
                        let data = turn_attribute(datagram, TURN_DATA_ATTRIBUTE).expect("Send data");
                        if data.first().is_some_and(|first| first & 0xc0 == 0x80) {
                            counters.send_indications.fetch_add(1, Ordering::Relaxed);
                        }
                        relays[allocation]
                            .send_to(data, peer)
                            .await
                            .expect("relayed payload sends");
                    }
                    _ => {}
                }
            }
            Some(datagram) = received.recv() => {
                if let Some(client) = clients.get(datagram.allocation).copied().flatten() {
                    if datagram.bytes.first().is_some_and(|first| first & 0xc0 == 0x80) {
                        counters.data_indications.fetch_add(1, Ordering::Relaxed);
                    }
                    control
                        .send_to(&turn_data_indication(datagram.from, &datagram.bytes), client)
                        .await
                        .expect("TURN Data indication sends");
                }
            }
        }
    }
    readers.abort_all();
    while readers.join_next().await.is_some() {}
}

async fn turn_fixture_pair() -> (TurnFixture, TurnFixture) {
    let caller_control = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("TURN control binds");
    let callee_control = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("TURN control binds");
    let caller_relays = vec![
        Arc::new(UdpSocket::bind("127.0.0.1:0").await.expect("relay binds")),
        Arc::new(UdpSocket::bind("127.0.0.1:0").await.expect("relay binds")),
    ];
    let callee_relays = vec![
        Arc::new(UdpSocket::bind("127.0.0.1:0").await.expect("relay binds")),
        Arc::new(UdpSocket::bind("127.0.0.1:0").await.expect("relay binds")),
    ];
    let caller_address = caller_control.local_addr().expect("TURN address");
    let callee_address = callee_control.local_addr().expect("TURN address");
    let caller_counters = Arc::new(TurnCounters::default());
    let callee_counters = Arc::new(TurnCounters::default());
    let (caller_shutdown, caller_cancelled) = oneshot::channel();
    let (callee_shutdown, callee_cancelled) = oneshot::channel();
    let caller_task = tokio::spawn(serve_turn_fixture(
        caller_control,
        caller_relays,
        Arc::clone(&caller_counters),
        caller_cancelled,
    ));
    let callee_task = tokio::spawn(serve_turn_fixture(
        callee_control,
        callee_relays,
        Arc::clone(&callee_counters),
        callee_cancelled,
    ));
    (
        TurnFixture {
            address: caller_address,
            counters: caller_counters,
            shutdown: caller_shutdown,
            task: caller_task,
        },
        TurnFixture {
            address: callee_address,
            counters: callee_counters,
            shutdown: callee_shutdown,
            task: callee_task,
        },
    )
}

async fn refusing_turn_fixture() -> TurnFixture {
    let socket = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("TURN refusal binds");
    let address = socket.local_addr().expect("TURN address");
    let counters = Arc::new(TurnCounters::default());
    let (shutdown, mut cancelled) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut datagram = vec![0u8; 65_535];
        loop {
            tokio::select! {
                _ = &mut cancelled => break,
                incoming = socket.recv_from(&mut datagram) => {
                    let (length, from) = incoming.expect("refusing TURN receives");
                    let Some(transaction) = turn_transaction(&datagram[..length]) else { continue };
                    socket
                        .send_to(&turn_challenge(transaction, 486), from)
                        .await
                        .expect("TURN refusal sends");
                }
            }
        }
    });
    TurnFixture {
        address,
        counters,
        shutdown,
        task,
    }
}

/// The first failing-first witness: before M-27 there is no call-level ICE policy, the INVITE
/// carries no candidate, and neither side can hand the negotiated description to the media port.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_selected_for_ice_offers_answers_and_carries_audio() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let offer = String::from_utf8_lossy(incoming.request.body());
        assert!(offer.contains("a=ice-ufrag:"), "offer:\n{offer}");
        assert!(offer.contains("a=candidate:"), "offer:\n{offer}");
        answer_with_policy(&callee_endpoint, &incoming, loopback(), ice())
            .await
            .expect("answers with ICE")
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_address),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()).with_media_policy(ice()),
    )
    .await
    .expect("the ICE call connects");
    let callee = answering.await.expect("answer task");

    // Keep producing frames while checks converge. The assertion is on what arrives, not on a
    // sleep guessed to be long enough for nomination.
    let tone = vec![8_000i16; 16_000];
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(heard.len(), REQUIRED, "ICE carried the required clip");
}

/// `M-23`'s acceptance test: a re-offer whose `ice-ufrag` **and** `ice-pwd` both changed starts a
/// new ICE session (RFC 8839 §4.4.1.1.1), and the audio does not stop while it does.
///
/// The last clause is the one worth having a test for. A restart that goes silent is worse than no
/// restart, so the recording spans the whole exchange: it starts before the re-INVITE goes out and
/// is still required to complete afterwards. `Agent::restart` deliberately keeps the selected pair
/// for exactly this, and nothing else here would notice if it stopped.
#[tokio::test(flavor = "multi_thread")]
async fn a_reoffer_that_changes_both_ufrag_and_pwd_restarts_ice_without_dropping_audio() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let invite = String::from_utf8_lossy(incoming.request.body()).into_owned();
        let call = answer_with_policy(&callee_endpoint, &incoming, loopback(), ice())
            .await
            .expect("answers with ICE");
        (call, callee_incoming, invite)
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_address),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()).with_media_policy(ice()),
    )
    .await
    .expect("the ICE call connects");
    let (quality_seen, mut quality_events) = tokio::sync::watch::channel(0u64);
    caller.set_rtcp_quality_hook(Some(RtcpQualityHook::new(move |_| {
        quality_seen.send_modify(|count| *count = count.saturating_add(1));
    })));
    let (mut callee, mut callee_incoming, invite) = answering.await.expect("answer task");
    let before = credentials_in(&invite);

    // Queued on the session's own playback worker rather than awaited here, so audio keeps going
    // out across the signalling below instead of being played before it and again after it.
    let _playing = caller
        .media()
        .start_playback(vec![5_000i16; 40_000], Interrupt::Never);

    // The callee's half of the re-INVITE exchange, driven inline. `handle` is what applies
    // RFC 8839 §4.4 on the answering side, and running it concurrently with `restart_ice` is what
    // makes the exchange complete at all: `restart_ice` returns only once the 200 is back.
    let serving = serve_until_reoffer(&mut callee, &mut callee_incoming);
    let ((reoffer, answered), restarted) = tokio::join!(serving, caller.restart_ice());
    answered.expect("the callee answers the restart");
    restarted.expect("the restart is accepted");
    assert!(
        caller.rtcp_quality_hook().is_some(),
        "an ICE restart must retain the logical call's RTCP quality hook"
    );
    // Ignore any report that raced the restart itself, then require one produced afterwards. This
    // proves the hook still receives through the restarted ICE path rather than only remaining in
    // the public slot.
    quality_events.borrow_and_update();
    tokio::time::timeout(DELIVERY_BOUND, quality_events.changed())
        .await
        .expect("a post-restart RTCP report is a bounded wait")
        .expect("the quality callback remains owned by the call");

    let after = credentials_in(&reoffer);
    assert_ne!(before.0, after.0, "ice-ufrag changed:\n{reoffer}");
    assert_ne!(before.1, after.1, "ice-pwd changed:\n{reoffer}");
    assert!(
        reoffer.contains("a=candidate:"),
        "a restart re-offers its candidates (RFC 8839 §4.4):\n{reoffer}"
    );
    assert!(
        !reoffer.contains("a=remote-candidates:"),
        "a restart cannot claim the previous generation's selected pair: {reoffer}"
    );

    let heard = callee
        .media()
        .record_at_least(REQUIRED, DELIVERY_BOUND)
        .await;
    assert_eq!(heard.len(), REQUIRED, "audio crossed the restart");
}

/// A stream that is doing ICE restates its half in every later description, and an offer that
/// changes only *one* credential is not a restart (RFC 8839 §4.4, §4.4.1.1.1).
///
/// Hold is the case that makes this worth asserting. It is the commonest re-offer there is, §6
/// makes a missing `candidate` mean the peer has stopped doing ICE, and RFC 8839 §4.4.1.1.1 makes
/// `c=0.0.0.0` imply a restart — so a hold that dropped its ICE attributes or spelled itself with
/// a null connection address would either silence ICE or restart it on every mute.
#[tokio::test(flavor = "multi_thread")]
async fn holding_an_ice_call_re_signals_ice_and_does_not_restart_it() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let invite = String::from_utf8_lossy(incoming.request.body()).into_owned();
        let call = answer_with_policy(&callee_endpoint, &incoming, loopback(), ice())
            .await
            .expect("answers with ICE");
        (call, callee_incoming, invite)
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_address),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()).with_media_policy(ice()),
    )
    .await
    .expect("the ICE call connects");
    let (mut callee, mut callee_incoming, invite) = answering.await.expect("answer task");
    let before = credentials_in(&invite);

    // Nomination, observed through media on the selected pair rather than through a sleep. The
    // subsequent offer is eligible for RFC 8839 §5.2 only after this generation is Completed.
    let tone = vec![8_000i16; 16_000];
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(heard.len(), REQUIRED, "ICE completed before the re-offer");

    let serving = serve_until_reoffer(&mut callee, &mut callee_incoming);
    let ((held, answered), reinvited) =
        tokio::join!(serving, caller.reinvite(sipx_sdp::Direction::SendOnly));
    answered.expect("the callee answers the hold");
    reinvited.expect("the hold is accepted");

    assert!(
        held.contains("a=sendonly"),
        "hold is a direction (RFC 3264):\n{held}"
    );
    assert!(
        !held.contains("c=IN IP4 0.0.0.0"),
        "c=0.0.0.0 would imply a restart (RFC 8839 §4.4.1.1.1):\n{held}"
    );
    assert!(
        held.contains("a=candidate:"),
        "a stream doing ICE re-signals its candidates (RFC 8839 §6):\n{held}"
    );
    assert_eq!(
        credentials_in(&held),
        before,
        "hold is not a restart, so neither credential moves:\n{held}"
    );
    let held_description = sipx_sdp::parse(&held).expect("the held offer is SDP");
    let remote_candidates = held_description.media[0].ice_remote_candidates();
    assert_eq!(
        remote_candidates.len(),
        1,
        "a completed controlling stream names its selected RTP peer: {held}"
    );
    assert_eq!(
        remote_candidates[0].component,
        sipx_sdp::ice::ComponentId::RTP
    );
}

/// M-27's acceptance test. Both descriptions make their default/high-priority host path a silent
/// socket. The only usable addresses are the lower-priority reflexive candidates, so audio proves
/// a nominated pair replaced the defaults rather than symmetric RTP rescuing the call.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();
    let proxy = UdpSocket::bind("127.0.0.1:0").await.expect("proxy binds");
    let proxy_address = proxy.local_addr().expect("proxy address");
    let (_caller_dead_socket, caller_dead) = dead_end().await;
    let (_callee_dead_socket, callee_dead) = dead_end().await;

    let forwarding = tokio::spawn(async move {
        let mut datagram = vec![0u8; 65_535];
        let (length, caller_address) = proxy.recv_from(&mut datagram).await.expect("INVITE");
        let offer = behind_nat(&datagram[..length], caller_dead);
        proxy
            .send_to(&offer, callee_address)
            .await
            .expect("forwards INVITE");

        let (length, _) = proxy.recv_from(&mut datagram).await.expect("200 answer");
        let answer = behind_nat(&datagram[..length], callee_dead);
        proxy
            .send_to(&answer, caller_address)
            .await
            .expect("forwards answer");
    });

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("proxied INVITE");
        let offer = String::from_utf8_lossy(incoming.request.body());
        assert!(offer.contains("typ srflx"), "rewritten offer:\n{offer}");
        answer_with_policy(&callee_endpoint, &incoming, loopback(), ice())
            .await
            .expect("answers with ICE")
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let caller = tokio::time::timeout(
        Duration::from_secs(12),
        dial(
            &caller_endpoint,
            Target::udp(proxy_address),
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback()).with_media_policy(ice()),
        ),
    )
    .await
    .expect("ICE converges within the test bound")
    .expect("caller connects");
    let callee = answering.await.expect("answer task");
    forwarding.await.expect("proxy task");

    // Packets sent before nomination deliberately disappear into the silent default. Continue
    // causally until the receiver has enough rather than sleeping and assuming selection happened.
    let tone = vec![9_000i16; 24_000];
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(heard.len(), REQUIRED, "the nominated pair carried audio");
}

/// Selecting no ICE is the compatibility contract: no ICE vocabulary is added to the SDP and
/// the existing symmetric-RTP call path remains the one that starts.
#[tokio::test(flavor = "multi_thread")]
async fn the_default_call_path_puts_no_ice_on_the_wire() {
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let offer = String::from_utf8_lossy(incoming.request.body());
        assert!(!offer.contains("a=ice-"), "default offer:\n{offer}");
        assert!(!offer.contains("a=candidate:"), "default offer:\n{offer}");
        sipx_call::answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers without ICE")
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_address),
        &to,
        &DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the default call connects");
    let callee = answering.await.expect("answer task");

    let tone = vec![7_000i16; 8_000];
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(heard.len(), REQUIRED, "symmetric RTP still carries audio");
}

/// A configured STUN server may be unavailable. Gathering is bounded and keeps the host
/// candidates it already has, so the call proceeds instead of turning infrastructure loss into
/// signalling failure.
#[tokio::test(flavor = "multi_thread")]
async fn an_unavailable_stun_server_degrades_to_host_candidates() {
    let silent_stun = UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("silent STUN socket binds");
    let stun_address = silent_stun.local_addr().expect("STUN address");
    let (callee_endpoint, mut callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let offer = String::from_utf8_lossy(incoming.request.body());
        assert!(offer.contains("typ host"), "host fallback offer:\n{offer}");
        assert!(!offer.contains("typ srflx"), "silent STUN offer:\n{offer}");
        answer_with_policy(&callee_endpoint, &incoming, loopback(), ice())
            .await
            .expect("answers host ICE")
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let caller_policy = MediaPolicy::default().with_ice(IcePolicy::Stun(stun_address));
    let caller = tokio::time::timeout(
        Duration::from_secs(8),
        dial(
            &caller_endpoint,
            Target::udp(callee_address),
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback())
                .with_media_policy(caller_policy),
        ),
    )
    .await
    .expect("bounded gathering finishes")
    .expect("host candidates connect the call");
    let callee = answering.await.expect("answer task");

    let tone = vec![6_000i16; 8_000];
    let (_played, heard) = tokio::join!(
        caller.media().play(&tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(heard.len(), REQUIRED, "host ICE still carries audio");
}

fn turn_media_counts(caller: &TurnFixture, callee: &TurnFixture) -> (usize, usize) {
    (
        caller.counters.send_indications.load(Ordering::Relaxed)
            + callee.counters.send_indications.load(Ordering::Relaxed),
        caller.counters.data_indications.load(Ordering::Relaxed)
            + callee.counters.data_indications.load(Ordering::Relaxed),
    )
}

async fn assert_bidirectional_relay_audio(
    caller: &sipx_call::Call,
    callee: &sipx_call::Call,
    caller_turn: &TurnFixture,
    callee_turn: &TurnFixture,
) {
    let (sends_before, data_before) = turn_media_counts(caller_turn, callee_turn);
    let caller_tone = vec![9_000i16; 24_000];
    let (_played, heard_by_callee) = tokio::join!(
        caller.media().play(&caller_tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(
        heard_by_callee.len(),
        REQUIRED,
        "caller audio crossed the nominated relay"
    );
    let (sends_after_caller, data_after_caller) = turn_media_counts(caller_turn, callee_turn);
    assert!(
        sends_after_caller + data_after_caller > sends_before + data_before,
        "caller RTP never entered a TURN Send or Data indication"
    );

    let callee_tone = vec![-7_000i16; 24_000];
    let (_played, heard_by_caller) = tokio::join!(
        callee.media().play(&callee_tone, 160),
        caller.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(
        heard_by_caller.len(),
        REQUIRED,
        "callee audio crossed the nominated relay"
    );
    let (sends_after, data_after) = turn_media_counts(caller_turn, callee_turn);
    assert!(
        sends_after + data_after > sends_after_caller + data_after_caller,
        "callee RTP never entered a TURN Send or Data indication"
    );
    assert!(
        sends_after > sends_before,
        "selected relay never carried a Send indication"
    );
    assert!(
        data_after > data_before,
        "selected relay never carried a Data indication"
    );
}

/// M-24/A-41's call contract: the selected pair, not merely the gathered SDP, is relayed. Both
/// descriptions retain relay candidates but point their host candidates at silent sockets. Each
/// retained relay address is an actual fixture allocation socket, so bidirectional audio and the
/// fixture counters witness the Send/Data path in both directions.
#[tokio::test(flavor = "multi_thread")]
async fn a_call_selects_a_sha256_authenticated_relay_and_carries_audio_both_ways() {
    let (caller_turn, callee_turn) = turn_fixture_pair().await;
    let caller_turn_policy =
        TurnPolicy::new(caller_turn.address, "1000", "relay-password").expect("caller TURN policy");
    let callee_turn_policy =
        TurnPolicy::new(callee_turn.address, "1000", "relay-password").expect("callee TURN policy");

    let (callee_endpoint, callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();
    let proxy = UdpSocket::bind("127.0.0.1:0").await.expect("proxy binds");
    let proxy_address = proxy.local_addr().expect("proxy address");
    let (_caller_dead_socket, caller_dead) = dead_end().await;
    let (_callee_dead_socket, callee_dead) = dead_end().await;

    let forwarding = tokio::spawn(async move {
        let mut datagram = vec![0u8; 65_535];
        let (length, caller_address) = proxy.recv_from(&mut datagram).await.expect("INVITE");
        let offer = behind_relay(&datagram[..length], caller_dead);
        proxy
            .send_to(&offer, callee_address)
            .await
            .expect("forwards relay-only INVITE");

        let (length, _) = proxy.recv_from(&mut datagram).await.expect("200 answer");
        let answer = behind_relay(&datagram[..length], callee_dead);
        proxy
            .send_to(&answer, caller_address)
            .await
            .expect("forwards relay-only answer");
    });

    let answering = tokio::spawn(async move {
        let mut dispatcher = Dispatcher::new(callee_endpoint.clone(), callee_incoming);
        let Dispatched::Invitation(invitation) = dispatcher.next().await.expect("an invitation")
        else {
            panic!("expected an invitation");
        };
        let config = CallConfig::new(MediaAddress::new(loopback())).with_media_policy(
            MediaPolicy::default().with_ice(IcePolicy::Turn(callee_turn_policy)),
        );
        invitation
            .answer_with_config(&callee_endpoint, config)
            .await
            .expect("answers with TURN")
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let caller_config = CallConfig::new(MediaAddress::new(loopback()))
        .with_media_policy(MediaPolicy::default().with_ice(IcePolicy::Turn(caller_turn_policy)));
    let caller = tokio::time::timeout(
        Duration::from_secs(15),
        dial(
            &caller_endpoint,
            Target::udp(proxy_address),
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback())
                .with_call_config(caller_config),
        ),
    )
    .await
    .expect("relayed ICE converges within the failure bound")
    .expect("caller connects through TURN");
    let callee = answering.await.expect("answer task");
    forwarding.await.expect("proxy task");

    assert_bidirectional_relay_audio(&caller, &callee, &caller_turn, &callee_turn).await;
    assert_eq!(caller.media().ice_path(), IcePath::Relayed);
    assert_eq!(callee.media().ice_path(), IcePath::Relayed);

    drop(caller);
    drop(callee);
    let ((), ()) = tokio::join!(caller_turn.shutdown(), callee_turn.shutdown());
}

/// A relay is path diversity. A prompt TURN refusal contributes no relay candidate, but it cannot
/// erase the direct candidates already gathered by either role or turn a working direct call into
/// a signalling failure.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_relay_still_selects_the_direct_path() {
    let refusing_turn = refusing_turn_fixture().await;
    let turn =
        TurnPolicy::new(refusing_turn.address, "1000", "relay-password").expect("TURN policy");
    let (callee_endpoint, callee_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("callee binds");
    let (caller_endpoint, _caller_incoming) =
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("caller binds");
    let callee_address = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let mut dispatcher = Dispatcher::new(callee_endpoint.clone(), callee_incoming);
        let Dispatched::Invitation(invitation) = dispatcher.next().await.expect("an invitation")
        else {
            panic!("expected an invitation");
        };
        invitation
            .answer_with_config(
                &callee_endpoint,
                CallConfig::new(MediaAddress::new(loopback())).with_media_policy(ice()),
            )
            .await
            .expect("answers with direct ICE")
    });

    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("hostname"),
    ));
    let caller_config = CallConfig::new(MediaAddress::new(loopback()))
        .with_media_policy(MediaPolicy::default().with_ice(IcePolicy::Turn(turn)));
    let caller = tokio::time::timeout(
        Duration::from_secs(8),
        dial(
            &caller_endpoint,
            Target::udp(callee_address),
            &to,
            &DialOptions::new("<sip:caller@example.net>", loopback())
                .with_call_config(caller_config),
        ),
    )
    .await
    .expect("relay refusal and direct nomination are bounded")
    .expect("direct candidates connect the call");
    let callee = answering.await.expect("answer task");

    let caller_tone = vec![6_000i16; 8_000];
    let (_played, heard_by_callee) = tokio::join!(
        caller.media().play(&caller_tone, 160),
        callee.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(
        heard_by_callee.len(),
        REQUIRED,
        "direct caller audio arrives"
    );
    let callee_tone = vec![-6_000i16; 8_000];
    let (_played, heard_by_caller) = tokio::join!(
        callee.media().play(&callee_tone, 160),
        caller.media().record_at_least(REQUIRED, DELIVERY_BOUND),
    );
    assert_eq!(
        heard_by_caller.len(),
        REQUIRED,
        "direct callee audio arrives"
    );
    assert_eq!(caller.media().ice_path(), IcePath::Host);
    assert_eq!(callee.media().ice_path(), IcePath::Host);

    drop(caller);
    drop(callee);
    refusing_turn.shutdown().await;
}
