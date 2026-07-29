//! Push notifications (RFC 8599), at the URI and header level.
//!
//! The problem: a mobile client is not running, or is running with every socket torn down by the
//! operating system. There is no flow to route a call down and no keep-alive that could hold one
//! open, so the registrar's binding names an address that reaches nothing. RFC 8599's answer is to
//! route *around* SIP for one hop — the proxy asks the client's push notification service to wake
//! it, and the client, once awake, goes and gets a flow.
//!
//! What lives here is the part that is pure syntax, and it is three separate things that are easy
//! to conflate:
//!
//! - **The `pn-*` parameters** (§8.7) a `Contact` URI carries, which tell the registrar which push
//!   service to ask and how that service names this device. They are *URI* parameters, so they go
//!   inside the angle brackets of a `Contact`; outside them a `;` starts a header parameter and a
//!   registrar reading the URI would never see them (RFC 3261 §20).
//! - **The feature-capability indicators** (§8.2), which travel in `Feature-Caps` (RFC 6809) and
//!   are how the registrar answers back: which push service it actually supports, whether it wants
//!   refreshes anyway, and what it will call this binding.
//! - **555** (§8.1), the status code that says the client's whole reachability plan is wrong.
//!
//! Nothing here sends or receives a push notification. sipx implements the SIP half of RFC 8599
//! and nothing else — the push service is behind a trait in `sipx-ua` and this repository ships no
//! implementation of one. Deciding what a registration should say, and reading what came back,
//! belongs in `sipx-ua` for the same reason [`crate::gruu`]'s registration half does.

use std::time::Duration;

use bytes::Bytes;

use crate::error::{BuildError, HeaderError};
use crate::headers::grammar::{self, HeaderParam, trim};
use crate::message::{StatusCode, TypedHeader};
use crate::name::HeaderName;
use crate::params::Param;
use crate::uri::Uri;

/// The `Contact` URI parameter naming the push notification service (§8.7).
pub const PN_PROVIDER: &str = "pn-provider";

/// The `Contact` URI parameter carrying whatever else the named service needs (§8.7).
///
/// Its meaning is the service's, not SIP's, which is why nothing here interprets it.
pub const PN_PARAM: &str = "pn-param";

/// The `Contact` URI parameter carrying the identifier the service knows this device by (§8.7).
pub const PN_PRID: &str = "pn-prid";

/// The `Contact` URI parameter carrying the PURR — the Push Resource Reachability Reference the
/// proxy assigned this binding (§8.7).
///
/// Read, never minted here: a UA does not choose its own PURR. See [`purr`].
pub const PN_PURR: &str = "pn-purr";

/// The feature-capability indicator naming a push notification service (§8.2).
pub const SIP_PNS: &str = "+sip.pns";

/// The feature-capability indicator asking for binding refreshes even without a push (§8.2).
pub const SIP_PNSREG: &str = "+sip.pnsreg";

/// The feature-capability indicator carrying the PURR assigned to a binding (§8.2).
pub const SIP_PNSPURR: &str = "+sip.pnspurr";

/// 555 (Push Notification Service Not Supported), registered in §8.1.
pub const NOT_SUPPORTED: u16 = 555;

/// The reason phrase §8.1 registers alongside [`NOT_SUPPORTED`].
pub const NOT_SUPPORTED_REASON: &str = "Push Notification Service Not Supported";

/// Whether a status is §8.1's 555.
///
/// Worth a name because of what it is not: 555 is not "the server said no" in the way a 403 is. It
/// says the push service the request named cannot be used here, so every retry against this
/// registrar with these parameters will fail the same way. A client that folds it into a generic
/// 5xx retries forever against a plan that cannot work.
#[must_use]
pub fn is_not_supported(status: StatusCode) -> bool {
    status.code() == NOT_SUPPORTED
}

