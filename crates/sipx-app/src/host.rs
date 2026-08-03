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
//! **What this host deliberately is not.** Document-mode webhooks are real; session and embedded
//! bindings remain later phases. Their absence is not papered over — it is routed through the
//! document's own §9.2 `on_unreachable` declaration. The document actor similarly returns HTTP,
//! timeout and document failures to the contract interpreter, which is the sole component deciding
//! whether the invitation is answered, refused or ended.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use sipx_app_protocol::{
    CallSnapshot, Direction, Effect, EventKind, Input, Interpreter, OnFailure as ContractOnFailure,
    Output, Policy, Source, Timer, Timestamp,
};
use sipx_call::{Call, CallEvent, CallEvents, Dispatched, Dispatcher, Invitation};
use sipx_media::{Interrupt, Playback, PlaybackId};
use sipx_sip::{HeaderName, StatusCode, Uri, build::ResponseBuilder, uri::Host as UriHost};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use sipx_ua::{Config as AgentConfig, UserAgent};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::config::{
    Admission, AppBinding, AppPolicy, ConfigError, Grants, HostConfig, Listener, Protocol, Running,
};
use crate::harness::policy::OnFailure;
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
    /// A named signing secret was absent when the host started.
    MissingSecret(String),
    /// The document declares no `sip` listener, so there is nothing for a call to arrive on. A
    /// document can be valid and still describe a host that cannot answer anything.
    NoCallListener,
}

/// One admitted webhook call. It owns the call, its event receiver and its interpreter; no value
/// is shared between actors, which is the design's one-call/one-program rule in process form.
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
    callback_budget: Duration,
    interpreter: Interpreter,
    webhook: Webhook,
    timers: [Option<Instant>; 3],
    playbacks: BTreeMap<PlaybackId, (String, Playback)>,
    ended: mpsc::Sender<String>,
}

