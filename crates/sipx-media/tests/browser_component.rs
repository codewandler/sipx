//! Browser-audio component vectors from `docs/specs/webrtc-audio.md` §9.5 and §9.6.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation
)]

use std::net::SocketAddr;

use sipx_media::browser::{
    ComponentIngress, ComponentState, IngressClass, IngressDisposition, IngressDrop,
    SelectedComponent, classify_datagram,
};
use sipx_media::dtls::{self, Handshake, Profile, Role};
use sipx_sdp::fingerprint::{Fingerprint, HashFunc};

#[test]
fn ba_pkt_vectors_classify_before_any_protocol_parser_runs() {
    assert_eq!(classify_datagram(&[0x00, 0x01]), Ok(IngressClass::Stun));
    assert_eq!(
        classify_datagram(&[0x16, 0xfe, 0xfd, 0x00]),
        Ok(IngressClass::Dtls)
    );
    assert_eq!(classify_datagram(&[0x80, 0x6f]), Ok(IngressClass::Srtp));
    assert_eq!(classify_datagram(&[0x80, 0xc8]), Ok(IngressClass::Srtcp));
    assert_eq!(
        classify_datagram(&[0x40, 0x00]),
        Err(IngressDrop::UnknownProtocol)
    );
    assert_eq!(classify_datagram(&[]), Err(IngressDrop::Empty));
    assert_eq!(
        classify_datagram(&[0x80]),
        Err(IngressDrop::TruncatedClassPrefix)
    );
    assert_eq!(
        classify_datagram(&vec![0; 2049]),
        Err(IngressDrop::Oversized)
    );
}

#[test]
fn ba_state_1_accounts_for_every_security_transition() {
    let local: SocketAddr = "127.0.0.1:41000".parse().expect("local address");
    let peer: SocketAddr = "127.0.0.1:42000".parse().expect("peer address");
    let stranger: SocketAddr = "127.0.0.1:43000".parse().expect("stranger address");
    let mut ingress = ComponentIngress::new(7);

    assert_eq!(
        ingress.admit(peer, &[0x16, 0xfe, 0xfd, 0x00]),
        IngressDisposition::Dropped(IngressDrop::BeforeNomination)
    );
    ingress
        .nominate(SelectedComponent::new(local, peer, 7))
        .expect("current-generation nomination");
    assert_eq!(ingress.snapshot().state, ComponentState::Nominated);
    assert_eq!(
        ingress.admit(stranger, &[0x16, 0xfe, 0xfd, 0x00]),
        IngressDisposition::Dropped(IngressDrop::WrongPeer)
    );
    ingress
        .begin_dtls(peer)
        .expect("nominated peer starts DTLS");
    assert_eq!(
        ingress.admit(peer, &[0x16, 0xfe, 0xfd, 0x00]),
        IngressDisposition::Accepted(IngressClass::Dtls)
    );
    assert_eq!(
        ingress.admit(peer, &[0x80, 0x6f]),
        IngressDisposition::Dropped(IngressDrop::KeysUnavailable)
    );

    ingress
        .install_verified_keys(verified_keys())
        .expect("verified keys install atomically");
    assert_eq!(ingress.snapshot().state, ComponentState::KeysInstalled);
    let keys = ingress
        .start_media()
        .expect("media starts after key install");
    assert_eq!(keys.local.0.len(), 16);
    assert_eq!(keys.remote.1.len(), 14);
    assert_eq!(
        ingress.admit(peer, &[0x80, 0x6f]),
        IngressDisposition::Accepted(IngressClass::Srtp)
    );
    assert_eq!(
        ingress.admit(peer, &[0x80, 0xc8]),
        IngressDisposition::Accepted(IngressClass::Srtcp)
    );

    let snapshot = ingress.snapshot();
    assert_eq!(snapshot.state, ComponentState::Running);
    assert_eq!(snapshot.selected.expect("selected peer").remote, peer);
    assert_eq!(snapshot.counts.dtls_before_nomination, 1);
    assert_eq!(snapshot.counts.dtls_wrong_peer, 1);
    assert_eq!(snapshot.counts.srtp_keys_unavailable, 1);
    assert_eq!(snapshot.counts.total(), 3);

    assert_eq!(
        ingress.admit(peer, &[0x16, 0xfe, 0xfd, 0x00]),
        IngressDisposition::Dropped(IngressDrop::UnexpectedDtls)
    );
    assert_eq!(
        ingress.admit(peer, &vec![0; 2049]),
        IngressDisposition::Dropped(IngressDrop::Oversized)
    );
    let refused = ingress.snapshot().counts;
    assert_eq!(refused.dtls_unexpected_records, 1);
    assert_eq!(refused.ingress_oversized, 1);
    assert_eq!(refused.total(), 5);
}

