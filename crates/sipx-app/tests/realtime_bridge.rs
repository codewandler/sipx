//! The realtime bridge, held to `docs/specs/openai-realtime.md`'s vectors.
//!
//! Every test is named for the vector it enforces, and every one of them runs against `A-21`'s
//! stand-in peer over loopback: no account, no credential, no container, default `cargo test`
//! matrix. The vectors this story owns are ORB-2, 3, 4, 6, 7, 8, 9, 10, 12, 13, 15, 16 and 18;
//! ORB-1, 5, 11 and 14 belong to the client and the peer and are enforced in their own suites,
//! and ORB-17 is the live proof.
//!
//! **The media seam is a fixture, deliberately.** [`TestCall`] implements
//! [`CallAudio`](sipx_app::realtime::CallAudio) over two channels, so a test can hold the media
//! path still and read the queue behind it. §4.3's flush and §5.4's bounds are claims about a
//! number of frames, and against a real [`MediaSession`](sipx_media::MediaSession) draining at the
//! RTP clock there is no instant at which that number is observable — only "audio arrived
//! eventually", which is the assertion the spec exists to replace. The real session is proven
//! against instead in `realtime_call.rs`, which bridges an actual SIP call end to end.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};
use sipx_app::realtime::{
    ALAW_SILENCE, BridgeLimits, BridgeMeters, BridgeOutcome, BridgeReport, CallAudio, FRAME_BYTES,
    MULAW_SILENCE, RealtimeBridge, SessionSetup, SetupStep,
};
use sipx_app::wss::{WssClient, WssClientConfig};
use sipx_media::Encoded;
use sipx_testkit::certs::Ca;
use sipx_testkit::realtime_peer::{
    CancelPolicy, F_RAMP_BASE64, F_SILENCE, F_SILENCE_BASE64, FIXTURE_BEARER, Malformed,
    PeerConfig, RealtimePeer, StallPoint, Withhold, tone_frame,
};
use sipx_transport::tls::{ClientTls, TrustAnchors};
use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio::task::JoinHandle;

/// How long a test waits for something the bridge is supposed to do.
///
/// A **bound on failure**: how long this side waits before concluding it is not coming. It orders
/// nothing — every positive assertion below completes on the event itself.
const ARRIVAL: Duration = Duration::from_secs(10);

/// How long a hole has to be before "the bridge sent nothing" is true.
///
/// A **definition of silence**: the assertion underneath is negative, so a slower machine
/// lengthens the real hole rather than shortening this window.
const QUIET: Duration = Duration::from_millis(250);

/// The setup bound a test drives, where §3's own 10 s would only make the suite slow.
///
/// A **bound on failure**, and the same one §3 states with a smaller number: what it bounds is how
/// long the bridge waits for an acknowledgement before giving up. That the shipped default really
/// is 10 s is asserted separately, against the constant.
const SHORT_SETUP: Duration = Duration::from_millis(400);

/// The instructions every test configures, so the one `session.update` can be checked for them.
const INSTRUCTIONS: &str = "answer briefly and never mention the weather";

// ------------------------------------------------------------------------- the media seam ----

/// A call leg a test drives from both ends.
struct TestCall {
    payload_type: u8,
    inbound: tokio::sync::Mutex<mpsc::Receiver<Encoded>>,
    sent: Mutex<Vec<Encoded>>,
    delivered: Notify,
    /// Frames that have crossed the queue boundary and are waiting for the media fixture.
    sending: AtomicUsize,
    sending_changed: Notify,
    /// When set, the media path takes a permit before accepting a frame — which is how a test
    /// holds it still and reads the downlink queue that has formed behind it.
    gate: Option<Arc<Semaphore>>,
}

impl TestCall {
    /// A call whose media path accepts everything as fast as it arrives.
    fn open(payload_type: u8) -> (Arc<Self>, mpsc::Sender<Encoded>) {
        Self::with_gate(payload_type, None)
    }

    fn with_gate(
        payload_type: u8,
        gate: Option<Arc<Semaphore>>,
    ) -> (Arc<Self>, mpsc::Sender<Encoded>) {
        // Deeper than the bridge's own uplink bound so this fixture never becomes the thing that
        // drops a frame: §5.4's counter must be the bridge's arithmetic, not the harness's.
        let (frames, inbound) = mpsc::channel(256);
        (
            Arc::new(Self {
                payload_type,
                inbound: tokio::sync::Mutex::new(inbound),
                sent: Mutex::new(Vec::new()),
                delivered: Notify::new(),
                sending: AtomicUsize::new(0),
                sending_changed: Notify::new(),
                gate,
            }),
            frames,
        )
    }

    /// Every frame the bridge has handed to the media path.
    fn sent(&self) -> Vec<Encoded> {
        self.sent.lock().unwrap().clone()
    }

    /// Every byte of it, concatenated in arrival order.
    fn heard(&self) -> Vec<u8> {
        self.sent()
            .iter()
            .flat_map(|frame| frame.payload.to_vec())
            .collect()
    }

