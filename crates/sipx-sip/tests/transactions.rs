//! Transaction behaviour, driven with no clock and no socket.
//!
//! Every scenario here is one the RFC describes and most implementations discover the hard
//! way, in production, as a flaky integration test. Because the machines are sans-IO, each is
//! an ordinary unit test: feed inputs, assert outputs.
//!
//! Numbered scenarios refer to the table in `docs/specs/sip-transaction.md` §7.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use bytes::Bytes;
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::transaction::{
    ClientState, ClientTransaction, Dispatch, Output, Reliability, ServerState, ServerTransaction,
    Timer, Timers, TransactionKey, TransactionLayer, TuEvent, sent_messages, tu_events,
};
use sipx_sip::{HeaderName, Host, HostName, Message, Method, Request, Response, StatusCode, Uri};

fn uri() -> Uri {
    Uri::sip(Host::Name(
        HostName::new("example.com").expect("valid host"),
    ))
}

fn request(method: &Method, branch: &str) -> Request {
    RequestBuilder::new(method.clone(), uri())
        .header(
            HeaderName::Via,
            Bytes::from(format!("SIP/2.0/UDP h.example.com;branch={branch}")),
        )
        .expect("valid Via")
        .header(HeaderName::From, "<sip:caller@example.net>;tag=abc")
        .expect("valid From")
        .header(HeaderName::To, "<sip:callee@example.com>")
        .expect("valid To")
        .header(HeaderName::CallId, "call-1@example.net")
        .expect("valid Call-ID")
        .cseq(1, method)
        .expect("valid CSeq")
        .max_forwards(70)
        .build()
}

fn response(request: &Request, code: u16, to_tag: Option<&str>) -> Response {
    let mut builder = ResponseBuilder::to_request(
        request,
        StatusCode::new(code).expect("valid code"),
        "Testing",
    )
    .expect("valid response");
    if let Some(tag) = to_tag {
        // A real UAS tags the To header on its first response — replacing it, not adding a
        // second one, which would make the response invalid.
        builder = builder
            .set_header(
                &HeaderName::To,
                Bytes::from(format!("<sip:callee@example.com>;tag={tag}")),
            )
            .expect("valid To");
    }
    builder.build()
}

/// An ACK carrying the branch of the INVITE it acknowledges, which is what makes the two
/// share a transaction key.
fn ack_for(branch: &str) -> Request {
    RequestBuilder::new(Method::Ack, uri())
        .header(
            HeaderName::Via,
            Bytes::from(format!("SIP/2.0/UDP h.example.com;branch={branch}")),
        )
        .expect("valid Via")
        .header(HeaderName::From, "<sip:caller@example.net>;tag=abc")
        .expect("valid From")
        .header(HeaderName::To, "<sip:callee@example.com>;tag=uas")
        .expect("valid To")
        .header(HeaderName::CallId, "call-1@example.net")
        .expect("valid Call-ID")
        .cseq(1, &Method::Ack)
        .expect("valid CSeq")
        .max_forwards(70)
        .build()
}

fn timers() -> Timers {
    Timers::default()
}

fn set_timers(outputs: &[Output]) -> Vec<(Timer, Duration)> {
    outputs
        .iter()
        .filter_map(|o| match o {
            Output::SetTimer { timer, after } => Some((*timer, *after)),
            _ => None,
        })
        .collect()
}

fn cleared_timers(outputs: &[Output]) -> Vec<Timer> {
    outputs
        .iter()
        .filter_map(|o| match o {
            Output::ClearTimer(t) => Some(*t),
            _ => None,
        })
        .collect()
}

fn sent_requests(outputs: &[Output]) -> Vec<&Request> {
    sent_messages(outputs)
        .into_iter()
        .filter_map(Message::as_request)
        .collect()
}

fn sent_responses(outputs: &[Output]) -> Vec<&Response> {
    sent_messages(outputs)
        .into_iter()
        .filter_map(Message::as_response)
        .collect()
}

// ---------------------------------------------------------------------------------------
// INVITE client transaction — spec §4.1
// ---------------------------------------------------------------------------------------

