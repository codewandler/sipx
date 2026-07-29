//! ICE on the media port: the driver, end to end (`M-22`, `docs/specs/ice.md` §2, §11, §13.3).
//!
//! Two things are being asserted here and they pull in opposite directions, which is why they
//! share a file.
//!
//! **A nominated pair carries audio.** Two sessions whose advertised addresses do not work find
//! each other by checking, and the audio arrives on the pair they agreed on. That is the point of
//! the whole epic and it is the test `M-16` named.
//!
//! **A peer that offers no ICE is unaffected.** No candidate offered, no check sent, no timer
//! armed, symmetric RTP exactly as before. A stack that requires ICE to place a call has
//! regressed, and the rest of this crate's suite — `quality`, `srtp`, `bridge`, `conference`,
//! `opus`, all of which start sessions through the same constructor — is the wider proof.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_media::ice::stun::{self, Class, Message, Peering};
use sipx_media::ice::{self, Gathering, Negotiation, negotiate};
use sipx_media::{Codec, Config, MediaPort, MediaSession};
use sipx_sdp::ice::{ComponentId, Credentials};
use tokio::net::UdpSocket;

/// Ta, Tn and Tr, shrunk so the exchange finishes inside a test rather than inside a call.
///
/// Nothing else is changed: the pacing floor, the RTO floor and Rc are the RFC's, so what is
/// being measured is still the real convergence path and not a special one.
fn timers() -> ice::Timers {
    ice::Timers {
        ta: Duration::from_millis(20),
        tn: Duration::from_millis(250),
        tr: Duration::from_millis(200),
        ..ice::Timers::default()
    }
}

fn credentials(ufrag: &str) -> Credentials {
    Credentials::new(ufrag, "asd88fgpdd777uzjYhagZg").expect("a valid credential")
}

fn gathering(ufrag: &str, offerer: bool) -> Gathering {
    let mut gathering = Gathering::new(credentials(ufrag), offerer);
    gathering.agent.timers = timers();
    gathering
}

/// A socket bound and never read: a real address, reserved, that answers nothing.
///
/// This is what stands in for a host candidate on the far side of a NAT — the LAN address a peer
/// honestly advertises and that we cannot reach. A bound socket rather than an unroutable literal
/// so that the test's failure mode is a timeout on this machine and not a routing decision on
/// whatever machine it runs on.
async fn dead_end() -> (UdpSocket, SocketAddr) {
    let socket = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .expect("a loopback port");
    let address = socket.local_addr().expect("bound");
    (socket, address)
}

/// An offer or answer from a peer whose **host** candidate is unreachable and whose
/// server-reflexive one is where it can actually be found.
///
/// Which is the ordinary NAT case, stated in SDP: the highest-priority candidate — and therefore
/// the `c=`/`m=` default destination — is the address that does not work.
fn description(unreachable: SocketAddr, reachable: SocketAddr, ufrag: &str) -> String {
    format!(
        "v=0\r\n\
         o=- 1 1 IN IP4 {ip}\r\n\
         s=-\r\n\
         c=IN IP4 {ip}\r\n\
         t=0 0\r\n\
         m=audio {port} RTP/AVP 0\r\n\
         a=ice-ufrag:{ufrag}\r\n\
         a=ice-pwd:asd88fgpdd777uzjYhagZg\r\n\
         a=ice-options:ice2\r\n\
         a=candidate:1 1 UDP 2130706431 {ip} {port} typ host\r\n\
         a=candidate:2 1 UDP 1694498815 {reachable_ip} {reachable_port} typ srflx \
         raddr {ip} rport {port}\r\n",
        ip = unreachable.ip(),
        port = unreachable.port(),
        reachable_ip = reachable.ip(),
        reachable_port = reachable.port(),
        ufrag = ufrag,
    )
}

fn read(text: &str) -> Negotiation {
    let session = sipx_sdp::parse(text).expect("the fixture parses");
    negotiate(&session, session.media.first().expect("one stream"))
}

/// A tone loud enough that a receiver cannot mistake silence for it.
fn tone(samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|index| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let value = (8000.0 * f64::sin(index as f64 * 0.2)) as i16;
            value
        })
        .collect()
}

