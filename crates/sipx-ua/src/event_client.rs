//! Sans-I/O subscriber state for SIP event packages (RFC 6665).
//!
//! This module owns no socket, task or clock. A driver applies [`Output`] values and feeds
//! responses, NOTIFY requests and fired timer generations back through [`EventClient`]. The
//! normative state tables and byte vectors live in `docs/specs/event-client.md`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::auth::{Challenge, Credentials, respond, strongest};
use sipx_sip::build::RequestBuilder;
use sipx_sip::event::{Reason, State, Subscription};
use sipx_sip::headers::{CSeq, Contact, Expires, From as FromHeader, RecordRoute, To};
use sipx_sip::{Address, Header, HeaderName, Method, Request, Response, Uri};
use thiserror::Error;

/// RFC 6665's Timer N: 64 times SIP's default 500 ms T1.
pub const DEFAULT_TIMER_N: Duration = Duration::from_secs(32);
/// Default number of logical subscriptions held by one client.
pub const DEFAULT_CAPACITY: usize = 1_024;
/// Default number of application deliveries retained for one subscription.
pub const DEFAULT_DELIVERY_CAPACITY: usize = 32;
/// Default maximum body accepted in either direction.
pub const DEFAULT_BODY_LIMIT: usize = 65_536;

/// A transport identity supplied by the I/O driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// UDP datagrams.
    Udp,
    /// TCP stream.
    Tcp,
    /// TLS stream.
    Tls,
    /// WebSocket stream.
    Ws,
    /// Secure WebSocket stream.
    Wss,
    /// QUIC connection.
    Quic,
}

/// The peer and, for a stream, exact connection generation used by an exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Peer {
    /// Remote socket address.
    pub address: SocketAddr,
    /// SIP transport.
    pub transport: Transport,
    /// Driver-defined stream generation; absent for UDP.
    pub connection: Option<u64>,
}

/// Default fail-closed NOTIFY origin policy.
#[derive(Debug, Default)]
pub struct SamePeer;

/// Policy applied before a NOTIFY can select or mutate a dialog.
pub trait NotifyTrustPolicy: Send + Sync + 'static {
    /// Whether this request arrived through an authorized peer/connection.
    fn accepts(&self, selected_target: Peer, received_from: Peer, request: &Request) -> bool;
}

impl NotifyTrustPolicy for SamePeer {
    fn accepts(&self, selected_target: Peer, received_from: Peer, _request: &Request) -> bool {
        selected_target == received_from
    }
}

/// Package-specific parsing behind the generic event lifecycle.
pub trait PackageConsumer: Send + 'static {
    /// Owned application value produced by this package.
    type Value: Send + 'static;

    /// Exact Event package token.
    fn event(&self) -> &str;

    /// Optional Event `id` parameter.
    fn event_id(&self) -> Option<&str> {
        None
    }

    /// Values advertised in `Accept`.
    fn accept(&self) -> &[String];

    /// Neutral value delivered before the first NOTIFY, when the package defines one.
    fn neutral(&mut self) -> Option<Self::Value>;

    /// Whether an empty terminal body is valid without invoking the consumer.
    fn empty_terminal_is_valid(&self) -> bool {
        true
    }

    /// Parse one bounded NOTIFY body.
    fn consume(
        &mut self,
        content_type: Option<&[u8]>,
        body: &[u8],
    ) -> Result<Self::Value, PackageRejection>;
}

/// A package-controlled final refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageRejection {
    /// SIP final status, normally 400 or 415.
    pub status: u16,
}

impl PackageRejection {
    /// Reject malformed package bytes.
    #[must_use]
    pub const fn malformed() -> Self {
        Self { status: 400 }
    }

    /// Reject an unsupported media type.
    #[must_use]
    pub const fn unsupported_media() -> Self {
        Self { status: 415 }
    }
}

/// Bounded client configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum logical subscription intents.
    pub capacity: usize,
    /// Maximum undrained deliveries per subscription.
    pub delivery_capacity: usize,
    /// Maximum retained NOTIFY body.
    pub notify_body_limit: usize,
    /// Maximum outbound SUBSCRIBE body.
    pub subscribe_body_limit: usize,
    /// Maximum digest retries per operation.
    pub authentication_retries: u8,
    /// Maximum 423 retries per operation.
    pub interval_retries: u8,
    /// Timer N duration.
    pub timer_n: Duration,
    /// Host maximum accepted interval.
    pub maximum_expiry: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            delivery_capacity: DEFAULT_DELIVERY_CAPACITY,
            notify_body_limit: DEFAULT_BODY_LIMIT,
            subscribe_body_limit: DEFAULT_BODY_LIMIT,
            authentication_retries: 2,
            interval_retries: 1,
            timer_n: DEFAULT_TIMER_N,
            maximum_expiry: Duration::from_secs(u64::from(u32::MAX)),
        }
    }
}

