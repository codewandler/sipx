//! GRUU, end to end (RFC 5627).
//!
//! The point of a GRUU is that it names *one* instance of a registered user, where the address of
//! record names all of them. That distinction is invisible to RFC 3261's URI comparison — §5.4 says
//! so outright: "A public GRUU will always be equivalent to the AOR based on URI equality rules" —
//! so a UA that recognises its own GRUU by URI equivalence alone answers to requests meant for
//! every one of the user's devices. These tests exercise the whole path: what the REGISTER offers,
//! what the registrar hands back, and which arriving requests the instance then owns.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, Target, bind};
use sipx_ua::{Config, GruuKind, InstanceId, UserAgent};
use tokio::sync::mpsc::Receiver;

/// The temporary GRUU the stub registrar mints, in §7's valueless `gr` form.
///
/// Opaque on purpose: §5.4 requires that "given a pair of GRUUs, it MUST be computationally
/// infeasible to determine whether they were issued for the same AOR or instance ID", so nothing
/// about this value may be derived from the instance it belongs to.
const TEMP_GRUU: &str = "sip:t7k2xq9f4m@sipx.test;gr";

/// What a stub registrar returns alongside the binding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Issue {
    /// Both GRUUs, which is what §5.2 has a registrar do when it has both and the UA asked.
    Both,
    /// Only the public one — §4.2 requires a UA to be ready for one, both or neither.
    PublicOnly,
    /// Neither: every registrar that does not implement RFC 5627.
    Neither,
}

/// What one REGISTER told the registrar.
#[derive(Clone, Debug)]
struct Seen {
    contact: String,
    supported: String,
}

/// A registrar that issues GRUUs for whatever instance the `Contact` presents.
async fn registrar(issue: Issue) -> (Target, Arc<tokio::sync::Mutex<Vec<Seen>>>) {
    let (handle, mut incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::udp(handle.local_addr());
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            let value = |name: &HeaderName| {
                request
                    .request
                    .headers
                    .value(name)
                    .map(|raw| String::from_utf8_lossy(&raw).into_owned())
                    .unwrap_or_default()
            };
            let record = Seen {
                contact: value(&HeaderName::Contact),
                supported: value(&HeaderName::Supported),
            };
            recorder.lock().await.push(record.clone());
            let response = answer(&request, issue, &record.contact);
            let _ = handle.respond(&request.key, response).await;
        }
    });
    (target, seen)
}

/// The instance ID a `Contact` presents in its `+sip.instance` media feature tag (RFC 5626 §4.1).
fn instance_of(contact: &str) -> Option<String> {
    let start = contact.find("+sip.instance=\"<")? + "+sip.instance=\"<".len();
    let rest = contact.get(start..)?;
    Some(rest.split('>').next()?.to_owned())
}

/// A 200 that echoes the binding with the GRUUs §5.2 has a registrar attach to it.
fn answer(request: &Incoming, issue: Issue, contact: &str) -> sipx_sip::Response {
    use std::fmt::Write as _;

    let mut echoed = format!("{contact};expires=3600");
    if issue != Issue::Neither
        && let Some(instance) = instance_of(contact)
    {
        // §5.4: a public GRUU is the AOR with the instance ID hung off it in `gr`. That is the
        // whole of why it compares equal to the AOR under RFC 3261's rules.
        let _ = write!(echoed, ";pub-gruu=\"sip:alice@sipx.test;gr={instance}\"");
        if issue == Issue::Both {
            let _ = write!(echoed, ";temp-gruu=\"{TEMP_GRUU}\"");
        }
    }
    ResponseBuilder::to_request(&request.request, StatusCode::new(200).expect("valid"), "OK")
        .expect("builds")
        .header(HeaderName::Contact, Bytes::from(echoed))
        .expect("valid")
        .build()
}

fn config(contact: String, target: Target) -> Config {
    Config::new(
        "<sip:alice@sipx.test>",
        contact,
        Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid"))),
        target,
    )
}

