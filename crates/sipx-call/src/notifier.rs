//! The socket driver for the RFC 6665 notifier state machine.
//!
//! [`sipx_ua::subscribe::Subscriptions`] remains the only protocol store. This module adds the
//! dialog target, package document and owned expiry task that only a live endpoint can supply.
//! The decision table and lifetime rules are in
//! `docs/specs/event-notifier.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::event::{Packages, Reason, Subscription};
use sipx_sip::headers::{CSeq, Expires};
use sipx_sip::{HeaderName, Method, Request, StatusCode};
use sipx_transport::{Handle, Incoming, Target};
use sipx_ua::packages::{DIALOG_INFO_TYPE, DialogWatch, REGINFO_TYPE, RegistrationWatch};
use sipx_ua::presence::{PIDF_TYPE, Pidf};
use sipx_ua::subscribe::{Answer, Id, Subscriptions};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::call::{add_routes, contact_for, in_dialog_target, token};
use crate::dialog::{Dialog, to_tag};
use crate::dispatch::with_to_tag;

const RETRY_AFTER: &[u8] = b"5";
const NOTIFY_RESPONSE_BOUND: Duration = Duration::from_secs(2);

/// Runtime measurements for an endpoint event notifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifierCounts {
    /// Expiry/notification tasks currently alive.
    pub active_tasks: usize,
    /// Tasks started since this notifier was constructed.
    pub started_tasks: u64,
    /// Tasks which have exited, including cancellation on dispatcher drop.
    pub finished_tasks: u64,
    /// New subscriptions refused at the configured capacity.
    pub shed: u64,
}

#[derive(Debug, Default)]
struct Counters {
    active: AtomicUsize,
    started: AtomicU64,
    finished: AtomicU64,
    shed: AtomicU64,
}

/// Read-only application handle for the notifier attached to a dispatcher.
///
/// The returned store is the exact allocation the socket driver mutates. Exposing it makes state
/// observable for policy, diagnostics and deterministic tests without asking the application to
/// route SUBSCRIBE itself.
#[derive(Debug, Clone)]
pub struct NotifierHandle {
    store: Arc<Mutex<Subscriptions>>,
    counts: Arc<Counters>,
}

impl NotifierHandle {
    /// The one subscription store used by both the library and socket paths.
    #[must_use]
    pub fn subscriptions(&self) -> Arc<Mutex<Subscriptions>> {
        Arc::clone(&self.store)
    }

    /// A point-in-time runtime measurement.
    #[must_use]
    pub fn counts(&self) -> NotifierCounts {
        NotifierCounts {
            active_tasks: self.counts.active.load(Ordering::Relaxed),
            started_tasks: self.counts.started.load(Ordering::Relaxed),
            finished_tasks: self.counts.finished.load(Ordering::Relaxed),
            shed: self.counts.shed.load(Ordering::Relaxed),
        }
    }
}

/// A bounded RFC 6665 notifier ready to attach to one [`crate::Dispatcher`].
#[derive(Debug)]
pub struct Notifier {
    endpoint: Option<Handle>,
    store: Arc<Mutex<Subscriptions>>,
    counts: Arc<Counters>,
    tasks: HashMap<Id, Running>,
    origin: Instant,
}

impl Notifier {
    /// Serve the three built-in event packages, granting at most `policy_maximum` and holding no
    /// more than `capacity` concurrent subscriptions.
    #[must_use]
    pub fn new(policy_maximum: Duration, capacity: usize) -> Self {
        let packages = Packages::new()
            .with(DialogWatch::package())
            .with(RegistrationWatch::package())
            .with("presence");
        Self {
            endpoint: None,
            store: Arc::new(Mutex::new(
                Subscriptions::new(packages, policy_maximum).with_capacity(capacity),
            )),
            counts: Arc::new(Counters::default()),
            tasks: HashMap::new(),
            origin: Instant::now(),
        }
    }

    /// A cloneable observation handle. It does not own runtime tasks.
    #[must_use]
    pub fn handle(&self) -> NotifierHandle {
        NotifierHandle {
            store: Arc::clone(&self.store),
            counts: Arc::clone(&self.counts),
        }
    }

    pub(crate) fn attach(&mut self, endpoint: Handle) {
        self.endpoint = Some(endpoint);
    }

