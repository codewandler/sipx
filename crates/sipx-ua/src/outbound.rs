//! Client-initiated connections — Outbound (RFC 5626).
//!
//! The problem it solves: a UA behind a NAT registers a `Contact` naming an address that only
//! exists inside the NAT, and the binding the registrar records is unroutable the moment the
//! mapping lapses. Outbound's answer is to stop routing to an *address* and route down a **flow**
//! instead — the connection the client itself opened — identified by a token the registrar puts in
//! `Path` and hands back to itself later.
//!
//! Everything here is a decision, not an action: which parameters a REGISTER carries, whether the
//! registrar accepted the mechanism, how long to wait before the next keep-alive, and how long to
//! wait before retrying a flow that failed. The randomised choices take the fraction as an
//! argument so a test can pin them, with a thin `rand` wrapper for callers that do not care.

use std::time::Duration;

use sipx_sip::{HeaderName, Response};

/// The option tag, registered in RFC 5626 §11.4.
pub const OPTION_TAG: &str = "outbound";

/// The largest `reg-id` RFC 5626 §4.2 allows: values run from 1 to 2^31 - 1.
pub const MAX_REG_ID: u32 = 0x7fff_ffff;

/// A device identity that outlives a reboot, a re-address and a change of network (§4.1).
///
/// §4.1 requires the value to be "persistent", which is the whole point: the registrar uses it to
/// recognise that a new registration replaces an old one *for the same device* rather than adding
/// a second contact for it. A UA that mints a fresh instance on every start accumulates dead
/// bindings at the registrar and looks, to it, like a growing crowd of identical phones.
///
/// §4.1 says a UA "SHOULD" use a UUID URN (RFC 4122), which is what [`InstanceId::generate`]
/// makes — but the type accepts any URN, because §4.1 permits other schemes and a UA that
/// persisted one has to be able to present it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceId(String);

impl InstanceId {
    /// A fresh random UUID URN (§4.1, RFC 4122 §4.4 — version 4).
    ///
    /// **Generate this once and store it.** Calling it on every start satisfies the syntax and
    /// defeats the mechanism.
    #[must_use]
    pub fn generate() -> Self {
        use rand::Rng as _;
        use std::fmt::Write as _;
        let mut bytes = [0u8; 16];
        rand::rng().fill(&mut bytes);
        // RFC 4122 §4.4: version 4 in the high nibble of octet 6, variant 10 in the top bits of
        // octet 8. Without these a peer is entitled to read the value as some other version and
        // decide it is malformed.
        if let Some(octet) = bytes.get_mut(6) {
            *octet = (*octet & 0x0f) | 0x40;
        }
        if let Some(octet) = bytes.get_mut(8) {
            *octet = (*octet & 0x3f) | 0x80;
        }
        let hex = bytes
            .iter()
            .fold(String::with_capacity(32), |mut out, byte| {
                let _ = write!(out, "{byte:02x}");
                out
            });
        let mut uuid = String::with_capacity(36);
        for (index, chunk) in [0..8, 8..12, 12..16, 16..20, 20..32]
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                uuid.push('-');
            }
            uuid.push_str(hex.get(chunk).unwrap_or_default());
        }
        Self(format!("urn:uuid:{uuid}"))
    }

    /// Adopt an instance ID a UA persisted earlier.
    ///
    /// Rejects anything that is not a URN: §4.1's grammar is `instance-val = urn`, and a value
    /// that is not one would be quoted into the `Contact` and rejected by the registrar rather
    /// than by us.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().trim_start_matches('<').trim_end_matches('>');
        (value.len() > 4 && value.get(..4)?.eq_ignore_ascii_case("urn:"))
            .then(|| Self(value.to_owned()))
    }

    /// The URN itself.
    #[must_use]
    pub fn urn(&self) -> &str {
        &self.0
    }

    /// The `Contact` header parameter, quoted and bracketed as §4.1's grammar requires:
    /// `+sip.instance="<urn:uuid:…>"`.
    ///
    /// The angle brackets are inside the quotes. Both are load-bearing — the URN contains colons,
    /// which would otherwise terminate the parameter value.
    #[must_use]
    pub fn contact_param(&self) -> String {
        format!("+sip.instance=\"<{}>\"", self.0)
    }
}

