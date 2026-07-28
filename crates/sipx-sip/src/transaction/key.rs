//! Transaction matching keys (RFC 3261 §17.1.3, §17.2.3).
//!
//! Matching decides which transaction a message belongs to, and getting it wrong is not a
//! subtle failure: a response matched to the wrong transaction answers the wrong request.
//!
//! There are two schemes. Senders that follow RFC 3261 put a magic cookie at the front of the
//! `Via` `branch`, and the key is essentially that branch. Senders that predate it — still
//! present on the public internet, and represented in the RFC 4475 corpus — do not, and the
//! key has to be reconstructed from six other fields. The magic cookie is what tells the two
//! apart, so its absence selects the fallback rather than causing a rejection.

use crate::headers::{CSeq, Via};
use crate::message::{Method, Request, Response};
use crate::name::HeaderName;

/// A key identifying a transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TransactionKey {
    /// RFC 3261 matching: the branch carries the magic cookie.
    Rfc3261 {
        /// The `branch` parameter of the topmost `Via`.
        branch: Vec<u8>,
        /// The topmost `Via`'s sent-by, lowercased.
        sent_by: Vec<u8>,
        /// The method, with `ACK` folded to `INVITE`.
        method: Vec<u8>,
    },
    /// RFC 2543 matching, reconstructed from the fields that were available before `branch`
    /// meant anything.
    Legacy {
        /// The Request-URI, for a request; empty for a response.
        request_uri: Vec<u8>,
        /// The topmost `Via`, verbatim.
        top_via: Vec<u8>,
        /// The `From` tag.
        from_tag: Vec<u8>,
        /// The `Call-ID`.
        call_id: Vec<u8>,
        /// The `CSeq` number.
        cseq: u32,
        /// The method, with `ACK` folded to `INVITE`.
        method: Vec<u8>,
    },
}

/// An ACK is matched to the INVITE it acknowledges, so the two share a key.
///
/// **CANCEL is not.** It carries the branch of the request it cancels (RFC 3261 §9.1), which
/// is how it names its target — but RFC 3261 §17.2.3 folds the method only for ACK, so a
/// CANCEL runs in a transaction of its own. Folding it too makes a received CANCEL look like a
/// retransmitted INVITE: it is absorbed, nobody is told, and the callee goes on ringing.
/// [`TransactionKey::for_cancelled_invite`] is how the INVITE is found instead.
fn match_method(method: &Method) -> Vec<u8> {
    match method {
        Method::Ack => Method::Invite.as_bytes().to_vec(),
        other => other.as_bytes().to_vec(),
    }
}

fn sent_by(via: &Via) -> Vec<u8> {
    let mut out = via.host.to_bytes().to_ascii_lowercase();
    if let Some(port) = via.port {
        out.push(b':');
        out.extend_from_slice(port.to_string().as_bytes());
    }
    out
}

impl TransactionKey {
    /// The key a received request belongs to (RFC 3261 §17.2.3).
    ///
    /// Returns `None` if the request has no usable `Via`, which is not a transaction question
    /// — a request without a `Via` cannot be answered at all, and validation reports it.
    #[must_use]
    pub fn from_request(request: &Request) -> Option<Self> {
        let via = request.headers.typed::<Via>()?.ok()?;
        let method = match_method(&request.method);

        if via.has_rfc3261_branch() {
            return Some(Self::Rfc3261 {
                branch: via.branch()?.to_vec(),
                sent_by: sent_by(&via),
                method,
            });
        }

        let cseq = request.headers.typed::<CSeq>()?.ok()?;
        Some(Self::Legacy {
            request_uri: request.uri.to_bytes().to_vec(),
            top_via: request
                .headers
                .value(&HeaderName::Via)
                .map(|v| v.to_vec())
                .unwrap_or_default(),
            from_tag: request
                .headers
                .typed::<crate::headers::From>()
                .and_then(Result::ok)
                .and_then(|f| f.tag().map(<[u8]>::to_vec))
                .unwrap_or_default(),
            call_id: request
                .headers
                .value(&HeaderName::CallId)
                .map(|v| v.to_vec())
                .unwrap_or_default(),
            cseq: cseq.sequence,
            method,
        })
    }

    /// The key a request sent by this element belongs to (RFC 3261 §17.1.3).
    ///
    /// The same derivation as [`Self::from_request`]: a client transaction has to be findable
    /// by the key its responses will produce.
    #[must_use]
    pub fn from_sent_request(request: &Request) -> Option<Self> {
        Self::from_request(request)
    }

    /// The key a received response belongs to (RFC 3261 §17.1.3).
    ///
    /// The branch of the topmost `Via` plus the `CSeq` method — and *only* those two. In
    /// particular the sent-by is not part of it, because the response may come back from a
    /// different address than the request went to.
    #[must_use]
    pub fn from_response(response: &Response) -> Option<Self> {
        let via = response.headers.typed::<Via>()?.ok()?;
        let cseq = response.headers.typed::<CSeq>()?.ok()?;
        let method = match_method(&cseq.method);

        if via.has_rfc3261_branch() {
            return Some(Self::Rfc3261 {
                branch: via.branch()?.to_vec(),
                sent_by: sent_by(&via),
                method,
            });
        }

        Some(Self::Legacy {
            request_uri: Vec::new(),
            top_via: response
                .headers
                .value(&HeaderName::Via)
                .map(|v| v.to_vec())
                .unwrap_or_default(),
            from_tag: response
                .headers
                .typed::<crate::headers::From>()
                .and_then(Result::ok)
                .and_then(|f| f.tag().map(<[u8]>::to_vec))
                .unwrap_or_default(),
            call_id: response
                .headers
                .value(&HeaderName::CallId)
                .map(|v| v.to_vec())
                .unwrap_or_default(),
            cseq: cseq.sequence,
            method,
        })
    }

    /// The key of the INVITE a CANCEL refers to (RFC 3261 §9.2).
    ///
    /// A CANCEL shares the INVITE's branch and differs only in method, so this is that key
    /// with the method put back. Returns `None` for anything that is not a CANCEL, since no
    /// other request cancels one.
    #[must_use]
    pub fn for_cancelled_invite(request: &Request) -> Option<Self> {
        if request.method != Method::Cancel {
            return None;
        }
        let key = Self::from_request(request)?;
        Some(match key {
            Self::Rfc3261 {
                branch, sent_by, ..
            } => Self::Rfc3261 {
                branch,
                sent_by,
                method: Method::Invite.as_bytes().to_vec(),
            },
            Self::Legacy {
                request_uri,
                top_via,
                from_tag,
                call_id,
                cseq,
                ..
            } => Self::Legacy {
                request_uri,
                top_via,
                from_tag,
                call_id,
                cseq,
                method: Method::Invite.as_bytes().to_vec(),
            },
        })
    }

    /// Whether this key was derived by the pre-RFC-3261 rules.
    #[must_use]
    pub fn is_legacy(&self) -> bool {
        matches!(self, Self::Legacy { .. })
    }
}
