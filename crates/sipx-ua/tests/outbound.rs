//! Outbound, end to end (RFC 5626).
//!
//! The mechanism exists because a `Contact` naming an address behind a NAT is unroutable the moment
//! the mapping lapses. Registering once per outbound proxy is what makes that survivable — but only
//! if the flows are independent, which is a property of this code rather than of the protocol.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, Host, HostName, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Incoming, Target, bind};
use sipx_ua::{Config, Flows, InstanceId, Power, RegId, UserAgent};
use tokio::sync::mpsc::Receiver;

/// How a stub registrar answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// A 2xx that reports an Outbound registration, as §6 requires of one.
    Outbound,
    /// A 2xx that says nothing about Outbound — every registrar that does not implement RFC 5626.
    Ordinary,
    /// A 2xx naming a `Flow-Timer`.
    FlowTimer(u64),
    /// Refuses, so the flow fails.
    Refusing,
}

/// A registrar that records the `Contact` of every REGISTER it sees.
async fn registrar(
    policy: Policy,
) -> (Target, Arc<tokio::sync::Mutex<Vec<String>>>, Arc<AtomicU32>) {
    let (handle, mut incoming) = bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let target = Target::udp(handle.local_addr());
    let seen = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let count = Arc::new(AtomicU32::new(0));
    let recorder = Arc::clone(&seen);
    let counter = Arc::clone(&count);
    tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            let contact = request
                .request
                .headers
                .value(&HeaderName::Contact)
                .map(|value| String::from_utf8_lossy(&value).into_owned())
                .unwrap_or_default();
            recorder.lock().await.push(contact.clone());
            counter.fetch_add(1, Ordering::SeqCst);
            let response = answer(&request, policy, &contact);
            let _ = handle.respond(&request.key, response).await;
        }
    });
    (target, seen, count)
}

fn answer(request: &Incoming, policy: Policy, contact: &str) -> sipx_sip::Response {
    if policy == Policy::Refusing {
        return ResponseBuilder::to_request(
            &request.request,
            StatusCode::new(503).expect("valid"),
            "Service Unavailable",
        )
        .expect("builds")
        .build();
    }
    let mut builder =
        ResponseBuilder::to_request(&request.request, StatusCode::new(200).expect("valid"), "OK")
            .expect("builds")
            .header(
                HeaderName::Contact,
                Bytes::from(format!("{contact};expires=3600")),
            )
            .expect("valid");
    match policy {
        // §6: a registrar that performed an outbound registration MUST say so in Require.
        Policy::Outbound => {
            builder = builder
                .header(HeaderName::Require, Bytes::from_static(b"outbound"))
                .expect("valid");
        }
        Policy::FlowTimer(seconds) => {
            builder = builder
                .header(HeaderName::Require, Bytes::from_static(b"outbound"))
                .expect("valid")
                .header(HeaderName::FlowTimer, Bytes::from(seconds.to_string()))
                .expect("valid");
        }
        Policy::Ordinary | Policy::Refusing => {}
    }
    builder.build()
}

fn config(contact: String, target: Target) -> Config {
    Config::new(
        "<sip:alice@sipx.test>",
        contact,
        Uri::sip(Host::Name(HostName::new("sipx.test").expect("valid"))),
        target,
    )
    // §4.4.1's ten seconds is the default and the right default; a test that waits it out is just
    // a slow test, and what is under test here is which flow fails, not how long it takes to.
    .with_keepalive_timeout(Duration::from_millis(300))
}

