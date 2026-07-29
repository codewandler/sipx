//! One endpoint's requests, routed to any number of concurrent calls (story `C-4`).
//!
//! An endpoint hands out exactly one [`Receiver<Incoming>`](tokio::sync::mpsc::Receiver). Every
//! request that arrives, for any call in any dialog, comes out of that one stream — so an
//! application holding more than one call had to write its own demultiplexer, and
//! [`serve`](crate::serve) dropped whatever the single call it drove did not claim. Both are
//! ways to lose an ACK, which is the loss that leaks calls: nothing retransmits it once Timer H
//! expires and no timer reaps the dialog it would have completed.
//!
//! This is that demultiplexer, written once. It owns the receiver, routes each request to the
//! call it belongs to, and gives every request that belongs to no call a defined answer instead
//! of silence. The decision table, the counters and the vectors the tests are derived from are
//! in [`docs/specs/call-dispatch.md`](../../../docs/specs/call-dispatch.md).
//!
//! ```no_run
//! # async fn example(endpoint: sipx_transport::Handle,
//! #                  incoming: tokio::sync::mpsc::Receiver<sipx_transport::Incoming>)
//! #     -> sipx_call::Result<()> {
//! use sipx_call::{Dispatched, Dispatcher, answer, serve};
//!
//! let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
//! while let Some(event) = dispatcher.next().await {
//!     if let Dispatched::Invitation(invitation) = event {
//!         let (invite, mut requests) = invitation.into_parts();
//!         let mut call = answer(&endpoint, &invite, "203.0.113.7".parse().expect("addr")).await?;
//!         tokio::spawn(async move { serve(&mut call, &mut requests).await });
//!     }
//! }
//! # Ok(())
//! # }
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Method, Request, StatusCode};
use sipx_transport::{Handle, Incoming};
use tokio::sync::mpsc;

use crate::call::token;
use crate::dialog::{Dialog, from_tag, to_tag};

/// How many requests one call's inbox holds before the dispatcher sheds for it.
///
/// Deep enough that a call busy with one signalling exchange does not shed the next request
/// behind it, shallow enough that a task which has stopped reading is noticed rather than
/// buffered for. Override with [`Dispatcher::with_queue`].
pub const DEFAULT_QUEUE: usize = 16;

/// The `Retry-After` a shed request carries, in seconds.
///
/// The same value [`sipx_transport`] sheds with, for the same reason: the peer is being told the
/// moment was wrong, not that the call is gone, and a number lets it act on that.
const RETRY_AFTER: &[u8] = b"5";

/// What the dispatcher could not place itself, handed to the application.
#[derive(Debug)]
#[non_exhaustive]
pub enum Dispatched {
    /// An INVITE that belongs to no live call: an incoming call.
    ///
    /// Answering, ringing or rejecting it is the application's decision — the dispatcher takes
    /// none of them. Its inbox is already routed, so nothing that arrives while the application
    /// decides can be missed.
    Invitation(Invitation),
    /// A request outside any dialog whose method this stack advertises but the dispatcher does
    /// not place: OPTIONS and CANCEL.
    ///
    /// Surfaced rather than refused because the `Allow` a 405 would carry
    /// ([`sipx_sip::update::ALLOW`]) names them, and answering 405 to a method the same message
    /// says is supported tells the peer two different things at once. What answers an OPTIONS is
    /// a user agent (`sipx_ua::Agent::answer`), which the call framework does not have.
    OutOfDialog(Incoming),
}

/// An incoming call: the INVITE, and the inbox of the call it may become.
///
/// The inbox exists before the application has decided anything, and that is the point. The ACK
/// to our own 2xx can arrive before `answer` has returned, so a route installed only once a
/// [`Call`](crate::Call) existed would have nowhere to put it.
///
/// Dropping this without answering releases the route: the next request for that dialog is
/// answered as an unknown one rather than queued for a call that will never exist.
#[derive(Debug)]
pub struct Invitation {
    incoming: Incoming,
    requests: mpsc::Receiver<Incoming>,
}

impl Invitation {
    /// The INVITE, to answer, ring or refuse.
    #[must_use]
    pub fn request(&self) -> &Incoming {
        &self.incoming
    }

    /// Split into the INVITE and the inbox, ready for
    /// [`answer`](crate::answer) and [`serve`](crate::serve).
    #[must_use]
    pub fn into_parts(self) -> (Incoming, mpsc::Receiver<Incoming>) {
        (self.incoming, self.requests)
    }
}

