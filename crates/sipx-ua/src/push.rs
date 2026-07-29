//! Being reachable through a push notification (RFC 8599), from the user agent's side.
//!
//! The client this is for holds no connection at all: no socket, no keep-alive, and quite possibly
//! not running. Every other mechanism in this crate assumes there is *something* the registrar can
//! route down — Outbound keeps a flow open, GRUU names the instance at the end of one. RFC 8599 is
//! what is left when there is nothing: the proxy leaves SIP for one hop, asks the client's push
//! notification service to wake it, and the client goes and gets a flow.
//!
//! Three things are worth knowing before reading on.
//!
//! - **The push is not the call.** §4.1.3: "When a UA receives a push notification, the UA MUST
//!   send a binding-refresh REGISTER request." The notification is permission to go and get a
//!   flow; the request it was sent for arrives down that flow afterwards. A client that waits for
//!   the INVITE instead of refreshing is waiting on a path that does not exist yet — which is why
//!   the ordering is a type here ([`Pending`]) rather than a comment.
//! - **sipx ships no push service.** [`PushService`] is a trait and this repository implements it
//!   nowhere. sipx is a stack, not a client of anybody's push transport, and the
//!   [vision](../../../docs/vision.md)'s non-goals rule out the alternative. What sipx owes is the
//!   SIP half: the parameters, the option negotiation, 555, and the refresh ordering.
//! - **The proxy half is not here.** §5.6 has a proxy hold the request in a bucket while the
//!   client wakes, and §4.2's registrar behaviour mints the PURR. Both are roles sipx does not
//!   play, and neither shares anything with this but the wire format.

use std::time::Duration;

use sipx_sip::Response;
use sipx_sip::error::BuildError;
use sipx_sip::push::{Device, Indicators};

pub use sipx_sip::push::{NOT_SUPPORTED, NOT_SUPPORTED_REASON};

/// The push notification service a device can be woken through (§3).
///
/// **sipx implements this nowhere, and that is deliberate.** Waking a device means speaking some
/// vendor's HTTP API over some vendor's credentials, on a schedule that vendor sets — none of
/// which is SIP, and all of which would date faster than the rest of this crate. What sipx needs
/// from a push service is three strings, and this is them.
///
/// An implementation is an adapter the application writes over whatever it already uses to reach
/// its push service. The tests use a stub for the same reason: there is nothing here that a real
/// implementation would exercise differently.
pub trait PushService {
    /// The `pn-provider` value naming this service (§8.7).
    ///
    /// A value from the registry §8.8 creates. It is the name the *registrar* has to recognise, so
    /// inventing one produces a binding nothing will ever wake — see [`Support::supports`].
    fn provider(&self) -> &str;

    /// The `pn-prid` value: the identifier this service knows the device by (§8.7).
    fn prid(&self) -> &str;

    /// The `pn-param` value, when the service needs one (§8.7).
    ///
    /// Service-specific and not SIP's business, which is why the default is `None`.
    fn param(&self) -> Option<&str> {
        None
    }

    /// The parameters a REGISTER's `Contact` URI must carry to name this service (§4.1.2).
    ///
    /// Fails when one of the three values is not something a URI parameter can hold; see
    /// [`Device`] for why that is checked here rather than discovered at the registrar.
    fn device(&self) -> Result<Device, BuildError> {
        let device = Device::new(self.provider(), self.prid())?;
        match self.param() {
            Some(param) => device.with_param(param),
            None => Ok(device),
        }
    }
}

/// What a registrar said about push, read from the `Feature-Caps` of a REGISTER response (§8.2).
///
/// The interesting question this answers is not "did the registration succeed" — it did, or there
/// would be no response to read. It is **"can this registrar actually wake me"**, and the two come
/// apart: a registrar that supports some other push service answers 200 and records a perfectly
/// good binding that nothing will ever ring. That failure looks exactly like success from every
/// angle except this one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Support {
    services: Vec<String>,
    refreshes_required: bool,
    refresh_interval: Option<Duration>,
    purr: Option<String>,
}

impl Support {
    /// Read what a REGISTER response said about push (§8.2).
    ///
    /// Every `Feature-Caps` row is read, because a registrar offering several push services says
    /// so with several indicators and a client that stopped at the first would miss its own.
    #[must_use]
    pub fn from_response(response: &Response) -> Self {
        let mut support = Self::default();
        for value in response.headers.typed_all::<Indicators>() {
            let Ok(indicators) = value else {
                continue;
            };
            if let Some(pns) = indicators.pns() {
                support
                    .services
                    .push(String::from_utf8_lossy(pns).into_owned());
            }
            if indicators.refreshes_required() {
                support.refreshes_required = true;
                support.refresh_interval = support
                    .refresh_interval
                    .or_else(|| indicators.refresh_interval());
            }
            if let Some(purr) = indicators.purr() {
                support.purr = Some(String::from_utf8_lossy(purr).into_owned());
            }
        }
        support
    }

    /// The push notification services the registrar named (§8.2's `sip.pns`).
    #[must_use]
    pub fn services(&self) -> &[String] {
        &self.services
    }