    /// Consume one SUBSCRIBE. The dispatcher has already selected the method.
    pub(crate) async fn receive(&mut self, incoming: &Incoming) {
        self.tasks.retain(|_, task| !task.join.is_finished());
        let Some(endpoint) = self.endpoint.clone() else {
            return;
        };
        if !valid_subscribe_headers(&incoming.request) {
            answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        }
        let Some(id) = Id::from_request(&incoming.request) else {
            answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        };

        if !PackageState::supports(&id.event) {
            let allow = lock(&self.store).packages().allow_events();
            answer(
                &endpoint,
                incoming,
                489,
                "Bad Event",
                Some((HeaderName::AllowEvents, Bytes::from(allow))),
                None,
                None,
            )
            .await;
            return;
        }

        let is_initial = to_tag(&incoming.request.headers).is_none();
        if is_initial && self.tasks.contains_key(&id) {
            answer(
                &endpoint,
                incoming,
                481,
                "Call/Transaction Does Not Exist",
                None,
                None,
                None,
            )
            .await;
            return;
        }

        if let Some(tag) = to_tag(&incoming.request.headers) {
            let known = self.tasks.get(&id).is_some_and(|task| {
                task.local_tag
                    .as_bytes()
                    .eq_ignore_ascii_case(tag.as_slice())
            });
            if !known {
                answer(
                    &endpoint,
                    incoming,
                    481,
                    "Call/Transaction Does Not Exist",
                    None,
                    None,
                    None,
                )
                .await;
                return;
            }
        }

        // A terminating task still owns one package document and one scheduler slot. Do not let a
        // rapid unsubscribe/re-subscribe cycle exceed the configured peer-driven task bound while
        // that final NOTIFY is leaving.
        let at_runtime_capacity = {
            let store = lock(&self.store);
            is_initial
                && !self.tasks.contains_key(&id)
                && store.packages().serves(&id.event)
                && self.tasks.len() >= store.capacity()
        };
        if at_runtime_capacity {
            self.counts.shed.fetch_add(1, Ordering::Relaxed);
            answer(
                &endpoint,
                incoming,
                503,
                "Service Unavailable",
                Some((HeaderName::RetryAfter, Bytes::from_static(RETRY_AFTER))),
                None,
                None,
            )
            .await;
            return;
        }

        let now = self.origin.elapsed().as_secs();
        let outcome = lock(&self.store).on_subscribe(&incoming.request, now);
        self.apply(endpoint, incoming, outcome).await;
    }

    async fn apply(&mut self, endpoint: Handle, incoming: &Incoming, outcome: Answer) {
        match outcome {
            Answer::Malformed => {
                answer(&endpoint, incoming, 400, "Bad Request", None, None, None).await;
            }
            Answer::Unserved { status } => {
                let allow = lock(&self.store).packages().allow_events();
                answer(
                    &endpoint,
                    incoming,
                    status,
                    "Bad Event",
                    Some((HeaderName::AllowEvents, Bytes::from(allow))),
                    None,
                    None,
                )
                .await;
            }
            Answer::AtCapacity => {
                self.counts.shed.fetch_add(1, Ordering::Relaxed);
                answer(
                    &endpoint,
                    incoming,
                    503,
                    "Service Unavailable",
                    Some((HeaderName::RetryAfter, Bytes::from_static(RETRY_AFTER))),
                    None,
                    None,
                )
                .await;
            }
            Answer::Established { id, expires } => {
                self.establish(&endpoint, incoming, id, expires).await;
            }
            Answer::Refreshed { id, expires } => {
                self.refresh(&endpoint, incoming, &id, expires).await;
            }
            Answer::Unsubscribed { id } => {
                self.unsubscribe(&endpoint, incoming, &id).await;
            }
        }
    }

    async fn refresh(&self, endpoint: &Handle, incoming: &Incoming, id: &Id, expires: Duration) {
        let Some(running) = self.tasks.get(id) else {
            answer(
                endpoint,
                incoming,
                481,
                "Call/Transaction Does Not Exist",
                None,
                None,
                None,
            )
            .await;
            return;
        };
        let allow = lock(&self.store).packages().allow_events();
        answer(
            endpoint,
            incoming,
            200,
            "OK",
            Some((HeaderName::AllowEvents, Bytes::from(allow))),
            Some(&running.local_tag),
            Some(expires),
        )
        .await;
        let _ = running.command.send(Command::Refresh(expires));
    }

    async fn unsubscribe(&self, endpoint: &Handle, incoming: &Incoming, id: &Id) {
        let Some(running) = self.tasks.get(id) else {
            answer(
                endpoint,
                incoming,
                481,
                "Call/Transaction Does Not Exist",
                None,
                None,
                None,
            )
            .await;
            return;
        };
        let allow = lock(&self.store).packages().allow_events();
        answer(
            endpoint,
            incoming,
            200,
            "OK",
            Some((HeaderName::AllowEvents, Bytes::from(allow))),
            Some(&running.local_tag),
            Some(Duration::ZERO),
        )
        .await;
        let _ = running
            .command
            .send(Command::Terminate(Reason::Deactivated));
    }

