//! M10's first two clauses, demonstrated as they are written (`X-52`).
//!
//! The milestone's exit criterion is one sentence, and two thirds of it had been shown by mechanism
//! rather than by demonstration when `X-50` went looking for the evidence:
//!
//! - "one of two registrations of the same address of record can be called individually" — `T-20`
//!   showed a UA recognising its own GRUU and refusing both the address of record and another
//!   instance's, against **one** agent and an `OPTIONS`. One registration and no call.
//! - "a push wakes a client that held no connection **into an answered call**" — `T-21` showed
//!   RFC 8599 §4.1.3's order, push then binding-refresh REGISTER then the INVITE, and stopped when
//!   the INVITE arrived. Nothing answered it.
//!
//! Neither of those is reopened here: each delivered the mechanism it was written for. This file is
//! the composition on top — two registrations and a call that reaches exactly one of them, and a
//! pushed client that answers the call it was woken for and carries audio on it.
//!
//! They live beside the command line tool's tests for the reason `interop_call.rs` gives: this crate
//! is the one that already depends on the whole stack — registration, signalling, media and audio —
//! and a clause about being *reached* is a claim about all four at once. Nothing here calls into the
//! binary; the library is the thing under test.
//!
//! **What is stubbed, and why.** A registrar that mints GRUUs (RFC 5627 §5.2), a proxy that resolves
//! one to a binding (§5.2 again) and a proxy that holds a request while a pushed client wakes
//! (RFC 8599 §5.6) are all *server* roles. sipx implements none of them and is not going to: it is
//! the UA half of both RFCs. So each is a test double here, kept as thin as the clause allows — and
//! the routing double in particular is written to read the request's URI and nothing else, because a
//! double that consulted anything the test knows would be demonstrating the test rather than the
//! clause.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]
// `caller` and `callee` differ by two letters and are the words this domain uses. Same allow and
// same reason as `crates/sipx-call/tests/call.rs`.
#![allow(clippy::similar_names)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_audio::g711;
use sipx_call::{Call, DialOptions, answer, dial};
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, Target, bind};
use sipx_ua::push::PushService;
use sipx_ua::{Config, GruuKind, InstanceId, UserAgent};
use tokio::sync::mpsc::Receiver;
use tokio::sync::{Mutex, oneshot};

/// The address of record both instances of the first test register, and the one the woken client
/// registers in the second.
const AOR: &str = "sip:alice@sipx.test";

/// How long a test here waits for audio it played to arrive before calling it lost.
///
/// A bound on failure, not a window to measure in — the same constant and the same reason as
/// `crates/sipx-call/tests/call.rs` (`X-28`). What the assertion under it waits for is a *quantity*
/// of audio, so load lengthens the real wait and a bound set an order of magnitude above the honest
/// answer absorbs that.
const DELIVERY_BOUND: Duration = Duration::from_secs(10);

/// The PURR the push registrar assigns the binding (RFC 8599 §8.2).
const PURR: &str = "opaque-purr-1";

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("loopback")
}