#[test]
fn a_nomination_from_an_old_generation_cannot_move_the_gate() {
    let local: SocketAddr = "127.0.0.1:41000".parse().expect("local address");
    let peer: SocketAddr = "127.0.0.1:42000".parse().expect("peer address");
    let mut ingress = ComponentIngress::new(8);

    assert!(
        ingress
            .nominate(SelectedComponent::new(local, peer, 7))
            .is_err()
    );
    assert_eq!(ingress.snapshot().state, ComponentState::IceChecking);
    assert!(ingress.snapshot().selected.is_none());
}

#[derive(Debug)]
struct Stub {
    certificate: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
#[error("stub handshake failed")]
struct StubError;

impl Handshake for Stub {
    type Error = StubError;

    fn run(&mut self, _role: Role) -> Result<(), Self::Error> {
        Ok(())
    }

    fn peer_certificate(&self) -> Option<Vec<u8>> {
        Some(self.certificate.clone())
    }

    fn profile(&self) -> Option<Profile> {
        Some(Profile::Aes128CmHmacSha1_80)
    }

    fn export(&self, len: usize) -> Result<Vec<u8>, Self::Error> {
        Ok((0u8..).take(len).collect())
    }
}

fn verified_keys() -> dtls::VerifiedKeys {
    let certificate = b"the nominated peer certificate".to_vec();
    let fingerprint = Fingerprint::of(&certificate, HashFunc::Sha256);
    dtls::establish_verified(&mut Stub { certificate }, Role::Client, Some(&fingerprint))
        .expect("the matching fingerprint verifies")
}

#[test]
fn ba_sdp_n10_never_produces_installable_keys() {
    let certificate = b"the nominated peer certificate".to_vec();
    let wrong = Fingerprint::of(b"a different certificate", HashFunc::Sha256);
    let result = dtls::establish_verified(&mut Stub { certificate }, Role::Client, Some(&wrong));
    assert!(matches!(result, Err(dtls::Error::FingerprintMismatch)));
}

/// The component uses the transform's separate SRTP and SRTCP replay windows: advancing either
/// stream cannot consume the other's index, and each authenticated packet is accepted once.
#[test]
fn srtp_and_srtcp_replays_remain_separate_when_interleaved() {
    use sipx_rtp::{SrtpContext, SrtpError};

    let profile = sipx_rtp::srtp::Profile::AesCm128HmacSha1_80;
    let mut sender = SrtpContext::new(profile, &[0x11; 16], &[0x22; 14]).expect("sender keys");
    let mut receiver = SrtpContext::new(profile, &[0x11; 16], &[0x22; 14]).expect("receiver keys");
    let media_plain = [
        0x80, 111, 0, 0, 0, 0, 0, 0, 0xca, 0xfe, 0xba, 0xbe, 1, 2, 3, 4,
    ];
    let control_plain = [0x80, 201, 0, 1, 0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0];
    let media = sender
        .protect(&media_plain)
        .expect("protect RTP index zero");
    let control = sender
        .protect_rtcp(&control_plain)
        .expect("protect SRTCP index zero");

    assert_eq!(
        receiver.unprotect(&media).expect("RTP accepted"),
        media_plain
    );
    assert_eq!(
        receiver
            .unprotect_rtcp(&control)
            .expect("SRTCP independently accepted"),
        control_plain
    );
    assert_eq!(receiver.unprotect(&media), Err(SrtpError::Replayed(0)));
    assert_eq!(
        receiver.unprotect_rtcp(&control),
        Err(SrtpError::ReplayedRtcp(0))
    );
}

#[cfg(all(feature = "dtls", feature = "opus"))]
fn browser_gathering(ufrag: &str, offerer: bool) -> sipx_media::ice::Gathering {
    let credentials = sipx_sdp::ice::Credentials::new(ufrag, "browserPassword0123456789AB")
        .expect("valid ICE credentials");
    let mut gathering = sipx_media::ice::Gathering::new(credentials, offerer);
    gathering.agent.timers = sipx_media::ice::Timers {
        ta: std::time::Duration::from_millis(20),
        tn: std::time::Duration::from_millis(250),
        tr: std::time::Duration::from_millis(200),
        ..sipx_media::ice::Timers::default()
    };
    gathering
}

#[cfg(all(feature = "dtls", feature = "opus"))]
fn peer_description(local: &sipx_media::ice::LocalDescription) -> sipx_media::ice::Negotiation {
    sipx_media::ice::Negotiation::Ice {
        credentials: local.credentials().clone(),
        candidates: local.candidates().to_vec(),
        lite: false,
    }
}

#[cfg(all(feature = "dtls", feature = "opus"))]
fn opus_config(remote: SocketAddr) -> sipx_media::Config {
    let mut config = sipx_media::Config::new(remote, sipx_media::Codec::Opus);
    config.payload_type = Some(111);
    config.rtcp_mode = sipx_sdp::RtcpMode::Mux;
    config.rtcp_interval = Some(std::time::Duration::from_millis(50));
    config
}

#[cfg(all(feature = "dtls", feature = "opus"))]
fn tone(samples: usize, hz: f64) -> Vec<i16> {
    (0..samples)
        .map(|index| {
            let seconds = index as f64 / 48_000.0;
            ((seconds * hz * std::f64::consts::TAU).sin() * 12_000.0) as i16
        })
        .collect()
}

/// BA-STATE-1's live composition: one socket receiver survives ICE, DTLS, SRTP and SRTCP.
#[cfg(all(feature = "dtls", feature = "opus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_sipx_components_carry_non_silent_opus_and_process_muxed_srtcp() {
    use sipx_media::dtls::openssl::Identity;
    use sipx_media::{MediaPort, browser::ComponentState};