async fn local_endpoint() -> (sipx_transport::Handle, Receiver<Incoming>) {
    bind(TransportConfig::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

/// The story's failing-first test.
///
/// Two flows to two outbound proxies. One proxy refuses; the other does not. §4.2 registers to
/// each of the outbound proxies precisely so that this case leaves the user reachable — so what
/// must not happen is the failing flow's error standing for the set.
#[tokio::test]
async fn a_second_flow_survives_the_first_being_cut() {
    let (failing, _, refusals) = registrar(Policy::Refusing).await;
    let (working, contacts, _) = registrar(Policy::Outbound).await;

    let mut flows = Flows::for_instance(InstanceId::generate());

    let (endpoint_one, _incoming_one) = local_endpoint().await;
    let contact_one = format!("<sip:alice@{}>", endpoint_one.local_addr());
    let first = flows
        .add(
            endpoint_one,
            config(contact_one, failing.clone()),
            failing.clone(),
        )
        .expect("a reg-id");

    let (endpoint_two, _incoming_two) = local_endpoint().await;
    let contact_two = format!("<sip:alice@{}>", endpoint_two.local_addr());
    let second = flows
        .add(
            endpoint_two,
            config(contact_two, working.clone()),
            working.clone(),
        )
        .expect("a reg-id");

    assert_eq!(
        first.value(),
        1,
        "reg-id numbers from the order flows are added"
    );
    assert_eq!(second.value(), 2);

    let attempts = flows.register().await;
    assert_eq!(
        attempts.len(),
        2,
        "every flow is attempted, whatever the others did"
    );

    let first_attempt = attempts
        .iter()
        .find(|attempt| attempt.reg_id == first)
        .expect("the first flow reported");
    let second_attempt = attempts
        .iter()
        .find(|attempt| attempt.reg_id == second)
        .expect("the second flow reported");

    assert!(
        first_attempt.outcome.is_err(),
        "the refusing proxy should have failed"
    );
    assert!(
        second_attempt.outcome.is_ok(),
        "the working proxy's flow was taken down by the other one failing: {:?}",
        second_attempt.outcome
    );
    assert_eq!(
        flows.active_flows(),
        vec![second],
        "the set should report exactly the flow that is up"
    );
    assert!(flows.any_active(), "the user is still reachable");

    // §4.5: the failed flow backs off on its own schedule, and because one flow is still up the
    // base is 90 seconds rather than 30 — a UA that is reachable already has nothing to gain by
    // hurrying, and a registrar having a bad day has something to lose.
    let retry = flows
        .retry_after(first)
        .expect("a failed flow has a retry delay");
    assert!(
        (Duration::from_secs(45)..=Duration::from_secs(90)).contains(&retry),
        "the first retry should be 50-100% of the 90-second base: {retry:?}"
    );
    assert!(
        flows.retry_after(second).is_none(),
        "a flow that is up is not waiting to retry"
    );
    assert!(refusals.load(Ordering::SeqCst) >= 1);

    // And the working flow registered with the Outbound parameters.
    let seen = contacts.lock().await.clone();
    let contact = seen.first().expect("the working registrar saw a REGISTER");
    assert!(contact.contains(";reg-id=2"), "{contact}");
    assert!(contact.contains("+sip.instance=\"<urn:uuid:"), "{contact}");
}

#[tokio::test]
async fn every_flow_registers_under_one_instance_with_its_own_reg_id() {
    let (target, contacts, _) = registrar(Policy::Outbound).await;
    let mut flows = Flows::for_instance(InstanceId::generate());
    for _ in 0..3 {
        let (endpoint, _incoming) = local_endpoint().await;
        let contact = format!("<sip:alice@{}>", endpoint.local_addr());
        flows
            .add(endpoint, config(contact, target.clone()), target.clone())
            .expect("a reg-id");
    }

    let attempts = flows.register().await;
    assert_eq!(attempts.len(), 3);
    assert_eq!(flows.active(), 3);

    let seen = contacts.lock().await.clone();
    let instance = flows.instance().urn().to_owned();
    let mut reg_ids: Vec<String> = Vec::new();
    for contact in &seen {
        assert!(
            contact.contains(&format!("+sip.instance=\"<{instance}>\"")),
            "every flow registers under the one device identity: {contact}"
        );
        let reg_id = contact
            .split(";reg-id=")
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .expect("a reg-id")
            .to_owned();
        reg_ids.push(reg_id);
    }
    reg_ids.sort();
    assert_eq!(
        reg_ids,
        vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        "each flow needs its own reg-id, or the registrar replaces one binding with the next"
    );
}

#[tokio::test]
async fn a_registrar_that_says_nothing_about_outbound_leaves_no_flow_to_keep_alive() {
    // Most registrars in the world do not implement RFC 5626. Asking and not getting it is not an
    // error — the binding is an ordinary one — but a UA that assumed otherwise would ping a flow
    // nothing routes down and mark the registration failed when the pings went unanswered.
    let (target, _, _) = registrar(Policy::Ordinary).await;
    let (endpoint, _incoming) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        config(contact.clone(), target.clone()).with_outbound(sipx_ua::Flow {
            instance: InstanceId::generate(),
            reg_id: RegId::new(1).expect("valid"),
        }),
    );

    agent.register().await.expect("registers");
    assert!(
        !agent.flow_accepted(),
        "silence in Require is not acceptance (§6)"
    );
    assert!(
        agent.keepalive_after(Power::Unconstrained).is_none(),
        "there is no flow, so there is nothing to keep alive"
    );
    assert_eq!(
        agent.dialog_contact(),
        contact,
        "no flow means no `ob`: the contact is reachable at its address or not at all"
    );
}

#[tokio::test]
async fn an_accepted_flow_is_kept_alive_and_marks_its_dialog_contact_with_ob() {
    let (target, _, _) = registrar(Policy::Outbound).await;
    let (endpoint, _incoming) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        config(contact, target).with_outbound(sipx_ua::Flow {
            instance: InstanceId::generate(),
            reg_id: RegId::new(1).expect("valid"),
        }),
    );

    agent.register().await.expect("registers");
    assert!(agent.flow_accepted(), "§6: the registrar said it did one");

    // UDP, so §4.4.2's 24-29 second range.
    let interval = agent
        .keepalive_after(Power::Unconstrained)
        .expect("an accepted flow is kept alive");
    assert!(
        (Duration::from_secs(24)..=Duration::from_secs(29)).contains(&interval),
        "{interval:?}"
    );
    assert!(
        agent.dialog_contact().contains(";ob>"),
        "§4.3 requires `ob` on a dialog-forming request's Contact: {}",
        agent.dialog_contact()
    );
}