impl Config {
    /// Validate every peer-driven bound before allocating a client.
    pub fn validate(&self) -> Result<(), StartError> {
        if self.capacity == 0
            || self.delivery_capacity == 0
            || self.notify_body_limit == 0
            || self.subscribe_body_limit == 0
            || self.authentication_retries == 0
            || self.interval_retries == 0
            || self.timer_n.is_zero()
            || self.maximum_expiry.is_zero()
            || self.maximum_expiry.as_secs() > u64::from(u32::MAX)
        {
            return Err(StartError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Driver-supplied fields for one initial SUBSCRIBE.
pub struct Start<C> {
    /// Resource URI and initial Request-URI.
    pub resource: Uri,
    /// Address placed in From, without a tag.
    pub local_identity: String,
    /// Contact value advertised by this client.
    pub contact: String,
    /// Selected network target.
    pub target: Peer,
    /// Desired positive lifetime.
    pub expires: Duration,
    /// Optional bounded request body.
    pub body: Bytes,
    /// Optional request media type when `body` is non-empty.
    pub content_type: Option<String>,
    /// Optional digest credentials.
    pub credentials: Option<Credentials>,
    /// Fresh opaque Call-ID.
    pub call_id: String,
    /// Fresh local From tag.
    pub from_tag: String,
    /// Non-zero first local `CSeq`.
    pub initial_cseq: u32,
    /// Package parser and state owner.
    pub consumer: C,
    /// NOTIFY origin authorization.
    pub trust: Arc<dyn NotifyTrustPolicy>,
}

impl<C> std::fmt::Debug for Start<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Start")
            .field("resource", &self.resource)
            .field("local_identity", &self.local_identity)
            .field("contact", &self.contact)
            .field("target", &self.target)
            .field("expires", &self.expires)
            .field("body_len", &self.body.len())
            .field("content_type", &self.content_type)
            .field("has_credentials", &self.credentials.is_some())
            .field("call_id", &self.call_id)
            .field("from_tag", &self.from_tag)
            .field("initial_cseq", &self.initial_cseq)
            .finish_non_exhaustive()
    }
}

/// Opaque local identity of one subscription intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

impl SubscriptionId {
    /// Stable numeric value for logs and application maps.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// A fired timer name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    /// Initial/refresh/unsubscribe NOTIFY wait.
    N,
    /// Current finite subscription lifetime.
    Expiry,
    /// Scheduled refresh.
    Refresh,
    /// Package-directed retry.
    Retry,
}

/// Public lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// Waiting for the first matching NOTIFY.
    NotifyWait,
    /// An active subscription.
    Active,
    /// A pending subscription.
    Pending,
    /// Waiting for the terminal NOTIFY after Expires 0.
    Unsubscribing,
}

/// Typed terminal outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Termination {
    /// Timer N fired before the required NOTIFY.
    NoInitialNotify,
    /// The finite expiry fired locally.
    LocalExpiry,
    /// A local request sequence could not be incremented safely.
    LocalCSeqExhausted,
    /// A successful response carried an invalid interval.
    InvalidExpiry,
    /// A successful response did not identify the selected dialog.
    MalformedResponse,
    /// A 423 could not be followed safely.
    IntervalRejected,
    /// Authentication could not be completed under the configured bound.
    AuthenticationExhausted,
    /// A final response rejected the logical operation.
    Rejected(u16),
    /// The transaction ended without a final response.
    TransactionFailed,
    /// The notifier supplied a terminal framework reason.
    Remote(Option<Reason>),
    /// Unsubscribe ended without peer confirmation.
    UnsubscribeUnconfirmed(Box<Termination>),
    /// The global shutdown deadline released the usage.
    Shutdown,
}

/// Observable lifecycle facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateChange {
    /// The subscription entered a framework state.
    State(Lifecycle),
    /// A response conflicted with a usage already established by NOTIFY.
    ConflictingSubscribeResponse,
    /// Timer N ended a refresh attempt; the prior authoritative expiry remains unchanged.
    RefreshUnconfirmed,
    /// A terminal state released the subscription.
    Terminated(Termination),
}

/// Framework metadata delivered beside a package value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationMeta {
    /// Parsed Subscription-State.
    pub subscription: Subscription,
    /// Remote NOTIFY `CSeq`.
    pub remote_cseq: u32,
}

/// One pure action for the I/O-facing driver.
#[derive(Debug)]
pub enum Output<V> {
    /// Send a complete SUBSCRIBE through a real client transaction.
    SendSubscribe {
        /// Logical owner.
        id: SubscriptionId,
        /// Complete request apart from transport-owned Via.
        request: Box<Request>,
        /// Selected peer.
        target: Peer,
    },
    /// Answer an inbound NOTIFY server transaction.
    RespondNotify {
        /// Driver's server-transaction token.
        transaction: u64,
        /// Final status.
        status: u16,
        /// Retry-After for bounded delivery backpressure.
        retry_after: Option<Duration>,
    },
    /// Deliver one parsed package value.
    Deliver {
        /// Logical owner.
        id: SubscriptionId,
        /// Framework metadata; absent only for the initial neutral value.
        metadata: Option<NotificationMeta>,
        /// Package value.
        value: V,
    },
    /// Arm or replace one timer generation.
    ArmTimer {
        /// Logical owner.
        id: SubscriptionId,
        /// Timer kind.
        timer: Timer,
        /// New generation.
        generation: u64,
        /// Relative duration.
        after: Duration,
    },
    /// Cancel one timer generation.
    CancelTimer {
        /// Logical owner.
        id: SubscriptionId,
        /// Timer kind.
        timer: Timer,
        /// Generation made stale.
        generation: u64,
    },
    /// Surface a typed state fact.
    StateChanged {
        /// Logical owner.
        id: SubscriptionId,
        /// New fact.
        change: StateChange,
    },
    /// Shutdown released every owned resource.
    Stopped,
}

/// Failure before an initial request exists.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartError {
    /// A configured maximum was zero or unrepresentable.
    #[error("invalid event-client configuration")]
    InvalidConfiguration,
    /// The client already owns its configured number of subscriptions.
    #[error("event-client capacity exceeded")]
    CapacityExceeded,
    /// The request body exceeds its configured maximum.
    #[error("SUBSCRIBE body exceeds configured maximum")]
    BodyTooLarge,
    /// An identity, interval, package or request header is invalid.
    #[error("invalid subscription start")]
    InvalidStart,
    /// A SIP request could not be constructed.
    #[error("could not build SUBSCRIBE")]
    Build,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationKind {
    Initial,
    Refresh,
    Unsubscribe,
}

struct Operation {
    kind: OperationKind,
    attempted: Duration,
    request: Request,
    auth_retries: u8,
    interval_retries: u8,
    notify_expiry: bool,
}

struct Dialog {
    remote_tag: Vec<u8>,
    local: String,
    remote: String,
    remote_target: Uri,
    route_set: Vec<RouteHop>,
    peer: Peer,
    remote_cseq: u32,
}

struct RouteHop {
    uri: Uri,
    wire: String,
}

#[derive(Default)]
struct Timers {
    n: Option<u64>,
    expiry: Option<u64>,
    refresh: Option<u64>,
    retry: Option<u64>,
    next: u64,
}

struct Entry<C> {
    lifecycle: Lifecycle,
    contact: String,
    target: Peer,
    desired: Duration,
    body: Bytes,
    credentials: Option<Credentials>,
    call_id: String,
    from_tag: String,
    local_cseq: u32,
    event: String,
    event_id: Option<String>,
    accepts: Vec<String>,
    consumer: C,
    trust: Arc<dyn NotifyTrustPolicy>,
    response_tag: Option<Vec<u8>>,
    response_expiry: Option<Duration>,
    dialog: Option<Dialog>,
    operation: Option<Operation>,
    pending_unsubscribe: bool,
    timers: Timers,
    queued: usize,
}

