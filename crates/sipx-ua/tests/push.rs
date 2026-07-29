//! Being reachable through a push notification, end to end (RFC 8599).
//!
//! The client in these tests holds no connection at all: no socket the registrar can route down,
//! no keep-alive, nothing. What makes it reachable is an ordering, and §4.1.3 states it in one
//! sentence — "When a UA receives a push notification, the UA MUST send a binding-refresh REGISTER
//! request". The push is not the call. It is permission to go and get a flow, and the request the
//! push was sent for arrives down that flow afterwards. A client that skips the refresh and waits
//! for the INVITE is waiting on a path that does not exist yet.
//!
//! sipx implements the UA half only. It sends no push notification and receives none — the service
//! is behind [`PushService`] and there is no implementation of it in this repository; the stub
//! below is a test double. Holding the request while the client wakes is §5.6's proxy behaviour and
//! is stubbed here for the same reason.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::push::Device;
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, Target, bind};
use sipx_ua::push::PushService;
use sipx_ua::{Config, UserAgent};
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;

/// The push notification service the client is registered with.
///
/// A test double, and the only kind of implementation of [`PushService`] that exists here: sipx is
/// a stack, not a client of anybody's push transport. The `pn-provider` value is `webpush`, which
/// is one of the values RFC 8599 §8.8 seeds its registry with and names a protocol (RFC 8030)
/// rather than a vendor.
struct Doorbell;

impl PushService for Doorbell {
    fn provider(&self) -> &'static str {
        "webpush"
    }

    fn prid(&self) -> &'static str {
        "c1a5b3e7d9f2"
    }

    fn param(&self) -> Option<&str> {
        Some("7f3ad0")
    }
}

/// What the stub registrar did, in the order it did it.
type Timeline = Arc<Mutex<Vec<&'static str>>>;

/// What one REGISTER told the registrar.
#[derive(Clone, Debug, Default)]
struct Seen {
    contact: String,
}

/// How the stub registrar answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// A 200 naming the same push service the client did, plus a PURR for the binding.
    Supported,
    /// A 200 naming a *different* push service — which is how a client finds out its whole
    /// reachability plan is wrong without being told so outright.
    SomeOtherService,
    /// 555, §8.1's answer: this registrar does not support the named push service at all.
    NotSupported,
}

async fn local_endpoint() -> (Handle, Receiver<Incoming>) {
    bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn config(contact: String, target: Target) -> Config {
    Config::new(
        "<sip:alice@sipx.test>",
        contact,
        Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid"))),
        target,
    )
}

/// The INVITE the push was sent for — §4.1.3's "pending request", released down the flow the
/// binding-refresh REGISTER just created.
fn invite(to: &str) -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Invite,
        Uri::parse(Bytes::from(to.to_owned())).expect("a URI"),
    )
    .header(HeaderName::To, Bytes::from_static(b"<sip:alice@sipx.test>"))
    .expect("valid")
    .header(
        HeaderName::From,
        Bytes::from_static(b"<sip:bob@sipx.test>;tag=caller"),
    )
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from_static(b"woken@sipx.test"))
    .expect("valid")
    .cseq(1, &Method::Invite)
    .expect("valid")
    .max_forwards(70)
    .build()
}

/// A registrar that answers REGISTER, and — when `release` is set — a proxy that has been holding
/// one INVITE and lets it go the moment the binding is refreshed.
///
/// The second half is §5.6's, which is a proxy role and not sipx's. It is stubbed because the
/// ordering under test is only observable against something that keeps it.
async fn registrar(
    answer: Answer,
    release: Option<Target>,
) -> (Target, Timeline, Arc<Mutex<Seen>>) {
    let (handle, mut incoming) = local_endpoint().await;
    let target = Target::udp(handle.local_addr());
    let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(Mutex::new(Seen::default()));
    let (recorder, record) = (Arc::clone(&timeline), Arc::clone(&seen));
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            if request.request.method != Method::Register {
                continue;
            }
            let contact = request
                .request
                .headers
                .value(&HeaderName::Contact)
                .map(|raw| String::from_utf8_lossy(&raw).into_owned())
                .unwrap_or_default();
            *record.lock().await = Seen {
                contact: contact.clone(),
            };
            recorder.lock().await.push("register");
            let _ = handle
                .respond(&request.key, respond(&request, answer, &contact))
                .await;
            // Only now. The client has a flow; before the refresh there was nowhere to send this.
            if let Some(to) = &release {
                let _ = handle.send(invite("sip:alice@sipx.test"), to.clone()).await;
            }
        }
    });
    (target, timeline, seen)
}

fn respond(request: &Incoming, answer: Answer, contact: &str) -> sipx_sip::Response {
    let (code, reason, caps) = match answer {
        Answer::Supported => (
            200,
            "OK",
            Some("*;+sip.pns=\"webpush\";+sip.pnsreg=\"120\";+sip.pnspurr=\"opaque-purr-1\""),
        ),
        Answer::SomeOtherService => (200, "OK", Some("*;+sip.pns=\"fcm\"")),
        Answer::NotSupported => (555, "Push Notification Service Not Supported", None),
    };
    let mut builder = ResponseBuilder::to_request(
        &request.request,
        StatusCode::new(code).expect("valid"),
        reason,
    )
    .expect("builds");
    if code == 200 {
        builder = builder
            .header(
                HeaderName::Contact,
                Bytes::from(format!("{contact};expires=600")),
            )
            .expect("valid");
    }
    if let Some(caps) = caps {
        builder = builder
            .header(
                HeaderName::Other(Bytes::from_static(b"Feature-Caps")),
                Bytes::from(caps.to_owned()),
            )
            .expect("valid");
    }
    builder.build()
}

