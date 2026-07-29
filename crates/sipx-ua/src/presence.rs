//! Presence, and publishing it (RFC 3856, RFC 3863, RFC 3903).
//!
//! `S-17`'s packages report state sipx already keeps. Presence has none — nothing in a SIP stack
//! knows whether a person is at their desk — so this is the half that lets somebody who *does*
//! know put it in: PUBLISH creates soft state, an entity tag identifies it, and a subscriber to the
//! `presence` package is told when it changes.
//!
//! **The entity tag is the whole mechanism and the part that is easy to skip.** Without it two
//! publishers for one resource silently overwrite each other and neither can tell; with it, a
//! publisher whose state has expired is told to start again rather than allowed to resurrect a
//! document the server has already forgotten.

use std::fmt::Write as _;
use std::time::Duration;

/// The MIME type a presence document carries (RFC 3863 §4).
pub const PIDF_TYPE: &str = "application/pidf+xml";

/// Whether a contact is reachable (RFC 3863 §4.1.3).
///
/// Two values, and only two. §4.1.3 defines `open` and `closed` and nothing else; the rich
/// vocabulary people expect — busy, away, on the phone — is RFC 4480's, which is a separate
/// document and a separate story.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Basic {
    /// Reachable.
    Open,
    /// Not.
    Closed,
}

impl Basic {
    /// The token as it appears in the document.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

/// One way of reaching a presentity (RFC 3863 §4.1.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Tuple {
    /// Identifies this tuple within the document.
    pub id: String,
    /// Whether it is reachable.
    pub status: Basic,
    /// Where, if the tuple names somewhere.
    pub contact: Option<String>,
    /// How much this contact is preferred, in `0.0..=1.0` (§4.1.4).
    pub priority: Option<f32>,
    /// Free text for a human.
    pub note: Option<String>,
}

impl Tuple {
    /// A reachable tuple.
    #[must_use]
    pub fn open(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status: Basic::Open,
            contact: None,
            priority: None,
            note: None,
        }
    }

    /// An unreachable one.
    #[must_use]
    pub fn closed(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            status: Basic::Closed,
            contact: None,
            priority: None,
            note: None,
        }
    }

    /// Reachable at this URI.
    #[must_use]
    pub fn at(mut self, contact: impl Into<String>) -> Self {
        self.contact = Some(contact.into());
        self
    }

    /// With this preference.
    #[must_use]
    pub fn with_priority(mut self, priority: f32) -> Self {
        // Clamped rather than trusted: §4.1.4 fixes the range, and a document carrying 7.5 is one
        // a watcher may reject outright — losing the whole presence rather than one number.
        self.priority = Some(priority.clamp(0.0, 1.0));
        self
    }

    /// With a note for a human.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// A presence document (RFC 3863).
///
/// A typed document rather than a string template, which is what the story asked for and what
/// stops a caller producing something unparseable by concatenation.
#[derive(Debug, Clone, PartialEq)]
pub struct Pidf {
    /// Whose presence this is.
    pub entity: String,
    /// The ways of reaching them.
    pub tuples: Vec<Tuple>,
}

impl Pidf {
    /// A document for a presentity.
    #[must_use]
    pub fn new(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            tuples: Vec::new(),
        }
    }

    /// With this tuple.
    #[must_use]
    pub fn with(mut self, tuple: Tuple) -> Self {
        self.tuples.push(tuple);
        self
    }

    /// Render as `application/pidf+xml`.
    #[must_use]
    pub fn to_xml(&self) -> String {
        let mut out = String::with_capacity(256);
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = write!(
            out,
            "<presence xmlns=\"urn:ietf:params:xml:ns:pidf\" entity=\"{}\">",
            escape(&self.entity)
        );
        for tuple in &self.tuples {
            let _ = write!(out, "\n  <tuple id=\"{}\">", escape(&tuple.id));
            let _ = write!(
                out,
                "\n    <status>\n      <basic>{}</basic>\n    </status>",
                tuple.status.as_str()
            );
            if let Some(contact) = &tuple.contact {
                match tuple.priority {
                    Some(priority) => {
                        let _ = write!(
                            out,
                            "\n    <contact priority=\"{priority}\">{}</contact>",
                            escape(contact)
                        );
                    }
                    None => {
                        let _ = write!(out, "\n    <contact>{}</contact>", escape(contact));
                    }
                }
            }
            if let Some(note) = &tuple.note {
                let _ = write!(out, "\n    <note>{}</note>", escape(note));
            }
            out.push_str("\n  </tuple>");
        }
        out.push_str("\n</presence>\n");
        out
    }
}

