//! A deterministic stand-in for the realtime agent endpoint.
//!
//! [`docs/specs/openai-realtime.md`](../../../docs/specs/openai-realtime.md) is a contract with
//! two sides. The bridge holds one of them; this module holds the other, so every vector in that
//! spec except the live proof runs in the default `cargo test` matrix with no account, no
//! credential, no network beyond loopback and no container. A bridge whose only counterparty is
//! the vendor can be tested once a day by whoever holds the key; a bridge whose counterparty is
//! this module is tested by everyone, on every commit, including the failure rows — and the
//! failure rows are the half nobody can produce on demand from a real endpoint.
//!
//! **Cleartext on loopback, deliberately.** The peer speaks `ws://127.0.0.1:<port>` rather than
//! `wss://`. Certificates are the one fixture cost that spreads: a TLS stand-in needs a trust
//! anchor threaded into every test that reaches it, and the first test that finds that awkward
//! disables verification, which is a worse habit than the one the certificate was for. The
//! client this peer exists for permits cleartext to loopback and refuses it everywhere else, so
//! the seam is closed by the client's own rule rather than by a certificate this peer would have
//! to issue. [`certs`](crate::certs) is still where a test that genuinely needs TLS gets one.
//!
//! **Scripted, never guessed.** The peer performs the protocol spine by itself — it checks the
//! bearer, answers with `session.created`, answers a `session.update` with `session.updated`,
//! and records every client event and every appended audio byte — and does nothing else unless
//! a test directs it. Downlink audio, cancels, closes and every malformed frame are directives
//! whose future resolves *after the frame is on the socket*, so a test orders its script by
//! awaiting the peer rather than by waiting on a clock. No behaviour here is timed.
//!
//! **The negatives are the point.** §6's failure taxonomy is reachable one row at a time:
//! a refused bearer ([`PeerConfig::expecting_bearer`]), withheld setup acknowledgements
//! ([`Withhold`]), a peer that answers the upgrade and then goes silent ([`StallPoint`]), a
//! frame over the 1 MiB bound ([`RealtimePeer::send_oversize`]), events that cannot be read
//! ([`Malformed`]), a normal close and an abrupt reset. Each is asserted from the client's side
//! of the socket in this crate's own tests, because a stand-in whose misbehaviour is a flag
//! nobody observes proves nothing about the bridge tested against it.

use std::fmt;
use std::net::SocketAddr;
use std::ops::ControlFlow;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::{JoinHandle, JoinSet};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tokio_tungstenite::tungstenite::{Bytes as WsBytes, Message, Utf8Bytes};
use tokio_util::sync::CancellationToken;

/// The call's 20 ms packet: 160 bytes of G.711 at 8000 Hz, one byte per sample (RFC 3551
/// §4.5.14), which spec §4.1 makes the unit of audio in both directions.
pub const FRAME_BYTES: usize = 160;

/// Spec §4.2's **F-silence**: 20 ms of μ-law digital silence.
pub const F_SILENCE: [u8; FRAME_BYTES] = [0xFF; FRAME_BYTES];

/// F-silence's base64, quoted from spec §4.2.
///
/// A literal, never a value an assertion computes — the discipline
/// [`webhook-binding.md`](../../../docs/specs/webhook-binding.md) WB-8 states and §4.2 adopts:
/// an expected value derived by the code under test is an expectation that cannot disagree.
pub const F_SILENCE_BASE64: &str = "/////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////w==";

/// Spec §4.2's **F-ramp** in base64: the 160 bytes `0x00, 0x01, … 0x9F`.
///
/// Also the first frame of the tone this peer speaks ([`tone_frame`]), so a bridge test can
/// correlate what reached the media path against a literal from the spec.
pub const F_RAMP_BASE64: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2enw==";

/// The bearer the peer expects unless a test configures another one.
///
/// A fixture value with no resemblance to a credential shape, so a leaked log line from a test
/// run cannot be mistaken for a real key by a scanner or by a person.
pub const FIXTURE_BEARER: &str = "fixture-bearer";

/// How long [`RealtimePeer::observe`] waits before reporting that it saw nothing.
///
/// A **bound on failure**: how long the fixture waits before concluding the observation is not
/// coming. It orders nothing — every wait in this module completes on the record changing, and
/// this only stops a broken peer from hanging a suite until CI's own timeout kills it.
pub const OBSERVATION_BOUND: Duration = Duration::from_secs(10);

/// One frame of the tone the peer speaks, by position in the response.
///
/// The tone is a sawtooth over the G.711 code space: frame `index` carries the bytes
/// `index * 160 …` counting upward and wrapping. Two properties earn it:
///
/// - **Frame 0 is spec §4.2's F-ramp**, so the bytes a bridge test expects at `send_encoded` are
///   the spec's own vector rather than a fixture's invention.
/// - **Every frame differs from its neighbours**, so a test that receives audio can say *which*
///   frames arrived and in what order — which is what makes a truncated response (ORB-8)
///   distinguishable from a late one.
#[must_use]
pub fn tone_frame(index: usize) -> [u8; FRAME_BYTES] {
    let mut frame = [0u8; FRAME_BYTES];
    let base = index.wrapping_mul(FRAME_BYTES);
    for (offset, byte) in frame.iter_mut().enumerate() {
        *byte = u8::try_from(base.wrapping_add(offset) % 256).unwrap_or_default();
    }
    frame
}