/// Which flow a registration is for (§4.2).
///
/// One `reg-id` per flow, and the *same* number when that flow is refreshed or re-established —
/// which is what tells the registrar "this replaces the binding for flow 2" rather than "here is
/// another contact". §4.2 also requires the sequence to be stable across reboots, so these are
/// numbered from the outbound proxy set's order rather than allocated as flows come up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct RegId(u32);

impl RegId {
    /// A `reg-id`, if the value is one RFC 5626 §4.2 permits.
    ///
    /// Zero is excluded by the RFC explicitly. That is not pedantry: a registrar that receives
    /// `reg-id=0` is entitled to reject the registration, and the failure looks like a
    /// credentials problem rather than an off-by-one.
    #[must_use]
    pub fn new(value: u32) -> Option<Self> {
        (1..=MAX_REG_ID).contains(&value).then_some(Self(value))
    }

    /// The number.
    #[must_use]
    pub fn value(self) -> u32 {
        self.0
    }
}

/// The `Contact` a REGISTER for one flow carries.
///
/// `base` is the contact as it would be without Outbound — `<sip:alice@192.0.2.5:5060>`. The two
/// parameters are appended as *header* parameters, after the angle brackets, because that is where
/// §4.1's and §4.2's grammars put them: `+sip.instance` is a `contact-param` and `reg-id` is too.
/// Putting either inside the brackets makes it a URI parameter the registrar will not look at.
#[must_use]
pub fn contact(base: &str, instance: &InstanceId, reg_id: RegId) -> String {
    format!(
        "{base};reg-id={};{}",
        reg_id.value(),
        instance.contact_param()
    )
}

/// Add the `ob` URI parameter to a contact for a dialog-forming request (§4.3).
///
/// §4.3: a UA sending a dialog-forming request "MUST include the 'ob' parameter in its Contact
/// header field" when it has no GRUU. It marks the contact as one that is only reachable back down
/// *this flow*, so a mid-dialog request is sent over the flow rather than to the address — which,
/// behind a NAT, is the difference between a re-INVITE arriving and vanishing.
///
/// `ob` is a URI parameter, so it goes inside the angle brackets, unlike `reg-id`.
#[must_use]
pub fn with_ob(contact: &str) -> String {
    let trimmed = contact.trim();
    match (trimmed.find('<'), trimmed.rfind('>')) {
        (Some(open), Some(close)) if open < close => {
            let mut out = String::with_capacity(trimmed.len() + 3);
            out.push_str(trimmed.get(..close).unwrap_or_default());
            out.push_str(";ob");
            out.push_str(trimmed.get(close..).unwrap_or_default());
            out
        }
        // No angle brackets: a bare URI, so the whole value is the URI and the parameter goes on
        // the end. Bracketing it here would change what the header means if it has parameters.
        _ => format!("{trimmed};ob"),
    }
}

/// Whether the registrar actually performed an *outbound* registration (§6).
///
/// §6 requires a registrar that did to "include the 'outbound' option tag in a Require header
/// field" of the 2xx. Checking it is what stops a UA from running keep-alives on a flow nothing is
/// routing down, and from believing a `NAT`ed binding is durable when the registrar recorded an
/// ordinary one.
#[must_use]
pub fn accepted(response: &Response) -> bool {
    response
        .headers
        .get_all(&HeaderName::Require)
        .any(|header| contains_tag(&header.value(), OPTION_TAG.as_bytes()))
}

/// Whether a registrar demands Outbound of its clients — `Require: outbound` on a failure.
///
/// Distinguished from [`accepted`] by where it appears rather than by the header: the same tag in
/// the same header means "I did this" on a 2xx and "you must do this" on a 4xx.
#[must_use]
pub fn required_by(response: &Response) -> bool {
    !response.status.is_success() && accepted(response)
}

