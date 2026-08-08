//! The realtime bridge: one call leg, one realtime session, one WebSocket between them.
//!
//! **Supported** (`A-22`): [`crate::host`] owns this component for every configured realtime call,
//! and the shipped `sipx-host` process exercises that path through SIP and RTP.
//!
//! [`docs/specs/openai-realtime.md`](../../../docs/specs/openai-realtime.md) is normative for
//! everything here and this module is one half of it; `sipx-testkit`'s stand-in peer is the other,
//! so every vector but the live proof runs in the default matrix with no credentials. Where a
//! sentence below cites a section, that section is the contract and this is its reading.
//!
//! **The bridge is byte passthrough.** The call's negotiated G.711 payload leaves
//! [`recv_encoded`](sipx_media::MediaSession::recv_encoded) in relay mode, travels up as the RFC
//! 4648 §4 base64 of exactly those bytes, and the agent's G.711 comes back the same way into
//! [`send_encoded`](sipx_media::MediaSession::send_encoded). Nothing here decodes audio, resamples
//! it, or re-times it: the only transformation on the path is base64, which is exact. A call whose
//! negotiated payload is neither PCMU nor PCMA is refused before a socket is opened (§3), because
//! the alternative is a transcoder this epic deliberately does not have.
//!
//! **Two queues, both bounded, every loss counted** (§5.4). The uplink absorbs write jitter only —
//! 32 frames, 640 ms — so sustained fullness means the socket has stalled and liveness ends the
//! bridge rather than the queue hiding it. The downlink is sized for the far end's shape: a
//! response's audio arrives as a burst far ahead of real time while the media path drains at the
//! RTP clock, so 2048 frames of headroom is 40.96 s. Neither queue ever blocks its producer and
//! neither ever grows: a full queue drops the offered frame, counts it, and the session lives,
//! because media tolerates loss where a control plane would not
//! ([`session-binding.md`](../../../docs/specs/session-binding.md) §3 supplies the discipline; the
//! full-queue policy is the realtime spec's own).
//!
//! **Barge-in has exactly one owner** (§4.3). The session is configured with
//! `interrupt_response: false`, so the far end never cancels itself and the causal chain a test
//! asserts is one chain: `speech_started` arrives, the bridge cancels, empties the queue and the
//! re-framing accumulator together, and drops deltas until the response ends. What that bound
//! cannot cover is audio the *call's* far end has already been sent, which is why the spec states
//! the residual as "≤ 1 frame still ahead of the flush locally" and this module keeps exactly one
//! frame in flight at the media seam rather than a pipeline of them.
//!
//! **A dead socket ends the bridge, and there is no reconnect** (§6). Reconnecting would invent a
//! conversation state the far end no longer has, so every ending is a typed
//! [`BridgeOutcome`] handed back to whoever owns the call. No outcome, log line or error message
//! carries the bearer: the credential is held as bytes that never reach `Debug`, and a refused
//! upgrade is reported by the secret's **name** ([`host-config.md`](../../../docs/specs/host-config.md)
//! N7).
//!
//! **The media seam is a trait** ([`CallAudio`]), implemented for
//! [`MediaSession`] and for nothing else in production. It exists because
//! the queue-depth bounds above are claims about *this* module's arithmetic, and a test that can
//! only observe them through a session draining at the RTP clock cannot assert a number — it can
//! only assert that audio eventually arrived, which is the assertion the spec was written to
//! replace.

use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use futures_util::future::BoxFuture;
use sipx_app_protocol::json::Json;
use sipx_media::{Encoded, MediaSession};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::task::JoinSet;
use tokio::time::{Duration, Instant};

use crate::wss::{WssClient, WssConnection, WssError, WssMessage};

/// The call's 20 ms packet: 160 bytes of G.711 at 8000 Hz, one byte per sample (RFC 3551
/// §4.5.14), which spec §4.1 makes the unit of audio in both directions.
pub const FRAME_BYTES: usize = 160;

/// Spec §5.4's uplink bound: 32 frames, 640 ms of call audio.
pub const UPLINK_QUEUE_FRAMES: usize = 32;

/// Spec §5.4's downlink bound: 2048 frames, 40.96 s of agent audio.
pub const DOWNLINK_QUEUE_FRAMES: usize = 2048;

/// Spec §3's setup bound: `session.created` within this of the upgrade, `session.updated` within
/// this of sending `session.update`.
pub const SETUP_BOUND: Duration = Duration::from_secs(10);

/// Spec §4.3's cancel-race window: how long after a `response.cancel` an `error` is read as the
/// race rather than as a session failure, when no `response.done` closes the window first.
pub const CANCEL_RACE_WINDOW: Duration = Duration::from_secs(10);

/// μ-law digital silence, the byte §4.1 pads a partial frame with on a PCMU call.
pub const MULAW_SILENCE: u8 = 0xFF;

/// A-law digital silence, the byte §4.1 pads a partial frame with on a PCMA call.
pub const ALAW_SILENCE: u8 = 0xD5;

/// The negotiated wire format, in the two forms §3 needs: what the session is told, and what a
/// partial frame is padded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Format {
    /// The `format.type` §3's table maps the negotiated payload to.
    wire: &'static str,
    /// The format's digital silence (§4.1).
    silence: u8,
}

impl Format {
    /// The format for a negotiated payload type, or `None` when the call is not bridgeable.
    ///
    /// Keyed on the **payload type** rather than on [`sipx_media::Codec`] because that is what
    /// §3's table is keyed on, and because `Codec` gains variants behind Cargo features: a match
    /// over it would be exhaustive in one build and not in another, so "neither PCMU nor PCMA"
    /// would mean different things depending on how the host was compiled.
    fn of(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self {
                wire: "audio/pcmu",
                silence: MULAW_SILENCE,
            }),
            8 => Some(Self {
                wire: "audio/pcma",
                silence: ALAW_SILENCE,
            }),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------------- the media seam ----

/// One call leg's audio, in the two directions a bridge needs and nothing else.
///
/// [`MediaSession`] implements it; a test supplies its own so the queue bounds of §5.4 and the
/// residual bound of §4.3 can be asserted as numbers. Both methods return a boxed future rather
/// than being `async fn`, because the bridge holds this behind an `Arc<dyn …>` — one call leg is
/// chosen at run time by the host's configuration, and a generic parameter would push that choice
/// into every type on the path.
pub trait CallAudio: Send + Sync {
    /// The payload type this call negotiated, which §3 maps to the session's audio format.
    fn wire_payload_type(&self) -> u8;