/// What a PUBLISH asked for (RFC 3903 §4.1, §6).
///
/// The three operations differ only by what is present, which is why they are read as one thing
/// and dispatched on: an entity tag with no body is a refresh, with a body a modify, and with
/// `Expires: 0` a removal.
#[derive(Debug, Clone, PartialEq)]
pub enum Publish {
    /// First publication: a body and no entity tag.
    Initial {
        /// The document.
        body: String,
        /// How long it should live.
        expires: Duration,
    },
    /// A refresh: an entity tag and no body.
    Refresh {
        /// The tag identifying the state.
        tag: String,
        /// The new lifetime.
        expires: Duration,
    },
    /// A modification: an entity tag and a body.
    Modify {
        /// The tag identifying the state.
        tag: String,
        /// The replacement document.
        body: String,
        /// The new lifetime.
        expires: Duration,
    },
    /// A removal: an entity tag and `Expires: 0`.
    Remove {
        /// The tag identifying the state.
        tag: String,
    },
    /// Neither a body nor a tag — §6 step 5 says reject this.
    ///
    /// It is not an empty publication: there is nothing to publish and nothing to identify, so
    /// there is no operation it could be.
    Empty,
}

impl Publish {
    /// Read what a PUBLISH is asking for, from its tag, body and expiry.
    #[must_use]
    pub fn read(tag: Option<String>, body: Option<String>, expires: Duration) -> Self {
        match (tag, body) {
            (Some(tag), _) if expires.is_zero() => Self::Remove { tag },
            (Some(tag), Some(body)) => Self::Modify { tag, body, expires },
            (Some(tag), None) => Self::Refresh { tag, expires },
            (None, Some(body)) => Self::Initial { body, expires },
            (None, None) => Self::Empty,
        }
    }
}

/// How a publication attempt was answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Published {
    /// Accepted. The tag identifies the state from now on and goes in `SIP-ETag` (§6 step 6).
    ///
    /// A **fresh** tag on every acceptance, including a refresh: §6 has the ESC issue one per
    /// response, and a publisher that kept using an old one after a refresh would be rejected the
    /// next time it tried.
    Accepted {
        /// The new entity tag.
        tag: String,
        /// How long the state will live.
        expires: Duration,
    },
    /// Removed.
    Removed,
    /// The entity tag names state this server does not have (§6 step 3).
    ///
    /// **412, not 404 and not silently accepting it as new.** A publisher whose state expired
    /// while it was not looking has to start again with a fresh publication; treating its refresh
    /// as a new publication would resurrect a document the server had already forgotten and that
    /// nothing has re-sent.
    ConditionFailed,
    /// Nothing to publish and nothing to identify (§6 step 5).
    Invalid,
}

/// The status a stale entity tag is refused with (RFC 3903 §6 step 3).
pub const CONDITIONAL_REQUEST_FAILED: u16 = 412;

/// The soft state one event state compositor holds.
///
/// "Soft" is the whole model: a publication expires unless refreshed, so a publisher that
/// disappears stops being believed rather than leaving a presence nobody can clear.
#[derive(Debug)]
pub struct Compositor {
    held: Vec<Entry>,
    next_tag: u64,
    maximum: Duration,
}

#[derive(Debug)]
struct Entry {
    tag: String,
    entity: String,
    body: String,
    expires_at: u64,
}

impl Compositor {
    /// A compositor granting at most this long.
    #[must_use]
    pub fn new(maximum: Duration) -> Self {
        Self {
            held: Vec::new(),
            next_tag: 0,
            maximum,
        }
    }

    /// How many publications are held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    /// Whether nothing is published.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    /// The document currently published for a presentity, if any.
    #[must_use]
    pub fn document(&self, entity: &str) -> Option<&str> {
        self.held
            .iter()
            .find(|entry| entry.entity == entity)
            .map(|entry| entry.body.as_str())
    }