    /// Wait until `count` frames have reached the media path.
    async fn await_sent(&self, count: usize) -> Vec<Encoded> {
        // ARRIVAL is a bound on failure; the loop completes on the frame itself.
        tokio::time::timeout(ARRIVAL, async {
            loop {
                let changed = self.delivered.notified();
                {
                    let sent = self.sent.lock().unwrap();
                    if sent.len() >= count {
                        return sent.clone();
                    }
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("{count} frames never reached the media path"))
    }

    /// Wait until `count` frames have left the downlink queue and are committed to this seam.
    async fn await_sending(&self, count: usize) {
        // ARRIVAL is a bound on failure; the loop completes on the hand-off itself.
        tokio::time::timeout(ARRIVAL, async {
            loop {
                let changed = self.sending_changed.notified();
                if self.sending.load(Ordering::SeqCst) >= count {
                    return;
                }
                changed.await;
            }
        })
        .await
        .unwrap_or_else(|_elapsed| panic!("{count} frames never reached the media seam"));
    }
}

impl CallAudio for TestCall {
    fn wire_payload_type(&self) -> u8 {
        self.payload_type
    }

    fn recv_encoded(&self) -> BoxFuture<'_, Option<Encoded>> {
        Box::pin(async move { self.inbound.lock().await.recv().await })
    }

    fn send_encoded(&self, encoded: Encoded) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            self.sending.fetch_add(1, Ordering::SeqCst);
            self.sending_changed.notify_waiters();
            if let Some(gate) = &self.gate {
                let Ok(permit) = gate.acquire().await else {
                    return false;
                };
                permit.forget();
            }
            self.sent.lock().unwrap().push(encoded);
            self.delivered.notify_waiters();
            true
        })
    }
}

// ---------------------------------------------------------------------------- the scaffold ----

/// A client verifying against a fixture authority: hermetic, and never the platform trust store.
///
/// The peer is cleartext on loopback, so no certificate is ever presented — but the client needs a
/// policy to hold, and taking the machine's would make these tests depend on a store that differs
/// between a developer's box and CI.
fn client() -> WssClient {
    WssClient::new(client_tls())
}

fn client_tls() -> ClientTls {
    let mut anchors = TrustAnchors::only();
    anchors
        .add_pem(Ca::new().pem().as_bytes())
        .expect("a fixture anchor");
    ClientTls::new(&anchors).expect("a client policy")
}

/// The configured session a test dials the stand-in with.
fn setup(peer: &RealtimePeer, bearer: &str) -> SessionSetup {
    SessionSetup::new(
        peer.url(),
        "gpt-realtime-2.1",
        INSTRUCTIONS,
        "openai-api-key",
        bearer.as_bytes(),
    )
    .expect("a usable credential")
}

/// One bridge running against the peer, with the handles a script needs.
struct Bridged {
    call: Arc<TestCall>,
    frames: mpsc::Sender<Encoded>,
    meters: Arc<BridgeMeters>,
    task: JoinHandle<BridgeReport>,
}

impl Bridged {
    /// End the call, which is how a well-behaved bridge stops (§6).
    async fn hang_up(self) -> BridgeReport {
        let Self { frames, task, .. } = self;
        drop(frames);
        task.await.expect("the bridge task")
    }

    /// Wait for the bridge to end of its own accord — the peer did something terminal.
    async fn ended(self) -> BridgeReport {
        let Self { frames, task, .. } = self;
        let report = tokio::time::timeout(ARRIVAL, task) // a bound on failure
            .await
            .expect("the bridge ended")
            .expect("the bridge task");
        drop(frames);
        report
    }
}

/// Start a bridge against `peer` on a call of `payload_type`, with the spec's own bounds.
fn start(peer: &RealtimePeer, payload_type: u8) -> Bridged {
    start_with(
        peer,
        payload_type,
        FIXTURE_BEARER,
        BridgeLimits::default(),
        None,
    )
}

/// Start a bridge whose media path is held still, so the downlink queue can be read.
fn start_held(peer: &RealtimePeer, payload_type: u8) -> (Bridged, Arc<Semaphore>) {
    let gate = Arc::new(Semaphore::new(0));
    let bridged = start_with(
        peer,
        payload_type,
        FIXTURE_BEARER,
        BridgeLimits::default(),
        Some(Arc::clone(&gate)),
    );
    (bridged, gate)
}

fn start_with(
    peer: &RealtimePeer,
    payload_type: u8,
    bearer: &str,
    limits: BridgeLimits,
    gate: Option<Arc<Semaphore>>,
) -> Bridged {
    start_with_client(peer, payload_type, bearer, limits, gate, client())
}

fn start_with_client(
    peer: &RealtimePeer,
    payload_type: u8,
    bearer: &str,
    limits: BridgeLimits,
    gate: Option<Arc<Semaphore>>,
    client: WssClient,
) -> Bridged {
    let (call, frames) = TestCall::with_gate(payload_type, gate);
    let bridge = RealtimeBridge::with_limits(client, setup(peer, bearer), limits);
    let meters = bridge.meters();
    let audio: Arc<dyn CallAudio> = Arc::clone(&call) as Arc<dyn CallAudio>;
    let task = tokio::spawn(async move { bridge.run(audio, std::future::pending()).await });
    Bridged {
        call,
        frames,
        meters,
        task,
    }
}