/// **The test `M-16` named, and the reason this story comes after the agent rather than beside
/// it.**
///
/// Both sides advertise a host candidate that cannot carry anything — a bound, silent port — and
/// that host candidate is the highest priority one, so it is also where the `c=`/`m=` line points
/// and where symmetric RTP alone would send every packet for the life of the call. The only path
/// that works is the server-reflexive pair, and nothing but a nominated candidate pair will find
/// it.
///
/// The assertion is audio and not a pair state on purpose: a checklist that concludes and a call
/// that is audible are different claims, and this story is the second one.
#[tokio::test(flavor = "multi_thread")]
async fn a_nominated_candidate_pair_carries_audio_when_the_host_candidates_cannot() {
    let (_alice_dead, alice_unreachable) = dead_end().await;
    let (_bob_dead, bob_unreachable) = dead_end().await;

    let alice_port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("a media port");
    let bob_port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("a media port");
    let (alice_reachable, bob_reachable) = (alice_port.local_addr(), bob_port.local_addr());

    let mut alice_local = alice_port.gather(&gathering("aaaa", true)).await;
    let mut bob_local = bob_port.gather(&gathering("bbbb", false)).await;

    // What each side receives is the other's description with its host candidate pointing at the
    // dead end and its reflexive one at the port it is really on.
    let to_alice = read(&description(bob_unreachable, bob_reachable, "bbbb"));
    let to_bob = read(&description(alice_unreachable, alice_reachable, "aaaa"));
    assert!(to_alice.runs_ice() && to_bob.runs_ice());
    assert!(alice_local.accept(&to_alice));
    assert!(bob_local.accept(&to_bob));

    // And where each side would send without ICE: the peer's default destination, which is the
    // address that does not work.
    let alice = alice_port.start_with_ice(Config::new(bob_unreachable, Codec::Pcmu), alice_local);
    let bob = bob_port.start_with_ice(Config::new(alice_unreachable, Codec::Pcmu), bob_local);

    let samples = alice.samples_per_packet();
    let speaker = tokio::spawn(async move {
        for _ in 0..150 {
            if !alice.send(tone(samples)).await {
                return;
            }
        }
    });

    let heard = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let frame = bob.recv().await?;
            if frame.iter().any(|sample| sample.abs() > 1000) {
                return Some(frame);
            }
        }
    })
    .await
    .expect("audio arrives on the nominated pair within eight seconds")
    .expect("the session is still running");

    assert!(!heard.is_empty(), "the nominated pair carried the tone");
    speaker.abort();
}

/// Acceptance's hard line, and the vision's: **no `a=candidate` means today's behaviour.**
///
/// Nothing is offered, no check leaves the port, no timer runs, and the destination is still
/// learned from the first packet that arrives — which is what carries a call through a NAT when
/// the far end is not doing ICE, which is most of them.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_offers_no_ice_keeps_symmetric_rtp() {
    assert_eq!(
        read(concat!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n",
            "m=audio 49170 RTP/AVP 0\r\n",
        )),
        Negotiation::Absent,
        "no candidate attributes is not ice"
    );

    // Where the peer said to send, and where it actually is. Symmetric RTP is the difference.
    let (_advertised, advertised) = dead_end().await;
    let peer = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();

    let session = MediaSession::start("127.0.0.1:0".parse().unwrap(), {
        let mut config = Config::new(advertised, Codec::Pcmu);
        config.rtcp_interval = None;
        config
    })
    .await
    .expect("a session");
    let local = session.local_addr();

    // The peer opens the pinhole with one packet from the address it is really on.
    let packet = sipx_rtp::Packet::new(
        0,
        1,
        160,
        0x1234_5678,
        bytes::Bytes::from(vec![0xffu8; 160]),
    )
    .encode();
    peer.send_to(&packet, local).await.unwrap();

    // Now this side speaks, and it must arrive at the observed source rather than the advertised
    // address — which is exactly what a stream with no ICE must go on doing.
    let samples = session.samples_per_packet();
    tokio::spawn(async move {
        for _ in 0..100 {
            if !session.send(tone(samples)).await {
                return;
            }
        }
    });

    let mut datagram = vec![0u8; 2048];
    let (len, from) = tokio::time::timeout(Duration::from_secs(3), peer.recv_from(&mut datagram))
        .await
        .expect("the session sent to the address it heard from")
        .unwrap();
    assert_eq!(from, local);
    assert_eq!(
        sipx_media::dtls::classify(&datagram[..len]),
        sipx_media::Arriving::Rtp,
        "a stream with no ice sends media and never a connectivity check"
    );
}