/// How a push notification service names one device (§4.1.2, §8.7).
///
/// The three values a `Contact` URI carries so that a proxy can wake this client: which service to
/// ask, what that service calls this device, and whatever else the service needs in between.
///
/// # Why constructing one can fail
///
/// A `pn-prid` is an opaque token minted by somebody else, and RFC 3261 §25.1's `pvalue` does not
/// admit every octet — `=`, `;`, `?` and `@` among them. A value carrying one of those, pasted into
/// a URI unchecked, does not produce a rejected registration; it produces a *different* URI, with
/// the tail of the token read as another parameter. So the values are checked once, here, and a
/// caller holding a token that needs octets outside `pvalue` percent-escapes it — `escaped` is part
/// of the grammar and survives [`Uri`] round-tripping unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    provider: String,
    param: Option<String>,
    prid: String,
}

impl Device {
    /// The service to ask, and the identifier it knows this device by (§4.1.2).
    ///
    /// `provider` is a value from the registry §8.8 creates; `prid` is the token the service
    /// issued for this device.
    pub fn new(provider: &str, prid: &str) -> Result<Self, BuildError> {
        Ok(Self {
            provider: pvalue(provider, PN_PROVIDER)?,
            param: None,
            prid: pvalue(prid, PN_PRID)?,
        })
    }

    /// Add the `pn-param` the named service needs (§4.1.2).
    ///
    /// Optional because it is service-specific: some need nothing beyond the identifier.
    pub fn with_param(mut self, param: &str) -> Result<Self, BuildError> {
        self.param = Some(pvalue(param, PN_PARAM)?);
        Ok(self)
    }

    /// The `pn-provider` value.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The `pn-param` value, if the service needs one.
    #[must_use]
    pub fn param(&self) -> Option<&str> {
        self.param.as_deref()
    }

    /// The `pn-prid` value.
    #[must_use]
    pub fn prid(&self) -> &str {
        &self.prid
    }

    /// Put these parameters on a URI, replacing any already there (§4.1.2).
    ///
    /// Replacing rather than appending is not tidiness: RFC 3261 §19.1.1 says "any given
    /// parameter-name MUST NOT appear more than once" in a URI, and a duplicate makes the whole
    /// URI unparseable at the far end. A stale `pn-prid` left beside a fresh one would take the
    /// registration down rather than merely be ignored.
    pub fn set_on(&self, uri: &mut Uri) {
        for name in [PN_PROVIDER, PN_PARAM, PN_PRID] {
            uri.remove_param(name);
        }
        uri.push_param(Param::new(
            Bytes::from_static(PN_PROVIDER.as_bytes()),
            Bytes::from(self.provider.clone()),
        ));
        if let Some(param) = &self.param {
            uri.push_param(Param::new(
                Bytes::from_static(PN_PARAM.as_bytes()),
                Bytes::from(param.clone()),
            ));
        }
        uri.push_param(Param::new(
            Bytes::from_static(PN_PRID.as_bytes()),
            Bytes::from(self.prid.clone()),
        ));
    }

    /// Read the push parameters off a URI (§8.7).
    ///
    /// `None` unless both `pn-provider` and `pn-prid` are there: §4.1.2 has a UA insert them
    /// together, and either alone names no service that could be asked to wake anything.
    #[must_use]
    pub fn from_uri(uri: &Uri) -> Option<Self> {
        let params = uri.params()?;
        let text = |name: &str| {
            params
                .value(name)
                .and_then(|raw| std::str::from_utf8(raw).ok())
        };
        Some(Self {
            provider: pvalue(text(PN_PROVIDER)?, PN_PROVIDER).ok()?,
            param: text(PN_PARAM).and_then(|value| pvalue(value, PN_PARAM).ok()),
            prid: pvalue(text(PN_PRID)?, PN_PRID).ok()?,
        })
    }
}

/// The PURR a URI carries, if it carries one (§8.7).
///
/// Returned raw and not interpreted. The PURR is the proxy's name for a binding, so that a
/// mid-dialog request can be matched to the binding it belongs to without re-deriving it from the
/// `pn-*` values — which means it is only useful to a party that *stores* bindings. sipx stores
/// none: it is a user agent, not a registrar or a proxy, so it reads the PURR its registrar
/// assigned (see [`Indicators::purr`]), carries it, and does no matching with it. That half of
/// §5.6 belongs where the other proxy roles do.
#[must_use]
pub fn purr(uri: &Uri) -> Option<&[u8]> {
    uri.params().and_then(|params| params.value(PN_PURR))
}

