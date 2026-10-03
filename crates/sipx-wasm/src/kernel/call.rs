//! The call state tables (`docs/specs/browser-sdk.md` §5.4) and the SDP gatekeeping around them.
//!
//! Two tables, one per direction, and state never moves backwards. A command outside the listed
//! rows is `E_STATE`; a description outside [`webrtc-audio`](../../../../docs/specs/webrtc-audio.md)
//! §4 is refused **inside** the kernel and never reaches the browser's parser.
//!
//! The kernel is the gatekeeper in both directions and the author in neither: the browser writes
//! every description, and this module decides whether it may cross.

use sipx_sdp::browser_audio::{self, BrowserAudioRole, ProfileError};
use sipx_sdp::fingerprint::SetupCapabilities;
use sipx_sip::auth::{Challenge, Credentials, respond, strongest};
use sipx_sip::{
    Header, HeaderName, Message, Method, Request, Response, StatusCode, TransactionKey,
};

use super::Kernel;
use crate::bounds;
use crate::command::{MediaKind, Verb};
use crate::error::{Error, Result};
use crate::event::{Cause, CauseClass, CodecFact, Direction, Event, MediaFacts};
use crate::sip::{self, Dialog};

/// The nonterminal states of §5.4, in their `"call"`-event spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CallState {
    Dialing,
    InviteSent,
    Ringing,
    AnswerDelivered,
    Incoming,
    AnswerPending,
    AnswerSent,
    SipEstablished,
    /// Terminal. §5.4: every `Ended(…)` row emits `"call-ended"` instead of a `"call"` state, so
    /// this spelling never reaches the wire — it exists so a later command can be refused.
    Ended,
}

impl CallState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dialing => "dialing",
            Self::InviteSent => "inviteSent",
            Self::Ringing => "ringing",
            Self::AnswerDelivered => "answerDelivered",
            Self::Incoming => "incoming",
            Self::AnswerPending => "answerPending",
            Self::AnswerSent => "answerSent",
            Self::SipEstablished => "sipEstablished",
            Self::Ended => "ended",
        }
    }
}

/// One call.
#[derive(Debug)]
pub(crate) struct Call {
    pub(crate) dir: Direction,
    state: CallState,
    dialog: Dialog,
    /// The INVITE's own branch, which a CANCEL must reuse (RFC 3261 §9.1).
    invite_branch: Option<String>,
    invite_cseq: u32,
    invite_key: Option<TransactionKey>,
    auth_retries: u8,
    auth_challenges: Vec<(Challenge, u32)>,
    cancelled: bool,
    /// A received 2xx owes ACK and BYE; retain ownership until both branches are available.
    pending_cleanup: Option<Cause>,
    /// The server transaction of a received INVITE, so a 180/200/486/488 can be sent on it.
    server_key: Option<TransactionKey>,
    /// The offer this endpoint sent, kept so the answer can be validated against it.
    local_offer: Option<String>,
    /// The offer received from the peer, kept for the same reason on the inbound side.
    remote_offer: Option<String>,
    /// The command whose promise settles when this call is established or ends.
    lifecycle_command: Option<u64>,
}

impl Call {
    pub(crate) fn state(&self) -> CallState {
        self.state
    }

    fn remember_challenge(&mut self, challenge: Challenge) {
        let count = self
            .auth_challenges
            .iter()
            .find(|(old, _)| {
                old.from_proxy == challenge.from_proxy
                    && old.realm == challenge.realm
                    && old.nonce == challenge.nonce
                    && old.algorithm == challenge.algorithm
            })
            .map_or(0, |(_, count)| *count);
        self.auth_challenges
            .retain(|(old, _)| old.from_proxy != challenge.from_proxy);
        self.auth_challenges.push((challenge, count));
    }
}

impl Kernel {
    /// Route one accepted command to the verb's owner.
    pub(super) fn dispatch(&mut self, id: u64, verb: Verb) -> Result<()> {
        match verb {
            Verb::Register { expires } => self.register(id, expires),
            Verb::Unregister {} => self.unregister(id),
            Verb::Dial { target } => self.dial(id, &target),
            Verb::Ring { call } => self.ring(id, call),
            Verb::Answer { call } => self.answer(id, call),
            Verb::Reject { call, status } => self.reject(id, call, status),
            Verb::Hangup { call } => self.hangup(id, call),
            Verb::LocalMedia { call, kind, sdp } => self.local_media(id, call, kind, &sdp),
            Verb::MediaApplied { call } => self.media_applied(id, call),
            Verb::MediaFailed { call, reason } => self.media_failed(id, call, &reason),
        }
    }

    /// A live call in a state that admits commands, or `E_STATE`.
    fn call_state(&self, number: u32) -> Result<CallState> {
        match self.calls.get(&number) {
            Some(call) if call.state != CallState::Ended => Ok(call.state),
            // An unknown call number and a call that already ended are the same answer on
            // purpose: state never moves backwards, so neither can accept anything.
            _ => Err(Error::State),
        }
    }

    fn call_mut(&mut self, number: u32) -> Result<&mut Call> {
        self.calls.get_mut(&number).ok_or(Error::State)
    }

    /// Emit the `"call"` snapshot for a call's current state.
    fn announce(&mut self, number: u32) {
        let Some(call) = self.calls.get(&number) else {
            return;
        };
        let event = Event::Call {
            call: number,
            dir: call.dir,
            state: call.state.as_str(),
            from: Some(call.dialog.local_uri.clone()),
            to: Some(call.dialog.remote_uri.clone()),
        };
        self.emit(&event);
    }

