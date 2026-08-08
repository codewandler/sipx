//! Full-duplex application sessions.
//!
//! The registry is the sans-network half of [`session-binding.md`](../../../docs/specs/session-binding.md):
//! bounded sessions and queues, deterministic call pinning, unknown-call races, and atomic death
//! fan-out. The WebSocket driver is a thin owner of these handles.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use sipx_app_protocol::{Document, Envelope, SessionErrorCode, SessionReply, SessionRequest};
use tokio::sync::{mpsc, watch};
use tokio::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tokio_tungstenite::tungstenite::{Bytes as WsBytes, Message, Utf8Bytes};

/// Maximum simultaneous sessions for one configured app.
pub const MAX_SESSIONS_PER_APP: usize = 32;
/// Maximum calls pinned to one session.
pub const MAX_CALLS_PER_SESSION: usize = 256;
/// Host-to-app frames waiting for one session.
pub const OUTBOUND_QUEUE: usize = 64;
/// App-to-host documents waiting for one call actor.
pub const DOCUMENT_QUEUE: usize = 8;
/// Maximum live WebSocket tasks for one listener.
pub const MAX_CONNECTION_TASKS: usize = 128;
/// RFC 6455 Ping cadence.
pub const PING_INTERVAL: Duration = Duration::from_secs(30);
/// How long a Ping may remain unanswered.
pub const PING_GRACE: Duration = Duration::from_secs(10);

/// A stable connection identity, ordered by establishment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SessionId(u64);

/// Why the host ends a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClose {
    /// RFC 6455 close status.
    pub code: u16,
    /// Short operator-readable reason.
    pub reason: &'static str,
}

impl SessionClose {
    const GONE: Self = Self {
        code: 1001,
        reason: "session closed",
    };
    const OVERFLOW: Self = Self {
        code: 1013,
        reason: "outbound queue full",
    };
}

/// Why a session could not be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConnectError {
    /// The app is unknown or the bearer did not match. Deliberately one diagnosis.
    Unauthorized,
    /// This app already has the declared maximum number of sessions.
    TooManySessions,
}

/// Why a call could not be pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PinError {
    /// No live session has capacity for this app.
    Unreachable,
    /// The call id is already pinned.
    DuplicateCall,
}

/// The result of routing one app document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDocument {
    /// The call actor owns the whole document.
    Accepted,
    /// The call is absent, ended, or belongs to another session.
    UnknownCall,
    /// The call actor is not consuming its bounded queue.
    CallBusy,
}

/// The result of queuing one host event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeliverError {
    /// The call is no longer pinned.
    UnknownCall,
    /// Queue overflow atomically killed the session with close 1013.
    SessionOverflow,
}

/// One configured session app's upgrade credential and originate grant.
pub struct SessionApp {
    secret: Vec<u8>,
    originate: bool,
}

impl fmt::Debug for SessionApp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionApp")
            .field("secret", &"[redacted]")
            .field("originate", &self.originate)
            .finish()
    }
}

impl SessionApp {
    /// A configured app. Secret material is retained only for constant-time upgrade checks.
    #[must_use]
    pub fn new(secret: Vec<u8>, originate: bool) -> Self {
        Self { secret, originate }
    }
}

/// A validated originate request ready for the call framework.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginateRequest {
    /// The requesting session; success stays pinned here.
    pub session: SessionId,
    /// Configured app name.
    pub app: String,
    /// App correlation.
    pub request: String,
    /// Destination SIP URI.
    pub target: String,
    /// Caller SIP URI.
    pub from: String,
}

/// What a parsed text frame asks the driver to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAction {
    /// Send this reply immediately.
    Reply(SessionReply),
    /// Ask the call framework to originate; it later calls [`SessionEndpoint::originated`].
    Originate(OriginateRequest),
}

/// Authenticated frame handling over one [`SessionHub`].
#[derive(Debug, Clone)]
pub struct SessionEndpoint {
    hub: SessionHub,
    apps: Arc<BTreeMap<String, SessionApp>>,
}

impl SessionEndpoint {
    /// One endpoint for the configured session apps.
    #[must_use]
    pub fn new(hub: SessionHub, apps: BTreeMap<String, SessionApp>) -> Self {
        Self {
            hub,
            apps: Arc::new(apps),
        }
    }