/// The first `frames` frames of the tone, concatenated — what a bridge should hand to the media
/// path after receiving that many deltas.
#[must_use]
pub fn tone_bytes(frames: usize) -> Vec<u8> {
    (0..frames).flat_map(tone_frame).collect()
}

// ------------------------------------------------------------------------------ the record ----

/// How one upgrade attempt was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeOutcome {
    /// The bearer matched and the peer replied 101.
    Accepted,
    /// The bearer was wrong or absent and the peer replied with this status before the 101,
    /// which is what §6 records as `AuthRefused`.
    Refused(u16),
}

/// What one upgrade attempt carried.
///
/// The `Authorization` header is kept verbatim because ORB-1 asserts that the resolved secret's
/// *bytes* reached the wire, and an assertion against a redacted record could not fail. Nothing
/// here is logged; the value only ever lives in a test's memory, and only fixture keys are ever
/// presented to this peer.
#[derive(Debug, Clone)]
pub struct Upgrade {
    /// The request target as sent — path and query, so ORB-1 can read `?model=` from it.
    pub target: String,
    /// The `Authorization` header verbatim, or `None` when the request carried none.
    pub authorization: Option<String>,
    /// Every header name the request carried, lowercased. ORB-1 asserts the retired beta header
    /// is *absent*, which is only evidence if the peer would have seen it.
    pub header_names: Vec<String>,
    /// How the peer answered.
    pub outcome: UpgradeOutcome,
}

/// A client event as the peer read it.
///
/// The three named variants are spec §5.1's exhaustive client subset. The other two exist so
/// that a bridge sending anything else is a visible fact rather than an absence: ORB-5's claim is
/// that *only* the three arrive, and a record that could not represent a fourth would prove it
/// vacuously.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// `session.update`, kept whole — a test reads the session object from it to check the
    /// formats §3 pins to the call's negotiated codec.
    SessionUpdate(Value),
    /// `input_audio_buffer.append`, with its `audio` member decoded per RFC 4648 §4.
    Append {
        /// The decoded payload: one 20 ms G.711 frame, per §4.1.
        audio: Vec<u8>,
    },
    /// `response.cancel` — barge-in, per §4.3.
    Cancel,
    /// A JSON event whose `type` is outside §5.1. A bridge that sends one has a defect against
    /// ORB-5.
    Outside {
        /// The `type` member as it arrived.
        event_type: String,
    },
    /// A frame the peer could not read as a client event at all: not JSON, no string `type`, or
    /// binary.
    Unreadable {
        /// What was wrong with it, for the failure message.
        reason: String,
    },
}

/// Everything the peer observed, and everything it emitted.
///
/// A snapshot: [`RealtimePeer::record`] clones it, so a test reads a consistent picture rather
/// than a moving one. The counts span the peer's whole lifetime and every connection it served,
/// which is what lets ORB-16 assert that no *second* upgrade was attempted.
#[derive(Debug, Clone, Default)]
pub struct Record {
    /// Every upgrade attempt, in order.
    pub upgrades: Vec<Upgrade>,
    /// Every client event, in order.
    pub client_events: Vec<ClientEvent>,
    /// Every appended audio byte, concatenated in arrival order — the uplink as the far end
    /// heard it.
    pub appended_audio: Vec<u8>,
    /// RFC 6455 Ping frames received. Liveness is the client's timer; this is how a test proves
    /// the probe arrived at all.
    pub pings: usize,
    /// `response.output_audio.delta` events actually written to the socket.
    pub deltas_sent: usize,
    /// Deltas a test directed that the peer refused to send because the response had been
    /// cancelled ([`CancelPolicy::Truncate`]).
    pub deltas_suppressed: usize,
    /// Connections that have ended, however they ended.
    pub sessions_ended: usize,
}

impl Record {
    /// How many `input_audio_buffer.append` events arrived.
    #[must_use]
    pub fn appends(&self) -> usize {
        self.client_events
            .iter()
            .filter(|event| matches!(event, ClientEvent::Append { .. }))
            .count()
    }

    /// How many `response.cancel` events arrived.
    #[must_use]
    pub fn cancels(&self) -> usize {
        self.client_events
            .iter()
            .filter(|event| matches!(event, ClientEvent::Cancel))
            .count()
    }

    /// Every `session.update` event, whole.
    #[must_use]
    pub fn session_updates(&self) -> Vec<&Value> {
        self.client_events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::SessionUpdate(update) => Some(update),
                _ => None,
            })
            .collect()
    }

    /// Everything the client sent that spec §5.1 does not allow it to send.
    ///
    /// ORB-5 passes exactly when this is empty.
    #[must_use]
    pub fn events_outside_the_client_subset(&self) -> Vec<String> {
        self.client_events
            .iter()
            .filter_map(|event| match event {
                ClientEvent::Outside { event_type } => Some(event_type.clone()),
                ClientEvent::Unreadable { reason } => Some(format!("unreadable: {reason}")),
                _ => None,
            })
            .collect()
    }

    /// Upgrades the peer accepted.
    #[must_use]
    pub fn accepted(&self) -> usize {
        self.outcomes(UpgradeOutcome::Accepted)
    }

    /// Upgrades the peer refused, whatever the status.
    #[must_use]
    pub fn refused(&self) -> usize {
        self.upgrades
            .iter()
            .filter(|upgrade| matches!(upgrade.outcome, UpgradeOutcome::Refused(_)))
            .count()
    }

    fn outcomes(&self, outcome: UpgradeOutcome) -> usize {
        self.upgrades
            .iter()
            .filter(|upgrade| upgrade.outcome == outcome)
            .count()
    }
}

