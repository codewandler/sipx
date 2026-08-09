//! `sipx peers` — who is there to call.
//!
//! Without `--registrar` this command consults only a file. With it, the same generic event
//! lifecycle used by library applications consumes bounded registration information from a live
//! registrar; a book is merged only when the caller names one explicitly.
//!
//! **The book's format.** One peer per line, a name and a URI separated by whitespace, `#` for
//! a comment, blank lines ignored:
//!
//! ```text
//! # who this phone knows about
//! alice   sip:alice@192.0.2.17:5060
//! bob     sips:bob@example.com
//! ```
//!
//! Chosen because principle 6 wants it usable from a shell, and this is the shape a shell
//! already has verbs for — `echo "alice sip:alice@host" >> "$book"` writes it and
//! `while read -r name uri` reads it. Nothing structured (TOML, JSON, INI) is writable from a
//! script without either a parser or a `sed` invocation that corrupts the file on the second
//! run, and none of them is worth a dependency for two fields. The reasoning is recorded in
//! `docs/designs/discovery.md`.
//!
//! **Strict, not forgiving.** A line that is not a name and a URI fails the whole listing,
//! naming the line. Skipping it would print a list that is short by one and says so nowhere,
//! and the design is explicit that a partial list must never be presented as complete.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use bytes::Bytes;
use rand::Rng as _;
use sipx_call::Dispatcher;
use sipx_call::subscriber::{
    EventNotification, EventSubscription, EventSubscriptionEvent, EventSubscriptions,
};
use sipx_sip::Uri;
use sipx_transport::{Config as TransportConfig, Target, TransportKind, bind};
use sipx_ua::Credentials;
use sipx_ua::event_client::{
    Config as EventConfig, Peer as EventPeer, SamePeer, Start as EventStart, StateChange,
    Termination, Transport,
};
use sipx_ua::reginfo::{RegistrationConsumer, RegistrationSnapshot};

use crate::cli::PeersOptions;
use crate::output::{Exit, Format, Report, fail};

/// Where a peer was learned from.
///
/// Carried on every entry even though there is only one variant today. A peer typed into a file
/// and one that answered a multicast query thirty seconds ago are not the same kind of fact, and
/// a list that flattens them cannot gain `S-24`'s registrar or `T-24`'s local link without
/// breaking every script that already reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// Written down locally, by a person or a script.
    Book,
    /// Current registration state delivered by a registrar subscription.
    Registrar,
}

impl Source {
    /// The stable name a script matches on.
    fn as_str(self) -> &'static str {
        match self {
            Self::Book => "book",
            Self::Registrar => "registrar",
        }
    }
}

/// One thing that can be called.
///
/// A name for a person to use and `P-6` to look up, and a URI that is enough to dial — nothing
/// else. This is a phone book and not a dial plan: it is consulted when someone names a peer,
/// never while routing an inbound request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Peer {
    /// What to call it by.
    pub(crate) name: String,
    /// What to dial.
    pub(crate) uri: String,
    /// Where it was learned from.
    pub(crate) source: Source,
    /// Age of the live snapshot; absent for facts without an observation time.
    pub(crate) age: Option<Duration>,
}

impl Peer {
    /// The entry as a result line, in the form `P-1` set for every other command.
    fn report(&self) -> Report {
        let report = Report::new()
            .text("status", "peer")
            .text("name", self.name.as_str())
            .text("uri", self.uri.as_str())
            .text("source", self.source.as_str());
        match self.age {
            Some(age) => report.seconds("age", age),
            None => report,
        }
    }
}

/// Why the peer book could not be listed.
///
/// A typed error rather than an empty list: "I could not read the book" and "the book is empty"
/// are different facts, and a script that cannot tell them apart will call nobody and report
/// success.
#[derive(Debug)]
pub(crate) enum Error {
    /// Nothing said where the book is, and the environment names no default.
    NoLocation,
    /// The book is where it should be and could not be read.
    Unreadable {
        /// The path that was tried.
        path: PathBuf,
        /// What the filesystem said.
        cause: std::io::Error,
    },
    /// A line in the book is not a peer.
    Malformed {
        /// The book being read.
        path: PathBuf,
        /// The offending line, counting from one, as an editor counts.
        line: usize,
        /// What was expected there.
        reason: &'static str,
    },
}