/// T1: with no answer, the request goes out at 0, T1, 2·T1, 4·T1 … and the transaction gives
/// up at 64·T1. Doubling without a ceiling is what distinguishes Timer A from Timer E.
#[test]
fn invite_client_retransmits_with_doubling_intervals_then_times_out() {
    let (mut tx, out) = ClientTransaction::new(
        request(&Method::Invite, "z9hG4bK1"),
        Reliability::Unreliable,
        timers(),
    );
    assert_eq!(sent_requests(&out).len(), 1, "the request goes out at once");
    assert_eq!(
        set_timers(&out),
        vec![
            (Timer::A, Duration::from_millis(500)),
            (Timer::B, Duration::from_secs(32)),
        ]
    );
    assert_eq!(tx.state(), ClientState::Calling);

    for expected in [1000u64, 2000, 4000, 8000] {
        let out = tx.on_timer(Timer::A);
        assert_eq!(sent_requests(&out).len(), 1, "retransmission");
        assert_eq!(
            set_timers(&out),
            vec![(Timer::A, Duration::from_millis(expected))],
            "Timer A doubles without a ceiling"
        );
    }

    let out = tx.on_timer(Timer::B);
    assert!(matches!(tu_events(&out).as_slice(), [TuEvent::Timeout]));
    assert_eq!(tx.state(), ClientState::Terminated);
}

/// T2: a non-2xx final response is acknowledged **by the transaction**, reusing the request's
/// branch so the far end matches the ACK to the INVITE it acknowledges.
#[test]
fn invite_client_acks_a_non_2xx_itself_reusing_the_branch() {
    let req = request(&Method::Invite, "z9hG4bK1");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());

    let out = tx.on_response(response(&req, 486, Some("uas-tag")));
    let acks = sent_requests(&out);
    assert_eq!(acks.len(), 1, "exactly one ACK");
    assert_eq!(acks[0].method, Method::Ack);

    let via = acks[0].headers.value(&HeaderName::Via).expect("a Via");
    assert!(
        String::from_utf8_lossy(&via).contains("branch=z9hG4bK1"),
        "the ACK must reuse the INVITE's branch"
    );
    let to = acks[0].headers.value(&HeaderName::To).expect("a To");
    assert!(
        String::from_utf8_lossy(&to).contains("tag=uas-tag"),
        "the To tag from the response must be echoed, or the far end cannot match the ACK"
    );
    let cseq = acks[0].headers.value(&HeaderName::CSeq).expect("a CSeq");
    assert_eq!(cseq.as_ref(), b"1 ACK", "same sequence number, method ACK");

    assert_eq!(tx.state(), ClientState::Completed);
    assert_eq!(set_timers(&out), vec![(Timer::D, Duration::from_secs(32))]);
}

/// T3: the other half of the same rule, and the one that breaks calls when it is wrong. A 2xx
/// is **not** acknowledged here: that ACK is a separate transaction the TU builds, because
/// only the TU knows the dialog's route set.
#[test]
fn invite_client_tx_acks_non_2xx_only() {
    let req = request(&Method::Invite, "z9hG4bK1");

    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let out = tx.on_response(response(&req, 404, Some("t")));
    assert_eq!(
        sent_requests(&out).len(),
        1,
        "a 404 is acknowledged by the transaction"
    );

    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let out = tx.on_response(response(&req, 200, Some("t")));
    assert_eq!(
        sent_requests(&out).len(),
        0,
        "a 200 must NOT be acknowledged by the transaction"
    );
    assert_eq!(tx.state(), ClientState::Accepted, "RFC 6026");
}

/// T4: RFC 6026. A forking proxy answers twice; the TU must hear about both, and the
/// transaction must still exist for the second.
#[test]
fn invite_client_delivers_every_2xx_from_a_fork() {
    let req = request(&Method::Invite, "z9hG4bK1");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());

    let first = tx.on_response(response(&req, 200, Some("branch-a")));
    assert_eq!(tu_events(&first).len(), 1);
    assert_eq!(tx.state(), ClientState::Accepted);
    assert_eq!(
        set_timers(&first),
        vec![(Timer::M, Duration::from_secs(32))]
    );

    let second = tx.on_response(response(&req, 200, Some("branch-b")));
    assert_eq!(
        tu_events(&second).len(),
        1,
        "the second fork's answer must reach the TU too"
    );
    assert_eq!(tx.state(), ClientState::Accepted);

    let out = tx.on_timer(Timer::M);
    assert_eq!(tx.state(), ClientState::Terminated);
    assert!(matches!(out.as_slice(), [Output::Terminated(_)]));
}

