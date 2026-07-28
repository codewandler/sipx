//! Registration (RFC 3261 §10): telling a registrar where to reach you, and keeping it told.
//!
//! A registration is not a request, it is a lease. The interesting parts are all about the
//! lease rather than the message:
//!
//! - The registrar decides the expiry, not the client. Asking for 3600 and being granted 60 is
//!   normal, and a client that refreshes on its own number instead of the granted one
//!   de-registers itself every time.
//! - The refresh has to happen *before* the lease ends, with enough margin to retry. sipx uses
//!   90% of the granted interval, floored so a very short lease still leaves room.
//! - `Call-ID` stays the same across refreshes and `CSeq` increases. A new `Call-ID` makes it a
//!   new registration rather than a refresh, which is how a client ends up with duplicate
//!   contacts at the registrar.
//! - A 401 or 407 is expected on the first attempt, not an error.

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::ContactValue;
use sipx_sip::{Address, HeaderName, Method, Request, Response, Uri};

use crate::auth::{Challenge, Credentials, new_cnonce, respond, strongest};

/// How long a registration lease has left, and when to refresh it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    /// What the registrar granted.
    pub granted: Duration,
    /// When to refresh.
    pub refresh_after: Duration,
}

impl Lease {
    /// The refresh point for a granted interval.
    ///
    /// 90% of the lease, so a failed refresh still has time for the transaction to time out
    /// and be retried before the registration actually lapses. A refresh at 100% is a
    /// registration that lapses whenever a single packet is lost.
    #[must_use]
    pub fn from_granted(granted: Duration) -> Self {
        let seconds = granted.as_secs();
        let refresh = if seconds <= 20 {
            // Very short leases are used by test harnesses and some SBCs. Ten seconds of
            // margin does not fit in a 15-second lease, so fall back to half.
            seconds / 2
        } else {
            seconds * 9 / 10
        };
        Self {
            granted,
            refresh_after: Duration::from_secs(refresh.max(1)),
        }
    }
}

/// A successful registration: the lease, and the two route vectors that came back with it.
///
/// A struct rather than three positional fields on the enum variant, because `PathSet` and
/// `ServiceRoute` are the same shape and opposite directions — `Path` routes requests *toward*
/// this UA and is not ours to follow, `Service-Route` routes the requests we *send*. Positionally
/// interchangeable arguments of identical type are how they would eventually get swapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    /// What the registrar granted, and when to refresh it.
    pub lease: Lease,
    /// The proxies the registrar recorded on the path back to this contact (RFC 3327).
    pub path: PathSet,
    /// The proxies this UA's own outbound requests must traverse (RFC 3608).
    pub service_route: ServiceRoute,
    /// Whether the registrar reports having performed an *outbound* registration (RFC 5626 §6).
    ///
    /// §6 requires a registrar that did to say so in `Require`. Believing it happened without
    /// being told means keeping a flow alive that nothing is routing down, and treating a binding
    /// that is only as durable as its NAT mapping as though it were durable.
    pub flow_accepted: bool,
    /// The `Flow-Timer` the registrar named, if any (RFC 5626 §4.4).
    ///
    /// How long it will hold the flow open without traffic. When present it replaces the UA's own
    /// choice of keep-alive interval outright.
    pub flow_timer: Option<Duration>,
}

/// What a registration attempt produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Registered, with the lease the registrar granted and the route vectors it returned.
    Registered(Box<Registered>),
    /// The registrar wants credentials. Answer with [`authorize`] and send again.
    Challenged(Box<Challenge>),
    /// The registrar refused.
    Rejected {
        /// The status code.
        status: u16,
        /// Its reason phrase.
        reason: String,
    },
}

/// What to register.
#[derive(Debug, Clone)]
pub struct Registration {
    /// The registrar's URI, which is the Request-URI of the REGISTER.
    pub registrar: Uri,
    /// The address of record being registered.
    pub aor: String,
    /// Where to reach this user agent.
    pub contact: String,
    /// How long a lease to ask for.
    pub expires: Duration,
    /// The `Call-ID`, constant across refreshes.
    pub call_id: String,
    /// The `CSeq`, increasing across refreshes.
    pub cseq: u32,
    /// Whether this registration is one Outbound flow (RFC 5626), and which.
    ///
    /// When set, the `Contact` carries `reg-id` and `+sip.instance` and the REGISTER offers the
    /// `outbound` option tag.
    pub outbound: Option<crate::outbound::Flow>,
}