    /// End a call: one terminal notification, never two spellings of it (§5.4).
    fn end(&mut self, number: u32, cause: Cause) {
        let Some(call) = self.calls.get_mut(&number) else {
            return;
        };
        if call.state == CallState::Ended {
            return;
        }
        call.state = CallState::Ended;
        let lifecycle = call.lifecycle_command.take();
        let reason = cause.reason.clone();
        let class = cause.class;
        self.emit(&Event::CallEnded {
            call: number,
            cause,
        });
        if let Some(id) = lifecycle {
            self.refuse(
                id,
                terminal_code(class),
                reason.unwrap_or_else(|| class.as_str().to_owned()),
            );
        }
        // The call object is retired here rather than kept as a tombstone: §4.9 caps *concurrent*
        // calls, and a page that dialled eight times over an hour has not used up its budget.
        self.calls.remove(&number);
        self.forget_transactions(number);
    }

    // ------------------------------------------------------------------ outbound

    /// §5.4 outbound row 1: `"dial"` accepted. **No SIP yet** — a permission failure before media
    /// costs no signalling.
    fn dial(&mut self, id: u64, target: &str) -> Result<()> {
        if self.calls.len() >= bounds::MAX_CALLS {
            self.refuse(
                id,
                "call-limit",
                "this kernel already carries eight concurrent calls",
            );
            return Ok(());
        }
        let call_id = self.entropy.call_id()?;
        let local_tag = self.entropy.tag()?;
        let number = self.next_call;
        self.next_call = self.next_call.saturating_add(1);

        self.calls.insert(
            number,
            Call {
                dir: Direction::Out,
                state: CallState::Dialing,
                dialog: Dialog {
                    call_id,
                    local_tag,
                    remote_tag: None,
                    local_uri: self.config.aor.clone(),
                    remote_uri: target.to_owned(),
                    remote_target: None,
                    local_cseq: 0,
                },
                invite_branch: None,
                invite_cseq: 0,
                invite_key: None,
                auth_retries: 0,
                auth_challenges: Vec::new(),
                cancelled: false,
                pending_cleanup: None,
                server_key: None,
                local_offer: None,
                remote_offer: None,
                lifecycle_command: Some(id),
            },
        );
        self.announce(number);
        self.emit(&Event::NeedLocalMedia {
            call: number,
            kind: MediaKind::Offer,
        });
        self.ask_for_entropy_if_low();
        Ok(())
    }

    /// §5.2 `"local-media"`: the browser-created description, validated before the kernel carries
    /// it in SIP.
    fn local_media(&mut self, id: u64, number: u32, kind: MediaKind, sdp: &str) -> Result<()> {
        if sdp.len() > bounds::MAX_SDP {
            self.refuse(id, "sdp-too-large", "the description exceeds 16 KiB");
            return Ok(());
        }
        match (self.call_state(number)?, kind) {
            (CallState::Dialing, MediaKind::Offer) => self.local_offer(id, number, sdp),
            (CallState::AnswerPending, MediaKind::Answer) => self.local_answer(id, number, sdp),
            _ => Err(Error::State),
        }
    }

    /// §5.4 outbound rows 2 and 3.
    fn local_offer(&mut self, id: u64, number: u32, sdp: &str) -> Result<()> {
        if let Err(error) = validate(sdp, BrowserAudioRole::Offerer) {
            self.refuse(id, "sdp-profile", error.to_string());
            self.end(
                number,
                Cause::class(CauseClass::Media).with_reason(error.to_string()),
            );
            return Ok(());
        }

        let branch = self.entropy.branch()?;
        let Some(call) = self.calls.get_mut(&number) else {
            return Err(Error::State);
        };
        call.dialog.local_cseq = call.dialog.local_cseq.saturating_add(1);
        call.invite_cseq = call.dialog.local_cseq;
        call.invite_branch = Some(branch.clone());
        call.local_offer = Some(sdp.to_owned());
        let dialog = call.dialog.clone();
        let cseq = call.invite_cseq;

        let Ok(request) = sip::invite(&self.config, &dialog, &branch, cseq, sdp) else {
            self.poison("an INVITE this kernel composed did not build");
            return Ok(());
        };
        let Some((key, outputs)) = self.transactions.send_request(request, Self::reliability())
        else {
            self.poison("the transaction layer refused an INVITE");
            return Ok(());
        };
        self.call_mut(number)?.invite_key = Some(key.clone());
        self.own_transaction(key.clone(), number);
        self.set_state(number, CallState::InviteSent);
        self.drive(&key, outputs);
        self.announce(number);
        self.succeed(id);
        self.ask_for_entropy_if_low();
        Ok(())
    }

    /// §5.4 inbound rows 6 and 7: the answer the browser wrote, checked against the offer that
    /// arrived.
    fn local_answer(&mut self, id: u64, number: u32, sdp: &str) -> Result<()> {
        let (offer, server_key, dialog) = {
            let call = self.call_mut(number)?;
            (
                call.remote_offer.clone(),
                call.server_key.clone(),
                call.dialog.clone(),
            )
        };
        let Some(offer) = offer else {
            self.poison("an inbound call reached AnswerPending with no remote offer");
            return Ok(());
        };
        let Some(server_key) = server_key else {
            self.poison("an inbound call reached AnswerPending with no server transaction");
            return Ok(());
        };

        let facts = media_facts(&offer, sdp, &dialog, Direction::In);
        if let Err(error) = &facts {
            self.refuse(id, "sdp-profile", error.to_string());
            self.respond_on(&server_key, 488, "Not Acceptable Here", &dialog, None);
            self.end(
                number,
                Cause::class(CauseClass::Media).with_reason(error.to_string()),
            );
            return Ok(());
        }

        if let Ok(kernel) = facts {
            self.emit(&Event::NegotiatedMedia {
                call: number,
                kernel,
            });
        }
        self.respond_on(&server_key, 200, "OK", &dialog, Some(sdp));
        self.set_state(number, CallState::AnswerSent);
        self.announce(number);
        self.succeed(id);
        Ok(())
    }