/// The push feature-capability indicators one `Feature-Caps` value carries (§8.2).
///
/// RFC 6809 §4 gives the header the shape `*` followed by `;`-separated indicators, and RFC 8599
/// §8.2 registers three of them that a client cares about. Indicators belonging to other
/// mechanisms are ignored rather than rejected — that is what an extensible list is for, and a
/// parser that failed on an unknown one would break against every proxy that grows a feature.
///
/// This models one value. A registrar may send several, on one row or on several; read them with
/// [`crate::message::Headers::typed_all`], which treats those two spellings as the same message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Indicators {
    pns: Option<Vec<u8>>,
    pnsreg: Option<Vec<u8>>,
    pnspurr: Option<Vec<u8>>,
}

impl Indicators {
    /// The push notification service this value names (§8.2's `sip.pns`).
    ///
    /// This is the answer to "does this registrar support the service I asked for". A registrar
    /// naming a different one has not refused the registration — it has accepted a binding that
    /// nothing will ever wake, which is worse, because it looks like success.
    #[must_use]
    pub fn pns(&self) -> Option<&[u8]> {
        self.pns.as_deref()
    }

    /// Whether the registrar asked for binding refreshes even in the absence of a push (§8.2's
    /// `sip.pnsreg`).
    ///
    /// Presence is a fact of its own, separate from the interval: an indicator sent with a value
    /// this side cannot read still says the registrar wants refreshes, and treating it as absent
    /// would let the binding lapse.
    #[must_use]
    pub fn refreshes_required(&self) -> bool {
        self.pnsreg.is_some()
    }

    /// How long the registrar said to leave between those refreshes (§8.2's `sip.pnsreg`).
    ///
    /// `None` when the indicator is absent *and* when its value is not a number of seconds; see
    /// [`Indicators::refreshes_required`] for the difference.
    #[must_use]
    pub fn refresh_interval(&self) -> Option<Duration> {
        let raw = self.pnsreg.as_deref()?;
        std::str::from_utf8(raw)
            .ok()?
            .trim()
            .parse::<u64>()
            .ok()
            .map(Duration::from_secs)
    }

    /// The PURR the proxy assigned this binding (§8.2's `sip.pnspurr`).
    #[must_use]
    pub fn purr(&self) -> Option<&[u8]> {
        self.pnspurr.as_deref()
    }

    /// Whether this value said nothing about push at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pns.is_none() && self.pnsreg.is_none() && self.pnspurr.is_none()
    }

    /// Pick the three push indicators out of a parsed indicator list.
    ///
    /// Two readings of a valueless indicator, because the three do not mean the same kind of
    /// thing. `sip.pns` and `sip.pnspurr` **name** something — a push service, a binding — and a
    /// name with no characters in it names nothing; kept, it would answer
    /// [`crate::push::Indicators::pns`] with a service that compares equal to the empty string,
    /// which is a service no client asked for and every client with an empty provider matches.
    /// `sip.pnsreg` asks for something, and the asking is the whole of it: present without a
    /// readable interval it still says the registrar wants refreshes, and dropping it would let
    /// the binding lapse.
    fn from_params(params: &[HeaderParam]) -> Self {
        let named = |name: &str| {
            grammar::param(params, name)
                .and_then(|found| found.value.clone())
                .filter(|value| !value.is_empty())
        };
        Self {
            pns: named(SIP_PNS),
            pnsreg: grammar::param(params, SIP_PNSREG)
                .map(|found| found.value.clone().unwrap_or_default()),
            pnspurr: named(SIP_PNSPURR),
        }
    }
}

impl TypedHeader for Indicators {
    const NAME: HeaderName = HeaderName::FeatureCaps;

    /// Decodes the **first** value in the row; use
    /// [`crate::message::Headers::typed_all`] when every one is needed.
    fn decode(value: &[u8]) -> Result<Self, HeaderError> {
        let parts = grammar::split_list(value, "Feature-Caps")?;
        decode_one(parts.first().copied().unwrap_or(&[]))
    }

    fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
        grammar::split_list(value, "Feature-Caps")?
            .into_iter()
            .map(decode_one)
            .collect()
    }
}

