//! Bounded RFC 3680 registration-information consumer.
//!
//! This is package policy behind [`crate::event_client::PackageConsumer`], not another event
//! client. The normative merge and refusal rules are in `docs/specs/registration-discovery.md`.

use std::collections::{HashMap, HashSet};

use bytes::Bytes;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use sipx_sip::Uri;

use crate::event_client::{PackageConsumer, PackageRejection};
use crate::packages::REGINFO_TYPE;

const NAMESPACE: Namespace<'static> = Namespace(b"urn:ietf:params:xml:ns:reginfo");

/// Where a registration snapshot was learned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrarSource {
    /// Exact resource URI subscribed at the registrar.
    pub resource: String,
}

/// One currently active registrar contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationPeer {
    /// User-facing name derived from the registration AOR.
    pub name: String,
    /// Address of record which owns this contact.
    pub aor: String,
    /// Exact active SIP or SIPS contact URI.
    pub uri: String,
    /// Stable registration key from the document.
    pub registration_id: String,
    /// Stable contact key from the document.
    pub contact_id: String,
    /// Registrar resource which supplied this fact.
    pub source: RegistrarSource,
}

/// Complete current state after one full or partial document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationSnapshot {
    /// Last applied document version.
    pub version: u32,
    /// Active contacts, sorted by AOR, URI and document keys.
    pub peers: Vec<RegistrationPeer>,
}

/// Stateful, bounded `reg` package consumer.
#[derive(Debug)]
pub struct RegistrationConsumer {
    source: RegistrarSource,
    contact_limit: usize,
    version: Option<u32>,
    peers: HashMap<(String, String), RegistrationPeer>,
    accepts: Vec<String>,
}

impl RegistrationConsumer {
    /// Construct one registrar view. A zero contact limit is rejected.
    pub fn new(
        resource: impl Into<String>,
        contact_limit: usize,
    ) -> Result<Self, PackageRejection> {
        let resource = resource.into();
        if contact_limit == 0 || !is_sip_uri(&resource) {
            return Err(PackageRejection::malformed());
        }
        Ok(Self {
            source: RegistrarSource { resource },
            contact_limit,
            version: None,
            peers: HashMap::new(),
            accepts: vec![REGINFO_TYPE.to_owned()],
        })
    }

    fn apply(&mut self, document: Document) -> Result<RegistrationSnapshot, PackageRejection> {
        match (self.version, document.kind) {
            (None, DocumentKind::Full) if document.version == 0 => {}
            (Some(previous), DocumentKind::Partial)
                if previous.checked_add(1) == Some(document.version) => {}
            (Some(previous), DocumentKind::Full) if document.version > previous => {}
            _ => return Err(PackageRejection::malformed()),
        }

        let mut next = if document.kind == DocumentKind::Full {
            HashMap::new()
        } else {
            self.peers.clone()
        };
        for registration in document.registrations {
            if matches!(
                registration.state,
                RegistrationState::Init | RegistrationState::Terminated
            ) {
                next.retain(|(registration_id, _), _| registration_id != &registration.id);
                continue;
            }
            for contact in registration.contacts {
                let key = (registration.id.clone(), contact.id.clone());
                if next.keys().any(|(registration_id, contact_id)| {
                    contact_id == &contact.id && registration_id != &registration.id
                }) {
                    return Err(PackageRejection::malformed());
                }
                if contact.active()? {
                    let uri = contact.uri.ok_or_else(PackageRejection::malformed)?;
                    if !is_sip_uri(&uri) {
                        return Err(PackageRejection::malformed());
                    }
                    next.insert(
                        key,
                        RegistrationPeer {
                            name: peer_name(&registration.aor),
                            aor: registration.aor.clone(),
                            uri,
                            registration_id: registration.id.clone(),
                            contact_id: contact.id,
                            source: self.source.clone(),
                        },
                    );
                    if next.len() > self.contact_limit {
                        return Err(PackageRejection { status: 413 });
                    }
                } else {
                    next.remove(&key);
                }
            }
        }

        let mut peers: Vec<_> = next.values().cloned().collect();
        peers.sort_by(|left, right| {
            (
                &left.aor,
                &left.uri,
                &left.registration_id,
                &left.contact_id,
            )
                .cmp(&(
                    &right.aor,
                    &right.uri,
                    &right.registration_id,
                    &right.contact_id,
                ))
        });
        self.version = Some(document.version);
        self.peers = next;
        Ok(RegistrationSnapshot {
            version: document.version,
            peers,
        })
    }
}