    /// The next payload as it arrived, still encoded, or `None` once the call's media has stopped.
    fn recv_encoded(&self) -> BoxFuture<'_, Option<Encoded>>;

    /// Put one payload on the wire exactly as given. `false` once the call's media has stopped.
    fn send_encoded(&self, encoded: Encoded) -> BoxFuture<'_, bool>;
}

impl CallAudio for MediaSession {
    fn wire_payload_type(&self) -> u8 {
        Self::wire_payload_type(self)
    }

    fn recv_encoded(&self) -> BoxFuture<'_, Option<Encoded>> {
        Box::pin(Self::recv_encoded(self))
    }

    fn send_encoded(&self, encoded: Encoded) -> BoxFuture<'_, bool> {
        Box::pin(Self::send_encoded(self, encoded))
    }
}

// ------------------------------------------------------------------------ the session setup ----

/// A resolved credential that cannot be presented, named by its secret's **name** and nothing
/// else.
///
/// The value is not in this type, not in its `Display` and not in its `Debug`: a secret that is
/// unusable is exactly the one an operator is about to paste into a bug report (N7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusableCredential {
    /// The secret's name, as configuration wrote it.
    pub secret: String,
}

impl fmt::Display for UnusableCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the secret named `{}` resolved to bytes that cannot travel in an Authorization \
             header (RFC 9110 §5.5)",
            self.secret
        )
    }
}

impl std::error::Error for UnusableCredential {}

/// Where the realtime session is, how it is configured, and what authenticates to it.
///
/// Everything but the credential comes from the host document; the credential is resolved from the
/// environment at startup, before any call is admitted, the way
/// [`webhook-binding.md`](../../../docs/specs/webhook-binding.md) §3 has it.
#[derive(Clone)]
pub struct SessionSetup {
    endpoint: String,
    model: String,
    instructions: String,
    secret: String,
    /// The `Authorization` value, whole. Never printed, never compared, never in `Debug`.
    authorization: String,
}

impl fmt::Debug for SessionSetup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The secret's *name* is shape and belongs in a diagnostic; the header built from its
        // value is the one field in this struct that must never be printed anywhere, and a derived
        // `Debug` would put it in whatever log the host writes on its first bad day.
        formatter
            .debug_struct("SessionSetup")
            .field("endpoint", &self.endpoint)
            .field("model", &self.model)
            .field("secret", &self.secret)
            .finish_non_exhaustive()
    }
}

impl SessionSetup {
    /// Configure a session from the document's values and the resolved secret.
    ///
    /// # Errors
    /// [`UnusableCredential`], naming the secret and never its value, when the resolved bytes
    /// cannot be an HTTP header value — an empty secret, a stray newline, a binary blob.
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        instructions: impl Into<String>,
        secret: impl Into<String>,
        key: &[u8],
    ) -> Result<Self, UnusableCredential> {
        let secret = secret.into();
        let refused = || UnusableCredential {
            secret: secret.clone(),
        };
        let key = std::str::from_utf8(key).map_err(|_not_text| refused())?;
        // RFC 9110 §5.5 field values are visible ASCII plus space and horizontal tab. Checked here
        // rather than at the first call, so a key with a trailing newline — how a secret usually
        // arrives out of a file — fails at startup with the name of the file to fix (N8).
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
        {
            return Err(refused());
        }
        Ok(Self {
            endpoint: endpoint.into(),
            model: model.into(),
            instructions: instructions.into(),
            secret,
            authorization: format!("Bearer {key}"),
        })
    }

    /// The name of the secret this session authenticates with — never its value.
    #[must_use]
    pub fn secret(&self) -> &str {
        &self.secret
    }

    /// The URL the upgrade is sent to: the configured endpoint with §2's `?model=`.
    #[must_use]
    pub fn url(&self) -> String {
        let separator = if self.endpoint.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{}{separator}model={}", self.endpoint, self.model)
    }
}

/// The bounds the bridge runs under. Every default is the spec's own number.
///
/// They are settable so a test can drive the failure they bound without waiting out the real one;
/// nothing here orders anything, so shortening one can only make a bridge give up sooner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeLimits {
    /// The uplink queue's depth in frames (§5.4).
    pub uplink_frames: usize,
    /// The downlink queue's depth in frames (§5.4).
    pub downlink_frames: usize,
    /// How long each half of setup may take (§3).
    pub setup_bound: Duration,
    /// How long after a cancel an `error` is the race rather than a failure (§4.3).
    pub cancel_race_window: Duration,
}

impl Default for BridgeLimits {
    fn default() -> Self {
        Self {
            uplink_frames: UPLINK_QUEUE_FRAMES,
            downlink_frames: DOWNLINK_QUEUE_FRAMES,
            setup_bound: SETUP_BOUND,
            cancel_race_window: CANCEL_RACE_WINDOW,
        }
    }
}

// ---------------------------------------------------------------------------- what it counts ----

/// A snapshot of everything §4.3 and §5.4 name a counter for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BridgeCounters {
    /// Uplink frames offered to a full queue (`bridge_uplink_dropped`).
    pub uplink_dropped: u64,
    /// Downlink frames offered to a full queue (`bridge_downlink_dropped`).
    pub downlink_dropped: u64,
    /// Queued agent frames a barge-in threw away (`bridge_barge_in_flushed`). Bounded by the
    /// downlink queue's depth, which is what makes it the number §4.3 asks a test to assert.
    pub barge_in_flushed: u64,
    /// Delta events dropped between a cancel and the response ending (`bridge_cancelled_deltas`).
    /// Deliberately unbounded: how many the far end sends after a cancel is the far end's.
    pub cancelled_deltas: u64,
    /// `error` events classified as the cancel/done race (`bridge_cancel_race`).
    pub cancel_race: u64,
    /// Events outside §5.2, ignored so a vendor addition is not an outage
    /// (`bridge_ignored_events`).
    pub ignored_events: u64,
    /// `input_audio_buffer.append` events written to the socket.
    pub appended: u64,
    /// Frames handed to the media path.
    pub delivered: u64,
}