/// RFC 5764 §5.1.2 on the port media uses: a connectivity check must never reach the jitter
/// buffer, and an RTP packet must never reach the agent.
///
/// Both halves are asserted against the same running session. The check gets a STUN response
/// because it reached the agent; the RTP packet gets none because it did not, and neither of them
/// is ever handed to the other path.
#[tokio::test(flavor = "multi_thread")]
async fn a_check_and_a_media_packet_are_demultiplexed_on_one_port() {
    let (_dead, unreachable) = dead_end().await;
    let peer = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let peer_address = peer.local_addr().unwrap();

    let port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("a media port");
    let local = port.local_addr();
    let mut description_in = gathering("aaaa", true);
    description_in.agent.timers = timers();
    let mut ours = port.gather(&description_in).await;
    assert!(ours.accept(&read(&description(unreachable, peer_address, "bbbb"))));
    let session = port.start_with_ice(
        {
            let mut config = Config::new(unreachable, Codec::Pcmu);
            config.rtcp_interval = None;
            config
        },
        ours,
    );

    // A check from the peer, keyed as §11.2 requires: our ufrag first, and our password.
    let peering = Peering::new(credentials("bbbb"), credentials("aaaa"));
    let transaction = stun::new_transaction_id();
    let check = stun::connectivity_check(
        transaction,
        &peering,
        sipx_sdp::ice::Priority::new(1_694_498_815).unwrap(),
        stun::RoleAttribute::Controlled { tiebreaker: 7 },
    )
    .expect("encodes");
    peer.send_to(&check, local).await.unwrap();

    let mut datagram = vec![0u8; 2048];
    let answered = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (len, _) = peer.recv_from(&mut datagram).await.unwrap();
            let Ok(message) = Message::decode(&datagram[..len]) else {
                continue;
            };
            if message.transaction() == transaction && message.class() == Class::Success {
                return message;
            }
        }
    })
    .await
    .expect("the check reached the agent and was answered");
    assert!(answered.verify_integrity(peering.outbound_key()));

    // And an RTP packet on the same port, which must not be answered as though it were a check.
    // A media packet reaching the agent would be a parse of unauthenticated bytes by the one
    // parser that may act on them.
    let media =
        sipx_rtp::Packet::new(0, 1, 160, 0x1234_5678, bytes::Bytes::from(vec![0u8; 160])).encode();
    peer.send_to(&media, local).await.unwrap();
    let quiet = tokio::time::timeout(Duration::from_millis(400), async {
        loop {
            let (len, _) = peer.recv_from(&mut datagram).await.unwrap();
            let stun = matches!(
                sipx_media::dtls::classify(&datagram[..len]),
                sipx_media::Arriving::Stun
            );
            let success = Message::decode(&datagram[..len])
                .is_ok_and(|message| message.class() == Class::Success);
            if stun && success {
                return true;
            }
        }
    })
    .await;
    assert!(
        quiet.is_err() || !quiet.unwrap(),
        "an rtp packet must not be answered as a connectivity check"
    );
    drop(session);
}

/// §11 and RFC 8839 §6: a selected pair that has carried nothing for Tr is kept alive with a
/// **Binding Indication** — no authentication, a `FINGERPRINT` so the far end can demultiplex it,
/// and nothing else. Only on selected pairs, so nothing like it appears before there is one.
#[tokio::test(flavor = "multi_thread")]
async fn a_selected_pair_is_kept_alive_with_a_binding_indication() {
    let peer = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let peer_address = peer.local_addr().unwrap();

    let port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("a media port");
    let local = port.local_addr();
    let mut ours = port.gather(&gathering("aaaa", true)).await;
    assert!(ours.accept(&read(&description(peer_address, peer_address, "bbbb"))));
    let session = port.start_with_ice(
        {
            let mut config = Config::new(peer_address, Codec::Pcmu);
            config.rtcp_interval = None;
            config
        },
        ours,
    );

    // Answer whatever checks arrive, so that a pair becomes valid and is nominated, and watch for
    // the first thing that is not a request.
    let peering = Peering::new(credentials("bbbb"), credentials("aaaa"));
    let mut datagram = vec![0u8; 2048];
    let indication = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let (len, from) = peer.recv_from(&mut datagram).await.unwrap();
            let Ok(message) = Message::decode(&datagram[..len]) else {
                continue;
            };
            match message.class() {
                Class::Request => {
                    let reply = stun::check_success(message.transaction(), &peering, from)
                        .expect("encodes");
                    peer.send_to(&reply, from).await.unwrap();
                }
                Class::Indication => return message,
                Class::Success | Class::Error => {}
            }
        }
    })
    .await
    .expect("a keepalive follows the selected pair within Tr");

    assert_eq!(indication.class(), Class::Indication);
    assert!(
        !indication.has_integrity(),
        "§11: a keepalive MUST NOT utilize any authentication mechanism"
    );
    assert!(
        indication.has_fingerprint(),
        "§11: it SHOULD carry FINGERPRINT so the far end can demultiplex it"
    );
    assert!(
        indication.attributes().is_empty(),
        "§11: and SHOULD NOT carry anything else"
    );
    assert_eq!(session.local_addr(), local);
}