// -------------------------------------------------------------------- the configured modes ----

/// Which setup acknowledgement the peer withholds (ORB-15).
///
/// §3 gives each a 10 s bound, and `SetupTimeout` is the outcome when one is missed. Withholding
/// is not silence on the socket: the peer keeps reading, so the client's timer is the only thing
/// that can end the session, which is exactly the claim the vector makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Withhold {
    /// Answer setup the way the spec says.
    #[default]
    Nothing,
    /// Never send `session.created`.
    SessionCreated,
    /// Never send `session.updated`, however many `session.update` events arrive.
    SessionUpdated,
}

/// Where the peer stops serving the socket while holding it open (ORB-14).
///
/// A stall is not a close: the connection stays established and unread, so no Pong is ever
/// written and the client's liveness timer (§6: 30 s + 10 s) is the only thing that can end it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallPoint {
    /// Answer the upgrade, then nothing at all — not even `session.created`.
    Upgrade,
    /// Complete setup, then go silent mid-call.
    Session,
}

/// What the peer does with a response after the client cancels it (§4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CancelPolicy {
    /// Send no further delta for the cancelled response: a directed delta is suppressed and
    /// counted in [`Record::deltas_suppressed`]. This is what lets a bridge test assert
    /// truncation as a fact.
    #[default]
    Truncate,
    /// Keep sending directed deltas after the cancel. ORB-8's script needs this: the spec claims
    /// no bound on how many deltas arrive after a cancel, because that number is the peer's, and
    /// `bridge_cancelled_deltas` counts exactly the events "the peer chose to send".
    KeepStreaming,
}

/// One frame the peer sends that no one can read as an event (§5.3, ORB-13 and ORB-18).
///
/// Each variant is one row of the read-set rule, and each is a real frame on the wire rather
/// than an error injected into the client.
#[derive(Debug, Clone)]
pub enum Malformed {
    /// `not json{` — a text frame that is not JSON at all.
    NotJson,
    /// A JSON object with no `type` member.
    NoType,
    /// A binary frame. Every event in this contract is a JSON text frame.
    Binary,
    /// A `response.output_audio.delta` whose `delta` is `not base64!!`.
    DeltaNotBase64 {
        /// The response the delta claims to belong to.
        response: String,
    },
    /// A `response.output_audio.delta` with no `delta` member at all.
    DeltaMissing {
        /// The response the delta claims to belong to.
        response: String,
    },
    /// A `response.output_audio.done` with no `response_id`.
    AudioDoneWithoutResponseId,
}

/// What became of a directed event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Emission {
    /// The frame was written to the socket before this result was returned.
    Sent,
    /// The response had been cancelled and [`CancelPolicy::Truncate`] is in force, so nothing
    /// was written.
    SuppressedByCancel,
}

/// Everything that can go wrong driving the peer.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// The loopback listener could not be bound.
    #[error("the stand-in peer could not bind a loopback listener: {0}")]
    Bind(String),
    /// No client is connected, so there is nothing to send the directive to.
    #[error("the stand-in peer has no connected session")]
    NoSession,
    /// The connection ended before the directive could be performed.
    #[error("the stand-in peer's session ended before the directive was performed")]
    SessionEnded,
    /// The record never satisfied the condition within [`OBSERVATION_BOUND`].
    #[error("the stand-in peer did not observe {what} within {OBSERVATION_BOUND:?}")]
    NotObserved {
        /// What the caller was waiting for, in its own words.
        what: String,
    },
}

// ------------------------------------------------------------------------- the peer itself ----