    let alice_port = MediaPort::bind("127.0.0.1:0".parse().expect("bind address"))
        .await
        .expect("Alice component");
    let bob_port = MediaPort::bind("127.0.0.1:0".parse().expect("bind address"))
        .await
        .expect("Bob component");
    let (alice_addr, bob_addr) = (alice_port.local_addr(), bob_port.local_addr());
    let alice_gathering = browser_gathering("alice1", true);
    let bob_gathering = browser_gathering("bob001", false);
    let (mut alice_ice, mut bob_ice) = tokio::join!(
        alice_port.gather_with_rtcp_mode(&alice_gathering, sipx_sdp::RtcpMode::Mux,),
        bob_port.gather_with_rtcp_mode(&bob_gathering, sipx_sdp::RtcpMode::Mux,),
    );
    assert!(alice_ice.accept(&peer_description(&bob_ice)));
    assert!(bob_ice.accept(&peer_description(&alice_ice)));

    let alice_identity = Identity::generate().expect("Alice identity");
    let bob_identity = Identity::generate().expect("Bob identity");
    let alice_fingerprint = alice_identity.fingerprint().expect("Alice fingerprint");
    let bob_fingerprint = bob_identity.fingerprint().expect("Bob fingerprint");

    let start = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        tokio::join!(
            alice_port.start_browser_audio(
                opus_config(bob_addr),
                alice_ice,
                0,
                alice_identity,
                Role::Client,
                bob_fingerprint,
                std::time::Duration::from_secs(5),
            ),
            bob_port.start_browser_audio(
                opus_config(alice_addr),
                bob_ice,
                0,
                bob_identity,
                Role::Server,
                alice_fingerprint,
                std::time::Duration::from_secs(5),
            ),
        )
    })
    .await
    .expect("failure bound: ICE and DTLS finish within eight seconds");
    let (alice, bob) = (start.0.expect("Alice starts"), start.1.expect("Bob starts"));

    let alice_facts = alice.browser_component().expect("Alice browser facts");
    let bob_facts = bob.browser_component().expect("Bob browser facts");
    assert_eq!(alice_facts.state, ComponentState::Running);
    assert_eq!(bob_facts.state, ComponentState::Running);
    assert_eq!(alice_facts.selected.expect("Alice pair").remote, bob_addr);
    assert_eq!(bob_facts.selected.expect("Bob pair").remote, alice_addr);

    let frames = 40;
    let samples = alice.samples_per_packet() * frames;
    let alice_tone = tone(samples, 440.0);
    let bob_tone = tone(samples, 660.0);
    let (alice_played, bob_heard, bob_played, alice_heard) = tokio::join!(
        alice.play(&alice_tone, alice.samples_per_packet()),
        bob.record_at_least(samples, std::time::Duration::from_secs(8)),
        bob.play(&bob_tone, bob.samples_per_packet()),
        alice.record_at_least(samples, std::time::Duration::from_secs(8)),
    );
    assert!(alice_played && bob_played);
    assert!(bob_heard.iter().any(|sample| sample.abs() > 500));
    assert!(alice_heard.iter().any(|sample| sample.abs() > 500));
    assert!(
        alice
            .browser_component()
            .expect("Alice facts")
            .counts
            .srtcp_processed
            > 0,
        "Alice processed protected RTCP on the nominated mux port"
    );
    assert!(
        bob.browser_component()
            .expect("Bob facts")
            .counts
            .srtcp_processed
            > 0,
        "Bob processed protected RTCP on the nominated mux port"
    );

    tokio::join!(alice.shutdown(), bob.shutdown());
}