    /// §5.4 outbound row 9: `"media-applied"` releases the held ACK.
    fn media_applied(&mut self, id: u64, number: u32) -> Result<()> {
        if self.call_state(number)? != CallState::AnswerDelivered {
            return Err(Error::State);
        }
        self.send_ack(number)?;
        self.set_state(number, CallState::SipEstablished);
        self.announce(number);
        let lifecycle = self.call_mut(number)?.lifecycle_command.take();
        if let Some(dial) = lifecycle {
            self.succeed(dial);
        }
        self.succeed(id);
        Ok(())
    }

    /// §5.4 outbound row 10: the browser refused the answer, so the kernel completes the exchange
    /// it owes — ACK, then BYE — before the call ends.
    fn media_failed(&mut self, id: u64, number: u32, reason: &str) -> Result<()> {
        if self.call_state(number)? != CallState::AnswerDelivered {
            return Err(Error::State);
        }
        self.send_ack_bye(number)?;
        self.succeed(id);
        self.end(
            number,
            Cause::class(CauseClass::Media).with_reason(reason.to_owned()),
        );
        Ok(())
    }

    // ------------------------------------------------------------------ inbound

    /// §5.4 inbound row 3.
    fn ring(&mut self, id: u64, number: u32) -> Result<()> {
        if self.call_state(number)? != CallState::Incoming {
            return Err(Error::State);
        }
        let (key, dialog) = self.server_context(number)?;
        self.respond_on(&key, 180, "Ringing", &dialog, None);
        self.succeed(id);
        Ok(())
    }

    /// §5.4 inbound row 4.
    fn answer(&mut self, id: u64, number: u32) -> Result<()> {
        if self.call_state(number)? != CallState::Incoming {
            return Err(Error::State);
        }
        self.set_state(number, CallState::AnswerPending);
        self.call_mut(number)?.lifecycle_command = Some(id);
        self.emit(&Event::NeedLocalMedia {
            call: number,
            kind: MediaKind::Answer,
        });
        Ok(())
    }

    /// §5.4 inbound row 5, explicit-status half.
    fn reject(&mut self, id: u64, number: u32, status: u16) -> Result<()> {
        if self.call_state(number)? != CallState::Incoming {
            return Err(Error::State);
        }
        let (key, dialog) = self.server_context(number)?;
        self.respond_on(&key, status, "Rejected", &dialog, None);
        self.succeed(id);
        self.end(
            number,
            Cause {
                class: CauseClass::Refused,
                status: Some(u64::from(status)),
                reason: None,
            },
        );
        Ok(())
    }

    /// `"hangup"`: one verb, one row per state (§5.2 — "end a call in any state").
    fn hangup(&mut self, id: u64, number: u32) -> Result<()> {
        if self
            .calls
            .get(&number)
            .is_some_and(|call| call.pending_cleanup.is_some())
        {
            self.succeed(id);
            return Ok(());
        }
        match self.call_state(number)? {
            // No SIP owed: the INVITE was never serialised.
            CallState::Dialing => {
                self.succeed(id);
                self.end(number, Cause::class(CauseClass::Local));
            }
            // CANCEL; the call ends when the 487 exchange completes.
            CallState::InviteSent | CallState::Ringing => {
                if !self.call_mut(number)?.cancelled {
                    self.call_mut(number)?.cancelled = true;
                    self.send_cancel(number)?;
                }
                self.succeed(id);
            }
            // An unanswered incoming call is refused 486, which §5.2 names explicitly.
            CallState::Incoming | CallState::AnswerPending => {
                let (key, dialog) = self.server_context(number)?;
                self.respond_on(&key, 486, "Busy Here", &dialog, None);
                self.succeed(id);
                let class = if self.calls.get(&number).map(|call| call.state)
                    == Some(CallState::Incoming)
                {
                    CauseClass::Refused
                } else {
                    CauseClass::Local
                };
                self.end(number, Cause::class(class));
            }
            CallState::AnswerDelivered | CallState::AnswerSent | CallState::SipEstablished => {
                if self.call_state(number)? == CallState::AnswerDelivered {
                    self.send_ack_bye(number)?;
                } else {
                    self.send_bye(number)?;
                }
                self.succeed(id);
                self.end(number, Cause::class(CauseClass::Local));
            }
            CallState::Ended => return Err(Error::State),
        }
        Ok(())
    }

    // ------------------------------------------------------------------ outgoing requests

    /// Reserve the complete two-branch draw before either wire effect. Commands may retry
    /// after `E_ENTROPY` without a partial ACK or consumed branch.
    fn send_ack_bye(&mut self, number: u32) -> Result<()> {
        if self.entropy.level() < 16 {
            return Err(Error::Entropy);
        }
        self.send_ack(number)?;
        self.send_bye(number)
    }