impl Error {
    /// The exit code this failure leaves.
    ///
    /// A missing location is fixable on the command line, so it is a usage error; a book that
    /// exists and cannot be used is not the caller's spelling and exits `failed`.
    fn exit(&self) -> Exit {
        match self {
            Self::NoLocation => Exit::Usage,
            Self::Unreadable { .. } | Self::Malformed { .. } => Exit::Failed,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoLocation => write!(
                f,
                "no peer book: pass --book <FILE> or set SIPX_PEERS, since neither \
                 XDG_CONFIG_HOME nor HOME is set"
            ),
            Self::Unreadable { path, cause } => {
                write!(f, "cannot read the peer book {}: {cause}", path.display())
            }
            Self::Malformed { path, line, reason } => {
                write!(f, "{}:{line}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unreadable { cause, .. } => Some(cause),
            Self::NoLocation | Self::Malformed { .. } => None,
        }
    }
}

pub(crate) async fn run(options: PeersOptions, format: Format) -> Exit {
    let registrar = options.registrar.as_deref();
    let mut peers = match (registrar, options.book.as_deref()) {
        (Some(_), None) => Vec::new(),
        (_, explicit) => match load(explicit) {
            Ok(peers) => peers,
            Err(error) => return fail(format, error.exit(), &error.to_string()),
        },
    };
    if let Some(registrar) = registrar {
        match discover(&options, registrar).await {
            Ok(discovered) => peers.extend(discovered),
            Err(failure) => return failure.report(format),
        }
    }

    for (index, peer) in peers.iter().enumerate() {
        // One JSON object per line, which is what `P-1` set and what lets a reader split on
        // newlines. In the human form the entries are blocks, so a blank line separates them.
        if format == Format::Text && index > 0 {
            println!();
        }
        peer.report().emit(format);
    }

    Exit::Success
}

/// Why the registrar could not be consulted, and how far the pass over its addresses got.
///
/// The counts travel with the failure rather than being formatted where one is raised: `T-41`
/// established `candidates_attempted`/`candidates_resolved` as one shape across commands, and a
/// script that learned them from `register` has to read the same pair here.
#[derive(Debug)]
pub(crate) struct Unreachable {
    /// The exit a script branches on.
    exit: Exit,
    /// What to tell the person reading it.
    message: String,
    /// How far the serial pass got, when the failure came from one.
    attempts: Option<crate::destination::Attempts>,
}

impl Unreachable {
    /// A failure that no candidate pass produced — validation, or the lookup that precedes one.
    fn stated(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            attempts: None,
        }
    }

    /// Emit the record, on stderr like every other failure: nothing that is not a peer listing may
    /// land where the next stage of a pipeline will parse it as one.
    fn report(self, format: Format) -> Exit {
        let report = crate::destination::with_attempts(
            Report::new()
                .text("status", self.exit.as_str())
                .text("error", self.message),
            self.attempts,
        );
        eprintln!("{}", report.render(format));
        self.exit
    }
}

/// One candidate's subscription attempt that produced no snapshot.
#[derive(Debug)]
struct Failed {
    /// The exit this outcome leaves.
    exit: Exit,
    /// What went wrong, in the words the caller sees.
    message: String,
    /// Whether another address could answer differently.
    unreachable: bool,
}

#[allow(
    clippy::too_many_lines,
    reason = "validation, endpoint ownership and terminal cleanup remain visible in protocol order"
)]
async fn discover(options: &PeersOptions, registrar: &str) -> Result<Vec<Peer>, Unreachable> {
    let parsed = Uri::parse(Bytes::copy_from_slice(registrar.as_bytes())).map_err(|_| {
        Unreachable::stated(
            Exit::Usage,
            format!("not a SIP registrar address of record: {registrar}"),
        )
    })?;
    let Some((user, domain)) = crate::register::parse_aor(registrar) else {
        return Err(Unreachable::stated(
            Exit::Usage,
            format!("not a SIP registrar address of record: {registrar}"),
        ));
    };
    let mut selection = crate::signalling::Selection::from_options(
        &options.signalling,
        parsed.scheme().is_secure(),
    )
    .map_err(|message| Unreachable::stated(Exit::Usage, message))?;
    // Refused before the lookup rather than after it: an interval that cannot subscribe is
    // knowable without asking anyone, and it is also the bound the lookup is held under.
    let expires = Duration::from_secs(options.expires);
    if expires.is_zero() {
        return Err(Unreachable::stated(
            Exit::Usage,
            "--expires must be positive for a registrar subscription",
        ));
    }
    // This command states no attempt deadline, so the lifetime it asks the registrar for is the
    // only duration its caller gives it. A subscription that may live one second must not spend
    // `T-38`'s eight finding where to send itself, and a generous lifetime — the 3600-second
    // default among them — leaves the resolver's own bounds exactly where they were.
    let resolver = crate::destination::Resolver::within(Some(expires));
    let candidates = resolver
        .resolve(
            &parsed,
            options.target.as_deref(),
            selection,
            &options.signalling,
        )
        .await
        .map_err(|error| {
            Unreachable::stated(crate::destination::exit(&error), error.to_string())
        })?;
    // The head of the list still decides what is bound: the endpoint is opened once and every
    // candidate is attempted over it, exactly as `register` does. What changes with `P-31` is that
    // the tail is no longer discarded.
    let selected = crate::destination::first(&candidates)
        .map_err(|error| Unreachable::stated(crate::destination::exit(&error), error.to_string()))?
        .clone();
    selection = selection.negotiated(selected.transport);
    let local = options.local;
    let mut transport_config = TransportConfig::new(local);
    transport_config.sent_by =
        crate::advertise::reachable_ip(local, selected.addr.ip()).to_string();
    selection
        .configure_client(&options.signalling, &mut transport_config)
        .map_err(|message| Unreachable::stated(Exit::Usage, message))?;
    let (endpoint, incoming) = bind(transport_config)
        .await
        .map_err(|error| Unreachable::stated(Exit::Failed, format!("bind: {error}")))?;
    let runtime = EventSubscriptions::new(EventConfig::default())
        .map_err(|error| Unreachable::stated(Exit::Failed, error.to_string()))?;
    let subscriptions = runtime.handle();
    let mut dispatcher =
        Dispatcher::new(endpoint.clone(), incoming).with_event_subscriptions(runtime);
    let dispatch = tokio::spawn(async move { while dispatcher.next().await.is_some() {} });

    let watch = Duration::from_secs(options.watch);
    let local_identity = format!("<sip:{user}@{domain}>");
    let contact = format!("<sip:{user}@{}>", endpoint.advertised());

    // `P-31`: the serial pass, taken from the library rather than written a fifth time. What is
    // this command's and not the pass's is the classification below and the budget it is funded
    // from — one deadline over every candidate together, never a copy of it each.
    let outcome = crate::destination::walk(&candidates, Some(FIRST_NOTIFY), |target, remaining| {
        let resource = parsed.clone();
        let local_identity = local_identity.clone();
        let contact = contact.clone();
        let credentials = options
            .password
            .clone()
            .map(|password| Credentials::new(user.clone(), password));
        let consumer = RegistrationConsumer::new(registrar, CONTACT_LIMIT);
        let target = event_peer(target);
        // `None` when the candidate's transport has no event-client name — see `event_peer`. The
        // address is reported unreachable so the pass moves to the next one, which is what a
        // candidate this command cannot dial is.
        // A fresh dialog identity per candidate. A subscription attempted at another address is a
        // new usage (RFC 6665 §4.1.2.1), and reusing the Call-ID and tag of the attempt that just
        // failed would present it to the registrar as the first one arriving twice.
        let nonce: u64 = rand::rng().random();
        let subscriptions = &subscriptions;
        async move {
            let Ok(consumer) = consumer else {
                return crate::destination::Attempted::Answered(Failed::stated(
                    Exit::Usage,
                    "invalid registrar resource for the registration package",
                ));
            };
            let Some(target) = target else {
                return crate::destination::Attempted::Unreachable(Failed::stated(
                    Exit::Failed,
                    "the candidate's transport has no event-client name",
                ));
            };
            let start = EventStart {
                resource,
                local_identity,
                contact,
                target,
                expires,
                body: Bytes::new(),
                content_type: None,
                credentials,
                call_id: format!("peers-{nonce:016x}@sipx"),
                from_tag: format!("{nonce:016x}"),
                initial_cseq: 1,
                consumer,
                trust: std::sync::Arc::new(SamePeer),
            };
            match subscriptions.subscribe(start) {
                Ok(mut subscription) => {
                    let observed = observe(&mut subscription, watch, remaining).await;
                    let _ = subscription.unsubscribe().await;
                    match observed {
                        Ok(delivery) => {
                            crate::destination::Attempted::Reached(registrar_peers(delivery))
                        }
                        Err(failed) if failed.unreachable => {
                            crate::destination::Attempted::Unreachable(failed)
                        }
                        Err(failed) => crate::destination::Attempted::Answered(failed),
                    }
                }
                Err(error) => crate::destination::Attempted::Answered(Failed::stated(
                    Exit::Failed,
                    error.to_string(),
                )),
            }
        }
    })
    .await;

    // Terminal cleanup is the command's on every path: nothing this invocation started may still
    // be running when a script reads the result (`P-27`).
    endpoint.shutdown().await;
    let _ = dispatch.await;
    outcome.map_err(unreached)
}