/// Walk setup and prove it happened: the peer read one `session.update`.
async fn establish(peer: &RealtimePeer) -> Value {
    let record = peer
        .await_session_update()
        .await
        .expect("the peer read a session.update");
    record.session_updates()[0].clone()
}

/// Push one payload up from the call.
async fn speak(frames: &mpsc::Sender<Encoded>, payload_type: u8, bytes: &[u8]) {
    frames
        .send(Encoded::new(payload_type, Bytes::copy_from_slice(bytes)))
        .await
        .expect("the call leg accepts a frame");
}

// ------------------------------------------------------------------------------ the vectors ----

/// ORB-2: after `session.created` the bridge sends exactly one `session.update` with §3's shape,
/// and audio starts only after `session.updated`.
///
/// The second arm is the non-vacuity: against a peer that never acknowledges, the same uplink
/// frames produce no append at all, so "audio started after the acknowledgement" is a fact about
/// the ordering rather than about the test being slow.
#[tokio::test]
async fn orb_2_one_session_update_pins_the_format_and_audio_waits_for_the_acknowledgement() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    let update = establish(&peer).await;

    assert_eq!(update["type"], "session.update");
    let session = &update["session"];
    assert_eq!(session["type"], "realtime");
    assert_eq!(session["output_modalities"], json!(["audio"]));
    assert_eq!(session["instructions"], INSTRUCTIONS);
    assert_eq!(session["audio"]["input"]["format"]["type"], "audio/pcmu");
    assert_eq!(session["audio"]["output"]["format"]["type"], "audio/pcmu");
    let detection = &session["audio"]["input"]["turn_detection"];
    assert_eq!(detection["type"], "server_vad");
    assert_eq!(detection["create_response"], json!(true));
    assert_eq!(
        detection["interrupt_response"],
        json!(false),
        "§3: cancellation has exactly one owner, the bridge's barge-in rule"
    );

    speak(&bridged.frames, 0, &F_SILENCE).await;
    let record = peer.await_appends(1).await.expect("the frame travels");
    assert_eq!(
        record.session_updates().len(),
        1,
        "exactly one session.update, however much audio follows"
    );
    let report = bridged.hang_up().await;
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);

    // The acknowledgement is what admits audio, not the passage of time.
    let withholding = PeerConfig::new()
        .withholding(Withhold::SessionUpdated)
        .start()
        .await
        .expect("the peer binds");
    let bridged = start_with(
        &withholding,
        0,
        FIXTURE_BEARER,
        BridgeLimits {
            setup_bound: SHORT_SETUP,
            ..BridgeLimits::default()
        },
        None,
    );
    withholding
        .await_session_update()
        .await
        .expect("the peer read the update");
    for _ in 0..4 {
        speak(&bridged.frames, 0, &F_SILENCE).await;
    }
    // A definition of silence: no append may appear while the acknowledgement is outstanding.
    let quiet = tokio::time::timeout(QUIET, withholding.await_appends(1)).await;
    assert!(
        quiet.is_err(),
        "audio may not start before session.updated: {quiet:?}"
    );
    let report = bridged.ended().await;
    assert_eq!(
        report.outcome,
        BridgeOutcome::SetupTimeout {
            awaiting: SetupStep::SessionUpdated,
            bound: SHORT_SETUP,
        }
    );
    assert_eq!(report.counters.appended, 0, "not one frame was written");
}

/// ORB-3: one uplink frame becomes exactly one `input_audio_buffer.append` whose `audio` is
/// §4.2's 216-character literal.
///
/// Asserted as `encode(appended_audio) == F_SILENCE_BASE64` rather than against the member as it
/// travelled, because the peer keeps the *decoded* bytes — an assertion on what it stored would
/// hold even if the bridge had sent some other encoding of them.
#[tokio::test]
async fn orb_3_an_uplink_frame_is_one_append_carrying_the_specs_literal() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;

    speak(&bridged.frames, 0, &F_SILENCE).await;
    let record = peer.await_appends(1).await.expect("one append");
    assert_eq!(
        record.appends(),
        1,
        "one frame, one append: §4.1 never batches"
    );
    assert_eq!(
        BASE64.encode(&record.appended_audio),
        F_SILENCE_BASE64,
        "the bytes that arrived re-encode to §4.2's literal"
    );
    assert_eq!(record.appended_audio.len(), FRAME_BYTES);

    // Passthrough, asserted as byte identity: what left the call is what reached the far end.
    assert_eq!(record.appended_audio, F_SILENCE.to_vec());
    let report = bridged.hang_up().await;
    assert_eq!(report.counters.appended, 1);
}

/// ORB-4: one delta carrying F-ramp's base64 reaches the media path as exactly the bytes
/// `0x00…0x9F`, in one 160-byte frame.
#[tokio::test]
async fn orb_4_a_delta_reaches_the_media_path_as_the_ramp_it_carried() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;

    // The fixture's frame 0 *is* §4.2's F-ramp, checked here against the spec's own literal so the
    // expectation below is the spec's rather than the fixture's.
    assert_eq!(
        BASE64.decode(F_RAMP_BASE64).expect("the literal decodes"),
        tone_frame(0).to_vec()
    );
    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta");
    let sent = bridged.call.await_sent(1).await;
    assert_eq!(sent.len(), 1, "one frame, not a stream of fragments");
    assert_eq!(sent[0].payload.len(), FRAME_BYTES);
    assert_eq!(sent[0].payload_type, 0, "the call's own payload type");
    assert_eq!(sent[0].payload.to_vec(), tone_frame(0).to_vec());

    let report = bridged.hang_up().await;
    assert_eq!(report.counters.delivered, 1);
}

