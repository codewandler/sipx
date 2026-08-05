//! Application-owned requests on a live dialog (`S-40`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::net::{IpAddr, SocketAddr};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{
    ApplicationRequest, Call, CallEvent, CallEvents, Credentials, DialOptions, answer, dial,
};
use sipx_sip::build::RequestBuilder;
use sipx_sip::{
    Header, HeaderName, Headers, Host, HostName, Method, Request, Response, StatusCode, Uri,
};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

const SIGNALLING_BOUND: Duration = Duration::from_secs(10);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    let mut config = Config::new("127.0.0.1:0".parse().expect("valid"));
    config.limits = sipx_sip::Limits::stream();
    bind(config).await.expect("binds")
}

async fn connected(options: DialOptions) -> (Call, Receiver<Incoming>, Call, Receiver<Incoming>) {
    let (caller, caller_incoming, callee, callee_incoming, _callee_endpoint) =
        Box::pin(connected_with_server(options)).await;
    (caller, caller_incoming, callee, callee_incoming)
}

async fn connected_with_server(
    options: DialOptions,
) -> (Call, Receiver<Incoming>, Call, Receiver<Incoming>, Handle) {
    Box::pin(connected_with_server_and_timers(
        options,
        sipx_sip::Timers::default(),
    ))
    .await
}

async fn connected_with_server_and_timers(
    options: DialOptions,
    caller_timers: sipx_sip::Timers,
) -> (Call, Receiver<Incoming>, Call, Receiver<Incoming>, Handle) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let mut caller_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    caller_config.timers = caller_timers;
    caller_config.limits = sipx_sip::Limits::stream();
    let (caller_endpoint, caller_incoming) = bind(caller_config).await.expect("caller binds");
    let target = Target::udp(callee_endpoint.local_addr());
    let uri = Uri::sip(Host::Name(
        HostName::new("callee.example").expect("valid host"),
    ));

    let answering = async {
        let invite = callee_incoming.recv().await.expect("INVITE arrives");
        answer(&callee_endpoint, &invite, loopback())
            .await
            .expect("answers")
    };
    let calling = dial(&caller_endpoint, target, &uri, &options);
    let (callee, caller) = tokio::join!(answering, calling);
    (
        caller.expect("call connects"),
        caller_incoming,
        callee,
        callee_incoming,
        callee_endpoint,
    )
}

async fn next_application_request(
    call: &mut Call,
    incoming: &mut Receiver<Incoming>,
    events: &mut CallEvents,
) -> ApplicationRequest {
    tokio::time::timeout(SIGNALLING_BOUND, async {
        loop {
            tokio::select! {
                message = incoming.recv() => {
                    let message = message.expect("endpoint remains open");
                    assert!(call.handle(&message).await.expect("handles request"));
                }
                event = events.recv() => {
                    if let Some(CallEvent::ApplicationRequest(request)) = event {
                        return request;
                    }
                }
            }
        }
    })
    .await
    .expect("a bound on failure waiting for the application request")
}

async fn next_application_request_with_wire(
    call: &mut Call,
    incoming: &mut Receiver<Incoming>,
    events: &mut CallEvents,
) -> (ApplicationRequest, Request) {
    tokio::time::timeout(SIGNALLING_BOUND, async {
        let mut wire = None;
        loop {
            tokio::select! {
                message = incoming.recv() => {
                    let message = message.expect("endpoint remains open");
                    let application_owned = matches!(
                        message.request.method,
                        Method::Info | Method::Message | Method::Other(_)
                    );
                    assert!(call.handle(&message).await.expect("handles request"));
                    if application_owned {
                        wire = Some(message.request);
                    }
                }
                event = events.recv() => {
                    if let Some(CallEvent::ApplicationRequest(request)) = event {
                        return (request, wire.take().expect("wire request precedes its event"));
                    }
                }
            }
        }
    })
    .await
    .expect("a bound on failure waiting for the application request and wire image")
}