/// How many contacts one registration snapshot may carry before it is refused.
const CONTACT_LIMIT: usize = 4_096;

/// Turn the pass's outcome into the record the command emits.
fn unreached(outcome: crate::destination::Unreached<Failed>) -> Unreachable {
    let attempts = outcome.attempts();
    match outcome {
        // `first` above already refused an empty list, so nothing reaches this today. It is
        // answered rather than swept into a catch-all because a pass that silently reported
        // "resolution returned nothing" as some other failure would be a regression nobody sees.
        crate::destination::Unreached::Nothing => {
            Unreachable::stated(Exit::Failed, "target resolution returned no candidates")
        }
        // Distinguished from a registrar that accepted and stayed silent by naming the pass rather
        // than the subscription. Both exit `Timeout`, because a script branching on the exit code
        // must not have to know which clock ran out.
        crate::destination::Unreached::Expired { .. } => Unreachable {
            exit: Exit::Timeout,
            message: format!(
                "no address of the registrar answered within {}s",
                FIRST_NOTIFY.as_secs()
            ),
            attempts,
        },
        crate::destination::Unreached::Unreachable { last, .. } => Unreachable {
            exit: last.exit,
            message: last.message,
            attempts,
        },
        crate::destination::Unreached::Answered(failed) => {
            Unreachable::stated(failed.exit, failed.message)
        }
        // `Unreached` is `#[non_exhaustive]` since `M-83`. The arm above says why `Nothing` is
        // answered by name rather than swept into a catch-all, and the same rule applies here:
        // this states that the ending is one the command does not know, instead of borrowing a
        // message from an ending that did not happen.
        _ => Unreachable::stated(
            Exit::Failed,
            "the candidate pass ended for an unstated reason",
        ),
    }
}