/// ORB-6: 400 bytes of delta then `response.output_audio.done` becomes two whole frames and one
/// padded to full length with the format's silence byte (§4.1).
#[tokio::test]
async fn orb_6_a_partial_tail_is_padded_with_the_formats_silence() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;

    let audio: Vec<u8> = (0..400u32)
        .map(|byte| u8::try_from(byte % 251).unwrap())
        .collect();
    peer.send_delta("resp_001", &audio).await.expect("a delta");
    let sent = bridged.call.await_sent(2).await;
    assert_eq!(
        sent.len(),
        2,
        "400 bytes is two whole frames and a remainder"
    );
    // A definition of silence: the remainder must not become a frame until the response ends.
    let waiting = tokio::time::timeout(QUIET, bridged.call.await_sent(3)).await;
    assert!(
        waiting.is_err(),
        "80 bytes is not a frame and must not be sent short: {waiting:?}"
    );

    peer.send_audio_done("resp_001").await.expect("audio done");
    let sent = bridged.call.await_sent(3).await;
    assert_eq!(sent[2].payload.len(), FRAME_BYTES);
    assert_eq!(&sent[2].payload[..80], &audio[320..400]);
    assert_eq!(
        &sent[2].payload[80..],
        &[MULAW_SILENCE; 80][..],
        "§4.1 pads μ-law with 0xFF"
    );
    assert_eq!(
        bridged.call.heard()[..400],
        audio[..],
        "passthrough is exact"
    );
    let _report = bridged.hang_up().await;
}

/// ORB-7: an A-law call says `audio/pcma` both directions, and ORB-3, ORB-4 and ORB-6 hold
/// unchanged — with A-law's own silence byte doing the padding.
#[tokio::test]
async fn orb_7_an_a_law_call_pins_pcma_and_pads_with_its_own_silence() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 8);
    let update = establish(&peer).await;
    let session = &update["session"];
    assert_eq!(session["audio"]["input"]["format"]["type"], "audio/pcma");
    assert_eq!(session["audio"]["output"]["format"]["type"], "audio/pcma");

    // ORB-3's shape.
    let frame = [0xD5u8; FRAME_BYTES];
    speak(&bridged.frames, 8, &frame).await;
    let record = peer.await_appends(1).await.expect("one append");
    assert_eq!(record.appended_audio, frame.to_vec());

    // ORB-4's shape.
    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta");
    let sent = bridged.call.await_sent(1).await;
    assert_eq!(sent[0].payload.to_vec(), tone_frame(0).to_vec());
    assert_eq!(sent[0].payload_type, 8);

    // ORB-6's shape, with A-law's silence.
    peer.send_delta("resp_001", &[0x2Au8; 80])
        .await
        .expect("a partial delta");
    peer.send_audio_done("resp_001").await.expect("audio done");
    let sent = bridged.call.await_sent(2).await;
    assert_eq!(sent[1].payload.len(), FRAME_BYTES);
    assert_eq!(&sent[1].payload[..80], &[0x2Au8; 80][..]);
    assert_eq!(
        &sent[1].payload[80..],
        &[ALAW_SILENCE; 80][..],
        "§4.1 pads A-law with 0xD5, not with μ-law's byte"
    );
    let _report = bridged.hang_up().await;
}

