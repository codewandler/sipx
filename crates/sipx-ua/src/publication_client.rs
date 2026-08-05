//! Sans-I/O event-state publisher (RFC 3903).
//!
//! The endpoint driver supplies transactions, fresh digest cnonces and fired timer generations.
//! The normative state table is `docs/specs/publication-endpoint.md`.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::auth::{Challenge, Credentials, respond, strongest};
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::Expires;
use sipx_sip::{Header, HeaderName, Method, Request, Response, Uri};
use thiserror::Error;

use crate::event_client::Peer;

/// Default number of publisher usages owned by one runtime.
pub const DEFAULT_CAPACITY: usize = 1_024;
/// Default maximum retained publication body.
pub const DEFAULT_BODY_LIMIT: usize = 65_536;

/// Bounded publisher policy.
#[derive(Debug, Clone)]
pub struct Config {
    /// Maximum logical publishers in one runtime.
    pub capacity: usize,
    /// Maximum retained request body.
    pub body_limit: usize,
    /// Digest retries per logical operation.
    pub authentication_retries: u8,
    /// 423 retries per logical operation.
    pub interval_retries: u8,
    /// Maximum requested or accepted expiry.
    pub maximum_expiry: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            body_limit: DEFAULT_BODY_LIMIT,
            authentication_retries: 2,
            interval_retries: 1,
            maximum_expiry: Duration::from_secs(u64::from(u32::MAX)),
        }
    }
}

impl Config {
    /// Validate allocation and wire-value bounds.
    pub fn validate(&self) -> Result<(), StartError> {
        if self.capacity == 0
            || self.body_limit == 0
            || self.maximum_expiry.is_zero()
            || self.maximum_expiry.as_secs() > u64::from(u32::MAX)
        {
            return Err(StartError::InvalidConfiguration);
        }
        Ok(())
    }
}

/// Driver-supplied initial publication.
#[derive(Debug)]
pub struct Start {
    /// Published resource and Request-URI.
    pub resource: Uri,
    /// From address without a tag.
    pub local_identity: String,
    /// Selected compositor target.
    pub target: Peer,
    /// Event package token.
    pub event: String,
    /// Positive requested lifetime.
    pub expires: Duration,
    /// Mandatory initial event state.
    pub body: Bytes,
    /// Event-package media type.
    pub content_type: String,
    /// Optional digest credentials.
    pub credentials: Option<Credentials>,
    /// Fresh Call-ID.
    pub call_id: String,
    /// Fresh From tag.
    pub from_tag: String,
    /// Non-zero first `CSeq`.
    pub initial_cseq: u32,
}

/// Failure before an initial PUBLISH exists.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StartError {
    /// A configured bound cannot be used.
    #[error("invalid publisher configuration")]
    InvalidConfiguration,
    /// An identity, package or interval is invalid.
    #[error("invalid publication start")]
    InvalidStart,
    /// The publication body exceeds its configured maximum.
    #[error("publication body exceeds configured maximum")]
    BodyTooLarge,
    /// A SIP request could not be built.
    #[error("could not build PUBLISH")]
    Build,
}

/// Failure to admit an application operation.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandError {
    /// The publisher has already terminated.
    #[error("publication is no longer active")]
    Terminated,
    /// Another new PUBLISH request is still in flight.
    #[error("another publication operation is in flight")]
    Busy,
    /// A replacement body is empty or exceeds the configured maximum.
    #[error("invalid publication body")]
    InvalidBody,
}

/// Publisher timer kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    /// Automatic refresh deadline.
    Refresh,
    /// Last authoritative local expiry.
    Expiry,
}

/// Observable successful state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedState {
    /// Current opaque entity tag.
    pub tag: String,
    /// Current granted lifetime.
    pub expires: Duration,
}