/// What a dispatcher has refused, shed or could not place.
///
/// The same shape as [`sipx_transport::ShedCounts`] and for the same reason (`T-19`): loss that
/// cannot be counted from outside is loss nobody is told about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DispatchCounts {
    /// Requests answered `503` because the call they belong to was not reading its inbox.
    pub shed: u64,
    /// ACKs that could not be delivered — to a full inbox, or to no call at all.
    ///
    /// Counted apart because an ACK cannot be refused: SIP has no response to one, so nothing
    /// retransmits it after Timer H and the dialog it would have completed is not reaped unless
    /// RFC 4028 session timers happen to be running. This is the one that leaks calls.
    pub acks: u64,
    /// In-dialog requests answered `481 Call/Transaction Does Not Exist` (RFC 3261 §12.2.2).
    pub unmatched: u64,
    /// Out-of-dialog requests answered `405 Method Not Allowed` (RFC 3261 §8.2.1).
    pub unsupported: u64,
}

impl DispatchCounts {
    /// Everything the dispatcher did not deliver, of every kind.
    #[must_use]
    pub fn total(self) -> u64 {
        self.shed
            .saturating_add(self.acks)
            .saturating_add(self.unmatched)
            .saturating_add(self.unsupported)
    }
}

/// What identifies a route: a `Call-ID` and the tag of the party at the other end.
///
/// The local tag is deliberately absent — see
/// [`docs/specs/call-dispatch.md`](../../../docs/specs/call-dispatch.md) §2. It is what lets a
/// route be reserved from an INVITE, before this side has chosen a tag of its own.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    call_id: Vec<u8>,
    peer_tag: Vec<u8>,
}

impl RouteKey {
    /// The route an arriving request belongs to, if it names one at all.
    ///
    /// `None` when the `Call-ID` or the `From` tag is missing, which RFC 3261 §8.1.1 makes
    /// mandatory on every request: such a message cannot be placed in any dialog, present or
    /// future.
    fn of(request: &Request) -> Option<Self> {
        Some(Self {
            call_id: request.headers.value(&HeaderName::CallId)?.into_owned(),
            peer_tag: from_tag(&request.headers)?,
        })
    }

    /// The route a call is reached at.
    fn of_dialog(dialog: &Dialog) -> Self {
        Self {
            call_id: dialog.id.call_id.clone(),
            peer_tag: dialog.id.remote_tag.clone(),
        }
    }
}

/// The routing table, and the counters that describe what missed it.
#[derive(Debug)]
struct Table {
    routes: Mutex<HashMap<RouteKey, mpsc::Sender<Incoming>>>,
    counts: Counters,
    queue: usize,
}

#[derive(Debug, Default)]
struct Counters {
    shed: AtomicU64,
    acks: AtomicU64,
    unmatched: AtomicU64,
    unsupported: AtomicU64,
}

/// Which counter a request that was not delivered belongs on.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Shed,
    Ack,
    Unmatched,
    Unsupported,
}

/// The set of calls a dispatcher routes to: a cheap, cloneable handle to its routing table.
///
/// Needed because the dispatcher's loop and the code that places outbound calls are not the same
/// task. A call made with [`dial`](crate::dial) is registered through this from wherever it was
/// dialled; an inbound one needs nothing, because [`Dispatcher::next`] reserved its route before
/// the application ever saw the INVITE.
#[derive(Debug, Clone)]
pub struct Calls(Arc<Table>);

impl Calls {
    /// Route this dialog's in-dialog requests to the returned inbox.
    ///
    /// Hand the inbox to [`serve`](crate::serve). Dropping it — which is what ending a call and
    /// returning from `serve` does — releases the route.
    ///
    /// There is a window on the outbound path this cannot close: [`dial`](crate::dial) returns
    /// only once the 2xx has arrived, so a BYE that overtakes it is answered `481`. Closing it
    /// needs the `Call-ID` to be known before the INVITE is sent, which is a change to `dial`
    /// this story does not make.
    ///
    /// Registering a dialog that already has a route replaces it, and the previous inbox stops
    /// receiving.
    pub fn register(&self, dialog: &Dialog) -> mpsc::Receiver<Incoming> {
        let (tx, rx) = mpsc::channel(self.0.queue);
        let mut routes = self.lock();
        // A call that has ended dropped its inbox, so its route is dead weight until something
        // arrives for it. Sweeping on the one operation that is already taking the lock keeps a
        // long-lived dispatcher from accumulating them.
        routes.retain(|_, sender| !sender.is_closed());
        routes.insert(RouteKey::of_dialog(dialog), tx);
        rx
    }

