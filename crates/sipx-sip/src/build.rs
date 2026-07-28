//! Building messages.
//!
//! The threat this module exists for is header injection: a caller puts a user-supplied
//! string into a display name or a `Call-ID`, the string contains CRLF, and one header becomes
//! three — or a body becomes a second request. Every SIP stack has a story about this.
//!
//! The usual defence is a `validate()` function the caller is supposed to remember. sipx does
//! not have one, because "supposed to remember" is not a security property. Instead there is
//! **no way to build a message from unvalidated bytes**: every constructor here is fallible
//! and checks its input, and the only unchecked path into a [`Header`] is the parser's, which
//! is crate-private and operates on bytes that were already framed.
//!
//! The test that keeps this true is table-driven over every field a caller can populate, so a
//! newly added field with no guard fails it.

use bytes::Bytes;

use crate::error::BuildError;
use crate::headers::grammar::is_token_char;
use crate::message::{Header, Headers, Method, Request, Response, StatusCode};
use crate::name::HeaderName;
use crate::uri::Uri;

/// Reject anything that could end a line or terminate a C string.
///
/// CR and LF are the injection vector. NUL is here because it is the classic way to smuggle
/// a value past a length-agnostic consumer further down the chain, and nothing in SIP needs
/// one unescaped.
pub(crate) fn check_value(value: &[u8], field: &'static str) -> Result<(), BuildError> {
    if let Some(pos) = value
        .iter()
        .position(|&b| matches!(b, b'\r' | b'\n' | b'\0'))
    {
        return Err(BuildError::IllegalCharacter {
            field,
            offset: pos,
            byte: value.get(pos).copied().unwrap_or(0),
        });
    }
    Ok(())
}

/// Reject anything that is not a single `token`.
pub(crate) fn check_token(value: &[u8], field: &'static str) -> Result<(), BuildError> {
    check_value(value, field)?;
    if value.is_empty() || !value.iter().all(|&b| is_token_char(b)) {
        return Err(BuildError::NotAToken { field });
    }
    Ok(())
}

impl Header {
    /// Build a header, rejecting a value that could inject a line break.
    ///
    /// This is the only public way to make a header, and it is fallible on purpose.
    pub fn build(name: HeaderName, value: impl Into<Bytes>) -> Result<Self, BuildError> {
        let value = value.into();
        check_value(&value, "header value")?;
        if let HeaderName::Other(raw) = &name {
            check_token(raw, "header name")?;
        }
        Ok(Self::new_unchecked(name, value))
    }
}

/// Builds a request.
///
/// ```
/// # use sipx_sip::{build::RequestBuilder, Method, Uri, Host, HostName, HeaderName};
/// let uri = Uri::sip(Host::Name(HostName::new("example.com")?));
/// let request = RequestBuilder::new(Method::Options, uri)
///     .header(HeaderName::CallId, "abc123@example.com")?
///     .max_forwards(70)
///     .build();
/// # Ok::<(), sipx_sip::error::BuildError>(())
/// ```
#[derive(Debug)]
pub struct RequestBuilder {
    method: Method,
    uri: Uri,
    headers: Headers,
    body: Bytes,
}

impl RequestBuilder {
    /// Start a request.
    #[must_use]
    pub fn new(method: Method, uri: Uri) -> Self {
        Self {
            method,
            uri,
            headers: Headers::new(),
            body: Bytes::new(),
        }
    }

    /// Append a header, rejecting a value that could inject a line break.
    pub fn header(mut self, name: HeaderName, value: impl Into<Bytes>) -> Result<Self, BuildError> {
        self.headers.push(Header::build(name, value)?);
        Ok(self)
    }

    /// Replace every header of this name with one carrying this value.
    ///
    /// Distinct from [`Self::header`] because appending is right for `Via` and `Route`, where
    /// repetition is meaningful, and wrong for `To` or `CSeq`, where a second copy makes the
    /// message invalid.
    pub fn set_header(
        mut self,
        name: &HeaderName,
        value: impl Into<Bytes>,
    ) -> Result<Self, BuildError> {
        let header = Header::build(name.clone(), value)?;
        self.headers.remove_all(name);
        self.headers.push(header);
        Ok(self)
    }