/// BA-SDP-N10 over the live owner: a complete handshake with the wrong fingerprint installs no
/// media session, and the failed preparation reaps both socket-owning tasks before returning.
#[cfg(all(feature = "dtls", feature = "opus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_fingerprint_mismatch_installs_no_keys_and_releases_the_component() {
    use sipx_media::MediaPort;
    use sipx_media::browser::BrowserStartError;
    use sipx_media::dtls::openssl::Identity;

    let alice_port = MediaPort::bind("127.0.0.1:0".parse().expect("bind address"))
        .await
        .expect("Alice component");
    let bob_port = MediaPort::bind("127.0.0.1:0".parse().expect("bind address"))
        .await
        .expect("Bob component");
    let (alice_addr, bob_addr) = (alice_port.local_addr(), bob_port.local_addr());
    let alice_gathering = browser_gathering("alice2", true);
    let bob_gathering = browser_gathering("bob002", false);
    let (mut alice_ice, mut bob_ice) = tokio::join!(
        alice_port.gather_with_rtcp_mode(&alice_gathering, sipx_sdp::RtcpMode::Mux),
        bob_port.gather_with_rtcp_mode(&bob_gathering, sipx_sdp::RtcpMode::Mux),
    );
    assert!(alice_ice.accept(&peer_description(&bob_ice)));
    assert!(bob_ice.accept(&peer_description(&alice_ice)));

    let alice_identity = Identity::generate().expect("Alice identity");
    let bob_identity = Identity::generate().expect("Bob identity");
    let alice_fingerprint = alice_identity.fingerprint().expect("Alice fingerprint");
    let wrong_bob = sipx_sdp::fingerprint::Fingerprint::of(
        b"a certificate the nominated peer does not hold",
        HashFunc::Sha256,
    );
    let results = tokio::time::timeout(std::time::Duration::from_secs(8), async {
        tokio::join!(
            alice_port.start_browser_audio(
                opus_config(bob_addr),
                alice_ice,
                0,
                alice_identity,
                Role::Client,
                wrong_bob,
                std::time::Duration::from_secs(5),
            ),
            bob_port.start_browser_audio(
                opus_config(alice_addr),
                bob_ice,
                0,
                bob_identity,
                Role::Server,
                alice_fingerprint,
                std::time::Duration::from_secs(5),
            ),
        )
    })
    .await
    .expect("failure bound: the mismatched handshake resolves");
    assert!(matches!(
        results.0,
        Err(BrowserStartError::Dtls(dtls::Error::FingerprintMismatch))
    ));
    if let Ok(bob) = results.1 {
        bob.shutdown().await;
    }

    let rebound = tokio::net::UdpSocket::bind(alice_addr)
        .await
        .expect("the failed component owner was reaped before the error returned");
    drop(rebound);
}