    /// Stop routing to this dialog.
    ///
    /// Rarely needed — dropping the inbox does the same thing lazily — but explicit when an
    /// application tears a call down without dropping the receiver it was serving from.
    pub fn forget(&self, dialog: &Dialog) {
        self.lock().remove(&RouteKey::of_dialog(dialog));
    }

    /// How many calls are currently routed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no call is routed at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What has been refused, shed or left unplaced.
    #[must_use]
    pub fn counts(&self) -> DispatchCounts {
        let counts = &self.0.counts;
        DispatchCounts {
            shed: counts.shed.load(Ordering::Relaxed),
            acks: counts.acks.load(Ordering::Relaxed),
            unmatched: counts.unmatched.load(Ordering::Relaxed),
            unsupported: counts.unsupported.load(Ordering::Relaxed),
        }
    }

    /// Record one request the dispatcher did not deliver.
    ///
    /// One method per kind rather than a field reached through from the dispatcher, because the
    /// counters exist so that loss is visible and a caller that had to name a path to reach them
    /// is a caller that can quietly name the wrong one.
    fn counted(&self, kind: Kind) {
        let counts = &self.0.counts;
        match kind {
            Kind::Shed => &counts.shed,
            Kind::Ack => &counts.acks,
            Kind::Unmatched => &counts.unmatched,
            Kind::Unsupported => &counts.unsupported,
        }
        .fetch_add(1, Ordering::Relaxed);
    }

    /// The table, whether or not a previous holder panicked.
    ///
    /// Every critical section here is a map operation with no `await` and nothing fallible in
    /// it, so a poisoned lock cannot mean a half-updated table — and refusing to route the rest
    /// of an endpoint's calls because one unrelated task unwound would be a far worse failure
    /// than the one poisoning guards against.
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<RouteKey, mpsc::Sender<Incoming>>> {
        self.0.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The sender for a route, if there is one. Cloned rather than borrowed so the lock is not
    /// held across the send that follows.
    fn sender(&self, key: &RouteKey) -> Option<mpsc::Sender<Incoming>> {
        self.lock().get(key).cloned()
    }

    /// Reserve a route for a call that does not exist yet, and take its inbox.
    fn reserve(&self, key: RouteKey) -> mpsc::Receiver<Incoming> {
        let (tx, rx) = mpsc::channel(self.0.queue);
        let mut routes = self.lock();
        routes.retain(|_, sender| !sender.is_closed());
        routes.insert(key, tx);
        rx
    }

    fn remove(&self, key: &RouteKey) {
        self.lock().remove(key);
    }

    fn contains(&self, key: &RouteKey) -> bool {
        self.lock().contains_key(key)
    }
}

/// One endpoint's incoming requests, routed to any number of concurrent calls.
///
/// Owns the endpoint's `Receiver<Incoming>`, which is the whole reason it can promise anything:
/// there is exactly one of those, so anything else reading it would be reading requests this
/// cannot then route.
#[derive(Debug)]
pub struct Dispatcher {
    endpoint: Handle,
    incoming: mpsc::Receiver<Incoming>,
    calls: Calls,
}

impl Dispatcher {
    /// Take over an endpoint's incoming requests, with the default per-call queue depth.
    #[must_use]
    pub fn new(endpoint: Handle, incoming: mpsc::Receiver<Incoming>) -> Self {
        Self::with_queue(endpoint, incoming, DEFAULT_QUEUE)
    }

    /// The same, with a per-call queue depth of your own.
    ///
    /// A depth of zero is raised to one: an unbuffered channel would shed every request that
    /// arrived while the call was between `recv` calls, which is most of them.
    #[must_use]
    pub fn with_queue(endpoint: Handle, incoming: mpsc::Receiver<Incoming>, queue: usize) -> Self {
        Self {
            endpoint,
            incoming,
            calls: Calls(Arc::new(Table {
                routes: Mutex::new(HashMap::new()),
                counts: Counters::default(),
                queue: queue.max(1),
            })),
        }
    }

