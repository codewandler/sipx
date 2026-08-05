//! Runtime driver for the sans-I/O RFC 6665 event client.
//!
//! [`sipx_ua::event_client::EventClient`] owns protocol decisions. This module owns the bounded
//! socket transactions, timers and application channels that apply those decisions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Response, StatusCode};
use sipx_transport::{Handle, Incoming, Target, TransportKind};
use sipx_ua::event_client::{
    Config, EventClient, NotificationMeta, Output, PackageConsumer, Peer, Start, StartError,
    StateChange, SubscriptionId, Timer, Transport,
};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const DRIVER_QUEUE: usize = 64;

/// Runtime resource measurements for outbound event subscriptions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSubscriptionCounts {
    /// Live lifecycle tasks.
    pub active_tasks: usize,
    /// Live timer tasks.
    pub active_timers: usize,
    /// SUBSCRIBE transactions whose final result is still being observed.
    pub active_transactions: usize,
    /// Lifecycle tasks started since construction.
    pub started_tasks: u64,
    /// Lifecycle tasks which exited.
    pub finished_tasks: u64,
    /// NOTIFY requests refused because a bounded driver queue was full.
    pub shed: u64,
}

#[derive(Debug, Default)]
struct Counters {
    tasks: AtomicUsize,
    timers: AtomicUsize,
    transactions: AtomicUsize,
    started: AtomicU64,
    finished: AtomicU64,
    shed: AtomicU64,
}

/// Failure before an outbound subscription owns runtime work.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EventSubscriptionError {
    /// The client has not been attached to a dispatcher yet.
    #[error("event subscriptions are not attached to a dispatcher")]
    NotAttached,
    /// Dispatcher shutdown has atomically closed admission.
    #[error("event subscription shutdown has closed admission")]
    ShuttingDown,
    /// A peer-driven or configured client bound was reached.
    #[error("event subscription capacity exceeded")]
    CapacityExceeded,
    /// The caller reused an active Call-ID.
    #[error("an event subscription already owns this Call-ID")]
    DuplicateIdentity,
    /// The pure client rejected the start fields.
    #[error(transparent)]
    Start(#[from] StartError),
}

/// One application delivery from a package consumer.
#[derive(Debug)]
pub struct EventNotification<V> {
    /// Local monotonic instant when the runtime accepted this package value.
    pub received_at: tokio::time::Instant,
    /// Framework state and remote sequence; absent for the initial neutral value.
    pub metadata: Option<NotificationMeta>,
    /// Parsed package value.
    pub value: V,
}

/// One application-visible event from a running subscription.
#[derive(Debug)]
pub enum EventSubscriptionEvent<V> {
    /// A package value was accepted from a NOTIFY.
    Notification(EventNotification<V>),
    /// The framework lifecycle changed.
    State(StateChange),
}

/// Application ownership of one running subscription.
#[derive(Debug)]
pub struct EventSubscription<V> {
    id: SubscriptionId,
    deliveries: mpsc::Receiver<EventNotification<V>>,
    states: mpsc::Receiver<StateChange>,
    commands: mpsc::Sender<Command>,
}

impl<V> EventSubscription<V> {
    /// Opaque identity allocated by the pure client.
    #[must_use]
    pub fn id(&self) -> SubscriptionId {
        self.id
    }

    /// Receive one package value and release its bounded queue slot.
    pub async fn recv(&mut self) -> Option<EventNotification<V>> {
        let delivery = self.deliveries.recv().await?;
        // discard: the bounded command queue cannot fill before its equally bounded deliveries.
        let _ = self.commands.try_send(Command::Drained(1));
        Some(delivery)
    }

    /// Receive one lifecycle fact.
    pub async fn next_state(&mut self) -> Option<StateChange> {
        self.states.recv().await
    }

    /// Receive the next package value or lifecycle fact without favoring either channel.
    ///
    /// This is the cancellation-safe choice for applications which must distinguish an initial
    /// refusal from the first package snapshot. [`Self::recv`] and [`Self::next_state`] remain
    /// available when an application intentionally observes only one side.
    pub async fn next_event(&mut self) -> Option<EventSubscriptionEvent<V>> {
        tokio::select! {
            Some(delivery) = self.deliveries.recv() => {
                // discard: the bounded command queue cannot fill before its bounded deliveries.
                let _ = self.commands.try_send(Command::Drained(1));
                Some(EventSubscriptionEvent::Notification(delivery))
            }
            Some(change) = self.states.recv() => Some(EventSubscriptionEvent::State(change)),
            else => None,
        }
    }

