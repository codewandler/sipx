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

use std::net::IpAddr;
use std::time::Duration;

use sipx_call::{DialOptions, IcePolicy, MediaPolicy, answer_with_policy, dial};
use sipx_media::Interrupt;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};
use tokio::net::UdpSocket;

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

    let after = credentials_in(&reoffer);
    assert_ne!(before.0, after.0, "ice-ufrag changed:\n{reoffer}");
    assert_ne!(before.1, after.1, "ice-pwd changed:\n{reoffer}");
    assert!(
        reoffer.contains("a=candidate:"),
        "a restart re-offers its candidates (RFC 8839 §4.4):\n{reoffer}"
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