/// ORB-8: barge-in cancels, empties the queue and the accumulator together, drops what follows,
/// and lets at most one already-committed frame through.
///
/// The media path is held still on purpose. §4.3's flush is a claim about *how many frames* were
/// thrown away, and a media path that drains at the RTP clock offers no instant at which that
/// number exists.
#[tokio::test]
async fn orb_8_barge_in_cancels_flushes_and_bounds_what_still_reaches_the_call() {
    let peer = PeerConfig::new()
        // §4.3 claims no bound on deltas that arrive after a cancel, because that number is the
        // far end's; a peer that stopped obligingly could not produce the ones this counts.
        .on_cancel(CancelPolicy::KeepStreaming)
        .start()
        .await
        .expect("the peer binds");
    let (bridged, gate) = start_held(&peer, 0);
    establish(&peer).await;

    // Seventeen frames, one of which the writer takes and parks on, leaves sixteen queued; the
    // eighty bytes after them are the accumulator's residue.
    for frame in 0..17 {
        peer.send_delta("resp_001", &tone_frame(frame))
            .await
            .expect("a delta");
    }
    // Complete on the actual hand-off rather than assuming the downlink task has been scheduled:
    // this is the one frame §4.3 permits to remain ahead of the flush.
    bridged.call.await_sending(1).await;
    peer.send_delta("resp_001", &[0x11u8; 80])
        .await
        .expect("a partial delta");

    // No polling and no wait: the peer wrote these on one socket in order, and the bridge reads
    // that socket in order, so everything above is queued by the time `speech_started` is read.
    peer.send_speech_started().await.expect("speech_started");
    peer.await_cancel().await.expect("the bridge cancels");

    // Two more deltas the far end chose to send after the cancel, then the response ends.
    peer.send_delta("resp_001", &tone_frame(90))
        .await
        .expect("a delta after the cancel");
    peer.send_delta("resp_001", &tone_frame(91))
        .await
        .expect("a delta after the cancel");
    peer.send_response_done("resp_001", "cancelled")
        .await
        .expect("the response ends");

    // One more round trip through the socket, so the two deltas and the done have been read
    // before the counters are asserted: the peer answers this append after them, in order.
    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("the frame travels");

    let counters = bridged.meters.snapshot();
    assert_eq!(
        counters.barge_in_flushed, 16,
        "sixteen queued frames went; the residue counts nothing because it never became a frame"
    );
    assert_eq!(
        counters.cancelled_deltas, 2,
        "every delta between the cancel and response.done is dropped and counted"
    );
    assert_eq!(bridged.meters.downlink_depth(), 0, "the queue is empty");
    assert_eq!(
        bridged.meters.accumulator_bytes(),
        0,
        "the accumulator emptied with it, so no frame can be built from the residue"
    );

    // Now let the media path run. Exactly one frame — the one already committed when the flush
    // happened — may arrive, which is §4.3's ≤ 20 ms residual.
    gate.add_permits(8);
    let sent = bridged.call.await_sent(1).await;
    assert_eq!(sent.len(), 1, "at most one frame was ahead of the flush");
    assert_eq!(sent[0].payload.to_vec(), tone_frame(0).to_vec());
    let waiting = tokio::time::timeout(QUIET, bridged.call.await_sent(2)).await; // silence
    assert!(waiting.is_err(), "nothing follows it: {waiting:?}");

    let report = bridged.hang_up().await;
    assert_eq!(report.counters.barge_in_flushed, 16);
    assert_eq!(report.counters.delivered, 1);
}

/// ORB-9: an `error` inside the cancel-race window is the race and the session lives; the same
/// `error` outside it ends the bridge `SessionError`.
#[tokio::test]
async fn orb_9_an_error_is_the_cancel_race_inside_the_window_and_fatal_outside_it() {
    let peer = PeerConfig::new()
        .on_cancel(CancelPolicy::KeepStreaming)
        .start()
        .await
        .expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;

    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta puts a response in flight");
    peer.send_speech_started().await.expect("speech_started");
    peer.await_cancel().await.expect("the bridge cancels");
    peer.send_error("response_cancel_not_active", "no active response")
        .await
        .expect("the racing error");

    // The session is still live: the peer's next event is still consumed, and the call's next
    // frame still travels.
    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("the session lives");
    assert_eq!(bridged.meters.snapshot().cancel_race, 1);

    // Outside the window — the response has ended, so the window is closed — the same error is
    // the session's end.
    peer.send_response_done("resp_001", "cancelled")
        .await
        .expect("the response ends");
    peer.send_error("server_error", "something broke")
        .await
        .expect("the fatal error");
    let report = bridged.ended().await;
    assert_eq!(
        report.outcome,
        BridgeOutcome::SessionError {
            code: Some("server_error".to_owned()),
        }
    );
    assert_eq!(report.counters.cancel_race, 1, "only the first one raced");
}

/// ORB-10: a refused upgrade is `AuthRefused`, no session is established, no audio ever travels,
/// and the outcome names the secret and provably not its value.
#[tokio::test]
async fn orb_10_a_refused_upgrade_names_the_secret_and_never_its_value() {
    let peer = PeerConfig::new()
        .expecting_bearer("the-key-the-peer-wants")
        .start()
        .await
        .expect("the peer binds");
    let bridged = start_with(
        &peer,
        0,
        "sk-the-key-the-bridge-has",
        BridgeLimits::default(),
        None,
    );
    let report = bridged.ended().await;

    assert_eq!(
        report.outcome,
        BridgeOutcome::AuthRefused {
            secret: "openai-api-key".to_owned(),
            status: Some(401),
        }
    );
    let printed = format!("{} / {:?}", report.outcome, report.outcome);
    assert!(printed.contains("openai-api-key"), "{printed}");
    assert!(
        !printed.contains("sk-the-key-the-bridge-has"),
        "the outcome carried the credential: {printed}"
    );

    let record = peer.record();
    assert_eq!(record.refused(), 1, "the peer saw the attempt");
    assert_eq!(record.accepted(), 0, "no session was ever established");
    assert!(record.appended_audio.is_empty(), "no audio ever travelled");
    assert_eq!(report.counters.appended, 0);

    // Non-vacuity: the same bridge with the bearer the peer wants does establish a session, so
    // the refusal above is the credential and not the fixture.
    let bridged = start_with(
        &peer,
        0,
        "the-key-the-peer-wants",
        BridgeLimits::default(),
        None,
    );
    establish(&peer).await;
    assert_eq!(peer.record().accepted(), 1);
    let _report = bridged.hang_up().await;
}

