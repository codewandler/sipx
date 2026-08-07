//! Establishing a call: INVITE with an SDP offer, media bound to the answer, and BYE.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use bytes::Bytes;
use sipx_media::ice::{LocalDescription, Negotiation as IceNegotiation};
use sipx_media::{Codec, Interrupt, MediaPort, MediaSession, Playback};
use sipx_sdp::ice::{ComponentId, Credentials as IceCredentials};
use sipx_sdp::{Capabilities, Connection, Direction, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::session::{self, MinSe, SessionExpires};
use sipx_sip::update::{self, Reception};
use sipx_sip::{
    HeaderName, HistoryInfo, Method, Reason, ReasonValue, Request, Response, StatusCode, Uri,
};
use sipx_transport::{Handle, Incoming, Target, TransportKind};

pub use sipx_sip::auth::Credentials;

use crate::dialog::{Dialog, strip_header_params};
use crate::error::{
    CancellationCleanup, CancellationDisposition, Error, InvitationCancellation, Result,
};
use crate::event::{CallEvent, CallEvents, EndCause, EventSink};
use crate::extension::{self, ApplicationRequest};
use crate::identity::OutboundIdentityPolicy;
use crate::media_policy::{Codecs, IcePolicy, Keying, MediaPolicy, MediaProfile, NegotiatedKeying};
use crate::snapshot::{
    DialogNotQuiescent, DialogPersistenceError, DialogRestoreContext, DialogSnapshot,
    SessionSnapshot, SnapshotParts,
};
use crate::transfer::{
    Referral, Replaces, Transfer, TransferState, is_terminated, parse_sipfrag, sipfrag,
};

// The concern seams split out of this file (`X-67`). Private modules: everything they hold is
// either an inherent method of [`Call`] — reachable through the type wherever the impl block
// lives — or re-exported below, so no public path moves.
mod hold;
mod ice;
mod offer_answer;
mod refer;
mod reinvite;
mod timers;

pub use refer::{answer_replacing, answer_replacing_with};

use ice::{IceOffer, add_ice, peer_ice_credentials};
pub(crate) use offer_answer::{Negotiated, offer_from};
use offer_answer::{
    Settled, answering_rtcp_mode, exchanged_rtcp_mode, negotiated, offer_crypto,
    preserve_rtcp_mode, settle_answer, srtp_keys_answering,
};
use timers::{SessionState, required_interval};

/// 200 OK.
///
/// `StatusCode::new` is fallible because most codes come from the wire; this one is a literal
/// that is always in range. Threading a `Result` out of every call site for it would mean
/// inventing an error that can never happen — and the previous attempt reported it as "no
/// final response to the INVITE", which would have been actively misleading.
const OK: u16 = 200;

pub(crate) fn ok_status() -> StatusCode {
    StatusCode::new(OK).unwrap_or_else(|| unreachable!("200 is a valid status code"))
}

/// Queue the events construction already knows happened: `Ringing`, if the far end rang first,
/// then `Answered` — every `Call` gets exactly this sequence at birth, on both the caller's and
/// the callee's side, which is why both construction sites share it rather than repeating it.
pub(crate) fn emit_construction_events(events: &EventSink, ringing: Option<bool>) {
    if let Some(reliable) = ringing {
        events.emit(CallEvent::Ringing { reliable });
    }
    events.emit(CallEvent::Answered);
}

/// A fresh token for a `Call-ID` or a `tag`.
///
/// Its own function rather than the user agent's digest `cnonce`: a dialog identifier is not an
/// authentication nonce, and borrowing one ties this layer to the one that handles credentials
/// for no reason beyond both wanting random hex.
fn token_with_rng<R>(rng: &mut R) -> String
where
    R: rand::CryptoRng + ?Sized,
{
    let value = rand::RngCore::next_u64(rng);
    format!("{value:016x}")
}

pub(crate) fn token() -> String {
    token_with_rng(&mut rand::rng())
}

/// The two address roles of one media socket.
///
/// `advertised` is written into SDP and may be a public NAT mapping the host does not own.
/// `bind` selects the local interface on which the RTP socket is opened. Passing an [`IpAddr`]
/// keeps the historical behaviour by using it for both roles.
///
/// When ICE is enabled, these addresses are only the local gathering base and initial SDP
/// default. A nominated ICE pair owns the live destination; symmetric RTP cannot replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaAddress {
    advertised: IpAddr,
    bind: IpAddr,
}

impl MediaAddress {
    /// Advertise and bind the same address.
    #[must_use]
    pub const fn new(advertised: IpAddr) -> Self {
        Self {
            advertised,
            bind: advertised,
        }
    }

    /// Bind RTP on `bind` while continuing to advertise the address passed to [`Self::new`].
    ///
    /// An unspecified bind address is valid. The advertised address must be reachable by the
    /// peer; an unspecified one is refused before signalling. With ICE enabled,
    /// the nominated pair takes precedence over symmetric-RTP source learning.
    #[must_use]
    pub const fn with_bind(mut self, bind: IpAddr) -> Self {
        self.bind = bind;
        self
    }

    /// The address serialized into SDP.
    #[must_use]
    pub const fn advertised(self) -> IpAddr {
        self.advertised
    }

    /// The local address supplied to the RTP socket bind.
    #[must_use]
    pub const fn bind(self) -> IpAddr {
        self.bind
    }

    fn validate(self) -> Result<Self> {
        if self.advertised.is_unspecified() {
            return Err(Error::UnspecifiedMediaAddress);
        }
        Ok(self)
    }
}

impl From<IpAddr> for MediaAddress {
    fn from(address: IpAddr) -> Self {
        Self::new(address)
    }
}

/// A call in progress.
#[derive(Debug)]
pub struct Call {
    /// The dialog it runs in.
    pub dialog: Dialog,
    /// The successful final response that established this dialog.
    ///
    /// A caller retains the actual 2xx it received; an answerer records the 200 it sent. Keeping
    /// this fact on the call lets applications report response-code distributions without
    /// inventing `200` for a peer that answered with a different successful status.
    initial_status: u16,
    media: Arc<MediaSession>,
    /// Replaced sessions whose workers have been stopped but not yet completely joined. A
    /// cancelled renegotiation leaves this ownership in the call for retry or terminal cleanup.
    retired_media: Vec<Arc<MediaSession>>,
    endpoint: Handle,
    /// Where in-dialog requests go: the peer's `Contact`, not where the INVITE was sent.
    target: Target,
    /// Set while a 2xx is still being retransmitted; cleared when the ACK arrives.
    ack_stop: Option<CancellationToken>,
    /// Completion of the successful-final-response retransmitter. Retained separately from its
    /// stop signal so an ACK or terminal call path proves that the worker has actually exited.
    ack_retransmission: Option<OwnedTask>,
    /// Capabilities behind an offer sent in a re-INVITE's 2xx, awaiting its answer in the ACK.
    delayed_offer: Option<Capabilities>,
    ended: bool,
    /// Where this side receives media, so a re-offer can name the same address.
    media_address: IpAddr,
    /// Where replacement media sockets bind during an in-dialog renegotiation.
    media_bind_address: IpAddr,
    /// The codec set this call was placed or answered with, so a re-offer offers the same
    /// set — a re-INVITE that silently narrowed to G.711 would move an Opus call mid-call.
    codecs: Codecs,
    /// Named composition policy retained for renegotiation and diagnostics.
    profile: MediaProfile,
    /// What the running session negotiated, for comparison against a re-offer.
    current: Negotiated,
    /// The peer's ICE credentials as this side last saw them (RFC 8839 §4.4.1.1.1).
    ///
    /// The only thing a restart can be recognised against: a later offer restarts ICE when **both**
    /// its `ice-ufrag` and its `ice-pwd` differ from these. `None` is a call that never ran ICE, or
    /// one whose peer has not described it — and neither can restart something that never started.
    peer_ice: Option<sipx_sdp::ice::Credentials>,
    /// Whether the call is on hold, and which way.
    hold: Direction,
    /// Whether the media is encrypted.
    encrypted: bool,
    /// The initial keying policy, retained so a later offer cannot silently downgrade it.
    keying: Keying,
    /// A transfer the far end has asked for and we have not yet answered.
    referral: Option<Referral>,
    /// A transfer we asked for, and what has become of it.
    transfer: Option<Transfer>,
    /// The RFC 4028 session timer, if one was negotiated.
    session: Option<SessionState>,
    /// Whose turn it is to offer and to answer (RFC 3311 §5, RFC 3264).
    ///
    /// Idle here: a `Call` exists only once the INVITE's offer/answer has completed, so nothing
    /// is outstanding at construction on either side.
    negotiation: update::Negotiation,
    /// Whether the peer's `Allow` listed UPDATE (RFC 3311 §4).
    ///
    /// Read from the message that introduced the peer — the INVITE for a UAS, the 2xx for a
    /// UAC — and refreshed from any later one. It is the only permission there is: RFC 4028
    /// §7.4 turns it into the choice between UPDATE and a re-INVITE for a session refresh, and
    /// a refresh sent by a method the far end does not implement draws a 405 and tears down a
    /// call that was working.
    peer_allows_update: bool,
    /// Where this call's events go (story `C-3`). Every state change below is emitted through
    /// this at the point it happens, not reconstructed afterwards from the fields above.
    events: EventSink,
    /// The one receiver [`Self::events`] hands out, until it does.
    events_rx: Option<CallEvents>,
    /// The diversion history received on the request or final response that established this
    /// call. Retained because the application cannot recover it after the transaction stream is
    /// consumed by call setup.
    history: Option<HistoryInfo>,
    /// Digest credentials retained for authenticated requests originated inside this dialog.
    dialog_credentials: Option<Credentials>,
    /// Case-sensitive private method tokens the application explicitly admitted.
    admitted_dialog_methods: Vec<Bytes>,
}

/// A call-owned task that cannot detach if the call is abandoned without explicit shutdown.
#[derive(Debug)]
struct OwnedTask(tokio::task::JoinHandle<()>);

impl OwnedTask {
    fn new(owner: tokio::task::JoinHandle<()>) -> Self {
        Self(owner)
    }

    async fn joined(&mut self) {
        // discard: the caller has already selected the task's terminal protocol outcome; this
        // await is only the ownership barrier and a cancellation JoinError cannot change it.
        let _ = (&mut self.0).await;
    }
}

impl Drop for OwnedTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn cancel_and_join(stop: &CancellationToken, owner: &mut OwnedTask) {
    stop.cancel();
    owner.joined().await;
}

async fn until_cancelled<F: Future>(stop: &CancellationToken, operation: F) -> Option<F::Output> {
    tokio::select! {
        biased;
        () = stop.cancelled() => None,
        output = operation => Some(output),
    }
}

trait Retirable {
    async fn finish(&self);
}

impl Retirable for Arc<MediaSession> {
    async fn finish(&self) {
        self.shutdown().await;
    }
}

async fn drain_retired<T: Retirable>(retired: &mut Vec<T>) {
    while let Some(previous) = retired.last() {
        previous.finish().await;
        retired.pop();
    }
}

fn retired_media_snapshot_refusal(count: usize) -> Option<DialogNotQuiescent> {
    (count != 0).then_some(DialogNotQuiescent::MediaCleanup)
}