/// Reusable sans-I/O event subscriber.
pub struct EventClient<C: PackageConsumer> {
    config: Config,
    entries: HashMap<SubscriptionId, Entry<C>>,
    next_id: u64,
    shutting_down: bool,
}

/// Outputs produced while allocating one new subscription.
pub type Started<V> = (SubscriptionId, Vec<Output<V>>);

impl<C: PackageConsumer> std::fmt::Debug for EventClient<C> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EventClient")
            .field("config", &self.config)
            .field("active", &self.entries.len())
            .field("next_id", &self.next_id)
            .field("shutting_down", &self.shutting_down)
            .finish()
    }
}

impl<C: PackageConsumer> EventClient<C> {
    /// Construct a bounded client.
    pub fn new(config: Config) -> Result<Self, StartError> {
        config.validate()?;
        Ok(Self {
            config,
            entries: HashMap::new(),
            next_id: 1,
            shutting_down: false,
        })
    }

    /// Number of logical intents currently owned.
    #[must_use]
    pub fn active(&self) -> usize {
        self.entries.len()
    }

    /// Whether a subscription is still owned.
    #[must_use]
    pub fn contains(&self, id: SubscriptionId) -> bool {
        self.entries.contains_key(&id)
    }

    /// Begin one subscription and return its ordered initial outputs.
    pub fn start(&mut self, start: Start<C>) -> Result<Started<C::Value>, StartError> {
        if self.shutting_down || self.entries.len() >= self.config.capacity {
            return Err(StartError::CapacityExceeded);
        }
        if start.body.len() > self.config.subscribe_body_limit
            || start.expires.is_zero()
            || start.expires > self.config.maximum_expiry
            || start.expires.as_secs() > u64::from(u32::MAX)
            || start.initial_cseq == 0
            || start.call_id.trim().is_empty()
            || start.from_tag.trim().is_empty()
            || start.consumer.event().trim().is_empty()
        {
            return Err(if start.body.len() > self.config.subscribe_body_limit {
                StartError::BodyTooLarge
            } else {
                StartError::InvalidStart
            });
        }
        let id = SubscriptionId(self.next_id);
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(StartError::CapacityExceeded)?;
        let event = start.consumer.event().to_owned();
        let event_id = start.consumer.event_id().map(str::to_owned);
        let accepts = start.consumer.accept().to_vec();
        let request = build_initial(&start, &event, event_id.as_deref())?;
        let mut entry = Entry {
            lifecycle: Lifecycle::NotifyWait,
            contact: start.contact,
            target: start.target,
            desired: start.expires,
            body: start.body,
            credentials: start.credentials,
            call_id: start.call_id,
            from_tag: start.from_tag,
            local_cseq: start.initial_cseq,
            event,
            event_id,
            accepts,
            consumer: start.consumer,
            trust: start.trust,
            response_tag: None,
            response_expiry: None,
            dialog: None,
            operation: Some(Operation {
                kind: OperationKind::Initial,
                attempted: start.expires,
                request: request.clone(),
                auth_retries: 0,
                interval_retries: 0,
                notify_expiry: false,
            }),
            pending_unsubscribe: false,
            timers: Timers::default(),
            queued: 0,
        };
        let mut outputs = Vec::new();
        if let Some(value) = entry.consumer.neutral() {
            entry.queued = 1;
            outputs.push(Output::Deliver {
                id,
                metadata: None,
                value,
            });
        }
        outputs.push(Output::SendSubscribe {
            id,
            request: Box::new(request),
            target: entry.target,
        });
        arm(&mut entry, id, Timer::N, self.config.timer_n, &mut outputs);
        arm(&mut entry, id, Timer::Expiry, start.expires, &mut outputs);
        outputs.push(Output::StateChanged {
            id,
            change: StateChange::State(Lifecycle::NotifyWait),
        });
        self.entries.insert(id, entry);
        Ok((id, outputs))
    }

