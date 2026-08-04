//! The host process: a real application on the stack's public API (story `X-38`).
//!
//! Everything else in this crate is apparatus. [`crate::config`] reads the document that declares a
//! host and [`crate::harness`] runs scripted scenarios on virtual time; neither answers a call, and
//! for as long as that was true this crate's stability declaration could only say that its surface
//! had never been constrained by a caller.
//!
//! This module is the caller. It binds the listeners a [`HostConfig`] declares, admits each arriving
//! invitation through [`Running::admit`] — so the document's routing and its failure semantics decide
//! what happens, rather than a constant written here — answers it on `sipx-call`, and serves it to
//! its end.
//!
//! # Why this module is alpha predicate 1
//!
//! The predicate is *no claim outlives its caller*, and `X-30`, `X-33` and `X-37` each measured a
//! cheaper way to check it and found the same hole: a path check is satisfied by citing a file whose
//! relevant branch is dead, so it can only ever say a capability was *mentioned* somewhere. What it
//! cannot say is whether the capability is worth selecting.
//!
//! An application can. This one has no dead branches to cite: it either builds on the API and
//! carries a call, or it does not compile. So the reachable-from-a-call surface is *defined* as what
//! this module reaches, and `scripts/check-app-surface.py` reads that definition off the workspace
//! rather than off a list somebody keeps.
//!
//! **What this host deliberately is not.** Document-mode webhooks and authenticated full-duplex
//! sessions are real; an embedded binding is not. An absent configured binding is routed through
//! the document's own §9.2 `on_unreachable` declaration. The document actor similarly returns HTTP,
//! timeout, session-loss, and document failures to the contract interpreter, which is the sole
//! component deciding whether the invitation is answered, refused or ended.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use sipx_app_protocol::{
    CallSnapshot, Direction, Effect, EventKind, Failure, Input, Interpreter,
    OnFailure as ContractOnFailure, Output, Policy, Response, Source, Timer, Timestamp,
};
use sipx_call::{
    Call, CallEvent, CallEvents, Calls, DialOptions, Dispatched, Dispatcher, Invitation,
    dial_early_until,
};
use sipx_media::{Interrupt, Playback, PlaybackId};
use sipx_sip::{HeaderName, StatusCode, Uri, build::ResponseBuilder, uri::Host as UriHost};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use sipx_ua::{Config as AgentConfig, UserAgent};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::config::{
    ActorCompletion, Admission, AppBinding, AppPolicy, ConfigError, Grants, HostConfig, Listener,
    Protocol, Running,
};
use crate::harness::policy::OnFailure;
use crate::session::{
    CallInput, DeliverError, MAX_CONNECTION_TASKS, OriginateRequest, SessionApp, SessionCall,
    SessionEndpoint, SessionHub, accept_websocket,
};
use crate::webhook::{Webhook, WebhookClient, WebhookError};

/// The status a host answers with when it cannot place a call at all — a bug on this side, and
/// never silence. RFC 3261 §21.5.1.
const INTERNAL_ERROR: u16 = 500;

/// The status for a request the host understands the shape of and has no user agent to answer:
/// an OPTIONS or a REGISTER arriving on a call listener. RFC 3261 §21.5.2.
const NOT_IMPLEMENTED: u16 = 501;

/// Why a host could not start, or could not keep running.
///
/// Exhaustive by design: a host either read its document, bound its listener and carried its
/// calls, or it failed at one of those three boundaries. A caller that wants to print the reason
/// needs no more resolution than that, and a fourth boundary would be a change to the host model
/// rather than another diagnosis inside it.
#[derive(Debug)]
pub enum HostError {
    /// The configuration document was refused. Carries the document's own diagnosis, which names
    /// the line.
    Config(ConfigError),
    /// A listener could not be bound, or a response could not be sent.
    Transport(sipx_transport::Error),
    /// A document-mode binding could not be constructed.
    Webhook(WebhookError),
    /// A session listener could not be bound or accept connections.
    SessionIo(std::io::Error),
    /// A named signing secret was absent when the host started.
    MissingSecret(String),
    /// The document declares no `sip` listener, so there is nothing for a call to arrive on. A
    /// document can be valid and still describe a host that cannot answer anything.
    NoCallListener,
}

/// One admitted document call. It owns the call, its event receiver and its interpreter; no value
/// is shared between actors, which is the design's one-call/one-program rule in process form.
#[derive(Debug)]
enum DocumentBinding {
    Webhook { webhook: Webhook, budget: Duration },
    Session(SessionCall),
}

impl DocumentBinding {
    async fn next_session_input(&mut self) -> SessionInput {
        let Self::Session(call) = self else {
            return std::future::pending().await;
        };
        match call.next_input().await {
            CallInput::Document(document) => SessionInput::Document(document),
            CallInput::Dead => SessionInput::Dead,
        }
    }
}

#[derive(Debug)]
enum SessionInput {
    Document(sipx_app_protocol::Document),
    Dead,
}

struct DocumentCall {
    handle: Handle,
    invitation: Option<Invitation>,
    invitation_events: Option<CallEvents>,
    call: Option<Call>,
    events: Option<CallEvents>,
    inbox: Option<mpsc::Receiver<Incoming>>,
    call_id: String,
    media_address: IpAddr,
    grants: Grants,
    interpreter: Interpreter,
    binding: DocumentBinding,
    timers: [Option<Instant>; 3],
    playbacks: BTreeMap<PlaybackId, (String, Playback)>,
    shutdown: watch::Receiver<bool>,
    stopping: bool,
    terminal: bool,
    initial: Option<EventKind>,
}

struct OutboundCall {
    call: Call,
    events: CallEvents,
    inbox: mpsc::Receiver<Incoming>,
    call_id: String,
}

impl DocumentCall {
    fn new(
        handle: Handle,
        mut invitation: Invitation,
        call_id: String,
        media_address: IpAddr,
        policy: AppPolicy,
        binding: DocumentBinding,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let invitation_events = invitation.events();
        let interpreter = Interpreter::new(
            incoming_snapshot(invitation.request(), &call_id),
            protocol_policy(&policy),
        );
        Self {
            handle,
            invitation: Some(invitation),
            invitation_events,
            call: None,
            events: None,
            inbox: None,
            call_id,
            media_address,
            grants: policy.grants,
            interpreter,
            binding,
            timers: [None; 3],
            playbacks: BTreeMap::new(),
            shutdown,
            stopping: false,
            terminal: false,
            initial: Some(EventKind::Incoming),
        }
    }

    fn new_outbound(
        handle: Handle,
        outbound: OutboundCall,
        media_address: IpAddr,
        policy: AppPolicy,
        session: SessionCall,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let OutboundCall {
            call,
            events,
            inbox,
            call_id,
        } = outbound;
        let interpreter_policy = protocol_policy(&policy);
        Self {
            handle,
            invitation: None,
            invitation_events: None,
            call: Some(call),
            events: Some(events),
            inbox: Some(inbox),
            call_id: call_id.clone(),
            media_address,
            grants: policy.grants,
            interpreter: Interpreter::new(
                CallSnapshot::new(call_id, Direction::Outbound),
                interpreter_policy,
            ),
            binding: DocumentBinding::Session(session),
            timers: [None; 3],
            playbacks: BTreeMap::new(),
            shutdown,
            stopping: false,
            terminal: false,
            initial: None,
        }
    }

    async fn run(mut self) -> String {
        if let Some(initial) = self.initial.take() {
            let outputs = self.interpreter.handle(timestamp(), Input::Event(initial));
            self.drive(outputs).await;
        }

        if self.stopping {
            self.teardown().await;
        } else if self.call.is_some() {
            self.serve_answered().await;
        } else if !self.terminal && self.invitation.is_some() {
            self.wait_for_cancel().await;
        }
        if self.stopping {
            self.teardown().await;
        }
        self.call_id
    }

    async fn wait_for_cancel(&mut self) {
        let action = self.next_invitation_action().await;
        if matches!(action, ActorAction::Closed | ActorAction::Shutdown) {
            self.stopping = true;
        }
        let outputs = self.apply_action(action).await;
        self.drive(outputs).await;
    }

    async fn serve_answered(&mut self) {
        loop {
            if self.call.as_ref().is_none_or(Call::is_ended) {
                break;
            }
            let action = self.next_answered_action().await;
            if matches!(action, ActorAction::Closed | ActorAction::Shutdown) {
                self.stopping = true;
                break;
            }
            let outputs = self.apply_action(action).await;
            self.drive(outputs).await;
            if self.stopping {
                break;
            }
        }
    }

    async fn next_answered_action(&mut self) -> ActorAction {
        let contract_timer = self.next_timer();
        let session_deadline = self.call.as_ref().and_then(Call::session_deadline);
        let Some(call) = self.call.as_mut() else {
            return ActorAction::Closed;
        };
        let Some(events) = self.events.as_mut() else {
            return ActorAction::Closed;
        };
        let Some(inbox) = self.inbox.as_mut() else {
            return ActorAction::Closed;
        };
        let shutdown = &mut self.shutdown;
        let binding = &mut self.binding;
        tokio::select! {
            _ = shutdown.changed() => ActorAction::Shutdown,
            event = events.recv() => event.map_or(ActorAction::Closed, ActorAction::CallEvent),
            incoming = inbox.recv() => incoming.map_or(ActorAction::Closed, |message| ActorAction::Incoming(Box::new(message))),
            digit = call.media().recv_digit() => ActorAction::Digit(digit.map(|(digit, duration)| {
                (
                    digit.as_char(),
                    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
                )
            })),
            () = sleep_until(contract_timer.map(|(at, _)| at)) => {
                contract_timer.map_or(ActorAction::Closed, |(_, timer)| ActorAction::Timer(timer))
            }
            () = sleep_until(session_deadline) => ActorAction::SessionDeadline,
            input = binding.next_session_input() => ActorAction::Session(input),
        }
    }