/// How the peer behaves, before it is started.
#[derive(Debug, Clone)]
pub struct PeerConfig {
    bearer: String,
    withhold: Withhold,
    stall: Option<StallPoint>,
    cancel: CancelPolicy,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerConfig {
    /// A peer that behaves: [`FIXTURE_BEARER`], both acknowledgements, no stall, cancel honoured.
    #[must_use]
    pub fn new() -> Self {
        Self {
            bearer: FIXTURE_BEARER.to_owned(),
            withhold: Withhold::Nothing,
            stall: None,
            cancel: CancelPolicy::Truncate,
        }
    }

    /// The bearer the upgrade must carry verbatim. Anything else is refused 401 (ORB-10).
    #[must_use]
    pub fn expecting_bearer(mut self, bearer: &str) -> Self {
        bearer.clone_into(&mut self.bearer);
        self
    }

    /// Withhold one of setup's acknowledgements (ORB-15).
    #[must_use]
    pub fn withholding(mut self, withhold: Withhold) -> Self {
        self.withhold = withhold;
        self
    }

    /// Go silent at this point while holding the socket open (ORB-14).
    #[must_use]
    pub fn stalling_at(mut self, stall: StallPoint) -> Self {
        self.stall = Some(stall);
        self
    }

    /// What to do with directed deltas after the client cancels (§4.3).
    #[must_use]
    pub fn on_cancel(mut self, cancel: CancelPolicy) -> Self {
        self.cancel = cancel;
        self
    }

    /// Bind a loopback listener and start serving.
    ///
    /// The peer serves connections until it is dropped or [`RealtimePeer::shutdown`] is awaited.
    pub async fn start(self) -> Result<RealtimePeer, PeerError> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|error| PeerError::Bind(error.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|error| PeerError::Bind(error.to_string()))?;
        let shared = Arc::new(Shared::default());
        let shutdown = CancellationToken::new();
        let accepting = tokio::spawn(accept(
            listener,
            self,
            Arc::clone(&shared),
            shutdown.clone(),
        ));
        Ok(RealtimePeer {
            url: format!("ws://{addr}/v1/realtime"),
            addr,
            shared,
            shutdown,
            accepting: Some(accepting),
        })
    }
}

/// A running stand-in peer.
///
/// Dropping it cancels the listener and every session it is serving; there is no way to leave one
/// running past the end of a test, which is what keeps a suite's ports and tasks bounded.
#[derive(Debug)]
pub struct RealtimePeer {
    url: String,
    addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
    accepting: Option<JoinHandle<()>>,
}

impl Drop for RealtimePeer {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

impl RealtimePeer {
    /// The URL a client connects to, without the `?model=` the client is expected to add.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The loopback address the peer is listening on.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A snapshot of everything observed so far.
    #[must_use]
    pub fn record(&self) -> Record {
        self.shared.snapshot()
    }

    /// Wait until the record satisfies `condition`, and return it.
    ///
    /// The wait completes on the record changing, never on a clock; `what` names the observation
    /// so a peer that never produces it fails with a sentence rather than with a hang.
    pub async fn observe<F>(&self, what: &str, condition: F) -> Result<Record, PeerError>
    where
        F: Fn(&Record) -> bool,
    {
        // OBSERVATION_BOUND bounds a failure — how long to wait before concluding the
        // observation is not coming. The loop below completes on the notification, so this
        // duration orders nothing.
        tokio::time::timeout(OBSERVATION_BOUND, async {
            loop {
                let changed = self.shared.changed.notified();
                {
                    let record = self.shared.lock();
                    if condition(&record) {
                        return record.clone();
                    }
                }
                changed.await;
            }
        })
        .await
        .map_err(|_elapsed| PeerError::NotObserved {
            what: what.to_owned(),
        })
    }

    /// Wait until at least one upgrade has been answered, either way.
    pub async fn await_upgrade(&self) -> Result<Record, PeerError> {
        self.observe("an upgrade", |record| !record.upgrades.is_empty())
            .await
    }

    /// Wait until the client's `session.update` has been read.
    pub async fn await_session_update(&self) -> Result<Record, PeerError> {
        self.observe("a session.update", |record| {
            !record.session_updates().is_empty()
        })
        .await
    }

    /// Wait until `count` uplink frames have been appended.
    pub async fn await_appends(&self, count: usize) -> Result<Record, PeerError> {
        self.observe(&format!("{count} appends"), move |record| {
            record.appends() >= count
        })
        .await
    }

    /// Wait until the client has cancelled a response.
    pub async fn await_cancel(&self) -> Result<Record, PeerError> {
        self.observe("a response.cancel", |record| record.cancels() > 0)
            .await
    }

    /// Send one `response.output_audio.delta` carrying these bytes, base64 per RFC 4648 §4.
    pub async fn send_delta(&self, response: &str, audio: &[u8]) -> Result<Emission, PeerError> {
        self.direct(Action::Delta {
            response: response.to_owned(),
            audio: audio.to_vec(),
        })
        .await
    }

    /// Speak `frames` frames of the tone as one delta each, stopping at the first one the peer
    /// suppresses. Returns how many reached the socket.
    pub async fn speak_tone(&self, response: &str, frames: usize) -> Result<usize, PeerError> {
        let mut sent = 0;
        for frame in 0..frames {
            if self.send_delta(response, &tone_frame(frame)).await? == Emission::SuppressedByCancel
            {
                break;
            }
            sent += 1;
        }
        Ok(sent)
    }

    /// Send `response.output_audio.done`, which is where §4.1's partial-frame padding happens.
    pub async fn send_audio_done(&self, response: &str) -> Result<Emission, PeerError> {
        self.direct(Action::Scripted(Scripted::AudioDone {
            response: Some(response.to_owned()),
        }))
        .await
    }

    /// Send `response.done` with this status, ending any cancel-race window (§4.3).
    pub async fn send_response_done(
        &self,
        response: &str,
        status: &str,
    ) -> Result<Emission, PeerError> {
        self.direct(Action::Scripted(Scripted::ResponseDone {
            response: response.to_owned(),
            status: status.to_owned(),
        }))
        .await
    }

    /// Send `input_audio_buffer.speech_started`, the barge-in trigger of §4.3.
    pub async fn send_speech_started(&self) -> Result<Emission, PeerError> {
        self.direct(Action::Scripted(Scripted::SpeechStarted)).await
    }