    /// Consume one final SUBSCRIBE response. `cnonce` is fresh driver-supplied entropy for a
    /// possible digest retry.
    #[allow(
        clippy::too_many_lines,
        reason = "the response table is kept in protocol order beside the normative state table"
    )]
    pub fn response(
        &mut self,
        id: SubscriptionId,
        response: Option<&Response>,
        cnonce: &str,
    ) -> Vec<Output<C::Value>> {
        let mut outputs = Vec::new();
        let Some(entry) = self.entries.get_mut(&id) else {
            return outputs;
        };
        let Some(mut operation) = entry.operation.take() else {
            return outputs;
        };

        let Some(response) = response else {
            operation_failure(
                entry,
                id,
                operation.kind,
                Termination::TransactionFailed,
                self.config.timer_n,
                &mut outputs,
            );
            finish_if_terminal(&mut self.entries, id, &outputs);
            return outputs;
        };

        if matches!(response.status.code(), 401 | 407) {
            let from_proxy = response.status.code() == 407;
            let name = if from_proxy {
                HeaderName::ProxyAuthenticate
            } else {
                HeaderName::WwwAuthenticate
            };
            let challenge = strongest(
                response
                    .headers
                    .get_all(&name)
                    .filter_map(|header| Challenge::parse(&header.value(), from_proxy))
                    .collect(),
            );
            if operation.auth_retries >= self.config.authentication_retries
                || challenge.is_none()
                || entry.credentials.is_none()
                || !increment_cseq(entry, id, operation.kind, &mut outputs)
            {
                if !outputs.iter().any(is_terminal::<C::Value>) {
                    operation_failure(
                        entry,
                        id,
                        operation.kind,
                        Termination::AuthenticationExhausted,
                        self.config.timer_n,
                        &mut outputs,
                    );
                }
            } else if let (Some(challenge), Some(credentials)) =
                (challenge, entry.credentials.as_ref())
            {
                operation.auth_retries = operation.auth_retries.saturating_add(1);
                let mut retry = operation.request.clone();
                replace_cseq(&mut retry, entry.local_cseq);
                let uri = String::from_utf8_lossy(&retry.uri.to_bytes()).into_owned();
                let value = respond(
                    &challenge,
                    credentials,
                    "SUBSCRIBE",
                    &uri,
                    u32::from(operation.auth_retries),
                    cnonce,
                );
                retry.headers.remove_all(&challenge.response_header());
                if let Ok(header) = Header::build(challenge.response_header(), Bytes::from(value)) {
                    retry.headers.push(header);
                    operation.request = retry.clone();
                    entry.operation = Some(operation);
                    outputs.push(Output::SendSubscribe {
                        id,
                        request: Box::new(retry),
                        target: request_peer(entry),
                    });
                    arm(entry, id, Timer::N, self.config.timer_n, &mut outputs);
                    return outputs;
                }
                operation_failure(
                    entry,
                    id,
                    operation.kind,
                    Termination::AuthenticationExhausted,
                    self.config.timer_n,
                    &mut outputs,
                );
            }
            finish_if_terminal(&mut self.entries, id, &outputs);
            return outputs;
        }

        if response.status.code() == 423 {
            let minimum = strict_scalar(response, &HeaderName::MinExpires);
            let valid = operation.kind != OperationKind::Unsubscribe
                && operation.interval_retries < self.config.interval_retries
                && minimum.is_some_and(|value| {
                    value > operation.attempted
                        && value <= self.config.maximum_expiry
                        && u32::try_from(value.as_secs()).is_ok()
                });
            if valid
                && increment_cseq(entry, id, operation.kind, &mut outputs)
                && let Some(minimum) = minimum
            {
                operation.interval_retries = operation.interval_retries.saturating_add(1);
                operation.attempted = minimum;
                let mut retry = operation.request.clone();
                replace_cseq(&mut retry, entry.local_cseq);
                replace_duration_header(&mut retry, HeaderName::Expires, minimum);
                operation.request = retry.clone();
                entry.operation = Some(operation);
                outputs.push(Output::SendSubscribe {
                    id,
                    request: Box::new(retry),
                    target: request_peer(entry),
                });
                arm(entry, id, Timer::N, self.config.timer_n, &mut outputs);
                arm(entry, id, Timer::Expiry, minimum, &mut outputs);
                return outputs;
            }
            if !outputs.iter().any(is_terminal::<C::Value>) {
                operation_failure(
                    entry,
                    id,
                    operation.kind,
                    Termination::IntervalRejected,
                    self.config.timer_n,
                    &mut outputs,
                );
            }
            finish_if_terminal(&mut self.entries, id, &outputs);
            return outputs;
        }

        if response.status.is_success() {
            let granted = strict_expires(response);
            let valid_interval = match operation.kind {
                OperationKind::Initial | OperationKind::Refresh => {
                    granted.is_some_and(|value| !value.is_zero() && value <= operation.attempted)
                }
                OperationKind::Unsubscribe => granted == Some(Duration::ZERO),
            };
            let tag = response_tag(response);
            let valid_dialog = match operation.kind {
                OperationKind::Initial => entry.dialog.is_some() || tag.is_some(),
                OperationKind::Refresh | OperationKind::Unsubscribe => {
                    entry.dialog.as_ref().is_some_and(|dialog| {
                        tag.as_ref().is_some_and(|tag| {
                            tag.eq_ignore_ascii_case(dialog.remote_tag.as_slice())
                        })
                    })
                }
            };
            if !valid_interval {
                operation_failure(
                    entry,
                    id,
                    operation.kind,
                    Termination::InvalidExpiry,
                    self.config.timer_n,
                    &mut outputs,
                );
            } else if !valid_dialog {
                operation_failure(
                    entry,
                    id,
                    operation.kind,
                    Termination::MalformedResponse,
                    self.config.timer_n,
                    &mut outputs,
                );
            } else if let Some(granted) = granted {
                if operation.kind == OperationKind::Unsubscribe {
                    entry.operation = None;
                } else {
                    if operation.kind == OperationKind::Initial && entry.dialog.is_none() {
                        entry.response_tag = tag;
                    } else if operation.kind == OperationKind::Refresh {
                        refresh_dialog_from_response(entry, response);
                    }
                    if !operation.notify_expiry {
                        entry.response_expiry = Some(granted);
                        arm(entry, id, Timer::Expiry, granted, &mut outputs);
                        if matches!(entry.lifecycle, Lifecycle::Active | Lifecycle::Pending) {
                            arm_refresh(entry, id, granted, &mut outputs);
                        }
                    }
                    entry.operation = None;
                    maybe_begin_pending_unsubscribe(entry, id, self.config.timer_n, &mut outputs);
                }
            }
            finish_if_terminal(&mut self.entries, id, &outputs);
            return outputs;
        }

        if operation.kind == OperationKind::Refresh && fatal_refresh_status(response.status.code())
        {
            terminate(
                entry,
                id,
                Termination::Rejected(response.status.code()),
                &mut outputs,
            );
        } else {
            operation_failure(
                entry,
                id,
                operation.kind,
                Termination::Rejected(response.status.code()),
                self.config.timer_n,
                &mut outputs,
            );
        }
        finish_if_terminal(&mut self.entries, id, &outputs);
        outputs
    }

    /// Consume and answer one NOTIFY. The driver token is returned unchanged in
    /// [`Output::RespondNotify`].
    #[allow(
        clippy::too_many_lines,
        reason = "the fail-closed NOTIFY validation order mirrors the normative decision table"
    )]
    pub fn notify(
        &mut self,
        transaction: u64,
        request: &Request,
        source: Peer,
    ) -> Vec<Output<C::Value>> {
        let mut outputs = Vec::new();
        let Some((id, entry)) = self.entries.iter_mut().find(|(_, entry)| {
            call_id(request).is_some_and(|call_id| call_id == entry.call_id.as_bytes())
                && tag::<To>(request).is_some_and(|tag| tag == entry.from_tag.as_bytes())
        }) else {
            outputs.push(respond_notify(transaction, 481, None));
            return outputs;
        };
        let id = *id;

        if request.method != Method::Notify
            || request.headers.count(&HeaderName::CallId) != 1
            || request.headers.count(&HeaderName::From) != 1
            || request.headers.count(&HeaderName::To) != 1
        {
            outputs.push(respond_notify(transaction, 400, None));
            return outputs;
        }
        let Some((event, event_id)) = parse_event(request) else {
            outputs.push(respond_notify(transaction, 489, None));
            return outputs;
        };
        if !event.eq_ignore_ascii_case(&entry.event)
            || event_id.as_deref() != entry.event_id.as_deref()
        {
            outputs.push(respond_notify(transaction, 489, None));
            return outputs;
        }
        let Some(remote_tag) = tag::<FromHeader>(request) else {
            outputs.push(respond_notify(transaction, 400, None));
            return outputs;
        };
        if entry.dialog.as_ref().is_some_and(|dialog| {
            !dialog
                .remote_tag
                .eq_ignore_ascii_case(remote_tag.as_slice())
        }) || (entry.dialog.is_none()
            && entry
                .response_tag
                .as_ref()
                .is_some_and(|candidate| !candidate.eq_ignore_ascii_case(remote_tag.as_slice())))
        {
            outputs.push(respond_notify(transaction, 481, None));
            return outputs;
        }
        if !entry.trust.accepts(request_peer(entry), source, request) {
            outputs.push(respond_notify(transaction, 403, None));
            return outputs;
        }
        let cseq = if request.headers.count(&HeaderName::CSeq) == 1 {
            if let Some(Ok(CSeq {
                sequence,
                method: Method::Notify,
            })) = request.headers.typed::<CSeq>()
            {
                sequence
            } else {
                outputs.push(respond_notify(transaction, 400, None));
                return outputs;
            }
        } else {
            outputs.push(respond_notify(transaction, 400, None));
            return outputs;
        };
        if entry
            .dialog
            .as_ref()
            .is_some_and(|dialog| cseq <= dialog.remote_cseq)
        {
            outputs.push(respond_notify(transaction, 500, None));
            return outputs;
        }
        let contacts: Vec<_> = request.headers.typed_all::<Contact>().collect();
        let [Ok(contact)] = contacts.as_slice() else {
            outputs.push(respond_notify(transaction, 400, None));
            return outputs;
        };
        let state = if request.headers.count(&HeaderName::SubscriptionState) == 1 {
            request
                .headers
                .value(&HeaderName::SubscriptionState)
                .and_then(|value| Subscription::parse(&value))
        } else {
            None
        };
        let Some(state) = state else {
            outputs.push(respond_notify(transaction, 400, None));
            return outputs;
        };
        if request.body().len() > self.config.notify_body_limit {
            outputs.push(respond_notify(transaction, 413, None));
            return outputs;
        }
        let should_consume = !request.body().is_empty()
            || state.state != State::Terminated
            || !entry.consumer.empty_terminal_is_valid();
        if should_consume && entry.queued >= self.config.delivery_capacity {
            outputs.push(respond_notify(
                transaction,
                503,
                Some(Duration::from_secs(1)),
            ));
            return outputs;
        }
        let value = if should_consume {
            let content_type = request.headers.value(&HeaderName::ContentType);
            match entry
                .consumer
                .consume(content_type.as_deref(), request.body())
            {
                Ok(value) => Some(value),
                Err(rejection) => {
                    outputs.push(respond_notify(transaction, rejection.status, None));
                    return outputs;
                }
            }
        } else {
            None
        };

        let remote = request
            .headers
            .value(&HeaderName::From)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        let local = request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .unwrap_or_default();
        let routes: Result<Vec<_>, _> = request
            .headers
            .typed_all::<RecordRoute>()
            .map(|route| route.map(|route| route_hop(&route)))
            .collect();
        let Ok(routes) = routes else {
            outputs.push(respond_notify(transaction, 400, None));
            return outputs;
        };
        match entry.dialog.as_mut() {
            Some(dialog) => {
                dialog.remote_target = contact.uri.clone();
                dialog.peer = peer_for_uri(&contact.uri, source);
                dialog.remote_cseq = cseq;
            }
            None => {
                entry.dialog = Some(Dialog {
                    remote_tag,
                    local,
                    remote,
                    remote_target: contact.uri.clone(),
                    route_set: routes,
                    peer: peer_for_uri(&contact.uri, source),
                    remote_cseq: cseq,
                });
            }
        }
        outputs.push(respond_notify(transaction, 200, None));
        cancel(entry, id, Timer::N, &mut outputs);

        if let Some(operation) = entry.operation.as_mut()
            && state.expires.is_some()
        {
            operation.notify_expiry = true;
        }
        match state.state {
            State::Active | State::Pending => {
                entry.lifecycle = if state.state == State::Active {
                    Lifecycle::Active
                } else {
                    Lifecycle::Pending
                };
                if let Some(expires) = state.expires {
                    arm(entry, id, Timer::Expiry, expires, &mut outputs);
                    arm_refresh(entry, id, expires, &mut outputs);
                } else if let Some(expires) = entry.response_expiry {
                    arm_refresh(entry, id, expires, &mut outputs);
                }
                outputs.push(Output::StateChanged {
                    id,
                    change: StateChange::State(entry.lifecycle),
                });
                if let Some(value) = value {
                    entry.queued = entry.queued.saturating_add(1);
                    outputs.push(Output::Deliver {
                        id,
                        metadata: Some(NotificationMeta {
                            subscription: state,
                            remote_cseq: cseq,
                        }),
                        value,
                    });
                }
                maybe_begin_pending_unsubscribe(entry, id, self.config.timer_n, &mut outputs);
            }
            State::Terminated => {
                cancel_all(entry, id, &mut outputs);
                if let Some(value) = value {
                    outputs.push(Output::Deliver {
                        id,
                        metadata: Some(NotificationMeta {
                            subscription: state.clone(),
                            remote_cseq: cseq,
                        }),
                        value,
                    });
                }
                outputs.push(Output::StateChanged {
                    id,
                    change: StateChange::Terminated(Termination::Remote(state.reason)),
                });
            }
        }
        finish_if_terminal(&mut self.entries, id, &outputs);
        outputs
    }

    /// Report application deliveries removed from the bounded queue.
    pub fn consumer_drained(&mut self, id: SubscriptionId, count: usize) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.queued = entry.queued.saturating_sub(count);
        }
    }

    /// Fire one exact timer generation.
    pub fn timer_fired(
        &mut self,
        id: SubscriptionId,
        timer: Timer,
        generation: u64,
    ) -> Vec<Output<C::Value>> {
        let mut outputs = Vec::new();
        let Some(entry) = self.entries.get_mut(&id) else {
            return outputs;
        };
        if timer_generation(&entry.timers, timer) != Some(generation) {
            return outputs;
        }
        set_timer(&mut entry.timers, timer, None);
        match timer {
            Timer::N => match entry.lifecycle {
                Lifecycle::NotifyWait => {
                    terminate(entry, id, Termination::NoInitialNotify, &mut outputs);
                }
                Lifecycle::Unsubscribing => terminate(
                    entry,
                    id,
                    Termination::UnsubscribeUnconfirmed(Box::new(Termination::NoInitialNotify)),
                    &mut outputs,
                ),
                Lifecycle::Active | Lifecycle::Pending => {
                    entry.operation = None;
                    outputs.push(Output::StateChanged {
                        id,
                        change: StateChange::RefreshUnconfirmed,
                    });
                    maybe_begin_pending_unsubscribe(entry, id, self.config.timer_n, &mut outputs);
                }
            },
            Timer::Expiry => terminate(entry, id, Termination::LocalExpiry, &mut outputs),
            Timer::Refresh => {
                if !self.shutting_down
                    && entry.operation.is_none()
                    && increment_cseq(entry, id, OperationKind::Refresh, &mut outputs)
                {
                    match build_in_dialog(entry, entry.desired) {
                        Ok(request) => {
                            entry.operation = Some(Operation {
                                kind: OperationKind::Refresh,
                                attempted: entry.desired,
                                request: request.clone(),
                                auth_retries: 0,
                                interval_retries: 0,
                                notify_expiry: false,
                            });
                            outputs.push(Output::SendSubscribe {
                                id,
                                request: Box::new(request),
                                target: request_peer(entry),
                            });
                            arm(entry, id, Timer::N, self.config.timer_n, &mut outputs);
                        }
                        Err(()) => {
                            terminate(entry, id, Termination::TransactionFailed, &mut outputs);
                        }
                    }
                }
            }
            Timer::Retry => {}
        }
        finish_if_terminal(&mut self.entries, id, &outputs);
        outputs
    }

    /// Request an in-dialog Expires 0 operation.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> Vec<Output<C::Value>> {
        let mut outputs = Vec::new();
        let Some(entry) = self.entries.get_mut(&id) else {
            return outputs;
        };
        cancel(entry, id, Timer::Refresh, &mut outputs);
        cancel(entry, id, Timer::Retry, &mut outputs);
        if entry.operation.is_some() {
            entry.pending_unsubscribe = true;
        } else if entry.dialog.is_none() {
            terminate(
                entry,
                id,
                Termination::UnsubscribeUnconfirmed(Box::new(Termination::TransactionFailed)),
                &mut outputs,
            );
        } else {
            entry.pending_unsubscribe = true;
            maybe_begin_pending_unsubscribe(entry, id, self.config.timer_n, &mut outputs);
        }
        finish_if_terminal(&mut self.entries, id, &outputs);
        outputs
    }

    /// Close admission and begin one bounded unsubscribe per live usage.
    pub fn shutdown(&mut self) -> Vec<Output<C::Value>> {
        self.shutting_down = true;
        let ids: Vec<_> = self.entries.keys().copied().collect();
        let mut outputs = Vec::new();
        for id in ids {
            outputs.extend(self.unsubscribe(id));
        }
        if self.entries.is_empty() {
            outputs.push(Output::Stopped);
        }
        outputs
    }

    /// Force the global shutdown deadline and release all state.
    pub fn shutdown_deadline(&mut self) -> Vec<Output<C::Value>> {
        let mut outputs = Vec::new();
        for (id, mut entry) in self.entries.drain() {
            cancel_all(&mut entry, id, &mut outputs);
            outputs.push(Output::StateChanged {
                id,
                change: StateChange::Terminated(Termination::Shutdown),
            });
        }
        outputs.push(Output::Stopped);
        outputs
    }
}

