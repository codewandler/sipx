//! The user agent: registering, keeping registered, and answering what arrives.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Method, Request, StatusCode, Uri};
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
    nonce_count: u32,
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
            nonce_count: 0,
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
            self.nonce_count = self.nonce_count.saturating_add(1);
            request = self.registration.request()?;
            registrar::authorize(&mut request, &challenge, credentials, self.nonce_count)?;
            outcome = self.attempt(request).await?;

            // A stale nonce is the server asking for the same credentials against a fresh
            // nonce, not a refusal. One further attempt, then stop.
            if let Outcome::Challenged(again) = &outcome {
                if again.stale {
                    self.registration.advance();
                    self.nonce_count = self.nonce_count.saturating_add(1);
                    let mut retry = self.registration.request()?;
                    registrar::authorize(&mut retry, again, credentials, self.nonce_count)?;
                    outcome = self.attempt(retry).await?;
                }
            }
        }

        match outcome {
            Outcome::Registered(lease) => Ok(lease),
            Outcome::Challenged(_) => Err(Error::AuthenticationFailed),
            Outcome::Rejected { status, reason } => Err(Error::Rejected { status, reason }),
        }
    }

    async fn attempt(&self, request: Request) -> Result<Outcome> {
        let mut responses = self.endpoint.send(request, self.config.target).await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;
        Ok(registrar::interpret(&response, self.config.expires))
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
        let response = ResponseBuilder::to_request(
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
        )?
        .build();
        self.endpoint.respond(&incoming.key, response).await?;
        Ok(())
    }

    /// The transport handle this agent sends through.
    #[must_use]
    pub fn endpoint(&self) -> &Handle {
        &self.endpoint
    }
}