fn cseq(headers: &Headers) -> u32 {
    let value = headers
        .value(&HeaderName::CSeq)
        .expect("application request carries CSeq");
    std::str::from_utf8(&value)
        .expect("CSeq is ASCII")
        .split_ascii_whitespace()
        .next()
        .expect("CSeq has a number")
        .parse()
        .expect("CSeq number parses")
}

async fn exchange_application_request(
    sender: &mut Call,
    receiver: &mut Call,
    incoming: &mut Receiver<Incoming>,
    events: &mut CallEvents,
    method: Method,
    headers: &[Header],
    body: Bytes,
) -> Request {
    let expected_method = method.clone();
    let expected_body = body.clone();
    let send = sender.send_dialog_request(method, headers, body);
    let answer = async {
        let (request, wire) = next_application_request_with_wire(receiver, incoming, events).await;
        assert_eq!(request.method(), &expected_method);
        assert_eq!(request.body(), expected_body.as_ref());
        ok(request).await;
        wire
    };
    let (response, wire) = tokio::join!(send, answer);
    assert!(
        response
            .expect("application-owned request succeeds")
            .status
            .is_success()
    );
    wire
}

async fn ok(request: ApplicationRequest) {
    request
        .respond(
            StatusCode::new(200).expect("valid status"),
            "OK",
            &[],
            Bytes::new(),
        )
        .await
        .expect("responds");
}

fn raw_dialog_request(
    peer: &Handle,
    dialog: &sipx_call::Dialog,
    transport: sipx_transport::TransportKind,
    method: &Method,
    cseq: u32,
    content_type: Option<&str>,
    body: Bytes,
) -> Request {
    let (local, remote) = dialog.local_and_remote();
    let mut builder = RequestBuilder::new(method.clone(), dialog.remote_target.clone())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/{} {};rport;branch={}",
                transport.as_str(),
                peer.sent_by_for(transport),
                sipx_transport::new_branch()
            )),
        )
        .expect("Via")
        .header(HeaderName::To, Bytes::from(local))
        .expect("To")
        .header(HeaderName::From, Bytes::from(remote))
        .expect("From")
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))
        .expect("Call-ID")
        .cseq(cseq, method)
        .expect("CSeq")
        .max_forwards(70);
    if let Some(content_type) = content_type {
        builder = builder
            .header(
                HeaderName::ContentType,
                Bytes::from(content_type.to_owned()),
            )
            .expect("Content-Type");
    }
    builder.body(body).build()
}

async fn invalid_inbound(
    peer: &Handle,
    destination: SocketAddr,
    call: &mut Call,
    incoming: &mut Receiver<Incoming>,
    transport: sipx_transport::TransportKind,
    request: Request,
) -> (sipx_call::Result<bool>, Response) {
    let expected_method = request.method.clone();
    let expected_cseq = cseq(&request.headers);
    let mut responses = peer
        .send(request, Target::new(destination, transport))
        .await
        .expect("request starts");
    let handled = loop {
        let received = incoming.recv().await.expect("request arrives");
        if received.request.method == expected_method
            && cseq(&received.request.headers) == expected_cseq
        {
            break call.handle(&received).await;
        }
        assert!(
            call.handle(&received)
                .await
                .expect("prior dialog traffic handled")
        );
    };
    let response = responses
        .final_response()
        .await
        .expect("the invalid request receives a final response");
    (handled, response)
}

fn captured_application_request(
    runtime: &tokio::runtime::Runtime,
) -> (
    ApplicationRequest,
    tokio::task::JoinHandle<sipx_call::Result<Response>>,
) {
    runtime.block_on(async {
        let options = DialOptions::new("<sip:caller@example.test>", loopback());
        let (mut caller, _caller_incoming, mut callee, mut callee_incoming) =
            Box::pin(connected(options)).await;
        let mut events = callee.events().expect("callee event stream");
        let sending = tokio::spawn(async move {
            caller
                .send_dialog_request(Method::Info, &[], Bytes::new())
                .await
        });
        let request =
            next_application_request(&mut callee, &mut callee_incoming, &mut events).await;
        (request, sending)
    })
}

