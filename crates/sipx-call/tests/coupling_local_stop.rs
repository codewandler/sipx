//! Owned local-stop vectors in docs/specs/call-coupling.md.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
use bytes::Bytes;
use sipx_call::coupling::{CouplingControl, LegTermination};
use sipx_call::{
    Calls, DialOptions, Dispatched, Dispatcher, EarlyCoupling, MediaAddress, MediaPolicy,
};
use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::{HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use std::{net::IpAddr, time::Duration};
use tokio::sync::mpsc;

const LIMIT: Duration = Duration::from_secs(3);
fn ip() -> IpAddr {
    "127.0.0.1".parse().unwrap()
}
fn uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("fixture.invalid").unwrap()))
}
fn sdp() -> &'static str {
    "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 45000 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\n"
}
async fn endpoint() -> (Handle, mpsc::Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .unwrap()
}
async fn next(rx: &mut mpsc::Receiver<Incoming>) -> Incoming {
    tokio::time::timeout(LIMIT, rx.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn response(endpoint: &Handle, request: &Incoming, code: u16, body: bool) {
    let original = request.request.headers.value(&HeaderName::To).unwrap();
    let original = String::from_utf8_lossy(original.as_ref());
    let tagged = if original.contains(";tag=") {
        original.into_owned()
    } else {
        format!("{original};tag=target")
    };
    let mut b =
        ResponseBuilder::to_request(&request.request, StatusCode::new(code).unwrap(), "Fixture")
            .unwrap()
            .set_header(&HeaderName::To, Bytes::from(tagged))
            .unwrap()
            .header(
                HeaderName::Contact,
                Bytes::from(format!("<sip:target@{}>", endpoint.local_addr())),
            )
            .unwrap();
    if body {
        b = b
            .header(
                HeaderName::ContentType,
                Bytes::from_static(b"application/sdp"),
            )
            .unwrap()
            .body(Bytes::from_static(sdp().as_bytes()));
    }
    endpoint.respond(&request.key, b.build()).await.unwrap();
}
struct Harness {
    source: Handle,
    edge: Handle,
    target: Handle,
    target_rx: mpsc::Receiver<Incoming>,
    source_request: sipx_sip::Request,
    source_responses: sipx_transport::Responses,
    source_rx: mpsc::Receiver<Incoming>,
    invitation: Option<sipx_call::Invitation>,
    calls: Calls,
    pump: tokio::task::JoinHandle<()>,
}
impl Harness {
    async fn new(offerless: bool) -> Self {
        let (source, source_rx) = endpoint().await;
        let (edge, edge_rx) = endpoint().await;
        let (target, target_rx) = endpoint().await;
        let mut dispatcher = Dispatcher::new(edge.clone(), edge_rx);
        let calls = dispatcher.calls();
        let (tx, mut invitations) = mpsc::channel(2);
        let pump = tokio::spawn(async move {
            while let Some(event) = dispatcher.next().await {
                if let Dispatched::Invitation(i) = event
                    && tx.send(i).await.is_err()
                {
                    break;
                }
            }
        });
        let mut request = RequestBuilder::new(Method::Invite, uri())
            .header(
                HeaderName::Via,
                Bytes::from(format!(
                    "SIP/2.0/UDP {};rport;branch={}",
                    source.local_addr(),
                    sipx_transport::new_branch()
                )),
            )
            .unwrap()
            .header(
                HeaderName::From,
                Bytes::from_static(b"<sip:source@fixture.invalid>;tag=source"),
            )
            .unwrap()
            .header(
                HeaderName::To,
                Bytes::from_static(b"<sip:edge@fixture.invalid>"),
            )
            .unwrap()
            .header(
                HeaderName::CallId,
                Bytes::from_static(b"local-stop-fixture"),
            )
            .unwrap()
            .header(
                HeaderName::Contact,
                Bytes::from(format!("<sip:source@{}>", source.local_addr())),
            )
            .unwrap()
            .cseq(1, &Method::Invite)
            .unwrap()
            .max_forwards(70);
        if !offerless {
            request = request
                .header(
                    HeaderName::ContentType,
                    Bytes::from_static(b"application/sdp"),
                )
                .unwrap()
                .body(Bytes::from_static(sdp().as_bytes()));
        }
        let source_request = request.build();
        let source_responses = source
            .send(source_request.clone(), Target::udp(edge.local_addr()))
            .await
            .unwrap();
        let invitation = tokio::time::timeout(LIMIT, invitations.recv())
            .await
            .unwrap();
        Self {
            source,
            edge,
            target,
            target_rx,
            source_request,
            source_responses,
            source_rx,
            invitation,
            calls,
            pump,
        }
    }
    fn drive(
        &mut self,
        control: CouplingControl,
        cleanup: Duration,
    ) -> tokio::task::JoinHandle<sipx_call::coupling::CouplingTermination> {
        let invitation = self.invitation.take().unwrap();
        let calls = self.calls.clone();
        let edge = self.edge.clone();
        let target = Target::udp(self.target.local_addr());
        tokio::spawn(async move {
            let options = DialOptions::new("<sip:edge@fixture.invalid>", ip())
                .with_cancellation_timeout(cleanup);
            EarlyCoupling::dial_supervised(
                invitation,
                &calls,
                &edge,
                target,
                &uri(),
                &options,
                MediaAddress::new(ip()),
                MediaPolicy::default(),
                &control,
            )
            .await
        })
    }
    async fn close(self) {
        self.edge.shutdown().await;
        self.source.shutdown().await;
        self.target.shutdown().await;
        self.pump.await.unwrap();
    }
}

#[tokio::test]
async fn stop_before_owner_poll_prevents_target_invite() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(200));
    control.request_stop();
    let owner = h.drive(control.clone(), Duration::from_millis(200));
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(report.observed(), "{report:?}");
    assert!(matches!(report.two, LegTermination::NotStarted));
    assert_eq!(
        h.source_responses
            .final_response()
            .await
            .unwrap()
            .status
            .code(),
        487
    );
    // Fixed duration proves silence after the owner's completed no-INVITE decision.
    assert!(
        tokio::time::timeout(Duration::from_millis(30), h.target_rx.recv())
            .await
            .is_err()
    );
    h.close().await;
}

