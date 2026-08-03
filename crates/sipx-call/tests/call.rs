//! A call, end to end: INVITE with SDP, G.711 audio, and BYE.
//!
//! This is milestone M3's exit criterion. Two sipx endpoints, real UDP sockets for signalling
//! and for media, a WAV played into the call and recorded at the far end, and an assertion on
//! the samples that came out.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
// `caller` and `callee` differ by two letters and are the names the RFCs, the industry and
// everyone reading this test already use. Renaming them to satisfy a similarity heuristic
// would make the test harder to read, not easier.
#![allow(clippy::similar_names)]

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::{Wav, g711, read_wav, write_wav};
use sipx_call::{Call, Credentials, answer, dial};
use sipx_sip::{CSeq, HeaderName, Host, HostName, Method, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use sipx_ua::{Authenticator, Presented, Verdict};
use tokio::sync::mpsc::Receiver;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// How long a test here waits for audio it played to arrive before calling it lost (`X-28`).
/// A bound on failure, not a window to measure in — see `MediaSession::record_at_least`.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// How long a test here waits for a signalling event — a request reaching the far end — before
/// concluding it is never coming (`X-29`). Two orders of magnitude above the honest answer on an
/// idle machine, for the same reason [`DELIVERY_BOUND`] is.
const SIGNALLING_BOUND: Duration = Duration::from_secs(10);

/// How long a collection here waits for the **first** digit before calling it lost (`M-34`).
/// A bound on failure, like [`DELIVERY_BOUND`]: when a caller presses the first key is a property
/// of the caller, never of the digits.
const FIRST_DIGIT_BOUND: Duration = Duration::from_secs(10);

/// How long a silence means the caller has stopped dialling (`M-34`). A definition of silence, so
/// it is set past any scheduling delay rather than close to the spacing the digits arrive with.
const DIGIT_GAP: Duration = Duration::from_secs(1);

/// Wait until something has happened, rather than sleeping and assuming it has (`X-29`).
///
/// `within` is a **bound on failure** — how long before we conclude the thing is never going to
/// happen — and not a window to measure in. `X-28` waited for a *quantity* of audio, which is why
/// counting worked there; these tests wait for an *event*, so the shape is a deadline loop on the
/// condition. Load can only lengthen the wait, and "it never arrived" fails with a message that
/// says so instead of flaking.
async fn until(within: Duration, what: &str, mut condition: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + within;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

/// A recognisable clip: a 440 Hz tone with an envelope, so a test that silently recorded
/// silence could not pass.
fn clip(milliseconds: usize) -> Wav {
    let samples = milliseconds * 8;
    Wav::narrowband(
        (0..samples)
            .map(|i| {
                let t = f64::from(u32::try_from(i).unwrap_or(0)) / 8000.0;
                let envelope = (t * 3.0).min(1.0);
                let value = (t * 440.0 * 2.0 * std::f64::consts::PI).sin() * 12000.0 * envelope;
                i16::try_from(value.round() as i32).unwrap_or(0)
            })
            .collect(),
    )
}

/// Set up a caller and a callee, connect them, and hand back both sides of the call.
async fn connected() -> (Call, Call) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        assert_eq!(incoming.request.method, Method::Invite);
        assert!(
            incoming
                .request
                .headers
                .typed::<sipx_sip::headers::Supported>()
                .and_then(std::result::Result::ok)
                .is_some_and(|tags| tags.contains("histinfo")),
            "a caller which wants the response history advertises histinfo"
        );
        assert_eq!(
            incoming
                .request
                .headers
                .typed::<sipx_sip::HistoryInfo>()
                .and_then(std::result::Result::ok)
                .map(|history| history.0.len()),
            Some(1),
            "the initial target starts at one history entry"
        );
        assert!(
            incoming.request.headers.get(&HeaderName::Reason).is_none(),
            "Reason is not permitted on an initial INVITE"
        );
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("the call connects");

    assert!(
        caller.history().is_some(),
        "the non-100 answer returns the requested history"
    );
    let callee = answering.await.expect("the answering side finishes");
    (caller, callee)
}

/// M3's exit criterion.
#[tokio::test]
async fn a_call_carries_audio_from_one_endpoint_to_the_other() {
    let (caller, callee) = connected().await;

    // Write the source out and read it back, so the test exercises the WAV path too rather
    // than only the samples in memory.
    let source = clip(300);
    let mut encoded = Vec::new();
    write_wav(&mut encoded, &source).expect("writes");
    let source = read_wav(encoded.as_slice()).expect("reads");

    let played = source.samples.clone();
    let recorded = tokio::join!(
        async {
            caller.media().play(&played, 160).await;
        },
        async {
            callee
                .media()
                .record_at_least(played.len(), DELIVERY_BOUND)
                .await
        }
    )
    .1;

    assert!(!recorded.is_empty(), "the callee heard nothing at all");
    assert_eq!(recorded.len(), source.samples.len(), "every sample arrived");

    // G.711 is lossy, so the samples cannot be compared directly. The codec is idempotent on
    // its own output, so encoding both sides must agree exactly — a stronger claim than
    // "close enough", and one that a dropped or reordered packet would break.
    assert_eq!(
        g711::ulaw_encode_all(&source.samples),
        g711::ulaw_encode_all(&recorded),
        "the audio that arrived is the audio that was played"
    );

    // And the recording is a real clip, not silence that happens to be the right length.
    let peak = recorded
        .iter()
        .map(|s| i32::from(s.abs()))
        .max()
        .unwrap_or(0);
    assert!(
        peak > 8000,
        "the recorded audio is too quiet to be the tone: peak {peak}"
    );
}

/// Both directions at once, which is what a call actually is.
#[tokio::test]
async fn audio_flows_in_both_directions() {
    let (caller, callee) = connected().await;

    let from_caller = clip(200).samples;
    let from_callee: Vec<i16> = clip(200).samples.iter().map(|s| -s).collect();

    let (heard_by_callee, heard_by_caller) = tokio::join!(
        async {
            let played = from_caller.clone();
            let (_played, recorded) = tokio::join!(
                caller.media().play(&played, 160),
                callee.media().record_at_least(played.len(), DELIVERY_BOUND)
            );
            recorded
        },
        async {
            let played = from_callee.clone();
            let (_played, recorded) = tokio::join!(
                callee.media().play(&played, 160),
                caller.media().record_at_least(played.len(), DELIVERY_BOUND)
            );
            recorded
        }
    );

    assert!(!heard_by_callee.is_empty(), "the callee heard nothing");
    assert!(!heard_by_caller.is_empty(), "the caller heard nothing");
    assert_ne!(
        g711::ulaw_encode_all(&heard_by_callee),
        g711::ulaw_encode_all(&heard_by_caller),
        "each side must hear the other, not its own audio looped back"
    );
}

/// The negotiation the call rests on: both sides agree on a codec and on where to send.
#[tokio::test]
async fn the_two_sides_agree_on_a_codec_and_a_media_port() {
    let (caller, callee) = connected().await;

    assert_ne!(
        caller.media().local_addr().port(),
        callee.media().local_addr().port(),
        "each side binds its own media port"
    );

    // A short exchange proves the ports and codec actually match, which the SDP alone does not.
    caller.media().play(&clip(60).samples, 160).await;
    let heard = callee.media().record_at_least(480, DELIVERY_BOUND).await;
    assert_eq!(heard.len(), 480, "60 ms is three packets");
}

/// A dialog established by the exchange, seen from both ends.
#[tokio::test]
async fn the_call_establishes_a_dialog_both_sides_agree_on() {
    let (caller, callee) = connected().await;

    assert_eq!(caller.dialog.id.call_id, callee.dialog.id.call_id);
    assert_eq!(
        caller.dialog.id.local_tag, callee.dialog.id.remote_tag,
        "the caller's tag is the callee's remote tag"
    );
    assert_eq!(caller.dialog.id.remote_tag, callee.dialog.id.local_tag);
    assert_eq!(caller.dialog.role, sipx_call::Role::Caller);
    assert_eq!(callee.dialog.role, sipx_call::Role::Callee);
}

/// Hanging up ends the call and releases the media.
#[tokio::test]
async fn hanging_up_ends_the_call() {
    let (mut caller, callee) = connected().await;

    caller.hang_up().await.expect("hangs up");

    // The media session is stopped: nothing more is delivered.
    let after = callee
        .media()
        .record_until_idle(Duration::from_millis(200))
        .await;
    let before = caller.media().packets_sent();
    caller.media().play(&clip(100).samples, 160).await;
    // A definition of silence: how long a hole has to be before "it stopped sending" is true.
    // The assertion is negative, so load lengthens the window and can only make it fail (`X-44`).
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        caller.media().packets_sent(),
        before,
        "a stopped session sends nothing"
    );
    assert!(after.is_empty(), "and the far end hears nothing more");
}

