//! The two event packages that report state sipx already keeps (RFC 4235, RFC 3680).
//!
//! Dialogs and registrations. Both come first among packages for the same reason: sipx *has* this
//! state already — a dialog store and a registration lease — so they exercise [`crate::subscribe`]
//! without also needing a state model of their own. Presence does need one, which is why it is a
//! separate story.
//!
//! Together they are what a busy-lamp field on a desk phone subscribes to: `dialog` says whether a
//! line is ringing or in a call, `reg` says whether the phone is registered at all.
//! **Supported** (`S-35`): [`sipx_call::Notifier`](https://docs.rs/sipx-call/latest/sipx_call/struct.Notifier.html)
//! selects these package documents through the live endpoint dispatcher. Breaking changes receive
//! migration guidance while sipx remains pre-1.0.
//!

use std::fmt::Write as _;

/// The MIME type a `dialog` notification carries (RFC 4235 §4).
pub const DIALOG_INFO_TYPE: &str = "application/dialog-info+xml";
/// The MIME type a `reg` notification carries (RFC 3680 §4).
pub const REGINFO_TYPE: &str = "application/reginfo+xml";

/// Where a dialog has got to (RFC 4235 §3.7.1).
///
/// The five states of the RFC's own state machine, and they are not decoration: a watcher renders
/// `early` as "ringing" and `confirmed` as "on a call", so collapsing them is a busy-lamp field
/// that lights up at the wrong time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogState {
    /// The UAC has sent an INVITE and heard nothing.
    Trying,
    /// A provisional arrived without a tag, so there is no dialog identifier yet.
    Proceeding,
    /// A provisional with a tag: an early dialog exists.
    Early,
    /// A 2xx arrived. The call is up.
    Confirmed,
    /// Cancelled, rejected, ended with a BYE, timed out or replaced.
    Terminated,
}

impl DialogState {
    /// The token as it appears in the document.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trying => "trying",
            Self::Proceeding => "proceeding",
            Self::Early => "early",
            Self::Confirmed => "confirmed",
            Self::Terminated => "terminated",
        }
    }
}

/// Which side started the dialog (RFC 4235 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// This endpoint placed the call.
    Initiator,
    /// It received it.
    Recipient,
}

impl Direction {
    /// The token as it appears in the document.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initiator => "initiator",
            Self::Recipient => "recipient",
        }
    }
}

/// One dialog, as a watcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    /// An identifier for this dialog within the document.
    pub id: String,
    /// Where it has got to.
    pub state: DialogState,
    /// Which side started it.
    pub direction: Direction,
}

/// The `dialog` event package (RFC 4235).
///
/// Holds one watcher's view: the dialogs, and the version counter that view is up to.
#[derive(Debug)]
pub struct DialogWatch {
    entity: String,
    version: u32,
    /// Whether the next document is the first, which must be `full`.
    sent_full: bool,
}

impl DialogWatch {
    /// A watch on this address of record.
    #[must_use]
    pub fn new(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            version: 0,
            sent_full: false,
        }
    }

    /// The `Event` package name.
    #[must_use]
    pub fn package() -> &'static str {
        "dialog"
    }

    /// The version the next document will carry.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The next document for this watcher.
    ///
    /// **The first is always `full` and the rest are `partial`** (§4.1). A watcher that joined
    /// mid-call is given the whole picture once and told about changes after that; sending only
    /// changes from the start would leave it inferring a state nobody ever described.
    ///
    /// The version is per *subscription*, not per resource (§4.1): two watchers of the same set of
    /// dialogs each count from zero, and sharing a counter would make one of them see gaps.
    pub fn document(&mut self, dialogs: &[Dialog]) -> String {
        let full = !self.sent_full;
        self.sent_full = true;
        let version = self.version;
        // Saturating rather than wrapping: §4.1 requires a non-negative 32-bit integer, and a
        // counter that wrapped to zero would look to a watcher like a new subscription.
        self.version = self.version.saturating_add(1);

        let mut out = String::with_capacity(256);
        out.push_str("<?xml version=\"1.0\"?>\n");
        let _ = write!(
            out,
            "<dialog-info xmlns=\"urn:ietf:params:xml:ns:dialog-info\" version=\"{version}\" \
             state=\"{}\" entity=\"{}\">",
            if full { "full" } else { "partial" },
            escape(&self.entity)
        );
        for dialog in dialogs {
            let _ = write!(
                out,
                "\n  <dialog id=\"{}\" direction=\"{}\">\n    <state>{}</state>\n  </dialog>",
                escape(&dialog.id),
                dialog.direction.as_str(),
                dialog.state.as_str()
            );
        }
        out.push_str("\n</dialog-info>\n");
        out
    }
}

/// What happened to a registered contact (RFC 3680 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactEvent {
    /// A new binding.
    Registered,
    /// An existing one, refreshed.
    Refreshed,
    /// It ran out of time.
    Expired,
    /// It was removed deliberately.
    Unregistered,
}