#[tokio::test]
async fn info_and_message_are_typed_events_in_both_directions() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, mut caller_incoming, mut callee, mut callee_incoming) =
        connected(options).await;
    let mut caller_events = caller.events().expect("caller event stream");
    let mut callee_events = callee.events().expect("callee event stream");
    let content_type = Header::build(HeaderName::ContentType, "text/plain").expect("valid header");

    let caller_info = exchange_application_request(
        &mut caller,
        &mut callee,
        &mut callee_incoming,
        &mut callee_events,
        Method::Info,
        std::slice::from_ref(&content_type),
        Bytes::from_static(b"caller-info"),
    )
    .await;
    let callee_info = exchange_application_request(
        &mut callee,
        &mut caller,
        &mut caller_incoming,
        &mut caller_events,
        Method::Info,
        std::slice::from_ref(&content_type),
        Bytes::from_static(b"callee-info"),
    )
    .await;
    let caller_message = exchange_application_request(
        &mut caller,
        &mut callee,
        &mut callee_incoming,
        &mut callee_events,
        Method::Message,
        std::slice::from_ref(&content_type),
        Bytes::from_static(b"caller-message"),
    )
    .await;
    let callee_message = exchange_application_request(
        &mut callee,
        &mut caller,
        &mut caller_incoming,
        &mut caller_events,
        Method::Message,
        std::slice::from_ref(&content_type),
        Bytes::from_static(b"callee-message"),
    )
    .await;

    assert_eq!(
        cseq(&caller_message.headers),
        cseq(&caller_info.headers) + 1
    );
    assert_eq!(
        cseq(&callee_message.headers),
        cseq(&callee_info.headers) + 1
    );
}

#[tokio::test]
async fn an_admitted_private_method_is_case_sensitive_and_uses_dialog_state() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, mut caller_incoming, mut callee, mut callee_incoming) =
        connected(options).await;
    let private = Method::Other(Bytes::from_static(b"PRIVATE"));
    caller
        .admit_dialog_method(&private)
        .expect("admits private token");
    callee
        .admit_dialog_method(&private)
        .expect("admits private token");
    let mut callee_events = callee.events().expect("callee event stream");
    let mut caller_events = caller.events().expect("caller event stream");

    let caller_private = exchange_application_request(
        &mut caller,
        &mut callee,
        &mut callee_incoming,
        &mut callee_events,
        private.clone(),
        &[],
        Bytes::new(),
    )
    .await;
    let callee_private = exchange_application_request(
        &mut callee,
        &mut caller,
        &mut caller_incoming,
        &mut caller_events,
        private.clone(),
        &[],
        Bytes::new(),
    )
    .await;
    assert!(caller_private.headers.get(&HeaderName::CallId).is_some());
    assert!(callee_private.headers.get(&HeaderName::CallId).is_some());

    assert!(matches!(
        caller
            .send_dialog_request(
                Method::Other(Bytes::from_static(b"private")),
                &[],
                Bytes::new()
            )
            .await,
        Err(sipx_call::Error::StackOwnedDialogMethod(_))
    ));
    assert!(matches!(
        caller
            .send_dialog_request(Method::Bye, &[], Bytes::new())
            .await,
        Err(sipx_call::Error::StackOwnedDialogMethod(Method::Bye))
    ));
    for (token, known) in [
        (Bytes::from_static(b"INVITE"), Method::Invite),
        (Bytes::from_static(b"ACK"), Method::Ack),
        (Bytes::from_static(b"BYE"), Method::Bye),
        (Bytes::from_static(b"CANCEL"), Method::Cancel),
        (Bytes::from_static(b"REGISTER"), Method::Register),
        (Bytes::from_static(b"OPTIONS"), Method::Options),
        (Bytes::from_static(b"INFO"), Method::Info),
        (Bytes::from_static(b"PRACK"), Method::Prack),
        (Bytes::from_static(b"UPDATE"), Method::Update),
        (Bytes::from_static(b"SUBSCRIBE"), Method::Subscribe),
        (Bytes::from_static(b"NOTIFY"), Method::Notify),
        (Bytes::from_static(b"REFER"), Method::Refer),
        (Bytes::from_static(b"MESSAGE"), Method::Message),
        (Bytes::from_static(b"PUBLISH"), Method::Publish),
    ] {
        let alias = Method::Other(token);
        assert!(matches!(
            caller.admit_dialog_method(&alias),
            Err(sipx_call::Error::StackOwnedDialogMethod(method)) if method == known
        ));
    }
}