/// A call to somewhere that refuses is an error with the status in it, not a hang.
#[tokio::test]
async fn a_refused_call_reports_the_status() {
    let (busy_endpoint, mut busy_incoming) = endpoint().await;
    let busy_addr = busy_endpoint.local_addr();

    tokio::spawn(async move {
        while let Some(incoming) = busy_incoming.recv().await {
            let response = sipx_sip::build::ResponseBuilder::to_request(
                &incoming.request,
                sipx_sip::StatusCode::new(486).expect("valid"),
                "Busy Here",
            )
            .expect("builds")
            .build();
            let _ = busy_endpoint.respond(&incoming.key, response).await;
        }
    });

    let (caller_endpoint, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("busy.example").expect("valid")));
    let result = dial(
        &caller_endpoint,
        Target::udp(busy_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await;

    match result {
        Err(sipx_call::Error::Rejected { status, reason }) => {
            assert_eq!(status, 486);
            assert_eq!(reason, "Busy Here");
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
}

/// `S-28`'s failing-first test: a proxy challenge is a retryable step in placing one call, not
/// the final outcome. The wire assertions distinguish a retry from a second unrelated call.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the whole challenged transaction is one byte-level acceptance vector"
)]
async fn a_call_challenged_by_a_proxy_retries_with_credentials_and_connects() {
    const USERNAME: &str = "alice";
    const PASSWORD: &str = "Circle Of Life";

    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let answering = tokio::spawn(async move {
        let first = callee_incoming
            .recv()
            .await
            .expect("the first INVITE arrives");
        let first_call_id = first.request.headers.value(&HeaderName::CallId);
        let first_from = first.request.headers.value(&HeaderName::From);
        let first_via = first.request.headers.value(&HeaderName::Via);
        assert_eq!(
            first
                .request
                .headers
                .typed::<CSeq>()
                .and_then(Result::ok)
                .map(|cseq| cseq.sequence),
            Some(1)
        );

        let mut authenticator = Authenticator::new("proxy.example", [7; 32]);
        let challenged = sipx_sip::build::ResponseBuilder::to_request(
            &first.request,
            sipx_sip::StatusCode::new(407).expect("valid"),
            "Proxy Authentication Required",
        )
        .expect("builds")
        .set_header(
            &HeaderName::To,
            Bytes::from_static(b"<sip:callee.example>;tag=challenged"),
        )
        .expect("valid")
        .header(
            HeaderName::ProxyAuthenticate,
            Bytes::from(authenticator.challenge(false)),
        )
        .expect("valid")
        .build();
        callee_endpoint
            .respond(&first.key, challenged)
            .await
            .expect("challenges");

        let second = callee_incoming.recv().await.expect("the retry arrives");
        assert_eq!(
            second.request.headers.value(&HeaderName::CallId),
            first_call_id
        );
        assert_eq!(second.request.headers.value(&HeaderName::From), first_from);
        assert!(
            !second.request.body().is_empty(),
            "the retry keeps an SDP offer"
        );
        assert_ne!(
            second.request.headers.value(&HeaderName::Via),
            first_via,
            "a retried request is a new client transaction with a fresh branch"
        );
        assert_eq!(
            second
                .request
                .headers
                .typed::<CSeq>()
                .and_then(Result::ok)
                .map(|cseq| cseq.sequence),
            Some(2)
        );
        let presented = Presented::from_request(&second.request, true)
            .expect("the retry carries Proxy-Authorization");
        assert_eq!(presented.username, USERNAME);
        assert_eq!(
            authenticator.verify(&presented, "INVITE", PASSWORD),
            Verdict::Authenticated,
            "the digest does not answer the proxy's challenge"
        );
        answer(&callee_endpoint, &second, loopback())
            .await
            .expect("answers the authenticated call")
    });

    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:alice@example.net>", loopback())
            .with_credentials(Credentials::new(USERNAME, PASSWORD)),
    )
    .await
    .expect("the authenticated call connects");
    let callee = answering.await.expect("the answering side finishes");

    let sent = clip(100);
    let (_, heard) = tokio::join!(
        caller.media().play(&sent.samples, 160),
        callee
            .media()
            .record_at_least(sent.samples.len(), DELIVERY_BOUND)
    );
    assert_eq!(
        g711::ulaw_encode_all(&heard),
        g711::ulaw_encode_all(&sent.samples)
    );
}

/// The far end hanging up must stop our media. Without in-dialog routing, an incoming BYE
/// reaches nothing and the local session goes on sending RTP into a call that no longer
/// exists — which is worse than a call that never connects, because it does not stop.
#[tokio::test]
async fn a_bye_from_the_far_end_ends_the_call_locally() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let mut caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("connects");
    let mut callee = answering.await.expect("the answering side finishes");

    assert!(!caller.is_ended());
    callee.hang_up().await.expect("the callee hangs up");

    // The caller must see the BYE and act on it.
    let bye = tokio::time::timeout(Duration::from_secs(2), caller_incoming.recv())
        .await
        .expect("no timeout")
        .expect("a BYE arrives");
    assert_eq!(bye.request.method, sipx_sip::Method::Bye);
    let reason = bye
        .request
        .headers
        .typed::<sipx_sip::Reason>()
        .and_then(std::result::Result::ok)
        .expect("a locally generated BYE explains why it ended");
    assert_eq!(reason.0[0].protocol(), b"Q.850");
    assert_eq!(reason.0[0].cause(), 16);
    assert!(
        caller.handle(&bye).await.expect("handles"),
        "the BYE belongs to this call"
    );
    assert!(caller.is_ended(), "the call must be over");

    // And the media has stopped: nothing more goes out.
    let before = caller.media().packets_sent();
    caller.media().play(&clip(100).samples, 160).await;
    // A definition of silence, as in `hanging_up_ends_the_call` above: the assertion is negative,
    // so load lengthens the window and can only make it fail (`X-44`).
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        caller.media().packets_sent(),
        before,
        "a call ended by the far end must stop sending"
    );
}

