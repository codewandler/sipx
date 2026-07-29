//! Header field names (RFC 3261 §7.3, §20).
//!
//! Names compare case-insensitively, and a compact form is the same header as its long form.
//! Both facts are load-bearing: RFC 4475 §3.1.1.1 spells `Max-Forwards` as `MaX-fOrWaRdS` and
//! writes `Content-Length` as `l` and `Subject` as `s` in the same message.
//!
//! Resolving a name does not lose the original spelling. The entry that holds a header keeps
//! the bytes as they arrived so a forwarded message is re-emitted unchanged; this type is
//! only how the *meaning* of a name is decided.

use std::hash::{Hash, Hasher};

use bytes::Bytes;

use crate::escape;

macro_rules! header_names {
    ($( $variant:ident => $canonical:literal $(| $compact:literal)? ; )*) => {
        /// A header field name.
        #[derive(Debug, Clone)]
        #[non_exhaustive]
        pub enum HeaderName {
            $(
                #[doc = concat!("`", $canonical, "`")]
                $variant,
            )*
            /// A header sipx does not model. Compared case-insensitively; its bytes are kept.
            Other(Bytes),
        }

        impl HeaderName {
            /// The canonical spelling, as sipx writes it in messages it constructs.
            #[must_use]
            pub fn canonical(&self) -> &[u8] {
                match self {
                    $( Self::$variant => $canonical.as_bytes(), )*
                    Self::Other(raw) => raw,
                }
            }

            /// The single-letter compact form, where the header has one.
            #[must_use]
            pub fn compact(&self) -> Option<u8> {
                match self {
                    $( $( Self::$variant => Some($compact), )? )*
                    _ => None,
                }
            }

            /// Resolve a name as it appeared on the wire.
            ///
            /// Never fails: an unrecognized name is [`HeaderName::Other`], because a proxy has
            /// to forward headers it does not know.
            #[must_use]
            pub fn parse(raw: &Bytes) -> Self {
                if raw.len() == 1 {
                    if let Some(&b) = raw.first() {
                        let lower = b.to_ascii_lowercase();
                        $( $( if lower == $compact { return Self::$variant; } )? )*
                    }
                }
                $(
                    if escape::eq_ignore_ascii_case(raw, $canonical.as_bytes()) {
                        return Self::$variant;
                    }
                )*
                Self::Other(raw.clone())
            }
        }
    };
}

