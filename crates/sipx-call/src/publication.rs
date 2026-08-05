//! Live endpoint driver for RFC 3903 event-state publication.
//!
//! The protocol contract is `docs/specs/publication-endpoint.md`. The inbound role mutates the
//! exact injected [`Compositor`]; the outbound role drives [`Publisher`] through real transactions.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::headers::{CSeq, Expires};
use sipx_sip::{HeaderName, Method, Request, Response, StatusCode};
use sipx_transport::{Handle, Incoming, Target, TransportKind};
use sipx_ua::auth::new_cnonce;
use sipx_ua::presence::{Compositor, PIDF_TYPE, Publish, Published};
use sipx_ua::publication_client::{
    CommandError, Config as PublisherConfig, Output, Publisher, Start, StartError, StateChange,
    Timer,
};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::dispatch::with_to_tag;

const DRIVER_QUEUE: usize = 32;
const RETRY_AFTER: Duration = Duration::from_secs(1);

/// Inbound publication admission and expiry policy.
#[derive(Debug, Clone)]
pub struct PublicationConfig {
    /// Minimum positive expiry; smaller requests receive 423.
    pub minimum_expiry: Duration,
    /// Expiry used when the request omits Expires.
    pub default_expiry: Duration,
    /// Maximum active resources held by the endpoint driver.
    pub capacity: usize,
    /// Maximum accepted PUBLISH body.
    pub body_limit: usize,
    /// Outbound publisher policy.
    pub publisher: PublisherConfig,
}

impl Default for PublicationConfig {
    fn default() -> Self {
        Self {
            minimum_expiry: Duration::from_secs(60),
            default_expiry: Duration::from_secs(3_600),
            capacity: 1_024,
            body_limit: 65_536,
            publisher: PublisherConfig::default(),
        }
    }
}