/// A live view of what a running bridge has counted.
///
/// Held behind an `Arc` by the bridge and by whoever started it, so a host can report progress and
/// a test can assert a queue's depth at the instant a script reaches it — which is the only way to
/// observe §4.3's flush as a number rather than as an eventual silence.
#[derive(Debug, Default)]
pub struct BridgeMeters {
    uplink_dropped: AtomicU64,
    downlink_dropped: AtomicU64,
    barge_in_flushed: AtomicU64,
    cancelled_deltas: AtomicU64,
    cancel_race: AtomicU64,
    ignored_events: AtomicU64,
    appended: AtomicU64,
    delivered: AtomicU64,
    downlink_depth: AtomicUsize,
    accumulator_bytes: AtomicUsize,
}

impl BridgeMeters {
    /// Everything counted so far.
    #[must_use]
    pub fn snapshot(&self) -> BridgeCounters {
        BridgeCounters {
            uplink_dropped: self.uplink_dropped.load(Ordering::Relaxed),
            downlink_dropped: self.downlink_dropped.load(Ordering::Relaxed),
            barge_in_flushed: self.barge_in_flushed.load(Ordering::Relaxed),
            cancelled_deltas: self.cancelled_deltas.load(Ordering::Relaxed),
            cancel_race: self.cancel_race.load(Ordering::Relaxed),
            ignored_events: self.ignored_events.load(Ordering::Relaxed),
            appended: self.appended.load(Ordering::Relaxed),
            delivered: self.delivered.load(Ordering::Relaxed),
        }
    }

    /// How many whole agent frames are queued for the media path right now.
    #[must_use]
    pub fn downlink_depth(&self) -> usize {
        self.downlink_depth.load(Ordering::Relaxed)
    }

    /// How many decoded bytes are in the re-framing accumulator right now — always fewer than
    /// [`FRAME_BYTES`], because a whole frame leaves it immediately (§4.1).
    #[must_use]
    pub fn accumulator_bytes(&self) -> usize {
        self.accumulator_bytes.load(Ordering::Relaxed)
    }
}

// --------------------------------------------------------------------------- how it can end ----

/// Which half of setup was outstanding when its bound elapsed (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    /// The server's `session.created`, owed within the bound of the completed upgrade.
    SessionCreated,
    /// The server's `session.updated`, owed within the bound of our `session.update`.
    SessionUpdated,
}

impl fmt::Display for SetupStep {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::SessionCreated => "session.created",
            Self::SessionUpdated => "session.updated",
        })
    }
}

/// Every way the bridge ends — §6's taxonomy, with one addition named there.
///
/// No variant carries the bearer value or any fragment of the `Authorization` header, and
/// [`AuthRefused`](Self::AuthRefused) carries the secret's *name* precisely so that the one
/// outcome an operator reads while holding a bad key still cannot print it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BridgeOutcome {
    /// The call leg ended; the bridge closed the socket normally (1000).
    CallEnded,
    /// The negotiated payload is neither PCMU nor PCMA, so there is nothing to pass through.
    /// Decided before a socket is opened (§3).
    NotBridgeable {
        /// What the call negotiated instead.
        payload_type: u8,
    },
    /// The peer answered the upgrade with a status rather than a 101 (§6).
    AuthRefused {
        /// The name of the secret that was presented. **Never its value.**
        secret: String,
        /// The status the peer refused with, when it sent one.
        status: Option<u16>,
    },
    /// A setup acknowledgement did not arrive within its bound (§3).
    SetupTimeout {
        /// Which one was owed.
        awaiting: SetupStep,
        /// The bound that elapsed.
        bound: Duration,
    },
    /// The peer closed, or the connection failed once established (§6).
    PeerClosed {
        /// The RFC 6455 §5.5.1 close code, when the peer sent a close frame carrying one. `None`
        /// for an EOF or an abrupt reset, neither of which has one to carry.
        code: Option<u16>,
        /// What the transport reported, when the connection ended without a close handshake.
        /// `None` distinguishes an orderly ending from a broken one, which is the difference
        /// ORB-16's two halves turn on.
        detail: Option<String>,
    },
    /// No Pong within the liveness grace (§6). The first one is terminal: the client's held
    /// messages are gone with it, so reading on would be reporting progress on a dead path.
    PeerStalled {
        /// The grace that elapsed unanswered.
        bound: Duration,
    },
    /// A frame that could not be read as an event of §5.2, or a §5.2 event that failed its read
    /// set (§5.3). Fatal on the first occurrence.
    MalformedEvent {
        /// What could not be read, in the bridge's own words. Never the frame's bytes.
        detail: String,
    },
    /// An inbound message over the configured bound (§5.3), refused by the WSS client before any
    /// JSON was parsed.
    OversizeFrame {
        /// What the peer declared.
        size: usize,
        /// The bound it exceeded.
        limit: usize,
    },
    /// An `error` event outside the cancel-race window (§4.3, §6).
    SessionError {
        /// `error.code`, when the event carried one — an `error` is consumed whatever its
        /// members, so this may be absent (§5.3).
        code: Option<String>,
    },
    /// The host asked the bridge to stop; its tasks were cancelled and joined (§6).
    Cancelled,
    /// The socket was never established: the endpoint could not be dialed, verified or upgraded.
    ///
    /// **§6's table has no row for this**, and calling it [`PeerClosed`](Self::PeerClosed) would
    /// let ORB-16's assertion be satisfied by a name that does not resolve. The spec's own rule —
    /// "when the socket closes *or fails* … the bridge ends with a typed outcome" — is what this
    /// answers; the row is reported against `A-19` rather than invented into `PeerClosed`.
    Unreachable {
        /// Why, in the WSS client's words.
        detail: String,
    },
}

impl fmt::Display for BridgeOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CallEnded => formatter.write_str("the call ended"),
            Self::NotBridgeable { payload_type } => write!(
                formatter,
                "payload type {payload_type} is neither PCMU nor PCMA, so there is nothing to \
                 pass through"
            ),
            Self::AuthRefused { secret, status } => match status {
                Some(status) => write!(
                    formatter,
                    "the endpoint refused the upgrade with {status}; the credential presented was \
                     the secret named `{secret}`"
                ),
                None => write!(
                    formatter,
                    "the endpoint refused the upgrade; the credential presented was the secret \
                     named `{secret}`"
                ),
            },
            Self::SetupTimeout { awaiting, bound } => {
                write!(formatter, "no {awaiting} within {bound:?}")
            }
            Self::PeerClosed { code, detail } => match (code, detail) {
                (Some(code), _) => write!(formatter, "the endpoint closed with {code}"),
                (None, Some(detail)) => {
                    write!(formatter, "the connection to the endpoint failed: {detail}")
                }
                (None, None) => formatter.write_str("the endpoint closed without a code"),
            },
            Self::PeerStalled { bound } => {
                write!(
                    formatter,
                    "the endpoint answered no liveness probe in {bound:?}"
                )
            }
            Self::MalformedEvent { detail } => {
                write!(
                    formatter,
                    "the endpoint sent something unreadable: {detail}"
                )
            }
            Self::OversizeFrame { size, limit } => write!(
                formatter,
                "the endpoint sent {size} bytes against a bound of {limit}"
            ),
            Self::SessionError { code } => match code {
                Some(code) => write!(formatter, "the session failed: {code}"),
                None => formatter.write_str("the session failed"),
            },
            Self::Cancelled => formatter.write_str("the host stopped the bridge"),
            Self::Unreachable { detail } => {
                write!(formatter, "the endpoint could not be reached: {detail}")
            }
        }
    }
}

