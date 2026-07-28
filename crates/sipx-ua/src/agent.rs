//! The user agent: registering, keeping registered, and answering what arrives.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{Address, HeaderName, Method, Request, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};

use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::outbound;
use crate::registrar::{self, Lease, Outcome, Registration};

/// How a user agent is configured.
#[derive(Debug, Clone)]
pub struct Config {
    /// The address of record, as it appears in `To` and `From`.
    pub aor: String,
    /// Where to reach this agent.
    pub contact: String,
    /// The registrar's URI.
    pub registrar: Uri,
    /// Where to send registrations.
    pub target: Target,
    /// Credentials, if the registrar wants them.
    pub credentials: Option<Credentials>,
    /// The lease to ask for.
    pub expires: Duration,
    /// What to put in `User-Agent`.
    pub user_agent: String,
    /// Which flow this registration is, when Outbound is in use (RFC 5626).
    ///
    /// `None` registers the ordinary way: a `Contact` naming an address, and a binding that is
    /// only as durable as the NAT mapping behind it.
    pub outbound: Option<Flow>,
    /// How long a keep-alive may go unanswered before the flow is failed (RFC 5626 §4.4).
    ///
    /// Defaults to §4.4.1's ten seconds. It is configurable because the RFC gives *two* rules and
    /// only one of them is a duration: §4.4.1 fixes ten seconds for the CRLF pong, while §4.4.2
    /// bounds the STUN case by 7 retransmissions of an RTO estimate instead. Ten seconds is the
    /// conservative reading of both, and a deployment that knows its round-trip times — or a test
    /// that does not want to wait — is entitled to a shorter one.
    pub keepalive_timeout: Duration,
}

pub use crate::outbound::Flow;

impl Config {
    /// A configuration for an address of record.
    #[must_use]
    pub fn new(
        aor: impl Into<String>,
        contact: impl Into<String>,
        registrar: Uri,
        target: Target,
    ) -> Self {
        Self {
            aor: aor.into(),
            contact: contact.into(),
            registrar,
            target,
            credentials: None,
            expires: Duration::from_secs(3600),
            user_agent: concat!("sipx/", env!("CARGO_PKG_VERSION")).to_owned(),
            outbound: None,
            keepalive_timeout: outbound::PONG_TIMEOUT,
        }
    }

    /// Fail a flow whose keep-alive is unanswered for this long (RFC 5626 §4.4).
    #[must_use]
    pub fn with_keepalive_timeout(mut self, within: Duration) -> Self {
        self.keepalive_timeout = within;
        self
    }

    /// Register this contact as one Outbound flow (RFC 5626).
    ///
    /// The `Contact` gains `reg-id` and `+sip.instance`, and the REGISTER offers the `outbound`
    /// option tag. Whether the registrar actually *did* an outbound registration is a separate
    /// question, answered by `UserAgent::flow_accepted` after the fact — §6 has the registrar say
    /// so in `Require`, and a UA that assumes it would keep a flow alive that nothing routes down.
    #[must_use]
    pub fn with_outbound(mut self, flow: Flow) -> Self {
        self.outbound = Some(flow);
        self
    }

    /// Add credentials.
    #[must_use]
    pub fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = Some(credentials);
        self
    }
}

/// A user agent bound to a transport endpoint.
#[derive(Debug)]
pub struct UserAgent {
    endpoint: Handle,
    config: Config,
    registration: Registration,
    /// The last nonce answered, and how many requests have used it. RFC 7616 §3.4.3 defines
    /// `nc` per nonce, not per client, so the pair travels together.
    nonce_use: Option<(String, u32)>,
    /// The proxies the registrar recorded as being on the path back here (RFC 3327).
    path: registrar::PathSet,
    /// The proxies this UA's own outbound requests must traverse (RFC 3608).
    service_route: registrar::ServiceRoute,
    /// Whether the registrar reported an *outbound* registration (RFC 5626 §6).
    flow_accepted: bool,
    /// The `Flow-Timer` the registrar named, if it named one (RFC 5626 §4.4).
    flow_timer: Option<Duration>,
    /// The reflexive address the last keep-alive reported (RFC 5626 §4.4.2).
    ///
    /// Kept because a *change* in it is a flow failure: the NAT has rebound, so the address the
    /// registrar has for this flow no longer reaches it, even though the socket still works.
    reflexive: Option<std::net::SocketAddr>,
}

