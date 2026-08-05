//! Establishing a call: INVITE with an SDP offer, media bound to the answer, and BYE.

use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

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
use crate::error::{Error, Result};
use crate::event::{CallEvent, CallEvents, EndCause, EventSink};
use crate::identity::OutboundIdentityPolicy;
use crate::media_policy::{Codecs, IcePolicy, Keying, MediaPolicy, MediaProfile, NegotiatedKeying};
use crate::snapshot::{
    DialogNotQuiescent, DialogPersistenceError, DialogRestoreContext, DialogSnapshot,
    SessionSnapshot, SnapshotParts,
};
use crate::transfer::{
    Referral, Replaces, Transfer, TransferState, is_terminated, parse_sipfrag, sipfrag,
};

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
    endpoint: Handle,
    /// Where in-dialog requests go: the peer's `Contact`, not where the INVITE was sent.
    target: Target,
    /// Set while a 2xx is still being retransmitted; cleared when the ACK arrives.
    awaiting_ack: Option<Arc<tokio::sync::Notify>>,
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
}

/// The pure half of accepting a peer's in-dialog offer.
///
/// Kept as one value so [`Call::can_accept_offer`] and [`Call::renegotiate`] cannot drift: the
/// coupling asks the first question before it changes its other leg, and the call later applies
/// exactly the description that passed that check.
struct PreparedRenegotiation {
    offer: SessionDescription,
    negotiated: Negotiated,
    answer: SessionDescription,
    direction: Direction,
}

/// Refuse a socket-ownership change until renegotiation can replace the media session atomically.
fn preserve_rtcp_mode(current: sipx_sdp::RtcpMode, proposed: sipx_sdp::RtcpMode) -> Result<()> {
    if current == proposed {
        Ok(())
    } else {
        Err(Error::RtcpModeChange { current, proposed })
    }
}

/// The mode one answer selected for its corresponding offered audio section.
fn exchanged_rtcp_mode(
    offer: &SessionDescription,
    answer: &SessionDescription,
) -> sipx_sdp::RtcpMode {
    offer
        .media
        .iter()
        .zip(&answer.media)
        .find(|(offered, _)| offered.media == "audio")
        .map_or(sipx_sdp::RtcpMode::Separate, |(offered, answered)| {
            sipx_sdp::RtcpMode::from_exchange(offered, answered)
        })
}

/// The RTCP shape this implementation will select when answering `offer`.
fn answering_rtcp_mode(offer: &SessionDescription) -> sipx_sdp::RtcpMode {
    offer
        .media
        .iter()
        .find(|media| media.media == "audio" && !media.is_rejected())
        .filter(|media| media.rtcp_mux())
        .map_or(sipx_sdp::RtcpMode::Separate, |_| sipx_sdp::RtcpMode::Mux)
}

/// A negotiated session timer and the deadline it is currently counting down to.
#[derive(Debug, Clone, Copy)]
struct SessionState {
    terms: session::Session,
    /// When [`Call::on_session_deadline`] should be called.
    ///
    /// Held as an absolute instant rather than recomputed from "now" on each poll, so that a
    /// call driven by a loop that also does other work cannot have its timer pushed back
    /// indefinitely by its own busyness.
    act_at: Instant,
}

