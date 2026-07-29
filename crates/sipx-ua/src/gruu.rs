//! GRUUs: obtaining them from a registrar, and choosing which one to use (RFC 5627).
//!
//! A registration says "this user is reachable here". A GRUU says "*this device* is reachable
//! here", which is a different claim and the one a transfer, a conference invitation or a
//! callback needs: an address of record resolves to every phone the user has registered, and a
//! request routed to all of them is not a request routed back to the one that was talking.
//!
//! sipx **obtains and uses** GRUUs. Minting them is §5's registrar behaviour and is not here —
//! sipx is not a registrar, and the two halves share nothing but the wire format.
//!
//! Three things are worth knowing before reading on:
//!
//! - **The instance ID is the same one Outbound registers with.** §4.1 identifies the instance
//!   with the `+sip.instance` media feature tag that RFC 5626 §4.1 also defines. Two mechanisms,
//!   one identity — [`Registration`](crate::registrar::Registration) holds it in a single field
//!   so that they cannot come to hold two, which is a fault that only appears under a registrar
//!   that correlates them.
//! - **The two GRUUs are not interchangeable.** See [`Kind`]. Substituting one for the other
//!   silently is the failure this module goes out of its way not to have.
//! - **Recognising one needs more than URI equality.** §5.4: "A public GRUU will always be
//!   equivalent to the AOR based on URI equality rules." The comparison that does not make that
//!   mistake is [`sipx_sip::gruu::addressed_to`].

use bytes::Bytes;
use sipx_sip::headers::ContactValue;
use sipx_sip::{Address, Response, Uri};

use crate::outbound::InstanceId;

/// The option tag a UA offers to ask for a GRUU.
///
/// §4.1: a compliant UA "MUST include the Supported header field" in every REGISTER and "the
/// value of that header field MUST include 'gruu' as one of the option tags". A registrar that
/// does not see it has been told nothing was asked for, and §5.2 has it attach nothing.
pub const OPTION_TAG: &str = "gruu";

/// The `Contact` header field parameter carrying the public GRUU (§7).
const PUB_GRUU_PARAM: &str = "pub-gruu";

/// The `Contact` header field parameter carrying the temporary GRUU (§7).
const TEMP_GRUU_PARAM: &str = "temp-gruu";

/// The `Contact` header field parameter naming the instance a binding belongs to.
///
/// RFC 5626 §4.1's media feature tag, which §4.1 of this RFC reuses rather than inventing a
/// second name for the same fact.
const INSTANCE_PARAM: &str = "+sip.instance";

/// Which of the two GRUUs a UA puts in a `Contact` (§4.4).
///
/// # Why the default is [`Kind::Public`]
///
/// The two differ in exactly one property each way, and the trade is not sipx's to make.
///
/// A **public** GRUU is a stable identifier for the instance. It survives re-registration, and
/// §5.2 has a registrar keep treating it as valid even after the binding lapses — answering 480
/// until the device comes back — so an address handed out under one still names this device
/// tomorrow. What it does not offer is privacy: two public GRUUs for one instance are the same
/// URI, so anyone holding two of them can see they are the same device.
///
/// A **temporary** GRUU buys precisely that privacy. §5.4: "Given a pair of GRUUs, it MUST be
/// computationally infeasible to determine whether they were issued for the same AOR or instance
/// ID or for different AORs and instance IDs." It pays for it in lifetime — §4.2 requires a UA
/// to discard every temporary GRUU it learned whenever its `Call-ID` changes, so an address
/// handed out under one stops resolving as soon as the UA registers afresh.
///
/// So the default is the public one, because it is the choice that keeps working, and because
/// only the application knows whether the address it is about to put in a `Contact` has to
/// outlive this registration. Unlinkability is a property that must be *asked* for — and, having
/// been asked for, is never quietly downgraded: see [`Gruus::preferred`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// One stable URI for this instance, usable for as long as the instance exists.
    #[default]
    Public,
    /// An address that cannot be correlated with any other GRUU, and that lapses with the
    /// registration that produced it.
    Temporary,
}