impl Call {
    /// Capture the bounded protocol state needed to continue this confirmed dialog.
    ///
    /// `now` is explicit and only the session timer's remaining duration is retained. Sockets,
    /// endpoint handles, media sessions, tasks, transactions, credentials, keys, entropy and
    /// process-local clock instants never enter [`DialogSnapshot`]. Capture refuses any call with
    /// active work whose safe continuation would require one of those runtime values.
    pub fn dialog_snapshot(
        &self,
        now: Instant,
    ) -> std::result::Result<DialogSnapshot, DialogPersistenceError> {
        if self.ended {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::Ended,
            ));
        }
        if self.ack_retransmission.is_some() {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::AwaitingAck,
            ));
        }
        if !self.negotiation.is_idle() {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::OfferAnswer,
            ));
        }
        if let Some(reason) = retired_media_snapshot_refusal(self.retired_media.len()) {
            return Err(DialogPersistenceError::NotQuiescent(reason));
        }
        if self.referral.is_some() || self.transfer.is_some() {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::Transfer,
            ));
        }
        if self.media.runs_ice() {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::Ice,
            ));
        }
        let session = self
            .session
            .map(|state| {
                let remaining = state
                    .act_at
                    .checked_duration_since(now)
                    .filter(|remaining| !remaining.is_zero())
                    .ok_or({
                        DialogPersistenceError::SessionActionDue(if state.terms.we_refresh {
                            crate::DialogSessionAction::Refresh
                        } else {
                            crate::DialogSessionAction::Expire
                        })
                    })?;
                Ok(SessionSnapshot {
                    interval: state.terms.interval,
                    we_refresh: state.terms.we_refresh,
                    remaining,
                })
            })
            .transpose()?;

        DialogSnapshot::from_parts(SnapshotParts {
            role: self.dialog.role,
            id: self.dialog.id.clone(),
            local_party: strip_header_params(&self.dialog.local_uri),
            remote_party: strip_header_params(&self.dialog.remote_uri),
            remote_target: self.dialog.remote_target.clone(),
            route_set: self.dialog.route_set.clone(),
            local_cseq: self.dialog.local_cseq,
            remote_cseq: self.dialog.remote_cseq,
            protected_signalling: self.target.transport.is_secure(),
            media_keying: self.negotiated_keying(),
            media_profile: self.profile,
            codecs: self.codecs,
            codec: self.current.codec,
            clock_rate: self.current.clock_rate,
            payload_type: self.current.wire_payload_type(),
            receive_payload_type: self.current.receive_wire_payload_type(),
            dtmf_payload_type: self.current.dtmf,
            rtcp_mode: self.current.rtcp_mode,
            hold: self.hold,
            peer_allows_update: self.peer_allows_update,
            session,
        })
    }

    /// Attach validated durable dialog state to fresh endpoint and media drivers.
    ///
    /// Restoration is synchronous and performs no I/O. Every snapshot and context invariant is
    /// checked before handles are cloned or events are published, so a refusal creates no task or
    /// transaction and leaves the borrowed context running exactly as supplied. Snapshot storage,
    /// authorization, encryption at rest, distribution and single-owner election belong to the
    /// host; a successful decode proves format validity, not permission to resume a call.
    pub fn restore_dialog(
        snapshot: &DialogSnapshot,
        context: &DialogRestoreContext,
    ) -> std::result::Result<Self, DialogPersistenceError> {
        let session = snapshot.validate_restore(context)?;
        // The only mutation in restoration, after every fallible snapshot/context check. It is
        // an atomic one-owner claim rather than runtime work: no task, transaction, socket or
        // media worker starts here, and a concurrent duplicate restore gets a typed refusal.
        context.claim()?;
        let (events, events_rx) = EventSink::new();
        Ok(Self {
            dialog: snapshot.dialog(),
            initial_status: OK,
            media: Arc::clone(&context.media),
            retired_media: Vec::new(),
            endpoint: context.endpoint.clone(),
            target: context.target.clone(),
            ack_stop: None,
            ack_retransmission: None,
            delayed_offer: None,
            ended: false,
            media_address: context.media_address.advertised(),
            media_bind_address: context.media_address.bind(),
            codecs: snapshot.codecs_value(),
            profile: snapshot.media_profile_value(),
            current: snapshot.negotiated(context.remote_media),
            peer_ice: None,
            // Validation proved the freshly built media driver carries the durable direction.
            // Install the injected runtime fact so restoration never trusts snapshot state alone.
            hold: context.direction,
            encrypted: context.media.is_encrypted(),
            keying: context.policy.keying,
            referral: None,
            transfer: None,
            session: session.map(|(interval, we_refresh, act_at)| SessionState {
                terms: session::Session {
                    interval,
                    we_refresh,
                },
                act_at,
            }),
            negotiation: update::Negotiation::idle(),
            peer_allows_update: snapshot.peer_allows_update_value(),
            // Application-owned extension admission and credentials are runtime policy, not
            // durable dialog facts. The host must install them again after restoration.
            dialog_credentials: None,
            admitted_dialog_methods: Vec::new(),
            events,
            events_rx: Some(events_rx),
            history: None,
        })
    }

    /// Signal and join the successful-final-response retransmitter, if one is active.
    ///
    /// The handle stays in `self` while it is awaited. Cancelling the caller therefore leaves the
    /// ownership intact for the next ACK or terminal path instead of detaching the retransmitter.
    async fn stop_ack_retransmission(&mut self) {
        if let Some(stop) = self.ack_stop.take() {
            if let Some(owner) = self.ack_retransmission.as_mut() {
                cancel_and_join(&stop, owner).await;
            } else {
                stop.cancel();
            }
        } else if let Some(owner) = self.ack_retransmission.as_mut() {
            owner.joined().await;
        }
        self.ack_retransmission = None;
    }

    async fn reap_retired_media(&mut self) {
        drain_retired(&mut self.retired_media).await;
    }

    /// The successful final response that established this call.
    #[must_use]
    pub fn initial_status(&self) -> u16 {
        self.initial_status
    }

    /// The audio.
    #[must_use]
    pub fn media(&self) -> &MediaSession {
        &self.media
    }

    /// A shared media handle for an owning actor that must move one operation into a bounded task.
    ///
    /// Most applications should use [`Self::media`]. This form exists for interactive owners that
    /// must keep accepting control commands while a recording receives frames; sharing the session
    /// does not share or clone the `Call`'s signalling state.
    pub fn media_handle(&self) -> Arc<MediaSession> {
        Arc::clone(&self.media)
    }

    /// Install or clear the application callback for peer RTCP quality reports.
    ///
    /// This is call-owned policy: it remains installed across an ordinary re-INVITE, a media
    /// session replacement, and an ICE restart. The callback itself must return promptly; see
    /// [`sipx_media::RtcpQualityHook`].
    pub fn set_rtcp_quality_hook(&self, hook: Option<sipx_media::RtcpQualityHook>) {
        self.media.set_rtcp_quality_hook(hook);
    }

    /// The peer RTCP quality callback currently attached to this call.
    #[must_use]
    pub fn rtcp_quality_hook(&self) -> Option<sipx_media::RtcpQualityHook> {
        self.media.rtcp_quality_hook()
    }

    /// A response handle for a coupling that must answer glare while an outgoing request borrows
    /// this call's dialog state.
    pub(crate) fn responder(&self) -> Handle {
        self.endpoint.clone()
    }

    /// Send a DTMF digit.
    pub async fn send_digit(&self, digit: sipx_rtp::Digit, duration: Duration) -> bool {
        self.media.send_digit(digit, duration).await
    }

    /// Send a string of digits, each held for `duration`.
    ///
    /// Characters that are not DTMF digits are skipped rather than rejected: a caller passing
    /// a formatted number should not have to strip the spaces and dashes itself.
    pub async fn send_digits(&self, digits: &str, duration: Duration) -> bool {
        for c in digits.chars() {
            let Some(digit) = sipx_rtp::Digit::from_char(c) else {
                continue;
            };
            if !self.media.send_digit(digit, duration).await {
                return false;
            }
        }
        true
    }

    /// Take the next digit the far end pressed.
    ///
    /// The digit only; a caller that wants how long it was held can read [`Self::media`]'s own
    /// [`MediaSession::recv_digit`], which this delegates to.
    pub async fn recv_digit(&self) -> Option<sipx_rtp::Digit> {
        self.media
            .recv_digit()
            .await
            .map(|(digit, _duration)| digit)
    }

    /// Move one completed telephone event from media into this call's event stream.
    ///
    /// Call owners that use [`serve`] or [`serve_until`] get this automatically. An owner with
    /// its own signalling `select!` loop can await this branch beside incoming SIP and
    /// [`CallEvents::recv`](crate::CallEvents::recv). The handoff uses only the media session's
    /// bounded digit queue and the call's bounded event queue; it does not spawn a task or create
    /// another buffer.
    ///
    /// A stopped media channel waits forever rather than returning immediately. That makes this a
    /// safe `select!` arm after teardown instead of a closed-channel busy loop; dropping the
    /// future remains cancellation-safe.
    pub async fn drive_media_event(&self) {
        match self.media.recv_digit().await {
            Some((digit, duration)) => self.events.emit(CallEvent::Dtmf { digit, duration }),
            None => std::future::pending().await,
        }
    }

    /// Play a clip and wait for it, reporting on the event stream when it stops.
    ///
    /// Paced by the send loop, so this resolves when the audio has actually gone out rather than
    /// when it was queued. Emits [`CallEvent::PlaybackFinished`] either way, with `completed`
    /// saying which happened: the clip ran to the end, or something cut it short. A host driving
    /// the call from its events needs that distinction — "the announcement finished" and "the
    /// caller hung up during the announcement" lead to different next steps.
    ///
    /// The packet size is the session's own, so a clip plays correctly under a codec whose clock
    /// is not 8 kHz without the caller knowing the rate.
    ///
    /// This is [`Self::start_playback`] awaited, with [`Interrupt::Never`] — the clip runs to its
    /// end whatever the far end presses. A caller that wants to stop it, or wants a keypress to,
    /// needs the handle. Cancel-on-drop, like [`MediaSession::play`]: abandoning this future — a
    /// `timeout` that fires, a lost `select!` — stops the clip rather than leaving it playing.
    pub async fn play(&self, samples: &[i16]) -> bool {
        let playback = self
            .media
            .start_playback(samples.to_vec(), Interrupt::Never);
        let end = playback.play_out().await;
        // Emitted from here rather than from a watcher task, so a caller that awaits this call
        // can read the event immediately afterwards instead of racing a spawn.
        self.events.emit(CallEvent::PlaybackFinished {
            playback: playback.id(),
            completed: end.completed(),
        });
        end.completed()
    }

    /// Convert and play explicit linear PCM, reporting completion on the call event stream.
    ///
    /// # Errors
    ///
    /// Returns [`sipx_audio::PcmError`] before queuing audio when the format cannot be converted.
    pub async fn play_pcm(
        &self,
        pcm: &sipx_audio::Pcm,
    ) -> std::result::Result<bool, sipx_audio::PcmError> {
        let playback = self.media.start_pcm_playback(pcm, Interrupt::Never)?;
        let end = playback.play_out().await;
        self.events.emit(CallEvent::PlaybackFinished {
            playback: playback.id(),
            completed: end.completed(),
        });
        Ok(end.completed())
    }

    /// Start a clip and hand back a handle to it, without waiting (`M-17`).
    ///
    /// The primitive under "play a prompt and collect digits": the caller goes on to read digits
    /// while the audio plays, and can reach back through the handle to stop the prompt — or ask
    /// for [`Interrupt::OnDigit`] and have the far end's first keypress stop it. That keypress is
    /// **not** consumed by interrupting; it arrives at [`Self::recv_digit`] like any other, which
    /// is what makes the application contract's `gather{prompt, interruptible}`
    /// (`docs/specs/app-contract.md` §6.2) buildable rather than a menu that eats the first digit
    /// of every PIN.
    ///
    /// Clips **queue**: a second playback started while one is running begins when that one ends.
    /// See [`MediaSession::start_playback`] for why, and for what a clip queued while another is
    /// stopping does. The bound on stopping is [`Playback::STOP_BOUND_PACKETS`] packets.
    ///
    /// Reports [`CallEvent::PlaybackFinished`] for this playback however it ends, without the
    /// caller having to await the handle — a watcher task does it, so a fire-and-forget
    /// announcement is still observable to a host driving the call from its events.
    pub fn start_playback(&self, samples: Vec<i16>, interrupt: Interrupt) -> Playback {
        let playback = self.media.start_playback(samples, interrupt);
        let watcher = playback.clone();
        let emitter = self.events.emitter();
        tokio::spawn(async move {
            let end = watcher.finished().await;
            emitter.emit(CallEvent::PlaybackFinished {
                playback: watcher.id(),
                completed: end.completed(),
            });
        });
        playback
    }

    /// Convert explicit linear PCM and start a controllable playback.
    ///
    /// # Errors
    ///
    /// Returns [`sipx_audio::PcmError`] before creating a playback when conversion is refused.
    pub fn start_pcm_playback(
        &self,
        pcm: &sipx_audio::Pcm,
        interrupt: Interrupt,
    ) -> std::result::Result<Playback, sipx_audio::PcmError> {
        let playback = self.media.start_pcm_playback(pcm, interrupt)?;
        let watcher = playback.clone();
        let emitter = self.events.emitter();
        tokio::spawn(async move {
            let end = watcher.finished().await;
            emitter.emit(CallEvent::PlaybackFinished {
                playback: watcher.id(),
                completed: end.completed(),
            });
        });
        Ok(playback)
    }

    /// Record until the far end goes quiet for `idle`, and report the result on the event stream.
    ///
    /// Emits [`CallEvent::RecordingFinished`] carrying how much audio was captured — measured
    /// from the samples themselves and the session's clock rate, not by timing the call, so the
    /// number describes the recording rather than how long this side waited for it. The trailing
    /// `idle` silence is not part of it: it is how the end was detected, not something the far
    /// end said.
    pub async fn record_until_idle(&self, idle: Duration) -> Vec<i16> {
        let samples = self.media.record_until_idle(idle).await;
        self.finished_recording(samples)
    }

    /// Record until `samples` samples have arrived or `within` elapses, and report the result on
    /// the event stream.
    ///
    /// The counted wait, for a caller that knows how much audio the far end was given;
    /// [`MediaSession::record_at_least`] has the reasoning, and why `within` is a bound on
    /// failure rather than a window to measure in. Emits the same
    /// [`CallEvent::RecordingFinished`] as [`Self::record_until_idle`], measured the same way —
    /// from the samples, not from how long this side waited for them.
    pub async fn record_at_least(&self, samples: usize, within: Duration) -> Vec<i16> {
        let samples = self.media.record_at_least(samples, within).await;
        self.finished_recording(samples)
    }

    /// Announce a finished recording and hand it back.
    ///
    /// Shared by both recording verbs so the duration on the event cannot come to mean one thing
    /// for one of them and something else for the other.
    fn finished_recording(&self, samples: Vec<i16>) -> Vec<i16> {
        let rate = u64::from(self.media.clock_rate()).max(1);
        let duration = Duration::from_micros(samples.len() as u64 * 1_000_000 / rate);
        self.events.emit(CallEvent::RecordingFinished { duration });
        samples
    }

    /// Stop contributing audio to the far end, without telling it anything (story `M-18`).
    ///
    /// # Mute is not hold
    ///
    /// This is the distinction the whole verb exists for, and getting it wrong is how a call ends
    /// up renegotiated when all that was wanted was a quiet microphone:
    ///
    /// | | `mute` | [`reinvite(Direction::SendOnly)`](Self::reinvite) |
    /// |---|---|---|
    /// | Signalling | none — no re-INVITE, nothing on the wire | a re-INVITE the far end must answer |
    /// | The SDP direction | unchanged; the session is the one that was negotiated | changed, and that *is* the mechanism |
    /// | What the far end knows | nothing; [`is_on_hold`](Self::is_on_hold) there is unaffected | that this call is on hold, and it may play its own hold music |
    /// | The RTP stream | keeps flowing, carrying silence | governed by the new direction |
    /// | Can fail | no | yes — the far end can refuse the renegotiation |
    ///
    /// Hold is a state two parties agree on. Mute is a decision one party makes about its own
    /// microphone, and a far end that could tell the difference between a muted caller and a
    /// silent one would be reading something it was never sent.
    ///
    /// # What it does and does not gate
    ///
    /// Outbound audio only, and it is a gate rather than a suppressor: [`Self::play`] still runs
    /// and still resolves the same way, the packets still go out at the same pacing, and what the
    /// far end decodes out of them is silence. Reception is untouched — [`Self::recv_digit`],
    /// [`Self::record_until_idle`] and [`MediaSession::quality`] all keep working while muted —
    /// and so is DTMF in the sending direction: [`Self::send_digits`] is an explicit act by this
    /// endpoint, like a keypad tone on a handset, not something the microphone picked up.
    ///
    /// Emits [`CallEvent::Muted`] on the transition, and nothing when the call was already muted.
    pub fn mute(&self) {
        if !self.media.set_muted(true) {
            self.events.emit(CallEvent::Muted);
        }
    }

    /// Contribute audio to the far end again, undoing [`Self::mute`].
    ///
    /// Emits [`CallEvent::Unmuted`] on the transition, and nothing when the call was not muted.
    /// Like [`Self::mute`] it sends nothing: there is no renegotiation to undo, because muting
    /// never made one.
    pub fn unmute(&self) {
        if self.media.set_muted(false) {
            self.events.emit(CallEvent::Unmuted);
        }
    }

    /// Whether this side's outbound audio is muted.
    ///
    /// Local state, and a different question from [`Self::is_on_hold`], which reports what the
    /// *far end* has signalled about the session.
    #[must_use]
    pub fn is_muted(&self) -> bool {
        self.media.is_muted()
    }

    /// Whether the media is encrypted (RFC 3711).
    ///
    /// Worth asking, and worth being able to answer without a packet capture. A call whose
    /// signalling is encrypted and whose audio is not looks identical from the outside to one
    /// where both are — which is exactly the confusion that makes people believe `sips:` covers
    /// the media. It does not.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    /// The keying mechanism this established call actually negotiated.
    ///
    /// Unlike the initial [`Keying`] policy, the result contains no `Auto`: by confirmation the
    /// compatibility choice has resolved to either plain RTP or SDES-SRTP.
    #[must_use]
    pub fn negotiated_keying(&self) -> NegotiatedKeying {
        if !self.encrypted {
            NegotiatedKeying::Plain
        } else if self.keying == Keying::DtlsSrtp {
            NegotiatedKeying::DtlsSrtp
        } else {
            NegotiatedKeying::Sdes
        }
    }

    /// The named media profile this established call retained.
    #[must_use]
    pub const fn media_profile(&self) -> MediaProfile {
        self.profile
    }

    /// Nominated-pair, generation, state, and bounded ingress facts for browser audio.
    #[must_use]
    pub fn browser_component(&self) -> Option<sipx_media::browser::BrowserComponentSnapshot> {
        self.media.browser_component()
    }

    /// RTP payload type selected for sending the established audio codec.
    #[must_use]
    pub fn negotiated_payload_type(&self) -> u8 {
        self.current.wire_payload_type()
    }

    /// RTP payload type accepted when receiving the established audio codec.
    ///
    /// Usually equal to [`Self::negotiated_payload_type`], but each SDP description may assign a
    /// different dynamic number to the same format (RFC 3264 §6.1).
    #[must_use]
    pub fn negotiated_receive_payload_type(&self) -> u8 {
        self.current.receive_wire_payload_type()
    }

    /// RTP clock rate selected for the established audio codec.
    #[must_use]
    pub fn negotiated_clock_rate(&self) -> u32 {
        self.media.clock_rate()
    }

    /// Whether the call has ended, from either side.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        self.ended
    }

    /// This call's event stream (story `C-3`).
    ///
    /// `Some` the first time this is called, `None` every time after — there is exactly one
    /// consumer, per the vision's "own it, don't share it" (principle 3), so the receiver is
    /// handed out rather than cloned.
    pub fn events(&mut self) -> Option<CallEvents> {
        self.events_rx.take()
    }

    /// Retain credentials for authenticated requests originated inside this dialog.
    ///
    /// Outbound calls inherit [`DialOptions::credentials`]. This setter supplies the equivalent
    /// policy for answered calls or rotates the credentials on an existing call.
    pub fn set_dialog_credentials(&mut self, credentials: Credentials) {
        self.dialog_credentials = Some(credentials);
    }

    /// Admit one private, case-sensitive method token to the application-owned dialog path.
    ///
    /// Known SIP methods are refused: their ownership is decided by the stack, not converted into
    /// a private extension by application policy.
    pub fn admit_dialog_method(&mut self, method: &Method) -> Result<()> {
        let token = extension::validate_method_for_admission(method)?;
        if !self.admitted_dialog_methods.contains(&token) {
            self.admitted_dialog_methods.push(token);
        }
        Ok(())
    }

    /// Send an application-owned request inside this dialog.
    ///
    /// The dialog supplies the Request-URI, route set, identifiers and next `CSeq`. `headers` may
    /// contain application fields such as `Content-Type`, but never routing, dialog, framing, or
    /// authorization fields. A supported 401/407 challenge is retried once when dialog credentials
    /// are available.
    pub async fn send_dialog_request(
        &mut self,
        method: Method,
        headers: &[sipx_sip::Header],
        body: Bytes,
    ) -> Result<Response> {
        if self.ended {
            return Err(Error::DialogEnded);
        }
        if !extension::application_owned(&method, &self.admitted_dialog_methods) {
            return Err(Error::StackOwnedDialogMethod(method));
        }
        extension::validate_request_parts(headers, &body)?;

        let credentials = self.dialog_credentials.clone();
        let first = self
            .send_application_attempt(&method, headers, body.clone(), None)
            .await?;
        if first.status.is_success() {
            return Ok(first);
        }

        let failure = rejection(&first);
        let Error::AuthenticationChallenge { challenge, .. } = failure else {
            return Err(failure);
        };
        let Some(credentials) = credentials else {
            return Err(Error::Rejected {
                status: first.status.code(),
                reason: String::from_utf8_lossy(&first.reason).into_owned(),
            });
        };
        let cnonce = token();
        let authorization = Authorization {
            challenge: &challenge,
            credentials: &credentials,
            nonce_count: 1,
            cnonce: &cnonce,
        };
        let response = self
            .send_application_attempt(&method, headers, body, Some(&authorization))
            .await?;
        if !response.status.is_success() {
            return Err(rejection(&response));
        }
        Ok(response)
    }

    async fn send_application_attempt(
        &mut self,
        method: &Method,
        headers: &[sipx_sip::Header],
        body: Bytes,
        authorization: Option<&Authorization<'_>>,
    ) -> Result<Response> {
        let cseq = self.dialog.next_cseq();
        let mut request = application_request(&self.dialog, method, cseq, headers, body)?;
        if let Some(authorization) = authorization {
            authorize_invite(&mut request, authorization)?;
        }
        let mut responses = self.endpoint.send(request, self.target.clone()).await?;
        responses.final_response().await.ok_or(Error::NoResponse)
    }

    /// Feed an in-dialog request to the call.
    ///
    /// Returns whether it belonged here. Without this an incoming BYE reaches nothing and the
    /// local media session goes on sending RTP into a call the far end has torn down — worse
    /// than a call that never connects, because it does not stop.
    pub async fn handle(&mut self, incoming: &Incoming) -> Result<bool> {
        if !self.dialog.matches(&incoming.request) {
            return Ok(false);
        }

        match incoming.request.method {
            Method::Ack => {
                // The 2xx got through; stop retransmitting it.
                self.stop_ack_retransmission().await;
                self.accept_delayed_offer_answer(incoming.request.body())
                    .await?;
                Ok(true)
            }
            // An INVITE inside an existing dialog is a re-INVITE: a renegotiation of the call
            // already running, not a new one.
            Method::Invite => {
                self.on_reinvite(incoming).await?;
                Ok(true)
            }
            // RFC 3311 §5.1: an in-dialog renegotiation that does not disturb any INVITE
            // transaction. In a confirmed dialog that is mostly a session refresh (RFC 4028
            // §7.4), but a peer may equally use it to move the media, and either way it has to
            // be answered promptly — §5.2 gives the UAS no window in which to ask anybody.
            Method::Update => {
                self.on_update(incoming).await?;
                Ok(true)
            }
            // A REFER is not answered here, and that is deliberate. Every other in-dialog
            // request has one correct response; a REFER asks *may I place a call on your
            // behalf*, and only the application knows whether it may. `accept_referral` and
            // `refuse_referral` are the two answers, and until one is given the transferor is
            // waiting — which is honest, because it is.
            Method::Refer => {
                self.on_refer(incoming).await?;
                Ok(true)
            }
            Method::Notify => {
                self.on_notify(incoming).await?;
                Ok(true)
            }
            // RFC 3261 §11.2: OPTIONS may be sent inside a dialog, where it is the cheapest
            // keep-alive there is. Answered here rather than left to the application because
            // `sipx_sip::update::ALLOW` — the one list this stack advertises, and the one a 405
            // from [`serve`] carries — names OPTIONS. An advertisement that is not true is worse
            // than a narrower one: a peer that reads the list and is then refused has been told
            // two different things by the same endpoint.
            Method::Options => {
                self.on_options(incoming).await?;
                Ok(true)
            }
            Method::Bye => {
                // §12.2.2 applies to every in-dialog request, not only the ones that
                // renegotiate: a BYE from behind the current sequence number is a stale
                // duplicate, and honouring it ends a call that is still running.
                if self.out_of_order(&incoming.request) {
                    self.refuse(incoming, 500, "Server Internal Error").await?;
                    return Ok(true);
                }
                self.record_remote_cseq(&incoming.request);

                self.media.stop();
                self.ended = true;
                // Nothing left to keep alive, and leaving an elapsed deadline armed would have
                // `session_deadline` keep returning a time in the past, spinning any loop that
                // selects on it.
                self.session = None;
                self.stop_ack_retransmission().await;
                // Emitted here, at the point `ended` actually flips, rather than after the 200
                // OK below — the call is over the moment the far end's BYE is accepted, whether
                // or not building or sending the response then succeeds.
                self.events.end(EndCause::RemoteBye);
                let responded: Result<()> = async {
                    let response =
                        ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?.build();
                    self.endpoint.respond(&incoming.key, response).await?;
                    Ok(())
                }
                .await;
                self.media.shutdown().await;
                self.reap_retired_media().await;
                responded?;
                Ok(true)
            }
            ref method if extension::application_owned(method, &self.admitted_dialog_methods) => {
                if self.out_of_order(&incoming.request) {
                    self.refuse(incoming, 500, "Server Internal Error").await?;
                    return Ok(true);
                }
                if incoming.request.body().len() > extension::MAX_APPLICATION_BODY {
                    self.refuse(incoming, 413, "Content Too Large").await?;
                    return Err(Error::ApplicationBodyTooLarge {
                        actual: incoming.request.body().len(),
                        limit: extension::MAX_APPLICATION_BODY,
                    });
                }
                if !incoming.request.body().is_empty()
                    && incoming
                        .request
                        .headers
                        .get(&HeaderName::ContentType)
                        .is_none()
                {
                    self.refuse(incoming, 415, "Unsupported Media Type").await?;
                    return Err(Error::ApplicationContentTypeRequired);
                }
                self.record_remote_cseq(&incoming.request);
                self.events
                    .emit(CallEvent::ApplicationRequest(ApplicationRequest::new(
                        self.endpoint.clone(),
                        incoming.key.clone(),
                        &incoming.request,
                    )?));
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// The diversion history received while this call was established.
    #[must_use]
    pub fn history(&self) -> Option<&HistoryInfo> {
        self.history.as_ref()
    }

    /// Whether an in-dialog request arrived out of order.
    ///
    /// RFC 3261 §12.2.2 rejects a request behind the dialog's sequence number with a 500
    /// rather than applying it, and §12.2.1.1 requires each new in-dialog request to
    /// *increment* the number — so a repeat of the current one is a duplicate that has escaped
    /// the transaction layer's absorption window, not a fresh request, and is refused on the
    /// same grounds. This is not only the re-INVITE case: a stale BYE honoured here ends a
    /// call that a later request has already changed.
    fn out_of_order(&self, request: &Request) -> bool {
        self.dialog.is_out_of_order(request)
    }

    /// Record the sequence number of an in-dialog request this side has accepted.
    fn record_remote_cseq(&mut self, request: &Request) {
        self.dialog.record_remote_cseq(request);
    }

    /// Refuse a renegotiation with 488, saying why (RFC 3311 §5.2, RFC 3261 §20.43).
    ///
    /// The `Warning` is a SHOULD, and it is the difference between a peer that can log why its
    /// renegotiation was refused and one that can only log that it was.
    async fn refuse_unacceptable(&self, incoming: &Incoming) -> Result<()> {
        let status = StatusCode::new(488).unwrap_or_else(ok_status);
        let response =
            ResponseBuilder::to_request(&incoming.request, status, "Not Acceptable Here")?
                .header(
                    HeaderName::Warning,
                    Bytes::from(crate::update::warning(&self.endpoint)),
                )?
                .build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// Answer an in-dialog OPTIONS (RFC 3261 §11.2).
    ///
    /// The point of OPTIONS is the capability list, so a 200 with an empty `Allow` is a wasted
    /// exchange: the peer asked what we can do and learned nothing. No `Contact` and no session
    /// description — §11.2 allows a description here, and sending one would be an offer nobody
    /// asked for inside a call that already has one.
    async fn on_options(&mut self, incoming: &Incoming) -> Result<()> {
        // §12.2.2 applies to every in-dialog request, this one included. Going through the
        // dialog's own guard rather than past it is the point: a path that keeps its own copy of
        // the rule is a path the rule can be forgotten on.
        if self.out_of_order(&incoming.request) {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        self.record_remote_cseq(&incoming.request);

        let response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
            .header(
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?
            .header(HeaderName::Accept, Bytes::from_static(b"application/sdp"))?
            .build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// Answer a request that reached this call and that [`Self::handle`] did not claim.
    ///
    /// Three different things bring one here, and they are three different answers — collapsing
    /// them was a real defect, not an untidiness. The first version answered 481 to everything
    /// that failed [`Dialog::matches`](crate::Dialog::matches), and `matches` is false for any
    /// request with no `To` tag: so a bare INVITE or CANCEL reaching a one-call [`serve`] drew
    /// RFC 3261 §12.2.2's "the dialog you named does not exist" for a request that named no
    /// dialog at all.
    ///
    /// - **It matches this dialog**, but the method is one this call does not implement:
    ///   §8.2.1's **405**, with the `Allow` that section makes mandatory.
    /// - **It names a dialog that is not this one** — it carries a `To` tag, or its method
    ///   exists only inside a dialog: §12.2.2's **481**.
    /// - **It names no dialog**, so it is a new exchange arriving where exactly one call is
    ///   being served: **486 Busy Here** for an INVITE (§21.4.24 — "not willing or able to take
    ///   additional calls", which is precisely the one-call contract), and 405 for anything
    ///   else. A dispatcher is the answer to wanting more than one call here, and the 486 says
    ///   so in the only vocabulary the peer has.
    ///
    /// An ACK gets nothing, because SIP has no response to one.
    ///
    /// Failures are logged rather than returned. This exists so nothing is discarded in silence
    /// (`T-19`, story `C-4`), and handing the caller an error to ignore would put the silence
    /// back one level up.
    pub(crate) async fn refuse_unclaimed(&self, incoming: &Incoming) {
        // There is no response to an ACK, and an ACK for a 2xx is a transaction of its own
        // (RFC 3261 §17.1.1.3). Nothing to send; a stray one is still worth a line.
        if incoming.request.method == Method::Ack {
            tracing::debug!("an ACK reached a call that did not claim it");
            return;
        }
        let request = &incoming.request;
        let (code, reason) = if self.dialog.matches(request) {
            (405u16, "Method Not Allowed")
        } else if crate::dialog::to_tag(&request.headers).is_some()
            || crate::dispatch::dialog_only(&request.method)
        {
            (481, "Call/Transaction Does Not Exist")
        } else if request.method == Method::Invite {
            (486, "Busy Here")
        } else {
            (405, "Method Not Allowed")
        };
        let Some(status) = StatusCode::new(code) else {
            return;
        };
        let allow = code == 405;
        let built =
            ResponseBuilder::to_request(&incoming.request, status, reason).and_then(|builder| {
                if allow {
                    builder.header(
                        HeaderName::Allow,
                        Bytes::from_static(update::ALLOW.as_bytes()),
                    )
                } else {
                    Ok(builder)
                }
            });
        match built {
            Ok(builder) => {
                // discard: the refusal is lost and **this loss reaches no counter**, which is
                // stated rather than papered over. `DiscardCounts::unanswered` is not it: that is
                // bumped only by the driver's 180 s sweep over transactions still handed over, so
                // it covers a `respond` that was never called and not one that was called and
                // failed — and on `Error::NoTransaction` there is no transaction left to sweep at
                // all. What bounds the damage is the peer: it retransmits the request and its own
                // transaction times out, so nothing hangs. Closing this needs a counter for
                // responses the endpoint could not send, which is a change to
                // `sipx_transport::Handle::respond`'s contract rather than to this call site.
                if let Err(error) = self.endpoint.respond(&incoming.key, builder.build()).await {
                    tracing::warn!(%error, code, "could not refuse an unclaimed request");
                }
            }
            // discard: the same loss one step earlier and with the same downstream count — a
            // refusal that cannot be built is a refusal that is not sent.
            Err(error) => tracing::warn!(%error, code, "could not build the refusal"),
        }
    }

    /// Refuse a renegotiation without ending the call.
    pub(crate) async fn refuse(
        &self,
        incoming: &Incoming,
        code: u16,
        reason: impl Into<Bytes>,
    ) -> Result<()> {
        Self::refuse_with(&self.endpoint, incoming, code, reason).await
    }

    /// Refuse through a cloned endpoint while the owning call is driving an outgoing exchange.
    pub(crate) async fn refuse_with(
        endpoint: &Handle,
        incoming: &Incoming,
        code: u16,
        reason: impl Into<Bytes>,
    ) -> Result<()> {
        let status = StatusCode::new(code).unwrap_or_else(ok_status);
        let response = ResponseBuilder::to_request(&incoming.request, status, reason)?.build();
        endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// End the call, for `cause`, which is emitted the moment `ended` flips — before the BYE is
    /// even built, so a call is reported over regardless of whether transmitting the BYE then
    /// succeeds. Shared by [`Self::hang_up`] (this side decided to end it) and the session-timer
    /// path in [`Self::on_session_deadline`] (the far end stopped answering), so both go through
    /// exactly the same teardown and the event this emits cannot drift from which one happened.
    ///
    /// Anything still queued is sent first, then the media stops, then the BYE goes out.
    /// Stopping first would discard the tail of whatever was playing — the last word of a
    /// clip, the last digit of a PIN — because sending is paced and the queue outlives the
    /// call by however much is left in it.
    async fn end(&mut self, cause: EndCause) -> Result<()> {
        let reason = match cause {
            EndCause::Timeout => request_timeout_reason(),
            _ => normal_clearing_reason(),
        };
        self.end_with_reason(cause, &reason).await
    }

    async fn begin_end(
        &mut self,
        cause: EndCause,
        reason: &ReasonValue,
    ) -> Result<Option<(Request, u32)>> {
        if self.ended {
            self.finish_media_ownership().await;
            return Ok(None);
        }
        self.media.flush(Duration::from_secs(5)).await;
        self.media.stop();
        self.ended = true;
        self.session = None;
        self.stop_ack_retransmission().await;
        self.events.end(cause);

        let cseq = self.dialog.next_cseq();
        match bye_request(&self.dialog, cseq, reason) {
            Ok(bye) => Ok(Some((bye, cseq))),
            Err(error) => {
                self.finish_media_ownership().await;
                Err(error)
            }
        }
    }

    async fn finish_media_ownership(&mut self) {
        self.stop_ack_retransmission().await;
        self.media.shutdown().await;
        self.reap_retired_media().await;
    }

    async fn end_with_reason(&mut self, cause: EndCause, reason: &ReasonValue) -> Result<()> {
        let Some((bye, _)) = self.begin_end(cause, reason).await? else {
            return Ok(());
        };
        let sent: Result<()> = async {
            let mut responses = self.endpoint.send(bye, self.target.clone()).await?;
            // A BYE that is never answered still ends the call locally: the alternative is a call
            // that cannot be hung up because the far end has already gone.
            //
            // discard: the BYE was handed over, and one the endpoint could not put on the wire is
            // counted at the transmit as `sipx_transport::UnsentCounts::bye` — the number an operator
            // asking "why did that call linger" needs. What is dropped here is only waiting for the
            // 200, and this side has already ended the call either way. The bound bounds a failure
            // (`X-29`).
            let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
            Ok(())
        }
        .await;
        self.finish_media_ownership().await;
        sent
    }

    /// End the call because this side decided to.
    pub async fn hang_up(&mut self) -> Result<()> {
        self.end(EndCause::LocalHangup).await
    }

    /// End the call and return the valid final response to the originated BYE.
    ///
    /// Unlike [`Self::hang_up`], this is an evidence-producing teardown: `within` bounds failure,
    /// and success requires the final response to name this exact dialog and the BYE's exact
    /// `CSeq`.
    /// A valid non-2xx is returned as [`Error::Rejected`], while a mismatched response is
    /// [`Error::InvalidDialogResponse`].
    pub async fn hang_up_observed(&mut self, within: Duration) -> Result<u16> {
        let reason = normal_clearing_reason();
        let Some((bye, cseq)) = self.begin_end(EndCause::LocalHangup, &reason).await? else {
            return Err(Error::InvalidDialogResponse);
        };
        let observed = async {
            let mut responses = self.endpoint.send(bye, self.target.clone()).await?;
            // Fixed duration bounds a failed teardown; the final response is the happens-before.
            let response = tokio::time::timeout(within, responses.final_response())
                .await
                .map_err(|_| Error::SignallingTeardownTimeout(within))?
                .ok_or(Error::SignallingTeardownTimeout(within))?;
            if !crate::signalling::response_matches_dialog(&response, &self.dialog, cseq) {
                return Err(Error::InvalidDialogResponse);
            }
            let status = response.status.code();
            if !response.status.is_success() {
                return Err(Error::Rejected {
                    status,
                    reason: String::from_utf8_lossy(&response.reason).into_owned(),
                });
            }
            Ok(status)
        }
        .await;
        self.finish_media_ownership().await;
        observed
    }

    /// End locally while continuing to answer requests which crossed the originated BYE.
    ///
    /// The ordinary observed hangup is sufficient when a dispatcher keeps driving the dialog.
    /// A one-call command owns the receiver itself, though, and must not stop reading it while it
    /// awaits the BYE response: the peer may have selected a BYE at the same time. That crossed
    /// request is still answered, while [`Self::begin_end`] ensures this side originates only one.
    async fn hang_up_observed_while_serving(
        &mut self,
        incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
        within: Duration,
    ) -> Result<u16> {
        let reason = normal_clearing_reason();
        let Some((bye, cseq)) = self.begin_end(EndCause::LocalHangup, &reason).await? else {
            return Err(Error::InvalidDialogResponse);
        };
        let endpoint = self.endpoint.clone();
        let target = self.target.clone();
        let dialog = self.dialog.clone();
        let observed = async move {
            let mut responses = endpoint.send(bye, target).await?;
            let response = responses
                .final_response()
                .await
                .ok_or(Error::SignallingTeardownTimeout(within))?;
            if !crate::signalling::response_matches_dialog(&response, &dialog, cseq) {
                return Err(Error::InvalidDialogResponse);
            }
            // The dialog is already ended locally. Any valid final response completes this
            // teardown exchange and is evidence worth reporting; a non-success status cannot
            // resurrect the call or turn successful media work into a command failure.
            Ok(response.status.code())
        };
        tokio::pin!(observed);
        let deadline = Instant::now() + within;
        let mut incoming_open = true;
        let result = loop {
            tokio::select! {
                biased;
                message = incoming.recv(), if incoming_open => match message {
                    Some(message)
                        if matches!(message.request.method, Method::Ack | Method::Bye) =>
                    {
                        if !self.handle(&message).await? {
                            self.refuse_unclaimed(&message).await;
                        }
                    }
                    Some(message) => self.refuse_unclaimed(&message).await,
                    None => incoming_open = false,
                },
                response = &mut observed => break response,
                () = tokio::time::sleep_until(deadline) => {
                    break Err(Error::SignallingTeardownTimeout(within));
                }
            }
        };
        self.finish_media_ownership().await;
        result
    }

    /// End the call with an explicit protocol cause.
    ///
    /// This is the coupled-leg shape from RFC 3326 §3.1: a controller which knows the winning
    /// response can tell the other dialog why it is being ended instead of reducing every
    /// teardown to a local hangup.
    pub async fn hang_up_with_reason(&mut self, reason: ReasonValue) -> Result<()> {
        self.end_with_reason(EndCause::LocalHangup, &reason).await
    }
}

/// Drive a call until it ends, honouring its session timer.
///
/// The loop a call needs is not just "read the next message": a session timer is a deadline,
/// and a call that only ever wakes on incoming traffic can never notice that no traffic has
/// arrived. This is that loop, written once so that the RFC 4028 half of it is not something
/// every caller has to remember.
///
/// Returns when the far end hangs up, or [`Error::SessionExpired`] when it stops answering.
///
/// # One call, or one of many
///
/// This is **the one-call convenience over [`Dispatcher`](crate::Dispatcher)** (story `C-4`), and
/// the receiver it takes is what makes it both things at once. Handed the endpoint's own
/// `Receiver<Incoming>` it is the single-call program it has always been; handed an inbox a
/// dispatcher routed, it drives one call of any number on the same endpoint. There is no second
/// loop for the many-call case, which is the point — a hand-rolled demultiplexer beside this one
/// is a fresh chance to drop an ACK.
///
/// The one-call form claims the whole endpoint, so it is right only when this is the only call
/// on it. Anything else arriving there is not this call's, and is answered as such below.
///
/// # Nothing is discarded
///
/// A request [`Call::handle`] does not claim is **answered**, not dropped: 405 with `Allow` when
/// it belongs to this dialog but names a method this call does not implement (RFC 3261 §8.2.1),
/// 481 when it names a dialog that is not this one (§12.2.2), 486 for a second INVITE arriving
/// where one call is being served (§21.4.24), and nothing at all for an ACK, which SIP has no
/// response to. This used to be a silent drop, and it is the call-layer twin of what `T-19`
/// removed at the transport layer.
pub async fn serve(
    call: &mut Call,
    incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
) -> Result<()> {
    while !call.is_ended() {
        let deadline = call.session_deadline();
        tokio::select! {
            message = incoming.recv() => match message {
                Some(message) => {
                    if !call.handle(&message).await? {
                        call.refuse_unclaimed(&message).await;
                    }
                }
                // The endpoint has shut down. The call cannot be worked any further, and
                // pretending otherwise would spin on a closed channel.
                None => return Ok(()),
            },
            () = sleep_until(deadline) => call.on_session_deadline().await?,
            // Built fresh on each pass so a re-INVITE follows the current media generation.
            () = call.drive_media_event() => {}
        }
    }
    Ok(())
}

/// Why [`serve_until`] returned.
#[derive(Debug)]
#[non_exhaustive]
pub enum Served<T> {
    /// The far end ended the confirmed dialog. Media work was cancelled and joined.
    Remote {
        /// The protocol cause selected by the call.
        cause: EndCause,
        /// The media work's partial or complete result after the call stopped it.
        output: T,
    },
    /// The supplied media work completed, after which one observed BYE ended the dialog.
    Local {
        /// The media work's result.
        output: T,
        /// The valid final status observed for the originated BYE, or its bounded failure.
        bye: Result<u16>,
    },
    /// The supplied stop input won, after which one observed BYE ended the dialog.
    Interrupted {
        /// The media work's partial result after cancellation.
        output: T,
        /// The valid final status observed for the originated BYE, or its bounded failure.
        bye: Result<u16>,
    },
}

/// Drive one confirmed call, its media work and a local stop input as one owned lifecycle.
///
/// Inbound dialog traffic has priority over `interrupted`, which has priority over local work
/// completion. ACK therefore stops successful-response retransmission while media runs, and a BYE
/// already queued when a local terminal input is polled wins without this side originating one.
/// A BYE which crosses local teardown is answered by the same loop while the originated BYE's
/// response remains bounded.
///
/// `work` receives the current media session and a cancellation token. The token fires before a
/// remote or interrupted end is joined. The future MUST be cancellation-safe and MUST resolve once
/// that token and the media session have stopped; this function does not return until it has.
pub async fn serve_until<F, W, S, T>(
    call: &mut Call,
    incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
    work: F,
    interrupted: S,
) -> Result<Served<T>>
where
    F: FnOnce(Arc<MediaSession>, CancellationToken) -> W,
    W: Future<Output = T>,
    S: Future<Output = ()>,
{
    let stop_work = CancellationToken::new();
    let work = work(Arc::clone(&call.media), stop_work.clone());
    tokio::pin!(work);
    tokio::pin!(interrupted);

    loop {
        let deadline = call.session_deadline();
        tokio::select! {
            biased;
            message = incoming.recv() => {
                if let Some(message) = message {
                    let handled = match call.handle(&message).await {
                        Ok(handled) => handled,
                        Err(error) => {
                            stop_work.cancel();
                            let _ = call.hang_up().await;
                            let _ = work.as_mut().await;
                            return Err(error);
                        }
                    };
                    if call.is_ended() {
                        stop_work.cancel();
                        let output = work.as_mut().await;
                        return Ok(Served::Remote {
                            cause: EndCause::RemoteBye,
                            output,
                        });
                    }
                    if !handled {
                        call.refuse_unclaimed(&message).await;
                    }
                } else {
                    stop_work.cancel();
                    let _ = call.hang_up().await;
                    let _ = work.as_mut().await;
                    return Err(Error::Transport(sipx_transport::Error::EndpointClosed));
                }
            },
            () = interrupted.as_mut() => {
                stop_work.cancel();
                let bye = call
                    .hang_up_observed_while_serving(incoming, Duration::from_secs(2))
                    .await;
                let output = work.as_mut().await;
                return Ok(Served::Interrupted { output, bye });
            }
            output = work.as_mut() => {
                let bye = call
                    .hang_up_observed_while_serving(incoming, Duration::from_secs(2))
                    .await;
                return Ok(Served::Local { output, bye });
            }
            () = sleep_until(deadline) => {
                if let Err(error) = call.on_session_deadline().await {
                    stop_work.cancel();
                    let _ = work.as_mut().await;
                    return Err(error);
                }
                if call.is_ended() {
                    stop_work.cancel();
                    let output = work.as_mut().await;
                    return Ok(Served::Remote {
                        cause: EndCause::Timeout,
                        output,
                    });
                }
            }
            () = call.drive_media_event() => {}
        }
    }
}

/// Wait for a deadline, or forever if there is none.
///
/// A free function rather than a method so that it borrows nothing: a future that borrowed the
/// call would collide with the `&mut` the other arm of the `select!` needs.
pub(crate) async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Add the dialog's route set, in order, as `Route` headers.
///
/// Without these, a request through a Record-Routing proxy — which is to say almost any real
/// deployment — is addressed straight at the peer's `Contact`, which the proxy will not relay
/// and the peer may not be reachable at. The call establishes and cannot be ended.
pub(crate) fn add_routes(
    mut builder: RequestBuilder,
    routes: &[String],
) -> std::result::Result<RequestBuilder, sipx_sip::error::BuildError> {
    for route in routes {
        builder = builder.header(HeaderName::Route, Bytes::from(route.clone()))?;
    }
    Ok(builder)
}

/// Build one application-owned request entirely from live dialog state.
fn application_request(
    dialog: &Dialog,
    method: &Method,
    cseq: u32,
    headers: &[sipx_sip::Header],
    body: Bytes,
) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let mut builder = RequestBuilder::new(method.clone(), uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .cseq(cseq, method)?
        .max_forwards(70);
    for header in headers {
        builder = builder.header(
            header.name().clone(),
            Bytes::copy_from_slice(header.raw_value()),
        )?;
    }
    Ok(add_routes(builder, &routes)?.body(body).build())
}

fn bye_request(dialog: &Dialog, cseq: u32, reason: &ReasonValue) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let builder = RequestBuilder::new(Method::Bye, uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        .header(HeaderName::Reason, Reason::from(reason.clone()).to_bytes())?
        .cseq(cseq, &Method::Bye)?
        .max_forwards(70);
    Ok(add_routes(builder, &routes)?.build())
}

/// How a call is placed.
#[derive(Debug, Clone)]
pub struct DialOptions {
    /// Our own address of record.
    pub from: String,
    /// Where this side receives media.
    pub media_address: IpAddr,
    /// The local interface on which the media socket is opened.
    ///
    /// This defaults to [`Self::media_address`] when constructed with [`Self::new`]. Set it
    /// independently when the SDP address is a public mapping which is not locally bindable. ICE
    /// nomination still owns the eventual media path when ICE is enabled; ordinary RTP cannot
    /// override that result.
    ///
    /// # Beta API migration
    ///
    /// Adding this public field deliberately breaks external `DialOptions` struct literals and
    /// exhaustive patterns. Add `media_bind_address` (normally equal to `media_address`) or move
    /// to [`Self::new`] and the builder methods. Constructor-based callers remain compatible.
    pub media_bind_address: IpAddr,
    /// Direction advertised by the initial SDP offer.
    ///
    /// `SendRecv` is the ordinary endpoint default. A two-dialog owner uses this to map the
    /// source leg's initial offer onto fresh SDP for its target leg without copying endpoint
    /// addresses, ports or key material.
    pub initial_direction: Direction,
    /// How long to wait for an answer before giving up and cancelling.
    ///
    /// `None` waits as long as the transaction layer does — 64·T1, or 32 seconds with the
    /// default constants. A bound *here* rather than around the call is what makes giving up
    /// correct: dropping the future partway through leaves the far end believing it is in a
    /// call, and only code inside the exchange can send the CANCEL that stops it.
    pub timeout: Option<Duration>,
    /// How long invitation cancellation may wait for its protocol completion events.
    ///
    /// This is distinct from [`Self::timeout`]: the answer deadline freezes the call outcome,
    /// then this allowance admits CANCEL or handles a final response which crossed that deadline.
    /// Zero performs no timed wait and never means an unbounded fallback.
    ///
    /// # Beta API migration
    ///
    /// Adding this public field deliberately breaks external `DialOptions` struct literals. Add
    /// `cancellation_timeout` or use [`Self::new`] and [`Self::with_cancellation_timeout`].
    pub cancellation_timeout: Duration,
    /// Ask for an RFC 4028 session timer of this length.
    ///
    /// `None` is the default and means no timer is requested. That is not the same as no timer
    /// being *run*: a far end that asks for one gets it, because refusing to refresh a session
    /// the peer is timing would have it hang up on a call that is working.
    pub session_expires: Option<Duration>,
    /// The pre-loaded route set to put on the INVITE, outermost proxy first (RFC 3608 §6.1).
    ///
    /// Empty by default: the INVITE goes to `target` and no further. Set it from a registrar's
    /// `Service-Route` — `UserAgent::service_route().rendered()` produces exactly this — when the
    /// registration says outbound requests must traverse proxies. Without it, a call placed
    /// through a registration reaches a proxy holding no state for it. This field serializes the
    /// `Route` headers; the application must resolve the outer hop and supply that transport
    /// destination as the [`Target`] passed to [`dial`]. The call layer does not resolve a Route
    /// URI or override the caller's target.
    pub service_route: Vec<String>,
    /// Application-supplied fields on the initial INVITE.
    ///
    /// Values have already passed [`sipx_sip::Header::build`]'s line-injection checks. The call
    /// layer retains them in the options so authentication and session-timer retries send the
    /// same request metadata as the first attempt. Applications remain responsible for refusing
    /// stack-owned routing and dialog fields before constructing these values.
    pub headers: Vec<sipx_sip::Header>,
    /// The media policy for this call.
    ///
    /// The default is G.711, no ICE. In particular, enabling a crate feature never changes what
    /// goes on the wire without an application selecting it.
    pub media: MediaPolicy,
    /// Credentials to answer a 401 or 407 during this call attempt.
    ///
    /// Owned by the application and retained only in the options it passes. Their `Debug`
    /// representation redacts the password, and the call path never logs an authorization value.
    /// [`dial`] and [`dial_once`] perform the bounded retry; [`dial_early`] surfaces
    /// [`Error::AuthenticationChallenge`] because its handle names the original INVITE.
    pub credentials: Option<Credentials>,
    /// Authentication service selected for this call's initial INVITE attempts.
    ///
    /// `None` is the wire-compatible default: no `Date` or `Identity` is added and no authority,
    /// credential, or time input is consulted. The policy owns those explicit caller inputs.
    pub identity: Option<OutboundIdentityPolicy>,
}

impl DialOptions {
    /// Options for a call from an address of record.
    #[must_use]
    pub fn new(from: impl Into<String>, media_address: IpAddr) -> Self {
        Self {
            from: from.into(),
            media_address,
            media_bind_address: media_address,
            initial_direction: Direction::SendRecv,
            timeout: None,
            cancellation_timeout: Duration::from_secs(2),
            session_expires: None,
            service_route: Vec::new(),
            headers: Vec::new(),
            media: MediaPolicy::default(),
            credentials: None,
            identity: None,
        }
    }

    /// Offer these codecs, most preferred first.
    ///
    /// [`Codecs::Opus`] puts Opus ahead of the G.711 pair in the offer; the far end's answer
    /// decides what the call carries, and a peer without Opus still gets G.711.
    #[must_use]
    pub fn with_codecs(mut self, codecs: Codecs) -> Self {
        self.media.codecs = codecs;
        self
    }

    /// Advertise this direction in the initial offer.
    #[must_use]
    pub const fn with_initial_direction(mut self, direction: Direction) -> Self {
        self.initial_direction = direction;
        self
    }

    /// Key this call with the selected mechanism.
    #[must_use]
    pub fn with_keying(mut self, keying: Keying) -> Self {
        self.media.keying = keying;
        self
    }

    /// Use this complete media policy for the call.
    #[must_use]
    pub fn with_media_policy(mut self, media: MediaPolicy) -> Self {
        self.media = media;
        self
    }

    /// Bind RTP on this local address without changing the address advertised in SDP.
    #[must_use]
    pub const fn with_media_bind_address(mut self, address: IpAddr) -> Self {
        self.media_bind_address = address;
        self
    }

    /// Traverse these proxies on the way out, outermost first (RFC 3608).
    ///
    /// The values are `Route` header values — `<sip:proxy.example;lr>` — which is what
    /// `ServiceRoute::rendered` returns. Order is normative: §6.1 requires a UA that exercises a
    /// service route to preserve the order the registrar listed. This only serializes headers:
    /// resolve the outer hop in the application and pass that address as the `target` to [`dial`].
    #[must_use]
    pub fn with_service_route(mut self, hops: Vec<String>) -> Self {
        self.service_route = hops;
        self
    }

    /// Add a validated application-owned field to every initial INVITE attempt.
    #[must_use]
    pub fn with_header(mut self, header: sipx_sip::Header) -> Self {
        self.headers.push(header);
        self
    }

    /// Answer a digest challenge with these credentials (RFC 3261 §22).
    #[must_use]
    pub fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Sign every initial INVITE attempt with this caller-owned authentication policy.
    #[must_use]
    pub fn with_identity(mut self, identity: OutboundIdentityPolicy) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Detect a far end that vanishes, by refreshing the session on this interval (RFC 4028).
    ///
    /// Without this, a peer that loses power leaves the call up forever: there is no BYE, the
    /// socket never closes, and nothing else in SIP notices. The interval is raised to the
    /// RFC's ninety-second floor if it is shorter, because a shorter one is an amplification
    /// vector rather than a configuration choice.
    #[must_use]
    pub fn with_session_timer(mut self, interval: Duration) -> Self {
        self.session_expires = Some(interval.max(session::ABSOLUTE_MIN_INTERVAL));
        self
    }

    /// Give up after this long.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Bound the distinct invitation-withdrawal phase after timeout or caller cancellation.
    #[must_use]
    pub const fn with_cancellation_timeout(mut self, timeout: Duration) -> Self {
        self.cancellation_timeout = timeout;
        self
    }
}

/// Place a call.
/// What this side offers, and the description that carries it.
///
/// A key is offered only when the transport protects it: SDES puts the master key in the SDP
/// body, so offering one over cleartext SIP publishes it (RFC 4568 §7.1).
///
/// Both the receive address and the codec set come from the caller's [`DialOptions`], so they are
/// taken as one rather than passed apart: they are two halves of the same decision about what this
/// side is offering, and splitting them invites a call site that reads the set from somewhere else.
#[derive(Debug)]
enum PendingKeying {
    Sdes,
    #[cfg(feature = "dtls")]
    Dtls(sipx_media::dtls::openssl::Identity),
}

/// Refuse an impossible named profile before binding, gathering, certificate creation, or SIP I/O.
fn validate_profile_preflight(policy: MediaPolicy, transport: TransportKind) -> Result<()> {
    if policy.profile == MediaProfile::Standard {
        return Ok(());
    }
    if !cfg!(feature = "opus") {
        return Err(sipx_sdp::browser_audio::ProfileError::OpusUnavailable.into());
    }
    if !cfg!(feature = "dtls") {
        return Err(Error::DtlsUnavailable);
    }
    if transport != TransportKind::Wss {
        return Err(sipx_sdp::browser_audio::ProfileError::InsecureSignalling.into());
    }
    if policy.ice == IcePolicy::Disabled {
        return Err(sipx_sdp::browser_audio::ProfileError::IceRequired.into());
    }
    if policy.keying != Keying::DtlsSrtp {
        return Err(sipx_sdp::browser_audio::ProfileError::WeakerMedia.into());
    }
    #[cfg(feature = "opus")]
    if policy.codecs != Codecs::Opus {
        return if policy.codecs.carries(Codec::Opus) {
            Err(sipx_sdp::browser_audio::ProfileError::CodecSetIncomplete.into())
        } else {
            Err(sipx_sdp::browser_audio::ProfileError::OpusUnavailable.into())
        };
    }
    Ok(())
}

/// Build the capabilities selected by policy and retain anything the later handshake needs.
fn media_capabilities(
    policy: MediaPolicy,
    address: IpAddr,
    port: u16,
    secure_signalling: bool,
) -> Result<(Capabilities, PendingKeying)> {
    if policy.profile == MediaProfile::Standard
        && policy.keying == Keying::DtlsSrtp
        && policy.ice != IcePolicy::Disabled
    {
        return Err(Error::Sdp(
            "DTLS-SRTP cannot yet be combined with ICE on one media port".to_owned(),
        ));
    }
    // Offer mux on every ordinary audio exchange. A peer that omits it in the answer selects the
    // established adjacent-port fallback; no retry or second offer is needed (RFC 5761 §5.1.1).
    let capabilities = policy.codecs.capabilities(address, port).with_rtcp_mux();
    match policy.keying {
        Keying::Auto => Ok((
            capabilities.with_srtp(secure_signalling),
            PendingKeying::Sdes,
        )),
        Keying::Plain => Ok((capabilities, PendingKeying::Sdes)),
        Keying::Sdes => {
            if !secure_signalling {
                return Err(Error::Sdp(
                    "SDES-SRTP requires protected signalling".to_owned(),
                ));
            }
            Ok((capabilities.with_srtp(true), PendingKeying::Sdes))
        }
        Keying::DtlsSrtp => {
            #[cfg(feature = "dtls")]
            {
                let identity = sipx_media::dtls::openssl::Identity::generate()
                    .map_err(|error| Error::Dtls(error.to_string()))?;
                let fingerprint = identity
                    .fingerprint()
                    .map_err(|error| Error::Dtls(error.to_string()))?;
                Ok((
                    capabilities.with_dtls_srtp(fingerprint),
                    PendingKeying::Dtls(identity),
                ))
            }
            #[cfg(not(feature = "dtls"))]
            {
                Err(Error::DtlsUnavailable)
            }
        }
    }
}

async fn offered_media(
    options: &DialOptions,
    port: &MediaPort,
    transport: TransportKind,
) -> Result<(
    Capabilities,
    SessionDescription,
    Option<LocalDescription>,
    PendingKeying,
)> {
    let local_ice = match options.media.gathering(true)? {
        // An initial offer has not settled mux yet, so retain component 2 and its `a=rtcp`
        // destination for RFC 5761's no-second-exchange fallback.
        Some(gathering) => {
            let rtcp_mode = if options.media.profile == MediaProfile::BrowserAudio {
                sipx_sdp::RtcpMode::Mux
            } else {
                sipx_sdp::RtcpMode::Separate
            };
            Some(port.gather_with_rtcp_mode(&gathering, rtcp_mode).await)
        }
        None => None,
    };
    let advertised = local_ice
        .as_ref()
        .and_then(|local| local.default_destination(ComponentId::RTP))
        .unwrap_or_else(|| SocketAddr::new(options.media_address, port.local_addr().port()));
    let (capabilities, keying) = media_capabilities(
        options.media,
        advertised.ip(),
        advertised.port(),
        transport.is_secure(),
    )?;
    let mut capabilities = capabilities;
    capabilities.direction = options.initial_direction;
    let offer = if options.media.profile == MediaProfile::BrowserAudio {
        let local = local_ice
            .as_ref()
            .ok_or(sipx_sdp::browser_audio::ProfileError::IceRequired)?;
        let fingerprint = capabilities
            .dtls()
            .cloned()
            .ok_or(sipx_sdp::browser_audio::ProfileError::FingerprintRequired)?;
        sipx_sdp::browser_audio::offer(&sipx_sdp::browser_audio::BrowserAudioLocal {
            address: advertised.ip(),
            port: advertised.port(),
            session_id: capabilities.session_id,
            session_version: capabilities.session_version,
            direction: options.initial_direction,
            ice: local.credentials().clone(),
            candidates: local.candidates().to_vec(),
            fingerprint,
            setup: sipx_sdp::fingerprint::SetupCapabilities::both(),
        })?
    } else {
        let mut offer = offer_from(&capabilities);
        if let Some(local) = &local_ice {
            add_ice(&mut offer, local, &[]);
        }
        offer
    };
    Ok((capabilities, offer, local_ice, keying))
}

/// The INVITE that opens a call.
///
/// Its own function only because `dial` had grown past the point where the interesting part —
/// what happens to the *response* — was visible among the header construction.
/// What identifies the dialog an INVITE is trying to create.
///
/// Held apart from the message so a retry can keep it. RFC 4028 §7.3 says a request re-sent
/// after a `422` "SHOULD have the same value as the Call-ID, To, and From of the previous
/// request" — a fresh identity would look to the far end like a second, unrelated call attempt
/// rather than the answer to the counter-offer it just made.
#[derive(Debug, Clone)]
struct Identity {
    call_id: String,
    from_tag: String,
    cseq: u32,
}

impl Identity {
    fn fresh() -> Self {
        Self {
            call_id: format!("{}@sipx", token()),
            from_tag: token(),
            cseq: 1,
        }
    }

    /// The same dialog, one transaction later.
    fn again(&self) -> Self {
        Self {
            cseq: self.cseq.saturating_add(1),
            ..self.clone()
        }
    }
}

/// Everything that goes into the INVITE besides where it is being sent.
struct Invitation<'a> {
    to: &'a Uri,
    from: &'a str,
    via: &'a str,
    offer: Option<&'a SessionDescription>,
    session_expires: Option<Duration>,
    identity: &'a Identity,
    /// The pre-loaded route set, outermost first (RFC 3608 §6.1).
    service_route: &'a [String],
    /// Validated application-owned fields, preserved across retries.
    headers: &'a [sipx_sip::Header],
}

fn build_invite(
    endpoint: &Handle,
    target: &Target,
    invitation: &Invitation<'_>,
) -> Result<Request> {
    let &Invitation {
        to,
        from,
        via,
        offer,
        session_expires,
        identity,
        service_route,
        headers,
    } = invitation;
    let Identity {
        call_id,
        from_tag,
        cseq,
    } = identity;
    let mut builder = RequestBuilder::new(Method::Invite, to.clone())
        .header(HeaderName::Via, Bytes::from(via.to_owned()))?
        .header(
            HeaderName::To,
            Bytes::from(format!("<{}>", String::from_utf8_lossy(&to.to_bytes()))),
        )?
        .header(
            HeaderName::From,
            Bytes::from(format!("{from};tag={from_tag}")),
        )?
        .header(HeaderName::CallId, Bytes::from(call_id.clone()))?
        .cseq(*cseq, &Method::Invite)?
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, target.transport)),
        )?
        .max_forwards(70);
    if let Some(offer) = offer {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(offer.to_string_sdp()));
    }

    // One `Supported` row listing everything this side can do. Both tags are statements of
    // capability rather than requests: `timer` tells a far end that wants liveness detection
    // that it may have it, and `100rel` (RFC 3262 §4) is what permits the far end to send a
    // reliable provisional at all — §3 forbids it outright if we stay quiet, which means a
    // silent UAC gets unreliable ringing even from a UAS that would rather not send it.
    builder = builder.header(
        HeaderName::Supported,
        Bytes::from_static(b"timer, 100rel, histinfo"),
    )?;
    builder = builder.header(
        HeaderName::HistoryInfo,
        HistoryInfo::initial(to.clone()).to_bytes(),
    )?;
    // RFC 3311 §4: "A UAC compliant to this specification SHOULD also include an Allow header
    // field in the INVITE request, listing the method UPDATE." It is the only way the far end
    // is permitted to decide it may renegotiate the early session or refresh with an UPDATE
    // rather than a re-INVITE, so leaving it off does not merely omit a courtesy — it silently
    // forces every peer onto the heavier method for the life of the dialog.
    builder = builder.header(
        HeaderName::Allow,
        Bytes::from_static(update::ALLOW.as_bytes()),
    )?;
    if let Some(interval) = session_expires {
        // No `refresher` parameter. RFC 4028 Table 2 row 4 lets the UAS choose when the UAC
        // has not, and the UAS is the side that knows whether it is behind a NAT or a proxy
        // that cares. Naming ourselves would override a better-informed decision.
        let expires = SessionExpires {
            interval,
            refresher: None,
        };
        builder = builder
            .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
            .header(
                HeaderName::MinSe,
                Bytes::from(session::ABSOLUTE_MIN_INTERVAL.as_secs().to_string()),
            )?;
    }
    for header in headers {
        builder = builder.header(
            header.name().clone(),
            Bytes::copy_from_slice(header.raw_value()),
        )?;
    }
    // Pre-loaded Route, before the request goes anywhere. RFC 3608 §6.1 has the service route
    // used "as a preloaded Route header field in outgoing initial requests", and §6.1 requires
    // the order preserved — which `add_routes` does by appending in sequence. This is the only
    // place the INVITE can acquire it: a Route added after the transaction is created would not
    // be on the message the far end matched.
    let builder = add_routes(builder, service_route)?;
    Ok(builder.build())
}

/// One digest answer to attach to a retried INVITE.
struct Authorization<'a> {
    challenge: &'a sipx_sip::auth::Challenge,
    credentials: &'a Credentials,
    nonce_count: u32,
    cnonce: &'a str,
}

/// Add the header a 401 or 407 asked for, covering this request's method and URI.
fn authorize_invite(request: &mut Request, authorization: &Authorization<'_>) -> Result<()> {
    let uri = String::from_utf8_lossy(&request.uri.to_bytes()).into_owned();
    let method = String::from_utf8_lossy(request.method.as_bytes()).into_owned();
    let value = sipx_sip::auth::respond(
        authorization.challenge,
        authorization.credentials,
        &method,
        &uri,
        authorization.nonce_count,
        authorization.cnonce,
    );
    request.headers.push(sipx_sip::Header::build(
        authorization.challenge.response_header(),
        Bytes::from(value),
    )?);
    Ok(())
}

/// Place a call, retrying once if the far end refuses the session interval.
///
/// RFC 4028 §7.3: a `422` is not a refusal of the call, it is a counter-offer of an interval.
/// Retrying is bounded to a single attempt on purpose — a peer that answers 422 to its own
/// stated minimum is broken, and a loop there is an outbound call flood.
pub async fn dial(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Call> {
    dial_retrying(endpoint, target, to, options, true, None).await
}

/// Place a call until it completes or `cancelled` resolves.
///
/// This has the same bounded authentication and session-interval retries as [`dial`]. If
/// cancellation wins while an INVITE is outstanding, the invitation is withdrawn before this
/// returns, including the ACK-then-BYE race when a successful final response was already in
/// flight. The returned [`Error::Cancelled`] distinguishes that local stop from a peer refusal.
pub async fn dial_until<F>(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
    cancelled: F,
) -> Result<Call>
where
    F: Future<Output = ()> + Send,
{
    tokio::pin!(cancelled);
    let cancelled: Pin<&mut (dyn Future<Output = ()> + Send)> = cancelled.as_mut();
    dial_retrying(endpoint, target, to, options, true, Some(cancelled)).await
}

/// Place a call, surfacing a `422` instead of retrying it.
///
/// [`dial`] is this plus one retry, and is what almost everything wants. This is here for a
/// caller that would rather decide for itself what to do about an interval it does not like —
/// a gateway with a policy about how often it is willing to be woken, say.
pub async fn dial_once(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Call> {
    dial_retrying(endpoint, target, to, options, false, None).await
}

type Cancelled<'a> = Pin<&'a mut (dyn Future<Output = ()> + Send)>;

/// Drive the two bounded retry reasons an initial INVITE has: authentication and session interval.
async fn dial_retrying(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
    retry_interval: bool,
    mut cancelled: Option<Cancelled<'_>>,
) -> Result<Call> {
    let credentials = options.credentials.clone();
    let mut attempted = options.clone();
    let mut identity = Identity::fresh();
    let mut challenge: Option<Box<sipx_sip::auth::Challenge>> = None;
    let mut nonce_use: Option<(String, u32)> = None;
    let mut stale_retried = false;
    let mut interval_retried = false;

    loop {
        let cnonce = token();
        let authorization =
            challenge
                .as_deref()
                .zip(credentials.as_ref())
                .map(|(challenge, credentials)| Authorization {
                    challenge,
                    credentials,
                    nonce_count: nonce_count_for(&mut nonce_use, &challenge.nonce),
                    cnonce: &cnonce,
                });
        let result = dial_with(
            endpoint,
            target.clone(),
            to,
            &attempted,
            &identity,
            authorization.as_ref(),
            &mut cancelled,
        )
        .await;

        match result {
            Err(Error::AuthenticationChallenge {
                status,
                reason,
                challenge: received,
            }) => {
                let rejected = || Error::Rejected {
                    status,
                    reason: reason.clone(),
                };
                if credentials.is_none() {
                    return Err(rejected());
                }
                if challenge.is_none() {
                    challenge = Some(received);
                } else if received.stale && !stale_retried {
                    stale_retried = true;
                    challenge = Some(received);
                } else {
                    return Err(rejected());
                }
            }
            Err(Error::IntervalTooBrief(required)) if retry_interval && !interval_retried => {
                interval_retried = true;
                attempted.session_expires = Some(required.max(session::ABSOLUTE_MIN_INTERVAL));
            }
            other => return other,
        }
        identity = identity.again();
    }
}

/// The count of requests sent under `nonce`, starting over when the nonce changes.
fn nonce_count_for(nonce_use: &mut Option<(String, u32)>, nonce: &str) -> u32 {
    let count = match nonce_use {
        Some((last, count)) if last == nonce => count.saturating_add(1),
        _ => 1,
    };
    *nonce_use = Some((nonce.to_owned(), count));
    count
}

/// Bind the media port and build the INVITE that will advertise it.
///
/// Split out of `dial_with` only for length: what is interesting there is what happens to the
/// *response*, and it was buried under header construction.
async fn open_invitation(
    endpoint: &Handle,
    target: &Target,
    to: &Uri,
    options: &DialOptions,
    identity: &Identity,
    authorization: Option<&Authorization<'_>>,
) -> Result<(
    MediaPort,
    Capabilities,
    Option<LocalDescription>,
    PendingKeying,
    Request,
)> {
    validate_profile_preflight(options.media, target.transport)?;
    MediaAddress::new(options.media_address)
        .with_bind(options.media_bind_address)
        .validate()?;
    // The offer has to name the port audio will arrive on, and only a bound socket knows it.
    // So the port is bound now and the session started once the answer says where and in what.
    let port = MediaPort::bind(SocketAddr::new(options.media_bind_address, 0))
        .await
        .map_err(Error::Io)?;

    let (capabilities, offer, ice, keying) =
        offered_media(options, &port, target.transport).await?;

    // The `Via` is built here rather than left to the transport, because a CANCEL has to carry
    // the *same* branch as the INVITE it cancels — that identity is what matches the two at the
    // far end (RFC 3261 §9.1). Letting the transport generate it would leave this layer unable
    // to name the transaction it started.
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );

    let mut invite = build_invite(
        endpoint,
        target,
        &Invitation {
            to,
            from: options.from.as_str(),
            via: &via,
            offer: Some(&offer),
            session_expires: options.session_expires,
            identity,
            service_route: &options.service_route,
            headers: &options.headers,
        },
    )?;
    if let Some(authorization) = authorization {
        authorize_invite(&mut invite, authorization)?;
    }
    if let Some(identity) = &options.identity {
        identity.sign(&mut invite)?;
    }
    Ok((port, capabilities, ice, keying, invite))
}

/// Build the RFC 3262 delayed-offer form of an INVITE.
///
/// No media socket is bound yet because the remote offer determines whether there is a session
/// to answer. The socket is created when that offer arrives in a reliable provisional, before
/// its answer is placed in PRACK.
fn open_offerless_invitation(
    endpoint: &Handle,
    target: &Target,
    to: &Uri,
    options: &DialOptions,
    identity: &Identity,
) -> Result<Request> {
    MediaAddress::new(options.media_address)
        .with_bind(options.media_bind_address)
        .validate()?;
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );
    let mut invite = build_invite(
        endpoint,
        target,
        &Invitation {
            to,
            from: options.from.as_str(),
            via: &via,
            offer: None,
            session_expires: options.session_expires,
            identity,
            service_route: &options.service_route,
            headers: &options.headers,
        },
    )?;
    if let Some(identity) = &options.identity {
        identity.sign(&mut invite)?;
    }
    Ok(invite)
}

/// Take back an invitation the caller has stopped waiting for.
///
/// Split out of `dial_with` for length, but it is the part with all the hazards in it, so it
/// keeps its own name: everything here is about not leaving the far end in a call.
async fn withdraw(
    endpoint: &Handle,
    invite: &Request,
    target: Target,
    responses: &mut sipx_transport::Responses,
    reason: &ReasonValue,
    limit: Duration,
) -> CancellationCleanup {
    // Giving up is not just ceasing to wait. The far end is ringing and has been told
    // nothing; without a CANCEL it goes on ringing, and someone answering afterwards
    // ends up in a call with a party that has left.
    //
    // The transport operation owns §9.1's race: it waits until the exact INVITE has a provisional,
    // or returns the final/timeout/transport event that won instead. Events it observes remain on
    // `responses`, so the crossing-2xx safeguard below sees the same transaction history.
    let started = Instant::now();
    let deadline = started + limit;
    let mut cancel_sent = false;
    let mut final_response_observed = false;
    let operation = async {
        match endpoint
            .cancel_invite(responses, Some(Reason::from(reason.clone())))
            .await
        {
            Ok(sipx_transport::CancelInviteOutcome::Sent(_cancellation)) => {
                cancel_sent = true;
            }
            Ok(sipx_transport::CancelInviteOutcome::FinalResponse { response, .. }) => {
                final_response_observed = true;
                if response.status.is_success() {
                    ack_then_bye(endpoint, invite, &response, target).await;
                }
                return true;
            }
            Ok(_) => return true,
            // The loss is counted where transport output is attempted. This is already the
            // giving-up path; endpoint shutdown remains the finite ownership barrier.
            Err(error) => {
                tracing::debug!(%error, "could not create CANCEL transaction");
                return false;
            }
        }

        // CANCEL cannot close the race it exists to manage: a 200 already in flight arrives
        // anyway, and RFC 3261 §15 says a UAC that will not proceed must acknowledge it and then
        // hang up rather than leave it unanswered.
        while let Some(event) = responses.next().await {
            let sipx_sip::transaction::TuEvent::Response(late) = event else {
                continue;
            };
            if !late.status.is_final() {
                continue;
            }
            final_response_observed = true;
            if late.status.is_success() {
                ack_then_bye(endpoint, invite, &late, target.clone()).await;
            }
            return true;
        }
        true
    };
    // Fixed duration bounds failed cancellation; a transaction event is successful completion.
    let (completed, exhausted) = match tokio::time::timeout_at(deadline, Box::pin(operation)).await
    {
        Ok(completed) => (completed, false),
        Err(_elapsed) => (false, true),
    };
    CancellationCleanup {
        limit,
        elapsed: started.elapsed(),
        disposition: if exhausted {
            CancellationDisposition::Exhausted {
                cancel_sent,
                final_response_observed,
            }
        } else if completed {
            CancellationDisposition::Completed {
                cancel_sent,
                final_response_observed,
            }
        } else {
            CancellationDisposition::Failed { cancel_sent }
        },
    }
}

/// Acknowledge a 2xx this side will not proceed with, and hang the dialog up (RFC 3261 §15).
///
/// Both callers reach it having already put a 2xx beyond recall: [`withdraw`], where a CANCEL lost
/// its race, and [`dial_with`], where establishing the call failed after the far end already
/// believed one existed. Neither may simply walk away — an unacknowledged 2xx is retransmitted for
/// 32 seconds and then streamed at a port this side has closed.
///
/// Every step is best-effort by design. This runs on a path that is already failing, and a BYE
/// that cannot be built or sent must not mask the error that brought us here.
async fn ack_then_bye(endpoint: &Handle, invite: &Request, response: &Response, target: Target) {
    let Some(dialog) = Dialog::from_response(invite, response) else {
        return;
    };
    let in_dialog = in_dialog_target(&dialog, target);
    // discard: counted as `sipx_transport::UnsentCounts::ack`. An ACK for a 2xx that does not go
    // out is the worst of the three — it has no transaction to retry it (RFC 3261 §13.2.2.4), so
    // the far end retransmits its 2xx for thirty-two seconds and then streams at a port this side
    // has closed. This one goes out through `send_directly`, so the `Result` dropped here *does*
    // report the transmit; it is dropped because the BYE below is the only remedy and is attempted
    // whether or not the ACK landed, and returning the error would only mask the failure that
    // brought us here.
    let _ = send_ack(endpoint, &dialog, in_dialog.clone()).await;
    if let Ok(bye) = bye_request(
        &dialog,
        dialog.local_cseq.saturating_add(1),
        &normal_clearing_reason(),
    ) {
        // discard: counted as `sipx_transport::UnsentCounts::bye` — the number an operator asking
        // "why did that call linger" needs, because a BYE that does not reach the wire leaves a
        // dialog up at the far end that no timer reaps unless RFC 4028 session timers happen to be
        // running. The count is taken at the transmit and not from this `Result`, which reports
        // only that the transaction was created. Nothing here can retry it: this is the failure
        // path itself.
        let _ = endpoint.send(bye, in_dialog).await;
    }
}

/// What a non-2xx final response means to the caller.
///
/// RFC 4028 §6's 422 is separated out because it is the one rejection that is *actionable*: it
/// names the interval the far end would accept, so a caller can retry with it rather than only
/// learn that it failed. Every other status is reported as it arrived.
fn rejection(response: &Response) -> Error {
    const INTERVAL_TOO_SMALL: u16 = 422;
    if response.status.code() == INTERVAL_TOO_SMALL
        && let Some(required) = required_interval(response)
    {
        return Error::IntervalTooBrief(required);
    }
    if matches!(response.status.code(), 401 | 407) {
        let from_proxy = response.status.code() == 407;
        let header = if from_proxy {
            HeaderName::ProxyAuthenticate
        } else {
            HeaderName::WwwAuthenticate
        };
        let challenges = response
            .headers
            .get_all(&header)
            .filter_map(|header| sipx_sip::auth::Challenge::parse(&header.value(), from_proxy))
            .collect();
        if let Some(challenge) = sipx_sip::auth::strongest(challenges) {
            return Error::AuthenticationChallenge {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
                challenge: Box::new(challenge),
            };
        }
    }
    Error::Rejected {
        status: response.status.code(),
        reason: String::from_utf8_lossy(&response.reason).into_owned(),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the establishment sequence keeps every post-2xx path visibly ACK-safe"
)]
async fn dial_with(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
    identity: &Identity,
    authorization: Option<&Authorization<'_>>,
    cancelled: &mut Option<Cancelled<'_>>,
) -> Result<Call> {
    let media_address = options.media_address;
    let (port, capabilities, ice, keying, invite) =
        open_invitation(endpoint, &target, to, options, identity, authorization).await?;

    let mut responses = endpoint.send(invite.clone(), target.clone()).await?;
    let invitation_started = Instant::now();

    let mut acknowledging = Acknowledging {
        endpoint,
        invite: &invite,
        target: &target,
        capabilities: &capabilities,
        seen: sipx_sip::rel::Sequence::default(),
    };
    let (response, ringing) = match await_final(
        &mut responses,
        options.timeout,
        &mut acknowledging,
        cancelled,
    )
    .await
    {
        Waited::Final { response, ringing } => (response, ringing),
        Waited::Gone => return Err(Error::NoResponse),
        Waited::Transport(error) => return Err(Error::Transport(error)),
        Waited::GaveUp => {
            let invitation_elapsed = invitation_started.elapsed();
            let cleanup = withdraw(
                endpoint,
                &invite,
                target.clone(),
                &mut responses,
                &request_timeout_reason(),
                options.cancellation_timeout,
            )
            .await;
            return Err(Error::Cancelled(InvitationCancellation {
                timed_out: true,
                invitation_limit: options.timeout,
                invitation_elapsed,
                cleanup,
            }));
        }
        Waited::Cancelled => {
            let invitation_elapsed = invitation_started.elapsed();
            let cleanup = withdraw(
                endpoint,
                &invite,
                target.clone(),
                &mut responses,
                &normal_clearing_reason(),
                options.cancellation_timeout,
            )
            .await;
            return Err(Error::Cancelled(InvitationCancellation {
                timed_out: false,
                invitation_limit: options.timeout,
                invitation_elapsed,
                cleanup,
            }));
        }
    };

    if !response.status.is_success() {
        // A non-2xx is acknowledged by the transaction layer itself, so there is nothing to
        // send here — only a media port to release, which happens when `port` drops.
        return Err(rejection(&response));
    }

    // From here the far end believes a dialog exists, so *every* path must acknowledge.
    // Returning an error without one leaves it retransmitting its 200 for 32 seconds and then
    // streaming media at a port we have closed.
    // Where in-dialog requests go if the 2xx carries no `Contact` to refresh the target with.
    let fallback = target.clone();
    match establish(
        &invite,
        &response,
        fallback,
        port,
        ice,
        &capabilities,
        options,
    ) {
        Ok((dialog, port, in_dialog, settled, ice, answer)) => {
            let ack = build_ack(endpoint, &dialog, &in_dialog)?;
            endpoint
                .send_directly(ack.clone(), in_dialog.clone())
                .await?;
            // The stream stays open rather than being dropped here: a retransmitted 2xx means
            // this ACK was lost and RFC 3261 §13.2.2.4 requires another (see
            // `reack_retransmitted_2xx`).
            tokio::spawn(reack_retransmitted_2xx(
                endpoint.clone(),
                responses,
                ack,
                in_dialog.clone(),
            ));
            // RFC 5763 peers are allowed to wait for the SIP exchange to complete before opening
            // the media connection. In particular, the ACK must be on the wire before a selected
            // DTLS handshake can wait for that peer, or two correct endpoints can deadlock.
            let (media, settled) = match key_and_start(
                port,
                ice,
                settled,
                keying,
                &answer,
                false,
                options.media.profile,
            )
            .await
            {
                Ok(started) => started,
                Err(error) => {
                    // The 2xx was already acknowledged, so RFC 3261 §15 tears down the dialog
                    // whose selected media path could not be keyed. The transport counts an
                    // unsent BYE; the result is discarded so it cannot mask the DTLS error.
                    if let Ok(bye) = bye_request(
                        &dialog,
                        dialog.local_cseq.saturating_add(1),
                        &normal_clearing_reason(),
                    ) {
                        // discard: the original DTLS failure is the cause returned to the
                        // caller; a best-effort teardown failure must not replace it.
                        let _ = endpoint.send(bye, in_dialog).await;
                    }
                    return Err(error);
                }
            };
            // Emitted at construction — the earliest point this call has a stream anyone could
            // read from — from what was actually observed while waiting for the final response,
            // not reconstructed later from anything left lying around.
            let (events, events_rx) = EventSink::new();
            emit_construction_events(&events, ringing);
            Ok(Call {
                dialog,
                initial_status: response.status.code(),
                media: Arc::new(media),
                retired_media: Vec::new(),
                endpoint: endpoint.clone(),
                target: in_dialog,
                ack_stop: None,
                ack_retransmission: None,
                delayed_offer: None,
                ended: false,
                media_address,
                media_bind_address: options.media_bind_address,
                codecs: options.media.codecs,
                profile: options.media.profile,
                current: settled.negotiated,
                peer_ice: peer_ice_credentials(response.body()),
                encrypted: options.media.profile == MediaProfile::BrowserAudio
                    || settled.srtp.is_some(),
                keying: options.media.keying,
                hold: Direction::SendRecv,
                referral: None,
                transfer: None,
                session: session::adopt(
                    response
                        .headers
                        .typed::<SessionExpires>()
                        .and_then(std::result::Result::ok),
                    options.session_expires,
                )
                .map(SessionState::armed),
                negotiation: update::Negotiation::idle(),
                // From the 2xx, which RFC 3311 §4 asks to carry it. A dialog that reaches here
                // has completed one offer/answer exchange, so nothing is outstanding either way.
                peer_allows_update: update::peer_allows(&response.headers),
                events,
                events_rx: Some(events_rx),
                history: HistoryInfo::from_headers(&response.headers)
                    .and_then(std::result::Result::ok),
                dialog_credentials: options.credentials.clone(),
                admitted_dialog_methods: Vec::new(),
            })
        }
        Err(error) => {
            // RFC 3261 §15: a UAC that cannot proceed after a 2xx acknowledges it and then
            // sends BYE. Walking away silently is what leaves the far end streaming.
            ack_then_bye(endpoint, &invite, &response, target).await;
            Err(error)
        }
    }
}

/// Everything after a 2xx that can fail, kept together so the caller can ACK on either path.
///
/// `offered` and `options` are taken whole rather than as the two fields read out of them, because
/// both are the same question asked twice — what this side put in the offer — and a call site that
/// passed a crypto list from one place and a codec set from another could pass two that disagree.
fn establish(
    invite: &Request,
    response: &Response,
    fallback: Target,
    port: MediaPort,
    ice: Option<LocalDescription>,
    offered: &Capabilities,
    options: &DialOptions,
) -> Result<(
    Dialog,
    MediaPort,
    Target,
    Settled,
    Option<LocalDescription>,
    SessionDescription,
)> {
    let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
    validate_establishment_answer(options.media.profile, invite.body(), &answer)?;
    let settled = settle_answer(offered, &answer, options.media.codecs)?;
    let dialog = Dialog::from_response(invite, response).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, fallback);
    let ice = match ice {
        Some(mut local) => {
            let negotiation = answer
                .media
                .first()
                .map_or(IceNegotiation::Absent, |audio| {
                    sipx_media::ice::negotiate(&answer, audio)
                });
            local.accept(&negotiation);
            Some(local)
        }
        None => None,
    };
    Ok((dialog, port, target, settled, ice, answer))
}