impl PublicationConfig {
    fn validate(&self) -> Result<(), PublicationError> {
        self.publisher.validate()?;
        if self.minimum_expiry.is_zero()
            || self.default_expiry < self.minimum_expiry
            || self.default_expiry.as_secs() > u64::from(u32::MAX)
            || self.capacity == 0
            || self.body_limit == 0
        {
            return Err(PublicationError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Inbound authorization is an explicit application decision.
pub trait PublicationAuthorization: Send + Sync + 'static {
    /// Whether this source may mutate this resource and event package.
    fn authorize(&self, request: &Request, source: SocketAddr, transport: TransportKind) -> bool;
}

/// Application-owned composition decision around the exact publication store.
pub trait PublicationComposition: Send + Sync + 'static {
    /// Apply one authorized operation, optionally composing it with existing state.
    fn apply(
        &self,
        compositor: &mut Compositor,
        entity: &str,
        publication: Publish,
        now: u64,
    ) -> Published;
}

/// Explicit policy that uses the compositor's one-document-per-resource semantics.
#[derive(Debug, Default)]
pub struct ReplacePublicationState;

impl PublicationComposition for ReplacePublicationState {
    fn apply(
        &self,
        compositor: &mut Compositor,
        entity: &str,
        publication: Publish,
        now: u64,
    ) -> Published {
        compositor.apply(entity, publication, now)
    }
}

/// Explicit permissive policy for development and already-authenticated frontends.
#[derive(Debug, Default)]
pub struct AllowPublications;

impl PublicationAuthorization for AllowPublications {
    fn authorize(
        &self,
        _request: &Request,
        _source: SocketAddr,
        _transport: TransportKind,
    ) -> bool {
        true
    }
}

/// Runtime resource snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationCounts {
    /// Live publisher and inbound expiry tasks.
    pub active_tasks: usize,
    /// Live timer tasks.
    pub active_timers: usize,
    /// Live outbound client transactions.
    pub active_transactions: usize,
    /// Live outbound publisher handles.
    pub active_publishers: usize,
    /// Live inbound compositor publications.
    pub active_publications: usize,
    /// Inbound or outbound capacity refusals.
    pub shed: u64,
}

#[derive(Debug, Default)]
struct Counters {
    tasks: AtomicUsize,
    timers: AtomicUsize,
    transactions: AtomicUsize,
    publishers: AtomicUsize,
    shed: AtomicU64,
}

/// Failure before a runtime operation is admitted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PublicationError {
    /// The publisher driver is not attached to a dispatcher.
    #[error("publications are not attached to a dispatcher")]
    NotAttached,
    /// Dispatcher shutdown has atomically closed admission.
    #[error("publication shutdown has closed admission")]
    ShuttingDown,
    /// Runtime configuration is invalid.
    #[error("invalid publication configuration")]
    InvalidConfiguration,
    /// The configured publisher capacity is full.
    #[error("publisher capacity exceeded")]
    CapacityExceeded,
    /// Another local publisher already serializes this resource.
    #[error("a publisher already owns this resource")]
    DuplicateResource,
    /// The sans-I/O client refused a lifecycle command.
    #[error(transparent)]
    Command(#[from] CommandError),
    /// The sans-I/O client rejected the initial fields.
    #[error(transparent)]
    Start(#[from] StartError),
}

#[derive(Debug)]
struct Shared {
    endpoint: Mutex<Option<Handle>>,
    resources: Mutex<HashSet<Vec<u8>>>,
    drivers: Mutex<HashMap<Vec<u8>, JoinHandle<()>>>,
    config: PublisherConfig,
    counters: Arc<Counters>,
    shutdown: CancellationToken,
}

/// Application ownership of one outbound publication.
#[derive(Debug)]
pub struct Publication {
    commands: mpsc::Sender<Command>,
    states: watch::Receiver<Option<StateChange>>,
    body_limit: usize,
}

impl Publication {
    /// Wait for the next authoritative or terminal state.
    pub async fn next_state(&mut self) -> Option<StateChange> {
        self.states.changed().await.ok()?;
        self.states.borrow_and_update().clone()
    }

    /// Conditionally replace the publication body.
    pub async fn modify(
        &self,
        body: Bytes,
        content_type: impl Into<String>,
    ) -> Result<(), PublicationError> {
        if body.is_empty() || body.len() > self.body_limit {
            return Err(CommandError::InvalidBody.into());
        }
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Modify(body, content_type.into(), reply))
            .await
            .map_err(|_| PublicationError::NotAttached)?;
        result.await.map_err(|_| PublicationError::NotAttached)??;
        Ok(())
    }

    /// Conditionally remove the publication.
    pub async fn remove(&self) -> Result<(), PublicationError> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Remove(reply))
            .await
            .map_err(|_| PublicationError::NotAttached)?;
        result.await.map_err(|_| PublicationError::NotAttached)??;
        Ok(())
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        let (reply, _) = oneshot::channel();
        // discard: Drop cannot wait; the finite lease and dispatcher shutdown remain backstops.
        let _ = self.commands.try_send(Command::Remove(reply));
    }
}

/// Cloneable application handle for inbound observation and outbound starts.
#[derive(Debug, Clone)]
pub struct PublicationsHandle {
    shared: Arc<Shared>,
    compositor: Arc<Mutex<Compositor>>,
}