#[tokio::test]
async fn the_registrars_flow_timer_replaces_our_own_interval() {
    // §4.4: the registrar is saying how long it will hold the flow open without traffic. A UA that
    // averaged that with its own preference would lose the flow between pings.
    let (target, _, _) = registrar(Policy::FlowTimer(15)).await;
    let (endpoint, _incoming) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        config(contact, target).with_outbound(sipx_ua::Flow {
            instance: InstanceId::generate(),
            reg_id: RegId::new(1).expect("valid"),
        }),
    );

    agent.register().await.expect("registers");
    assert_eq!(
        agent.keepalive_after(Power::Unconstrained),
        Some(Duration::from_secs(15))
    );
}

/// A registrar that answers REGISTER and also answers STUN keep-alives — or stops answering them.
///
/// The two halves share the socket on purpose. RFC 5626 §4.4's keep-alive travels over the flow it
/// is testing, so a fixture that answered pings on a second socket would prove nothing.
async fn flow_peer(answer_keepalives: bool) -> Target {
    let socket = Arc::new(
        tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds"),
    );
    let addr = socket.local_addr().expect("has an address");
    let listening = Arc::clone(&socket);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        while let Ok((len, from)) = listening.recv_from(&mut buf).await {
            let datagram = buf.get(..len).unwrap_or(&[]).to_vec();
            if sipx_transport::stun::is_stun(&datagram) {
                if !answer_keepalives {
                    continue;
                }
                let Some(id) = datagram
                    .get(8..20)
                    .and_then(|slice| <[u8; 12]>::try_from(slice).ok())
                else {
                    continue;
                };
                let mut response = sipx_transport::stun::binding_request(&id);
                response[0] = 0x01;
                response[1] = 0x01;
                let _ = listening.send_to(&response, from).await;
                continue;
            }
            // A REGISTER. Answered by hand rather than through an endpoint, because this socket
            // has to serve both protocols.
            let text = String::from_utf8_lossy(&datagram).into_owned();
            let field = |name: &str| {
                text.lines()
                    .find(|line| {
                        line.to_ascii_lowercase()
                            .starts_with(&name.to_ascii_lowercase())
                    })
                    .map(|line| line.trim_end().to_owned())
                    .unwrap_or_default()
            };
            let response = format!(
                "SIP/2.0 200 OK\r\n{}\r\n{}\r\n{}\r\n{}\r\n{}\r\nRequire: outbound\r\n\
                 {};expires=3600\r\nContent-Length: 0\r\n\r\n",
                field("Via:"),
                field("To:"),
                field("From:"),
                field("Call-ID:"),
                field("CSeq:"),
                field("Contact:"),
            );
            let _ = listening.send_to(response.as_bytes(), from).await;
        }
    });
    Target::udp(addr)
}

/// The keep-alive half of the story's criterion: one flow's ping failing must not disturb the rest.
#[tokio::test]
async fn a_flow_whose_keepalive_goes_unanswered_fails_alone() {
    let silent = flow_peer(false).await;
    let answering = flow_peer(true).await;

    let mut flows = Flows::for_instance(InstanceId::generate());
    for target in [silent, answering] {
        let (endpoint, _incoming) = local_endpoint().await;
        let contact = format!("<sip:alice@{}>", endpoint.local_addr());
        flows
            .add(endpoint, config(contact, target.clone()), target)
            .expect("a reg-id");
        // The endpoint receiver is dropped at the end of the loop; nothing arrives on it here,
        // since every response belongs to a transaction rather than to the application.
    }

    let registered = flows.register().await;
    assert!(
        registered.iter().all(|attempt| attempt.outcome.is_ok()),
        "both flows should register: {registered:?}"
    );
    assert!(
        registered.iter().all(|attempt| attempt.flow_accepted),
        "the fixture answers with Require: outbound (§6)"
    );
    assert_eq!(flows.active(), 2);

    let pinged = flows.keepalive().await;
    assert_eq!(pinged.len(), 2, "every accepted flow is pinged");

    let failures: Vec<_> = pinged.iter().filter(|a| a.outcome.is_err()).collect();
    assert_eq!(
        failures.len(),
        1,
        "exactly one flow should have failed: {pinged:?}"
    );
    assert_eq!(
        flows.active(),
        1,
        "the surviving flow is still registered, so the user is still reachable"
    );
    let failed = failures[0].reg_id;
    assert!(
        failures[0].retry_after.is_some(),
        "a failed flow gets a §4.5 retry delay rather than being forgotten"
    );
    assert!(
        !flows.active_flows().contains(&failed),
        "the failed flow should not still count as up"
    );
}