/// A 2xx must be acknowledged even when the caller cannot use it. Walking away leaves the far
/// end retransmitting for 32 seconds and then streaming at a port we have closed.
#[tokio::test]
async fn a_2xx_the_caller_cannot_use_is_still_acknowledged() {
    let (answerer, mut answerer_incoming) = endpoint().await;
    let answerer_addr = answerer.local_addr();

    // Answer 200 OK with an SDP offering only a codec sipx does not implement, so the caller
    // gets a usable dialog and an unusable media stream.
    let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<sipx_sip::Method>::new()));
    let recorder = std::sync::Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(incoming) = answerer_incoming.recv().await {
            recorder.lock().await.push(incoming.request.method.clone());
            if incoming.request.method != sipx_sip::Method::Invite {
                continue;
            }
            let sdp = format!(
                "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
                 m=audio {} RTP/AVP 9\r\na=rtpmap:9 G722/8000\r\n",
                40404
            );
            let response = sipx_sip::build::ResponseBuilder::to_request(
                &incoming.request,
                sipx_sip::StatusCode::new(200).expect("valid"),
                "OK",
            )
            .expect("builds")
            .set_header(
                &HeaderName::To,
                Bytes::from_static(b"<sip:callee@example.com>;tag=theirs"),
            )
            .expect("valid")
            .header(
                HeaderName::Contact,
                Bytes::from(format!("<sip:sipx@{answerer_addr}>")),
            )
            .expect("valid")
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )
            .expect("valid")
            .body(Bytes::from(sdp))
            .build();
            let _ = answerer.respond(&incoming.key, response).await;
        }
    });

    let (caller_endpoint, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let result = dial(
        &caller_endpoint,
        Target::udp(answerer_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await;

    assert!(
        matches!(result, Err(sipx_call::Error::NoCommonCodec)),
        "G.722 alone is not usable: {result:?}"
    );

    // Wait for the teardown to reach the far end, rather than sleeping 300 ms and assuming it
    // did (`X-29`). The recorder is a task on the other side of a real socket, so there is no
    // happens-before to lean on here — only a bound on failure.
    until(
        SIGNALLING_BOUND,
        "the 2xx was never acknowledged and torn down",
        async || {
            let methods = seen.lock().await;
            methods.contains(&sipx_sip::Method::Ack) && methods.contains(&sipx_sip::Method::Bye)
        },
    )
    .await;
    let methods = seen.lock().await.clone();
    assert!(
        methods.contains(&sipx_sip::Method::Ack),
        "the 2xx must be acknowledged even though the call could not proceed: {methods:?}"
    );
    assert!(
        methods.contains(&sipx_sip::Method::Bye),
        "and then torn down, per RFC 3261 section 15: {methods:?}"
    );
}

/// The `Contact` must carry the endpoint's advertised address, not its socket's local one. An
/// endpoint bound to 0.0.0.0 would otherwise tell the peer to reach it at 0.0.0.0, and every
/// in-dialog request the peer sends becomes unroutable.
#[tokio::test]
async fn the_contact_advertises_the_configured_address() {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.sent_by = "sipx.example.net".to_owned();
    config.sent_by_port = Some(5080);
    let (handle, _rx) = sipx_transport::bind(config).await.expect("binds");

    let contact = sipx_call::call::contact_for(&handle, sipx_transport::TransportKind::Udp);
    assert_eq!(contact, "<sip:sipx@sipx.example.net:5080>");
    assert!(
        !contact.contains(&handle.local_addr().port().to_string()),
        "the socket's own port must not leak into the Contact: {contact}"
    );
}

/// DTMF across a real call, negotiated rather than assumed. sipx advertises
/// `telephone-event` in every offer it sends, so until this worked that advertisement was a
/// promise the stack did not keep.
#[tokio::test]
async fn a_call_carries_dtmf_digits() {
    let (caller, callee) = connected().await;

    // Establish the media path first: symmetric RTP has to learn where the caller is.
    caller.media().play(&clip(60).samples, 160).await;
    let _ = callee.media().record_at_least(480, DELIVERY_BOUND).await;

    caller.send_digits("1234#", Duration::from_millis(80)).await;

    let collected = callee
        .media()
        .collect_digits(FIRST_DIGIT_BOUND, DIGIT_GAP)
        .await;
    assert_eq!(collected, "1234#");
}

/// The payload type comes from the answer, not from an assumption. 101 is what sipx offers,
/// not what everyone uses, and sending keypresses on a number the far end put to another
/// purpose is worse than not sending them.
#[tokio::test]
async fn the_dtmf_payload_type_is_taken_from_the_negotiation() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _rx) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    // The callee answers with telephone-event on 96 rather than 101.
    tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
             m=audio 41234 RTP/AVP 0 96\r\n\
             a=rtpmap:0 PCMU/8000\r\n\
             a=rtpmap:96 telephone-event/8000\r\n\
             a=sendrecv\r\n";
        let response = sipx_sip::build::ResponseBuilder::to_request(
            &incoming.request,
            sipx_sip::StatusCode::new(200).expect("valid"),
            "OK",
        )
        .expect("builds")
        .set_header(
            &HeaderName::To,
            Bytes::from_static(b"<sip:callee@example.com>;tag=theirs"),
        )
        .expect("valid")
        .header(
            HeaderName::Contact,
            Bytes::from(format!("<sip:sipx@{callee_addr}>")),
        )
        .expect("valid")
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )
        .expect("valid")
        .body(Bytes::from(sdp))
        .build();
        let _ = callee_endpoint.respond(&incoming.key, response).await;
    });

    // A raw socket standing in for the far end's media port, so the payload type is visible.
    let far_media = tokio::net::UdpSocket::bind("127.0.0.1:41234")
        .await
        .expect("binds the port the answer names");

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("connects");

    caller
        .send_digit(
            sipx_rtp::Digit::from_char('9').expect("a digit"),
            Duration::from_millis(80),
        )
        .await;

    let mut datagram = vec![0u8; 2048];
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), far_media.recv_from(&mut datagram))
        .await
        .expect("no timeout")
        .expect("a packet");
    let packet = sipx_rtp::Packet::decode(&Bytes::copy_from_slice(&datagram[..len]))
        .expect("a valid RTP packet");
    assert_eq!(
        packet.payload_type, 96,
        "the digit must go out on the payload type the answer named, not on 101"
    );
}