impl PackageConsumer for RegistrationConsumer {
    type Value = RegistrationSnapshot;

    fn event(&self) -> &'static str {
        "reg"
    }

    fn accept(&self) -> &[String] {
        &self.accepts
    }

    fn neutral(&mut self) -> Option<Self::Value> {
        None
    }

    fn empty_terminal_is_valid(&self) -> bool {
        true
    }

    fn consume(
        &mut self,
        content_type: Option<&[u8]>,
        body: &[u8],
    ) -> Result<Self::Value, PackageRejection> {
        if !content_type.is_some_and(reginfo_content_type) {
            return Err(PackageRejection::unsupported_media());
        }
        let text = std::str::from_utf8(body).map_err(|_| PackageRejection::malformed())?;
        self.apply(parse(text)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentKind {
    Full,
    Partial,
}

#[derive(Debug)]
struct Document {
    version: u32,
    kind: DocumentKind,
    registrations: Vec<Registration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationState {
    Init,
    Active,
    Terminated,
}

#[derive(Debug)]
struct Registration {
    aor: String,
    id: String,
    state: RegistrationState,
    contacts: Vec<ContactChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContactState {
    Active,
    Terminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContactEvent {
    Registered,
    Created,
    Refreshed,
    Shortened,
    Expired,
    Deactivated,
    Probation,
    Unregistered,
    Rejected,
}

#[derive(Debug)]
struct ContactChange {
    id: String,
    state: ContactState,
    event: ContactEvent,
    uri: Option<String>,
}

impl ContactChange {
    fn active(&self) -> Result<bool, PackageRejection> {
        match (self.state, self.event) {
            (
                ContactState::Active,
                ContactEvent::Registered
                | ContactEvent::Created
                | ContactEvent::Refreshed
                | ContactEvent::Shortened,
            ) => Ok(true),
            (
                ContactState::Terminated,
                ContactEvent::Expired
                | ContactEvent::Deactivated
                | ContactEvent::Probation
                | ContactEvent::Unregistered
                | ContactEvent::Rejected,
            ) => Ok(false),
            _ => Err(PackageRejection::malformed()),
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the XML state stack stays in one fail-closed event table so nesting order is auditable"
)]
fn parse(input: &str) -> Result<Document, PackageRejection> {
    let mut reader = NsReader::from_str(input);
    reader.config_mut().trim_text(true);
    let mut document: Option<Document> = None;
    let mut registration: Option<Registration> = None;
    let mut contact: Option<ContactChange> = None;
    let mut in_uri = false;
    let mut foreign_depth = 0_usize;
    let mut seen_registrations = HashSet::new();
    let mut seen_contacts = HashSet::new();

    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|_| PackageRejection::malformed())?;
        match event {
            Event::Start(_) if foreign_depth > 0 => {
                foreign_depth = foreign_depth.saturating_add(1);
            }
            Event::End(_) if foreign_depth > 0 => foreign_depth = foreign_depth.saturating_sub(1),
            _ if foreign_depth > 0 => {}
            Event::Start(_) if !native(&namespace) => foreign_depth = 1,
            Event::Start(element) => match element.local_name().as_ref() {
                b"reginfo" if document.is_none() => {
                    document = Some(document_start(&reader, &element)?);
                }
                b"registration" if document.is_some() && registration.is_none() => {
                    let parsed = registration_start(&reader, &element)?;
                    if !seen_registrations.insert(parsed.id.clone()) {
                        return Err(PackageRejection::malformed());
                    }
                    registration = Some(parsed);
                }
                b"contact" if registration.is_some() && contact.is_none() => {
                    let parsed = contact_start(&reader, &element)?;
                    if !seen_contacts.insert(parsed.id.clone()) {
                        return Err(PackageRejection::malformed());
                    }
                    contact = Some(parsed);
                }
                b"uri" if contact.is_some() && !in_uri => in_uri = true,
                _ => return Err(PackageRejection::malformed()),
            },
            Event::Empty(element) if native(&namespace) => match element.local_name().as_ref() {
                b"reginfo" if document.is_none() => {
                    document = Some(document_start(&reader, &element)?);
                }
                b"registration" if document.is_some() && registration.is_none() => {
                    let parsed = registration_start(&reader, &element)?;
                    if !seen_registrations.insert(parsed.id.clone()) {
                        return Err(PackageRejection::malformed());
                    }
                    document
                        .as_mut()
                        .ok_or_else(PackageRejection::malformed)?
                        .registrations
                        .push(parsed);
                }
                b"contact" if registration.is_some() && contact.is_none() => {
                    let parsed = contact_start(&reader, &element)?;
                    if !seen_contacts.insert(parsed.id.clone()) {
                        return Err(PackageRejection::malformed());
                    }
                    registration
                        .as_mut()
                        .ok_or_else(PackageRejection::malformed)?
                        .contacts
                        .push(parsed);
                }
                _ => return Err(PackageRejection::malformed()),
            },
            Event::End(element) if native(&namespace) => match element.local_name().as_ref() {
                b"uri" if in_uri => in_uri = false,
                b"contact" if contact.is_some() && !in_uri => {
                    registration
                        .as_mut()
                        .ok_or_else(PackageRejection::malformed)?
                        .contacts
                        .push(contact.take().ok_or_else(PackageRejection::malformed)?);
                }
                b"registration" if registration.is_some() && contact.is_none() => {
                    document
                        .as_mut()
                        .ok_or_else(PackageRejection::malformed)?
                        .registrations
                        .push(
                            registration
                                .take()
                                .ok_or_else(PackageRejection::malformed)?,
                        );
                }
                b"reginfo" if registration.is_none() && contact.is_none() => {}
                _ => return Err(PackageRejection::malformed()),
            },
            Event::Text(text) if in_uri => append_uri(
                contact.as_mut().ok_or_else(PackageRejection::malformed)?,
                &text.decode().map_err(|_| PackageRejection::malformed())?,
            ),
            Event::CData(text) if in_uri => append_uri(
                contact.as_mut().ok_or_else(PackageRejection::malformed)?,
                &text.decode().map_err(|_| PackageRejection::malformed())?,
            ),
            Event::GeneralRef(reference) if in_uri => {
                let decoded = reference
                    .decode()
                    .map_err(|_| PackageRejection::malformed())?;
                let value = match decoded.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "apos" => "'",
                    "quot" => "\"",
                    _ => return Err(PackageRejection::malformed()),
                };
                append_uri(
                    contact.as_mut().ok_or_else(PackageRejection::malformed)?,
                    value,
                );
            }
            Event::Text(_) | Event::Comment(_) | Event::Decl(_) | Event::Empty(_) => {}
            Event::DocType(_) | Event::GeneralRef(_) | Event::PI(_) | Event::CData(_) => {
                return Err(PackageRejection::malformed());
            }
            Event::Eof => break,
            Event::End(_) => return Err(PackageRejection::malformed()),
        }
    }

    if registration.is_some() || contact.is_some() || in_uri || foreign_depth != 0 {
        return Err(PackageRejection::malformed());
    }
    document.ok_or_else(PackageRejection::malformed)
}

fn document_start(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Document, PackageRejection> {
    let version = required_attribute(reader, element, b"version")?
        .parse::<u32>()
        .map_err(|_| PackageRejection::malformed())?;
    let kind = match required_attribute(reader, element, b"state")?.as_str() {
        "full" => DocumentKind::Full,
        "partial" => DocumentKind::Partial,
        _ => return Err(PackageRejection::malformed()),
    };
    Ok(Document {
        version,
        kind,
        registrations: Vec::new(),
    })
}

fn registration_start(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Registration, PackageRejection> {
    let aor = required_attribute(reader, element, b"aor")?;
    if !is_sip_uri(&aor) {
        return Err(PackageRejection::malformed());
    }
    let id = nonempty(required_attribute(reader, element, b"id")?)?;
    let state = match required_attribute(reader, element, b"state")?.as_str() {
        "init" => RegistrationState::Init,
        "active" => RegistrationState::Active,
        "terminated" => RegistrationState::Terminated,
        _ => return Err(PackageRejection::malformed()),
    };
    Ok(Registration {
        aor,
        id,
        state,
        contacts: Vec::new(),
    })
}

fn contact_start(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<ContactChange, PackageRejection> {
    let id = nonempty(required_attribute(reader, element, b"id")?)?;
    let state = match required_attribute(reader, element, b"state")?.as_str() {
        "active" => ContactState::Active,
        "terminated" => ContactState::Terminated,
        _ => return Err(PackageRejection::malformed()),
    };
    let event = match required_attribute(reader, element, b"event")?.as_str() {
        "registered" => ContactEvent::Registered,
        "created" => ContactEvent::Created,
        "refreshed" => ContactEvent::Refreshed,
        "shortened" => ContactEvent::Shortened,
        "expired" => ContactEvent::Expired,
        "deactivated" => ContactEvent::Deactivated,
        "probation" => ContactEvent::Probation,
        "unregistered" => ContactEvent::Unregistered,
        "rejected" => ContactEvent::Rejected,
        _ => return Err(PackageRejection::malformed()),
    };
    if event == ContactEvent::Shortened {
        required_attribute(reader, element, b"expires")?
            .parse::<u32>()
            .map_err(|_| PackageRejection::malformed())?;
    }
    if event == ContactEvent::Probation {
        required_attribute(reader, element, b"retry-after")?
            .parse::<u32>()
            .map_err(|_| PackageRejection::malformed())?;
    }
    Ok(ContactChange {
        id,
        state,
        event,
        uri: None,
    })
}

fn required_attribute(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    name: &[u8],
) -> Result<String, PackageRejection> {
    let mut found = None;
    for attribute in element.attributes() {
        let attribute = attribute.map_err(|_| PackageRejection::malformed())?;
        if attribute.key.as_ref() == name {
            if found.is_some() {
                return Err(PackageRejection::malformed());
            }
            found = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map_err(|_| PackageRejection::malformed())?
                    .into_owned(),
            );
        }
    }
    found.ok_or_else(PackageRejection::malformed)
}

fn append_uri(contact: &mut ContactChange, value: &str) {
    contact.uri.get_or_insert_with(String::new).push_str(value);
}

fn nonempty(value: String) -> Result<String, PackageRejection> {
    (!value.is_empty())
        .then_some(value)
        .ok_or_else(PackageRejection::malformed)
}

fn native(namespace: &ResolveResult<'_>) -> bool {
    matches!(namespace, ResolveResult::Bound(value) if *value == NAMESPACE)
}

fn reginfo_content_type(value: &[u8]) -> bool {
    value
        .split(|byte| *byte == b';')
        .next()
        .is_some_and(|media| {
            media
                .trim_ascii()
                .eq_ignore_ascii_case(REGINFO_TYPE.as_bytes())
        })
}

fn is_sip_uri(value: &str) -> bool {
    Uri::parse(Bytes::copy_from_slice(value.as_bytes())).is_ok_and(|uri| uri.scheme().is_sip())
}

fn peer_name(aor: &str) -> String {
    Uri::parse(Bytes::copy_from_slice(aor.as_bytes()))
        .ok()
        .and_then(|uri| uri.decoded_user())
        .filter(|user| !user.is_empty())
        .map_or_else(
            || aor.to_owned(),
            |user| String::from_utf8_lossy(&user).into_owned(),
        )
}