fn build_initial<C: PackageConsumer>(
    start: &Start<C>,
    event: &str,
    event_id: Option<&str>,
) -> Result<Request, StartError> {
    let event = event_value(event, event_id);
    let mut builder = RequestBuilder::new(Method::Subscribe, start.resource.clone())
        .header(HeaderName::To, Bytes::from(format!("<{}>", start.resource)))
        .map_err(|_| StartError::Build)?
        .header(
            HeaderName::From,
            Bytes::from(format!("{};tag={}", start.local_identity, start.from_tag)),
        )
        .map_err(|_| StartError::Build)?
        .header(HeaderName::CallId, Bytes::from(start.call_id.clone()))
        .map_err(|_| StartError::Build)?
        .cseq(start.initial_cseq, &Method::Subscribe)
        .map_err(|_| StartError::Build)?
        .header(HeaderName::Contact, Bytes::from(start.contact.clone()))
        .map_err(|_| StartError::Build)?
        .header(HeaderName::Event, Bytes::from(event))
        .map_err(|_| StartError::Build)?
        .header(
            HeaderName::Expires,
            Bytes::from(start.expires.as_secs().to_string()),
        )
        .map_err(|_| StartError::Build)?
        .max_forwards(70);
    if !start.consumer.accept().is_empty() {
        builder = builder
            .header(
                HeaderName::Accept,
                Bytes::from(start.consumer.accept().join(", ")),
            )
            .map_err(|_| StartError::Build)?;
    }
    if let Some(content_type) = &start.content_type {
        builder = builder
            .header(HeaderName::ContentType, Bytes::from(content_type.clone()))
            .map_err(|_| StartError::Build)?;
    }
    Ok(builder.body(start.body.clone()).build())
}

