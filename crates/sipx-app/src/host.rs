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
//! **What this host deliberately is not.** It runs no app callback, because there is nothing to call
//! yet: the transports that carry `sipx.app.v1` to customer code are `A-2` (documents), `A-4`
//! (sessions) and `A-5` (the embedded runtime), and all three are still open. That absence is not
//! papered over — it is routed through the document's own §9.2 declaration as
//! [`OnFailure`]`::on_unreachable`, which is the honest name for "the app could not be reached at
//! all". A host whose operator wrote `on_unreachable = "reject"` refuses the call and says so; one
//! who wrote the §9.2 default answers it and holds it until the caller hangs up. The knob is
//! load-bearing here rather than decorative, which is what makes it a knob and not a comment.

use std::fmt;
use std::net::IpAddr;

use sipx_call::{Dispatched, Dispatcher, Invitation};
use sipx_sip::{HeaderName, StatusCode, Uri, build::ResponseBuilder, uri::Host as UriHost};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use sipx_ua::{Config as AgentConfig, UserAgent};
use tokio::sync::mpsc;

use crate::config::{Admission, ConfigError, HostConfig, Listener, Protocol, Running};
use crate::harness::policy::OnFailure;

/// The status a host answers with when it cannot place a call at all — a bug on this side, and
/// never silence. RFC 3261 §21.5.1.
const INTERNAL_ERROR: u16 = 500;

/// The status for a request the host understands the shape of and has no user agent to answer:
/// an OPTIONS or a REGISTER arriving on a call listener. RFC 3261 §21.5.2.
const NOT_IMPLEMENTED: u16 = 501;

/// Why a host could not start, or could not keep running.
///
/// Not `#[non_exhaustive]` and deliberately small: a host either read its document, bound its
/// listener and carried its calls, or it failed at one of those three, and a caller that wants to
/// print the reason needs no more resolution than that.
#[derive(Debug)]
pub enum HostError {
    /// The configuration document was refused. Carries the document's own diagnosis, which names
    /// the line.
    Config(ConfigError),
    /// A listener could not be bound, or a response could not be sent.
    Transport(sipx_transport::Error),
    /// The document declares no `sip` listener, so there is nothing for a call to arrive on. A
    /// document can be valid and still describe a host that cannot answer anything.
    NoCallListener,
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Transport(error) => write!(formatter, "{error}"),
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
            Self::NoCallListener => None,
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

/// A host with a configuration in force.
///
/// Constructed from a document rather than from parts, because the document is the unit a reload
/// applies in (N9) and a host assembled field by field could not honour that.
#[derive(Debug)]
pub struct Host {
    running: Running,
    media_address: IpAddr,
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
        let config = HostConfig::parse(document)?;
        Ok(Self {
            running: Running::start(config),
            media_address,
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
        let listener = self.call_listener()?;
        let (handle, incoming) = bind(Self::endpoint_config(&listener)).await?;
        self.serve(handle, incoming).await
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

        while let Some(dispatched) = dispatcher.next().await {
            for call in drain(&mut endings) {
                self.running.end(&call);
            }
            match dispatched {
                Dispatched::Invitation(invitation) => {
                    self.admit(&handle, &listener, invitation, &ended).await;
                }
                Dispatched::OutOfDialog(request) => {
                    answer_out_of_dialog(&agent, &handle, &request).await;
                }
                _ => {}
            }
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
                // There is no app process to call yet (`A-2`, `A-4`, `A-5`), so the app is
                // unreachable in the document's own vocabulary, and §9.2 has already been told
                // what to do about that.
                match policy.failure.on_unreachable {
                    OnFailure::Reject { status } => {
                        refuse(handle, invitation.request(), status, "Unavailable").await;
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
                refuse(handle, invitation.request(), status, "Declined").await;
            }
            // A listener that is not there, or a session listener a call arrived on. Both are
            // host bugs rather than the caller's problem, and neither is ever silence (N6).
            Admission::NoSuchListener | Admission::NotACallListener => {
                refuse(
                    handle,
                    invitation.request(),
                    INTERNAL_ERROR,
                    "Server Internal Error",
                )
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
    /// The host runs no app callback yet, so *every* admitted call takes the `on_unreachable` branch.
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
}