    async fn next_invitation_action(&mut self) -> ActorAction {
        let Some(events) = self.invitation_events.as_mut() else {
            return std::future::pending().await;
        };
        let shutdown = &mut self.shutdown;
        let binding = &mut self.binding;
        tokio::select! {
            _ = shutdown.changed() => ActorAction::Shutdown,
            event = events.recv() => event.map_or(ActorAction::Closed, ActorAction::CallEvent),
            input = binding.next_session_input() => ActorAction::Session(input),
        }
    }

    async fn apply_action(&mut self, action: ActorAction) -> Vec<Output> {
        match action {
            ActorAction::CallEvent(event) => self.call_event(event),
            ActorAction::Digit(Some((digit, duration_ms))) => self.interpreter.handle(
                timestamp(),
                Input::Event(EventKind::Dtmf { digit, duration_ms }),
            ),
            ActorAction::Digit(None) | ActorAction::Closed => Vec::new(),
            ActorAction::Shutdown => {
                self.stopping = true;
                Vec::new()
            }
            ActorAction::Incoming(incoming) => {
                let ended = if let Some(call) = self.call.as_mut() {
                    let _ = call.handle(&incoming).await;
                    call.is_ended()
                } else {
                    false
                };
                let mut outputs = Vec::new();
                // `Call::handle(BYE)` records `call.ended` before its 200 leaves. Drain that
                // synchronously queued event before returning to the HTTP race, so a simultaneously
                // ready webhook timeout cannot apply failure policy to an already-ended call.
                if ended {
                    loop {
                        let Some(event) = self.events.as_mut().and_then(CallEvents::try_recv)
                        else {
                            break;
                        };
                        outputs.extend(self.call_event(event));
                    }
                }
                outputs
            }
            ActorAction::Timer(timer) => {
                self.clear_timer(timer);
                self.interpreter
                    .handle(timestamp(), Input::TimerFired(timer))
            }
            ActorAction::SessionDeadline => {
                if let Some(call) = self.call.as_mut() {
                    let _ = call.on_session_deadline().await;
                }
                Vec::new()
            }
            ActorAction::Session(SessionInput::Document(document)) => self
                .interpreter
                .handle(timestamp(), Input::Document(document)),
            ActorAction::Session(SessionInput::Dead) => self
                .interpreter
                .handle(timestamp(), Input::BindingFailed(Failure::Unreachable)),
        }
    }

    fn call_event(&mut self, event: CallEvent) -> Vec<Output> {
        match event {
            CallEvent::Muted => self
                .interpreter
                .handle(timestamp(), Input::MediaGate { muted: true }),
            CallEvent::Unmuted => self
                .interpreter
                .handle(timestamp(), Input::MediaGate { muted: false }),
            CallEvent::PlaybackFinished {
                playback,
                completed,
            } => {
                let id = self
                    .playbacks
                    .remove(&playback)
                    .map_or_else(String::new, |(id, _)| id);
                self.interpreter.handle(
                    timestamp(),
                    Input::Event(EventKind::PlaybackFinished {
                        instruction_id: id,
                        completed,
                    }),
                )
            }
            other => {
                let id = self.interpreter.running().unwrap_or("");
                sipx_app_protocol::event_from_call(&other, id).map_or_else(Vec::new, |event| {
                    if matches!(event, EventKind::Ended { .. }) {
                        self.terminal = true;
                        // A pending INVITE's CANCEL transaction and 487 have already been
                        // answered by the dispatcher. Release its route now; retaining it
                        // would make `run` wait for a second event that can never exist.
                        self.invitation_events = None;
                        self.invitation = None;
                    }
                    self.interpreter.handle(timestamp(), Input::Event(event))
                })
            }
        }
    }

    async fn drive(&mut self, outputs: Vec<Output>) {
        let mut pending: VecDeque<Output> = outputs.into();
        while let Some(output) = pending.pop_front() {
            if self.stopping {
                break;
            }
            let more = match output {
                Output::Deliver { envelope, callback } => {
                    self.deliver_while_serving(*envelope, callback).await
                }
                Output::Effect(effect) => {
                    self.effect(effect).await;
                    Vec::new()
                }
                // The binding owns the same whole-exchange budget. Feeding a `Timeout` response
                // spends the callback token, so retaining a second wall timer here could only fire
                // late and be ignored.
                Output::SetTimer {
                    timer: Timer::Callback,
                    ..
                }
                | Output::ClearTimer(Timer::Callback) => Vec::new(),
                Output::SetTimer { timer, after_ms } => {
                    self.set_timer(timer, Duration::from_millis(u64::from(after_ms)));
                    Vec::new()
                }
                Output::ClearTimer(timer) => {
                    self.clear_timer(timer);
                    Vec::new()
                }
            };
            for output in more.into_iter().rev() {
                pending.push_front(output);
            }
        }
    }

    /// Keep the call actor live while one document callback is in flight.
    ///
    /// The contract permits only one app callback, not only one source of call input. A BYE,
    /// CANCEL, routed in-dialog request, media digit or contract timer can all race HTTP; leaving
    /// them unpolled until HTTP returns can strand a mandatory SIP response behind the callback's
    /// own timeout.
    async fn deliver_while_serving(
        &mut self,
        envelope: sipx_app_protocol::Envelope,
        callback: sipx_app_protocol::Callback,
    ) -> Vec<Output> {
        if let DocumentBinding::Session(call) = &self.binding {
            if matches!(
                call.deliver(&envelope),
                Err(DeliverError::UnknownCall | DeliverError::SessionOverflow)
            ) {
                return self.interpreter.handle(
                    timestamp(),
                    Input::Response {
                        callback,
                        response: Response::Failed(Failure::Unreachable),
                    },
                );
            }
            return self.deliver_session_while_serving(callback).await;
        }
        let (webhook, budget) = match &self.binding {
            DocumentBinding::Webhook { webhook, budget } => (webhook.clone(), *budget),
            DocumentBinding::Session(_) => return Vec::new(),
        };
        let delivery = webhook.deliver(&envelope, budget, unix_seconds());
        tokio::pin!(delivery);
        let mut deferred = Vec::new();

        let response = loop {
            let action = if self.call.is_some() {
                tokio::select! {
                    response = &mut delivery => break response,
                    action = self.next_answered_action() => action,
                }
            } else {
                tokio::select! {
                    biased;
                    action = self.next_invitation_action() => action,
                    response = &mut delivery => break response,
                }
            };
            if matches!(action, ActorAction::Closed | ActorAction::Shutdown) {
                self.stopping = true;
                return deferred;
            }
            let outputs = self.apply_action(action).await;
            self.perform_while_callback(outputs, &mut deferred).await;
        };

        deferred.extend(
            self.interpreter
                .handle(timestamp(), Input::Response { callback, response }),
        );
        deferred
    }

    async fn deliver_session_while_serving(
        &mut self,
        callback: sipx_app_protocol::Callback,
    ) -> Vec<Output> {
        let mut deferred = Vec::new();
        let response = loop {
            let action = if self.call.is_some() {
                self.next_answered_action().await
            } else {
                self.next_invitation_action().await
            };
            match action {
                ActorAction::Session(SessionInput::Document(document)) => {
                    break Response::Document(document);
                }
                ActorAction::Session(SessionInput::Dead) => {
                    break Response::Failed(Failure::Unreachable);
                }
                ActorAction::Closed | ActorAction::Shutdown => {
                    self.stopping = true;
                    return deferred;
                }
                other => {
                    let outputs = self.apply_action(other).await;
                    self.perform_while_callback(outputs, &mut deferred).await;
                }
            }
        };
        deferred.extend(
            self.interpreter
                .handle(timestamp(), Input::Response { callback, response }),
        );
        deferred
    }

    async fn perform_while_callback(&mut self, outputs: Vec<Output>, deferred: &mut Vec<Output>) {
        for output in outputs {
            match output {
                // The interpreter's callback token makes this unreachable while another callback
                // is outstanding. Preserve it for the ordinary driver if that invariant changes.
                Output::Deliver { .. } => deferred.push(output),
                Output::Effect(effect) => self.effect(effect).await,
                Output::SetTimer {
                    timer: Timer::Callback,
                    ..
                }
                | Output::ClearTimer(Timer::Callback) => {}
                Output::SetTimer { timer, after_ms } => {
                    self.set_timer(timer, Duration::from_millis(u64::from(after_ms)));
                }
                Output::ClearTimer(timer) => self.clear_timer(timer),
            }
        }
    }

    async fn effect(&mut self, effect: Effect) {
        match effect {
            Effect::Answer => self.answer().await,
            Effect::Reject { status, .. } => {
                if let Some(invitation) = self.invitation.take() {
                    let _ = invitation.refuse(&self.handle, status, "Rejected").await;
                    self.invitation_events = None;
                }
            }
            Effect::HangUp { .. } => {
                if let Some(call) = self.call.as_mut() {
                    let _ = call.hang_up().await;
                } else if let Some(invitation) = self.invitation.take() {
                    let _ = invitation
                        .refuse(&self.handle, INTERNAL_ERROR, "Server Internal Error")
                        .await;
                    self.invitation_events = None;
                }
            }
            Effect::Play {
                instruction_id,
                source,
                interruptible,
            } => {
                let Some(samples) = samples(&source, &self.grants) else {
                    self.fail_effect().await;
                    return;
                };
                if let Some(call) = self.call.as_ref() {
                    let interrupt = if interruptible {
                        Interrupt::OnDigit
                    } else {
                        Interrupt::Never
                    };
                    let playback = call.start_playback(samples, interrupt);
                    self.playbacks
                        .insert(playback.id(), (instruction_id, playback));
                }
            }
            Effect::StopPlayback => {
                for (_, playback) in self.playbacks.values() {
                    playback.stop();
                }
            }
            Effect::SendDigits {
                digits,
                duration_ms,
            } => {
                if let Some(call) = self.call.as_ref() {
                    let duration = Duration::from_millis(u64::from(duration_ms.unwrap_or(100)));
                    let _ = call.send_digits(&digits, duration).await;
                }
            }
            Effect::Mute => {
                if let Some(call) = self.call.as_ref() {
                    call.mute();
                }
            }
            Effect::Unmute => {
                if let Some(call) = self.call.as_ref() {
                    call.unmute();
                }
            }
            // These operations need host facilities outside phase 1 (outbound legs, recording
            // storage, coupling and transfers). The interpreter has still made the sole decision
            // about what the document means; the driver refuses an operation it cannot perform.
            _ => self.fail_effect().await,
        }
    }