    /// Network inputs have already been accepted; unlike a refused host command, their
    /// cleanup cannot be retried by the caller. Keep one bounded obligation on the call.
    fn cleanup_answer(&mut self, number: u32, cause: Cause) {
        if let Some(call) = self.calls.get_mut(&number) {
            call.pending_cleanup = Some(cause);
        }
        self.resume_cleanup(number);
    }

    fn resume_cleanup(&mut self, number: u32) {
        let Some(cause) = self
            .calls
            .get(&number)
            .and_then(|call| call.pending_cleanup.clone())
        else {
            return;
        };
        match self.send_ack_bye(number) {
            Ok(()) => self.end(number, cause),
            Err(Error::Entropy) => self.ask_for_entropy_if_low(),
            Err(_) => self.poison("a pending answer cleanup lost its call"),
        }
    }

    pub(super) fn resume_answer_cleanups(&mut self) {
        let pending: Vec<u32> = self
            .calls
            .iter()
            .filter_map(|(number, call)| call.pending_cleanup.as_ref().map(|_| *number))
            .collect();
        for number in pending {
            self.resume_cleanup(number);
        }
    }

    fn send_ack(&mut self, number: u32) -> Result<()> {
        let branch = self.entropy.branch()?;
        let call = self.call_mut(number)?;
        let cseq = call.invite_cseq;
        let dialog = call.dialog.clone();
        let Ok(request) = sip::ack(&self.config, &dialog, &branch, cseq) else {
            self.poison("an ACK this kernel composed did not build");
            return Ok(());
        };
        // RFC 3261 §13.2.2.4: the ACK for a 2xx is not a transaction. It goes on the wire
        // directly, and its retransmission is the transaction user's business, not §17's.
        self.wire(&Message::Request(request));
        self.ask_for_entropy_if_low();
        Ok(())
    }

    fn send_bye(&mut self, number: u32) -> Result<()> {
        let branch = self.entropy.branch()?;
        let call = self.call_mut(number)?;
        call.dialog.local_cseq = call.dialog.local_cseq.saturating_add(1);
        let cseq = call.dialog.local_cseq;
        let dialog = call.dialog.clone();
        let Ok(request) = sip::bye(&self.config, &dialog, &branch, cseq) else {
            self.poison("a BYE this kernel composed did not build");
            return Ok(());
        };
        if let Some((key, outputs)) = self.transactions.send_request(request, Self::reliability()) {
            self.own_transaction(key.clone(), number);
            self.drive(&key, outputs);
        }
        self.ask_for_entropy_if_low();
        Ok(())
    }

    fn send_cancel(&mut self, number: u32) -> Result<()> {
        let call = self.call_mut(number)?;
        let Some(branch) = call.invite_branch.clone() else {
            return Err(Error::State);
        };
        let cseq = call.invite_cseq;
        let dialog = call.dialog.clone();
        let Ok(request) = sip::cancel(&self.config, &dialog, &branch, cseq) else {
            self.poison("a CANCEL this kernel composed did not build");
            return Ok(());
        };
        if let Some((key, outputs)) = self.transactions.send_request(request, Self::reliability()) {
            self.own_transaction(key.clone(), number);
            self.drive(&key, outputs);
        }
        Ok(())
    }

    /// Send a response on a server transaction.
    fn respond_on(
        &mut self,
        key: &TransactionKey,
        status: u16,
        reason: &str,
        dialog: &Dialog,
        sdp: Option<&str>,
    ) {
        let Some(request) = self.transactions.server_request(key).cloned() else {
            self.poison("a server transaction vanished before its response");
            return;
        };
        let contact = if (200..300).contains(&status) {
            Some(sip::contact_uri(&self.config, dialog))
        } else {
            None
        };
        let Ok(response) = sip::respond(
            &request,
            status,
            reason,
            Some(&dialog.local_tag),
            contact.as_deref(),
            sdp,
        ) else {
            self.poison("a response this kernel composed did not build");
            return;
        };
        let outputs = self.transactions.send_response(key, response);
        self.drive(key, outputs);
    }

    fn server_context(&mut self, number: u32) -> Result<(TransactionKey, Dialog)> {
        let call = self.call_mut(number)?;
        let Some(key) = call.server_key.clone() else {
            return Err(Error::State);
        };
        Ok((key, call.dialog.clone()))
    }

    fn set_state(&mut self, number: u32, state: CallState) {
        if let Some(call) = self.calls.get_mut(&number) {
            call.state = state;
        }
    }

    // ------------------------------------------------------------------ incoming messages

    /// A response the transaction layer matched.
    pub(super) fn on_response(&mut self, key: &TransactionKey, response: &Response) {
        if self.registration.owns(key) {
            self.on_registration_response(response);
            return;
        }
        let Some(number) = self.transaction_owner(key) else {
            return;
        };
        let method = self
            .transactions
            .client_request(key)
            .map(|request| request.method.clone());
        match method {
            Some(Method::Invite)
                if self
                    .calls
                    .get(&number)
                    .and_then(|call| call.invite_key.as_ref())
                    == Some(key) =>
            {
                self.on_invite_response(number, response);
            }
            Some(Method::Bye) if response.status.is_final() => {
                self.end(number, Cause::class(CauseClass::Local));
            }
            // A CANCEL's own 2xx says only that the CANCEL arrived; the call ends on the 487 for
            // the INVITE, which is a different transaction.
            _ => {}
        }
    }

