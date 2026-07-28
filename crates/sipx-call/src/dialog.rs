//! Dialogs (RFC 3261 §12).
//!
//! A dialog is the shared state two user agents keep between an INVITE and a BYE. Getting it
//! wrong produces calls that establish and then cannot be ended, which is worse than a call
//! that never establishes: the media keeps flowing.
//!
//! The three parts that matter, and the ways each goes wrong:
//!
//! - **The identifier** is `Call-ID` plus both tags. The local and remote tags swap places
//!   depending on which side you are, and a UAS that builds the identifier as though it were
//!   the UAC will fail to match its own dialog's BYE.
//! - **The sequence numbers are independent.** Each side numbers its own requests. Sharing one
//!   counter means the first in-dialog request from each side collides.
//! - **The route set** is the `Record-Route` of the dialog-forming exchange, and it is
//!   *reversed* for a UAC. Sending in the wrong order routes a BYE through the proxies
//!   backwards, and it never arrives.

use bytes::Bytes;
use sipx_sip::headers::{From as FromHeader, To};
use sipx_sip::{HeaderName, Request, Response, Uri};

/// Which side of the dialog this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// We sent the INVITE.
    Caller,
    /// We received it.
    Callee,
}

/// What identifies a dialog. `Call-ID` plus the two tags.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DialogId {
    /// The `Call-ID`.
    pub call_id: Vec<u8>,
    /// Our tag.
    pub local_tag: Vec<u8>,
    /// Their tag.
    pub remote_tag: Vec<u8>,
}

/// A dialog.
#[derive(Debug, Clone)]
pub struct Dialog {
    /// Which side we are.
    pub role: Role,
    /// What identifies it.
    pub id: DialogId,
    /// Our address of record.
    pub local_uri: String,
    /// Theirs.
    pub remote_uri: String,
    /// Where to send in-dialog requests, from their `Contact`.
    pub remote_target: Uri,
    /// Our sequence number for requests we originate. Independent of theirs.
    pub local_cseq: u32,
    /// The highest sequence number we have seen from them.
    pub remote_cseq: Option<u32>,
    /// The route set, already in send order.
    pub route_set: Vec<String>,
}

impl Dialog {
    /// Build the caller's half from the request it sent and the response it got.
    ///
    /// Returns `None` if the response carries no `To` tag, which means no dialog was created —
    /// a 100 Trying, for instance.
    #[must_use]
    pub fn from_response(request: &Request, response: &Response) -> Option<Self> {
        let call_id = response.headers.value(&HeaderName::CallId)?.into_owned();
        let local_tag = tag_of::<FromHeader>(&response.headers)?;
        let remote_tag = tag_of::<To>(&response.headers)?;
        let remote_target = contact_uri(&response.headers)?;

        // RFC 3261 §12.1.2: for a UAC the route set is the Record-Route in *reverse* order.
        // The response lists them outermost-first as seen from the callee; sending needs them
        // in the order the request will traverse.
        let mut route_set = record_routes(&response.headers);
        route_set.reverse();

        Some(Self {
            role: Role::Caller,
            id: DialogId {
                call_id,
                local_tag,
                remote_tag,
            },
            local_uri: header_string(&request.headers, &HeaderName::From),
            remote_uri: header_string(&request.headers, &HeaderName::To),
            remote_target,
            local_cseq: cseq_number(&request.headers).unwrap_or(1),
            remote_cseq: None,
            route_set,
        })
    }

    /// Build the callee's half from the request that created it and the tag we chose.
    #[must_use]
    pub fn from_request(request: &Request, local_tag: &str) -> Option<Self> {
        let call_id = request.headers.value(&HeaderName::CallId)?.into_owned();
        let remote_tag = tag_of::<FromHeader>(&request.headers)?;
        let remote_target = contact_uri(&request.headers)?;

        // For a UAS the order is as received — the mirror of the caller's reversal, and the
        // reason both are written out rather than shared.
        let route_set = record_routes(&request.headers);

        Some(Self {
            role: Role::Callee,
            id: DialogId {
                call_id,
                local_tag: local_tag.as_bytes().to_vec(),
                remote_tag,
            },
            local_uri: header_string(&request.headers, &HeaderName::To),
            remote_uri: header_string(&request.headers, &HeaderName::From),
            remote_target,
            // Our own numbering starts fresh; theirs is separate and recorded below.
            local_cseq: 0,
            remote_cseq: cseq_number(&request.headers),
            route_set,
        })
    }