/// Set up a call where the caller side is driven by a task that routes in-dialog requests, so
/// re-INVITEs from the callee reach the caller's `Call`.
async fn connected_with_routing() -> (
    std::sync::Arc<tokio::sync::Mutex<Call>>,
    Call,
    tokio::task::JoinHandle<()>,
    std::net::SocketAddr,
) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, mut caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE arrives");
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let caller_addr = caller_endpoint.local_addr();
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &to,
        &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
    )
    .await
    .expect("connects");
    let callee = answering.await.expect("the answering side finishes");

    let caller = std::sync::Arc::new(tokio::sync::Mutex::new(caller));
    let routed = std::sync::Arc::clone(&caller);
    let pump = tokio::spawn(async move {
        while let Some(incoming) = caller_incoming.recv().await {
            let _ = routed.lock().await.handle(&incoming).await;
        }
    });

    (caller, callee, pump, caller_addr)
}

/// The acceptance test for M-8: a re-INVITE renegotiates a running call.
#[tokio::test]
async fn a_reinvite_moves_the_media_without_dropping_the_call() {
    let (caller, mut callee, pump, _) = connected_with_routing().await;

    let port_before = caller.lock().await.media().local_addr().port();

    // The callee re-offers, moving its own media port.
    callee
        .reinvite(sipx_sdp::Direction::SendRecv)
        .await
        .expect("the re-INVITE is accepted");

    // No wait at all, because there is already a happens-before to lean on (`X-29`). `reinvite`
    // returns only once the 200 has come back, and the caller applies the renegotiation *before*
    // it responds (`call.rs::on_reinvite`) — inside a `handle` call the pump holds this mutex
    // across. So acquiring the lock here means the re-INVITE has been applied; the 200 cannot
    // exist otherwise. A sleep was never what made this true, only what hid that it already was.
    let caller = caller.lock().await;
    assert!(!caller.is_ended(), "the call must still be running");
    assert_eq!(
        caller.media().local_addr().port(),
        port_before,
        "our own receive port does not move just because theirs did"
    );

    // And audio still flows after the renegotiation.
    drop(caller);
    pump.abort();
}

/// Hold and resume, which is what a re-INVITE is mostly used for.
#[tokio::test]
async fn a_reinvite_can_put_the_call_on_hold_and_take_it_off() {
    let (caller, mut callee, pump, _) = connected_with_routing().await;

    callee
        .reinvite(sipx_sdp::Direction::SendOnly)
        .await
        .expect("hold is accepted");
    // Both waits here are gone rather than converted, for the reason given in
    // `a_reinvite_moves_the_media_without_dropping_the_call`: `reinvite` returning means the far
    // end answered 200, and it answers only after applying the direction (`X-29`).
    assert!(
        caller.lock().await.is_on_hold(),
        "sendonly from the far end means it will not play what we send"
    );

    callee
        .reinvite(sipx_sdp::Direction::SendRecv)
        .await
        .expect("resume is accepted");
    assert!(
        !caller.lock().await.is_on_hold(),
        "sendrecv takes it off hold"
    );

    pump.abort();
}

/// A renegotiation that cannot be answered must be refused with 488 and **leave the call
/// running**. Tearing it down would lose a call that was working a moment earlier.
#[tokio::test]
async fn a_reinvite_that_cannot_be_answered_is_refused_and_the_call_survives() {
    let (caller, _callee, pump, caller_addr) = connected_with_routing().await;

    // Build a re-INVITE inside the established dialog, offering only a codec sipx cannot
    // carry. The tags swap, because this request travels the other way.
    let (to, from, call_id) = {
        let guard = caller.lock().await;
        let (local, remote) = guard.dialog.local_and_remote();
        // `local` is the caller's own address of record, so from the far end's point of view
        // it is the `To`.
        (local, remote, guard.dialog.id.call_id.clone())
    };

    let sdp = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
         m=audio 45000 RTP/AVP 9\r\na=rtpmap:9 G722/8000\r\na=sendrecv\r\n";
    let reinvite = sipx_sip::build::RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("caller.example").expect("valid"))),
    )
    .header(HeaderName::To, Bytes::from(to))
    .expect("valid")
    .header(HeaderName::From, Bytes::from(from))
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from(call_id))
    .expect("valid")
    .cseq(99, &Method::Invite)
    .expect("valid")
    .header(
        HeaderName::ContentType,
        Bytes::from_static(b"application/sdp"),
    )
    .expect("valid")
    .max_forwards(70)
    .body(Bytes::from(sdp))
    .build();

    let (prober, _rx) = endpoint().await;
    let mut responses = prober
        .send(reinvite, Target::udp(caller_addr))
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(3), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");

    assert_eq!(
        response.status.code(),
        488,
        "an unusable offer is Not Acceptable Here, not a teardown"
    );
    assert!(
        !caller.lock().await.is_ended(),
        "the call the renegotiation failed on must still be running"
    );

    pump.abort();
}

/// A re-INVITE whose sequence number is not greater than the last one is out of order.
/// Applying it would let a delayed packet undo a later change.
#[tokio::test]
async fn an_out_of_order_reinvite_is_rejected() {
    let (caller, mut callee, pump, caller_addr) = connected_with_routing().await;

    // A legitimate renegotiation first, which advances the remote sequence number.
    callee
        .reinvite(sipx_sdp::Direction::SendOnly)
        .await
        .expect("accepted");
    // The sequence number this test needs advanced is already advanced (`X-29`).
    // `on_reinvite` records the remote CSeq before it answers, and `reinvite` returns only once
    // that answer is back — so there is nothing left to wait for, and the 150 ms sleep that used
    // to stand here was a guess at a happens-before that the exchange already provides.

    let (to, from, call_id) = {
        let guard = caller.lock().await;
        let (local, remote) = guard.dialog.local_and_remote();
        (local, remote, guard.dialog.id.call_id.clone())
    };

    // Now one with sequence number 1, which is behind whatever the call is at.
    let stale = sipx_sip::build::RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("caller.example").expect("valid"))),
    )
    .header(HeaderName::To, Bytes::from(to))
    .expect("valid")
    .header(HeaderName::From, Bytes::from(from))
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from(call_id))
    .expect("valid")
    .cseq(1, &Method::Invite)
    .expect("valid")
    .header(
        HeaderName::ContentType,
        Bytes::from_static(b"application/sdp"),
    )
    .expect("valid")
    .max_forwards(70)
    .body(Bytes::from_static(
        b"v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
          m=audio 46000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
    ))
    .build();

    let (prober, _rx) = endpoint().await;
    let mut responses = prober
        .send(stale, Target::udp(caller_addr))
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(3), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a final response");

    assert_eq!(response.status.code(), 500, "out of order, not applied");
    assert!(!caller.lock().await.is_ended());
    pump.abort();
}