    /// A handle for registering calls this dispatcher did not itself surface.
    #[must_use]
    pub fn calls(&self) -> Calls {
        self.calls.clone()
    }

    /// Route this dialog's requests to the returned inbox — [`Calls::register`], for an
    /// application that holds the dispatcher directly.
    pub fn register(&self, dialog: &Dialog) -> mpsc::Receiver<Incoming> {
        self.calls.register(dialog)
    }

    /// What has been refused, shed or left unplaced.
    #[must_use]
    pub fn counts(&self) -> DispatchCounts {
        self.calls.counts()
    }

    /// The next thing the dispatcher cannot place itself.
    ///
    /// Routes everything else on the way, so this must be called in a loop for a dispatcher to
    /// do its job at all — the ACKs and BYEs of every call it has already handed out move only
    /// while it is being polled. `None` once the endpoint has shut down.
    pub async fn next(&mut self) -> Option<Dispatched> {
        loop {
            let incoming = self.incoming.recv().await?;
            if let Some(surfaced) = self.route(incoming).await {
                return Some(surfaced);
            }
        }
    }

    /// Place one request: the decision table of
    /// [`docs/specs/call-dispatch.md`](../../../docs/specs/call-dispatch.md) §3, in order.
    async fn route(&mut self, incoming: Incoming) -> Option<Dispatched> {
        let Some(key) = RouteKey::of(&incoming.request) else {
            // RFC 3261 §8.1.1 makes `Call-ID` and the `From` tag mandatory on every request.
            // Without them this cannot be placed in any dialog, now or later.
            self.refuse(&incoming, 400, "Bad Request", None).await;
            return None;
        };

        // An INVITE with no `To` tag is a new call, and has to be recognised as one *before* the
        // route lookup: routed by key alone it would land in an existing call, whose
        // `Dialog::matches` would then reject it and leave nothing to answer it.
        if incoming.request.method == Method::Invite && to_tag(&incoming.request.headers).is_none()
        {
            if self.calls.contains(&key) {
                // RFC 3261 §8.2.2.2: a second INVITE bearing the same `Call-ID` and `From` tag
                // as one already in flight is a merged request.
                self.refuse(&incoming, 482, "Loop Detected", None).await;
                return None;
            }
            let requests = self.calls.reserve(key);
            return Some(Dispatched::Invitation(Invitation { incoming, requests }));
        }

        if let Some(sender) = self.calls.sender(&key) {
            self.deliver(&key, sender, incoming).await;
            return None;
        }

        match incoming.request.method {
            // Nothing to answer: SIP has no response to an ACK, and an ACK for a 2xx is a
            // transaction of its own (RFC 3261 §17.1.1.3). Counted and logged instead, because
            // a stray one means a dialog somewhere completed against a call that is not here.
            Method::Ack => {
                self.calls.counted(Kind::Ack);
                tracing::warn!(
                    source = %incoming.source,
                    "an ACK arrived for no live call and cannot be refused"
                );
                None
            }
            // RFC 3261 §12.2.2. Either it says it is in a dialog, or its method exists only
            // inside one — an orphan of a dialog that is gone, not an invitation to open a new
            // exchange.
            ref method if to_tag(&incoming.request.headers).is_some() || dialog_only(method) => {
                self.calls.counted(Kind::Unmatched);
                self.refuse(&incoming, 481, "Call/Transaction Does Not Exist", None)
                    .await;
                None
            }
            // On the `Allow` the 405 below would carry, so refusing it there would have one
            // message say two different things. The application decides.
            ref method if advertised(method) => Some(Dispatched::OutOfDialog(incoming)),
            // RFC 3261 §8.2.1: "the UAS MUST generate a 405 ... and MUST add an Allow header
            // field".
            _ => {
                self.calls.counted(Kind::Unsupported);
                self.refuse(
                    &incoming,
                    405,
                    "Method Not Allowed",
                    Some((
                        HeaderName::Allow,
                        Bytes::from_static(sipx_sip::update::ALLOW.as_bytes()),
                    )),
                )
                .await;
                None
            }
        }
    }