    /// The `To` and `From` for a request we originate.
    ///
    /// They swap according to role, which is the detail a UAS most often gets wrong: a BYE
    /// from the callee has the callee in `From`, not in `To`.
    #[must_use]
    pub fn local_and_remote(&self) -> (String, String) {
        let local = format!(
            "{};tag={}",
            strip_header_params(&self.local_uri),
            String::from_utf8_lossy(&self.id.local_tag)
        );
        let remote = format!(
            "{};tag={}",
            strip_header_params(&self.remote_uri),
            String::from_utf8_lossy(&self.id.remote_tag)
        );
        (local, remote)
    }

    /// Take the next sequence number for a request we originate.
    pub fn next_cseq(&mut self) -> u32 {
        self.local_cseq = self.local_cseq.saturating_add(1);
        self.local_cseq
    }

    /// Whether an in-dialog request belongs to this dialog.
    #[must_use]
    pub fn matches(&self, request: &Request) -> bool {
        let Some(call_id) = request.headers.value(&HeaderName::CallId) else {
            return false;
        };
        if call_id.as_ref() != self.id.call_id.as_slice() {
            return false;
        }
        // The tags arrive swapped relative to how we hold them: their `From` is our remote.
        let their_tag = tag_of::<FromHeader>(&request.headers);
        let our_tag = tag_of::<To>(&request.headers);
        their_tag.as_deref() == Some(self.id.remote_tag.as_slice())
            && our_tag.as_deref() == Some(self.id.local_tag.as_slice())
    }
}

/// The name-addr part of a `To` or `From`, without its header parameters.
///
/// Splitting on the first `;` is wrong: a URI inside angle brackets may carry its own
/// parameters, so `<sip:alice@example.com;transport=tcp>;tag=abc` would be truncated to
/// `<sip:alice@example.com` — an unterminated bracket the far end answers with 400, leaving a
/// call that cannot be hung up. Parameters after the closing bracket belong to the header;
/// those inside belong to the URI and stay.
pub(crate) fn strip_header_params(value: &str) -> String {
    if let Some(end) = value.rfind('>') {
        return value.get(..=end).unwrap_or(value).trim().to_owned();
    }
    // Without brackets there can be no URI parameters — RFC 3261 §20.10 requires brackets
    // exactly when the URI carries any — so the first `;` is the header's.
    value.split(';').next().unwrap_or(value).trim().to_owned()
}

fn header_string(headers: &sipx_sip::Headers, name: &HeaderName) -> String {
    headers
        .value(name)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .unwrap_or_default()
}

fn tag_of<T>(headers: &sipx_sip::Headers) -> Option<Vec<u8>>
where
    T: sipx_sip::TypedHeader,
    T: HasTag,
{
    headers
        .typed::<T>()
        .and_then(Result::ok)
        .and_then(|header| header.tag_bytes())
}

/// Both `To` and `From` carry a tag, and both are needed from either side.
pub trait HasTag {
    /// The `tag` parameter.
    fn tag_bytes(&self) -> Option<Vec<u8>>;
}

impl HasTag for To {
    fn tag_bytes(&self) -> Option<Vec<u8>> {
        self.tag().map(<[u8]>::to_vec)
    }
}

impl HasTag for FromHeader {
    fn tag_bytes(&self) -> Option<Vec<u8>> {
        self.tag().map(<[u8]>::to_vec)
    }
}

fn contact_uri(headers: &sipx_sip::Headers) -> Option<Uri> {
    let value = headers.value(&HeaderName::Contact)?;
    let text = String::from_utf8_lossy(&value);
    // A Contact is usually `<uri>` with optional parameters; the angle brackets are what make
    // a URI with its own parameters unambiguous.
    let inner = text
        .split_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map_or_else(
            || text.split(';').next().unwrap_or(&text).trim().to_owned(),
            |(uri, _)| uri.to_owned(),
        );
    Uri::parse(Bytes::from(inner)).ok()
}

/// The route set, one entry per route.
///
/// Two things this must get right. `Record-Route` is a comma-separated list header, so one
/// line may carry several routes, and the reversal a UAC performs has to reverse *routes*, not
/// lines. And a malformed route must not take the well-formed ones with it: the route set is
/// what a BYE travels along, and losing it silently is how a call becomes unhangupable.
fn record_routes(headers: &sipx_sip::Headers) -> Vec<String> {
    let mut routes = Vec::new();
    for header in headers.get_all(&HeaderName::RecordRoute) {
        routes.extend(split_routes(&header.value()));
    }
    routes
}