async fn local_endpoint() -> (Handle, Receiver<Incoming>) {
    bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

fn aor_uri() -> Uri {
    Uri::parse(Bytes::from_static(AOR.as_bytes())).expect("a URI")
}

fn ua_config(contact: String, registrar: Target) -> Config {
    Config::new(
        format!("<{AOR}>"),
        contact,
        Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid"))),
        registrar,
    )
}

/// One header of an arriving request, as text.
fn header(request: &Incoming, name: &HeaderName) -> String {
    request
        .request
        .headers
        .value(name)
        .map(|raw| String::from_utf8_lossy(&raw).into_owned())
        .unwrap_or_default()
}

/// The instance ID a `Contact` presents in its `+sip.instance` media feature tag (RFC 5626 §4.1).
fn instance_of(contact: &str) -> Option<String> {
    let start = contact.find("+sip.instance=\"<")? + "+sip.instance=\"<".len();
    Some(contact.get(start..)?.split('>').next()?.to_owned())
}

/// A recognisable clip: a 440 Hz tone with a 20 ms attack, so a call that recorded silence could not
/// pass the audio assertion by recording the right *number* of samples.
///
/// The attack is short on purpose. A ramp long enough to matter over a clip this length would put the
/// clip's own peak below what [`heard`] requires of it, and the assertion that would then fail is
/// about the fixture rather than about the call.
fn clip(milliseconds: usize) -> Vec<i16> {
    (0..milliseconds * 8)
        .map(|i| {
            let t = f64::from(u32::try_from(i).unwrap_or(0)) / 8000.0;
            let envelope = (t * 50.0).min(1.0);
            let value = (t * 440.0 * std::f64::consts::TAU).sin() * 12_000.0 * envelope;
            i16::try_from(value.round() as i32).unwrap_or(0)
        })
        .collect()
}

/// The loudest sample in a clip.
fn peak_of(samples: &[i16]) -> i32 {
    samples
        .iter()
        .map(|sample| i32::from(sample.abs()))
        .max()
        .unwrap_or(0)
}

/// Play `samples` into `from` and hand back what arrived at `to`.
async fn carried(from: &Call, to: &Call, samples: &[i16]) -> Vec<i16> {
    let (_played, heard) = tokio::join!(
        from.media().play(samples, 160),
        to.media().record_at_least(samples.len(), DELIVERY_BOUND)
    );
    heard
}

/// Whether an INVITE is waiting at an endpoint, draining whatever else is.
///
/// `try_recv` and not a timed wait, because there is a happens-before to stand on: the call this is
/// asked after was placed, routed, answered and carried audio, so anything this endpoint was ever
/// going to be sent has been sent. A duration here would be a definition of silence standing in for
/// that ordering, which is the substitution `docs/designs/media.md` forbids.
///
/// An INVITE and not "anything at all", because an endpoint that has already taken a call of its own
/// has its ACK and its in-dialog traffic waiting here too. What the clause forbids is the *call*
/// arriving at the wrong instance.
fn saw_an_invite(arriving: &mut Receiver<Incoming>) -> bool {
    let mut seen = false;
    while let Ok(request) = arriving.try_recv() {
        seen |= request.request.method == Method::Invite;
    }
    seen
}

/// Assert that `recorded` is the clip that was played, and not silence of the right length.
///
/// G.711 is lossy, so the samples cannot be compared directly; the codec is idempotent on its own
/// output, so encoding both sides must agree exactly — a stronger claim than "close enough", and one
/// a dropped or reordered packet would break. The clip's own loudness is asserted alongside it,
/// because equality is only evidence of audio when what it is equal to is audible.
fn heard(who: &str, played: &[i16], recorded: &[i16]) {
    let loudness = peak_of(played);
    assert!(
        loudness > 8000,
        "the clip is too quiet for equality with it to mean audio arrived: peak {loudness}"
    );
    assert_eq!(recorded.len(), played.len(), "{who} heard a short clip");
    assert_eq!(
        g711::ulaw_encode_all(played),
        g711::ulaw_encode_all(recorded),
        "{who} heard something other than what was played"
    );
}

// ---------------------------------------------------------------------------------------------
// The GRUU clause: two registrations of one address of record, and a call that reaches one.
// ---------------------------------------------------------------------------------------------

/// One binding a registrar holds for the address of record.
#[derive(Clone, Debug)]
struct Binding {
    /// The instance the `Contact` presented in `+sip.instance` (RFC 5626 §4.1, RFC 5627 §4.1).
    instance: String,
    /// The `Contact` value as it arrived, echoed back in the 2xx with §5.2's GRUU hung off it.
    contact: String,
    /// The flow it arrived over, and so where a request routed to this binding is sent.
    flow: SocketAddr,
}

/// Where a request goes, decided from its Request-URI and the bindings alone.
///
/// This is the whole of the routing double, and the whole of the clause: RFC 5627 §5.2 has a
/// registrar resolve a public GRUU to the *one* binding whose instance the `gr` parameter names,
/// while RFC 3261 §16.6 has the address of record resolve to every binding there is. Two answers
/// from one function, separated by nothing but the URI — which is why a call can be placed at one
/// instance of a registration and why a call placed at the address of record cannot.
///
/// [`sipx_sip::gruu::gr_value`] is what reads the parameter, and reading it is *not* the same as URI
/// equality: §5.4 warns that "a public GRUU will always be equivalent to the AOR based on URI
/// equality rules", so a double that compared URIs would fan every GRUU out to both instances and
/// this test would pass while demonstrating the opposite of the clause.
fn route(bindings: &[Binding], request_uri: &Uri) -> Vec<SocketAddr> {
    match sipx_sip::gruu::gr_value(request_uri) {
        Some(instance) => bindings
            .iter()
            .filter(|binding| binding.instance.as_bytes() == instance)
            .map(|binding| binding.flow)
            .collect(),
        None => bindings.iter().map(|binding| binding.flow).collect(),
    }
}

/// A registrar that mints a public GRUU per instance and keeps every binding for the AOR (§5.2).
async fn registrar() -> (Target, Arc<Mutex<Vec<Binding>>>) {
    let (handle, mut incoming) = local_endpoint().await;
    let target = Target::udp(handle.local_addr());
    let bindings: Arc<Mutex<Vec<Binding>>> = Arc::new(Mutex::new(Vec::new()));
    let held = Arc::clone(&bindings);
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            if request.request.method != Method::Register {
                continue;
            }
            let contact = header(&request, &HeaderName::Contact);
            let Some(instance) = instance_of(&contact) else {
                continue;
            };
            let snapshot = {
                let mut bindings = held.lock().await;
                // A refresh replaces the binding it refreshes rather than adding a second one.
                bindings.retain(|binding| binding.instance != instance);
                bindings.push(Binding {
                    instance,
                    contact,
                    flow: request.source,
                });
                bindings.clone()
            };
            let _ = handle
                .respond(&request.key, registered(&request, &snapshot))
                .await;
        }
    });
    (target, bindings)
}

