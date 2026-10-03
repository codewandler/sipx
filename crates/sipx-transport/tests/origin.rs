//! Admission is installed before either upgrade listener starts.
#![cfg(feature = "wss")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use futures_util::SinkExt as _;
use sipx_testkit::certs::Ca;
use sipx_transport::tls::{ClientTls, Identity, ServerTls, TrustAnchors};
use sipx_transport::{BindOptions, Config, WebSocketOriginPolicy, bind_with_options};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::{Error, http::Request};

#[test]
fn configured_origins_are_bounded_and_validated() {
    for origin in [
        "null",
        "*",
        "https://*.example.com",
        "https://example.com/",
        "https://a@b",
        "https://a?b",
        "https://a#b",
        "file://a",
        "https://a:99999",
        "https://[::1]suffix",
        "https://[::1]:",
        "https://[::1]:word",
        "https://a:",
        "https://a:word",
        "https://a:+80",
        "https://[::1]:+80",
        "https://a b",
        "https://a\nb",
    ] {
        assert!(
            WebSocketOriginPolicy::require_listed(vec![origin.to_owned()]).is_err(),
            "{origin}"
        );
    }
    assert!(
        WebSocketOriginPolicy::require_listed(vec!["https://example.com".to_owned(); 129]).is_err()
    );
    assert!(
        WebSocketOriginPolicy::require_listed(vec![format!("https://{}", "a".repeat(2048))])
            .is_err()
    );
    assert!(
        WebSocketOriginPolicy::require_listed(vec![
            "http://localhost:8080".into(),
            "https://[::1]:443".into()
        ])
        .is_ok()
    );
}

async fn upgrade<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    origins: &[&str],
    sip: bool,
) -> bool {
    let mut request = Request::builder()
        .method("GET")
        .uri("ws://localhost/")
        .header("Host", "localhost")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==");
    if sip {
        request = request.header("Sec-WebSocket-Protocol", "sip");
    }
    for origin in origins {
        request = request.header("Origin", *origin);
    }
    match tokio_tungstenite::client_async(request.body(()).unwrap(), stream).await {
        Ok((mut socket, _)) => {
            let branch = sipx_transport::new_branch();
            let options = format!(
                "OPTIONS sip:peer@example.test SIP/2.0\r\n\
                 Via: SIP/2.0/WS browser.invalid;branch={branch};rport\r\n\
                 From: <sip:browser@example.test>;tag=origin\r\n\
                 To: <sip:peer@example.test>\r\n\
                 Call-ID: {branch}@example.test\r\n\
                 CSeq: 1 OPTIONS\r\nMax-Forwards: 70\r\nContent-Length: 0\r\n\r\n"
            );
            socket
                .send(tokio_tungstenite::tungstenite::Message::text(options))
                .await
                .unwrap();
            socket.close(None).await.unwrap();
            true
        }
        Err(Error::Http(response)) => {
            assert_eq!(response.status().as_u16(), if sip { 403 } else { 400 });
            false
        }
        Err(error) => panic!("unexpected handshake failure: {error}"),
    }
}

#[tokio::test]
async fn ws_and_wss_reject_unlisted_origins_before_adoption() {
    let ca = Ca::new();
    let (cert, key) = ca.issue_for("localhost");
    let mut config = Config::new("127.0.0.1:0".parse().unwrap());
    config.ws_server = Some(0);
    config.wss_server = Some((
        ServerTls::new(Identity::from_pem(cert.as_bytes(), key.as_bytes()).unwrap()).unwrap(),
        0,
    ));
    let policy =
        WebSocketOriginPolicy::require_listed(vec!["https://console.example".into()]).unwrap();
    let (server, mut incoming) = bind_with_options(
        config,
        BindOptions::default().with_websocket_origins(policy),
    )
    .await
    .unwrap();
    let mut anchors = TrustAnchors::only();
    anchors.add_pem(ca.pem().as_bytes()).unwrap();
    let tls = ClientTls::new(&anchors).unwrap();
    for secure in [false, true] {
        for (origins, accepted) in [
            (vec![], false),
            (vec!["null"], false),
            (vec!["https://elsewhere.example"], false),
            (
                vec!["https://console.example", "https://console.example"],
                false,
            ),
            (vec!["https://console.example/"], false),
            (vec!["https://console.example"], true),
        ] {
            let addr = if secure {
                server.wss_addr().unwrap()
            } else {
                server.ws_addr().unwrap()
            };
            let stream = TcpStream::connect(addr).await.unwrap();
            let result = if secure {
                let stream = tls
                    .connector()
                    .connect("localhost".try_into().unwrap(), stream)
                    .await
                    .unwrap();
                upgrade(stream, &origins, true).await
            } else {
                upgrade(stream, &origins, true).await
            };
            assert_eq!(result, accepted, "secure={secure} origins={origins:?}");
            if accepted {
                let message =
                    tokio::time::timeout(std::time::Duration::from_secs(3), incoming.recv())
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(message.request.method, sipx_sip::Method::Options);
            } else {
                assert!(incoming.try_recv().is_err());
            }
        }
        let addr = if secure {
            server.wss_addr().unwrap()
        } else {
            server.ws_addr().unwrap()
        };
        let stream = TcpStream::connect(addr).await.unwrap();
        let result = if secure {
            let stream = tls
                .connector()
                .connect("localhost".try_into().unwrap(), stream)
                .await
                .unwrap();
            upgrade(stream, &["https://console.example"], false).await
        } else {
            upgrade(stream, &["https://console.example"], false).await
        };
        assert!(!result);
    }
    server.shutdown().await;
}

#[tokio::test]
async fn serialized_origin_is_not_normalized_or_suffix_matched() {
    let mut config = Config::new("127.0.0.1:0".parse().unwrap());
    config.ws_server = Some(0);
    let policy =
        WebSocketOriginPolicy::require_listed(vec!["https://console.example".into()]).unwrap();
    let (server, mut incoming) = bind_with_options(
        config,
        BindOptions::default().with_websocket_origins(policy),
    )
    .await
    .unwrap();
    for origin in [
        "HTTPS://console.example",
        "https://CONSOLE.example",
        "https://console.example:443",
        "https://console.example.evil",
        "https://console.example,https://console.example",
    ] {
        let stream = TcpStream::connect(server.ws_addr().unwrap()).await.unwrap();
        assert!(!upgrade(stream, &[origin], true).await, "{origin}");
        assert!(incoming.try_recv().is_err());
    }
    let stream = TcpStream::connect(server.ws_addr().unwrap()).await.unwrap();
    assert!(upgrade(stream, &["https://console.example"], true).await);
    let received = tokio::time::timeout(std::time::Duration::from_secs(3), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.request.method, sipx_sip::Method::Options);
    server.shutdown().await;
}
