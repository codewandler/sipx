//! Constructing the requests and responses the kernel owes.
//!
//! `sipx-sip` supplies the injection-safe [`RequestBuilder`]/[`ResponseBuilder`] and the §17
//! transaction machines; the method-shaped builders above them live in `sipx-call`, which is
//! tokio-bound and therefore out of reach here. This module is that missing half, written for the
//! one vocabulary `docs/specs/browser-sdk.md` §5.2 admits and nothing wider.
//!
//! Everything is RFC 7118-shaped: the transport token is `WS`/`WSS`, the client's `Contact` is
//! deliberately unroutable, and one SIP message is one WebSocket message.

use bytes::Bytes;
use sipx_sip::error::BuildError;
use sipx_sip::headers::{ContactValue, To};
use sipx_sip::{HeaderName, Method, Request, RequestBuilder, Response, StatusCode, Uri};

use crate::config::Config;

/// RFC 3261 §8.1.1.6's recommended starting value.
const MAX_FORWARDS: u8 = 70;

/// The dialog identity the kernel carries for a registration or a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dialog {
    /// The Call-ID, derived from the entropy tape (§4.7) with no `@host` part.
    pub(crate) call_id: String,
    /// This endpoint's tag.
    pub(crate) local_tag: String,
    /// The peer's tag, once a response or request has carried one.
    pub(crate) remote_tag: Option<String>,
    /// The local address of record.
    pub(crate) local_uri: String,
    /// The peer's address of record.
    pub(crate) remote_uri: String,
    /// Where in-dialog requests go: the peer's `Contact`, once it has sent one.
    pub(crate) remote_target: Option<String>,
    /// The local sequence number; the next request uses this value incremented.
    pub(crate) local_cseq: u32,
}

impl Dialog {
    /// The `From` header value this side sends.
    fn local_address(&self) -> String {
        format!("<{}>;tag={}", self.local_uri, self.local_tag)
    }

    /// The `To` header value this side sends, carrying the peer's tag once it is known.
    fn remote_address(&self) -> String {
        match &self.remote_tag {
            Some(tag) => format!("<{}>;tag={tag}", self.remote_uri),
            None => format!("<{}>", self.remote_uri),
        }
    }

    /// The Request-URI for an in-dialog request: the peer's `Contact` when it gave one.
    fn target(&self) -> &str {
        self.remote_target.as_deref().unwrap_or(&self.remote_uri)
    }
}

/// The unroutable `Contact` RFC 7118 §5.2 requires of a WebSocket client.
///
/// The instance host is derived from the dialog's Call-ID rather than drawn from the entropy
/// tape. §4.7 enumerates the four identifiers that consume octets and this is not one of them, so
/// taking more would put `BSDK-ENT-1`'s accounting out by sixteen and make a pinned tape yield
/// unpinned identifiers. The Call-ID is already 128 bits of the same tape, and a `.invalid` host
/// is never resolved by anyone — its whole job is to be unique and unreachable.
pub(crate) fn contact_uri(config: &Config, dialog: &Dialog) -> String {
    let instance = dialog.call_id.get(..16).unwrap_or(&dialog.call_id);
    // RFC 7118 §5.2's parameter is `transport=ws` on both schemes: the URI parameter names the
    // WebSocket transport, and TLS is the `wss` scheme's business on the connection, not a
    // second token here.
    format!("sip:{}@{instance}.invalid;transport=ws", config.aor_user())
}

/// RFC 7118 §5's Via transport token.
fn via_transport(config: &Config) -> &'static str {
    if config.transport.scheme == "wss" {
        "WSS"
    } else {
        "WS"
    }
}

/// The Via this endpoint sends: an unroutable sent-by and the branch drawn for this transaction.
fn via_value(config: &Config, dialog: &Dialog, branch: &str) -> String {
    let instance = dialog.call_id.get(..16).unwrap_or(&dialog.call_id);
    format!(
        "SIP/2.0/{} {instance}.invalid;branch={branch}",
        via_transport(config)
    )
}

fn uri(raw: &str) -> Result<Uri, BuildError> {
    Uri::parse(Bytes::from(raw.to_owned().into_bytes())).map_err(|_| BuildError::NotAToken {
        field: "request-uri",
    })
}

/// The common skeleton: `Via`, `Max-Forwards`, `From`, `To`, `Call-ID`, `CSeq`.
fn skeleton(
    config: &Config,
    dialog: &Dialog,
    method: &Method,
    request_uri: &str,
    branch: &str,
    cseq: u32,
) -> Result<RequestBuilder, BuildError> {
    RequestBuilder::new(method.clone(), uri(request_uri)?)
        .header(HeaderName::Via, via_value(config, dialog, branch))?
        .max_forwards(MAX_FORWARDS)
        .header(HeaderName::From, dialog.local_address())?
        .header(HeaderName::To, dialog.remote_address())?
        .header(HeaderName::CallId, dialog.call_id.clone())?
        .cseq(cseq, method)
}