/// The DTLS deadline is a typed terminal result, and its supervisor waits for the blocking adapter
/// to finish before releasing the one socket.
#[cfg(all(feature = "dtls", feature = "opus"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_silent_nominated_peer_yields_typed_dtls_timeout_and_no_detached_owner() {
    use sipx_media::browser::BrowserStartError;
    use sipx_media::dtls::openssl::Identity;
    use sipx_media::{Codec, Config, MediaPort};

    let alice_port = MediaPort::bind("127.0.0.1:0".parse().expect("bind address"))
        .await
        .expect("Alice component");
    let bob_port = MediaPort::bind("127.0.0.1:0".parse().expect("bind address"))
        .await
        .expect("Bob ICE peer");
    let (alice_addr, bob_addr) = (alice_port.local_addr(), bob_port.local_addr());
    let alice_gathering = browser_gathering("alice3", true);
    let bob_gathering = browser_gathering("bob003", false);
    let (mut alice_ice, mut bob_ice) = tokio::join!(
        alice_port.gather_with_rtcp_mode(&alice_gathering, sipx_sdp::RtcpMode::Mux),
        bob_port.gather_with_rtcp_mode(&bob_gathering, sipx_sdp::RtcpMode::Mux),
    );
    assert!(alice_ice.accept(&peer_description(&bob_ice)));
    assert!(bob_ice.accept(&peer_description(&alice_ice)));

    let alice_identity = Identity::generate().expect("Alice identity");
    let absent_identity = Identity::generate().expect("fingerprint fixture");
    let absent_fingerprint = absent_identity.fingerprint().expect("fingerprint fixture");
    let mut bob_config = Config::new(alice_addr, Codec::Pcmu);
    bob_config.rtcp_mode = sipx_sdp::RtcpMode::Mux;
    bob_config.rtcp_interval = None;
    let (alice_result, bob) = tokio::time::timeout(std::time::Duration::from_secs(4), async {
        tokio::join!(
            alice_port.start_browser_audio(
                opus_config(bob_addr),
                alice_ice,
                0,
                alice_identity,
                Role::Client,
                absent_fingerprint,
                std::time::Duration::from_millis(200),
            ),
            async move {
                bob_port
                    .start_with_ice(bob_config, bob_ice)
                    .expect("ordinary ICE peer starts")
            },
        )
    })
    .await
    .expect("failure bound: the silent DTLS peer resolves");
    assert!(matches!(alice_result, Err(BrowserStartError::DtlsTimeout)));
    drop(bob);

    let rebound = tokio::net::UdpSocket::bind(alice_addr)
        .await
        .expect("timeout reaped the supervisor, adapter, ICE and component owner");
    drop(rebound);
}