#[tokio::test]
async fn application_owned_responses_cannot_refresh_the_remote_target() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming) = connected(options).await;
    let private = Method::Other(Bytes::from_static(b"PRIVATE"));
    caller
        .admit_dialog_method(&private)
        .expect("admits outbound private method");
    callee
        .admit_dialog_method(&private)
        .expect("admits inbound private method");
    let mut events = callee.events().expect("callee event stream");
    let original_target = caller.dialog.remote_target.to_bytes();
    let injected = Header::build(HeaderName::Contact, "<sip:redirected@192.0.2.200:65000>")
        .expect("valid Contact");

    for method in [Method::Info, Method::Message, private] {
        let send = caller.send_dialog_request(method, &[], Bytes::new());
        let answer = async {
            let request =
                next_application_request(&mut callee, &mut callee_incoming, &mut events).await;
            request
                .respond(
                    StatusCode::new(200).expect("valid status"),
                    "OK",
                    std::slice::from_ref(&injected),
                    Bytes::new(),
                )
                .await
                .expect("responds with Contact");
        };
        let (response, ()) = tokio::join!(send, answer);
        assert!(response.expect("request succeeds").status.is_success());
        assert_eq!(caller.dialog.remote_target.to_bytes(), original_target);
    }
}

#[tokio::test]
async fn outbound_requests_use_the_live_remote_target_and_route_set() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming, server) =
        connected_with_server(options).await;
    let mut events = callee.events().expect("callee event stream");
    let server_addr = server.local_addr();
    let remote_target = Uri::parse(Bytes::from(format!(
        "sip:application@{}:{}",
        server_addr.ip(),
        server_addr.port()
    )))
    .expect("live remote target");
    let route = format!("<sip:{}:{};lr>", server_addr.ip(), server_addr.port());
    caller.dialog.remote_target = remote_target.clone();
    caller.dialog.route_set = vec![route.clone()];

    let wire = exchange_application_request(
        &mut caller,
        &mut callee,
        &mut callee_incoming,
        &mut events,
        Method::Info,
        &[],
        Bytes::new(),
    )
    .await;

    assert_eq!(wire.uri.to_bytes(), remote_target.to_bytes());
    assert_eq!(
        wire.headers
            .value(&HeaderName::Route)
            .expect("route set is rendered")
            .as_ref(),
        route.as_bytes()
    );
}