/// Hold the named profile boundary ahead of every stateful part of answer application.
///
/// The generic codec settlement below this function is intentionally more permissive than the
/// browser-audio contract. Running the complete exchange validator here keeps an invalid answer
/// from reaching `LocalDescription::accept`, ACK transmission, ICE checks, or DTLS setup.
fn validate_establishment_answer(
    profile: MediaProfile,
    offered: &[u8],
    answer: &SessionDescription,
) -> Result<()> {
    if profile == MediaProfile::BrowserAudio {
        let offered = sipx_sdp::parse(&String::from_utf8_lossy(offered))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        // discard: the complete relation is checked here; the generic settlement immediately
        // below derives the retained codec/ICE facts from this same validated answer.
        let _ = sipx_sdp::browser_audio::validate_answer(
            &offered,
            answer,
            sipx_sdp::fingerprint::SetupCapabilities::both(),
        )?;
    }
    Ok(())
}

/// Resolve the local handshake role from the peer description using SDP's shared level fallback.
fn dtls_local_setup(
    peer_description: &SessionDescription,
    local_is_answerer: bool,
) -> Result<sipx_sdp::fingerprint::Setup> {
    let audio = peer_description.media.first().ok_or(Error::NoCommonCodec)?;
    let peer_setup = sipx_sdp::answer::setup_of(peer_description, audio);
    let roles = sipx_sdp::fingerprint::SetupCapabilities::both();
    if local_is_answerer {
        roles
            .answer_to(peer_setup.unwrap_or(sipx_sdp::fingerprint::Setup::ActPass))
            .map_err(Error::from)
    } else {
        roles.from_answer(peer_setup).map_err(Error::from)
    }
}