    async fn answer(&mut self) {
        let Some(invitation) = self.invitation.as_ref() else {
            return;
        };
        let Ok(mut call) = invitation.answer(&self.handle, self.media_address).await else {
            if let Some(invitation) = self.invitation.take() {
                let _ = invitation
                    .refuse(&self.handle, INTERNAL_ERROR, "Server Internal Error")
                    .await;
            }
            self.invitation_events = None;
            return;
        };
        let events = call.events();
        let Some(invitation) = self.invitation.take() else {
            return;
        };
        let (_, inbox) = invitation.into_parts();
        self.invitation_events = None;
        self.events = events;
        self.inbox = Some(inbox);
        self.call = Some(call);
    }

    async fn fail_effect(&mut self) {
        if let Some(call) = self.call.as_mut() {
            let _ = call.hang_up().await;
        } else if let Some(invitation) = self.invitation.take() {
            let _ = invitation
                .refuse(&self.handle, INTERNAL_ERROR, "Server Internal Error")
                .await;
            self.invitation_events = None;
        }
    }

    /// Finish one actor after the host has stopped accepting network work.
    ///
    /// Both SIP operations are themselves bounded by the transaction layer. The actor awaits
    /// them instead of dropping the call state, so a confirmed dialog gets its BYE and a pending
    /// invitation gets an explicit final response while its endpoint is still usable.
    async fn teardown(&mut self) {
        if let Some(call) = self.call.as_mut()
            && !call.is_ended()
        {
            let _ = call.hang_up().await;
        }
        if let Some(invitation) = self.invitation.take() {
            let _ = invitation
                .refuse(&self.handle, INTERNAL_ERROR, "Server Shutting Down")
                .await;
            self.invitation_events = None;
        }
    }

    fn set_timer(&mut self, timer: Timer, after: Duration) {
        if let Some(slot) = timer_slot(timer)
            && let Some(deadline) = self.timers.get_mut(slot)
        {
            *deadline = Instant::now().checked_add(after);
        }
    }

    fn clear_timer(&mut self, timer: Timer) {
        if let Some(slot) = timer_slot(timer)
            && let Some(deadline) = self.timers.get_mut(slot)
        {
            *deadline = None;
        }
    }

    fn next_timer(&self) -> Option<(Instant, Timer)> {
        [Timer::Pause, Timer::GatherOverall, Timer::GatherDigit]
            .into_iter()
            .filter_map(|timer| {
                timer_slot(timer).and_then(|slot| {
                    self.timers
                        .get(slot)
                        .copied()
                        .flatten()
                        .map(|at| (at, timer))
                })
            })
            .min_by_key(|(at, _)| *at)
    }
}

enum ActorAction {
    CallEvent(CallEvent),
    Incoming(Box<Incoming>),
    Digit(Option<(char, u32)>),
    Timer(Timer),
    SessionDeadline,
    Session(SessionInput),
    Shutdown,
    Closed,
}

type CallActor = Pin<Box<dyn Future<Output = ActorCompletion> + Send + 'static>>;

/// Maximum actor starts or completions that may wait behind the serving loop. Reaching it applies
/// backpressure to admission or actor completion; it never allocates an unbounded background
/// queue. Active calls themselves remain independently bounded by the host's transport limits.
const ACTOR_QUEUE: usize = 1024;
const MAX_ACTORS: usize = 1024;

/// Owns call actors beyond the lifetime of the `serve` future that admitted them.
///
/// On the ordinary path `serve` receives every completion and joins the supervisor. If its own
/// future is cancelled, dropping this guard closes the spawn channel and signals shutdown; the
/// supervisor still owns the actor `JoinSet`, so it cooperatively drains calls instead of letting
/// `JoinSet::drop` abort them at an arbitrary SIP await.
struct ActorSupervisor {
    spawn: Option<mpsc::Sender<CallActor>>,
    completed: mpsc::Receiver<ActorCompletion>,
    shutdown: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    completion_open: bool,
    permits: std::sync::Arc<Semaphore>,
}

impl ActorSupervisor {
    fn new() -> Self {
        Self::with_limit(MAX_ACTORS)
    }

    fn with_limit(actor_limit: usize) -> Self {
        let (spawn, requests) = mpsc::channel(ACTOR_QUEUE);
        let (completed_tx, completed) = mpsc::channel(ACTOR_QUEUE);
        let (shutdown, _) = watch::channel(false);
        let actor_shutdown = shutdown.clone();
        let task = tokio::spawn(supervise_actors(requests, completed_tx, actor_shutdown));
        Self {
            spawn: Some(spawn),
            completed,
            shutdown,
            task: Some(task),
            completion_open: true,
            permits: std::sync::Arc::new(Semaphore::new(actor_limit)),
        }
    }