/// S-36 / RFC 3261 §§22.2 and 22.4: an in-dialog application request challenged with 401 or
/// 407 is retried with credentials and a fresh `CSeq` in either dialog direction.
#[tokio::test]
async fn digest_challenges_retry_info_and_message_in_both_directions() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback())
        .with_credentials(Credentials::new("caller", "secret"));
    let (mut caller, mut caller_incoming, mut callee, mut callee_incoming) =
        connected(options).await;
    callee.set_dialog_credentials(Credentials::new("callee", "secret"));
    let mut callee_events = callee.events().expect("callee event stream");
    let mut caller_events = caller.events().expect("caller event stream");

    let send = caller.send_dialog_request(Method::Info, &[], Bytes::new());
    let challenge_then_answer = async {
        let first =
            next_application_request(&mut callee, &mut callee_incoming, &mut callee_events).await;
        let first_cseq = cseq(first.headers());
        assert!(first.headers().get(&HeaderName::Authorization).is_none());
        let challenge = Header::build(
            HeaderName::WwwAuthenticate,
            r#"Digest realm="example.test", nonce="one", qop="auth", algorithm=SHA-256"#,
        )
        .expect("valid challenge");
        first
            .respond(
                StatusCode::new(401).expect("valid status"),
                "Unauthorized",
                &[challenge],
                Bytes::new(),
            )
            .await
            .expect("challenges");

        let retry =
            next_application_request(&mut callee, &mut callee_incoming, &mut callee_events).await;
        assert_eq!(cseq(retry.headers()), first_cseq + 1);
        assert!(retry.headers().get(&HeaderName::Authorization).is_some());
        ok(retry).await;
    };
    let (response, ()) = tokio::join!(send, challenge_then_answer);
    assert!(
        response
            .expect("authenticated INFO succeeds")
            .status
            .is_success()
    );

    let send = callee.send_dialog_request(Method::Message, &[], Bytes::new());
    let challenge_then_answer = async {
        let first =
            next_application_request(&mut caller, &mut caller_incoming, &mut caller_events).await;
        let first_cseq = cseq(first.headers());
        assert!(
            first
                .headers()
                .get(&HeaderName::ProxyAuthorization)
                .is_none()
        );
        let challenge = Header::build(
            HeaderName::ProxyAuthenticate,
            r#"Digest realm="proxy.example.test", nonce="two", qop="auth", algorithm=SHA-256"#,
        )
        .expect("valid challenge");
        first
            .respond(
                StatusCode::new(407).expect("valid status"),
                "Proxy Authentication Required",
                &[challenge],
                Bytes::new(),
            )
            .await
            .expect("challenges");

        let retry =
            next_application_request(&mut caller, &mut caller_incoming, &mut caller_events).await;
        assert_eq!(cseq(retry.headers()), first_cseq + 1);
        assert!(
            retry
                .headers()
                .get(&HeaderName::ProxyAuthorization)
                .is_some()
        );
        ok(retry).await;
    };
    let (response, ()) = tokio::join!(send, challenge_then_answer);
    assert!(
        response
            .expect("authenticated MESSAGE succeeds")
            .status
            .is_success()
    );
}

#[tokio::test]
async fn dropping_the_response_owner_refuses_the_request() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming) = connected(options).await;
    let mut events = callee.events().expect("callee event stream");

    let send = caller.send_dialog_request(Method::Message, &[], Bytes::new());
    let abandon = async {
        let request =
            next_application_request(&mut callee, &mut callee_incoming, &mut events).await;
        drop(request);
    };
    let (response, ()) = tokio::join!(send, abandon);
    assert!(matches!(
        response,
        Err(sipx_call::Error::Rejected { status: 500, .. })
    ));
}

#[tokio::test]
async fn cloned_response_owners_still_share_exactly_one_answer() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming) = connected(options).await;
    let mut events = callee.events().expect("callee event stream");

    let send = caller.send_dialog_request(Method::Info, &[], Bytes::new());
    let answer_from_clone = async {
        let request =
            next_application_request(&mut callee, &mut callee_incoming, &mut events).await;
        let duplicate = request.clone();
        ok(request).await;
        let error = duplicate
            .respond(
                StatusCode::new(200).expect("valid status"),
                "OK again",
                &[],
                Bytes::new(),
            )
            .await
            .expect_err("the shared capability was already spent");
        assert!(matches!(
            error,
            sipx_call::Error::ApplicationResponseAlreadySent
        ));
    };
    let (response, ()) = tokio::join!(send, answer_from_clone);
    assert!(response.expect("clone answers once").status.is_success());
}