/// A retransmitted final response gets the same ACK again — and the TU is not told twice,
/// because it has already dealt with that response.
#[test]
fn invite_client_reacks_a_retransmitted_final_without_telling_the_tu() {
    let req = request(&Method::Invite, "z9hG4bK1");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let _ = tx.on_response(response(&req, 486, Some("t")));

    let again = tx.on_response(response(&req, 486, Some("t")));
    assert_eq!(sent_requests(&again).len(), 1, "the ACK is repeated");
    assert!(tu_events(&again).is_empty(), "the TU is not told twice");
}

#[test]
fn invite_client_stops_retransmitting_when_a_provisional_arrives() {
    let req = request(&Method::Invite, "z9hG4bK1");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());

    let out = tx.on_response(response(&req, 180, None));
    assert_eq!(tx.state(), ClientState::Proceeding);
    assert_eq!(cleared_timers(&out), vec![Timer::A]);
    assert!(
        !cleared_timers(&out).contains(&Timer::B),
        "Timer B stays: alive is not the same as answered"
    );
}

// ---------------------------------------------------------------------------------------
// Non-INVITE client transaction — spec §4.2
// ---------------------------------------------------------------------------------------

/// Timer E doubles but stops at T2 — the difference from Timer A, and the reason a
/// non-INVITE transaction does not back off into uselessness.
#[test]
fn non_invite_client_backoff_is_capped_at_t2() {
    let (mut tx, out) = ClientTransaction::new(
        request(&Method::Options, "z9hG4bK2"),
        Reliability::Unreliable,
        timers(),
    );
    assert_eq!(tx.state(), ClientState::Trying);
    assert_eq!(
        set_timers(&out),
        vec![
            (Timer::E, Duration::from_millis(500)),
            (Timer::F, Duration::from_secs(32)),
        ]
    );

    for expected in [1000u64, 2000, 4000, 4000, 4000] {
        let out = tx.on_timer(Timer::E);
        assert_eq!(
            set_timers(&out),
            vec![(Timer::E, Duration::from_millis(expected))],
            "Timer E must not exceed T2"
        );
    }
}

/// Timer F must terminate the non-INVITE machine from `Trying`, which is where it waits.
/// Handling the timeout only from `Calling` — the INVITE machine's waiting state — leaves a
/// transaction to nowhere hanging forever.
#[test]
fn non_invite_client_times_out_from_trying() {
    let (mut tx, _) = ClientTransaction::new(
        request(&Method::Options, "z9hG4bK2"),
        Reliability::Unreliable,
        timers(),
    );
    assert_eq!(tx.state(), ClientState::Trying);

    let out = tx.on_timer(Timer::F);
    assert!(matches!(tu_events(&out).as_slice(), [TuEvent::Timeout]));
    assert_eq!(tx.state(), ClientState::Terminated);
}

/// And from `Proceeding`, where the far end has answered provisionally and then gone quiet.
#[test]
fn a_client_transaction_times_out_from_proceeding_too() {
    let req = request(&Method::Options, "z9hG4bK2");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let _ = tx.on_response(response(&req, 100, None));
    assert_eq!(tx.state(), ClientState::Proceeding);

    let out = tx.on_timer(Timer::F);
    assert!(matches!(tu_events(&out).as_slice(), [TuEvent::Timeout]));
    assert_eq!(tx.state(), ClientState::Terminated);

    let req = request(&Method::Invite, "z9hG4bK3");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let _ = tx.on_response(response(&req, 180, None));
    assert_eq!(tx.state(), ClientState::Proceeding);
    let out = tx.on_timer(Timer::B);
    assert!(matches!(tu_events(&out).as_slice(), [TuEvent::Timeout]));
    assert_eq!(tx.state(), ClientState::Terminated);
}

