//! Two transaction machines talking through a lossy link, in one process, with no clock.
//!
//! This is what the loopback link is for. Retransmission is the behaviour the transaction machines
//! exist to provide, and testing it over a real socket means waiting 500 milliseconds for Timer A
//! and hoping the packet loss you asked for is the packet loss you got. Here both the link and the
//! timer queue take `now` as an argument, so a lost datagram and the retransmission that recovers
//! from it cost no wall-clock time at all.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    // `caller`/`callee` are the words for these two roles, and renaming them to satisfy a
    // similarity heuristic would make the test harder to read than the heuristic is worth.
    clippy::similar_names
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::transaction::{
    Dispatch, Output, Reliability, Timer, TransactionKey, TransactionLayer, TuEvent,
};
use sipx_sip::{HeaderName, Limits, Method, Request, StatusCode, parse_datagram};
use sipx_testkit::link::{Faults, Link, Side};
use sipx_transport::timers::TimerQueue;
use tokio::time::Instant;

/// One end of the conversation: a transaction layer and the timers it asked for.
struct Stack {
    layer: TransactionLayer,
    timers: TimerQueue<(TransactionKey, Timer)>,
}

impl Stack {
    fn new() -> Self {
        Self {
            layer: TransactionLayer::new(sipx_sip::transaction::Timers::default()),
            timers: TimerQueue::new(),
        }
    }

    /// Perform a transaction's outputs, returning what must go on the wire.
    fn perform(
        &mut self,
        key: &TransactionKey,
        outputs: Vec<Output>,
        now: Instant,
        events: &mut Vec<TuEvent>,
    ) -> Vec<Bytes> {
        let mut wire = Vec::new();
        for output in outputs {
            match output {
                Output::Send(message) => wire.push(message.to_bytes()),
                Output::SetTimer { timer, after } => {
                    self.timers.set((key.clone(), timer), now, after);
                }
                Output::ClearTimer(timer) => self.timers.clear(&(key.clone(), timer)),
                Output::ToTu(event) => events.push(*event),
                Output::Terminated(_) => self.timers.forget_matching(|(k, _)| k == key),
            }
        }
        wire
    }

    /// Fire everything due at `now`, returning what that produced for the wire.
    fn on_timers(&mut self, now: Instant, events: &mut Vec<TuEvent>) -> Vec<Bytes> {
        let mut wire = Vec::new();
        for (key, timer) in self.timers.take_due(now) {
            let outputs = self.layer.on_timer(&key, timer);
            wire.extend(self.perform(&key, outputs, now, events));
        }
        wire
    }

    /// Feed a datagram in.
    fn receive(&mut self, bytes: &Bytes, now: Instant, events: &mut Vec<TuEvent>) -> Vec<Bytes> {
        let Ok(message) = parse_datagram(bytes.clone(), &Limits::datagram()) else {
            return Vec::new();
        };
        match self.layer.receive(message, Reliability::Unreliable) {
            Dispatch::Created { key, outputs } | Dispatch::Matched { key, outputs } => {
                self.perform(&key, outputs, now, events)
            }
            Dispatch::Unmatched(_) => Vec::new(),
        }
    }
}

fn invite() -> Request {
    sipx_sip::build::RequestBuilder::new(
        Method::Invite,
        sipx_sip::Uri::sip(sipx_sip::Host::Name(
            sipx_sip::HostName::new("callee.example").expect("valid"),
        )),
    )
    .header(
        HeaderName::Via,
        Bytes::from_static(b"SIP/2.0/UDP caller.example;branch=z9hG4bK-loopback-1"),
    )
    .expect("via")
    .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))
    .expect("to")
    .header(
        HeaderName::From,
        Bytes::from_static(b"<sip:caller@example.net>;tag=abc"),
    )
    .expect("from")
    .header(HeaderName::CallId, Bytes::from_static(b"loopback@sipx"))
    .expect("call-id")
    .cseq(1, &Method::Invite)
    .expect("cseq")
    .max_forwards(70)
    .build()
}