/// Typed terminal outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Termination {
    /// A conditional tag was rejected and was discarded.
    StaleTag,
    /// A successful response omitted or corrupted required authority.
    MalformedResponse,
    /// Authentication retry policy was exhausted.
    AuthenticationExhausted,
    /// Interval negotiation was invalid or exhausted.
    IntervalRejected,
    /// A final response rejected the operation.
    Rejected(u16),
    /// The client transaction ended without a final response.
    TransactionFailed,
    /// Local `CSeq` could not be incremented safely.
    LocalCSeqExhausted,
    /// The last authoritative expiry elapsed.
    LocalExpiry,
    /// A successful remove ended the publication.
    Removed,
    /// The runtime shutdown deadline released the usage.
    Shutdown,
}

/// Observable publisher change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateChange {
    /// A successful operation installed fresh authority.
    Published(PublishedState),
    /// The publisher released all state.
    Terminated(Termination),
}

/// One pure action for an I/O driver.
#[derive(Debug)]
pub enum Output {
    /// Send a complete request apart from transport-owned Via.
    SendPublish {
        /// Request bytes and headers.
        request: Box<Request>,
        /// Selected peer.
        target: Peer,
    },
    /// Arm or replace a timer generation.
    ArmTimer {
        /// Timer kind.
        timer: Timer,
        /// Exact generation.
        generation: u64,
        /// Relative duration.
        after: Duration,
    },
    /// Cancel one timer generation.
    CancelTimer {
        /// Timer kind.
        timer: Timer,
        /// Generation made stale.
        generation: u64,
    },
    /// Surface application state.
    StateChanged(StateChange),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationKind {
    Initial,
    Refresh,
    Modify,
    Remove,
}

struct Operation {
    kind: OperationKind,
    attempted: Duration,
    request: Request,
    auth_retries: u8,
    interval_retries: u8,
}

#[derive(Default)]
struct Timers {
    refresh: Option<u64>,
    expiry: Option<u64>,
    next: u64,
}

/// One sans-I/O publication lifecycle.
pub struct Publisher {
    config: Config,
    resource: Uri,
    local_identity: String,
    target: Peer,
    event: String,
    desired: Duration,
    body: Bytes,
    content_type: String,
    credentials: Option<Credentials>,
    call_id: String,
    from_tag: String,
    cseq: u32,
    tag: Option<String>,
    granted: Option<Duration>,
    operation: Option<Operation>,
    pending_remove: bool,
    timers: Timers,
    active: bool,
}

impl std::fmt::Debug for Publisher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Publisher")
            .field("resource", &self.resource)
            .field("event", &self.event)
            .field("cseq", &self.cseq)
            .field("has_tag", &self.tag.is_some())
            .field("granted", &self.granted)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl Publisher {
    /// Start one initial publication.
    pub fn start(config: Config, start: Start) -> Result<(Self, Vec<Output>), StartError> {
        config.validate()?;
        if start.body.len() > config.body_limit {
            return Err(StartError::BodyTooLarge);
        }
        if start.body.is_empty()
            || start.event.trim().is_empty()
            || !token(start.event.as_bytes())
            || start.content_type.trim().is_empty()
            || start.local_identity.trim().is_empty()
            || start.call_id.trim().is_empty()
            || start.from_tag.trim().is_empty()
            || !token(start.from_tag.as_bytes())
            || start.initial_cseq == 0
            || start.expires.is_zero()
            || start.expires > config.maximum_expiry
            || start.expires.as_secs() > u64::from(u32::MAX)
        {
            return Err(StartError::InvalidStart);
        }
        let request = build_request(
            &start.resource,
            &start.local_identity,
            &start.call_id,
            &start.from_tag,
            start.initial_cseq,
            &start.event,
            start.expires,
            None,
            Some((&start.content_type, start.body.clone())),
        )?;
        let output = Output::SendPublish {
            request: Box::new(request.clone()),
            target: start.target,
        };
        Ok((
            Self {
                config,
                resource: start.resource,
                local_identity: start.local_identity,
                target: start.target,
                event: start.event,
                desired: start.expires,
                body: start.body,
                content_type: start.content_type,
                credentials: start.credentials,
                call_id: start.call_id,
                from_tag: start.from_tag,
                cseq: start.initial_cseq,
                tag: None,
                granted: None,
                operation: Some(Operation {
                    kind: OperationKind::Initial,
                    attempted: start.expires,
                    request,
                    auth_retries: 0,
                    interval_retries: 0,
                }),
                pending_remove: false,
                timers: Timers::default(),
                active: true,
            },
            vec![output],
        ))
    }

    /// Whether this lifecycle still owns state.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Current entity tag after a successful operation.
    #[must_use]
    pub fn entity_tag(&self) -> Option<&str> {
        self.tag.as_deref()
    }

    /// Current granted expiry after a successful operation.
    #[must_use]
    pub fn granted_expiry(&self) -> Option<Duration> {
        self.granted
    }

    /// Consume a final response; `cnonce` is fresh driver entropy for a possible digest retry.
    #[allow(
        clippy::too_many_lines,
        reason = "the RFC response table remains in protocol order"
    )]
    pub fn response(&mut self, response: Option<&Response>, cnonce: &str) -> Vec<Output> {
        let mut outputs = Vec::new();
        let Some(mut operation) = self.operation.take() else {
            return outputs;
        };
        let Some(response) = response else {
            terminate(self, Termination::TransactionFailed, &mut outputs);
            return outputs;
        };

        if matches!(response.status.code(), 401 | 407) {
            let proxy = response.status.code() == 407;
            let header = if proxy {
                HeaderName::ProxyAuthenticate
            } else {
                HeaderName::WwwAuthenticate
            };
            let challenge = strongest(
                response
                    .headers
                    .get_all(&header)
                    .filter_map(|value| Challenge::parse(&value.value(), proxy))
                    .collect(),
            );
            if operation.auth_retries >= self.config.authentication_retries
                || challenge.is_none()
                || self.credentials.is_none()
                || !increment_cseq(self, &mut outputs)
            {
                if self.active {
                    terminate(self, Termination::AuthenticationExhausted, &mut outputs);
                }
                return outputs;
            }
            if let (Some(challenge), Some(credentials)) = (challenge, self.credentials.as_ref()) {
                operation.auth_retries = operation.auth_retries.saturating_add(1);
                let mut retry = operation.request.clone();
                replace_cseq(&mut retry, self.cseq);
                let uri = String::from_utf8_lossy(&retry.uri.to_bytes()).into_owned();
                let authorization = respond(
                    &challenge,
                    credentials,
                    "PUBLISH",
                    &uri,
                    u32::from(operation.auth_retries),
                    cnonce,
                );
                retry.headers.remove_all(&challenge.response_header());
                if let Ok(header) =
                    Header::build(challenge.response_header(), Bytes::from(authorization))
                {
                    retry.headers.push(header);
                    operation.request = retry.clone();
                    self.operation = Some(operation);
                    outputs.push(Output::SendPublish {
                        request: Box::new(retry),
                        target: self.target,
                    });
                    return outputs;
                }
            }
            terminate(self, Termination::AuthenticationExhausted, &mut outputs);
            return outputs;
        }

        if response.status.code() == 423 {
            let minimum = strict_duration(response, &HeaderName::MinExpires);
            let valid = operation.kind != OperationKind::Remove
                && operation.interval_retries < self.config.interval_retries
                && minimum.is_some_and(|minimum| {
                    minimum > operation.attempted
                        && minimum <= self.config.maximum_expiry
                        && u32::try_from(minimum.as_secs()).is_ok()
                });
            if valid
                && increment_cseq(self, &mut outputs)
                && let Some(minimum) = minimum
            {
                operation.interval_retries = operation.interval_retries.saturating_add(1);
                operation.attempted = minimum;
                let mut retry = operation.request.clone();
                replace_cseq(&mut retry, self.cseq);
                replace_duration(&mut retry, minimum);
                operation.request = retry.clone();
                self.operation = Some(operation);
                outputs.push(Output::SendPublish {
                    request: Box::new(retry),
                    target: self.target,
                });
                return outputs;
            }
            if self.active {
                terminate(self, Termination::IntervalRejected, &mut outputs);
            }
            return outputs;
        }

        if response.status.code() == 412 && operation.kind != OperationKind::Initial {
            self.tag = None;
            terminate(self, Termination::StaleTag, &mut outputs);
            return outputs;
        }

        if response.status.is_success() {
            let tag = strict_tag(response);
            let expires = strict_duration(response, &HeaderName::Expires);
            let valid_expiry = if operation.kind == OperationKind::Remove {
                expires == Some(Duration::ZERO)
            } else {
                expires.is_some_and(|value| !value.is_zero() && value <= operation.attempted)
            };
            if tag.is_none() || !valid_expiry {
                terminate(self, Termination::MalformedResponse, &mut outputs);
                return outputs;
            }
            if operation.kind == OperationKind::Remove {
                self.tag = None;
                terminate(self, Termination::Removed, &mut outputs);
                return outputs;
            }
            if let (Some(tag), Some(expires)) = (tag, expires) {
                self.tag = Some(tag.clone());
                self.granted = Some(expires);
                self.operation = None;
                arm(self, Timer::Expiry, expires, &mut outputs);
                arm_refresh(self, expires, &mut outputs);
                outputs.push(Output::StateChanged(StateChange::Published(
                    PublishedState { tag, expires },
                )));
                maybe_begin_remove(self, &mut outputs);
            }
            return outputs;
        }

        terminate(
            self,
            Termination::Rejected(response.status.code()),
            &mut outputs,
        );
        outputs
    }

    /// Replace event state conditionally.
    pub fn modify(
        &mut self,
        body: Bytes,
        content_type: String,
    ) -> Result<Vec<Output>, CommandError> {
        if !self.active || self.tag.is_none() {
            return Err(CommandError::Terminated);
        }
        if self.operation.is_some() {
            return Err(CommandError::Busy);
        }
        if body.is_empty() || body.len() > self.config.body_limit || content_type.trim().is_empty()
        {
            return Err(CommandError::InvalidBody);
        }
        let mut outputs = Vec::new();
        cancel(self, Timer::Refresh, &mut outputs);
        if !increment_cseq(self, &mut outputs) {
            return Ok(outputs);
        }
        self.body = body;
        self.content_type = content_type;
        let expires = self.granted.unwrap_or(self.desired);
        begin(self, OperationKind::Modify, expires, true, &mut outputs);
        Ok(outputs)
    }

    /// Remove event state. At most one request is queued behind a live operation.
    pub fn remove(&mut self) -> Result<Vec<Output>, CommandError> {
        if !self.active || self.tag.is_none() {
            return Err(CommandError::Terminated);
        }
        let mut outputs = Vec::new();
        cancel(self, Timer::Refresh, &mut outputs);
        if self.operation.is_some() {
            self.pending_remove = true;
        } else {
            self.pending_remove = true;
            maybe_begin_remove(self, &mut outputs);
        }
        Ok(outputs)
    }

    /// Fire one exact timer generation.
    pub fn timer_fired(&mut self, timer: Timer, generation: u64) -> Vec<Output> {
        let mut outputs = Vec::new();
        if timer_generation(&self.timers, timer) != Some(generation) {
            return outputs;
        }
        set_timer(&mut self.timers, timer, None);
        match timer {
            Timer::Expiry => terminate(self, Termination::LocalExpiry, &mut outputs),
            Timer::Refresh => {
                if self.operation.is_none()
                    && self.tag.is_some()
                    && increment_cseq(self, &mut outputs)
                {
                    let expires = self.granted.unwrap_or(self.desired);
                    begin(self, OperationKind::Refresh, expires, false, &mut outputs);
                }
            }
        }
        outputs
    }

    /// Force the bounded runtime shutdown deadline.
    pub fn shutdown_deadline(&mut self) -> Vec<Output> {
        let mut outputs = Vec::new();
        if self.active {
            terminate(self, Termination::Shutdown, &mut outputs);
        }
        outputs
    }
}

