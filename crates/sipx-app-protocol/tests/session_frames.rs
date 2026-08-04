//! Session-frame wire vectors.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_app_protocol::{Document, SessionErrorCode, SessionReply, SessionRequest};

#[test]
fn document_frame_keeps_call_and_correlation() {
    let text =
        r#"{"contract":"sipx.app.v1","request":"replace-7","call":"call-4","instructions":[]}"#;
    let request = SessionRequest::parse(text).expect("the frame is valid");
    assert_eq!(
        request,
        SessionRequest::Document {
            request: "replace-7".to_owned(),
            call: "call-4".to_owned(),
            document: Document::keep_going(),
        }
    );
}

#[test]
fn originate_and_replies_have_one_stable_shape() {
    let request = SessionRequest::parse(
        r#"{"contract":"sipx.app.v1","request":"dial-2","do":"originate","target":"sip:bob@example.net","from":"sip:alerts@example.com"}"#,
    )
    .expect("the frame is valid");
    assert_eq!(
        request,
        SessionRequest::Originate {
            request: "dial-2".to_owned(),
            target: "sip:bob@example.net".to_owned(),
            from: "sip:alerts@example.com".to_owned(),
        }
    );

    assert_eq!(
        SessionReply::result("dial-2", "call-9").to_text(),
        r#"{"contract":"sipx.app.v1","request":"dial-2","result":{"call":"call-9"}}"#
    );
    assert_eq!(
        SessionReply::error(
            Some("replace-7"),
            SessionErrorCode::UnknownCall,
            "the call is not live on this session",
        )
        .to_text(),
        r#"{"contract":"sipx.app.v1","error":{"code":"unknown_call","message":"the call is not live on this session"},"request":"replace-7"}"#
    );
}

#[test]
fn correlation_is_bounded_and_nonempty() {
    assert!(
        SessionRequest::parse(
            r#"{"contract":"sipx.app.v1","request":"","call":"c","instructions":[]}"#
        )
        .is_err()
    );
    let long = "x".repeat(129);
    let text =
        format!(r#"{{"contract":"sipx.app.v1","request":"{long}","call":"c","instructions":[]}}"#);
    assert!(SessionRequest::parse(&text).is_err());
    assert_eq!(
        SessionRequest::correlation_from_text(
            r#"{"contract":"sipx.app.v1","request":"kept","do":"unknown"}"#
        ),
        Some("kept".to_owned())
    );
}