/// The proxies a registrar recorded as being on the path back to this contact (RFC 3327).
///
/// Held and reported, not routed on. RFC 3327 §5.1 is explicit that "the general operation of
/// the UA is to ignore the Path header field in the response" — the path vector exists so that
/// requests arriving *at* the registrar can be routed toward a UA behind a NAT, and it is the
/// registrar that walks it, not the UA. A UA that turned it into a pre-loaded route set would
/// be sending its own requests through proxies that never asked to carry them; the header for
/// that job is `Service-Route` (RFC 3608), which is a different list with different semantics.
///
/// What §5.1 does say it is for is inspection: "such inspection might allow the UA to detect
/// intermediate proxies that have inappropriately added themselves". That is only possible if
/// the value survives, which is why it is kept rather than parsed and dropped.
#[derive(Debug, Clone, Default)]
pub struct PathSet(pub Vec<Address>);

impl PathSet {
    /// The proxies, outermost first — the order they appeared in, which is the order a request
    /// travelling toward the UA would traverse them.
    #[must_use]
    pub fn hops(&self) -> &[Address] {
        &self.0
    }

    /// Whether the registrar recorded no path at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Each hop rendered back to the form it arrived in, for logging and for comparison.
    #[must_use]
    pub fn rendered(&self) -> Vec<String> {
        render_hops(&self.0)
    }

    /// Whether a proxy this side did not expect is on the path.
    ///
    /// RFC 3327 §5.1 gives inspection as the UA's reason to care: "such inspection might allow
    /// the UA to detect intermediate proxies that have inappropriately added themselves". That
    /// judgement needs a policy the UA holds, so this asks the question and leaves the answer
    /// to the caller rather than inventing a trust rule here.
    #[must_use]
    pub fn hops_outside(&self, expected: &[&str]) -> Vec<String> {
        self.rendered()
            .into_iter()
            .filter(|hop| !expected.iter().any(|allowed| hop.contains(allowed)))
            .collect()
    }

    /// Read the path vector out of a REGISTER response.
    ///
    /// Parsed rather than kept as text, and kept as [`Address`] rather than as a URI, because
    /// the parameters are load-bearing: RFC 5626 §5.3 hangs the `ob` marker off a `Path` value,
    /// and `T-15` needs to read it. A path vector flattened to a list of URIs would be
    /// syntactically fine and quietly useless for Outbound.
    #[must_use]
    pub fn from_response(response: &Response) -> Self {
        Self(
            response
                .headers
                .typed_all::<sipx_sip::headers::address::Path>()
                .filter_map(std::result::Result::ok)
                .map(|path| path.0)
                .collect(),
        )
    }
}

impl PartialEq for PathSet {
    fn eq(&self, other: &Self) -> bool {
        self.rendered() == other.rendered()
    }
}

impl Eq for PathSet {}

/// The proxies a registrar says this UA's own requests must traverse (RFC 3608).
///
/// The opposite direction from [`PathSet`], and — unlike `Path` — this one *is* the UA's to act
/// on. RFC 3608 §6: the route "applies only to requests originating in the user agent", and §6.1
/// has the UA "use the content of the Service-Route header field as a preloaded Route header
/// field in outgoing initial requests". Without it, a request sipx sends goes straight at the
/// destination and arrives at a proxy holding no state for the registration it belongs to.
///
/// Order is normative: §6.1 requires a UA that exercises the route to "preserve the order" the
/// values arrived in.
#[derive(Debug, Clone, Default)]
pub struct ServiceRoute(pub Vec<Address>);

impl ServiceRoute {
    /// The proxies, in the order the registrar listed them — which is the order to traverse.
    #[must_use]
    pub fn hops(&self) -> &[Address] {
        &self.0
    }