impl PublicationsHandle {
    /// Start one serialized outbound publication.
    pub fn publish(&self, start: Start) -> Result<Publication, PublicationError> {
        let resource = start.resource.to_bytes().to_vec();
        let mut drivers = lock(&self.shared.drivers);
        drivers.retain(|_, task| !task.is_finished());
        if self.shared.shutdown.is_cancelled() {
            return Err(PublicationError::ShuttingDown);
        }
        if drivers.contains_key(&resource) {
            return Err(PublicationError::DuplicateResource);
        }
        let endpoint = lock(&self.shared.endpoint)
            .clone()
            .ok_or(PublicationError::NotAttached)?;
        reserve(&self.shared)?;
        let mut resources = lock(&self.shared.resources);
        if resources.contains(&resource) {
            release(&self.shared.counters);
            return Err(PublicationError::DuplicateResource);
        }
        let (publisher, initial) = match Publisher::start(self.shared.config.clone(), start) {
            Ok(started) => started,
            Err(error) => {
                release(&self.shared.counters);
                return Err(error.into());
            }
        };
        resources.insert(resource.clone());
        drop(resources);
        let (commands, command_rx) = mpsc::channel(DRIVER_QUEUE);
        let (states, state_rx) = watch::channel(None);
        let driver = Driver {
            endpoint,
            publisher,
            resource: resource.clone(),
            commands: command_rx,
            states,
            events: None,
            response: None,
            timers: HashMap::new(),
            shared: Arc::clone(&self.shared),
        };
        let task = tokio::spawn(driver.run(initial));
        drivers.insert(resource, task);
        drop(drivers);
        Ok(Publication {
            commands,
            states: state_rx,
            body_limit: self.shared.config.body_limit,
        })
    }

    /// The exact compositor allocation used by inbound PUBLISH.
    #[must_use]
    pub fn compositor(&self) -> Arc<Mutex<Compositor>> {
        Arc::clone(&self.compositor)
    }

    /// Point-in-time owned-work counts.
    #[must_use]
    pub fn counts(&self) -> PublicationCounts {
        PublicationCounts {
            active_tasks: self.shared.counters.tasks.load(Ordering::Relaxed),
            active_timers: self.shared.counters.timers.load(Ordering::Relaxed),
            active_transactions: self.shared.counters.transactions.load(Ordering::Relaxed),
            active_publishers: self.shared.counters.publishers.load(Ordering::Relaxed),
            active_publications: lock(&self.compositor).len(),
            shed: self.shared.counters.shed.load(Ordering::Relaxed),
        }
    }
}

/// Dispatcher-owned inbound compositor and outbound publisher runtime.
pub struct Publications {
    endpoint: Option<Handle>,
    compositor: Arc<Mutex<Compositor>>,
    composition: Arc<dyn PublicationComposition>,
    authorization: Arc<dyn PublicationAuthorization>,
    config: PublicationConfig,
    expiry_tasks: HashMap<String, JoinHandle<()>>,
    origin: Instant,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Publications {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Publications")
            .field("config", &self.config)
            .field("inbound_tasks", &self.expiry_tasks.len())
            .finish_non_exhaustive()
    }
}

impl Publications {
    /// Construct a bounded service around the application's exact compositor and policies.
    pub fn new(
        config: PublicationConfig,
        compositor: Compositor,
        composition: Arc<dyn PublicationComposition>,
        authorization: Arc<dyn PublicationAuthorization>,
    ) -> Result<Self, PublicationError> {
        config.validate()?;
        let compositor = Arc::new(Mutex::new(compositor));
        let counters = Arc::new(Counters::default());
        Ok(Self {
            endpoint: None,
            compositor: Arc::clone(&compositor),
            composition,
            authorization,
            config: config.clone(),
            expiry_tasks: HashMap::new(),
            origin: Instant::now(),
            shared: Arc::new(Shared {
                endpoint: Mutex::new(None),
                resources: Mutex::new(HashSet::new()),
                drivers: Mutex::new(HashMap::new()),
                config: config.publisher,
                counters,
                shutdown: CancellationToken::new(),
            }),
        })
    }

    /// Application handle retained across dispatcher ownership transfer.
    #[must_use]
    pub fn handle(&self) -> PublicationsHandle {
        PublicationsHandle {
            shared: Arc::clone(&self.shared),
            compositor: Arc::clone(&self.compositor),
        }
    }

    pub(crate) fn attach(&mut self, endpoint: Handle) {
        self.endpoint = Some(endpoint.clone());
        *lock(&self.shared.endpoint) = Some(endpoint);
    }