/// Reject an unusable DTLS offer before binding or gathering for its answer.
fn validate_dtls_offer_setup(offer: &SessionDescription, policy: MediaPolicy) -> Result<()> {
    if policy.keying == Keying::DtlsSrtp {
        // discard: validation is the side effect; the selected role is resolved again when the
        // handshake starts, after the successful answer has been transmitted.
        let _ = dtls_local_setup(offer, true)?;
    }
    Ok(())
}

/// Complete selected keying and only then start the media workers on the same bound port.
#[cfg_attr(not(feature = "dtls"), allow(unused_mut, clippy::unused_async))]
async fn key_and_start(
    port: MediaPort,
    ice: Option<LocalDescription>,
    mut settled: Settled,
    keying: PendingKeying,
    peer_description: &SessionDescription,
    local_is_answerer: bool,
    profile: MediaProfile,
) -> Result<(MediaSession, Settled)> {
    #[cfg(not(feature = "dtls"))]
    // discard: these inputs select DTLS roles only; the feature-off build has no such branch.
    let _ = (peer_description, local_is_answerer, profile);
    #[cfg(feature = "dtls")]
    if profile == MediaProfile::BrowserAudio {
        let remote_role = if local_is_answerer {
            sipx_sdp::browser_audio::BrowserAudioRole::Offerer
        } else {
            sipx_sdp::browser_audio::BrowserAudioRole::Answerer
        };
        let remote = sipx_sdp::browser_audio::validate(peer_description, remote_role)?;
        if !local_is_answerer
            && remote.payloads
                != (sipx_sdp::browser_audio::BrowserAudioPayloads {
                    opus: 111,
                    pcmu: 0,
                    pcma: 8,
                    comfort_noise: 13,
                    telephone_event: 101,
                })
        {
            return Err(sipx_sdp::browser_audio::ProfileError::CodecSetIncomplete.into());
        }
        let local = ice.ok_or(sipx_sdp::browser_audio::ProfileError::IceRequired)?;
        let PendingKeying::Dtls(identity) = keying else {
            return Err(sipx_sdp::browser_audio::ProfileError::WeakerMedia.into());
        };
        let local_setup = dtls_local_setup(peer_description, local_is_answerer)?;
        let role = match local_setup {
            sipx_sdp::fingerprint::Setup::Active => sipx_media::dtls::Role::Client,
            sipx_sdp::fingerprint::Setup::Passive => sipx_media::dtls::Role::Server,
            _ => return Err(sipx_sdp::browser_audio::ProfileError::SetupRole.into()),
        };
        let media = port
            .start_browser_audio(
                settled.media_config(),
                local,
                0,
                identity,
                role,
                remote.fingerprint,
                Duration::from_secs(5),
            )
            .await
            .map_err(browser_start_error)?;
        return Ok((media, settled));
    }
    match keying {
        PendingKeying::Sdes => {}
        #[cfg(feature = "dtls")]
        PendingKeying::Dtls(identity) => {
            let audio = peer_description.media.first().ok_or(Error::NoCommonCodec)?;
            let fingerprint = audio
                .fingerprint()
                .or_else(|| peer_description.fingerprint())
                .ok_or_else(|| Error::Sdp("the DTLS peer supplied no fingerprint".to_owned()))?;
            let local_setup = dtls_local_setup(peer_description, local_is_answerer)?;
            let role = match local_setup {
                sipx_sdp::fingerprint::Setup::Active => sipx_media::dtls::Role::Client,
                sipx_sdp::fingerprint::Setup::Passive => sipx_media::dtls::Role::Server,
                _ => {
                    return Err(Error::Sdp(
                        "the DTLS exchange did not select active or passive".to_owned(),
                    ));
                }
            };
            let remote = settled.negotiated.remote;
            let (keyed, keys) = port
                .key_with_dtls(identity, remote, role, fingerprint, Duration::from_secs(5))
                .await
                .map_err(|error| Error::Dtls(error.to_string()))?;
            settled.srtp = Some(keys);
            let media = match ice {
                Some(local) => keyed.start_with_ice(settled.media_config(), local)?,
                None => keyed.start(settled.media_config())?,
            };
            return Ok((media, settled));
        }
    }
    let media = match ice {
        Some(local) => port.start_with_ice(settled.media_config(), local)?,
        None => port.start(settled.media_config())?,
    };
    Ok((media, settled))
}

#[cfg(feature = "dtls")]
fn browser_start_error(error: sipx_media::browser::BrowserStartError) -> Error {
    use sipx_media::browser::BrowserStartError;
    match error {
        BrowserStartError::IceFailed | BrowserStartError::IceStopped => {
            sipx_sdp::browser_audio::ProfileError::NoNominatedPair.into()
        }
        BrowserStartError::RtcpMuxRequired => {
            sipx_sdp::browser_audio::ProfileError::RtcpMuxRequired.into()
        }
        BrowserStartError::DtlsTimeout => sipx_sdp::browser_audio::ProfileError::DtlsTimeout.into(),
        BrowserStartError::Dtls(sipx_media::dtls::Error::FingerprintMismatch) => {
            sipx_sdp::browser_audio::ProfileError::FingerprintMismatch.into()
        }
        BrowserStartError::Dtls(sipx_media::dtls::Error::NoProfile) => {
            sipx_sdp::browser_audio::ProfileError::NoSrtpProfile.into()
        }
        BrowserStartError::Setup(error) => Error::Media(error),
        other => Error::Dtls(other.to_string()),
    }
}

/// Answer an incoming INVITE.
///
/// The 200 OK is retransmitted until the ACK arrives, which is the transaction user's job:
/// `sipx-sip`'s server transaction moves to `Accepted` and absorbs retransmissions of the
/// *request*, but it does not resend the response. Over UDP one lost 200 means the caller
/// gives up while this side holds an established call, so this is not optional.
///
/// Answers from the default codec set, [`Codecs::G711`]. [`answer_with`] takes a selection.
pub async fn answer(endpoint: &Handle, incoming: &Incoming, media_address: IpAddr) -> Result<Call> {
    answer_at(endpoint, incoming, MediaAddress::new(media_address)).await
}

/// [`answer`] with independent advertised and bound media addresses.
pub async fn answer_at(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: MediaAddress,
) -> Result<Call> {
    answer_tagged(
        endpoint,
        incoming,
        media_address,
        &token(),
        None,
        MediaPolicy::default(),
        &[],
    )
    .await
}

/// [`answer`], from a chosen codec set rather than the default one (`M-30`).
///
/// The answering counterpart of [`DialOptions::with_codecs`]. `codecs` bounds what the answer may
/// settle on, and no more than that: RFC 3264 §6.1 gives the *order* to the offerer, so a caller
/// offering G.711 first is answered G.711 first even from [`Codecs::Opus`]. What the selection
/// decides is whether Opus is on the table at all — an offer of it answered from
/// [`Codecs::G711`] is answered G.711, because a call must never settle on a codec the
/// application did not ask to carry.
pub async fn answer_with(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    codecs: Codecs,
) -> Result<Call> {
    answer_with_policy_at(
        endpoint,
        incoming,
        MediaAddress::new(media_address),
        MediaPolicy::default().with_codecs(codecs),
    )
    .await
}

/// Answer using one coherent codec, security and ICE policy.
pub async fn answer_with_policy(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    policy: MediaPolicy,
) -> Result<Call> {
    answer_with_policy_at(endpoint, incoming, MediaAddress::new(media_address), policy).await
}

/// [`answer_with_policy`] with independent advertised and bound media addresses.
pub async fn answer_with_policy_at(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: MediaAddress,
    policy: MediaPolicy,
) -> Result<Call> {
    answer_tagged(
        endpoint,
        incoming,
        media_address,
        &token(),
        None,
        policy,
        &[],
    )
    .await
}

/// Answer using one coherent media policy and validated application-owned response fields.
pub async fn answer_with_policy_and_headers(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    policy: MediaPolicy,
    headers: &[sipx_sip::Header],
) -> Result<Call> {
    answer_with_policy_and_headers_at(
        endpoint,
        incoming,
        MediaAddress::new(media_address),
        policy,
        headers,
    )
    .await
}

/// [`answer_with_policy_and_headers`] with independent advertised and bound media addresses.
pub async fn answer_with_policy_and_headers_at(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: MediaAddress,
    policy: MediaPolicy,
    headers: &[sipx_sip::Header],
) -> Result<Call> {
    answer_tagged(
        endpoint,
        incoming,
        media_address,
        &token(),
        None,
        policy,
        headers,
    )
    .await
}

/// The same, with the `To` tag chosen by the caller rather than freshly minted.
///
/// [`Invitation::answer`](crate::Invitation::answer) uses it so that every response this stack
/// sends about one invitation carries one tag — the `200` accepting it, and the `200` that
/// [`Dispatcher`](crate::Dispatcher) sends for a CANCEL that arrives too late to stop it. RFC 3261
/// §9.2 asks for exactly that agreement ("the `To` tag of the response to the CANCEL and the `To`
/// tag in the response to the original request SHOULD be the same"), and it can only be honoured
/// by whoever owns both, which is the invitation.
pub(crate) async fn answer_tagged(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: MediaAddress,
    tag: &str,
    claim: Option<Claim<'_>>,
    policy: MediaPolicy,
    headers: &[sipx_sip::Header],
) -> Result<Call> {
    let offer = match sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body())) {
        Ok(offer) => offer,
        Err(error) => {
            let error = Error::Sdp(error.to_string());
            refuse_initial_offer(endpoint, incoming, tag, claim, 400, "Bad Request").await?;
            return Err(error);
        }
    };
    // No provisional was sent on this path, so there is nothing to report as `Ringing`.
    answer_negotiated(
        endpoint,
        incoming,
        media_address,
        offer,
        tag,
        None,
        claim,
        policy,
        headers,
    )
    .await
}

/// The media an invitation has bound, and what the far end has said about it.
///
/// An enum rather than two `Option`s because exactly one is true at a time, and the difference
/// between them is the whole of RFC 3311 §5.1's precondition: a session that has been offered
/// and not answered may not be renegotiated, and one that has been answered may.
#[derive(Debug)]
enum EarlyMedia {
    /// Bound, and named in the INVITE's offer. The far end has not answered it yet.
    Offered(MediaPort),
    /// The INVITE carried no offer. A reliable provisional may supply one for this side to
    /// answer in PRACK (RFC 3262 section 5).
    WaitingForOffer,
    /// Answered in a reliable provisional (RFC 3262 §5), and renegotiable from here.
    Answered(Box<Early>),
}