/// A 200 listing **every** current binding for the address of record, each carrying the public GRUU
/// §5.2 mints for the instance that owns it.
///
/// Every binding, not only the one just refreshed: RFC 3261 §10.3 has a registrar return the
/// complete set, and doing that here is what makes the two rows distinguishable by nothing but
/// `+sip.instance` — the selection RFC 5627 §4.2 requires of a UA reading its own GRUU out of the
/// answer, and the reason a UA that read the first row would end up answering to another phone's
/// address.
fn registered(request: &Incoming, bindings: &[Binding]) -> sipx_sip::Response {
    let mut builder = ResponseBuilder::to_request(
        &request.request,
        StatusCode::new(200).expect("valid"),
        "OK",
    )
    .expect("builds");
    for binding in bindings {
        let value = format!(
            "{};pub-gruu=\"{AOR};gr={}\";expires=3600",
            binding.contact, binding.instance
        );
        builder = builder
            .header(HeaderName::Contact, Bytes::from(value))
            .expect("valid");
    }
    builder.build()
}

/// One instance of the address of record: its own endpoint, its own instance ID, registered.
async fn instance(registrar: Target) -> (UserAgent, Receiver<Incoming>) {
    let (endpoint, arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        ua_config(contact, registrar).with_gruu(InstanceId::generate(), GruuKind::Public),
    );
    agent.register().await.expect("the instance registers");
    (agent, arriving)
}

/// Place a call at `gruu`, carried to `flow`, and answer it at the instance the GRUU names.
///
/// Both ends of the call come back, because the clause is about a call that *works* and not about an
/// INVITE that arrived; the caller asserts audio on the pair.
///
/// `other` is here so that the two halves of "individually" are asserted in one place: the instance
/// the GRUU names claims the request, and the instance it does not name would have refused it even if
/// the routing had sent it there. Splitting those apart is how a test comes to assert the first and
/// quietly assume the second.
async fn called_at(
    caller: &Handle,
    gruu: &Uri,
    flow: SocketAddr,
    mine: &UserAgent,
    other: &UserAgent,
    arriving: &mut Receiver<Incoming>,
) -> (Call, Call) {
    let dialing = tokio::spawn({
        let (endpoint, to) = (caller.clone(), gruu.clone());
        async move {
            dial(
                &endpoint,
                Target::udp(flow),
                &to,
                &DialOptions::new("<sip:bob@sipx.test>", loopback()),
            )
            .await
        }
    });

    let incoming = arriving
        .recv()
        .await
        .expect("the INVITE reached the instance the GRUU names");
    assert_eq!(incoming.request.method, Method::Invite);
    assert!(
        mine.sent_to_our_gruu(&incoming.request),
        "the instance did not recognise the call as addressed to its own GRUU"
    );
    assert!(
        !other.sent_to_our_gruu(&incoming.request),
        "the other instance would have claimed a call addressed to this one's GRUU"
    );

    let callee = answer(mine.endpoint(), &incoming, loopback())
        .await
        .expect("the instance answers the call placed at its GRUU");
    let caller = dialing
        .await
        .expect("the dialling task")
        .expect("the call connects");
    (caller, callee)
}

/// Carry a clip each way over a connected pair, and assert both ends heard what was played.
async fn audio_passes(caller: &Call, callee: &Call, callee_name: &str) {
    let from_caller = clip(200);
    let from_callee: Vec<i16> = from_caller.iter().map(|sample| -sample).collect();
    heard(
        callee_name,
        &from_caller,
        &carried(caller, callee, &from_caller).await,
    );
    heard(
        "the caller",
        &from_callee,
        &carried(callee, caller, &from_callee).await,
    );
}