    /// Consume one PUBLISH selected by the dispatcher.
    #[allow(
        clippy::too_many_lines,
        reason = "the fail-closed inbound decision table stays in one visible protocol order"
    )]
    pub(crate) async fn receive(&mut self, incoming: &Incoming) {
        self.expiry_tasks.retain(|_, task| !task.is_finished());
        let Some(endpoint) = self.endpoint.clone() else {
            return;
        };
        if !valid_request(&incoming.request) {
            answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        }
        if !self
            .authorization
            .authorize(&incoming.request, incoming.source, incoming.transport)
        {
            answer(&endpoint, incoming, 403, "Forbidden", None, None, None).await;
            return;
        }
        if event(&incoming.request).as_deref() != Some("presence") {
            answer(
                &endpoint,
                incoming,
                489,
                "Bad Event",
                Some((HeaderName::AllowEvents, Bytes::from_static(b"presence"))),
                None,
                None,
            )
            .await;
            return;
        }
        let Ok(tag) = conditional_tag(&incoming.request) else {
            answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        };
        let Some(expires) = requested_expiry(&incoming.request, self.config.default_expiry) else {
            answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        };
        if !expires.is_zero() && expires < self.config.minimum_expiry {
            answer(
                &endpoint,
                incoming,
                423,
                "Interval Too Brief",
                Some((
                    HeaderName::MinExpires,
                    Bytes::from(self.config.minimum_expiry.as_secs().to_string()),
                )),
                None,
                None,
            )
            .await;
            return;
        }
        if incoming.request.body().len() > self.config.body_limit {
            answer(
                &endpoint,
                incoming,
                413,
                "Content Too Large",
                None,
                None,
                None,
            )
            .await;
            return;
        }
        let body = if incoming.request.body().is_empty() {
            None
        } else {
            if incoming.request.headers.count(&HeaderName::ContentType) != 1
                || incoming
                    .request
                    .headers
                    .value(&HeaderName::ContentType)
                    .as_deref()
                    != Some(PIDF_TYPE.as_bytes())
            {
                answer(
                    &endpoint,
                    incoming,
                    415,
                    "Unsupported Media Type",
                    None,
                    None,
                    None,
                )
                .await;
                return;
            }
            let Ok(body) = std::str::from_utf8(incoming.request.body()) else {
                answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
                return;
            };
            Some(body.to_owned())
        };

        let entity = String::from_utf8_lossy(&incoming.request.uri.to_bytes()).into_owned();
        let now = self.origin.elapsed().as_secs();
        let publication = Publish::read(tag, body, expires);
        let outcome = {
            let mut compositor = lock(&self.compositor);
            compositor.expire(now);
            let is_new = matches!(publication, Publish::Initial { .. })
                && compositor.document(&entity).is_none();
            if is_new && compositor.len() >= self.config.capacity {
                None
            } else {
                Some(
                    self.composition
                        .apply(&mut compositor, &entity, publication, now),
                )
            }
        };
        let Some(outcome) = outcome else {
            self.shared.counters.shed.fetch_add(1, Ordering::Relaxed);
            answer(
                &endpoint,
                incoming,
                503,
                "Service Unavailable",
                Some((
                    HeaderName::RetryAfter,
                    Bytes::from(RETRY_AFTER.as_secs().to_string()),
                )),
                None,
                None,
            )
            .await;
            return;
        };
        match outcome {
            Published::Accepted { tag, expires } => {
                answer(
                    &endpoint,
                    incoming,
                    200,
                    "OK",
                    None,
                    Some((&tag, expires)),
                    None,
                )
                .await;
                self.arm_expiry(entity, expires).await;
            }
            Published::Removed { tag } => {
                answer(
                    &endpoint,
                    incoming,
                    200,
                    "OK",
                    None,
                    Some((&tag, Duration::ZERO)),
                    None,
                )
                .await;
                if let Some(task) = self.expiry_tasks.remove(&entity) {
                    abort_and_join(task).await;
                }
            }
            Published::ConditionFailed => {
                answer(
                    &endpoint,
                    incoming,
                    412,
                    "Conditional Request Failed",
                    None,
                    None,
                    None,
                )
                .await;
            }
            Published::Invalid => {
                answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            }
            Published::Unavailable => {
                answer(
                    &endpoint,
                    incoming,
                    500,
                    "Server Internal Error",
                    None,
                    None,
                    None,
                )
                .await;
            }
        }
    }

    async fn arm_expiry(&mut self, entity: String, expires: Duration) {
        if let Some(previous) = self.expiry_tasks.remove(&entity) {
            abort_and_join(previous).await;
        }
        let compositor = Arc::clone(&self.compositor);
        let counters = Arc::clone(&self.shared.counters);
        let origin = self.origin;
        self.expiry_tasks.insert(
            entity,
            tokio::spawn(async move {
                let _guard = WorkGuard::timer(counters);
                // Protocol timer: the duration is the publication expiry being asserted.
                tokio::time::sleep(expires).await;
                lock(&compositor).expire(origin.elapsed().as_secs());
            }),
        );
    }

    /// Cancel the dispatcher-owned service and join every task before returning.
    pub(crate) async fn shutdown(&mut self) {
        let drivers: Vec<_> = {
            let mut drivers = lock(&self.shared.drivers);
            self.shared.shutdown.cancel();
            drivers.drain().map(|(_, task)| task).collect()
        };
        let expiry_tasks: Vec<_> = self.expiry_tasks.drain().map(|(_, task)| task).collect();
        for task in expiry_tasks {
            abort_and_join(task).await;
        }
        for task in drivers {
            if let Err(error) = task.await {
                tracing::warn!(%error, "publication driver did not join cleanly");
            }
        }
    }
}