/// The story's failing-first test.
///
/// Three facts have to hold at once, and they are the whole of RFC 8599's UA half:
///
/// - the REGISTER told the registrar how to reach the push service (§4.1.2's `Contact` URI
///   parameters), or none of what follows can happen;
/// - the push produced a **binding-refresh REGISTER** (§4.1.3), not a wait;
/// - the INVITE arrived *after* that REGISTER, down the flow it created.
///
/// The last is the one that is easy to get backwards, and getting it backwards produces a client
/// that is silently unreachable for exactly the calls push notification exists to deliver.
#[tokio::test]
async fn a_push_wakes_a_client_that_refreshes_its_binding_before_the_invite() {
    let (endpoint, mut arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let to_client = Target::udp(endpoint.local_addr());
    let (target, timeline, seen) = registrar(Answer::Supported, Some(to_client)).await;

    let device = Doorbell.device().expect("valid push parameters");
    let mut agent = UserAgent::new(endpoint, config(contact, target).with_push(device));

    // The client holds no connection: it has not registered, and nothing is on its way.
    assert!(arriving.try_recv().is_err());

    // The push. sipx neither sends nor receives one — this is the test double ringing.
    timeline.lock().await.push("push");
    let pending = agent.woken().await.expect("the binding is refreshed");

    // §4.1.2: the parameters that told the registrar which push service to use, and they are URI
    // parameters — inside the angle brackets, where the registrar's URI parser will find them.
    let registered = seen.lock().await.contact.clone();
    for param in [
        "pn-provider=webpush",
        "pn-param=7f3ad0",
        "pn-prid=c1a5b3e7d9f2",
    ] {
        assert!(
            registered.contains(param),
            "the REGISTER did not carry {param}: {registered}"
        );
    }
    // Inside the angle brackets, where a `;` starts a URI parameter. Outside them it starts a
    // *header* parameter, which is a different field of a different grammar (RFC 3261 §20), and a
    // registrar reading `Contact` URIs would never see it.
    assert!(
        registered.find("pn-provider=") < registered.rfind('>'),
        "the push parameters were pasted onto the serialized contact rather than put in its URI: \
         {registered}"
    );

    // §8.2: the registrar named the push service it supports, and assigned this binding a PURR.
    assert!(agent.push_support().supports("webpush"));
    assert_eq!(pending.purr.as_deref(), Some("opaque-purr-1"));

    // And the request the push was sent for, which could not have arrived any earlier.
    let incoming = arriving.recv().await.expect("the INVITE arrived");
    assert_eq!(incoming.request.method, Method::Invite);
    timeline.lock().await.push("invite");

    assert_eq!(
        *timeline.lock().await,
        ["push", "register", "invite"],
        "§4.1.3's order is push, then the binding-refresh REGISTER, then the request"
    );
}

/// §8.1: 555 is the one answer that says the client's whole reachability plan is wrong. Reported
/// as a generic failure it is indistinguishable from a bad password, and a client retries forever.
#[tokio::test]
async fn a_555_says_the_named_push_service_is_not_supported() {
    let (endpoint, _arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let (target, _timeline, _seen) = registrar(Answer::NotSupported, None).await;

    let device = Doorbell.device().expect("valid push parameters");
    let mut agent = UserAgent::new(endpoint, config(contact, target).with_push(device));

    let error = agent.register().await.expect_err("555 is a refusal");
    assert!(
        matches!(&error, sipx_ua::Error::PushNotSupported { .. }),
        "555 was surfaced as a generic failure: {error}"
    );
}

/// §8.2's `sip.pns` exists so a client can tell whether the registrar supports the service it
/// named. A registrar that answers 200 while naming a different one has not refused — and a client
/// that does not read the indicator will sit there believing it is reachable.
#[tokio::test]
async fn a_registrar_naming_another_push_service_is_not_support_for_ours() {
    let (endpoint, _arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let (target, _timeline, _seen) = registrar(Answer::SomeOtherService, None).await;

    let device = Doorbell.device().expect("valid push parameters");
    let mut agent = UserAgent::new(endpoint, config(contact, target).with_push(device));

    agent.register().await.expect("the registration succeeded");

    assert!(!agent.push_support().supports("webpush"));
    assert!(agent.push_support().supports("fcm"));
    assert!(
        agent.push_support().purr().is_none(),
        "no PURR was assigned, and inventing one would name a binding that does not exist"
    );
}

/// §4.1.2's parameters are URI parameters, and the difference is not cosmetic: outside the angle
/// brackets a `;` starts a *header* parameter, which is a different field of a different grammar
/// (RFC 3261 §20). A registrar reading `Contact` URIs would never see them.
#[test]
fn the_push_parameters_are_uri_parameters() {
    let device = Device::new("webpush", "c1a5b3e7d9f2").expect("valid");
    let mut uri = Uri::parse(Bytes::from_static(b"sip:alice@sipx.test")).expect("a URI");
    device.set_on(&mut uri);
    assert_eq!(
        uri.to_bytes(),
        Bytes::from_static(b"sip:alice@sipx.test;pn-provider=webpush;pn-prid=c1a5b3e7d9f2")
    );
    assert_eq!(Device::from_uri(&uri).as_ref(), Some(&device));
}
