//! The join between the event packages and the notifier that serves them (RFC 6665 §4.4).
//!
//! [`sipx_ua::packages`] and [`sipx_ua::presence`] produce documents and nothing else — they hold
//! no subscriptions and never look inside the notifier. This is the file that proves it, by doing
//! the joining from outside: a real SUBSCRIBE goes through [`Subscriptions`], the package produces
//! the body the NOTIFY would carry, and the subscription ends through the framework's own
//! operation.
//!
//! Two acceptance criteria only exist at this seam and cannot be tested on either side alone: that
//! each package is reachable **by the name a subscriber asks for**, and that a subscription ends
//! with `reason=noresource` when the thing it watched disappears — rather than being left to lapse,
//! which leaves a busy-lamp field lit for something that is gone until its expiry runs out.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::event::{BAD_EVENT, Packages, Reason, State};
use sipx_sip::{Limits, Message, Request, parse_datagram};
use sipx_ua::packages::{
    Contact, ContactEvent, Dialog, DialogState, DialogWatch, Direction, RegistrationWatch,
};
use sipx_ua::presence::{Compositor, Pidf, Publish, Published, Tuple};
use sipx_ua::subscribe::{Answer, Subscriptions};

const NOW: u64 = 1_700_000_000;
const ENTITY: &str = "sip:alice@sipx.test";

/// A SUBSCRIBE for a package, from a watcher identified by its tag.
fn subscribe(event: &str, tag: &str) -> Request {
    let text = format!(
        "SUBSCRIBE sip:alice@sipx.test SIP/2.0\r\n\
         Via: SIP/2.0/UDP watcher.example;branch=z9hG4bK{tag}\r\n\
         To: <sip:alice@sipx.test>\r\n\
         From: <sip:watcher@example.net>;tag={tag}\r\n\
         Call-ID: sub-{tag}@watcher\r\n\
         CSeq: 1 SUBSCRIBE\r\n\
         Event: {event}\r\n\
         Expires: 600\r\n\
         Max-Forwards: 70\r\n\
         Content-Length: 0\r\n\r\n"
    );
    match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses") {
        Message::Request(request) => request,
        Message::Response(_) => panic!("a request"),
    }
}

/// A notifier serving all three packages, named the way each package names itself.
fn notifier() -> Subscriptions {
    Subscriptions::new(
        Packages::new()
            .with(DialogWatch::package())
            .with(RegistrationWatch::package())
            .with("presence"),
        Duration::from_secs(3600),
    )
}

/// Both packages register with the framework **by name** — the name a subscriber puts in `Event`,
/// not one the notifier is separately told about. Registering under a name nothing asks for is a
/// package that is never reached, and the subscriber cannot tell that from a notifier that is slow.
#[test]
fn both_packages_register_with_the_notifier_under_the_name_a_subscriber_asks_for() {
    // Registered with what each package calls itself; subscribed to with the literal token a desk
    // phone puts in `Event`. Using the package's own name on both sides would pass whatever it
    // returned, which tests the notifier's string comparison and nothing about the package.
    let mut notifier = notifier();

    assert!(matches!(
        notifier.on_subscribe(&subscribe("dialog", "w1"), NOW),
        Answer::Established { .. }
    ));
    assert!(matches!(
        notifier.on_subscribe(&subscribe("reg", "w2"), NOW),
        Answer::Established { .. }
    ));
    assert_eq!(notifier.active(), 2);
}

/// A notifier that does not serve a package refuses it by name, rather than accepting the
/// subscription and never producing a document for it.
#[test]
fn a_notifier_without_a_package_refuses_a_subscription_to_it() {
    let mut only_dialog = Subscriptions::new(
        Packages::new().with(DialogWatch::package()),
        Duration::from_secs(3600),
    );
    assert_eq!(
        only_dialog.on_subscribe(&subscribe(RegistrationWatch::package(), "w1"), NOW),
        Answer::Unserved { status: BAD_EVENT }
    );
    assert_eq!(only_dialog.active(), 0);
}

/// §4.1.3: `noresource` says the thing subscribed to no longer exists. The alternative — letting
/// the subscription lapse — tells the watcher nothing until its expiry runs out, so a busy-lamp
/// field goes on showing state for a line that is gone.
#[test]
fn a_dialog_subscription_ends_with_noresource_when_its_resource_disappears() {
    let mut notifier = notifier();
    let mut watch = DialogWatch::new(ENTITY);

    let Answer::Established { id, .. } = notifier.on_subscribe(&subscribe("dialog", "w1"), NOW)
    else {
        panic!("a new subscription");
    };

    // The watcher sees a call while the resource is there.
    let document = watch.document(&[Dialog {
        id: "d1".to_owned(),
        state: DialogState::Confirmed,
        direction: Direction::Recipient,
    }]);
    assert!(document.contains("<state>confirmed</state>"), "{document}");
    let live = notifier
        .notify_state(&id, NOW)
        .expect("an active subscription is notified");
    assert_eq!(live.state, State::Active);

    // Then the address of record itself goes away.
    let ending = notifier
        .terminate(&id, Reason::NoResource)
        .expect("the terminating state");
    assert_eq!(ending.to_value(), "terminated;reason=noresource");

    // Cleanly: no further notification, and *not* by timing out — the subscription still had
    // most of its ten minutes left, and nothing expired it.
    assert!(notifier.notify_state(&id, NOW).is_none());
    assert_eq!(notifier.active(), 0);
    assert!(
        notifier.expire(NOW + 1).is_empty(),
        "it ended because the resource went, not because the clock ran out"
    );
}