impl Drop for Publications {
    fn drop(&mut self) {
        let _drivers = lock(&self.shared.drivers);
        self.shared.shutdown.cancel();
        for task in self.expiry_tasks.values() {
            task.abort();
        }
    }
}

#[derive(Debug)]
enum Command {
    Modify(Bytes, String, oneshot::Sender<Result<(), CommandError>>),
    Remove(oneshot::Sender<Result<(), CommandError>>),
}

#[derive(Debug)]
enum RuntimeEvent {
    Response(Option<Response>),
    Timer(Timer, u64),
}

struct Driver {
    endpoint: Handle,
    publisher: Publisher,
    resource: Vec<u8>,
    commands: mpsc::Receiver<Command>,
    states: watch::Sender<Option<StateChange>>,
    events: Option<(mpsc::Sender<RuntimeEvent>, mpsc::Receiver<RuntimeEvent>)>,
    response: Option<JoinHandle<()>>,
    timers: HashMap<Timer, JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Driver {
    async fn run(mut self, initial: Vec<Output>) {
        let _guard = WorkGuard::publisher(Arc::clone(&self.shared.counters));
        let (events, event_rx) = mpsc::channel(DRIVER_QUEUE);
        self.events = Some((events, event_rx));
        self.apply(initial).await;
        while self.publisher.is_active() {
            let input = {
                let Some((_, events)) = self.events.as_mut() else {
                    break;
                };
                tokio::select! {
                    biased;
                    () = self.shared.shutdown.cancelled() => DriverInput::Shutdown,
                    command = self.commands.recv() => DriverInput::Command(command),
                    event = events.recv() => DriverInput::Event(event),
                }
            };
            let outputs = match input {
                DriverInput::Command(Some(Command::Modify(body, content_type, reply))) => {
                    command_result(self.publisher.modify(body, content_type), reply)
                }
                DriverInput::Command(Some(Command::Remove(reply))) => {
                    command_result(self.publisher.remove(), reply)
                }
                DriverInput::Event(Some(RuntimeEvent::Response(response))) => {
                    self.publisher.response(response.as_ref(), &new_cnonce())
                }
                DriverInput::Event(Some(RuntimeEvent::Timer(timer, generation))) => {
                    self.publisher.timer_fired(timer, generation)
                }
                DriverInput::Shutdown | DriverInput::Command(None) | DriverInput::Event(None) => {
                    self.publisher.shutdown_deadline()
                }
            };
            self.apply(outputs).await;
        }
        if let Some(response) = self.response.take() {
            abort_and_join(response).await;
        }
        for (_, timer) in self.timers.drain() {
            abort_and_join(timer).await;
        }
        lock(&self.shared.resources).remove(&self.resource);
    }

    async fn apply(&mut self, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::SendPublish { request, target } => self.send(*request, target).await,
                Output::ArmTimer {
                    timer,
                    generation,
                    after,
                } => self.arm(timer, generation, after).await,
                Output::CancelTimer { timer, .. } => {
                    if let Some(task) = self.timers.remove(&timer) {
                        abort_and_join(task).await;
                    }
                }
                Output::StateChanged(change) => {
                    // discard: no receiver means the application released its observation handle.
                    let _ = self.states.send(Some(change));
                }
            }
        }
    }

    async fn send(&mut self, request: Request, peer: sipx_ua::event_client::Peer) {
        if let Some(previous) = self.response.take() {
            abort_and_join(previous).await;
        }
        let Some((events, _)) = self.events.as_ref() else {
            return;
        };
        match self.endpoint.send(request, transport_target(peer)).await {
            Ok(mut responses) => {
                let events = events.clone();
                let counters = Arc::clone(&self.shared.counters);
                self.response = Some(tokio::spawn(async move {
                    let _guard = WorkGuard::transaction(counters);
                    let response = responses.final_response().await;
                    // discard: closure means the owning driver is already tearing down.
                    let _ = events.send(RuntimeEvent::Response(response)).await;
                }));
            }
            Err(error) => {
                tracing::warn!(%error, "could not send PUBLISH");
                // discard: closure means the owning driver is already tearing down.
                let _ = events.try_send(RuntimeEvent::Response(None));
            }
        }
    }

    async fn arm(&mut self, timer: Timer, generation: u64, after: Duration) {
        if let Some(previous) = self.timers.remove(&timer) {
            abort_and_join(previous).await;
        }
        let Some((events, _)) = self.events.as_ref() else {
            return;
        };
        let events = events.clone();
        let counters = Arc::clone(&self.shared.counters);
        self.timers.insert(
            timer,
            tokio::spawn(async move {
                let _guard = WorkGuard::timer(counters);
                // Protocol timer: the duration is the state-machine input this task represents.
                tokio::time::sleep(after).await;
                // discard: closure means the owning driver has cancelled this timer's state.
                let _ = events.send(RuntimeEvent::Timer(timer, generation)).await;
            }),
        );
    }
}