    /// Apply a publication.
    pub fn apply(&mut self, entity: &str, publish: Publish, now: u64) -> Published {
        match publish {
            Publish::Empty => Published::Invalid,
            Publish::Initial { body, expires } => {
                let expires = expires.min(self.maximum);
                let tag = self.mint();
                // A second publication for one presentity replaces the first. Composing several
                // publishers' documents is what the RFC calls composition policy, and it is a
                // policy question rather than a mechanism — so it belongs to whoever has one.
                self.held.retain(|entry| entry.entity != entity);
                self.held.push(Entry {
                    tag: tag.clone(),
                    entity: entity.to_owned(),
                    body,
                    expires_at: now.saturating_add(expires.as_secs()),
                });
                Published::Accepted { tag, expires }
            }
            Publish::Refresh { tag, expires } => {
                let expires = expires.min(self.maximum);
                let Some(index) = self.find(&tag, now) else {
                    return Published::ConditionFailed;
                };
                let fresh = self.mint();
                let Some(entry) = self.held.get_mut(index) else {
                    return Published::ConditionFailed;
                };
                entry.tag.clone_from(&fresh);
                entry.expires_at = now.saturating_add(expires.as_secs());
                Published::Accepted {
                    tag: fresh,
                    expires,
                }
            }
            Publish::Modify { tag, body, expires } => {
                let expires = expires.min(self.maximum);
                let Some(index) = self.find(&tag, now) else {
                    return Published::ConditionFailed;
                };
                let fresh = self.mint();
                let Some(entry) = self.held.get_mut(index) else {
                    return Published::ConditionFailed;
                };
                entry.tag.clone_from(&fresh);
                entry.body = body;
                entry.expires_at = now.saturating_add(expires.as_secs());
                Published::Accepted {
                    tag: fresh,
                    expires,
                }
            }
            Publish::Remove { tag } => {
                let Some(index) = self.find(&tag, now) else {
                    return Published::ConditionFailed;
                };
                self.held.remove(index);
                Published::Removed
            }
        }
    }

    /// Forget everything that has run out of time.
    pub fn expire(&mut self, now: u64) -> usize {
        let before = self.held.len();
        self.held.retain(|entry| entry.expires_at > now);
        before - self.held.len()
    }

    /// The index of live state with this tag.
    ///
    /// Expiry is checked here rather than only in `expire`, so a refresh arriving after the state
    /// lapsed is refused whether or not anyone has swept yet. Otherwise whether a publisher is
    /// told 412 would depend on how recently a timer ran.
    fn find(&self, tag: &str, now: u64) -> Option<usize> {
        self.held
            .iter()
            .position(|entry| entry.tag == tag && entry.expires_at > now)
    }