/// The same for `reg`, whose resource going is the case a watcher meets most: the address of
/// record it was watching stops existing, which is not the same as its last contact expiring.
#[test]
fn a_registration_subscription_ends_with_noresource_when_its_resource_disappears() {
    let mut notifier = notifier();
    let mut watch = RegistrationWatch::new(ENTITY);

    let Answer::Established { id, .. } = notifier.on_subscribe(&subscribe("reg", "w1"), NOW) else {
        panic!("a new subscription");
    };

    let bound = watch.document(&[Contact {
        id: "c1".to_owned(),
        uri: "sip:alice@192.0.2.5".to_owned(),
        event: ContactEvent::Registered,
    }]);
    assert!(bound.contains("event=\"registered\""), "{bound}");

    // A contact expiring is reported *in* a document — the subscription is still watching.
    let gone = watch.document(&[Contact {
        id: "c1".to_owned(),
        uri: "sip:alice@192.0.2.5".to_owned(),
        event: ContactEvent::Expired,
    }]);
    assert!(gone.contains("event=\"expired\""), "{gone}");
    assert!(
        notifier.notify_state(&id, NOW).is_some(),
        "an unregistered address of record is still an address of record"
    );

    // The resource disappearing is the different, stronger thing.
    let ending = notifier
        .terminate(&id, Reason::NoResource)
        .expect("the terminating state");
    assert_eq!(ending.to_value(), "terminated;reason=noresource");
    assert!(notifier.notify_state(&id, NOW).is_none());
}

/// The `presence` package registers with the framework and serves PIDF — the package name and the
/// document type together, because a subscriber that asked for `presence` and got something that
/// is not `application/pidf+xml` has no way to read it.
#[test]
fn the_presence_package_registers_with_the_notifier_and_serves_pidf() {
    let mut notifier = notifier();
    assert!(matches!(
        notifier.on_subscribe(&subscribe("presence", "w1"), NOW),
        Answer::Established { .. }
    ));

    let document = Pidf::new(ENTITY)
        .with(Tuple::open("t1").at("sip:alice@192.0.2.5"))
        .to_xml();
    assert!(
        document.contains("xmlns=\"urn:ietf:params:xml:ns:pidf\""),
        "{document}"
    );
    assert_eq!(sipx_ua::presence::PIDF_TYPE, "application/pidf+xml");
}

/// The story's whole chain, with nothing assumed: somebody publishes, and the watcher that
/// subscribed to `presence` is the one that gets the document.
#[test]
fn publishing_to_a_resource_with_a_live_subscription_notifies_the_subscriber() {
    let mut notifier = notifier();
    let mut compositor = Compositor::new(Duration::from_secs(3600));

    let Answer::Established { id, .. } = notifier.on_subscribe(&subscribe("presence", "w1"), NOW)
    else {
        panic!("a new subscription");
    };

    let published = compositor.apply(
        ENTITY,
        Publish::read(
            None,
            Some(
                Pidf::new(ENTITY)
                    .with(Tuple::open("t1").at("sip:alice@192.0.2.5"))
                    .to_xml(),
            ),
            Duration::from_secs(600),
        ),
        NOW,
    );
    let Published::Accepted { tag, .. } = published else {
        panic!("the publication is accepted, got {published:?}");
    };

    // What the NOTIFY for that subscription carries: the state from the framework, the body from
    // the compositor.
    let state = notifier
        .notify_state(&id, NOW)
        .expect("a live subscription is notified");
    assert_eq!(state.state, State::Active);
    assert!(state.to_value().starts_with("active;expires="), "{state:?}");

    let body = compositor
        .document(ENTITY)
        .expect("the document that was published");
    assert!(body.contains("<basic>open</basic>"), "{body}");
    assert!(body.contains("sip:alice@192.0.2.5"), "{body}");

    // A change reaches the same subscriber, which is what makes it a subscription rather than one
    // fetch: publish again and the body the next NOTIFY would carry is the new one.
    let away = Pidf::new(ENTITY)
        .with(Tuple::closed("t1").with_note("in a meeting"))
        .to_xml();
    assert!(matches!(
        compositor.apply(
            ENTITY,
            Publish::read(Some(tag), Some(away), Duration::from_secs(600)),
            NOW + 5
        ),
        Published::Accepted { .. }
    ));
    let body = compositor.document(ENTITY).expect("the replaced document");
    assert!(body.contains("<basic>closed</basic>"), "{body}");
    assert!(body.contains("in a meeting"), "{body}");
    assert!(
        notifier.notify_state(&id, NOW + 5).is_some(),
        "and the subscription is still the one being notified"
    );

    // Once the subscription is over, a further publication reaches nobody through it — the
    // notifier produces no state, so there is no NOTIFY to put the body in.
    notifier
        .terminate(&id, Reason::NoResource)
        .expect("the terminating state");
    assert!(notifier.notify_state(&id, NOW + 6).is_none());
}
