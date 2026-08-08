//! JSON decoding that observes object members before a map can collapse duplicates.

use std::collections::HashSet;
use std::fmt;

use serde::de::{self, DeserializeSeed as _, Visitor};

struct UniqueValue;

struct UniqueVisitor;

impl<'de> serde::de::DeserializeSeed<'de> for UniqueValue {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value with unique object members")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        UniqueValue.deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(UniqueValue)? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut names = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(name) = object.next_key::<String>()? {
            if !names.insert(name.clone()) {
                return Err(de::Error::custom(format!("duplicate JSON member {name:?}")));
            }
            values.insert(name, object.next_value_seed(UniqueValue)?);
        }
        Ok(serde_json::Value::Object(values))
    }
}

pub(crate) fn parse(text: &str) -> Result<serde_json::Value, String> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = UniqueValue
        .deserialize(&mut deserializer)
        .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
    Ok(value)
}

pub(crate) fn value(text: &str) -> serde_json::Value {
    parse(text).unwrap_or_else(|error| panic!("strict JSON result: {error}: {text}"))
}

pub(crate) fn versioned(producer: &str, text: &str) -> serde_json::Value {
    parse(text).unwrap_or_else(|error| panic!("strict {producer} JSON result: {error}: {text}"))
}

// The cfg matches its **only** caller exactly, `target_os` included. The caller is the Linux
// virtual-device test, `dph_12_wav_and_virtual_device_carry_the_same_clip`, which is gated on both
// the feature and the platform; gating this on the feature alone left it dead on macOS and Windows,
// where `-D warnings` makes dead code an error. That is why the two `device audio compiles` jobs
// have been red — a platform failure in appearance and a `cfg` that disagreed with its caller in
// fact. The Linux gate cannot see it: on a Linux host the caller compiles and the function is used.
// `X-125` is filed for closing that blind spot rather than relying on the next reader noticing.
#[cfg(all(feature = "device-audio", target_os = "linux"))]
pub(crate) fn versioned_bytes(producer: &str, bytes: &[u8]) -> serde_json::Value {
    versioned(
        producer,
        std::str::from_utf8(bytes).expect("JSON result is UTF-8"),
    )
}

#[test]
fn duplicate_members_are_rejected_recursively() {
    for sample in [
        r#"{"same":1,"same":2}"#,
        r#"{"nested":{"same":1,"same":2}}"#,
        r#"[{"same":1,"same":2}]"#,
    ] {
        let error = parse(sample).expect_err("duplicate member is refused");
        assert!(error.contains("same"), "{error}");
    }
}