    /// Send an `error` event (§5.2), which is either the cancel race or session-fatal.
    pub async fn send_error(&self, code: &str, message: &str) -> Result<Emission, PeerError> {
        self.direct(Action::Scripted(Scripted::Error {
            code: code.to_owned(),
            message: message.to_owned(),
        }))
        .await
    }

    /// Send an event outside §5.2 — what ORB-12 requires be ignored with a counter.
    pub async fn send_unknown(&self, event_type: &str) -> Result<Emission, PeerError> {
        self.direct(Action::Scripted(Scripted::Unknown {
            event_type: event_type.to_owned(),
        }))
        .await
    }

    /// Send a frame that cannot be read as an event (§5.3).
    pub async fn send_malformed(&self, malformed: Malformed) -> Result<Emission, PeerError> {
        self.direct(Action::Malformed(malformed)).await
    }

    /// Send one text frame of at least `bytes` bytes — §5.3's 1 MiB bound is 1 048 576 (ORB-11).
    pub async fn send_oversize(&self, bytes: usize) -> Result<Emission, PeerError> {
        self.direct(Action::Scripted(Scripted::Oversize { bytes }))
            .await
    }

    /// Close the connection with 1000, the way a well-behaved far end ends a session (ORB-16).
    pub async fn close_normally(&self) -> Result<Emission, PeerError> {
        self.direct(Action::Close { code: 1000 }).await
    }

    /// Reset the connection: no close frame, no close handshake (ORB-16's second half).
    pub async fn reset(&self) -> Result<Emission, PeerError> {
        self.direct(Action::Reset).await
    }

    /// Stop serving and join every task the peer owns.
    ///
    /// Dropping the peer does the same thing without the join; this exists for a test that wants
    /// to prove there is no orphan left behind.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        if let Some(accepting) = self.accepting.take() {
            let _joined = accepting.await;
        }
    }

    async fn direct(&self, action: Action) -> Result<Emission, PeerError> {
        let session = self.shared.session().ok_or(PeerError::NoSession)?;
        let (done, performed) = oneshot::channel();
        session
            .send(Directive { action, done })
            .await
            .map_err(|_closed| PeerError::SessionEnded)?;
        performed.await.map_err(|_closed| PeerError::SessionEnded)
    }
}

// ------------------------------------------------------------------------------- internals ----

/// The record, its change notification, and the inbox of the connection directives go to.
#[derive(Debug, Default)]
struct Shared {
    record: Mutex<Record>,
    changed: Notify,
    session: Mutex<Option<(u64, mpsc::Sender<Directive>)>>,
    generations: Mutex<u64>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Record> {
        self.record.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update<F: FnOnce(&mut Record)>(&self, edit: F) {
        edit(&mut self.lock());
        self.changed.notify_waiters();
    }

    fn snapshot(&self) -> Record {
        self.lock().clone()
    }

    /// Make this connection the one directives go to, and return its generation.
    ///
    /// The most recent connection wins. A peer serving two at once is a fixture serving a client
    /// that reconnected when it should not have (ORB-16), and the record — which counts every
    /// upgrade — is what that vector asserts on, not this slot.
    fn register(&self, sender: mpsc::Sender<Directive>) -> u64 {
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *generations += 1;
        let generation = *generations;
        *self.session.lock().unwrap_or_else(PoisonError::into_inner) = Some((generation, sender));
        generation
    }

    fn unregister(&self, generation: u64) {
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        if session
            .as_ref()
            .is_some_and(|(open, _)| *open == generation)
        {
            *session = None;
        }
    }

    fn session(&self) -> Option<mpsc::Sender<Directive>> {
        self.session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|(_, sender)| sender.clone())
    }
}

/// One thing a test asked the peer to put on the socket.
#[derive(Debug)]
struct Directive {
    action: Action,
    done: oneshot::Sender<Emission>,
}

#[derive(Debug)]
enum Action {
    /// Downlink audio, which is the one action the cancel policy can withhold.
    Delta { response: String, audio: Vec<u8> },
    /// An event whose whole behaviour is the JSON it puts on the socket.
    Scripted(Scripted),
    /// A frame nobody can read as an event (§5.3).
    Malformed(Malformed),
    /// A close frame carrying this code.
    Close { code: u16 },
    /// An abrupt end with no close handshake at all.
    Reset,
}

#[derive(Debug)]
enum Scripted {
    AudioDone { response: Option<String> },
    ResponseDone { response: String, status: String },
    SpeechStarted,
    Error { code: String, message: String },
    Unknown { event_type: String },
    Oversize { bytes: usize },
}

/// The socket the peer serves. Not split into a sink and a stream: the connection never reads and
/// writes at once, and keeping it whole is what leaves [`TcpStream::set_linger`] reachable for
/// the abrupt reset.
type Socket = tokio_tungstenite::WebSocketStream<TcpStream>;

/// What the client's last `response.cancel` applies to.
///
/// `response.cancel` carries no `response_id` (§5.1): the in-progress response is the target. So
/// the peer resolves it against what it was sending at the time, and a cancel that arrives with
/// nothing in flight — which §4.3 permits — applies to whatever response starts next rather than
/// being lost.
#[derive(Debug, Default, PartialEq, Eq)]
enum Cancelled {
    #[default]
    No,
    Pending,
    Response(String),
}

