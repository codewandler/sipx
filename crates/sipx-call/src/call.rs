//! Establishing a call: INVITE with an SDP offer, media bound to the answer, and BYE.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;

use bytes::Bytes;
use sipx_media::{Codec, Interrupt, MediaPort, MediaSession, Playback};
use sipx_sdp::{Capabilities, Direction, SessionDescription};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::session::{self, MinSe, SessionExpires};
use sipx_sip::update::{self, Reception};
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target, TransportKind};

use crate::dialog::{Dialog, strip_header_params};
use crate::error::{Error, Result};
use crate::event::{CallEvent, CallEvents, EndCause, EventSink};
use crate::transfer::{
    Referral, Replaces, Transfer, TransferState, is_terminated, parse_sipfrag, sipfrag,
};

/// 200 OK.
///
/// `StatusCode::new` is fallible because most codes come from the wire; this one is a literal
/// that is always in range. Threading a `Result` out of every call site for it would mean
/// inventing an error that can never happen — and the previous attempt reported it as "no
/// final response to the INVITE", which would have been actively misleading.
pub(crate) fn ok_status() -> StatusCode {
    const OK: u16 = 200;
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
pub(crate) fn token() -> String {
    use rand::Rng as _;
    let value: u64 = rand::rng().random();
    format!("{value:016x}")
}

/// A call in progress.
#[derive(Debug)]
pub struct Call {
    /// The dialog it runs in.
    pub dialog: Dialog,
    media: MediaSession,
    endpoint: Handle,
    /// Where in-dialog requests go: the peer's `Contact`, not where the INVITE was sent.
    target: Target,
    /// Set while a 2xx is still being retransmitted; cleared when the ACK arrives.
    awaiting_ack: Option<Arc<tokio::sync::Notify>>,
    ended: bool,
    /// Where this side receives media, so a re-offer can name the same address.
    media_address: IpAddr,
    /// What the running session negotiated, for comparison against a re-offer.
    current: Negotiated,
    /// Whether the call is on hold, and which way.
    hold: Direction,
    /// Whether the media is encrypted.
    encrypted: bool,
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
    /// The audio.
    #[must_use]
    pub fn media(&self) -> &MediaSession {
        &self.media
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
    async fn renegotiate(&mut self, body: &[u8]) -> Result<Option<SessionDescription>> {
        let Ok(offer) = sipx_sdp::parse(&String::from_utf8_lossy(body)) else {
            return Ok(None);
        };
        let Ok(renegotiated) = negotiated(&offer) else {
            return Ok(None);
        };

        let capabilities = Capabilities::g711(self.media_address, self.media.local_addr().port());
        let answer_sdp = sipx_sdp::answer(&offer, &capabilities);
        if answer_sdp
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return Ok(None);
        }

        // Hold is a direction, not a separate state: `sendonly` or `inactive` from the far end
        // means it will not play what we send.
        let was_on_hold = self.is_on_hold();
        self.hold = offer
            .media
            .iter()
            .find(|m| m.media == "audio" && !m.is_rejected())
            .map_or(Direction::SendRecv, sipx_sdp::MediaDescription::direction);
        // Emitted right where `hold` changes, not by polling it afterwards — a renegotiation
        // that does not change the direction (a keep-alive, say) must not report a hold that
        // never happened.
        match (was_on_hold, self.is_on_hold()) {
            (false, true) => self.events.emit(CallEvent::Hold),
            (true, false) => self.events.emit(CallEvent::Resumed),
            _ => {}
        }

        self.move_media_if_changed(renegotiated).await?;
        Ok(Some(answer_sdp))
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
        if !self.negotiation.may_offer() {
            return Err(Error::Rejected {
                status: sipx_sip::update::Refusal::Glare.status(),
                reason: "an offer is already outstanding on this dialog".to_owned(),
            });
        }

        let mut capabilities =
            Capabilities::g711(self.media_address, self.media.local_addr().port());
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

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            && let Ok(renegotiated) = negotiated(&answer)
        {
            self.move_media_if_changed(renegotiated).await?;
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
    async fn refuse_unclaimed(&self, incoming: &Incoming) {
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
                if let Err(error) = self.endpoint.respond(&incoming.key, builder.build()).await {
                    tracing::warn!(%error, code, "could not refuse an unclaimed request");
                }
            }
            Err(error) => tracing::warn!(%error, code, "could not build the refusal"),
        }
    }

    /// Refuse a renegotiation without ending the call.
    async fn refuse(&self, incoming: &Incoming, code: u16, reason: &'static str) -> Result<()> {
        let status = StatusCode::new(code).unwrap_or_else(ok_status);
        let response = ResponseBuilder::to_request(&incoming.request, status, reason)?.build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// Rebuild the media session, but only if where or how the media flows actually changed.
    ///
    /// Restarting an unchanged session would drop packets for no reason on every re-INVITE, and
    /// some peers send one every thirty seconds as a keep-alive.
    async fn move_media_if_changed(&mut self, to: Negotiated) -> Result<()> {
        if to.remote != self.current.remote || to.codec != self.current.codec {
            let port = MediaPort::bind(SocketAddr::new(self.media_address, 0))
                .await
                .map_err(Error::Io)?;
            let replacement = port.start(to.media_config());
            // Mute is a property of the call, not of the session that happens to be carrying it
            // (`M-18`). Without this a re-INVITE that moves the media — the far end changing
            // address or codec, which this side did not ask for and cannot refuse — unmutes the
            // call behind the application's back.
            replacement.set_muted(self.media.is_muted());
            let previous = std::mem::replace(&mut self.media, replacement);
            previous.stop();
        }
        self.current = to;
        Ok(())
    }

    /// Send a re-INVITE renegotiating this call.
    ///
    /// `direction` puts the call on hold (`SendOnly` or `Inactive`) or takes it off
    /// (`SendRecv`).
    pub async fn reinvite(&mut self, direction: Direction) -> Result<()> {
        let (local, remote) = self.dialog.local_and_remote();
        let cseq = self.dialog.next_cseq();

        let mut capabilities =
            Capabilities::g711(self.media_address, self.media.local_addr().port());
        capabilities.direction = direction;
        // The session version must increase with each modified offer, so the far end can tell
        // a changed description from a repeated one.
        capabilities.session_version = u64::from(cseq);
        let offer = offer_from(&capabilities);

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

        if let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            && let Ok(renegotiated) = negotiated(&answer)
        {
            self.move_media_if_changed(renegotiated).await?;
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
        let bye = bye_request(&self.dialog, cseq)?;
        let mut responses = self.endpoint.send(bye, self.target.clone()).await?;
        // A BYE that is never answered still ends the call locally: the alternative is a call
        // that cannot be hung up because the far end has already gone.
        let _ = tokio::time::timeout(Duration::from_secs(2), responses.final_response()).await;
        Ok(())
    }

    /// End the call because this side decided to.
    pub async fn hang_up(&mut self) -> Result<()> {
        self.end(EndCause::LocalHangup).await
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
async fn sleep_until(deadline: Option<Instant>) {
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

fn bye_request(dialog: &Dialog, cseq: u32) -> Result<Request> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let builder = RequestBuilder::new(Method::Bye, uri)
        .header(HeaderName::To, Bytes::from(remote))?
        .header(HeaderName::From, Bytes::from(local))?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))?
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
    /// through a registration reaches a proxy holding no state for it.
    pub service_route: Vec<String>,
}

impl DialOptions {
    /// Options for a call from an address of record.
    #[must_use]
    pub fn new(from: impl Into<String>, media_address: IpAddr) -> Self {
        Self {
            from: from.into(),
            media_address,
            timeout: None,
            session_expires: None,
            service_route: Vec::new(),
        }
    }

    /// Traverse these proxies on the way out, outermost first (RFC 3608).
    ///
    /// The values are `Route` header values — `<sip:proxy.example;lr>` — which is what
    /// `ServiceRoute::rendered` returns. Order is normative: §6.1 requires a UA that exercises a
    /// service route to preserve the order the registrar listed.
    #[must_use]
    pub fn with_service_route(mut self, hops: Vec<String>) -> Self {
        self.service_route = hops;
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
fn offered_media(
    media_address: IpAddr,
    port: &MediaPort,
    transport: TransportKind,
) -> (Capabilities, SessionDescription) {
    let capabilities = Capabilities::g711(media_address, port.local_addr().port())
        .with_srtp(transport.is_secure());
    let offer = offer_from(&capabilities);
    (capabilities, offer)
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
    offer: &'a SessionDescription,
    session_expires: Option<Duration>,
    identity: &'a Identity,
    /// The pre-loaded route set, outermost first (RFC 3608 §6.1).
    service_route: &'a [String],
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
        .header(
            HeaderName::ContentType,
            Bytes::from_static(b"application/sdp"),
        )?
        .max_forwards(70)
        .body(Bytes::from(offer.to_string_sdp()));

    // One `Supported` row listing everything this side can do. Both tags are statements of
    // capability rather than requests: `timer` tells a far end that wants liveness detection
    // that it may have it, and `100rel` (RFC 3262 §4) is what permits the far end to send a
    // reliable provisional at all — §3 forbids it outright if we stay quiet, which means a
    // silent UAC gets unreliable ringing even from a UAS that would rather not send it.
    builder = builder.header(HeaderName::Supported, Bytes::from_static(b"timer, 100rel"))?;
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
    // Pre-loaded Route, before the request goes anywhere. RFC 3608 §6.1 has the service route
    // used "as a preloaded Route header field in outgoing initial requests", and §6.1 requires
    // the order preserved — which `add_routes` does by appending in sequence. This is the only
    // place the INVITE can acquire it: a Route added after the transaction is created would not
    // be on the message the far end matched.
    let builder = add_routes(builder, service_route)?;
    Ok(builder.build())
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
    let identity = Identity::fresh();
    match dial_with(endpoint, target.clone(), to, options, &identity).await {
        Err(Error::IntervalTooBrief(required)) => {
            let mut retried = options.clone();
            retried.session_expires = Some(required.max(session::ABSOLUTE_MIN_INTERVAL));
            dial_with(endpoint, target, to, &retried, &identity.again()).await
        }
        other => other,
    }
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
    dial_with(endpoint, target, to, options, &Identity::fresh()).await
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
) -> Result<(MediaPort, Capabilities, String, Request)> {
    // The offer has to name the port audio will arrive on, and only a bound socket knows it.
    // So the port is bound now and the session started once the answer says where and in what.
    let port = MediaPort::bind(SocketAddr::new(options.media_address, 0))
        .await
        .map_err(Error::Io)?;

    let (capabilities, offer) = offered_media(options.media_address, &port, target.transport);

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

    let invite = build_invite(
        endpoint,
        target,
        &Invitation {
            to,
            from: options.from.as_str(),
            via: &via,
            offer: &offer,
            session_expires: options.session_expires,
            identity,
            service_route: &options.service_route,
        },
    )?;
    Ok((port, capabilities, via, invite))
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
        let _ = send_cancel(endpoint, invite, via, target.clone()).await;
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
                let _ = send_cancel(endpoint, invite, via, target.clone()).await;
            }
            continue;
        }
        if late.status.is_success()
            && let Some(dialog) = Dialog::from_response(invite, &late)
        {
            let in_dialog = in_dialog_target(&dialog, target.clone());
            let _ = send_ack(endpoint, &dialog, in_dialog.clone()).await;
            if let Ok(bye) = bye_request(&dialog, dialog.local_cseq.saturating_add(1)) {
                let _ = endpoint.send(bye, in_dialog).await;
            }
        }
        break;
    }
}

async fn dial_with(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
    identity: &Identity,
) -> Result<Call> {
    let media_address = options.media_address;
    let (port, capabilities, via, invite) =
        open_invitation(endpoint, &target, to, options, identity).await?;

    let mut responses = endpoint.send(invite.clone(), target.clone()).await?;

    let mut acknowledging = Acknowledging {
        endpoint,
        invite: &invite,
        target: &target,
        capabilities: &capabilities,
        seen: sipx_sip::rel::Sequence::default(),
    };
    let (response, ringing) =
        match await_final(&mut responses, options.timeout, &mut acknowledging).await {
            Waited::Final { response, ringing } => (response, ringing),
            Waited::Gone => return Err(Error::NoResponse),
            Waited::GaveUp { provisional } => {
                withdraw(
                    endpoint,
                    &invite,
                    &via,
                    target.clone(),
                    &mut responses,
                    provisional,
                )
                .await;
                return Err(Error::Cancelled(options.timeout.unwrap_or(Duration::ZERO)));
            }
        };

    if !response.status.is_success() {
        // A non-2xx is acknowledged by the transaction layer itself, so there is nothing to
        // send here — only a media port to release, which happens when `port` drops.
        const INTERVAL_TOO_SMALL: u16 = 422;
        if response.status.code() == INTERVAL_TOO_SMALL
            && let Some(required) = required_interval(&response)
        {
            return Err(Error::IntervalTooBrief(required));
        }
        return Err(Error::Rejected {
            status: response.status.code(),
            reason: String::from_utf8_lossy(&response.reason).into_owned(),
        });
    }

    // From here the far end believes a dialog exists, so *every* path must acknowledge.
    // Returning an error without one leaves it retransmitting its 200 for 32 seconds and then
    // streaming media at a port we have closed.
    match establish(
        &invite,
        &response,
        target.clone(),
        port,
        capabilities.crypto.as_ref(),
    ) {
        Ok((dialog, media, in_dialog, settled)) => {
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
            // Emitted at construction — the earliest point this call has a stream anyone could
            // read from — from what was actually observed while waiting for the final response,
            // not reconstructed later from anything left lying around.
            let (events, events_rx) = EventSink::new();
            emit_construction_events(&events, ringing);
            Ok(Call {
                dialog,
                media,
                endpoint: endpoint.clone(),
                target: in_dialog,
                awaiting_ack: None,
                ended: false,
                media_address,
                current: settled.negotiated,
                encrypted: settled.srtp.is_some(),
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
            })
        }
        Err(error) => {
            // RFC 3261 §15: a UAC that cannot proceed after a 2xx acknowledges it and then
            // sends BYE. Walking away silently is what leaves the far end streaming.
            if let Some(dialog) = Dialog::from_response(&invite, &response) {
                let in_dialog = in_dialog_target(&dialog, target.clone());
                let _ = send_ack(endpoint, &dialog, in_dialog.clone()).await;
                if let Ok(bye) = bye_request(&dialog, dialog.local_cseq.saturating_add(1)) {
                    let _ = endpoint.send(bye, in_dialog).await;
                }
            }
            Err(error)
        }
    }
}

/// Everything after a 2xx that can fail, kept together so the caller can ACK on either path.
fn establish(
    invite: &Request,
    response: &Response,
    fallback: Target,
    port: MediaPort,
    offered: Option<&sipx_sdp::crypto::Crypto>,
) -> Result<(Dialog, MediaSession, Target, Settled)> {
    let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
    let settled = settle_answer(offered, &answer)?;
    let dialog = Dialog::from_response(invite, response).ok_or(Error::NoDialog)?;
    let target = in_dialog_target(&dialog, fallback);
    let media = port.start(settled.media_config());
    Ok((dialog, media, target, settled))
}

/// What the far end's answer to *our* offer settles.
///
/// The calling side's counterpart of [`Early::settle`], and the reason it is a function is that
/// an answer can now reach us in two places: the 200 that [`establish`] reads, and — once
/// [`dial_early`] exists — the reliable provisional that makes an early dialog renegotiable at
/// all (RFC 3262 §5). There is no port to bind on either path, because ours was bound before the
/// INVITE named it.
fn settle_answer(
    offered: Option<&sipx_sdp::crypto::Crypto>,
    answer: &SessionDescription,
) -> Result<Settled> {
    // Both halves or neither. A stream keyed at one end only is a call that connects and
    // carries silence, which is worse than one that fails to connect.
    Ok(Settled {
        negotiated: negotiated(answer)?,
        srtp: srtp_keys(offered, answered_crypto(answer)),
    })
}

/// Answer an incoming INVITE.
///
/// The 200 OK is retransmitted until the ACK arrives, which is the transaction user's job:
/// `sipx-sip`'s server transaction moves to `Accepted` and absorbs retransmissions of the
/// *request*, but it does not resend the response. Over UDP one lost 200 means the caller
/// gives up while this side holds an established call, so this is not optional.
pub async fn answer(endpoint: &Handle, incoming: &Incoming, media_address: IpAddr) -> Result<Call> {
    answer_tagged(endpoint, incoming, media_address, &token(), None).await
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
    media_address: IpAddr,
    tag: &str,
    claim: Option<Claim<'_>>,
) -> Result<Call> {
    // Ahead of the claim, deliberately: an offer that cannot be read fails here with nothing
    // sent, and an invitation that was never taken is one a CANCEL can still end.
    let offer = sipx_sdp::parse(&String::from_utf8_lossy(incoming.request.body()))
        .map_err(|error| Error::Sdp(error.to_string()))?;
    // No provisional was sent on this path, so there is nothing to report as `Ringing`.
    answer_negotiated(endpoint, incoming, media_address, offer, tag, None, claim).await
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
/// [`Call::hangup`] leaves the far end in a call. The discipline is the application's: making it
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
    /// What the INVITE offered, kept because an SRTP answer has to be paired with the offer it
    /// answers and because a later UPDATE offers from the same starting point.
    capabilities: Capabilities,
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
pub async fn dial_early(
    endpoint: &Handle,
    target: Target,
    to: &Uri,
    options: &DialOptions,
) -> Result<Dialing> {
    let (port, capabilities, via, invite) =
        open_invitation(endpoint, &target, to, options, &Identity::fresh()).await?;
    let responses = endpoint.send(invite.clone(), target.clone()).await?;

    let mut dialing = Dialing {
        endpoint: endpoint.clone(),
        in_dialog: target.clone(),
        invite,
        via,
        target,
        responses: Some(responses),
        dialog: None,
        seen: sipx_sip::rel::Sequence::default(),
        media: Some(EarlyMedia::Offered(port)),
        capabilities,
        // RFC 3264: the INVITE carried our offer, so an exchange is open until the far end
        // answers it — which before the 200 can only happen in a reliable provisional.
        negotiation: update::Negotiation::offering(),
        peer_allows_update: false,
        hold: Direction::SendRecv,
        ringing: None,
        provisional: false,
        deadline: options.timeout.map(|limit| tokio::time::Instant::now() + limit),
        options: options.clone(),
        answered_already: None,
    };
    dialing.reach_early_dialog().await?;
    Ok(dialing)
}

impl Dialing {
    /// The early dialog, once a provisional has established one (RFC 3261 §12.1.1).
    ///
    /// Exposed read-only because `C-2` will want to know *which* dialog a provisional's media
    /// belongs to — with forking, one invitation can produce several — without this handle
    /// having to guess in advance what it will be asked.
    #[must_use]
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
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
    /// The rules are [`crate::update`]'s, shared with [`Ringing::update`](crate::Ringing::update)
    /// — §5.1 makes UPDATE something either end may send, so there is one implementation and two
    /// callers.
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

    /// Wait for the invitation to be answered, and take the call it becomes.
    ///
    /// Consuming, because everything it needs moves into the [`Call`]. Provisionals that arrive
    /// while waiting are handled exactly as they were before it returned — PRACKed, and read for
    /// the answer that makes the session renegotiable — so an application that calls this
    /// immediately is in the same position as one that had called [`dial`].
    ///
    /// # Errors
    ///
    /// [`Error::Rejected`] if the far end declined, [`Error::IntervalTooBrief`] for a `422`
    /// (see [`dial_early`] on why it is not retried), [`Error::Cancelled`] if the deadline
    /// passed — the invitation is withdrawn first — and [`Error::NoResponse`] if the
    /// transaction ended without a final response.
    pub async fn answered(mut self) -> Result<Call> {
        if let Some(call) = self.answered_already.take() {
            return Ok(*call);
        }
        loop {
            match self.next_response().await {
                Arrived::Provisional(response) => self.observe(&response).await,
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
                    self.observe(&response).await;
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
    async fn observe(&mut self, response: &Response) {
        self.provisional = true;
        let reliable = crate::rel::reliable_sequence(response);
        // A bare `100 Trying` only acknowledges that the request arrived (RFC 3261 §17.2.1); it
        // is not the far end's phone ringing, and 100rel does not apply to it (RFC 3262 §3).
        const TRYING: u16 = 100;
        if response.status.code() > TRYING {
            self.ringing = Some(reliable.is_some());
        }

        if self.dialog.is_none() {
            if let Some(dialog) = Dialog::from_response(&self.invite, response) {
                self.in_dialog = in_dialog_target(&dialog, self.target.clone());
                self.dialog = Some(dialog);
            }
        } else if !self.belongs(response) {
            // A provisional bearing a different `To` tag is a *different* early dialog — the
            // far end forked, and two branches are ringing. This handle names one of them, and
            // adopting the other's description or acknowledging its `RSeq` against this one's
            // sequence space would mix them. Ignored rather than merged; giving an application
            // a handle per branch is `C-2`'s to design, and nothing here forecloses it.
            return;
        }

        if update::peer_allows(&response.headers) {
            // §4 is a statement of capability, and a capability does not lapse because a later
            // response omitted the header.
            self.peer_allows_update = true;
        }

        if let Some(rseq) = reliable {
            // RFC 3262 §5: an answer may only travel in a reliable provisional, so this is the
            // only place before the 200 where our INVITE's offer can be closed out. An
            // unreliable provisional carrying a description is not one — §5 forbids it, and one
            // lost leaves the two sides disagreeing about what is in force with no way to
            // notice — so it is ignored rather than adopted.
            self.adopt_early_answer(response);
            if let Some(dialog) = self.dialog.as_mut() {
                dialog.refresh_target(&response.headers);
            }
            // A failure is logged rather than fatal, for `await_final`'s reason: the invitation
            // is still running, and abandoning a ringing call because one PRACK did not get
            // through is a worse outcome than the unreliability it was fixing.
            if let Err(error) = self.acknowledge(response, rseq).await {
                tracing::debug!(%error, "could not acknowledge a reliable provisional");
            }
        }
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
    /// mirror of [`ring_early`](crate::ring_early). A description that cannot be read or cannot
    /// be negotiated leaves the session where it was — still `Offered` — so the exchange stays
    /// open and [`Self::update`] keeps refusing, which is the truthful state.
    fn adopt_early_answer(&mut self, response: &Response) {
        if !matches!(self.media, Some(EarlyMedia::Offered(_))) || response.body().is_empty() {
            return;
        }
        // Parsed and settled *before* the port is moved out, so that a failure on either step
        // leaves `media` exactly as it was rather than emptied.
        let Ok(answer) = sipx_sdp::parse(&String::from_utf8_lossy(response.body())) else {
            return;
        };
        let Ok(settled) = settle_answer(self.capabilities.crypto.as_ref(), &answer) else {
            return;
        };
        let Some(EarlyMedia::Offered(port)) = self.media.take() else {
            return;
        };
        self.media = Some(EarlyMedia::Answered(Box::new(Early {
            port,
            capabilities: self.capabilities.clone(),
            settled,
            media_address: self.options.media_address,
        })));
        self.negotiation.received_answer();
    }

    /// PRACK a reliable provisional through the early dialog itself (RFC 3262 §4).
    ///
    /// Through *the* dialog, not a copy of it. The PRACK is an in-dialog request and takes the
    /// next number in this side's own sequence space (RFC 3261 §12.2.1.1); the `dial` path
    /// builds a throwaway `Dialog` per acknowledgement because it keeps none, which restarts
    /// that space at the INVITE's number every time. Here an UPDATE may follow, and it would
    /// then reuse the PRACK's number.
    async fn acknowledge(&mut self, response: &Response, rseq: u32) -> Result<()> {
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
        let body = crate::rel::prack_body(
            !self.invite.body().is_empty(),
            response.body(),
            &self.capabilities,
        );
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
            const INTERVAL_TOO_SMALL: u16 = 422;
            if response.status.code() == INTERVAL_TOO_SMALL
                && let Some(required) = required_interval(&response)
            {
                return Err(Error::IntervalTooBrief(required));
            }
            return Err(Error::Rejected {
                status: response.status.code(),
                reason: String::from_utf8_lossy(&response.reason).into_owned(),
            });
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
                let (events, events_rx) = EventSink::new();
                emit_construction_events(&events, self.ringing);
                Ok(Call {
                    dialog,
                    media,
                    endpoint: self.endpoint.clone(),
                    target: self.in_dialog.clone(),
                    awaiting_ack: None,
                    ended: false,
                    media_address: self.options.media_address,
                    current: settled.negotiated,
                    encrypted: settled.srtp.is_some(),
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
                    events_rx: Some(events_rx),
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
                    let _ = send_ack(&self.endpoint, &dialog, in_dialog.clone()).await;
                    if let Ok(bye) = bye_request(&dialog, dialog.local_cseq.saturating_add(1)) {
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
            // The usual case: the early dialog, its sequence space intact.
            (true, Some(early), _) => early,
            (_, _, Some(fresh)) => fresh,
            // A 2xx with no usable `To` tag or `Contact` establishes no dialog of its own; if a
            // provisional already did, that is still the dialog this call is in.
            (_, Some(early), None) => early,
            (_, None, None) => return Err(Error::NoDialog),
        };
        dialog.refresh_target(&response.headers);
        self.in_dialog = in_dialog_target(&dialog, self.target.clone());

        let (port, settled) = match self.media.take() {
            // The answer arrived in a provisional, and any UPDATE since settled its own. So the
            // 2xx's body is *not* read: at this point it can only be a repeat of the answer or,
            // worse, a description that undoes the renegotiation. `answer_early` sends no body
            // in this exact case, and for the same reason.
            Some(EarlyMedia::Answered(early)) if confirms_early => (early.port, early.settled),
            Some(EarlyMedia::Answered(early)) => {
                (early.port, self.settle_from(response)?)
            }
            Some(EarlyMedia::Offered(port)) => (port, self.settle_from(response)?),
            None => return Err(Error::NoDialog),
        };
        let media = port.start(settled.media_config());
        Ok((dialog, media, settled))
    }

    /// Read the answer out of the 2xx, for the case where no provisional carried one.
    fn settle_from(&mut self, response: &Response) -> Result<Settled> {
        let answer = sipx_sdp::parse(&String::from_utf8_lossy(response.body()))
            .map_err(|error| Error::Sdp(error.to_string()))?;
        let settled = settle_answer(self.capabilities.crypto.as_ref(), &answer)?;
        // Our INVITE's offer is answered here rather than in a provisional, so the exchange
        // closes now. Without this the first UPDATE on the confirmed call would be refused as
        // glare against an offer that has in fact been answered.
        self.negotiation.received_answer();
        Ok(settled)
    }

    /// Take back the invitation, whatever state it is in.
    async fn give_up(&mut self) {
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
        )
        .await;
    }
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
    pub(crate) port: MediaPort,
    pub(crate) capabilities: Capabilities,
    pub(crate) settled: Settled,
    pub(crate) media_address: IpAddr,
}

impl Early {
    /// Bind a port and answer `offer` with it.
    pub(crate) async fn settle(
        media_address: IpAddr,
        secure: bool,
        offer: &SessionDescription,
    ) -> Result<(Self, SessionDescription)> {
        let negotiated = negotiated(offer)?;
        let port = MediaPort::bind(SocketAddr::new(media_address, 0))
            .await
            .map_err(Error::Io)?;
        let capabilities =
            Capabilities::g711(media_address, port.local_addr().port()).with_srtp(secure);
        let answer = sipx_sdp::answer(offer, &capabilities);
        if answer
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return Err(Error::NoCommonCodec);
        }
        let settled = Settled {
            negotiated,
            srtp: srtp_keys(capabilities.crypto.as_ref(), offer_crypto(offer)),
        };
        Ok((
            Self {
                port,
                capabilities,
                settled,
                media_address,
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
        if let Ok(negotiated) = negotiated(answer) {
            self.settled.negotiated = negotiated;
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
        let negotiated = negotiated(offer).ok()?;
        let answer = sipx_sdp::answer(offer, &self.capabilities);
        if answer
            .media
            .iter()
            .all(sipx_sdp::MediaDescription::is_rejected)
        {
            return None;
        }
        self.settled = Settled {
            negotiated,
            srtp: srtp_keys(self.capabilities.crypto.as_ref(), offer_crypto(offer)),
        };
        Some(answer)
    }
}

/// Answer an INVITE that has already been rung (RFC 3262).
///
/// The tag comes from the [`Ringing`](crate::Ringing) rather than being fresh, and that is the
/// whole reason this exists. A provisional that established a dialog has already told the caller
/// what this side's tag is (RFC 3261 §12.1.1); a 200 with a different one creates a *second*
/// dialog. The caller ACKs the dialog it knows about, this side waits for an ACK to the other,
/// and the 200 is retransmitted for 32 seconds into a call that is actually up.
pub async fn answer_ringing(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    ringing: &crate::Ringing,
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

    let media = early.port.start(early.settled.media_config());
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
        media,
        endpoint: endpoint.clone(),
        target,
        awaiting_ack: Some(acked),
        ended: false,
        media_address: early.media_address,
        current: early.settled.negotiated,
        hold: Direction::SendRecv,
        encrypted: early.settled.is_encrypted(),
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
async fn answer_negotiated(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    offer: SessionDescription,
    tag: &str,
    reliable_ringing: Option<bool>,
    claim: Option<Claim<'_>>,
) -> Result<Call> {
    let negotiated = negotiated(&offer)?;

    // The port is bound before the session starts, because the answer has to name it *and* the
    // session has to be created with the keys that answer settles on. Starting the session first
    // — as this did — leaves nowhere to put them.
    let port = MediaPort::bind(SocketAddr::new(media_address, 0))
        .await
        .map_err(Error::Io)?;

    let capabilities = Capabilities::g711(media_address, port.local_addr().port())
        .with_srtp(incoming.transport.is_secure());
    let answer_sdp = sipx_sdp::answer(&offer, &capabilities);
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
        srtp: srtp_keys(capabilities.crypto.as_ref(), offer_crypto(&offer)),
    };
    let media = port.start(settled.media_config());

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

    let mut response = ResponseBuilder::to_request(&incoming.request, ok_status(), "OK")?
        .set_header(&HeaderName::To, Bytes::from(to_with_tag))?
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
        .body(Bytes::from(answer_sdp.to_string_sdp()));

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

    // As in `dial_with`: emitted at construction, from what was actually observed (ringing
    // first, if this path came through it) rather than recomputed afterwards.
    let (events, events_rx) = EventSink::new();
    emit_construction_events(&events, reliable_ringing);

    Ok(Call {
        dialog,
        media,
        endpoint: endpoint.clone(),
        target,
        awaiting_ack: Some(acked),
        ended: false,
        media_address,
        current: settled.negotiated,
        hold: Direction::SendRecv,
        encrypted: settled.srtp.is_some(),
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
    /// The transaction ended without a final response.
    Gone,
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
) -> Waited {
    let deadline = limit.map(|limit| tokio::time::Instant::now() + limit);
    let mut provisional = false;
    let mut ringing = None;
    loop {
        let event = match deadline {
            None => responses.next().await,
            Some(deadline) => match tokio::time::timeout_at(deadline, responses.next()).await {
                Ok(event) => event,
                Err(_elapsed) => return Waited::GaveUp { provisional },
            },
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
async fn send_cancel(endpoint: &Handle, invite: &Request, via: &str, target: Target) -> Result<()> {
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
        .cseq(sequence, &Method::Cancel)?
        .max_forwards(70)
        .build();
    endpoint.send(request, target).await?;
    Ok(())
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
    // Over a WebSocket the `Contact` is not consulted at all. RFC 7118 §5.2: the peer has no
    // listening port, its `Contact` names something that will never resolve, and the connection
    // the dialog was established on is the only way to reach it. This is the RFC 5923 rule for
    // stream transports made absolute — there is no fallback because there is nowhere to fall
    // back to, and honouring a `Contact` here would send the BYE to an address that either does
    // not answer or belongs to somebody else.
    if matches!(fallback.transport, TransportKind::Ws | TransportKind::Wss) {
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
    audio.set_direction(capabilities.direction);
    sdp.media.push(audio);
    sdp
}

/// What negotiation settled on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Negotiated {
    remote: SocketAddr,
    codec: Codec,
    /// The payload type the far end uses for `telephone-event`, if it offered one.
    ///
    /// Taken from the description rather than assumed, because it is a *dynamic* type: 101 is
    /// what sipx offers, not what everyone uses, and assuming it would send keypresses on
    /// whatever the far end put that number to.
    dtmf: Option<u8>,
}

/// What negotiation settled on, plus the keys — which are not `Copy` and do not belong in a
/// type that is.
#[derive(Debug, Clone)]
pub(crate) struct Settled {
    pub(crate) negotiated: Negotiated,
    srtp: Option<sipx_media::SrtpKeys>,
}

impl Negotiated {
    fn media_config(self) -> sipx_media::Config {
        let mut config = sipx_media::Config::new(self.remote, self.codec);
        config.dtmf_payload_type = self.dtmf;
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

/// Pair our offered key with the far end's answered one.
///
/// `None` unless *both* are present. One key is not a session: a stream keyed at one end only
/// is a stream the other end cannot read, and treating a half-answer as success would produce a
/// call that connects and carries silence.
pub(crate) fn srtp_keys(
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
pub(crate) fn negotiated(sdp: &SessionDescription) -> Result<Negotiated> {
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
    // order, so the first playable one is the one to use.
    let codec = audio
        .formats
        .iter()
        .find_map(|format| format.parse::<u8>().ok().and_then(Codec::from_payload_type))
        .ok_or(Error::NoCommonCodec)?;

    Ok(Negotiated {
        remote: SocketAddr::new(address, audio.port),
        codec,
        dtmf: telephone_event_payload_type(audio),
    })
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
        TransportKind::Ws | TransportKind::Wss => format!(
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
pub async fn answer_replacing(
    endpoint: &Handle,
    incoming: &Incoming,
    media_address: IpAddr,
    replaced: &mut Call,
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
    let taken_over = answer(endpoint, incoming, media_address).await?;

    // Then end the one being replaced (RFC 3891 §3). Its media stops with it.
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