    fn receiver(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    fn reserve(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.permits.clone().try_acquire_owned().ok()
    }

    async fn spawn(&self, actor: CallActor) {
        if let Some(spawn) = &self.spawn {
            let _ = spawn.send(actor).await;
        }
    }

    async fn join_next(&mut self) -> Option<ActorCompletion> {
        let completed = self.completed.recv().await;
        if completed.is_none() {
            self.completion_open = false;
        }
        completed
    }

    fn completion_open(&self) -> bool {
        self.completion_open
    }

    async fn finish(&mut self, running: &mut Running) {
        self.stop();
        while let Some(completion) = self.completed.recv().await {
            running.complete(completion);
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    fn stop(&mut self) {
        let _ = self.shutdown.send(true);
        self.spawn = None;
    }
}

impl Drop for ActorSupervisor {
    fn drop(&mut self) {
        self.stop();
        // The supervisor, not a call actor, remains scheduled here. It owns and joins every actor
        // after the cancelled serving future can no longer await asynchronous SIP teardown.
        let _ = self.task.take();
    }
}

async fn supervise_actors(
    mut requests: mpsc::Receiver<CallActor>,
    completed: mpsc::Sender<ActorCompletion>,
    shutdown: watch::Sender<bool>,
) {
    let mut actors = JoinSet::new();
    loop {
        tokio::select! {
            actor = requests.recv() => match actor {
                Some(actor) => {
                    actors.spawn(actor);
                }
                None => break,
            },
            joined = actors.join_next(), if !actors.is_empty() => {
                if let Some(Ok(call)) = joined {
                    let _ = completed.send(call).await;
                }
            }
        }
    }
    let _ = shutdown.send(true);
    while let Some(joined) = actors.join_next().await {
        if let Ok(call) = joined {
            let _ = completed.send(call).await;
        }
    }
}

fn timer_slot(timer: Timer) -> Option<usize> {
    match timer {
        Timer::Pause => Some(0),
        Timer::GatherOverall => Some(1),
        Timer::GatherDigit => Some(2),
        Timer::Callback => None,
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn protocol_policy(policy: &AppPolicy) -> Policy {
    Policy {
        timeout_ms: u32::try_from(policy.failure.timeout.as_millis()).unwrap_or(u32::MAX),
        on_timeout: protocol_action(&policy.failure.on_timeout),
        on_5xx: protocol_action(&policy.failure.on_5xx),
        on_unreachable: protocol_action(&policy.failure.on_unreachable),
        on_4xx: protocol_action(&policy.failure.on_4xx),
        dial_headers: policy.grants.dial_headers.clone(),
    }
}

fn protocol_action(action: &OnFailure) -> ContractOnFailure {
    match action {
        OnFailure::Continue => ContractOnFailure::Continue,
        OnFailure::Hangup => ContractOnFailure::Hangup,
        OnFailure::Reject { status } => ContractOnFailure::Reject { status: *status },
    }
}

fn incoming_snapshot(incoming: &Incoming, id: &str) -> CallSnapshot {
    let header = |name: HeaderName| {
        incoming
            .request
            .headers
            .value(&name)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default()
    };
    let mut snapshot = CallSnapshot::new(id, Direction::Inbound)
        .between(header(HeaderName::From), header(HeaderName::To));
    for name in sipx_app_protocol::Headers::SELECTED {
        let parsed = match name {
            "from" => HeaderName::From,
            "to" => HeaderName::To,
            "p-asserted-identity" => HeaderName::parse(&Bytes::from_static(b"P-Asserted-Identity")),
            "diversion" => HeaderName::parse(&Bytes::from_static(b"Diversion")),
            _ => continue,
        };
        let value = header(parsed);
        if !value.is_empty() {
            snapshot.headers.set(name, value);
        }
    }
    snapshot
}

fn samples(source: &Source, grants: &Grants) -> Option<Vec<i16>> {
    match source {
        Source::Inline(bytes) => {
            let mut chunks = bytes.chunks_exact(2);
            let samples = chunks
                .by_ref()
                .filter_map(|chunk| <[u8; 2]>::try_from(chunk).ok())
                .map(i16::from_le_bytes)
                .collect();
            chunks.remainder().is_empty().then_some(samples)
        }
        Source::File(path) => {
            let requested = std::fs::canonicalize(path).ok()?;
            let allowed = grants.play_roots.iter().any(|root| {
                std::fs::canonicalize(root).is_ok_and(|root| requested.starts_with(root))
            });
            if !allowed {
                return None;
            }
            let file = std::fs::File::open(requested).ok()?;
            let wave = sipx_audio::read_wav(file).ok()?;
            (wave.sample_rate == 8_000).then_some(wave.samples)
        }
    }
}

fn unix_millis() -> i64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn unix_seconds() -> i64 {
    unix_millis() / 1_000
}

fn timestamp() -> Timestamp {
    Timestamp::from_unix_millis(unix_millis())
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Transport(error) => write!(formatter, "{error}"),
            Self::Webhook(error) => write!(formatter, "{error}"),
            Self::SessionIo(error) => write!(formatter, "session listener: {error}"),
            Self::MissingSecret(name) => write!(
                formatter,
                "signing secret `{name}` is not available as `SIPX_SECRET_{name}`"
            ),
            Self::NoCallListener => write!(
                formatter,
                "the document declares no `sip` listener, so no call can arrive"
            ),
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Transport(error) => Some(error),
            Self::Webhook(error) => Some(error),
            Self::SessionIo(error) => Some(error),
            Self::MissingSecret(_) | Self::NoCallListener => None,
        }
    }
}

impl From<ConfigError> for HostError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<sipx_transport::Error> for HostError {
    fn from(error: sipx_transport::Error) -> Self {
        Self::Transport(error)
    }
}

impl From<WebhookError> for HostError {
    fn from(error: WebhookError) -> Self {
        Self::Webhook(error)
    }
}

struct PreparedOriginated {
    call_id: String,
    call: Call,
    events: CallEvents,
    inbox: mpsc::Receiver<Incoming>,
    policy: AppPolicy,
    session: SessionCall,
    permit: tokio::sync::OwnedSemaphorePermit,
}

struct OriginateContext {
    handle: Handle,
    calls: Calls,
    endpoint: SessionEndpoint,
    media_address: IpAddr,
    policy: AppPolicy,
    permit: tokio::sync::OwnedSemaphorePermit,
}

enum OriginateOutcome {
    Ready(Box<PreparedOriginated>),
    Failed(OriginateRequest),
}

struct OriginateTasks {
    tasks: JoinSet<OriginateOutcome>,
    shutdown: watch::Sender<bool>,
}

impl OriginateTasks {
    fn new() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            tasks: JoinSet::new(),
            shutdown,
        }
    }

    fn spawn(&mut self, task: impl Future<Output = OriginateOutcome> + Send + 'static) {
        self.tasks.spawn(task);
    }

    async fn join_next(&mut self) -> Option<OriginateOutcome> {
        self.tasks.join_next().await.and_then(Result::ok)
    }

    fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    fn receiver(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    fn stop(&self) {
        let _ = self.shutdown.send(true);
    }
}

impl Drop for OriginateTasks {
    fn drop(&mut self) {
        self.stop();
        // Setup tasks retain their own cancellation signal and perform ACK/BYE or CANCEL cleanup.
        // Detaching lets that bounded cleanup finish after a cancelled host future drops this
        // owner; aborting here would recreate the invitation leak `dial_early_until` closes.
        self.tasks.detach_all();
    }
}

async fn prepare_originated(
    context: OriginateContext,
    request: OriginateRequest,
    mut shutdown: watch::Receiver<bool>,
) -> OriginateOutcome {
    let OriginateContext {
        handle,
        calls,
        endpoint,
        media_address,
        policy,
        permit,
    } = context;
    let Some(uri) = Uri::parse(Bytes::copy_from_slice(request.target.as_bytes())).ok() else {
        return OriginateOutcome::Failed(request);
    };
    let valid_from = Uri::parse(Bytes::copy_from_slice(request.from.as_bytes()))
        .is_ok_and(|from| from.scheme().is_sip());
    if !uri.scheme().is_sip() || !valid_from {
        return OriginateOutcome::Failed(request);
    }
    let Some(target) = resolve_originate_target(&uri).await else {
        return OriginateOutcome::Failed(request);
    };
    let options = DialOptions::new(request.from.clone(), media_address);
    let Ok(dialing) = dial_early_until(
        &handle,
        target,
        &uri,
        &options,
        shutdown_signal(&mut shutdown),
    )
    .await
    else {
        return OriginateOutcome::Failed(request);
    };
    let Some(dialog) = dialing.dialog().cloned() else {
        return OriginateOutcome::Failed(request);
    };
    let call_id = String::from_utf8_lossy(&dialog.id.call_id).into_owned();
    let inbox = calls.register(&dialog);
    let mut dialing = dialing;
    let Some(events) = dialing.events() else {
        calls.forget(&dialog);
        return OriginateOutcome::Failed(request);
    };
    let Ok(call) = dialing.answered_until(shutdown_signal(&mut shutdown)).await else {
        return OriginateOutcome::Failed(request);
    };
    let Ok((session, reply)) = endpoint.originated(&request, call_id.clone()) else {
        let mut call = call;
        let _ = call.hang_up().await;
        return OriginateOutcome::Failed(request);
    };
    if endpoint.send_reply(request.session, &reply).is_err() {
        // The actor below applies this app's own `on_unreachable`; sending the result and call
        // events use the same bounded queue, so a failure here has already marked the session dead.
    }
    OriginateOutcome::Ready(Box::new(PreparedOriginated {
        call_id,
        call,
        events,
        inbox,
        policy,
        session,
        permit,
    }))
}

async fn resolve_originate_target(uri: &Uri) -> Option<Target> {
    match uri.host()? {
        UriHost::Ip(ip) => Some(Target::udp(std::net::SocketAddr::new(
            *ip,
            uri.port().unwrap_or(5060),
        ))),
        UriHost::Name(_) => {
            let resolver = Arc::new(sipx_transport::dns::DnsResolver::from_system().ok()?);
            let mut rng = FirstCandidate;
            sipx_transport::dns::resolve_uri(uri, &resolver, &mut rng)
                .await
                .into_iter()
                .next()
        }
    }
}

struct FirstCandidate;

impl sipx_transport::resolve::Rng for FirstCandidate {
    fn below(&mut self, _max: u32) -> u32 {
        0
    }
}

/// A host with a configuration in force.
///
/// Constructed from a document rather than from parts, because the document is the unit a reload
/// applies in (N9) and a host assembled field by field could not honour that.
#[derive(Debug)]
pub struct Host {
    running: Running,
    media_address: IpAddr,
    webhooks: BTreeMap<String, Webhook>,
    sessions: SessionEndpoint,
}

impl Host {
    /// Read a host configuration document and put it in force.
    ///
    /// `media_address` is the address RTP is advertised on. It is separate from any listener's bind
    /// address for the reason `sipx-transport` gives for `sent_by`: behind a NAT the address to
    /// advertise and the address to bind differ, and guessing produces a call with one-way audio.
    ///
    /// # Errors
    /// The document's own refusal, naming the line.
    pub fn start(document: &str, media_address: IpAddr) -> Result<Self, HostError> {
        Self::start_with_secrets(document, media_address, |name| {
            std::env::var(format!("SIPX_SECRET_{name}"))
                .ok()
                .map(String::into_bytes)
        })
    }

    /// Start with an explicit secret resolver.
    ///
    /// The binary uses `SIPX_SECRET_<name>` through [`Self::start`]. Supplying the resolver makes
    /// startup deterministic for embedders and tests while keeping material outside the document.
    pub fn start_with_secrets(
        document: &str,
        media_address: IpAddr,
        mut resolve: impl FnMut(&str) -> Option<Vec<u8>>,
    ) -> Result<Self, HostError> {
        let config = HostConfig::parse(document)?;
        let mut webhooks = BTreeMap::new();
        let mut session_apps = BTreeMap::new();
        let has_webhooks = config
            .apps()
            .any(|app| matches!(&app.binding, AppBinding::Webhook { .. }));
        let http = if has_webhooks {
            Some(WebhookClient::new()?)
        } else {
            None
        };
        for app in config.apps() {
            match &app.binding {
                AppBinding::Webhook {
                    url,
                    signing_secrets,
                } => {
                    let mut secrets = Vec::with_capacity(signing_secrets.len());
                    for name in signing_secrets {
                        let Some(secret) = resolve(name.as_str()) else {
                            return Err(HostError::MissingSecret(name.to_string()));
                        };
                        secrets.push(secret);
                    }
                    let Some(http) = http.as_ref() else {
                        continue;
                    };
                    webhooks.insert(app.name.clone(), Webhook::with_client(http, url, secrets)?);
                }
                AppBinding::Session { bearer_secret } => {
                    let Some(secret) = resolve(bearer_secret.as_str()) else {
                        return Err(HostError::MissingSecret(bearer_secret.to_string()));
                    };
                    session_apps.insert(
                        app.name.clone(),
                        SessionApp::new(secret, app.grants.originate),
                    );
                }
                AppBinding::Embedded { .. } => {}
            }
        }
        let sessions = SessionEndpoint::new(SessionHub::new(), session_apps);
        Ok(Self {
            running: Running::start(config),
            media_address,
            webhooks,
            sessions,
        })
    }

    /// The configuration new calls are admitted under.
    pub fn running(&self) -> &Running {
        &self.running
    }

    /// The live full-duplex session registry.
    pub fn sessions(&self) -> &SessionHub {
        self.sessions.hub()
    }

    /// The authenticated session frame endpoint.
    pub fn session_endpoint(&self) -> &SessionEndpoint {
        &self.sessions
    }

    /// The first `sip` listener in the document, which is the one [`Host::run`] answers on.
    ///
    /// A document may declare several; binding all of them concurrently is a host-lifecycle
    /// question rather than an API-surface one, and this application exists to exercise the API.
    ///
    /// # Errors
    /// [`HostError::NoCallListener`] when the document declares only session listeners.
    pub fn call_listener(&self) -> Result<Listener, HostError> {
        self.running
            .current()
            .listeners()
            .find(|listener| listener.protocol == Protocol::Sip)
            .cloned()
            .ok_or(HostError::NoCallListener)
    }

    /// The first configured application-session listener.
    #[must_use]
    pub fn session_listener(&self) -> Option<Listener> {
        self.running
            .current()
            .listeners()
            .find(|listener| listener.protocol == Protocol::Session)
            .cloned()
    }

    /// Bind the configured session listener, if there is one.
    pub async fn bind_session_endpoint(&self) -> Result<Option<TcpListener>, HostError> {
        let Some(listener) = self.session_listener() else {
            return Ok(None);
        };
        TcpListener::bind(listener.bind)
            .await
            .map(Some)
            .map_err(HostError::SessionIo)
    }

    /// Bind the first `sip` listener and answer calls on it until the endpoint closes.
    ///
    /// # Errors
    /// A document with no call listener, or a listener that could not be bound.
    pub async fn run(&mut self) -> Result<(), HostError> {
        let (handle, incoming) = self.bind_endpoint().await?;
        Box::pin(self.serve(handle, incoming)).await
    }

    /// Bind the configured call listener without entering the serving loop.
    ///
    /// This is the readiness boundary for process wrappers: once it returns, [`Handle::local_addr`]
    /// is an address a far end can call, including when the document requested port zero.
    pub async fn bind_endpoint(&self) -> Result<(Handle, mpsc::Receiver<Incoming>), HostError> {
        let listener = self.call_listener()?;
        bind(Self::endpoint_config(&listener))
            .await
            .map_err(HostError::from)
    }

    /// Answer calls on an endpoint that is already bound, until it closes.
    ///
    /// [`Host::run`] is this plus the bind. They are separate so that **something can drive the
    /// application** (`X-38` rework): a document binds `127.0.0.1:0`, so a test that let the host bind
    /// could not learn the port to send an INVITE to, and the review found that nothing in the
    /// repository ran this module at all — the story claimed "sipx-app answers a call" and no test
    /// asserted it. A caller that binds the endpoint itself knows the address, and everything below
    /// this line is the same code `run` executes.
    ///
    /// # Errors
    /// A document with no call listener.
    pub async fn serve(
        &mut self,
        handle: Handle,
        incoming: mpsc::Receiver<Incoming>,
    ) -> Result<(), HostError> {
        let listener = self.call_listener()?;
        let agent = UserAgent::new(handle.clone(), Self::agent_config(&listener));
        let mut dispatcher = Dispatcher::new(handle.clone(), incoming);
        let outbound_calls = dispatcher.calls();
        let session_listener = self.bind_session_endpoint().await?;
        let (originates, mut originate_rx) = mpsc::channel::<OriginateRequest>(64);
        let (socket_shutdown, _) = watch::channel(false);
        let socket_limit = Arc::new(Semaphore::new(MAX_CONNECTION_TASKS));
        let mut sockets = JoinSet::new();
        let mut originating = OriginateTasks::new();
        // The serving future owns every call task. N11 is why admission cannot end at spawn time,
        // and ownership is why endpoint shutdown cannot silently discard a live SIP dialog: the
        // stop signal reaches every actor, then this future joins every actor before returning.
        let mut actors = ActorSupervisor::new();

        loop {
            // Keep one `Dispatcher::next` future alive while calls report completion. Dropping and
            // recreating it when an ending arrives could cancel a response the dispatcher is in
            // the middle of sending; pinning it across the inner select preserves that work while
            // letting admissions be released even when no new network request follows.
            let event = {
                let next = dispatcher.next();
                tokio::pin!(next);
                loop {
                    tokio::select! {
                        event = &mut next => break event,
                        joined = actors.join_next(), if actors.completion_open() => {
                            if let Some(completion) = joined {
                                self.running.complete(completion);
                            }
                        }
                        accepted = accept_session(session_listener.as_ref()) => {
                            let stream = accepted.map_err(HostError::SessionIo)?;
                            if let Ok(permit) = Arc::clone(&socket_limit).try_acquire_owned() {
                                let endpoint = self.sessions.clone();
                                let originates = originates.clone();
                                let shutdown = socket_shutdown.subscribe();
                                sockets.spawn(async move {
                                    let _permit = permit;
                                    let _ = accept_websocket(stream, endpoint, originates, shutdown).await;
                                });
                            }
                        }
                        Some(request) = originate_rx.recv() => {
                            let policy = self.running.app_policy(&request.app);
                            let permit = actors.reserve();
                            if let (Some(policy), Some(permit)) = (policy, permit) {
                                originating.spawn(prepare_originated(
                                    OriginateContext {
                                        handle: handle.clone(),
                                        calls: outbound_calls.clone(),
                                        endpoint: self.sessions.clone(),
                                        media_address: self.media_address,
                                        policy,
                                        permit,
                                    },
                                    request,
                                    originating.receiver(),
                                ));
                            } else {
                                let reply = SessionEndpoint::originate_failed(&request);
                                let _ = self.sessions.send_reply(request.session, &reply);
                            }
                        }
                        outcome = originating.join_next(), if !originating.is_empty() => {
                            if let Some(outcome) = outcome {
                                self.handle_originated(outcome, &handle, &mut actors).await;
                            }
                        }
                        Some(_) = sockets.join_next(), if !sockets.is_empty() => {}
                    }
                }
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Dispatched::Invitation(invitation) => {
                    self.admit(&handle, &listener, invitation, &mut actors)
                        .await;
                }
                Dispatched::OutOfDialog(request) => {
                    answer_out_of_dialog(&agent, &handle, &request).await;
                }
                _ => {}
            }
        }
        let _ = socket_shutdown.send(true);
        while sockets.join_next().await.is_some() {}
        originating.stop();
        while let Some(outcome) = originating.join_next().await {
            self.handle_originated(outcome, &handle, &mut actors).await;
        }
        actors.finish(&mut self.running).await;
        Ok(())
    }

    /// The endpoint configuration a listener asks for.
    fn endpoint_config(listener: &Listener) -> Config {
        let mut config = Config::new(listener.bind);
        // A listener bound to `0.0.0.0` has nothing sensible to advertise, and `sipx-transport`
        // warns that letting the default stand tells the far end to reply to `0.0.0.0`. The
        // document's `advertise` is exactly the operator's answer to that.
        if let Some(advertise) = &listener.advertise {
            config.sent_by.clone_from(advertise);
        }
        config
    }

    /// The user agent that answers what a call listener must answer and a call cannot.
    ///
    /// A [`UserAgent`] cannot be built without registration parameters even when only its answering
    /// half is wanted, so the registrar named here is the listener's own address and nothing ever
    /// sends to it — `register` is not called. That friction is the API's rather than this host's,
    /// and it is recorded here rather than worked around silently.
    fn agent_config(listener: &Listener) -> AgentConfig {
        let contact = format!("<sip:{}>", listener.bind);
        AgentConfig::new(
            contact.clone(),
            contact,
            Uri::sip(UriHost::Ip(listener.bind.ip())),
            Target::udp(listener.bind),
        )
    }

    /// Admit one invitation under the document's routing, and act on the answer.
    #[allow(clippy::too_many_lines)]
    fn admit<'a>(
        &'a mut self,
        handle: &'a Handle,
        listener: &'a Listener,
        invitation: Invitation,
        actors: &'a mut ActorSupervisor,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let call_id = call_id(invitation.request());
            match self.running.admit(&call_id, &listener.name) {
                Admission::App(policy) => {
                    if let AppBinding::Webhook { .. } = &policy.binding
                        && let Some(webhook) = self.webhooks.get(&policy.app).cloned()
                    {
                        let Some(permit) = actors.reserve() else {
                            let _ = invitation.refuse(handle, 503, "Service Unavailable").await;
                            self.running.end(&call_id);
                            return;
                        };
                        let Some(lease) = self.running.completion_lease(call_id.clone()) else {
                            let _ = invitation
                                .refuse(handle, INTERNAL_ERROR, "Server Internal Error")
                                .await;
                            self.running.end(&call_id);
                            return;
                        };
                        let budget = policy.failure.timeout;
                        let actor = DocumentCall::new(
                            handle.clone(),
                            invitation,
                            call_id,
                            self.media_address,
                            *policy,
                            DocumentBinding::Webhook { webhook, budget },
                            actors.receiver(),
                        );
                        actors
                            .spawn(Box::pin(async move {
                                let _permit = permit;
                                let _call_id = actor.run().await;
                                let completion = lease.completion();
                                drop(lease);
                                completion
                            }))
                            .await;
                        return;
                    }

                    if matches!(&policy.binding, AppBinding::Session { .. })
                        && let Ok(session) = self.sessions.hub().pin(&policy.app, call_id.clone())
                    {
                        let Some(permit) = actors.reserve() else {
                            let _ = invitation.refuse(handle, 503, "Service Unavailable").await;
                            self.running.end(&call_id);
                            return;
                        };
                        let Some(lease) = self.running.completion_lease(call_id.clone()) else {
                            let _ = invitation
                                .refuse(handle, INTERNAL_ERROR, "Server Internal Error")
                                .await;
                            self.running.end(&call_id);
                            return;
                        };
                        let actor = DocumentCall::new(
                            handle.clone(),
                            invitation,
                            call_id,
                            self.media_address,
                            *policy,
                            DocumentBinding::Session(session),
                            actors.receiver(),
                        );
                        actors
                            .spawn(Box::pin(async move {
                                let _permit = permit;
                                let _call_id = actor.run().await;
                                let completion = lease.completion();
                                drop(lease);
                                completion
                            }))
                            .await;
                        return;
                    }

                    // An absent session and the later embedded binding are unreachable in the
                    // document's own vocabulary.
                    match policy.failure.on_unreachable {
                        OnFailure::Reject { status } => {
                            let _ = invitation.refuse(handle, status, "Unavailable").await;
                            self.running.end(&call_id);
                        }
                        OnFailure::Hangup => {
                            let shutdown = actors.receiver();
                            self.carry(handle, invitation, call_id, true, actors, shutdown)
                                .await;
                        }
                        OnFailure::Continue => {
                            let shutdown = actors.receiver();
                            self.carry(handle, invitation, call_id, false, actors, shutdown)
                                .await;
                        }
                    }
                }
                Admission::Refuse(status) => {
                    let _ = invitation.refuse(handle, status, "Declined").await;
                }
                // A listener that is not there, or a session listener a call arrived on. Both are
                // host bugs rather than the caller's problem, and neither is ever silence (N6).
                Admission::NoSuchListener | Admission::NotACallListener => {
                    let _ = invitation
                        .refuse(handle, INTERNAL_ERROR, "Server Internal Error")
                        .await;
                }
            }
        })
    }

    /// Answer an invitation and serve the call to its end on its own task.
    ///
    /// The owned task returns `call_id` whichever way the call finishes, so the admission is
    /// forgotten exactly once and only after the call is really over.
    async fn handle_originated(
        &mut self,
        outcome: OriginateOutcome,
        handle: &Handle,
        actors: &mut ActorSupervisor,
    ) {
        let prepared = match outcome {
            OriginateOutcome::Ready(prepared) => *prepared,
            OriginateOutcome::Failed(request) => {
                let reply = SessionEndpoint::originate_failed(&request);
                let _ = self.sessions.send_reply(request.session, &reply);
                return;
            }
        };
        let PreparedOriginated {
            call_id,
            call,
            events,
            inbox,
            policy,
            session,
            permit,
        } = prepared;
        self.running.admit_outbound(&call_id, policy.clone());
        let Some(lease) = self.running.completion_lease(call_id.clone()) else {
            let mut call = call;
            let _ = call.hang_up().await;
            self.running.end(&call_id);
            return;
        };
        let actor = DocumentCall::new_outbound(
            handle.clone(),
            OutboundCall {
                call,
                events,
                inbox,
                call_id,
            },
            self.media_address,
            policy,
            session,
            actors.receiver(),
        );
        actors
            .spawn(Box::pin(async move {
                let _permit = permit;
                let _call_id = actor.run().await;
                let completion = lease.completion();
                drop(lease);
                completion
            }))
            .await;
    }

    async fn carry(
        &mut self,
        handle: &Handle,
        invitation: Invitation,
        call_id: String,
        hang_up_at_once: bool,
        actors: &ActorSupervisor,
        mut shutdown: watch::Receiver<bool>,
    ) {
        let Some(permit) = actors.reserve() else {
            let _ = invitation.refuse(handle, 503, "Service Unavailable").await;
            self.running.end(&call_id);
            return;
        };
        // The caller gave up, or the answer could not be sent. Either way there is no call to carry
        // and nothing useful to say to a peer that is already gone — but the admission still has to
        // be released, or a host that is refused often enough leaks one entry per attempt.
        let Ok(mut call) = invitation.answer(handle, self.media_address).await else {
            self.running.end(&call_id);
            return;
        };
        let (_, mut inbox) = invitation.into_parts();
        let Some(lease) = self.running.completion_lease(call_id.clone()) else {
            self.running.end(&call_id);
            return;
        };
        actors
            .spawn(Box::pin(async move {
                let _permit = permit;
                if hang_up_at_once {
                    let _ = call.hang_up().await;
                } else {
                    // `serve` is the one loop: it honours the RFC 4028 session timer, answers what
                    // the call does not claim rather than dropping it, and returns when the call
                    // ends.
                    tokio::select! {
                        () = shutdown_signal(&mut shutdown) => {}
                        _ = sipx_call::serve(&mut call, &mut inbox) => {}
                    }
                    if !call.is_ended() {
                        let _ = call.hang_up().await;
                    }
                }
                let completion = lease.completion();
                drop(lease);
                completion
            }))
            .await;
    }
}

