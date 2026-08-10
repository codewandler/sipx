//! Behavioural half of the at-rest byte-carrier rule.
//!
//! The structural checker can prove that each reachable public carrier has a hand-written
//! `Debug`; only a test of the rendered value can prove that implementation kept the bytes out.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use bytes::Bytes;
use sipx_app::session::SessionApp;
use sipx_app::webhook::Webhook;
use sipx_app::wss::WssMessage;

#[test]
fn a_session_apps_debug_keeps_its_upgrade_credential_out() {
    let printed = format!(
        "{:?}",
        SessionApp::new(b"session-upgrade-secret".to_vec(), true)
    );
    assert!(!printed.contains("session-upgrade-secret"), "{printed}");
    assert!(printed.contains("[redacted]"), "{printed}");
}

#[test]
fn a_webhooks_debug_keeps_every_signing_key_out() {
    let printed = format!(
        "{:?}",
        Webhook::new(
            "https://app.example.invalid/calls",
            vec![
                b"old-signing-secret".to_vec(),
                b"new-signing-secret".to_vec(),
            ],
        )
        .expect("valid webhook")
    );
    assert!(!printed.contains("old-signing-secret"), "{printed}");
    assert!(!printed.contains("new-signing-secret"), "{printed}");
    assert!(printed.contains("signing_keys: 2"), "{printed}");
}

#[test]
fn a_wss_messages_debug_keeps_text_and_binary_call_content_out() {
    let text = "text-call-secret-μ";
    let binary = b"binary-call-secret";
    let printed_text = format!("{:?}", WssMessage::Text(text.to_owned()));
    let printed_binary = format!("{:?}", WssMessage::Binary(Bytes::copy_from_slice(binary)));

    assert!(!printed_text.contains(text), "{printed_text}");
    assert!(
        !printed_binary.contains("binary-call-secret"),
        "{printed_binary}"
    );
    assert!(printed_text.contains("Text"), "{printed_text}");
    assert!(
        printed_text.contains(&text.len().to_string()),
        "{printed_text}"
    );
    assert!(printed_binary.contains("Binary"), "{printed_binary}");
    assert!(
        printed_binary.contains(&binary.len().to_string()),
        "{printed_binary}"
    );
}