/// An invitation this side has placed, which the far end has not yet answered.
///
/// The calling side's counterpart of [`Ringing`](crate::Ringing), and the reason it is a separate
/// entry point is that [`dial`] cannot be both. `dial` waits for the final response inside
/// itself, which is what almost every application wants and is why its signature is unchanged;
/// but an application that wants to do anything *while* the far end rings has to hold the early
/// dialog, and before this there was no moment at which it could.
///
/// What it holds is what the eventual [`Call`] will need: the INVITE's still-open response
/// stream, the media port bound before the offer named it, and — once a provisional creates one —
/// the dialog itself. [`Self::answered`] hands all three over rather than rebuilding them, which
/// matters most for the dialog: its sequence space already carries the PRACK and any UPDATE sent
/// while ringing, and a dialog built afresh from the 2xx would restart that space at the INVITE's
/// own number, putting the first BYE behind a request the far end has already seen (RFC 3261
/// §12.2.1.1).
///
/// **Nothing happens on its own.** A `Dialing` dropped without [`Self::answered`] or
/// [`Self::cancel`] leaves the far end ringing, exactly as a [`Call`] dropped without
/// [`Call::hang_up`] leaves the far end in a call. The discipline is the application's: making it
/// implicit would mean withdrawing an invitation from a destructor that cannot await the CANCEL
/// it sends, nor the `200` that may cross it.
#[derive(Debug)]
pub struct Dialing {
    endpoint: Handle,
    /// The INVITE itself. A CANCEL must repeat its identity, and a PRACK its sequence number.
    invite: Request,
    /// Where the INVITE was sent, and the fallback for in-dialog requests.
    target: Target,
    /// Where in-dialog requests go, once a `Contact` has said somewhere better.
    in_dialog: Target,
    /// The INVITE transaction, still open. `None` once [`Self::answered`] has handed it to
    /// `reack_retransmitted_2xx`, which is the only other thing entitled to read from it.
    responses: Option<sipx_transport::Responses>,
    /// The early dialog a provisional established (RFC 3261 §12.1.1).
    dialog: Option<Dialog>,
    /// Which reliable provisionals have been acknowledged (RFC 3262 §4).
    seen: sipx_sip::rel::Sequence,
    /// `None` only after [`Self::answered`] has handed the port to the [`Call`].
    media: Option<EarlyMedia>,
    /// A delayed offer prepared for the target leg but not `PRACKed` until the source answers it.
    coupled_prack: Option<CoupledPrack>,
    /// A final response that crossed the held PRACK; confirmed only after that PRACK leaves.
    coupled_final: Option<Box<Response>>,
    /// The ICE agent gathered for the same port, retained until an answer supplies its peer half.
    ice: Option<LocalDescription>,
    /// What the INVITE offered, kept because an SRTP answer has to be paired with the offer it
    /// answers and because a later UPDATE offers from the same starting point.
    capabilities: Option<Capabilities>,
    negotiation: update::Negotiation,
    peer_allows_update: bool,
    /// The direction the last UPDATE from this side set, carried into the [`Call`] so that an
    /// invitation put on hold before it was answered is answered on hold.
    hold: Direction,
    /// Whether anything past a bare `100 Trying` arrived, and whether it was reliable — the
    /// same thing [`Waited::Final`] carries, and for the same reason.
    ringing: Option<bool>,
    /// When to stop waiting, counted from when the INVITE went out rather than from each call
    /// to [`Self::answered`] — the far end is ringing against one deadline, not a fresh one per
    /// method call.
    deadline: Option<tokio::time::Instant>,
    /// Monotonic origin shared by the answer deadline and cancellation report.
    invitation_started: tokio::time::Instant,
    options: DialOptions,
    /// A final response that arrived before the application was handed anything.
    ///
    /// A far end that goes straight from the INVITE to a `200` never gives its caller an early
    /// dialog. That is not a failure — the call is perfectly good — so it is completed here and
    /// [`Self::answered`] hands it over at once. Completed rather than parked, because a `2xx`
    /// held while an application decides what to do with a handle is a `2xx` the far end is
    /// retransmitting (RFC 3261 §13.2.2.4).
    answered_already: Option<Box<Call>>,
    /// The event stream begins with this app-visible attempt, before a `Call` exists.
    events: Option<EventSink>,
    /// Handed out once by [`Self::events`], or moved into the confirmed [`Call`].
    events_rx: Option<CallEvents>,
}

/// What one read from the INVITE transaction produced.
enum Arrived {
    /// A provisional response.
    Provisional(Box<Response>),
    /// A final response.
    Final(Box<Response>),
    /// The deadline passed.
    GaveUp,
    /// The transaction ended without a final response.
    Gone,
}

enum BeforeDeadline<T> {
    Event(T),
    Expired,
}

/// Observe an event only while the answer budget remains.
///
/// Deadline first is the public exact-boundary rule: an event must be observed *before*, not at,
/// the deadline to change the call outcome.
async fn before_deadline<T>(
    deadline: tokio::time::Instant,
    event: impl Future<Output = T>,
) -> BeforeDeadline<T> {
    tokio::pin!(event);
    tokio::select! {
        biased;
        () = tokio::time::sleep_until(deadline) => BeforeDeadline::Expired,
        event = event.as_mut() => BeforeDeadline::Event(event),
    }
}

/// One early-dial event surfaced to the owning two-dialog coupling.
pub(crate) enum CouplingDialEvent {
    /// A provisional was consumed and any required PRACK was sent.
    Progress,
    /// A reliable provisional carried an offer whose PRACK is held for the source leg's answer.
    ReliableOffer(Direction),
    /// The outbound invitation confirmed.
    Answered(Box<Call>),
    /// One routed in-dialog request arrived, or that route closed.
    Incoming(Box<Option<Incoming>>),
}

#[derive(Debug)]
struct CoupledPrack {
    response: Box<Response>,
    rseq: u32,
    answer: SessionDescription,
}

/// Place a call and get the early dialog, rather than waiting for the call itself.
///
/// [`dial`] and [`dial_once`] wait for the final response and hand back a [`Call`]; this hands
/// back a [`Dialing`] as soon as the far end has established a dialog, so the application can act
/// while it rings — renegotiate the session with an UPDATE (RFC 3311 §5.1), answer one, or read
/// the description a provisional carried. [`Dialing::answered`] then waits for the call exactly
/// as `dial` would have.
///
/// It returns as soon as *a dialog* exists, which is not the same as an answered session: a far
/// end that rings `180` with no body has established a dialog and described nothing.
/// [`Dialing::has_early_session`] is what distinguishes them, and it is what
/// [`Dialing::update`] requires.
///
/// Unlike [`dial`] there is no retry on a `422`. The retry is a *second* INVITE, and the handle
/// an application would be holding names the first; [`Error::IntervalTooBrief`] comes back from
/// [`Dialing::answered`] instead, as it does from [`dial_once`].
///
/// # Errors
///
/// Fails if the INVITE cannot be built or sent, if the deadline passes before any dialog is
/// established — in which case the invitation is withdrawn first, so the far end stops ringing —
/// or if the transaction ends with no response at all.
///
/// It also fails if a reliable provisional answers our offer with a description that cannot be
/// used: [`Error::Sdp`] for a body that does not parse or an `a=crypto` that fails RFC 4568
/// §5.1.3's check, [`Error::NoCommonCodec`] for one that cannot be negotiated. The invitation is
/// withdrawn with a CANCEL first (RFC 3261 §9.1), so no handle to a dead invitation comes back. A
/// provisional carrying *no* description is not this case and is not an error.
pub async fn dial_early(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Dialing> {
    let mut dialing = begin_dial_early(endpoint, target, to, options).await?;
    dialing.reach_early_dialog().await?;
    Ok(dialing)
}

/// Place a call until an early dialog exists or `cancelled` resolves.
///
/// This is the cancellation-safe early-dialog counterpart of [`dial_until`]. If cancellation
/// wins after the INVITE has left, the invitation is withdrawn before this returns, including
/// the ACK-then-BYE cleanup when a successful final response crossed the cancellation. A caller
/// that continues waiting for confirmation can use [`Dialing::answered_until`] with the same
/// cancellation signal.
///
/// # Errors
///
/// The same setup and early-dialog errors as [`dial_early`]. Local cancellation returns
/// [`Error::Cancelled`] after the outstanding invitation has been cleaned up.
pub async fn dial_early_until<F>(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
    cancelled: F,
) -> Result<Dialing>
where
    F: Future<Output = ()> + Send,
{
    let mut dialing = begin_dial_early(endpoint, target, to, options).await?;
    tokio::pin!(cancelled);
    tokio::select! {
        biased;
        () = cancelled.as_mut() => {
            let cancellation = dialing.local_cancellation(false).await;
            Err(Error::Cancelled(cancellation))
        }
        result = dialing.reach_early_dialog() => {
            result?;
            Ok(dialing)
        }
    }
}

async fn begin_dial_early(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Dialing> {
    if options.media.keying == Keying::DtlsSrtp {
        return Err(Error::DtlsEarlyMedia);
    }
    let (port, capabilities, ice, _keying, invite) =
        open_invitation(endpoint, &target, to, options, &Identity::fresh(), None).await?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;
    let invitation_started = Instant::now();

    let (events, events_rx) = EventSink::new();
    Ok(Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        target,
        responses: Some(responses),
        dialog: None,
        seen: sipx_sip::rel::Sequence::default(),
        media: Some(EarlyMedia::Offered(port)),
        coupled_prack: None,
        coupled_final: None,
        ice,
        capabilities: Some(capabilities),
        // RFC 3264: the INVITE carried our offer, so an exchange is open until the far end
        // answers it — which before the 200 can only happen in a reliable provisional.
        negotiation: update::Negotiation::offering(),
        peer_allows_update: false,
        hold: Direction::SendRecv,
        ringing: None,
        deadline: options.timeout.map(|limit| invitation_started + limit),
        invitation_started,
        options: options.clone(),
        answered_already: None,
        events: Some(events),
        events_rx: Some(events_rx),
    })
}

/// Place an offerless INVITE and answer an offer from a reliable provisional in PRACK.
///
/// This is RFC 3262 section 5's delayed-offer shape. An SDP-bearing reliable provisional is not
/// merely observed: it is answered on the same dialog sequence and the resulting media is retained
/// through confirmation. Like [`dial_early`], this function can return for a bodiless provisional;
/// [`Dialing::has_early_session`] distinguishes that case from a negotiated early session.
///
/// # Errors
///
/// The same transport, timeout and final-response errors as [`dial_early`], plus the SDP or media
/// error produced while answering the provisional offer. DTLS-SRTP is refused because its active
/// handshake cannot be started safely before the final response on this path.
pub async fn dial_early_without_offer(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Dialing> {
    if options.media.keying == Keying::DtlsSrtp {
        return Err(Error::DtlsEarlyMedia);
    }
    let identity = Identity::fresh();
    let invite = open_offerless_invitation(endpoint, &target, to, options, &identity)?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;
    let invitation_started = Instant::now();
    let (events, events_rx) = EventSink::new();
    let mut dialing = Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        target,
        responses: Some(responses),
        dialog: None,
        seen: sipx_sip::rel::Sequence::default(),
        media: Some(EarlyMedia::WaitingForOffer),
        coupled_prack: None,
        coupled_final: None,
        ice: None,
        capabilities: None,
        negotiation: update::Negotiation::idle(),
        peer_allows_update: false,
        hold: Direction::SendRecv,
        ringing: None,
        deadline: options.timeout.map(|limit| invitation_started + limit),
        invitation_started,
        options: options.clone(),
        answered_already: None,
        events: Some(events),
        events_rx: Some(events_rx),
    };
    dialing.reach_early_dialog().await?;
    Ok(dialing)
}

pub(crate) async fn dial_early_without_offer_for_coupling(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<(Dialing, Option<Direction>)> {
    if options.media.keying == Keying::DtlsSrtp {
        return Err(Error::DtlsEarlyMedia);
    }
    let identity = Identity::fresh();
    let invite = open_offerless_invitation(endpoint, &target, to, options, &identity)?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;
    let invitation_started = Instant::now();
    let (events, events_rx) = EventSink::new();
    let mut dialing = Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        target,
        responses: Some(responses),
        dialog: None,
        seen: sipx_sip::rel::Sequence::default(),
        media: Some(EarlyMedia::WaitingForOffer),
        coupled_prack: None,
        coupled_final: None,
        ice: None,
        capabilities: None,
        negotiation: update::Negotiation::idle(),
        peer_allows_update: false,
        hold: Direction::SendRecv,
        ringing: None,
        deadline: options.timeout.map(|limit| invitation_started + limit),
        invitation_started,
        options: options.clone(),
        answered_already: None,
        events: Some(events),
        events_rx: Some(events_rx),
    };
    let direction = dialing.reach_early_dialog_for_coupling().await?;
    Ok((dialing, direction))
}

impl Dialing {
    /// The early dialog, once a provisional has established one (RFC 3261 §12.1.1).
    ///
    /// Exposed read-only because `C-2` will want to know *which* dialog a provisional's media
    /// belongs to — with forking, one invitation can produce several — without this handle
    /// having to guess in advance what it will be asked.
    #[must_use]
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog
            .as_ref()
            .or_else(|| self.answered_already.as_ref().map(|call| &call.dialog))
    }

    /// Whether the far end has answered this invitation's offer, in a reliable provisional.
    ///
    /// The precondition for [`Self::update`], and worth reading as the question RFC 3311 §5.1
    /// actually asks: not "is there a dialog" but "is there an offer/answer exchange still
    /// open". A `180` with no body establishes the first and does nothing about the second.
    #[must_use]
    pub fn has_early_session(&self) -> bool {
        matches!(self.media, Some(EarlyMedia::Answered(_)))
    }

    /// The running early-media session, once a reliable provisional answered the INVITE offer.
    ///
    /// `None` for a bodiless provisional and before an answer arrives. When this is `Some`, the
    /// same session is moved into the [`Call`] returned by [`Self::answered`].
    #[must_use]
    pub fn media(&self) -> Option<&MediaSession> {
        match self.media.as_ref() {
            Some(EarlyMedia::Answered(early)) => Some(&early.media),
            _ => self.answered_already.as_ref().map(|call| call.media()),
        }
    }

    /// This attempt's event stream, continuing on the confirmed call.
    ///
    /// Handed out once. A reliable provisional that starts media queues
    /// [`CallEvent::EarlyMediaStarted`] before this method can return it; the same receiver later
    /// observes [`CallEvent::Answered`] without being replaced at confirmation.
    pub fn events(&mut self) -> Option<CallEvents> {
        self.events_rx.take().or_else(|| {
            self.answered_already
                .as_mut()
                .and_then(|call| call.events())
        })
    }

    /// Drive this invitation until early media starts or a final response arrives.
    ///
    /// [`dial_early`] returns on the first early dialog, which may be a bodiless `180`; a later
    /// reliable `183` can still answer the offer. This method keeps the handle in the
    /// application's ownership while reading through those later provisionals. `true` means
    /// [`Self::media`] is now available and [`CallEvent::EarlyMediaStarted`] has been emitted.
    /// `false` means the invitation reached a final response first; [`Self::answered`] then hands
    /// back the already-completed call (or its final error).
    ///
    /// # Errors
    ///
    /// The same provisional, final-refusal, timeout, cancellation, and transaction errors as
    /// [`Self::answered`]. `false` reports a successful final response with no early-media phase;
    /// the already-completed call is retained for [`Self::answered`].
    pub async fn wait_for_early_media(&mut self) -> Result<bool> {
        if self.has_early_session() {
            return Ok(true);
        }
        if self.answered_already.is_some() {
            return Ok(false);
        }
        loop {
            match self.next_response().await {
                Arrived::Provisional(response) => {
                    if let Err(error) = self.observe(&response).await {
                        return Err(self.abandon(error).await);
                    }
                    if self.has_early_session() {
                        return Ok(true);
                    }
                }
                Arrived::Final(response) => {
                    let call = self.confirm(*response).await?;
                    self.answered_already = Some(Box::new(call));
                    return Ok(false);
                }
                Arrived::GaveUp => {
                    let cancellation = self.local_cancellation(true).await;
                    return Err(Error::Cancelled(cancellation));
                }
                Arrived::Gone => return Err(Error::NoResponse),
            }
        }
    }

    /// Whether the far end has said it accepts UPDATE (RFC 3311 §4).
    ///
    /// Advisory, not enforced: §4 says a UAS "SHOULD" list it, and refusing to send on a peer
    /// that merely omitted the header would fail calls that would have worked. Worth checking
    /// before [`Self::update`] if a `405` would be more expensive than not trying.
    #[must_use]
    pub fn peer_allows_update(&self) -> bool {
        self.peer_allows_update
    }

    /// Renegotiate the early session from this side (RFC 3311 §5.1).
    ///
    /// One implementation of §5.1, shared with [`Ringing::update`](crate::Ringing::update): the
    /// RFC makes UPDATE something either end may send, so there is one body of rules and two
    /// callers rather than a copy per role.
    ///
    /// # Errors
    ///
    /// [`Error::NoEarlySession`] if the far end has not answered our offer yet
    /// ([`Self::has_early_session`]); [`Error::NoDialog`] if no provisional established one; and
    /// [`Error::Rejected`] if the far end refuses, including the `491` of an offer that crossed
    /// one of ours.
    pub async fn update(&mut self, direction: Direction) -> Result<()> {
        let Some(early) = self.early_dialog() else {
            return Err(Error::NoDialog);
        };
        crate::update::offer(early, direction).await?;
        self.hold = direction;
        Ok(())
    }

    /// Answer an UPDATE that arrived in this early dialog (RFC 3311 §5.2).
    ///
    /// Returns whether it was one for this dialog, so an application with one inbox can offer
    /// everything it receives and act on what is left. The refusals are the same three the
    /// answering side gives, because they are the same code.
    ///
    /// # Errors
    ///
    /// Fails only if the response could not be built or sent. A *refusal* is a successful call:
    /// §5.2's 488 and 500 are responses this stack sends deliberately, not errors here.
    pub async fn on_update(&mut self, incoming: &Incoming) -> Result<bool> {
        let Some(early) = self.early_dialog() else {
            return Ok(false);
        };
        crate::update::receive(early, incoming).await
    }

    /// Advance either the INVITE transaction or its routed early-dialog inbox once.
    ///
    /// Kept crate-private for [`crate::coupling::EarlyCoupling`]: unlike [`Self::answered`], this
    /// does not consume the dialing handle or hold it across every provisional. The coupling can
    /// therefore service UPDATEs from either pending leg and observe cancellation while the
    /// outbound final response is still outstanding.
    pub(crate) async fn coupling_step(
        &mut self,
        incoming: &mut tokio::sync::mpsc::Receiver<Incoming>,
    ) -> Result<CouplingDialEvent> {
        if let Some(call) = self.answered_already.take() {
            return Ok(CouplingDialEvent::Answered(call));
        }
        if self.coupled_prack.is_none()
            && let Some(response) = self.coupled_final.take()
        {
            return self
                .confirm(*response)
                .await
                .map(Box::new)
                .map(CouplingDialEvent::Answered);
        }
        tokio::select! {
            request = incoming.recv() => Ok(CouplingDialEvent::Incoming(Box::new(request))),
            arrived = self.next_response() => match arrived {
                Arrived::Provisional(response) => {
                    if self.is_coupled_offer_candidate(&response) {
                        return match self.stage_coupled_offer(*response).await {
                            Ok(direction) => Ok(CouplingDialEvent::ReliableOffer(direction)),
                            Err(error) => Err(self.abandon(error).await),
                        };
                    }
                    if let Err(error) = self.observe(&response).await {
                        return Err(self.abandon(error).await);
                    }
                    Ok(CouplingDialEvent::Progress)
                }
                Arrived::Final(response) => {
                    if self.coupled_prack.is_some() {
                        self.coupled_final = Some(response);
                        Ok(CouplingDialEvent::Progress)
                    } else {
                        self.confirm(*response)
                            .await
                            .map(Box::new)
                            .map(CouplingDialEvent::Answered)
                    }
                },
                Arrived::GaveUp => {
                    let cancellation = self.local_cancellation(true).await;
                    Err(Error::Cancelled(cancellation))
                }
                Arrived::Gone => Err(Error::NoResponse),
            }
        }
    }

    pub(crate) async fn complete_coupled_prack(&mut self) -> Result<()> {
        let Some(pending) = self.coupled_prack.take() else {
            return Err(Error::NoDialog);
        };
        self.acknowledge(&pending.response, pending.rseq, Some(pending.answer))
            .await
    }

    /// Wait for the invitation to be answered, and take the call it becomes.
    ///
    /// Consuming, because everything it needs moves into the [`Call`]. Provisionals that arrive
    /// while waiting are handled exactly as they were before it returned — `PRACK`ed, and read for
    /// the answer that makes the session renegotiable — so an application that calls this
    /// immediately is in the same position as one that had called [`dial`].
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] if the far end declined, [`Error::IntervalTooBrief`] for a `422`
    /// (see [`dial_early`] on why it is not retried), [`Error::Cancelled`] if the deadline
    /// passed — the invitation is withdrawn first — and [`Error::NoResponse`] if the
    /// transaction ended without a final response.
    ///
    /// And, from a *provisional* rather than from the answer: [`Error::Sdp`] or
    /// [`Error::NoCommonCodec`] if a reliable provisional answers our offer with a description
    /// that cannot be used (RFC 3262 §5). That one withdraws the invitation with a CANCEL (RFC
    /// 3261 §9.1) rather than waiting for a 2xx to fail on, because a far end that answered no
    /// offer of ours may never send one.
    pub async fn answered(mut self) -> Result<Call> {
        self.drive_answered(None).await
    }

    /// Wait for confirmation until `cancelled` resolves.
    ///
    /// Cancellation withdraws the owned invitation before returning, including ACK-then-BYE
    /// cleanup for a successful final response already in flight. This closes the ownership gap
    /// between [`dial_early_until`] returning an early handle and the final answer arriving.
    ///
    /// # Errors
    ///
    /// The same errors as [`Self::answered`]. Local cancellation returns [`Error::Cancelled`]
    /// after cleanup completes.
    pub async fn answered_until<F>(mut self, cancelled: F) -> Result<Call>
    where
        F: Future<Output = ()> + Send,
    {
        tokio::pin!(cancelled);
        let cancelled: Pin<&mut (dyn Future<Output = ()> + Send)> = cancelled.as_mut();
        self.drive_answered(Some(cancelled)).await
    }

    async fn drive_answered(&mut self, mut cancelled: Option<Cancelled<'_>>) -> Result<Call> {
        if let Some(call) = self.answered_already.take() {
            return Ok(*call);
        }
        loop {
            let arrived = match cancelled.as_mut() {
                None => self.next_response().await,
                Some(cancelled) => {
                    tokio::select! {
                        biased;
                        () = cancelled.as_mut() => {
                            let cancellation = self.local_cancellation(false).await;
                            return Err(Error::Cancelled(cancellation));
                        }
                        arrived = self.next_response() => arrived,
                    }
                }
            };
            match arrived {
                Arrived::Provisional(response) => {
                    if let Err(error) = self.observe(&response).await {
                        return Err(self.abandon(error).await);
                    }
                }
                Arrived::Final(response) => return self.confirm(*response).await,
                Arrived::GaveUp => {
                    let cancellation = self.local_cancellation(true).await;
                    return Err(Error::Cancelled(cancellation));
                }
                Arrived::Gone => return Err(Error::NoResponse),
            }
        }
    }

    /// Give up on the invitation, and make sure the far end stops ringing (RFC 3261 §9.1, §15).
    ///
    /// The counterpart of [`Self::answered`], and the reason both consume the handle. A `200`
    /// that crosses the CANCEL is acknowledged and then hung up, which §15 requires and a CANCEL
    /// cannot do on its own.
    pub async fn cancel(mut self) {
        let _cleanup = self.give_up().await;
    }

    /// Cancel this invitation and return the measured ownership handoff.
    ///
    /// This is the observable form used by command supervisors. [`Self::cancel`] retains the
    /// compatibility shape for callers that need only the protocol side effect.
    pub async fn cancel_observed(mut self) -> InvitationCancellation {
        self.local_cancellation(false).await
    }

    /// Cancel this invitation with an explicit protocol cause.
    ///
    /// A SIP 200 reason represents the RFC 3326 §3.1 case where another coupled or forked leg
    /// completed the call; other valid SIP and Q.850 causes are retained unchanged.
    pub async fn cancel_with_reason(mut self, reason: ReasonValue) {
        let _cleanup = self.give_up_with_reason(&reason).await;
    }

    /// The early dialog's mutable parts, borrowed for one UPDATE.
    ///
    /// `None` before any provisional has established a dialog, which is a peer there is nothing
    /// to send an in-dialog request *to*.
    fn early_dialog(&mut self) -> Option<crate::update::EarlyDialog<'_>> {
        Some(crate::update::EarlyDialog {
            endpoint: &self.endpoint,
            dialog: self.dialog.as_mut()?,
            target: &mut self.in_dialog,
            negotiation: &mut self.negotiation,
            peer_allows: &mut self.peer_allows_update,
            early: match self.media.as_mut() {
                Some(EarlyMedia::Answered(early)) => Some(early),
                _ => None,
            },
        })
    }

    /// Read responses until a dialog exists, or the invitation is over before one did.
    async fn reach_early_dialog(&mut self) -> Result<()> {
        loop {
            match self.next_response().await {
                Arrived::Provisional(response) => {
                    if let Err(error) = self.observe(&response).await {
                        return Err(self.abandon(error).await);
                    }
                    if self.dialog.is_some() {
                        return Ok(());
                    }
                }
                Arrived::Final(response) => {
                    let call = self.confirm(*response).await?;
                    self.answered_already = Some(Box::new(call));
                    return Ok(());
                }
                Arrived::GaveUp => {
                    let cancellation = self.local_cancellation(true).await;
                    return Err(Error::Cancelled(cancellation));
                }
                Arrived::Gone => return Err(Error::NoResponse),
            }
        }
    }

    async fn reach_early_dialog_for_coupling(&mut self) -> Result<Option<Direction>> {
        loop {
            match self.next_response().await {
                Arrived::Provisional(response) => {
                    if self.is_coupled_offer_candidate(&response) {
                        return match self.stage_coupled_offer(*response).await {
                            Ok(direction) => Ok(Some(direction)),
                            Err(error) => Err(self.abandon(error).await),
                        };
                    }
                    if let Err(error) = self.observe(&response).await {
                        return Err(self.abandon(error).await);
                    }
                    if self.dialog.is_some() {
                        return Ok(None);
                    }
                }
                Arrived::Final(response) => {
                    let call = self.confirm(*response).await?;
                    self.answered_already = Some(Box::new(call));
                    return Ok(None);
                }
                Arrived::GaveUp => {
                    let cancellation = self.local_cancellation(true).await;
                    return Err(Error::Cancelled(cancellation));
                }
                Arrived::Gone => return Err(Error::NoResponse),
            }
        }
    }

    /// One response from the INVITE transaction, bounded by the invitation's own deadline.
    async fn next_response(&mut self) -> Arrived {
        let deadline = self.deadline;
        let Some(responses) = self.responses.as_mut() else {
            return Arrived::Gone;
        };
        loop {
            let event = match deadline {
                None => responses.next().await,
                Some(deadline) => match before_deadline(deadline, responses.next()).await {
                    BeforeDeadline::Event(event) => event,
                    BeforeDeadline::Expired => return Arrived::GaveUp,
                },
            };
            match event {
                Some(sipx_sip::transaction::TuEvent::Response(response)) => {
                    return if response.status.is_final() {
                        Arrived::Final(response)
                    } else {
                        Arrived::Provisional(response)
                    };
                }
                Some(_) => {}
                None => return Arrived::Gone,
            }
        }
    }

    /// Fold a provisional into the early dialog: the dialog it may create, the answer it may
    /// carry, and the PRACK it may require.
    ///
    /// # Errors
    ///
    /// Whatever [`Self::adopt_early_answer`] refused. It is the only fatal thing a provisional can
    /// do: everything else here is either optional (a dialog it did not establish, an `Allow` it
    /// did not carry) or recoverable (a PRACK that did not get through).
    async fn observe(&mut self, response: &Response) -> Result<()> {
        if !self.observe_metadata(response) {
            return Ok(());
        }
        let reliable = crate::rel::reliable_sequence(response);

        if let Some(rseq) = reliable {
            // RFC 3262 §5: an answer may only travel in a reliable provisional, so this is the
            // only place before the 200 where our INVITE's offer can be closed out. An
            // unreliable provisional carrying a description is not one — §5 forbids it, and one
            // lost leaves the two sides disagreeing about what is in force with no way to
            // notice — so it is ignored rather than adopted.
            //
            // Held rather than propagated on the spot: the provisional is acknowledged first even
            // when its description is refused. RFC 3262 §4 makes the PRACK a MUST for every
            // reliable provisional a UAC receives, and the far end retransmits until one arrives;
            // failing a beat earlier would leave it resending a response into a CANCEL that has
            // already gone.
            let adopted = if matches!(self.media, Some(EarlyMedia::WaitingForOffer))
                && !response.body().is_empty()
            {
                self.adopt_early_offer(response).await.map(Some)
            } else {
                self.adopt_early_answer(response).map(|()| None)
            };
            if let Some(dialog) = self.dialog.as_mut() {
                dialog.refresh_target(&response.headers);
            }
            // A failure is logged rather than fatal, for `await_final`'s reason: the invitation
            // is still running, and abandoning a ringing call because one PRACK did not get
            // through is a worse outcome than the unreliability it was fixing.
            let prack_answer = adopted.as_ref().ok().and_then(Clone::clone);
            if let Err(error) = self.acknowledge(response, rseq, prack_answer).await {
                tracing::debug!(%error, "could not acknowledge a reliable provisional");
            }
            adopted?;
        }
        Ok(())
    }

    fn is_coupled_offer_candidate(&self, response: &Response) -> bool {
        matches!(self.media, Some(EarlyMedia::WaitingForOffer))
            && !response.body().is_empty()
            && crate::rel::reliable_sequence(response).is_some()
    }

    async fn stage_coupled_offer(&mut self, response: Response) -> Result<Direction> {
        if !self.observe_metadata(&response) {
            return Err(Error::NoDialog);
        }
        let rseq = crate::rel::reliable_sequence(&response).ok_or(Error::NoDialog)?;
        let offer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        let direction = offer
            .media
            .iter()
            .find(|media| media.media == "audio" && !media.is_rejected())
            .map(sipx_sdp::MediaDescription::direction)
            .ok_or_else(|| {
                Error::Sdp("the reliable provisional carried no usable audio offer".to_owned())
            })?;
        let answer = self.adopt_early_offer(&response).await?;
        if let Some(dialog) = self.dialog.as_mut() {
            dialog.refresh_target(&response.headers);
        }
        self.coupled_prack = Some(CoupledPrack {
            response: Box::new(response),
            rseq,
            answer,
        });
        Ok(direction)
    }

    fn observe_metadata(&mut self, response: &Response) -> bool {
        const TRYING: u16 = 100;

        let reliable = crate::rel::reliable_sequence(response);
        if response.status.code() > TRYING && self.ringing.is_none() {
            let is_reliable = reliable.is_some();
            self.ringing = Some(is_reliable);
            if let Some(events) = self.events.as_ref() {
                events.emit(CallEvent::Ringing {
                    reliable: is_reliable,
                });
            }
        }

        if self.dialog.is_none() {
            if let Some(dialog) = Dialog::from_response(&self.invite, response) {
                self.in_dialog = in_dialog_target(&dialog, self.target.clone());
                self.dialog = Some(dialog);
            }
        } else if !self.belongs(response) {
            return false;
        }

        if update::peer_allows(&response.headers) {
            self.peer_allows_update = true;
        }
        true
    }

    /// Answer a reliable provisional's offer for an offerless INVITE.
    async fn adopt_early_offer(&mut self, response: &Response) -> Result<SessionDescription> {
        if response.body().is_empty() {
            return Err(Error::Sdp(
                "an offerless INVITE received a reliable provisional with no offer".to_owned(),
            ));
        }
        let offer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        let (early, answer) = Early::settle(
            MediaAddress::new(self.options.media_address)
                .with_bind(self.options.media_bind_address),
            self.target.transport.is_secure(),
            &offer,
            self.options.media,
        )
        .await?;
        self.media = Some(EarlyMedia::Answered(Box::new(early)));
        self.negotiation.sent_answer();
        if let Some(events) = self.events.as_ref() {
            events.emit(CallEvent::EarlyMediaStarted);
        }
        Ok(answer)
    }

    /// Whether a response belongs to the dialog this handle holds.
    fn belongs(&self, response: &Response) -> bool {
        self.dialog.as_ref().is_none_or(|dialog| {
            Dialog::from_response(&self.invite, response)
                .is_none_or(|fresh| fresh.id.remote_tag == dialog.id.remote_tag)
        })
    }

    /// Take the answer to our INVITE's offer out of a reliable provisional (RFC 3262 §5).
    ///
    /// This is what makes the early dialog renegotiable at all, and it is the calling side's
    /// mirror of [`ring_early`](crate::ring_early).
    ///
    /// `Ok(())` covers two shapes, and the difference matters. A provisional that carries **no
    /// description** is ordinary — a `180` establishes a dialog and answers nothing — and one that
    /// arrives after the exchange has already closed is a repeat of an answer we took the first
    /// time. Neither is a failure and neither is reported.
    ///
    /// # Errors
    ///
    /// A description that *is* there and cannot be used: [`Error::Sdp`] for a body that does not
    /// parse or an `a=crypto` that fails RFC 4568 §5.1.3's check, [`Error::NoCommonCodec`] for one
    /// that cannot be negotiated. Before `S-25` all three returned `()` and left a `debug` line,
    /// so they were indistinguishable from each other and from the silent cases above — and for a
    /// caller that never receives a 2xx, indistinguishable from nothing having happened.
    ///
    /// The session is left where it was on either path — still `Offered`, so the exchange stays
    /// open and [`Self::update`] keeps refusing, which is the truthful state. What changes is that
    /// the refusal now reaches [`Self::observe`], which withdraws the invitation over it.
    fn adopt_early_answer(&mut self, response: &Response) -> Result<()> {
        if !matches!(self.media, Some(EarlyMedia::Offered(_))) || response.body().is_empty() {
            return Ok(());
        }
        // Parsed and settled *before* the port is moved out, so that a failure on either step
        // leaves `media` exactly as it was rather than emptied.
        let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        // The same vocabulary the 2xx path uses: `settle_from` runs this exact function on the
        // final response, and a refusal that arrived early is the same refusal. Naming it
        // differently here would ask an application to match on two errors for one fault.
        //
        // `M-30` added the selected codec set to this call. It widens what can be refused here:
        // an early answer naming a codec outside the set now fails where it previously could
        // not, and `S-25` turns that failure into a CANCEL. Our own offer only names codecs in
        // the set, so a conformant answer cannot trip it — an answer that does is naming
        // something we never offered, which is exactly what `S-25` exists to refuse rather than
        // hang on.
        let Some(capabilities) = self.capabilities.clone() else {
            return Err(Error::NoDialog);
        };
        let settled = settle_answer(&capabilities, &answer, self.options.media.codecs)?;
        self.accept_remote_ice(&answer);
        let Some(EarlyMedia::Offered(port)) = self.media.take() else {
            return Ok(());
        };
        let media = match self.ice.take() {
            Some(local) => port.start_with_ice(settled.media_config(), local)?,
            None => port.start(settled.media_config())?,
        };
        self.media = Some(EarlyMedia::Answered(Box::new(Early {
            media,
            capabilities,
            settled,
            media_address: self.options.media_address,
            media_bind_address: self.options.media_bind_address,
            codecs: self.options.media.codecs,
            keying: self.options.media.keying,
        })));
        self.negotiation.received_answer();
        if let Some(events) = self.events.as_ref() {
            events.emit(CallEvent::EarlyMediaStarted);
        }
        Ok(())
    }

    /// Withdraw the invitation because a reliable provisional's description cannot be used.
    ///
    /// **CANCEL, not ACK-then-BYE** — this is the failure mode `S-25` had to choose, and it is
    /// chosen by where we are rather than by what went wrong. RFC 3261 §9.1 withdraws an
    /// invitation that has only been answered provisionally; there is no final response here to
    /// acknowledge, so the ACK-then-BYE that [`Self::confirm`] performs after a 2xx (§15) has
    /// nothing to attach itself to. [`Self::give_up`] is already that request, sent through
    /// [`withdraw`], which also covers the one case a CANCEL cannot: a `200` that crossed it is
    /// acknowledged and hung up, because by then §15 *does* apply.
    ///
    /// The alternative considered and rejected was to carry on and let the 2xx fail — which is
    /// what happened before this story. It fails safely (nothing is keyed on a refused answer,
    /// and `settle_from` re-runs the same check) but it is not a report: a far end that answers
    /// no offer of ours will not send a 2xx to fail on, and the caller's only outcome was
    /// [`Error::Cancelled`] when its own deadline passed.
    async fn abandon(&mut self, error: Error) -> Error {
        let _cleanup = self.give_up().await;
        error
    }

    /// PRACK a reliable provisional through the early dialog itself (RFC 3262 §4).
    ///
    /// Through *the* dialog, not a copy of it. The PRACK is an in-dialog request and takes the
    /// next number in this side's own sequence space (RFC 3261 §12.2.1.1); the `dial` path
    /// builds a throwaway `Dialog` per acknowledgement because it keeps none, which restarts
    /// that space at the INVITE's number every time. Here an UPDATE may follow, and it would
    /// then reuse the PRACK's number.
    async fn acknowledge(
        &mut self,
        response: &Response,
        rseq: u32,
        answer: Option<SessionDescription>,
    ) -> Result<()> {
        // §4: out of order means an earlier one is missing, and a duplicate has already been
        // acknowledged. Neither is PRACKed.
        if self.seen.accept(rseq) != sipx_sip::rel::Received::Acknowledge {
            return Ok(());
        }
        let dialog = self.dialog.as_mut().ok_or(Error::NoDialog)?;
        let invite_cseq = self
            .invite
            .headers
            .typed::<sipx_sip::CSeq>()
            .and_then(std::result::Result::ok)
            .map_or(1, |cseq| cseq.sequence);
        let body = match answer {
            Some(answer) => Some(answer),
            None => self.capabilities.as_ref().and_then(|capabilities| {
                crate::rel::prack_body(
                    !self.invite.body().is_empty(),
                    response.body(),
                    capabilities,
                )
            }),
        };
        crate::rel::send_prack(
            &self.endpoint,
            dialog,
            &self.in_dialog,
            rseq,
            invite_cseq,
            body,
        )
        .await
    }

    /// Turn a final response into a [`Call`], or into the error it describes.
    async fn confirm(&mut self, response: Response) -> Result<Call> {
        if !response.status.is_success() {
            // A non-2xx is acknowledged by the transaction layer itself, so there is nothing to
            // send here — only a media port to release, which happens when this is dropped.
            return Err(rejection(&response));
        }

        // From here the far end believes a dialog exists, so *every* path must acknowledge.
        // Returning an error without one leaves it retransmitting its 200 for 32 seconds and
        // then streaming media at a port we have closed.
        match self.accept(&response) {
            Ok((dialog, media, settled)) => {
                let ack = build_ack(&self.endpoint, &dialog, &self.in_dialog)?;
                self.endpoint
                    .send_directly(ack.clone(), self.in_dialog.clone())
                    .await?;
                // The stream stays open rather than being dropped: a retransmitted 2xx means
                // this ACK was lost and RFC 3261 §13.2.2.4 requires another.
                if let Some(responses) = self.responses.take() {
                    tokio::spawn(reack_retransmitted_2xx(
                        self.endpoint.clone(),
                        responses,
                        ack,
                        self.in_dialog.clone(),
                    ));
                }
                let Some(events) = self.events.take() else {
                    return Err(Error::NoDialog);
                };
                events.emit(CallEvent::Answered);
                let events_rx = self.events_rx.take();
                Ok(Call {
                    dialog,
                    initial_status: response.status.code(),
                    media: Arc::new(media),
                    retired_media: Vec::new(),
                    endpoint: self.endpoint.clone(),
                    target: self.in_dialog.clone(),
                    ack_stop: None,
                    ack_retransmission: None,
                    delayed_offer: None,
                    ended: false,
                    media_address: self.options.media_address,
                    media_bind_address: self.options.media_bind_address,
                    codecs: self.options.media.codecs,
                    profile: self.options.media.profile,
                    current: settled.negotiated,
                    peer_ice: peer_ice_credentials(response.body()),
                    encrypted: self.options.media.profile == MediaProfile::BrowserAudio
                        || settled.srtp.is_some(),
                    keying: self.options.media.keying,
                    hold: self.hold,
                    referral: None,
                    transfer: None,
                    session: session::adopt(
                        response
                            .headers
                            .typed::<SessionExpires>()
                            .and_then(std::result::Result::ok),
                        self.options.session_expires,
                    )
                    .map(SessionState::armed),
                    negotiation: self.negotiation,
                    peer_allows_update: self.peer_allows_update
                        || update::peer_allows(&response.headers),
                    events,
                    events_rx,
                    history: HistoryInfo::from_headers(&response.headers)
                        .and_then(std::result::Result::ok),
                    dialog_credentials: self.options.credentials.clone(),
                    admitted_dialog_methods: Vec::new(),
                })
            }
            Err(error) => {
                // RFC 3261 §15: a UAC that cannot proceed after a 2xx acknowledges it and then
                // sends BYE. Walking away silently is what leaves the far end streaming.
                let dialog = self
                    .dialog
                    .take()
                    .or_else(|| Dialog::from_response(&self.invite, &response));
                if let Some(dialog) = dialog {
                    let in_dialog = in_dialog_target(&dialog, self.target.clone());
                    // discard: counted as `sipx_transport::UnsentCounts::ack`, exactly as in
                    // `ack_then_bye` — the same two sends on the same failing path, written inline
                    // here only because this one already holds the dialog. Via `send_directly`, so
                    // this `Result` does report the transmit.
                    let _ = send_ack(&self.endpoint, &dialog, in_dialog.clone()).await;
                    if let Ok(bye) = bye_request(
                        &dialog,
                        dialog.local_cseq.saturating_add(1),
                        &normal_clearing_reason(),
                    ) {
                        // discard: counted as `sipx_transport::UnsentCounts::bye`, at the
                        // transmit rather than from this `Result`, which reports only that the
                        // transaction was created. It is dropped so that the error which brought
                        // us into this branch is the one the caller is given, rather than being
                        // masked by a teardown failure.
                        let _ = self.endpoint.send(bye, in_dialog).await;
                    }
                }
                Err(error)
            }
        }
    }

    /// Everything after a 2xx that can fail, kept together so [`Self::confirm`] can ACK either way.
    ///
    /// [`establish`] is the same step for [`dial`], and this is not it: nothing here is rebuilt.
    /// The dialog is the one the provisional created, and when the early session was already
    /// answered the description in force is what *it* settled — including anything an UPDATE has
    /// changed since.
    fn accept(&mut self, response: &Response) -> Result<(Dialog, MediaSession, Settled)> {
        // A 2xx bearing a different `To` tag is a different dialog — a forked branch won — and
        // the early one it did not confirm has nothing to contribute to it.
        let confirms_early = self.belongs(response);
        let fresh = Dialog::from_response(&self.invite, response);
        let mut dialog = match (confirms_early, self.dialog.take(), fresh) {
            // The early dialog, its sequence space intact. Two ways to get here and one answer:
            // the usual one, where the 2xx confirms what the provisional established; and a 2xx
            // carrying no usable `To` tag or `Contact`, which establishes no dialog of its own
            // but does not unmake the one a provisional already did.
            (true, Some(early), _) | (_, Some(early), None) => early,
            // A forked branch won: this 2xx names a dialog that is not the early one.
            (_, _, Some(fresh)) => fresh,
            (_, None, None) => return Err(Error::NoDialog),
        };
        dialog.refresh_target(&response.headers);
        self.in_dialog = in_dialog_target(&dialog, self.target.clone());

        let (media, settled) = match self.media.take() {
            // The answer arrived in a provisional, and any UPDATE since settled its own. So the
            // 2xx's body is *not* read: at this point it can only be a repeat of the answer or,
            // worse, a description that undoes the renegotiation. `answer_early` sends no body
            // in this exact case, and for the same reason.
            Some(EarlyMedia::Answered(early)) if confirms_early => (early.media, early.settled),
            Some(EarlyMedia::Answered(early)) => {
                // This 2xx confirmed a different fork from the early dialog the handle names.
                // Never attach the losing branch's running stream to the winner. Multi-branch
                // selection is application policy; until this handle can represent both, the
                // honest outcome is to tear the loser down and ACK-then-BYE the unrepresented
                // winner through `confirm`'s error path.
                drop(early);
                return Err(Error::NoDialog);
            }
            Some(EarlyMedia::Offered(port)) => {
                let settled = self.settle_from(response)?;
                let media = match self.ice.take() {
                    Some(local) => port.start_with_ice(settled.media_config(), local)?,
                    None => port.start(settled.media_config())?,
                };
                (media, settled)
            }
            Some(EarlyMedia::WaitingForOffer) => return Err(Error::NoEarlySession),
            None => return Err(Error::NoDialog),
        };
        Ok((dialog, media, settled))
    }

    /// Read the answer out of the 2xx, for the case where no provisional carried one.
    fn settle_from(&mut self, response: &Response) -> Result<Settled> {
        let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        let Some(capabilities) = self.capabilities.as_ref() else {
            return Err(Error::NoEarlySession);
        };
        let settled = settle_answer(capabilities, &answer, self.options.media.codecs)?;
        self.accept_remote_ice(&answer);
        // Our INVITE's offer is answered here rather than in a provisional, so the exchange
        // closes now. Without this the first UPDATE on the confirmed call would be refused as
        // glare against an offer that has in fact been answered.
        self.negotiation.received_answer();
        Ok(settled)
    }

    /// Give the gathered agent the answer to the offer that created it.
    fn accept_remote_ice(&mut self, answer: &SessionDescription) {
        let negotiation = answer
            .media
            .first()
            .map_or(IceNegotiation::Absent, |audio| {
                sipx_media::ice::negotiate(answer, audio)
            });
        if let Some(local) = self.ice.as_mut() {
            local.accept(&negotiation);
        }
    }

    /// Take back the invitation, whatever state it is in.
    async fn local_cancellation(&mut self, timed_out: bool) -> InvitationCancellation {
        let invitation_elapsed = self.invitation_started.elapsed();
        let cleanup = self.give_up().await;
        InvitationCancellation {
            timed_out,
            invitation_limit: self.options.timeout,
            invitation_elapsed,
            cleanup,
        }
    }

    async fn give_up(&mut self) -> CancellationCleanup {
        self.give_up_with_reason(&normal_clearing_reason()).await
    }

    async fn give_up_with_reason(&mut self, reason: &ReasonValue) -> CancellationCleanup {
        let limit = self.options.cancellation_timeout;
        if let Some(response) = self.coupled_final.take() {
            let started = Instant::now();
            let operation = async {
                if response.status.is_success() {
                    ack_then_bye(&self.endpoint, &self.invite, &response, self.target.clone())
                        .await;
                }
            };
            let exhausted = tokio::time::timeout_at(started + limit, Box::pin(operation))
                .await
                .is_err();
            return CancellationCleanup {
                limit,
                elapsed: started.elapsed(),
                disposition: if exhausted {
                    CancellationDisposition::Exhausted {
                        cancel_sent: false,
                        final_response_observed: true,
                    }
                } else {
                    CancellationDisposition::Completed {
                        cancel_sent: false,
                        final_response_observed: true,
                    }
                },
            };
        }
        let Some(responses) = self.responses.as_mut() else {
            return CancellationCleanup {
                limit,
                elapsed: Duration::ZERO,
                disposition: CancellationDisposition::Completed {
                    cancel_sent: false,
                    final_response_observed: false,
                },
            };
        };
        withdraw(
            &self.endpoint,
            &self.invite,
            self.target.clone(),
            responses,
            reason,
            limit,
        )
        .await
    }
}