#[tokio::test]
async fn local_stop_before_provisional_cancels_both_initial_shapes() {
    for offerless in [false, true] {
        let mut h = Harness::new(offerless).await;
        let control = CouplingControl::new(Duration::from_millis(300));
        let owner = h.drive(control.clone(), Duration::from_millis(300));
        let invite = next(&mut h.target_rx).await;
        assert_eq!(invite.request.method, Method::Invite);
        control.request_stop();
        control.request_stop();
        // Fixed duration proves RFC3261 silence before the exact INVITE gets a provisional.
        assert!(
            tokio::time::timeout(Duration::from_millis(20), h.target_rx.recv())
                .await
                .is_err()
        );
        response(&h.target, &invite, 180, false).await;
        let cancel = next(&mut h.target_rx).await;
        assert_eq!(cancel.request.method, Method::Cancel);
        response(&h.target, &cancel, 200, false).await;
        response(&h.target, &invite, 487, false).await;
        let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
        assert!(report.observed(), "{report:?}");
        assert_eq!(
            h.source_responses
                .final_response()
                .await
                .unwrap()
                .status
                .code(),
            487
        );
        assert!(control.stop(LIMIT).await.unwrap().observed());
        h.close().await;
    }
}

#[tokio::test]
async fn late_200_requires_bye_acknowledgement_and_dropped_waiter_keeps_owner() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(500));
    let owner = h.drive(control.clone(), Duration::from_millis(500));
    let invite = next(&mut h.target_rx).await;
    response(&h.target, &invite, 180, false).await;
    // The source provisional is a barrier: the early coupling has returned from initial dialing.
    tokio::time::timeout(LIMIT, async {
        loop {
            match h.source_responses.next().await {
                Some(sipx_sip::transaction::TuEvent::Response(r)) if r.status.code() == 180 => {
                    break;
                }
                Some(_) => {}
                None => panic!("source transaction closed before provisional"),
            }
        }
    })
    .await
    .unwrap();
    let mut waiter = Box::pin(control.stop(LIMIT));
    assert!(futures_util::poll!(waiter.as_mut()).is_pending());
    drop(waiter);
    let cancel = next(&mut h.target_rx).await;
    assert_eq!(cancel.request.method, Method::Cancel);
    response(&h.target, &cancel, 200, false).await;
    response(&h.target, &invite, 200, true).await;
    let ack = next(&mut h.target_rx).await;
    assert_eq!(ack.request.method, Method::Ack);
    let bye = next(&mut h.target_rx).await;
    assert_eq!(bye.request.method, Method::Bye);
    assert!(
        control.wait(Duration::from_millis(20)).await.is_none(),
        "sending BYE is not observed termination"
    );
    response(&h.target, &bye, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(report.observed(), "{report:?}");
    assert!(control.stop(LIMIT).await.unwrap().observed());
    h.close().await;
}