async fn abort_and_join(task: JoinHandle<()>) {
    task.abort();
    // discard: cancellation is the requested outcome; the await is solely the ownership barrier.
    let _ = task.await;
}

enum DriverInput {
    Command(Option<Command>),
    Event(Option<RuntimeEvent>),
    Shutdown,
}

fn command_result(
    result: Result<Vec<Output>, CommandError>,
    reply: oneshot::Sender<Result<(), CommandError>>,
) -> Vec<Output> {
    match result {
        Ok(outputs) => {
            // discard: the command still took effect if its caller stopped waiting for the reply.
            let _ = reply.send(Ok(()));
            outputs
        }
        Err(error) => {
            // discard: the typed failure has no observer after its caller cancels the command.
            let _ = reply.send(Err(error));
            Vec::new()
        }
    }
}

enum WorkKind {
    Publisher,
    Timer,
    Transaction,
}

struct WorkGuard {
    counters: Arc<Counters>,
    kind: WorkKind,
}

impl WorkGuard {
    fn publisher(counters: Arc<Counters>) -> Self {
        counters.tasks.fetch_add(1, Ordering::Relaxed);
        Self {
            counters,
            kind: WorkKind::Publisher,
        }
    }

    fn timer(counters: Arc<Counters>) -> Self {
        counters.tasks.fetch_add(1, Ordering::Relaxed);
        counters.timers.fetch_add(1, Ordering::Relaxed);
        Self {
            counters,
            kind: WorkKind::Timer,
        }
    }