fn contains_tag(value: &[u8], tag: &[u8]) -> bool {
    value
        .split(|&b| b == b',')
        .any(|item| trim_ascii(item).eq_ignore_ascii_case(tag))
}

fn trim_ascii(value: &[u8]) -> &[u8] {
    let start = value
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(value.len());
    let end = value
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |last| last + 1);
    value.get(start..end).unwrap_or_default()
}

/// The `Flow-Timer` the registrar named, if any (§6, §4.4).
///
/// When present it replaces the UA's own choice of keep-alive interval: the registrar is saying
/// how long it will hold the flow open without traffic, and a UA that pings less often than that
/// loses the flow between pings.
#[must_use]
pub fn flow_timer(response: &Response) -> Option<Duration> {
    let value = response.headers.value(&HeaderName::FlowTimer)?;
    let text = String::from_utf8_lossy(&value);
    text.trim().parse::<u64>().ok().map(Duration::from_secs)
}

/// How a flow is kept alive, which depends on the transport (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keepalive {
    /// Double-CRLF ping, single-CRLF pong (§4.4.1). Required for connection-oriented transports.
    Crlf,
    /// STUN Binding Requests over the same flow (§4.4.2). Required for UDP.
    Stun,
}

/// Whether the device is one where a keep-alive every two minutes is a battery problem (§4.4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    /// Mains, or a battery large enough not to care.
    Unconstrained,
    /// A phone. §4.4.1 raises the interval by a factor of seven for these.
    Constrained,
}

/// Pick the keep-alive technique §4.4 mandates for a transport.
#[must_use]
pub fn keepalive_for(transport: sipx_transport::TransportKind) -> Keepalive {
    match transport {
        // §4.4.2: "All SIP UAs MUST support the STUN keep-alive technique for UDP flows."
        sipx_transport::TransportKind::Udp => Keepalive::Stun,
        // §4.4.1: the CRLF technique, for the connection-oriented transports. It is deliberately
        // the *SIP* framing rather than a transport-level ping, so it proves the SIP peer is
        // reading rather than only that the socket is open.
        _ => Keepalive::Crlf,
    }
}

/// How long to wait before the next keep-alive (§4.4.1, §4.4.2).
///
/// `fraction` selects within the range and must be in `0.0..=1.0`; every ping re-draws it, because
/// §4.4.1 says "the random number will be different for each keep-alive ping". Randomising is not
/// decoration: a fleet that pings on a fixed period synchronises after any shared outage and
/// arrives at the registrar as one spike.
///
/// A `Flow-Timer` from the registrar wins outright — it is a statement about how long *it* will
/// hold the flow, so a UA's own preference is not a competing opinion.
///
/// The published defaults are used verbatim: 95–120 seconds, or 672–840 where battery matters, and
/// 24–29 for STUN. §4.4.1 describes the lower bound as "20% less than the upper bound" and then
/// gives 95 for an upper bound of 120, which is 20.8% less rather than 20%. The literal numbers
/// are what interoperates, so those are the ones here.
#[must_use]
pub fn keepalive_interval(
    flow_timer: Option<Duration>,
    keepalive: Keepalive,
    power: Power,
    fraction: f64,
) -> Duration {
    if let Some(timer) = flow_timer {
        return timer;
    }
    let (low, high) = match (keepalive, power) {
        (Keepalive::Stun, _) => (24u64, 29u64),
        (Keepalive::Crlf, Power::Unconstrained) => (95, 120),
        (Keepalive::Crlf, Power::Constrained) => (672, 840),
    };
    Duration::from_secs(within(low, high, fraction))
}

/// How long a UA waits before pronouncing a CRLF-kept flow dead (§4.4.1).
///
/// "If a pong is not received within 10 seconds after sending a ping ... then the client MUST
/// treat the flow as failed."
pub const PONG_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest §4.5 will ever have a UA wait between attempts to re-establish a flow.
pub const MAX_RECOVERY_WAIT: Duration = Duration::from_secs(1800);