    async fn establish(
        &mut self,
        endpoint: &Handle,
        incoming: &Incoming,
        id: Id,
        expires: Duration,
    ) {
        let local_tag = token();
        let Some(dialog) = Dialog::from_request(&incoming.request, &local_tag) else {
            lock(&self.store).terminate(&id, Reason::Rejected);
            lock(&self.store).sweep();
            answer(endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        };
        let Some(package) = PackageState::for_request(&incoming.request, &id.event) else {
            lock(&self.store).terminate(&id, Reason::Rejected);
            lock(&self.store).sweep();
            answer(endpoint, incoming, 400, "Bad Request", None, None, None).await;
            return;
        };

        let allow = lock(&self.store).packages().allow_events();
        answer(
            endpoint,
            incoming,
            200,
            "OK",
            Some((HeaderName::AllowEvents, Bytes::from(allow))),
            Some(&local_tag),
            Some(expires),
        )
        .await;

        let (command, receiver) = watch::channel(Command::Refresh(expires));
        let task = Lifecycle {
            endpoint: endpoint.clone(),
            target: in_dialog_target(&dialog, Target::new(incoming.source, incoming.transport)),
            dialog,
            package,
            id: id.clone(),
            store: Arc::clone(&self.store),
            receiver,
        };
        self.counts.active.fetch_add(1, Ordering::Relaxed);
        self.counts.started.fetch_add(1, Ordering::Relaxed);
        let guard = TaskGuard(Arc::clone(&self.counts));
        let join = tokio::spawn(task.run(expires, guard));
        self.tasks.insert(
            id,
            Running {
                local_tag,
                command,
                join,
            },
        );
    }
}

impl Drop for Notifier {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.join.abort();
        }
    }
}

#[derive(Debug)]
struct Running {
    local_tag: String,
    command: watch::Sender<Command>,
    join: JoinHandle<()>,
}

#[derive(Debug, Clone, Copy)]
enum Command {
    Refresh(Duration),
    Terminate(Reason),
}

#[derive(Debug)]
struct Lifecycle {
    endpoint: Handle,
    target: Target,
    dialog: Dialog,
    package: PackageState,
    id: Id,
    store: Arc<Mutex<Subscriptions>>,
    receiver: watch::Receiver<Command>,
}

impl Lifecycle {
    async fn run(mut self, expires: Duration, _guard: TaskGuard) {
        let mut deadline = Instant::now() + expires;
        let active = Subscription::active(expires);
        self.send_notify(&active).await;

        loop {
            tokio::select! {
                // This timer defines protocol expiry; it is not a happens-before substitute.
                () = tokio::time::sleep_until(deadline) => {
                    let terminated = lock(&self.store)
                        .terminate(&self.id, Reason::Timeout)
                        .unwrap_or_else(|| Subscription::terminated(Reason::Timeout));
                    self.send_notify(&terminated).await;
                    lock(&self.store).sweep();
                    return;
                }
                changed = self.receiver.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    let command = *self.receiver.borrow_and_update();
                    match command {
                        Command::Refresh(duration) => deadline = Instant::now() + duration,
                        Command::Terminate(reason) => {
                            let terminated = Subscription::terminated(reason);
                            self.send_notify(&terminated).await;
                            lock(&self.store).sweep();
                            return;
                        }
                    }
                }
            }
        }
    }

    async fn send_notify(&mut self, state: &Subscription) {
        let (local, remote) = self.dialog.local_and_remote();
        let (uri, routes) = self.dialog.request_target();
        let cseq = self.dialog.next_cseq();
        let (content_type, body) = self.package.document();
        let built = RequestBuilder::new(Method::Notify, uri)
            .header(HeaderName::To, Bytes::from(remote))
            .and_then(|builder| builder.header(HeaderName::From, Bytes::from(local)))
            .and_then(|builder| {
                builder.header(
                    HeaderName::CallId,
                    Bytes::from(self.dialog.id.call_id.clone()),
                )
            })
            .and_then(|builder| builder.cseq(cseq, &Method::Notify))
            .and_then(|builder| {
                builder.header(
                    HeaderName::Contact,
                    Bytes::from(contact_for(&self.endpoint, self.target.transport)),
                )
            })
            .and_then(|builder| {
                builder.header(HeaderName::Event, Bytes::from(self.id.event.clone()))
            })
            .and_then(|builder| {
                builder.header(HeaderName::SubscriptionState, Bytes::from(state.to_value()))
            })
            .and_then(|builder| {
                builder.header(HeaderName::ContentType, Bytes::from_static(content_type))
            })
            .and_then(|builder| add_routes(builder.max_forwards(70).body(body), &routes));
        let request = match built {
            Ok(builder) => builder.build(),
            Err(error) => {
                tracing::warn!(%error, "could not build subscription NOTIFY");
                return;
            }
        };
        match self.endpoint.send(request, self.target.clone()).await {
            Ok(mut responses) => {
                // This duration bounds a failed NOTIFY transaction; subscription state does not
                // depend on whether the peer supplies the final response.
                let _ =
                    tokio::time::timeout(NOTIFY_RESPONSE_BOUND, responses.final_response()).await;
            }
            Err(error) => {
                tracing::warn!(%error, "could not send subscription NOTIFY");
            }
        }
    }
}