    /// Authenticate and establish one app session.
    pub fn connect(&self, app: &str, bearer: &[u8]) -> Result<SessionConnection, ConnectError> {
        self.apps
            .get(app)
            .filter(|configured| secret_matches(&configured.secret, bearer))
            .ok_or(ConnectError::Unauthorized)?;
        self.hub.connect(app)
    }

    /// Parse and route one UTF-8 text frame.
    #[must_use]
    pub fn handle_text(&self, session: SessionId, app: &str, text: &str) -> SessionAction {
        if !self.hub.belongs_to(session, app) {
            return SessionAction::Reply(SessionReply::error(
                None::<String>,
                SessionErrorCode::BadFrame,
                "the session does not belong to this app",
            ));
        }
        let Ok(request) = SessionRequest::parse(text) else {
            return SessionAction::Reply(SessionReply::error(
                SessionRequest::correlation_from_text(text),
                SessionErrorCode::BadFrame,
                "the text frame is not a valid session command",
            ));
        };
        match request {
            SessionRequest::Document {
                request,
                call,
                document,
            } => {
                let (code, message) = match self.hub.route_document(session, &call, document) {
                    RouteDocument::Accepted => {
                        return SessionAction::Reply(SessionReply::result(request, call));
                    }
                    RouteDocument::UnknownCall => (
                        SessionErrorCode::UnknownCall,
                        "the call is not live on this session",
                    ),
                    RouteDocument::CallBusy => (
                        SessionErrorCode::CallBusy,
                        "the call document queue is full",
                    ),
                };
                SessionAction::Reply(SessionReply::error(Some(request), code, message))
            }
            SessionRequest::Originate {
                request,
                target,
                from,
            } => {
                if !self
                    .apps
                    .get(app)
                    .is_some_and(|configured| configured.originate)
                {
                    return SessionAction::Reply(SessionReply::error(
                        Some(request),
                        SessionErrorCode::OriginateForbidden,
                        "this app is not granted originate",
                    ));
                }
                SessionAction::Originate(OriginateRequest {
                    session,
                    app: app.to_owned(),
                    request,
                    target,
                    from,
                })
            }
            _ => SessionAction::Reply(SessionReply::error(
                None::<String>,
                SessionErrorCode::BadFrame,
                "the session command is not supported by this host",
            )),
        }
    }

    /// Complete a successful originate by pinning the new call to its requesting session.
    pub fn originated(
        &self,
        request: &OriginateRequest,
        call: impl Into<String>,
    ) -> Result<(SessionCall, SessionReply), PinError> {
        let call = call.into();
        let pin = self.hub.pin_to(request.session, call.clone())?;
        Ok((pin, SessionReply::result(&request.request, call)))
    }

    /// A failed originate reply; the session remains live.
    #[must_use]
    pub fn originate_failed(request: &OriginateRequest) -> SessionReply {
        SessionReply::error(
            Some(&request.request),
            SessionErrorCode::OriginateFailed,
            "the outbound call could not be placed",
        )
    }

    /// The underlying pin registry.
    #[must_use]
    pub fn hub(&self) -> &SessionHub {
        &self.hub
    }

    /// Queue a correlated control reply on the session's bounded outbound queue.
    pub fn send_reply(&self, session: SessionId, reply: &SessionReply) -> Result<(), DeliverError> {
        self.hub.send_text(session, reply.to_text())
    }
}

fn secret_matches(expected: &[u8], presented: &[u8]) -> bool {
    let Ok(mut expected_mac) = Hmac::<Sha256>::new_from_slice(b"sipx.app.v1 session bearer") else {
        return false;
    };
    expected_mac.update(expected);
    let expected = expected_mac.finalize().into_bytes();
    let Ok(mut presented_mac) = Hmac::<Sha256>::new_from_slice(b"sipx.app.v1 session bearer")
    else {
        return false;
    };
    presented_mac.update(presented);
    presented_mac.verify_slice(&expected).is_ok()
}

/// A cloneable registry shared by the host, connection tasks, and call actors.
#[derive(Debug, Clone)]
pub struct SessionHub {
    inner: Arc<Mutex<State>>,
    limits: Limits,
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    sessions: usize,
    calls: usize,
    outbound: usize,
    documents: usize,
}

#[derive(Debug, Default)]
struct State {
    next_session: u64,
    sessions: BTreeMap<SessionId, Entry>,
    calls: BTreeMap<String, CallEntry>,
}

