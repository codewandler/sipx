//! RFC 3325 asserted and preferred identity header values.
//!
//! Trust is deliberately not represented here. Construction enforces RFC 3325's strict
//! one-or-two-value shape. Reception follows RFC 5876 §4.5 instead: syntactically valid values
//! with an unexpected scheme or position are reported as ignored so a proxy can remove them
//! without discarding the valid identities that preceded them.

use bytes::Bytes;

use crate::error::HeaderError;
use crate::headers::address::Address;
use crate::headers::grammar;
use crate::message::{Headers, TypedHeader};
use crate::name::HeaderName;
use crate::uri::Scheme;

/// Why RFC 5876 §4.5 says a received identity value is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IgnoredIdentityReason {
    /// The URI is not SIP, SIPS or TEL.
    UnexpectedScheme,
    /// A SIP URI already occurred earlier in the field.
    DuplicateSip,
    /// A SIPS URI already occurred earlier in the field.
    DuplicateSips,
    /// A TEL URI already occurred earlier in the field.
    DuplicateTel,
    /// A SIPS URI occurred earlier, so this SIP URI is an unexpected combination.
    SipAfterSips,
    /// A SIP URI occurred earlier, so this SIPS URI is an unexpected combination.
    SipsAfterSip,
}

/// One syntactically parsed received identity that RFC 5876 §4.5 says not to use or forward.
#[derive(Debug, Clone)]
pub struct IgnoredIdentity {
    index: usize,
    address: Address,
    reason: IgnoredIdentityReason,
}

impl IgnoredIdentity {
    /// Zero-based position in the combined field, across comma-joined values and repeated rows.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// The parsed value. An unexpected scheme remains available for diagnostics and policy logs.
    #[must_use]
    pub fn address(&self) -> &Address {
        &self.address
    }

    /// The RFC 5876 filtering rule that matched this value.
    #[must_use]
    pub fn reason(&self) -> IgnoredIdentityReason {
        self.reason
    }
}

fn validate_common(address: &Address, header: &'static str) -> Result<(), HeaderError> {
    // RFC 8217 updates RFC 3325: a URI containing `,`, `;` or `?` has to use name-addr.
    // Address::parse therefore interprets a bare semicolon tail as header parameters. RFC 3325
    // defines no such parameters, so every non-empty tail is malformed rather than URI content.
    if !address.params.is_empty() {
        return Err(HeaderError::Syntax { header });
    }

    if let Some(display_name) = &address.display_name {
        // `Address` predates checked construction and exposes its fields. Keep the identity
        // wrapper's private-field invariant meaningful even for a manually assembled Address:
        // serialization writes printable ASCII (escaping quote and backslash), horizontal tab,
        // or well-formed UTF-8, never a control byte that could alter a field line.
        let valid_ascii = display_name.iter().all(|byte| {
            byte.is_ascii_graphic() || matches!(byte, b' ' | b'\t') || !byte.is_ascii()
        });
        if !valid_ascii || std::str::from_utf8(display_name).is_err() {
            return Err(HeaderError::Syntax { header });
        }
    }

    Ok(())
}

fn validate_address(address: &Address, header: &'static str) -> Result<(), HeaderError> {
    validate_common(address, header)?;
    if !matches!(
        address.uri.scheme(),
        Scheme::Sip | Scheme::Sips | Scheme::Tel
    ) {
        return Err(HeaderError::Syntax { header });
    }
    Ok(())
}

fn parse_received_value(value: &[u8], header: &'static str) -> Result<Address, HeaderError> {
    let address = Address::parse(value, header)?;
    validate_common(&address, header)?;
    Ok(address)
}

fn parse_value(value: &[u8], header: &'static str) -> Result<Address, HeaderError> {
    let address = parse_received_value(value, header)?;
    validate_address(&address, header)?;
    Ok(address)
}

fn parse_list(value: &[u8], header: &'static str) -> Result<Vec<Address>, HeaderError> {
    grammar::split_list(value, header)?
        .into_iter()
        .map(|part| parse_value(part, header))
        .collect()
}

