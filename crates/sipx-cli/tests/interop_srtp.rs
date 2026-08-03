//! A call whose media is encrypted, against an implementation that keyed it independently.
//!
//! `X-27`. Until this file, `grep -i "srtp\|savp\|dtls\|sdes"` over `tests/interop/` matched
//! nothing: the harness had placed calls against real peers since `X-17` and had never once done
//! it with encrypted media. That is why `M-25`'s defect shipped in six releases — `sipx-rtp`
//! sized the SRTP session authentication key at 94 octets where RFC 3711 §5.2 and §8.2 fix `n_a`
//! at 160 bits, so sipx derived a *different* HMAC key from the one every conformant peer
//! derives, and every tag failed in both directions on the first packet. All seventeen SRTP tests
//! passed throughout, because a round trip between two ends that are wrong the same way is a
//! round trip that works.
//!
//! What makes this test different in kind from those seventeen is whose opinion the assertions
//! are. The peer authenticates sipx's packets against a session key it derived from the master
//! key in the SDP *by its own reading of RFC 3711*, and it says out loud when that fails. So the
//! two claims below — that the peer rejected nothing, and that the audio it echoed is the audio
//! sipx sent — are both the far end's account of the call. Nothing here is sipx agreeing with
//! sipx.
//!
//! `#[ignore]`d, like the rest of the interop suite. `tests/interop/run.sh --peer asterisk`
//! starts a peer and runs it.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

mod interop_media;

use std::time::Duration;

use bytes::Bytes;
#[cfg(feature = "dtls")]
use sipx_call::Keying;
use sipx_call::{DialOptions, dial};
use sipx_sip::Uri;
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};

use interop_media::{addr_in, assert_echo, echo_round_trip, loopback};

/// Where the peer answers a call whose media is keyed with SDES, and who it thinks is calling.
///
/// Both are peer facts and both come from the profile. *Which* extension answers with encryption
/// and *which* identity selects an endpoint configured for it are things a peer decides, and a
/// second peer will decide them differently.
fn sdes_uri() -> String {
    std::env::var("SIPX_INTEROP_SDES_URI").unwrap_or_else(|_| "sip:echo@127.0.0.1:5061".to_owned())
}

fn sdes_from() -> String {
    std::env::var("SIPX_INTEROP_SDES_FROM")
        .unwrap_or_else(|_| "<sip:sipx-srtp@127.0.0.1>".to_owned())
}

#[cfg(feature = "dtls")]
fn dtls_uri() -> String {
    std::env::var("SIPX_INTEROP_DTLS_URI").unwrap_or_else(|_| "sip:echo@127.0.0.1:5060".to_owned())
}

#[cfg(feature = "dtls")]
fn dtls_from() -> String {
    std::env::var("SIPX_INTEROP_DTLS_FROM")
        .unwrap_or_else(|_| "<sip:sipx-dtls@127.0.0.1>".to_owned())
}

/// What this peer prints when it refuses a packet whose tag did not verify.
///
/// A peer fact, declared in the profile, because it is the peer's wording. It is also why the
/// assertion it feeds is worth making: a packet that fails SRTP authentication is *dropped*, so
/// a stack that only ever looked at its own copy of the audio would never learn that the far end
/// had thrown every packet away.
fn rejection_marker() -> String {
    std::env::var("SIPX_INTEROP_SRTP_REJECTED")
        .unwrap_or_else(|_| "SRTP unprotect failed".to_owned())
}

/// How to ask this peer for its own account of the media it carried. Diagnostic only.
fn media_report_command() -> String {
    std::env::var("SIPX_INTEROP_MEDIA_REPORT")
        .unwrap_or_else(|_| "asterisk -rx 'pjsip show channelstats'".to_owned())
}