    /// Hand a request to the call it belongs to, or say why it could not be.
    ///
    /// Never awaits room. The whole promise of a per-call queue is that one application task
    /// which has stopped reading cannot stop the loop that serves every other call, and a
    /// dispatcher that blocked here would trade a shed request for every call on the endpoint.
    async fn deliver(&self, key: &RouteKey, sender: mpsc::Sender<Incoming>, incoming: Incoming) {
        let is_ack = incoming.request.method == Method::Ack;
        match sender.try_send(incoming) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(incoming)) => {
                if is_ack {
                    self.calls.counted(Kind::Ack);
                    tracing::error!(
                        source = %incoming.source,
                        "a call's queue is full; an ACK was dropped and cannot be refused — \
                         the dialog it would have completed will not be reaped"
                    );
                } else {
                    self.calls.counted(Kind::Shed);
                    tracing::warn!(
                        source = %incoming.source,
                        method = %incoming.request.method,
                        "a call's queue is full; refusing the transaction for that call"
                    );
                    self.refuse(
                        &incoming,
                        503,
                        "Service Unavailable",
                        Some((HeaderName::RetryAfter, Bytes::from_static(RETRY_AFTER))),
                    )
                    .await;
                }
            }
            Err(mpsc::error::TrySendError::Closed(incoming)) => {
                // The application dropped the inbox, which is what ending a call does. The
                // route is stale, so it goes, and the request gets the answer any request for a
                // dialog this endpoint does not have gets.
                self.calls.remove(key);
                if is_ack {
                    self.calls.counted(Kind::Ack);
                } else {
                    self.calls.counted(Kind::Unmatched);
                    self.refuse(&incoming, 481, "Call/Transaction Does Not Exist", None)
                        .await;
                }
            }
        }
    }

    /// Answer a request the dispatcher will not route.
    ///
    /// Failures are logged rather than returned: this is the path that exists so that nothing is
    /// dropped in silence, and giving it an error for the caller to ignore would put the silence
    /// back one level up.
    async fn refuse(
        &self,
        incoming: &Incoming,
        status: u16,
        reason: &'static str,
        extra: Option<(HeaderName, Bytes)>,
    ) {
        let Some(code) = StatusCode::new(status) else {
            return;
        };
        let built = ResponseBuilder::to_request(&incoming.request, code, reason)
            .and_then(|builder| match extra {
                Some((name, value)) => builder.header(name, value),
                None => Ok(builder),
            })
            .and_then(|builder| with_to_tag(builder, &incoming.request));
        let response = match built {
            Ok(builder) => builder.build(),
            Err(error) => {
                tracing::warn!(%error, status, "could not build the refusal for a request");
                return;
            }
        };
        if let Err(error) = self.endpoint.respond(&incoming.key, response).await {
            tracing::warn!(%error, status, "could not send the refusal for a request");
        }
    }
}

/// Give a response a `To` tag if the request did not already carry one.
///
/// RFC 3261 §8.2.6.2: every response but a 100 must have one, and an out-of-dialog request
/// arrives without it. A refusal with no tag is a response a peer is entitled to discard, which
/// would turn a considered answer back into the silence this whole path exists to remove.
fn with_to_tag(
    builder: sipx_sip::build::ResponseBuilder,
    request: &Request,
) -> std::result::Result<sipx_sip::build::ResponseBuilder, sipx_sip::error::BuildError> {
    if to_tag(&request.headers).is_some() {
        return Ok(builder);
    }
    let Some(to) = request.headers.value(&HeaderName::To) else {
        return Ok(builder);
    };
    // Appending works in both forms of the header: after `>` in a name-addr, and after a bare
    // addr-spec, where the semicolon starts a header parameter (RFC 3261 §20).
    let value = format!("{};tag={}", String::from_utf8_lossy(&to), token());
    builder.set_header(&HeaderName::To, Bytes::from(value))
}

/// Whether a method is defined only inside a dialog.
///
/// One arriving without a `To` tag is therefore an orphan of a dialog that is gone rather than a
/// new exchange, and RFC 3261 §12.2.2's 481 is the honest answer. Listing them is narrower than
/// the alternative of treating every unrecognised out-of-dialog request as an orphan, which
/// would answer 481 to things that are simply unsupported.
fn dialog_only(method: &Method) -> bool {
    matches!(
        method,
        Method::Bye
            | Method::Update
            | Method::Prack
            | Method::Refer
            | Method::Notify
            | Method::Info
    )
}

