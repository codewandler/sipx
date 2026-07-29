//! The user agent: registering, keeping registered, and answering what arrives.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{Address, HeaderName, Method, Request, StatusCode, Uri};
use sipx_transport::{Handle, Incoming, Target};

use crate::auth::Credentials;
use crate::error::{Error, Result};
use crate::gruu;
use crate::outbound::{self, InstanceId, RegId};
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
    /// The device identity this agent registers under (RFC 5626 §4.1, RFC 5627 §4.1).
    ///
    /// One field for both mechanisms, because both name the instance with the same
    /// `+sip.instance` media feature tag and a registrar that correlates them must see one
    /// value. Set it with [`Config::with_outbound`] or [`Config::with_gruu`]; whichever is
    /// called last decides, and either way there is only ever one identity to present.
    ///
    /// `None` registers the ordinary way: a `Contact` naming an address and nothing naming the
    /// device behind it, so every restart looks to the registrar like a new phone.
    pub instance: Option<InstanceId>,
    /// Which Outbound flow this registration is, when Outbound is in use (RFC 5626 §4.2).
    ///
    /// `None` registers the ordinary way: a binding that is only as durable as the NAT mapping
    /// behind it.
    pub reg_id: Option<RegId>,
    /// Which GRUU this agent uses, when it is asking for one (RFC 5627 §4.4).
    ///
    /// `None` does not ask. See [`gruu::Kind`] for why the choice is the application's.
    pub gruu: Option<gruu::Kind>,
    /// How a push notification service can wake this device (RFC 8599 §4.1.2).
    ///
    /// `None` registers without push, which is every client that holds a connection of its
    /// own. Set it with [`Config::with_push`]; the values come from the application's push
    /// service, behind [`crate::push::PushService`].
    pub push: Option<sipx_sip::push::Device>,
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
            instance: None,
            reg_id: None,
            gruu: None,
            push: None,
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
        self.instance = Some(flow.instance);
        self.reg_id = Some(flow.reg_id);
        self
    }

    /// Ask the registrar for a GRUU, and say which of the two to use (RFC 5627 §4.1, §4.4).
    ///
    /// The REGISTER gains the `gruu` option tag and presents `instance`; the GRUUs that come back
    /// are readable through [`UserAgent::gruus`] and are what
    /// [`UserAgent::dialog_contact`] then publishes.
    ///
    /// **`instance` is the same identity Outbound registers with**, and it is stored in the same
    /// field: a UA using both mechanisms presents one instance ID, because a registrar
    /// correlating them would otherwise see one device claiming to be two. Whether the registrar
    /// actually *issues* a GRUU is its business — §4.2 requires a UA to cope with one, both or
    /// neither, and getting neither is not an error.
    ///
    /// See [`gruu::Kind`] for why `Kind::Public` is the default and why asking for
    /// `Kind::Temporary` never quietly yields the public one.
    #[must_use]
    pub fn with_gruu(mut self, instance: InstanceId, kind: gruu::Kind) -> Self {
        self.instance = Some(instance);
        self.gruu = Some(kind);
        self
    }

    /// Add credentials.
    #[must_use]
    pub fn with_credentials(mut self, credentials: Credentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Be reachable through this push notification service (RFC 8599 §4.1.2).
    ///
    /// The `Contact` **URI** gains `pn-provider`, `pn-param` and `pn-prid` — inside the angle
    /// brackets, where a registrar's URI parser looks; §8.7 registers them as URI parameters
    /// and a `;` outside the brackets starts a different grammar entirely.
    ///
    /// Registering is only half the mechanism. When the push arrives, call
    /// [`UserAgent::woken`] — §4.1.3's binding-refresh REGISTER — *before* expecting the
    /// request the push was sent for, because until the refresh there is no flow for it to
    /// arrive on. And after any registration, ask [`UserAgent::push_support`] whether the
    /// registrar named this service: a 200 from a registrar that supports a different one is a
    /// binding nothing will ever wake.
    #[must_use]
    pub fn with_push(mut self, device: sipx_sip::push::Device) -> Self {
        self.push = Some(device);
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
    /// The GRUUs the registrar issued for this instance (RFC 5627 §4.2).
    ///
    /// Replaced on every 2xx and cleared whenever an attempt does not produce one, because a
    /// GRUU is only as valid as the binding behind it: §5.2 has a registrar stop resolving a
    /// temporary GRUU once nothing is bound to it, and §4.2 requires a UA to discard the ones it
    /// learned when its `Call-ID` changes. Keeping a stale one means publishing an address that
    /// no longer reaches anything, in the header a peer will route its next request by.
    gruus: gruu::Gruus,
    /// What the registrar said about push (RFC 8599 §8.2).
    ///
    /// Replaced on every 2xx and cleared when an attempt fails, for the reason the GRUUs are:
    /// it describes the registration that exists, and holding what an older one said would
    /// answer [`UserAgent::push_support`]'s question about a binding that is gone.
    push_support: crate::push::Support,
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
            instance: config.instance.clone(),
            reg_id: config.reg_id,
            gruu: config.gruu,
            push: config.push.clone(),
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
            gruus: gruu::Gruus::default(),
            push_support: crate::push::Support::default(),
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

    /// The GRUUs the registrar issued for this instance (RFC 5627 §4.2).
    ///
    /// Empty until a registration succeeds, empty afterwards if GRUU was not asked for, and empty
    /// again if it was and the registrar issued nothing — §4.2 requires a UA to be ready for one,
    /// both or neither, and a registrar that does not implement RFC 5627 answers a REGISTER
    /// perfectly well and attaches none.
    #[must_use]
    pub fn gruus(&self) -> &gruu::Gruus {
        &self.gruus
    }

    /// Whether a request that arrived was sent to one of this instance's GRUUs (RFC 5627 §4.5).
    ///
    /// This is the question the mechanism exists to make answerable, and it is not the question
    /// "is this request for me": an address of record reaches every device the user registered,
    /// and RFC 5627 §5.4 notes that a public GRUU "will always be equivalent to the AOR based on
    /// URI equality rules". A `true` here means the sender addressed *this* instance and nothing
    /// else — which is what a transfer target or a callback is relying on.
    #[must_use]
    pub fn sent_to_our_gruu(&self, request: &Request) -> bool {
        self.gruus.sent_to(&request.uri)
    }

    /// The `Contact` to put on a dialog-forming or target-refresh request (RFC 5627 §4.4,
    /// RFC 5626 §4.3).
    ///
    /// Three answers, in the order the RFCs put them:
    ///
    /// - **The GRUU**, when one is known. §4.4: "A UA SHOULD use a GRUU when populating the
    ///   Contact header field of dialog-forming and target refresh requests and responses." It is
    ///   an address that survives this flow, this NAT mapping and this registration, which is
    ///   more than either of the others can say.
    /// - **The contact with `ob`**, when this is an accepted flow and no GRUU is known. RFC 5626
    ///   §4.3 makes that a MUST *in the absence of a GRUU*, and it tells the far end that
    ///   mid-dialog requests belong on this flow rather than at the address in the URI — behind a
    ///   NAT, the difference between a re-INVITE arriving and vanishing.
    /// - **The plain contact**, when neither applies.
    ///
    /// A caller that asked for a temporary GRUU and did not get one lands in the second or third
    /// case, never the first: the public GRUU is not a substitute for an unlinkable address, and
    /// quietly publishing the device's permanent name to a peer that was promised otherwise is a
    /// worse outcome than publishing the contact. It is logged, because the caller asked for
    /// something it did not get.
    #[must_use]
    pub fn dialog_contact(&self) -> String {
        if let Some(kind) = self.config.gruu {
            if let Some(gruu) = self.gruus.preferred(kind) {
                return format!("<{gruu}>");
            }
            if kind == gruu::Kind::Temporary {
                tracing::warn!(
                    "no temporary GRUU was issued; publishing the contact rather than the public \
                     GRUU, which would not be unlinkable"
                );
            }
        }
        if self.flow_accepted {
            crate::outbound::with_ob(&self.config.contact)
        } else {
            self.config.contact.clone()
        }
    }

    /// What the registrar said about push notifications (RFC 8599 §8.2).
    ///
    /// Empty until a registration succeeds, and empty afterwards when the registrar implements
    /// nothing of RFC 8599 — which is not a refusal, just silence. The question to ask it is
    /// [`Support::supports`](crate::push::Support::supports) with the provider this side
    /// registered: a registrar that answered 200 while naming a *different* push service has
    /// recorded a binding nothing will ever wake, and this is the only place that says so.
    #[must_use]
    pub fn push_support(&self) -> &crate::push::Support {
        &self.push_support
    }

    /// A push notification arrived: refresh the binding, and only then expect the request
    /// (RFC 8599 §4.1.3).
    ///
    /// §4.1.3: "When a UA receives a push notification, the UA MUST send a binding-refresh
    /// REGISTER request." The push is not the call — it is permission to go and get a flow,
    /// and the request the push was sent for arrives down the flow this REGISTER creates. A
    /// client that skips this and waits for the INVITE is waiting on a path that does not
    /// exist yet, which is why the [`Pending`](crate::push::Pending) that licenses the wait
    /// comes from here and nowhere else.
    ///
    /// sipx neither sends nor receives the push itself: the service is behind
    /// [`crate::push::PushService`], and *when* to call this is the application's — it is
    /// whatever "the notification fired" means on its platform.
    pub async fn woken(&mut self) -> Result<crate::push::Pending> {
        let lease = self.register().await?;
        Ok(crate::push::Pending {
            lease,
            purr: self.push_support.purr().map(str::to_owned),
        })
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
                    gruus,
                    push,
                } = *registered;
                self.flow_accepted = flow_accepted;
                self.flow_timer = flow_timer;
                self.path = path;
                // Replaced, never merged — the same rule as the service route, and for a
                // stronger reason: RFC 5627 §4.2 requires temporary GRUUs learned earlier to be
                // discarded outright rather than kept alongside, and a set that merges cannot
                // tell which of the two it is holding.
                self.gruus = gruus;
                // Replaced, never merged, like the GRUUs: this is what the registrar said about
                // the binding it just recorded, and what it said about an earlier one answers
                // "can this registrar wake me" for a binding that no longer exists.
                self.push_support = push;
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
            // The binding did not happen, so neither did the GRUUs that hang off it. Holding
            // them would leave the agent publishing an address for a registration it does not
            // have (RFC 5627 §5.2).
            Outcome::Challenged(_) => {
                self.gruus = gruu::Gruus::default();
                self.push_support = crate::push::Support::default();
                Err(Error::AuthenticationFailed)
            }
            // §8.1's one answer that is not about this attempt: the named push service will not
            // become usable on a retry, so it surfaces as itself rather than as the rejection
            // below — folded in, it is indistinguishable from a bad password.
            Outcome::PushNotSupported { reason } => {
                self.gruus = gruu::Gruus::default();
                self.push_support = crate::push::Support::default();
                Err(Error::PushNotSupported { reason })
            }
            Outcome::Rejected { status, reason } => {
                self.gruus = gruu::Gruus::default();
                self.push_support = crate::push::Support::default();
                Err(Error::Rejected { status, reason })
            }
        }
    }

    async fn attempt(&self, request: Request) -> Result<Outcome> {
        let mut responses = self
            .endpoint
            .send(request, self.config.target.clone())
            .await?;
        let response = responses.final_response().await.ok_or(Error::NoResponse)?;
        Ok(registrar::interpret(&response, &self.registration))
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
