//! Canonical event encoders expanded from the Rust-owned browser contract.
use crate::command::MediaKind;
pub(crate) use crate::contract::{CauseClass, Direction, RegistrationState};
use crate::json::Writer;

trait WireValue {
    fn write_field(&self, writer: &mut Writer, name: &str);
}
trait WireObject {
    fn write_object(&self, writer: &mut Writer);
}
impl<T: WireValue + ?Sized> WireValue for &T {
    fn write_field(&self, writer: &mut Writer, name: &str) {
        (*self).write_field(writer, name);
    }
}
impl WireValue for str {
    fn write_field(&self, writer: &mut Writer, name: &str) {
        writer.string(name, self);
    }
}
impl WireValue for String {
    fn write_field(&self, writer: &mut Writer, name: &str) {
        self.as_str().write_field(writer, name);
    }
}
impl WireValue for bool {
    fn write_field(&self, writer: &mut Writer, name: &str) {
        writer.boolean(name, *self);
    }
}
macro_rules! numeric_values {
    ($($ty:ty),*) => { $(impl WireValue for $ty {
        fn write_field(&self, writer: &mut Writer, name: &str) { writer.number(name, u64::from(*self)); }
    })* };
}
numeric_values!(u8, u32, u64);
impl<T: WireValue> WireValue for Option<T> {
    fn write_field(&self, writer: &mut Writer, name: &str) {
        if let Some(value) = self {
            value.write_field(writer, name);
        }
    }
}
impl<T: WireObject> WireValue for Vec<T> {
    fn write_field(&self, writer: &mut Writer, name: &str) {
        writer.objects(name, self, |object, value| value.write_object(object));
    }
}
macro_rules! string_values {
    ($($ty:ty),*) => { $(impl WireValue for $ty {
        fn write_field(&self, writer: &mut Writer, name: &str) { writer.string(name, self.as_str()); }
    })* };
}
string_values!(MediaKind, RegistrationState, Direction, CauseClass);
macro_rules! define_objects {
    ($($name:ident { $($field:ident: $ty:ty),* };)*) => { $(
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub(crate) struct $name { $(pub(crate) $field: $ty,)* }
        impl WireObject for $name {
            fn write_object(&self, writer: &mut Writer) { $(self.$field.write_field(writer, stringify!($field));)* }
        }
        impl WireValue for $name {
            fn write_field(&self, writer: &mut Writer, name: &str) { writer.object_field(name, |object| self.write_object(object)); }
        }
    )* };
}
browser_objects!(define_objects);

/// Internal outcome ownership; encoded fields are declared in `browser_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub(crate) id: u64,
    pub(crate) error: Option<OutcomeError>,
}

macro_rules! define_events {
    ($($variant:ident($wire:literal) { $($field:ident: $ty:ty),* } => { $($key:ident: $wire_ty:ty = $value:expr),* };)*) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub(crate) enum Event { $($variant { $($field: $ty,)* },)* }
        impl Event {
            pub(crate) fn encode(&self) -> Vec<u8> {
                let mut writer = Writer::object();
                writer.number("v", 1);
                match self { $(Self::$variant { $($field,)* } => {
                    writer.string("evt", $wire);
                    $(let value: &$wire_ty = &($value); value.write_field(&mut writer, stringify!($key));)*
                },)* }
                writer.finish().into_bytes()
            }
        }
    };
}
browser_events!(define_events);

impl Cause {
    pub(crate) fn class(class: CauseClass) -> Self {
        Self {
            class,
            status: None,
            reason: None,
        }
    }

    pub(crate) fn sip(status: u64, reason: impl Into<String>) -> Self {
        Self {
            class: CauseClass::Sip,
            status: Some(status),
            reason: Some(reason.into()),
        }
    }

    pub(crate) fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

impl OutcomeError {
    pub(crate) fn new(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }
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

    #[test]
    fn bsdk_evt_1_need_entropy() {
        let bytes = Event::NeedEntropy { min: 64 }.encode();
        assert_eq!(bytes, br#"{"v":1,"evt":"need-entropy","min":64}"#);
        assert_eq!(bytes.len(), 37);
    }

    #[test]
    fn bsdk_evt_2_registration_registered() {
        let bytes = Event::Registration {
            state: RegistrationState::Registered,
            expires: Some(600),
            status: None,
            reason: None,
        }
        .encode();
        assert_eq!(
            bytes,
            br#"{"v":1,"evt":"registration","state":"registered","expires":600}"#
        );
        assert_eq!(bytes.len(), 63);
    }

    #[test]
    fn bsdk_evt_3_need_local_media() {
        let bytes = Event::NeedLocalMedia {
            call: 1,
            kind: MediaKind::Offer,
        }
        .encode();
        assert_eq!(
            bytes,
            &br#"{"v":1,"evt":"need-local-media","call":1,"kind":"offer","constraints":{"audio":true,"video":false}}"#[..]
        );
        assert_eq!(bytes.len(), 99);
    }

    #[test]
    fn an_outcome_failure_carries_a_typed_code() {
        let bytes = Event::Outcome {
            outcome: Outcome {
                id: 7,
                error: Some(OutcomeError::new("call-limit", "eight concurrent calls")),
            },
        }
        .encode();
        assert_eq!(
            bytes,
            &br#"{"v":1,"evt":"outcome","id":7,"ok":false,"error":{"code":"call-limit","reason":"eight concurrent calls"}}"#[..]
        );
    }

    #[test]
    fn a_successful_outcome_has_no_error_object() {
        let bytes = Event::Outcome {
            outcome: Outcome { id: 1, error: None },
        }
        .encode();
        assert_eq!(bytes, &br#"{"v":1,"evt":"outcome","id":1,"ok":true}"#[..]);
    }
}