    /// Whether the registrar dictated no route at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Each hop rendered as a `Route` header value, in order.
    ///
    /// This is the form to preload: `Route: <sip:proxy.example;lr>`, one per hop.
    #[must_use]
    pub fn rendered(&self) -> Vec<String> {
        render_hops(&self.0)
    }

    /// Read the service route out of a REGISTER response.
    ///
    /// **Absent means empty, and empty means clear.** RFC 3608 §6.1 says the stored value "is
    /// updated according to the Service-Route header field of the latest 200 class response",
    /// and that "if there is no Service-Route header field in the response, the UA clears any
    /// service route for that address-of-record previously stored". Both rules are the same rule
    /// — replace unconditionally — which is why this returns an empty set rather than an
    /// `Option` a caller could mistake for "leave what you had".
    #[must_use]
    pub fn from_response(response: &Response) -> Self {
        Self(
            response
                .headers
                .typed_all::<sipx_sip::headers::address::ServiceRoute>()
                .filter_map(std::result::Result::ok)
                .map(|route| route.0)
                .collect(),
        )
    }

    /// The hops the registrar sent without the `;lr` parameter RFC 3608 §5 requires.
    ///
    /// §5: values "MUST include the loose-routing indicator parameter `;lr`". A hop without it
    /// asks for RFC 2543 strict routing, where each proxy rewrites the Request-URI — a mechanism
    /// sipx does not implement and would be wrong to pretend to. Reported rather than rejected:
    /// the offending party is the registrar, the request will still reach the proxy named, and a
    /// UA that discarded the whole route set over a missing parameter would be unroutable for a
    /// reason its operator could not see.
    ///
    /// `lr` is a *URI* parameter — inside the angle brackets — not a header parameter after them.
    /// Looking for it in the wrong list finds nothing and reports every hop, which is how this
    /// method was wrong the first time it was written.
    #[must_use]
    pub fn hops_without_loose_routing(&self) -> Vec<String> {
        self.0
            .iter()
            .filter(|hop| {
                // `contains`, not `value`: `;lr` is a valueless flag, and `value` returns `None`
                // for a present-but-valueless parameter as well as an absent one.
                hop.uri.params().is_none_or(|params| !params.contains("lr"))
            })
            .map(|hop| format!("<{}>", String::from_utf8_lossy(&hop.uri.to_bytes())))
            .collect()
    }
}

impl PartialEq for ServiceRoute {
    fn eq(&self, other: &Self) -> bool {
        self.rendered() == other.rendered()
    }
}

impl Eq for ServiceRoute {}

/// Render address-list hops back to the header values they arrived as, in order.
///
/// Shared by the two route vectors deliberately: they render identically, and only their
/// *meaning* differs. Keeping one renderer means a fix to the parameter handling cannot apply to
/// one direction and not the other.
fn render_hops(hops: &[Address]) -> Vec<String> {
    hops.iter()
        .map(|hop| {
            let mut text = format!("<{}>", String::from_utf8_lossy(&hop.uri.to_bytes()));
            for param in &hop.params {
                text.push(';');
                text.push_str(&String::from_utf8_lossy(&param.name));
                if let Some(value) = &param.value {
                    text.push('=');
                    text.push_str(&String::from_utf8_lossy(value));
                }
            }
            text
        })
        .collect()
}

impl Registration {
    /// Build the REGISTER request.
    ///
    /// Note the two URIs that are easy to confuse: the Request-URI names the *registrar*, the
    /// `To` names the *user*. A REGISTER addressed to the user reaches nothing.
    pub fn request(&self) -> Result<Request, sipx_sip::error::BuildError> {
        Ok(
            RequestBuilder::new(Method::Register, self.registrar.clone())
                .header(HeaderName::To, Bytes::from(self.aor.clone()))?
                .header(
                    HeaderName::From,
                    Bytes::from(format!("{};tag={}", self.aor, new_cnonce())),
                )?
                .header(HeaderName::CallId, Bytes::from(self.call_id.clone()))?
                .cseq(self.cseq, &Method::Register)?
                .header(HeaderName::Contact, Bytes::from(self.contact()))?
                // RFC 3327 §5.1: a UA "SHOULD include the option tag 'path' ... in all
                // Supported header fields". Without it §5.2 tells intermediate proxies not to
                // add themselves, so a UA that stays quiet here is unreachable from behind the
                // very proxies the mechanism exists to traverse.
                .header(HeaderName::Supported, Bytes::from(self.supported()))?
                .header(
                    HeaderName::Expires,
                    Bytes::from(self.expires.as_secs().to_string()),
                )?
                .max_forwards(70)
                .build(),
        )
    }