/// Whether this stack advertises the method (RFC 3311 §4, RFC 3261 §20.5).
///
/// Read from [`sipx_sip::update::ALLOW`] rather than written out again, because that constant is
/// what a 405 puts on the wire and what a peer reads as permission. A second copy that drifted
/// would have one message advertise a method the next refuses.
fn advertised(method: &Method) -> bool {
    let token = method.as_bytes();
    sipx_sip::update::ALLOW
        .split(',')
        .any(|allowed| allowed.trim().as_bytes().eq_ignore_ascii_case(token))
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
    use sipx_sip::{Limits, Message, parse_datagram};

    fn request(text: &str) -> Request {
        match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram()).expect("parses") {
            Message::Request(r) => r,
            Message::Response(_) => panic!("a request"),
        }
    }

    fn bye(call_id: &str, from_tag: &str, to_tag: &str) -> Request {
        request(&format!(
            "BYE sip:callee@192.0.2.9:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKbye\r\n\
             To: <sip:callee@example.com>;tag={to_tag}\r\n\
             From: <sip:caller@example.net>;tag={from_tag}\r\n\
             Call-ID: {call_id}\r\n\
             CSeq: 2 BYE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n"
        ))
    }

    /// The key is the `Call-ID` and the peer's tag, and *not* ours — which is what lets a route
    /// be reserved from an INVITE, before this side has chosen a tag at all.
    #[test]
    fn a_route_is_keyed_without_our_own_tag() {
        let first = RouteKey::of(&bye("c@sipx", "theirs", "ours")).expect("a key");
        let second = RouteKey::of(&bye("c@sipx", "theirs", "a-different-local-tag")).expect("key");
        assert_eq!(first, second, "our own tag must not enter the key");

        let other_call = RouteKey::of(&bye("other@sipx", "theirs", "ours")).expect("a key");
        assert_ne!(first, other_call);
        let other_peer = RouteKey::of(&bye("c@sipx", "someone-else", "ours")).expect("a key");
        assert_ne!(first, other_peer, "a different peer is a different route");
    }

    /// RFC 3261 §8.1.1 makes both parts mandatory. A request missing either cannot be placed in
    /// any dialog, and the dispatcher answers 400 rather than inventing a route for it.
    #[test]
    fn a_request_that_names_no_dialog_has_no_key() {
        let tagless = request(
            "BYE sip:callee@192.0.2.9:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKbye\r\n\
             To: <sip:callee@example.com>;tag=ours\r\n\
             From: <sip:caller@example.net>\r\n\
             Call-ID: c@sipx\r\n\
             CSeq: 2 BYE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(RouteKey::of(&tagless).is_none(), "no From tag, no route");
    }

    /// The 405's `Allow` and this predicate must be the same list, or one message advertises a
    /// method the next refuses. Reading the constant is what makes that structural.
    #[test]
    fn what_is_advertised_is_exactly_the_allow_constant() {
        for method in [
            Method::Invite,
            Method::Ack,
            Method::Cancel,
            Method::Bye,
            Method::Options,
            Method::Update,
        ] {
            assert!(
                advertised(&method),
                "{method} is on ALLOW but not advertised"
            );
        }
        for method in [
            Method::Register,
            Method::Subscribe,
            Method::Publish,
            Method::Message,
            Method::Other(Bytes::from_static(b"FROBNICATE")),
        ] {
            assert!(!advertised(&method), "{method} is not on ALLOW");
        }
    }

    /// A token, not a substring: `UPDATEX` is a different method, and a substring test would
    /// advertise it.
    #[test]
    fn advertising_matches_tokens_and_not_substrings() {
        assert!(!advertised(&Method::Other(Bytes::from_static(b"UPDATEX"))));
        assert!(!advertised(&Method::Other(Bytes::from_static(b"INV"))));
    }

    /// The methods that only exist inside a dialog get 481 when no call claims them; the rest
    /// are either surfaced or refused 405, which are different answers to different questions.
    #[test]
    fn the_dialog_only_methods_are_the_ones_that_orphan() {
        for method in [
            Method::Bye,
            Method::Update,
            Method::Prack,
            Method::Refer,
            Method::Notify,
            Method::Info,
        ] {
            assert!(dialog_only(&method), "{method} exists only in a dialog");
        }
        for method in [Method::Invite, Method::Options, Method::Register] {
            assert!(!dialog_only(&method), "{method} can start something");
        }
    }
}