    /// §5.4 outbound rows 5 to 8.
    fn on_invite_response(&mut self, number: u32, response: &Response) {
        let status = response.status;
        if self.call_state(number).is_err()
            || self
                .calls
                .get(&number)
                .is_some_and(|call| call.pending_cleanup.is_some())
        {
            return;
        }
        // A duplicate final response cannot emit media facts or establish twice. The ACK
        // stays held while the browser applies the answer; after that it may be repeated.
        if matches!(
            self.call_state(number),
            Ok(CallState::AnswerDelivered | CallState::SipEstablished)
        ) {
            if self.call_state(number) == Ok(CallState::SipEstablished) && status.is_success() {
                let _ = self.send_ack(number);
            }
            return;
        }
        if matches!(status.code(), 401 | 407) {
            if self.calls.get(&number).is_some_and(|call| call.cancelled) {
                self.end(number, Cause::class(CauseClass::Local));
            } else {
                self.retry_invite(number, response);
            }
            return;
        }
        if let Some(tag) = sip::to_tag(&response.headers)
            && let Ok(call) = self.call_mut(number)
        {
            call.dialog.remote_tag = Some(tag);
        }

        if status.is_provisional() {
            if status.code() > 100 && self.call_state(number) == Ok(CallState::InviteSent) {
                self.set_state(number, CallState::Ringing);
                self.announce(number);
            }
            return;
        }

        if status.is_success() {
            self.on_invite_success(number, response);
            return;
        }

        // 3xx–6xx. The client transaction has already generated the ACK (RFC 3261 §17.1.1.3), so
        // the kernel owes nothing on the wire.
        let reason = String::from_utf8_lossy(&response.reason).into_owned();
        let cause = if status.code() == 487 {
            // The 487 that completes a CANCEL exchange: this side ended the call.
            Cause::class(CauseClass::Local)
        } else {
            Cause::sip(u64::from(status.code()), reason)
        };
        self.end(number, cause);
    }

    /// RFC 3261 §22: the transaction already emitted the non-2xx ACK. Only a current,
    /// uncancelled call may originate its replacement initial INVITE.
    fn retry_invite(&mut self, number: u32, response: &Response) {
        let status = u64::from(response.status.code());
        if let Some(reason) = self.invite_auth_refusal(number) {
            self.end(number, Cause::sip(status, reason));
            return;
        }
        let Some(challenge) = invite_challenge(response) else {
            self.end(
                number,
                Cause::sip(status, "no supported authentication challenge"),
            );
            return;
        };
        // Reserve the complete draw before consuming any of it, as §4.7 requires.
        if self.entropy.level() < 24 {
            self.end(
                number,
                Cause::sip(status, "insufficient authentication entropy"),
            );
            self.ask_for_entropy_if_low();
            return;
        }
        let (Ok(cnonce), Ok(branch)) = (self.entropy.cnonce(), self.entropy.branch()) else {
            self.end(
                number,
                Cause::sip(status, "insufficient authentication entropy"),
            );
            return;
        };
        let Some(call) = self.calls.get_mut(&number) else {
            return;
        };
        call.remember_challenge(challenge);
        let Some(offer) = call.local_offer.as_deref() else {
            return;
        };
        let credentials =
            Credentials::new(self.config.username.clone(), self.config.password.clone());
        let headers = call
            .auth_challenges
            .iter_mut()
            .map(|(challenge, count)| {
                *count += 1;
                Header::build(
                    challenge.response_header(),
                    respond(
                        challenge,
                        &credentials,
                        "INVITE",
                        &call.dialog.remote_uri,
                        *count,
                        &cnonce,
                    ),
                )
            })
            .collect::<core::result::Result<Vec<_>, _>>();
        let Ok(headers) = headers else {
            self.end(
                number,
                Cause::sip(status, "invalid authentication challenge"),
            );
            return;
        };
        call.dialog.remote_tag = None;
        call.dialog.remote_target = None;
        call.dialog.local_cseq = call.dialog.local_cseq.saturating_add(1);
        call.invite_cseq = call.dialog.local_cseq;
        call.invite_branch = Some(branch.clone());
        let Ok(mut request) =
            sip::invite(&self.config, &call.dialog, &branch, call.invite_cseq, offer)
        else {
            self.end(
                number,
                Cause::sip(status, "authenticated request could not be composed"),
            );
            return;
        };
        for header in headers {
            request.headers.push(header);
        }
        let Some((key, outputs)) = self.transactions.send_request(request, Self::reliability())
        else {
            self.end(
                number,
                Cause::sip(status, "authenticated transaction could not be created"),
            );
            return;
        };
        call.auth_retries += 1;
        call.invite_key = Some(key.clone());
        call.state = CallState::InviteSent;
        self.own_transaction(key.clone(), number);
        self.drive(&key, outputs);
        self.announce(number);
        self.ask_for_entropy_if_low();
    }