header_names! {
    // RFC 3261 §20, in the order the RFC lists them. Compact forms from §20 and, for the
    // extension headers, from the RFCs that define them.
    Accept              => "Accept";
    AcceptContact       => "Accept-Contact" | b'a';       // RFC 3841
    AcceptEncoding      => "Accept-Encoding";
    AcceptLanguage      => "Accept-Language";
    AlertInfo           => "Alert-Info";
    Allow               => "Allow";
    AllowEvents         => "Allow-Events" | b'u';         // RFC 6665
    AuthenticationInfo  => "Authentication-Info";
    Authorization       => "Authorization";
    CallId              => "Call-ID" | b'i';
    CallInfo            => "Call-Info";
    Contact             => "Contact" | b'm';
    ContentDisposition  => "Content-Disposition";
    ContentEncoding     => "Content-Encoding" | b'e';
    ContentLanguage     => "Content-Language";
    ContentLength       => "Content-Length" | b'l';
    ContentType         => "Content-Type" | b'c';
    CSeq                => "CSeq";
    Date                => "Date";
    ErrorInfo           => "Error-Info";
    Event               => "Event" | b'o';                // RFC 6665
    Expires             => "Expires";
    FeatureCaps         => "Feature-Caps";                // RFC 6809
    From                => "From" | b'f';
    Identity            => "Identity" | b'y';             // RFC 4474
    IdentityInfo        => "Identity-Info" | b'n';        // RFC 4474
    FlowTimer           => "Flow-Timer";                  // RFC 5626
    InReplyTo           => "In-Reply-To";
    MaxForwards         => "Max-Forwards";
    MimeVersion         => "MIME-Version";
    MinExpires          => "Min-Expires";
    MinSe               => "Min-SE";                      // RFC 4028
    Organization        => "Organization";
    Path                => "Path";                        // RFC 3327
    Priority            => "Priority";
    ProxyAuthenticate   => "Proxy-Authenticate";
    ProxyAuthorization  => "Proxy-Authorization";
    ProxyRequire        => "Proxy-Require";
    RAck                => "RAck";                        // RFC 3262
    Reason              => "Reason";                      // RFC 3326
    RecordRoute         => "Record-Route";
    ReferSub            => "Refer-Sub";                   // RFC 4488
    ReferTo             => "Refer-To" | b'r';             // RFC 3515
    ReferredBy          => "Referred-By" | b'b';          // RFC 3892
    RejectContact       => "Reject-Contact" | b'j';       // RFC 3841
    Replaces            => "Replaces";                    // RFC 3891
    ReplyTo             => "Reply-To";
    RequestDisposition  => "Request-Disposition" | b'd';  // RFC 3841
    Require             => "Require";
    RetryAfter          => "Retry-After";
    Route               => "Route";
    RSeq                => "RSeq";                        // RFC 3262
    Server              => "Server";
    ServiceRoute        => "Service-Route";               // RFC 3608
    SessionExpires      => "Session-Expires" | b'x';      // RFC 4028
    Subject             => "Subject" | b's';
    SubscriptionState   => "Subscription-State";          // RFC 6665
    Supported           => "Supported" | b'k';
    Timestamp           => "Timestamp";
    To                  => "To" | b't';
    Unsupported         => "Unsupported";
    UserAgent           => "User-Agent";
    Via                 => "Via" | b'v';
    Warning             => "Warning";
    WwwAuthenticate     => "WWW-Authenticate";
}

impl HeaderName {
    /// Whether this header's grammar is a comma-separated list, so that repeated header lines
    /// and one line of comma-separated values mean the same thing (RFC 3261 §7.3.1).
    ///
    /// The authentication headers are the exception the RFC calls out by name: their values
    /// contain commas of their own, so splitting on commas would corrupt them.
    #[must_use]
    pub fn is_comma_separated_list(&self) -> bool {
        matches!(
            self,
            Self::Accept
                | Self::AcceptContact
                | Self::AcceptEncoding
                | Self::AcceptLanguage
                | Self::AlertInfo
                | Self::Allow
                | Self::AllowEvents
                | Self::CallInfo
                | Self::Contact
                | Self::ContentEncoding
                | Self::ContentLanguage
                | Self::ErrorInfo
                // RFC 6809 §4: `Feature-Caps = "Feature-Caps" HCOLON fc-value *(COMMA fc-value)`.
                | Self::FeatureCaps
                | Self::InReplyTo
                | Self::Path
                | Self::ProxyRequire
                | Self::RecordRoute
                | Self::Reason
                | Self::RejectContact
                | Self::Require
                | Self::Route
                | Self::ServiceRoute
                | Self::Supported
                | Self::Unsupported
                | Self::Via
                | Self::Warning
        )
    }

    /// Whether the RFC permits at most one of this header in a message.
    ///
    /// RFC 4475 §3.3.8 turns on this: a message with two `To` headers parses, and must be
    /// rejected by validation rather than by silently using the first.
    #[must_use]
    pub fn is_single_value(&self) -> bool {
        matches!(
            self,
            Self::CallId
                | Self::ContentLength
                | Self::ContentType
                | Self::CSeq
                | Self::Date
                | Self::Expires
                | Self::From
                | Self::MaxForwards
                | Self::MinExpires
                | Self::Organization
                | Self::Server
                | Self::Subject
                | Self::Timestamp
                | Self::To
                | Self::UserAgent
        )
    }
}

impl PartialEq for HeaderName {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Other(a), Self::Other(b)) => escape::eq_ignore_ascii_case(a, b),
            // A known name never equals an `Other`: `parse` resolves every known spelling,
            // including compact forms, so an `Other` cannot hold one.
            (Self::Other(_), _) | (_, Self::Other(_)) => false,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

impl Eq for HeaderName {}

