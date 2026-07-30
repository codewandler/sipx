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
    async fn admit(&mut self, handle: &Handle, listener: &Listener, invitation: Invitation) {
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
                    OnFailure::Hangup | OnFailure::Continue => {
                        let hang_up_at_once =
                            matches!(policy.failure.on_unreachable, OnFailure::Hangup);
                        self.carry(handle, invitation, hang_up_at_once).await;
                        self.running.end(&call_id);
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
    async fn carry(&self, handle: &Handle, invitation: Invitation, hang_up_at_once: bool) {
        let call = match invitation.answer(handle, self.media_address).await {
            Ok(call) => call,
            // The caller gave up, or the answer could not be sent. Either way there is no call
            // to carry and nothing useful to say to a peer that is already gone.
            Err(_) => return,
        };
        let (_, inbox) = invitation.into_parts();
        tokio::spawn(async move {
            let mut call = call;
            let mut inbox = inbox;
            if hang_up_at_once {
                let _ = call.hang_up().await;
                return;
            }
            // `serve` is the one loop: it honours the RFC 4028 session timer, answers what the
            // call does not claim rather than dropping it, and returns when the call ends.
            let _ = sipx_call::serve(&mut call, &mut inbox).await;
            if !call.is_ended() {
                let _ = call.hang_up().await;
            }
        });
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
    const DOCUMENT: &str = "\
[listener.edge]
protocol = \"sip\"
bind = \"127.0.0.1:0\"
transport = \"udp\"
app = \"greeter\"

[app.greeter]
url = \"https://example.net/greeter\"
";

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
}