    /// Whether the registrar named this push service.
    ///
    /// The question a client has to ask after every registration, and the reason `sip.pns` exists.
    /// `false` does not mean the registration failed — it means the binding is one nothing will
    /// wake, and a client that never asks will sit there believing it is reachable.
    ///
    /// Compared case-insensitively: §8.8's registry values are tokens.
    #[must_use]
    pub fn supports(&self, provider: &str) -> bool {
        self.services
            .iter()
            .any(|named| named.eq_ignore_ascii_case(provider))
    }

    /// Whether the registrar asked for binding refreshes even without a push (§8.2's
    /// `sip.pnsreg`).
    #[must_use]
    pub fn refreshes_required(&self) -> bool {
        self.refreshes_required
    }

    /// How long to leave between those refreshes, when the registrar said a readable number.
    ///
    /// `None` alongside a true [`Support::refreshes_required`] means the registrar asked for
    /// refreshes without naming an interval this side could read; the lease's own refresh point
    /// still applies.
    #[must_use]
    pub fn refresh_interval(&self) -> Option<Duration> {
        self.refresh_interval
    }

    /// The PURR the registrar assigned this binding (§8.2's `sip.pnspurr`).
    ///
    /// Carried, not acted on. A PURR exists so that a request can be matched to a *stored*
    /// binding without re-deriving it from the `pn-*` values, and the party that stores bindings
    /// is the registrar or the proxy of §5.6 — not a user agent, which has exactly one. sipx
    /// therefore reads it, keeps it, and hands it to the application, and does no matching with
    /// it; that half arrives with the proxy role or not at all.
    #[must_use]
    pub fn purr(&self) -> Option<&str> {
        self.purr.as_deref()
    }

    /// Whether the registrar said nothing about push at all — every registrar that does not
    /// implement RFC 8599, which is not an error and not a refusal.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.services.is_empty() && !self.refreshes_required && self.purr.is_none()
    }
}

/// Permission to expect the request a push notification was sent for (§4.1.3).
///
/// **The only way to get one is [`crate::UserAgent::woken`], and only after the binding-refresh
/// REGISTER has succeeded.** That is the whole point of the type. §4.1.3 fixes an order — push,
/// then REGISTER, then the request — and it is the easiest thing in this RFC to run backwards,
/// because waiting for the INVITE is what a woken client *feels* like it should do. A client that
/// waits without refreshing is waiting on a flow that does not exist, and the call it was woken
/// for times out somewhere it cannot see.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Pending {
    /// The lease the binding-refresh REGISTER renewed.
    pub lease: crate::registrar::Lease,
    /// The PURR the registrar assigned this binding, if it assigned one (§8.2's `sip.pnspurr`).
    pub purr: Option<String>,
}