#[tokio::test]
async fn late_200_missing_bye_acknowledgement_remains_unknown() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(100));
    let owner = h.drive(control.clone(), Duration::from_millis(100));
    let invite = next(&mut h.target_rx).await;
    control.request_stop();
    response(&h.target, &invite, 180, false).await;
    let cancel = next(&mut h.target_rx).await;
    assert_eq!(cancel.request.method, Method::Cancel);
    response(&h.target, &cancel, 200, false).await;
    response(&h.target, &invite, 200, true).await;
    assert_eq!(next(&mut h.target_rx).await.request.method, Method::Ack);
    assert_eq!(next(&mut h.target_rx).await.request.method, Method::Bye);
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(!report.observed(), "{report:?}");
    assert!(matches!(report.two, LegTermination::Unknown));
    h.close().await;
}

fn dialog_request(
    endpoint: &Handle,
    dialog: &sipx_call::Dialog,
    method: &Method,
    cseq: u32,
) -> sipx_sip::Request {
    let (from, to) = dialog.local_and_remote();
    RequestBuilder::new(method.clone(), dialog.remote_target.clone())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP {};rport;branch={}",
                endpoint.local_addr(),
                sipx_transport::new_branch()
            )),
        )
        .unwrap()
        .header(HeaderName::From, Bytes::from(from))
        .unwrap()
        .header(HeaderName::To, Bytes::from(to))
        .unwrap()
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))
        .unwrap()
        .cseq(cseq, method)
        .unwrap()
        .max_forwards(70)
        .build()
}
async fn confirmed(h: &mut Harness, control: &CouplingControl) -> sipx_call::Dialog {
    let invite = next(&mut h.target_rx).await;
    response(&h.target, &invite, 200, true).await;
    let answer = tokio::time::timeout(LIMIT, h.source_responses.final_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.status.code(), 200);
    let dialog = sipx_call::Dialog::from_response(&h.source_request, &answer).unwrap();
    h.source
        .send_directly(
            dialog_request(&h.source, &dialog, &Method::Ack, 1),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    assert!(control.wait_confirmed(LIMIT).await);
    dialog
}
async fn next_bye(rx: &mut mpsc::Receiver<Incoming>) -> Incoming {
    tokio::time::timeout(LIMIT, async {
        loop {
            let message = rx.recv().await.unwrap();
            if message.request.method == Method::Bye {
                return message;
            }
            assert_eq!(message.request.method, Method::Ack);
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn confirmed_local_stop_waits_for_both_legs_and_is_repeatable() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(500));
    let owner = h.drive(control.clone(), Duration::from_millis(500));
    confirmed(&mut h, &control).await;
    control.request_stop();
    control.request_stop();
    let (one, two) = tokio::join!(next_bye(&mut h.source_rx), next_bye(&mut h.target_rx));
    response(&h.source, &one, 200, false).await;
    assert!(
        control.wait(Duration::from_millis(20)).await.is_none(),
        "one acknowledged leg is insufficient"
    );
    response(&h.target, &two, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(report.observed(), "{report:?}");
    assert!(matches!(report.one, LegTermination::Bye(200)));
    assert!(matches!(report.two, LegTermination::Bye(200)));
    assert!(control.stop(LIMIT).await.unwrap().observed());
    h.close().await;
}

#[tokio::test]
async fn confirmed_missing_or_rejected_bye_is_unknown() {
    for status in [None, Some(503)] {
        let mut h = Harness::new(false).await;
        let control = CouplingControl::new(Duration::from_millis(80));
        let owner = h.drive(control.clone(), Duration::from_millis(80));
        confirmed(&mut h, &control).await;
        control.request_stop();
        let (one, two) = tokio::join!(next_bye(&mut h.source_rx), next_bye(&mut h.target_rx));
        response(&h.source, &one, 200, false).await;
        if let Some(status) = status {
            response(&h.target, &two, status, false).await;
        }
        let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
        assert!(!report.observed(), "{report:?}");
        assert!(matches!(report.two, LegTermination::Unknown));
        h.close().await;
    }
}

#[tokio::test]
async fn crossed_peer_bye_and_local_stop_keep_both_exchanges_owned() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(500));
    let owner = h.drive(control.clone(), Duration::from_millis(500));
    let dialog = confirmed(&mut h, &control).await;
    control.request_stop();
    let source_bye = next_bye(&mut h.source_rx).await;
    // Receipt of the local BYE is the barrier. Now a peer BYE deliberately crosses its response.
    let mut peer_bye = h
        .source
        .send(
            dialog_request(&h.source, &dialog, &Method::Bye, 2),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    let accepted = tokio::time::timeout(LIMIT, peer_bye.final_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.status.code(), 200);
    response(&h.source, &source_bye, 200, false).await;
    let target_bye = next_bye(&mut h.target_rx).await;
    response(&h.target, &target_bye, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(report.observed(), "{report:?}");
    h.close().await;
}

async fn accepted_leg(h: &mut Harness) -> (sipx_call::Call, mpsc::Receiver<Incoming>) {
    let invitation = h.invitation.take().unwrap();
    let call = invitation.answer(&h.edge, ip()).await.unwrap();
    let (_, incoming) = invitation.into_parts();
    let answer = tokio::time::timeout(LIMIT, h.source_responses.final_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.status.code(), 200);
    let dialog = sipx_call::Dialog::from_response(&h.source_request, &answer).unwrap();
    h.source
        .send_directly(
            dialog_request(&h.source, &dialog, &Method::Ack, 1),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    (call, incoming)
}

#[tokio::test]
async fn consuming_confirmed_owner_joins_media_shutdown_racing_stop() {
    let mut a = Harness::new(false).await;
    let mut b = Harness::new(false).await;
    let (one, one_rx) = accepted_leg(&mut a).await;
    let (two, two_rx) = accepted_leg(&mut b).await;
    let media_one = one.media_handle();
    let media_two = two.media_handle();
    let mut coupling = sipx_call::Coupling::new(one, two);
    coupling.bridge_media();
    let control = CouplingControl::new(Duration::from_millis(500));
    let driver_control = control.clone();
    let owner = tokio::spawn(async move {
        coupling
            .run_supervised(one_rx, two_rx, &driver_control)
            .await
    });
    assert!(control.wait_confirmed(LIMIT).await);
    // Real media-worker shutdown races local stop while the registered owner remains driven.
    let ((), ()) = tokio::join!(media_one.shutdown(), async {
        control.request_stop();
    });
    let (bye_one, bye_two) = tokio::join!(next_bye(&mut a.source_rx), next_bye(&mut b.source_rx));
    response(&a.source, &bye_one, 200, false).await;
    response(&b.source, &bye_two, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(report.observed(), "{report:?}");
    assert!(media_one.is_stopped());
    assert!(media_two.is_stopped());
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn ready_native_stop_cannot_originate_an_invite() {
    let (edge, _edge_rx) = endpoint().await;
    let (target, mut target_rx) = endpoint().await;
    let options = DialOptions::new("<sip:edge@fixture.invalid>", ip())
        .with_cancellation_timeout(Duration::from_millis(30));
    assert!(
        sipx_call::dial_early_until(
            &edge,
            Target::udp(target.local_addr()),
            &uri(),
            &options,
            std::future::ready(())
        )
        .await
        .is_err()
    );
    // Check after the decision has completed; receiving INVITE here violates sticky stop.
    assert!(
        tokio::time::timeout(Duration::from_millis(30), target_rx.recv())
            .await
            .is_err(),
        "a ready stop must be checked before handing INVITE to the transport"
    );
    edge.shutdown().await;
    target.shutdown().await;
}

#[tokio::test]
async fn peer_bye_observes_other_leg_and_preserves_protocol_cause() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(500));
    let owner = h.drive(control.clone(), Duration::from_millis(500));
    let dialog = confirmed(&mut h, &control).await;
    let mut bye = dialog_request(&h.source, &dialog, &Method::Bye, 2);
    bye.headers.push_front(
        sipx_sip::Header::build(
            HeaderName::Reason,
            Bytes::from_static(b"Q.850;cause=31;text=\"Normal unspecified\""),
        )
        .unwrap(),
    );
    let mut responses = h
        .source
        .send(bye, Target::udp(h.edge.local_addr()))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(LIMIT, responses.final_response())
            .await
            .unwrap()
            .unwrap()
            .status
            .code(),
        200
    );
    let forwarded = next_bye(&mut h.target_rx).await;
    let reason = forwarded
        .request
        .headers
        .value(&HeaderName::Reason)
        .unwrap();
    assert!(
        String::from_utf8_lossy(reason.as_ref()).contains("cause=31"),
        "{reason:?}"
    );
    assert!(control.wait(Duration::from_millis(20)).await.is_none());
    response(&h.target, &forwarded, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    assert!(report.observed(), "{report:?}");
    assert!(matches!(report.one, LegTermination::RemoteBye));
    assert!(matches!(report.two, LegTermination::Bye(200)));
    h.close().await;
}

// S5: accepted peer BYE is positive evidence even when the crossed local BYE is unanswered.
#[tokio::test]
async fn adversary_crossed_remote_bye_is_terminal_without_local_bye_response() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(250));
    let owner = h.drive(control.clone(), Duration::from_millis(250));
    let dialog = confirmed(&mut h, &control).await;
    control.request_stop();
    let source_bye = next_bye(&mut h.source_rx).await;
    assert_eq!(source_bye.request.method, Method::Bye);
    let mut peer_bye = h
        .source
        .send(
            dialog_request(&h.source, &dialog, &Method::Bye, 2),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    let accepted = tokio::time::timeout(LIMIT, peer_bye.final_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(accepted.status.code(), 200);
    let target_bye = next_bye(&mut h.target_rx).await;
    response(&h.target, &target_bye, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    // Complete fixture ownership before asserting, including on a regression.
    h.close().await;
    assert!(
        report.observed(),
        "accepted peer BYE proves its dialog ended: {report:?}"
    );
    assert_eq!(report.one, LegTermination::RemoteBye);
    assert_eq!(report.two, LegTermination::Bye(200));
}

// S3: a final rejection to the crossed-dialog BYE cannot become successful withdrawal.
#[tokio::test]
async fn adversary_late_200_rejected_bye_remains_unknown() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(250));
    let owner = h.drive(control.clone(), Duration::from_millis(250));
    let invite = next(&mut h.target_rx).await;
    control.request_stop();
    response(&h.target, &invite, 180, false).await;
    let cancel = next(&mut h.target_rx).await;
    assert_eq!(cancel.request.method, Method::Cancel);
    response(&h.target, &cancel, 200, false).await;
    response(&h.target, &invite, 200, true).await;
    assert_eq!(next(&mut h.target_rx).await.request.method, Method::Ack);
    let bye = next(&mut h.target_rx).await;
    assert_eq!(bye.request.method, Method::Bye);
    response(&h.target, &bye, 503, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    h.close().await;
    assert!(
        !report.observed(),
        "negative BYE is not evidence of absence: {report:?}"
    );
    assert_eq!(report.two, LegTermination::Unknown);
    assert_eq!(report.one, LegTermination::Rejected(487));
}

#[tokio::test]
async fn correction_rejected_crossed_bye_is_not_terminal_evidence() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(150));
    let owner = h.drive(control.clone(), Duration::from_millis(150));
    let dialog = confirmed(&mut h, &control).await;
    control.request_stop();
    let _unanswered = next_bye(&mut h.source_rx).await;
    // The initial INVITE already consumed remote CSeq 1. Local teardown has also set ended,
    // but a stale BYE still receives 500 and must never become RemoteBye evidence.
    let mut rejected = h
        .source
        .send(
            dialog_request(&h.source, &dialog, &Method::Bye, 1),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(LIMIT, rejected.final_response())
            .await
            .unwrap()
            .unwrap()
            .status
            .code(),
        500
    );
    let target_bye = next_bye(&mut h.target_rx).await;
    response(&h.target, &target_bye, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    h.close().await;
    assert!(
        !report.observed(),
        "rejected BYE is not acceptance: {report:?}"
    );
    assert_eq!(report.one, LegTermination::Unknown);
    assert_eq!(report.two, LegTermination::Bye(200));
}

#[tokio::test]
async fn correction_peer_acceptance_survives_stop_interrupting_the_other_leg_wait() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(500));
    let owner = h.drive(control.clone(), Duration::from_millis(500));
    let invite = next(&mut h.target_rx).await;
    let target_dialog = sipx_call::Dialog::from_request(&invite.request, "target").unwrap();
    response(&h.target, &invite, 200, true).await;
    let answer = tokio::time::timeout(LIMIT, h.source_responses.final_response())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.status.code(), 200);
    let source_dialog = sipx_call::Dialog::from_response(&h.source_request, &answer).unwrap();
    h.source
        .send_directly(
            dialog_request(&h.source, &source_dialog, &Method::Ack, 1),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    assert!(control.wait_confirmed(LIMIT).await);
    let mut source_bye = h
        .source
        .send(
            dialog_request(&h.source, &source_dialog, &Method::Bye, 2),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(LIMIT, source_bye.final_response())
            .await
            .unwrap()
            .unwrap()
            .status
            .code(),
        200
    );
    // The normal peer-BYE driver is awaiting this target BYE. Cross it with a valid target BYE,
    // observe acceptance, then stop before acknowledging the owner's originated BYE.
    let _unanswered = next_bye(&mut h.target_rx).await;
    let mut target_bye = h
        .target
        .send(
            dialog_request(&h.target, &target_dialog, &Method::Bye, 1),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(LIMIT, target_bye.final_response())
            .await
            .unwrap()
            .unwrap()
            .status
            .code(),
        200
    );
    control.request_stop();
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    h.close().await;
    assert!(
        report.observed(),
        "accepted BYE survives dropped intermediate wait: {report:?}"
    );
    assert_eq!(report.one, LegTermination::RemoteBye);
    assert_eq!(report.two, LegTermination::RemoteBye);
}

#[tokio::test]
async fn adversary2_unmatched_bye_cannot_create_terminal_evidence() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(250));
    let owner = h.drive(control.clone(), Duration::from_millis(250));
    let dialog = confirmed(&mut h, &control).await;
    control.request_stop();
    let unanswered = next_bye(&mut h.source_rx).await;
    assert_eq!(unanswered.request.method, Method::Bye);
    let mut wrong = dialog_request(&h.source, &dialog, &Method::Bye, 2);
    wrong.headers.remove_all(&HeaderName::From);
    wrong.headers.push_front(
        sipx_sip::Header::build(
            HeaderName::From,
            Bytes::from_static(b"<sip:source@fixture.invalid>;tag=unmatched-source"),
        )
        .unwrap(),
    );
    let mut replies = h
        .source
        .send(wrong, Target::udp(h.edge.local_addr()))
        .await
        .unwrap();
    let refusal = tokio::time::timeout(LIMIT, replies.final_response())
        .await
        .unwrap()
        .unwrap();
    let target_bye = next_bye(&mut h.target_rx).await;
    response(&h.target, &target_bye, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    h.close().await;
    assert_eq!(refusal.status.code(), 481);
    assert!(
        !report.observed(),
        "another dialog's BYE cannot prove termination: {report:?}"
    );
    assert_eq!(report.one, LegTermination::Unknown);
    assert_eq!(report.two, LegTermination::Bye(200));
}

#[tokio::test]
async fn adversary2_accepted_bye_survives_negative_local_bye_response() {
    let mut h = Harness::new(false).await;
    let control = CouplingControl::new(Duration::from_millis(250));
    let owner = h.drive(control.clone(), Duration::from_millis(250));
    let dialog = confirmed(&mut h, &control).await;
    control.request_stop();
    let source_bye = next_bye(&mut h.source_rx).await;
    let mut replies = h
        .source
        .send(
            dialog_request(&h.source, &dialog, &Method::Bye, 2),
            Target::udp(h.edge.local_addr()),
        )
        .await
        .unwrap();
    let accepted = tokio::time::timeout(LIMIT, replies.final_response())
        .await
        .unwrap()
        .unwrap();
    // Peer acceptance is the barrier before rejecting the separately originated local BYE.
    assert_eq!(accepted.status.code(), 200);
    response(&h.source, &source_bye, 503, false).await;
    let target_bye = next_bye(&mut h.target_rx).await;
    response(&h.target, &target_bye, 200, false).await;
    let report = tokio::time::timeout(LIMIT, owner).await.unwrap().unwrap();
    let repeated = control.stop(LIMIT).await.unwrap();
    h.close().await;
    assert!(
        report.observed(),
        "accepted remote BYE survives a negative local response: {report:?}"
    );
    assert_eq!(report.one, LegTermination::RemoteBye);
    assert_eq!(report.two, LegTermination::Bye(200));
    assert_eq!(repeated.one, report.one);
    assert_eq!(repeated.two, report.two);
}