/// A REGISTER, with the AOR's domain as the Request-URI (RFC 3261 §10.2).
pub(crate) fn register(
    config: &Config,
    dialog: &Dialog,
    branch: &str,
    cseq: u32,
    expires: u32,
) -> Result<Request, BuildError> {
    let request_uri = format!("sip:{}", config.aor_domain());
    Ok(skeleton(
        config,
        dialog,
        &Method::Register,
        &request_uri,
        branch,
        cseq,
    )?
    .header(
        HeaderName::Contact,
        format!("<{}>", contact_uri(config, dialog)),
    )?
    .header(HeaderName::Expires, expires.to_string())?
    .build())
}

/// An INVITE carrying the browser's offer.
pub(crate) fn invite(
    config: &Config,
    dialog: &Dialog,
    branch: &str,
    cseq: u32,
    sdp: &str,
) -> Result<Request, BuildError> {
    Ok(skeleton(
        config,
        dialog,
        &Method::Invite,
        &dialog.remote_uri,
        branch,
        cseq,
    )?
    .header(
        HeaderName::Contact,
        format!("<{}>", contact_uri(config, dialog)),
    )?
    .header(HeaderName::ContentType, "application/sdp")?
    .body(sdp.to_owned())
    .build())
}

/// The ACK for a 2xx, which RFC 3261 §13.2.2.4 makes the transaction user's own request.
pub(crate) fn ack(
    config: &Config,
    dialog: &Dialog,
    branch: &str,
    cseq: u32,
) -> Result<Request, BuildError> {
    Ok(skeleton(config, dialog, &Method::Ack, dialog.target(), branch, cseq)?.build())
}

/// A BYE.
pub(crate) fn bye(
    config: &Config,
    dialog: &Dialog,
    branch: &str,
    cseq: u32,
) -> Result<Request, BuildError> {
    Ok(skeleton(config, dialog, &Method::Bye, dialog.target(), branch, cseq)?.build())
}

/// A CANCEL.
///
/// RFC 3261 §9.1: it reuses the cancelled INVITE's Request-URI, Call-ID, From, To, sequence
/// number and — crucially — its **top Via branch**, which is why cancelling costs no entropy.
pub(crate) fn cancel(
    config: &Config,
    dialog: &Dialog,
    invite_branch: &str,
    cseq: u32,
) -> Result<Request, BuildError> {
    Ok(skeleton(
        config,
        dialog,
        &Method::Cancel,
        &dialog.remote_uri,
        invite_branch,
        cseq,
    )?
    .build())
}

/// A response to a received request, with this endpoint's tag on the `To` header.
///
/// `ResponseBuilder::to_request` copies `Via` in order and in full, plus `From`, `To`, `Call-ID`
/// and `CSeq`; the local tag is written over the copied `To` because a dialog-forming response
/// has to carry one.
pub(crate) fn respond(
    request: &Request,
    status: u16,
    reason: &str,
    local_tag: Option<&str>,
    contact: Option<&str>,
    sdp: Option<&str>,
) -> Result<Response, BuildError> {
    let code = StatusCode::new(status).ok_or(BuildError::NotAToken { field: "status" })?;
    let mut builder = sipx_sip::ResponseBuilder::to_request(request, code, reason.to_owned())?;
    if let Some(tag) = local_tag {
        let to = request
            .headers
            .value(&HeaderName::To)
            .map(|value| String::from_utf8_lossy(&value).into_owned())
            .ok_or(BuildError::MissingRequiredResponseHeader { header: "To" })?;
        if !to.contains(";tag=") {
            builder = builder.set_header(&HeaderName::To, format!("{to};tag={tag}"))?;
        }
    }
    if let Some(contact) = contact {
        builder = builder.header(HeaderName::Contact, format!("<{contact}>"))?;
    }
    if let Some(sdp) = sdp {
        builder = builder
            .header(HeaderName::ContentType, "application/sdp")?
            .body(sdp.to_owned());
    }
    Ok(builder.build())
}

/// The tag on a message's `To` header, when it carries one.
pub(crate) fn to_tag(headers: &sipx_sip::Headers) -> Option<String> {
    let to = headers.typed::<To>()?.ok()?;
    to.0.tag()
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
}