/// A registrar whose STUN answers report a *different* reflexive address the second time.
async fn rebinding_peer() -> Target {
    let socket = Arc::new(
        tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds"),
    );
    let addr = socket.local_addr().expect("has an address");
    let listening = Arc::clone(&socket);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        let mut pings = 0u32;
        while let Ok((len, from)) = listening.recv_from(&mut buf).await {
            let datagram = buf.get(..len).unwrap_or(&[]).to_vec();
            if sipx_transport::stun::is_stun(&datagram) {
                let Some(id) = datagram
                    .get(8..20)
                    .and_then(|slice| <[u8; 12]>::try_from(slice).ok())
                else {
                    continue;
                };
                // The NAT rebinds between the first ping and the second: same flow, same socket,
                // different mapping.
                let mapped: std::net::SocketAddr = if pings == 0 {
                    "192.0.2.1:32853".parse().expect("valid")
                } else {
                    "192.0.2.1:41000".parse().expect("valid")
                };
                pings += 1;
                let std::net::IpAddr::V4(ip) = mapped.ip() else {
                    continue;
                };
                let cookie = sipx_transport::stun::MAGIC_COOKIE;
                let mut response = sipx_transport::stun::binding_request(&id);
                response[0] = 0x01;
                response[1] = 0x01;
                response[2] = 0x00;
                response[3] = 12;
                response.extend_from_slice(&0x0020u16.to_be_bytes());
                response.extend_from_slice(&8u16.to_be_bytes());
                response.push(0);
                response.push(0x01);
                response.extend_from_slice(
                    &(mapped.port() ^ u16::try_from(cookie >> 16).expect("fits")).to_be_bytes(),
                );
                response.extend_from_slice(&(u32::from(ip) ^ cookie).to_be_bytes());
                let _ = listening.send_to(&response, from).await;
                continue;
            }
            let text = String::from_utf8_lossy(&datagram).into_owned();
            let field = |name: &str| {
                text.lines()
                    .find(|line| {
                        line.to_ascii_lowercase()
                            .starts_with(&name.to_ascii_lowercase())
                    })
                    .map(|line| line.trim_end().to_owned())
                    .unwrap_or_default()
            };
            let response = format!(
                "SIP/2.0 200 OK\r\n{}\r\n{}\r\n{}\r\n{}\r\n{}\r\nRequire: outbound\r\n\
                 {};expires=3600\r\nContent-Length: 0\r\n\r\n",
                field("Via:"),
                field("To:"),
                field("From:"),
                field("Call-ID:"),
                field("CSeq:"),
                field("Contact:"),
            );
            let _ = listening.send_to(response.as_bytes(), from).await;
        }
    });
    Target::udp(addr)
}

#[tokio::test]
async fn a_flow_whose_reflexive_address_changes_has_failed_even_though_the_pings_are_answered() {
    // §4.4.2: a UA "considers the flow failed" if the XOR-MAPPED-ADDRESS changes. This is the rule
    // that makes STUN worth using rather than pinging with an OPTIONS: the socket still works and
    // every ping is answered, but the mapping the registrar holds no longer reaches this UA, so a
    // call routed down the flow would never arrive. A keep-alive that only asked "did anything come
    // back" would report this flow healthy until a call went missing.
    let target = rebinding_peer().await;
    let (endpoint, _incoming) = local_endpoint().await;
    let contact = format!("<sip:alice@{}>", endpoint.local_addr());
    let mut agent = UserAgent::new(
        endpoint,
        config(contact, target).with_outbound(sipx_ua::Flow {
            instance: InstanceId::generate(),
            reg_id: RegId::new(1).expect("valid"),
        }),
    );
    agent.register().await.expect("registers");
    assert!(agent.flow_accepted());

    agent
        .keepalive()
        .await
        .expect("the first ping learns the mapping");
    assert_eq!(
        agent.reflexive_address(),
        Some("192.0.2.1:32853".parse().expect("valid"))
    );

    let outcome = agent.keepalive().await;
    assert!(
        matches!(outcome, Err(sipx_ua::Error::FlowRebound { .. })),
        "a changed mapping is a failed flow, however healthy the socket looks: {outcome:?}"
    );
}