impl ContactEvent {
    /// The token as it appears in the document.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Refreshed => "refreshed",
            Self::Expired => "expired",
            Self::Unregistered => "unregistered",
        }
    }

    /// Whether the contact is usable after this event.
    ///
    /// The distinction a watcher acts on: `expired` and `unregistered` mean the contact is gone,
    /// and the two are kept apart because *why* it went is what a display says — "lost its
    /// connection" reads differently from "logged out".
    #[must_use]
    pub fn still_bound(self) -> bool {
        matches!(self, Self::Registered | Self::Refreshed)
    }
}

/// One registered contact, as a watcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// An identifier for this contact within the document.
    pub id: String,
    /// The contact URI.
    pub uri: String,
    /// What just happened to it.
    pub event: ContactEvent,
}

/// The `reg` event package (RFC 3680).
#[derive(Debug)]
pub struct RegistrationWatch {
    entity: String,
    version: u32,
    sent_full: bool,
}

impl RegistrationWatch {
    /// A watch on this address of record.
    #[must_use]
    pub fn new(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            version: 0,
            sent_full: false,
        }
    }

    /// The `Event` package name.
    #[must_use]
    pub fn package() -> &'static str {
        "reg"
    }

    /// The version the next document will carry.
    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The next document for this watcher, with the same full-then-partial discipline.
    pub fn document(&mut self, contacts: &[Contact]) -> String {
        let full = !self.sent_full;
        self.sent_full = true;
        let version = self.version;
        self.version = self.version.saturating_add(1);

        let mut out = String::with_capacity(256);
        out.push_str("<?xml version=\"1.0\"?>\n");
        let _ = write!(
            out,
            "<reginfo xmlns=\"urn:ietf:params:xml:ns:reginfo\" version=\"{version}\" \
             state=\"{}\">",
            if full { "full" } else { "partial" }
        );
        let bound = contacts.iter().any(|contact| contact.event.still_bound());
        let _ = write!(
            out,
            "\n  <registration aor=\"{}\" id=\"0\" state=\"{}\">",
            escape(&self.entity),
            if bound { "active" } else { "terminated" }
        );
        for contact in contacts {
            let _ = write!(
                out,
                "\n    <contact id=\"{}\" state=\"{}\" event=\"{}\">\n      <uri>{}</uri>\n    </contact>",
                escape(&contact.id),
                if contact.event.still_bound() {
                    "active"
                } else {
                    "terminated"
                },
                contact.event.as_str(),
                escape(&contact.uri)
            );
        }
        out.push_str("\n  </registration>\n</reginfo>\n");
        out
    }
}