fn build_in_dialog<C: PackageConsumer>(entry: &Entry<C>, expires: Duration) -> Result<Request, ()> {
    let dialog = entry.dialog.as_ref().ok_or(())?;
    let event = event_value(&entry.event, entry.event_id.as_deref());
    let (request_uri, routes) = dialog_request_target(dialog);
    let mut builder = RequestBuilder::new(Method::Subscribe, request_uri)
        .header(HeaderName::To, Bytes::from(dialog.remote.clone()))
        .map_err(|_| ())?
        .header(HeaderName::From, Bytes::from(dialog.local.clone()))
        .map_err(|_| ())?
        .header(HeaderName::CallId, Bytes::from(entry.call_id.clone()))
        .map_err(|_| ())?
        .cseq(entry.local_cseq, &Method::Subscribe)
        .map_err(|_| ())?
        .header(HeaderName::Contact, Bytes::from(entry.contact.clone()))
        .map_err(|_| ())?
        .header(HeaderName::Event, Bytes::from(event))
        .map_err(|_| ())?
        .header(
            HeaderName::Expires,
            Bytes::from(expires.as_secs().to_string()),
        )
        .map_err(|_| ())?
        .max_forwards(70);
    if !entry.accepts.is_empty() {
        builder = builder
            .header(HeaderName::Accept, Bytes::from(entry.accepts.join(", ")))
            .map_err(|_| ())?;
    }
    for route in routes {
        builder = builder
            .header(HeaderName::Route, Bytes::from(route))
            .map_err(|_| ())?;
    }
    Ok(builder.body(entry.body.clone()).build())
}