/// M10's first clause, as it is written: **one of two registrations of the same address of record
/// can be called individually.**
///
/// Two instances register one AOR. Each is then called at its own GRUU, carried to wherever [`route`]
/// resolves that GRUU — the only decision here allowed to know anything, and what it knows is the
/// Request-URI. Each answers its own call and carries audio both ways, and neither ever sees the
/// other's.
///
/// **Both calls, not one.** A single call reaching one instance while the other stays silent is also
/// what a test would show if the second instance had never registered, or had registered and were
/// unreachable — and `X-50` filed this story because that is exactly the kind of substitution M10's
/// evidence had already made once. Calling *each* of them closes it: the second instance's silence
/// during the first call cannot be its own brokenness, because the next thing it does is take a call.
///
/// The contrast that makes the GRUU load-bearing is the routing assertion: the same function applied
/// to the address of record resolves to **both** bindings. Being individually callable is not a
/// property of having registered — it is a property of the GRUU, and this is where the two come apart.
#[tokio::test(flavor = "multi_thread")]
async fn each_of_two_registrations_of_an_address_of_record_is_called_individually() {
    let (registrar_target, bindings) = registrar().await;
    let (one, mut arriving_at_one) = instance(registrar_target.clone()).await;
    let (two, mut arriving_at_two) = instance(registrar_target).await;

    // Two registrations, one address of record. Without this the rest proves nothing: a test that
    // fails because the second instance never registered has said nothing about being individually
    // callable.
    let bindings = bindings.lock().await.clone();
    assert_eq!(
        bindings.len(),
        2,
        "both instances must be currently registered for the same AOR: {bindings:?}"
    );
    let gruu_of_one = one
        .gruus()
        .public()
        .expect("the registrar issued a public GRUU to the first instance")
        .clone();
    let gruu_of_two = two
        .gruus()
        .public()
        .expect("the registrar issued a public GRUU to the second instance")
        .clone();
    assert_ne!(
        gruu_of_one.to_string(),
        gruu_of_two.to_string(),
        "two instances of one AOR were issued the same GRUU, which names neither"
    );

    // The address of record names both bindings, and that is exactly what a GRUU is for.
    let fan_out = route(&bindings, &aor_uri());
    assert_eq!(
        fan_out.len(),
        2,
        "the AOR must reach every registration; if it reached one, the clause would be vacuous"
    );
    let (flow_of_one, flow_of_two) = (route(&bindings, &gruu_of_one), route(&bindings, &gruu_of_two));
    assert_eq!(
        flow_of_one,
        vec![one.endpoint().local_addr()],
        "the GRUU must resolve to the one binding whose instance it names (RFC 5627 §5.2)"
    );
    assert_eq!(flow_of_two, vec![two.endpoint().local_addr()]);

    let (caller_endpoint, _caller_incoming) = local_endpoint().await;

    // The first instance's call, at the first instance's GRUU.
    let (caller_one, callee_one) = called_at(
        &caller_endpoint,
        &gruu_of_one,
        flow_of_one[0],
        &one,
        &two,
        &mut arriving_at_one,
    )
    .await;
    audio_passes(&caller_one, &callee_one, "the first instance").await;
    assert!(
        !saw_an_invite(&mut arriving_at_two),
        "a call placed at the first instance's GRUU reached the second registration too"
    );

    // And the second instance's call, at the second instance's GRUU. Same address of record, same
    // registrar, different instance — which is the whole of "individually".
    let (caller_two, callee_two) = called_at(
        &caller_endpoint,
        &gruu_of_two,
        flow_of_two[0],
        &two,
        &one,
        &mut arriving_at_two,
    )
    .await;
    audio_passes(&caller_two, &callee_two, "the second instance").await;
    assert!(
        !saw_an_invite(&mut arriving_at_one),
        "a call placed at the second instance's GRUU reached the first registration too"
    );
}

// ---------------------------------------------------------------------------------------------
// The push clause: a client holding no connection, woken into an answered call.
// ---------------------------------------------------------------------------------------------

/// The push notification service the client is registered with.
///
/// A test double, and the only kind of implementation of [`PushService`] that exists here: sipx is a
/// stack, not a client of anybody's push transport. `webpush` is one of the values RFC 8599 §8.8
/// seeds its registry with, and it names a protocol (RFC 8030) rather than a vendor.
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

/// What the stub did, in the order it did it.
type Timeline = Arc<Mutex<Vec<&'static str>>>;