struct TaskGuard(Arc<Counters>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
        self.0.finished.fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
enum PackageState {
    Dialog(DialogWatch),
    Registration(RegistrationWatch),
    Presence(Pidf),
}

impl PackageState {
    fn supports(event: &str) -> bool {
        matches!(
            event.split(';').next().map(str::trim),
            Some(package)
                if package.eq_ignore_ascii_case("dialog")
                    || package.eq_ignore_ascii_case("reg")
                    || package.eq_ignore_ascii_case("presence")
        )
    }

    fn for_request(request: &Request, event: &str) -> Option<Self> {
        let entity = String::from_utf8_lossy(&request.uri.to_bytes()).into_owned();
        match event.split(';').next()?.trim() {
            package if package.eq_ignore_ascii_case("dialog") => {
                Some(Self::Dialog(DialogWatch::new(entity)))
            }
            package if package.eq_ignore_ascii_case("reg") => {
                Some(Self::Registration(RegistrationWatch::new(entity)))
            }
            package if package.eq_ignore_ascii_case("presence") => {
                Some(Self::Presence(Pidf::new(entity)))
            }
            _ => None,
        }
    }

    fn document(&mut self) -> (&'static [u8], Bytes) {
        match self {
            Self::Dialog(watch) => (
                DIALOG_INFO_TYPE.as_bytes(),
                Bytes::from(watch.document(&[])),
            ),
            Self::Registration(watch) => {
                (REGINFO_TYPE.as_bytes(), Bytes::from(watch.document(&[])))
            }
            Self::Presence(pidf) => (PIDF_TYPE.as_bytes(), Bytes::from(pidf.to_xml())),
        }
    }
}

fn lock(store: &Arc<Mutex<Subscriptions>>) -> MutexGuard<'_, Subscriptions> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

fn valid_subscribe_headers(request: &Request) -> bool {
    request.method == Method::Subscribe
        && request.headers.count(&HeaderName::CSeq) == 1
        && matches!(
            request.headers.typed::<CSeq>(),
            Some(Ok(CSeq {
                method: Method::Subscribe,
                ..
            }))
        )
        && request.headers.count(&HeaderName::Expires) <= 1
        && !matches!(request.headers.typed::<Expires>(), Some(Err(_)))
}

async fn answer(
    endpoint: &Handle,
    incoming: &Incoming,
    status: u16,
    reason: &'static str,
    extra: Option<(HeaderName, Bytes)>,
    tag: Option<&str>,
    expires: Option<Duration>,
) {
    let Some(status) = StatusCode::new(status) else {
        return;
    };
    let built = ResponseBuilder::to_request(&incoming.request, status, reason)
        .and_then(|builder| {
            if status.is_success() {
                builder.header(
                    HeaderName::Contact,
                    Bytes::from(contact_for(endpoint, incoming.transport)),
                )
            } else {
                Ok(builder)
            }
        })
        .and_then(|builder| match extra {
            Some((name, value)) => builder.header(name, value),
            None => Ok(builder),
        })
        .and_then(|builder| match expires {
            Some(value) => builder.header(
                HeaderName::Expires,
                Bytes::from(value.as_secs().to_string()),
            ),
            None => Ok(builder),
        })
        .and_then(|builder| with_to_tag(builder, &incoming.request, tag));
    let response = match built {
        Ok(builder) => builder.build(),
        Err(error) => {
            tracing::warn!(%error, "could not build SUBSCRIBE response");
            return;
        }
    };
    if let Err(error) = endpoint.respond(&incoming.key, response).await {
        tracing::warn!(%error, "could not send SUBSCRIBE response");
    }
}