/// How long to wait for the registrar's first NOTIFY before giving up (`P-30`).
///
/// Without this the wait is the event client's Timer N — 64·T1, thirty-two seconds — which the
/// command states nowhere and the caller cannot change. Once `P-26` bounded resolution, that
/// inherited schedule became the longest thing `peers` could do without saying so. Twenty seconds
/// matches `register`'s default attempt deadline, so the two commands answer on the same clock.
///
/// `P-31` makes it the bound over the *whole* serial pass rather than over one address's wait: it
/// funds every candidate together, because sixteen candidates with a copy of it each would multiply
/// the only duration this command states by sixteen (`P-26`). `--watch` is not spent from it — that
/// is an observation window opened after a registrar has been reached, not part of reaching one.
const FIRST_NOTIFY: Duration = Duration::from_secs(20);

async fn observe(
    subscription: &mut EventSubscription<RegistrationSnapshot>,
    watch: Duration,
    within: Option<Duration>,
) -> Result<EventNotification<RegistrationSnapshot>, Failed> {
    let first =
        match tokio::time::timeout(within.unwrap_or(FIRST_NOTIFY), next_snapshot(subscription))
            .await
        {
            Ok(result) => result?,
            // Distinguished from `Termination::NoInitialNotify` — which is the *registrar* saying
            // it will not notify — by naming the bound that expired. Both exit `Timeout`, because a
            // script branching on the exit code must not have to know which clock ran out; the
            // message is what tells them apart, exactly as `register`'s does. The stated bound is
            // reported rather than the slice of it this candidate was funded from: "no notification
            // within 4s" describes an accounting detail, and the caller asked for twenty.
            Err(_) => {
                return Err(Failed::stated(
                    Exit::Timeout,
                    format!(
                        "the registrar accepted the subscription but sent no notification \
                         within {}s",
                        FIRST_NOTIFY.as_secs()
                    ),
                ));
            }
        };
    if watch.is_zero() {
        return Ok(first);
    }
    let mut latest = first;
    // The clock is the measurement: `--watch` asks for an observation window of exactly this size.
    let deadline = tokio::time::sleep(watch);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => return Ok(latest),
            event = subscription.next_event() => match event {
                Some(EventSubscriptionEvent::Notification(delivery)) => latest = delivery,
                Some(EventSubscriptionEvent::State(StateChange::Terminated(reason))) => {
                    return Err(termination(&reason));
                }
                // A state change, or an event kind this build does not model: neither is a
                // snapshot, so the window keeps waiting for one rather than ending early.
                Some(_) => {}
                None => {
                    return Err(Failed::stated(Exit::Failed, "registrar subscription ended"));
                }
            },
        }
    }
}