/// Giving up on an unanswered call must tell the far end. Without a CANCEL the callee goes on
/// ringing, and someone answering afterwards ends up in a call with a party that has left.
#[tokio::test]
async fn giving_up_cancels_the_invitation() {
    let (ringing, mut incoming) = endpoint().await;
    let ringing_addr = ringing.local_addr();

    // A callee that rings and never answers, recording what it is sent.
    let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<sipx_sip::Method>::new()));
    let recorder = std::sync::Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            recorder.lock().await.push(request.request.method.clone());
            // 180 Ringing, and nothing more.
            if request.request.method == Method::Invite {
                let response = sipx_sip::build::ResponseBuilder::to_request(
                    &request.request,
                    sipx_sip::StatusCode::new(180).expect("valid"),
                    "Ringing",
                )
                .expect("builds")
                .build();
                let _ = ringing.respond(&request.key, response).await;
            }
        }
    });

    let (caller, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("ringing.example").expect("valid")));
    let options = sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
        .with_timeout(Duration::from_millis(400));

    let result = dial(&caller, Target::udp(ringing_addr), &to, &options).await;
    assert!(
        matches!(result, Err(sipx_call::Error::Cancelled(_))),
        "expected a cancellation, got {result:?}"
    );

    // Wait for the CANCEL to reach the ringing callee rather than sleeping past it (`X-29`).
    until(
        SIGNALLING_BOUND,
        "the callee was never sent a CANCEL",
        async || seen.lock().await.contains(&Method::Cancel),
    )
    .await;
    let methods = seen.lock().await.clone();
    assert!(
        methods.contains(&Method::Cancel),
        "the callee must be told to stop ringing: {methods:?}"
    );
}

/// The CANCEL carries the INVITE's own branch. That identity is what matches the two at the far
/// end; a fresh branch cancels nothing and the callee keeps ringing.
#[tokio::test]
async fn the_cancel_carries_the_invites_branch() {
    let (ringing, mut incoming) = endpoint().await;
    let ringing_addr = ringing.local_addr();

    let branches = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<(
        sipx_sip::Method,
        String,
        Option<(Vec<u8>, u16)>,
    )>::new()));
    let recorder = std::sync::Arc::clone(&branches);
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            let via = request
                .request
                .headers
                .typed::<sipx_sip::headers::Via>()
                .and_then(std::result::Result::ok)
                .and_then(|via| {
                    via.branch()
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                })
                .unwrap_or_default();
            recorder.lock().await.push((
                request.request.method.clone(),
                via,
                request
                    .request
                    .headers
                    .typed::<sipx_sip::Reason>()
                    .and_then(std::result::Result::ok)
                    .and_then(|reason| {
                        reason
                            .0
                            .first()
                            .map(|value| (value.protocol().to_vec(), value.cause()))
                    }),
            ));
            if request.request.method == Method::Invite {
                let response = sipx_sip::build::ResponseBuilder::to_request(
                    &request.request,
                    sipx_sip::StatusCode::new(180).expect("valid"),
                    "Ringing",
                )
                .expect("builds")
                .build();
                let _ = ringing.respond(&request.key, response).await;
            }
        }
    });

    let (caller, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("ringing.example").expect("valid")));
    let options = sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
        .with_timeout(Duration::from_millis(400));
    let _ = dial(&caller, Target::udp(ringing_addr), &to, &options).await;

    // Both requests have to be on the recorder's list before their branches can be compared, so
    // wait for the second of them rather than sleeping past both (`X-29`).
    until(
        SIGNALLING_BOUND,
        "the INVITE and its CANCEL never both reached the callee",
        async || {
            let seen = branches.lock().await;
            seen.iter().any(|(method, _, _)| *method == Method::Invite)
                && seen.iter().any(|(method, _, _)| *method == Method::Cancel)
        },
    )
    .await;
    let seen = branches.lock().await.clone();

    let invite = seen
        .iter()
        .find(|(method, _, _)| *method == Method::Invite)
        .map(|(_, branch, _)| branch.clone())
        .expect("an INVITE was sent");
    let cancel = seen
        .iter()
        .find(|(method, _, _)| *method == Method::Cancel)
        .map(|(_, branch, _)| branch.clone())
        .expect("a CANCEL was sent");

    assert!(!invite.is_empty(), "the INVITE must carry a branch");
    assert_eq!(
        cancel, invite,
        "a CANCEL with a different branch cancels nothing"
    );
    let cancel_reason = seen
        .iter()
        .find(|(method, _, _)| *method == Method::Cancel)
        .and_then(|(_, _, reason)| reason.clone())
        .expect("a locally generated CANCEL explains why it was sent");
    assert_eq!(cancel_reason, (b"SIP".to_vec(), 408));
}

/// Waiting is still the default: without a timeout, a call is bounded only by the transaction
/// layer, and nothing is cancelled early.
#[tokio::test]
async fn without_a_timeout_the_call_is_not_cancelled() {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("an INVITE");
        // Answer slowly, but well within the transaction's patience.
        tokio::time::sleep(Duration::from_millis(300)).await;
        answer(&callee_endpoint, &incoming, loopback())
            .await
            .expect("answers")
    });

    let (caller_endpoint, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("slow.example").expect("valid")));
    let options = sipx_call::DialOptions::new("<sip:caller@example.net>", loopback());

    let call = dial(&caller_endpoint, Target::udp(callee_addr), &to, &options)
        .await
        .expect("a slow answer is still an answer");
    assert!(!call.is_ended());
    let _ = answering.await;
}

/// RFC 3261 §12.2.1.1: with a route set, an in-dialog request is *sent to* the first `Route`
/// entry, not to the remote target. That entry is the proxy which record-routed itself into
/// the dialog precisely so it would see the rest of it — and behind a NAT or on a separate
/// segment it is the only element that can reach the far end at all. Addressing the request to
/// the peer's `Contact` and putting the routes in as headers produces the BYE that never
/// arrives: the call cannot be hung up, and the media keeps flowing.
#[tokio::test]
async fn an_in_dialog_request_goes_to_the_first_route_not_the_remote_target() {
    use tokio::net::UdpSocket;

    // Stands in for a record-routing proxy: it only has to exist and be listened on.
    let proxy = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let proxy_addr = proxy.local_addr().expect("has an address");

    // And an address the callee claims in its Contact, which nothing listens on. A request
    // sent there is lost, which is exactly the production failure this reproduces.
    let unreachable = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let contact_addr = unreachable.local_addr().expect("has an address");
    drop(unreachable);

    let callee = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let callee_addr = callee.local_addr().expect("has an address");

    let (caller_endpoint, _caller_incoming) = endpoint().await;

    // A raw callee: answer the INVITE with a 200 that record-routes through the proxy.
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        let (len, from) = callee.recv_from(&mut buf).await.expect("an INVITE");
        let invite = String::from_utf8_lossy(&buf[..len]).into_owned();
        let header = |name: &str| {
            invite
                .lines()
                .find(|line| {
                    line.to_ascii_lowercase()
                        .starts_with(&name.to_ascii_lowercase())
                })
                .unwrap_or_default()
                .to_owned()
        };
        let sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
             m=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n",
            callee_addr.port()
        );
        let response = format!(
            "SIP/2.0 200 OK\r\n{}\r\nRecord-Route: <sip:127.0.0.1:{};lr>\r\n{}\r\n{};tag=callee\r\n{}\r\n{}\r\n\
             Contact: <sip:callee@127.0.0.1:{}>\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
            header("Via:"),
            proxy_addr.port(),
            header("From:"),
            header("To:"),
            header("Call-ID:"),
            header("CSeq:"),
            contact_addr.port(),
            sdp.len(),
            sdp
        );
        let _ = callee.send_to(response.as_bytes(), from).await;
    });

    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let dialing = tokio::spawn(async move {
        dial(
            &caller_endpoint,
            Target::udp(callee_addr),
            &to,
            &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
    });

    // The ACK is the first in-dialog request, and it must arrive at the proxy.
    let mut buf = vec![0u8; 8192];
    let received = tokio::time::timeout(Duration::from_secs(3), proxy.recv_from(&mut buf))
        .await
        .expect("the ACK must be sent to the route set's first hop, not the remote target");
    let (len, _) = received.expect("reads");
    let ack = String::from_utf8_lossy(&buf[..len]).into_owned();
    assert!(ack.starts_with("ACK "), "the proxy sees the ACK: {ack}");
    assert!(
        ack.contains(&format!("Route: <sip:127.0.0.1:{};lr>", proxy_addr.port())),
        "and it still carries the route set: {ack}"
    );

    let _ = dialing.await;
}