impl Hash for HeaderName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash the lowercased canonical form so that hashing agrees with case-insensitive
        // equality. Getting this wrong would make a `HashMap<HeaderName, _>` lose entries.
        for b in self.canonical() {
            state.write_u8(b.to_ascii_lowercase());
        }
    }
}

impl std::fmt::Display for HeaderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(self.canonical()))
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
    use std::collections::HashSet;

    fn name(s: &str) -> HeaderName {
        HeaderName::parse(&Bytes::from(s.to_owned()))
    }

    #[test]
    fn names_resolve_case_insensitively() {
        // RFC 4475 3.1.1.1 spells it exactly this way.
        assert_eq!(name("MaX-fOrWaRdS"), HeaderName::MaxForwards);
        assert_eq!(name("content-length"), HeaderName::ContentLength);
        assert_eq!(name("WWW-Authenticate"), HeaderName::WwwAuthenticate);
    }

    #[test]
    fn compact_forms_are_the_same_header() {
        for (compact, long) in [
            ("i", "Call-ID"),
            ("m", "Contact"),
            ("e", "Content-Encoding"),
            ("l", "Content-Length"),
            ("c", "Content-Type"),
            ("f", "From"),
            ("s", "Subject"),
            ("k", "Supported"),
            ("t", "To"),
            ("v", "Via"),
            ("r", "Refer-To"),
            ("o", "Event"),
        ] {
            assert_eq!(name(compact), name(long), "{compact} should be {long}");
            assert_eq!(name(&compact.to_uppercase()), name(long));
        }
    }

    #[test]
    fn compact_form_is_reported_for_headers_that_have_one() {
        assert_eq!(HeaderName::Via.compact(), Some(b'v'));
        assert_eq!(HeaderName::CSeq.compact(), None);
    }

    #[test]
    fn unknown_names_are_preserved_and_compare_case_insensitively() {
        let a = name("NewFangledHeader");
        let b = name("newfangledheader");
        assert_eq!(a, b);
        assert_eq!(a.canonical(), b"NewFangledHeader");
        assert_ne!(a, name("UnknownHeaderWithUnusualValue"));
        assert_ne!(a, HeaderName::Via);
    }

    /// A `HashMap` keyed by header name would silently lose entries if hashing disagreed with
    /// equality — the classic way case-insensitive keys go wrong.
    #[test]
    fn hashing_agrees_with_equality() {
        let mut set = HashSet::new();
        set.insert(name("Via"));
        assert!(set.contains(&name("v")));
        assert!(set.contains(&name("VIA")));

        set.insert(name("X-Custom"));
        assert!(set.contains(&name("x-custom")));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn single_letter_names_that_are_not_compact_forms_stay_unknown() {
        // 'z' is not a registered compact form.
        assert_eq!(name("z"), HeaderName::Other(Bytes::from_static(b"z")));
    }

    #[test]
    fn list_and_single_value_headers_are_classified() {
        assert!(HeaderName::Via.is_comma_separated_list());
        assert!(HeaderName::Route.is_comma_separated_list());
        // RFC 3327 §4 gives `Path` the same `route-param *(COMMA route-param)` grammar the
        // other route headers have. Nothing in the crate branches on this predicate today —
        // the address-list decoder splits on commas itself — but it is public API, and a
        // caller asking whether it may join two `Path` rows deserves the right answer.
        assert!(HeaderName::Path.is_comma_separated_list());
        // RFC 3608 §5: `Service-Route = "Service-Route" HCOLON sr-value *( COMMA sr-value )`.
        assert!(HeaderName::ServiceRoute.is_comma_separated_list());
        // The RFC names the authentication headers as the exception: their values contain
        // commas of their own.
        assert!(!HeaderName::WwwAuthenticate.is_comma_separated_list());
        assert!(!HeaderName::Authorization.is_comma_separated_list());
        assert!(!HeaderName::ProxyAuthenticate.is_comma_separated_list());

        assert!(HeaderName::To.is_single_value());
        assert!(HeaderName::CSeq.is_single_value());
        assert!(!HeaderName::Via.is_single_value());
    }
}