async fn next_snapshot(
    subscription: &mut EventSubscription<RegistrationSnapshot>,
) -> Result<EventNotification<RegistrationSnapshot>, Failed> {
    loop {
        match subscription.next_event().await {
            Some(EventSubscriptionEvent::Notification(delivery)) => return Ok(delivery),
            Some(EventSubscriptionEvent::State(StateChange::Terminated(reason))) => {
                return Err(termination(&reason));
            }
            // See above: anything that is not a snapshot leaves the loop waiting for one.
            Some(_) => {}
            None => {
                return Err(Failed::stated(
                    Exit::Failed,
                    "registrar subscription ended before a snapshot",
                ));
            }
        }
    }
}

impl Failed {
    /// An outcome the registrar itself produced, which another of its addresses would repeat.
    fn stated(exit: Exit, message: impl Into<String>) -> Self {
        Self {
            exit,
            message: message.into(),
            unreachable: false,
        }
    }
}

fn termination(reason: &Termination) -> Failed {
    let exit = match reason {
        Termination::Rejected(status) => Exit::for_status(*status),
        Termination::AuthenticationExhausted => Exit::Unauthorized,
        Termination::NoInitialNotify | Termination::LocalExpiry => Exit::Timeout,
        _ => Exit::Failed,
    };
    Failed {
        exit,
        message: format!("registrar subscription failed: {reason:?}"),
        // The only outcome that belongs to *this address* rather than to the registrar the name
        // stands for: the SUBSCRIBE never reached a final response, so nothing has answered yet
        // and a later address still can. A refusal, an exhausted challenge or an accepted
        // subscription that stays silent is the registrar speaking, and asking a second address
        // repeats a question that has already been answered — the rule
        // `UserAgent::register_candidates` applies to a REGISTER, applied to a SUBSCRIBE.
        unreachable: matches!(reason, Termination::TransactionFailed),
    }
}

fn registrar_peers(delivery: EventNotification<RegistrationSnapshot>) -> Vec<Peer> {
    let age = delivery.received_at.elapsed();
    delivery
        .value
        .peers
        .into_iter()
        .map(|peer| Peer {
            name: peer.name,
            uri: peer.uri,
            source: Source::Registrar,
            age: Some(age),
        })
        .collect()
}