    fn mint(&mut self) -> String {
        self.next_tag = self.next_tag.saturating_add(1);
        format!("sipx-{:016x}", self.next_tag)
    }
}

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
    use sipx_sip::event::Packages;

    const NOW: u64 = 1_700_000_000;

    fn document() -> String {
        Pidf::new("sip:alice@sipx.test")
            .with(Tuple::open("t1").at("sip:alice@192.0.2.5"))
            .to_xml()
    }

    fn compositor() -> Compositor {
        Compositor::new(Duration::from_secs(3600))
    }

    /// The story's failing-first test.
    ///
    /// Somebody publishes presence; a watcher subscribed to the `presence` package gets the
    /// document. That is the whole chain, and every piece of it is here rather than assumed.
    #[test]
    fn a_published_presence_document_reaches_a_subscriber() {
        // The notifier serves `presence`, so a SUBSCRIBE for it is not refused (`S-13`).
        let packages = Packages::new().with("presence");
        assert!(packages.serves("presence"));

        let mut compositor = compositor();
        let published = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(600)),
            NOW,
        );
        assert!(matches!(published, Published::Accepted { .. }));

        // What a NOTIFY for that subscription would carry.
        let notified = compositor
            .document("sip:alice@sipx.test")
            .expect("a subscriber gets the document that was published");
        assert!(notified.contains("<basic>open</basic>"), "{notified}");
        assert!(
            notified.contains("sip:alice@192.0.2.5"),
            "and the contact it named: {notified}"
        );
    }

    /// §4.1: the three operations differ by what is present.
    #[test]
    fn the_operation_is_read_from_the_tag_the_body_and_the_expiry() {
        let hour = Duration::from_secs(3600);
        assert!(matches!(
            Publish::read(None, Some("doc".to_owned()), hour),
            Publish::Initial { .. }
        ));
        assert!(matches!(
            Publish::read(Some("t".to_owned()), None, hour),
            Publish::Refresh { .. }
        ));
        assert!(matches!(
            Publish::read(Some("t".to_owned()), Some("doc".to_owned()), hour),
            Publish::Modify { .. }
        ));
        assert!(matches!(
            Publish::read(Some("t".to_owned()), None, Duration::ZERO),
            Publish::Remove { .. }
        ));
        // §6 step 5: neither a body nor a tag is not an empty publication — there is nothing to
        // publish and nothing to identify, so there is no operation it could be.
        assert_eq!(Publish::read(None, None, hour), Publish::Empty);
    }

    /// §6 step 3: "If no match is found, the ESC MUST reject the publication with a response of
    /// 412 (Conditional Request Failed)".
    #[test]
    fn a_tag_this_server_does_not_hold_is_refused_with_412() {
        let mut compositor = compositor();
        assert_eq!(
            compositor.apply(
                "sip:alice@sipx.test",
                Publish::read(
                    Some("nobody-issued-this".to_owned()),
                    None,
                    Duration::from_secs(600)
                ),
                NOW,
            ),
            Published::ConditionFailed
        );
        assert_eq!(CONDITIONAL_REQUEST_FAILED, 412);
    }

    /// The case the story singles out: state that expired while the publisher was not looking.
    /// Accepting the refresh as a new publication would resurrect a document the server had
    /// already forgotten and that nothing has re-sent.
    #[test]
    fn a_refresh_of_expired_state_is_refused_rather_than_accepted_as_new() {
        let mut compositor = compositor();
        let Published::Accepted { tag, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(60)),
            NOW,
        ) else {
            panic!("the first publication is accepted");
        };

        let refresh = Publish::read(Some(tag), None, Duration::from_secs(60));
        assert_eq!(
            compositor.apply("sip:alice@sipx.test", refresh, NOW + 61),
            Published::ConditionFailed,
            "the state had lapsed; the publisher must start again"
        );
    }

    /// And it is refused whether or not anyone has swept — otherwise whether a publisher is told
    /// 412 depends on how recently a timer ran.
    #[test]
    fn expiry_is_judged_on_the_clock_and_not_on_whether_a_sweep_has_happened() {
        let mut compositor = compositor();
        let Published::Accepted { tag, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(60)),
            NOW,
        ) else {
            panic!("accepted");
        };
        assert_eq!(compositor.len(), 1, "nothing has swept it yet");
        assert_eq!(
            compositor.apply(
                "sip:alice@sipx.test",
                Publish::read(Some(tag), None, Duration::from_secs(60)),
                NOW + 61
            ),
            Published::ConditionFailed
        );
    }

    /// §6 step 6: the response carries a `SIP-ETag`, and a fresh one each time. A publisher that
    /// kept using its old tag after a refresh would be rejected on its next attempt.
    #[test]
    fn every_acceptance_issues_a_new_tag() {
        let mut compositor = compositor();
        let Published::Accepted { tag: first, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(600)),
            NOW,
        ) else {
            panic!("accepted");
        };
        let Published::Accepted { tag: second, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(Some(first.clone()), None, Duration::from_secs(600)),
            NOW + 10,
        ) else {
            panic!("the refresh is accepted");
        };
        assert_ne!(first, second, "a refresh issues a fresh tag");

        // And the old tag is no longer good, which is what makes the new one meaningful.
        assert_eq!(
            compositor.apply(
                "sip:alice@sipx.test",
                Publish::read(Some(first), None, Duration::from_secs(600)),
                NOW + 20
            ),
            Published::ConditionFailed
        );
    }

    /// §6 step 5: `Expires: 0` removes the state the tag identifies.
    #[test]
    fn expires_zero_removes_the_publication() {
        let mut compositor = compositor();
        let Published::Accepted { tag, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(600)),
            NOW,
        ) else {
            panic!("accepted");
        };
        assert_eq!(
            compositor.apply(
                "sip:alice@sipx.test",
                Publish::read(Some(tag), None, Duration::ZERO),
                NOW
            ),
            Published::Removed
        );
        assert!(compositor.is_empty());
        assert!(compositor.document("sip:alice@sipx.test").is_none());
    }

    #[test]
    fn a_modification_replaces_the_document() {
        let mut compositor = compositor();
        let Published::Accepted { tag, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(600)),
            NOW,
        ) else {
            panic!("accepted");
        };
        let away = Pidf::new("sip:alice@sipx.test")
            .with(Tuple::closed("t1").with_note("in a meeting"))
            .to_xml();
        assert!(matches!(
            compositor.apply(
                "sip:alice@sipx.test",
                Publish::read(Some(tag), Some(away), Duration::from_secs(600)),
                NOW
            ),
            Published::Accepted { .. }
        ));
        let held = compositor
            .document("sip:alice@sipx.test")
            .expect("a document");
        assert!(held.contains("<basic>closed</basic>"), "{held}");
        assert!(held.contains("in a meeting"), "{held}");
    }

    /// A publication with neither a body nor a tag is refused (§6 step 5).
    #[test]
    fn a_publication_with_nothing_in_it_is_invalid() {
        let mut compositor = compositor();
        assert_eq!(
            compositor.apply(
                "sip:alice@sipx.test",
                Publish::read(None, None, Duration::from_secs(600)),
                NOW
            ),
            Published::Invalid
        );
        assert!(compositor.is_empty());
    }

    #[test]
    fn a_pidf_document_is_typed_rather_than_concatenated() {
        let xml = Pidf::new("sip:alice@sipx.test")
            .with(
                Tuple::open("t1")
                    .at("sip:alice@192.0.2.5")
                    .with_priority(0.8)
                    .with_note("at my desk"),
            )
            .to_xml();
        assert!(
            xml.contains("xmlns=\"urn:ietf:params:xml:ns:pidf\""),
            "{xml}"
        );
        assert!(xml.contains("entity=\"sip:alice@sipx.test\""), "{xml}");
        assert!(xml.contains("<basic>open</basic>"), "{xml}");
        assert!(xml.contains("priority=\"0.8\""), "{xml}");
        assert!(xml.contains("<note>at my desk</note>"), "{xml}");
        assert_eq!(PIDF_TYPE, "application/pidf+xml");
    }

    /// §4.1.4 fixes the priority range. A document carrying 7.5 is one a watcher may reject
    /// outright, losing the whole presence rather than one number.
    #[test]
    fn a_priority_outside_the_range_is_clamped_rather_than_emitted() {
        let tuple = Tuple::open("t1").at("sip:a@b").with_priority(7.5);
        assert_eq!(tuple.priority, Some(1.0));
        let low = Tuple::open("t1").at("sip:a@b").with_priority(-3.0);
        assert_eq!(low.priority, Some(0.0));
    }

    #[test]
    fn xml_metacharacters_in_a_note_do_not_break_the_document() {
        let xml = Pidf::new("sip:alice@sipx.test")
            .with(Tuple::open("t1").with_note("tea & <biscuits>"))
            .to_xml();
        assert!(xml.contains("tea &amp; &lt;biscuits&gt;"), "{xml}");
        assert!(!xml.contains("<biscuits>"), "{xml}");
    }

    #[test]
    fn a_second_publication_for_one_presentity_replaces_the_first() {
        let mut compositor = compositor();
        let _ = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(600)),
            NOW,
        );
        let _ = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(600)),
            NOW,
        );
        assert_eq!(
            compositor.len(),
            1,
            "one presentity, one published document"
        );
    }

    #[test]
    fn the_compositor_shortens_a_generous_expiry_to_its_maximum() {
        let mut compositor = Compositor::new(Duration::from_secs(300));
        let Published::Accepted { expires, .. } = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(86400)),
            NOW,
        ) else {
            panic!("accepted");
        };
        assert_eq!(expires, Duration::from_secs(300));
    }

    #[test]
    fn expiring_forgets_what_has_run_out() {
        let mut compositor = compositor();
        let _ = compositor.apply(
            "sip:alice@sipx.test",
            Publish::read(None, Some(document()), Duration::from_secs(60)),
            NOW,
        );
        assert_eq!(compositor.expire(NOW + 30), 0, "not yet");
        assert_eq!(compositor.expire(NOW + 61), 1);
        assert!(compositor.is_empty());
    }
}