    /// Send Expires 0 and wait only for command admission. Terminal NOTIFY or Timer N completes
    /// the protocol operation and is observable through [`Self::next_state`].
    pub async fn unsubscribe(&self) -> Result<(), EventSubscriptionError> {
        self.commands
            .send(Command::Unsubscribe)
            .await
            .map_err(|_| EventSubscriptionError::NotAttached)
    }
}

impl<V> Drop for EventSubscription<V> {
    fn drop(&mut self) {
        // discard: Drop cannot wait; finite expiry and dispatcher shutdown remain backstops.
        let _ = self.commands.try_send(Command::Unsubscribe);
    }
}

#[derive(Debug)]
struct Shared {
    endpoint: Mutex<Option<Handle>>,
    routes: Mutex<HashMap<Vec<u8>, mpsc::Sender<Incoming>>>,
    drivers: Mutex<HashMap<Vec<u8>, JoinHandle<()>>>,
    config: Config,
    counters: Arc<Counters>,
    shutdown: CancellationToken,
}

/// Cloneable application handle. It does not keep dispatcher-owned tasks alive.
#[derive(Debug, Clone)]
pub struct EventSubscriptionsHandle {
    shared: Arc<Shared>,
}

impl EventSubscriptionsHandle {
    /// Start one package-generic subscription through the attached endpoint.
    pub fn subscribe<C: PackageConsumer>(
        &self,
        start: Start<C>,
    ) -> Result<EventSubscription<C::Value>, EventSubscriptionError> {
        let call_id = start.call_id.as_bytes().to_vec();
        let mut drivers = lock(&self.shared.drivers);
        drivers.retain(|_, task| !task.is_finished());
        if self.shared.shutdown.is_cancelled() {
            return Err(EventSubscriptionError::ShuttingDown);
        }
        if drivers.contains_key(&call_id) {
            return Err(EventSubscriptionError::DuplicateIdentity);
        }
        let endpoint = lock(&self.shared.endpoint)
            .clone()
            .ok_or(EventSubscriptionError::NotAttached)?;
        reserve(&self.shared)?;
        let mut core = match EventClient::new(self.shared.config.clone()) {
            Ok(core) => core,
            Err(error) => {
                release(&self.shared.counters);
                return Err(error.into());
            }
        };
        let (id, initial) = match core.start(start) {
            Ok(started) => started,
            Err(error) => {
                release(&self.shared.counters);
                return Err(error.into());
            }
        };
        let (incoming_tx, incoming_rx) = mpsc::channel(DRIVER_QUEUE);
        let mut routes = lock(&self.shared.routes);
        if routes.contains_key(&call_id) {
            release(&self.shared.counters);
            return Err(EventSubscriptionError::DuplicateIdentity);
        }
        routes.insert(call_id.clone(), incoming_tx);
        drop(routes);
        let (delivery_tx, deliveries) = mpsc::channel(self.shared.config.delivery_capacity);
        let (state_tx, states) = mpsc::channel(DRIVER_QUEUE);
        let (command_tx, command_rx) = mpsc::channel(DRIVER_QUEUE);
        let driver = Driver {
            id,
            call_id: call_id.clone(),
            endpoint,
            core,
            incoming: incoming_rx,
            commands: command_rx,
            delivery: delivery_tx,
            states: state_tx,
            events: None,
            response: None,
            timers: HashMap::new(),
            shared: Arc::clone(&self.shared),
        };
        self.shared.counters.started.fetch_add(1, Ordering::Relaxed);
        let task = tokio::spawn(driver.run(initial));
        drivers.insert(call_id, task);
        drop(drivers);
        Ok(EventSubscription {
            id,
            deliveries,
            states,
            commands: command_tx,
        })
    }

    /// Point-in-time owned-work counts.
    #[must_use]
    pub fn counts(&self) -> EventSubscriptionCounts {
        EventSubscriptionCounts {
            active_tasks: self.shared.counters.tasks.load(Ordering::Relaxed),
            active_timers: self.shared.counters.timers.load(Ordering::Relaxed),
            active_transactions: self.shared.counters.transactions.load(Ordering::Relaxed),
            started_tasks: self.shared.counters.started.load(Ordering::Relaxed),
            finished_tasks: self.shared.counters.finished.load(Ordering::Relaxed),
            shed: self.shared.counters.shed.load(Ordering::Relaxed),
        }
    }
}