#[test]
fn non_invite_client_completes_on_a_final_response() {
    let req = request(&Method::Options, "z9hG4bK2");
    let (mut tx, _) = ClientTransaction::new(req.clone(), Reliability::Unreliable, timers());

    let out = tx.on_response(response(&req, 200, None));
    assert_eq!(tx.state(), ClientState::Completed);
    assert_eq!(cleared_timers(&out), vec![Timer::E, Timer::F]);
    assert_eq!(set_timers(&out), vec![(Timer::K, Duration::from_secs(5))]);
    assert_eq!(tu_events(&out).len(), 1);

    let again = tx.on_response(response(&req, 200, None));
    assert!(
        again.is_empty(),
        "a retransmitted response in Completed is absorbed entirely"
    );
}

// ---------------------------------------------------------------------------------------
// Server transactions — spec §4.3, §4.4
// ---------------------------------------------------------------------------------------

/// T5: the load-bearing behaviour of the whole layer. A UDP peer that misses one response
/// resends the request every T1; if each copy reached the application, a REGISTER would be
/// processed seven times.
#[test]
fn server_tx_absorbs_request_retransmission_without_second_delivery() {
    let req = request(&Method::Register, "z9hG4bK3");
    let (mut tx, out) = ServerTransaction::new(req.clone(), Reliability::Unreliable, timers());
    assert_eq!(tu_events(&out).len(), 1, "the TU sees the request once");
    assert_eq!(tx.state(), ServerState::Trying);

    // Before any response there is nothing to resend, and still nothing to tell the TU.
    let again = tx.on_request(&req);
    assert!(tu_events(&again).is_empty(), "the TU must not see it twice");
    assert!(sent_messages(&again).is_empty());

    // After a final response, a retransmission gets that response back — and the TU still
    // hears nothing.
    let out = tx.on_tu_response(response(&req, 200, Some("uas")));
    assert_eq!(tx.state(), ServerState::Completed);
    assert_eq!(set_timers(&out), vec![(Timer::J, Duration::from_secs(32))]);

    let again = tx.on_request(&req);
    assert_eq!(sent_responses(&again).len(), 1, "the response is resent");
    assert!(tu_events(&again).is_empty(), "the TU still hears nothing");
}

/// T7 and T8: the transaction answers 100 Trying on the TU's behalf only if the TU has not
/// answered within 200 ms.
#[test]
fn invite_server_sends_100_trying_only_when_the_tu_is_slow() {
    let req = request(&Method::Invite, "z9hG4bK4");

    let (mut tx, out) = ServerTransaction::new(req.clone(), Reliability::Unreliable, timers());
    assert_eq!(
        set_timers(&out),
        vec![(Timer::Trying100, Duration::from_millis(200))]
    );
    let fired = tx.on_timer(Timer::Trying100);
    let sent = sent_responses(&fired);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].status.code(), 100);

    // If the TU answers first, the timer is cleared and no 100 is generated.
    let (mut tx, _) = ServerTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let out = tx.on_tu_response(response(&req, 180, Some("uas")));
    assert_eq!(cleared_timers(&out), vec![Timer::Trying100]);
    let fired = tx.on_timer(Timer::Trying100);
    assert!(
        sent_responses(&fired).is_empty(),
        "no 100 once the TU has answered"
    );
}

/// T11: an ACK for a non-2xx is part of the INVITE transaction and stops there.
#[test]
fn invite_server_absorbs_the_ack_for_a_non_2xx() {
    let req = request(&Method::Invite, "z9hG4bK5");
    let (mut tx, _) = ServerTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let out = tx.on_tu_response(response(&req, 486, Some("uas")));
    assert_eq!(tx.state(), ServerState::Completed);
    assert_eq!(
        set_timers(&out),
        vec![
            (Timer::G, Duration::from_millis(500)),
            (Timer::H, Duration::from_secs(32)),
        ]
    );

    let ack = ack_for("z9hG4bK5");
    let out = tx.on_request(&ack);
    assert_eq!(tx.state(), ServerState::Confirmed);
    assert!(
        tu_events(&out).is_empty(),
        "the ACK for a non-2xx belongs to the transaction, not the TU"
    );
    assert_eq!(cleared_timers(&out), vec![Timer::G, Timer::H]);
    assert_eq!(set_timers(&out), vec![(Timer::I, Duration::from_secs(5))]);

    let out = tx.on_timer(Timer::I);
    assert_eq!(tx.state(), ServerState::Terminated);
    assert!(matches!(out.as_slice(), [Output::Terminated(_)]));
}

