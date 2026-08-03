//! User-agent retargeting with RFC 7044 diversion history.

use bytes::Bytes;
use sipx_sip::{
    BuildError, Header, HeaderError, HeaderName, HistoryInfo, ReasonValue, Request,
    TargetChangeKind, Uri,
};
use thiserror::Error;

/// Why a request could not be safely retargeted.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RetargetError {
    /// The received History-Info cache is malformed.
    #[error("cannot read diversion history: {0}")]
    History(#[from] HeaderError),
    /// A generated header could not be represented safely.
    #[error("cannot build retargeted request: {0}")]
    Build(#[from] BuildError),
}

/// Retarget a request and extend its diversion history.
///
/// This is the UA operation from RFC 7044 §§9.1-9.2. It retains every received entry, exposes a
/// missing previous target with a `.0` index, embeds the reason in the previous SIP/SIPS URI,
/// appends the new target at `.1`, and applies history privacy before the request is emitted.
pub fn retarget(
    request: &Request,
    next: Uri,
    reason: &ReasonValue,
    kind: TargetChangeKind,
) -> Result<Request, RetargetError> {
    let history = HistoryInfo::from_headers(&request.headers)
        .transpose()?
        .unwrap_or_default()
        .retargeted(request.uri.clone(), next.clone(), reason, kind);
    let mut history = history;
    history.apply_message_privacy(&request.headers)?;

    let mut retargeted = request.clone();
    retargeted.set_uri(next);
    retargeted.headers.remove_all(&HeaderName::HistoryInfo);
    retargeted
        .headers
        .push(Header::build(HeaderName::HistoryInfo, history.to_bytes())?);
    let advertises_history = retargeted
        .headers
        .typed_all::<sipx_sip::headers::Supported>()
        .filter_map(Result::ok)
        .any(|tags| tags.contains("histinfo"));
    if !advertises_history {
        retargeted.headers.push(Header::build(
            HeaderName::Supported,
            Bytes::from_static(b"histinfo"),
        )?);
    }
    Ok(retargeted)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use sipx_sip::{Limits, StatusCode, parse_datagram};

    #[test]
    fn a_retargeted_request_carries_the_previous_target_and_the_reason_it_moved() {
        let request = parse_datagram(
            Bytes::from_static(
                b"INVITE sip:alice@example.test SIP/2.0\r\n\
                  Supported: timer\r\n\
                  Content-Length: 0\r\n\r\n",
            ),
            &Limits::datagram(),
        )
        .unwrap()
        .as_request()
        .unwrap()
        .clone();
        let next = Uri::parse(Bytes::from_static(b"sip:bob@example.test")).unwrap();
        let moved = retarget(
            &request,
            next,
            &ReasonValue::sip(StatusCode::new(302).unwrap(), None),
            TargetChangeKind::Mp,
        )
        .unwrap();

        assert_eq!(
            moved.uri.to_bytes(),
            Bytes::from_static(b"sip:bob@example.test")
        );
        assert_eq!(
            moved.headers.value(&HeaderName::HistoryInfo).as_deref(),
            Some(
                &b"<sip:alice@example.test?Reason=SIP%3Bcause%3D302>;index=1, <sip:bob@example.test>;index=1.1;mp=1"[..]
            )
        );
        let mut wire = Vec::new();
        moved.write_to(&mut wire);
        assert!(
            String::from_utf8_lossy(&wire).starts_with("INVITE sip:bob@example.test SIP/2.0\r\n")
        );
    }

    #[test]
    fn retargeting_applies_message_history_privacy() {
        let request = parse_datagram(
            Bytes::from_static(
                b"INVITE sip:alice@example.test SIP/2.0\r\n\
                  Privacy: history\r\n\
                  Content-Length: 0\r\n\r\n",
            ),
            &Limits::datagram(),
        )
        .unwrap()
        .as_request()
        .unwrap()
        .clone();
        let next = Uri::parse(Bytes::from_static(b"sip:bob@example.test")).unwrap();
        let moved = retarget(
            &request,
            next,
            &ReasonValue::sip(StatusCode::new(302).unwrap(), None),
            TargetChangeKind::Mp,
        )
        .unwrap();
        assert_eq!(
            moved.headers.value(&HeaderName::HistoryInfo).as_deref(),
            Some(
                &b"<sip:anonymous@anonymous.invalid>;index=1, <sip:anonymous@anonymous.invalid>;index=1.1;mp=1"[..]
            )
        );
    }
}