/// RFC 8839 §5.3: the offerer's default destination for a component matched none of its
/// candidates, so the answer says `ice-mismatch` and the stream is carried by RFC 3264's
/// procedures instead — which on this media port means symmetric RTP and no agent at all.
#[tokio::test(flavor = "multi_thread")]
async fn an_ice_mismatch_is_reported_and_the_stream_falls_back() {
    let (_dead, elsewhere) = dead_end().await;
    let (_dead_two, advertised) = dead_end().await;

    // Candidates that describe one path, a `c=`/`m=` pair that names another — an ALG in the
    // middle rewrote the address the offerer advertised.
    let offer = format!(
        "v=0\r\n\
         o=- 1 1 IN IP4 {ip}\r\n\
         s=-\r\n\
         c=IN IP4 {ip}\r\n\
         t=0 0\r\n\
         m=audio {port} RTP/AVP 0\r\n\
         a=ice-ufrag:bbbb\r\n\
         a=ice-pwd:asd88fgpdd777uzjYhagZg\r\n\
         a=candidate:1 1 UDP 2130706431 {other_ip} {other_port} typ host\r\n",
        ip = advertised.ip(),
        port = advertised.port(),
        other_ip = elsewhere.ip(),
        other_port = elsewhere.port(),
    );
    let negotiation = read(&offer);
    assert_eq!(negotiation, Negotiation::Mismatch);
    assert_eq!(
        negotiation.answer_attributes(),
        vec![sipx_sdp::Attribute::flag("ice-mismatch")],
        "the answer reports it at media level"
    );

    let port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("a media port");
    let mut ours = port.gather(&gathering("aaaa", false)).await;
    assert!(
        !ours.accept(&negotiation),
        "§5.3: ICE MUST NOT be used for a mismatched stream"
    );

    // And the session that results is an ordinary one: no agent, no check, symmetric RTP.
    let peer = UdpSocket::bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .unwrap();
    let session = port.start_with_ice(
        {
            let mut config = Config::new(advertised, Codec::Pcmu);
            config.rtcp_interval = None;
            config
        },
        ours,
    );
    let local = session.local_addr();

    let packet = sipx_rtp::Packet::new(
        0,
        1,
        160,
        0x9876_5432,
        bytes::Bytes::from(vec![0xffu8; 160]),
    )
    .encode();
    peer.send_to(&packet, local).await.unwrap();
    let samples = session.samples_per_packet();
    tokio::spawn(async move {
        for _ in 0..100 {
            if !session.send(tone(samples)).await {
                return;
            }
        }
    });

    let mut datagram = vec![0u8; 2048];
    let (len, _) = tokio::time::timeout(Duration::from_secs(3), peer.recv_from(&mut datagram))
        .await
        .expect("the fallback stream still learns its destination")
        .unwrap();
    assert_eq!(
        sipx_media::dtls::classify(&datagram[..len]),
        sipx_media::Arriving::Rtp,
        "a mismatched stream sends media, never a check"
    );
}

/// `docs/specs/ice.md` §6.1: component 2 is offered **only** when the control port was obtained,
/// and the description a caller puts in its SDP says exactly what was gathered.
#[tokio::test]
async fn the_offer_carries_what_was_gathered_and_only_that() {
    let port = MediaPort::bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("a media port");
    let local = port.gather(&gathering("aaaa", true)).await;

    let attributes = local.attributes();
    let names: Vec<&str> = attributes
        .iter()
        .map(|attribute| attribute.name.as_str())
        .collect();
    assert_eq!(names[0], "ice-ufrag");
    assert_eq!(names[1], "ice-pwd");
    assert_eq!(names[2], "ice-options");
    assert_eq!(
        attributes[2].value.as_deref(),
        Some("ice2"),
        "§8: aggressive nomination is unavailable, and ice2 is how a peer is told"
    );

    let candidates: Vec<ComponentId> = local
        .candidates()
        .iter()
        .map(|candidate| candidate.component)
        .collect();
    assert!(candidates.contains(&ComponentId::RTP));
    assert_eq!(
        candidates.contains(&ComponentId::RTCP),
        port.has_control_port(),
        "component 2 exactly when the control port was obtained"
    );
    assert_eq!(
        local.default_destination(ComponentId::RTP),
        Some(port.local_addr()),
        "§13.2: the default destination is the highest-priority candidate"
    );
}