/// The GRUUs a registrar issued for one instance's binding (§4.2, §5.2).
///
/// Held with the registration rather than beside it, and replaced wholesale on every 2xx. That
/// is not tidiness: §4.2 requires a UA to "discard all temporary GRUUs learned through prior
/// REGISTER responses" whenever the `Call-ID` changes, and a set that is replaced rather than
/// merged cannot carry a stale one across.
#[derive(Debug, Clone, Default)]
pub struct Gruus {
    public: Option<Uri>,
    temporary: Option<Uri>,
}

impl Gruus {
    /// Read the GRUUs a REGISTER 2xx issued for `instance` (§4.2).
    ///
    /// §4.2 pairs the two parameters with the `Contact` carrying the `+sip.instance` they were
    /// minted for — and a 2xx lists *every* current binding for the address of record, other
    /// devices' included (RFC 3261 §10.3). Selecting the row by instance rather than by position
    /// is what stops this adopting another phone's GRUU and then answering to it.
    ///
    /// A value that is not a GRUU is dropped rather than kept. §7 gives a GRUU the `gr`
    /// parameter, and a URI without one is the address of record: using it as though it named
    /// this instance would route every one of the user's devices at a request meant for one.
    #[must_use]
    pub fn from_response(response: &Response, instance: &InstanceId) -> Self {
        for value in response.headers.typed_all::<ContactValue>() {
            let Ok(ContactValue::Address(address)) = value else {
                continue;
            };
            if !names_instance(&address, instance) {
                continue;
            }
            return Self {
                public: gruu_param(&address, PUB_GRUU_PARAM),
                temporary: gruu_param(&address, TEMP_GRUU_PARAM),
            };
        }
        Self::default()
    }

    /// The public GRUU, if the registrar issued one.
    #[must_use]
    pub fn public(&self) -> Option<&Uri> {
        self.public.as_ref()
    }

    /// The temporary GRUU, if the registrar issued one.
    #[must_use]
    pub fn temporary(&self) -> Option<&Uri> {
        self.temporary.as_ref()
    }

    /// Whether the registrar issued neither.
    ///
    /// §4.2: "A UA must be prepared for a Contact to contain just one, both, or neither" — a
    /// registrar that does not implement RFC 5627 answers a REGISTER perfectly well and attaches
    /// nothing, and that is not an error.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.public.is_none() && self.temporary.is_none()
    }

    /// The GRUU to use for a caller that asked for `kind` (§4.4).
    ///
    /// **One never stands in for the other.** Asking for a temporary GRUU and being handed the
    /// public one would tell the caller the opposite of the truth about what it just put in a
    /// `Contact`: it believes it published an address nobody can correlate, and it published the
    /// device's permanent name. `None` — falling back to the ordinary contact — is the honest
    /// answer, and it is the one that leaks least.
    #[must_use]
    pub fn preferred(&self, kind: Kind) -> Option<&Uri> {
        match kind {
            Kind::Public => self.public(),
            Kind::Temporary => self.temporary(),
        }
    }

    /// Whether a request whose Request-URI is `request_uri` was sent to one of these (§4.5).
    #[must_use]
    pub fn sent_to(&self, request_uri: &Uri) -> bool {
        [self.public.as_ref(), self.temporary.as_ref()]
            .into_iter()
            .flatten()
            .any(|ours| sipx_sip::gruu::addressed_to(request_uri, ours))
    }

    /// Each GRUU rendered back to text, for logging and for comparison.
    #[must_use]
    fn rendered(&self) -> Vec<Option<String>> {
        vec![
            self.public.as_ref().map(Uri::to_string),
            self.temporary.as_ref().map(Uri::to_string),
        ]
    }
}

/// Compared by what the two URIs say, because [`Uri`] deliberately has no `PartialEq`: RFC 3261
/// §19.1.4 equivalence is not transitive, and it is the wrong relation here anyway — two
/// registrations that returned *different spellings* of one GRUU did return different values.
impl PartialEq for Gruus {
    fn eq(&self, other: &Self) -> bool {
        self.rendered() == other.rendered()
    }
}