impl BridgeOutcome {
    /// A short machine-readable name, for a report a script reads.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::CallEnded => "call_ended",
            Self::NotBridgeable { .. } => "not_bridgeable",
            Self::AuthRefused { .. } => "auth_refused",
            Self::SetupTimeout { .. } => "setup_timeout",
            Self::PeerClosed { .. } => "peer_closed",
            Self::PeerStalled { .. } => "peer_stalled",
            Self::MalformedEvent { .. } => "malformed_event",
            Self::OversizeFrame { .. } => "oversize_frame",
            Self::SessionError { .. } => "session_error",
            Self::Cancelled => "cancelled",
            Self::Unreachable { .. } => "unreachable",
        }
    }
}

/// How one bridged call went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeReport {
    /// The one way it ended.
    pub outcome: BridgeOutcome,
    /// What it counted on the way.
    pub counters: BridgeCounters,
}

// ------------------------------------------------------------------------------ the bridge ----

/// One call leg bridged to one realtime session.
#[derive(Debug)]
pub struct RealtimeBridge {
    client: WssClient,
    session: SessionSetup,
    limits: BridgeLimits,
    meters: Arc<BridgeMeters>,
}

impl RealtimeBridge {
    /// A bridge that dials `session` with `client`, under the spec's own bounds.
    #[must_use]
    pub fn new(client: WssClient, session: SessionSetup) -> Self {
        Self::with_limits(client, session, BridgeLimits::default())
    }

    /// A bridge under the caller's bounds.
    #[must_use]
    pub fn with_limits(client: WssClient, session: SessionSetup, limits: BridgeLimits) -> Self {
        Self {
            client,
            session,
            limits,
            meters: Arc::new(BridgeMeters::default()),
        }
    }

    /// What this bridge is counting, readable while it runs.
    #[must_use]
    pub fn meters(&self) -> Arc<BridgeMeters> {
        Arc::clone(&self.meters)
    }

    /// Hold the call and the session together until one of them ends.
    ///
    /// `shutdown` is the host's stop signal; completing it ends the bridge
    /// [`Cancelled`](BridgeOutcome::Cancelled) with every task joined. Dropping this future
    /// instead is also safe — the tasks it owns are aborted with it — but it produces no report.
    pub async fn run(
        self,
        audio: Arc<dyn CallAudio>,
        shutdown: impl Future<Output = ()> + Send,
    ) -> BridgeReport {
        let outcome = self.drive(audio, shutdown).await;
        BridgeReport {
            outcome,
            counters: self.meters.snapshot(),
        }
    }

    async fn drive(
        &self,
        audio: Arc<dyn CallAudio>,
        shutdown: impl Future<Output = ()> + Send,
    ) -> BridgeOutcome {
        let payload_type = audio.wire_payload_type();
        let Some(format) = Format::of(payload_type) else {
            // §3: not bridgeable, and decided before a socket is opened — a transcode-free bridge
            // has nothing to offer a call it cannot pass through, and dialing first would present
            // a credential on a call that was never going to work.
            return BridgeOutcome::NotBridgeable { payload_type };
        };

        let request = match crate::wss::WssRequest::new(self.session.url())
            .header("Authorization", &self.session.authorization)
        {
            Ok(request) => request,
            Err(error) => return unreachable_outcome(&error),
        };
        let connection = match self.client.connect(request).await {
            Ok(connection) => connection,
            Err(WssError::Handshake {
                status: Some(status),
                ..
            }) => {
                return BridgeOutcome::AuthRefused {
                    secret: self.session.secret.clone(),
                    status: Some(status),
                };
            }
            Err(error) => return unreachable_outcome(&error),
        };

        // Every task the bridge owns is created here and joined below, so there is no path out of
        // this function that leaves one forwarding audio between a call and a socket nobody holds.
        let mut tasks = JoinSet::new();
        let downlink = Arc::new(Downlink::new(self.limits.downlink_frames));
        let (uplink_tx, uplink_rx) = mpsc::channel::<Bytes>(self.limits.uplink_frames);
        let (ended_tx, ended_rx) = oneshot::channel::<()>();

        tasks.spawn(pump_uplink(
            Arc::clone(&audio),
            uplink_tx,
            Arc::clone(&self.meters),
            payload_type,
            ended_tx,
        ));
        tasks.spawn(pump_downlink(
            audio,
            Arc::clone(&downlink),
            Arc::clone(&self.meters),
            payload_type,
        ));

        let mut session = Session {
            connection,
            format,
            instructions: self.session.instructions.clone(),
            downlink: Arc::clone(&downlink),
            meters: Arc::clone(&self.meters),
            limits: self.limits,
            in_flight: None,
            dropping_deltas: false,
            cancel_race_until: None,
            configured: false,
        };
        let outcome = session.serve(uplink_rx, ended_rx, shutdown).await;

        // Order matters: the writer is woken so it can see the flag rather than being aborted
        // mid-frame, and `shutdown` then aborts and *joins* whatever is left. Either way nothing
        // outlives this call — `JoinSet` aborts on drop too, so a cancelled `run` is also clean.
        downlink.finish();
        tasks.shutdown().await;
        outcome
    }
}

