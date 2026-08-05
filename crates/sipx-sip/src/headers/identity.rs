//! RFC 3325 asserted and preferred identity header values.
//!
//! Trust is deliberately not represented here. These types establish only the shared wire
//! grammar: one or two address values, with a SIP/SIPS and tel pairing when both are present.

use bytes::Bytes;

use crate::error::HeaderError;
use crate::headers::address::Address;
use crate::headers::grammar;
use crate::message::TypedHeader;
use crate::name::HeaderName;
use crate::uri::Scheme;

fn validate_address(address: &Address, header: &'static str) -> Result<(), HeaderError> {
    if !address.params.is_empty()
        || !matches!(
            address.uri.scheme(),
            Scheme::Sip | Scheme::Sips | Scheme::Tel
        )
    {
        return Err(HeaderError::Syntax { header });
    }
    Ok(())
}

fn parse_value(value: &[u8], header: &'static str) -> Result<Address, HeaderError> {
    let address = Address::parse_without_header_params(value, header)?;
    validate_address(&address, header)?;
    Ok(address)
}

fn parse_list(value: &[u8], header: &'static str) -> Result<Vec<Address>, HeaderError> {
    grammar::split_list(value, header)?
        .into_iter()
        .map(|part| parse_value(part, header))
        .collect()
}

fn validate_list(values: &[&Address], header: &'static str) -> Result<(), HeaderError> {
    match values {
        [_] => Ok(()),
        [first, second]
            if is_sip_family(first) != is_sip_family(second) && is_tel(first) != is_tel(second) =>
        {
            Ok(())
        }
        _ => Err(HeaderError::Syntax { header }),
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

macro_rules! identity_header {
    ($(#[$meta:meta])* $type:ident => $variant:ident, $label:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $type(Address);

        impl $type {
            /// Construct a value from an address that already passed the shared address grammar.
            ///
            /// RFC 3325 permits only SIP, SIPS and tel URIs and defines no header-parameter tail.
            /// Keeping the field private makes those constraints true of every value, including
            /// values an application constructs rather than decodes from a message.
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

            fn decode(value: &[u8]) -> Result<Self, HeaderError> {
                parse_value(value, $label).map(Self)
            }

            fn decode_list(value: &[u8]) -> Result<Vec<Self>, HeaderError> {
                parse_list(value, $label).map(|values| values.into_iter().map(Self).collect())
            }

            fn validate_list(values: &[&Self]) -> Result<(), HeaderError> {
                let addresses: Vec<&Address> = values.iter().map(|value| &value.0).collect();
                validate_list(&addresses, $label)
            }
        }
    };
}

identity_header!(
    /// One `P-Asserted-Identity` value (RFC 3325 §9.1).
    ///
    /// Use [`crate::message::Headers::typed_all`] to decode the complete validated value list.
    PAssertedIdentity => PAssertedIdentity, "P-Asserted-Identity"
);
identity_header!(
    /// One `P-Preferred-Identity` value (RFC 3325 §9.2).
    ///
    /// Use [`crate::message::Headers::typed_all`] to decode the complete validated value list.
    PPreferredIdentity => PPreferredIdentity, "P-Preferred-Identity"
);