/// The fixture authority the peer's certificate was issued by.
///
/// Trusting it is an *addition* to the anchor set, never a bypass — there is no way to say
/// "accept anything", so a mistake here produces a failed handshake rather than a test that
/// quietly proves nothing.
fn interop_anchors() -> sipx_transport::tls::TrustAnchors {
    let path = std::env::var("SIPX_INTEROP_CA").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/interop/asterisk/tls/ca.pem"
        )
        .to_owned()
    });
    let pem = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("{path}: {e}; run ./tests/interop/run.sh, which issues it"));
    let mut anchors = sipx_transport::tls::TrustAnchors::only();
    anchors.add_pem(&pem).expect("a usable fixture CA");
    anchors
}

fn container() -> String {
    std::env::var("SIPX_INTEROP_CONTAINER").unwrap_or_else(|_| {
        panic!("SIPX_INTEROP_CONTAINER is unset; run this through tests/interop/run.sh")
    })
}

/// Everything the peer has said so far, with the two streams kept apart.
///
/// Apart, and not concatenated, because this peer writes to **both** and a single offset into the
/// join is not a position in either. The first version of this test measured one length before
/// the call and sliced the join afterwards; the peer's complaint is on `stderr`, the offset landed
/// past it, and the assertion below read as satisfied on a call where every packet was refused.
/// That is this story's own failure mode, reproduced inside the test written to close it.
///
/// Read whole rather than searched through a pipe, for the reason `run.sh` gives at length: under
/// a shell's `pipefail`, `docker logs | grep -q` reports *failure on a match*.
#[derive(Default)]
struct PeerLog {
    out: String,
    err: String,
}