/// One connection's protocol state.
#[derive(Debug, Default)]
struct Session {
    /// The response the last delta belonged to.
    in_flight: Option<String>,
    /// The outstanding cancel, if any.
    cancelled: Cancelled,
    events: u32,
}

impl Session {
    fn next_event_id(&mut self) -> String {
        self.events += 1;
        format!("event_{:03}", self.events)
    }

    /// Whether a delta for this response must be withheld.
    fn is_cancelled(&self, response: &str) -> bool {
        match &self.cancelled {
            Cancelled::No => false,
            Cancelled::Pending => true,
            Cancelled::Response(cancelled) => cancelled == response,
        }
    }
}

async fn accept(
    listener: TcpListener,
    config: PeerConfig,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
) {
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => {
                let Ok((stream, _from)) = accepted else { break };
                sessions.spawn(serve(
                    stream,
                    config.clone(),
                    Arc::clone(&shared),
                    shutdown.child_token(),
                ));
            }
            Some(_finished) = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    // Cancellation reaches the sessions through the shared token; this joins them so a peer that
    // has been shut down leaves no task behind.
    sessions.shutdown().await;
}

#[allow(clippy::result_large_err)] // the handshake callback's error is tungstenite's own response type
async fn serve(
    stream: TcpStream,
    config: PeerConfig,
    shared: Arc<Shared>,
    shutdown: CancellationToken,
) {
    let expected = format!("Bearer {}", config.bearer);
    let inspecting = Arc::clone(&shared);
    let upgraded = accept_hdr_async(stream, move |request: &Request, response: Response| {
        inspect_upgrade(request, response, &expected, &inspecting)
    })
    .await;
    let Ok(mut socket) = upgraded else {
        // A refusal is already in the record, written by the callback that decided it.
        return;
    };

    if config.stall == Some(StallPoint::Upgrade) {
        // Answer the upgrade and nothing else, holding the connection open (ORB-14). Nothing is
        // read, so tungstenite never writes the Pong it would otherwise queue.
        shutdown.cancelled().await;
        return;
    }

    let (directives, mut inbox) = mpsc::channel::<Directive>(16);
    // The connection holds a sender of its own, so a later connection replacing it in the shared
    // slot cannot make this one's inbox look closed.
    let _own = directives.clone();
    let generation = shared.register(directives);
    let mut session = Session::default();

    if config.withhold != Withhold::SessionCreated {
        let created = json!({
            "type": "session.created",
            "event_id": session.next_event_id(),
            "session": {"id": "sess_fixture", "object": "realtime.session", "type": "realtime"},
        });
        if write(&mut socket, created).await.is_break() {
            finish(&shared, generation);
            return;
        }
    }

    loop {
        let step = tokio::select! {
            biased;
            () = shutdown.cancelled() => Step::Stop,
            frame = socket.next() => Step::Inbound(frame),
            directive = inbox.recv() => Step::Directed(directive),
        };
        let flow = match step {
            Step::Stop | Step::Directed(None) => ControlFlow::Break(()),
            Step::Inbound(frame) => {
                inbound(frame, &mut socket, &config, &shared, &mut session).await
            }
            Step::Directed(Some(directive)) => {
                directed(directive, &mut socket, &config, &shared, &mut session).await
            }
        };
        if flow.is_break() {
            break;
        }
    }
    finish(&shared, generation);
}

/// What the connection loop woke for.
enum Step {
    Stop,
    Inbound(Option<Result<Message, tokio_tungstenite::tungstenite::Error>>),
    Directed(Option<Directive>),
}

fn finish(shared: &Arc<Shared>, generation: u64) {
    shared.unregister(generation);
    shared.update(|record| record.sessions_ended += 1);
}

// The error variant is the HTTP response tungstenite will write; its size is that crate's shape,
// not a choice available here.
#[allow(clippy::result_large_err)]
fn inspect_upgrade(
    request: &Request,
    response: Response,
    expected: &str,
    shared: &Arc<Shared>,
) -> Result<Response, ErrorResponse> {
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let authorised = authorization.as_deref() == Some(expected);
    let upgrade = Upgrade {
        target: request.uri().to_string(),
        authorization,
        header_names: request
            .headers()
            .keys()
            .map(|name| name.as_str().to_owned())
            .collect(),
        outcome: if authorised {
            UpgradeOutcome::Accepted
        } else {
            UpgradeOutcome::Refused(401)
        },
    };
    shared.update(move |record| record.upgrades.push(upgrade));
    if authorised {
        Ok(response)
    } else {
        // §6: the upgrade is refused with a 4xx before the 101, and §2 forbids any outcome from
        // carrying the credential — so this body names neither what was presented nor what was
        // expected.
        let mut refusal = ErrorResponse::new(Some(
            "invalid_request_error: the bearer token is missing or does not match".to_owned(),
        ));
        *refusal.status_mut() = StatusCode::UNAUTHORIZED;
        Err(refusal)
    }
}

