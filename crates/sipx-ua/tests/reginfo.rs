//! RFC 3680 registration package merge and hardening vectors.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_ua::event_client::PackageConsumer;
use sipx_ua::reginfo::RegistrationConsumer;

const TYPE: &[u8] = b"application/reginfo+xml";

fn consumer(limit: usize) -> RegistrationConsumer {
    RegistrationConsumer::new("sip:all@example.test", limit).expect("consumer")
}

fn full(version: u32, contacts: &str) -> String {
    format!(
        "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" state=\"full\">\
         <registration aor=\"sip:alice@example.test\" id=\"r1\" state=\"active\">\
         {contacts}</registration></reginfo>"
    )
}

fn partial(version: u32, contacts: &str) -> String {
    format!(
        "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" state=\"partial\">\
         <registration aor=\"sip:alice@example.test\" id=\"r1\" state=\"active\">\
         {contacts}</registration></reginfo>"
    )
}

fn active(id: &str, event: &str, uri: &str) -> String {
    format!("<contact id=\"{id}\" state=\"active\" event=\"{event}\"><uri>{uri}</uri></contact>")
}

fn ended(id: &str, event: &str) -> String {
    format!("<contact id=\"{id}\" state=\"terminated\" event=\"{event}\"/>")
}

/// S24-V1 and V2.
#[test]
fn full_and_partial_documents_keep_only_current_contacts() {
    let mut package = consumer(4);
    let first = package
        .consume(
            Some(TYPE),
            full(0, &active("c1", "registered", "sip:alice@192.0.2.10")).as_bytes(),
        )
        .expect("full state");
    assert_eq!(first.version, 0);
    assert_eq!(first.peers[0].name, "alice");
    assert_eq!(first.peers[0].source.resource, "sip:all@example.test");

    let second = package
        .consume(
            Some(TYPE),
            partial(
                1,
                &format!(
                    "{}{}",
                    active("c1", "refreshed", "sip:alice@192.0.2.11"),
                    active("c2", "created", "sips:alice@device.example.test")
                ),
            )
            .as_bytes(),
        )
        .expect("partial state");
    assert_eq!(second.peers.len(), 2);
    assert!(
        second
            .peers
            .iter()
            .any(|peer| peer.uri == "sip:alice@192.0.2.11")
    );

    let third = package
        .consume(Some(TYPE), partial(2, &ended("c1", "expired")).as_bytes())
        .expect("expiry");
    assert_eq!(third.peers.len(), 1);
    assert_eq!(third.peers[0].contact_id, "c2");

    let terminated = "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"3\" state=\"partial\">\
        <registration aor=\"sip:alice@example.test\" id=\"r1\" state=\"terminated\"/>\
        </reginfo>";
    let fourth = package
        .consume(Some(TYPE), terminated.as_bytes())
        .expect("registration terminates");
    assert!(fourth.peers.is_empty());
}

/// S24-V3.
#[test]
fn malformed_gap_and_capacity_fail_atomically() {
    let mut package = consumer(1);
    let initial = package
        .consume(
            Some(TYPE),
            full(0, &active("c1", "registered", "sip:a@192.0.2.1")).as_bytes(),
        )
        .expect("initial");
    assert_eq!(initial.peers.len(), 1);

    let overflow = partial(1, &active("c2", "registered", "sip:a@192.0.2.2"));
    assert_eq!(
        package
            .consume(Some(TYPE), overflow.as_bytes())
            .expect_err("bounded")
            .status,
        413
    );

    let gap = partial(2, &ended("c1", "unregistered"));
    assert_eq!(
        package
            .consume(Some(TYPE), gap.as_bytes())
            .expect_err("gap")
            .status,
        400
    );

    let recovered = package
        .consume(
            Some(b"Application/RegInfo+XML; charset=utf-8"),
            full(3, &active("c1", "refreshed", "sip:a@192.0.2.1")).as_bytes(),
        )
        .expect("a later full document restores authority");
    assert_eq!(recovered.peers.len(), 1);

    let foreign = "<!DOCTYPE reginfo [<!ENTITY x 'sip:a@192.0.2.9'>]><reginfo \
        xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"4\" state=\"partial\"/>";
    assert_eq!(
        package
            .consume(Some(TYPE), foreign.as_bytes())
            .expect_err("DTD")
            .status,
        400
    );
}

#[test]
fn contact_ids_are_unique_across_retained_registrations() {
    let mut package = consumer(4);
    package
        .consume(
            Some(TYPE),
            full(0, &active("shared", "registered", "sip:a@192.0.2.1")).as_bytes(),
        )
        .expect("initial");
    let collision = "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"1\" state=\"partial\">\
        <registration aor=\"sip:bob@example.test\" id=\"r2\" state=\"active\">\
        <contact id=\"shared\" state=\"active\" event=\"created\"><uri>sip:b@192.0.2.2</uri></contact>\
        </registration></reginfo>";
    assert_eq!(
        package
            .consume(Some(TYPE), collision.as_bytes())
            .expect_err("contact IDs are subscription-global")
            .status,
        400
    );

    let prefixed = "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" xmlns:x=\"urn:example\" \
        x:version=\"1\" state=\"partial\"/>";
    assert_eq!(
        package
            .consume(Some(TYPE), prefixed.as_bytes())
            .expect_err("a foreign attribute cannot satisfy a required native attribute")
            .status,
        400
    );
}