async fn shutdown_signal(shutdown: &mut watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    let _ = shutdown.changed().await;
}

async fn accept_session(listener: Option<&TcpListener>) -> std::io::Result<TcpStream> {
    let Some(listener) = listener else {
        return std::future::pending().await;
    };
    listener.accept().await.map(|(stream, _)| stream)
}

/// Answer a request that arrived outside any dialog.
///
/// RFC 3261 §11 makes OPTIONS a liveness probe, and a host that leaves it unanswered is one a
/// carrier marks down — so this is not optional politeness. The user agent owns the `Allow` list
/// that says what this stack answers; going through it rather than building a 200 here is what keeps
/// the advertised list and the real one from drifting apart. Anything it does not claim gets an
/// honest "not implemented" rather than silence.
async fn answer_out_of_dialog(agent: &UserAgent, handle: &Handle, request: &Incoming) {
    if !matches!(agent.answer(request).await, Ok(true)) {
        refuse(handle, request, NOT_IMPLEMENTED, "Not Implemented").await;
    }
}

/// A request's `Call-ID`, which is the name a call is admitted and remembered under.
///
/// A request with no `Call-ID` cannot be a dialog-forming INVITE — the parser would have refused it
/// — but reading the header still has to produce a value rather than a panic, so an absent one
/// becomes an empty name and is admitted and ended under it like any other.
fn call_id(incoming: &Incoming) -> String {
    incoming
        .request
        .headers
        .value(&HeaderName::CallId)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .unwrap_or_default()
}