impl UserAgent {
    /// A user agent that will send through `endpoint`.
    #[must_use]
    pub fn new(endpoint: Handle, config: Config) -> Self {
        let registration = Registration {
            registrar: config.registrar.clone(),
            aor: config.aor.clone(),
            contact: config.contact.clone(),
            expires: config.expires,
            call_id: format!("{}@sipx", crate::auth::new_cnonce()),
            // Zero, not one: `register` advances before building, so the first request is 1
            // and every later one is strictly greater. A REGISTER that reuses a sequence
            // number inside the same Call-ID is out of order, and a registrar is entitled to
            // ignore it — which looks exactly like the refresh silently not happening.
            cseq: 0,
            outbound: config.outbound.clone(),
        };
        Self {
            endpoint,
            config,
            registration,
            nonce_use: None,
            path: registrar::PathSet::default(),
            service_route: registrar::ServiceRoute::default(),
            flow_accepted: false,
            flow_timer: None,
            reflexive: None,
        }
    }

    /// The path the registrar recorded for this binding (RFC 3327).
    ///
    /// Empty until a registration succeeds, and empty afterwards if no proxy on the way put
    /// itself on the path. This is reported rather than routed on: §5.1 says "the general
    /// operation of the UA is to ignore the Path header field in the response", because the
    /// vector exists so that requests arriving *at the registrar* can be steered back toward a
    /// UA behind a NAT. What §5.1 does offer it for is inspection — seeing a proxy that has
    /// "inappropriately added" itself — and that is only possible if the value survives.
    #[must_use]
    pub fn path(&self) -> &registrar::PathSet {
        &self.path
    }

    /// The route the registrar dictated for requests this UA sends (RFC 3608).
    ///
    /// The opposite direction from [`UserAgent::path`], and the one a UA is meant to act on:
    /// §6.1 has it used "as a preloaded Route header field in outgoing initial requests". sipx
    /// does not preload it behind the caller's back — a `Route` set silently attached to every
    /// request is the kind of thing that is impossible to debug from the outside — so this is
    /// handed to whoever builds the request, via `DialOptions::with_service_route` for a call.
    ///
    /// Empty until a registration succeeds, and empty again after any 2xx that carries no
    /// `Service-Route`.
    #[must_use]
    pub fn service_route(&self) -> &registrar::ServiceRoute {
        &self.service_route
    }

    /// Whether the registrar reported performing an Outbound registration (RFC 5626 §6).
    ///
    /// False until a registration succeeds, and false afterwards if the registrar did not put the
    /// option tag in `Require` — which is the case for every registrar that does not implement
    /// RFC 5626 at all. Asking for Outbound and not getting it is not an error: the binding is an
    /// ordinary one, and the only thing that changes is that there is no flow to keep alive.
    #[must_use]
    pub fn flow_accepted(&self) -> bool {
        self.flow_accepted
    }

    /// How long to wait before the next keep-alive on this flow, if it is one (RFC 5626 §4.4).
    ///
    /// `None` when the registrar did not perform an Outbound registration — there is no flow, so
    /// pinging would be traffic with nothing at the far end that cares. Re-drawn on every call,
    /// because §4.4.1 requires a fresh random interval for each ping: a fleet on a fixed period
    /// synchronises after any shared outage and arrives back as one spike.
    #[must_use]
    pub fn keepalive_after(&self, power: crate::outbound::Power) -> Option<Duration> {
        self.flow_accepted.then(|| {
            crate::outbound::keepalive_interval(
                self.flow_timer,
                crate::outbound::keepalive_for(self.config.target.transport),
                power,
                crate::outbound::fraction(),
            )
        })
    }