/// One connection's protocol state, and the loop that serves it.
struct Session {
    connection: WssConnection,
    format: Format,
    /// §3's `instructions`, which are host configuration and travel in the one `session.update`.
    instructions: String,
    downlink: Arc<Downlink>,
    meters: Arc<BridgeMeters>,
    limits: BridgeLimits,
    /// The response the last delta belonged to — what a cancel would target (§4.3).
    in_flight: Option<String>,
    /// Between a barge-in and the response ending, every delta is dropped and counted (§4.3).
    dropping_deltas: bool,
    /// When the cancel-race window closes, if one is open (§4.3).
    cancel_race_until: Option<Instant>,
    /// Whether `session.updated` has landed, which is what admits audio to the socket (§3).
    configured: bool,
}

/// What the serving loop woke for.
enum Step {
    Shutdown,
    CallEnded,
    Uplink(Option<Bytes>),
    Inbound(Result<Option<WssMessage>, WssError>),
}

impl Session {
    async fn serve(
        &mut self,
        mut uplink: mpsc::Receiver<Bytes>,
        ended: oneshot::Receiver<()>,
        shutdown: impl Future<Output = ()> + Send,
    ) -> BridgeOutcome {
        let mut shutdown = std::pin::pin!(shutdown);
        let mut ended = std::pin::pin!(ended);

        // Setup first, each half under its own bound. Audio is not read from the uplink queue in
        // here on purpose: §3 admits frames to the queue during the window and drains them in
        // order afterwards, so what a slow acknowledgement costs is at most the queue's 640 ms
        // plus counted overflow — never an append the far end has not agreed to receive.
        if let Some(outcome) = self.establish(&mut shutdown, &mut ended).await {
            return outcome;
        }

        loop {
            // Unbiased on purpose. `biased` here would let whichever branch came first starve the
            // others: a talkative peer could defer every uplink frame, and a call at full rate
            // could defer the socket read that answers the peer's Pings.
            let step = tokio::select! {
                () = &mut shutdown => Step::Shutdown,
                _ended = &mut ended => Step::CallEnded,
                frame = uplink.recv() => Step::Uplink(frame),
                message = self.connection.next() => Step::Inbound(message),
            };
            match step {
                Step::Shutdown => return self.stop(BridgeOutcome::Cancelled).await,
                // The uplink task holds the only sender, so either signal means the call's media
                // has stopped. §6: close normally, and say so.
                Step::CallEnded | Step::Uplink(None) => {
                    return self.stop(BridgeOutcome::CallEnded).await;
                }
                Step::Uplink(Some(payload)) => {
                    if let Some(outcome) = self.append(&payload).await {
                        return outcome;
                    }
                }
                Step::Inbound(message) => {
                    if let Some(outcome) = self.consume(message).await {
                        return outcome;
                    }
                }
            }
        }
    }

    /// §3's two acknowledgements, each under its own bound.
    async fn establish(
        &mut self,
        shutdown: &mut std::pin::Pin<&mut impl Future<Output = ()>>,
        ended: &mut std::pin::Pin<&mut oneshot::Receiver<()>>,
    ) -> Option<BridgeOutcome> {
        if let Some(outcome) = self
            .await_setup(SetupStep::SessionCreated, shutdown, ended)
            .await
        {
            return Some(outcome);
        }
        let update = self.session_update();
        if let Err(error) = self.connection.send_text(&update.to_text()).await {
            return Some(closed_outcome(&error, self.connection.close_code()));
        }
        if let Some(outcome) = self
            .await_setup(SetupStep::SessionUpdated, shutdown, ended)
            .await
        {
            return Some(outcome);
        }
        self.configured = true;
        None
    }

    /// Read until `step` arrives, or its bound elapses. `None` means it arrived.
    async fn await_setup(
        &mut self,
        step: SetupStep,
        shutdown: &mut std::pin::Pin<&mut impl Future<Output = ()>>,
        ended: &mut std::pin::Pin<&mut oneshot::Receiver<()>>,
    ) -> Option<BridgeOutcome> {
        let bound = self.limits.setup_bound;
        let reading = async {
            loop {
                let woke = tokio::select! {
                    () = &mut *shutdown => Step::Shutdown,
                    _ended = &mut *ended => Step::CallEnded,
                    message = self.connection.next() => Step::Inbound(message),
                };
                match woke {
                    Step::Shutdown => return Some(BridgeOutcome::Cancelled),
                    // No uplink branch is offered here (§3 queues those frames rather than
                    // sending them), so the only other way out is the call itself ending.
                    Step::CallEnded | Step::Uplink(_) => return Some(BridgeOutcome::CallEnded),
                    Step::Inbound(message) => match self.consume_setup(step, message).await {
                        SetupRead::Arrived => return None,
                        SetupRead::Continue => {}
                        SetupRead::Ended(outcome) => return Some(outcome),
                    },
                }
            }
        };
        // A **bound on failure**: how long the bridge waits before concluding the acknowledgement
        // is not coming. It orders nothing — the read above completes on the frame itself, and
        // §5.4 states that ordering in this contract is always by event and never by a clock.
        match tokio::time::timeout(bound, reading).await {
            Ok(outcome) => outcome,
            Err(_elapsed) => Some(BridgeOutcome::SetupTimeout {
                awaiting: step,
                bound,
            }),
        }
    }

    /// One message read while waiting for a setup acknowledgement.
    async fn consume_setup(
        &mut self,
        awaited: SetupStep,
        message: Result<Option<WssMessage>, WssError>,
    ) -> SetupRead {
        let text = match self.text_of(message) {
            Ok(Some(text)) => text,
            Ok(None) => return SetupRead::Continue,
            Err(outcome) => return SetupRead::Ended(outcome),
        };
        let event = match Event::read(&text) {
            Ok(event) => event,
            Err(detail) => return SetupRead::Ended(BridgeOutcome::MalformedEvent { detail }),
        };
        let arrived = match awaited {
            SetupStep::SessionCreated => event.kind == "session.created",
            SetupStep::SessionUpdated => event.kind == "session.updated",
        };
        if arrived {
            return SetupRead::Arrived;
        }
        match self.dispatch(&event).await {
            Some(outcome) => SetupRead::Ended(outcome),
            None => SetupRead::Continue,
        }
    }

    /// One inbound message in the serving loop. `Some` ends the bridge.
    async fn consume(
        &mut self,
        message: Result<Option<WssMessage>, WssError>,
    ) -> Option<BridgeOutcome> {
        let text = match self.text_of(message) {
            Ok(Some(text)) => text,
            Ok(None) => return None,
            Err(outcome) => return Some(outcome),
        };
        match Event::read(&text) {
            Ok(event) => self.dispatch(&event).await,
            Err(detail) => Some(BridgeOutcome::MalformedEvent { detail }),
        }
    }