/// Put the push parameters into a contact's URI (§4.1.2).
///
/// The parameters are **URI** parameters, and the difference is the whole of why this is not a
/// `format!`. Inside the angle brackets a `;` starts a `uri-parameter`; outside them it starts a
/// header parameter, which RFC 3261 §20 makes a different field of a different grammar. A
/// registrar reading `Contact` URIs would never see one pasted on the outside — and would answer
/// 200 to the registration, so nothing would say it had gone wrong.
///
/// A contact arriving without angle brackets gains them, because that form has nowhere to put a
/// URI parameter at all; any header parameters after it stay outside, where they were.
///
/// A contact whose URI does not parse is returned unchanged and logged. It is the application's
/// own configuration rather than network input, and a REGISTER built from it will fail on its own
/// terms; silently dropping the push parameters into a *different* URI would be worse.
#[must_use]
pub fn in_contact(contact: &str, device: &Device) -> String {
    use bytes::Bytes;

    let trimmed = contact.trim();
    let (prefix, uri_text, tail) = match (trimmed.find('<'), trimmed.rfind('>')) {
        (Some(open), Some(close)) if open < close => (
            trimmed.get(..=open).unwrap_or_default(),
            trimmed.get(open + 1..close).unwrap_or_default(),
            trimmed.get(close + 1..).unwrap_or_default(),
        ),
        // A bare addr-spec: the URI runs to the first semicolon, and everything from there is a
        // header parameter list that must stay one.
        _ => {
            let end = trimmed.find(';').unwrap_or(trimmed.len());
            (
                "<",
                trimmed.get(..end).unwrap_or_default(),
                trimmed.get(end..).unwrap_or_default(),
            )
        }
    };

    let Ok(mut uri) = sipx_sip::Uri::parse(Bytes::from(uri_text.to_owned())) else {
        tracing::warn!(
            contact,
            "the contact is not a URI sipx can parse, so RFC 8599 §4.1.2's push parameters were \
             left off the registration"
        );
        return contact.to_owned();
    };
    device.set_on(&mut uri);
    format!(
        "{prefix}{}>{tail}",
        String::from_utf8_lossy(&uri.to_bytes())
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
    use bytes::Bytes;
    use sipx_sip::{Limits, Message, parse_datagram};

    /// A stub, and the only implementation of [`PushService`] anywhere near this repository.
    struct Stub {
        param: Option<&'static str>,
    }

    impl PushService for Stub {
        fn provider(&self) -> &'static str {
            "webpush"
        }

        fn prid(&self) -> &'static str {
            "c1a5b3e7d9f2"
        }

        fn param(&self) -> Option<&str> {
            self.param
        }
    }

    fn device() -> Device {
        Stub {
            param: Some("7f3ad0"),
        }
        .device()
        .expect("valid")
    }

    /// The trait exists to turn three service-specific strings into §4.1.2's parameters, and
    /// `pn-param` is the one that is optional.
    #[test]
    fn a_service_becomes_the_parameters_a_contact_carries() {
        assert_eq!(device().provider(), "webpush");
        assert_eq!(device().param(), Some("7f3ad0"));
        assert_eq!(device().prid(), "c1a5b3e7d9f2");
        assert_eq!(Stub { param: None }.device().expect("valid").param(), None);
    }

    /// The failure this function exists to prevent: parameters after the `>` are header
    /// parameters, and a registrar reading the `Contact` URI never sees them (RFC 3261 §20).
    #[test]
    fn the_parameters_land_inside_the_angle_brackets() {
        assert_eq!(
            in_contact("<sip:alice@192.0.2.5:5060>", &device()),
            "<sip:alice@192.0.2.5:5060;pn-provider=webpush;pn-param=7f3ad0\
             ;pn-prid=c1a5b3e7d9f2>"
        );
    }

    /// A display name and the header parameters after the brackets belong to the header, not the
    /// URI, and must come through untouched.
    #[test]
    fn a_display_name_and_header_parameters_survive() {
        assert_eq!(
            in_contact("\"Alice\" <sip:alice@192.0.2.5>;expires=600", &device()),
            "\"Alice\" <sip:alice@192.0.2.5;pn-provider=webpush;pn-param=7f3ad0\
             ;pn-prid=c1a5b3e7d9f2>;expires=600"
        );
    }

    /// A bare addr-spec has nowhere to put a URI parameter, so it gains the brackets — and its
    /// header parameters stay outside them, which is the half that is easy to lose.
    #[test]
    fn a_bare_contact_gains_the_brackets_a_uri_parameter_needs() {
        assert_eq!(
            in_contact("sip:alice@192.0.2.5", &device()),
            "<sip:alice@192.0.2.5;pn-provider=webpush;pn-param=7f3ad0;pn-prid=c1a5b3e7d9f2>"
        );
        assert_eq!(
            in_contact("sip:alice@192.0.2.5;expires=600", &device()),
            "<sip:alice@192.0.2.5;pn-provider=webpush;pn-param=7f3ad0\
             ;pn-prid=c1a5b3e7d9f2>;expires=600"
        );
    }

    /// Configuration this side cannot parse is left alone rather than rewritten into something
    /// else. The REGISTER will fail on its own terms, which is a fault an operator can see.
    #[test]
    fn a_contact_that_is_not_a_uri_is_left_as_it_was() {
        assert_eq!(in_contact("<not a uri>", &device()), "<not a uri>");
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

    #[test]
    fn the_registrars_answer_says_which_service_it_can_use() {
        let support = Support::from_response(&response(
            "Feature-Caps: *;+sip.pns=\"webpush\";+sip.pnsreg=\"120\"\
             ;+sip.pnspurr=\"opaque-purr-1\"\r\n",
        ));
        assert!(support.supports("webpush"));
        // §8.8's values are tokens, and a token compares case-insensitively.
        assert!(support.supports("WebPush"));
        assert!(support.refreshes_required());
        assert_eq!(support.refresh_interval(), Some(Duration::from_secs(120)));
        assert_eq!(support.purr(), Some("opaque-purr-1"));
    }

    /// The failure that looks like success: a 200, a good binding, and a push service nobody here
    /// can reach.
    #[test]
    fn a_registrar_naming_another_service_does_not_support_ours() {
        let support = Support::from_response(&response("Feature-Caps: *;+sip.pns=\"other\"\r\n"));
        assert!(!support.supports("webpush"));
        assert!(
            !support.is_empty(),
            "it said something, just not our service"
        );
        assert_eq!(support.purr(), None);
    }

    /// A registrar offering several services says so with several indicators, and a client that
    /// stopped at the first would miss its own.
    #[test]
    fn every_named_service_is_read_not_only_the_first() {
        let support = Support::from_response(&response(
            "Feature-Caps: *;+sip.pns=\"other\"\r\n\
             Feature-Caps: *;+sip.pns=\"webpush\"\r\n",
        ));
        assert_eq!(
            support.services(),
            ["other".to_owned(), "webpush".to_owned()]
        );
        assert!(support.supports("webpush"));
    }

    /// Every registrar that does not implement RFC 8599 answers a REGISTER perfectly well and
    /// says nothing. That is not a refusal, and it is not an error.
    #[test]
    fn a_registrar_that_says_nothing_about_push_is_empty_rather_than_negative() {
        let support = Support::from_response(&response(""));
        assert!(support.is_empty());
        assert!(!support.supports("webpush"));
        assert!(!support.refreshes_required());
    }
}