#[derive(Debug)]
struct Entry {
    app: String,
    calls: BTreeSet<String>,
    outbound: mpsc::Sender<String>,
    closed: watch::Sender<Option<SessionClose>>,
}

#[derive(Debug)]
struct CallEntry {
    session: SessionId,
    documents: mpsc::Sender<Document>,
}

impl Default for SessionHub {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionHub {
    /// An empty registry with the normative bounds.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(State::default())),
            limits: Limits {
                sessions: MAX_SESSIONS_PER_APP,
                calls: MAX_CALLS_PER_SESSION,
                outbound: OUTBOUND_QUEUE,
                documents: DOCUMENT_QUEUE,
            },
        }
    }

    #[cfg(test)]
    fn with_limits(sessions: usize, calls: usize, outbound: usize, documents: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State::default())),
            limits: Limits {
                sessions,
                calls,
                outbound,
                documents,
            },
        }
    }

    /// Establish one authenticated app connection.
    pub fn connect(&self, app: impl Into<String>) -> Result<SessionConnection, ConnectError> {
        let app = app.into();
        let mut state = self.lock();
        let active = state
            .sessions
            .values()
            .filter(|session| session.app == app)
            .count();
        if active >= self.limits.sessions {
            return Err(ConnectError::TooManySessions);
        }
        state.next_session = state.next_session.saturating_add(1);
        let id = SessionId(state.next_session);
        let (outbound, outbound_rx) = mpsc::channel(self.limits.outbound.max(1));
        let (closed, closed_rx) = watch::channel(None);
        state.sessions.insert(
            id,
            Entry {
                app,
                calls: BTreeSet::new(),
                outbound,
                closed,
            },
        );
        Ok(SessionConnection {
            id,
            outbound: outbound_rx,
            closed: closed_rx,
            hub: self.clone(),
        })
    }

    /// Pin a new call by least load, then oldest session.
    pub fn pin(&self, app: &str, call: impl Into<String>) -> Result<SessionCall, PinError> {
        let call = call.into();
        let chosen = {
            let state = self.lock();
            if state.calls.contains_key(&call) {
                return Err(PinError::DuplicateCall);
            }
            state
                .sessions
                .iter()
                .filter(|(_, session)| {
                    session.app == app && session.calls.len() < self.limits.calls
                })
                .min_by_key(|(id, session)| (session.calls.len(), **id))
                .map(|(id, _)| *id)
                .ok_or(PinError::Unreachable)?
        };
        self.pin_to(chosen, call)
    }

    /// Pin an originated call to the session that requested it.
    pub fn pin_to(
        &self,
        session: SessionId,
        call: impl Into<String>,
    ) -> Result<SessionCall, PinError> {
        let call = call.into();
        let mut state = self.lock();
        if state.calls.contains_key(&call) {
            return Err(PinError::DuplicateCall);
        }
        let Some(entry) = state.sessions.get_mut(&session) else {
            return Err(PinError::Unreachable);
        };
        if entry.calls.len() >= self.limits.calls {
            return Err(PinError::Unreachable);
        }
        let closed = entry.closed.subscribe();
        let (documents, documents_rx) = mpsc::channel(self.limits.documents.max(1));
        entry.calls.insert(call.clone());
        state
            .calls
            .insert(call.clone(), CallEntry { session, documents });
        Ok(SessionCall {
            id: call,
            session,
            documents: documents_rx,
            closed,
            hub: self.clone(),
        })
    }

    /// Route a document only when the call is live on the sending session.
    pub fn route_document(
        &self,
        session: SessionId,
        call: &str,
        document: Document,
    ) -> RouteDocument {
        let state = self.lock();
        let Some(entry) = state.calls.get(call) else {
            return RouteDocument::UnknownCall;
        };
        if entry.session != session {
            return RouteDocument::UnknownCall;
        }
        match entry.documents.try_send(document) {
            Ok(()) => RouteDocument::Accepted,
            Err(mpsc::error::TrySendError::Full(_)) => RouteDocument::CallBusy,
            Err(mpsc::error::TrySendError::Closed(_)) => RouteDocument::UnknownCall,
        }
    }

    /// Queue one ordinary contract envelope for its pinned session.
    pub fn deliver(&self, call: &str, envelope: &Envelope) -> Result<(), DeliverError> {
        let result = {
            let state = self.lock();
            let session = state
                .calls
                .get(call)
                .map(|entry| entry.session)
                .ok_or(DeliverError::UnknownCall)?;
            let sender = state
                .sessions
                .get(&session)
                .map(|entry| entry.outbound.clone())
                .ok_or(DeliverError::UnknownCall)?;
            (session, sender.try_send(envelope.to_text()))
        };
        match result {
            (_, Ok(())) => Ok(()),
            (session, Err(mpsc::error::TrySendError::Full(_))) => {
                self.kill(session, SessionClose::OVERFLOW);
                Err(DeliverError::SessionOverflow)
            }
            (_, Err(mpsc::error::TrySendError::Closed(_))) => Err(DeliverError::UnknownCall),
        }
    }

    fn send_text(&self, session: SessionId, text: String) -> Result<(), DeliverError> {
        let result = self
            .lock()
            .sessions
            .get(&session)
            .map(|entry| entry.outbound.clone())
            .ok_or(DeliverError::UnknownCall)?
            .try_send(text);
        match result {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.kill(session, SessionClose::OVERFLOW);
                Err(DeliverError::SessionOverflow)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(DeliverError::UnknownCall),
        }
    }

    fn end_call(&self, call: &str, session: SessionId) {
        let mut state = self.lock();
        if state
            .calls
            .get(call)
            .is_some_and(|entry| entry.session == session)
        {
            state.calls.remove(call);
            if let Some(entry) = state.sessions.get_mut(&session) {
                entry.calls.remove(call);
            }
        }
    }

    fn belongs_to(&self, session: SessionId, app: &str) -> bool {
        self.lock()
            .sessions
            .get(&session)
            .is_some_and(|entry| entry.app == app)
    }

    fn kill(&self, session: SessionId, close: SessionClose) {
        let mut state = self.lock();
        let Some(entry) = state.sessions.remove(&session) else {
            return;
        };
        let calls: Vec<String> = entry.calls.into_iter().collect();
        let _ = entry.closed.send(Some(close));
        for call in calls {
            state.calls.remove(&call);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The WebSocket task's side of one established session.
#[derive(Debug)]
pub struct SessionConnection {
    id: SessionId,
    outbound: mpsc::Receiver<String>,
    closed: watch::Receiver<Option<SessionClose>>,
    hub: SessionHub,
}

#[derive(Debug)]
enum ConnectionInput {
    Text(String),
    Closed(SessionClose),
}

impl SessionConnection {
    /// Stable id used for document routing and originate pinning.
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Receive the next host-to-app text frame.
    pub async fn recv(&mut self) -> Option<String> {
        self.outbound.recv().await
    }

    /// Observe a host-initiated close without competing with the full data queue.
    pub async fn closed(&mut self) -> Option<SessionClose> {
        if self.closed.borrow().is_some() {
            return self.closed.borrow().clone();
        }
        self.closed.changed().await.ok()?;
        self.closed.borrow().clone()
    }

    async fn next(&mut self) -> ConnectionInput {
        let outbound = &mut self.outbound;
        let closed = &mut self.closed;
        tokio::select! {
            text = outbound.recv() => text.map_or(
                ConnectionInput::Closed(SessionClose::GONE),
                ConnectionInput::Text,
            ),
            changed = closed.changed() => {
                let _ = changed;
                ConnectionInput::Closed(
                    closed.borrow().clone().unwrap_or(SessionClose::GONE),
                )
            }
        }
    }

    /// End this session and fan death out to every pin.
    pub fn disconnect(&self) {
        self.hub.kill(self.id, SessionClose::GONE);
    }
}

impl Drop for SessionConnection {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// One call actor's lifetime pin.
#[derive(Debug)]
pub struct SessionCall {
    id: String,
    session: SessionId,
    documents: mpsc::Receiver<Document>,
    closed: watch::Receiver<Option<SessionClose>>,
    hub: SessionHub,
}

/// The next binding-side input for one call actor.
#[derive(Debug)]
pub(crate) enum CallInput {
    Document(Document),
    Dead,
}

impl SessionCall {
    /// The call id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The one session that owns this call.
    #[must_use]
    pub fn session(&self) -> SessionId {
        self.session
    }

    /// Receive the next whole replacement document.
    pub async fn recv_document(&mut self) -> Option<Document> {
        self.documents.recv().await
    }

    pub(crate) async fn next_input(&mut self) -> CallInput {
        let documents = &mut self.documents;
        let closed = &mut self.closed;
        tokio::select! {
            document = documents.recv() => document.map_or(CallInput::Dead, CallInput::Document),
            changed = closed.changed() => {
                let _ = changed;
                CallInput::Dead
            },
        }
    }

    /// Wait until the pinned session dies.
    pub async fn session_closed(&mut self) -> Option<SessionClose> {
        if self.closed.borrow().is_some() {
            return self.closed.borrow().clone();
        }
        self.closed.changed().await.ok()?;
        self.closed.borrow().clone()
    }

    /// Deliver one event without waiting for queue room.
    pub fn deliver(&self, envelope: &Envelope) -> Result<(), DeliverError> {
        self.hub.deliver(&self.id, envelope)
    }
}

impl Drop for SessionCall {
    fn drop(&mut self) {
        self.hub.end_call(&self.id, self.session);
    }
}

/// A WebSocket session failed before or during its bounded serving loop.
#[derive(Debug)]
pub struct SessionSocketError(String);

impl fmt::Display for SessionSocketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionSocketError {}

/// Authenticate and serve one WebSocket connection.
///
/// Originate requests are admitted to `originates` without waiting; the host processes them on an
/// owned bounded task and returns the eventual reply through [`SessionEndpoint::send_reply`].
#[allow(clippy::result_large_err)]
pub async fn accept_websocket<S>(
    stream: S,
    endpoint: SessionEndpoint,
    originates: mpsc::Sender<OriginateRequest>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), SessionSocketError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    type Upgrade = Option<(String, SessionConnection)>;
    let upgraded: Arc<Mutex<Upgrade>> = Arc::new(Mutex::new(None));
    let captured = Arc::clone(&upgraded);
    let auth = endpoint.clone();
    let socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
        authenticate_upgrade(request, response, &auth, &captured)
    })
    .await
    .map_err(|error| SessionSocketError(error.to_string()))?;
    let (app, mut connection) = upgraded
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .ok_or_else(|| SessionSocketError("upgrade completed without an app".to_owned()))?;
    let session = connection.id();
    let (mut sink, mut source) = socket.split();
    let timer = tokio::time::sleep(PING_INTERVAL); // failure bound: when the next liveness probe must be sent
    tokio::pin!(timer);
    let mut waiting_for_pong = false;

    loop {
        tokio::select! {
            changed = shutdown.changed() => {
                let _ = changed;
                let _ = sink.send(close_message(1001, "host stopping")).await;
                break;
            }
            frame = source.next() => {
                match frame {
                    Some(Ok(Message::Text(text))) => {
                        match endpoint.handle_text(session, &app, &text) {
                            SessionAction::Reply(reply) => {
                                if sink.send(Message::Text(reply.to_text().into())).await.is_err() {
                                    break;
                                }
                            }
                            SessionAction::Originate(request) => {
                                if originates.try_send(request.clone()).is_err() {
                                    let reply = SessionEndpoint::originate_failed(&request);
                                    if sink.send(Message::Text(reply.to_text().into())).await.is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        waiting_for_pong = false;
                        timer.as_mut().reset(Instant::now() + PING_INTERVAL);
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if sink.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let _ = sink.send(close_message(1003, "binary frames are reserved")).await;
                        break;
                    }
                    Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                    Some(Ok(Message::Frame(_))) => {}
                }
            }
            output = connection.next() => {
                match output {
                    ConnectionInput::Text(text) => {
                        if sink.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    ConnectionInput::Closed(close) => {
                        let _ = sink.send(close_message(close.code, close.reason)).await;
                        break;
                    }
                }
            }
            () = &mut timer => {
                if waiting_for_pong {
                    let _ = sink.send(close_message(1001, "ping grace expired")).await;
                    break;
                }
                if sink.send(Message::Ping(WsBytes::new())).await.is_err() {
                    break;
                }
                waiting_for_pong = true;
                timer.as_mut().reset(Instant::now() + PING_GRACE);
            }
        }
    }
    connection.disconnect();
    Ok(())
}

#[allow(clippy::result_large_err)]
fn authenticate_upgrade(
    request: &Request,
    response: Response,
    endpoint: &SessionEndpoint,
    captured: &Arc<Mutex<Option<(String, SessionConnection)>>>,
) -> Result<Response, ErrorResponse> {
    let app = request
        .uri()
        .path()
        .strip_prefix("/v1/apps/")
        .filter(|app| !app.is_empty());
    let bearer = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let Some((app, bearer)) = app.zip(bearer) else {
        return Err(refusal(
            StatusCode::UNAUTHORIZED,
            "bearer authentication required",
        ));
    };
    let connection = endpoint
        .connect(app, bearer.as_bytes())
        .map_err(|error| match error {
            ConnectError::Unauthorized => {
                refusal(StatusCode::UNAUTHORIZED, "bearer authentication failed")
            }
            ConnectError::TooManySessions => {
                refusal(StatusCode::SERVICE_UNAVAILABLE, "session limit reached")
            }
        })?;
    *captured.lock().unwrap_or_else(PoisonError::into_inner) = Some((app.to_owned(), connection));
    Ok(response)
}

fn refusal(status: StatusCode, message: &str) -> ErrorResponse {
    let mut response = ErrorResponse::new(Some(message.to_owned()));
    *response.status_mut() = status;
    response
}

fn close_message(code: u16, reason: &'static str) -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::from(code),
        reason: Utf8Bytes::from_static(reason),
    }))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
pub mod vectors {
    //! Normative `SB-*` vectors from the session-binding specification.
    use sipx_app_protocol::{
        CallSnapshot, Direction, Failure, Input, Interpreter, OnFailure, Output, Policy, Timestamp,
    };

    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::client_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;

    fn envelope(call: &str, seq: u64) -> Envelope {
        Envelope {
            seq,
            at: Timestamp::from_unix_millis(0),
            call: CallSnapshot::new(call, Direction::Inbound),
            event: sipx_app_protocol::EventKind::Incoming,
        }
    }

    /// SB-1 pinning.
    #[test]
    fn sb_1_pinning() {
        let hub = SessionHub::new();
        let first = hub.connect("voice").unwrap();
        let second = hub.connect("voice").unwrap();
        let one = hub.pin("voice", "one").unwrap();
        let two = hub.pin("voice", "two").unwrap();
        let three = hub.pin("voice", "three").unwrap();
        assert_eq!(one.session(), first.id());
        assert_eq!(two.session(), second.id());
        assert_eq!(three.session(), first.id());
        drop(second);
        assert_eq!(one.session(), first.id(), "a live pin never migrates");
    }

    /// SB-2 dead fan-out.
    #[tokio::test]
    async fn sb_2_dead_fan_out_uses_each_calls_policy() {
        let hub = SessionHub::new();
        let connection = hub.connect("voice").unwrap();
        let mut rejected = hub.pin("voice", "rejected").unwrap();
        let mut continued = hub.pin("voice", "continued").unwrap();
        connection.disconnect();
        assert_eq!(rejected.session_closed().await.unwrap().code, 1001);
        assert_eq!(continued.session_closed().await.unwrap().code, 1001);

        let mut rejecting = Interpreter::new(
            CallSnapshot::new("rejected", Direction::Inbound),
            Policy {
                on_unreachable: OnFailure::Reject { status: 480 },
                ..Policy::default()
            },
        );
        let mut continuing = Interpreter::new(
            CallSnapshot::new("continued", Direction::Inbound),
            Policy {
                on_unreachable: OnFailure::Continue,
                ..Policy::default()
            },
        );
        assert!(matches!(
            rejecting
                .handle(
                    Timestamp::from_unix_millis(0),
                    Input::BindingFailed(Failure::Unreachable)
                )
                .first(),
            Some(Output::Effect(sipx_app_protocol::Effect::Reject {
                status: 480,
                ..
            }))
        ));
        assert!(
            continuing
                .handle(
                    Timestamp::from_unix_millis(0),
                    Input::BindingFailed(Failure::Unreachable)
                )
                .is_empty()
        );
    }

    /// SB-3 overflow.
    #[tokio::test]
    async fn sb_3_overflow_closes_1013() {
        let hub = SessionHub::with_limits(1, 1, 2, 1);
        let mut connection = hub.connect("voice").unwrap();
        let call = hub.pin("voice", "call").unwrap();
        assert_eq!(call.deliver(&envelope("call", 1)), Ok(()));
        assert_eq!(call.deliver(&envelope("call", 2)), Ok(()));
        assert_eq!(
            call.deliver(&envelope("call", 3)),
            Err(DeliverError::SessionOverflow)
        );
        assert_eq!(connection.closed().await.unwrap().code, 1013);
    }

    /// SB-4 unknown-call race.
    #[test]
    fn sb_4_unknown_call_race_is_typed_and_ignored() {
        let hub = SessionHub::new();
        let connection = hub.connect("voice").unwrap();
        let call = hub.pin("voice", "ended").unwrap();
        drop(call);
        assert_eq!(
            hub.route_document(connection.id(), "ended", Document::keep_going()),
            RouteDocument::UnknownCall
        );
    }

    /// SB-5 originate.
    #[test]
    fn sb_5_originate_is_denied_by_default_and_success_introduces_a_pin() {
        let hub = SessionHub::new();
        let endpoint = SessionEndpoint::new(
            hub,
            BTreeMap::from([
                ("denied".to_owned(), SessionApp::new(b"one".to_vec(), false)),
                ("granted".to_owned(), SessionApp::new(b"two".to_vec(), true)),
            ]),
        );
        assert!(matches!(
            endpoint.connect("granted", b"wrong"),
            Err(ConnectError::Unauthorized)
        ));
        let denied = endpoint.connect("denied", b"one").unwrap();
        let granted = endpoint.connect("granted", b"two").unwrap();
        let frame = r#"{"contract":"sipx.app.v1","request":"dial-1","do":"originate","target":"sip:bob@example.net","from":"sip:alerts@example.com"}"#;
        let SessionAction::Reply(forbidden) = endpoint.handle_text(denied.id(), "denied", frame)
        else {
            panic!("deny-by-default must reply locally");
        };
        assert!(forbidden.to_text().contains("originate_forbidden"));

        let SessionAction::Originate(request) =
            endpoint.handle_text(granted.id(), "granted", frame)
        else {
            panic!("the granted request reaches the call framework");
        };
        let (pin, reply) = endpoint.originated(&request, "outbound-1").unwrap();
        assert_eq!(pin.session(), granted.id());
        assert_eq!(
            reply.to_text(),
            r#"{"contract":"sipx.app.v1","request":"dial-1","result":{"call":"outbound-1"}}"#
        );
    }

    /// SB-4 on the actual WebSocket framing boundary.
    #[tokio::test]
    async fn websocket_authenticates_and_returns_typed_unknown_call() {
        let endpoint = SessionEndpoint::new(
            SessionHub::new(),
            BTreeMap::from([(
                "voice".to_owned(),
                SessionApp::new(b"secret".to_vec(), false),
            )]),
        );
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (originates, _requests) = mpsc::channel(1);
        let (_stop, shutdown) = watch::channel(false);
        let serving = tokio::spawn(accept_websocket(server, endpoint, originates, shutdown));
        let mut request = "ws://localhost/v1/apps/voice"
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", HeaderValue::from_static("Bearer secret"));
        let (mut socket, _) = client_async(request, client).await.unwrap();
        socket
            .send(Message::Text(
                r#"{"contract":"sipx.app.v1","request":"late","call":"ended","instructions":[]}"#
                    .into(),
            ))
            .await
            .unwrap();
        let Message::Text(reply) = socket.next().await.unwrap().unwrap() else {
            panic!("the host returns a text error");
        };
        assert!(reply.contains("unknown_call"));
        socket.close(None).await.unwrap();
        serving.await.unwrap().unwrap();
    }

    /// Binary is reserved at the actual WebSocket boundary.
    #[tokio::test]
    async fn websocket_binary_closes_1003() {
        let endpoint = SessionEndpoint::new(
            SessionHub::new(),
            BTreeMap::from([(
                "voice".to_owned(),
                SessionApp::new(b"secret".to_vec(), false),
            )]),
        );
        let (client, server) = tokio::io::duplex(16 * 1024);
        let (originates, _requests) = mpsc::channel(1);
        let (_stop, shutdown) = watch::channel(false);
        let serving = tokio::spawn(accept_websocket(server, endpoint, originates, shutdown));
        let mut request = "ws://localhost/v1/apps/voice"
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", HeaderValue::from_static("Bearer secret"));
        let (mut socket, _) = client_async(request, client).await.unwrap();
        socket.send(Message::Binary(vec![1].into())).await.unwrap();
        let Message::Close(Some(close)) = socket.next().await.unwrap().unwrap() else {
            panic!("the host closes explicitly");
        };
        assert_eq!(close.code, CloseCode::Unsupported);
        serving.await.unwrap().unwrap();
    }
}