    /// Append a `Max-Forwards` header. Cannot fail: a `u8` has no CRLF in it.
    #[must_use]
    pub fn max_forwards(mut self, hops: u8) -> Self {
        self.headers.push(Header::new_unchecked(
            HeaderName::MaxForwards,
            Bytes::from(hops.to_string()),
        ));
        self
    }

    /// Append a `CSeq` header.
    pub fn cseq(mut self, sequence: u32, method: &Method) -> Result<Self, BuildError> {
        check_token(method.as_bytes(), "CSeq method")?;
        let mut value = sequence.to_string().into_bytes();
        value.push(b' ');
        value.extend_from_slice(method.as_bytes());
        self.headers
            .push(Header::new_unchecked(HeaderName::CSeq, Bytes::from(value)));
        Ok(self)
    }

    /// Set the body, and the `Content-Length` that goes with it.
    ///
    /// The two are set together because a body without a matching length is a framing bug
    /// waiting to happen, and there is no reason to let a caller create one.
    #[must_use]
    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        let body = body.into();
        self.headers.remove_all(&HeaderName::ContentLength);
        self.headers.push(Header::new_unchecked(
            HeaderName::ContentLength,
            Bytes::from(body.len().to_string()),
        ));
        self.body = body;
        self
    }

    /// Finish.
    ///
    /// If no body was set, a `Content-Length: 0` is added, because a stream transport cannot
    /// frame a message without one.
    #[must_use]
    pub fn build(mut self) -> Request {
        if self.headers.get(&HeaderName::ContentLength).is_none() {
            self.headers.push(Header::new_unchecked(
                HeaderName::ContentLength,
                Bytes::from_static(b"0"),
            ));
        }
        let mut request = Request::new(self.method, self.uri);
        request.headers = self.headers;
        request.set_body(self.body);
        request
    }
}

/// Builds a response.
#[derive(Debug)]
pub struct ResponseBuilder {
    status: StatusCode,
    reason: Bytes,
    headers: Headers,
    body: Bytes,
}

impl ResponseBuilder {
    /// Start a response.
    ///
    /// The reason phrase is checked: it is the one field on a start line that carries free
    /// text, which makes it the obvious place to try to inject a line break.
    pub fn new(status: StatusCode, reason: impl Into<Bytes>) -> Result<Self, BuildError> {
        let reason = reason.into();
        check_value(&reason, "reason phrase")?;
        Ok(Self {
            status,
            reason,
            headers: Headers::new(),
            body: Bytes::new(),
        })
    }

    /// Start a response to a request, copying the headers RFC 3261 §8.2.6.2 requires.
    ///
    /// `Via` is copied **in order and in full**: the response finds its way back by walking
    /// that list, and reordering or deduplicating it strands the response. `From`, `To`,
    /// `Call-ID` and `CSeq` are copied verbatim, which also means a request whose `To` was
    /// unparseable still gets a well-formed response — the point of copying rather than
    /// re-deriving.
    pub fn to_request(
        request: &Request,
        status: StatusCode,
        reason: impl Into<Bytes>,
    ) -> Result<Self, BuildError> {
        let mut builder = Self::new(status, reason)?;
        for name in [
            HeaderName::Via,
            HeaderName::From,
            HeaderName::To,
            HeaderName::CallId,
            HeaderName::CSeq,
        ] {
            for header in request.headers.get_all(&name) {
                builder.headers.push(header.clone());
            }
        }
        Ok(builder)
    }

    /// Append a header, rejecting a value that could inject a line break.
    pub fn header(mut self, name: HeaderName, value: impl Into<Bytes>) -> Result<Self, BuildError> {
        self.headers.push(Header::build(name, value)?);
        Ok(self)
    }

    /// Replace every header of this name with one carrying this value.
    pub fn set_header(
        mut self,
        name: &HeaderName,
        value: impl Into<Bytes>,
    ) -> Result<Self, BuildError> {
        let header = Header::build(name.clone(), value)?;
        self.headers.remove_all(name);
        self.headers.push(header);
        Ok(self)
    }