    /// Send one keep-alive on this flow and judge the answer (RFC 5626 §4.4).
    ///
    /// Three ways this reports a failed flow, and §4.4 makes each of them one:
    ///
    /// - no answer within [`outbound::PONG_TIMEOUT`] (§4.4.1),
    /// - a STUN Binding Error Response (§4.4.2),
    /// - a reflexive address **different from the last one** (§4.4.2).
    ///
    /// The third is the one that is easy to leave out and the reason STUN is the UDP technique at
    /// all. The socket still works; what has changed is that the NAT rebound, so the mapping the
    /// registrar holds for this flow no longer reaches it. A keep-alive that only asked "did
    /// anything come back" would call that flow healthy right up until a call failed to arrive.
    ///
    /// `Ok(())` on a flow the registrar did not accept: there is no flow, so there is nothing to
    /// keep alive and nothing has failed.
    pub async fn keepalive(&mut self) -> Result<()> {
        if !self.flow_accepted {
            return Ok(());
        }
        let mapped = self
            .endpoint
            .keepalive(self.config.target.clone(), self.config.keepalive_timeout)
            .await?;
        if let (Some(previous), Some(current)) = (self.reflexive, mapped)
            && previous != current
        {
            self.reflexive = Some(current);
            return Err(Error::FlowRebound { previous, current });
        }
        if mapped.is_some() {
            self.reflexive = mapped;
        }
        Ok(())
    }

    /// The reflexive address the last keep-alive reported, if one did (RFC 5626 §4.4.2).
    #[must_use]
    pub fn reflexive_address(&self) -> Option<std::net::SocketAddr> {
        self.reflexive
    }

    /// The `Contact` to put on a dialog-forming request (RFC 5626 §4.3).
    ///
    /// Carries `ob` when this is an accepted flow and no GRUU is in play: §4.3 makes that a MUST,
    /// and it is what tells the far end that mid-dialog requests belong on *this flow* rather than
    /// at the address in the URI — which, behind a NAT, is the difference between a re-INVITE
    /// arriving and vanishing.
    #[must_use]
    pub fn dialog_contact(&self) -> String {
        if self.flow_accepted {
            crate::outbound::with_ob(&self.config.contact)
        } else {
            self.config.contact.clone()
        }
    }

    /// Register, answering a challenge if one comes.
    ///
    /// One retry, not a loop. A second challenge after credentials were supplied means the
    /// credentials are wrong — unless the server says the nonce was merely stale, which is a
    /// different thing and is retried. Looping on a genuine rejection is how a client locks
    /// out the account it is trying to use.
    pub async fn register(&mut self) -> Result<Lease> {
        self.registration.advance();
        let mut request = self.registration.request()?;
        let mut outcome = self.attempt(request.clone()).await?;

        if let Outcome::Challenged(challenge) = outcome {
            let credentials = self
                .config
                .credentials
                .as_ref()
                .ok_or(Error::CredentialsRequired)?;

            self.registration.advance();
            let count = nonce_count_for(&mut self.nonce_use, &challenge.nonce);
            request = self.registration.request()?;
            registrar::authorize(&mut request, &challenge, credentials, count)?;
            outcome = self.attempt(request).await?;

            // A stale nonce is the server asking for the same credentials against a fresh
            // nonce, not a refusal. One further attempt, then stop.
            if let Outcome::Challenged(again) = &outcome
                && again.stale
            {
                self.registration.advance();
                let count = nonce_count_for(&mut self.nonce_use, &again.nonce);
                let mut retry = self.registration.request()?;
                registrar::authorize(&mut retry, again, credentials, count)?;
                outcome = self.attempt(retry).await?;
            }
        }

        match outcome {
            Outcome::Registered(registered) => {
                let registrar::Registered {
                    lease,
                    path,
                    service_route,
                    flow_accepted,
                    flow_timer,
                } = *registered;
                self.flow_accepted = flow_accepted;
                self.flow_timer = flow_timer;
                self.path = path;
                // Replaced on every success, never merged. RFC 3608 §6.1: the stored value is
                // "updated according to the Service-Route header field of the latest 200 class
                // response", and a response with no such header "clears any service route ...
                // previously stored". Both are one rule, and assignment is it.
                for hop in service_route.hops_without_loose_routing() {
                    tracing::warn!(
                        hop = %hop,
                        "the registrar's Service-Route omits ;lr, which RFC 3608 §5 requires"
                    );
                }
                self.service_route = service_route;
                Ok(lease)
            }
            Outcome::Challenged(_) => Err(Error::AuthenticationFailed),
            Outcome::Rejected { status, reason } => Err(Error::Rejected { status, reason }),
        }
    }

