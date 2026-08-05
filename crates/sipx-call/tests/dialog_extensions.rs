//! Application-owned requests on a live dialog (`S-40`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::similar_names
)]

use std::net::IpAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{
    ApplicationRequest, Call, CallEvent, CallEvents, Credentials, DialOptions, answer, dial,
};
use sipx_sip::{Header, HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;

const SIGNALLING_BOUND: Duration = Duration::from_secs(10);

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds")
}

async fn connected(options: DialOptions) -> (Call, Receiver<Incoming>, Call, Receiver<Incoming>) {
    let (caller, caller_incoming, callee, callee_incoming, _callee_endpoint) =
        connected_with_server(options).await;
    (caller, caller_incoming, callee, callee_incoming)
}

async fn connected_with_server(
    options: DialOptions,
) -> (Call, Receiver<Incoming>, Call, Receiver<Incoming>, Handle) {
    connected_with_server_and_timers(options, sipx_sip::Timers::default()).await
}

async fn connected_with_server_and_timers(
    options: DialOptions,
    caller_timers: sipx_sip::Timers,
) -> (Call, Receiver<Incoming>, Call, Receiver<Incoming>, Handle) {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let mut caller_config = Config::new("127.0.0.1:0".parse().expect("valid"));
    caller_config.timers = caller_timers;
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

#[tokio::test]
async fn info_and_message_are_typed_events_in_both_directions() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, mut caller_incoming, mut callee, mut callee_incoming) =
        connected(options).await;
    let mut caller_events = caller.events().expect("caller event stream");
    let mut callee_events = callee.events().expect("callee event stream");
    let content_type = Header::build(HeaderName::ContentType, "text/plain").expect("valid header");

    let send_info = caller.send_dialog_request(
        Method::Info,
        std::slice::from_ref(&content_type),
        Bytes::from_static(b"caller-info"),
    );
    let answer_info = async {
        let request =
            next_application_request(&mut callee, &mut callee_incoming, &mut callee_events).await;
        assert_eq!(request.method(), &Method::Info);
        assert_eq!(request.body(), b"caller-info");
        ok(request).await;
    };
    let (sent, ()) = tokio::join!(send_info, answer_info);
    assert!(sent.expect("INFO succeeds").status.is_success());

    let send_message = callee.send_dialog_request(
        Method::Message,
        std::slice::from_ref(&content_type),
        Bytes::from_static(b"callee-message"),
    );
    let answer_message = async {
        let request =
            next_application_request(&mut caller, &mut caller_incoming, &mut caller_events).await;
        assert_eq!(request.method(), &Method::Message);
        assert_eq!(request.body(), b"callee-message");
        ok(request).await;
    };
    let (sent, ()) = tokio::join!(send_message, answer_message);
    assert!(sent.expect("MESSAGE succeeds").status.is_success());
}

#[tokio::test]
async fn an_admitted_private_method_is_case_sensitive_and_uses_dialog_state() {
    let options = DialOptions::new("<sip:caller@example.test>", loopback());
    let (mut caller, _caller_incoming, mut callee, mut callee_incoming) = connected(options).await;
    let private = Method::Other(Bytes::from_static(b"PRIVATE"));
    caller
        .admit_dialog_method(&private)
        .expect("admits private token");
    callee
        .admit_dialog_method(&private)
        .expect("admits private token");
    let mut events = callee.events().expect("callee event stream");

    let send = caller.send_dialog_request(private.clone(), &[], Bytes::new());
    let answer = async {
        let request =
            next_application_request(&mut callee, &mut callee_incoming, &mut events).await;
        assert_eq!(request.method(), &private);
        assert!(request.headers().get(&HeaderName::CallId).is_some());
        ok(request).await;
    };
    let (response, ()) = tokio::join!(send, answer);
    assert!(
        response
            .expect("private request succeeds")
            .status
            .is_success()
    );

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
}

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
        connected_with_server_and_timers(options, caller_timers).await;
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