/// Escape the five characters XML cannot carry literally.
///
/// A SIP URI can contain `&` in its parameters, and an unescaped one makes the document
/// unparseable — a watcher then sees nothing at all rather than a slightly wrong dialog.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
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

    fn ringing() -> Dialog {
        Dialog {
            id: "d1".to_owned(),
            state: DialogState::Early,
            direction: Direction::Recipient,
        }
    }

    /// §4.1: "Versions start at 0, and increment by one for each new document sent to a
    /// subscriber", and the first document is `full`.
    #[test]
    fn the_first_document_is_full_and_the_rest_are_partial() {
        let mut watch = DialogWatch::new("sip:alice@sipx.test");
        let first = watch.document(&[ringing()]);
        assert!(first.contains("version=\"0\""), "{first}");
        assert!(first.contains("state=\"full\""), "{first}");

        let second = watch.document(&[ringing()]);
        assert!(second.contains("version=\"1\""), "{second}");
        assert!(
            second.contains("state=\"partial\""),
            "a watcher is given the whole picture once and told about changes after: {second}"
        );
    }

    /// §4.1: versions are "scoped within a subscription". Two watchers each count from zero, and
    /// sharing a counter would make one of them see gaps it cannot explain.
    #[test]
    fn two_watchers_each_count_from_zero() {
        let mut one = DialogWatch::new("sip:alice@sipx.test");
        let mut other = DialogWatch::new("sip:alice@sipx.test");
        let _ = one.document(&[ringing()]);
        let _ = one.document(&[ringing()]);
        assert_eq!(one.version(), 2);
        assert_eq!(other.version(), 0, "a second watcher starts its own count");
        assert!(other.document(&[ringing()]).contains("version=\"0\""));
    }

    #[test]
    fn the_version_increases_monotonically() {
        let mut watch = DialogWatch::new("sip:alice@sipx.test");
        let mut seen = Vec::new();
        for _ in 0..5u32 {
            let document = watch.document(&[ringing()]);
            // From the `dialog-info` element, not from `<?xml version="1.0"?>` — which is the
            // first `version="` in the document and is not the one that counts.
            let version: u32 = document
                .split("<dialog-info")
                .nth(1)
                .and_then(|element| element.split("version=\"").nth(1))
                .and_then(|rest| rest.split('"').next())
                .and_then(|text| text.parse().ok())
                .expect("a version on the dialog-info element");
            seen.push(version);
        }
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);
    }

    /// The story's failing-first test.
    ///
    /// A watcher sees a call ring and then end. The states are what a busy-lamp field renders, so
    /// `early` and `confirmed` reaching it in the right order *is* the feature.
    #[test]
    fn a_watcher_sees_a_dialog_reach_confirmed_and_then_terminate() {
        let mut watch = DialogWatch::new("sip:alice@sipx.test");

        let ringing = watch.document(&[Dialog {
            id: "d1".to_owned(),
            state: DialogState::Early,
            direction: Direction::Recipient,
        }]);
        assert!(ringing.contains("<state>early</state>"), "{ringing}");

        let answered = watch.document(&[Dialog {
            id: "d1".to_owned(),
            state: DialogState::Confirmed,
            direction: Direction::Recipient,
        }]);
        assert!(answered.contains("<state>confirmed</state>"), "{answered}");

        let ended = watch.document(&[Dialog {
            id: "d1".to_owned(),
            state: DialogState::Terminated,
            direction: Direction::Recipient,
        }]);
        assert!(ended.contains("<state>terminated</state>"), "{ended}");

        // And the same dialog throughout, or a watcher would see three calls rather than one.
        for document in [&ringing, &answered, &ended] {
            assert!(document.contains("id=\"d1\""), "{document}");
        }
    }

    #[test]
    fn a_dialog_document_names_its_namespace_and_entity() {
        let mut watch = DialogWatch::new("sip:alice@sipx.test");
        let document = watch.document(&[]);
        assert!(
            document.contains("xmlns=\"urn:ietf:params:xml:ns:dialog-info\""),
            "{document}"
        );
        assert!(
            document.contains("entity=\"sip:alice@sipx.test\""),
            "{document}"
        );
        assert_eq!(DIALOG_INFO_TYPE, "application/dialog-info+xml");
    }

    #[test]
    fn every_dialog_state_has_the_spelling_the_rfc_gives() {
        for (state, token) in [
            (DialogState::Trying, "trying"),
            (DialogState::Proceeding, "proceeding"),
            (DialogState::Early, "early"),
            (DialogState::Confirmed, "confirmed"),
            (DialogState::Terminated, "terminated"),
        ] {
            assert_eq!(state.as_str(), token);
        }
    }

    /// RFC 3680: the `reg` package reports per-contact state and the event that changed it.
    #[test]
    fn a_registration_document_reports_the_event_that_changed_a_contact() {
        let mut watch = RegistrationWatch::new("sip:alice@sipx.test");
        let document = watch.document(&[Contact {
            id: "c1".to_owned(),
            uri: "sip:alice@192.0.2.5".to_owned(),
            event: ContactEvent::Registered,
        }]);
        assert!(
            document.contains("xmlns=\"urn:ietf:params:xml:ns:reginfo\""),
            "{document}"
        );
        assert!(document.contains("event=\"registered\""), "{document}");
        assert!(document.contains("state=\"active\""), "{document}");
        assert!(
            document.contains("<uri>sip:alice@192.0.2.5</uri>"),
            "{document}"
        );
        assert_eq!(REGINFO_TYPE, "application/reginfo+xml");
    }

    /// `expired` and `unregistered` both mean gone, and are kept apart because *why* it went is
    /// what a display says: "lost its connection" reads differently from "logged out".
    #[test]
    fn an_expired_contact_and_an_unregistered_one_are_both_gone_and_not_the_same() {
        assert!(!ContactEvent::Expired.still_bound());
        assert!(!ContactEvent::Unregistered.still_bound());
        assert!(ContactEvent::Registered.still_bound());
        assert!(ContactEvent::Refreshed.still_bound());
        assert_ne!(ContactEvent::Expired, ContactEvent::Unregistered);

        let mut watch = RegistrationWatch::new("sip:alice@sipx.test");
        let document = watch.document(&[Contact {
            id: "c1".to_owned(),
            uri: "sip:alice@192.0.2.5".to_owned(),
            event: ContactEvent::Expired,
        }]);
        assert!(document.contains("event=\"expired\""), "{document}");
        assert!(
            document.contains("state=\"terminated\""),
            "a contact that expired is not active: {document}"
        );
    }

    /// A SIP URI can carry `&` in its parameters, and an unescaped one makes the whole document
    /// unparseable — a watcher then sees nothing at all rather than a slightly wrong dialog.
    #[test]
    fn a_uri_containing_xml_metacharacters_does_not_break_the_document() {
        let mut watch = RegistrationWatch::new("sip:alice@sipx.test");
        let document = watch.document(&[Contact {
            id: "c1".to_owned(),
            uri: "sip:alice@host?X=1&Y=<2>".to_owned(),
            event: ContactEvent::Registered,
        }]);
        assert!(document.contains("&amp;"), "{document}");
        assert!(document.contains("&lt;2&gt;"), "{document}");
        assert!(
            !document.contains("Y=<2>"),
            "the raw angle brackets must not survive: {document}"
        );
    }

    #[test]
    fn both_packages_are_named_the_way_a_subscriber_asks_for_them() {
        assert_eq!(DialogWatch::package(), "dialog");
        assert_eq!(RegistrationWatch::package(), "reg");
    }
}