fn begin(
    publisher: &mut Publisher,
    kind: OperationKind,
    expires: Duration,
    with_body: bool,
    outputs: &mut Vec<Output>,
) {
    let Some(tag) = publisher.tag.as_deref() else {
        terminate(publisher, Termination::StaleTag, outputs);
        return;
    };
    let body = with_body.then(|| (publisher.content_type.as_str(), publisher.body.clone()));
    match build_request(
        &publisher.resource,
        &publisher.local_identity,
        &publisher.call_id,
        &publisher.from_tag,
        publisher.cseq,
        &publisher.event,
        expires,
        Some(tag),
        body,
    ) {
        Ok(request) => {
            publisher.operation = Some(Operation {
                kind,
                attempted: expires,
                request: request.clone(),
                auth_retries: 0,
                interval_retries: 0,
            });
            outputs.push(Output::SendPublish {
                request: Box::new(request),
                target: publisher.target,
            });
        }
        Err(_) => terminate(publisher, Termination::MalformedResponse, outputs),
    }
}

fn maybe_begin_remove(publisher: &mut Publisher, outputs: &mut Vec<Output>) {
    if !publisher.pending_remove || publisher.operation.is_some() || publisher.tag.is_none() {
        return;
    }
    publisher.pending_remove = false;
    cancel(publisher, Timer::Refresh, outputs);
    if increment_cseq(publisher, outputs) {
        begin(
            publisher,
            OperationKind::Remove,
            Duration::ZERO,
            false,
            outputs,
        );
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the arguments are the complete PUBLISH identity"
)]
fn build_request(
    resource: &Uri,
    local_identity: &str,
    call_id: &str,
    from_tag: &str,
    cseq: u32,
    event: &str,
    expires: Duration,
    tag: Option<&str>,
    body: Option<(&str, Bytes)>,
) -> Result<Request, StartError> {
    let mut builder = RequestBuilder::new(Method::Publish, resource.clone())
        .header(HeaderName::To, Bytes::from(format!("<{resource}>")))
        .map_err(|_| StartError::Build)?
        .header(
            HeaderName::From,
            Bytes::from(format!("{local_identity};tag={from_tag}")),
        )
        .map_err(|_| StartError::Build)?
        .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
        .map_err(|_| StartError::Build)?
        .cseq(cseq, &Method::Publish)
        .map_err(|_| StartError::Build)?
        .header(HeaderName::Event, Bytes::from(event.to_owned()))
        .map_err(|_| StartError::Build)?
        .header(
            HeaderName::Expires,
            Bytes::from(expires.as_secs().to_string()),
        )
        .map_err(|_| StartError::Build)?
        .max_forwards(70);
    if let Some(tag) = tag {
        builder = builder
            .header(HeaderName::SipIfMatch, Bytes::from(tag.to_owned()))
            .map_err(|_| StartError::Build)?;
    }
    let payload = match body {
        Some((content_type, body)) => {
            builder = builder
                .header(
                    HeaderName::ContentType,
                    Bytes::from(content_type.to_owned()),
                )
                .map_err(|_| StartError::Build)?;
            body
        }
        None => Bytes::new(),
    };
    Ok(builder.body(payload).build())
}