fn route_hop(address: &Address) -> RouteHop {
    let mut wire = format!("<{}>", address.uri);
    for parameter in &address.params {
        wire.push(';');
        wire.push_str(&String::from_utf8_lossy(&parameter.name));
        if let Some(value) = &parameter.value {
            wire.push('=');
            wire.push_str(&String::from_utf8_lossy(value));
        }
    }
    RouteHop {
        uri: address.uri.clone(),
        wire,
    }
}

fn dialog_request_target(dialog: &Dialog) -> (Uri, Vec<String>) {
    let Some(first) = dialog.route_set.first() else {
        return (dialog.remote_target.clone(), Vec::new());
    };
    if first
        .uri
        .params()
        .is_some_and(|parameters| parameters.contains("lr"))
    {
        return (
            dialog.remote_target.clone(),
            dialog
                .route_set
                .iter()
                .map(|route| route.wire.clone())
                .collect(),
        );
    }
    let mut routes: Vec<_> = dialog
        .route_set
        .iter()
        .skip(1)
        .map(|route| route.wire.clone())
        .collect();
    routes.push(format!("<{}>", dialog.remote_target));
    (first.uri.clone(), routes)
}

fn fatal_refresh_status(status: u16) -> bool {
    matches!(
        status,
        404 | 405 | 410 | 416 | 480 | 481 | 482 | 483 | 484 | 485 | 489 | 501 | 604
    )
}

fn event_value(event: &str, id: Option<&str>) -> String {
    id.map_or_else(|| event.to_owned(), |id| format!("{event};id={id}"))
}

fn replace_cseq(request: &mut Request, sequence: u32) {
    request.headers.remove_all(&HeaderName::CSeq);
    if let Ok(header) = Header::build(
        HeaderName::CSeq,
        Bytes::from(format!("{sequence} SUBSCRIBE")),
    ) {
        request.headers.push(header);
    }
}

fn replace_duration_header(request: &mut Request, name: HeaderName, value: Duration) {
    request.headers.remove_all(&name);
    if let Ok(header) = Header::build(name, Bytes::from(value.as_secs().to_string())) {
        request.headers.push(header);
    }
}

fn parse_event(request: &Request) -> Option<(String, Option<String>)> {
    if request.headers.count(&HeaderName::Event) != 1 {
        return None;
    }
    let value = request.headers.value(&HeaderName::Event)?;
    let text = std::str::from_utf8(&value).ok()?;
    let mut parts = text.split(';');
    let event = parts.next()?.trim();
    if event.is_empty() {
        return None;
    }
    let mut id = None;
    for parameter in parts {
        let Some((name, value)) = parameter.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("id") {
            if id.is_some() {
                return None;
            }
            id = Some(value.trim().trim_matches('"').to_owned());
        }
    }
    Some((event.to_owned(), id))
}

fn call_id(request: &Request) -> Option<std::borrow::Cow<'_, [u8]>> {
    (request.headers.count(&HeaderName::CallId) == 1)
        .then(|| request.headers.value(&HeaderName::CallId))
        .flatten()
}

fn tag<H>(request: &Request) -> Option<Vec<u8>>
where
    H: sipx_sip::TypedHeader + std::ops::Deref<Target = sipx_sip::Address>,
{
    if request.headers.count(&H::NAME) != 1 {
        return None;
    }
    request
        .headers
        .typed::<H>()?
        .ok()?
        .tag()
        .map(<[u8]>::to_vec)
}

fn response_tag(response: &Response) -> Option<Vec<u8>> {
    if response.headers.count(&HeaderName::To) != 1 {
        return None;
    }
    response
        .headers
        .typed::<To>()?
        .ok()?
        .tag()
        .map(<[u8]>::to_vec)
}

fn strict_expires(response: &Response) -> Option<Duration> {
    if response.headers.count(&HeaderName::Expires) != 1 {
        return None;
    }
    response
        .headers
        .typed::<Expires>()?
        .ok()
        .map(|value| Duration::from_secs(u64::from(value.0)))
}

fn strict_scalar(response: &Response, name: &HeaderName) -> Option<Duration> {
    if response.headers.count(name) != 1 {
        return None;
    }
    let value = response.headers.value(name)?;
    let seconds = std::str::from_utf8(&value)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    Some(Duration::from_secs(u64::from(seconds)))
}

fn request_peer<C>(entry: &Entry<C>) -> Peer {
    entry
        .dialog
        .as_ref()
        .map_or(entry.target, |dialog| dialog.peer)
}

fn refresh_dialog_from_response<C>(entry: &mut Entry<C>, response: &Response) {
    let contacts: Vec<_> = response.headers.typed_all::<Contact>().collect();
    let [Ok(contact)] = contacts.as_slice() else {
        return;
    };
    if let Some(dialog) = entry.dialog.as_mut() {
        dialog.remote_target = contact.uri.clone();
        dialog.peer = peer_for_uri(&contact.uri, dialog.peer);
    }
}