#[tokio::test]
async fn ended_and_invalid_outbound_requests_are_typed_and_send_nothing() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming) =
        Box::pin(connected(options)).await;
    while let Ok(pending) = callee_incoming.try_recv() {
        assert!(
            callee
                .handle(&pending)
                .await
                .expect("initial traffic handled")
        );
    }

    let content_type = Header::build(HeaderName::ContentType, "text/plain").expect("header");
    let oversized = Bytes::from(vec![0; sipx_call::MAX_APPLICATION_BODY + 1]);
    assert!(matches!(
        caller
            .send_dialog_request(
                Method::Message,
                std::slice::from_ref(&content_type),
                oversized,
            )
            .await,
        Err(sipx_call::Error::ApplicationBodyTooLarge { .. })
    ));
    assert!(
        callee_incoming.try_recv().is_err(),
        "oversize sent no bytes"
    );

    assert!(matches!(
        caller
            .send_dialog_request(Method::Info, &[], Bytes::from_static(b"body"))
            .await,
        Err(sipx_call::Error::ApplicationContentTypeRequired)
    ));
    assert!(
        callee_incoming.try_recv().is_err(),
        "missing Content-Type sent no bytes"
    );

    let ending = caller.hang_up();
    let answer_bye = async {
        let bye = callee_incoming.recv().await.expect("BYE arrives");
        assert!(callee.handle(&bye).await.expect("BYE handled"));
    };
    let (ended, ()) = tokio::join!(ending, answer_bye);
    ended.expect("hangup completes");
    assert!(matches!(
        caller
            .send_dialog_request(Method::Info, &[], Bytes::new())
            .await,
        Err(sipx_call::Error::DialogEnded)
    ));
    while let Ok(residual) = callee_incoming.try_recv() {
        assert_ne!(
            residual.request.method,
            Method::Info,
            "ended call sent no INFO"
        );
    }
}

#[tokio::test]
async fn invalid_inbound_bodies_are_refused_without_an_application_event() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (_caller, _caller_incoming, mut callee, mut callee_incoming, server) =
        Box::pin(connected_with_server(options)).await;
    let mut events = callee.events().expect("callee event stream");
    while events.try_recv().is_some() {}
    let (peer, _peer_incoming) = endpoint().await;

    let oversized = raw_dialog_request(
        &peer,
        &callee.dialog,
        sipx_transport::TransportKind::Tcp,
        &Method::Message,
        99,
        Some("text/plain"),
        Bytes::from(vec![0; sipx_call::MAX_APPLICATION_BODY + 1]),
    );
    let (handled, response) = invalid_inbound(
        &peer,
        server.local_addr(),
        &mut callee,
        &mut callee_incoming,
        sipx_transport::TransportKind::Tcp,
        oversized,
    )
    .await;
    assert!(matches!(
        handled,
        Err(sipx_call::Error::ApplicationBodyTooLarge { .. })
    ));
    assert_eq!(response.status.code(), 413);
    assert!(events.try_recv().is_none());

    let missing_type = raw_dialog_request(
        &peer,
        &callee.dialog,
        sipx_transport::TransportKind::Tcp,
        &Method::Info,
        100,
        None,
        Bytes::from_static(b"body"),
    );
    let (handled, response) = invalid_inbound(
        &peer,
        server.local_addr(),
        &mut callee,
        &mut callee_incoming,
        sipx_transport::TransportKind::Tcp,
        missing_type,
    )
    .await;
    assert!(matches!(
        handled,
        Err(sipx_call::Error::ApplicationContentTypeRequired)
    ));
    assert_eq!(response.status.code(), 415);
    assert!(events.try_recv().is_none());
}