/// The `200` that carries the answer, with the session-timer headers the negotiation settled on.
///
/// `agreed` is [`negotiate_session`]'s outcome: `None` when neither side asked for RFC 4028's
/// refresh, and otherwise the interval and refresher this answer commits to. `Require: timer` goes
/// on only when the negotiation said so — a 2xx that requires an extension the offer did not
/// support is a call the caller must reject.
///
/// # Errors
///
/// Returns [`Error`] when a header value cannot be built.
fn ok_with_answer(
    endpoint: &Handle,
    incoming: &Incoming,
    to_with_tag: &str,
    answer: &SessionDescription,
    agreed: Option<session::Accepted>,
    headers: &[sipx_sip::Header],
) -> Result<Response> {
    let mut response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
        .set_header(&HeaderName::To, Bytes::from(to_with_tag.to_owned()))?
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, incoming.transport)),
        )?
        // RFC 3311 §4: the 2xx "SHOULD contain an Allow header field listing the UPDATE
        // method". This is where a UAC learns it, and RFC 4028 §7.4 then reads it to decide
        // whether a session refresh may be an UPDATE.
        .header(
            HeaderName::Allow,
            Bytes::from_static(update::ALLOW.as_bytes()),
        )?
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .body(Bytes::from(answer.to_string_sdp()));

    if let Some(accepted) = agreed {
        let expires = SessionExpires {
            interval: accepted.interval,
            refresher: Some(accepted.refresher),
        };
        response = response
            .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
            .header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        if accepted.require {
            response = response.header(HeaderName::Require, Bytes::from_static(b"timer"))?;
        }
    }
    Ok(add_response_headers(response, headers)?.build())
}

fn add_response_headers(
    mut response: ResponseBuilder,
    headers: &[sipx_sip::Header],
) -> std::result::Result<ResponseBuilder, sipx_sip::error::BuildError> {
    for header in headers {
        response = response.header(
            header.name().clone(),
            Bytes::copy_from_slice(header.raw_value()),
        )?;
    }
    Ok(response)
}

/// Read the offer's ICE half and gather for the answer if the policy selects it (`ice.md` §13.4).
///
/// One function for the three answering paths — the free answer functions, the dispatcher's
/// invitation and [`Early::settle`] — because "when does an answerer gather?" is one rule and
/// three copies of it are three chances to disagree. Two of them already did: one asked
/// `matches!(.., Ice { .. })` and one asked [`IceNegotiation::runs_ice`], which are the same
/// question spelled two ways until one of them acquires a case the other lacks.
///
/// Gathering is deliberately *after* reading the peer's half. A policy selecting ICE never means
/// "require the peer to implement it" (`ice.md` §13.4), so an offer carrying no candidate costs no
/// gathering, no STUN transaction and no timer.
///
/// # Errors
///
/// Returns [`Error`] when the policy cannot produce a gathering configuration.
async fn answer_gathering(
    port: &MediaPort,
    offer: &SessionDescription,
    policy: MediaPolicy,
) -> Result<(IceNegotiation, Option<LocalDescription>)> {
    let remote = answer_ice_negotiation(offer, policy)?;
    if !remote.runs_ice() {
        return Ok((remote, None));
    }
    let local = match policy.gathering(false)? {
        Some(gathering) => Some(
            port.gather_with_rtcp_mode(&gathering, answering_rtcp_mode(offer))
                .await,
        ),
        None => None,
    };
    Ok((remote, local))
}

/// Read the peer ICE half that an answering call is allowed to retain.
///
/// Generic calls preserve every usable component for the ordinary mux fallback. The named
/// browser profile has already required mux, so its pure validator removes an offered RTCP
/// fallback before the media ICE agent can store or pair it.
fn answer_ice_negotiation(
    offer: &SessionDescription,
    policy: MediaPolicy,
) -> Result<IceNegotiation> {
    if policy.profile == MediaProfile::BrowserAudio {
        let remote = sipx_sdp::browser_audio::validate(
            offer,
            sipx_sdp::browser_audio::BrowserAudioRole::Offerer,
        )?;
        return Ok(IceNegotiation::Ice {
            credentials: remote.ice,
            candidates: remote.candidates,
            lite: offer.is_ice_lite(),
        });
    }
    Ok(offer.media.first().map_or(IceNegotiation::Absent, |audio| {
        sipx_media::ice::negotiate(offer, audio)
    }))
}

/// A session that has been described and answered, but not yet accepted.
///
/// What an early dialog needs in order to be renegotiable at all. RFC 3311 §5.1 will not let an
/// UPDATE carry an offer while an offer/answer exchange is open, so before the 200 there is
/// exactly one way to make one legal: the answer to the INVITE's offer travels in a reliable
/// provisional (RFC 3262 §5), and this is what that answer settled on.
///
/// The media port is bound here and handed to the eventual [`Call`] rather than bound again,
/// because the answer already told the far end which port to send to. Binding a second one
/// would make the 200 contradict the 183 for no reason.
#[derive(Debug)]
pub(crate) struct Early {
    pub(crate) media: MediaSession,
    pub(crate) capabilities: Capabilities,
    pub(crate) settled: Settled,
    pub(crate) media_address: IpAddr,
    pub(crate) media_bind_address: IpAddr,
    /// The codec set the provisional's answer was built from, kept because the exchange is not
    /// over: an UPDATE may reoffer before the 200, and it has to be answered from the same set
    /// rather than from the default one.
    pub(crate) codecs: Codecs,
    /// The exact application policy retained when the provisional becomes a confirmed call.
    pub(crate) keying: Keying,
}

/// A media offer placed in a reliable provisional and awaiting its answer in PRACK.
#[derive(Debug)]
pub(crate) struct EarlyOffer {
    port: MediaPort,
    capabilities: Capabilities,
    offer: SessionDescription,
    ice: Option<LocalDescription>,
    keying: PendingKeying,
    policy_keying: Keying,
    media_address: MediaAddress,
    codecs: Codecs,
}

impl EarlyOffer {
    pub(crate) async fn bind(
        media_address: MediaAddress,
        secure: bool,
        direction: Direction,
        policy: MediaPolicy,
    ) -> Result<Self> {
        let media_address = media_address.validate()?;
        if policy.keying == Keying::DtlsSrtp {
            return Err(Error::DtlsEarlyMedia);
        }
        let port = MediaPort::bind(SocketAddr::new(media_address.bind(), 0))
            .await
            .map_err(Error::Io)?;
        let local_ice = match policy.gathering(true)? {
            // As with the ordinary initial offer, mux is not settled until the answer arrives.
            Some(gathering) => Some(
                port.gather_with_rtcp_mode(&gathering, sipx_sdp::RtcpMode::Separate)
                    .await,
            ),
            None => None,
        };
        let advertised = local_ice
            .as_ref()
            .and_then(|local| local.default_destination(ComponentId::RTP))
            .unwrap_or_else(|| {
                SocketAddr::new(media_address.advertised(), port.local_addr().port())
            });
        let (mut capabilities, keying) =
            media_capabilities(policy, advertised.ip(), advertised.port(), secure)?;
        capabilities.direction = direction;
        let mut offer = offer_from(&capabilities);
        if let Some(local) = &local_ice {
            add_ice(&mut offer, local, &[]);
        }
        Ok(Self {
            port,
            capabilities,
            offer,
            ice: local_ice,
            keying,
            policy_keying: policy.keying,
            media_address,
            codecs: policy.codecs,
        })
    }

    pub(crate) fn description(&self) -> &SessionDescription {
        &self.offer
    }

    pub(crate) async fn settle(mut self, answer: &SessionDescription) -> Result<Early> {
        let settled = settle_answer(&self.capabilities, answer, self.codecs)?;
        if let Some(local) = self.ice.as_mut() {
            let remote = answer
                .media
                .first()
                .map_or(IceNegotiation::Absent, |audio| {
                    sipx_media::ice::negotiate(answer, audio)
                });
            local.accept(&remote);
        }
        let (media, settled) = key_and_start(
            self.port,
            self.ice,
            settled,
            self.keying,
            answer,
            false,
            MediaProfile::Standard,
        )
        .await?;
        Ok(Early {
            media,
            capabilities: self.capabilities,
            settled,
            media_address: self.media_address.advertised(),
            media_bind_address: self.media_address.bind(),
            codecs: self.codecs,
            keying: self.policy_keying,
        })
    }
}