    fn transaction(counters: Arc<Counters>) -> Self {
        counters.transactions.fetch_add(1, Ordering::Relaxed);
        Self {
            counters,
            kind: WorkKind::Transaction,
        }
    }
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        match self.kind {
            WorkKind::Publisher => {
                self.counters.tasks.fetch_sub(1, Ordering::Relaxed);
                self.counters.publishers.fetch_sub(1, Ordering::Relaxed);
            }
            WorkKind::Timer => {
                self.counters.tasks.fetch_sub(1, Ordering::Relaxed);
                self.counters.timers.fetch_sub(1, Ordering::Relaxed);
            }
            WorkKind::Transaction => {
                self.counters.transactions.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

// `fetch_update` remains the spelling available at the workspace MSRV; current nightly deprecates
// it before the replacement is available on that supported toolchain.
#[allow(deprecated)]
fn reserve(shared: &Shared) -> Result<(), PublicationError> {
    shared
        .counters
        .publishers
        .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |current| {
            (current < shared.config.capacity).then_some(current.saturating_add(1))
        })
        .map_err(|_| {
            shared.counters.shed.fetch_add(1, Ordering::Relaxed);
            PublicationError::CapacityExceeded
        })?;
    Ok(())
}

fn release(counters: &Counters) {
    counters.publishers.fetch_sub(1, Ordering::Relaxed);
}

fn valid_request(request: &Request) -> bool {
    request.method == Method::Publish
        && request.headers.count(&HeaderName::CallId) == 1
        && request.headers.count(&HeaderName::From) == 1
        && request.headers.count(&HeaderName::To) == 1
        && request.headers.count(&HeaderName::CSeq) == 1
        && matches!(
            request.headers.typed::<CSeq>(),
            Some(Ok(CSeq {
                method: Method::Publish,
                ..
            }))
        )
}

fn event(request: &Request) -> Option<String> {
    if request.headers.count(&HeaderName::Event) != 1 {
        return None;
    }
    let value = request.headers.value(&HeaderName::Event)?;
    let token = std::str::from_utf8(&value).ok()?.split(';').next()?.trim();
    (!token.is_empty()).then(|| token.to_ascii_lowercase())
}

fn conditional_tag(request: &Request) -> Result<Option<String>, ()> {
    match request.headers.count(&HeaderName::SipIfMatch) {
        0 => Ok(None),
        1 => {
            let value = request.headers.value(&HeaderName::SipIfMatch).ok_or(())?;
            if opaque_token(&value) {
                Ok(Some(String::from_utf8_lossy(&value).into_owned()))
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

fn requested_expiry(request: &Request, default: Duration) -> Option<Duration> {
    match request.headers.count(&HeaderName::Expires) {
        0 => Some(default),
        1 => request
            .headers
            .typed::<Expires>()?
            .ok()
            .map(|expires| Duration::from_secs(u64::from(expires.0))),
        _ => None,
    }
}

fn opaque_token(value: &[u8]) -> bool {
    !value.is_empty()
        && value.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
                )
        })
}

async fn answer(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    extra: Option<(HeaderName, Bytes)>,
    authority: Option<(&str, Duration)>,
    to_tag: Option<&str>,
) {
    let Some(status) = StatusCode::new(status) else {
        return;
    };
    let built = ResponseBuilder::to_request(&incoming.request, status, reason)
        .and_then(|builder| with_to_tag(builder, &incoming.request, to_tag))
        .and_then(|builder| match extra {
            Some((name, value)) => builder.header(name, value),
            None => Ok(builder),
        })
        .and_then(|builder| match authority {
            Some((tag, expires)) => builder
                .header(HeaderName::SipETag, Bytes::from(tag.to_owned()))?
                .header(
                    HeaderName::Expires,
                    Bytes::from(expires.as_secs().to_string()),
                ),
            None => Ok(builder),
        });
    let Ok(builder) = built else {
        return;
    };
    if let Err(error) = endpoint.respond(&incoming.key, builder.build()).await {
        tracing::warn!(%error, "could not answer PUBLISH");
    }
}

fn transport(value: sipx_ua::event_client::Transport) -> TransportKind {
    match value {
        sipx_ua::event_client::Transport::Udp => TransportKind::Udp,
        sipx_ua::event_client::Transport::Tcp => TransportKind::Tcp,
        sipx_ua::event_client::Transport::Tls => TransportKind::Tls,
        sipx_ua::event_client::Transport::Ws => TransportKind::Ws,
        sipx_ua::event_client::Transport::Wss => TransportKind::Wss,
        sipx_ua::event_client::Transport::Quic => TransportKind::Quic,
    }
}

fn transport_target(peer: sipx_ua::event_client::Peer) -> Target {
    let mut target = Target::new(peer.address, transport(peer.transport));
    if let Some(identity) = peer.identity {
        target = target.verifying(identity);
    }
    if let Some(path) = peer.path {
        target = target.at_path(path);
    }
    target
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod admission_tests {
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    use bytes::Bytes;
    use sipx_transport::{Config as TransportConfig, bind};
    use sipx_ua::event_client::{Peer, Transport};
    use sipx_ua::presence::Compositor;
    use sipx_ua::publication_client::Start;

    use super::*;

    fn start(target: std::net::SocketAddr) -> Start {
        Start {
            resource: sipx_sip::Uri::parse(Bytes::from_static(b"sip:resource@example.test"))
                .expect("URI"),
            local_identity: "<sip:client@example.test>".to_owned(),
            target: Peer::new(target, Transport::Udp),
            event: "presence".to_owned(),
            expires: Duration::from_secs(60),
            body: Bytes::from_static(b"<presence/>"),
            content_type: "application/pidf+xml".to_owned(),
            credentials: None,
            call_id: "admission@example.test".to_owned(),
            from_tag: "admission".to_owned(),
            initial_cseq: 1,
        }
    }

    #[tokio::test]
    async fn racing_shutdown_closes_admission_before_any_spawn() {
        let (endpoint, _) = bind(TransportConfig::new(
            "127.0.0.1:0".parse().expect("address"),
        ))
        .await
        .expect("endpoint");
        let mut runtime = Publications::new(
            PublicationConfig::default(),
            Compositor::new(Duration::from_secs(60)),
            Arc::new(ReplacePublicationState),
            Arc::new(AllowPublications),
        )
        .expect("runtime");
        runtime.attach(endpoint.clone());
        let handle = runtime.handle();
        let post_shutdown = handle.clone();
        let shared = Arc::clone(&runtime.shared);
        let drivers = lock(&shared.drivers);
        let barrier = Arc::new(Barrier::new(2));
        let contender = Arc::clone(&barrier);
        let target = endpoint.local_addr();
        let attempt = std::thread::spawn(move || {
            contender.wait();
            handle.publish(start(target))
        });
        barrier.wait();
        shared.shutdown.cancel();
        drop(drivers);
        assert!(matches!(
            attempt.join().expect("thread"),
            Err(PublicationError::ShuttingDown)
        ));
        assert!(matches!(
            post_shutdown.publish(start(target)),
            Err(PublicationError::ShuttingDown)
        ));
        assert!(lock(&shared.drivers).is_empty());
        endpoint.shutdown().await;
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
    use super::*;
    use sipx_ua::event_client::{Peer, Transport};

    #[test]
    fn publication_driver_preserves_secure_target_identity_and_resource() {
        let peer = Peer::new("192.0.2.20:7443".parse().expect("peer"), Transport::Wss)
            .verifying("compositor.example.test")
            .at_path("/publish");
        let target = transport_target(peer);
        assert_eq!(target.transport, TransportKind::Wss);
        assert_eq!(target.verify_as.as_deref(), Some("compositor.example.test"));
        assert_eq!(target.path.as_deref(), Some("/publish"));
    }
}
