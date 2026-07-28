//! The user agent: registering, keeping registered, and answering what arrives.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{Address, HeaderName, Method, Request, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};

use crate::auth::Credentials;
use crate::error::{Error, Result};
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
}

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
        }
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
        };
        Self {
            endpoint,
            config,
            registration,
            nonce_use: None,
            path: registrar::PathSet::default(),
            service_route: registrar::ServiceRoute::default(),
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
                } = *registered;
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
