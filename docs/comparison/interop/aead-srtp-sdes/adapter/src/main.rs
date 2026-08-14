use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{DialOptions, Keying, MediaPolicy, SrtpSuite, dial};
use sipx_rtp::srtp::Profile;
use sipx_sip::Uri;
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};

fn tone(milliseconds: usize) -> Vec<i16> {
    let samples = milliseconds * 8;
    (0..samples)
        .map(|i| {
            let t = i as f64 / 8_000.0;
            let envelope = (t * 4.0).min(1.0);
            let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12_000.0 * envelope;
            value.round() as i16
        })
        .collect()
}

fn longest_aligned_match(sent: &[u8], received: &[u8]) -> usize {
    const WINDOW: usize = 320;
    if sent.len() < WINDOW || received.len() < WINDOW {
        return 0;
    }
    received
        .windows(WINDOW)
        .enumerate()
        .filter_map(|(received_offset, probe)| {
            let sent_offset = sent.windows(WINDOW).position(|window| window == probe)?;
            Some(
                sent[sent_offset..]
                    .iter()
                    .zip(received[received_offset..].iter())
                    .take_while(|(left, right)| left == right)
                    .count(),
            )
        })
        .max()
        .unwrap_or(0)
}

fn suite(argument: &str) -> (SrtpSuite, Profile) {
    match argument {
        "AEAD_AES_128_GCM" => (SrtpSuite::AeadAes128Gcm, Profile::AeadAes128Gcm),
        "AEAD_AES_256_GCM" => (SrtpSuite::AeadAes256Gcm, Profile::AeadAes256Gcm),
        value => panic!("unsupported suite {value}"),
    }
}

#[tokio::main]
async fn main() {
    let mut arguments = std::env::args().skip(1);
    let suite_name = arguments.next().expect("suite");
    let ca = PathBuf::from(arguments.next().expect("CA path"));
    let target_address: SocketAddr = arguments
        .next()
        .expect("target address")
        .parse()
        .expect("socket address");
    let expect_kdf_failure = arguments.next().as_deref() == Some("expect-kdf-failure");
    let (suite, expected_profile) = suite(&suite_name);
    let loopback: IpAddr = "127.0.0.1".parse().expect("loopback");

    let mut anchors = sipx_transport::tls::TrustAnchors::only();
    anchors
        .add_pem(&std::fs::read(&ca).expect("read proof CA"))
        .expect("proof CA");
    let mut transport = TransportConfig::new("127.0.0.1:0".parse().expect("bind address"));
    transport.sent_by = loopback.to_string();
    transport.tls_client =
        Some(sipx_transport::tls::ClientTls::new(&anchors).expect("TLS client configuration"));
    let (handle, _incoming) = bind(transport).await.expect("SIP transport binds");

    let uri = Uri::parse(Bytes::from_static(b"sip:echo@sipx.test:5091;transport=tls"))
        .expect("proof URI");
    let target = Target::new(target_address, TransportKind::Tls).verifying("sipx.test");
    let policy = MediaPolicy::default()
        .with_keying(Keying::Sdes)
        .with_srtp_suite(suite);
    let options = DialOptions::new("<sip:sipx-srtp@sipx.test>", loopback)
        .with_timeout(Duration::from_secs(15))
        .with_media_policy(policy);

    let mut call = tokio::time::timeout(
        Duration::from_secs(20),
        dial(&handle, target, &uri, &options),
    )
    .await
    .expect("bounded call attempt")
    .expect("peer accepts exact-suite SDES call");

    let media = call.media();
    assert!(media.is_encrypted(), "call must not fall back to RTP");
    assert_eq!(
        media.srtp_profile(),
        Some(expected_profile),
        "negotiation must retain the exact required suite"
    );
    assert_eq!(media.wire_payload_type(), 0, "proof requires PCMU");
    assert_eq!(media.receive_payload_type(), 0, "proof requires PCMU");

    let source = tone(1_000);
    let sent_payload = sipx_audio::g711::ulaw_encode_all(&source);
    media.set_relay(true);
    let collected = tokio::join!(async { media.play(&source, 160).await }, async {
        let mut payload = Vec::new();
        let mut payload_types = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while let Ok(Some(packet)) = tokio::time::timeout_at(deadline, media.recv_encoded()).await {
            payload_types.push(packet.payload_type);
            payload.extend_from_slice(&packet.payload);
        }
        (payload_types, payload)
    });
    assert!(collected.0, "local tone playback was admitted");
    let (payload_types, echoed_payload) = collected.1;
    media.set_relay(false);

    let discards = media.discard_counts();
    if expect_kdf_failure {
        assert!(media.packets_sent() >= 45, "negative sent too little media");
        assert_eq!(
            media.packets_received(),
            0,
            "incorrect KDF unexpectedly authenticated peer RTP"
        );
        assert!(
            echoed_payload.is_empty(),
            "incorrect KDF unexpectedly exposed peer audio"
        );
        assert!(
            discards.srtp_unprotect_failures > 0,
            "incorrect KDF caused no observable SRTP authentication failure"
        );
        let result = serde_json::json!({
            "suite_required": suite_name,
            "keying": "SDES",
            "negative": "right-align-aead-master-salt-in-14-octet-x",
            "expected_outcome": "independent-peer-rejects-incorrect-derived-keys",
            "encrypted": media.is_encrypted(),
            "negotiated_profile": format!("{:?}", media.srtp_profile().expect("profile")),
            "packets_sent": media.packets_sent(),
            "packets_received": media.packets_received(),
            "peer_echo_audio_bytes": echoed_payload.len(),
            "srtp_unprotect_failures": discards.srtp_unprotect_failures,
            "srtcp_unprotect_failures": discards.srtcp_unprotect_failures,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("result JSON")
        );
        call.hang_up().await.expect("peer accepts negative BYE");
        assert!(call.is_ended(), "negative call ended locally");
        return;
    }

    assert!(
        !echoed_payload.is_empty(),
        "peer returned no authenticated audio"
    );
    assert!(
        payload_types.iter().all(|payload_type| *payload_type == 0),
        "peer returned an unexpected payload type: {payload_types:?}"
    );
    let matched = longest_aligned_match(&sent_payload, &echoed_payload);
    let match_floor = 2 * 160;
    let peer_audio_peak = sipx_audio::g711::ulaw_decode_all(&echoed_payload)
        .into_iter()
        .map(i16::unsigned_abs)
        .max()
        .unwrap_or(0);
    assert!(peer_audio_peak > 1_000, "peer returned only silence");
    assert!(
        matched >= match_floor,
        "peer did not return the authenticated audio unchanged: matched {matched}/{} bytes",
        sent_payload.len()
    );

    let result = serde_json::json!({
        "suite_required": suite_name,
        "keying": "SDES",
        "encrypted": media.is_encrypted(),
        "negotiated_profile": format!("{:?}", media.srtp_profile().expect("profile")),
        "packets_sent": media.packets_sent(),
        "packets_received": media.packets_received(),
        "sent_audio_bytes": sent_payload.len(),
        "peer_echo_audio_bytes": echoed_payload.len(),
        "peer_audio_peak": peer_audio_peak,
        "longest_bit_exact_echo_bytes": matched,
        "match_floor_bytes": match_floor,
        "srtp_unprotect_failures": discards.srtp_unprotect_failures,
        "srtcp_unprotect_failures": discards.srtcp_unprotect_failures,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&result).expect("result JSON")
    );

    call.hang_up().await.expect("peer accepts BYE");
    assert!(call.is_ended(), "call ended locally");
}