    fn invite_auth_refusal(&self, number: u32) -> Option<&'static str> {
        if !self.config.may_authenticate() {
            Some("authentication requires secure signalling")
        } else if self
            .calls
            .get(&number)
            .is_none_or(|call| call.auth_retries >= 2)
        {
            Some("authentication retry limit reached")
        } else {
            None
        }
    }

    /// A 2xx to INVITE: validate the answer, then hold the ACK until the browser has applied it.
    fn on_invite_success(&mut self, number: u32, response: &Response) {
        if let Some(target) = sip::contact_target(&response.headers)
            && let Ok(call) = self.call_mut(number)
        {
            call.dialog.remote_target = Some(target);
        }
        if self.calls.get(&number).is_some_and(|call| call.cancelled) {
            self.cleanup_answer(number, Cause::class(CauseClass::Local));
            return;
        }
        let Some(sdp) = sip::sdp_body(&response.headers, response.body()) else {
            // A 2xx with no answer leaves nothing to apply and no way to start media. The ACK is
            // still owed (RFC 3261 §13.2.2.4) before the BYE.
            self.cleanup_answer(
                number,
                Cause::class(CauseClass::Media).with_reason("the 2xx carried no answer"),
            );
            return;
        };

        let offer = self
            .calls
            .get(&number)
            .and_then(|call| call.local_offer.clone());
        let outcome = match (offer, self.calls.get(&number)) {
            (Some(offer), Some(call)) => media_facts(&offer, &sdp, &call.dialog, Direction::Out),
            _ => Err(ProfileError::MediaSectionCount),
        };
        let facts = match outcome {
            Ok(facts) => facts,
            Err(error) => {
                // §9.6's `BSDK-STATE-7`. The refusal names the profile rule, never the description:
                // an SDES key echoed into an event would publish exactly what the refusal exists to
                // reject.
                self.cleanup_answer(
                    number,
                    Cause::class(CauseClass::Media).with_reason(error.to_string()),
                );
                return;
            }
        };

        self.emit(&Event::NegotiatedMedia {
            call: number,
            kernel: facts,
        });
        self.set_state(number, CallState::AnswerDelivered);
        self.announce(number);
        self.emit(&Event::RemoteMedia {
            call: number,
            kind: MediaKind::Answer,
            sdp,
        });
    }

    /// A request the transaction layer accepted: an INVITE, a CANCEL or a BYE.
    pub(super) fn on_request(&mut self, key: &TransactionKey, request: &Request) {
        if !answerable(request) {
            // RFC 3261 §8.2.6.1: a response copies `Via`, `From`, `To`, `Call-ID` and `CSeq` from
            // the request. A request missing any of them cannot be answered *at all* — there is
            // nowhere to send the answer and nothing to correlate it with — so it is discarded and
            // counted, exactly as the native stack discards it. `rfc4475/insuf.dat` is this case,
            // and treating it as an internal fault would let one malformed datagram take a page's
            // endpoint down (§8.1).
            self.counters.parse_errors = self.counters.parse_errors.saturating_add(1);
            self.abandon(key);
            return;
        }
        match request.method {
            Method::Invite => self.on_incoming_invite(key, request),
            Method::Cancel => self.on_incoming_cancel(key, request),
            Method::Bye => self.on_incoming_bye(key, request),
            _ => {
                // Everything outside §5.2's vocabulary. RFC 3261 §8.2.1's answer to a method this
                // endpoint does not support, sent by the transaction so the peer stops retrying.
                self.respond_to_unsupported(key, 405, "Method Not Allowed");
            }
        }
    }

    /// §5.4 inbound rows 1 and 2, plus §4.9's concurrent-call cap.
    fn on_incoming_invite(&mut self, key: &TransactionKey, request: &Request) {
        if self.calls.len() >= bounds::MAX_CALLS {
            self.counters.refused_incoming = self.counters.refused_incoming.saturating_add(1);
            self.respond_to_unsupported(key, 486, "Busy Here");
            return;
        }
        let Some(sdp) = sip::sdp_body(&request.headers, request.body()) else {
            self.counters.refused_incoming = self.counters.refused_incoming.saturating_add(1);
            self.respond_to_unsupported(key, 488, "Not Acceptable Here");
            return;
        };
        if validate(&sdp, BrowserAudioRole::Offerer).is_err() {
            // §5.4: "respond 488 with no media resources; counted `refused_incoming`; no call
            // object". `BSDK-STATE-8` is this row with a video section.
            self.counters.refused_incoming = self.counters.refused_incoming.saturating_add(1);
            self.respond_to_unsupported(key, 488, "Not Acceptable Here");
            return;
        }

        // An inbound dialog's Call-ID is the peer's; the only identifier this side mints is its
        // own tag, so the draw is eight octets rather than §4.7's twenty-four.
        let Ok(local_tag) = self.entropy.tag() else {
            self.counters.refused_incoming = self.counters.refused_incoming.saturating_add(1);
            self.respond_to_unsupported(key, 500, "Server Internal Error");
            self.ask_for_entropy_if_low();
            return;
        };

        let number = self.next_call;
        self.next_call = self.next_call.saturating_add(1);
        let call_id = sip::call_id(&request.headers).unwrap_or_default();
        let remote_uri = request
            .headers
            .typed::<sipx_sip::headers::From>()
            .and_then(core::result::Result::ok)
            .map(|from| String::from_utf8_lossy(&from.0.uri.to_bytes()).into_owned())
            .unwrap_or_default();

        self.calls.insert(
            number,
            Call {
                dir: Direction::In,
                state: CallState::Incoming,
                dialog: Dialog {
                    call_id,
                    local_tag,
                    remote_tag: sip::from_tag(&request.headers),
                    local_uri: self.config.aor.clone(),
                    remote_uri,
                    remote_target: sip::contact_target(&request.headers),
                    local_cseq: sip::cseq(&request.headers).unwrap_or(0),
                },
                invite_branch: sip::top_branch(&request.headers),
                invite_cseq: sip::cseq(&request.headers).unwrap_or(0),
                invite_key: None,
                auth_retries: 0,
                auth_challenges: Vec::new(),
                cancelled: false,
                pending_cleanup: None,
                server_key: Some(key.clone()),
                local_offer: None,
                remote_offer: Some(sdp.clone()),
                lifecycle_command: None,
            },
        );
        self.own_transaction(key.clone(), number);
        self.announce(number);
        self.emit(&Event::RemoteMedia {
            call: number,
            kind: MediaKind::Offer,
            sdp,
        });
        self.ask_for_entropy_if_low();
    }

    /// §5.4 inbound row 6: a CANCEL for an INVITE this kernel has not answered.
    fn on_incoming_cancel(&mut self, key: &TransactionKey, request: &Request) {
        // The CANCEL's own 2xx, on the CANCEL's transaction.
        self.respond_to_unsupported(key, 200, "OK");

        let Some(invite_key) = TransactionKey::for_cancelled_invite(request) else {
            return;
        };
        let Some(number) = self.transaction_owner(&invite_key) else {
            return;
        };
        let Ok((server_key, dialog)) = self.server_context(number) else {
            return;
        };
        self.respond_on(&server_key, 487, "Request Terminated", &dialog, None);
        self.end(number, Cause::class(CauseClass::Remote));
    }

    /// A BYE inside an established dialog.
    fn on_incoming_bye(&mut self, key: &TransactionKey, request: &Request) {
        self.respond_to_unsupported(key, 200, "OK");
        let Some(call_id) = sip::call_id(&request.headers) else {
            return;
        };
        let number = self
            .calls
            .iter()
            .find(|(_, call)| call.dialog.call_id == call_id)
            .map(|(number, _)| *number);
        if let Some(number) = number {
            self.end(number, Cause::class(CauseClass::Remote));
        }
    }

    /// A response on a transaction that carries no call state of its own.
    fn respond_to_unsupported(&mut self, key: &TransactionKey, status: u16, reason: &str) {
        let Some(request) = self.transactions.server_request(key).cloned() else {
            return;
        };
        let Some(code) = StatusCode::new(status) else {
            self.poison("a status code this kernel chose is not a status code");
            return;
        };
        let Ok(builder) = sipx_sip::ResponseBuilder::to_request(&request, code, reason.to_owned())
        else {
            self.poison("a response this kernel composed did not build");
            return;
        };
        let outputs = self.transactions.send_response(key, builder.build());
        self.drive(key, outputs);
    }

    /// §5.4 inbound row 9: the ACK that establishes an answered call.
    pub(super) fn on_ack(&mut self, request: &Request) {
        let Some(call_id) = sip::call_id(&request.headers) else {
            return;
        };
        let number = self
            .calls
            .iter()
            .find(|(_, call)| call.dialog.call_id == call_id && call.state == CallState::AnswerSent)
            .map(|(number, _)| *number);
        let Some(number) = number else {
            return;
        };
        self.set_state(number, CallState::SipEstablished);
        self.announce(number);
        let lifecycle = self
            .calls
            .get_mut(&number)
            .and_then(|call| call.lifecycle_command.take());
        if let Some(id) = lifecycle {
            self.succeed(id);
        }
    }

    /// A transaction that ran out of time, or whose transport the host reported failed.
    pub(super) fn on_timeout(&mut self, key: &TransactionKey) {
        if self.registration.owns(key) {
            self.on_registration_timeout();
            return;
        }
        let Some(number) = self.transaction_owner(key) else {
            return;
        };
        if self
            .transactions
            .client_request(key)
            .is_some_and(|request| request.method == Method::Invite)
            && self
                .calls
                .get(&number)
                .and_then(|call| call.invite_key.as_ref())
                != Some(key)
        {
            return;
        }
        // A CANCEL/INVITE transaction timeout cannot erase cleanup owed by its accepted 2xx.
        if self
            .calls
            .get(&number)
            .is_some_and(|call| call.pending_cleanup.is_some())
        {
            return;
        }
        self.end(number, Cause::class(CauseClass::Timeout));
    }

    /// A message the transaction layer could not match.
    ///
    /// The ACK for a 2xx is the expected case: RFC 3261 §17 deliberately leaves it outside the
    /// INVITE transaction. Anything else is a stray, and a stray is dropped rather than acted on.
    pub(super) fn unmatched(&mut self, message: &Message) {
        if let Message::Request(request) = message
            && request.method == Method::Ack
        {
            self.on_ack(request);
        }
    }

    // ------------------------------------------------------------------ transaction ownership

    fn own_transaction(&mut self, key: TransactionKey, number: u32) {
        self.transaction_calls.push((key, number));
    }

    fn transaction_owner(&self, key: &TransactionKey) -> Option<u32> {
        self.transaction_calls
            .iter()
            .find(|(owned, _)| owned == key)
            .map(|(_, number)| *number)
    }

    /// Drop a transaction the kernel will never answer, and every timer it set.
    fn abandon(&mut self, key: &TransactionKey) {
        self.clear_transaction_timers(key);
        self.transactions.abandon(key);
        self.transaction_calls.retain(|(owned, _)| owned != key);
    }

    fn forget_transactions(&mut self, number: u32) {
        let keys: Vec<TransactionKey> = self
            .transaction_calls
            .iter()
            .filter(|(_, owner)| *owner == number)
            .map(|(key, _)| key.clone())
            .collect();
        for key in &keys {
            // Every timer the call set is cancelled — `BSDK-STATE-5` requires exactly this — and
            // the transaction is abandoned so a late retransmission finds nothing to drive.
            self.clear_transaction_timers(key);
            self.transactions.abandon(key);
            // abandon owns server transactions only. The call's terminal cleanup withdraws
            // transport ownership from its client transactions too; their terminal TU signal
            // is intentionally not delivered again after CallEnded.
            let _ = self.transactions.on_transport_error(key);
        }
        self.transaction_calls.retain(|(_, owner)| *owner != number);
    }
}