/// RFC 3261 §9.1: "If no provisional response has been received, the CANCEL request MUST NOT
/// be sent; rather, the client MUST wait for the arrival of a provisional response before
/// sending the request." A CANCEL sent before the far end has said anything can overtake the
/// INVITE it refers to, matching no transaction there while the INVITE goes on to ring.
#[tokio::test]
async fn a_cancel_is_not_sent_before_a_provisional_arrives() {
    use tokio::net::UdpSocket;

    // A raw socket, not a sipx endpoint: a server transaction answers 100 Trying of its own
    // accord when the application is slow (RFC 3261 §17.2.1), and that provisional would make
    // the invitation legitimately cancellable. This peer says nothing whatsoever.
    let silent = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let silent_addr = silent.local_addr().expect("has an address");

    let seen = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<String>::new()));
    let recorder = std::sync::Arc::clone(&seen);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        while let Ok((len, _)) = silent.recv_from(&mut buf).await {
            let text = String::from_utf8_lossy(&buf[..len]);
            if let Some(line) = text.lines().next() {
                recorder.lock().await.push(line.to_owned());
            }
        }
    });

    let (caller, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("silent.example").expect("valid")));
    let options = sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
        .with_timeout(Duration::from_millis(300));

    let result = dial(&caller, Target::udp(silent_addr), &to, &options).await;
    assert!(
        matches!(result, Err(sipx_call::Error::Cancelled(_))),
        "the dial still gives up: {result:?}"
    );

    // This site has one assertion of each kind, so it keeps one wait of each kind (`X-29`).
    //
    // The window stays for the *negative* half below — that no CANCEL was sent. A CANCEL would
    // leave at the moment the dial gave up, which has already happened, so this is a definition
    // of silence rather than a deadline to beat: load can only make it pass, and the failure mode
    // is a missed regression rather than a flake.
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The *positive* half — that the invitation was sent at all — is an arrival, so it gets a
    // deadline loop on the condition. No flake was ever observed here; the shape is the argument,
    // not a measurement.
    until(
        SIGNALLING_BOUND,
        "the invitation never reached the silent peer",
        async || {
            seen.lock()
                .await
                .iter()
                .any(|line| line.starts_with("INVITE "))
        },
    )
    .await;
    let lines = seen.lock().await.clone();
    assert!(
        lines.iter().any(|line| line.starts_with("INVITE ")),
        "the invitation was sent: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.starts_with("CANCEL ")),
        "nothing may be cancelled before the far end has answered provisionally: {lines:?}"
    );
}

/// RFC 3261 §12.2.2: "When a UAS receives a target refresh request, it MUST replace the
/// dialog's remote target URI with the URI from the Contact header field in that request".
/// Keeping the original sends every later request — the BYE above all — to where the peer used
/// to be, so a peer that re-homes mid-call can never be told the call is over.
#[test]
fn a_target_refresh_moves_the_remote_target() {
    use sipx_sip::build::RequestBuilder;

    let invite = RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("example.com").expect("valid"))),
    )
    .header(HeaderName::To, "<sip:callee@example.com>")
    .expect("valid")
    .header(HeaderName::From, "<sip:caller@example.net>;tag=abc")
    .expect("valid")
    .header(HeaderName::CallId, "refresh@example.net")
    .expect("valid")
    .cseq(1, &Method::Invite)
    .expect("valid")
    .header(HeaderName::Contact, "<sip:caller@192.0.2.10:5060>")
    .expect("valid")
    .max_forwards(70)
    .build();

    let mut dialog = sipx_call::Dialog::from_request(&invite, "tag").expect("a dialog");
    assert_eq!(
        String::from_utf8_lossy(&dialog.remote_target.to_bytes()),
        "sip:caller@192.0.2.10:5060"
    );

    let moved = RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("example.com").expect("valid"))),
    )
    .header(HeaderName::Contact, "<sip:caller@198.51.100.7:5080>")
    .expect("valid")
    .build();
    dialog.refresh_target(&moved.headers);
    assert_eq!(
        String::from_utf8_lossy(&dialog.remote_target.to_bytes()),
        "sip:caller@198.51.100.7:5080",
        "the dialog follows the peer to its new contact"
    );
}

/// RFC 3261 §12.2.1.1's two forms. A loose router leaves the Request-URI alone and travels in
/// `Route`; a strict router takes the Request-URI, and the remote target moves to the end of
/// the route set — otherwise the far end is handed a request addressed to a proxy.
#[test]
fn a_strict_route_set_rewrites_the_request_uri() {
    use sipx_sip::build::RequestBuilder;

    let with_route = |route: &'static str| {
        let invite = RequestBuilder::new(
            Method::Invite,
            Uri::sip(Host::Name(HostName::new("example.com").expect("valid"))),
        )
        .header(HeaderName::To, "<sip:callee@example.com>")
        .expect("valid")
        .header(HeaderName::From, "<sip:caller@example.net>;tag=abc")
        .expect("valid")
        .header(HeaderName::CallId, "routing@example.net")
        .expect("valid")
        .cseq(1, &Method::Invite)
        .expect("valid")
        .header(HeaderName::Contact, "<sip:caller@192.0.2.10:5060>")
        .expect("valid")
        .header(HeaderName::RecordRoute, route)
        .expect("valid")
        .max_forwards(70)
        .build();
        sipx_call::Dialog::from_request(&invite, "tag").expect("a dialog")
    };

    let loose = with_route("<sip:proxy.example;lr>");
    let (uri, routes) = loose.request_target();
    assert_eq!(
        String::from_utf8_lossy(&uri.to_bytes()),
        "sip:caller@192.0.2.10:5060",
        "a loose router leaves the Request-URI addressed to the peer"
    );
    assert_eq!(routes, vec!["<sip:proxy.example;lr>".to_owned()]);

    let strict = with_route("<sip:proxy.example>");
    let (uri, routes) = strict.request_target();
    assert_eq!(
        String::from_utf8_lossy(&uri.to_bytes()),
        "sip:proxy.example",
        "a strict router takes the Request-URI"
    );
    assert_eq!(
        routes,
        vec!["<sip:caller@192.0.2.10:5060>".to_owned()],
        "and the remote target becomes the last route"
    );
}