/// T12: an ACK for a 2xx is a *different* transaction, so it goes to the TU. RFC 6026 keeps
/// this transaction alive on Timer L only so a retransmitted 2xx does not create a second one.
#[test]
fn invite_server_hands_the_ack_for_a_2xx_to_the_tu() {
    let req = request(&Method::Invite, "z9hG4bK6");
    let (mut tx, _) = ServerTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let out = tx.on_tu_response(response(&req, 200, Some("uas")));
    assert_eq!(tx.state(), ServerState::Accepted);
    assert_eq!(set_timers(&out), vec![(Timer::L, Duration::from_secs(32))]);

    let ack = ack_for("z9hG4bK6");
    let out = tx.on_request(&ack);
    assert!(
        matches!(tu_events(&out).as_slice(), [TuEvent::Ack(_)]),
        "the ACK for a 2xx must reach the TU"
    );
    assert_eq!(tx.state(), ServerState::Accepted);
}

/// Timer G retransmits the final response until the ACK arrives, backing off to T2.
#[test]
fn invite_server_retransmits_its_final_response_until_acked() {
    let req = request(&Method::Invite, "z9hG4bK7");
    let (mut tx, _) = ServerTransaction::new(req.clone(), Reliability::Unreliable, timers());
    let _ = tx.on_tu_response(response(&req, 500, Some("uas")));

    for expected in [1000u64, 2000, 4000, 4000] {
        let out = tx.on_timer(Timer::G);
        assert_eq!(sent_responses(&out).len(), 1);
        assert_eq!(
            set_timers(&out),
            vec![(Timer::G, Duration::from_millis(expected))]
        );
    }

    let out = tx.on_timer(Timer::H);
    assert!(matches!(tu_events(&out).as_slice(), [TuEvent::Timeout]));
    assert_eq!(tx.state(), ServerState::Terminated);
}

/// T9: on a reliable transport there is nothing to retransmit, and the absorption timers fire
/// immediately because there is nothing left in flight to absorb.
#[test]
fn reliable_transports_set_no_retransmission_timers() {
    let req = request(&Method::Invite, "z9hG4bK8");
    let (_, out) = ClientTransaction::new(req.clone(), Reliability::Reliable, timers());
    let set: Vec<Timer> = set_timers(&out).into_iter().map(|(t, _)| t).collect();
    assert_eq!(set, vec![Timer::B], "no Timer A on a reliable transport");

    let (mut tx, _) = ServerTransaction::new(req.clone(), Reliability::Reliable, timers());
    let out = tx.on_tu_response(response(&req, 486, Some("uas")));
    let set = set_timers(&out);
    assert_eq!(
        set,
        vec![(Timer::H, Duration::from_secs(32))],
        "no Timer G on a reliable transport"
    );

    let (mut tx, _) = ServerTransaction::new(
        request(&Method::Options, "z9hG4bK9"),
        Reliability::Reliable,
        timers(),
    );
    let out = tx.on_tu_response(response(&request(&Method::Options, "z9hG4bK9"), 200, None));
    assert_eq!(
        set_timers(&out),
        vec![(Timer::J, Duration::ZERO)],
        "Timer J fires immediately when nothing can be in flight"
    );
}

// ---------------------------------------------------------------------------------------
// The transaction layer — spec §6
// ---------------------------------------------------------------------------------------