/// Whether a response to this request can be composed at all.
///
/// The five headers RFC 3261 §8.2.6.1 copies into every response. Their presence is checked as
/// *raw* headers rather than as typed ones: a `CSeq` whose value is malformed can still be echoed
/// verbatim into a 400, which is what the RFC asks for, but a `CSeq` that is not there cannot.
fn answerable(request: &Request) -> bool {
    [
        sipx_sip::HeaderName::Via,
        sipx_sip::HeaderName::From,
        sipx_sip::HeaderName::To,
        sipx_sip::HeaderName::CallId,
        sipx_sip::HeaderName::CSeq,
    ]
    .iter()
    .all(|name| request.headers.get(name).is_some())
}

/// The `"outcome"` code for a call that ended before its lifecycle command completed.
fn terminal_code(class: CauseClass) -> &'static str {
    match class {
        CauseClass::Local => "cancelled",
        CauseClass::Remote => "remote",
        CauseClass::Refused => "refused",
        CauseClass::Sip => "sip",
        CauseClass::Media => "media",
        CauseClass::Timeout => "timeout",
    }
}

/// The profile verdict: a typed refusal, never a description.
///
/// The distinction matters because the refusal's text reaches an `"outcome"` event, and §9.6's
/// `BSDK-STATE-7` requires that an SDES key in a refused answer never appears in one.
/// `ProfileError`'s `Display` names the rule that was broken and nothing from the description.
type Profile = core::result::Result<(), ProfileError>;