fn strict_tag(response: &Response) -> Option<String> {
    if response.headers.count(&HeaderName::SipETag) != 1 {
        return None;
    }
    let value = response.headers.value(&HeaderName::SipETag)?;
    token(&value).then(|| String::from_utf8_lossy(&value).into_owned())
}

fn strict_duration(response: &Response, name: &HeaderName) -> Option<Duration> {
    if response.headers.count(name) != 1 {
        return None;
    }
    if name == &HeaderName::Expires {
        return response
            .headers
            .typed::<Expires>()?
            .ok()
            .map(|value| Duration::from_secs(u64::from(value.0)));
    }
    let value = response.headers.value(name)?;
    let seconds = std::str::from_utf8(&value)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()?;
    Some(Duration::from_secs(u64::from(seconds)))
}

fn token(value: &[u8]) -> bool {
    !value.is_empty()
        && value.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.' | b'!' | b'%' | b'*' | b'_' | b'+' | b'`' | b'\'' | b'~'
                )
        })
}

fn replace_cseq(request: &mut Request, sequence: u32) {
    request.headers.remove_all(&HeaderName::CSeq);
    if let Ok(header) = Header::build(HeaderName::CSeq, Bytes::from(format!("{sequence} PUBLISH")))
    {
        request.headers.push(header);
    }
}

fn replace_duration(request: &mut Request, expires: Duration) {
    request.headers.remove_all(&HeaderName::Expires);
    if let Ok(header) = Header::build(
        HeaderName::Expires,
        Bytes::from(expires.as_secs().to_string()),
    ) {
        request.headers.push(header);
    }
}

fn increment_cseq(publisher: &mut Publisher, outputs: &mut Vec<Output>) -> bool {
    let Some(next) = publisher.cseq.checked_add(1) else {
        terminate(publisher, Termination::LocalCSeqExhausted, outputs);
        return false;
    };
    publisher.cseq = next;
    true
}

fn arm_refresh(publisher: &mut Publisher, expires: Duration, outputs: &mut Vec<Output>) {
    let seconds = expires.as_secs();
    let refresh = if seconds <= 1 {
        // A one-second grant has no positive integral instant before expiry. Waiting for its
        // boundary avoids an immediate request loop; either timer may then settle the usage.
        Duration::from_secs(1)
    } else {
        Duration::from_secs((seconds.saturating_mul(4) / 5).clamp(1, seconds - 1))
    };
    arm(publisher, Timer::Refresh, refresh, outputs);
}

fn arm(publisher: &mut Publisher, timer: Timer, after: Duration, outputs: &mut Vec<Output>) {
    cancel(publisher, timer, outputs);
    publisher.timers.next = publisher.timers.next.saturating_add(1);
    let generation = publisher.timers.next;
    set_timer(&mut publisher.timers, timer, Some(generation));
    outputs.push(Output::ArmTimer {
        timer,
        generation,
        after,
    });
}

fn cancel(publisher: &mut Publisher, timer: Timer, outputs: &mut Vec<Output>) {
    if let Some(generation) = timer_generation(&publisher.timers, timer) {
        set_timer(&mut publisher.timers, timer, None);
        outputs.push(Output::CancelTimer { timer, generation });
    }
}

fn timer_generation(timers: &Timers, timer: Timer) -> Option<u64> {
    match timer {
        Timer::Refresh => timers.refresh,
        Timer::Expiry => timers.expiry,
    }
}

fn set_timer(timers: &mut Timers, timer: Timer, generation: Option<u64>) {
    match timer {
        Timer::Refresh => timers.refresh = generation,
        Timer::Expiry => timers.expiry = generation,
    }
}

fn terminate(publisher: &mut Publisher, reason: Termination, outputs: &mut Vec<Output>) {
    cancel(publisher, Timer::Refresh, outputs);
    cancel(publisher, Timer::Expiry, outputs);
    publisher.operation = None;
    publisher.pending_remove = false;
    publisher.active = false;
    publisher.tag = None;
    publisher.granted = None;
    outputs.push(Output::StateChanged(StateChange::Terminated(reason)));
}