    /// The text of one message, or the outcome its absence means.
    ///
    /// `Ok(None)` is a frame that is neither text nor an ending — there is exactly one, a binary
    /// frame, and §5.3 makes it fatal, so the only `Ok(None)` left is unreachable and harmless.
    fn text_of(
        &self,
        message: Result<Option<WssMessage>, WssError>,
    ) -> Result<Option<String>, BridgeOutcome> {
        match message {
            Ok(Some(WssMessage::Text(text))) => Ok(Some(text)),
            // §5.3: every event in this contract is a JSON text frame, so a binary one cannot be
            // interpreted and is fatal on its first occurrence.
            Ok(Some(WssMessage::Binary(bytes))) => Err(BridgeOutcome::MalformedEvent {
                detail: format!("a binary frame of {} bytes", bytes.len()),
            }),
            Ok(None) => Err(BridgeOutcome::PeerClosed {
                code: self.connection.close_code(),
                detail: None,
            }),
            Err(error) => Err(closed_outcome(&error, self.connection.close_code())),
        }
    }

    /// §5.2's table, one row at a time. `Some` ends the bridge.
    async fn dispatch(&mut self, event: &Event) -> Option<BridgeOutcome> {
        match event.kind.as_str() {
            // Setup acknowledgements outside setup. §3 admits exactly one `session.update`, so a
            // second `session.created` cannot be answered with a second one; consuming it is the
            // only reading that keeps both sentences true.
            "session.created" | "session.updated" => None,
            "input_audio_buffer.speech_started" => self.barge_in().await,
            "response.output_audio.delta" => self.delta(event),
            "response.output_audio.done" => self.audio_done(event),
            "response.done" => {
                self.in_flight = None;
                self.dropping_deltas = false;
                self.cancel_race_until = None;
                None
            }
            "error" => self.error(event),
            // §5.3: the vendor emits many events this contract does not read. Counting them and
            // staying live is what keeps an *addition* at the far end from being an outage here.
            _ => {
                self.meters.ignored_events.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// §4.3, in its order and without waiting on anything.
    async fn barge_in(&mut self) -> Option<BridgeOutcome> {
        if self.in_flight.is_some() {
            // Step 1. `response.cancel` carries no `response_id` (§5.1): the in-progress response
            // is the target. Sent only when one is in flight — §4.3 performs the rest vacuously
            // otherwise, and a cancel with nothing to cancel is an event outside the subset.
            let cancel = Json::object([("type", Some(Json::Str("response.cancel".to_owned())))]);
            if let Err(error) = self.connection.send_text(&cancel.to_text()).await {
                return Some(closed_outcome(&error, self.connection.close_code()));
            }
            self.cancel_race_until = Some(Instant::now() + self.limits.cancel_race_window);
        }
        // Step 2. The queue and the accumulator empty together under one lock, so no frame can be
        // assembled out of residue that the flush was supposed to throw away.
        let flushed = self.downlink.flush(&self.meters);
        self.meters
            .barge_in_flushed
            .fetch_add(flushed, Ordering::Relaxed);
        // Step 3.
        self.dropping_deltas = true;
        None
    }

    /// `response.output_audio.delta` (§4.1, §4.3).
    fn delta(&mut self, event: &Event) -> Option<BridgeOutcome> {
        let Some(response) = event.string("response_id") else {
            return Some(malformed("a delta with no string `response_id`"));
        };
        let Some(delta) = event.string("delta") else {
            return Some(malformed("a delta with no string `delta`"));
        };
        let Ok(audio) = BASE64.decode(delta) else {
            return Some(malformed("a delta whose `delta` is not RFC 4648 §4 base64"));
        };
        if self.dropping_deltas {
            self.meters.cancelled_deltas.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        self.in_flight = Some(response.to_owned());
        self.downlink.accept(&audio, &self.meters);
        None
    }

    /// `response.output_audio.done` — where §4.1's partial-frame padding happens.
    fn audio_done(&mut self, event: &Event) -> Option<BridgeOutcome> {
        if event.string("response_id").is_none() {
            return Some(malformed("an audio done with no string `response_id`"));
        }
        if !self.dropping_deltas {
            self.downlink.pad(self.format.silence, &self.meters);
        }
        None
    }

    /// `error` — the cancel race, or the session's end (§4.3, §6).
    fn error(&mut self, event: &Event) -> Option<BridgeOutcome> {
        // The window is a **bound on failure**: it stops a peer that never sends `response.done`
        // from leaving the race open for the rest of the call. It orders nothing — a
        // `response.done` closes the window whenever it arrives, and this read only asks whether
        // that has already happened.
        let racing = self
            .cancel_race_until
            .is_some_and(|until| Instant::now() < until);
        if racing {
            self.meters.cancel_race.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(BridgeOutcome::SessionError {
            code: event
                .value
                .get("error")
                .and_then(|error| error.get("code"))
                .and_then(Json::as_str)
                .map(str::to_owned),
        })
    }

    /// One uplink frame as exactly one `input_audio_buffer.append` (§4.1).
    async fn append(&mut self, payload: &[u8]) -> Option<BridgeOutcome> {
        let event = Json::object([
            (
                "type",
                Some(Json::Str("input_audio_buffer.append".to_owned())),
            ),
            ("audio", Some(Json::Str(BASE64.encode(payload)))),
        ]);
        if let Err(error) = self.connection.send_text(&event.to_text()).await {
            return Some(closed_outcome(&error, self.connection.close_code()));
        }
        self.meters.appended.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// §3's one `session.update`, pinned to the call's negotiated wire format.
    fn session_update(&self) -> Json {
        let format = || {
            Json::object([(
                "format",
                Some(Json::object([(
                    "type",
                    Some(Json::Str(self.format.wire.to_owned())),
                )])),
            )])
        };
        let mut input = format();
        if let Json::Object(members) = &mut input {
            members.insert(
                "turn_detection".to_owned(),
                Json::object([
                    ("type", Some(Json::Str("server_vad".to_owned()))),
                    ("create_response", Some(Json::Bool(true))),
                    // Cancellation has exactly one owner: §4.3's barge-in rule, here. Turning the
                    // far end's own interruption off is what makes a test able to assert one
                    // causal chain instead of a race between two cancellers.
                    ("interrupt_response", Some(Json::Bool(false))),
                ]),
            );
        }
        Json::object([
            ("type", Some(Json::Str("session.update".to_owned()))),
            (
                "session",
                Some(Json::object([
                    ("type", Some(Json::Str("realtime".to_owned()))),
                    (
                        "output_modalities",
                        Some(Json::Array(vec![Json::Str("audio".to_owned())])),
                    ),
                    ("instructions", Some(Json::Str(self.instructions.clone()))),
                    (
                        "audio",
                        Some(Json::object([
                            ("input", Some(input)),
                            ("output", Some(format())),
                        ])),
                    ),
                ])),
            ),
        ])
    }

    /// End deliberately, so the far end learns this was not a failure to retry through (§6).
    async fn stop(&mut self, outcome: BridgeOutcome) -> BridgeOutcome {
        let _closed = self.connection.close().await;
        outcome
    }
}

/// What reading one message during setup produced.
enum SetupRead {
    /// The acknowledgement being waited for.
    Arrived,
    /// Something else, consumed; keep waiting.
    Continue,
    /// The bridge is over.
    Ended(BridgeOutcome),
}

/// One server event, read far enough to dispatch on (§5.2, §5.3).
struct Event {
    kind: String,
    value: Json,
}

impl Event {
    /// Read a text frame as an event, or say what could not be read (§5.3).
    fn read(text: &str) -> Result<Self, String> {
        let value = Json::parse(text)
            .map_err(|error| format!("a text frame that is not JSON: {error:?}"))?;
        let kind = value
            .get("type")
            .and_then(Json::as_str)
            .ok_or_else(|| "a JSON frame with no string `type`".to_owned())?
            .to_owned();
        Ok(Self { kind, value })
    }

    /// A member of this event, when it is a string.
    fn string(&self, member: &str) -> Option<&str> {
        self.value.get(member).and_then(Json::as_str)
    }
}

/// §5.3's disposition, spelled once.
fn malformed(detail: &str) -> BridgeOutcome {
    BridgeOutcome::MalformedEvent {
        detail: detail.to_owned(),
    }
}

/// A WSS failure before the socket was ever established (§6, and the row §6 lacks).
fn unreachable_outcome(error: &WssError) -> BridgeOutcome {
    BridgeOutcome::Unreachable {
        detail: error.to_string(),
    }
}

/// A WSS failure on an established connection, mapped onto §6's rows.
fn closed_outcome(error: &WssError, code: Option<u16>) -> BridgeOutcome {
    match error {
        WssError::Oversize { size, limit, .. } => BridgeOutcome::OversizeFrame {
            size: *size,
            limit: *limit,
        },
        WssError::Stalled { bound, .. } => BridgeOutcome::PeerStalled { bound: *bound },
        other => BridgeOutcome::PeerClosed {
            code,
            detail: Some(other.to_string()),
        },
    }
}

// ------------------------------------------------------------------------------ the queues ----

/// The downlink queue and the re-framing accumulator, which §4.3 empties together.
///
/// One `std::sync::Mutex` rather than an async one, and it is never held across an `await`: the
/// critical sections are a push, a pop and a flush, and an async lock would let the barge-in and
/// the writer interleave inside one — which is exactly the state §4.3 says must not be observable.
#[derive(Debug)]
struct Downlink {
    state: Mutex<DownlinkState>,
    ready: Notify,
    done: AtomicBool,
    bound: usize,
}

#[derive(Debug, Default)]
struct DownlinkState {
    queue: VecDeque<Bytes>,
    accumulator: Vec<u8>,
}

impl Downlink {
    fn new(bound: usize) -> Self {
        Self {
            state: Mutex::new(DownlinkState::default()),
            ready: Notify::new(),
            done: AtomicBool::new(false),
            bound,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DownlinkState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Accumulate decoded delta bytes and slice out every whole frame (§4.1).
    fn accept(&self, audio: &[u8], meters: &BridgeMeters) {
        let mut dropped = 0u64;
        {
            let mut state = self.lock();
            state.accumulator.extend_from_slice(audio);
            while state.accumulator.len() >= FRAME_BYTES {
                let rest = state.accumulator.split_off(FRAME_BYTES);
                let frame = std::mem::replace(&mut state.accumulator, rest);
                if state.queue.len() >= self.bound {
                    // §5.4: a full media queue drops the offered frame and the session lives.
                    dropped += 1;
                } else {
                    state.queue.push_back(Bytes::from(frame));
                }
            }
            meters
                .downlink_depth
                .store(state.queue.len(), Ordering::Relaxed);
            meters
                .accumulator_bytes
                .store(state.accumulator.len(), Ordering::Relaxed);
        }
        if dropped > 0 {
            meters
                .downlink_dropped
                .fetch_add(dropped, Ordering::Relaxed);
        }
        self.ready.notify_one();
    }

    /// Pad a partial tail to a full frame at `response.output_audio.done` (§4.1).
    fn pad(&self, silence: u8, meters: &BridgeMeters) {
        let mut dropped = false;
        {
            let mut state = self.lock();
            if state.accumulator.is_empty() {
                return;
            }
            let mut frame = std::mem::take(&mut state.accumulator);
            frame.resize(FRAME_BYTES, silence);
            if state.queue.len() >= self.bound {
                dropped = true;
            } else {
                state.queue.push_back(Bytes::from(frame));
            }
            meters
                .downlink_depth
                .store(state.queue.len(), Ordering::Relaxed);
            meters.accumulator_bytes.store(0, Ordering::Relaxed);
        }
        if dropped {
            meters.downlink_dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.ready.notify_one();
    }

    /// §4.3 step 2: empty the queue and the accumulator together, and say how many **frames** went.
    ///
    /// The accumulator's residue counts nothing. It never became a frame, and §4.1's padding rule
    /// is about ending a response rather than about audio being thrown away.
    fn flush(&self, meters: &BridgeMeters) -> u64 {
        let mut state = self.lock();
        let flushed = u64::try_from(state.queue.len()).unwrap_or(u64::MAX);
        state.queue.clear();
        state.accumulator.clear();
        meters.downlink_depth.store(0, Ordering::Relaxed);
        meters.accumulator_bytes.store(0, Ordering::Relaxed);
        flushed
    }

    /// Take the next frame for the media path.
    fn take(&self, meters: &BridgeMeters) -> Option<Bytes> {
        let mut state = self.lock();
        let frame = state.queue.pop_front();
        meters
            .downlink_depth
            .store(state.queue.len(), Ordering::Relaxed);
        frame
    }

    /// No more frames are coming; wake the writer so it can stop of its own accord.
    fn finish(&self) {
        self.done.store(true, Ordering::SeqCst);
        self.ready.notify_one();
    }
}

/// Call → session: every payload becomes exactly one append, or is dropped and counted (§5.4).
async fn pump_uplink(
    audio: Arc<dyn CallAudio>,
    uplink: mpsc::Sender<Bytes>,
    meters: Arc<BridgeMeters>,
    payload_type: u8,
    ended: oneshot::Sender<()>,
) {
    while let Some(encoded) = audio.recv_encoded().await {
        // Relay hands over whatever arrived, RFC 4733 events included. An
        // `input_audio_buffer.append` carrying a telephone-event packet would be this bridge
        // telling the far end that keypress noise is speech, so only this call's own negotiated
        // audio travels.
        if encoded.payload_type != payload_type {
            continue;
        }
        // The payload and nothing else. This bridge terminates RTP rather than relaying it — the
        // far side is a speech provider over a WebSocket, not an RTP peer — so an arriving header
        // extension has no packet to travel on and is dropped here on purpose (`M-79`).
        match uplink.try_send(encoded.payload) {
            Ok(()) => {}
            // §5.4: non-blocking admission, never a blocking send. A full uplink queue means the
            // socket has stalled, and liveness — not this queue — is what ends the bridge.
            Err(mpsc::error::TrySendError::Full(_frame)) => {
                meters.uplink_dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(mpsc::error::TrySendError::Closed(_frame)) => break,
        }
    }
    // Dropped rather than sent: the serving loop only needs to know the call's media has stopped,
    // and dropping says it whether this task ended at the call's end or at its own cancellation.
    drop(ended);
}

/// Session → call: one frame at a time, so at most one is ever ahead of a barge-in (§4.3).
async fn pump_downlink(
    audio: Arc<dyn CallAudio>,
    downlink: Arc<Downlink>,
    meters: Arc<BridgeMeters>,
    payload_type: u8,
) {
    loop {
        // Registered before the queue is read, so a frame pushed between the read and the wait
        // cannot be missed. `notify_one` stores its permit, so even a wake that arrives first is
        // still owed to this task.
        let woken = downlink.ready.notified();
        if let Some(payload) = downlink.take(&meters) {
            // Authored rather than relayed, so it carries no header extension (`M-79`): these
            // bytes were synthesised by the provider and did not arrive on an RTP packet that
            // could have qualified them.
            let delivered = audio
                .send_encoded(Encoded::new(payload_type, payload))
                .await;
            if !delivered {
                return;
            }
            meters.delivered.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if downlink.done.load(Ordering::SeqCst) {
            return;
        }
        woken.await;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// §3's table, and the fact that keys it on the payload type rather than on a Cargo feature.
    #[test]
    fn the_negotiated_payload_chooses_the_format_and_its_silence() {
        let mulaw = Format::of(0).expect("PCMU is bridgeable");
        assert_eq!(mulaw.wire, "audio/pcmu");
        assert_eq!(mulaw.silence, MULAW_SILENCE);
        let alaw = Format::of(8).expect("PCMA is bridgeable");
        assert_eq!(alaw.wire, "audio/pcma");
        assert_eq!(alaw.silence, ALAW_SILENCE);
        for other in [9u8, 96, 111, 101] {
            assert!(Format::of(other).is_none(), "{other} is not G.711");
        }
    }

    /// N7 over the type: the value is not a field anyone can read, and neither `Debug` nor the
    /// URL can carry it.
    #[test]
    fn a_configured_session_never_shows_its_credential() {
        let setup = SessionSetup::new(
            "wss://api.example.com/v1/realtime",
            "gpt-realtime-2.1",
            "be brief",
            "openai-api-key",
            b"sk-not-a-real-key",
        )
        .expect("a usable credential");
        assert_eq!(setup.secret(), "openai-api-key");
        assert_eq!(
            setup.url(),
            "wss://api.example.com/v1/realtime?model=gpt-realtime-2.1"
        );
        let printed = format!("{setup:?}");
        assert!(
            !printed.contains("sk-not-a-real-key"),
            "Debug carried it: {printed}"
        );
        assert!(printed.contains("openai-api-key"), "{printed}");
    }

    /// A secret that cannot travel is a startup failure naming the *name* (N7, N8).
    #[test]
    fn an_unusable_credential_is_refused_by_name() {
        for key in [&b""[..], b"has a\nnewline", &[0x80, 0x81][..]] {
            let error = SessionSetup::new("wss://x/y", "m", "i", "openai-api-key", key)
                .expect_err("refused");
            assert_eq!(error.secret, "openai-api-key");
            let printed = format!("{error} / {error:?}");
            assert!(printed.contains("openai-api-key"), "{printed}");
        }
    }

    /// A queue that is full drops the frame it was offered and counts it, and the residue that
    /// never became a frame counts nothing (§4.3, §5.4).
    #[test]
    fn a_full_downlink_queue_drops_and_counts_rather_than_growing() {
        let meters = BridgeMeters::default();
        let downlink = Downlink::new(2);
        downlink.accept(&[0u8; FRAME_BYTES * 4], &meters);
        assert_eq!(meters.downlink_depth(), 2, "the bound holds");
        assert_eq!(meters.snapshot().downlink_dropped, 2);

        let downlink = Downlink::new(8);
        downlink.accept(&[1u8; FRAME_BYTES + 80], &meters);
        assert_eq!(meters.downlink_depth(), 1);
        assert_eq!(meters.accumulator_bytes(), 80);
        assert_eq!(downlink.flush(&meters), 1, "one whole frame was queued");
        assert_eq!(meters.accumulator_bytes(), 0, "the residue went with it");
    }

    /// §4.1's padding: a partial tail becomes one full frame of the format's silence.
    #[test]
    fn a_partial_tail_is_padded_to_a_whole_frame() {
        let meters = BridgeMeters::default();
        let downlink = Downlink::new(8);
        downlink.accept(&[7u8; 80], &meters);
        assert_eq!(meters.downlink_depth(), 0, "80 bytes is not a frame yet");
        downlink.pad(ALAW_SILENCE, &meters);
        let frame = downlink.take(&meters).expect("a padded frame");
        assert_eq!(frame.len(), FRAME_BYTES);
        assert_eq!(&frame[..80], &[7u8; 80][..]);
        assert_eq!(&frame[80..], &[ALAW_SILENCE; 80][..]);
    }
}