    async fn attempt(&self, request: Request) -> Result<Outcome> {
        let mut responses = self
            .endpoint
            .send(request, self.config.target.clone())
            .await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;
        Ok(registrar::interpret(
            &response,
            self.config.expires,
            &self.registration.contact,
        ))
    }

    /// Register and keep registering, refreshing before each lease expires.
    ///
    /// Returns only if a refresh fails outright; a caller that wants to survive a transient
    /// failure should restart it.
    pub async fn keep_registered(&mut self) -> Result<std::convert::Infallible> {
        loop {
            let lease = self.register().await?;
            tracing::info!(
                granted = lease.granted.as_secs(),
                refresh_in = lease.refresh_after.as_secs(),
                "registered"
            );
            tokio::time::sleep(lease.refresh_after).await;
        }
    }

    /// Answer a request that arrived.
    ///
    /// Handles what a user agent must answer to be a good citizen on the network; anything
    /// else is left to the caller, which is why this returns whether it acted.
    pub async fn answer(&self, incoming: &Incoming) -> Result<bool> {
        match incoming.request.method {
            Method::Options => {
                self.answer_options(incoming).await?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Answer an `OPTIONS` ping (RFC 3261 §11.2).
    ///
    /// The point of OPTIONS is the capability list, so a 200 with an empty `Allow` is a wasted
    /// exchange: the peer asked what we can do and learned nothing.
    async fn answer_options(&self, incoming: &Incoming) -> Result<()> {
        let mut builder = ResponseBuilder::to_request(
            &incoming.request,
            StatusCode::new(200).ok_or(Error::NoResponse)?,
            "OK",
        )?
        .header(
            HeaderName::Allow,
            Bytes::from_static(b"INVITE, ACK, CANCEL, BYE, OPTIONS"),
        )?
        .header(HeaderName::Accept, Bytes::from_static(b"application/sdp"))?
        .header(
            HeaderName::UserAgent,
            Bytes::from(self.config.user_agent.clone()),
        )?;
        // RFC 3261 §8.2.6.2: every response but a 100 must carry a `To` tag, and an
        // out-of-dialog request arrives without one, so the tag is added rather than
        // copied. `new_cnonce` gives 64 random bits, which covers §19.3's demand for global
        // uniqueness with at least 32 bits of randomness. Appending works in both forms of
        // the header: after `>` in a name-addr, and after a bare addr-spec, where the
        // semicolon starts a header parameter (RFC 3261 §20).
        if let Some(to) = tagless_to(&incoming.request) {
            builder = builder.set_header(
                &HeaderName::To,
                Bytes::from(format!("{to};tag={}", crate::auth::new_cnonce())),
            )?;
        }
        let response = builder.build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// The transport handle this agent sends through.
    #[must_use]
    pub fn endpoint(&self) -> &Handle {
        &self.endpoint
    }
}

/// The request's `To` value, when it arrived without a tag and needs one added.
///
/// `None` also covers a `To` that does not parse: `ResponseBuilder::to_request` copies it
/// verbatim so a malformed request still gets a well-formed answer, and appending a tag to
/// a value whose shape is unknown could change what the rest of it means.
fn tagless_to(request: &Request) -> Option<String> {
    let value = request.headers.value(&HeaderName::To)?;
    let address = Address::parse(&value, "To").ok()?;
    address
        .tag()
        .is_none()
        .then(|| String::from_utf8_lossy(&value).into_owned())
}

/// The `nc` for a request about to answer `nonce`, recorded in `nonce_use`.
///
/// RFC 7616 §3.4.3: `nc` counts the requests sent *with this nonce*, so a nonce not seen
/// before starts at one — including the fresh nonce a stale challenge carries. A count
/// carried across nonces looks like a replay to the registrar that tracks it, which is the
/// registrar the count exists to satisfy.
fn nonce_count_for(nonce_use: &mut Option<(String, u32)>, nonce: &str) -> u32 {
    let count = match nonce_use {
        Some((last, count)) if last == nonce => count.saturating_add(1),
        _ => 1,
    };
    *nonce_use = Some((nonce.to_owned(), count));
    count
}