impl PeerLog {
    fn read() -> Self {
        let output = std::process::Command::new("docker")
            .args(["logs", &container()])
            .output()
            .expect("docker logs runs");
        Self {
            out: String::from_utf8_lossy(&output.stdout).into_owned(),
            err: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// What the peer has said since `mark`, on either stream.
    fn since(&self, mark: &Self) -> String {
        let tail = |whole: &str, seen: usize| {
            whole
                .get(seen.min(whole.len())..)
                .unwrap_or_default()
                .to_owned()
        };
        format!(
            "{}{}",
            tail(&self.out, mark.out.len()),
            tail(&self.err, mark.err.len())
        )
    }
}

/// The peer's own account of the media it carried, in whatever form it keeps it.
fn peer_media_report() -> String {
    let output = std::process::Command::new("docker")
        .args(["exec", &container(), "sh", "-c", &media_report_command()])
        .output()
        .expect("docker exec runs");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// `X-27`'s exit criterion: an implementation that did not learn SRTP from sipx accepts sipx's
/// encrypted packets.
///
/// Every assertion is about the far end. The negotiation assertion says the media really is
/// encrypted, so the case cannot pass by having quietly degraded into the cleartext call that is
/// already covered. The rejection assertion is the peer stating whether what arrived
/// authenticated. The audio assertion is the strongest of the three: the bytes compared came off
/// the peer's wire, and the peer could only have produced them by verifying sipx's tags and
/// decrypting the payload under a session key it derived for itself.
#[tokio::test]
#[ignore = "needs a user agent peer that keys media with SDES; see tests/interop/README.md"]
async fn a_real_peer_accepts_media_sipx_encrypted_with_sdes() {
    // Where the peer's account starts, so what is read afterwards is this call's and not an
    // earlier test's.
    let said_before = PeerLog::read();

    // SDES puts the master key in the SDP body, so sipx will not offer one over a path anyone
    // can read (RFC 4568 §7.1, and `Capabilities::with_srtp`'s `secure_signalling`). The
    // signalling is therefore TLS, and that is not an incidental detail: over UDP the offer would
    // be plain `RTP/AVP` and there would be no encrypted call to interoperate about.
    let mut config = TransportConfig::new("127.0.0.1:0".parse().expect("valid"));
    config.sent_by = loopback().to_string();
    config.tls_client =
        Some(sipx_transport::tls::ClientTls::new(&interop_anchors()).expect("a client"));
    let (handle, _incoming) = bind(config).await.expect("binds");

    let uri = sdes_uri();
    let to = Uri::parse(Bytes::from(uri.clone())).expect("a SIP URI");
    let target = Target::new(addr_in(&uri), TransportKind::Tls).verifying("sipx.test");
    let options = DialOptions::new(sdes_from(), loopback()).with_timeout(Duration::from_secs(15));

    let mut call = tokio::time::timeout(
        Duration::from_secs(20),
        dial(&handle, target, &to, &options),
    )
    .await
    .expect("the peer answers rather than leaving us ringing")
    .expect(
        "the peer accepts the encrypted call; it is configured to require SRTP, so a \
                 plain offer is refused rather than answered",
    );

    // The keying is the thing under test, so it is asserted rather than inferred from the audio.
    // Without this, a run in which sipx offered cleartext — a transport that was not secure, an
    // `a=crypto` the peer declined — would carry audio perfectly and read as a pass, which is
    // precisely the shape of failure this story exists to close.
    assert!(
        call.is_encrypted(),
        "the call connected with cleartext media; `RTP/SAVP` was offered and the answer did not \
         key it, so nothing here would have exercised SRTP at all"
    );

    let codec = call.media().codec();
    assert_eq!(
        codec.payload_type(),
        0,
        "the negotiation chose {codec:?}; the offer's first and the peer's configured codec is µ-law"
    );

    let (sent, echoed) = echo_round_trip(&call).await;

    // The peer's account, taken while the channel is still up so its media counters still exist.
    let report = peer_media_report();
    let since = PeerLog::read().since(&said_before);

    // The far end, in its own words, on whether sipx's packets authenticated. This is the line
    // `M-25`'s defect produced on the first packet of every call for six releases, and nothing in
    // this repository was ever in a position to read it.
    let rejected = rejection_marker();
    let complaints: Vec<&str> = since
        .lines()
        .filter(|line| line.contains(rejected.as_str()))
        .collect();
    assert!(
        complaints.is_empty(),
        "the peer refused sipx's encrypted packets — its words, not ours:\n  {}\n\nand its \
         account of the media:\n{report}",
        complaints.join("\n  ")
    );

    // The positive half, which no absence of a log line can give: these bytes came off the peer's
    // wire, so the peer read what sipx encrypted.
    assert_echo(&sent, &echoed, codec.payload_type());

    call.hang_up().await.expect("the BYE is accepted");
    assert!(call.is_ended(), "the call is over on our side too");
}

/// The same independent-peer proof through RFC 5763/5764 keying rather than an SDP master key.
#[cfg(feature = "dtls")]
#[tokio::test]
#[ignore = "needs a user agent peer that keys media with DTLS-SRTP; see tests/interop/README.md"]
async fn a_real_peer_accepts_media_sipx_encrypted_with_dtls_srtp() {
    let said_before = PeerLog::read();
    let (handle, _incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let uri = dtls_uri();
    let to = Uri::parse(Bytes::from(uri.clone())).expect("a SIP URI");
    let target = Target::udp(addr_in(&uri));
    let options = DialOptions::new(dtls_from(), loopback())
        .with_keying(Keying::DtlsSrtp)
        .with_timeout(Duration::from_secs(15));
    let mut call = tokio::time::timeout(
        Duration::from_secs(20),
        dial(&handle, target, &to, &options),
    )
    .await
    .expect("the peer answers within the call bound")
    .expect("the strict DTLS-SRTP endpoint accepts the call");

    assert!(
        call.is_encrypted(),
        "the DTLS-selected call fell back to cleartext"
    );
    let codec = call.media().codec();
    let (sent, echoed) = echo_round_trip(&call).await;
    let report = peer_media_report();
    let since = PeerLog::read().since(&said_before);
    let rejected = rejection_marker();
    let complaints: Vec<&str> = since
        .lines()
        .filter(|line| line.contains(rejected.as_str()))
        .collect();
    assert!(
        complaints.is_empty(),
        "the peer refused DTLS-keyed packets:\n  {}\n\npeer media report:\n{report}",
        complaints.join("\n  ")
    );
    assert_echo(&sent, &echoed, codec.payload_type());

    call.hang_up().await.expect("the BYE is accepted");
    assert!(call.is_ended());
}