    /// Set the body and its `Content-Length`.
    #[must_use]
    pub fn body(mut self, body: impl Into<Bytes>) -> Self {
        let body = body.into();
        self.headers.remove_all(&HeaderName::ContentLength);
        self.headers.push(Header::new_unchecked(
            HeaderName::ContentLength,
            Bytes::from(body.len().to_string()),
        ));
        self.body = body;
        self
    }

    /// Finish.
    #[must_use]
    pub fn build(mut self) -> Response {
        if self.headers.get(&HeaderName::ContentLength).is_none() {
            self.headers.push(Header::new_unchecked(
                HeaderName::ContentLength,
                Bytes::from_static(b"0"),
            ));
        }
        let mut response = Response::new(self.status, self.reason);
        response.headers = self.headers;
        response.set_body(self.body);
        response
    }
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
    use crate::headers::CSeq;
    use crate::uri::{Host, HostName};

    fn uri() -> Uri {
        Uri::sip(Host::Name(
            HostName::new(Bytes::from_static(b"example.com")).expect("a valid host"),
        ))
    }

    /// Every field a caller can populate, in one table.
    ///
    /// The table is the point. Guarding today's fields is easy; the failure mode is a field
    /// added next year with no guard, and this test fails the moment one appears — provided
    /// it is added here, which the module documentation asks for.
    #[test]
    fn crlf_injection_rejected_in_every_user_supplied_field() {
        // Each payload ends a header line early and starts a forged one.
        let payloads: &[&[u8]] = &[
            b"value\r\nInjected: yes",
            b"value\rInjected: yes",
            b"value\nInjected: yes",
            b"value\0truncated",
            b"\r\n",
            b"\n\n",
        ];

        for payload in payloads {
            let p = Bytes::copy_from_slice(payload);

            // A header value.
            assert!(
                Header::build(HeaderName::Subject, p.clone()).is_err(),
                "header value accepted {payload:?}"
            );
            // An unknown header's *name*.
            assert!(
                Header::build(HeaderName::Other(p.clone()), Bytes::from_static(b"x")).is_err(),
                "header name accepted {payload:?}"
            );
            // A request header, through the builder.
            assert!(
                RequestBuilder::new(Method::Options, uri())
                    .header(HeaderName::CallId, p.clone())
                    .is_err(),
                "request builder accepted {payload:?}"
            );
            // A response reason phrase — free text on the start line.
            assert!(
                ResponseBuilder::new(StatusCode::new(200).unwrap(), p.clone()).is_err(),
                "reason phrase accepted {payload:?}"
            );
            // A response header.
            assert!(
                ResponseBuilder::new(StatusCode::new(200).unwrap(), "OK")
                    .unwrap()
                    .header(HeaderName::Server, p.clone())
                    .is_err(),
                "response builder accepted {payload:?}"
            );
            // A CSeq method.
            assert!(
                RequestBuilder::new(Method::Options, uri())
                    .cseq(1, &Method::Other(p.clone()))
                    .is_err(),
                "CSeq method accepted {payload:?}"
            );
            // A hostname, which reaches the wire inside the Request-URI. This is the field
            // that made HostName a newtype with a private interior: a CRLF here forges an
            // entire request line, not merely a header.
            assert!(
                HostName::new(p.clone()).is_err(),
                "host name accepted {payload:?}"
            );
        }
    }