impl SessionState {
    fn armed(terms: session::Session) -> Self {
        Self {
            terms,
            act_at: Instant::now() + terms.act_after(),
        }
    }
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
        if self.awaiting_ack.is_some() {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::AwaitingAck,
            ));
        }
        if !self.negotiation.is_idle() {
            return Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::OfferAnswer,
            ));
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
            payload_type: self.current.wire_payload_type(),
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
            endpoint: context.endpoint.clone(),
            target: context.target.clone(),
            awaiting_ack: None,
            ended: false,
            media_address: context.media_address.advertised(),
            media_bind_address: context.media_address.bind(),
            codecs: snapshot.codecs_value(),
            profile: snapshot.media_profile_value(),
            current: snapshot.negotiated(context.remote_media),
            peer_ice: None,
            hold: snapshot.hold_value(),
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
            events,
            events_rx: Some(events_rx),
            history: None,
        })
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
        let rate = u64::from(self.media.codec().clock_rate()).max(1);
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

    /// RTP payload type selected for the established audio codec.
    #[must_use]
    pub fn negotiated_payload_type(&self) -> u8 {
        self.current.wire_payload_type()
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
                if let Some(notify) = self.awaiting_ack.take() {
                    notify.notify_waiters();
                }
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
                if let Some(notify) = self.awaiting_ack.take() {
                    notify.notify_waiters();
                }
                // Emitted here, at the point `ended` actually flips, rather than after the 200
                // OK below — the call is over the moment the far end's BYE is accepted, whether
                // or not building or sending the response then succeeds.
                self.events.end(EndCause::RemoteBye);
                let response =
                    ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?.build();
                self.endpoint.respond(&incoming.key, response).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// The negotiated session interval, and whether this side is the one refreshing it.
    ///
    /// `None` means no timer was agreed, so nothing will ever notice a far end that stops
    /// answering — worth being able to check, because that is a property of the *peer*, not of
    /// what this side asked for.
    #[must_use]
    pub fn session_interval(&self) -> Option<(Duration, bool)> {
        self.session
            .map(|state| (state.terms.interval, state.terms.we_refresh))
    }

    /// The diversion history received while this call was established.
    #[must_use]
    pub fn history(&self) -> Option<&HistoryInfo> {
        self.history.as_ref()
    }

    /// When [`Self::on_session_deadline`] next needs to be called, if a timer was negotiated.
    ///
    /// Returned as an instant rather than as a future on purpose. A future would borrow the
    /// call for as long as it was being awaited, which is exactly the borrow
    /// [`Self::handle`] needs in the other arm of the `select!` this is written for.
    #[must_use]
    pub fn session_deadline(&self) -> Option<Instant> {
        self.session.map(|state| state.act_at)
    }

    /// Do whatever the session timer's deadline asked for (RFC 4028 §10).
    ///
    /// For the refresher that is an UPDATE or a re-INVITE — whichever the peer's `Allow` says
    /// it can take (§7.4); for the other side it is a BYE,
    /// because nothing arrived and the far end is presumed gone. Calling this early is harmless
    /// — it re-reads the deadline and does nothing if it has not passed.
    pub async fn on_session_deadline(&mut self) -> Result<()> {
        let Some(state) = self.session else {
            return Ok(());
        };
        if Instant::now() < state.act_at {
            return Ok(());
        }
        if !state.terms.we_refresh {
            // §10: the side that is not refreshing "SHOULD send a BYE to terminate the
            // session". The media stops with it — a half-torn-down call that keeps streaming
            // is the failure this whole mechanism exists to end, not a gentler version of it.
            self.end(EndCause::Timeout).await?;
            return Err(Error::SessionExpired);
        }
        match self.refresh_session().await {
            Ok(()) => Ok(()),
            // §10: a refresh that times out or draws a 408 or 481 means the dialog is gone at
            // the far end, and RFC 3261 §12.2.1.2 says to BYE. Any other failure is about the
            // refresh, not the call: a 491 glare or a 500 leaves the session running until the
            // deadline we do not move, so the next attempt is the retry.
            Err(Error::NoResponse) => {
                self.end(EndCause::Timeout).await?;
                Err(Error::SessionExpired)
            }
            Err(Error::Rejected { status, reason }) => {
                const REQUEST_TIMEOUT: u16 = 408;
                const NO_SUCH_DIALOG: u16 = 481;
                if status == REQUEST_TIMEOUT || status == NO_SUCH_DIALOG {
                    self.end(EndCause::Timeout).await?;
                    return Err(Error::SessionExpired);
                }
                // Push the retry out so a peer answering 500 to every refresh is not asked
                // again immediately for the rest of the session interval.
                self.rearm();
                Err(Error::Rejected { status, reason })
            }
            Err(other) => {
                self.rearm();
                Err(other)
            }
        }
    }

    /// Restart the countdown, because the session was refreshed.
    fn rearm(&mut self) {
        if let Some(state) = self.session.as_mut() {
            state.act_at = Instant::now() + state.terms.act_after();
        }
    }

    /// Whether the far end has put the call on hold.
    #[must_use]
    pub fn is_on_hold(&self) -> bool {
        !self.hold.receives()
    }

    /// Renegotiate an established call from a re-INVITE.
    ///
    /// The rule that shapes this: **a renegotiation that fails must leave the call running.** A
    /// re-INVITE tries to change something about a call that already works, so answering 488
    /// and carrying on is right; tearing the call down because the new offer was unusable
    /// would lose a call that was fine a moment ago.
    async fn on_reinvite(&mut self, incoming: &Incoming) -> Result<()> {
        if self.out_of_order(&incoming.request) {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        self.record_remote_cseq(&incoming.request);

        // §5.2 rule 2's other source, and the reason the spec names INVITE alongside UPDATE: a
        // re-INVITE's offer is one this side owes an answer to until it produces one.
        if crate::update::carries_offer(&incoming.request) {
            self.negotiation.received_offer();
        }
        let renegotiated = self.renegotiate(incoming.request.body()).await;
        // On every path out of here the debt is settled: a 488 kills the offer and a 2xx
        // answers it, and a failure to renegotiate at all leaves nothing to answer.
        self.negotiation.sent_answer();
        let Some(answer_sdp) = renegotiated? else {
            return self.refuse_unacceptable(incoming).await;
        };

        // RFC 4028 §7.2: any re-INVITE inside the dialog refreshes the session, whether or not
        // it was sent for that reason. Only counting the ones that carry `Session-Expires`
        // would hang up on a peer that is demonstrably alive and talking to us.
        self.rearm();

        // RFC 3261 §12.2.2: a re-INVITE is a target refresh request, so its `Contact` replaces
        // the dialog's remote target. Without this the BYE still goes to where the peer was
        // when the call started, and a peer that has moved can never be told it is over.
        self.dialog.refresh_target(&incoming.request.headers);
        self.target = in_dialog_target(
            &self.dialog,
            Target::new(incoming.source, incoming.transport),
        );

        let response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .body(Bytes::from(answer_sdp.to_string_sdp()))
            .build();
        self.endpoint
            .respond(&incoming.key, response.clone())
            .await?;

        // RFC 3261 §13.3.1.4 applies to the 2xx of *any* INVITE, not only the first: it is
        // retransmitted until the ACK arrives. The server transaction deliberately absorbs
        // retransmitted INVITEs without answering them again (RFC 6026), so if the TU does not
        // resend, one lost 200 deadlocks the renegotiation until the peer's Timer B — a single
        // dropped packet breaking hold and resume for half a minute.
        if let Some(previous) = self.awaiting_ack.take() {
            previous.notify_waiters();
        }
        let acked = Arc::new(tokio::sync::Notify::new());
        tokio::spawn(retransmit_until_acked(
            self.endpoint.clone(),
            incoming.key.clone(),
            response,
            Arc::clone(&acked),
        ));
        self.awaiting_ack = Some(acked);
        Ok(())
    }

    /// Apply an offer that arrived in-dialog, and produce the answer to send back.
    ///
    /// `None` means the description is unusable and the caller must refuse — 488 for a
    /// re-INVITE (`M-8`) and for an UPDATE (RFC 3311 §5.2), which is the same rule for the same
    /// reason: **a renegotiation that fails leaves the call running.** Both requests try to
    /// change something that already works, so refusing the change and keeping the session is
    /// right; tearing the call down because the new offer was unusable would lose a call that
    /// was fine a moment ago.
    ///
    /// Shared by the two paths because they ask exactly the same question of exactly the same
    /// session and differ only in what carries the answer back.
    fn prepare_renegotiation(&self, body: &[u8]) -> Option<PreparedRenegotiation> {
        if self.keying == Keying::DtlsSrtp {
            return None;
        }
        let offer = sipx_sdp::parse(&String::from_utf8_lossy(body)).ok()?;
        let mut negotiated = negotiated(&offer, self.codecs).ok()?;

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        let answer = sipx_sdp::answer(&offer, &capabilities);
        if answer
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return None;
        }
        let proposed_mode = exchanged_rtcp_mode(&offer, &answer);
        // A muxed session owns one receive socket. Answering an offer that removed mux while
        // silently retaining that owner would put the wire and the running state in disagreement.
        // Refuse the offer with 488; the typed error is shared with the outbound paths below.
        preserve_rtcp_mode(self.current.rtcp_mode, proposed_mode).ok()?;
        negotiated.rtcp_mode = proposed_mode;
        let direction = offer
            .media
            .iter()
            .find(|media| media.media == "audio" && !media.is_rejected())
            .map(sipx_sdp::MediaDescription::direction)?;
        Some(PreparedRenegotiation {
            offer,
            negotiated,
            answer,
            direction,
        })
    }

    /// Whether this call can answer an in-dialog offer, without changing call or media state.
    ///
    /// The coupling uses this before opening an exchange on its other leg. Syntax alone is not
    /// enough: the source call may have no common codec or may use DTLS-SRTP, whose renegotiation
    /// this layer deliberately refuses.
    pub(crate) fn can_accept_offer(&self, body: &[u8]) -> Option<Direction> {
        self.prepare_renegotiation(body)
            .map(|prepared| prepared.direction)
    }

    async fn renegotiate(&mut self, body: &[u8]) -> Result<Option<SessionDescription>> {
        let Some(mut prepared) = self.prepare_renegotiation(body) else {
            return Ok(None);
        };
        self.answer_ice(&prepared.offer, &mut prepared.answer).await;

        // Hold is a direction, not a separate state: `sendonly` or `inactive` from the far end
        // means it will not play what we send.
        let was_on_hold = self.is_on_hold();
        self.hold = prepared.direction;
        // Emitted right where `hold` changes, not by polling it afterwards — a renegotiation
        // that does not change the direction (a keep-alive, say) must not report a hold that
        // never happened.
        match (was_on_hold, self.is_on_hold()) {
            (false, true) => self.events.emit(CallEvent::Hold),
            (true, false) => self.events.emit(CallEvent::Resumed),
            _ => {}
        }

        self.move_media_if_changed(prepared.negotiated).await?;
        Ok(Some(prepared.answer))
    }

    /// Give the running agent the ICE half of an answer to one of our later offers.
    ///
    /// The offering side's mirror of [`Self::answer_ice`], and it signals nothing: the answer is
    /// the end of this exchange, so what comes back from the agent has no description left to go
    /// into. What matters is that the agent hears it at all — a restart this side offered is only
    /// half a restart until the peer's new credentials and candidates arrive.
    async fn accept_answer_ice(&mut self, answer: &SessionDescription) {
        if !self.media.runs_ice() {
            return;
        }
        let peer = answer
            .media
            .first()
            .map_or(IceNegotiation::Absent, |audio| {
                sipx_media::ice::negotiate(answer, audio)
            });
        // Recorded for the same reason the initial exchange records it: the next offer from the
        // peer is a restart only if it differs from what was last seen, and an answer is a
        // description like any other.
        self.peer_ice_restarted(&peer);
        // discard: this is the media path, and M12's clause is about the signalling one — the
        // counters for a media session that could not apply a renegotiation are `M-32`, which is
        // why `sipx-media` is not in the guard's `CRATES`. Nothing signalling is lost here in any
        // case: the peer's ICE half was recorded on the line above, which is what the *next* offer
        // is compared against, and a renegotiation that does not take leaves the candidate pair
        // already carrying the call in use.
        let _ = self.media.renegotiate_ice(None, Some(&peer)).await;
    }

    /// Put this side's ICE half into a later offer (RFC 8839 §4.4; `ice.md` §13.5).
    ///
    /// The offering counterpart of [`Self::answer_ice`], and it carries the same rule: a stream
    /// doing ICE restates its half in **every** subsequent offer, because §6 makes their absence
    /// mean this side has stopped. [`Self::restart_ice`] is the one caller that also draws new
    /// credentials, and drawing them is the whole of what it does — §4.4.1.1.1 says both values
    /// changing *is* the restart, so there is no second flag to set on the wire.
    async fn offer_ice(&mut self, offer: &mut SessionDescription, ice: IceOffer) {
        if !self.media.runs_ice() {
            return;
        }
        let local = match ice {
            IceOffer::Continue => None,
            IceOffer::Restart => fresh_ice_parameters(),
        };
        // No peer half: this is an offer, and the answer that responds to it comes back through
        // `Dialing`/`renegotiate` like any other.
        let Some(signalled) = self.media.renegotiate_ice(local, None).await else {
            return;
        };
        let Some(audio) = offer.media.first_mut() else {
            return;
        };
        audio
            .attributes
            .retain(|attribute| !is_ice_attribute(attribute));
        audio.attributes.extend(ice_attributes(&signalled));
    }

    /// Put this side's ICE half into the answer to a later offer (RFC 8839 §4.4; `ice.md` §13.5).
    ///
    /// Three things happen here and they are one operation because they must not be reordered:
    /// the offer is read for the peer's half, this side takes new parameters when §4.4.1.1.1 says
    /// the offer is a restart, and both are handed to the running agent — which is what decides
    /// whether the session is rebuilt. What comes back is what this answer signals.
    ///
    /// **A stream doing ICE re-signals on every exchange**, not only on a restart. §6 makes the
    /// absence of `candidate` attributes mean the peer has stopped doing ICE, so an answer that
    /// dropped them mid-call would tell the far end to fall back to symmetric RTP on a path it had
    /// already agreed to check. Hold, resume, a codec change and a session refresh all come
    /// through here, and none of them is a restart.
    ///
    /// A call with no agent is left exactly as it was: no attributes, no round trip to a driver
    /// that does not exist.
    async fn answer_ice(&mut self, offer: &SessionDescription, answer: &mut SessionDescription) {
        if !self.media.runs_ice() {
            return;
        }
        let peer = offer.media.first().map_or(IceNegotiation::Absent, |audio| {
            sipx_media::ice::negotiate(offer, audio)
        });
        // §4.4.1.1.1 is a question about the *peer's* two credentials, and this side answers it
        // only to know whether to draw its own new ones. The agent asks it again for itself, from
        // the credentials it is actually keyed to; see `MediaSession::renegotiate_ice`.
        let local = self
            .peer_ice_restarted(&peer)
            .then(fresh_ice_parameters)
            .flatten();
        let Some(signalled) = self.media.renegotiate_ice(local, Some(&peer)).await else {
            return;
        };
        let Some(audio) = answer.media.first_mut() else {
            return;
        };
        audio
            .attributes
            .retain(|attribute| !is_ice_attribute(attribute));
        audio.attributes.extend(ice_attributes(&signalled));
    }

    /// Whether this offer restarts ICE (RFC 8839 §4.4.1.1.1).
    ///
    /// **Both** credentials changed, and only both. One alone is not a restart, which is the case
    /// the rule is worded to exclude: a peer may legitimately re-send a description with one value
    /// re-derived and the other unchanged, and treating that as a restart would tear down a
    /// working session for nothing.
    ///
    /// The comparison is against what this side last *saw*, which is why it is recorded here
    /// rather than derived from the SDP twice — "the same value moving between the session level
    /// and the media level is not a restart" is only true if what is compared is the effective
    /// value for the stream, which is what [`sipx_media::ice::negotiate`] resolves.
    fn peer_ice_restarted(&mut self, peer: &IceNegotiation) -> bool {
        let IceNegotiation::Ice { credentials, .. } = peer else {
            return false;
        };
        let restarted = self.peer_ice.as_ref().is_some_and(|seen| {
            seen.ufrag() != credentials.ufrag() && seen.pwd() != credentials.pwd()
        });
        self.peer_ice = Some(credentials.clone());
        restarted
    }

    /// Answer an UPDATE that arrived in this dialog (RFC 3311 §5.2).
    ///
    /// The three refusals are three different answers, and the difference is the point: 491
    /// means the two sides collided and both should wait a randomised interval before trying
    /// again; a 500 with `Retry-After` means the request was well formed and simply early. A
    /// peer told the wrong one either backs off when it did not need to or retries straight
    /// into the same wall.
    ///
    /// Whichever it is, **the dialog survives** — including the 488 for a description this side
    /// cannot use. Every one of these is about a change that will not happen, not about the
    /// session that is already running.
    async fn on_update(&mut self, incoming: &Incoming) -> Result<()> {
        if self.out_of_order(&incoming.request) {
            return self.refuse(incoming, 500, "Server Internal Error").await;
        }
        self.record_remote_cseq(&incoming.request);

        let has_offer = crate::update::carries_offer(&incoming.request);
        if let Reception::Refuse(refusal) = self.negotiation.receive(has_offer) {
            return crate::update::refuse(&self.endpoint, incoming, refusal).await;
        }

        let mut builder = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?;

        if has_offer {
            // §5.2: the UAS "MUST adjust the session parameters accordingly and generate an
            // answer in the 2xx response".
            //
            // The result is captured rather than propagated with `?`, because `renegotiate`
            // can fail on something that has nothing to do with the peer — a media port that
            // will not bind. Returning through the `?` would leave this UPDATE forever in
            // progress and the offer forever owed, and every later UPDATE on the dialog would
            // draw §5.2's "you are too early" for a transaction nobody is waiting on.
            let renegotiated = self.renegotiate(incoming.request.body()).await;
            let Some(answer_sdp) = renegotiated.inspect_err(|_| self.negotiation.answered())?
            else {
                // The offer is dead, so nothing is owed for it any more — and this is a final
                // response, so no UPDATE is in progress either.
                self.negotiation.answered();
                return self.refuse_unacceptable(incoming).await;
            };
            builder = builder
                .header(
                    HeaderName::ContentType,
                    Bytes::from_static(b"application/sdp"),
                )?
                .body(Bytes::from(answer_sdp.to_string_sdp()));
        }

        // RFC 4028 §7.4: an UPDATE refreshes the session whether or not it was sent for that
        // reason, so the 2xx names the terms in force and the deadline moves. Only counting the
        // ones that carry `Session-Expires` would hang up on a peer that is demonstrably alive.
        if let Some(state) = self.session {
            let expires = SessionExpires {
                interval: state.terms.interval,
                refresher: Some(if state.terms.we_refresh {
                    session::Refresher::Uas
                } else {
                    session::Refresher::Uac
                }),
            };
            builder = builder
                .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
                .header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        }

        // §5.1: UPDATE is a target refresh request, so its `Contact` replaces the dialog's
        // remote target — the same rule RFC 3261 §12.2.2 gives a re-INVITE, and for the same
        // reason: without it the BYE goes to where the peer used to be.
        self.dialog.refresh_target(&incoming.request.headers);
        self.target = in_dialog_target(
            &self.dialog,
            Target::new(incoming.source, incoming.transport),
        );
        self.peer_allows_update = update::peer_allows(&incoming.request.headers);

        let sent = self.endpoint.respond(&incoming.key, builder.build()).await;
        // Cleared whether or not the response got out. A send that failed will not be retried
        // here, so leaving the exchange open would answer every later UPDATE on this dialog
        // with §5.2's "you are too early" — permanently, for a transaction nobody is waiting
        // on any more.
        self.negotiation.answered();
        sent?;
        self.rearm();
        Ok(())
    }

    /// Renegotiate this call with an UPDATE (RFC 3311).
    ///
    /// [`Self::reinvite`] remains the right way to renegotiate a *confirmed* dialog — §5.1
    /// recommends it, because an UPDATE must be answered promptly and leaves the far end no
    /// window in which to ask a user whether the change is acceptable. This is here for the
    /// cases where that does not apply: a peer that asked for UPDATE, or a change that nobody
    /// would be asked about.
    ///
    /// Refuses locally rather than putting an illegal request on the wire when an offer of ours
    /// is unanswered or one of theirs is unanswered by us (§5.1, RFC 3264): the far end would
    /// answer 491 or 500 and the round trip would have told us only what we already knew.
    pub async fn update(&mut self, direction: Direction) -> Result<()> {
        if self.profile == MediaProfile::BrowserAudio {
            return Err(sipx_sdp::browser_audio::ProfileError::ProfileRemoved.into());
        }
        if self.keying == Keying::DtlsSrtp {
            return Err(Error::DtlsRenegotiation);
        }
        if !self.negotiation.may_offer() {
            return Err(Error::Rejected {
                status: sipx_sip::update::Refusal::Glare.status(),
                reason: "an offer is already outstanding on this dialog".to_owned(),
            });
        }

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        capabilities.direction = direction;
        // As for a re-INVITE: the version must increase with each modified offer, so the far
        // end can tell a changed description from a repeated one.
        capabilities.session_version = u64::from(self.dialog.local_cseq.saturating_add(1));
        let offer = offer_from(&capabilities);

        let (builder, routes) =
            crate::update::request(&self.endpoint, &mut self.dialog, &self.target, Some(&offer))?;
        let request = crate::update::finish(builder, &routes)?;

        self.negotiation.sent_offer();
        let response = crate::update::send(&self.endpoint, request, self.target.clone()).await;
        // Whatever came back closed the exchange: a 2xx carries the answer, and a failure means
        // there will never be one. Leaving the flag set would refuse every later offer of ours.
        self.negotiation.received_answer();
        let response = response?;
        if !response.status.is_success() {
            return Err(crate::update::rejected(&response));
        }

        self.dialog.refresh_target(&response.headers);
        self.target = in_dialog_target(&self.dialog, self.target.clone());
        self.peer_allows_update = update::peer_allows(&response.headers);

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body())) {
            // The answer's ICE half, before the codec comparison: on a restart it carries the
            // peer's new credentials and candidates, and an agent that is not told about them
            // checks a path nobody is answering on. On an ordinary re-offer it is the same half
            // again, which the agent merges (RFC 8839 §4.2) rather than replaces — so a
            // re-answer cannot silence ICE on a call that is working.
            if let Ok(renegotiated) = negotiated(&answer, self.codecs) {
                preserve_rtcp_mode(self.current.rtcp_mode, renegotiated.rtcp_mode)?;
                // Do not let an answer that failed the mode guard mutate the running ICE
                // generation. Socket ownership and candidate state move together or neither does.
                self.accept_answer_ice(&answer).await;
                self.move_media_if_changed(renegotiated).await?;
            }
        }
        self.hold = direction;
        self.adopt_session(&response);
        self.rearm();
        Ok(())
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

    /// Rebuild the media session, but only if where or how the media flows actually changed.
    ///
    /// Restarting an unchanged session would drop packets for no reason on every re-INVITE, and
    /// some peers send one every thirty seconds as a keep-alive.
    async fn move_media_if_changed(&mut self, to: Negotiated) -> Result<()> {
        // The payload type is the codec's number on the wire: a re-offer can move Opus from
        // 111 to 96 and leave the codec unchanged, and a session not rebuilt for that goes on
        // sending on the number the far end just reassigned.
        //
        // Compared as the *wire* number, not as the raw `Option`. A peer may add or drop the
        // redundant `a=rtpmap:0 PCMU/8000` between two descriptions of the same static codec, and
        // `Some(0)` against `None` would read as a change when nothing changed — rebuilding the
        // session, and dropping audio, on a re-INVITE that only reworded the SDP.
        if to.remote != self.current.remote
            || to.codec != self.current.codec
            || to.wire_payload_type() != self.current.wire_payload_type()
            || to.rtcp_mode != self.current.rtcp_mode
        {
            let port = MediaPort::bind(SocketAddr::new(self.media_bind_address, 0))
                .await
                .map_err(Error::Io)?;
            let replacement = port.start(to.media_config())?;
            // Mute is a property of the call, not of the session that happens to be carrying it
            // (`M-18`). Without this a re-INVITE that moves the media — the far end changing
            // address or codec, which this side did not ask for and cannot refuse — unmutes the
            // call behind the application's back.
            replacement.set_muted(self.media.is_muted());
            let previous = std::mem::replace(&mut self.media, Arc::new(replacement));
            previous.stop();
        }
        self.current = to;
        Ok(())
    }

    /// Restart ICE on this call (RFC 8445 §9, RFC 8839 §4.4.1.1.1; `ice.md` §13.5).
    ///
    /// Sends a re-INVITE whose offer carries **new** `ice-ufrag` and `ice-pwd` for this stream,
    /// which is the entire signal — the peer reads both having changed and begins a new ICE
    /// session. Everything else about the call is unchanged, including its direction, so a
    /// restart does not resume a call that was on hold.
    ///
    /// Media keeps flowing on the pair the finished session selected until the new one selects its
    /// own. That is what makes a restart usable in the situation it exists for: the path has become
    /// doubtful, not yet unusable, and going silent while checks converge would turn a recoverable
    /// call into a dropped one.
    ///
    /// A call not running ICE is left alone and reports success. There is nothing to restart, and
    /// making the caller distinguish "no ICE" from "restart failed" would push the check to every
    /// call site.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the re-INVITE cannot be built or sent, or when the far end refuses
    /// it — the same failures as any other renegotiation, and like them it leaves the call running.
    pub async fn restart_ice(&mut self) -> Result<()> {
        if !self.media.runs_ice() {
            return Ok(());
        }
        self.reoffer(self.hold, IceOffer::Restart).await
    }

    /// Send a re-INVITE renegotiating this call.
    ///
    /// `direction` puts the call on hold (`SendOnly` or `Inactive`) or takes it off
    /// (`SendRecv`).
    ///
    /// Note what hold is **not**: RFC 8839 §4.4.1.1.1 makes `c=0.0.0.0` imply an ICE restart, so a
    /// hold spelled with a null connection address would restart ICE on every mute. Hold here is a
    /// direction and nothing else (RFC 3264), which is what it has always been, and this is the
    /// story that makes that a decision rather than an accident.
    pub async fn reinvite(&mut self, direction: Direction) -> Result<()> {
        self.reoffer(direction, IceOffer::Continue).await
    }

    /// The re-INVITE both public entry points send, and the one place their difference lives.
    ///
    /// `ice` is a parameter rather than a field on [`Call`] because it is a property of *this*
    /// offer and of nothing else. Held as state it would be a fourth `bool` on a struct that
    /// already has three — which is what `clippy::struct_excessive_bools` objects to, and the
    /// objection is right: a flag set before a call and cleared after it is a state machine
    /// written in the hardest way to read.
    async fn reoffer(&mut self, direction: Direction, ice: IceOffer) -> Result<()> {
        if self.profile == MediaProfile::BrowserAudio {
            return Err(sipx_sdp::browser_audio::ProfileError::ProfileRemoved.into());
        }
        if self.keying == Keying::DtlsSrtp {
            return Err(Error::DtlsRenegotiation);
        }
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();

        let mut capabilities = self
            .codecs
            .capabilities(self.media_address, self.media.local_addr().port());
        if self.current.rtcp_mode == sipx_sdp::RtcpMode::Mux {
            capabilities = capabilities.with_rtcp_mux();
        }
        capabilities.direction = direction;
        // The session version must increase with each modified offer, so the far end can tell
        // a changed description from a repeated one.
        capabilities.session_version = u64::from(cseq);
        let mut offer = offer_from(&capabilities);
        self.offer_ice(&mut offer, ice).await;

        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Invite, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Invite)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(
                HeaderName::Allow,
                Bytes::from_static(update::ALLOW.as_bytes()),
            )?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )?
            .max_forwards(70)
            .body(Bytes::from(offer.to_string_sdp()));

        // RFC 4028 §7.4: a refresh names the current interval and the current refresher, so
        // that proxies on the path can see the value in force and object to it. Any re-INVITE
        // refreshes the session (§7.2), so these go on every one rather than only on the ones
        // sent because the timer asked.
        let mut builder = builder.header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        if let Some(state) = self.session {
            let expires = SessionExpires {
                interval: state.terms.interval,
                refresher: Some(if state.terms.we_refresh {
                    session::Refresher::Uac
                } else {
                    session::Refresher::Uas
                }),
            };
            builder = builder
                .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
                .header(
                    HeaderName::MinSe,
                    Bytes::from(session::ABSOLUTE_MIN_INTERVAL.as_secs().to_string()),
                )?;
        }

        let request = add_routes(builder, &routes)?.build();
        // RFC 3311 §5.2 rule 2 names an offer sent "in an UPDATE, PRACK or INVITE", and this is
        // the INVITE case: the offer is outstanding for as long as the response takes. Marked
        // and cleared around the whole exchange, so a failure cannot leave the flag set and
        // refuse every later offer of ours.
        self.negotiation.sent_offer();
        let exchange = async {
            let mut responses = self.endpoint.send(request, self.target.clone()).await?;
            responses.final_response().await.ok_or(Error::NoResponse)
        }
        .await;
        self.negotiation.received_answer();
        let response = exchange?;

        if !response.status.is_success() {
            // The far end refused the change. The call it refused to change is still running,
            // so this is an error about the renegotiation, not about the call.
            const INTERVAL_TOO_SMALL: u16 = 422;
            if response.status.code() == INTERVAL_TOO_SMALL
                && let Some(required) = required_interval(&response)
                && let Some(state) = self.session.as_mut()
            {
                // §10: only a 2xx extends the expiration, so adopting the longer interval does
                // *not* buy time — the refresh still has to succeed before the deadline that
                // was already running. The next attempt is the one that must land.
                state.terms.interval = required.max(session::ABSOLUTE_MIN_INTERVAL);
            }
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }

        // RFC 3261 §12.2.1.2: the 2xx to a target refresh request refreshes the target here
        // too, and it must be applied before the ACK — which is itself an in-dialog request
        // and belongs at the peer's new location.
        self.dialog.refresh_target(&response.headers);
        self.target = in_dialog_target(&self.dialog, self.target.clone());

        send_ack(&self.endpoint, &self.dialog, self.target.clone()).await?;

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body())) {
            // The answer's ICE half, before the codec comparison: on a restart it carries the
            // peer's new credentials and candidates, and an agent that is not told about them
            // checks a path nobody is answering on. On an ordinary re-offer it is the same half
            // again, which the agent merges (RFC 8839 §4.2) rather than replaces — so a
            // re-answer cannot silence ICE on a call that is working.
            if let Ok(renegotiated) = negotiated(&answer, self.codecs) {
                preserve_rtcp_mode(self.current.rtcp_mode, renegotiated.rtcp_mode)?;
                self.accept_answer_ice(&answer).await;
                self.move_media_if_changed(renegotiated).await?;
            }
        }
        self.hold = direction;
        // §7.2: the session expiration is measured from the 2xx, and a re-INVITE sent for any
        // other reason refreshes it just the same.
        self.adopt_session(&response);
        self.rearm();
        Ok(())
    }

    /// Take the session terms from the 2xx to a refresh we sent (RFC 4028 §7.2).
    ///
    /// Shared by the re-INVITE and the UPDATE paths: §7.2 measures the expiration from the 2xx
    /// and says nothing about which request drew it, so reading it in two places would be two
    /// chances to read it differently.
    fn adopt_session(&mut self, response: &Response) {
        if let Some(agreed) = session::adopt(
            response
                .headers
                .typed::<SessionExpires>()
                .and_then(std::result::Result::ok),
            self.session.map(|state| state.terms.interval),
        ) && let Some(state) = self.session.as_mut()
        {
            state.terms = agreed;
        }
    }

    /// Refresh the session, by whichever method the peer allows (RFC 4028 §7.4).
    ///
    /// > "If a UAC knows that its peer supports the UPDATE method, it is RECOMMENDED that
    /// > UPDATE be used instead of a re-INVITE."
    ///
    /// It is only *known* from the peer's `Allow` (RFC 3311 §4), so that is what decides.
    /// Guessing the other way costs a working call: a refresh the far end answers 405 is a
    /// refresh that never happens, and the deadline behind it hangs up on a peer that is alive.
    ///
    /// The UPDATE carries **no body**. A refresh changes nothing — the description in force
    /// stays in force — and re-offering it would put a liveness check under §5.2's offer/answer
    /// rules, where it could be refused 491 or 500 for a reason that has nothing to do with
    /// whether the far end is still there.
    async fn refresh_session(&mut self) -> Result<()> {
        if !self.peer_allows_update {
            return self.reinvite(self.hold).await;
        }

        let (mut builder, routes) =
            crate::update::request(&self.endpoint, &mut self.dialog, &self.target, None)?;
        // §7.4: a refresh names the interval and the refresher in force, so proxies on the path
        // can see the value and object to it. `Min-SE` is this side's own floor, and it is a
        // defence rather than a courtesy (§11.2).
        builder = builder.header(HeaderName::Supported, Bytes::from_static(b"timer"))?;
        if let Some(state) = self.session {
            let expires = SessionExpires {
                interval: state.terms.interval,
                refresher: Some(if state.terms.we_refresh {
                    session::Refresher::Uac
                } else {
                    session::Refresher::Uas
                }),
            };
            builder = builder
                .header(HeaderName::SessionExpires, Bytes::from(expires.to_string()))?
                .header(
                    HeaderName::MinSe,
                    Bytes::from(session::ABSOLUTE_MIN_INTERVAL.as_secs().to_string()),
                )?;
        }

        let request = crate::update::finish(builder, &routes)?;
        let response = crate::update::send(&self.endpoint, request, self.target.clone()).await?;

        if !response.status.is_success() {
            const INTERVAL_TOO_SMALL: u16 = 422;
            if response.status.code() == INTERVAL_TOO_SMALL
                && let Some(required) = required_interval(&response)
                && let Some(state) = self.session.as_mut()
            {
                // As on the re-INVITE path: only a 2xx extends the expiration, so adopting the
                // longer interval does not buy time. The next attempt is the one that has to
                // land, and it has to land before the deadline that is already running.
                state.terms.interval = required.max(session::ABSOLUTE_MIN_INTERVAL);
            }
            return Err(crate::update::rejected(&response));
        }

        self.dialog.refresh_target(&response.headers);
        self.target = in_dialog_target(&self.dialog, self.target.clone());
        self.adopt_session(&response);
        self.rearm();
        Ok(())
    }

    /// Ask the far end to transfer this call to `target` (RFC 3515).
    ///
    /// Returns once the transferee has accepted the *request*, which is not the same as the
    /// transfer having worked: a `202 Accepted` means "I will try". What became of it arrives
    /// afterwards, as NOTIFY, and shows up in [`Self::transfer`]. Reporting success here would
    /// tell a user their call was handed over when it may have been refused or rung out.
    pub async fn refer(&mut self, target: &Uri) -> Result<()> {
        let refer_to = String::from_utf8_lossy(&target.to_bytes()).into_owned();
        self.refer_to_raw(&refer_to).await
    }

    /// Ask the far end to replace `other` with a call to this one's peer (RFC 3891 + 3515).
    ///
    /// The attended half of a transfer. Where a blind transfer says "call this number", this
    /// says "call this number, and when you get through, take the place of the call I already
    /// have with them" — which is what makes the handover seamless rather than a second ring.
    pub async fn refer_attended(&mut self, other: &Call) -> Result<()> {
        let replaces = Replaces {
            call_id: other.dialog.id.call_id.clone(),
            // From the point of view of the party that will receive the eventual INVITE, our
            // *remote* tag on `other` is that party's own local tag. Writing our own tag here
            // produces a header that names nothing and a transfer that always fails.
            to_tag: other.dialog.id.remote_tag.clone(),
            from_tag: other.dialog.id.local_tag.clone(),
            early_only: false,
        };
        let target = String::from_utf8_lossy(&other.dialog.remote_target.to_bytes()).into_owned();
        // `?` separates a URI from the headers it asks to be put in the request built from it
        // (RFC 3261 §19.1.1), and `Replaces` is one of those headers.
        let refer_to = format!(
            "{target}?Replaces={}",
            escape_uri_header(&replaces.to_header())
        );
        self.refer_to_raw(&refer_to).await
    }

    /// Send a REFER whose `Refer-To` is this text.
    async fn refer_to_raw(&mut self, refer_to: &str) -> Result<()> {
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();

        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Refer, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local.clone()))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Refer)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            .header(HeaderName::ReferTo, Bytes::from(format!("<{refer_to}>")))?
            // RFC 3892. The transferee is being asked to call a stranger on our say-so; saying
            // who we are is the only basis it has for deciding whether to.
            .header(
                HeaderName::ReferredBy,
                Bytes::from(strip_header_params(&local)),
            )?
            .max_forwards(70);

        let request = add_routes(builder, &routes)?.build();
        let mut responses = self.endpoint.send(request, self.target.clone()).await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;

        if !response.status.is_success() {
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
        }

        // Nothing is known yet beyond "it was taken on". The first NOTIFY replaces this.
        self.transfer = Some(Transfer {
            state: TransferState::Trying,
            finished: false,
        });
        Ok(())
    }

    /// The transfer the far end has asked for, if it has asked and we have not answered.
    #[must_use]
    pub fn referral(&self) -> Option<&Referral> {
        self.referral.as_ref()
    }

    /// A transfer we asked for, and what has become of it. `None` if we asked for none.
    #[must_use]
    pub fn transfer(&self) -> Option<&Transfer> {
        self.transfer.as_ref()
    }

    /// Accept the transfer, place the call, and report the outcome (RFC 3515 §2.4.5).
    ///
    /// `target` is where to *send* the new INVITE; the `Refer-To` URI is what goes in it. The
    /// two are separate for the same reason they are separate in [`dial`]: resolving a URI to
    /// an address is RFC 3263's job and lives in the transport, not here.
    ///
    /// The original call is left running. Whether to hang up on the transferor is a policy
    /// decision — a blind transfer usually ends it, an attended one does not — and it belongs
    /// to whoever is making that decision, not to this function.
    pub async fn accept_referral(&mut self, target: Target, options: &DialOptions) -> Result<Call> {
        let Some(referral) = self.referral.take() else {
            return Err(Error::NoReferral);
        };

        let accepted = ResponseBuilder::to_request(
            &referral.request,
            StatusCode::new(202).unwrap_or_else(|| unreachable!("202 is a valid status code")),
            "Accepted",
        )?
        .build();
        self.endpoint.respond(&referral.key, accepted).await?;

        // "I am trying", straight away. RFC 3515 §2.4.4 asks for an immediate NOTIFY so the
        // transferor knows the subscription exists before anything can go wrong with the call.
        self.notify_transfer(&referral, 100, "Trying", false)
            .await?;

        let placed = dial(&self.endpoint, target, &referral.target, options).await;

        let (status, reason) = match &placed {
            Ok(_) => (200, "OK".to_owned()),
            Err(Error::Rejected { status, reason }) => (*status, reason.clone()),
            // Anything else never reached the target at all. 503 is what a proxy would say for
            // the same situation, and it tells the transferor something true.
            Err(_) => (503, "Service Unavailable".to_owned()),
        };
        // Terminating, whether it worked or not. A transferee that reports the outcome and then
        // says nothing leaves a subscription open on both sides for a transfer that is over.
        self.notify_transfer(&referral, status, &reason, true)
            .await?;

        placed
    }

    /// Refuse the transfer (RFC 3515 §2.4.2).
    ///
    /// No subscription is created by a REFER that was not accepted, so nothing further is owed
    /// and no NOTIFY is sent. The transferor learns the outcome from the status, which is why
    /// it should be one they can act on — 603 for "no", 488 for "not that target".
    pub async fn refuse_referral(&mut self, status: u16, reason: &'static str) -> Result<()> {
        let Some(referral) = self.referral.take() else {
            return Err(Error::NoReferral);
        };
        let code = StatusCode::new(status).ok_or(Error::NoReferral)?;
        let response = ResponseBuilder::to_request(&referral.request, code, reason)?.build();
        self.endpoint.respond(&referral.key, response).await?;
        Ok(())
    }

    /// Note a REFER, or refuse one that cannot be honoured whatever the application thinks.
    async fn on_refer(&mut self, incoming: &Incoming) -> Result<()> {
        let sequence = incoming
            .request
            .headers
            .typed::<sipx_sip::headers::CSeq>()
            .and_then(std::result::Result::ok)
            .map_or(0, |cseq| cseq.sequence);

        let refer_to = incoming.request.headers.value(&HeaderName::ReferTo);
        let target = refer_to.as_deref().and_then(|value| {
            let text = String::from_utf8_lossy(value);
            Uri::parse(Bytes::from(unbracket(text.trim()))).ok()
        });

        let Some(target) = target else {
            // A missing or unparseable `Refer-To` is not a decision for the application: there
            // is nowhere to transfer to, and 400 says exactly that.
            self.refuse_now(incoming, 400, "Bad Request").await?;
            return Ok(());
        };

        // An attended transfer's `Refer-To` carries a `Replaces` (RFC 3891 + 3515), built by
        // `refer_attended` above as a URI header parameter. A substring check on the raw value
        // rather than a full URI-header parse: `Uri` does not expose its header component, and
        // this only has to distinguish "asks to replace a dialog" from "does not", not validate
        // one.
        let attended = refer_to.as_deref().is_some_and(contains_replaces);

        self.referral = Some(Referral {
            target: target.clone(),
            referred_by: incoming
                .request
                .headers
                .value(&HeaderName::ReferredBy)
                .map(|value| String::from_utf8_lossy(&value).into_owned()),
            event_id: sequence,
            key: incoming.key.clone(),
            request: incoming.request.clone(),
        });
        self.events
            .emit(CallEvent::TransferRequested { target, attended });
        Ok(())
    }

    /// Take in what the transferee says about a transfer we asked for.
    async fn on_notify(&mut self, incoming: &Incoming) -> Result<()> {
        // Answered first and unconditionally. A NOTIFY we do not understand is still a request
        // that must not be left to time out, and the subscription is ours whether or not this
        // particular notification made sense.
        let ok = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?.build();
        self.endpoint.respond(&incoming.key, ok).await?;

        let is_refer = incoming
            .request
            .headers
            .value(&HeaderName::Event)
            .is_some_and(|value| {
                String::from_utf8_lossy(&value)
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("refer")
            });
        if !is_refer {
            return Ok(());
        }

        let finished = incoming
            .request
            .headers
            .value(&HeaderName::SubscriptionState)
            .is_some_and(|value| is_terminated(&value));

        let state = parse_sipfrag(incoming.request.body())
            .map(|(status, reason)| TransferState::from_status(status, &reason));

        let transfer = self.transfer.get_or_insert(Transfer {
            state: TransferState::Trying,
            finished: false,
        });
        if let Some(state) = state {
            transfer.state = state.clone();
            self.events.emit(CallEvent::TransferProgress(state));
        }
        // Once terminated, always terminated: a stray notification afterwards must not reopen a
        // subscription the transferee has already closed.
        transfer.finished |= finished;
        Ok(())
    }

    /// Report progress on a transfer we accepted.
    async fn notify_transfer(
        &mut self,
        referral: &Referral,
        status: u16,
        reason: &str,
        terminate: bool,
    ) -> Result<()> {
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();
        let subscription = if terminate {
            // `noresource` is the reason RFC 6665 §4.1.3 gives for "the thing you subscribed to
            // no longer exists", which is what a finished transfer is.
            "terminated;reason=noresource".to_owned()
        } else {
            "active;expires=60".to_owned()
        };

        let (uri, routes) = self.dialog.request_target();
        let builder = RequestBuilder::new(Method::Notify, uri)
            .header(HeaderName::To, Bytes::from(remote))?
            .header(HeaderName::From, Bytes::from(local))?
            .header(
                HeaderName::CallId,
                Bytes::from(self.dialog.id.call_id.clone()),
            )?
            .cseq(cseq, &Method::Notify)?
            .header(
                HeaderName::Contact,
                Bytes::from(contact_for(&self.endpoint, self.target.transport)),
            )?
            // The `id` ties this to the REFER that created the subscription, so a transferor
            // with two transfers in flight can tell which one this is about (RFC 3515 §2.4.4).
            .header(
                HeaderName::Event,
                Bytes::from(format!("refer;id={}", referral.event_id)),
            )?
            .header(HeaderName::SubscriptionState, Bytes::from(subscription))?
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"message/sipfrag;version=2.0"),
            )?
            .max_forwards(70)
            .body(Bytes::from(sipfrag(status, reason)));

        let request = add_routes(builder, &routes)?.build();
        let mut responses = self.endpoint.send(request, self.target.clone()).await?;
        // A NOTIFY the transferor never answers does not undo the transfer; the call it asked
        // for has already happened either way.
        //
        // discard: nothing is thrown away that anyone could act on. The NOTIFY itself was handed
        // over — one the endpoint could not put on the wire is counted at the transmit by
        // `sipx_transport::UnsentCounts` — and what is dropped here is only *waiting* for its
        // answer. The bound bounds a failure (`X-29`): the transfer's outcome does not depend on
        // the reply arriving.
        let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
        Ok(())
    }

    /// Refuse a request outright, without involving the application.
    async fn refuse_now(
        &mut self,
        incoming: &Incoming,
        status: u16,
        reason: &'static str,
    ) -> Result<()> {
        let Some(code) = StatusCode::new(status) else {
            return Ok(());
        };
        let response = ResponseBuilder::to_request(&incoming.request, code, reason)?.build();
        self.endpoint.respond(&incoming.key, response).await?;
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

    async fn end_with_reason(&mut self, cause: EndCause, reason: &ReasonValue) -> Result<()> {
        if self.ended {
            return Ok(());
        }
        self.media.flush(Duration::from_secs(5)).await;
        self.media.stop();
        self.ended = true;
        self.session = None;
        if let Some(notify) = self.awaiting_ack.take() {
            notify.notify_waiters();
        }
        self.events.end(cause);

        let cseq = self.dialog.next_cseq();
        let bye = bye_request(&self.dialog, cseq, reason)?;
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

    /// End the call because this side decided to.
    pub async fn hang_up(&mut self) -> Result<()> {
        self.end(EndCause::LocalHangup).await
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
            // DTMF arrives over RTP, not signalling, so nothing above ever sees it — this is
            // the one place a digit becomes a `CallEvent`. Read fresh from `call.media()` on
            // every pass rather than once outside the loop, so a re-INVITE that moves the
            // media session (`move_media_if_changed`) is followed automatically: the next
            // iteration's future is built against whichever session is current.
            digit = call.media().recv_digit() => {
                if let Some((digit, duration)) = digit {
                    call.events.emit(CallEvent::Dtmf { digit, duration });
                }
            }
        }
    }
    Ok(())
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

/// Percent-escape a value going into a URI header field.
///
/// A `Replaces` value contains `;` and `=`, both of which end a URI header in the grammar of
/// RFC 3261 §19.1.1. Left unescaped, the `Refer-To` would be truncated at the first semicolon,
/// the transferee would place an ordinary call, and the transfer would appear to work while the
/// original call was never replaced.
fn escape_uri_header(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'@' => {
                out.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                // discard: nothing can be lost. `write!` into a `String` returns `fmt::Error`
                // only if the formatter itself fails, and `String`'s never does — there is no
                // I/O and no allocation failure path to report.
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Whether a `Refer-To` value carries a `Replaces` header parameter — RFC 3891's marker of an
/// attended transfer, as [`Call::refer_attended`] builds it (`<target>?Replaces=...`).
///
/// A substring check rather than a full URI-header parse: `Uri` does not expose its header
/// component, and telling "asks to replace a dialog" from "does not" is all this needs to do.
fn contains_replaces(value: &[u8]) -> bool {
    String::from_utf8_lossy(value)
        .to_ascii_lowercase()
        .contains("replaces=")
}

/// Strip the angle brackets a `Refer-To` almost always carries.
///
/// `Refer-To: <sip:x@y>` and `Refer-To: sip:x@y` are both legal; only the first can carry URI
/// parameters unambiguously, so it is the one everything sends. Any display name before the
/// bracket goes with them.
fn unbracket(value: &str) -> String {
    match (value.find('<'), value.rfind('>')) {
        (Some(open), Some(close)) if close > open => value
            .get(open + 1..close)
            .unwrap_or(value)
            .trim()
            .to_owned(),
        _ => value.to_owned(),
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

/// Put one gathered local description into the audio stream it belongs to.
fn add_ice(
    description: &mut SessionDescription,
    local: &LocalDescription,
    additional: &[sipx_sdp::Attribute],
) {
    let Some(default) = local.default_destination(ComponentId::RTP) else {
        return;
    };
    description.connection = Some(Connection::new(default.ip()));
    if let Some(audio) = description.media.first_mut() {
        audio.port = default.port();
        if let Some(control) = local.default_destination(ComponentId::RTCP) {
            let address_type = if control.is_ipv6() { "IP6" } else { "IP4" };
            audio.attributes.push(sipx_sdp::Attribute::valued(
                "rtcp",
                format!("{} IN {address_type} {}", control.port(), control.ip()),
            ));
        }
        audio.attributes.extend(local.attributes());
        audio.attributes.extend_from_slice(additional);
    }
}

/// The `a=` names RFC 8839 §5 gives ICE, so a later description can replace its own half.
///
/// Replaced rather than appended: `sipx_sdp::answer` copies the stream it is answering, so an
/// answer built from an offer that carried ICE starts out holding the *peer's* `ice-ufrag`,
/// `ice-pwd` and candidates. Extending that with ours would produce a description claiming both
/// sets, and a peer reading the first `ice-ufrag` it finds would key its checks to its own
/// credentials.
const ICE_ATTRIBUTES: &[&str] = &[
    "ice-ufrag",
    "ice-pwd",
    "ice-options",
    "ice-lite",
    "ice-pacing",
    "candidate",
    "remote-candidates",
];

/// Whether an attribute is one of the ICE names a later description restates.
///
/// `ice-mismatch` is deliberately **not** here. RFC 8839 §5.3 makes it a statement about the
/// exchange rather than a parameter of this side's ICE session, and a stream that carries it is
/// one ICE is not running for at all.
fn is_ice_attribute(attribute: &sipx_sdp::Attribute) -> bool {
    ICE_ATTRIBUTES
        .iter()
        .any(|name| attribute.name.eq_ignore_ascii_case(name))
}

/// This side's ICE half for a later offer or answer (RFC 8839 §4.4; `ice.md` §13.5).
///
/// The same three lines an initial description carries, from the agent rather than from the
/// gathering that has long since finished — `ice2` included, because §13.5's re-signalling has to
/// restate the whole half and a peer that stopped seeing `ice-options` would read it as a change.
fn ice_attributes(local: &sipx_media::ice::Local) -> Vec<sipx_sdp::Attribute> {
    let mut attributes = vec![
        sipx_sdp::Attribute::valued("ice-ufrag", local.credentials.ufrag()),
        sipx_sdp::Attribute::valued("ice-pwd", local.credentials.pwd()),
        sipx_sdp::Attribute::valued("ice-options", sipx_sdp::ice::ICE2),
    ];
    attributes.extend(
        local
            .candidates
            .iter()
            .map(|candidate| sipx_sdp::Attribute::valued("candidate", candidate.to_value())),
    );
    attributes
}

/// What a later offer says about the ICE session already running (RFC 8839 §4.4; `ice.md` §13.5).
///
/// Two variants and not a `bool`, because the wire difference between them is not a flag: a
/// continuing offer restates the credentials in force, and a restart states new ones. §4.4.1.1.1
/// makes *that change* the entire signal, so there is nothing else for either variant to set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IceOffer {
    /// Restate this side's half unchanged — hold, resume, a codec change, a session refresh.
    Continue,
    /// Draw new credentials and a new tiebreaker, which is what begins a new ICE session.
    Restart,
}

/// The peer's ICE credentials as the description in this body states them, for [`Call::peer_ice`].
///
/// Read from the message that completed the initial exchange — the 2xx for a caller, the INVITE
/// for a callee — because a restart is only ever recognisable as a *change* from what was last
/// seen, and a call that recorded nothing would read the peer's first re-offer as one.
///
/// `None` for a description with no ICE, which is the ordinary call: nothing that arrives later can
/// restart a session that never began.
fn peer_ice_credentials(body: &[u8]) -> Option<sipx_sdp::ice::Credentials> {
    let description = sipx_sdp::parse(&String::from_utf8_lossy(body)).ok()?;
    let audio = description.media.first()?;
    match sipx_media::ice::negotiate(&description, audio) {
        IceNegotiation::Ice { credentials, .. } => Some(credentials),
        IceNegotiation::Absent | IceNegotiation::Mismatch => None,
    }
}

/// Credentials and a tiebreaker for a new ICE session (RFC 8839 §5.4, RFC 8445 §7.1.3).
///
/// Drawn per session and never reused, for the reason `ice.md` §13.4 gives about the initial
/// exchange and which applies unchanged to a restart: credentials that outlive the session they
/// authenticated make one session's checks valid in another. A tiebreaker carried across would
/// resolve a role conflict the way the *previous* session resolved it.
///
/// `None` when credentials could not be built — the same failure `MediaPolicy::gathering` reports
/// on the initial exchange, from the same generator, so it is not reachable in practice. It
/// degrades rather than failing the renegotiation: the agent keeps the credentials it has, the
/// answer restates them, and the peer keys its new session's checks to those. RFC 8839 §4.4.1.1.1
/// asks the answerer for new ones; reusing them is worse than complying and much better than
/// refusing a re-offer on a call that is working.
fn fresh_ice_parameters() -> Option<(sipx_sdp::ice::Credentials, u64)> {
    let credentials = IceCredentials::new(token(), format!("{}{}", token(), token()))?;
    Some((credentials, rand::random()))
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

/// The `Min-SE` a `422` demands, if it named one (RFC 4028 §6).
fn required_interval(response: &Response) -> Option<Duration> {
    response
        .headers
        .typed::<MinSe>()
        .and_then(std::result::Result::ok)
        .map(|min| min.0)
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
    String,
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
    Ok((port, capabilities, ice, keying, via, invite))
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
) -> Result<(String, Request)> {
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
    Ok((via, invite))
}

/// Take back an invitation the caller has stopped waiting for.
///
/// Split out of `dial_with` for length, but it is the part with all the hazards in it, so it
/// keeps its own name: everything here is about not leaving the far end in a call.
async fn withdraw(
    endpoint: &Handle,
    invite: &Request,
    via: &str,
    target: Target,
    responses: &mut sipx_transport::Responses,
    provisional: bool,
    reason: &ReasonValue,
) {
    // Giving up is not just ceasing to wait. The far end is ringing and has been told
    // nothing; without a CANCEL it goes on ringing, and someone answering afterwards
    // ends up in a call with a party that has left.
    //
    // Only once it has answered provisionally, though. RFC 3261 §9.1: with nothing
    // received, "the client MUST wait for the arrival of a provisional response before
    // sending" the CANCEL — one sent first can overtake the INVITE it refers to, match
    // nothing at the far end, and leave the invitation it was meant to withdraw
    // running. So this waits rather than giving up on cancelling, bounded because a
    // peer that never answers at all would otherwise hold the call attempt open.
    if provisional {
        // discard: counted, not ignored. A CANCEL that does not reach the wire leaves the far end
        // ringing, which is exactly the loss §12.1 exists to make visible — so the driver counts it
        // as `sipx_transport::UnsentCounts::cancel` where the socket is actually written. What is
        // discarded *here* is the `Result`, and there is nothing this path can do with it: it is
        // already the giving-up path, and the only remedy for a failed CANCEL is the ACK-then-BYE
        // below, which runs regardless. Note that this `Result` being `Ok` does not mean the
        // CANCEL went out — `Handle::send` returns once the transaction exists — which is exactly
        // why the count is taken below rather than from what is dropped here.
        let _ = send_cancel(endpoint, invite, via, target.clone(), reason).await;
    }

    // CANCEL cannot close the race it exists to manage: a 200 already in flight
    // arrives anyway, and RFC 3261 §15 says a UAC that will not proceed must
    // acknowledge it and then hang up rather than leave it unanswered.
    let mut cancelled = provisional;
    let grace = tokio::time::Instant::now() + Duration::from_secs(2);
    while let Ok(Some(event)) = tokio::time::timeout_at(grace, responses.next()).await {
        let sipx_sip::transaction::TuEvent::Response(late) = event else {
            continue;
        };
        if !late.status.is_final() {
            if !cancelled {
                cancelled = true;
                // discard: the same loss and the same counter as above —
                // `sipx_transport::UnsentCounts::cancel`, taken at the transmit. This is the
                // CANCEL that could not be sent before a provisional arrived, sent now that one
                // has; the `Result` is discarded for the same reason, and `cancelled` is set
                // either way so a second provisional does not produce a second CANCEL.
                let _ = send_cancel(endpoint, invite, via, target.clone(), reason).await;
            }
            continue;
        }
        if late.status.is_success() {
            ack_then_bye(endpoint, invite, &late, target.clone()).await;
        }
        break;
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
    let (port, capabilities, ice, keying, via, invite) =
        open_invitation(endpoint, &target, to, options, identity, authorization).await?;

    let mut responses = endpoint.send(invite.clone(), target.clone()).await?;

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
        Waited::GaveUp { provisional } => {
            withdraw(
                endpoint,
                &invite,
                &via,
                target.clone(),
                &mut responses,
                provisional,
                &request_timeout_reason(),
            )
            .await;
            return Err(Error::Cancelled(options.timeout.unwrap_or(Duration::ZERO)));
        }
        Waited::Cancelled { provisional } => {
            withdraw(
                endpoint,
                &invite,
                &via,
                target.clone(),
                &mut responses,
                provisional,
                &normal_clearing_reason(),
            )
            .await;
            return Err(Error::Cancelled(Duration::ZERO));
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
                endpoint: endpoint.clone(),
                target: in_dialog,
                awaiting_ack: None,
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

/// What the far end's answer to *our* offer settles.
///
/// The calling side's counterpart of [`Early::settle`], and the reason it is a function is that
/// an answer can now reach us in two places: the 200 that [`establish`] reads, and — once
/// [`dial_early`] exists — the reliable provisional that makes an early dialog renegotiable at
/// all (RFC 3262 §5). There is no port to bind on either path, because ours was bound before the
/// INVITE named it.
fn settle_answer(
    offered: &Capabilities,
    answer: &SessionDescription,
    codecs: Codecs,
) -> Result<Settled> {
    // Both halves or neither, *and* the two halves have to be the ones the two ends agreed on:
    // a stream keyed at one end only is a call that connects and carries silence, and one keyed
    // on an answer that echoed a tag nobody sent is a call encrypted to nothing. Neither is
    // worth having, so both come back as `Error::Sdp` rather than as a quietly plain call.
    let answered = answered_crypto(answer);
    let mut negotiated = negotiated(answer, codecs)?;
    let answered_mux = answer
        .media
        .iter()
        .find(|media| media.media == "audio" && !media.is_rejected())
        .is_some_and(sipx_sdp::MediaDescription::rtcp_mux);
    negotiated.rtcp_mode = if offered.rtcp_mux && answered_mux {
        sipx_sdp::RtcpMode::Mux
    } else {
        sipx_sdp::RtcpMode::Separate
    };
    Ok(Settled {
        negotiated,
        srtp: srtp_keys(offered.crypto.as_slice(), answered.as_ref())?,
    })
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
    // Ahead of the claim, deliberately: an offer that cannot be read fails here with nothing
    // sent, and an invitation that was never taken is one a CANCEL can still end.
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
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
    /// The `Via` the INVITE went out with, which a CANCEL carries verbatim (RFC 3261 §9.1).
    via: String,
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
    /// Whether the far end has answered provisionally at all, which RFC 3261 §9.1 makes the
    /// precondition for cancelling.
    provisional: bool,
    /// When to stop waiting, counted from when the INVITE went out rather than from each call
    /// to [`Self::answered`] — the far end is ringing against one deadline, not a fresh one per
    /// method call.
    deadline: Option<tokio::time::Instant>,
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
            dialing.give_up().await;
            Err(Error::Cancelled(Duration::ZERO))
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
    let (port, capabilities, ice, _keying, via, invite) =
        open_invitation(endpoint, &target, to, options, &Identity::fresh(), None).await?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;

    let (events, events_rx) = EventSink::new();
    Ok(Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        via,
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
        provisional: false,
        deadline: options
            .timeout
            .map(|limit| tokio::time::Instant::now() + limit),
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
    let (via, invite) = open_offerless_invitation(endpoint, &target, to, options, &identity)?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;
    let (events, events_rx) = EventSink::new();
    let mut dialing = Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        via,
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
        provisional: false,
        deadline: options
            .timeout
            .map(|limit| tokio::time::Instant::now() + limit),
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
    let (via, invite) = open_offerless_invitation(endpoint, &target, to, options, &identity)?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;
    let (events, events_rx) = EventSink::new();
    let mut dialing = Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        via,
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
        provisional: false,
        deadline: options
            .timeout
            .map(|limit| tokio::time::Instant::now() + limit),
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
                    self.give_up().await;
                    return Err(Error::Cancelled(
                        self.options.timeout.unwrap_or(Duration::ZERO),
                    ));
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
                    self.give_up().await;
                    Err(Error::Cancelled(
                        self.options.timeout.unwrap_or(Duration::ZERO),
                    ))
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
                            self.give_up().await;
                            return Err(Error::Cancelled(Duration::ZERO));
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
                    self.give_up().await;
                    return Err(Error::Cancelled(
                        self.options.timeout.unwrap_or(Duration::ZERO),
                    ));
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
        self.give_up().await;
    }

    /// Cancel this invitation with an explicit protocol cause.
    ///
    /// A SIP 200 reason represents the RFC 3326 §3.1 case where another coupled or forked leg
    /// completed the call; other valid SIP and Q.850 causes are retained unchanged.
    pub async fn cancel_with_reason(mut self, reason: ReasonValue) {
        self.give_up_with_reason(&reason).await;
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
                    self.give_up().await;
                    return Err(Error::Cancelled(
                        self.options.timeout.unwrap_or(Duration::ZERO),
                    ));
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
                    self.give_up().await;
                    return Err(Error::Cancelled(
                        self.options.timeout.unwrap_or(Duration::ZERO),
                    ));
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
                Some(deadline) => match tokio::time::timeout_at(deadline, responses.next()).await {
                    Ok(event) => event,
                    Err(_elapsed) => return Arrived::GaveUp,
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

        self.provisional = true;
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
        self.give_up().await;
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
                    endpoint: self.endpoint.clone(),
                    target: self.in_dialog.clone(),
                    awaiting_ack: None,
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
    async fn give_up(&mut self) {
        self.give_up_with_reason(&normal_clearing_reason()).await;
    }

    async fn give_up_with_reason(&mut self, reason: &ReasonValue) {
        if let Some(response) = self.coupled_final.take() {
            if response.status.is_success() {
                ack_then_bye(&self.endpoint, &self.invite, &response, self.target.clone()).await;
            }
            return;
        }
        let Some(responses) = self.responses.as_mut() else {
            return;
        };
        withdraw(
            &self.endpoint,
            &self.invite,
            &self.via,
            self.target.clone(),
            responses,
            self.provisional,
            reason,
        )
        .await;
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
    let remote = offer.media.first().map_or(IceNegotiation::Absent, |audio| {
        sipx_media::ice::negotiate(offer, audio)
    });
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
    pub(crate) fn adopt_answer(&mut self, answer: &SessionDescription) {
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
            let _ = self.replace_media(settled);
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
    pub(crate) fn reanswer(&mut self, offer: &SessionDescription) -> Option<SessionDescription> {
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
        self.replace_media(settled).ok()?;
        Some(answer)
    }

    /// Apply an early UPDATE to the session that is already running.
    ///
    /// This is the same transition [`Call::move_media_if_changed`] performs for a confirmed
    /// dialog, but it happens at UPDATE time rather than being deferred to the INVITE's 2xx. The
    /// resulting session is then the one confirmation moves into `Call`, so answer time itself
    /// still neither rebinds nor leaves a gap.
    fn replace_media(&mut self, settled: Settled) -> Result<()> {
        let to = settled.negotiated;
        let changed = to.remote != self.settled.negotiated.remote
            || to.codec != self.settled.negotiated.codec
            || to.wire_payload_type() != self.settled.negotiated.wire_payload_type()
            || settled.is_encrypted() != self.settled.is_encrypted();
        if changed && !self.media.reconfigure(settled.media_config())? {
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
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
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

    let acked = Arc::new(tokio::sync::Notify::new());
    tokio::spawn(retransmit_until_acked(
        endpoint.clone(),
        incoming.key.clone(),
        response,
        Arc::clone(&acked),
    ));

    let (events, events_rx) = EventSink::new();
    emit_construction_events(&events, Some(ringing.is_reliable()));

    Ok(Call {
        dialog,
        initial_status: OK,
        media: Arc::new(media),
        endpoint: endpoint.clone(),
        target,
        awaiting_ack: Some(acked),
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
    let negotiated = negotiated(&offer, policy.codecs)?;

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

    let acked = Arc::new(tokio::sync::Notify::new());
    tokio::spawn(retransmit_until_acked(
        endpoint.clone(),
        incoming.key.clone(),
        response,
        Arc::clone(&acked),
    ));

    // The answer must leave before an active answerer sends ClientHello. A caller is permitted to
    // wait for the final SDP (and then its ACK) before opening the media path.
    let (media, settled) = key_and_start(
        port,
        local_ice,
        settled,
        keying,
        &offer,
        true,
        policy.profile,
    )
    .await?;

    // As in `dial_with`: emitted at construction, from what was actually observed (ringing
    // first, if this path came through it) rather than recomputed afterwards.
    let (events, events_rx) = EventSink::new();
    emit_construction_events(&events, reliable_ringing);

    Ok(Call {
        dialog,
        initial_status: OK,
        media: Arc::new(media),
        endpoint: endpoint.clone(),
        target,
        awaiting_ack: Some(acked),
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
    })
}

/// Resend a 2xx on the T1 backoff until the ACK arrives or 64·T1 has passed.
async fn retransmit_until_acked(
    endpoint: Handle,
    key: sipx_sip::transaction::TransactionKey,
    response: Response,
    acked: Arc<tokio::sync::Notify>,
) {
    let t1 = Duration::from_millis(500);
    let mut interval = t1;
    let mut elapsed = Duration::ZERO;
    let give_up = t1 * 64;

    loop {
        tokio::select! {
            () = acked.notified() => return,
            () = tokio::time::sleep(interval) => {}
        }
        elapsed += interval;
        if elapsed >= give_up {
            tracing::warn!("no ACK for our 2xx after 64*T1; giving up");
            return;
        }
        if endpoint.respond(&key, response.clone()).await.is_err() {
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
    /// The deadline passed. Whether a provisional had arrived decides what may be done about
    /// it, so it is carried out rather than discarded.
    GaveUp {
        /// Whether the far end had answered provisionally.
        provisional: bool,
    },
    /// The owner asked this attempt to stop. A provisional decides whether CANCEL may be sent
    /// immediately or must wait for the peer to acknowledge the INVITE first.
    Cancelled {
        /// Whether the far end had answered provisionally.
        provisional: bool,
    },
    /// The transaction ended without a final response.
    Gone,
    /// The selected transport could not be established or used.
    Transport(sipx_transport::Error),
}

/// Wait for the final response to an INVITE, remembering whether a provisional arrived.
///
/// The provisional is not incidental bookkeeping: RFC 3261 §9.1 forbids cancelling an
/// invitation the far end has not answered provisionally, so the deadline alone does not
/// decide what to do when the wait runs out — what came back matters too.
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
    let mut provisional = false;
    let mut ringing = None;
    loop {
        let event = match (deadline, cancelled.as_mut()) {
            (None, None) => responses.next().await,
            (Some(deadline), None) => {
                match tokio::time::timeout_at(deadline, responses.next()).await {
                    Ok(event) => event,
                    Err(_elapsed) => return Waited::GaveUp { provisional },
                }
            }
            (None, Some(cancelled)) => {
                tokio::select! {
                    biased;
                    () = cancelled.as_mut() => return Waited::Cancelled { provisional },
                    event = responses.next() => event,
                }
            }
            (Some(deadline), Some(cancelled)) => {
                tokio::select! {
                    biased;
                    () = cancelled.as_mut() => return Waited::Cancelled { provisional },
                    () = tokio::time::sleep_until(deadline) => return Waited::GaveUp { provisional },
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
                provisional = true;
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
                // Preserve TLS verification failures because treating a rejected certificate as
                // "no response" hides the security decision the caller must act on. Other send
                // failures retain the call API's established NoResponse behavior; changing those
                // exit semantics is outside this transport-selection story.
                if let Some(error @ sipx_transport::Error::Tls(_)) =
                    responses.take_transport_error()
                {
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

/// Cancel an INVITE that has not been answered (RFC 3261 §9.1).
///
/// A CANCEL is not a new request in its own right: it carries the INVITE's `Via` verbatim —
/// branch and all — its `Call-ID`, `To`, `From` and sequence *number*, differing only in the
/// method. That is what identifies which invitation it is cancelling.
async fn send_cancel(
    endpoint: &Handle,
    invite: &Request,
    via: &str,
    target: Target,
    reason: &ReasonValue,
) -> Result<()> {
    let copy = |name: &HeaderName| {
        invite
            .headers
            .value(name)
            .map(|value| Bytes::from(value.into_owned()))
    };

    let mut builder = RequestBuilder::new(Method::Cancel, invite.uri.clone())
        .header(HeaderName::Via, Bytes::from(via.to_owned()))?;
    for name in [HeaderName::To, HeaderName::From, HeaderName::CallId] {
        if let Some(value) = copy(&name) {
            builder = builder.header(name, value)?;
        }
    }
    // The same sequence number as the INVITE, with the method changed. A fresh number would
    // make it a new request rather than a cancellation of that one.
    let sequence = invite
        .headers
        .typed::<sipx_sip::headers::CSeq>()
        .and_then(std::result::Result::ok)
        .map_or(1, |cseq| cseq.sequence);

    let request = builder
        .header(HeaderName::Reason, Reason::from(reason.clone()).to_bytes())?
        .cseq(sequence, &Method::Cancel)?
        .max_forwards(70)
        .build();
    endpoint.send(request, target).await?;
    Ok(())
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

pub(crate) fn offer_from(capabilities: &Capabilities) -> SessionDescription {
    let mut sdp = SessionDescription::new(
        capabilities.address,
        capabilities.session_id,
        capabilities.session_version,
    );
    let mut audio = sipx_sdp::MediaDescription::audio(
        capabilities.audio_port,
        capabilities.audio_formats.clone(),
    );
    for (payload, mapping) in &capabilities.rtpmaps {
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "rtpmap",
            format!("{payload} {mapping}"),
        ));
    }
    // The key, and the protocol that matches it. Offering `a=crypto` under `RTP/AVP` asks for a
    // stream that is encrypted and declared not to be; offering `RTP/SAVP` with no key asks for
    // encryption with nothing to key it. Both come from the same place, so neither can drift.
    if let Some(crypto) = &capabilities.crypto {
        capabilities.protocol().clone_into(&mut audio.protocol);
        audio
            .attributes
            .push(sipx_sdp::Attribute::valued("crypto", crypto.to_value()));
    }
    // The same rule for DTLS-SRTP, with the fingerprint in place of the key: `UDP/TLS/RTP/SAVP`
    // and an `a=fingerprint` come from one place so a stream cannot claim one and carry the
    // other. RFC 5763 §5 requires the *offerer* to say `actpass` and let the answerer choose.
    if let Some(fingerprint) = capabilities.dtls() {
        capabilities.protocol().clone_into(&mut audio.protocol);
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "fingerprint",
            fingerprint.to_value(),
        ));
        audio.attributes.push(sipx_sdp::Attribute::valued(
            "setup",
            sipx_sdp::fingerprint::Setup::ActPass.as_str().to_owned(),
        ));
    }
    if capabilities.rtcp_mux {
        audio.attributes.push(sipx_sdp::Attribute::flag("rtcp-mux"));
    }
    audio.set_direction(capabilities.direction);
    sdp.media.push(audio);
    sdp
}

/// What negotiation settled on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Negotiated {
    pub(crate) remote: SocketAddr,
    pub(crate) codec: Codec,
    /// The payload type to send `codec` with, when the description gave it a number.
    ///
    /// `None` only for a bare static type matched by number. Anything an rtpmap touched —
    /// Opus always, a remapped static possibly — has no number of its own that means anything:
    /// 111 is convention, and what the far end listens for is the number *it* assigned.
    pub(crate) payload_type: Option<u8>,
    /// The payload type the far end uses for `telephone-event`, if it offered one.
    ///
    /// Taken from the description rather than assumed, because it is a *dynamic* type: 101 is
    /// what sipx offers, not what everyone uses, and assuming it would send keypresses on
    /// whatever the far end put that number to.
    pub(crate) dtmf: Option<u8>,
    /// Whether RTCP shares the RTP port or uses its adjacent control port.
    pub(crate) rtcp_mode: sipx_sdp::RtcpMode,
}

/// What negotiation settled on, plus the keys — which are not `Copy` and do not belong in a
/// type that is.
#[derive(Debug, Clone)]
pub(crate) struct Settled {
    pub(crate) negotiated: Negotiated,
    srtp: Option<sipx_media::SrtpKeys>,
}

impl Negotiated {
    /// The number this codec actually goes out with: the one the description assigned, or the
    /// codec's own when it is a static type nothing remapped.
    ///
    /// Mirrors [`sipx_media::Config::wire_payload_type`], which is what the session reads — so this
    /// is the value to compare when asking whether the wire changed. The raw [`Self::payload_type`]
    /// is not: `Some(0)` and `None` are two descriptions of PCMU and the same byte on the wire.
    fn wire_payload_type(&self) -> u8 {
        self.payload_type
            .unwrap_or_else(|| self.codec.payload_type())
    }

    fn media_config(self) -> sipx_media::Config {
        let mut config = sipx_media::Config::new(self.remote, self.codec);
        config.payload_type = self.payload_type;
        config.dtmf_payload_type = self.dtmf;
        config.rtcp_mode = self.rtcp_mode;
        config
    }
}

impl Settled {
    /// Whether both halves of the keying are present, so the media is actually encrypted.
    pub(crate) fn is_encrypted(&self) -> bool {
        self.srtp.is_some()
    }

    pub(crate) fn media_config(&self) -> sipx_media::Config {
        let mut config = self.negotiated.media_config();
        config.srtp.clone_from(&self.srtp);
        config
    }
}

/// The keys an answer to *our* offer settles on, once it has been checked against what we sent.
///
/// RFC 4568 §5.1.3 makes the check a MUST on the offerer, and this is the only place a call can
/// run it: [`sipx_media::SrtpKeys::from_answer`] is the sole route from an answer to keys, and it
/// returns which of *our* offers the answer accepted, so the half we key with is the half we sent
/// rather than whichever one happened to be first. `docs/specs/srtp.md` §5.4.
///
/// `offered` is a slice and not one attribute because that is what the check takes. sipx offers
/// exactly one today, and a function that quietly assumed so would have to be found again the day
/// it offers two.
///
/// `Ok(None)` means this side offered no key at all — a plain call, which is the only case where
/// the absence of an `a=crypto` in the answer is not a failure. When we did offer, an answer
/// carrying nothing usable is refused: that is the shape "a suite that was never offered" arrives
/// in, since [`sipx_sdp::crypto::Crypto::parse`] refuses a suite sipx cannot key.
///
/// # Errors
///
/// [`Error::Sdp`] when the answer accepted a tag and suite this side never offered, or carried no
/// key. Not `None`: dropping to an unencrypted call would hand the user an insecure call presented
/// as a secure one, and dropping the stream would end the call with nothing anyone can act on.
pub(crate) fn srtp_keys(
    offered: &[sipx_sdp::crypto::Crypto],
    answered: Option<&sipx_sdp::crypto::Crypto>,
) -> Result<Option<sipx_media::SrtpKeys>> {
    if offered.is_empty() {
        // Nothing was offered, so there is nothing to verify and no local half to key with. An
        // answer cannot introduce SDES the offer did not ask for (RFC 4568 §5.1.2).
        return Ok(None);
    }
    sipx_media::SrtpKeys::from_answer(offered, answered)
        .map(Some)
        .map_err(|error| Error::Sdp(error.to_string()))
}

/// Pair the key we are *answering* with against the far end's offered one.
///
/// The other side of [`srtp_keys`], and deliberately not the same function. §5.1.3's check is the
/// offerer's: here this side chose the attribute and echoed its tag ([`sipx_sdp::answer`], RFC
/// 4568 §5.1.2), so there is nothing to verify — only two halves to put together.
///
/// `None` unless *both* are present. One key is not a session: a stream keyed at one end only
/// is a stream the other end cannot read, and treating a half-offer as success would produce a
/// call that connects and carries silence.
pub(crate) fn srtp_keys_answering(
    ours: Option<&sipx_sdp::crypto::Crypto>,
    theirs: Option<sipx_sdp::crypto::Crypto>,
) -> Option<sipx_media::SrtpKeys> {
    let (ours, theirs) = (ours?, theirs?);
    Some(sipx_media::SrtpKeys {
        local: (ours.master_key().to_vec(), ours.master_salt().to_vec()),
        remote: (theirs.master_key().to_vec(), theirs.master_salt().to_vec()),
    })
}

/// The keying the far end offered, from its description. Same shape as the answered one; named
/// separately because reading it from an *offer* and from an *answer* are different moments.
pub(crate) fn offer_crypto(sdp: &SessionDescription) -> Option<sipx_sdp::crypto::Crypto> {
    answered_crypto(sdp)
}

/// The keying the far end answered with, from its description.
fn answered_crypto(sdp: &SessionDescription) -> Option<sipx_sdp::crypto::Crypto> {
    sdp.media
        .iter()
        .find(|m| m.media == "audio" && !m.is_rejected())?
        .crypto()
}

/// The payload type carrying `telephone-event`, per the description's own rtpmaps.
fn telephone_event_payload_type(audio: &sipx_sdp::MediaDescription) -> Option<u8> {
    audio.formats.iter().find_map(|format| {
        let mapping = audio.rtpmap(format)?;
        let encoding = mapping.split('/').next().unwrap_or(mapping);
        encoding
            .eq_ignore_ascii_case("telephone-event")
            .then(|| format.parse::<u8>().ok())
            .flatten()
    })
}

/// Where to send media, and in what codec, from a description.
///
/// `codecs` is the set this side offered or answered from: negotiation may only settle on a
/// codec the application selected, so an Opus offer answered from a G.711 set settles on
/// G.711, not on a codec the answer never named.
pub(crate) fn negotiated(sdp: &SessionDescription, codecs: Codecs) -> Result<Negotiated> {
    let audio = sdp
        .media
        .iter()
        .find(|m| m.media == "audio" && !m.is_rejected())
        .ok_or(Error::NoCommonCodec)?;

    // A stream marked `inactive` carries nothing in either direction. Treating it as a working
    // call means holding a media session open for audio that will never come.
    if audio.direction() == Direction::Inactive {
        return Err(Error::NoCommonCodec);
    }

    let address = sdp.address_for(audio).ok_or(Error::NoCommonCodec)?;

    // The first format both sides can carry. The list is already in the offerer's preference
    // order, so the first playable one is the one to use. Playable is judged by what the
    // format's rtpmap says, never by a dynamic number alone — which is also the reason
    // `Codec::from_payload_type` deliberately never returns Opus: 111 is Opus here only because
    // this description said so.
    //
    // `sipx_sdp::answer` decides the same question when it builds the answer that goes on the
    // wire, and the two *must* agree: this settles what the session sends, and the answer is what
    // the far end was told to expect. They now agree by construction — both ask
    // `sipx_sdp::rtpmap::same_format` whether an offered rtpmap names a format this side has, so
    // there is one rule rather than two readings of it (`M-31`). What is left of the difference is
    // deliberate and one-directional: the answer also names `telephone-event`, which is not a
    // codec to settle on. `the_answer_and_the_negotiated_codec_agree` holds the agreement over a
    // table of offers, so this paragraph is a claim with a test under it rather than a hope.
    //
    // `carries` is part of the search and not a test applied to its result. Rejecting afterwards
    // would stop at the offerer's first choice and refuse the whole description if that one
    // format is outside our set — so an Opus-first offer reaching a G.711 call would come back
    // `NoCommonCodec` while the answer this side builds happily names the PCMU further down the
    // same list.
    let (codec, payload_type) = audio
        .formats
        .iter()
        .find_map(|format| codec_of(audio, format).filter(|(codec, _)| codecs.carries(*codec)))
        .ok_or(Error::NoCommonCodec)?;

    Ok(Negotiated {
        remote: SocketAddr::new(address, audio.port),
        codec,
        payload_type,
        dtmf: telephone_event_payload_type(audio),
        // On the answering side this is the offer's request, which sipx accepts. On the offering
        // side `settle_answer` additionally requires that this side actually offered the flag.
        rtcp_mode: if audio.rtcp_mux() {
            sipx_sdp::RtcpMode::Mux
        } else {
            sipx_sdp::RtcpMode::Separate
        },
    })
}

/// The codec a format names, and the payload type to put on the wire for it.
///
/// A format with an rtpmap is matched by the map: RFC 8866 §6.6 makes it authoritative even
/// for a static number, which is how an offer of `8` meaning iLBC is not read as PCMA. The
/// number is then *dynamic in meaning* — the map could have hung any name on it — so it goes
/// home with the codec rather than being reassumed from [`Codec::payload_type`]. Only a bare
/// static type, with no map at all, is matched by number.
fn codec_of(audio: &sipx_sdp::MediaDescription, format: &str) -> Option<(Codec, Option<u8>)> {
    let payload = format.parse::<u8>().ok()?;
    if let Some(rtpmap) = audio.rtpmap(format) {
        return codec_named(rtpmap).map(|codec| (codec, Some(payload)));
    }
    Codec::from_payload_type(payload).map(|codec| (codec, None))
}

/// The codec an rtpmap value names, if it is one we carry.
///
/// **The matching rule is not written here.** [`sipx_sdp::rtpmap::same_format`] decides whether two
/// `a=rtpmap` values name the same format, and this asks it once per codec sipx can run, against
/// the value that codec is offered with. It used to be written out a second time in this function,
/// with the clock rate parsed to a `u32` where [`sipx_sdp::answer`] compared the same field as
/// text — so the answer on the wire and the codec the session was built with could name different
/// formats for one offer (`M-31`).
///
/// `sipx-sdp` is the authority and not this crate, because the dependency only runs one way:
/// [`sipx_sdp::answer`] builds the answer sipx sends and cannot call up into `sipx-call`, so the
/// only arrangement in which one implementation serves both is the lower crate holding it. What
/// stays here is the half `sipx-sdp` must not learn — which rtpmaps sipx has a codec for, and
/// which codecs the application selected.
///
/// The order of the search does not matter: the values in [`carried`] are distinct formats, so an
/// rtpmap matches at most one of them. Preference order is the *offerer's*, and it is applied by
/// [`negotiated`] walking `m=`'s format list.
fn codec_named(rtpmap: &str) -> Option<Codec> {
    carried()
        .iter()
        .copied()
        .find(|&codec| sipx_sdp::rtpmap::same_format(rtpmap, offered_rtpmap(codec)))
}

/// Every codec sipx can run, and can therefore read out of an rtpmap.
///
/// Omitting a new [`Codec`] variant here means it is simply never named by an offer, which is the
/// safe direction to fail in — the same reasoning as [`Codecs::carries`]. The exhaustive match in
/// [`offered_rtpmap`] is what forces someone to decide.
fn carried() -> &'static [Codec] {
    &[
        Codec::Pcmu,
        Codec::Pcma,
        #[cfg(feature = "opus")]
        Codec::Opus,
    ]
}

/// The `a=rtpmap` value sipx offers a codec with.
///
/// The same strings [`sipx_sdp::Capabilities::g711`] and [`sipx_sdp::Capabilities::with_opus`] put
/// on the wire, and they have to be: a codec whose value here disagreed with the one offered would
/// be a codec negotiation settles on and no answer ever names, which is the whole of `M-31`.
/// `the_answer_and_the_negotiated_codec_agree` is what holds the two together, rather than a
/// comment asking them to match.
///
/// RFC 7587 §7 fixes Opus's RTP clock at 48000 and its rtpmap channel count at 2 whatever the
/// audio actually is, so `opus/16000` is nothing we have however it is numbered.
const fn offered_rtpmap(codec: Codec) -> &'static str {
    match codec {
        Codec::Pcmu => "PCMU/8000",
        Codec::Pcma => "PCMA/8000",
        #[cfg(feature = "opus")]
        Codec::Opus => "opus/48000/2",
    }
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

/// Answer an INVITE that asks to take the place of an existing call (RFC 3891).
///
/// The second half of an attended transfer: the transferor has spoken to the target, and hands
/// its original call over by telling one party to call the other with a `Replaces` header
/// naming the dialog to displace.
///
/// **The header must name `replaced`, all three fields of it.** A `Call-ID` travels in every
/// message of a dialog and is visible to every element on the path; the tags are random and
/// known only to the two parties. Accepting a match on the `Call-ID` alone — or trusting the
/// caller to have checked — turns this into a call-hijack primitive, so the check is here and
/// not in whoever calls it.
///
/// On success the replaced call is hung up and its media torn down. On failure the new INVITE
/// is refused and the existing call is left exactly as it was: a replacement that cannot be
/// honoured must not cost the user the call they already had.
///
/// Answers from the default codec set, [`Codecs::G711`]. [`answer_replacing_with`] takes a
/// selection.
pub async fn answer_replacing(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    replaced: &mut Call,
) -> Result<Call> {
    answer_replacing_with(
        endpoint,
        incoming,
        media_address,
        replaced,
        Codecs::default(),
    )
    .await
}

/// [`answer_replacing`], from a chosen codec set rather than the default one (`M-30`).
///
/// `codecs` applies to the *replacement*, which is the only call being negotiated here. The one
/// being replaced is hung up, and nothing renegotiates it on the way out.
pub async fn answer_replacing_with(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    replaced: &mut Call,
    codecs: Codecs,
) -> Result<Call> {
    let Some(asked_for) = Replaces::of(&incoming.request) else {
        refuse_request(endpoint, incoming, 400, "Bad Request").await?;
        return Err(Error::NoReplaces);
    };

    if !asked_for.matches(&replaced.dialog) {
        // 481, which RFC 3891 §3 asks for and which also gives nothing away: a caller guessing
        // tags gets the same answer whether the Call-ID was right or not, so there is nothing
        // to search.
        refuse_request(endpoint, incoming, 481, "Call/Transaction Does Not Exist").await?;
        return Err(Error::NoReplaces);
    }

    // Answer first. If this fails the old call is untouched, which is the right way round:
    // hanging up first and then failing to answer would leave the user with no call at all.
    let taken_over = answer_with(endpoint, incoming, media_address, codecs).await?;

    // Then end the one being replaced (RFC 3891 §3). Its media stops with it.
    //
    // discard: the BYE this sends is counted at the transmit as
    // `sipx_transport::UnsentCounts::bye` if the endpoint cannot put it on the wire. The `Result`
    // is discarded because the takeover has already succeeded on the line above and reporting a
    // teardown failure as the *transfer* failing would be false — the caller has the new call
    // either way, and `Call::end` has already marked the old one ended locally before the BYE was
    // ever built.
    let _ = replaced.hang_up().await;

    Ok(taken_over)
}

/// Refuse a request outright.
async fn refuse_request(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
) -> Result<()> {
    let Some(code) = StatusCode::new(status) else {
        return Ok(());
    };
    let response = ResponseBuilder::to_request(&incoming.request, code, reason)?.build();
    endpoint.respond(&incoming.key, response).await?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

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

    /// An audio description with the given formats and rtpmaps, as a peer would send it.
    fn offered(formats: &str, rtpmaps: &[&str]) -> SessionDescription {
        let mut body = format!(
            "v=0\r\n\
             o=- 1 1 IN IP4 192.0.2.1\r\n\
             s=-\r\n\
             c=IN IP4 192.0.2.1\r\n\
             t=0 0\r\n\
             m=audio 40000 RTP/AVP {formats}\r\n"
        );
        for rtpmap in rtpmaps {
            let _ = write!(body, "a=rtpmap:{rtpmap}\r\n");
        }
        sipx_sdp::parse(&body).expect("a description this test wrote")
    }

    /// The default is the G.711 pair, in every build. The `opus` feature adds a variant to
    /// [`Codecs`]; it must never move which one `Default` produces, or turning the feature on to
    /// get the *option* of Opus would silently change what every existing call offers.
    #[test]
    fn the_default_codec_set_is_g711() {
        assert_eq!(Codecs::default(), Codecs::G711);
        let capabilities = Codecs::default().capabilities("192.0.2.9".parse().unwrap(), 40000);
        assert!(
            !capabilities
                .rtpmaps
                .iter()
                .any(|(_, value)| value.to_ascii_lowercase().contains("opus")),
            "the default offer names no Opus: {:?}",
            capabilities.rtpmaps
        );
    }

    /// RFC 8866 §6.6 makes the rtpmap authoritative even over a static number. This is the rule
    /// that lets an Opus offer arrive at all — 111 means Opus only because the description said
    /// so — and the same rule refuses to read an offer of `8` remapped to something else as PCMA.
    #[test]
    fn a_format_is_read_from_its_rtpmap_and_not_from_its_number() {
        let remapped = offered("8 0", &["8 iLBC/8000", "0 PCMU/8000"]);
        let settled = negotiated(&remapped, Codecs::G711).expect("PCMU is common");
        assert_eq!(settled.codec, Codec::Pcmu);
        assert_eq!(
            settled.payload_type,
            Some(0),
            "the number the far end assigned travels with the codec"
        );
    }

    /// A bare static type with no rtpmap at all is the one case matched by number, which is what
    /// keeps every G.711-only peer that sends `m=audio … 0 8` and nothing else working.
    #[test]
    fn a_bare_static_type_is_still_matched_by_number() {
        let settled = negotiated(&offered("0", &[]), Codecs::G711).expect("PCMU is static");
        assert_eq!(settled.codec, Codec::Pcmu);
        assert_eq!(
            settled.payload_type, None,
            "nothing named it, so nothing overrides `Codec::payload_type`"
        );
    }

    /// The clock rate and channel count are part of a format's identity (RFC 8866 §6.6), so a
    /// name sipx knows at a rate it does not is not a match.
    #[test]
    fn a_known_name_at_an_unknown_clock_rate_is_not_a_match() {
        assert_eq!(codec_named("PCMU/16000"), None);
        assert_eq!(codec_named("opus/16000/2"), None);
        assert_eq!(codec_named("PCMU/8000"), Some(Codec::Pcmu));
        assert_eq!(codec_named("pcma/8000"), Some(Codec::Pcma));
    }

    /// The default build has no Opus, so an offer of it is not a codec that build can carry —
    /// and the offer is answered from what *is* common rather than refused. This is the promise
    /// the `opus` feature is off by default in order to make: `tests/opus.rs` is gated on the
    /// feature and cannot assert anything about the build that lacks it.
    #[cfg(not(feature = "opus"))]
    #[test]
    fn a_default_build_does_not_carry_an_offered_opus() {
        assert_eq!(codec_named("opus/48000/2"), None);
        let opus_first = offered("111 0", &["111 opus/48000/2", "0 PCMU/8000"]);
        let settled = negotiated(&opus_first, Codecs::G711).expect("G.711 is still offered");
        assert_eq!(settled.codec, Codec::Pcmu, "the first format sipx carries");
    }

    /// Selecting a set is what puts a codec on the table, and negotiation may not step outside
    /// it. An Opus offer answered from [`Codecs::G711`] settles on G.711 — not because Opus is
    /// absent from the build, but because the answer this side builds never named it, and a
    /// session started on a codec no answer named sends packets the far end cannot place.
    #[cfg(feature = "opus")]
    #[test]
    fn negotiation_does_not_settle_outside_the_selected_set() {
        assert_eq!(codec_named("opus/48000/2"), Some(Codec::Opus));
        let opus_first = offered("111 0", &["111 opus/48000/2", "0 PCMU/8000"]);

        let from_g711 = negotiated(&opus_first, Codecs::G711).expect("G.711 is still offered");
        assert_eq!(from_g711.codec, Codec::Pcmu);

        let from_opus = negotiated(&opus_first, Codecs::Opus).expect("Opus is on the table");
        assert_eq!(from_opus.codec, Codec::Opus);
        assert_eq!(
            from_opus.payload_type,
            Some(111),
            "on the number this offer assigned, not on a number 111 means by itself"
        );
    }

    /// A peer may spell a static type either way — `m=audio … 0` alone, or the same thing with a
    /// redundant `a=rtpmap:0 PCMU/8000` — and RFC 8866 §6.6 allows both for the same codec.
    ///
    /// So moving between the two spellings is not a *change*, and [`Call::move_media_if_changed`]
    /// must not rebuild the session for it: rebuilding costs an audible gap, and some peers
    /// re-INVITE every thirty seconds as a keep-alive. `negotiated` does record the difference —
    /// `Some(0)` against `None`, which is a true fact about what the description said — so the
    /// comparison is on [`Negotiated::wire_payload_type`], where the two collapse to the one byte
    /// that actually goes out.
    #[test]
    fn a_redundant_rtpmap_for_a_static_type_is_not_a_change() {
        let mapped = negotiated(&offered("0", &["0 PCMU/8000"]), Codecs::G711).expect("PCMU");
        let bare = negotiated(&offered("0", &[]), Codecs::G711).expect("PCMU");

        assert_eq!(mapped.codec, bare.codec);
        assert_eq!(mapped.payload_type, Some(0), "the rtpmap named it");
        assert_eq!(bare.payload_type, None, "nothing named it");
        assert_eq!(
            mapped.wire_payload_type(),
            bare.wire_payload_type(),
            "the same byte goes on the wire either way, so the session must not move",
        );
    }

    /// An *answer* naming a codec outside the selected set is refused, so nothing keys a session
    /// on it.
    ///
    /// Pinned separately from `negotiated` because of where the refusal lands rather than what it
    /// returns. It is a failure mode `M-30` adds to `settle_answer`, which had no codec opinion
    /// before; on this branch an early answer that trips it is swallowed by
    /// `Dialing::adopt_early_answer`, but that function propagates on `main` after `S-25`, so once
    /// the two are merged this same refusal ends the invitation over a CANCEL. That is a call
    /// termination neither branch produces alone, which is why the precondition is worth holding
    /// here rather than waiting for the merge to discover it.
    ///
    /// True in both feature configurations for two different reasons: with `opus` off no rtpmap can
    /// name Opus at all, and with it on `Codecs::G711` does not carry it.
    #[test]
    fn an_answer_outside_the_selected_set_is_refused() {
        let opus_only = offered("111", &["111 opus/48000/2"]);
        let capabilities =
            Capabilities::g711("127.0.0.1".parse().expect("loopback address"), 40_000);
        assert!(matches!(
            settle_answer(&capabilities, &opus_only, Codecs::G711),
            Err(Error::NoCommonCodec)
        ));
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

    #[test]
    fn the_answer_settles_mux_or_the_separate_port_fallback_without_a_retry() {
        let capabilities =
            Capabilities::g711("127.0.0.1".parse().expect("loopback address"), 40_000)
                .with_rtcp_mux();
        let separate_answer = offered("0", &["0 PCMU/8000"]);
        let separate = settle_answer(&capabilities, &separate_answer, Codecs::G711)
            .expect("the answer remains usable");
        assert_eq!(separate.negotiated.rtcp_mode, sipx_sdp::RtcpMode::Separate);

        let mut mux_answer = separate_answer;
        mux_answer
            .media
            .first_mut()
            .expect("audio answer")
            .attributes
            .push(sipx_sdp::Attribute::flag("rtcp-mux"));
        let mux = settle_answer(&capabilities, &mux_answer, Codecs::G711)
            .expect("the muxed answer remains usable");
        assert_eq!(mux.negotiated.rtcp_mode, sipx_sdp::RtcpMode::Mux);

        let not_offered =
            Capabilities::g711("127.0.0.1".parse().expect("loopback address"), 40_000);
        let unasked = settle_answer(&not_offered, &mux_answer, Codecs::G711)
            .expect("an unasked attribute does not break the answer");
        assert_eq!(
            unasked.negotiated.rtcp_mode,
            sipx_sdp::RtcpMode::Separate,
            "an answer cannot negotiate a feature that was not offered"
        );
    }

    /// A running one-port session cannot accept an in-dialog offer that drops mux while retaining
    /// its old socket owner. The same typed guard is used before inbound state is applied.
    #[test]
    fn an_inbound_reoffer_cannot_remove_the_running_mux_mode() {
        let offered_without_mux = offered("0", &["0 PCMU/8000"]);
        let answered_without_mux = offered("0", &["0 PCMU/8000"]);
        let proposed = exchanged_rtcp_mode(&offered_without_mux, &answered_without_mux);

        assert!(matches!(
            preserve_rtcp_mode(sipx_sdp::RtcpMode::Mux, proposed),
            Err(Error::RtcpModeChange {
                current: sipx_sdp::RtcpMode::Mux,
                proposed: sipx_sdp::RtcpMode::Separate,
            })
        ));
    }

    /// The outbound mirror: omission in an answer to a later offer is an explicit failure and
    /// leaves the established mux session in place instead of binding an unadvertised replacement.
    #[test]
    fn an_outbound_reoffer_answer_cannot_remove_the_running_mux_mode() {
        let answer_without_mux = offered("0", &["0 PCMU/8000"]);
        let renegotiated = negotiated(&answer_without_mux, Codecs::G711).expect("PCMU answer");

        assert!(matches!(
            preserve_rtcp_mode(sipx_sdp::RtcpMode::Mux, renegotiated.rtcp_mode),
            Err(Error::RtcpModeChange {
                current: sipx_sdp::RtcpMode::Mux,
                proposed: sipx_sdp::RtcpMode::Separate,
            })
        ));
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
                payload_type: Some(0),
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

    /// An offer with nothing sipx carries is refused rather than answered on a guess.
    #[test]
    fn an_offer_of_nothing_we_carry_has_no_common_codec() {
        let g729 = offered("18", &["18 G729/8000"]);
        assert!(matches!(
            negotiated(&g729, Codecs::G711),
            Err(Error::NoCommonCodec)
        ));
    }

    /// One row of the agreement table: an offer, and the set the application selected.
    ///
    /// The property is in [`tests::the_answer_and_the_negotiated_codec_agree`]. The rows exist so
    /// it is held against a *class* of rtpmap spellings rather than the one spelling that happened
    /// to be found — `M-31` was filed because a fix aimed at `08000` alone would leave the shape
    /// in place.
    struct Agreement {
        /// Why this row is in the table. Quoted in every failure, because a table-driven
        /// assertion that only prints the values makes the reader guess what was being tested.
        why: &'static str,
        /// The `m=audio` format list, in the offerer's preference order.
        formats: &'static str,
        /// The offer's `a=rtpmap` attribute values.
        rtpmaps: &'static [&'static str],
        /// The set the application selected for this call.
        codecs: Codecs,
    }

    /// The offers the agreement must hold over, in every build.
    ///
    /// Derived from `docs/specs/sdp-format-identity.md` §4.4's vectors. A `const` table rather than
    /// a function that builds one: it is data, and a hundred lines of data is not a hundred lines
    /// of control flow for anyone reading it — or for `clippy::too_many_lines`.
    const AGREEMENT_TABLE: &[Agreement] = &[
        Agreement {
            why: "a clock rate with a leading zero is the same rate — `08000` and `8000` are \
                      numerically equal and textually different, which is the split M-31 was \
                      filed for",
            formats: "0 8",
            rtpmaps: &["0 PCMU/08000", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "the same split in the *channel* field, so a fix aimed at the clock rate \
                      alone does not close the story",
            formats: "0 8",
            rtpmaps: &["0 PCMU/8000/01", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an offer that puts a codec sipx does not carry first: both rules must skip \
                      it and settle further down the list, not refuse the stream",
            formats: "18 0",
            rtpmaps: &["18 G729/8000", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a dynamic number carrying a codec sipx does have — 96 means PCMU here only \
                      because this offer said so (RFC 8866 §6.6), and both rules must read the \
                      map rather than the number",
            formats: "96 0",
            rtpmaps: &["96 PCMU/8000", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a bare static type, the one case with no rtpmap for either rule to read",
            formats: "0",
            rtpmaps: &[],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "mono spelled out where RFC 8866 §6.6 would have let it be implied",
            formats: "0",
            rtpmaps: &["0 PCMU/8000/1"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "stereo G.711 is a different format from mono G.711, and neither rule may \
                      settle on it",
            formats: "0 8",
            rtpmaps: &["0 PCMU/8000/2", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a signed clock rate is not a decimal digit string, so it identifies nothing for \
                  either rule. The *third* witness, and the one that was not predicted: \
                  `u32::from_str` accepts a leading `+`, so the parsing rule read `+8000` as 8000 \
                  while the textual one did not — the same split as a leading zero, arrived at from \
                  the other side. It is why the digits are checked in `sipx-sdp` rather than left \
                  to `from_str`, and note the single rule resolves it the *opposite* way from a \
                  leading zero: both callers decline it, and both settle on PCMA below",
            formats: "0 8",
            rtpmaps: &["0 PCMU/+8000", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a clock rate that overflows u32 — hostile input, and a non-match for both \
                      rules rather than a panic in either",
            formats: "0 8",
            rtpmaps: &["0 PCMU/99999999999999", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an rtpmap with no clock rate at all identifies nothing",
            formats: "0 8",
            rtpmaps: &["0 PCMU", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an empty clock rate is not zero and is not 8000",
            formats: "0 8",
            rtpmaps: &["0 PCMU/", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "whitespace inside the value: a rate neither rule can read, and both must \
                      fail to read it the same way",
            formats: "0 8",
            rtpmaps: &["0 PCMU/ 8000", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a fourth field is outside RFC 8866 §6.6's grammar, so the value identifies \
                      nothing — and must do so for both rules rather than one silently ignoring it",
            formats: "0 8",
            rtpmaps: &["0 PCMU/8000/1/9", "8 PCMA/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an Opus-first offer reaching a call that selected G.711: the M-30 case, and \
                      true in both feature configurations — with `opus` off no rtpmap can name it, \
                      with it on the set does not carry it",
            formats: "111 0",
            rtpmaps: &["111 opus/48000/2", "0 PCMU/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a dynamic number with no rtpmap is uninterpretable whatever the number, so \
                      the stream is refused rather than guessed at",
            formats: "111",
            rtpmaps: &[],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "a stream offering only telephone-event is not a call: the answer rejects it \
                      and negotiation must refuse it too",
            formats: "101",
            rtpmaps: &["101 telephone-event/8000"],
            codecs: Codecs::G711,
        },
        Agreement {
            why: "an offer of nothing sipx carries at all — both rules refuse, and the \
                      agreement holds on the refusing side as well",
            formats: "18",
            rtpmaps: &["18 G729/8000"],
            codecs: Codecs::G711,
        },
    ];

    /// The rows that only exist when the `opus` feature is on, because [`Codecs::Opus`] does.
    ///
    /// Empty in the default build rather than absent, so the test body has no `cfg` in it and the
    /// two configurations run the same code over different data.
    #[cfg(feature = "opus")]
    const OPUS_AGREEMENT_TABLE: &[Agreement] = &[
        Agreement {
            why: "Opus on the set that carries it, on the number this offer assigned",
            formats: "111 0",
            rtpmaps: &["111 opus/48000/2", "0 PCMU/8000"],
            codecs: Codecs::Opus,
        },
        Agreement {
            why: "the leading-zero split on Opus's own clock rate, so the class is closed in the \
                  gated path too and not only for G.711",
            formats: "111 0",
            rtpmaps: &["111 opus/048000/2", "0 PCMU/8000"],
            codecs: Codecs::Opus,
        },
        Agreement {
            why: "Opus at a rate RFC 7587 §7 does not assign is nothing sipx has, whatever number \
                  is beside it",
            formats: "111 0",
            rtpmaps: &["111 opus/16000/2", "0 PCMU/8000"],
            codecs: Codecs::Opus,
        },
    ];

    /// No Opus in this build, so no Opus rows. See [`OPUS_AGREEMENT_TABLE`].
    #[cfg(not(feature = "opus"))]
    const OPUS_AGREEMENT_TABLE: &[Agreement] = &[];

    /// The answer sipx puts on the wire and the codec it configures the media session with must
    /// name the same format. **`M-31`'s failing-first test.**
    ///
    /// This is the assertion that fails while the two rules disagree: with the answer comparing an
    /// rtpmap clock rate as text and `codec_named` parsing it to `u32`, an offer of
    /// `a=rtpmap:0 PCMU/08000` settles on `Pcmu` at payload type 0 while the answer names only
    /// `8`. sipx would then send µ-law on a number the answer never offered *and* decode the
    /// peer's PCMA through a µ-law session — audible garbage rather than silence, with nothing in
    /// the stack reporting an error.
    ///
    /// The property is a biconditional, not a one-way check, because both halves are reachable
    /// defects: a codec the answer never named is a session the far end cannot read, and a stream
    /// the answer accepted while negotiation refused it is a call that fails after the 200 OK went
    /// out. `wire_payload_type` is the value compared because that is the byte that leaves —
    /// `Some(0)` and `None` are two descriptions of the same PCMU.
    #[test]
    fn the_answer_and_the_negotiated_codec_agree() {
        let local: IpAddr = "192.0.2.9".parse().expect("a literal address");

        for row in AGREEMENT_TABLE.iter().chain(OPUS_AGREEMENT_TABLE) {
            let offer = offered(row.formats, row.rtpmaps);
            let answered = sipx_sdp::answer(&offer, &row.codecs.capabilities(local, 40000));
            let audio = answered
                .media
                .iter()
                .find(|stream| stream.media == "audio")
                .expect("the answer has one m= line per offered stream");

            match negotiated(&offer, row.codecs) {
                Ok(settled) => {
                    assert!(
                        !audio.is_rejected(),
                        "{}: negotiation settled on {:?} while the answer rejected the stream",
                        row.why,
                        settled.codec,
                    );
                    let wire = settled.wire_payload_type().to_string();
                    assert!(
                        audio.formats.contains(&wire),
                        "{}: negotiation settled on {:?} at payload type {wire}, which the answer \
                         never named ({:?})",
                        row.why,
                        settled.codec,
                        audio.formats,
                    );
                }
                Err(error) => {
                    assert!(
                        audio.is_rejected(),
                        "{}: negotiation refused the stream ({error}) while the answer accepted it \
                         with formats {:?}",
                        row.why,
                        audio.formats,
                    );
                }
            }
        }
    }
}