/// The tag on a message's `From` header, when it carries one.
pub(crate) fn from_tag(headers: &sipx_sip::Headers) -> Option<String> {
    let from = headers.typed::<sipx_sip::headers::From>()?.ok()?;
    from.0
        .tag()
        .map(|tag| String::from_utf8_lossy(tag).into_owned())
}

/// A message's `Contact` URI, which becomes the dialog's remote target.
pub(crate) fn contact_target(headers: &sipx_sip::Headers) -> Option<String> {
    let raw = headers.value(&HeaderName::Contact)?;
    let values = <ContactValue as sipx_sip::TypedHeader>::decode_list(&raw).ok()?;
    values.into_iter().find_map(|value| match value {
        ContactValue::Address(address) => {
            Some(String::from_utf8_lossy(&address.uri.to_bytes()).into_owned())
        }
        ContactValue::Wildcard => None,
    })
}

/// The `Call-ID` of a message, as a string.
pub(crate) fn call_id(headers: &sipx_sip::Headers) -> Option<String> {
    headers
        .value(&HeaderName::CallId)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
}

/// The top `Via` branch of a request.
pub(crate) fn top_branch(headers: &sipx_sip::Headers) -> Option<String> {
    let via = headers.typed::<sipx_sip::Via>()?.ok()?;
    via.branch()
        .map(|branch| String::from_utf8_lossy(branch).into_owned())
}

/// The `CSeq` sequence number of a message.
pub(crate) fn cseq(headers: &sipx_sip::Headers) -> Option<u32> {
    Some(headers.typed::<sipx_sip::CSeq>()?.ok()?.sequence)
}

/// The `application/sdp` body of a message, when it has one.
pub(crate) fn sdp_body(headers: &sipx_sip::Headers, body: &Bytes) -> Option<String> {
    let content_type = headers.typed::<sipx_sip::headers::ContentType>()?.ok()?;
    if !content_type.is("application", "sdp") {
        return None;
    }
    core::str::from_utf8(body).ok().map(str::to_owned)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config::parse(
            br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}"#,
        )
        .expect("BSDK-CFG-1")
    }

    fn dialog() -> Dialog {
        Dialog {
            call_id: "000102030405060708090a0b0c0d0e0f".to_owned(),
            local_tag: "1011121314151617".to_owned(),
            remote_tag: None,
            local_uri: "sip:alice@example.net".to_owned(),
            remote_uri: "sip:alice@example.net".to_owned(),
            remote_target: None,
            local_cseq: 1,
        }
    }

    fn rendered(request: &Request) -> String {
        let mut out = Vec::new();
        request.write_to(&mut out);
        String::from_utf8(out).expect("ASCII")
    }

    #[test]
    fn a_register_carries_the_pinned_identifiers_and_an_unroutable_contact() {
        let request =
            register(&config(), &dialog(), "z9hG4bK18191a1b1c1d1e1f", 1, 600).expect("well-formed");
        let text = rendered(&request);
        assert!(
            text.starts_with("REGISTER sip:example.net SIP/2.0\r\n"),
            "{text}"
        );
        assert!(
            text.contains("Call-ID: 000102030405060708090a0b0c0d0e0f\r\n"),
            "{text}"
        );
        assert!(text.contains(";tag=1011121314151617"), "{text}");
        assert!(text.contains("branch=z9hG4bK18191a1b1c1d1e1f"), "{text}");
        assert!(
            text.contains("SIP/2.0/WSS 0001020304050607.invalid"),
            "{text}"
        );
        assert!(
            text.contains("Contact: <sip:alice@0001020304050607.invalid;transport=ws>"),
            "{text}"
        );
        assert!(text.contains("Expires: 600\r\n"), "{text}");
    }

    #[test]
    fn a_cancel_reuses_the_invites_branch() {
        let request = cancel(&config(), &dialog(), "z9hG4bKdeadbeefdeadbeef", 1).expect("built");
        let text = rendered(&request);
        assert!(text.contains("branch=z9hG4bKdeadbeefdeadbeef"), "{text}");
        assert!(text.starts_with("CANCEL "), "{text}");
    }

    #[test]
    fn an_invite_carries_the_offer_as_an_sdp_body() {
        let mut dialog = dialog();
        dialog.remote_uri = "sip:bob@example.net".to_owned();
        let request =
            invite(&config(), &dialog, "z9hG4bKaaaaaaaaaaaaaaaa", 1, "v=0\r\n").expect("built");
        let text = rendered(&request);
        assert!(
            text.starts_with("INVITE sip:bob@example.net SIP/2.0\r\n"),
            "{text}"
        );
        assert!(text.contains("Content-Type: application/sdp\r\n"), "{text}");
        assert!(text.ends_with("\r\n\r\nv=0\r\n"), "{text}");
    }
}