impl Eq for Gruus {}

/// Whether this `Contact` is the binding for `instance` (§4.2).
///
/// The angle brackets are part of the wire form — RFC 5626 §4.1 quotes the URN inside them — and
/// are stripped from both sides so that a registrar echoing the parameter in either spelling is
/// still recognised.
#[must_use]
fn names_instance(address: &Address, instance: &InstanceId) -> bool {
    address
        .param(INSTANCE_PARAM)
        .is_some_and(|value| trim_brackets(value).eq_ignore_ascii_case(instance.urn().as_bytes()))
}

/// One of the two GRUU `Contact` parameters, parsed (§7).
///
/// §7 makes both a `quoted-string`; the parser has already removed the quotes and resolved the
/// escapes. Angle brackets are stripped as well, because they are not in the grammar and are
/// exactly the sort of thing an implementation adds anyway — accepting them costs nothing and
/// rejecting the GRUU costs reachability.
#[must_use]
fn gruu_param(address: &Address, name: &str) -> Option<Uri> {
    let raw = trim_brackets(address.param(name)?);
    let uri = Uri::parse(Bytes::copy_from_slice(raw)).ok()?;
    sipx_sip::gruu::is_gruu(&uri).then_some(uri)
}

/// Strip one layer of `<`/`>`, and the whitespace around it.
#[must_use]
fn trim_brackets(value: &[u8]) -> &[u8] {
    let value = value.trim_ascii();
    match (value.first(), value.last()) {
        (Some(b'<'), Some(b'>')) if value.len() >= 2 => {
            value.get(1..value.len() - 1).unwrap_or_default()
        }
        _ => value,
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
    use sipx_sip::{Limits, Message, parse_datagram};

    const INSTANCE: &str = "urn:uuid:f81d4fae-7dec-11d0-a765-00a0c91e6bf6";
    const OTHER_INSTANCE: &str = "urn:uuid:00000000-0000-4000-8000-000000000000";

    fn instance() -> InstanceId {
        InstanceId::parse(INSTANCE).expect("a urn")
    }

    fn ok_with(contacts: &str) -> Response {
        let text = format!(
            "SIP/2.0 200 OK\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             {contacts}\
             Content-Length: 0\r\n\r\n"
        );
        match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses") {
            Message::Response(r) => r,
            Message::Request(_) => panic!("a response"),
        }
    }

    fn issued(contacts: &str) -> Gruus {
        Gruus::from_response(&ok_with(contacts), &instance())
    }

    #[test]
    fn both_gruus_are_read_from_the_binding_that_names_this_instance() {
        let gruus = issued(&format!(
            "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;pub-gruu=\"sip:alice@example.com;gr={INSTANCE}\"\
             ;temp-gruu=\"sip:t7k2xq9f4m@example.com;gr\";expires=3600\r\n"
        ));
        assert_eq!(
            gruus.public().map(Uri::to_string),
            Some(format!("sip:alice@example.com;gr={INSTANCE}"))
        );
        assert_eq!(
            gruus.temporary().map(Uri::to_string),
            Some("sip:t7k2xq9f4m@example.com;gr".to_owned())
        );
    }

    /// §4.2: "A UA must be prepared for a Contact to contain just one, both, or neither."
    #[test]
    fn one_both_or_neither_are_all_ordinary_answers() {
        assert!(
            issued(&format!(
                "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\r\n"
            ))
            .is_empty()
        );
        let public_only = issued(&format!(
            "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;pub-gruu=\"sip:alice@example.com;gr={INSTANCE}\"\r\n"
        ));
        assert!(public_only.public().is_some() && public_only.temporary().is_none());
        let temp_only = issued(&format!(
            "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;temp-gruu=\"sip:t7k2xq9f4m@example.com;gr\"\r\n"
        ));
        assert!(temp_only.public().is_none() && temp_only.temporary().is_some());
    }

    /// RFC 3261 §10.3 has a 2xx list *every* binding for the address of record. Reading the GRUUs
    /// off the first row would adopt whichever device happened to register first and answer to a
    /// URI that routes somewhere else entirely.
    #[test]
    fn another_devices_gruu_is_not_ours() {
        let gruus = issued(&format!(
            "Contact: <sip:alice@198.51.100.9:5060>;+sip.instance=\"<{OTHER_INSTANCE}>\"\
             ;pub-gruu=\"sip:alice@example.com;gr={OTHER_INSTANCE}\"\r\n\
             Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;pub-gruu=\"sip:alice@example.com;gr={INSTANCE}\"\r\n"
        ));
        assert_eq!(
            gruus.public().map(Uri::to_string),
            Some(format!("sip:alice@example.com;gr={INSTANCE}")),
            "the row for another instance was taken for ours"
        );
    }

    /// A binding with no `+sip.instance` is not this instance's, whatever it carries.
    #[test]
    fn a_binding_that_names_no_instance_yields_nothing() {
        assert!(
            issued(
                "Contact: <sip:alice@192.0.2.5:5060>;pub-gruu=\"sip:alice@example.com;gr=x\"\r\n"
            )
            .is_empty()
        );
        assert!(issued("").is_empty());
    }

    /// §7 gives a GRUU the `gr` parameter. Without it the value is the address of record, and
    /// putting *that* in a `Contact` as though it named this instance would fan a mid-dialog
    /// request out to every device the user has.
    #[test]
    fn a_pub_gruu_without_the_gr_parameter_is_not_a_gruu() {
        assert!(
            issued(&format!(
                "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
                 ;pub-gruu=\"sip:alice@example.com\"\r\n"
            ))
            .is_empty()
        );
    }

    /// §7 spells both parameters as quoted strings and neither in angle brackets, but a value
    /// that arrives bracketed is still the URI it names.
    #[test]
    fn a_bracketed_value_is_accepted_the_same_as_a_bare_one() {
        let gruus = issued(&format!(
            "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;pub-gruu=\"<sip:alice@example.com;gr={INSTANCE}>\"\r\n"
        ));
        assert_eq!(
            gruus.public().map(Uri::to_string),
            Some(format!("sip:alice@example.com;gr={INSTANCE}"))
        );
    }

    /// The point of the type: a caller that asked for privacy is told it did not get it, rather
    /// than being handed the stable identifier and left believing otherwise.
    #[test]
    fn a_missing_temporary_gruu_is_never_answered_with_the_public_one() {
        let gruus = issued(&format!(
            "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;pub-gruu=\"sip:alice@example.com;gr={INSTANCE}\"\r\n"
        ));
        assert!(gruus.preferred(Kind::Public).is_some());
        assert!(
            gruus.preferred(Kind::Temporary).is_none(),
            "§5.4's unlinkability is not something a public GRUU can stand in for"
        );
    }

    #[test]
    fn the_default_choice_is_the_public_gruu() {
        assert_eq!(Kind::default(), Kind::Public);
    }

    #[test]
    fn a_request_is_recognised_only_when_it_names_one_of_our_gruus() {
        let gruus = issued(&format!(
            "Contact: <sip:alice@192.0.2.5:5060>;+sip.instance=\"<{INSTANCE}>\"\
             ;pub-gruu=\"sip:alice@example.com;gr={INSTANCE}\"\
             ;temp-gruu=\"sip:t7k2xq9f4m@example.com;gr\"\r\n"
        ));
        let uri = |text: &str| Uri::parse(Bytes::from(text.to_owned())).expect("a URI");
        assert!(gruus.sent_to(&uri(&format!("sip:alice@example.com;gr={INSTANCE}"))));
        assert!(gruus.sent_to(&uri("sip:t7k2xq9f4m@example.com;gr")));
        // §5.4: the address of record is URI-equivalent to the public GRUU and must still not
        // count, because it names every device rather than this one.
        assert!(!gruus.sent_to(&uri("sip:alice@example.com")));
        assert!(!gruus.sent_to(&uri(&format!("sip:alice@example.com;gr={OTHER_INSTANCE}"))));
        assert!(!Gruus::default().sent_to(&uri(&format!("sip:alice@example.com;gr={INSTANCE}"))));
    }
}