/// How long to wait before trying to re-establish a failed flow (§4.5).
///
/// `W = min(max-time, base-time * 2^consecutive-failures)`, then "a uniform random time between
/// 50 and 100% of the upper-bound wait time".
///
/// `any_active` picks the base: §4.5 gives 30 seconds when every flow has failed and 90 when at
/// least one is still up. The asymmetry is the interesting part — a UA that has *no* working flow
/// is unreachable and should hurry; one that still has a flow is reachable already, and hurrying
/// only adds load to a registrar that is plainly having a bad day.
#[must_use]
pub fn recovery_delay(consecutive_failures: u32, any_active: bool, fraction: f64) -> Duration {
    let base = if any_active { 90u64 } else { 30 };
    let doubled = base.saturating_mul(1u64 << consecutive_failures.min(32));
    let upper = doubled.min(MAX_RECOVERY_WAIT.as_secs());
    Duration::from_secs(within(upper / 2, upper, fraction))
}

/// Draw within an inclusive range of seconds, clamping the fraction rather than trusting it.
fn within(low: u64, high: u64, fraction: f64) -> u64 {
    let fraction = fraction.clamp(0.0, 1.0);
    let span = high.saturating_sub(low);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        clippy::cast_sign_loss,
        reason = "span is a small number of seconds and the fraction is clamped to 0..=1"
    )]
    let offset = (span as f64 * fraction).round() as u64;
    low.saturating_add(offset)
}