/// Parse and validate one description against `webrtc-audio.md` §4.
///
/// A description that will not parse is `MediaSectionCount` rather than a distinct code: the
/// contract's refusal vocabulary is `ProfileError`, and "this is not SDP" and "this is not one
/// audio section" are the same answer to the only question being asked.
fn validate(sdp: &str, role: BrowserAudioRole) -> Profile {
    let description = sipx_sdp::parse::parse(sdp).map_err(|_| ProfileError::MediaSectionCount)?;
    browser_audio::validate(&description, role).map(|_| ())
}

/// Validate the exchange first, then copy only the typed facts admitted by §5.5.
fn media_facts(
    offer: &str,
    answer: &str,
    dialog: &Dialog,
    direction: Direction,
) -> core::result::Result<MediaFacts, ProfileError> {
    let offered = sipx_sdp::parse::parse(offer).map_err(|_| ProfileError::MediaSectionCount)?;
    let answered = sipx_sdp::parse::parse(answer).map_err(|_| ProfileError::MediaSectionCount)?;
    let validated = browser_audio::validate_answer(&offered, &answered, SetupCapabilities::both())?;
    let media = answered
        .media
        .first()
        .ok_or(ProfileError::MediaSectionCount)?;
    let codecs: Vec<CodecFact> = media
        .formats
        .iter()
        .map(|format| {
            // RFC 3551 static payloads may omit rtpmap; profile validation above has
            // already checked these fixed assignments and rejects a conflicting mapping.
            let mapping = media
                .rtpmap(format)
                .or(match format.as_str() {
                    "0" => Some("PCMU/8000"),
                    "8" => Some("PCMA/8000"),
                    _ => None,
                })
                .ok_or(ProfileError::CodecSetIncomplete)?;
            let typed = sipx_sdp::rtpmap::Rtpmap::parse(mapping)
                .map_err(|_| ProfileError::CodecSetIncomplete)?;
            Ok(CodecFact {
                name: typed.encoding().to_ascii_lowercase(),
                clock_rate: typed.clock_rate(),
                channels: typed.channels(),
                payload_type: format
                    .parse()
                    .map_err(|_| ProfileError::CodecSetIncomplete)?,
            })
        })
        .collect::<core::result::Result<_, ProfileError>>()?;
    let selected_codec = codecs
        .iter()
        .find(|codec| codec.payload_type == validated.description.selected_audio_payload)
        .cloned()
        .ok_or(ProfileError::CodecSetIncomplete)?;
    Ok(MediaFacts {
        dialog: crate::event::DialogFact {
            call_id: dialog.call_id.clone(),
            local_tag: dialog.local_tag.clone(),
            remote_tag: dialog.remote_tag.clone().unwrap_or_default(),
        },
        rtcp_mux: true,
        audio_sections: 1,
        codecs,
        selected_codec,
        fingerprint_algorithm: validated.description.fingerprint.func.as_str(),
        answer_setup: validated.description.setup.as_str(),
        local_dtls_role: match direction {
            Direction::In => validated.description.setup.as_str(),
            Direction::Out => validated.local_setup.as_str(),
        },
    })
}

/// Only the challenge header associated with the current response status is admissible.
fn invite_challenge(response: &Response) -> Option<Challenge> {
    let proxy = response.status.code() == 407;
    let name = if proxy {
        HeaderName::ProxyAuthenticate
    } else {
        HeaderName::WwwAuthenticate
    };
    let challenges = response
        .headers
        .get_all(&name)
        .filter_map(|header| Challenge::parse(&header.value(), proxy))
        .collect();
    strongest(challenges)
}