/// Answer a request with a refusal.
///
/// Errors are dropped on purpose: this is already the failure path, and a host that cannot send a
/// refusal has nothing better to try.
async fn refuse(handle: &Handle, incoming: &Incoming, status: u16, reason: &'static str) {
    let Some(status) = StatusCode::new(status) else {
        return;
    };
    let Ok(builder) = ResponseBuilder::to_request(&incoming.request, status, reason) else {
        return;
    };
    let _ = handle.respond(&incoming.key, builder.build()).await;
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use sipx_call::{DialOptions, dial};
    use sipx_sip::{HostName, Method, Request, Response, build::RequestBuilder};
    use sipx_transport::TransportKind;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    /// A document with one UDP call listener routed to one app.
    const DOCUMENT: &str = r#"
[listener.edge]
protocol  = "sip"
transport = "udp"
bind      = "127.0.0.1:0"
app       = "greeter"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
"#;

    fn webhook_document(url: &str) -> String {
        format!(
            r#"
[listener.edge]
protocol  = "sip"
transport = "udp"
bind      = "127.0.0.1:0"
app       = "greeter"

[app.greeter]
binding = "webhook"
url = "{url}"
signing_secrets = ["hook"]

[app.greeter.on_failure]
timeout_ms = 250
on_timeout = "hangup"
on_4xx = {{ reject = 488 }}
"#
        )
    }

    async fn rejecting_webhook() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let url = format!("http://{}/hook", listener.local_addr().expect("address"));
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accepts");
            let mut request = [0_u8; 4096];
            let _ = socket.read(&mut request).await.expect("reads");
            socket
                .write_all(b"HTTP/1.1 400 Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("writes");
        });
        (url, task)
    }

    async fn answering_then_holding_webhook()
    -> (String, oneshot::Receiver<()>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let url = format!("http://{}/hook", listener.local_addr().expect("address"));
        let (held, held_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.expect("accepts incoming callback");
            let mut request = [0_u8; 4096];
            let _ = first
                .read(&mut request)
                .await
                .expect("reads incoming event");
            let body =
                r#"{"contract":"sipx.app.v1","instructions":[{"id":"answer","do":"answer"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            first
                .write_all(response.as_bytes())
                .await
                .expect("answers with answer instruction");

            let (mut second, _) = listener.accept().await.expect("accepts answered callback");
            let _ = second
                .read(&mut request)
                .await
                .expect("reads answered event");
            held.send(()).expect("reports held callback");
            let mut drained = [0_u8; 256];
            while second.read(&mut drained).await.expect("waits for timeout") != 0 {}

            let (mut third, _) = listener.accept().await.expect("accepts terminal callback");
            let mut terminal = Vec::new();
            loop {
                let read = third
                    .read(&mut request)
                    .await
                    .expect("reads terminal event");
                assert_ne!(read, 0, "the terminal request is complete");
                terminal.extend_from_slice(&request[..read]);
                if terminal
                    .windows(b"call.ended".len())
                    .any(|window| window == b"call.ended")
                {
                    break;
                }
                assert!(terminal.len() <= 16 * 1024, "the request stays bounded");
            }
            third
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("acknowledges terminal event");
        });
        (url, held_rx, task)
    }

    async fn holding_initial_webhook()
    -> (String, oneshot::Receiver<()>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let url = format!("http://{}/hook", listener.local_addr().expect("address"));
        let (held, held_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.expect("accepts incoming callback");
            let mut request = [0_u8; 4096];
            let _ = first
                .read(&mut request)
                .await
                .expect("reads incoming event");
            held.send(()).expect("reports held callback");
            let mut drained = [0_u8; 256];
            while first.read(&mut drained).await.expect("waits for timeout") != 0 {}

            let (mut terminal, _) = listener.accept().await.expect("accepts terminal callback");
            let mut body = Vec::new();
            loop {
                let read = terminal
                    .read(&mut request)
                    .await
                    .expect("reads terminal event");
                assert_ne!(read, 0, "the terminal request is complete");
                body.extend_from_slice(&request[..read]);
                if body
                    .windows(b"call.ended".len())
                    .any(|window| window == b"call.ended")
                {
                    break;
                }
                assert!(body.len() <= 16 * 1024, "the request stays bounded");
            }
            terminal
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("acknowledges terminal event");
        });
        (url, held_rx, task)
    }

    async fn answering_until_disconnected_webhook() -> (
        String,
        oneshot::Receiver<()>,
        oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
        let url = format!("http://{}/hook", listener.local_addr().expect("address"));
        let (held, held_rx) = oneshot::channel();
        let (release, released) = oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.expect("accepts incoming callback");
            let mut request = [0_u8; 4096];
            let _ = first
                .read(&mut request)
                .await
                .expect("reads incoming event");
            let body =
                r#"{"contract":"sipx.app.v1","instructions":[{"id":"answer","do":"answer"}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            first
                .write_all(response.as_bytes())
                .await
                .expect("answers with answer instruction");

            let (mut second, _) = listener.accept().await.expect("accepts answered callback");
            let _ = second
                .read(&mut request)
                .await
                .expect("reads answered event");
            held.send(()).expect("reports held callback");
            released.await.expect("the test releases the server");
            let _ = second
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
        });
        (url, held_rx, release, task)
    }

    async fn endpoint() -> (Handle, mpsc::Receiver<Incoming>) {
        bind(Config::new("127.0.0.1:0".parse().expect("address")))
            .await
            .expect("binds")
    }

    fn callee_uri() -> Uri {
        Uri::sip(UriHost::Name(HostName::new("host.example").expect("host")))
    }

    fn invitation(peer: &Handle, call_id: &str, branch: &str) -> Request {
        RequestBuilder::new(Method::Invite, callee_uri())
            .header(
                HeaderName::Via,
                Bytes::from(format!(
                    "SIP/2.0/UDP {};rport;branch={branch}",
                    peer.sent_by_for(TransportKind::Udp)
                )),
            )
            .expect("via")
            .header(HeaderName::To, "<sip:callee@host.example>")
            .expect("to")
            .header(HeaderName::From, "<sip:caller@test.example>;tag=caller")
            .expect("from")
            .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
            .expect("call-id")
            .cseq(1, &Method::Invite)
            .expect("cseq")
            .header(
                HeaderName::Contact,
                format!("<sip:caller@{}>", peer.local_addr()),
            )
            .expect("contact")
            .max_forwards(70)
            .build()
    }

    fn cancel_for(request: &Request) -> Request {
        let copy = |name: &HeaderName| {
            request
                .headers
                .value(name)
                .map(|value| Bytes::from(value.into_owned()))
                .expect("the INVITE carries the header")
        };
        RequestBuilder::new(Method::Cancel, request.uri.clone())
            .header(HeaderName::Via, copy(&HeaderName::Via))
            .expect("via")
            .header(HeaderName::To, copy(&HeaderName::To))
            .expect("to")
            .header(HeaderName::From, copy(&HeaderName::From))
            .expect("from")
            .header(HeaderName::CallId, copy(&HeaderName::CallId))
            .expect("call-id")
            .cseq(1, &Method::Cancel)
            .expect("cseq")
            .max_forwards(70)
            .build()
    }

    async fn final_response(
        peer: &Handle,
        address: std::net::SocketAddr,
        request: Request,
    ) -> Response {
        let mut responses = peer
            .send(request, Target::udp(address))
            .await
            .expect("sends");
        tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("the request is answered")
            .expect("a final response")
    }

    async fn dispatched_invitation(dispatcher: &mut Dispatcher) -> Invitation {
        let next = tokio::time::timeout(Duration::from_secs(5), dispatcher.next())
            .await
            .expect("the dispatcher receives the INVITE")
            .expect("the endpoint is open");
        let Dispatched::Invitation(invitation) = next else {
            panic!("an INVITE is dispatched as an invitation");
        };
        invitation
    }

    #[test]
    fn a_host_reads_its_listener_out_of_the_document() {
        let host = Host::start(DOCUMENT, "127.0.0.1".parse().unwrap()).unwrap();
        let listener = host.call_listener().unwrap();
        assert_eq!(listener.name, "edge");
        assert_eq!(listener.protocol, Protocol::Sip);
    }

    #[test]
    fn a_document_with_no_call_listener_cannot_answer_anything() {
        let sessions = "\
[listener.apps]
protocol = \"session\"
bind = \"127.0.0.1:0\"
";
        let host = Host::start(sessions, "127.0.0.1".parse().unwrap()).unwrap();
        assert!(matches!(
            host.call_listener(),
            Err(HostError::NoCallListener)
        ));
    }

    /// SB-6 coexistence and SB-7 total binding.
    #[test]
    fn sb_6_coexistence_and_sb_7_total_binding() {
        let document = r#"
[listener.web]
protocol = "sip"
transport = "udp"
bind = "127.0.0.1:0"
app = "webhook"

[listener.live]
protocol = "sip"
transport = "udp"
bind = "127.0.0.1:0"
app = "session"

[listener.apps]
protocol = "session"
bind = "127.0.0.1:0"

[app.webhook]
binding = "webhook"
url = "https://apps.example.test/calls"
signing_secrets = ["hook"]

[app.session]
binding = "session"
bearer_secret = "bearer"
"#;
        let mut host =
            Host::start_with_secrets(document, "127.0.0.1".parse().unwrap(), |name| match name {
                "hook" => Some(b"hook material".to_vec()),
                "bearer" => Some(b"session material".to_vec()),
                _ => None,
            })
            .unwrap();
        let connection = host
            .session_endpoint()
            .connect("session", b"session material")
            .unwrap();
        let Admission::App(webhook) = host.running.admit("web-call", "web") else {
            panic!("the webhook listener is total");
        };
        assert!(matches!(webhook.binding, AppBinding::Webhook { .. }));
        let Admission::App(session) = host.running.admit("session-call", "live") else {
            panic!("the session listener is total");
        };
        assert!(matches!(session.binding, AppBinding::Session { .. }));
        let pin = host.sessions().pin("session", "pinned").unwrap();
        assert_eq!(pin.session(), connection.id());
        host.running.end("web-call");
        host.running.end("session-call");
    }

    /// SB-5 through the real call framework rather than only frame validation.
    #[tokio::test]
    async fn sb_5_real_originate_returns_a_pinned_call() {
        let document = r#"
[listener.edge]
protocol = "sip"
transport = "udp"
bind = "127.0.0.1:0"
app = "session"

[listener.apps]
protocol = "session"
bind = "127.0.0.1:0"

[app.session]
binding = "session"
bearer_secret = "bearer"

[app.session.grants]
originate = true
"#;
        let host = Host::start_with_secrets(document, "127.0.0.1".parse().unwrap(), |name| {
            (name == "bearer").then(|| b"session material".to_vec())
        })
        .unwrap();
        let mut connection = host
            .session_endpoint()
            .connect("session", b"session material")
            .unwrap();
        let (origin, origin_incoming) = endpoint().await;
        let origin_dispatcher = Dispatcher::new(origin.clone(), origin_incoming);
        let calls = origin_dispatcher.calls();
        let (callee, callee_incoming) = endpoint().await;
        let target = callee.local_addr();
        let callee_handle = callee.clone();
        let far = tokio::spawn(async move {
            let mut dispatcher = Dispatcher::new(callee_handle.clone(), callee_incoming);
            let invitation = dispatched_invitation(&mut dispatcher).await;
            let mut call = invitation
                .answer(&callee_handle, "127.0.0.1".parse().unwrap())
                .await
                .unwrap();
            let (_, mut inbox) = invitation.into_parts();
            let serving = sipx_call::serve(&mut call, &mut inbox);
            tokio::pin!(serving);
            loop {
                tokio::select! {
                    result = &mut serving => {
                        result.unwrap();
                        break;
                    }
                    event = dispatcher.next() => {
                        assert!(event.is_some(), "the far endpoint remains live");
                    }
                }
            }
        });
        let request = OriginateRequest {
            session: connection.id(),
            app: "session".to_owned(),
            request: "dial-real".to_owned(),
            target: format!("sip:bob@{target}"),
            from: "sip:alerts@example.test".to_owned(),
        };
        let policy = host.running.app_policy("session").unwrap();
        let permit = Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap();
        let (_stop, shutdown) = watch::channel(false);
        let outcome = prepare_originated(
            OriginateContext {
                handle: origin.clone(),
                calls,
                endpoint: host.sessions.clone(),
                media_address: "127.0.0.1".parse().unwrap(),
                policy,
                permit,
            },
            request,
            shutdown,
        )
        .await;
        let OriginateOutcome::Ready(mut prepared) = outcome else {
            panic!("the real outbound call is established");
        };
        assert_eq!(prepared.session.session(), connection.id());
        let reply = connection.recv().await.unwrap();
        assert!(reply.contains(&prepared.call_id));
        prepared.call.hang_up().await.unwrap();
        far.await.unwrap();
        origin.shutdown().await;
        callee.shutdown().await;
    }

    #[test]
    fn a_refused_document_names_its_line_rather_than_panicking() {
        let error = Host::start(
            "[listener.edge]\nprotocol = \"nonsense\"\n",
            "127.0.0.1".parse().unwrap(),
        );
        assert!(matches!(error, Err(HostError::Config(_))));
    }

    /// The knob is load-bearing, and this is what says so.
    ///
    /// The embedded binding has no driver yet, so its admitted calls take `on_unreachable`.
    /// If that value did not come from the document, the operator's declaration would be decoration
    /// and the host would be answering according to a constant compiled into it.
    #[test]
    fn the_document_decides_what_an_unreachable_app_does() {
        let refusing = r#"
[listener.edge]
protocol  = "sip"
transport = "udp"
bind      = "127.0.0.1:0"
app       = "greeter"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"

[app.greeter.on_failure]
on_unreachable = { reject = 503 }
"#;
        let config = HostConfig::parse(refusing).unwrap();
        let mut running = Running::start(config);
        let Admission::App(policy) = running.admit("call-1", "edge") else {
            panic!("the document routes `edge` to an app");
        };
        assert_eq!(
            policy.failure.on_unreachable,
            OnFailure::Reject { status: 503 },
            "the host refuses with the status the document named, not a constant",
        );

        // And the §9.2 default is the other branch, so both are reachable from a document.
        let default = HostConfig::parse(DOCUMENT).unwrap();
        let mut running = Running::start(default);
        let Admission::App(policy) = running.admit("call-2", "edge") else {
            panic!("the document routes `edge` to an app");
        };
        assert_eq!(policy.failure.on_unreachable, OnFailure::Continue);
    }

    /// N11: a live call keeps the policy it was admitted with, so the admission may only be
    /// forgotten once the call is really over.
    ///
    /// This is a regression. The first version of `carry` spawned the call and then ended the
    /// admission immediately, which left `live_calls` reading zero with calls up and `policy_of`
    /// returning `None` for a call that was still running — the one thing `Running` exists to get
    /// right.
    #[test]
    fn an_admission_is_released_only_when_the_call_reports_its_end() {
        let config = HostConfig::parse(DOCUMENT).unwrap();
        let mut running = Running::start(config);
        running.admit("call-1", "edge");
        assert_eq!(running.live_calls(), 1);
        assert!(running.policy_of("call-1").is_some());

        assert_eq!(
            running.live_calls(),
            1,
            "a turn of the loop with no ending must not release a live call",
        );

        running.end("call-1");
        assert_eq!(running.live_calls(), 0);
        assert!(running.policy_of("call-1").is_none());
    }

    #[tokio::test]
    async fn a_rejected_webhook_actor_reports_that_it_ended() {
        let (url, webhook) = rejecting_webhook().await;
        let mut host = Host::start_with_secrets(
            &webhook_document(&url),
            "127.0.0.1".parse().unwrap(),
            |name| (name == "hook").then(|| b"test-secret".to_vec()),
        )
        .expect("the host starts");
        let listener = host.call_listener().expect("a call listener");
        let (callee, incoming) = endpoint().await;
        let address = callee.local_addr();
        let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
        let (peer, _incoming) = endpoint().await;
        let invite = invitation(&peer, "actor-ended@test.example", "z9hG4bK-actor-ended");
        let mut responses = peer
            .send(invite, Target::udp(address))
            .await
            .expect("sends");
        let invitation = dispatched_invitation(&mut dispatcher).await;
        let mut actors = ActorSupervisor::new();

        host.admit(&callee, &listener, invitation, &mut actors)
            .await;
        assert_eq!(host.running.live_calls(), 1, "the actor owns one admission");
        let refusal = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("the actor answers")
            .expect("a final refusal");
        assert_eq!(refusal.status.code(), 488);

        let ended_call = tokio::time::timeout(Duration::from_secs(5), actors.join_next())
            .await
            .expect("the rejected actor finishes")
            .expect("the actor reports its call id");
        assert_eq!(ended_call.call_id(), "actor-ended@test.example");
        host.running.complete(ended_call);
        assert_eq!(
            host.running.live_calls(),
            0,
            "the report releases admission"
        );
        webhook.await.expect("the webhook answered once");
    }

    #[tokio::test]
    async fn a_saturated_actor_set_refuses_admission_with_503() {
        let mut host = Host::start_with_secrets(
            &webhook_document("http://127.0.0.1:9/hook"),
            "127.0.0.1".parse().unwrap(),
            |name| (name == "hook").then(|| b"test-secret".to_vec()),
        )
        .expect("the host starts");
        let listener = host.call_listener().expect("a call listener");
        let (callee, incoming) = endpoint().await;
        let address = callee.local_addr();
        let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
        let (peer, _incoming) = endpoint().await;
        let invite = invitation(&peer, "saturated@test.example", "z9hG4bK-saturated");
        let mut responses = peer
            .send(invite, Target::udp(address))
            .await
            .expect("sends");
        let invitation = dispatched_invitation(&mut dispatcher).await;
        let mut actors = ActorSupervisor::with_limit(0);

        host.admit(&callee, &listener, invitation, &mut actors)
            .await;
        let refusal = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("the saturated host answers")
            .expect("a final refusal");
        assert_eq!(refusal.status.code(), 503);
        assert_eq!(host.running.live_calls(), 0, "admission is rolled back");
        actors.finish(&mut host.running).await;
    }

    #[tokio::test]
    async fn cancel_during_the_initial_callback_releases_the_actor_and_admission() {
        let (url, held, webhook) = holding_initial_webhook().await;
        let mut host = Host::start_with_secrets(
            &webhook_document(&url),
            "127.0.0.1".parse().unwrap(),
            |name| (name == "hook").then(|| b"test-secret".to_vec()),
        )
        .expect("the host starts");
        let listener = host.call_listener().expect("a call listener");
        let (callee, incoming) = endpoint().await;
        let address = callee.local_addr();
        let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
        let (peer, _incoming) = endpoint().await;
        let invite = invitation(
            &peer,
            "initial-cancel@test.example",
            "z9hG4bK-initial-cancel",
        );
        let mut invite_responses = peer
            .send(invite.clone(), Target::udp(address))
            .await
            .expect("sends");
        let invitation = dispatched_invitation(&mut dispatcher).await;
        let mut actors = ActorSupervisor::new();

        host.admit(&callee, &listener, invitation, &mut actors)
            .await;
        tokio::time::timeout(Duration::from_secs(5), held)
            .await
            .expect("the initial callback starts")
            .expect("the webhook reports it");
        let pump = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });

        let cancelled = final_response(&peer, address, cancel_for(&invite)).await;
        assert_eq!(cancelled.status.code(), 200, "CANCEL is acknowledged");
        let invite_final =
            tokio::time::timeout(Duration::from_secs(5), invite_responses.final_response())
                .await
                .expect("the INVITE transaction finishes")
                .expect("the dispatcher sends 487");
        assert_eq!(invite_final.status.code(), 487);

        let ended = tokio::time::timeout(Duration::from_secs(5), actors.join_next())
            .await
            .expect("the cancelled actor finishes")
            .expect("the supervisor reports its call id");
        assert_eq!(ended.call_id(), "initial-cancel@test.example");
        host.running.complete(ended);
        assert_eq!(host.running.live_calls(), 0, "the admission is released");
        webhook.await.expect("the terminal callback is delivered");

        callee.shutdown().await;
        pump.await.expect("the dispatcher stops cooperatively");
        actors.finish(&mut host.running).await;
    }

    #[tokio::test]
    async fn a_late_cancel_cannot_replace_a_webhook_final_refusal() {
        let (url, webhook) = rejecting_webhook().await;
        let mut host = Host::start_with_secrets(
            &webhook_document(&url),
            "127.0.0.1".parse().unwrap(),
            |name| (name == "hook").then(|| b"test-secret".to_vec()),
        )
        .expect("the host starts");
        let listener = host.call_listener().expect("a call listener");
        let (callee, incoming) = endpoint().await;
        let address = callee.local_addr();
        let mut dispatcher = Dispatcher::new(callee.clone(), incoming);
        let (peer, _incoming) = endpoint().await;
        let invite = invitation(&peer, "late-cancel@test.example", "z9hG4bK-late-cancel");
        let mut responses = peer
            .send(invite.clone(), Target::udp(address))
            .await
            .expect("sends");
        let mut invitation = dispatched_invitation(&mut dispatcher).await;
        let mut events = invitation
            .events()
            .expect("an invitation has one event stream");
        let mut actors = ActorSupervisor::new();

        host.admit(&callee, &listener, invitation, &mut actors)
            .await;
        let refusal = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("the actor answers")
            .expect("a final refusal");
        assert_eq!(refusal.status.code(), 488, "the webhook refusal wins");
        let _ = tokio::time::timeout(Duration::from_secs(5), actors.join_next())
            .await
            .expect("the actor finishes")
            .expect("the actor reports its end");

        let pump = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });
        let cancelled = final_response(&peer, address, cancel_for(&invite)).await;
        assert_eq!(
            cancelled.status.code(),
            200,
            "the matching CANCEL is acknowledged even though it is too late"
        );

        // A definition of silence: how long a hole has to be before "no cancellation event"
        // is true. This negative assertion can only become stricter on a loaded machine.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            events.try_recv().is_none(),
            "a late CANCEL must not replace the already-sent 488 with a 487"
        );
        callee.shutdown().await;
        pump.await.expect("the dispatcher stops cooperatively");
        actors.finish(&mut host.running).await;
        webhook.await.expect("the webhook answered once");
    }

    #[tokio::test]
    async fn a_bye_is_answered_while_the_webhook_callback_is_still_in_flight() {
        let (url, held, webhook) = answering_then_holding_webhook().await;
        let (callee, incoming) = endpoint().await;
        let address = callee.local_addr();
        let shutdown = callee.clone();
        let mut host = Host::start_with_secrets(
            &webhook_document(&url),
            "127.0.0.1".parse().unwrap(),
            |name| (name == "hook").then(|| b"test-secret".to_vec()),
        )
        .expect("the host starts");
        let host_task = tokio::spawn(async move { Box::pin(host.serve(callee, incoming)).await });
        let (peer, _incoming) = endpoint().await;
        let mut call = dial(
            &peer,
            Target::udp(address),
            &callee_uri(),
            &DialOptions::new("<sip:caller@test.example>", "127.0.0.1".parse().unwrap()),
        )
        .await
        .expect("the webhook answers the call");
        tokio::time::timeout(Duration::from_secs(5), held)
            .await
            .expect("the answered callback starts")
            .expect("the webhook reports it");

        tokio::time::timeout(Duration::from_secs(1), call.hang_up())
            .await
            .expect("BYE receives its 200 while HTTP remains held")
            .expect("the caller hangs up");
        tokio::time::timeout(Duration::from_secs(5), webhook)
            .await
            .expect("the terminal callback is delivered")
            .expect("the webhook task succeeds");
        shutdown.shutdown().await;
        host_task
            .await
            .expect("the host task does not panic")
            .expect("the host stops cooperatively");
    }

    #[tokio::test]
    async fn dropping_serve_cooperatively_ends_a_live_actor() {
        let (url, held, release_webhook, webhook) = answering_until_disconnected_webhook().await;
        let (callee, incoming) = endpoint().await;
        let address = callee.local_addr();
        let document = webhook_document(&url).replace("timeout_ms = 250", "timeout_ms = 30000");
        let mut host = Host::start_with_secrets(&document, "127.0.0.1".parse().unwrap(), |name| {
            (name == "hook").then(|| b"test-secret".to_vec())
        })
        .expect("the host starts");
        let (peer, mut peer_incoming) = endpoint().await;
        let mut serving = Box::pin(host.serve(callee, incoming));
        let setup = async {
            let call = dial(
                &peer,
                Target::udp(address),
                &callee_uri(),
                &DialOptions::new("<sip:caller@test.example>", "127.0.0.1".parse().unwrap()),
            )
            .await
            .expect("the webhook answers the call");
            tokio::time::timeout(Duration::from_secs(5), held)
                .await
                .expect("the answered callback starts")
                .expect("the webhook reports it");
            call
        };
        tokio::pin!(setup);
        let mut call = tokio::select! {
            result = &mut serving => panic!("the host stopped before cancellation: {result:?}"),
            call = &mut setup => call,
        };
        drop(serving);
        let bye = tokio::time::timeout(Duration::from_secs(20), peer_incoming.recv())
            .await
            .expect("the actor sends BYE during cancellation")
            .expect("the peer endpoint stays open");
        assert_eq!(bye.request.method, Method::Bye);
        call.handle(&bye).await.expect("the peer answers BYE");
        tokio::time::timeout(Duration::from_secs(5), host.running.wait_until_idle())
            .await
            .expect("the actor-owned lease releases admission");
        assert_eq!(host.running.live_calls(), 0);
        let _ = release_webhook.send(());
        webhook.await.expect("the webhook task ends");
    }
}