/// ORB-12: events outside §5.2 are ignored with a counter, and the audio path is unaffected.
#[tokio::test]
async fn orb_12_unknown_events_are_ignored_with_a_counter_and_cost_the_audio_nothing() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;

    peer.send_unknown("rate_limits.updated")
        .await
        .expect("a known-unknown");
    peer.send_unknown("conversation.item.retained.v9")
        .await
        .expect("a future event nobody has heard of");
    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta");
    let sent = bridged.call.await_sent(1).await;
    assert_eq!(sent[0].payload.to_vec(), tone_frame(0).to_vec());

    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("the uplink still runs");

    let report = bridged.hang_up().await;
    assert_eq!(
        report.counters.ignored_events, 2,
        "both were ignored, and counted so a vendor addition is visible rather than silent"
    );
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);
    assert_eq!(report.counters.delivered, 1);
    assert_eq!(report.counters.appended, 1);
}

/// ORB-13: a frame that cannot be read as an event at all is `MalformedEvent` on its first
/// occurrence, whichever of the three shapes it takes.
#[tokio::test]
async fn orb_13_an_unreadable_frame_ends_the_session_on_its_first_occurrence() {
    for malformed in [Malformed::NotJson, Malformed::NoType, Malformed::Binary] {
        let peer = PeerConfig::new().start().await.expect("the peer binds");
        let bridged = start(&peer, 0);
        establish(&peer).await;

        peer.send_malformed(malformed.clone())
            .await
            .expect("the malformed frame");
        let report = bridged.ended().await;
        assert!(
            matches!(report.outcome, BridgeOutcome::MalformedEvent { .. }),
            "{malformed:?} must be fatal, got {:?}",
            report.outcome
        );
        let record = peer
            .observe("the session ending", |record| record.sessions_ended == 1)
            .await
            .expect("the session ends");
        assert_eq!(record.upgrades.len(), 1, "and no second upgrade follows it");
    }
}

/// ORB-18: a §5.2 event that fails its read set is the same `MalformedEvent`, member by member.
#[tokio::test]
async fn orb_18_a_delta_that_fails_its_read_set_is_malformed() {
    let cases = [
        Malformed::DeltaNotBase64 {
            response: "resp_001".to_owned(),
        },
        Malformed::DeltaMissing {
            response: "resp_001".to_owned(),
        },
        Malformed::AudioDoneWithoutResponseId,
    ];
    for malformed in cases {
        let peer = PeerConfig::new().start().await.expect("the peer binds");
        let bridged = start(&peer, 0);
        establish(&peer).await;

        peer.send_malformed(malformed.clone())
            .await
            .expect("the malformed event");
        let report = bridged.ended().await;
        assert!(
            matches!(report.outcome, BridgeOutcome::MalformedEvent { .. }),
            "{malformed:?} must be fatal, got {:?}",
            report.outcome
        );
        assert_eq!(
            report.counters.delivered, 0,
            "nothing unreadable reached the call"
        );
    }

    // Non-vacuity: the same peer sending a *well-formed* delta of the same shape does not end the
    // session, so the three above fail on their read set rather than on being deltas.
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;
    peer.send_delta("resp_001", &tone_frame(0))
        .await
        .expect("a delta");
    peer.send_audio_done("resp_001").await.expect("audio done");
    let sent = bridged.call.await_sent(1).await;
    assert_eq!(sent[0].payload.to_vec(), tone_frame(0).to_vec());
    let report = bridged.hang_up().await;
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);
}

/// ORB-15: each half of setup has its own bound, and missing either is `SetupTimeout`.
///
/// The bound driven here is short; that the shipped one is §3's 10 s is asserted against the
/// constant, because a suite that waited out the real bound twice would cost twenty seconds to
/// learn a number that is written down.
#[tokio::test]
async fn orb_15_a_missing_setup_acknowledgement_is_a_typed_timeout() {
    assert_eq!(
        sipx_app::realtime::SETUP_BOUND,
        Duration::from_secs(10),
        "§3's bound is ten seconds in each direction"
    );

    let peer = PeerConfig::new()
        .withholding(Withhold::SessionCreated)
        .start()
        .await
        .expect("the peer binds");
    let bridged = start_with(
        &peer,
        0,
        FIXTURE_BEARER,
        BridgeLimits {
            setup_bound: SHORT_SETUP,
            ..BridgeLimits::default()
        },
        None,
    );
    let report = bridged.ended().await;
    assert_eq!(
        report.outcome,
        BridgeOutcome::SetupTimeout {
            awaiting: SetupStep::SessionCreated,
            bound: SHORT_SETUP,
        }
    );
    assert!(
        peer.record().session_updates().is_empty(),
        "nothing is configured before the server says hello"
    );

    let peer = PeerConfig::new()
        .withholding(Withhold::SessionUpdated)
        .start()
        .await
        .expect("the peer binds");
    let bridged = start_with(
        &peer,
        0,
        FIXTURE_BEARER,
        BridgeLimits {
            setup_bound: SHORT_SETUP,
            ..BridgeLimits::default()
        },
        None,
    );
    let report = bridged.ended().await;
    assert_eq!(
        report.outcome,
        BridgeOutcome::SetupTimeout {
            awaiting: SetupStep::SessionUpdated,
            bound: SHORT_SETUP,
        }
    );
    assert_eq!(
        peer.record().session_updates().len(),
        1,
        "the bridge did configure; the acknowledgement is what never came"
    );

    // Non-vacuity: the same short bound against a peer that answers establishes a session, so the
    // two timeouts above are the withholding and not the bound being unreachably small.
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start_with(
        &peer,
        0,
        FIXTURE_BEARER,
        BridgeLimits {
            setup_bound: SHORT_SETUP,
            ..BridgeLimits::default()
        },
        None,
    );
    establish(&peer).await;
    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("audio flows");
    let report = bridged.hang_up().await;
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);
}