/// The event client's name for a resolved target, or `None` when it has none.
///
/// Partial since `M-83` made both transport enums `#[non_exhaustive]`: the SIP transport set grows
/// and neither crate may promise otherwise across a release. A candidate this cannot name is
/// declined rather than approximated — subscribing to a registrar over a transport it did not
/// select is the one substitution that could downgrade a protected flow.
fn event_peer(target: &Target) -> Option<EventPeer> {
    let transport = match target.transport {
        TransportKind::Udp => Transport::Udp,
        TransportKind::Tcp => Transport::Tcp,
        TransportKind::Tls => Transport::Tls,
        TransportKind::Ws => Transport::Ws,
        TransportKind::Wss => Transport::Wss,
        TransportKind::Quic => Transport::Quic,
        _ => return None,
    };
    Some(EventPeer {
        address: target.addr,
        transport,
        connection: None,
        identity: target.verify_as.clone(),
        path: target.path.clone(),
    })
}

/// Read the book from wherever it lives.
fn load(explicit: Option<&str>) -> Result<Vec<Peer>, Error> {
    let path = locate(
        explicit,
        None,
        std::env::var("XDG_CONFIG_HOME").ok(),
        std::env::var("HOME").ok(),
    )?;
    let contents = std::fs::read_to_string(&path).map_err(|cause| Error::Unreadable {
        path: path.clone(),
        cause,
    })?;
    parse(&path, &contents)
}

/// Where the book lives.
///
/// An explicit path wins, then the environment, then the XDG config directory — the same order
/// `register` uses for a password, and for the same reason: the flag is the convenience and the
/// environment is what a script sets once.
fn locate(
    explicit: Option<&str>,
    from_env: Option<String>,
    xdg_config_home: Option<String>,
    home: Option<String>,
) -> Result<PathBuf, Error> {
    if let Some(path) = explicit {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = from_env.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    if let Some(config) = xdg_config_home.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(config).join("sipx").join("peers"));
    }
    if let Some(home) = home.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(home)
            .join(".config")
            .join("sipx")
            .join("peers"));
    }
    Err(Error::NoLocation)
}