impl Early {
    /// Bind a port and answer `offer` with it.
    pub(crate) async fn settle(
        media_address: MediaAddress,
        secure: bool,
        offer: &SessionDescription,
        policy: MediaPolicy,
    ) -> Result<(Self, SessionDescription)> {
        let media_address = media_address.validate()?;
        if policy.keying == Keying::DtlsSrtp {
            return Err(Error::DtlsEarlyMedia);
        }
        let negotiated = negotiated(offer, policy.codecs)?;
        let port = MediaPort::bind(SocketAddr::new(media_address.bind(), 0))
            .await
            .map_err(Error::Io)?;
        let (remote_ice, mut local_ice) = answer_gathering(&port, offer, policy).await?;
        let advertised = local_ice
            .as_ref()
            .and_then(|local| local.default_destination(ComponentId::RTP))
            .unwrap_or_else(|| {
                SocketAddr::new(media_address.advertised(), port.local_addr().port())
            });
        let (capabilities, _keying) =
            media_capabilities(policy, advertised.ip(), advertised.port(), secure)?;
        let mut answer = sipx_sdp::answer(offer, &capabilities);
        if let Some(local) = local_ice.as_mut() {
            local.accept(&remote_ice);
            add_ice(&mut answer, local, &remote_ice.answer_attributes());
        } else if policy.ice != IcePolicy::Disabled
            && let Some(audio) = answer.media.first_mut()
        {
            audio.attributes.extend(remote_ice.answer_attributes());
        }
        if answer
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return Err(Error::NoCommonCodec);
        }
        let settled = Settled {
            negotiated,
            srtp: srtp_keys_answering(capabilities.crypto.as_ref(), offer_crypto(offer)),
        };
        let media = match local_ice {
            Some(local) => port.start_with_ice(settled.media_config(), local)?,
            None => port.start(settled.media_config())?,
        };
        Ok((
            Self {
                media,
                capabilities,
                settled,
                media_address: media_address.advertised(),
                media_bind_address: media_address.bind(),
                codecs: policy.codecs,
                keying: policy.keying,
            },
            answer,
        ))
    }

    /// Take the far end's answer to an offer *we* made, which moves only where we send.
    ///
    /// Nothing is owed back for an answer, so unlike [`Self::reanswer`] this produces no
    /// description. An answer that cannot be read leaves the session where it was: the far end
    /// accepted something, and guessing which of our formats it meant is worse than keeping
    /// what the last completed exchange settled.
    pub(crate) async fn adopt_answer(&mut self, answer: &SessionDescription) {
        if let Ok(negotiated) = negotiated(answer, self.codecs) {
            let settled = Settled {
                negotiated,
                srtp: self.settled.srtp.clone(),
            };
            // A failed replacement leaves the working early stream in place. The UPDATE itself
            // was usable, so turning a local socket failure into a peer refusal would describe
            // the wrong fault; the eventual call still confirms the last session that ran.
            // discard: the peer has already answered our UPDATE, so there is no signalling
            // response left to change; the still-running media session is the safe fallback.
            let _ = self.replace_media(settled).await;
        }
    }

    /// Answer a *later* offer — one that arrived in an UPDATE — on the port already bound.
    ///
    /// `None` means the description is unusable, and the caller refuses 488 while the early
    /// dialog carries on: the same rule a re-INVITE gets in `M-8`, for the same reason.
    ///
    /// The port does not move. Our own receive address was published in the answer the peer
    /// already has, and changing it because *their* description changed would ask them to
    /// renegotiate again to learn where we went.
    pub(crate) async fn reanswer(
        &mut self,
        offer: &SessionDescription,
    ) -> Option<SessionDescription> {
        let negotiated = negotiated(offer, self.codecs).ok()?;
        let answer = sipx_sdp::answer(offer, &self.capabilities);
        if answer
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return None;
        }
        let settled = Settled {
            negotiated,
            srtp: srtp_keys_answering(self.capabilities.crypto.as_ref(), offer_crypto(offer)),
        };
        self.replace_media(settled).await.ok()?;
        Some(answer)
    }

    /// Apply an early UPDATE to the session that is already running.
    ///
    /// This is the same transition [`Call::move_media_if_changed`] performs for a confirmed
    /// dialog, but it happens at UPDATE time rather than being deferred to the INVITE's 2xx. The
    /// resulting session is then the one confirmation moves into `Call`, so answer time itself
    /// still neither rebinds nor leaves a gap.
    async fn replace_media(&mut self, settled: Settled) -> Result<()> {
        let to = settled.negotiated;
        let changed = to.remote != self.settled.negotiated.remote
            || to.codec != self.settled.negotiated.codec
            || to.wire_payload_type() != self.settled.negotiated.wire_payload_type()
            || settled.is_encrypted() != self.settled.is_encrypted();
        if changed && !self.media.reconfigure(settled.media_config()).await? {
            return Err(Error::Sdp(
                "an ICE-backed early session cannot change its media format in place".to_owned(),
            ));
        }
        self.settled = settled;
        Ok(())
    }
}

/// Answer an INVITE that has already been rung (RFC 3262).
///
/// The tag comes from the [`Ringing`](crate::Ringing) rather than being fresh, and that is the
/// whole reason this exists. A provisional that established a dialog has already told the caller
/// what this side's tag is (RFC 3261 §12.1.1); a 200 with a different one creates a *second*
/// dialog. The caller ACKs the dialog it knows about, this side waits for an ACK to the other,
/// and the 200 is retransmitted for 32 seconds into a call that is actually up.
///
/// Answers from the default codec set, [`Codecs::G711`]. [`answer_ringing_with`] takes a
/// selection.
pub async fn answer_ringing(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    ringing: &crate::Ringing,
) -> Result<Call> {
    answer_ringing_with(
        endpoint,
        incoming,
        media_address,
        ringing,
        Codecs::default(),
    )
    .await
}

/// [`answer_ringing`], from a chosen codec set rather than the default one (`M-30`).
///
/// The selection is made here rather than at [`ring`](crate::ring) because `ring` sends a
/// bodiless provisional: nothing about the session has been said yet when it goes out, so the
/// answer this builds is still the first one. That is exactly what separates this from
/// [`answer_early`], where the answer left in the 183 and the choice had to be made with it.
pub async fn answer_ringing_with(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    ringing: &crate::Ringing,
    codecs: Codecs,
) -> Result<Call> {
    answer_ringing_with_policy(
        endpoint,
        incoming,
        media_address,
        ringing,
        MediaPolicy::default().with_codecs(codecs),
    )
    .await
}

/// [`answer_ringing`], using one coherent codec and ICE policy.
pub async fn answer_ringing_with_policy(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    ringing: &crate::Ringing,
    policy: MediaPolicy,
) -> Result<Call> {
    answer_ringing_with_policy_at(
        endpoint,
        incoming,
        MediaAddress::new(media_address),
        ringing,
        policy,
    )
    .await
}

/// [`answer_ringing_with_policy`] with independent advertised and bound media addresses.
pub async fn answer_ringing_with_policy_at(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: MediaAddress,
    ringing: &crate::Ringing,
    policy: MediaPolicy,
) -> Result<Call> {
    // RFC 3262 §3 and §5: a 2xx must not go out while a reliable provisional carrying a session
    // description is unacknowledged. This path never puts a description in one — `ring` sends a
    // bodiless provisional, and `ring_early` is the entry point that does, where
    // [`answer_early`] enforces the MUST. What is left here is the weaker concern: answering
    // before the PRACK means retransmitting a `180` at a caller that has moved on, and the
    // ringing is stopped either way when `Ringing` drops.
    if !ringing.is_acknowledged() {
        tracing::debug!("answering before the reliable provisional was acknowledged");
    }
    let offer = match sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body())) {
        Ok(offer) => offer,
        Err(error) => {
            let error = Error::Sdp(error.to_string());
            refuse_initial_offer(endpoint, incoming, ringing.tag(), None, 400, "Bad Request")
                .await?;
            return Err(error);
        }
    };
    answer_negotiated(
        endpoint,
        incoming,
        media_address,
        offer,
        ringing.tag(),
        Some(ringing.is_reliable()),
        None,
        policy,
        &[],
    )
    .await
}

/// Answer an INVITE that was rung with [`crate::rel::ring_early`].
///
/// The counterpart of [`answer_ringing`] for a dialog whose offer/answer already completed in
/// the provisional. Three things follow from that and none is optional:
///
/// - **The provisional must already be acknowledged.** RFC 3262 §5 is a MUST: a UAS that put a
///   session description in a reliable provisional delays the 2xx until that provisional is
///   acknowledged. So this returns [`Error::UnacknowledgedProvisional`] rather than answering, and
///   the caller keeps feeding messages to [`Ringing::on_prack`](crate::Ringing::on_prack) until
///   [`Ringing::is_acknowledged`](crate::Ringing::is_acknowledged) is true. It cannot wait on
///   the caller's behalf: the PRACK arrives on the application's own inbox, and this holds the
///   `&mut` that handling it would need.
/// - **The 200 carries no session description.** There is nothing left to say: the offer was
///   answered in the 183, and anything an UPDATE renegotiated afterwards was answered in its own
///   2xx. Repeating the last answer here would be a second answer to the INVITE's offer, and
///   repeating the *first* one would silently undo the renegotiation. That is only safe
///   *because* of the rule above — the PRACK is proof the caller holds the answer, and without
///   it a lost 183 would leave the caller in a confirmed dialog with no description at all.
/// - **The media port is the one the provisional named**, not a fresh one, because that is the
///   port the far end has already been told to send to.
///
/// The `Ringing` is borrowed mutably and emptied rather than consumed, because it owns the
/// retransmission of the provisional and must go on owning it until it is dropped.
pub async fn answer_early(
    endpoint: &Handle,
    incoming: &Incoming,
    ringing: &mut crate::Ringing,
) -> Result<Call> {
    if !ringing.is_acknowledged() {
        return Err(Error::UnacknowledgedProvisional);
    }

    // Before anything is taken out of the `Ringing`. A 422 leaves here through the `?`, and it
    // is a counter-offer rather than a failure — the caller is expected to be rung again — so
    // it must not cost the bound port and the session the early exchange settled.
    let agreed = negotiate_session(endpoint, incoming).await?;

    let (early, dialog, negotiation, peer_allows_update) = ringing.take_early()?;
    let target = in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));

    let to_with_tag = {
        let existing = incoming
            .request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        format!("{};tag={}", strip_header_params(&existing), ringing.tag())
    };

    let mut response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
        .set_header(&HeaderName::To, Bytes::from(to_with_tag))?
        .header(
            HeaderName::Contact,
            Bytes::from(contact_for(endpoint, incoming.transport)),
        )?
        .header(
            HeaderName::Allow,
            Bytes::from_static(update::ALLOW.as_bytes()),
        )?;
    if let Some(accepted) = agreed {
        let expires = SessionExpires {
            interval: accepted.interval,
            refresher: Some(accepted.refresher),
        };
        response = response
            .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
            .header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        if accepted.require {
            response = response.header(HeaderName::Require, Bytes::from_static(b"timer"))?;
        }
    }
    let response = response.build();

    let media = early.media;
    endpoint.respond(&incoming.key, response.clone()).await?;

    let ack_stop = CancellationToken::new();
    let ack_retransmission = OwnedTask::new(tokio::spawn(retransmit_until_acked(
        endpoint.clone(),
        incoming.key.clone(),
        response,
        ack_stop.clone(),
    )));

    let (events, events_rx) = EventSink::new();
    emit_construction_events(&events, Some(ringing.is_reliable()));

    Ok(Call {
        dialog,
        initial_status: OK,
        media: Arc::new(media),
        retired_media: Vec::new(),
        endpoint: endpoint.clone(),
        target,
        ack_stop: Some(ack_stop),
        ack_retransmission: Some(ack_retransmission),
        delayed_offer: None,
        ended: false,
        media_address: early.media_address,
        media_bind_address: early.media_bind_address,
        codecs: early.codecs,
        profile: MediaProfile::Standard,
        current: early.settled.negotiated,
        peer_ice: peer_ice_credentials(incoming.request.body()),
        hold: Direction::SendRecv,
        encrypted: early.settled.is_encrypted(),
        keying: early.keying,
        referral: None,
        transfer: None,
        session: agreed.map(|accepted| {
            SessionState::armed(session::Session {
                interval: accepted.interval,
                we_refresh: accepted.refresher == session::Refresher::Uas,
            })
        }),
        negotiation,
        peer_allows_update,
        events,
        events_rx: Some(events_rx),
        history: HistoryInfo::from_headers(&incoming.request.headers)
            .and_then(std::result::Result::ok),
        dialog_credentials: None,
        admitted_dialog_methods: Vec::new(),
    })
}

/// Settle the RFC 4028 session timer for an incoming INVITE, refusing it if it asks for too
/// little.
///
/// Sends the `422` itself, because the refusal has to carry the floor and the only thing that
/// knows the floor is the negotiation. Returning "too brief" and leaving the caller to build
/// the response would make the one header that makes a 422 useful optional.
async fn negotiate_session(
    endpoint: &Handle,
    incoming: &Incoming,
) -> Result<Option<session::Accepted>> {
    Ok(
        match session::answer(
            incoming
                .request
                .headers
                .typed::<sipx_sip::headers::misc::Supported>()
                .and_then(std::result::Result::ok)
                .is_some_and(|s| s.contains(session::OPTION_TAG)),
            incoming
                .request
                .headers
                .typed::<SessionExpires>()
                .and_then(std::result::Result::ok),
            incoming
                .request
                .headers
                .typed::<MinSe>()
                .and_then(std::result::Result::ok)
                .map(|min| min.0),
            session::ABSOLUTE_MIN_INTERVAL,
        ) {
            session::Answer::TooBrief(floor) => {
                // RFC 4028 §6: the 422 has to carry the minimum, or the caller learns only that it
                // was wrong and not what would be right, and retries the same interval forever.
                const INTERVAL_TOO_SMALL: u16 = 422;
                let status = StatusCode::new(INTERVAL_TOO_SMALL)
                    .unwrap_or_else(|| unreachable!("422 is a valid status code"));
                let refusal = ResponseBuilder::to_request(
                    &incoming.request,
                    status,
                    "Session Interval Too Small",
                )?
                .header(HeaderName::MinSe, Bytes::from(floor.as_secs().to_string()))?
                .build();
                endpoint.respond(&incoming.key, refusal).await?;
                return Err(Error::IntervalTooBrief(floor));
            }
            session::Answer::None => None,
            session::Answer::Accept(accepted) => Some(accepted),
        },
    )
}

/// Called immediately before the `200` is handed to the transport, to take the invitation.
///
/// The dispatcher's hook into a path that otherwise knows nothing about invitations. Returning
/// `Err` aborts the answer with nothing sent; returning `Ok` means no later CANCEL may draw a
/// `487`, because a `200` is about to be on the wire. [`answer_negotiated`] documents why it is
/// invoked exactly where it is.
///
/// `Send + Sync` so that `&Claim` is `Send` and the futures carrying one stay spawnable, which
/// `an_answer_future_is_spawnable` holds to.
pub(crate) type Claim<'a> = &'a (dyn Fn() -> Result<()> + Send + Sync);

/// End an initial offer that cannot become a call, through the INVITE's server transaction.
///
/// Everything that can fail while shaping the response happens before the optional dispatcher
/// claim. Once claimed, a crossing CANCEL must not replace this final response with 487. A send
/// failure is returned instead of the local negotiation error so it stays observable and the
/// transport cannot count a response it did not hand to the socket.
async fn refuse_initial_offer(
    endpoint: &Handle,
    incoming: &Incoming,
    tag: &str,
    claim: Option<Claim<'_>>,
    code: u16,
    reason: &'static str,
) -> Result<()> {
    let status = StatusCode::new(code)
        .ok_or_else(|| Error::Sdp(format!("invalid initial-offer response status {code}")))?;
    let existing = incoming
        .request
        .headers
        .value(&HeaderName::To)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .unwrap_or_default();
    let to_with_tag = format!("{};tag={tag}", strip_header_params(&existing));
    let response = ResponseBuilder::to_request(&incoming.request, status, reason)?
        .set_header(&HeaderName::To, Bytes::from(to_with_tag))?
        .build();
    if let Some(claim) = claim {
        claim()?;
    }
    endpoint.respond(&incoming.key, response).await?;
    Ok(())
}

/// Answer an INVITE whose offer has already been parsed.
///
/// `reliable_ringing` is `Some` exactly when this side rang first (via [`crate::rel::ring`]):
/// `Some(reliable)` reports whether that provisional was 100rel-acknowledged, and `None` (the
/// [`answer`] path) means there is no ringing to report at all.
///
/// `claim` is the dispatcher's, and is invoked at one specific line below; see [`Claim`].
///
/// `codecs` is what this side is willing to carry. It bounds the answer at both ends of the one
/// exchange: [`negotiated`] may not settle outside it, and [`Codecs::capabilities`] builds the
/// answer from it — so the codec the session starts on is always one the answer named.
// Eight, and every one of them is a distinct fact about *this* answer that the caller holds and
// this does not. Bundling them into a struct would be a struct with one construction site per
// caller and no behaviour, which moves the argument list rather than shortening it.
#[allow(clippy::too_many_arguments)]
#[allow(
    clippy::too_many_lines,
    reason = "the answer lifecycle remains in wire order; custom fields add one retained input"
)]
async fn answer_negotiated(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: MediaAddress,
    offer: SessionDescription,
    tag: &str,
    reliable_ringing: Option<bool>,
    claim: Option<Claim<'_>>,
    policy: MediaPolicy,
    headers: &[sipx_sip::Header],
) -> Result<Call> {
    validate_profile_preflight(policy, incoming.transport)?;
    let media_address = media_address.validate()?;
    if policy.profile == MediaProfile::BrowserAudio {
        sipx_sdp::browser_audio::validate(
            &offer,
            sipx_sdp::browser_audio::BrowserAudioRole::Offerer,
        )?;
    }
    validate_dtls_offer_setup(&offer, policy)?;
    let negotiated = match negotiated(&offer, policy.codecs) {
        Ok(negotiated) => negotiated,
        Err(Error::NoCommonCodec) => {
            refuse_initial_offer(endpoint, incoming, tag, claim, 488, "Not Acceptable Here")
                .await?;
            return Err(Error::NoCommonCodec);
        }
        Err(error) => return Err(error),
    };

    // The port is bound before the session starts, because the answer has to name it *and* the
    // session has to be created with the keys that answer settles on. Starting the session first
    // — as this did — leaves nowhere to put them.
    let port = MediaPort::bind(SocketAddr::new(media_address.bind(), 0))
        .await
        .map_err(Error::Io)?;

    let (remote_ice, mut local_ice) = answer_gathering(&port, &offer, policy).await?;
    let advertised = local_ice
        .as_ref()
        .and_then(|local| local.default_destination(ComponentId::RTP))
        .unwrap_or_else(|| SocketAddr::new(media_address.advertised(), port.local_addr().port()));
    let (capabilities, keying) = media_capabilities(
        policy,
        advertised.ip(),
        advertised.port(),
        incoming.transport.is_secure(),
    )?;
    let mut answer_sdp = if policy.profile == MediaProfile::BrowserAudio {
        let local = local_ice
            .as_ref()
            .ok_or(sipx_sdp::browser_audio::ProfileError::IceRequired)?;
        let fingerprint = capabilities
            .dtls()
            .cloned()
            .ok_or(sipx_sdp::browser_audio::ProfileError::FingerprintRequired)?;
        sipx_sdp::browser_audio::answer(
            &offer,
            &sipx_sdp::browser_audio::BrowserAudioLocal {
                address: advertised.ip(),
                port: advertised.port(),
                session_id: capabilities.session_id,
                session_version: capabilities.session_version,
                direction: capabilities.direction,
                ice: local.credentials().clone(),
                candidates: local.candidates().to_vec(),
                fingerprint,
                setup: sipx_sdp::fingerprint::SetupCapabilities::both(),
            },
        )?
    } else {
        sipx_sdp::answer(&offer, &capabilities)
    };
    if let Some(local) = local_ice.as_mut() {
        local.accept(&remote_ice);
        if policy.profile == MediaProfile::Standard {
            add_ice(&mut answer_sdp, local, &remote_ice.answer_attributes());
        }
    } else if policy.ice != IcePolicy::Disabled
        && let Some(audio) = answer_sdp.media.first_mut()
    {
        audio.attributes.extend(remote_ice.answer_attributes());
    }
    if answer_sdp
        .media
        .iter()
        .all(sipx_sdp::MediaDescription::is_rejected)
    {
        refuse_initial_offer(endpoint, incoming, tag, claim, 488, "Not Acceptable Here").await?;
        return Err(Error::NoCommonCodec);
    }

    // Our key from the answer we just built, theirs from the offer we were sent.
    let settled = Settled {
        negotiated,
        srtp: srtp_keys_answering(capabilities.crypto.as_ref(), offer_crypto(&offer)),
    };
    let to_with_tag = {
        let existing = incoming
            .request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        format!("{};tag={tag}", strip_header_params(&existing))
    };

    let agreed = negotiate_session(endpoint, incoming).await?;

    let response = ok_with_answer(
        endpoint,
        incoming,
        &to_with_tag,
        &answer_sdp,
        agreed,
        headers,
    )?;

    // Before the 200, not after. An INVITE with no usable `Contact` cannot form a dialog
    // (RFC 3261 §12.1.1), and answering first would put a 2xx on the wire for a call this side
    // is then unable to hold: the caller ACKs, believes it has a confirmed dialog, and streams
    // media at an endpoint that has forgotten it and can never send the BYE.
    let dialog = Dialog::from_request(&incoming.request, tag).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport));

    // The last thing before the `200` leaves, and that placement is the whole contract.
    //
    // Taking the invitation *early* would be simpler, but every fallible step above — parsing the
    // offer, binding the port, negotiating the session, building the response, forming the dialog
    // — can return `Err` with nothing on the wire. An invitation taken by one of those failures
    // is one no CANCEL can ever end: the CANCEL draws its `200`, the `487` is suppressed because
    // the invitation looks answered, and the INVITE transaction is left without a final response
    // for the caller's Timer B to resolve.
    //
    // Taking it *here* keeps the guarantee the early claim was for. From this line on, the only
    // fallible expression is `respond` itself; everything after it is infallible. So a CANCEL
    // arriving from now on finds the invitation taken and correctly sends no `487` behind the
    // `200` — and a CANCEL arriving a moment earlier is honoured in full, which is what it is
    // owed, because nothing has been sent yet.
    //
    // `respond` failing is the one case that stays claimed. That is deliberate: a stream
    // transport can write part of a response before erroring, so "it failed" is not proof that
    // nothing reached the caller, and a `487` chasing a `200` is the worse of the two outcomes.
    if let Some(claim) = claim {
        claim()?;
    }

    endpoint.respond(&incoming.key, response.clone()).await?;

    let ack_stop = CancellationToken::new();
    let mut ack_retransmission = OwnedTask::new(tokio::spawn(retransmit_until_acked(
        endpoint.clone(),
        incoming.key.clone(),
        response,
        ack_stop.clone(),
    )));

    // The answer must leave before an active answerer sends ClientHello. A caller is permitted to
    // wait for the final SDP (and then its ACK) before opening the media path.
    let started = Box::pin(key_and_start(
        port,
        local_ice,
        settled,
        keying,
        &offer,
        true,
        policy.profile,
    ))
    .await;
    let (media, settled) = match started {
        Ok(started) => started,
        Err(error) => {
            cancel_and_join(&ack_stop, &mut ack_retransmission).await;
            return Err(error);
        }
    };

    // As in `dial_with`: emitted at construction, from what was actually observed (ringing
    // first, if this path came through it) rather than recomputed afterwards.
    let (events, events_rx) = EventSink::new();
    emit_construction_events(&events, reliable_ringing);

    Ok(Call {
        dialog,
        initial_status: OK,
        media: Arc::new(media),
        retired_media: Vec::new(),
        endpoint: endpoint.clone(),
        target,
        ack_stop: Some(ack_stop),
        ack_retransmission: Some(ack_retransmission),
        delayed_offer: None,
        ended: false,
        media_address: media_address.advertised(),
        media_bind_address: media_address.bind(),
        codecs: policy.codecs,
        profile: policy.profile,
        current: settled.negotiated,
        peer_ice: peer_ice_credentials(incoming.request.body()),
        hold: Direction::SendRecv,
        encrypted: policy.profile == MediaProfile::BrowserAudio || settled.srtp.is_some(),
        keying: policy.keying,
        referral: None,
        transfer: None,
        session: agreed.map(|accepted| {
            SessionState::armed(session::Session {
                interval: accepted.interval,
                we_refresh: accepted.refresher == session::Refresher::Uas,
            })
        }),
        negotiation: update::Negotiation::idle(),
        // From the INVITE, which RFC 3311 §4 asks a compliant UAC to put it on.
        peer_allows_update: update::peer_allows(&incoming.request.headers),
        events,
        events_rx: Some(events_rx),
        history: HistoryInfo::from_headers(&incoming.request.headers)
            .and_then(std::result::Result::ok),
        dialog_credentials: None,
        admitted_dialog_methods: Vec::new(),
    })
}

/// Resend a 2xx on the T1 backoff until the ACK arrives or 64·T1 has passed.
async fn retransmit_until_acked(
    endpoint: Handle,
    key: sipx_sip::transaction::TransactionKey,
    response: Response,
    stop: CancellationToken,
) {
    let t1 = Duration::from_millis(500);
    let mut interval = t1;
    let mut elapsed = Duration::ZERO;
    let give_up = t1 * 64;

    loop {
        if until_cancelled(&stop, tokio::time::sleep(interval))
            .await
            .is_none()
        {
            return;
        }
        elapsed += interval;
        if elapsed >= give_up {
            tracing::warn!("no ACK for our 2xx after 64*T1; giving up");
            return;
        }
        let Some(sent) = until_cancelled(&stop, endpoint.respond(&key, response.clone())).await
        else {
            return;
        };
        if sent.is_err() {
            return;
        }
        // Doubling capped at T2, exactly as the INVITE client transaction retransmits.
        interval = (interval * 2).min(Duration::from_secs(4));
    }
}

/// What waiting for a final response ended in.
enum Waited {
    /// A final response arrived.
    Final {
        /// The response itself.
        response: Response,
        /// Whether a provisional counting as *ringing* — anything past a bare `100 Trying` —
        /// was seen first, and whether it was reliable (RFC 3262). `None` when the far end
        /// went straight to the final response, which is the one time no `CallEvent::Ringing`
        /// belongs on the eventual call's event stream.
        ringing: Option<bool>,
    },
    /// The deadline passed.
    GaveUp,
    /// The owner asked this attempt to stop.
    Cancelled,
    /// The transaction ended without a final response.
    Gone,
    /// The selected transport could not be established or used.
    Transport(sipx_transport::Error),
}

/// Wait for the final response to an INVITE.
///
/// The transport response stream retains the provisional observation that RFC 3261 §9.1 needs
/// if this wait ends in local cancellation.
/// What a UAC needs in order to acknowledge a reliable provisional while it waits.
struct Acknowledging<'a> {
    endpoint: &'a Handle,
    invite: &'a Request,
    target: &'a Target,
    capabilities: &'a Capabilities,
    seen: sipx_sip::rel::Sequence,
}

async fn await_final(
    responses: &mut sipx_transport::Responses,
    limit: Option<Duration>,
    acknowledging: &mut Acknowledging<'_>,
    cancelled: &mut Option<Cancelled<'_>>,
) -> Waited {
    let deadline = limit.map(|limit| tokio::time::Instant::now() + limit);
    let mut ringing = None;
    loop {
        let event = match (deadline, cancelled.as_mut()) {
            (None, None) => responses.next().await,
            (Some(deadline), None) => match before_deadline(deadline, responses.next()).await {
                BeforeDeadline::Event(event) => event,
                BeforeDeadline::Expired => return Waited::GaveUp,
            },
            (None, Some(cancelled)) => {
                tokio::select! {
                    biased;
                    () = cancelled.as_mut() => return Waited::Cancelled,
                    event = responses.next() => event,
                }
            }
            (Some(deadline), Some(cancelled)) => {
                tokio::select! {
                    biased;
                    () = cancelled.as_mut() => return Waited::Cancelled,
                    () = tokio::time::sleep_until(deadline) => return Waited::GaveUp,
                    event = responses.next() => event,
                }
            }
        };
        match event {
            Some(sipx_sip::transaction::TuEvent::Response(response)) => {
                if response.status.is_final() {
                    return Waited::Final {
                        response: *response,
                        ringing,
                    };
                }
                // A bare `100 Trying` only acknowledges that the request arrived (RFC 3261
                // §17.2.1); it is not the far end's phone ringing, and 100rel does not apply
                // to it either (RFC 3262 §3), so it is excluded from what `ringing` tracks.
                if response.status.code() > 100 {
                    ringing = Some(crate::rel::reliable_sequence(&response).is_some());
                }
                // RFC 3262 §4. A failure here is logged rather than fatal: the invitation is
                // still running, and abandoning a ringing call because one PRACK did not get
                // through would be a worse outcome than the unreliability it was fixing.
                if let Err(error) = acknowledge(&response, acknowledging).await {
                    tracing::debug!(%error, "could not acknowledge a reliable provisional");
                }
            }
            Some(sipx_sip::transaction::TuEvent::TransportError) => {
                // The endpoint queues the concrete driver cause before this event. Preserve every
                // cause: connection refusal, TLS verification failure and a closed established
                // stream are definitive transport outcomes, not evidence that a reachable SIP
                // peer stayed silent.
                if let Some(error) = responses.take_transport_error() {
                    return Waited::Transport(error);
                }
                return Waited::Gone;
            }
            Some(_) => {}
            None => return Waited::Gone,
        }
    }
}