async fn local_endpoint() -> (Handle, Receiver<Incoming>) {
    bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

/// An OPTIONS aimed at `uri`, which is how a caller dereferences a GRUU (§4.5).
fn probe(uri: &str, call_id: &str) -> sipx_sip::Request {
    RequestBuilder::new(
        Method::Options,
        Uri::parse(Bytes::from(uri.to_owned())).expect("a URI"),
    )
    .header(HeaderName::To, Bytes::from(format!("<{uri}>")))
    .expect("valid")
    .header(
        HeaderName::From,
        Bytes::from_static(b"<sip:bob@sipx.test>;tag=caller"),
    )
    .expect("valid")
    .header(HeaderName::CallId, Bytes::from(call_id.to_owned()))
    .expect("valid")
    .cseq(1, &Method::Options)
    .expect("valid")
    .max_forwards(70)
    .build()
}

/// The story's failing-first test.
///
/// A GRUU is only worth having if a request sent to it lands on the instance that registered it and
/// on nothing else. Two things have to hold at once: the instance must recognise its own GRUU, and
/// it must *not* recognise the address of record — which §5.4 warns compares equal to a public GRUU
/// under RFC 3261 §19.1.4, because that section ignores a parameter present in only one of the two
/// URIs. A UA that leaned on `Uri::equivalent` here would answer for every device the user owns.
#[tokio::test]
async fn a_request_to_a_gruu_reaches_the_instance_that_registered_it() {
    let (target, seen) = registrar(Issue::Both).await;
    let (endpoint, mut arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let instance = InstanceId::generate();
    let mut agent = UserAgent::new(
        endpoint,
        config(contact, target).with_gruu(instance.clone(), GruuKind::Public),
    );

    agent.register().await.expect("registers");

    // §4.1: the REGISTER has to say `gruu` and carry the instance ID, or the registrar has no
    // reason to mint anything and nothing below can happen.
    let register = seen.lock().await.first().cloned().expect("a REGISTER");
    assert!(
        register.supported.contains("gruu"),
        "§4.1 makes the option tag a MUST: {}",
        register.supported
    );
    assert!(
        register
            .contact
            .contains(&format!("+sip.instance=\"<{}>\"", instance.urn())),
        "the REGISTER must present the instance the GRUU is for: {}",
        register.contact
    );

    let ours = agent
        .gruus()
        .public()
        .expect("the registrar issued a public GRUU")
        .to_string();
    assert_eq!(ours, format!("sip:alice@sipx.test;gr={}", instance.urn()));

    // The request the whole mechanism exists for: sent to the GRUU, and owned by this instance.
    let (caller, _caller_incoming) = local_endpoint().await;
    let to_us = Target::udp(agent.endpoint().local_addr());
    let mut responses = caller
        .send(probe(&ours, "to-the-gruu@sipx.test"), to_us.clone())
        .await
        .expect("sends");

    let incoming = arriving.recv().await.expect("the request arrived");
    assert!(
        agent.sent_to_our_gruu(&incoming.request),
        "a request to this instance's own GRUU was not recognised as one"
    );
    assert!(agent.answer(&incoming).await.expect("answers"));
    let response = responses.final_response().await.expect("a response");
    assert_eq!(response.status.code(), 200);

    // And the two ways of getting this wrong. The address of record is URI-equivalent to the public
    // GRUU (§5.4) and names every device the user has registered; another instance's GRUU differs
    // only in the value of `gr`.
    for (uri, why) in [
        (
            "sip:alice@sipx.test",
            "the address of record names every instance, not this one",
        ),
        (
            "sip:alice@sipx.test;gr=urn:uuid:00000000-0000-4000-8000-000000000000",
            "another instance's GRUU",
        ),
    ] {
        let mut elsewhere = caller
            .send(probe(uri, "not-ours@sipx.test"), to_us.clone())
            .await
            .expect("sends");
        let other = arriving.recv().await.expect("the request arrived");
        assert!(
            !agent.sent_to_our_gruu(&other.request),
            "{uri} was taken for this instance's GRUU: {why}"
        );
        // Still answered — an OPTIONS is answered whoever it was addressed to. What must not
        // happen is the instance claiming it was sent to *its* GRUU.
        assert!(agent.answer(&other).await.expect("answers"));
        let _ = elsewhere.final_response().await;
    }
}

/// §4.2: "A UA must be prepared for a Contact to contain just one, both, or neither."
#[tokio::test]
async fn a_registrar_that_issues_no_gruu_leaves_the_contact_alone() {
    let (target, _) = registrar(Issue::Neither).await;
    let (endpoint, _arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        config(contact.clone(), target).with_gruu(InstanceId::generate(), GruuKind::Public),
    );

    agent.register().await.expect("registers");

    assert!(agent.gruus().is_empty());
    assert_eq!(
        agent.dialog_contact(),
        contact,
        "§4.4 says to use a GRUU when there is one; with none, the plain contact is what is left"
    );
}

/// §4.4: "A UA SHOULD use a GRUU when populating the Contact header field of dialog-forming and
/// target refresh requests and responses."
#[tokio::test]
async fn a_dialog_forming_request_carries_the_gruu_rather_than_the_contact() {
    let (target, _) = registrar(Issue::Both).await;
    let (endpoint, _arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let instance = InstanceId::generate();
    let mut agent = UserAgent::new(
        endpoint,
        config(contact, target).with_gruu(instance.clone(), GruuKind::Public),
    );

    agent.register().await.expect("registers");

    assert_eq!(
        agent.dialog_contact(),
        format!("<sip:alice@sipx.test;gr={}>", instance.urn())
    );
}

/// The temporary GRUU is the half that is usually skipped, and skipping it *silently* is worse than
/// not offering GRUU at all: a caller that asked for an unlinkable address and was quietly given a
/// stable one has been told the opposite of the truth about what it just handed out.
#[tokio::test]
async fn asking_for_a_temporary_gruu_never_yields_the_public_one() {
    let (target, _) = registrar(Issue::PublicOnly).await;
    let (endpoint, _arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        config(contact.clone(), target).with_gruu(InstanceId::generate(), GruuKind::Temporary),
    );

    agent.register().await.expect("registers");

    assert!(agent.gruus().public().is_some(), "the registrar issued one");
    assert!(agent.gruus().temporary().is_none());
    assert_eq!(
        agent.dialog_contact(),
        contact,
        "a caller that asked for privacy must not be handed the stable identifier instead"
    );
}