/// Dispatcher-owned outbound event subscription runtime.
#[derive(Debug)]
pub struct EventSubscriptions {
    shared: Arc<Shared>,
}

impl EventSubscriptions {
    /// Construct a bounded runtime. Attach it with
    /// [`crate::Dispatcher::with_event_subscriptions`] before starting work.
    pub fn new(config: Config) -> Result<Self, EventSubscriptionError> {
        config.validate()?;
        Ok(Self {
            shared: Arc::new(Shared {
                endpoint: Mutex::new(None),
                routes: Mutex::new(HashMap::new()),
                drivers: Mutex::new(HashMap::new()),
                config,
                counters: Arc::new(Counters::default()),
                shutdown: CancellationToken::new(),
            }),
        })
    }

    /// Application handle which can start subscriptions after dispatcher attachment.
    #[must_use]
    pub fn handle(&self) -> EventSubscriptionsHandle {
        EventSubscriptionsHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    pub(crate) fn attach(&self, endpoint: Handle) {
        *lock(&self.shared.endpoint) = Some(endpoint);
    }

    /// Route one NOTIFY without moving it away from the dispatcher's fallback path.
    pub(crate) async fn receive(&self, incoming: &Incoming) -> bool {
        let Some(call_id) = incoming.request.headers.value(&HeaderName::CallId) else {
            return false;
        };
        let sender = lock(&self.shared.routes).get(call_id.as_ref()).cloned();
        let Some(sender) = sender else {
            return false;
        };
        let cloned = Incoming {
            key: incoming.key.clone(),
            request: incoming.request.clone(),
            source: incoming.source,
            transport: incoming.transport,
            connection_generation: incoming.connection_generation,
        };
        match sender.try_send(cloned) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.shared.counters.shed.fetch_add(1, Ordering::Relaxed);
                let endpoint = lock(&self.shared.endpoint).clone();
                answer_notify(
                    endpoint.as_ref(),
                    incoming,
                    503,
                    Some(Duration::from_secs(1)),
                )
                .await;
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    pub(crate) async fn shutdown(&mut self) {
        let drivers: Vec<_> = {
            let mut drivers = lock(&self.shared.drivers);
            self.shared.shutdown.cancel();
            drivers.drain().map(|(_, task)| task).collect()
        };
        for task in drivers {
            if let Err(error) = task.await {
                tracing::warn!(%error, "event subscription driver did not join cleanly");
            }
        }
    }
}

impl Drop for EventSubscriptions {
    fn drop(&mut self) {
        let _drivers = lock(&self.shared.drivers);
        self.shared.shutdown.cancel();
    }
}

#[derive(Debug)]
enum Command {
    Drained(usize),
    Unsubscribe,
}

#[derive(Debug)]
enum RuntimeEvent {
    Response(Option<Response>),
    Timer(Timer, u64),
}

struct Driver<C: PackageConsumer> {
    id: SubscriptionId,
    call_id: Vec<u8>,
    endpoint: Handle,
    core: EventClient<C>,
    incoming: mpsc::Receiver<Incoming>,
    commands: mpsc::Receiver<Command>,
    delivery: mpsc::Sender<EventNotification<C::Value>>,
    states: mpsc::Sender<StateChange>,
    events: Option<(mpsc::Sender<RuntimeEvent>, mpsc::Receiver<RuntimeEvent>)>,
    response: Option<JoinHandle<()>>,
    timers: HashMap<Timer, JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl<C: PackageConsumer> Driver<C> {
    async fn run(mut self, initial: Vec<Output<C::Value>>) {
        let guard = WorkGuard::task(Arc::clone(&self.shared.counters));
        let (event_tx, event_rx) = mpsc::channel(DRIVER_QUEUE);
        self.events = Some((event_tx, event_rx));
        self.apply(initial, None).await;
        loop {
            if !self.core.contains(self.id) {
                break;
            }
            let event = {
                let Some((_, events)) = self.events.as_mut() else {
                    break;
                };
                tokio::select! {
                    () = self.shared.shutdown.cancelled() => DriverInput::Shutdown,
                    incoming = self.incoming.recv() => incoming.map_or(DriverInput::Shutdown, |incoming| DriverInput::Incoming(Box::new(incoming))),
                    command = self.commands.recv() => DriverInput::Command(command),
                    event = events.recv() => DriverInput::Event(event),
                    () = self.delivery.closed() => DriverInput::Shutdown,
                }
            };
            let (outputs, incoming) = match event {
                DriverInput::Incoming(incoming) => {
                    let incoming = *incoming;
                    let source = peer_from_incoming(&incoming);
                    let outputs = self.core.notify(1, &incoming.request, source);
                    (outputs, Some(incoming))
                }
                DriverInput::Command(Some(Command::Drained(count))) => {
                    self.core.consumer_drained(self.id, count);
                    (Vec::new(), None)
                }
                DriverInput::Command(Some(Command::Unsubscribe)) => {
                    (self.core.unsubscribe(self.id), None)
                }
                DriverInput::Event(Some(RuntimeEvent::Response(response))) => (
                    self.core
                        .response(self.id, response.as_ref(), &sipx_ua::auth::new_cnonce()),
                    None,
                ),
                DriverInput::Event(Some(RuntimeEvent::Timer(timer, generation))) => {
                    (self.core.timer_fired(self.id, timer, generation), None)
                }
                DriverInput::Shutdown | DriverInput::Command(None) | DriverInput::Event(None) => {
                    (self.core.shutdown_deadline(), None)
                }
            };
            self.apply(outputs, incoming.as_ref()).await;
        }
        if self.core.contains(self.id) {
            let outputs = self.core.shutdown_deadline();
            self.apply(outputs, None).await;
        }
        if let Some(response) = self.response.take() {
            abort_and_join(response).await;
        }
        for (_, timer) in self.timers.drain() {
            abort_and_join(timer).await;
        }
        lock(&self.shared.routes).remove(&self.call_id);
        drop(guard);
    }

    async fn apply(&mut self, outputs: Vec<Output<C::Value>>, incoming: Option<&Incoming>) {
        for output in outputs {
            match output {
                Output::SendSubscribe {
                    request, target, ..
                } => self.send(*request, target).await,
                Output::RespondNotify {
                    status,
                    retry_after,
                    ..
                } => {
                    if let Some(incoming) = incoming {
                        answer_notify(Some(&self.endpoint), incoming, status, retry_after).await;
                    }
                }
                Output::Deliver {
                    metadata, value, ..
                } => {
                    // discard: failure requires concurrent consumer closure; the driver then stops.
                    let _ = self.delivery.try_send(EventNotification {
                        received_at: tokio::time::Instant::now(),
                        metadata,
                        value,
                    });
                }
                Output::ArmTimer {
                    timer,
                    generation,
                    after,
                    ..
                } => self.arm(timer, generation, after).await,
                Output::CancelTimer { timer, .. } => {
                    if let Some(task) = self.timers.remove(&timer) {
                        abort_and_join(task).await;
                    }
                }
                Output::StateChanged { change, .. } => {
                    // discard: a full or closed state channel means its consumer chose not to read.
                    let _ = self.states.try_send(change);
                }
                Output::Stopped => {}
            }
        }
    }

    async fn send(&mut self, request: sipx_sip::Request, peer: Peer) {
        if let Some(response) = self.response.take() {
            abort_and_join(response).await;
        }
        let Some((events, _)) = self.events.as_ref() else {
            return;
        };
        match self.endpoint.send(request, target(peer)).await {
            Ok(mut responses) => {
                self.core
                    .connection_selected(self.id, responses.connection_generation());
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
                tracing::warn!(%error, "could not send event SUBSCRIBE");
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
    Incoming(Box<Incoming>),
    Command(Option<Command>),
    Event(Option<RuntimeEvent>),
    Shutdown,
}

enum WorkKind {
    Task,
    Timer,
    Transaction,
}

struct WorkGuard {
    counters: Arc<Counters>,
    kind: WorkKind,
}

impl WorkGuard {
    fn task(counters: Arc<Counters>) -> Self {
        Self {
            counters,
            kind: WorkKind::Task,
        }
    }

    fn timer(counters: Arc<Counters>) -> Self {
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
            WorkKind::Task => {
                self.counters.tasks.fetch_sub(1, Ordering::Relaxed);
                self.counters.finished.fetch_add(1, Ordering::Relaxed);
            }
            WorkKind::Timer => {
                self.counters.timers.fetch_sub(1, Ordering::Relaxed);
            }
            WorkKind::Transaction => {
                self.counters.transactions.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

fn reserve(shared: &Shared) -> Result<(), EventSubscriptionError> {
    shared
        .counters
        .tasks
        .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |current| {
            (current < shared.config.capacity).then_some(current.saturating_add(1))
        })
        .map_err(|_| EventSubscriptionError::CapacityExceeded)?;
    Ok(())
}

fn release(counters: &Counters) {
    counters.tasks.fetch_sub(1, Ordering::Relaxed);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn peer_from_incoming(incoming: &Incoming) -> Peer {
    let mut peer = Peer::new(incoming.source, from_transport(incoming.transport));
    peer.connection = incoming.connection_generation;
    peer
}

fn target(peer: Peer) -> Target {
    let mut target = Target::new(peer.address, to_transport(peer.transport));
    if let Some(identity) = peer.identity {
        target = target.verifying(identity);
    }
    if let Some(path) = peer.path {
        target = target.at_path(path);
    }
    target
}

fn from_transport(transport: TransportKind) -> Transport {
    match transport {
        TransportKind::Udp => Transport::Udp,
        TransportKind::Tcp => Transport::Tcp,
        TransportKind::Tls => Transport::Tls,
        TransportKind::Ws => Transport::Ws,
        TransportKind::Wss => Transport::Wss,
        TransportKind::Quic => Transport::Quic,
    }
}

fn to_transport(transport: Transport) -> TransportKind {
    match transport {
        Transport::Udp => TransportKind::Udp,
        Transport::Tcp => TransportKind::Tcp,
        Transport::Tls => TransportKind::Tls,
        Transport::Ws => TransportKind::Ws,
        Transport::Wss => TransportKind::Wss,
        Transport::Quic => TransportKind::Quic,
    }
}

async fn answer_notify(
    endpoint: Option<&Handle>,
    incoming: &Incoming,
    status: u16,
    retry_after: Option<Duration>,
) {
    let (Some(endpoint), Some(status)) = (endpoint, StatusCode::new(status)) else {
        return;
    };
    let built = ResponseBuilder::to_request(&incoming.request, status, "Event notification")
        .and_then(|builder| match retry_after {
            Some(value) => builder.header(
                HeaderName::RetryAfter,
                Bytes::from(value.as_secs().to_string()),
            ),
            None => Ok(builder),
        });
    let Ok(builder) = built else {
        return;
    };
    if let Err(error) = endpoint.respond(&incoming.key, builder.build()).await {
        tracing::warn!(%error, "could not answer event NOTIFY");
    }
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
    use sipx_ua::event_client::{PackageRejection, SamePeer, Start, Transport};

    use super::*;

    #[derive(Debug)]
    struct Package;

    impl PackageConsumer for Package {
        type Value = ();
        fn event(&self) -> &'static str {
            "admission"
        }
        fn accept(&self) -> &[String] {
            &[]
        }
        fn neutral(&mut self) -> Option<()> {
            None
        }
        fn consume(&mut self, _: Option<&[u8]>, _: &[u8]) -> Result<(), PackageRejection> {
            Ok(())
        }
    }

    fn start(target: std::net::SocketAddr) -> Start<Package> {
        Start {
            resource: sipx_sip::Uri::parse(Bytes::from_static(b"sip:resource@example.test"))
                .expect("URI"),
            local_identity: "<sip:client@example.test>".to_owned(),
            contact: "<sip:client@127.0.0.1>".to_owned(),
            target: Peer::new(target, Transport::Udp),
            expires: Duration::from_secs(60),
            body: Bytes::new(),
            content_type: None,
            credentials: None,
            call_id: "admission@example.test".to_owned(),
            from_tag: "admission".to_owned(),
            initial_cseq: 1,
            consumer: Package,
            trust: Arc::new(SamePeer),
        }
    }

    #[tokio::test]
    async fn racing_shutdown_closes_admission_before_any_spawn() {
        let (endpoint, _) = bind(TransportConfig::new(
            "127.0.0.1:0".parse().expect("address"),
        ))
        .await
        .expect("endpoint");
        let runtime = EventSubscriptions::new(Config::default()).expect("runtime");
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
            handle.subscribe(start(target))
        });
        barrier.wait();
        shared.shutdown.cancel();
        drop(drivers);
        assert!(matches!(
            attempt.join().expect("thread"),
            Err(EventSubscriptionError::ShuttingDown)
        ));
        assert!(matches!(
            post_shutdown.subscribe(start(target)),
            Err(EventSubscriptionError::ShuttingDown)
        ));
        assert!(lock(&shared.drivers).is_empty());
        endpoint.shutdown().await;
    }
}