/// PRACK a reliable provisional, if that is what this is (RFC 3262 §4).
async fn acknowledge(response: &Response, ctx: &mut Acknowledging<'_>) -> Result<()> {
    let Some(rseq) = crate::rel::reliable_sequence(response) else {
        return Ok(());
    };
    // §4: out of order means an earlier one is missing, and a duplicate has already been
    // acknowledged. Neither is PRACKed — re-acknowledging a retransmission would turn one lost
    // packet into a stream of PRACKs, and acknowledging a gap would tell the UAS that
    // everything up to this number arrived when it did not.
    if ctx.seen.accept(rseq) != sipx_sip::rel::Received::Acknowledge {
        return Ok(());
    }

    // §4: "The provisional response MUST establish a dialog if one is not yet created." The
    // PRACK is an in-dialog request and has nowhere to go without it.
    let mut dialog = Dialog::from_response(ctx.invite, response).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, ctx.target.clone());
    let invite_cseq = ctx
        .invite
        .headers
        .typed::<sipx_sip::CSeq>()
        .and_then(std::result::Result::ok)
        .map_or(1, |cseq| cseq.sequence);

    let body = crate::rel::prack_body(
        !ctx.invite.body().is_empty(),
        response.body(),
        ctx.capabilities,
    );
    crate::rel::send_prack(ctx.endpoint, &mut dialog, &target, rseq, invite_cseq, body).await
}

fn normal_clearing_reason() -> ReasonValue {
    ReasonValue::q850(16, Some(b"Normal call clearing".to_vec()))
}

fn request_timeout_reason() -> ReasonValue {
    // A constant defined by the SIP status-code space; construction cannot fail.
    StatusCode::new(408).map_or_else(
        || ReasonValue::q850(102, Some(b"Recovery on timer expiry".to_vec())),
        |status| ReasonValue::sip(status, Some(b"Request Timeout".to_vec())),
    )
}

/// Acknowledge a 2xx (RFC 3261 §13.2.2.4).
///
/// This ACK is not part of the INVITE transaction and has no transaction of its own: it is
/// "passed to the transport layer directly for transmission", carries a *new* branch because
/// any proxy must treat it as a new request, and is resent only when a retransmitted 2xx
/// arrives. Handing it to the transaction layer instead earns it the retransmission timers of
/// a non-INVITE request — a stream of duplicate ACKs toward a response that will never come,
/// and a spurious timeout 32 seconds into a call that is up and talking.
async fn send_ack(endpoint: &Handle, dialog: &Dialog, target: Target) -> Result<()> {
    let ack = build_ack(endpoint, dialog, &target)?;
    endpoint.send_directly(ack, target).await?;
    Ok(())
}

/// Keep acknowledging a 2xx for as long as the far end keeps retransmitting it.
///
/// RFC 3261 §13.2.2.4: the UAC core "MUST generate an ACK for each 2xx received", and a
/// retransmitted 2xx says the previous ACK never arrived. Nobody else can do this — the INVITE
/// transaction has already passed the response up, and RFC 6026's `Accepted` state exists so
/// that the retransmission still has somewhere to arrive. Dropping the stream after the first
/// answer leaves the far end retransmitting for 64*T1 and then tearing down, from its side, a
/// call this side believes is established and is already sending audio into.
///
/// The same ACK goes out each time, rather than a freshly built one: it acknowledges one
/// response, and a new branch on every repeat would present each as a new request.
async fn reack_retransmitted_2xx(
    endpoint: Handle,
    mut responses: sipx_transport::Responses,
    ack: Request,
    target: Target,
) {
    while let Some(event) = responses.next().await {
        if let sipx_sip::transaction::TuEvent::Response(response) = event
            && response.status.is_success()
            && endpoint
                .send_directly(ack.clone(), target.clone())
                .await
                .is_err()
        {
            return;
        }
    }
}

fn build_ack(endpoint: &Handle, dialog: &Dialog, target: &Target) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let via = format!(
        "SIP/2.0/{} {};rport;branch={}",
        target.transport.as_str(),
        endpoint.sent_by_for(target.transport),
        sipx_transport::new_branch()
    );
    let ack = RequestBuilder::new(Method::Ack, uri)
        .header(HeaderName::Via, Bytes::from(via))?
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
        // The ACK for a 2xx carries the INVITE's sequence number, not a new one: it
        // acknowledges that request rather than being one of its own.
        .cseq(dialog.local_cseq, &Method::Ack)?
        .max_forwards(70);
    Ok(add_routes(ack, &routes)?.build())
}

/// Where in-dialog requests go.
///
/// RFC 3261 §12.2.1.1: the peer's `Contact`, not the address the INVITE was sent to. Those
/// differ whenever a redirect, a B2BUA or a load balancer is involved, and using the original
/// address means the ACK and the BYE reach the wrong element.
///
/// A `Contact` naming a hostname would need resolution, which this layer does not do; the
/// address the exchange arrived from is the honest fallback, and behind a NAT it is the only
/// one that works.
pub(crate) fn in_dialog_target(dialog: &Dialog, fallback: Target) -> Target {
    // Over a WebSocket or QUIC the `Contact` is not consulted at all. RFC 7118 §5.2: the peer has no
    // listening port, its `Contact` names something that will never resolve, and the connection
    // the dialog was established on is the only way to reach it. This is the RFC 5923 rule for
    // stream transports made absolute — there is no fallback because there is nowhere to fall
    // back to, and honouring a `Contact` here would send the BYE to an address that either does
    // not answer or belongs to somebody else.
    if matches!(
        fallback.transport,
        TransportKind::Ws | TransportKind::Wss | TransportKind::Quic
    ) {
        return fallback;
    }

    // The first hop, which is the remote target only when the dialog has no route set. With
    // one they are different elements, and RFC 3261 §12.2.1.1 hands the request to the former.
    let hop = dialog.hop();
    let Some(sipx_sip::Host::Ip(ip)) = hop.host() else {
        return fallback;
    };
    let transport = hop
        .transport()
        .and_then(TransportKind::parse)
        .unwrap_or(fallback.transport);
    let port = hop.port().unwrap_or_else(|| transport.default_port());
    Target::new(SocketAddr::new(*ip, port), transport)
}

/// The `Contact` this endpoint should advertise for a dialog on this transport.
///
/// Built from the endpoint's *advertised* address rather than its socket's local one. An
/// endpoint bound to `0.0.0.0` has a local address that means nothing to a peer, and behind a
/// NAT it is private — either way the peer stores it as the dialog's remote target and every
/// in-dialog request it sends becomes unroutable.
///
/// The transport matters because over a WebSocket there is no address to advertise at all: the
/// endpoint gives the invented name RFC 7118 §5.2 requires, and marks the URI with the same
/// transport token it puts in the `Via`, so a peer that does route on `Contact` knows not to
/// try `sip:` on port 5060.
#[must_use]
pub fn contact_for(endpoint: &Handle, transport: TransportKind) -> String {
    match transport {
        TransportKind::Ws | TransportKind::Wss | TransportKind::Quic => format!(
            "<sip:sipx@{};transport={}>",
            endpoint.sent_by_for(transport),
            transport.as_str().to_ascii_lowercase()
        ),
        _ => format!("<sip:sipx@{}>", endpoint.advertised()),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::Poll;

    use super::offer_answer::tests::offered;
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn answer_events_must_precede_their_deadline() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(1);
        assert!(matches!(
            before_deadline(
                deadline,
                tokio::time::sleep_until(deadline - Duration::from_millis(1))
            )
            .await,
            BeforeDeadline::Event(())
        ));

        let now = Instant::now();
        let deadline = now + Duration::from_secs(1);
        assert!(matches!(
            before_deadline(deadline, tokio::time::sleep_until(deadline)).await,
            BeforeDeadline::Expired
        ));

        let now = Instant::now();
        let deadline = now + Duration::from_secs(1);
        assert!(matches!(
            before_deadline(
                deadline,
                tokio::time::sleep_until(deadline + Duration::from_nanos(1))
            )
            .await,
            BeforeDeadline::Expired
        ));
    }

    #[tokio::test]
    async fn successful_response_stop_before_first_poll_is_latched() {
        let stop = CancellationToken::new();
        stop.cancel();
        let operation_polled = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&operation_polled);
        let operation = std::future::poll_fn(move |_context| {
            observed.store(true, Ordering::SeqCst);
            Poll::<()>::Pending
        });

        assert!(until_cancelled(&stop, operation).await.is_none());
        assert!(
            !operation_polled.load(Ordering::SeqCst),
            "latched cancellation wins before the handoff is first polled"
        );
    }

    #[tokio::test]
    async fn successful_response_stop_interrupts_a_pending_handoff() {
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let (polled_tx, polled_rx) = tokio::sync::oneshot::channel();
        let mut polled_tx = Some(polled_tx);
        let worker = tokio::spawn(async move {
            until_cancelled(
                &worker_stop,
                std::future::poll_fn(move |_context| {
                    if let Some(polled) = polled_tx.take() {
                        let _ = polled.send(());
                    }
                    Poll::<()>::Pending
                }),
            )
            .await
        });
        polled_rx.await.expect("the handoff is pending");

        stop.cancel();
        assert_eq!(worker.await.expect("worker joins"), None);
    }

    #[tokio::test]
    async fn answer_setup_failure_cancels_and_joins_its_retransmitter() {
        let stop = CancellationToken::new();
        let worker_stop = stop.clone();
        let finished = CancellationToken::new();
        let worker_finished = finished.clone();
        let mut owner = OwnedTask::new(tokio::spawn(async move {
            let _ = until_cancelled(&worker_stop, std::future::pending::<()>()).await;
            worker_finished.cancel();
        }));

        cancel_and_join(&stop, &mut owner).await;
        assert!(
            finished.is_cancelled(),
            "setup failure returns only after the retransmitter exits"
        );
    }

    struct TestRetired {
        entered: CancellationToken,
        release: CancellationToken,
    }

    impl Retirable for TestRetired {
        async fn finish(&self) {
            self.entered.cancel();
            self.release.cancelled().await;
        }
    }

    #[tokio::test]
    async fn cancelled_confirmed_replacement_retains_the_old_owner_for_retry() {
        let entered = CancellationToken::new();
        let release = CancellationToken::new();
        let mut retired = vec![TestRetired {
            entered: entered.clone(),
            release: release.clone(),
        }];
        let mut draining = Box::pin(drain_retired(&mut retired));
        tokio::select! {
            () = entered.cancelled() => {}
            () = &mut draining => panic!("retired generation completed before release"),
        }
        drop(draining);
        assert_eq!(retired.len(), 1, "cancellation preserved the old owner");
        assert_eq!(
            retired_media_snapshot_refusal(retired.len()),
            Some(DialogNotQuiescent::MediaCleanup),
            "snapshot capture refuses while cleanup ownership is retained"
        );

        release.cancel();
        drain_retired(&mut retired).await;
        assert!(retired.is_empty(), "retry joined and removed the old owner");
        assert_eq!(retired_media_snapshot_refusal(retired.len()), None);
    }

    /// M-49's pre-I/O boundary is pure: failure cannot have bound or gathered a socket.
    #[cfg(all(feature = "opus", feature = "dtls"))]
    #[test]
    fn browser_audio_preflight_is_fail_closed_before_io() {
        let policy = MediaPolicy::browser_audio();
        assert!(validate_profile_preflight(policy, TransportKind::Wss).is_ok());
        assert!(matches!(
            validate_profile_preflight(policy, TransportKind::Udp),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::InsecureSignalling
            ))
        ));
        assert!(matches!(
            validate_profile_preflight(policy.with_ice(IcePolicy::Disabled), TransportKind::Wss),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::IceRequired
            ))
        ));
        assert!(matches!(
            validate_profile_preflight(policy.with_keying(Keying::Plain), TransportKind::Wss),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::WeakerMedia
            ))
        ));
        let opus_only = Codecs::ordered(&[crate::CodecPreference::Opus]).expect("Opus build");
        assert!(matches!(
            validate_profile_preflight(policy.with_codecs(opus_only), TransportKind::Wss),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::CodecSetIncomplete
            ))
        ));
    }

    #[cfg(not(feature = "opus"))]
    #[test]
    fn browser_audio_reports_missing_opus_as_a_typed_pre_io_error() {
        assert!(matches!(
            validate_profile_preflight(MediaPolicy::browser_audio(), TransportKind::Wss),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::OpusUnavailable
            ))
        ));
    }

    #[cfg(all(feature = "opus", feature = "dtls"))]
    async fn browser_answer_fixture() -> (
        SessionDescription,
        SessionDescription,
        sipx_media::ice::LocalDescription,
        MediaPort,
    ) {
        let loopback: IpAddr = "127.0.0.1".parse().expect("loopback");
        let port = MediaPort::bind(SocketAddr::new(loopback, 0))
            .await
            .expect("offer port binds");
        let options = DialOptions::new("<sip:caller@example.invalid>", loopback)
            .with_media_policy(MediaPolicy::browser_audio());
        let (_capabilities, offer, local, _keying) =
            offered_media(&options, &port, TransportKind::Wss)
                .await
                .expect("browser offer gathers");
        let local = local.expect("browser offer retains ICE");
        let identity = sipx_media::dtls::openssl::Identity::generate().expect("answer identity");
        let fingerprint = identity.fingerprint().expect("answer fingerprint");
        let answer = sipx_sdp::browser_audio::answer(
            &offer,
            &sipx_sdp::browser_audio::BrowserAudioLocal {
                address: loopback,
                port: 40_000,
                session_id: 9_002,
                session_version: 1,
                direction: Direction::SendRecv,
                ice: sipx_sdp::ice::Credentials::new("peer", "peerPassword0123456789AB")
                    .expect("answer credentials"),
                candidates: vec![
                    sipx_sdp::ice::Candidate::parse(
                        "peer 1 UDP 2130706431 127.0.0.1 40000 typ host",
                    )
                    .expect("answer candidate"),
                ],
                fingerprint,
                setup: sipx_sdp::fingerprint::SetupCapabilities::both(),
            },
        )
        .expect("complete browser answer");
        (offer, answer, local, port)
    }

    /// M-70: the answering call constructs the media agent's remote description from the named
    /// profile result, not by reparsing fallback candidates the profile deliberately discarded.
    #[cfg(all(feature = "opus", feature = "dtls"))]
    #[tokio::test]
    async fn browser_answer_ice_retains_only_the_mux_component() {
        let (mut offer, _answer, _local, _port) = browser_answer_fixture().await;
        let mut fallback = offer.media[0]
            .ice_candidates()
            .into_iter()
            .next()
            .expect("generated offer has a component-one candidate");
        fallback.component = ComponentId::RTCP;
        fallback.port = offer.media[0]
            .port
            .checked_add(1)
            .expect("ephemeral media port has a fallback port");
        offer.media[0].attributes.push(sipx_sdp::Attribute::valued(
            "candidate",
            fallback.to_value(),
        ));

        let negotiation = answer_ice_negotiation(&offer, MediaPolicy::browser_audio())
            .expect("mux offer is accepted");
        let IceNegotiation::Ice { candidates, .. } = negotiation else {
            panic!("browser profile must retain an ICE generation");
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].component, ComponentId::RTP);
    }

    /// `M-49`: an incomplete final answer is refused at the call boundary before the retained ICE
    /// description accepts the peer half or any media owner can start.
    #[cfg(all(feature = "opus", feature = "dtls"))]
    #[tokio::test]
    async fn browser_answer_is_fully_validated_before_ice_state_changes() {
        let (offer, mut answer, local, _port) = browser_answer_fixture().await;
        answer.media[0]
            .attributes
            .retain(|attribute| attribute.name != "rtcp-mux");
        let ice_before = format!("{local:?}");

        assert!(matches!(
            validate_establishment_answer(
                MediaProfile::BrowserAudio,
                offer.to_string_sdp().as_bytes(),
                &answer,
            ),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::RtcpMuxRequired
            ))
        ));
        assert_eq!(
            format!("{local:?}"),
            ice_before,
            "a refused answer did not reach LocalDescription::accept"
        );
    }

    /// Generic codec negotiation permits an answer to change preference order; the named profile
    /// does not. The call boundary therefore uses the complete exchange validator, not only the
    /// parser for the answer in isolation.
    #[cfg(all(feature = "opus", feature = "dtls"))]
    #[tokio::test]
    async fn browser_answer_cannot_reorder_the_payloads_selected_by_the_offer() {
        let (offer, mut answer, local, _port) = browser_answer_fixture().await;
        answer.media[0].formats.swap(0, 1);
        let ice_before = format!("{local:?}");

        assert!(matches!(
            validate_establishment_answer(
                MediaProfile::BrowserAudio,
                offer.to_string_sdp().as_bytes(),
                &answer,
            ),
            Err(Error::Profile(
                sipx_sdp::browser_audio::ProfileError::CodecSetIncomplete
            ))
        ));
        assert_eq!(
            format!("{local:?}"),
            ice_before,
            "a refused answer did not reach LocalDescription::accept"
        );
    }

    /// The call boundary does not inherit the generic parser's extensible-candidate tolerance.
    /// Every browser-profile line must belong to the bounded host/server-reflexive set before the
    /// retained ICE generation is allowed to see any of them.
    #[cfg(all(feature = "opus", feature = "dtls"))]
    #[tokio::test]
    async fn browser_answer_rejects_every_unusable_candidate_before_ice_acceptance() {
        let (offer, answer, local, _port) = browser_answer_fixture().await;
        let ice_before = format!("{local:?}");
        for candidate in [
            "not a candidate",
            "relay 1 UDP 2130706430 127.0.0.1 40001 typ relay raddr 0.0.0.0 rport 9",
            "prflx 1 UDP 2130706429 127.0.0.1 40002 typ prflx raddr 0.0.0.0 rport 9",
        ] {
            let mut rejected = answer.clone();
            rejected.media[0]
                .attributes
                .push(sipx_sdp::Attribute::valued("candidate", candidate));
            assert!(matches!(
                validate_establishment_answer(
                    MediaProfile::BrowserAudio,
                    offer.to_string_sdp().as_bytes(),
                    &rejected,
                ),
                Err(Error::Profile(
                    sipx_sdp::browser_audio::ProfileError::IceRequired
                ))
            ));
            assert_eq!(
                format!("{local:?}"),
                ice_before,
                "candidate refusal changed retained ICE state: {candidate}"
            );
        }
    }

    const IDENTIFIER_SAMPLE_SIZE: u64 = 4096;

    fn bit_counts(values: impl IntoIterator<Item = u64>) -> [usize; 64] {
        let mut counts = [0; 64];
        for value in values {
            for (bit, count) in counts.iter_mut().enumerate() {
                *count += usize::from(value & (1_u64 << bit) != 0);
            }
        }
        counts
    }

    /// Both reliable-provisional shapes use the same address pair: an offer in a 183 for an
    /// offerless INVITE, and an answer in a 183 for an INVITE carrying an offer.
    #[tokio::test]
    async fn early_media_binds_locally_and_advertises_the_chosen_address_in_both_roles() {
        let advertised: IpAddr = "198.51.100.44".parse().expect("valid");
        let bind: IpAddr = "127.0.0.1".parse().expect("valid");
        let addresses = MediaAddress::new(advertised).with_bind(bind);

        let offered_early = EarlyOffer::bind(
            addresses,
            false,
            Direction::SendRecv,
            MediaPolicy::default(),
        )
        .await
        .expect("binds an early offer");
        assert_eq!(offered_early.port.local_addr().ip(), bind);
        assert_eq!(
            offered_early.description().connection,
            Some(Connection::new(advertised))
        );

        let remote_offer = offered("0", &[]);
        let (answered_early, answer) =
            Early::settle(addresses, false, &remote_offer, MediaPolicy::default())
                .await
                .expect("binds an early answer");
        assert_eq!(answered_early.media.local_addr().ip(), bind);
        assert_eq!(answer.connection, Some(Connection::new(advertised)));
    }

    /// RFC 3261 §19.3 makes a dialog tag a peer-visible identifier which must be hard to guess.
    /// Every hexadecimal position is sampled, rather than accepting a 64-bit-looking string whose
    /// high half is fixed by truncation or by a counter.
    #[test]
    fn dialog_tag_keeps_all_sixty_four_random_bits() {
        let values = (0..IDENTIFIER_SAMPLE_SIZE).map(|_| {
            let tag = token();
            assert_eq!(tag.len(), 16, "exactly 64 bits in hexadecimal");
            assert!(
                tag.bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                "the tag is canonical lowercase hexadecimal: {tag}"
            );
            u64::from_str_radix(&tag, 16).expect("the generator wrote hexadecimal")
        });
        for (bit, ones) in bit_counts(values).iter().copied().enumerate() {
            assert!(
                (1664..=2432).contains(&ones), // 128 positions * 2 * exp(-2 * 384^2 / 4096) < 1.4e-29.
                "dialog tag bit {bit} had {ones} ones in {IDENTIFIER_SAMPLE_SIZE} samples"
            );
        }
    }

    /// `token_with_rng` makes the source property part of type checking, not an inference from a
    /// finite sample. A deterministic `RngCore` without `CryptoRng` cannot instantiate this call.
    #[test]
    fn dialog_tag_requires_a_cryptographic_rng_by_construction() {
        fn draw<R: rand::CryptoRng + ?Sized>(rng: &mut R) -> String {
            token_with_rng(rng)
        }

        assert_eq!(draw(&mut rand::rng()).len(), 16);
    }

    #[test]
    fn the_initial_call_offer_requests_rtcp_mux() {
        let (capabilities, _keying) = media_capabilities(
            MediaPolicy::default(),
            "127.0.0.1".parse().expect("loopback address"),
            40_000,
            false,
        )
        .expect("default media capabilities");
        let offer = offer_from(&capabilities);

        assert!(capabilities.rtcp_mux);
        assert!(offer.media.first().expect("audio offer").rtcp_mux());
    }

    /// Session-level RFC 4145 roles are resolved identically by both call roles, including the
    /// passive answer that makes the offerer the DTLS client.
    #[test]
    fn session_level_setup_selects_the_complementary_call_role() {
        let mut offer = offered("0", &["0 PCMU/8000"]);
        offer
            .attributes
            .push(sipx_sdp::Attribute::valued("setup", "actpass"));
        assert_eq!(
            dtls_local_setup(&offer, true).expect("answerer role"),
            sipx_sdp::fingerprint::Setup::Active
        );

        for (answer, expected) in [
            ("active", sipx_sdp::fingerprint::Setup::Passive),
            ("passive", sipx_sdp::fingerprint::Setup::Active),
        ] {
            let mut description = offered("0", &["0 PCMU/8000"]);
            description
                .attributes
                .push(sipx_sdp::Attribute::valued("setup", answer));
            assert_eq!(
                dtls_local_setup(&description, false).expect("offerer role"),
                expected
            );
        }
    }

    /// `SETUP-2` through the actual call/media boundary: a passive answer makes the sipx offerer
    /// run the DTLS client handshake, and a real server completes it with both fingerprints
    /// verified before the returned media session starts.
    #[cfg(feature = "dtls")]
    #[tokio::test]
    async fn a_passive_answer_wires_the_offerer_as_the_dtls_client() {
        let client_port = MediaPort::bind("127.0.0.1:0".parse().expect("client address"))
            .await
            .expect("binds client");
        let server_port = MediaPort::bind("127.0.0.1:0".parse().expect("server address"))
            .await
            .expect("binds server");
        let client_address = client_port.local_addr();
        let server_address = server_port.local_addr();
        let client_identity =
            sipx_media::dtls::openssl::Identity::generate().expect("client identity");
        let server_identity =
            sipx_media::dtls::openssl::Identity::generate().expect("server identity");
        let client_fingerprint = client_identity.fingerprint().expect("client fingerprint");
        let server_fingerprint = server_identity.fingerprint().expect("server fingerprint");

        let mut passive_answer = offered("0", &["0 PCMU/8000"]);
        passive_answer.attributes.extend([
            sipx_sdp::Attribute::valued("setup", "passive"),
            sipx_sdp::Attribute::valued("fingerprint", server_fingerprint.to_value()),
        ]);
        let settled = Settled {
            negotiated: Negotiated {
                remote: server_address,
                codec: Codec::Pcmu,
                clock_rate: 8_000,
                payload_type: Some(0),
                receive_payload_type: Some(0),
                dtmf: None,
                rtcp_mode: sipx_sdp::RtcpMode::Separate,
            },
            srtp: None,
        };
        let handshake_bound = Duration::from_secs(5); // Bounds a failed handshake; not ordering.
        let server = server_port.key_with_dtls(
            server_identity,
            client_address,
            sipx_media::dtls::Role::Server,
            client_fingerprint,
            handshake_bound,
        );
        let client = key_and_start(
            client_port,
            None,
            settled,
            PendingKeying::Dtls(client_identity),
            &passive_answer,
            false,
            MediaProfile::Standard,
        );

        let (server, client) = tokio::join!(server, client);
        let (_server_port, _server_keys) = server.expect("server handshake completes");
        let (client_session, client_settled) = client.expect("offerer handshake completes");
        assert!(client_settled.is_encrypted());
        client_session.stop();
    }

    /// A `holdconn` DTLS offer is rejected by the call preflight before the answering path can
    /// bind a media port or send a successful response.
    #[test]
    fn a_holdconn_dtls_offer_is_a_typed_pre_response_refusal() {
        let mut offer = offered("0", &["0 PCMU/8000"]);
        offer
            .attributes
            .push(sipx_sdp::Attribute::valued("setup", "holdconn"));
        let policy = MediaPolicy::default().with_keying(Keying::DtlsSrtp);

        assert!(matches!(
            validate_dtls_offer_setup(&offer, policy),
            Err(Error::DtlsSetup(
                sipx_sdp::fingerprint::SetupRoleError::UnresolvedOffer(
                    sipx_sdp::fingerprint::Setup::HoldConn
                )
            ))
        ));
    }

    /// An initial mux offer retains component 2 and advertises its explicit `a=rtcp` destination,
    /// so an answer omitting mux can take the fallback without a second offer.
    #[tokio::test]
    async fn an_initial_mux_ice_offer_carries_the_control_fallback() {
        let loopback: IpAddr = "127.0.0.1".parse().expect("loopback");
        let port = MediaPort::bind(SocketAddr::new(loopback, 0))
            .await
            .expect("binds media");
        let options = DialOptions::new("<sip:caller@example.invalid>", loopback)
            .with_media_policy(MediaPolicy::default().with_ice(IcePolicy::Host));
        let (_capabilities, offer, local, _keying) =
            offered_media(&options, &port, TransportKind::Udp)
                .await
                .expect("gathers an offer");
        let local = local.expect("ICE description");
        let audio = offer.media.first().expect("audio offer");

        assert!(audio.rtcp_mux(), "mux is offered");
        assert!(
            local
                .candidates()
                .iter()
                .any(|candidate| candidate.component == ComponentId::RTCP),
            "the initial offer retains the component-2 fallback"
        );
        assert!(
            audio.attribute("rtcp").is_some(),
            "the fallback control destination is explicit"
        );
    }

    /// Once an answer agrees to mux, its local ICE half contains component 1 alone.
    #[tokio::test]
    async fn a_muxed_ice_answer_gathers_one_component() {
        let mut offer = offered("0", &["0 PCMU/8000"]);
        let audio = offer.media.first_mut().expect("audio offer");
        audio.attributes.extend([
            sipx_sdp::Attribute::flag("rtcp-mux"),
            sipx_sdp::Attribute::valued("ice-ufrag", "peer"),
            sipx_sdp::Attribute::valued("ice-pwd", "peerPassword0123456789AB"),
            sipx_sdp::Attribute::valued("candidate", "1 1 UDP 2130706431 192.0.2.1 40000 typ host"),
        ]);
        let port = MediaPort::bind("127.0.0.1:0".parse().expect("loopback"))
            .await
            .expect("binds media");
        let (_remote, local) = answer_gathering(
            &port,
            &offer,
            MediaPolicy::default().with_ice(IcePolicy::Host),
        )
        .await
        .expect("gathers answer");
        let local = local.expect("ICE description");

        assert_eq!(local.candidates().len(), 1);
        assert_eq!(local.candidates()[0].component, ComponentId::RTP);
        assert_eq!(local.default_destination(ComponentId::RTCP), None);
    }
}