#[test]
fn application_request_construction_fails_cleanly_without_a_runtime_context() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime builds");
    let (mut callee, incoming, sending) = runtime.block_on(async {
        let options = DialOptions::new("<sip:caller@example.test>", loopback());
        let (mut caller, _caller_incoming, mut callee, mut callee_incoming) =
            Box::pin(connected(options)).await;
        let sending = tokio::spawn(async move {
            caller
                .send_dialog_request(Method::Info, &[], Bytes::new())
                .await
        });
        loop {
            let incoming = callee_incoming.recv().await.expect("INFO arrives");
            if incoming.request.method == Method::Info {
                break (callee, incoming, sending);
            }
            assert!(
                callee
                    .handle(&incoming)
                    .await
                    .expect("initial traffic handled")
            );
        }
    });
    assert!(tokio::runtime::Handle::try_current().is_err());

    let mut handling = Box::pin(callee.handle(&incoming));
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let polled = handling.as_mut().poll(&mut context);
    assert!(
        matches!(
            polled,
            Poll::Ready(Err(sipx_call::Error::ApplicationRuntimeUnavailable))
        ),
        "unexpected public construction result: {polled:?}"
    );
    sending.abort();
}

#[test]
fn public_response_uses_its_captured_runtime_outside_runtime_context() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime builds");
    let (request, sending) = captured_application_request(&runtime);
    assert!(tokio::runtime::Handle::try_current().is_err());

    let mut responding = Box::pin(request.respond(
        StatusCode::new(200).expect("status"),
        "OK",
        &[],
        Bytes::new(),
    ));
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let first = responding.as_mut().poll(&mut context);
    let result = match first {
        Poll::Ready(result) => result,
        Poll::Pending => runtime.block_on(responding.as_mut()),
    };
    result.expect("public response completes on captured runtime");
    let response = runtime
        .block_on(sending)
        .expect("sender task completes")
        .expect("INFO succeeds");
    assert_eq!(response.status.code(), 200);
}

#[test]
fn last_public_response_owner_drops_outside_runtime_and_sends_fallback() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("runtime builds");
    let (request, sending) = captured_application_request(&runtime);
    assert!(tokio::runtime::Handle::try_current().is_err());
    drop(request);

    let result = runtime
        .block_on(sending)
        .expect("sender task completes after fallback");
    assert!(matches!(
        result,
        Err(sipx_call::Error::Rejected { status: 500, .. })
    ));
}

#[tokio::test(start_paused = true)]
async fn response_timeout_sends_504_and_releases_the_server_transaction() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let caller_timers = sipx_sip::Timers {
        // Keep the client transaction alive beyond the application response deadline so the test
        // observes the 504 rather than racing the ordinary 64*T1 timeout at the same tick.
        t1: Duration::from_secs(1),
        ..sipx_sip::Timers::default()
    };
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming, server) =
        Box::pin(connected_with_server_and_timers(options, caller_timers)).await;
    let mut events = callee.events().expect("callee event stream");

    // Let the completed INVITE transaction reach its normal cleanup horizon before measuring the
    // application-owned server transaction in isolation.
    tokio::time::advance(Duration::from_secs(33)).await;
    tokio::task::yield_now().await;
    assert_eq!(server.outstanding().await.expect("diagnostics"), 0);

    let send = caller.send_dialog_request(Method::Info, &[], Bytes::new());
    let let_deadline_answer = async {
        let request =
            next_application_request(&mut callee, &mut callee_incoming, &mut events).await;
        // Let the spawned deadline future register its timer before advancing virtual time.
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(32)).await;
        tokio::task::yield_now().await;
        assert_eq!(request.method(), &Method::Info, "the owner stays live");
        drop(request);
    };
    let (response, ()) = tokio::join!(send, let_deadline_answer);
    assert!(
        matches!(
            &response,
            Err(sipx_call::Error::Rejected { status: 504, .. })
        ),
        "unexpected timeout result: {response:?}"
    );

    // A final UDP response retains its server transaction for Timer J so retransmissions receive
    // the same answer. Advancing beyond that protocol lifetime must release every associated map.
    tokio::time::advance(Duration::from_secs(33)).await;
    tokio::task::yield_now().await;
    assert_eq!(server.outstanding().await.expect("diagnostics"), 0);
}