/// The story's failing-first test.
///
/// A datagram is lost; Timer A fires; the retransmission gets through. That is the whole reason the
/// client transaction has a Timer A, and until now there was no way to assert it without a socket
/// and a real half-second.
#[tokio::test(start_paused = true)]
async fn a_retransmission_gets_through_a_link_that_drops_the_first_datagram() {
    let mut caller = Stack::new();
    let mut callee = Stack::new();
    // Loses everything, until the test turns the loss off. Deterministic in the direction that
    // matters: the *first* datagram is certainly lost.
    let mut link = Link::new(1, Faults::losing(1.0));
    let epoch = Instant::now();
    let mut caller_events = Vec::new();
    let mut callee_events = Vec::new();

    let (key, outputs) = caller
        .layer
        .send_request(invite(), Reliability::Unreliable)
        .expect("a client transaction");
    for bytes in caller.perform(&key, outputs, epoch, &mut caller_events) {
        link.send(Side::Left, bytes, epoch);
    }

    assert_eq!(link.dropped(), 1, "the first INVITE is lost");
    assert!(
        link.take_due(epoch).is_empty(),
        "and nothing arrives at the callee"
    );

    // The link recovers. Timer A is T1 — half a second — and none of it is spent waiting.
    link = Link::perfect();
    let at_t1 = epoch + Duration::from_millis(500);
    let retransmitted = caller.on_timers(at_t1, &mut caller_events);
    assert_eq!(
        retransmitted.len(),
        1,
        "Timer A must produce exactly one retransmission"
    );
    for bytes in retransmitted {
        link.send(Side::Left, bytes, at_t1);
    }

    let arrived = link.take_due(at_t1);
    assert_eq!(arrived.len(), 1, "the retransmission gets through");
    for delivery in arrived {
        assert_eq!(delivery.to, Side::Right);
        let _ = callee.receive(&delivery.bytes, at_t1, &mut callee_events);
    }

    assert!(
        callee_events.iter().any(
            |event| matches!(event, TuEvent::Request(request) if request.method == Method::Invite)
        ),
        "the callee's transaction user should see the INVITE that the retransmission carried"
    );
}

/// The same exchange with no faults, taken to completion: INVITE, 180, 200, ACK. The point is that
/// a whole call's signalling crosses the link with no socket and no sleep, so the fault cases above
/// are measured against something that works.
#[tokio::test(start_paused = true)]
async fn a_full_exchange_crosses_a_perfect_link() {
    let mut caller = Stack::new();
    let mut callee = Stack::new();
    let mut link = Link::perfect();
    let now = Instant::now();
    let mut caller_events = Vec::new();
    let mut callee_events = Vec::new();

    let (key, outputs) = caller
        .layer
        .send_request(invite(), Reliability::Unreliable)
        .expect("a client transaction");
    for bytes in caller.perform(&key, outputs, now, &mut caller_events) {
        link.send(Side::Left, bytes, now);
    }

    // Deliver until the link is quiet, answering the INVITE when it lands.
    for _ in 0..8u32 {
        let arrived = link.take_due(now);
        if arrived.is_empty() {
            break;
        }
        for delivery in arrived {
            let (stack, events) = match delivery.to {
                Side::Right => (&mut callee, &mut callee_events),
                Side::Left => (&mut caller, &mut caller_events),
            };
            let before = events.len();
            for bytes in stack.receive(&delivery.bytes, now, events) {
                link.send(delivery.to, bytes, now);
            }
            if delivery.to == Side::Right && events.len() > before {
                // The INVITE reached the callee's transaction user. Answer it.
                if let Some(key) = &TransactionKey::from_request(&invite()) {
                    let ok = sipx_sip::build::ResponseBuilder::to_request(
                        &invite(),
                        StatusCode::new(200).expect("valid"),
                        "OK",
                    )
                    .expect("builds")
                    .header(
                        HeaderName::Contact,
                        Bytes::from_static(b"<sip:callee@callee.example>"),
                    )
                    .expect("contact")
                    .build();
                    let outputs = callee.layer.send_response(key, ok);
                    for bytes in callee.perform(key, outputs, now, &mut callee_events) {
                        link.send(Side::Right, bytes, now);
                    }
                }
            }
        }
    }

    assert!(
        caller_events.iter().any(|event| matches!(
            event,
            TuEvent::Response(response) if response.status.code() == 200
        )),
        "the caller's transaction user should see the 200: {caller_events:?}"
    );
}

/// Loss on its own is survivable; the transaction machine is what makes it so. Over a link losing
/// half its datagrams, the INVITE still arrives — it just takes a retransmission or two, and none
/// of them costs real time.
#[tokio::test(start_paused = true)]
async fn a_request_survives_a_link_that_loses_half_of_everything() {
    let mut caller = Stack::new();
    let mut callee = Stack::new();
    let mut link = Link::new(11, Faults::losing(0.5));
    let mut now = Instant::now();
    let mut caller_events = Vec::new();
    let mut callee_events = Vec::new();

    let (key, outputs) = caller
        .layer
        .send_request(invite(), Reliability::Unreliable)
        .expect("a client transaction");
    for bytes in caller.perform(&key, outputs, now, &mut caller_events) {
        link.send(Side::Left, bytes, now);
    }

    // Walk the clock forward over Timer A's backoff, delivering whatever survives.
    for _ in 0..8u32 {
        for delivery in link.take_due(now) {
            if delivery.to == Side::Right {
                let _ = callee.receive(&delivery.bytes, now, &mut callee_events);
            }
        }
        if !callee_events.is_empty() {
            break;
        }
        now += Duration::from_millis(500);
        for bytes in caller.on_timers(now, &mut caller_events) {
            link.send(Side::Left, bytes, now);
        }
    }

    assert!(
        !callee_events.is_empty(),
        "over eight retransmissions a link losing half its datagrams should still deliver one; \
         dropped {} so far",
        link.dropped()
    );
    assert!(link.dropped() > 0, "and the link really did lose some");
}