/// RFC 3261 §13.3.1.4 governs the 2xx to *any* INVITE, the re-INVITE included: it is
/// retransmitted on the T1 backoff until the ACK arrives. The server transaction deliberately
/// absorbs retransmitted INVITEs without answering them again (RFC 6026), so if the transaction
/// user does not resend, a single lost 200 deadlocks the renegotiation until the peer's Timer B
/// — one dropped packet breaking hold and resume for half a minute.
#[tokio::test]
async fn a_reinvite_200_is_retransmitted_until_it_is_acked() {
    use tokio::net::UdpSocket;

    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();

    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let sdp = |port: u16| {
        format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
             m=audio {port} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"
        )
    };
    let invite = |cseq: u32, branch: &str, body: &str| {
        format!(
            "INVITE sip:callee@127.0.0.1 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 127.0.0.1:{};branch={branch}\r\n\
             To: <sip:callee@127.0.0.1>\r\nFrom: <sip:peer@127.0.0.1>;tag=peertag\r\n\
             Call-ID: reinvite-retransmit@example.net\r\nCSeq: {cseq} INVITE\r\n\
             Contact: <sip:peer@127.0.0.1:{}>\r\nMax-Forwards: 70\r\n\
             Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{body}",
            peer_addr.port(),
            peer_addr.port(),
            body.len()
        )
    };

    // Establish the call, so the callee holds a real dialog.
    let first = invite(1, "z9hG4bKfirst", &sdp(40000));
    peer.send_to(first.as_bytes(), callee_addr)
        .await
        .expect("sends");

    let incoming = tokio::time::timeout(Duration::from_secs(2), callee_incoming.recv())
        .await
        .expect("no timeout")
        .expect("an INVITE");
    let mut callee = answer(&callee_endpoint, &incoming, loopback())
        .await
        .expect("answers");

    let mut buf = vec![0u8; 8192];
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
        .await
        .expect("no timeout")
        .expect("reads");
    let ok = String::from_utf8_lossy(&buf[..len]).into_owned();
    assert!(ok.starts_with("SIP/2.0 200"), "{ok}");
    let to_line = ok
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("to:"))
        .unwrap_or_default()
        .to_owned();

    let ack = format!(
        "ACK sip:callee@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{};branch=z9hG4bKack\r\n\
         {to_line}\r\nFrom: <sip:peer@127.0.0.1>;tag=peertag\r\n\
         Call-ID: reinvite-retransmit@example.net\r\nCSeq: 1 ACK\r\n\
         Max-Forwards: 70\r\nContent-Length: 0\r\n\r\n",
        peer_addr.port()
    );
    peer.send_to(ack.as_bytes(), callee_addr)
        .await
        .expect("sends");

    // Pump in-dialog requests into the call, which is what a real application does.
    //
    // No wait after this. The channel is the happens-before: `callee_incoming` is an mpsc
    // receiver, so a request that arrives before this task is first polled is queued and delivered
    // when it is polled, and nothing is lost by starting the pump late. The 100 ms sleep that used
    // to sit here bought nothing (`X-44`).
    tokio::spawn(async move {
        while let Some(incoming) = callee_incoming.recv().await {
            let _ = callee.handle(&incoming).await;
        }
    });

    // A re-INVITE, whose 200 this peer deliberately never acknowledges.
    let renegotiate = format!(
        "INVITE sip:callee@127.0.0.1 SIP/2.0\r\n\
         Via: SIP/2.0/UDP 127.0.0.1:{};branch=z9hG4bKsecond\r\n\
         {to_line}\r\nFrom: <sip:peer@127.0.0.1>;tag=peertag\r\n\
         Call-ID: reinvite-retransmit@example.net\r\nCSeq: 2 INVITE\r\n\
         Contact: <sip:peer@127.0.0.1:{}>\r\nMax-Forwards: 70\r\n\
         Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",
        peer_addr.port(),
        peer_addr.port(),
        sdp(40002).len(),
        sdp(40002)
    );
    peer.send_to(renegotiate.as_bytes(), callee_addr)
        .await
        .expect("sends");

    let mut answers = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, peer.recv_from(&mut buf)).await {
            Ok(Ok((len, _))) => {
                let text = String::from_utf8_lossy(&buf[..len]);
                if text.starts_with("SIP/2.0 200") && text.contains("CSeq: 2 INVITE") {
                    answers += 1;
                    if answers >= 2 {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    assert!(
        answers >= 2,
        "the 200 to a re-INVITE must be retransmitted until acknowledged; saw {answers}"
    );
}

/// RFC 3261 §12.2.2 applies to every in-dialog request, not only the ones that renegotiate. A
/// BYE from behind the dialog's current sequence number is a stale duplicate — one that
/// outlived the transaction layer's absorption window, or an injected one — and honouring it
/// ends a call that is still running.
#[tokio::test]
async fn a_stale_bye_is_rejected_rather_than_ending_the_call() {
    let (caller, mut callee, pump, caller_addr) = connected_with_routing().await;

    // A legitimate renegotiation first, which advances the remote sequence number.
    callee
        .reinvite(sipx_sdp::Direction::SendOnly)
        .await
        .expect("accepted");
    // The sequence number this test needs advanced is already advanced (`X-29`).
    // `on_reinvite` records the remote CSeq before it answers, and `reinvite` returns only once
    // that answer is back — so there is nothing left to wait for, and the 150 ms sleep that used
    // to stand here was a guess at a happens-before that the exchange already provides.

    let (to, from, call_id) = {
        let guard = caller.lock().await;
        let (local, remote) = guard.dialog.local_and_remote();
        (local, remote, guard.dialog.id.call_id.clone())
    };

    let stale = sipx_sip::build::RequestBuilder::new(
        Method::Bye,
        Uri::sip(Host::Name(HostName::new("caller.example").expect("valid"))),
    )
    .header(HeaderName::To, Bytes::from(to))
    .expect("valid")
    .header(HeaderName::From, Bytes::from(from))
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from(call_id))
    .expect("valid")
    .cseq(1, &Method::Bye)
    .expect("valid")
    .max_forwards(70)
    .build();

    let (sender, _rx) = endpoint().await;
    let mut responses = sender
        .send(stale, Target::udp(caller_addr))
        .await
        .expect("sends");
    let response = tokio::time::timeout(Duration::from_secs(2), responses.final_response())
        .await
        .expect("no timeout")
        .expect("a response");
    assert_eq!(
        response.status.code(),
        500,
        "a stale BYE is rejected, not obeyed"
    );
    assert!(
        !caller.lock().await.is_ended(),
        "and the call it tried to end is still running"
    );

    pump.abort();
}

/// RFC 3261 §13.2.2.4: the UAC core "MUST generate an ACK for each 2xx received". A
/// retransmitted 2xx means the previous ACK was lost, and only the UAC core can answer it —
/// the INVITE transaction has already passed the response up. Acknowledging only the first
/// leaves the far end retransmitting for 64*T1 and then tearing down, from its side, a call
/// this side believes is established and is already sending audio into.
#[tokio::test]
async fn a_retransmitted_2xx_is_acknowledged_again() {
    use tokio::net::UdpSocket;

    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let (caller_endpoint, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("peer.example").expect("valid")));
    let dialing = tokio::spawn(async move {
        dial(
            &caller_endpoint,
            Target::udp(peer_addr),
            &to,
            &sipx_call::DialOptions::new("<sip:caller@example.net>", loopback()),
        )
        .await
    });

    let mut buf = vec![0u8; 8192];
    let (len, from) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
        .await
        .expect("no timeout")
        .expect("an INVITE");
    let invite = String::from_utf8_lossy(&buf[..len]).into_owned();
    let header = |name: &str| {
        invite
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with(&name.to_ascii_lowercase())
            })
            .unwrap_or_default()
            .to_owned()
    };
    let sdp = "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\n\
               m=audio 41000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n";
    let ok = format!(
        "SIP/2.0 202 Accepted\r\n{}\r\n{}\r\n{};tag=peertag\r\n{}\r\n{}\r\n\
         Contact: <sip:peer@127.0.0.1:{}>\r\nContent-Type: application/sdp\r\n\
         Content-Length: {}\r\n\r\n{sdp}",
        header("Via:"),
        header("From:"),
        header("To:"),
        header("Call-ID:"),
        header("CSeq:"),
        peer_addr.port(),
        sdp.len()
    );

    // Answer, and then answer again as a peer whose ACK never arrived would.
    peer.send_to(ok.as_bytes(), from).await.expect("sends");
    let mut acks = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut resent = false;
    while tokio::time::Instant::now() < deadline && acks < 2 {
        match tokio::time::timeout_at(deadline, peer.recv_from(&mut buf)).await {
            Ok(Ok((len, _))) => {
                if String::from_utf8_lossy(&buf[..len]).starts_with("ACK ") {
                    acks += 1;
                    if !resent {
                        resent = true;
                        peer.send_to(ok.as_bytes(), from).await.expect("sends");
                    }
                }
            }
            _ => break,
        }
    }

    assert!(
        acks >= 2,
        "each 2xx must be acknowledged; saw {acks} ACK(s)"
    );
    let call = dialing
        .await
        .expect("dial task finishes")
        .expect("the 202 establishes a call");
    assert_eq!(
        call.initial_status(),
        202,
        "the call retains the successful status that actually arrived"
    );
}