#[test]
fn a_response_reaches_the_client_transaction_that_sent_the_request() {
    let mut layer = TransactionLayer::new(timers());
    let req = request(&Method::Options, "z9hG4bKa");
    let (key, _) = layer
        .send_request(req.clone(), Reliability::Unreliable)
        .expect("a key");
    assert_eq!(layer.len(), (1, 0));

    let dispatch = layer.receive(
        Message::Response(response(&req, 200, None)),
        Reliability::Unreliable,
    );
    match dispatch {
        Dispatch::Matched {
            key: matched,
            outputs,
        } => {
            assert_eq!(matched, key);
            assert_eq!(tu_events(&outputs).len(), 1);
        }
        other => panic!("expected a match, got {other:?}"),
    }
}

#[test]
fn an_unmatched_response_goes_up_rather_than_being_dropped() {
    let mut layer = TransactionLayer::new(timers());
    let stray = response(&request(&Method::Options, "z9hG4bKzz"), 200, None);
    assert!(matches!(
        layer.receive(Message::Response(stray), Reliability::Unreliable),
        Dispatch::Unmatched(_)
    ));
}

/// T13: senders that predate RFC 3261 are still on the internet, and the corpus has one. The
/// absence of the magic cookie selects the fallback rather than causing a rejection.
#[test]
fn legacy_branch_matching_rfc2543_fallback() {
    let mut layer = TransactionLayer::new(timers());
    // No z9hG4bK prefix: this sender is from before branch meant anything.
    let req = request(&Method::Options, "oldschoolbranch");

    let key = TransactionKey::from_request(&req).expect("a key");
    assert!(key.is_legacy(), "the fallback rules must be selected");

    let Dispatch::Created { key: created, .. } =
        layer.receive(Message::Request(req.clone()), Reliability::Unreliable)
    else {
        panic!("expected a new server transaction");
    };
    assert_eq!(created, key);

    // The retransmission matches the same transaction rather than creating a second one.
    let dispatch = layer.receive(Message::Request(req), Reliability::Unreliable);
    assert!(matches!(dispatch, Dispatch::Matched { .. }));
    assert_eq!(layer.len(), (0, 1), "still exactly one server transaction");
}

/// T14: a CANCEL matches the transaction of the request it cancels, not one of its own.
#[test]
fn cancel_matches_the_invite_it_cancels() {
    let invite = request(&Method::Invite, "z9hG4bKc");
    let cancel = request(&Method::Cancel, "z9hG4bKc");

    let invite_key = TransactionKey::from_request(&invite).expect("a key");
    let cancel_key = TransactionKey::from_request(&cancel).expect("a key");
    assert_eq!(
        invite_key, cancel_key,
        "a CANCEL carries the branch of the request it cancels"
    );
}

/// An ACK matches the INVITE it acknowledges — the same folding as CANCEL, for the same
/// reason.
#[test]
fn ack_matches_the_invite_it_acknowledges() {
    let invite = request(&Method::Invite, "z9hG4bKd");
    let ack = ack_for("z9hG4bKd");
    assert_eq!(
        TransactionKey::from_request(&invite),
        TransactionKey::from_request(&ack)
    );
}

/// T10: a transaction store that leaks is a slow, quiet outage.
#[test]
fn terminated_transactions_leave_no_trace() {
    let mut layer = TransactionLayer::new(timers());

    for i in 0..10_000u32 {
        let req = request(&Method::Options, &format!("z9hG4bK{i}"));
        let (key, _) = layer
            .send_request(req.clone(), Reliability::Unreliable)
            .expect("a key");
        let _ = layer.receive(
            Message::Response(response(&req, 200, None)),
            Reliability::Unreliable,
        );
        // Completed, waiting out Timer K; the timer firing is what retires it.
        let _ = layer.on_timer(&key, Timer::K);
    }

    assert!(
        layer.is_empty(),
        "10 000 completed transactions left {:?} behind",
        layer.len()
    );
}

#[test]
fn a_transport_error_terminates_and_tells_the_tu() {
    let mut layer = TransactionLayer::new(timers());
    let req = request(&Method::Options, "z9hG4bKe");
    let (key, _) = layer
        .send_request(req, Reliability::Unreliable)
        .expect("a key");

    let out = layer.on_transport_error(&key);
    assert!(matches!(
        tu_events(&out).as_slice(),
        [TuEvent::TransportError]
    ));
    assert!(layer.is_empty());
}