    /// A hostname must be a hostname, not merely free of line breaks.
    #[test]
    fn host_names_are_validated_not_just_screened() {
        assert!(HostName::new("example.com").is_ok());
        assert!(HostName::new("host-5.sub.example.com").is_ok());
        for bad in [
            "",
            "exa mple.com",
            "host@evil.com",
            "host;lr",
            "<host>",
            "host/path",
        ] {
            assert!(HostName::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn a_built_request_frames_correctly() {
        let request = RequestBuilder::new(Method::Options, uri())
            .header(HeaderName::CallId, "abc@example.com")
            .unwrap()
            .cseq(1, &Method::Options)
            .unwrap()
            .max_forwards(70)
            .build();

        let mut out = Vec::new();
        request.write_to(&mut out);
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("OPTIONS sip:example.com SIP/2.0\r\n"));
        assert!(text.contains("CSeq: 1 OPTIONS\r\n"));
        // Content-Length is added even with no body, because a stream cannot frame without it.
        assert!(text.contains("Content-Length: 0\r\n"));
        assert!(text.ends_with("\r\n\r\n"));
    }

    #[test]
    fn setting_a_body_sets_the_matching_content_length() {
        let request = RequestBuilder::new(Method::Options, uri())
            .body(Bytes::from_static(b"hello"))
            .build();
        assert_eq!(request.body().len(), 5);
        assert_eq!(
            request
                .headers
                .value(&HeaderName::ContentLength)
                .as_deref()
                .map(<[u8]>::to_vec),
            Some(b"5".to_vec())
        );

        // Replacing the body replaces the length rather than adding a second one, which would
        // be an unframeable message.
        let request = RequestBuilder::new(Method::Options, uri())
            .body(Bytes::from_static(b"hello"))
            .body(Bytes::from_static(b"hi"))
            .build();
        assert_eq!(request.headers.count(&HeaderName::ContentLength), 1);
        assert_eq!(
            request
                .headers
                .value(&HeaderName::ContentLength)
                .as_deref()
                .map(<[u8]>::to_vec),
            Some(b"2".to_vec())
        );
    }

    #[test]
    fn a_response_copies_the_via_stack_in_order() {
        let request = RequestBuilder::new(Method::Invite, uri())
            .header(HeaderName::Via, "SIP/2.0/UDP first;branch=z9hG4bK1")
            .unwrap()
            .header(HeaderName::Via, "SIP/2.0/UDP second;branch=z9hG4bK2")
            .unwrap()
            .header(HeaderName::From, "<sip:a@b>;tag=1")
            .unwrap()
            .header(HeaderName::To, "<sip:c@d>")
            .unwrap()
            .header(HeaderName::CallId, "x@y")
            .unwrap()
            .cseq(7, &Method::Invite)
            .unwrap()
            .build();

        let response =
            ResponseBuilder::to_request(&request, StatusCode::new(180).unwrap(), "Ringing")
                .unwrap()
                .build();

        let vias: Vec<_> = response
            .headers
            .get_all(&HeaderName::Via)
            .map(|h| h.value().to_vec())
            .collect();
        assert_eq!(
            vias,
            vec![
                b"SIP/2.0/UDP first;branch=z9hG4bK1".to_vec(),
                b"SIP/2.0/UDP second;branch=z9hG4bK2".to_vec(),
            ],
            "the Via stack must be copied in order; a response walks it back"
        );
        assert_eq!(
            response.headers.typed::<CSeq>().and_then(Result::ok),
            Some(CSeq {
                sequence: 7,
                method: Method::Invite
            })
        );
    }

    /// A request whose `To` cannot be parsed still deserves a well-formed 400. Copying the
    /// header bytes rather than re-deriving them is what makes that possible.
    #[test]
    fn a_response_can_be_built_for_a_request_with_an_unparseable_header() {
        use crate::{Limits, parse_datagram};
        let text = "OPTIONS sip:a@b.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP h;branch=z9hG4bKx\r\n\
             To: \"unterminated <sip:a@b.com>\r\n\
             From: <sip:c@d>;tag=1\r\n\
             Call-ID: x@y\r\n\
             CSeq: 1 OPTIONS\r\n\
             Content-Length: 0\r\n\r\n";
        let msg = parse_datagram(Bytes::from(text), &Limits::datagram()).expect("frames");
        let request = msg.as_request().expect("a request");
        assert!(
            request
                .headers
                .typed::<crate::headers::To>()
                .is_some_and(|r| r.is_err())
        );

        let response =
            ResponseBuilder::to_request(request, StatusCode::new(400).unwrap(), "Bad Request")
                .expect("must still build")
                .build();
        let mut out = Vec::new();
        response.write_to(&mut out);
        assert!(String::from_utf8_lossy(&out).starts_with("SIP/2.0 400 Bad Request\r\n"));
    }
}
