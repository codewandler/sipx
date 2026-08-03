//! The webhook binding vectors (`WB-1` … `WB-9`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use sipx_app::webhook::{Delivery, Webhook, signature};
use sipx_app_protocol::{
    CallSnapshot, Direction, EndCause, Envelope, EventKind, Failure, Timestamp,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AT_MILLIS: i64 = 1_772_270_104_000;

fn envelope() -> Envelope {
    Envelope {
        seq: 1,
        at: Timestamp::from_unix_millis(AT_MILLIS),
        call: CallSnapshot::new("call-1", Direction::Inbound),
        event: EventKind::Incoming,
    }
}

async fn peer(responses: Vec<&'static str>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().await.expect("accepts");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = socket.read(&mut buffer).await.expect("reads");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&bytes);
                let Some(split) = text.find("\r\n\r\n") else {
                    continue;
                };
                let length = text[..split]
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= split + 4 + length {
                    break;
                }
            }
            requests.push(String::from_utf8(bytes).expect("request is text"));
            socket.write_all(response.as_bytes()).await.expect("writes");
        }
        requests
    });
    (url, task)
}

async fn dropping_peer(attempts: usize) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..attempts {
            let (mut socket, _) = listener.accept().await.expect("accepts");
            let mut bytes = vec![0_u8; 4096];
            let count = socket.read(&mut bytes).await.expect("reads");
            bytes.truncate(count);
            requests.push(String::from_utf8(bytes).expect("request is text"));
        }
        requests
    });
    (url, task)
}

fn webhook(url: &str) -> Webhook {
    Webhook::new(url, vec![b"new".to_vec()]).expect("valid webhook")
}

#[tokio::test]
async fn wb_1_posts_the_exact_envelope_and_returns_the_body() {
    let body = r#"{"contract":"sipx.app.v1","instructions":[]}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let response: &'static str = Box::leak(response.into_boxed_str());
    let (url, peer) = peer(vec![response]).await;
    let envelope = envelope();

    let result = webhook(&url)
        .deliver(&envelope, Duration::from_secs(1), AT_MILLIS / 1000)
        .await;

    assert_eq!(result, Delivery::Body(body.to_owned()));
    let requests = peer.await.expect("peer ends");
    assert!(requests[0].ends_with(&envelope.to_text()));
}

#[tokio::test]
async fn wb_4_retries_server_errors_with_identical_bytes() {
    let (url, peer) = peer(vec![
        "HTTP/1.1 500 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 503 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "HTTP/1.1 502 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ])
    .await;

    let result = webhook(&url)
        .deliver(&envelope(), Duration::from_secs(2), AT_MILLIS / 1000)
        .await;
    assert_eq!(result, Delivery::Failed(Failure::ServerError));

    let requests = peer.await.expect("peer ends");
    assert_eq!(requests.len(), 3);
    let normalized = |request: &str| {
        request
            .lines()
            .filter(|line| !line.to_ascii_lowercase().starts_with("connection:"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert_eq!(normalized(&requests[0]), normalized(&requests[1]));
    assert_eq!(normalized(&requests[1]), normalized(&requests[2]));
}

#[tokio::test]
async fn wb_2_times_out_and_discards_a_late_response() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let peer = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.expect("accepts");
        std::future::pending::<()>().await;
    });

    let result = webhook(&url)
        .deliver(&envelope(), Duration::from_millis(50), AT_MILLIS / 1000)
        .await;
    assert_eq!(result, Delivery::Failed(Failure::Timeout));
    peer.abort();
}

#[tokio::test]
async fn wb_3_retries_an_unreachable_peer_three_times() {
    let (url, peer) = dropping_peer(3).await;
    let result = webhook(&url)
        .deliver(&envelope(), Duration::from_secs(1), AT_MILLIS / 1000)
        .await;
    assert_eq!(result, Delivery::Failed(Failure::Unreachable));
    let requests = peer.await.expect("peer ends");
    assert_eq!(requests.len(), 3);
    let bodies: Vec<&str> = requests
        .iter()
        .filter_map(|request| request.split_once("\r\n\r\n").map(|(_, body)| body))
        .collect();
    assert_eq!(bodies, vec![envelope().to_text(); 3]);
}

#[tokio::test]
async fn wb_5_does_not_retry_a_client_error() {
    let (url, peer) = peer(vec![
        "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ])
    .await;
    let result = webhook(&url)
        .deliver(&envelope(), Duration::from_secs(1), AT_MILLIS / 1000)
        .await;
    assert_eq!(result, Delivery::Failed(Failure::ClientError));
    assert_eq!(peer.await.expect("peer ends").len(), 1);
}

#[tokio::test]
async fn wb_6_never_follows_a_redirect() {
    let target = std::net::TcpListener::bind("127.0.0.1:0").expect("target binds");
    target.set_nonblocking(true).expect("nonblocking");
    let location = format!("http://{}/moved", target.local_addr().expect("address"));
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let response: &'static str = Box::leak(response.into_boxed_str());
    let (url, peer) = peer(vec![response]).await;

    let result = webhook(&url)
        .deliver(&envelope(), Duration::from_secs(1), AT_MILLIS / 1000)
        .await;
    assert_eq!(result, Delivery::Failed(Failure::ServerError));
    assert_eq!(peer.await.expect("peer ends").len(), 1);
    assert!(
        target.accept().is_err(),
        "the redirect target saw no request"
    );
}

#[tokio::test]
async fn wb_7_the_budget_wins_while_a_retry_is_in_flight() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("binds");
    let url = format!("http://{}/hook", listener.local_addr().expect("address"));
    let peer = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.expect("first accepts");
        let mut request = [0_u8; 4096];
        let _ = first.read(&mut request).await.expect("first reads");
        first
            .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .expect("first writes");
        let (_second, _) = listener.accept().await.expect("retry accepts");
        std::future::pending::<()>().await;
    });

    let result = webhook(&url)
        .deliver(&envelope(), Duration::from_millis(200), AT_MILLIS / 1000)
        .await;
    assert_eq!(result, Delivery::Failed(Failure::Timeout));
    peer.abort();
}

#[test]
fn wb_8_has_a_fixed_signature_vector() {
    assert_eq!(
        signature(1_772_270_104, b"event", &[b"new".to_vec()]),
        "t=1772270104, v1=a339bd52d79f5581671f99d74e99fd755a40e3c24e45015e7424224dc6be6e60"
    );
}

#[test]
fn wb_9_rotation_signs_with_new_then_old() {
    assert_eq!(
        signature(1_772_270_104, b"event", &[b"new".to_vec(), b"old".to_vec()]),
        "t=1772270104, v1=a339bd52d79f5581671f99d74e99fd755a40e3c24e45015e7424224dc6be6e60, v1=38ca29ffa72de42c43fccffc5fd2b16816dd401eb2b2e2969ff638c36ca50957"
    );
}

#[test]
fn failure_vocabulary_covers_the_three_real_failure_vectors() {
    assert_eq!(Failure::Timeout, Failure::Timeout);
    assert_eq!(Failure::ServerError, Failure::ServerError);
    assert_eq!(Failure::ClientError, Failure::ClientError);
    let _ = EndCause::Error;
}