/// A registrar that answers a binding-refresh REGISTER and, in answering it, releases the request
/// it has been holding (RFC 8599 §5.6).
///
/// The release is a signal rather than the request itself, because the request is a real call now and
/// a real call is placed by a user agent. What §5.6 licenses is only the *moment*.
///
/// **What the signal carries is the point.** It is the flow the binding was created on — the address
/// the REGISTER arrived from — and that address does not exist anywhere in this stub until the
/// REGISTER arrives. So the held call cannot be placed early even by mistake: there is nothing to
/// address it to. That is the clause's "held no connection" as a fact about the test rather than a
/// stipulation in a comment, and it is why the ordering here is a happens-before and not a wait.
async fn push_registrar() -> (Target, Timeline, oneshot::Receiver<SocketAddr>) {
    let (handle, mut incoming) = local_endpoint().await;
    let target = Target::udp(handle.local_addr());
    let timeline: Timeline = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&timeline);
    let (release, released) = oneshot::channel();
    tokio::spawn(async move {
        let mut release = Some(release);
        while let Some(request) = incoming.recv().await {
            if request.request.method != Method::Register {
                continue;
            }
            recorder.lock().await.push("register");
            let contact = header(&request, &HeaderName::Contact);
            let response = ResponseBuilder::to_request(
                &request.request,
                StatusCode::new(200).expect("valid"),
                "OK",
            )
            .expect("builds")
            .header(
                HeaderName::Contact,
                Bytes::from(format!("{contact};expires=600")),
            )
            .expect("valid")
            .header(
                HeaderName::Other(Bytes::from_static(b"Feature-Caps")),
                Bytes::from(format!(
                    "*;+sip.pns=\"webpush\";+sip.pnsreg=\"120\";+sip.pnspurr=\"{PURR}\""
                )),
            )
            .expect("valid")
            .build();
            let _ = handle.respond(&request.key, response).await;
            // Only now, and only down the flow this REGISTER arrived on. Before the refresh there
            // was no binding, so there was no address to release the held request to.
            if let Some(release) = release.take() {
                let _ = release.send(request.source);
            }
        }
    });
    (target, timeline, released)
}

/// M10's second clause, as it is written: **a push wakes a client that held no connection into an
/// answered call.**
///
/// `T-21` proved the ordering and stopped at the INVITE. This carries it the rest of the way: the
/// woken client answers, and the call it was woken for carries audio in both directions. The ordering
/// is still asserted, because an answered call proves nothing about push if the client was reachable
/// all along — the premise of the whole mechanism is that before the push there was no flow at all,
/// and here that premise is enforced rather than asserted: the held call is addressed to the flow the
/// binding-refresh REGISTER created, and until it arrives there is no such address.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_wakes_a_client_that_held_no_connection_into_an_answered_call() {
    let (client, mut arriving) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", client.local_addr());
    let (registrar_target, timeline, released) = push_registrar().await;

    let device = Doorbell.device().expect("valid push parameters");
    let mut agent = UserAgent::new(client, ua_config(contact, registrar_target).with_push(device));

    // The client holds no connection: it has not registered, and nothing is on its way.
    assert!(arriving.try_recv().is_err());

    // The call §5.6's proxy is holding. It goes out the moment the binding is refreshed and cannot go
    // out before: what it is addressed to is the flow that REGISTER created.
    let (caller_endpoint, _caller_incoming) = local_endpoint().await;
    let held = tokio::spawn(async move {
        let flow = released.await.expect("the binding was refreshed");
        dial(
            &caller_endpoint,
            Target::udp(flow),
            &aor_uri(),
            &DialOptions::new("<sip:bob@sipx.test>", loopback()),
        )
        .await
    });

    // The push. sipx neither sends nor receives one — this is the test double ringing.
    timeline.lock().await.push("push");
    let pending = agent.woken().await.expect("the binding is refreshed");
    assert_eq!(
        pending.purr.as_deref(),
        Some(PURR),
        "§8.2's PURR names the binding the request will be released down"
    );

    let incoming = arriving
        .recv()
        .await
        .expect("the INVITE the push was sent for");
    assert_eq!(incoming.request.method, Method::Invite);
    timeline.lock().await.push("invite");

    let callee = answer(agent.endpoint(), &incoming, loopback())
        .await
        .expect("the woken client answers the call");
    let caller = held
        .await
        .expect("the held call's task")
        .expect("the call connects");
    timeline.lock().await.push("answered");

    audio_passes(&caller, &callee, "the woken client").await;

    assert_eq!(
        *timeline.lock().await,
        ["push", "register", "invite", "answered"],
        "§4.1.3's order is push, then the binding-refresh REGISTER, then the request — and the \
         clause is that the request is answered"
    );
}