    /// The `Contact` to register: the configured one, plus the Outbound parameters when this
    /// registration is a flow (RFC 5626 §4.1, §4.2).
    #[must_use]
    pub fn contact(&self) -> String {
        match &self.outbound {
            Some(flow) => crate::outbound::contact(&self.contact, &flow.instance, flow.reg_id),
            None => self.contact.clone(),
        }
    }

    /// The option tags this REGISTER offers.
    ///
    /// `path` always (RFC 3327 §5.1), and `outbound` when this is a flow — §4.2 makes that a MUST,
    /// and without it a registrar has no way to know the request wants flow semantics.
    #[must_use]
    fn supported(&self) -> String {
        match self.outbound {
            Some(_) => format!("path, {}", crate::outbound::OPTION_TAG),
            None => "path".to_owned(),
        }
    }

    /// Advance the sequence number for the next attempt.
    ///
    /// The `Call-ID` deliberately does not change: a new one makes this a new registration
    /// rather than a refresh, which leaves the old contact at the registrar until it expires.
    pub fn advance(&mut self) {
        self.cseq = self.cseq.saturating_add(1);
    }
}

/// Read what a registrar said.
///
/// `contact` is the `Contact` this client registered, needed to find its own binding among
/// the ones the 200 lists.
#[must_use]
pub fn interpret(response: &Response, asked_for: Duration, contact: &str) -> Outcome {
    let status = response.status.code();

    if (200..300).contains(&status) {
        // The registrar's number wins. Refreshing on our own instead is how a client
        // de-registers itself on every cycle.
        let granted = granted_expiry(response, contact).unwrap_or(asked_for);
        // Returned even when this side never offered `path`. A registrar that adds one anyway
        // is doing something worth seeing rather than something to drop on the floor: the
        // whole security value §5.1 claims for the header is that the UA can look at it.
        return Outcome::Registered(Box::new(Registered {
            lease: Lease::from_granted(granted),
            path: PathSet::from_response(response),
            service_route: ServiceRoute::from_response(response),
            flow_accepted: crate::outbound::accepted(response),
            flow_timer: crate::outbound::flow_timer(response),
        }));
    }

    if status == 401 || status == 407 {
        let from_proxy = status == 407;
        let header = if from_proxy {
            HeaderName::ProxyAuthenticate
        } else {
            HeaderName::WwwAuthenticate
        };
        let challenges: Vec<Challenge> = response
            .headers
            .get_all(&header)
            .filter_map(|h| Challenge::parse(&h.value(), from_proxy))
            .collect();
        if let Some(challenge) = strongest(challenges) {
            return Outcome::Challenged(Box::new(challenge));
        }
    }

    Outcome::Rejected {
        status,
        reason: String::from_utf8_lossy(&response.reason).into_owned(),
    }
}