/// Turn a book into peers, or say which line is not one.
fn parse(path: &Path, contents: &str) -> Result<Vec<Peer>, Error> {
    let mut peers = Vec::new();

    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let malformed = |reason| Error::Malformed {
            path: path.to_path_buf(),
            // Counting from one, as an editor counts.
            line: index + 1,
            reason,
        };

        let mut fields = line.split_whitespace();
        let (Some(name), Some(uri)) = (fields.next(), fields.next()) else {
            return Err(malformed(
                "a peer is a name and a URI, e.g. `alice sip:alice@192.0.2.17:5060`",
            ));
        };
        // A SIP URI contains no whitespace, so a third field is a trailing comment or a typo.
        // Taking the rest of the line as the URI would produce something undiallable and say
        // nothing about it until someone tried to call it.
        if fields.next().is_some() {
            return Err(malformed(
                "a peer is two fields; comments go on their own line, starting with `#`",
            ));
        }
        if !(uri.starts_with("sip:") || uri.starts_with("sips:")) {
            return Err(malformed("a peer's URI must start with `sip:` or `sips:`"));
        }

        peers.push(Peer {
            name: name.to_owned(),
            uri: uri.to_owned(),
            source: Source::Book,
            age: None,
        });
    }

    Ok(peers)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn book(contents: &str) -> Result<Vec<Peer>, Error> {
        parse(Path::new("/tmp/peers"), contents)
    }

    #[test]
    fn a_line_becomes_a_peer_carrying_the_source_it_came_from() {
        let peers = book("alice   sip:alice@192.0.2.17:5060\n").expect("a peer");
        assert_eq!(
            peers,
            vec![Peer {
                name: "alice".to_owned(),
                uri: "sip:alice@192.0.2.17:5060".to_owned(),
                source: Source::Book,
                age: None,
            }]
        );
        assert_eq!(peers[0].source.as_str(), "book");
    }

    /// File order, not sorted: it is the order the user wrote, it is stable between runs, and a
    /// script that appended a peer finds it where it put it.
    #[test]
    fn peers_are_listed_in_the_order_the_book_gives_them() {
        let peers = book("bob sip:bob@example.com\nalice sips:alice@example.com\n").expect("peers");
        assert_eq!(
            peers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["bob", "alice"]
        );
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let peers = book(
            "# who this phone knows about\n\
             \n\
             alice sip:alice@example.com\n\
             \t  \n\
                # indented comments too\n",
        )
        .expect("peers");
        assert_eq!(peers.len(), 1);
    }

    /// A readable book with nothing in it is an empty list and a success. That is a different
    /// fact from a book that could not be read, which is the failure below.
    #[test]
    fn an_empty_book_is_an_empty_list_and_not_an_error() {
        assert_eq!(book("").expect("no peers"), vec![]);
        assert_eq!(book("# nobody yet\n").expect("no peers"), vec![]);
    }

    #[test]
    fn a_line_that_is_not_a_peer_names_the_line_it_failed_on() {
        let error = book("alice sip:alice@example.com\nbob\n").expect_err("not a peer");
        let Error::Malformed { line, .. } = error else {
            panic!("expected a malformed line, got {error:?}");
        };
        assert_eq!(line, 2, "counted the way an editor counts");
    }

    /// Taking the rest of the line as the URI would make this peer `sip:a@b # home`, which
    /// nothing can dial and nothing complains about until someone tries.
    #[test]
    fn a_trailing_comment_is_refused_rather_than_glued_onto_the_uri() {
        let error = book("alice sip:alice@example.com # home\n").expect_err("two fields only");
        assert!(
            error.to_string().contains("own line"),
            "the error must say where a comment goes: {error}"
        );
    }

    #[test]
    fn something_that_is_not_a_sip_uri_is_refused() {
        for bad in ["alice example.com", "alice tel:+15551234", "alice alice"] {
            let error = book(bad).expect_err("not a URI");
            assert!(error.to_string().contains("sip:"), "{bad}: {error}");
        }
    }

    #[test]
    fn an_explicit_path_wins_over_everything_else() {
        let path = locate(
            Some("/books/mine"),
            Some("/books/env".to_owned()),
            Some("/config".to_owned()),
            Some("/home/someone".to_owned()),
        )
        .expect("a path");
        assert_eq!(path, PathBuf::from("/books/mine"));
    }

    #[test]
    fn the_environment_wins_over_the_config_directory() {
        let path = locate(
            None,
            Some("/books/env".to_owned()),
            Some("/config".to_owned()),
            Some("/home/someone".to_owned()),
        )
        .expect("a path");
        assert_eq!(path, PathBuf::from("/books/env"));
    }

    #[test]
    fn the_default_is_the_xdg_config_path() {
        let path = locate(None, None, Some("/config".to_owned()), None).expect("a path");
        assert_eq!(path, PathBuf::from("/config/sipx/peers"));

        let path = locate(None, None, None, Some("/home/someone".to_owned())).expect("a path");
        assert_eq!(path, PathBuf::from("/home/someone/.config/sipx/peers"));
    }

    /// An unset variable and one set to nothing are the same intent, and treating the empty
    /// string as a path looks for the book at the filesystem root.
    #[test]
    fn an_empty_variable_is_the_same_as_an_unset_one() {
        let path = locate(
            None,
            Some(String::new()),
            Some(String::new()),
            Some("/home/someone".to_owned()),
        )
        .expect("a path");
        assert_eq!(path, PathBuf::from("/home/someone/.config/sipx/peers"));

        assert!(matches!(
            locate(None, None, None, None),
            Err(Error::NoLocation)
        ));
    }

    /// The exits a script branches on. A missing location is something the caller can fix on
    /// the command line; a book that will not parse is not.
    #[test]
    fn a_book_that_cannot_be_read_never_exits_zero() {
        for error in [
            Error::NoLocation,
            Error::Unreadable {
                path: PathBuf::from("/tmp/peers"),
                cause: std::io::Error::from(std::io::ErrorKind::NotFound),
            },
            Error::Malformed {
                path: PathBuf::from("/tmp/peers"),
                line: 1,
                reason: "nope",
            },
        ] {
            assert_ne!(error.exit(), Exit::Success, "{error}");
        }
        assert_eq!(Error::NoLocation.exit(), Exit::Usage);
    }

    /// The two forms carry the same facts — `P-1`'s rule, applied to a list.
    #[test]
    fn both_forms_carry_the_name_the_uri_and_the_source() {
        let peer = Peer {
            name: "alice".to_owned(),
            uri: "sip:alice@192.0.2.17:5060".to_owned(),
            source: Source::Book,
            age: None,
        };
        let json = peer.report().render(Format::Json);
        let text = peer.report().render(Format::Text);

        for fact in ["alice", "sip:alice@192.0.2.17:5060", "book"] {
            assert!(json.contains(fact), "{fact} missing from {json}");
            assert!(text.contains(fact), "{fact} missing from {text}");
        }
        assert!(!json.contains('\n'), "one line per peer: {json}");
    }

    #[test]
    fn registrar_entries_report_source_and_snapshot_age() {
        let delivery = EventNotification {
            received_at: tokio::time::Instant::now(),
            metadata: None,
            value: RegistrationSnapshot {
                version: 0,
                peers: vec![sipx_ua::reginfo::RegistrationPeer {
                    name: "alice".to_owned(),
                    aor: "sip:alice@example.test".to_owned(),
                    uri: "sip:alice@192.0.2.10".to_owned(),
                    registration_id: "r1".to_owned(),
                    contact_id: "c1".to_owned(),
                    source: sipx_ua::reginfo::RegistrarSource {
                        resource: "sip:all@example.test".to_owned(),
                    },
                }],
            },
        };
        let peers = registrar_peers(delivery);
        assert_eq!(peers[0].source, Source::Registrar);
        let report = peers[0].report();
        assert_eq!(
            report.names(),
            vec!["status", "name", "uri", "source", "age"]
        );
        assert!(
            report
                .render(Format::Json)
                .contains("\"source\":\"registrar\"")
        );
    }

    #[test]
    fn live_target_keeps_secure_transport_selectors() {
        let target = Target::new(
            "192.0.2.10:7443".parse().expect("target"),
            TransportKind::Wss,
        )
        .verifying("registrar.example.test")
        .at_path("/events");
        let peer = event_peer(&target).expect("a Wss target has an event-client transport");
        assert_eq!(peer.transport, Transport::Wss);
        assert_eq!(peer.identity.as_deref(), Some("registrar.example.test"));
        assert_eq!(peer.path.as_deref(), Some("/events"));
    }

    #[test]
    fn registrar_refusals_have_scriptable_exits() {
        assert_eq!(
            termination(&Termination::Rejected(403)).exit,
            Exit::Unauthorized
        );
        assert_eq!(
            termination(&Termination::Rejected(489)).exit,
            Exit::Rejected
        );
        assert_eq!(
            termination(&Termination::NoInitialNotify).exit,
            Exit::Timeout
        );
    }

    /// `P-31`: which failures move the pass to the next address, and which end it.
    ///
    /// The whole of the candidate walk's correctness is this predicate. A registrar's refusal
    /// retried against a second address asks a question that has already been answered — and
    /// worse, a challenge that failed once would be re-presented with the same credentials at
    /// every address behind the name.
    #[test]
    fn only_a_transaction_that_never_answered_moves_to_the_next_address() {
        assert!(
            termination(&Termination::TransactionFailed).unreachable,
            "nothing answered at this address, so a later one still might"
        );
        for answered in [
            Termination::Rejected(403),
            Termination::Rejected(489),
            Termination::AuthenticationExhausted,
            Termination::NoInitialNotify,
            Termination::LocalExpiry,
            Termination::MalformedResponse,
        ] {
            assert!(
                !termination(&answered).unreachable,
                "{answered:?} is the registrar speaking, and a second address repeats the question"
            );
        }
    }
}