/// Draw a fraction for the randomised choices above.
#[must_use]
pub fn fraction() -> f64 {
    use rand::Rng as _;
    rand::rng().random_range(0.0..=1.0)
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
    use bytes::Bytes;
    use sipx_sip::{Limits, Message, parse_datagram};

    fn response(extra: &str, status: &str) -> Response {
        let text = format!(
            "SIP/2.0 {status}\r\n\
             Via: SIP/2.0/UDP 192.0.2.5:5060;branch=z9hG4bKx\r\n\
             To: <sip:alice@example.com>;tag=r\r\n\
             From: <sip:alice@example.com>;tag=1\r\n\
             Call-ID: reg-1@192.0.2.5\r\n\
             CSeq: 1 REGISTER\r\n\
             {extra}\
             Content-Length: 0\r\n\r\n"
        );
        match parse_datagram(Bytes::from(text), &Limits::datagram()).expect("parses") {
            Message::Response(r) => r,
            Message::Request(_) => panic!("a response"),
        }
    }

    #[test]
    fn a_generated_instance_id_is_a_version_4_uuid_urn() {
        let id = InstanceId::generate();
        let urn = id.urn();
        assert!(urn.starts_with("urn:uuid:"), "{urn}");
        let uuid = urn.trim_start_matches("urn:uuid:");
        assert_eq!(uuid.len(), 36, "{uuid}");
        let groups: Vec<&str> = uuid.split('-').collect();
        assert_eq!(
            groups.iter().map(|g| g.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12],
            "{uuid}"
        );
        // RFC 4122 §4.4: version 4 and variant 10xx. A peer is entitled to reject a UUID that
        // claims no version.
        assert!(groups[2].starts_with('4'), "version nibble: {uuid}");
        assert!(
            matches!(groups[3].as_bytes()[0], b'8' | b'9' | b'a' | b'b'),
            "variant nibble: {uuid}"
        );
    }

    #[test]
    fn two_generated_instance_ids_differ() {
        assert_ne!(InstanceId::generate(), InstanceId::generate());
    }

    #[test]
    fn an_instance_id_is_quoted_and_bracketed_as_the_grammar_requires() {
        let id = InstanceId::parse("urn:uuid:00000000-0000-4000-8000-000000000000").expect("a urn");
        assert_eq!(
            id.contact_param(),
            "+sip.instance=\"<urn:uuid:00000000-0000-4000-8000-000000000000>\"",
            "the angle brackets go inside the quotes; the URN's colons would otherwise end the \
             parameter value"
        );
    }

    #[test]
    fn a_persisted_instance_id_is_accepted_with_or_without_its_brackets() {
        let bare = InstanceId::parse("urn:uuid:1234").expect("a urn");
        let bracketed = InstanceId::parse("<urn:uuid:1234>").expect("a urn");
        assert_eq!(bare, bracketed);
    }

    #[test]
    fn something_that_is_not_a_urn_is_not_an_instance_id() {
        // §4.1's grammar is `instance-val = urn`. A UA that let this through would be rejected by
        // the registrar instead, which is a much worse place to find out.
        assert!(InstanceId::parse("sip:alice@example.com").is_none());
        assert!(InstanceId::parse("").is_none());
        assert!(InstanceId::parse("urn:").is_none());
    }

    #[test]
    fn reg_id_zero_is_refused_because_the_rfc_excludes_it() {
        assert!(RegId::new(0).is_none(), "§4.2: reg-id runs from 1");
        assert_eq!(RegId::new(1).expect("valid").value(), 1);
        assert_eq!(RegId::new(MAX_REG_ID).expect("valid").value(), MAX_REG_ID);
        assert!(
            RegId::new(MAX_REG_ID + 1).is_none(),
            "§4.2 caps at 2^31 - 1"
        );
    }

    #[test]
    fn a_registers_contact_carries_both_parameters_outside_the_brackets() {
        let id = InstanceId::parse("urn:uuid:abc").expect("a urn");
        let contact = contact(
            "<sip:alice@192.0.2.5:5060>",
            &id,
            RegId::new(2).expect("valid"),
        );
        assert_eq!(
            contact, "<sip:alice@192.0.2.5:5060>;reg-id=2;+sip.instance=\"<urn:uuid:abc>\"",
            "both are contact-params, so they follow the closing bracket; inside it they would be \
             URI parameters the registrar does not read"
        );
    }

    #[test]
    fn ob_goes_inside_the_brackets_because_it_is_a_uri_parameter() {
        assert_eq!(
            with_ob("<sip:alice@192.0.2.5:5060>"),
            "<sip:alice@192.0.2.5:5060;ob>"
        );
        // And it must not disturb header parameters that follow.
        assert_eq!(
            with_ob("<sip:alice@192.0.2.5:5060>;expires=600"),
            "<sip:alice@192.0.2.5:5060;ob>;expires=600"
        );
    }

    #[test]
    fn a_bare_contact_uri_still_gets_ob() {
        assert_eq!(with_ob("sip:alice@192.0.2.5"), "sip:alice@192.0.2.5;ob");
    }

    #[test]
    fn the_registrar_says_it_did_an_outbound_registration_in_require() {
        // §6: a registrar that performed an outbound registration MUST say so in Require.
        assert!(accepted(&response("Require: outbound\r\n", "200 OK")));
        assert!(accepted(&response("Require: path, outbound\r\n", "200 OK")));
        assert!(accepted(&response("Require: OUTBOUND\r\n", "200 OK")));
        // Silence means an ordinary registration, and running keep-alives on it would be pinging
        // a flow nothing routes down.
        assert!(!accepted(&response("", "200 OK")));
        assert!(!accepted(&response("Supported: outbound\r\n", "200 OK")));
        // `outbound` inside another tag's name is not the tag.
        assert!(!accepted(&response("Require: outbounded\r\n", "200 OK")));
    }

    #[test]
    fn the_same_tag_means_demanded_on_a_failure_and_done_on_a_success() {
        let refused = response("Require: outbound\r\n", "420 Bad Extension");
        assert!(required_by(&refused), "a 4xx requiring it is a demand");
        let ok = response("Require: outbound\r\n", "200 OK");
        assert!(
            !required_by(&ok),
            "the same header on a 2xx is the registrar reporting what it did"
        );
    }

    #[test]
    fn a_flow_timer_from_the_registrar_replaces_our_own_choice() {
        let with = response("Flow-Timer: 25\r\n", "200 OK");
        assert_eq!(flow_timer(&with), Some(Duration::from_secs(25)));
        assert_eq!(
            keepalive_interval(
                flow_timer(&with),
                Keepalive::Crlf,
                Power::Unconstrained,
                0.5
            ),
            Duration::from_secs(25),
            "the registrar's number is a statement about how long it holds the flow, not a \
             preference to be averaged with ours"
        );
        assert_eq!(flow_timer(&response("", "200 OK")), None);
        assert_eq!(
            flow_timer(&response("Flow-Timer: soon\r\n", "200 OK")),
            None
        );
    }

    /// The literal defaults from §4.4.1 and §4.4.2.
    #[test]
    fn the_keepalive_ranges_are_the_ones_the_rfc_publishes() {
        let interval = |keepalive, power, fraction| {
            keepalive_interval(None, keepalive, power, fraction).as_secs()
        };
        assert_eq!(interval(Keepalive::Crlf, Power::Unconstrained, 0.0), 95);
        assert_eq!(interval(Keepalive::Crlf, Power::Unconstrained, 1.0), 120);
        assert_eq!(interval(Keepalive::Crlf, Power::Constrained, 0.0), 672);
        assert_eq!(interval(Keepalive::Crlf, Power::Constrained, 1.0), 840);
        assert_eq!(interval(Keepalive::Stun, Power::Unconstrained, 0.0), 24);
        assert_eq!(interval(Keepalive::Stun, Power::Unconstrained, 1.0), 29);
        // A fraction outside the range is clamped rather than trusted: an out-of-range draw
        // would otherwise produce an interval outside what the registrar tolerates.
        assert_eq!(interval(Keepalive::Stun, Power::Unconstrained, -3.0), 24);
        assert_eq!(interval(Keepalive::Stun, Power::Unconstrained, 9.0), 29);
    }

    #[test]
    fn udp_is_kept_alive_with_stun_and_everything_else_with_crlf() {
        use sipx_transport::TransportKind;
        assert_eq!(keepalive_for(TransportKind::Udp), Keepalive::Stun);
        assert_eq!(keepalive_for(TransportKind::Tcp), Keepalive::Crlf);
        assert_eq!(keepalive_for(TransportKind::Tls), Keepalive::Crlf);
        assert_eq!(keepalive_for(TransportKind::Ws), Keepalive::Crlf);
        assert_eq!(keepalive_for(TransportKind::Wss), Keepalive::Crlf);
    }

    /// §4.5: `W = min(max-time, base-time * 2^consecutive-failures)`, retried after 50–100% of W.
    #[test]
    fn flow_recovery_backs_off_by_doubling_and_stops_at_half_an_hour() {
        let all_failed = |failures, fraction| recovery_delay(failures, false, fraction).as_secs();
        // base-time 30 when every flow has failed.
        assert_eq!(all_failed(0, 1.0), 30);
        assert_eq!(all_failed(1, 1.0), 60);
        assert_eq!(all_failed(2, 1.0), 120);
        assert_eq!(all_failed(6, 1.0), 1800, "30 * 64 is past max-time");
        assert_eq!(all_failed(30, 1.0), 1800, "and it stays there");
        // The jitter floor is half of W, never zero: retrying immediately is how a UA turns one
        // registrar hiccup into a flood.
        assert_eq!(all_failed(2, 0.0), 60);
        assert_eq!(all_failed(0, 0.0), 15);
    }

    #[test]
    fn a_ua_with_a_working_flow_waits_three_times_as_long_before_retrying() {
        // §4.5's base-time is 90 when at least one flow is up and 30 when none is. A UA with no
        // flow is unreachable and should hurry; one that is still reachable is only adding load.
        assert_eq!(recovery_delay(0, true, 1.0).as_secs(), 90);
        assert_eq!(recovery_delay(0, false, 1.0).as_secs(), 30);
        assert_eq!(recovery_delay(3, true, 1.0).as_secs(), 720);
    }

    #[test]
    fn the_drawn_fraction_stays_in_range() {
        for _ in 0..64 {
            let drawn = fraction();
            assert!((0.0..=1.0).contains(&drawn), "{drawn}");
        }
    }
}