fn validate_list<'a>(
    values: impl IntoIterator<Item = &'a Address>,
    header: &'static str,
) -> Result<(), HeaderError> {
    let mut values = values.into_iter();
    let first = values.next().ok_or(HeaderError::Syntax { header })?;
    let second = values.next();
    if values.next().is_some() {
        return Err(HeaderError::Syntax { header });
    }
    match second {
        None => Ok(()),
        Some(second)
            if is_sip_family(first) != is_sip_family(second) && is_tel(first) != is_tel(second) =>
        {
            Ok(())
        }
        Some(_) => Err(HeaderError::Syntax { header }),
    }
}

#[must_use]
fn is_sip_family(address: &Address) -> bool {
    matches!(address.uri.scheme(), Scheme::Sip | Scheme::Sips)
}

#[must_use]
fn is_tel(address: &Address) -> bool {
    matches!(address.uri.scheme(), Scheme::Tel)
}

fn serialize(address: &Address) -> Bytes {
    let mut out = Vec::new();
    if let Some(display_name) = &address.display_name {
        out.push(b'"');
        for &byte in display_name {
            if matches!(byte, b'"' | b'\\') {
                out.push(b'\\');
            }
            out.push(byte);
        }
        out.extend_from_slice(b"\" ");
    }
    out.push(b'<');
    address.uri.write_to(&mut out);
    out.push(b'>');
    Bytes::from(out)
}

#[derive(Default)]
struct SeenSchemes {
    sip: bool,
    sips: bool,
    tel: bool,
}

impl SeenSchemes {
    fn classify(&mut self, address: &Address) -> Option<IgnoredIdentityReason> {
        match address.uri.scheme() {
            Scheme::Sip if self.sip => Some(IgnoredIdentityReason::DuplicateSip),
            Scheme::Sip => {
                self.sip = true;
                self.sips.then_some(IgnoredIdentityReason::SipAfterSips)
            }
            Scheme::Sips if self.sips => Some(IgnoredIdentityReason::DuplicateSips),
            Scheme::Sips => {
                self.sips = true;
                self.sip.then_some(IgnoredIdentityReason::SipsAfterSip)
            }
            Scheme::Tel if self.tel => Some(IgnoredIdentityReason::DuplicateTel),
            Scheme::Tel => {
                self.tel = true;
                None
            }
            Scheme::Other(_) => Some(IgnoredIdentityReason::UnexpectedScheme),
        }
    }
}

struct ReceivedIdentityList {
    values: Vec<Address>,
    ignored: Vec<IgnoredIdentity>,
}

fn receive_list(
    headers: &Headers,
    name: &HeaderName,
    label: &'static str,
) -> Result<Option<ReceivedIdentityList>, HeaderError> {
    let mut present = false;
    let mut index = 0usize;
    let mut seen = SeenSchemes::default();
    let mut values = Vec::new();
    let mut ignored = Vec::new();

    for row in headers.get_all(name) {
        present = true;
        let value = row.value();
        for part in grammar::split_list(value.as_ref(), label)? {
            let address = parse_received_value(part, label)?;
            if let Some(reason) = seen.classify(&address) {
                ignored.push(IgnoredIdentity {
                    index,
                    address,
                    reason,
                });
            } else {
                values.push(address);
            }
            index += 1;
        }
    }

    Ok(present.then_some(ReceivedIdentityList { values, ignored }))
}