/// RFC 3261 §12.2.1.1 places the first route into the Request-URI "stripping any parameters
/// that are not allowed in a Request-URI", and §19.1.1 names them: the `method` parameter and
/// the header component. The route came off the wire from another element, so neither can be
/// assumed absent, and a strict router handed either is entitled to reject the request.
#[test]
fn a_strict_route_request_uri_drops_what_may_not_appear_there() {
    use sipx_sip::build::RequestBuilder;

    let invite = RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("example.com").expect("valid"))),
    )
    .header(HeaderName::To, "<sip:callee@example.com>")
    .expect("valid")
    .header(HeaderName::From, "<sip:caller@example.net>;tag=abc")
    .expect("valid")
    .header(HeaderName::CallId, "stripping@example.net")
    .expect("valid")
    .cseq(1, &Method::Invite)
    .expect("valid")
    .header(HeaderName::Contact, "<sip:caller@192.0.2.10:5060>")
    .expect("valid")
    .header(
        HeaderName::RecordRoute,
        "<sip:proxy.example;transport=tcp;method=INVITE?Subject=x>",
    )
    .expect("valid")
    .max_forwards(70)
    .build();

    let dialog = sipx_call::Dialog::from_request(&invite, "tag").expect("a dialog");
    let (uri, _routes) = dialog.request_target();
    let text = String::from_utf8_lossy(&uri.to_bytes()).into_owned();

    assert!(
        !text.contains('?'),
        "the header component may not appear in a Request-URI: {text}"
    );
    assert!(
        !text.to_ascii_lowercase().contains("method="),
        "nor the method parameter: {text}"
    );
    assert!(
        text.contains("transport=tcp"),
        "but the parameters that are allowed survive: {text}"
    );
}

/// RFC 3261 §9.1: with no provisional received, the client "MUST wait for the arrival of a
/// provisional response before sending" the CANCEL. Waiting is the instruction — abandoning the
/// cancellation leaves a callee ringing for a call nobody is waiting for any more.
#[tokio::test]
async fn a_cancel_waits_for_a_late_provisional_rather_than_being_abandoned() {
    use tokio::net::UdpSocket;

    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let (caller, _rx) = endpoint().await;
    let to = Uri::sip(Host::Name(HostName::new("slow.example").expect("valid")));
    let options = sipx_call::DialOptions::new("<sip:caller@example.net>", loopback())
        .with_timeout(Duration::from_millis(250));
    let dialing =
        tokio::spawn(async move { dial(&caller, Target::udp(peer_addr), &to, &options).await });

    let mut buf = vec![0u8; 8192];
    let (len, from) = tokio::time::timeout(Duration::from_secs(2), peer.recv_from(&mut buf))
        .await
        .expect("no timeout")
        .expect("an INVITE");
    let invite = String::from_utf8_lossy(&buf[..len]).into_owned();
    let header = |name: &str| {
        invite
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with(&name.to_ascii_lowercase())
            })
            .unwrap_or_default()
            .to_owned()
    };

    // Ring only *after* the caller has already given up waiting.
    //
    // Ordering a stimulus: the 180 this test injects has to land *after* the caller's own 250 ms
    // timeout at the top of this test, not before it. Giving up puts nothing on the wire — a
    // CANCEL cannot be sent until the provisional arrives, which is the whole point — so there is
    // nothing to watch for.
    //
    // Not "load can only push this later": load stretches this sleep and the dial's own 250 ms
    // clock together, so the order is not guaranteed by the margin. What makes it tolerable is the
    // direction of the failure — a 180 that arrives too early is answered by a caller still
    // waiting, and the CANCEL below then arrives for the ordinary reason, so the test passes
    // vacuously rather than flaking. It proves less than it looks on a loaded machine (`X-44`).
    tokio::time::sleep(Duration::from_millis(500)).await;
    let ringing = format!(
        "SIP/2.0 180 Ringing\r\n{}\r\n{}\r\n{};tag=peertag\r\n{}\r\n{}\r\nContent-Length: 0\r\n\r\n",
        header("Via:"),
        header("From:"),
        header("To:"),
        header("Call-ID:"),
        header("CSeq:")
    );
    peer.send_to(ringing.as_bytes(), from).await.expect("sends");

    let mut cancelled = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, peer.recv_from(&mut buf)).await {
            Ok(Ok((len, _))) => {
                if String::from_utf8_lossy(&buf[..len]).starts_with("CANCEL ") {
                    cancelled = true;
                    break;
                }
            }
            _ => break,
        }
    }
    assert!(
        cancelled,
        "the CANCEL must follow the provisional it was waiting for"
    );
    let _ = dialing.await;
}