async fn inbound(
    frame: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    socket: &mut Socket,
    config: &PeerConfig,
    shared: &Arc<Shared>,
    session: &mut Session,
) -> ControlFlow<()> {
    match frame {
        Some(Ok(Message::Text(text))) => {
            let event = read_client_event(&text);
            match &event {
                ClientEvent::Cancel => {
                    session.cancelled = session
                        .in_flight
                        .clone()
                        .map_or(Cancelled::Pending, Cancelled::Response);
                }
                ClientEvent::Append { audio } => {
                    let audio = audio.clone();
                    shared.update(|record| record.appended_audio.extend_from_slice(&audio));
                }
                _ => {}
            }
            let reply_wanted = matches!(event, ClientEvent::SessionUpdate(_))
                && config.withhold != Withhold::SessionUpdated;
            shared.update(move |record| record.client_events.push(event));
            if config.stall == Some(StallPoint::Session) {
                // Mid-call silence (ORB-14): the event was read and recorded, and nothing is
                // written from here on. The connection stays open until the peer is dropped.
                return ControlFlow::Break(());
            }
            if reply_wanted {
                let updated = json!({
                    "type": "session.updated",
                    "event_id": session.next_event_id(),
                    "session": {"id": "sess_fixture", "object": "realtime.session", "type": "realtime"},
                });
                return write(socket, updated).await;
            }
            ControlFlow::Continue(())
        }
        Some(Ok(Message::Binary(bytes))) => {
            shared.update(move |record| {
                record.client_events.push(ClientEvent::Unreadable {
                    reason: format!("a binary frame of {} bytes", bytes.len()),
                });
            });
            ControlFlow::Continue(())
        }
        Some(Ok(Message::Ping(_))) => {
            shared.update(|record| record.pings += 1);
            // tungstenite queues the Pong itself and writes it on the next read or write; this
            // flush is what makes "answered" true now rather than at the next frame.
            let _flushed = socket.flush().await;
            ControlFlow::Continue(())
        }
        Some(Ok(Message::Pong(_) | Message::Frame(_))) => ControlFlow::Continue(()),
        Some(Ok(Message::Close(_)) | Err(_)) | None => ControlFlow::Break(()),
    }
}

fn read_client_event(text: &str) -> ClientEvent {
    let Ok(event) = serde_json::from_str::<Value>(text) else {
        return ClientEvent::Unreadable {
            reason: "a text frame that is not JSON".to_owned(),
        };
    };
    let Some(event_type) = event.get("type").and_then(Value::as_str) else {
        return ClientEvent::Unreadable {
            reason: "a JSON frame with no string `type`".to_owned(),
        };
    };
    match event_type {
        "session.update" => ClientEvent::SessionUpdate(event.clone()),
        "response.cancel" => ClientEvent::Cancel,
        "input_audio_buffer.append" => match event
            .get("audio")
            .and_then(Value::as_str)
            .map(|audio| BASE64.decode(audio))
        {
            Some(Ok(audio)) => ClientEvent::Append { audio },
            Some(Err(error)) => ClientEvent::Unreadable {
                reason: format!("an append whose audio is not RFC 4648 §4 base64: {error}"),
            },
            None => ClientEvent::Unreadable {
                reason: "an append with no string `audio` member".to_owned(),
            },
        },
        other => ClientEvent::Outside {
            event_type: other.to_owned(),
        },
    }
}

async fn directed(
    directive: Directive,
    socket: &mut Socket,
    config: &PeerConfig,
    shared: &Arc<Shared>,
    session: &mut Session,
) -> ControlFlow<()> {
    let Directive { action, done } = directive;
    match action {
        Action::Delta { response, audio } => {
            if config.cancel == CancelPolicy::Truncate && session.is_cancelled(&response) {
                shared.update(|record| record.deltas_suppressed += 1);
                let _answered = done.send(Emission::SuppressedByCancel);
                return ControlFlow::Continue(());
            }
            session.in_flight = Some(response.clone());
            let delta = json!({
                "type": "response.output_audio.delta",
                "event_id": session.next_event_id(),
                "response_id": response,
                "item_id": "item_fixture",
                "output_index": 0,
                "content_index": 0,
                "delta": BASE64.encode(&audio),
            });
            let flow = write(socket, delta).await;
            if flow.is_continue() {
                shared.update(|record| record.deltas_sent += 1);
            }
            answer(flow, done)
        }
        Action::Scripted(scripted) => {
            let event = scripted_event(scripted, session);
            answer(write(socket, event).await, done)
        }
        Action::Malformed(malformed) => malformed_frame(malformed, socket, session, done).await,
        Action::Close { code } => {
            let close = Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: Utf8Bytes::from_static("session ended"),
            }));
            let sent = socket.send(close).await.is_ok();
            let _flushed = socket.flush().await;
            let _answered = done.send(Emission::Sent);
            // The connection ends when the client's close echo arrives, which the inbound arm
            // reads; breaking here would drop the socket before the echo had anywhere to go.
            if sent {
                ControlFlow::Continue(())
            } else {
                ControlFlow::Break(())
            }
        }
        Action::Reset => {
            // A zero linger turns the close into an RST: no close frame, no handshake, which is
            // the second half of ORB-16 and the one a graceful shutdown cannot produce.
            //
            // tokio deprecates `set_linger` because a *non-zero* linger makes closing block the
            // thread that drops the socket. Zero is the opposite case — the send buffer is
            // discarded and the far end is told so immediately — which is exactly what this
            // vector needs and what no other safe API in the workspace can produce.
            #[allow(deprecated)]
            let _lingered = socket.get_ref().set_linger(Some(Duration::ZERO));
            let _answered = done.send(Emission::Sent);
            ControlFlow::Break(())
        }
    }
}