/// One `fc-value` (RFC 6809 §4): `"*" *( SEMI feature-cap )`.
fn decode_one(value: &[u8]) -> Result<Indicators, HeaderError> {
    let value = trim(value);
    // The `*` is not decoration: RFC 6809 §4 makes it the whole of the value's non-parameter
    // part, and a row that opens with anything else is not a `Feature-Caps` value.
    if value.first() != Some(&b'*') {
        return Err(HeaderError::Syntax {
            header: "Feature-Caps",
        });
    }
    let params = grammar::parse_params(trim(value.get(1..).unwrap_or(&[])), "Feature-Caps")?;
    Ok(Indicators::from_params(&params))
}

/// Check a value against RFC 3261 §25.1's `pvalue`, which is what a URI parameter may hold.
///
/// ```abnf
/// pvalue           = 1*paramchar
/// paramchar        = param-unreserved / unreserved / escaped
/// param-unreserved = "[" / "]" / "/" / ":" / "&" / "+" / "$"
/// ```
fn pvalue(value: &str, field: &'static str) -> Result<String, BuildError> {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return Err(BuildError::NotAToken { field });
    }
    let mut at = 0usize;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%' {
            // `escaped = "%" HEXDIG HEXDIG`, and a lone `%` is not one.
            if !bytes.get(at + 1).is_some_and(u8::is_ascii_hexdigit)
                || !bytes.get(at + 2).is_some_and(u8::is_ascii_hexdigit)
            {
                return Err(BuildError::NotAToken { field });
            }
            at += 3;
            continue;
        }
        if !is_paramchar(byte) {
            return Err(BuildError::NotAToken { field });
        }
        at += 1;
    }
    Ok(value.to_owned())
}