/// ORB-16: a normal close carries its code into the outcome, an abrupt reset carries none, and
/// neither is followed by a second upgrade.
///
/// The reset arm asserts the reset **as a reset**. "Not a close frame" is also true of an ordinary
/// EOF, and an EOF is exactly what the fixture produces if its zero linger is removed — so the
/// claim here is that the connection ended with a transport failure the bridge can name, which an
/// orderly `Ok(None)` never produces.
#[tokio::test]
async fn orb_16_a_close_carries_its_code_a_reset_carries_a_failure_and_neither_reconnects() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;
    peer.close_normally().await.expect("a close");
    let report = bridged.ended().await;
    assert_eq!(
        report.outcome,
        BridgeOutcome::PeerClosed {
            code: Some(1000),
            detail: None,
        },
        "a clean close is a code and nothing else"
    );
    let record = peer
        .observe("the session ending", |record| record.sessions_ended == 1)
        .await
        .expect("the session ends");
    assert_eq!(
        record.upgrades.len(),
        1,
        "no second upgrade: §6 has no reconnect"
    );

    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;
    peer.reset().await.expect("a reset");
    let report = bridged.ended().await;
    let BridgeOutcome::PeerClosed { code, detail } = &report.outcome else {
        panic!(
            "a reset ends the bridge PeerClosed, got {:?}",
            report.outcome
        )
    };
    assert_eq!(*code, None, "a reset carries no close code");
    let detail = detail
        .as_deref()
        .expect("a reset is a transport failure, not an orderly end");
    assert!(
        detail.to_ascii_lowercase().contains("reset"),
        "the connection was reset, not closed and not read to EOF: {detail}"
    );
    assert_eq!(peer.record().upgrades.len(), 1, "no second upgrade");
}

/// §6's mid-call negative: a peer that stays connected but stops servicing the socket is ended by
/// liveness, once, with the grace that actually elapsed.
#[tokio::test]
async fn a_mid_call_stall_ends_with_the_typed_liveness_outcome() {
    const PROBE: Duration = Duration::from_millis(50);
    const GRACE: Duration = Duration::from_millis(100);
    let peer = PeerConfig::new()
        .stalling_at(StallPoint::Session)
        .start()
        .await
        .expect("the peer binds");
    let fast_client = WssClient::with_config(
        client_tls(),
        WssClientConfig {
            ping_interval: PROBE,
            ping_grace: GRACE,
            ..WssClientConfig::default()
        },
    );
    let bridged = start_with_client(
        &peer,
        0,
        FIXTURE_BEARER,
        BridgeLimits::default(),
        None,
        fast_client,
    );
    establish(&peer).await;
    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1)
        .await
        .expect("the peer was live before it stalled");
    let report = bridged.ended().await;
    assert_eq!(report.outcome, BridgeOutcome::PeerStalled { bound: GRACE });
    assert_eq!(
        peer.record().upgrades.len(),
        1,
        "no reconnect after liveness ends"
    );
}

// ---------------------------------------------------- the rows §6 owns that have no ORB id ----

/// §3: a call whose negotiated payload is neither PCMU nor PCMA is refused **before** a socket is
/// opened, which is the difference between a bridge that cannot transcode and one that presents a
/// credential on a call it was never going to serve.
#[tokio::test]
async fn a_call_that_is_not_g711_never_reaches_the_endpoint() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 96);
    let report = bridged.ended().await;
    assert_eq!(
        report.outcome,
        BridgeOutcome::NotBridgeable { payload_type: 96 }
    );
    // A definition of silence: no upgrade may appear, so the refusal is before the socket.
    let quiet = tokio::time::timeout(QUIET, peer.await_upgrade()).await;
    assert!(
        quiet.is_err(),
        "the credential must not be presented on a call that cannot be bridged: {quiet:?}"
    );
    assert!(peer.record().upgrades.is_empty());
}