/// Build the event for an action whose whole behaviour is the JSON it puts on the socket.
///
/// Each carries members beyond the read set of §5.2 — `item_id`, `output_index`, `object` — for
/// the same reason the spec's own vectors do: the bridge must read what it names and ignore the
/// rest, and a peer that sent only the read set could not tell the two apart.
fn scripted_event(scripted: Scripted, session: &mut Session) -> Value {
    match scripted {
        Scripted::AudioDone { response } => {
            let mut event = json!({
                "type": "response.output_audio.done",
                "event_id": session.next_event_id(),
                "item_id": "item_fixture",
                "output_index": 0,
                "content_index": 0,
            });
            if let (Some(response), Some(members)) = (response, event.as_object_mut()) {
                members.insert("response_id".to_owned(), Value::String(response));
            }
            event
        }
        Scripted::ResponseDone { response, status } => {
            session.in_flight = None;
            session.cancelled = Cancelled::No;
            json!({
                "type": "response.done",
                "event_id": session.next_event_id(),
                "response": {"id": response, "object": "realtime.response", "status": status},
            })
        }
        Scripted::SpeechStarted => json!({
            "type": "input_audio_buffer.speech_started",
            "event_id": session.next_event_id(),
            "audio_start_ms": 460,
            "item_id": "item_fixture",
        }),
        Scripted::Error { code, message } => json!({
            "type": "error",
            "event_id": session.next_event_id(),
            "error": {
                "type": "invalid_request_error",
                "code": code,
                "message": message,
                "param": Value::Null,
            },
        }),
        Scripted::Unknown { event_type } => {
            json!({"type": event_type, "event_id": session.next_event_id()})
        }
        Scripted::Oversize { bytes } => {
            // A well-formed delta whose payload alone exceeds the bound, so the client's limit is
            // tested on the frame's size and not on its shape (§5.3: the bound is enforced before
            // any JSON parsing).
            json!({
                "type": "response.output_audio.delta",
                "event_id": session.next_event_id(),
                "response_id": "resp_oversize",
                "delta": "A".repeat(bytes.next_multiple_of(4)),
            })
        }
    }
}

async fn malformed_frame(
    malformed: Malformed,
    socket: &mut Socket,
    session: &mut Session,
    done: oneshot::Sender<Emission>,
) -> ControlFlow<()> {
    let flow = match malformed {
        Malformed::NotJson => socket
            .send(Message::Text(Utf8Bytes::from_static("not json{")))
            .await
            .map_or(ControlFlow::Break(()), |()| ControlFlow::Continue(())),
        Malformed::NoType => {
            let event = json!({"event_id": session.next_event_id(), "session": {}});
            write(socket, event).await
        }
        Malformed::Binary => socket
            .send(Message::Binary(WsBytes::from_static(b"\x00\x01binary")))
            .await
            .map_or(ControlFlow::Break(()), |()| ControlFlow::Continue(())),
        Malformed::DeltaNotBase64 { response } => {
            let event = json!({
                "type": "response.output_audio.delta",
                "event_id": session.next_event_id(),
                "response_id": response,
                "delta": "not base64!!",
            });
            write(socket, event).await
        }
        Malformed::DeltaMissing { response } => {
            let event = json!({
                "type": "response.output_audio.delta",
                "event_id": session.next_event_id(),
                "response_id": response,
                "item_id": "item_fixture",
            });
            write(socket, event).await
        }
        Malformed::AudioDoneWithoutResponseId => {
            let event = json!({
                "type": "response.output_audio.done",
                "event_id": session.next_event_id(),
                "item_id": "item_fixture",
            });
            write(socket, event).await
        }
    };
    answer(flow, done)
}

/// Resolve the directive that produced this write.
///
/// A failed write drops the sender rather than answering it, so the caller sees
/// [`PeerError::SessionEnded`] instead of a claim that a frame reached a socket which had
/// already gone.
fn answer(flow: ControlFlow<()>, done: oneshot::Sender<Emission>) -> ControlFlow<()> {
    if flow.is_continue() {
        let _answered = done.send(Emission::Sent);
    }
    flow
}

/// Write one event as a JSON text frame, flushing it so the directive that asked for it resolves
/// after the bytes are on the socket rather than before.
async fn write(socket: &mut Socket, event: Value) -> ControlFlow<()> {
    if socket
        .send(Message::Text(event.to_string().into()))
        .await
        .is_err()
    {
        return ControlFlow::Break(());
    }
    match socket.flush().await {
        Ok(()) => ControlFlow::Continue(()),
        Err(_) => ControlFlow::Break(()),
    }
}

impl fmt::Display for Emission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Sent => "sent",
            Self::SuppressedByCancel => "suppressed by cancel",
        })
    }
}