impl DocumentCall {
    fn new(
        handle: Handle,
        mut invitation: Invitation,
        call_id: String,
        media_address: IpAddr,
        policy: AppPolicy,
        webhook: Webhook,
        ended: mpsc::Sender<String>,
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
            callback_budget: policy.failure.timeout,
            interpreter,
            webhook,
            timers: [None; 3],
            playbacks: BTreeMap::new(),
            ended,
        }
    }

    async fn run(mut self) {
        let outputs = self
            .interpreter
            .handle(timestamp(), Input::Event(EventKind::Incoming));
        self.drive(outputs).await;

        if self.call.is_some() {
            self.serve_answered().await;
        } else if self.invitation.is_some() {
            self.wait_for_cancel().await;
        }
        let _ = self.ended.send(self.call_id).await;
    }

    async fn wait_for_cancel(&mut self) {
        let Some(events) = self.invitation_events.as_mut() else {
            return;
        };
        let Some(event) = events.recv().await else {
            return;
        };
        let Some(event) = sipx_app_protocol::event_from_call(&event, "") else {
            return;
        };
        let outputs = self.interpreter.handle(timestamp(), Input::Event(event));
        self.drive(outputs).await;
    }

    async fn serve_answered(&mut self) {
        loop {
            if self.call.as_ref().is_none_or(Call::is_ended) {
                break;
            }
            let contract_timer = self.next_timer();
            let session_deadline = self.call.as_ref().and_then(Call::session_deadline);
            let action = {
                let Some(call) = self.call.as_mut() else {
                    break;
                };
                let Some(events) = self.events.as_mut() else {
                    break;
                };
                let Some(inbox) = self.inbox.as_mut() else {
                    break;
                };
                tokio::select! {
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
                }
            };

            let outputs = match action {
                ActorAction::CallEvent(event) => self.call_event(event),
                ActorAction::Digit(Some((digit, duration_ms))) => self.interpreter.handle(
                    timestamp(),
                    Input::Event(EventKind::Dtmf { digit, duration_ms }),
                ),
                ActorAction::Digit(None) => Vec::new(),
                ActorAction::Incoming(incoming) => {
                    if let Some(call) = self.call.as_mut() {
                        let _ = call.handle(&incoming).await;
                    }
                    Vec::new()
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
                ActorAction::Closed => break,
            };
            self.drive(outputs).await;
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
                    self.interpreter.handle(timestamp(), Input::Event(event))
                })
            }
        }
    }

    async fn drive(&mut self, outputs: Vec<Output>) {
        let mut pending: VecDeque<Output> = outputs.into();
        while let Some(output) = pending.pop_front() {
            let more = match output {
                Output::Deliver { envelope, callback } => {
                    let response = self
                        .webhook
                        .deliver(&envelope, self.callback_budget, unix_seconds())
                        .await;
                    self.interpreter
                        .handle(timestamp(), Input::Response { callback, response })
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
    Closed,
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

/// A host with a configuration in force.
///
/// Constructed from a document rather than from parts, because the document is the unit a reload
/// applies in (N9) and a host assembled field by field could not honour that.
#[derive(Debug)]
pub struct Host {
    running: Running,
    media_address: IpAddr,
    webhooks: BTreeMap<String, Webhook>,
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
        let has_webhooks = config
            .apps()
            .any(|app| matches!(&app.binding, AppBinding::Webhook { .. }));
        let http = if has_webhooks {
            Some(WebhookClient::new()?)
        } else {
            None
        };
        for app in config.apps() {
            let AppBinding::Webhook {
                url,
                signing_secrets,
            } = &app.binding
            else {
                continue;
            };
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
        Ok(Self {
            running: Running::start(config),
            media_address,
            webhooks,
        })
    }

    /// The configuration new calls are admitted under.
    pub fn running(&self) -> &Running {
        &self.running
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

    /// Bind the first `sip` listener and answer calls on it until the endpoint closes.
    ///
    /// # Errors
    /// A document with no call listener, or a listener that could not be bound.
    pub async fn run(&mut self) -> Result<(), HostError> {
        let (handle, incoming) = self.bind_endpoint().await?;
        self.serve(handle, incoming).await
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
        // A call runs on its own task, so the task reports its own end here and the loop forgets it
        // on the next turn. N11 is why this exists rather than the admission simply being dropped:
        // a live call keeps the policy it was admitted with, and `Running` can only honour that
        // while it still knows the call is live. Ending the admission at spawn time — which is what
        // this did first — left `live_calls` reading zero with calls up.
        let (ended, mut endings) = mpsc::channel::<String>(ENDINGS);

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
                        ended_call = endings.recv() => {
                            if let Some(call) = ended_call {
                                self.running.end(&call);
                            }
                        }
                    }
                }
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Dispatched::Invitation(invitation) => {
                    self.admit(&handle, &listener, invitation, &ended).await;
                }
                Dispatched::OutOfDialog(request) => {
                    answer_out_of_dialog(&agent, &handle, &request).await;
                }
                _ => {}
            }
        }
        for call in drain(&mut endings) {
            self.running.end(&call);
        }
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
    async fn admit(
        &mut self,
        handle: &Handle,
        listener: &Listener,
        invitation: Invitation,
        ended: &mpsc::Sender<String>,
    ) {
        let call_id = call_id(invitation.request());
        match self.running.admit(&call_id, &listener.name) {
            Admission::App(policy) => {
                if let AppBinding::Webhook { .. } = &policy.binding
                    && let Some(webhook) = self.webhooks.get(&policy.app).cloned()
                {
                    let actor = DocumentCall::new(
                        handle.clone(),
                        invitation,
                        call_id,
                        self.media_address,
                        *policy,
                        webhook,
                        ended.clone(),
                    );
                    tokio::spawn(actor.run());
                    return;
                }

                // Session and embedded bindings are later phases. Until their drivers exist they
                // are unreachable in the document's own vocabulary.
                match policy.failure.on_unreachable {
                    OnFailure::Reject { status } => {
                        let _ = invitation.refuse(handle, status, "Unavailable").await;
                        self.running.end(&call_id);
                    }
                    OnFailure::Hangup => {
                        self.carry(handle, invitation, call_id, true, ended.clone())
                            .await;
                    }
                    OnFailure::Continue => {
                        self.carry(handle, invitation, call_id, false, ended.clone())
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
    }

    /// Answer an invitation and serve the call to its end on its own task.
    ///
    /// The task reports `call_id` back over `ended` whichever way the call finishes, so the
    /// admission is forgotten exactly once and only after the call is really over.
    async fn carry(
        &self,
        handle: &Handle,
        invitation: Invitation,
        call_id: String,
        hang_up_at_once: bool,
        ended: mpsc::Sender<String>,
    ) {
        // The caller gave up, or the answer could not be sent. Either way there is no call to carry
        // and nothing useful to say to a peer that is already gone — but the admission still has to
        // be released, or a host that is refused often enough leaks one entry per attempt.
        let Ok(mut call) = invitation.answer(handle, self.media_address).await else {
            let _ = ended.send(call_id).await;
            return;
        };
        let (_, mut inbox) = invitation.into_parts();
        tokio::spawn(async move {
            if hang_up_at_once {
                let _ = call.hang_up().await;
            } else {
                // `serve` is the one loop: it honours the RFC 4028 session timer, answers what the
                // call does not claim rather than dropping it, and returns when the call ends.
                let _ = sipx_call::serve(&mut call, &mut inbox).await;
                if !call.is_ended() {
                    let _ = call.hang_up().await;
                }
            }
            let _ = ended.send(call_id).await;
        });
    }
}

/// How many finished calls may be waiting to be forgotten before a task reporting one has to wait.
///
/// Generous rather than tuned: the only cost of a large queue here is memory for a string per call,
/// and the cost of a small one is a finishing call blocking on a loop that is busy answering.
const ENDINGS: usize = 1024;

/// Every call that has reported its end since the last turn of the loop.
///
/// Non-blocking on purpose. The alternative is selecting over this and `Dispatcher::next`, which
/// would make the loop's correctness depend on that future being cancel-safe — a property it does
/// not document, and one a host should not quietly assume.
fn drain(endings: &mut mpsc::Receiver<String>) -> Vec<String> {
    let mut ended = Vec::new();
    while let Ok(call) = endings.try_recv() {
        ended.push(call);
    }
    ended
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
    use sipx_sip::{HostName, Method, Request, Response, build::RequestBuilder};
    use sipx_transport::TransportKind;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

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

        let (ended, mut endings) = mpsc::channel::<String>(ENDINGS);
        assert!(
            drain(&mut endings).is_empty(),
            "nothing has ended, so nothing is forgotten",
        );
        assert_eq!(
            running.live_calls(),
            1,
            "a turn of the loop with no ending must not release a live call",
        );

        ended.try_send("call-1".to_owned()).unwrap();
        for call in drain(&mut endings) {
            running.end(&call);
        }
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
        let (ended, mut endings) = mpsc::channel(1);

        host.admit(&callee, &listener, invitation, &ended).await;
        assert_eq!(host.running.live_calls(), 1, "the actor owns one admission");
        let refusal = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("the actor answers")
            .expect("a final refusal");
        assert_eq!(refusal.status.code(), 488);

        let ended_call = tokio::time::timeout(Duration::from_secs(5), endings.recv())
            .await
            .expect("the rejected actor finishes")
            .expect("the actor reports its call id");
        assert_eq!(ended_call, "actor-ended@test.example");
        host.running.end(&ended_call);
        assert_eq!(
            host.running.live_calls(),
            0,
            "the report releases admission"
        );
        webhook.await.expect("the webhook answered once");
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
        let (ended, mut endings) = mpsc::channel(1);

        host.admit(&callee, &listener, invitation, &ended).await;
        let refusal = tokio::time::timeout(Duration::from_secs(5), responses.final_response())
            .await
            .expect("the actor answers")
            .expect("a final refusal");
        assert_eq!(refusal.status.code(), 488, "the webhook refusal wins");
        let _ = tokio::time::timeout(Duration::from_secs(5), endings.recv())
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
        pump.abort();
        webhook.await.expect("the webhook answered once");
    }
}