fn peer_for_uri(uri: &Uri, fallback: Peer) -> Peer {
    let Some(sipx_sip::Host::Ip(ip)) = uri.host() else {
        return fallback;
    };
    Peer {
        address: SocketAddr::new(*ip, uri.port().unwrap_or(5060)),
        ..fallback
    }
}

fn respond_notify<V>(transaction: u64, status: u16, retry_after: Option<Duration>) -> Output<V> {
    Output::RespondNotify {
        transaction,
        status,
        retry_after,
    }
}

fn timer_slot(timers: &mut Timers, timer: Timer) -> &mut Option<u64> {
    match timer {
        Timer::N => &mut timers.n,
        Timer::Expiry => &mut timers.expiry,
        Timer::Refresh => &mut timers.refresh,
        Timer::Retry => &mut timers.retry,
    }
}

fn timer_generation(timers: &Timers, timer: Timer) -> Option<u64> {
    match timer {
        Timer::N => timers.n,
        Timer::Expiry => timers.expiry,
        Timer::Refresh => timers.refresh,
        Timer::Retry => timers.retry,
    }
}

fn set_timer(timers: &mut Timers, timer: Timer, generation: Option<u64>) {
    *timer_slot(timers, timer) = generation;
}

fn arm<C, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    timer: Timer,
    after: Duration,
    outputs: &mut Vec<Output<V>>,
) {
    entry.timers.next = entry.timers.next.saturating_add(1);
    let generation = entry.timers.next;
    set_timer(&mut entry.timers, timer, Some(generation));
    outputs.push(Output::ArmTimer {
        id,
        timer,
        generation,
        after,
    });
}

fn cancel<C, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    timer: Timer,
    outputs: &mut Vec<Output<V>>,
) {
    if let Some(generation) = timer_generation(&entry.timers, timer) {
        set_timer(&mut entry.timers, timer, None);
        entry.timers.next = entry.timers.next.saturating_add(1);
        outputs.push(Output::CancelTimer {
            id,
            timer,
            generation,
        });
    }
}

fn cancel_all<C, V>(entry: &mut Entry<C>, id: SubscriptionId, outputs: &mut Vec<Output<V>>) {
    for timer in [Timer::N, Timer::Expiry, Timer::Refresh, Timer::Retry] {
        cancel(entry, id, timer, outputs);
    }
}

fn arm_refresh<C, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    expires: Duration,
    outputs: &mut Vec<Output<V>>,
) {
    let seconds = expires.as_secs();
    let refresh = if seconds <= 1 {
        Duration::ZERO
    } else {
        Duration::from_secs((seconds.saturating_mul(4) / 5).clamp(1, seconds - 1))
    };
    arm(entry, id, Timer::Refresh, refresh, outputs);
}

fn increment_cseq<C, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    kind: OperationKind,
    outputs: &mut Vec<Output<V>>,
) -> bool {
    let Some(next) = entry.local_cseq.checked_add(1) else {
        let reason = if kind == OperationKind::Unsubscribe {
            Termination::UnsubscribeUnconfirmed(Box::new(Termination::LocalCSeqExhausted))
        } else {
            Termination::LocalCSeqExhausted
        };
        terminate(entry, id, reason, outputs);
        return false;
    };
    entry.local_cseq = next;
    true
}

fn operation_failure<C: PackageConsumer, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    kind: OperationKind,
    reason: Termination,
    timer_n: Duration,
    outputs: &mut Vec<Output<V>>,
) {
    match entry.lifecycle {
        Lifecycle::NotifyWait => terminate(entry, id, reason, outputs),
        Lifecycle::Active | Lifecycle::Pending => {
            cancel(entry, id, Timer::N, outputs);
            outputs.push(Output::StateChanged {
                id,
                change: if kind == OperationKind::Refresh {
                    StateChange::RefreshUnconfirmed
                } else {
                    StateChange::ConflictingSubscribeResponse
                },
            });
            entry.operation = None;
            maybe_begin_pending_unsubscribe(entry, id, timer_n, outputs);
        }
        Lifecycle::Unsubscribing => terminate(
            entry,
            id,
            Termination::UnsubscribeUnconfirmed(Box::new(reason)),
            outputs,
        ),
    }
}

fn maybe_begin_pending_unsubscribe<C: PackageConsumer, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    timer_n: Duration,
    outputs: &mut Vec<Output<V>>,
) {
    if !entry.pending_unsubscribe || entry.operation.is_some() || entry.dialog.is_none() {
        return;
    }
    entry.pending_unsubscribe = false;
    if !increment_cseq(entry, id, OperationKind::Unsubscribe, outputs) {
        return;
    }
    match build_in_dialog(entry, Duration::ZERO) {
        Ok(request) => {
            entry.lifecycle = Lifecycle::Unsubscribing;
            entry.operation = Some(Operation {
                kind: OperationKind::Unsubscribe,
                attempted: Duration::ZERO,
                request: request.clone(),
                auth_retries: 0,
                interval_retries: 0,
                notify_expiry: false,
            });
            outputs.push(Output::SendSubscribe {
                id,
                request: Box::new(request),
                target: request_peer(entry),
            });
            arm(entry, id, Timer::N, timer_n, outputs);
            outputs.push(Output::StateChanged {
                id,
                change: StateChange::State(Lifecycle::Unsubscribing),
            });
        }
        Err(()) => terminate(
            entry,
            id,
            Termination::UnsubscribeUnconfirmed(Box::new(Termination::TransactionFailed)),
            outputs,
        ),
    }
}

fn terminate<C, V>(
    entry: &mut Entry<C>,
    id: SubscriptionId,
    reason: Termination,
    outputs: &mut Vec<Output<V>>,
) {
    cancel_all(entry, id, outputs);
    entry.operation = None;
    entry.pending_unsubscribe = false;
    outputs.push(Output::StateChanged {
        id,
        change: StateChange::Terminated(reason),
    });
}

fn is_terminal<V>(output: &Output<V>) -> bool {
    matches!(
        output,
        Output::StateChanged {
            change: StateChange::Terminated(_),
            ..
        }
    )
}

fn finish_if_terminal<C, V>(
    entries: &mut HashMap<SubscriptionId, Entry<C>>,
    id: SubscriptionId,
    outputs: &[Output<V>],
) {
    if outputs.iter().any(is_terminal) {
        entries.remove(&id);
    }
}