/// §5.4: both queues are bounded, neither blocks its producer, and every dropped frame is counted.
///
/// The uplink is held shut by withholding `session.updated` — §3's own window, in which frames are
/// admitted to the queue and nothing is written — so the thirty-third frame has nowhere to go.
#[tokio::test]
async fn a_full_uplink_queue_drops_the_offered_frame_and_counts_it() {
    let peer = PeerConfig::new()
        .withholding(Withhold::SessionUpdated)
        .start()
        .await
        .expect("the peer binds");
    let bridged = start_with(
        &peer,
        0,
        FIXTURE_BEARER,
        BridgeLimits {
            setup_bound: Duration::from_secs(30),
            ..BridgeLimits::default()
        },
        None,
    );
    peer.await_session_update()
        .await
        .expect("the bridge configured");

    // Forty frames against a bound of thirty-two. The producer never blocks: this loop completing
    // is itself the assertion that admission is non-blocking.
    for _ in 0..40 {
        speak(&bridged.frames, 0, &F_SILENCE).await;
    }
    // The count settles once the uplink task has read all forty. It completes on the counter, not
    // on a clock; ARRIVAL is a bound on failure.
    let dropped = tokio::time::timeout(ARRIVAL, async {
        loop {
            let counters = bridged.meters.snapshot();
            if counters.uplink_dropped >= 8 {
                return counters.uplink_dropped;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the queue fills and the overflow is counted");
    assert_eq!(dropped, 8, "forty offered against a bound of thirty-two");
    assert_eq!(
        bridged.meters.snapshot().appended,
        0,
        "and none of them was written: the acknowledgement is still outstanding"
    );
    let report = bridged.hang_up().await;
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);
    assert_eq!(report.counters.uplink_dropped, 8);
}

/// §5.4: a full downlink queue drops the offered frame and the session stays live.
#[tokio::test]
async fn a_full_downlink_queue_drops_the_offered_frame_and_the_session_lives() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let gate = Arc::new(Semaphore::new(0));
    let bridged = start_with(
        &peer,
        0,
        FIXTURE_BEARER,
        BridgeLimits {
            downlink_frames: 2,
            ..BridgeLimits::default()
        },
        Some(Arc::clone(&gate)),
    );
    establish(&peer).await;

    // A small bound, so the vector is a handful of frames rather than two thousand; the number the
    // spec fixes is asserted against the constant instead.
    assert_eq!(
        sipx_app::realtime::DOWNLINK_QUEUE_FRAMES,
        2048,
        "§5.4's downlink bound is 2048 frames"
    );
    assert_eq!(
        sipx_app::realtime::UPLINK_QUEUE_FRAMES,
        32,
        "§5.4's uplink bound is 32 frames"
    );

    // Commit one frame to the held media seam first, then fill the two-frame queue and offer one
    // more. The hand-off is an event, so no scheduler timing stands in for the claimed shape.
    peer.send_delta("resp_001", &[0x33u8; FRAME_BYTES])
        .await
        .expect("a delta");
    bridged.call.await_sending(1).await;
    let overflowing = vec![0x33u8; FRAME_BYTES * 3];
    peer.send_delta("resp_001", &overflowing)
        .await
        .expect("a burst against the full queue");
    peer.send_unknown("queue.checkpoint")
        .await
        .expect("an ordered checkpoint after the burst");
    let ignored = tokio::time::timeout(ARRIVAL, async {
        loop {
            let counters = bridged.meters.snapshot();
            if counters.ignored_events == 1 {
                return counters;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the bridge reads through the burst");
    assert_eq!(bridged.meters.downlink_depth(), 2);
    assert_eq!(ignored.downlink_dropped, 1);
    // The session is still live afterwards, which is the half of §5.4 that separates a media queue
    // from a control one.
    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("the session lives");
    let report = bridged.hang_up().await;
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);
}

/// §6: the host's stop signal ends the bridge `Cancelled`, and nothing it spawned outlives it.
///
/// The orphan check is `Arc::strong_count`: the two media tasks each hold a clone of the call leg,
/// so a count back at one is the mechanical statement that both are gone — not that they are
/// probably gone.
#[tokio::test]
async fn a_cancelled_bridge_joins_its_tasks_and_leaves_no_orphan() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let (call, frames) = TestCall::open(0);
    let bridge = RealtimeBridge::new(client(), setup(&peer, FIXTURE_BEARER));
    let (stop, stopped) = oneshot::channel::<()>();
    let audio: Arc<dyn CallAudio> = Arc::clone(&call) as Arc<dyn CallAudio>;
    let task = tokio::spawn(async move {
        bridge
            .run(audio, async move {
                let _stopped = stopped.await;
            })
            .await
    });
    establish(&peer).await;
    speak(&frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("the bridge is running");
    assert!(
        Arc::strong_count(&call) > 1,
        "the media tasks are holding the call leg"
    );

    stop.send(()).expect("the bridge is listening for the stop");
    let report = tokio::time::timeout(ARRIVAL, task) // a bound on failure
        .await
        .expect("the bridge stops")
        .expect("the bridge task");
    assert_eq!(report.outcome, BridgeOutcome::Cancelled);
    assert_eq!(
        Arc::strong_count(&call),
        1,
        "every task the bridge owned was joined before it reported"
    );
    drop(frames);
}

/// §6: when the call ends first the bridge closes normally and reports `CallEnded`, joining its
/// tasks the same way.
#[tokio::test]
async fn a_call_that_ends_first_closes_the_session_normally_and_leaves_no_orphan() {
    let peer = PeerConfig::new().start().await.expect("the peer binds");
    let bridged = start(&peer, 0);
    establish(&peer).await;
    speak(&bridged.frames, 0, &F_SILENCE).await;
    peer.await_appends(1).await.expect("the bridge is running");

    let call = Arc::clone(&bridged.call);
    let report = bridged.hang_up().await;
    assert_eq!(report.outcome, BridgeOutcome::CallEnded);
    assert_eq!(
        Arc::strong_count(&call),
        1,
        "no media task outlives the bridge"
    );
    let record = peer
        .observe("the session ending", |record| record.sessions_ended == 1)
        .await
        .expect("the peer's session ends too");
    assert_eq!(record.upgrades.len(), 1);
}