macro_rules! identity_header {
    (
        $(#[$meta:meta])*
        $type:ident, $list:ident => $variant:ident, $label:literal
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $type(Address);

        impl $type {
            /// Construct one strict RFC 3325 value from a parsed address.
            ///
            /// Only SIP, SIPS and TEL are accepted, and RFC 8217's name-addr rule is enforced.
            /// Use the checked complete-list type when constructing a whole field.
            pub fn new(address: Address) -> Result<Self, HeaderError> {
                validate_address(&address, $label)?;
                Ok(Self(address))
            }

            /// The parsed identity address.
            #[must_use]
            pub fn address(&self) -> &Address {
                &self.0
            }

            /// Serialize this identity as an unambiguous name-address value.
            #[must_use]
            pub fn to_bytes(&self) -> Bytes {
                serialize(&self.0)
            }
        }

        impl std::ops::Deref for $type {
            type Target = Address;

            fn deref(&self) -> &Address {
                &self.0
            }
        }

        impl TypedHeader for $type {
            const NAME: HeaderName = HeaderName::$variant;
            const VALIDATE_LIST: bool = true;

            fn decode(value: &[u8]) -> Result<Self, HeaderError> {
                parse_value(value, $label).map(Self)
            }

            fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
                parse_list(value, $label).map(|values| values.into_iter().map(Self).collect())
            }

            fn validate_list(values: &[&Self]) -> Result<(), HeaderError> {
                validate_list(values.iter().map(|value| &value.0), $label)
            }
        }

        /// A complete identity field: strict for construction, tolerant and explicit on receive.
        #[derive(Debug, Clone)]
        pub struct $list {
            values: Vec<$type>,
            ignored: Vec<IgnoredIdentity>,
        }

        impl $list {
            /// Construct a complete RFC 3325 field, enforcing its one-or-two-value invariant.
            pub fn new(
                values: impl IntoIterator<Item = $type>,
            ) -> Result<Self, HeaderError> {
                let values = values.into_iter().collect::<Vec<_>>();
                validate_list(values.iter().map(|value| &value.0), $label)?;
                Ok(Self {
                    values,
                    ignored: Vec::new(),
                })
            }

            /// Decode all received rows in wire order using RFC 5876 §4.5 filtering.
            ///
            /// `Ok(None)` means the field is absent. Unexpected schemes, duplicate schemes and
            /// a SIP/SIPS combination are not syntax errors: they appear in [`Self::ignored`]. A
            /// forwarding proxy must remove those indexed values before sending the request;
            /// applying several removals in descending index order keeps earlier indices stable.
            pub fn from_headers(headers: &Headers) -> Result<Option<Self>, HeaderError> {
                receive_list(headers, &HeaderName::$variant, $label).map(|received| {
                    received.map(|received| Self {
                        values: received.values.into_iter().map($type).collect(),
                        ignored: received.ignored,
                    })
                })
            }

            /// Values that may be used and forwarded, in their received or construction order.
            #[must_use]
            pub fn values(&self) -> &[$type] {
                &self.values
            }

            /// Values RFC 5876 says to ignore and not forward, with stable wire-order indices.
            ///
            /// Remove several values from [`Headers`] in reverse order so each remaining index
            /// still refers to the field shape this report describes.
            #[must_use]
            pub fn ignored(&self) -> &[IgnoredIdentity] {
                &self.ignored
            }

            /// Whether forwarding the original field unchanged would violate RFC 5876 §4.5.
            #[must_use]
            pub fn requires_rewrite(&self) -> bool {
                !self.ignored.is_empty()
            }

            /// Consume the list and return the usable values plus the ignored-value report.
            #[must_use]
            pub fn into_parts(self) -> (Vec<$type>, Vec<IgnoredIdentity>) {
                (self.values, self.ignored)
            }

            /// Serialize the usable values as one deterministic comma-and-space-delimited row.
            ///
            /// Ignored received values are deliberately absent: RFC 5876 forbids forwarding them.
            /// `None` means every received value was ignored, so a proxy removes the field rather
            /// than constructing an invalid empty row.
            #[must_use]
            pub fn to_bytes(&self) -> Option<Bytes> {
                if self.values.is_empty() {
                    return None;
                }
                let mut out = Vec::new();
                for (position, value) in self.values.iter().enumerate() {
                    if position != 0 {
                        out.extend_from_slice(b", ");
                    }
                    out.extend_from_slice(&value.to_bytes());
                }
                Some(Bytes::from(out))
            }
        }
    };
}

identity_header!(
    /// One strict `P-Asserted-Identity` value (RFC 3325 §9.1).
    ///
    /// [`PAssertedIdentityList`] is the complete construction and receive-list API.
    PAssertedIdentity, PAssertedIdentityList => PAssertedIdentity, "P-Asserted-Identity"
);
identity_header!(
    /// One strict `P-Preferred-Identity` value (RFC 3325 §9.2).
    ///
    /// [`PPreferredIdentityList`] is the complete construction and receive-list API.
    PPreferredIdentity, PPreferredIdentityList => PPreferredIdentity, "P-Preferred-Identity"
);