/// `paramchar` less `escaped`, which [`pvalue`] handles separately.
#[must_use]
fn is_paramchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            // unreserved: mark
            b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            // param-unreserved
            | b'[' | b']' | b'/' | b':' | b'&' | b'+' | b'$'
        )
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
    use crate::message::Headers;
    use crate::parser::{Limits, parse_datagram};
    use crate::{Message, Response};

    fn uri(text: &str) -> Uri {
        Uri::parse(Bytes::from(text.to_owned())).unwrap_or_else(|e| panic!("{text:?}: {e}"))
    }

    fn device() -> Device {
        Device::new("webpush", "c1a5b3e7d9f2")
            .expect("valid")
            .with_param("7f3ad0")
            .expect("valid")
    }

    /// §4.1.2's three parameters, in a URI, in the order the section lists them.
    #[test]
    fn the_push_parameters_go_into_the_uri_grammar() {
        let mut contact = uri("sip:alice@192.0.2.5:5060");
        device().set_on(&mut contact);
        assert_eq!(
            contact.to_bytes(),
            Bytes::from_static(
                b"sip:alice@192.0.2.5:5060;pn-provider=webpush;pn-param=7f3ad0;pn-prid=c1a5b3e7d9f2"
            )
        );
        // And back out again: what a registrar reads is what the UA meant to say.
        assert_eq!(Device::from_uri(&contact), Some(device()));
    }

    /// §4.1.2 makes `pn-param` service-specific, so a service that needs nothing beyond the
    /// identifier sends two parameters, not three.
    #[test]
    fn a_service_that_needs_no_pn_param_sends_none() {
        let mut contact = uri("sip:alice@192.0.2.5:5060");
        Device::new("webpush", "c1a5b3e7d9f2")
            .expect("valid")
            .set_on(&mut contact);
        assert_eq!(
            contact.to_bytes(),
            Bytes::from_static(
                b"sip:alice@192.0.2.5:5060;pn-provider=webpush;pn-prid=c1a5b3e7d9f2"
            )
        );
        assert!(Device::from_uri(&contact).is_some_and(|d| d.param().is_none()));
    }

    /// RFC 3261 §19.1.1: "any given parameter-name MUST NOT appear more than once". Appending a
    /// second `pn-prid` beside a stale one produces a URI the registrar cannot parse at all —
    /// which takes the registration down rather than merely losing the push parameters.
    #[test]
    fn setting_the_parameters_twice_replaces_them_rather_than_repeating_them() {
        let mut contact = uri("sip:alice@192.0.2.5:5060");
        device().set_on(&mut contact);
        Device::new("webpush", "0000deadbeef")
            .expect("valid")
            .set_on(&mut contact);
        assert_eq!(
            contact.to_bytes(),
            Bytes::from_static(
                b"sip:alice@192.0.2.5:5060;pn-provider=webpush;pn-prid=0000deadbeef"
            )
        );
        // The proof that matters: it still parses.
        assert!(Uri::parse(contact.to_bytes()).is_ok());
    }

    /// Other URI parameters are none of this mechanism's business and must survive it.
    #[test]
    fn the_other_uri_parameters_are_left_alone() {
        let mut contact = uri("sip:alice@192.0.2.5:5060;transport=tcp;ob");
        device().set_on(&mut contact);
        let text = String::from_utf8_lossy(&contact.to_bytes()).into_owned();
        assert!(
            text.starts_with("sip:alice@192.0.2.5:5060;transport=tcp;ob;"),
            "{text}"
        );
        assert!(text.contains(";pn-prid=c1a5b3e7d9f2"), "{text}");
    }

    /// A token carrying an octet `pvalue` does not admit would not be rejected by the registrar —
    /// it would silently become a *different* URI, its tail read as further parameters. So it is
    /// refused here, where the caller can still do something about it.
    #[test]
    fn a_value_outside_pvalue_is_refused_rather_than_pasted_in() {
        for bad in [
            "tok=en", "tok;en", "tok en", "tok@en", "tok?en", "", "tok%zz", "tok%4",
        ] {
            assert!(
                Device::new("webpush", bad).is_err(),
                "{bad:?} was accepted into a URI parameter"
            );
        }
        // `escaped` is in the grammar, which is how a caller carries the rest.
        assert!(Device::new("webpush", "tok%3Den").is_ok());
        // As are the characters `param-unreserved` names, which base64url tokens use.
        assert!(Device::new("webpush", "a-b_c.d~e+f/g").is_ok());
    }

    /// A URI with only half the pair names no service that anything could be asked to wake.
    #[test]
    fn half_a_binding_is_not_one() {
        assert!(Device::from_uri(&uri("sip:alice@192.0.2.5;pn-provider=webpush")).is_none());
        assert!(Device::from_uri(&uri("sip:alice@192.0.2.5;pn-prid=c1a5b3e7d9f2")).is_none());
        assert!(Device::from_uri(&uri("sip:alice@192.0.2.5")).is_none());
    }

    /// §8.7 registers `pn-purr` as a URI parameter, and it is read rather than minted.
    #[test]
    fn the_purr_is_read_off_a_uri_and_not_interpreted() {
        assert_eq!(
            purr(&uri("sip:alice@192.0.2.5;pn-purr=opaque-purr-1")),
            Some(&b"opaque-purr-1"[..])
        );
        assert_eq!(purr(&uri("sip:alice@192.0.2.5")), None);
    }

    fn response(caps: &str) -> Response {
        let text = format!(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             {caps}\
             Content-Length: 0\r\n\r\n"
        );
        match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses") {
            Message::Response(r) => r,
            Message::Request(_) => panic!("a response"),
        }
    }

    fn read(caps: &str) -> Vec<Indicators> {
        response(caps)
            .headers
            .typed_all::<Indicators>()
            .collect::<Result<Vec<_>, _>>()
            .expect("parses")
    }

    /// §8.2's three indicators, as RFC 6809 §4 spells them.
    #[test]
    fn the_push_indicators_are_read_out_of_feature_caps() {
        let read = read(
            "Feature-Caps: *;+sip.pns=\"webpush\";+sip.pnsreg=\"120\"\
             ;+sip.pnspurr=\"opaque-purr-1\"\r\n",
        );
        let one = read.first().expect("one value");
        assert_eq!(one.pns(), Some(&b"webpush"[..]));
        assert!(one.refreshes_required());
        assert_eq!(one.refresh_interval(), Some(Duration::from_secs(120)));
        assert_eq!(one.purr(), Some(&b"opaque-purr-1"[..]));
    }

    /// RFC 6809 §4 makes the header a comma-separated list, so one row of two values and two rows
    /// of one are the same message (RFC 3261 §7.3).
    #[test]
    fn a_comma_joined_row_is_the_same_as_separate_rows() {
        let joined = read("Feature-Caps: *;+sip.pns=\"webpush\", *;+sip.pnspurr=\"p1\"\r\n");
        let separate = read(
            "Feature-Caps: *;+sip.pns=\"webpush\"\r\n\
             Feature-Caps: *;+sip.pnspurr=\"p1\"\r\n",
        );
        assert_eq!(joined.len(), 2, "the comma-joined row was not split");
        assert_eq!(joined, separate);
    }

    /// An indicator belonging to some other mechanism is not an error. A parser that rejected one
    /// would break against every proxy that ever grows a feature.
    #[test]
    fn indicators_this_side_does_not_know_are_ignored() {
        let read = read("Feature-Caps: *;+sip.something.else=\"x\";+sip.pns=\"webpush\"\r\n");
        assert_eq!(
            read.first().and_then(Indicators::pns),
            Some(&b"webpush"[..])
        );
    }

    /// A registrar that says nothing about push has said nothing about push — not "no".
    #[test]
    fn a_value_with_no_push_indicators_is_empty_rather_than_negative() {
        assert!(read("Feature-Caps: *;+sip.other=\"x\"\r\n")[0].is_empty());
        assert!(
            response("")
                .headers
                .typed_all::<Indicators>()
                .next()
                .is_none()
        );
    }

    /// A valueless `sip.pns` names no service, and a name of no characters must not become one:
    /// kept, it would answer [`Indicators::pns`] with a service equal to the empty string, which
    /// no client asked for and any client with an empty provider would match. Same for the PURR,
    /// which names a binding. `sip.pnsreg` is the exception because its meaning is the asking.
    #[test]
    fn a_valueless_indicator_names_nothing_rather_than_naming_the_empty_string() {
        let values = read("Feature-Caps: *;+sip.pns;+sip.pnspurr;+sip.pnsreg\r\n");
        let one = values.first().expect("one value");
        assert_eq!(one.pns(), None, "an empty service name became a service");
        assert_eq!(one.purr(), None, "an empty PURR named a binding");
        assert!(
            one.refreshes_required(),
            "sip.pnsreg asks for refreshes by being there at all"
        );
        assert_eq!(one.refresh_interval(), None);
        // And an explicitly empty value is the same claim written differently.
        assert_eq!(
            read("Feature-Caps: *;+sip.pns=\"\"\r\n")
                .first()
                .and_then(Indicators::pns),
            None
        );
    }

    /// `sip.pnsreg` present with a value this side cannot read still says the registrar wants
    /// refreshes. Reading it as absent would let the binding lapse.
    #[test]
    fn an_unreadable_refresh_interval_is_still_a_demand_for_refreshes() {
        let read = read("Feature-Caps: *;+sip.pnsreg=\"soon\"\r\n");
        let one = read.first().expect("one value");
        assert!(one.refreshes_required());
        assert_eq!(one.refresh_interval(), None);
    }

    /// RFC 6809 §4's `fc-value` opens with `*`. A row that does not is not one of these.
    #[test]
    fn a_value_that_is_not_a_feature_caps_value_is_a_parse_error() {
        assert!(Indicators::decode(b"+sip.pns=\"webpush\"").is_err());
        assert!(Indicators::decode(b"").is_err());
        // A bare `*` is legal and says nothing.
        assert!(Indicators::decode(b"*").expect("parses").is_empty());
    }

    /// §8.1's code, and the reason it is worth telling apart from every other refusal.
    #[test]
    fn the_push_notification_status_code_is_555() {
        assert_eq!(NOT_SUPPORTED, 555);
        assert!(is_not_supported(StatusCode::new(555).expect("valid")));
        assert!(!is_not_supported(StatusCode::new(500).expect("valid")));
        assert!(!is_not_supported(StatusCode::new(403).expect("valid")));
    }

    /// The name table has to know the header, or `typed_all` looks for a variant nothing carries.
    #[test]
    fn feature_caps_resolves_to_the_header_this_reads() {
        let headers = response("Feature-Caps: *;+sip.pns=\"webpush\"\r\n").headers;
        assert!(matches!(
            Headers::typed::<Indicators>(&headers),
            Some(Ok(_))
        ));
    }
}