/// Split one `Record-Route` value into its routes, honouring angle brackets and quotes.
///
/// A comma inside `<...>` or inside a quoted display name is part of the route, not a
/// separator between two of them.
fn split_routes(value: &[u8]) -> Vec<String> {
    let mut routes = Vec::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut start = 0usize;

    for (index, &byte) in value.iter().enumerate() {
        match byte {
            b'"' => quoted = !quoted,
            b'<' if !quoted => depth += 1,
            b'>' if !quoted => depth = depth.saturating_sub(1),
            b',' if !quoted && depth == 0 => {
                push_route(&mut routes, value.get(start..index));
                start = index + 1;
            }
            _ => {}
        }
    }
    push_route(&mut routes, value.get(start..));
    routes
}

fn push_route(routes: &mut Vec<String>, slice: Option<&[u8]>) {
    let Some(slice) = slice else {
        return;
    };
    let text = String::from_utf8_lossy(slice).trim().to_owned();
    if !text.is_empty() {
        routes.push(text);
    }
}

fn cseq_number(headers: &sipx_sip::Headers) -> Option<u32> {
    headers
        .typed::<sipx_sip::headers::CSeq>()
        .and_then(Result::ok)
        .map(|cseq| cseq.sequence)
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
    use sipx_sip::{Limits, Message, parse_datagram};

    fn request(text: &str) -> Request {
        match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram()).expect("parses") {
            Message::Request(r) => r,
            Message::Response(_) => panic!("a request"),
        }
    }

    fn response(text: &str) -> Response {
        match parse_datagram(Bytes::from(text.to_owned()), &Limits::datagram()).expect("parses") {
            Message::Response(r) => r,
            Message::Request(_) => panic!("a response"),
        }
    }

    fn invite() -> Request {
        request(
            "INVITE sip:bob@example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             To: <sip:bob@example.com>\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:alice@192.0.2.1:5060>\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        )
    }

    fn ok() -> Response {
        response(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             To: <sip:bob@example.com>;tag=bobtag\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:bob@192.0.2.9:5060>\r\n\
             Content-Length: 0\r\n\r\n",
        )
    }

    #[test]
    fn the_callers_dialog_takes_its_tags_from_the_right_places() {
        let dialog = Dialog::from_response(&invite(), &ok()).expect("a dialog");
        assert_eq!(dialog.role, Role::Caller);
        assert_eq!(dialog.id.call_id, b"thecall@example.net");
        assert_eq!(dialog.id.local_tag, b"alicetag", "our tag is in From");
        assert_eq!(dialog.id.remote_tag, b"bobtag", "theirs is in To");
        assert_eq!(
            dialog.remote_target.to_bytes().as_ref(),
            b"sip:bob@192.0.2.9:5060"
        );
    }

    /// The callee's view is the mirror: the tags swap. A UAS that builds its dialog as though
    /// it were the UAC fails to match its own dialog's BYE.
    #[test]
    fn the_callees_dialog_is_the_mirror_of_the_callers() {
        let dialog = Dialog::from_request(&invite(), "bobtag").expect("a dialog");
        assert_eq!(dialog.role, Role::Callee);
        assert_eq!(dialog.id.local_tag, b"bobtag", "the tag we chose");
        assert_eq!(dialog.id.remote_tag, b"alicetag", "theirs is in From");
        assert_eq!(
            dialog.remote_target.to_bytes().as_ref(),
            b"sip:alice@192.0.2.1:5060"
        );
    }

    /// Both halves must agree on the identity of the dialog, with local and remote swapped.
    #[test]
    fn both_halves_describe_the_same_dialog() {
        let uac = Dialog::from_response(&invite(), &ok()).expect("a dialog");
        let uas = Dialog::from_request(&invite(), "bobtag").expect("a dialog");

        assert_eq!(uac.id.call_id, uas.id.call_id);
        assert_eq!(uac.id.local_tag, uas.id.remote_tag);
        assert_eq!(uac.id.remote_tag, uas.id.local_tag);
    }

    /// Each side numbers its own requests. A shared counter makes the first in-dialog request
    /// from each side collide.
    #[test]
    fn the_two_sides_number_their_requests_independently() {
        let mut uac = Dialog::from_response(&invite(), &ok()).expect("a dialog");
        let mut uas = Dialog::from_request(&invite(), "bobtag").expect("a dialog");

        assert_eq!(uac.next_cseq(), 2, "the INVITE was 1");
        assert_eq!(uas.next_cseq(), 1, "the callee starts its own count");
        assert_eq!(uac.next_cseq(), 3);
        assert_eq!(uas.next_cseq(), 2);
    }

    /// A BYE from the callee has the callee in `From`. Getting this backwards produces a call
    /// that establishes and cannot be ended, which is worse than one that never establishes:
    /// the media keeps flowing.
    #[test]
    fn a_request_from_the_callee_puts_the_callee_in_from() {
        let callee = Dialog::from_request(&invite(), "bobtag").expect("a dialog");
        let (local, remote) = callee.local_and_remote();
        assert!(local.contains("bob@example.com"), "{local}");
        assert!(local.contains("tag=bobtag"), "{local}");
        assert!(remote.contains("alice@example.net"), "{remote}");
        assert!(remote.contains("tag=alicetag"), "{remote}");
    }

    #[test]
    fn a_request_from_the_caller_puts_the_caller_in_from() {
        let caller = Dialog::from_response(&invite(), &ok()).expect("a dialog");
        let (local, remote) = caller.local_and_remote();
        assert!(local.contains("alice@example.net"), "{local}");
        assert!(local.contains("tag=alicetag"), "{local}");
        assert!(remote.contains("bob@example.com"), "{remote}");
    }

    /// An in-dialog request arrives with the tags swapped relative to how we hold them.
    #[test]
    fn an_in_dialog_request_matches_its_dialog() {
        let caller = Dialog::from_response(&invite(), &ok()).expect("a dialog");
        let bye = request(
            "BYE sip:alice@192.0.2.1:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.9:5060;branch=z9hG4bKbye\r\n\
             To: <sip:alice@example.net>;tag=alicetag\r\n\
             From: <sip:bob@example.com>;tag=bobtag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 BYE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(caller.matches(&bye));
    }

    #[test]
    fn a_request_from_another_call_does_not_match() {
        let caller = Dialog::from_response(&invite(), &ok()).expect("a dialog");
        let other = request(
            "BYE sip:alice@192.0.2.1:5060 SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.9:5060;branch=z9hG4bKbye\r\n\
             To: <sip:alice@example.net>;tag=alicetag\r\n\
             From: <sip:bob@example.com>;tag=bobtag\r\n\
             Call-ID: a-different-call@example.net\r\n\
             CSeq: 1 BYE\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(!caller.matches(&other));
    }

    /// A response with no `To` tag creates no dialog — a 100 Trying, for instance.
    #[test]
    fn a_response_without_a_to_tag_creates_no_dialog() {
        let trying = response(
            "SIP/2.0 100 Trying\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             To: <sip:bob@example.com>\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Content-Length: 0\r\n\r\n",
        );
        assert!(Dialog::from_response(&invite(), &trying).is_none());
    }

    /// RFC 3261 §12.1.2: the caller reverses the route set and the callee does not. Sending in
    /// the wrong order routes a BYE through the proxies backwards, and it never arrives.
    #[test]
    fn the_caller_reverses_the_route_set_and_the_callee_does_not() {
        let routed_ok = response(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             Record-Route: <sip:proxy1.example.com;lr>\r\n\
             Record-Route: <sip:proxy2.example.com;lr>\r\n\
             To: <sip:bob@example.com>;tag=bobtag\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:bob@192.0.2.9:5060>\r\n\
             Content-Length: 0\r\n\r\n",
        );
        let caller = Dialog::from_response(&invite(), &routed_ok).expect("a dialog");
        assert_eq!(caller.route_set.len(), 2);
        assert!(
            caller.route_set[0].contains("proxy2"),
            "reversed for the caller: {:?}",
            caller.route_set
        );

        let routed_invite = request(
            "INVITE sip:bob@example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             Record-Route: <sip:proxy1.example.com;lr>\r\n\
             Record-Route: <sip:proxy2.example.com;lr>\r\n\
             To: <sip:bob@example.com>\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:alice@192.0.2.1:5060>\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        );
        let uas = Dialog::from_request(&routed_invite, "bobtag").expect("a dialog");
        assert!(
            uas.route_set[0].contains("proxy1"),
            "as received for the callee: {:?}",
            uas.route_set
        );
    }

    /// Several routes on one line: `Record-Route` is a comma-separated list header. Treating a
    /// line as a route means the caller's reversal reverses lines rather than routes, and a
    /// BYE goes through the proxies backwards.
    #[test]
    fn several_routes_on_one_line_are_separate_routes() {
        let routed = response(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             Record-Route: <sip:proxy1.example.com;lr>, <sip:proxy2.example.com;lr>\r\n\
             Record-Route: <sip:proxy3.example.com;lr>\r\n\
             To: <sip:bob@example.com>;tag=bobtag\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:bob@192.0.2.9:5060>\r\n\
             Content-Length: 0\r\n\r\n",
        );
        let caller = Dialog::from_response(&invite(), &routed).expect("a dialog");
        assert_eq!(
            caller.route_set.len(),
            3,
            "three routes: {:?}",
            caller.route_set
        );
        assert!(
            caller.route_set[0].contains("proxy3"),
            "{:?}",
            caller.route_set
        );
        assert!(
            caller.route_set[1].contains("proxy2"),
            "{:?}",
            caller.route_set
        );
        assert!(
            caller.route_set[2].contains("proxy1"),
            "{:?}",
            caller.route_set
        );
    }

    /// A comma inside angle brackets or a quoted display name is part of the route.
    #[test]
    fn a_comma_inside_a_route_does_not_split_it() {
        assert_eq!(
            split_routes(br#""Proxy, Inc" <sip:p1.example.com;lr>, <sip:p2.example.com;lr>"#),
            vec![
                r#""Proxy, Inc" <sip:p1.example.com;lr>"#.to_owned(),
                "<sip:p2.example.com;lr>".to_owned()
            ]
        );
    }

    /// A URI inside angle brackets may carry its own parameters. Splitting on the first `;`
    /// truncates it to an unterminated bracket, which the far end answers 400 — leaving a call
    /// that cannot be hung up.
    #[test]
    fn a_uri_with_parameters_survives_having_its_tag_stripped() {
        assert_eq!(
            strip_header_params("<sip:alice@example.com;transport=tcp>;tag=abc"),
            "<sip:alice@example.com;transport=tcp>"
        );
        assert_eq!(
            strip_header_params("<sip:bob@example.com>;tag=x"),
            "<sip:bob@example.com>"
        );
        assert_eq!(
            strip_header_params("sip:carol@example.com;tag=y"),
            "sip:carol@example.com"
        );
        assert_eq!(
            strip_header_params(r#""Alice" <sip:alice@example.com;user=phone>;tag=z"#),
            r#""Alice" <sip:alice@example.com;user=phone>"#
        );
    }

    /// And the whole way round: a dialog built from such a `From` produces a well-formed one.
    #[test]
    fn a_request_from_a_dialog_with_uri_parameters_is_well_formed() {
        let with_params = request(
            "INVITE sip:bob@example.com SIP/2.0\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             To: <sip:bob@example.com;user=phone>\r\n\
             From: <sip:alice@example.net;transport=tcp>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: <sip:alice@192.0.2.1:5060>\r\n\
             Max-Forwards: 70\r\n\
             Content-Length: 0\r\n\r\n",
        );
        let callee = Dialog::from_request(&with_params, "bobtag").expect("a dialog");
        let (local, remote) = callee.local_and_remote();
        assert_eq!(local, "<sip:bob@example.com;user=phone>;tag=bobtag");
        assert_eq!(remote, "<sip:alice@example.net;transport=tcp>;tag=alicetag");
        assert_eq!(local.matches('<').count(), local.matches('>').count());
        assert_eq!(remote.matches('<').count(), remote.matches('>').count());
    }

    /// A `Contact` may carry parameters; the angle brackets are what make the URI unambiguous.
    #[test]
    fn a_contact_with_parameters_yields_only_the_uri() {
        let with_params = response(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKcall\r\n\
             To: <sip:bob@example.com>;tag=bobtag\r\n\
             From: <sip:alice@example.net>;tag=alicetag\r\n\
             Call-ID: thecall@example.net\r\n\
             CSeq: 1 INVITE\r\n\
             Contact: \"Bob\" <sip:bob@192.0.2.9:5060>;expires=300\r\n\
             Content-Length: 0\r\n\r\n",
        );
        let dialog = Dialog::from_response(&invite(), &with_params).expect("a dialog");
        assert_eq!(
            dialog.remote_target.to_bytes().as_ref(),
            b"sip:bob@192.0.2.9:5060"
        );
    }
}