/// The lease the registrar granted to *this client's* binding.
///
/// RFC 3261 §10.3 step 8: the 200 lists every current binding for the address of record,
/// not just the one refreshed, so per §10.2.4 the client finds its own by URI comparison
/// (§19.1.4) and takes the expiry from that row. The first row may be another device on
/// another lease. Only when no row is ours does the `Expires` header speak — it is the
/// per-contact parameter that is per-binding, not the header.
fn granted_expiry(response: &Response, contact: &str) -> Option<Duration> {
    if let Ok(own) = Address::parse(contact.as_bytes(), "Contact") {
        for value in response.headers.typed_all::<ContactValue>() {
            let Ok(ContactValue::Address(address)) = value else {
                continue;
            };
            if !address.uri.equivalent(&own.uri) {
                continue;
            }
            if let Some(seconds) = contact_expires(&address) {
                return Some(Duration::from_secs(seconds));
            }
            // Our binding, listed without a per-contact expiry: the header applies to it.
            break;
        }
    }
    response
        .headers
        .value(&HeaderName::Expires)
        .and_then(|value| {
            std::str::from_utf8(&value)
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
        .map(Duration::from_secs)
}

fn contact_expires(address: &Address) -> Option<u64> {
    let value = address.param("expires")?;
    std::str::from_utf8(value).ok()?.trim().parse().ok()
}

/// Add credentials answering a challenge to a request.
pub fn authorize(
    request: &mut Request,
    challenge: &Challenge,
    credentials: &Credentials,
    nonce_count: u32,
) -> Result<(), sipx_sip::error::BuildError> {
    let uri = String::from_utf8_lossy(&request.uri.to_bytes()).into_owned();
    let method = String::from_utf8_lossy(request.method.as_bytes()).into_owned();
    let value = respond(
        challenge,
        credentials,
        &method,
        &uri,
        nonce_count,
        &new_cnonce(),
    );
    let header = sipx_sip::Header::build(challenge.response_header(), Bytes::from(value))?;
    request.headers.push(header);
    Ok(())
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
    use sipx_sip::{Host, HostName, Limits, Message, parse_datagram};

    /// The `Contact` this client registers in every test here.
    const CONTACT: &str = "<sip:alice@192.0.2.5:5060>";

    fn registration() -> Registration {
        Registration {
            registrar: Uri::sip(Host::Name(HostName::new("example.com").expect("valid"))),
            aor: "<sip:alice@example.com>".to_owned(),
            contact: CONTACT.to_owned(),
            expires: Duration::from_secs(3600),
            call_id: "reg-1@192.0.2.5".to_owned(),
            cseq: 1,
            outbound: None,
        }
    }

    fn response(text: &str) -> Response {
        match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram()).expect("parses") {
            Message::Response(r) => r,
            Message::Request(_) => panic!("a response"),
        }
    }

    fn ok_with(extra: &str) -> Response {
        response(&format!(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             {extra}\
             Content-Length: 0\r\n\r\n"
        ))
    }

    /// The story's failing-first test.
    #[test]
    fn a_registration_preserves_the_path_it_was_returned() {
        // Two proxies, on separate rows — which is how they actually arrive, each one having
        // pushed itself onto the front on the way through (RFC 3327 §5.2).
        let outcome = interpret(
            &ok_with(
                "Path: <sip:edge.example.com;lr>\r\nPath: <sip:core.example.net;lr>\r\nContact: <sip:alice@192.0.2.5:5060>;expires=600\r\n",
            ),
            Duration::from_secs(3600),
            CONTACT,
        );
        let Outcome::Registered(registered) = outcome else {
            panic!("expected a registration, got {outcome:?}");
        };
        assert_eq!(registered.lease.granted, Duration::from_secs(600));
        assert_eq!(
            registered.path.rendered(),
            vec![
                "<sip:edge.example.com;lr>".to_owned(),
                "<sip:core.example.net;lr>".to_owned()
            ],
            "the path vector was lost, reordered, or flattened"
        );
    }

    #[test]
    fn a_comma_joined_path_is_the_same_as_separate_rows() {
        // RFC 3261 §7.3 makes these two spellings equivalent for a list header, and a path
        // vector read a line at a time turns two hops into one opaque string — losing the
        // order, which is the entire content of the vector.
        let joined = interpret(
            &ok_with("Path: <sip:edge.example.com;lr>, <sip:core.example.net;lr>\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        let separate = interpret(
            &ok_with("Path: <sip:edge.example.com;lr>\r\nPath: <sip:core.example.net;lr>\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        match (joined, separate) {
            (Outcome::Registered(one), Outcome::Registered(other)) => {
                assert_eq!(
                    one.path.rendered().len(),
                    2,
                    "the comma-joined row was not split"
                );
                assert_eq!(one.path, other.path);
            }
            other => panic!("expected two registrations, got {other:?}"),
        }
    }

    #[test]
    fn a_path_parameter_survives_because_outbound_will_need_it() {
        // RFC 5626 §5.3 hangs the `ob` marker off a Path value. A vector kept as bare URIs
        // would parse cleanly and be quietly useless to T-15.
        let outcome = interpret(
            &ok_with("Path: <sip:edge.example.com;lr;ob>\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        let Outcome::Registered(registered) = outcome else {
            panic!("expected a registration");
        };
        assert!(
            registered
                .path
                .hops()
                .first()
                .expect("one hop")
                .uri
                .params()
                .is_some_and(|params| params.contains("ob")),
            "the ob parameter was dropped: {:?}",
            registered.path.rendered()
        );
    }

    #[test]
    fn a_register_offers_the_path_option_tag() {
        // RFC 3327 §5.2 tells proxies not to add themselves unless the UA has indicated
        // support, so a UA that stays quiet here is unreachable from behind exactly the
        // proxies the mechanism exists to traverse.
        let request = registration().request().expect("builds");
        let supported = request
            .headers
            .value(&HeaderName::Supported)
            .expect("Supported is present");
        assert!(String::from_utf8_lossy(&supported).contains("path"));
    }

    #[test]
    fn a_path_returned_unasked_is_still_reported() {
        // §5.1's reason for the header to reach the UA at all: "such inspection might allow
        // the UA to detect intermediate proxies that have inappropriately added themselves".
        // Dropping it because we did not ask would remove the only defence it offers.
        let outcome = interpret(
            &ok_with("Path: <sip:stranger.example.org;lr>\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        let Outcome::Registered(registered) = outcome else {
            panic!("expected a registration");
        };
        assert_eq!(
            registered.path.hops_outside(&["edge.example.com"]),
            vec!["<sip:stranger.example.org;lr>".to_owned()]
        );
    }

    #[test]
    fn no_path_is_an_empty_set_rather_than_an_absent_one() {
        let outcome = interpret(&ok_with(""), Duration::from_secs(3600), CONTACT);
        let Outcome::Registered(registered) = outcome else {
            panic!("expected a registration");
        };
        assert!(registered.path.is_empty());
    }

    fn service_route_of(extra: &str) -> ServiceRoute {
        let outcome = interpret(&ok_with(extra), Duration::from_secs(3600), CONTACT);
        match outcome {
            Outcome::Registered(registered) => registered.service_route,
            other => panic!("expected a registration, got {other:?}"),
        }
    }

    /// RFC 3608 §6.1: a UA that exercises the route "MUST preserve the order".
    #[test]
    fn a_service_route_keeps_the_order_the_registrar_listed() {
        let route = service_route_of(
            "Service-Route: <sip:edge.example.com;lr>\r\n\
             Service-Route: <sip:core.example.net;lr>\r\n",
        );
        assert_eq!(
            route.rendered(),
            vec![
                "<sip:edge.example.com;lr>".to_owned(),
                "<sip:core.example.net;lr>".to_owned(),
            ],
            "the outbound route set is not in the order it arrived in"
        );
    }

    /// §5's grammar is `sr-value *( COMMA sr-value )`, so the two spellings are one value.
    #[test]
    fn a_comma_joined_service_route_is_the_same_as_separate_rows() {
        let joined = service_route_of(
            "Service-Route: <sip:edge.example.com;lr>, <sip:core.example.net;lr>\r\n",
        );
        let separate = service_route_of(
            "Service-Route: <sip:edge.example.com;lr>\r\n\
             Service-Route: <sip:core.example.net;lr>\r\n",
        );
        assert_eq!(joined.hops().len(), 2, "the comma-joined row was not split");
        assert_eq!(joined, separate);
    }

    /// RFC 3608 §6.1: "if there is no Service-Route header field in the response, the UA clears
    /// any service route for that address-of-record previously stored".
    ///
    /// The rule is easy to get backwards — treating an absent header as "nothing to say, keep
    /// what you had" leaves a UA routing through a proxy the registrar has stopped naming.
    #[test]
    fn a_response_without_a_service_route_says_clear_it_rather_than_keep_it() {
        assert!(
            service_route_of("").is_empty(),
            "an absent Service-Route must read as empty, so that storing it clears the old one"
        );
    }

    /// §5: values "MUST include the loose-routing indicator parameter `;lr`".
    ///
    /// Reported, not enforced: the request still reaches the proxy named, and discarding a whole
    /// route set over a missing parameter would make a UA unroutable for an invisible reason.
    #[test]
    fn a_hop_without_lr_is_reported_rather_than_dropped() {
        let route = service_route_of(
            "Service-Route: <sip:edge.example.com;lr>\r\n\
             Service-Route: <sip:strict.example.net>\r\n",
        );
        assert_eq!(route.hops().len(), 2, "the offending hop was dropped");
        assert_eq!(
            route.hops_without_loose_routing(),
            vec!["<sip:strict.example.net>".to_owned()],
            "the hop missing ;lr was not reported"
        );
    }

    /// The two vectors travel in opposite directions and must not be read from each other's
    /// header. A registrar that returns only a `Path` has dictated no outbound route.
    #[test]
    fn a_path_is_not_a_service_route() {
        let outcome = interpret(
            &ok_with("Path: <sip:edge.example.com;lr>\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        let Outcome::Registered(registered) = outcome else {
            panic!("expected a registration");
        };
        assert!(!registered.path.is_empty(), "the Path was lost");
        assert!(
            registered.service_route.is_empty(),
            "a Path was read as a Service-Route; the UA would route its own requests through \
             proxies that only asked to be on the inbound path"
        );
    }

    #[test]
    fn the_request_uri_names_the_registrar_and_the_to_names_the_user() {
        let request = registration().request().expect("builds");
        assert_eq!(request.uri.to_bytes().as_ref(), b"sip:example.com");
        assert_eq!(
            request
                .headers
                .value(&HeaderName::To)
                .expect("a To")
                .as_ref(),
            b"<sip:alice@example.com>"
        );
    }

    /// The registrar's number wins. A client that refreshes on the interval it *asked* for
    /// de-registers itself every cycle when the registrar grants less.
    #[test]
    fn the_granted_expiry_overrides_what_was_asked_for() {
        let outcome = interpret(
            &ok_with("Contact: <sip:alice@192.0.2.5:5060>;expires=60\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        match outcome {
            Outcome::Registered(r) => assert_eq!(r.lease.granted, Duration::from_secs(60)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    /// A per-contact expiry beats the `Expires` header, which applies to all of them.
    #[test]
    fn a_contact_expiry_beats_the_expires_header() {
        let outcome = interpret(
            &ok_with("Expires: 3600\r\nContact: <sip:alice@192.0.2.5:5060>;expires=120\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        match outcome {
            Outcome::Registered(r) => assert_eq!(r.lease.granted, Duration::from_secs(120)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    /// RFC 3261 §10.3 step 8: the 200 lists every current binding for the address of
    /// record, and §10.2.4 has the client find its own by URI comparison (§19.1.4). Another
    /// device's binding listed first must not become this client's refresh schedule — a
    /// lease scheduled off the wrong row lapses while the client still believes it holds it.
    #[test]
    fn the_expiry_comes_from_our_own_binding_not_the_first_listed() {
        let outcome = interpret(
            &ok_with(
                "Contact: <sip:alice@198.51.100.9:5060>;expires=3600\r\n\
                 Contact: <sip:alice@192.0.2.5:5060>;expires=60\r\n",
            ),
            Duration::from_secs(3600),
            CONTACT,
        );
        match outcome {
            Outcome::Registered(r) => assert_eq!(r.lease.granted, Duration::from_secs(60)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    #[test]
    fn the_expires_header_is_used_when_the_contact_has_none() {
        let outcome = interpret(
            &ok_with("Expires: 300\r\n"),
            Duration::from_secs(3600),
            CONTACT,
        );
        match outcome {
            Outcome::Registered(r) => assert_eq!(r.lease.granted, Duration::from_secs(300)),
            other => panic!("expected a lease, got {other:?}"),
        }
    }

    /// The refresh must leave room to retry. Refreshing exactly at expiry means a single lost
    /// packet drops the registration.
    #[test]
    fn the_refresh_leaves_margin_before_the_lease_ends() {
        let lease = Lease::from_granted(Duration::from_secs(3600));
        assert_eq!(lease.refresh_after, Duration::from_secs(3240));
        assert!(lease.refresh_after < lease.granted);

        // And a short lease still leaves something.
        let short = Lease::from_granted(Duration::from_secs(15));
        assert!(short.refresh_after < short.granted);
        assert!(short.refresh_after >= Duration::from_secs(1));

        // Even a degenerate one-second lease must not schedule a refresh at zero, which would
        // spin.
        let degenerate = Lease::from_granted(Duration::from_secs(1));
        assert_eq!(degenerate.refresh_after, Duration::from_secs(1));
    }

    #[test]
    fn a_401_is_a_challenge_rather_than_a_failure() {
        let challenged = response(
            "SIP/2.0 401 Unauthorized\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             WWW-Authenticate: Digest realm=\"example.com\", nonce=\"abc\", qop=\"auth\"\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match interpret(&challenged, Duration::from_secs(3600), CONTACT) {
            Outcome::Challenged(challenge) => {
                assert_eq!(challenge.realm, "example.com");
                assert!(challenge.qop_auth);
                assert!(!challenge.from_proxy);
            }
            other => panic!("expected a challenge, got {other:?}"),
        }
    }

    #[test]
    fn a_407_is_a_proxy_challenge_and_answered_in_the_proxy_header() {
        let challenged = response(
            "SIP/2.0 407 Proxy Authentication Required\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             Proxy-Authenticate: Digest realm=\"p\", nonce=\"n\"\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match interpret(&challenged, Duration::from_secs(3600), CONTACT) {
            Outcome::Challenged(challenge) => {
                assert!(challenge.from_proxy);
                assert_eq!(challenge.response_header(), HeaderName::ProxyAuthorization);
            }
            other => panic!("expected a challenge, got {other:?}"),
        }
    }

    #[test]
    fn a_403_is_a_rejection_not_a_challenge() {
        let refused = response(
            "SIP/2.0 403 Forbidden\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             Content-Length: 0\r\n\r\n",
        );
        match interpret(&refused, Duration::from_secs(3600), CONTACT) {
            Outcome::Rejected { status, reason } => {
                assert_eq!(status, 403);
                assert_eq!(reason, "Forbidden");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    /// A 401 whose challenge cannot be parsed is a rejection, not a challenge to answer. The
    /// alternative is retrying forever against a header we do not understand.
    #[test]
    fn a_401_with_an_unusable_challenge_is_a_rejection() {
        let bad = response(
            "SIP/2.0 401 Unauthorized\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             WWW-Authenticate: Basic realm=\"example.com\"\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(matches!(
            interpret(&bad, Duration::from_secs(3600), CONTACT),
            Outcome::Rejected { status: 401, .. }
        ));
    }

    /// A refresh keeps the `Call-ID` and advances the `CSeq`. A new `Call-ID` would leave the
    /// old contact registered until it expired on its own.
    #[test]
    fn a_refresh_keeps_the_call_id_and_advances_the_cseq() {
        let mut registration = registration();
        let first = registration.request().expect("builds");
        registration.advance();
        let second = registration.request().expect("builds");

        assert_eq!(
            first.headers.value(&HeaderName::CallId),
            second.headers.value(&HeaderName::CallId),
        );
        assert_eq!(
            second
                .headers
                .value(&HeaderName::CSeq)
                .expect("a CSeq")
                .as_ref(),
            b"2 REGISTER"
        );
    }

    /// The credentials are computed over the Request-URI of the request they authorize.
    #[test]
    fn authorization_covers_the_request_uri() {
        let mut request = registration().request().expect("builds");
        let challenge = Challenge::parse(
            br#"Digest realm="example.com", nonce="abc", qop="auth""#,
            false,
        )
        .expect("parses");
        authorize(
            &mut request,
            &challenge,
            &Credentials::new("alice", "secret"),
            1,
        )
        .expect("authorizes");

        let header = request
            .headers
            .value(&HeaderName::Authorization)
            .expect("an Authorization");
        let text = String::from_utf8_lossy(&header);
        assert!(text.contains(r#"uri="sip:example.com""#), "{text}");
        assert!(text.contains(r#"username="alice""#), "{text}");
        assert!(text.contains("nc=00000001"), "{text}");
    }
}
