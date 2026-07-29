//! ICE in SDP: the attributes RFC 8839 §5 defines.
//!
//! This is the signalling half of ICE and nothing else. The agent, the connectivity checks and
//! the sockets live in `sipx-media`; what belongs here is the grammar, because it is pure
//! parsing — no clock, no socket, no runtime — and because the rest of ICE should reason about a
//! typed description rather than search an SDP body for substrings.
//!
//! Two rules run through the whole module, and both exist because the alternative breaks calls
//! with peers that are behaving perfectly legally.
//!
//! **A line sipx cannot use is ignored, not fatal.** RFC 8839 §5.1's `connection-address` admits
//! an FQDN and its `transport` admits any token, so a description may carry candidates sipx has
//! no way to check. The candidate is dropped and the description survives. A parser that fails
//! the whole body on one such line refuses a call over a candidate it was never asked to use.
//!
//! **A `priority` is range-checked on parse.** The grammar is `1*10DIGIT`, so `4294967295` is
//! well-formed text, but §5.1 bounds the value at 2^31 − 1. RFC 8445 §6.1.2.3 then combines two
//! priorities as `2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)`, and that expression leaves `u64` for
//! operands near `u32::MAX` — `4294967295` on both sides is the case that overflows, which is
//! exactly the value the grammar admits and the range forbids. Checking on parse is what keeps
//! the value that would wrap from ever reaching the arithmetic; see [`Priority`] for the
//! headroom the bound actually buys, and [`docs/specs/ice.md`] §4 and §6.2.
//!
//! [`docs/specs/ice.md`]: https://github.com/codewandler/sipx/blob/main/docs/specs/ice.md

use std::fmt::{self, Write as _};
use std::net::IpAddr;

/// The `ice2` option tag every RFC 8839 agent must advertise (§5.6).
pub const ICE2: &str = "ice2";

/// `ice-char = ALPHA / DIGIT / "+" / "/"` (RFC 8839 §5.1).
fn is_ice_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '+' || c == '/'
}

/// Whether `text` is `min*max ice-char`. Lengths are in characters, which for `ice-char` is the
/// same as bytes.
fn is_ice_chars(text: &str, min: usize, max: usize) -> bool {
    let len = text.len();
    len >= min && len <= max && text.chars().all(is_ice_char)
}

/// A candidate foundation: `1*32ice-char` (RFC 8839 §5.1).
///
/// The value is opaque on the wire — RFC 8445 §5.1.1.3 gives it meaning only by equality, two
/// candidates sharing a foundation being unfrozen together. It is a type rather than a `String`
/// so the length and character-set bound cannot be lost between parsing and re-emitting.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Foundation(String);

impl Foundation {
    /// The foundation a token names, if the token is one.
    pub fn new(token: &str) -> Option<Self> {
        is_ice_chars(token, 1, 32).then(|| Self(token.to_owned()))
    }

    /// The token as it appears in SDP.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Foundation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which component of a media stream a candidate is for (RFC 8839 §5.1).
///
/// A number and not an enum of `Rtp`/`Rtcp`: §5.1 makes it `1*3DIGIT` between 1 and 256, and a
/// stream may have components sipx does not itself offer. Refusing a candidate for component 3
/// would drop a line the peer is entitled to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentId(u16);

impl ComponentId {
    /// RTP, which is component 1.
    pub const RTP: Self = Self(1);
    /// RTCP, which is component 2.
    pub const RTCP: Self = Self(2);

    /// The component with this identifier, if §5.1's 1–256 range admits it.
    pub fn new(id: u16) -> Option<Self> {
        (1..=256).contains(&id).then_some(Self(id))
    }

    /// The identifier as it appears in SDP.
    pub fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The transport a candidate names.
///
/// One variant, deliberately. RFC 8839 §5.1's grammar is `"UDP" / transport-extension`, and sipx
/// checks candidates over UDP only — so a candidate naming anything else parses as far as this
/// type and is then dropped by [`Candidate::parse`], rather than failing the description. A peer
/// offering an ICE-TCP candidate alongside UDP ones is offering something usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Transport {
    /// UDP, the only transport sipx checks over.
    Udp,
}

impl Transport {
    /// The token as it appears in SDP.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Udp => "UDP",
        }
    }

    /// The transport a token names, if it is one sipx can check over.
    ///
    /// Case-insensitive: ABNF string literals are, and RFC 5245 — which this grammar is inherited
    /// from — printed its own examples in lower case.
    pub fn parse(token: &str) -> Option<Self> {
        token
            .eq_ignore_ascii_case(Self::Udp.as_str())
            .then_some(Self::Udp)
    }
}

/// A candidate priority: a positive integer up to 2^31 − 1 (RFC 8839 §5.1).
///
/// The bound is the type's whole reason for existing. RFC 8445 §6.1.2.3 combines two priorities
/// into a pair priority as `2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)`. With both operands at
/// 2^31 − 1 that comes to `2^63 − 2`: the `G > D` term is zero when the two are equal, so the
/// `2^63 − 1` upper bound is approached and never reached, and every in-range pair therefore has
/// half a `u64` of headroom.
///
/// Carry an unchecked `u32` from the wire into the same expression and the headroom is spent.
/// The overflow is not one step past the bound — the arithmetic is still exact at 4294967294 —
/// but `4294967295` on both sides, the ten-digit value `1*10DIGIT` admits, comes to `2^64 + 2^32
/// − 2` and wraps. In a release build it wraps silently, reordering the checklist that the
/// arithmetic exists to order. `the_priority_bound_is_what_keeps_the_pair_priority_in_a_u64`
/// asserts both halves of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Priority(u32);

impl Priority {
    /// The largest priority RFC 8839 §5.1 permits.
    pub const MAX: Self = Self(0x7fff_ffff);
    /// The smallest. §5.1 says "positive", so zero is not a priority.
    pub const MIN: Self = Self(1);

    /// The priority with this value, if it is in range.
    pub fn new(value: u32) -> Option<Self> {
        (Self::MIN.0..=Self::MAX.0)
            .contains(&value)
            .then_some(Self(value))
    }

    /// Read a `priority` production.
    ///
    /// `1*10DIGIT` admits ten digits, so `4294967295` is well-formed text that is not a legal
    /// priority. It is read wide and then range-checked, so that an out-of-range value is
    /// rejected as out of range rather than silently truncated to something plausible.
    pub fn parse(text: &str) -> Option<Self> {
        let value: u64 = text.parse().ok()?;
        Self::new(u32::try_from(value).ok()?)
    }

    /// The value.
    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// How a candidate was obtained (RFC 8839 §5.1, RFC 8445 §5.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateType {
    /// A local interface address.
    Host,
    /// An address a STUN server reported.
    ServerReflexive,
    /// An address learned from a peer's connectivity check.
    PeerReflexive,
    /// An address on a TURN relay.
    Relayed,
}

impl CandidateType {
    /// The token as it appears in SDP.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::ServerReflexive => "srflx",
            Self::PeerReflexive => "prflx",
            Self::Relayed => "relay",
        }
    }

    /// The type a token names, if it is one sipx knows.
    ///
    /// `candidate-types` ends in `token`, so the set is extensible and a peer may name one sipx
    /// has never heard of. `None` here makes [`Candidate::parse`] ignore the line, which is the
    /// conservative reading rather than the only legal one: RFC 8839 §5.1 requires a document
    /// defining a new candidate type to define how it is processed, so a type sipx does not know
    /// is a type whose processing rules sipx does not have. Checking it as though it were a host
    /// candidate would be guessing at those rules against a peer that published them.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            "host" => Some(Self::Host),
            "srflx" => Some(Self::ServerReflexive),
            "prflx" => Some(Self::PeerReflexive),
            "relay" => Some(Self::Relayed),
            _ => None,
        }
    }
}

/// The `raddr`/`rport` pair a candidate may carry (RFC 8839 §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RelatedAddress {
    /// The related address.
    pub address: IpAddr,
    /// The related port.
    pub port: u16,
}

/// One `a=candidate` line (RFC 8839 §5.1). Media-level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The foundation.
    pub foundation: Foundation,
    /// Which component of the stream this candidate is for.
    pub component: ComponentId,
    /// The transport. Always [`Transport::Udp`]; see the type.
    pub transport: Transport,
    /// The priority, range-checked on parse.
    pub priority: Priority,
    /// The transport address.
    pub address: IpAddr,
    /// The port.
    pub port: u16,
    /// How the candidate was obtained.
    pub kind: CandidateType,
    /// The `raddr`/`rport` pair.
    ///
    /// RFC 8839 §5.1 requires it for `srflx`, `prflx` and `relay` and forbids it for `host`, and
    /// a candidate sipx *generates* must obey that. It is not enforced when reading: §5.1 gives
    /// the field to "diagnostics", nothing in RFC 8445's checks consults it, and dropping a
    /// peer's only working candidate over a diagnostic field would trade a call for a nicety. A
    /// privacy-preserving agent writes `0.0.0.0`/`::` and port 9 here, which is ordinary.
    pub related: Option<RelatedAddress>,
    /// `cand-extension` name/value pairs sipx does not model, in the order they arrived.
    ///
    /// Kept rather than dropped, the same discipline [`crate::session`] applies to unknown SDP
    /// lines one level up: §5.1 says unknown extensions MUST be ignored, and ignoring an
    /// extension is not the same as deleting it from a description that is about to be relayed.
    pub extensions: Vec<(String, String)>,
}

impl Candidate {
    /// Read an `a=candidate` value.
    ///
    /// `None` means **ignore this line and keep the description** — RFC 8839 §5.1's rule for a
    /// candidate carrying an FQDN or an address family the agent does not support, and by the
    /// same argument for a transport or candidate type sipx cannot check over. It also covers a
    /// line that is simply malformed, because the outcome the peer needs is identical.
    pub fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split_whitespace();
        let foundation = Foundation::new(parts.next()?)?;
        let component = ComponentId::new(parts.next()?.parse().ok()?)?;
        let transport = Transport::parse(parts.next()?)?;
        let priority = Priority::parse(parts.next()?)?;
        // An FQDN, or a literal of a family this build cannot represent, fails here — which is
        // exactly §5.1's "the candidate MUST be ignored".
        let address: IpAddr = parts.next()?.parse().ok()?;
        let port: u16 = parts.next()?.parse().ok()?;
        if parts.next()? != "typ" {
            return None;
        }
        let kind = CandidateType::parse(parts.next()?)?;

        // `rel-addr`, `rel-port` and every `cand-extension` are name/value pairs, so they are
        // read as pairs rather than by position. The grammar puts `raddr`/`rport` first; peers
        // that put an extension there are still understood, and a name with no value is not.
        //
        // `extension-att-value = *VCHAR` does admit an empty value, and this drops such a line
        // rather than keeping the name with an empty value. That is deliberate: an empty value
        // is only distinguishable from a missing one by a trailing space, so keeping it would
        // make `to_value` emit a trailing space — and a round trip that adds a byte the peer did
        // not send is a worse failure than ignoring a candidate whose extension said nothing.
        let mut related_address = None;
        let mut related_port = None;
        let mut extensions = Vec::new();
        while let Some(name) = parts.next() {
            let value = parts.next()?;
            match name {
                "raddr" => related_address = Some(value.parse::<IpAddr>().ok()?),
                "rport" => related_port = Some(value.parse::<u16>().ok()?),
                _ => extensions.push((name.to_owned(), value.to_owned())),
            }
        }
        let related = match (related_address, related_port) {
            (Some(address), Some(port)) => Some(RelatedAddress { address, port }),
            (None, None) => None,
            // Half a related address is not a related address, and guessing the other half would
            // put an address sipx invented into a description it may go on to relay.
            _ => return None,
        };

        Some(Self {
            foundation,
            component,
            transport,
            priority,
            address,
            port,
            kind,
            related,
            extensions,
        })
    }

    /// Render as an `a=candidate` value.
    pub fn to_value(&self) -> String {
        let mut out = String::with_capacity(64);
        let _ = write!(
            out,
            "{} {} {} {} {} {} typ {}",
            self.foundation,
            self.component,
            self.transport.as_str(),
            self.priority,
            self.address,
            self.port,
            self.kind.as_str()
        );
        if let Some(related) = &self.related {
            let _ = write!(out, " raddr {} rport {}", related.address, related.port);
        }
        for (name, value) in &self.extensions {
            let _ = write!(out, " {name} {value}");
        }
        out
    }
}

/// One entry of an `a=remote-candidates` line (RFC 8839 §5.2). Media-level.
///
/// A controlling agent includes it in an offer for a stream that is Completed, and in no other
/// case; it names the pair it selected so the answerer can agree without a second round of
/// checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteCandidate {
    /// Which component this is the selected remote candidate for.
    pub component: ComponentId,
    /// Its address.
    pub address: IpAddr,
    /// Its port.
    pub port: u16,
}

impl RemoteCandidate {
    /// Read an `a=remote-candidates` value, which carries one entry per component.
    ///
    /// `None` for a malformed line. Unlike a candidate this is all-or-nothing: §5.2 requires a
    /// value for *each* component, so a half-read line would name a selected pair for one
    /// component and silently drop the other.
    pub fn parse_list(value: &str) -> Option<Vec<Self>> {
        let mut parts = value.split_whitespace().peekable();
        let mut out = Vec::new();
        while parts.peek().is_some() {
            let component = ComponentId::new(parts.next()?.parse().ok()?)?;
            let address: IpAddr = parts.next()?.parse().ok()?;
            let port: u16 = parts.next()?.parse().ok()?;
            out.push(Self {
                component,
                address,
                port,
            });
        }
        (!out.is_empty()).then_some(out)
    }

    /// Render several remote candidates as one `a=remote-candidates` value.
    pub fn to_value(candidates: &[Self]) -> String {
        let mut out = String::with_capacity(candidates.len() * 24);
        for candidate in candidates {
            if !out.is_empty() {
                out.push(' ');
            }
            let _ = write!(
                out,
                "{} {} {}",
                candidate.component, candidate.address, candidate.port
            );
        }
        out
    }
}

/// The short-term credentials for a stream: `ice-ufrag` and `ice-pwd` (RFC 8839 §5.4).
///
/// The two travel together because §5.4 requires both for every data stream, whether they are
/// written at session or media level, and because RFC 8445 §7.1.2 keys a connectivity check's
/// `MESSAGE-INTEGRITY` on the password that goes with a particular fragment. A type carrying one
/// without the other would be a credential that cannot authenticate anything.
///
/// The fields are private so the length bounds cannot be lost after construction: the send and
/// receive bounds differ, and which one applied is a property of how the value arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    ufrag: String,
    pwd: String,
}

impl Credentials {
    /// The longest `ice-ufrag` or `ice-pwd` §5.4 permits sipx to **send**.
    pub const MAX_SENT_LEN: usize = 32;
    /// The longest either that sipx must **accept** on receive.
    pub const MAX_ACCEPTED_LEN: usize = 256;
    /// The shortest `ice-ufrag` the grammar admits.
    pub const MIN_UFRAG_LEN: usize = 4;
    /// The shortest `ice-pwd` the grammar admits.
    pub const MIN_PWD_LEN: usize = 22;

    /// Credentials sipx will put in an offer or answer.
    ///
    /// `None` when either value is outside what §5.4 permits to be sent: "MUST NOT be longer
    /// than 32 characters when sending, but an implementation MUST accept up to 256 characters
    /// when receiving". The asymmetry is why this is a different constructor from
    /// [`Credentials::received`] rather than a flag — sending 200 characters is a defect, and
    /// receiving them is Tuesday.
    pub fn new(ufrag: impl Into<String>, pwd: impl Into<String>) -> Option<Self> {
        Self::checked(ufrag.into(), pwd.into(), Self::MAX_SENT_LEN)
    }

    /// Credentials read from a peer's description.
    pub fn received(ufrag: impl Into<String>, pwd: impl Into<String>) -> Option<Self> {
        Self::checked(ufrag.into(), pwd.into(), Self::MAX_ACCEPTED_LEN)
    }

    fn checked(ufrag: String, pwd: String, max: usize) -> Option<Self> {
        let ok = is_ice_chars(&ufrag, Self::MIN_UFRAG_LEN, max)
            && is_ice_chars(&pwd, Self::MIN_PWD_LEN, max);
        ok.then_some(Self { ufrag, pwd })
    }

    /// The username fragment.
    pub fn ufrag(&self) -> &str {
        &self.ufrag
    }

    /// The password.
    pub fn pwd(&self) -> &str {
        &self.pwd
    }
}

/// The `a=ice-pacing` value: the Ta interval an agent wants, in milliseconds (RFC 8839 §5.5).
/// Session-level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pacing(u32);

impl Pacing {
    /// What §5.5 says the value is when the attribute is absent.
    pub const DEFAULT: Self = Self(50);

    /// The pacing for a number of milliseconds.
    pub fn from_millis(millis: u32) -> Self {
        Self(millis)
    }

    /// Read an `a=ice-pacing` value: `1*10DIGIT`.
    pub fn parse(text: &str) -> Option<Self> {
        let millis: u64 = text.parse().ok()?;
        Some(Self(u32::try_from(millis).ok()?))
    }

    /// The interval in milliseconds.
    pub fn millis(self) -> u32 {
        self.0
    }

    /// What the two agents will actually use.
    ///
    /// §5.5: "both agents will use the larger of the indicated values". The slower agent wins,
    /// because pacing exists to protect whichever end has less to spend.
    #[must_use]
    pub fn agreed(self, other: Self) -> Self {
        Self(self.0.max(other.0))
    }
}

impl fmt::Display for Pacing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Split an `a=ice-options` value into its tags (RFC 8839 §5.6).
///
/// A tag that is not `1*ice-char` is dropped and its neighbours are kept: the attribute is a list
/// of independent capabilities, and one unreadable entry says nothing about the others.
pub(crate) fn option_tags(value: &str) -> impl Iterator<Item = &str> {
    value
        .split_whitespace()
        .filter(|tag| is_ice_chars(tag, 1, usize::MAX))
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

    /// RFC 8839 §5.1's own example line, wrapped in the RFC for width and joined here.
    const RFC_EXAMPLE: &str =
        "2 1 UDP 1694498815 192.0.2.3 45664 typ srflx raddr 203.0.113.141 rport 8998";

    /// The vector `docs/specs/ice.md` §14 names, and the one the rest of ICE is built on: if a
    /// candidate does not survive a round trip byte-for-byte, every description sipx relays or
    /// re-offers is a description the peer did not send.
    #[test]
    fn the_rfc_8839_candidate_example_round_trips_unchanged() {
        let parsed = Candidate::parse(RFC_EXAMPLE).expect("the RFC's own example parses");
        assert_eq!(parsed.to_value(), RFC_EXAMPLE);

        assert_eq!(parsed.foundation.as_str(), "2");
        assert_eq!(parsed.component, ComponentId::RTP);
        assert_eq!(parsed.transport, Transport::Udp);
        // `docs/specs/ice.md` §4's table: server-reflexive, one address, RTP.
        assert_eq!(parsed.priority.get(), 1_694_498_815);
        assert_eq!(parsed.address, "192.0.2.3".parse::<IpAddr>().unwrap());
        assert_eq!(parsed.port, 45664);
        assert_eq!(parsed.kind, CandidateType::ServerReflexive);
        assert_eq!(
            parsed.related,
            Some(RelatedAddress {
                address: "203.0.113.141".parse::<IpAddr>().unwrap(),
                port: 8998,
            })
        );
        assert!(parsed.extensions.is_empty());
    }

    /// A privacy-preserving agent writes `0.0.0.0`/`::` and port 9 rather than reveal the
    /// address behind the NAT (RFC 8839 §5.1). It is ordinary, and it must not look malformed.
    #[test]
    fn a_masked_related_address_parses() {
        for line in [
            "1 1 UDP 1694498815 192.0.2.3 45664 typ srflx raddr 0.0.0.0 rport 9",
            "1 1 UDP 1694498815 2001:db8::3 45664 typ relay raddr :: rport 9",
        ] {
            let parsed = Candidate::parse(line).expect("a masked related address is well-formed");
            assert_eq!(parsed.to_value(), line);
            assert_eq!(parsed.related.expect("kept").port, 9);
        }
    }

    /// A host candidate carries no `raddr`/`rport`, and must not grow one on the way out.
    #[test]
    fn a_host_candidate_round_trips_without_a_related_address() {
        let line = "1 2 UDP 2130706430 192.0.2.1 5001 typ host";
        let parsed = Candidate::parse(line).expect("parses");
        assert_eq!(parsed.related, None);
        assert_eq!(parsed.component, ComponentId::RTCP);
        assert_eq!(parsed.to_value(), line);
    }

    /// RFC 8839 §5.1 says unknown `cand-extension` pairs are ignored. Ignoring one is not the
    /// same as deleting it: sipx re-offers descriptions, and a peer's extension has to reach the
    /// far end intact rather than be quietly dropped by the element in the middle.
    #[test]
    fn an_unknown_candidate_extension_survives() {
        let line = "3 1 UDP 2130706431 192.0.2.1 8998 typ host generation 0 network-id 4";
        let parsed = Candidate::parse(line).expect("an unknown extension is not an error");
        assert_eq!(
            parsed.extensions,
            vec![
                ("generation".to_owned(), "0".to_owned()),
                ("network-id".to_owned(), "4".to_owned()),
            ]
        );
        assert_eq!(parsed.to_value(), line);
    }

    /// The extensions follow `raddr`/`rport` on the way out whatever order they arrived in, so
    /// the line sipx writes matches the grammar even when the peer's did not.
    #[test]
    fn extensions_are_written_after_the_related_address() {
        let parsed = Candidate::parse(
            "1 1 UDP 100 192.0.2.1 1 typ srflx ufrag 8hhY raddr 192.0.2.9 rport 2",
        )
        .expect("parses");
        assert_eq!(
            parsed.to_value(),
            "1 1 UDP 100 192.0.2.1 1 typ srflx raddr 192.0.2.9 rport 2 ufrag 8hhY"
        );
    }

    /// The range check is load-bearing, not defensive. `1*10DIGIT` admits `4294967295`, and RFC
    /// 8445 §6.1.2.3's pair priority overflows a `u64` on it — see `docs/specs/ice.md` §6.2.
    #[test]
    fn a_priority_the_grammar_admits_but_the_range_forbids_is_rejected() {
        assert_eq!(
            Priority::parse("1694498815").map(Priority::get),
            Some(1_694_498_815)
        );
        assert_eq!(Priority::parse("2147483647"), Some(Priority::MAX));
        assert_eq!(Priority::parse("1"), Some(Priority::MIN));

        // Ten digits, so the grammar is satisfied; 2^31 - 1 is not.
        assert_eq!(
            Priority::parse("4294967295"),
            None,
            "u32::MAX is not a priority"
        );
        assert_eq!(Priority::parse("2147483648"), None, "one past 2^31 - 1");
        assert_eq!(Priority::parse("0"), None, "§5.1 says positive");
        assert_eq!(Priority::parse("-1"), None);
        assert_eq!(Priority::parse(""), None);

        // And the whole line goes with it, rather than the priority being clamped to something
        // plausible: a candidate whose priority sipx invented would be ordered wrongly against
        // the far end's copy of the same checklist.
        assert_eq!(
            Candidate::parse("1 1 UDP 4294967295 192.0.2.1 8998 typ host"),
            None
        );
    }

    /// Why the bound exists, asserted rather than asserted-in-a-comment. `docs/specs/ice.md` §6.2
    /// bounds RFC 8445 §6.1.2.3's pair priority at `2^32*(2^31−1) + 2*(2^31−1) + 1` = `2^63 − 1`;
    /// let the `1*10DIGIT` grammar's own worst case through unchecked instead and the identical
    /// expression leaves `u64`. `M-21` computes it, and this is the reason it may.
    #[test]
    fn the_priority_bound_is_what_keeps_the_pair_priority_in_a_u64() {
        let pair = |g: u64, d: u64| {
            (1u64 << 32)
                .checked_mul(g.min(d))
                .and_then(|v| v.checked_add(2 * g.max(d)))
                .and_then(|v| v.checked_add(u64::from(g > d)))
        };
        let max = u64::from(Priority::MAX.get());
        let bound = (1u64 << 63) - 1;

        // The extreme in-range pair. The `G > D` term is zero when both are at the ceiling, so
        // the attained maximum is one below the bound §6.2 states rather than equal to it, and
        // every in-range pair therefore has half a `u64` of headroom.
        assert_eq!(pair(max, max), Some(bound - 1));
        assert!(pair(max, max - 1).is_some_and(|priority| priority < bound));
        assert!(pair(Priority::MIN.get().into(), max).is_some());

        // `4294967295` — ten digits, so the grammar admits it, and `Priority::parse` does not.
        // This is the value that reaches the arithmetic in a stack that skips the range check.
        assert_eq!(Priority::parse("4294967295"), None);
        assert_eq!(
            pair(u64::from(u32::MAX), u64::from(u32::MAX)),
            None,
            "the value the range check keeps out is the value that overflows"
        );
    }

    /// RFC 8839 §5.1: a candidate naming an FQDN or an address family the agent does not support
    /// "MUST be ignored" — the line, not the description. Nor does a transport or a candidate
    /// type sipx cannot check over take the rest of the stream down with it.
    #[test]
    fn a_candidate_sipx_cannot_use_is_ignored_and_the_description_survives() {
        const OFFER: &str = concat!(
            "v=0\r\n",
            "o=- 1 1 IN IP4 192.0.2.1\r\n",
            "s=-\r\n",
            "c=IN IP4 192.0.2.1\r\n",
            "t=0 0\r\n",
            "m=audio 49170 RTP/AVP 0\r\n",
            "a=ice-ufrag:8hhY\r\n",
            "a=ice-pwd:asd88fgpdd777uzjYhagZg\r\n",
            "a=candidate:1 1 UDP 2130706431 relay.example.com 8998 typ host\r\n",
            "a=candidate:2 1 TCP 2130706431 192.0.2.1 8998 typ host tcptype active\r\n",
            "a=candidate:3 1 UDP 2130706431 192.0.2.1 8998 typ mystery\r\n",
            "a=candidate:4 1 UDP 2130706431 192.0.2.1 9000 typ host\r\n",
        );

        let offer = crate::parse(OFFER).expect("the description parses despite the four lines");
        let stream = offer.media.first().expect("one stream");
        let candidates = stream.ice_candidates();
        assert_eq!(
            candidates.len(),
            1,
            "only the last line is usable: {candidates:?}"
        );
        assert_eq!(candidates[0].foundation.as_str(), "4");

        // Ignored is not deleted. Every line is still on the description and still goes out.
        assert_eq!(offer.to_string_sdp(), OFFER);
    }

    /// The transport case in isolation: §5.1's `transport-extension` means a peer offering an
    /// ICE-TCP candidate alongside UDP ones is offering something usable, so the TCP line is
    /// accepted as well-formed and discarded rather than treated as a parse failure.
    #[test]
    fn a_transport_other_than_udp_is_discarded_and_udp_is_case_insensitive() {
        assert_eq!(Transport::parse("UDP"), Some(Transport::Udp));
        assert_eq!(Transport::parse("udp"), Some(Transport::Udp));
        assert_eq!(Transport::parse("TCP"), None);
        // Whatever case it arrived in, sipx writes the spelling the grammar prints.
        let parsed = Candidate::parse("1 1 udp 100 192.0.2.1 8998 typ host").expect("parses");
        assert_eq!(parsed.to_value(), "1 1 UDP 100 192.0.2.1 8998 typ host");
    }

    /// A line that is malformed rather than merely unsupported is ignored the same way — there
    /// is no shape of `a=candidate` that costs the peer the whole description.
    #[test]
    fn a_malformed_candidate_is_ignored_rather_than_fatal() {
        for line in [
            "",
            "1 1 UDP 2130706431 192.0.2.1 8998",
            "1 1 UDP 2130706431 192.0.2.1 8998 host",
            "1 0 UDP 2130706431 192.0.2.1 8998 typ host",
            "1 257 UDP 2130706431 192.0.2.1 8998 typ host",
            " 1 UDP 2130706431 192.0.2.1 99999 typ host",
            "1 1 UDP 2130706431 192.0.2.1 8998 typ srflx raddr 192.0.2.9",
            "1 1 UDP 2130706431 192.0.2.1 8998 typ host generation",
            // `*VCHAR` admits an empty extension value, and it is still dropped: see the note in
            // `parse`. Keeping it would cost a trailing space on every round trip.
            "1 1 UDP 100 192.0.2.1 9 typ host generation ",
            "th!s 1 UDP 2130706431 192.0.2.1 8998 typ host",
        ] {
            assert_eq!(Candidate::parse(line), None, "{line:?}");
        }

        // A trailing space on an otherwise complete line is *not* malformed — the pair loop just
        // ends — and it must not pick up an empty extension on the way out.
        let padded = Candidate::parse("1 1 UDP 100 192.0.2.1 9 typ host ").expect("parses");
        assert!(padded.extensions.is_empty());
        assert_eq!(padded.to_value(), "1 1 UDP 100 192.0.2.1 9 typ host");
    }

    /// RFC 8839 §5.2's example lines, and the rule that they are read at media level only.
    #[test]
    fn remote_candidates_name_one_address_per_component() {
        let one = RemoteCandidate::parse_list("1 192.0.2.3 45664").expect("parses");
        let two = RemoteCandidate::parse_list("2 192.0.2.3 45665").expect("parses");
        assert_eq!(one[0].component, ComponentId::RTP);
        assert_eq!(two[0].component, ComponentId::RTCP);
        assert_eq!(RemoteCandidate::to_value(&one), "1 192.0.2.3 45664");

        // Several may share one line, which is what the `0*(SP remote-candidate)` is for.
        let both =
            RemoteCandidate::parse_list("1 192.0.2.3 45664 2 192.0.2.3 45665").expect("parses");
        assert_eq!(both.len(), 2);
        assert_eq!(
            RemoteCandidate::to_value(&both),
            "1 192.0.2.3 45664 2 192.0.2.3 45665"
        );

        // All or nothing: §5.2 requires a value for each component, so half a line would claim a
        // selected pair for one component and silently drop the other.
        assert_eq!(
            RemoteCandidate::parse_list("1 192.0.2.3 45664 2 192.0.2.3"),
            None
        );
        assert_eq!(RemoteCandidate::parse_list(""), None);
    }

    /// RFC 8839 §5.4's own example values, and the length bounds — which are asymmetric on
    /// purpose: 32 characters is what sipx may send, 256 is what it must accept.
    #[test]
    fn credentials_are_short_to_send_and_long_to_accept() {
        let credentials =
            Credentials::new("8hhY", "asd88fgpdd777uzjYhagZg").expect("§5.4's example");
        assert_eq!(credentials.ufrag(), "8hhY");
        assert_eq!(credentials.pwd(), "asd88fgpdd777uzjYhagZg");

        let long_ufrag = "u".repeat(33);
        let long_pwd = "p".repeat(200);
        assert_eq!(
            Credentials::new(&long_ufrag, "asd88fgpdd777uzjYhagZg"),
            None,
            "33 sent"
        );
        assert!(
            Credentials::received(&long_ufrag, &long_pwd).is_some(),
            "up to 256 must be accepted"
        );
        assert_eq!(Credentials::received("u".repeat(257), &long_pwd), None);
        assert_eq!(Credentials::received(&long_ufrag, "p".repeat(257)), None);

        // Below the grammar's floor, at either end.
        assert_eq!(Credentials::received("8hh", "asd88fgpdd777uzjYhagZg"), None);
        assert_eq!(Credentials::received("8hhY", "tooshort"), None);
        // `ice-char` is ALPHA / DIGIT / "+" / "/" and nothing else.
        assert_eq!(
            Credentials::received("8h:Y", "asd88fgpdd777uzjYhagZg"),
            None
        );
    }

    fn description(session: &str, media: &str) -> crate::session::SessionDescription {
        let text = format!(
            "v=0\r\no=- 1 1 IN IP4 192.0.2.1\r\ns=-\r\nc=IN IP4 192.0.2.1\r\nt=0 0\r\n{session}m=audio 49170 RTP/AVP 0\r\n{media}"
        );
        crate::parse(&text).expect("parses")
    }

    /// RFC 8839 §5.4: both levels are allowed and the media level wins. The pair is taken from
    /// one level or the other and never mixed — a fragment from the `m=` line with a password
    /// from the session line authenticates nothing, and fails looking like a network fault.
    #[test]
    fn media_level_credentials_win_and_are_never_mixed_with_the_session_level() {
        let both = description(
            "a=ice-ufrag:sess\r\na=ice-pwd:sessionpasswordlongenough\r\n",
            "a=ice-ufrag:8hhY\r\na=ice-pwd:asd88fgpdd777uzjYhagZg\r\n",
        );
        let stream = both.media.first().expect("one stream");
        let credentials = both.ice_credentials_for(stream).expect("present");
        assert_eq!(credentials.ufrag(), "8hhY");
        assert_eq!(credentials.pwd(), "asd88fgpdd777uzjYhagZg");

        // Session level is a default for a stream that declares nothing.
        let inherited = description(
            "a=ice-ufrag:sess\r\na=ice-pwd:sessionpasswordlongenough\r\n",
            "",
        );
        let stream = inherited.media.first().expect("one stream");
        let credentials = inherited.ice_credentials_for(stream).expect("inherited");
        assert_eq!(credentials.ufrag(), "sess");
        assert_eq!(credentials.pwd(), "sessionpasswordlongenough");

        // Half a pair at the media level falls back to the session's *pair*, not to its password.
        let half = description(
            "a=ice-ufrag:sess\r\na=ice-pwd:sessionpasswordlongenough\r\n",
            "a=ice-ufrag:8hhY\r\n",
        );
        let stream = half.media.first().expect("one stream");
        let credentials = half.ice_credentials_for(stream).expect("falls back whole");
        assert_eq!(credentials.ufrag(), "sess");
        assert_eq!(credentials.pwd(), "sessionpasswordlongenough");

        // No ICE credentials anywhere means the stream is not doing ICE (§5.4).
        let none = description("", "");
        let stream = none.media.first().expect("one stream");
        assert_eq!(none.ice_credentials_for(stream), None);
    }

    /// Each attribute is read at the level RFC 8839 defines it at, and nowhere else. A
    /// media-level `a=ice-lite` does not make a peer lite — §5.3 puts it at session level, and
    /// honouring it anywhere would let one stream change how the whole agent is treated.
    #[test]
    fn each_attribute_is_read_only_at_the_level_that_defines_it() {
        let right = description("a=ice-lite\r\na=ice-pacing:100\r\n", "a=ice-mismatch\r\n");
        let stream = right.media.first().expect("one stream");
        assert!(right.is_ice_lite());
        assert_eq!(right.ice_pacing(), Pacing::from_millis(100));
        assert!(stream.ice_mismatch());

        let wrong = description("a=ice-mismatch\r\n", "a=ice-lite\r\na=ice-pacing:100\r\n");
        let stream = wrong.media.first().expect("one stream");
        assert!(!wrong.is_ice_lite(), "§5.3 puts ice-lite at session level");
        assert!(
            !stream.ice_mismatch(),
            "§5.3 puts ice-mismatch at media level"
        );
        assert_eq!(
            wrong.ice_pacing(),
            Pacing::DEFAULT,
            "§5.5 puts ice-pacing at session level"
        );

        // `candidate` and `remote-candidates` are media-level (§5.1, §5.2): a session-level copy
        // is not a candidate for any stream.
        let stray = description(
            "a=candidate:1 1 UDP 2130706431 192.0.2.1 8998 typ host\r\na=remote-candidates:1 192.0.2.3 45664\r\n",
            "",
        );
        let stream = stray.media.first().expect("one stream");
        assert!(stream.ice_candidates().is_empty());
        assert!(stream.ice_remote_candidates().is_empty());
    }

    /// §5.5: absent means 50 ms, and the two agents use the larger of what they asked for — the
    /// slower end wins, because pacing protects whichever end has less to spend.
    #[test]
    fn pacing_defaults_to_50_and_the_larger_value_is_agreed() {
        let silent = description("", "");
        assert_eq!(silent.ice_pacing(), Pacing::DEFAULT);
        assert_eq!(Pacing::DEFAULT.millis(), 50);
        assert_eq!(Pacing::parse("100"), Some(Pacing::from_millis(100)));
        assert_eq!(Pacing::parse("banana"), None);
        assert_eq!(
            Pacing::DEFAULT.agreed(Pacing::from_millis(200)),
            Pacing::from_millis(200)
        );
        assert_eq!(
            Pacing::from_millis(200).agreed(Pacing::DEFAULT),
            Pacing::from_millis(200)
        );
        // An unreadable value takes the default rather than nothing: §5.5 gives the absent case
        // a value, and a pacing of "unknown" has no meaning to give the checks.
        let broken = description("a=ice-pacing:not-a-number\r\n", "");
        assert_eq!(broken.ice_pacing(), Pacing::DEFAULT);
    }

    /// §5.6: option tags may appear at both levels, and both count. Unlike the credentials this
    /// is a union — an agent does not stop supporting an extension because one `m=` line named a
    /// different one.
    #[test]
    fn option_tags_are_read_from_both_levels() {
        let offer = description(
            "a=ice-options:ice2\r\n",
            "a=ice-options:rtp+ecn trickle\r\n",
        );
        let stream = offer.media.first().expect("one stream");
        assert_eq!(offer.ice_options().collect::<Vec<_>>(), vec![ICE2]);
        assert_eq!(
            stream.ice_options().collect::<Vec<_>>(),
            vec!["rtp+ecn", "trickle"]
        );
        assert_eq!(
            offer.ice_options_for(stream).collect::<Vec<_>>(),
            vec![ICE2, "rtp+ecn", "trickle"]
        );
        // A tag outside `1*ice-char` is dropped and its neighbours are kept: the attribute is a
        // list of independent capabilities.
        let ragged = description("a=ice-options:ice2 b@d trickle\r\n", "");
        assert_eq!(
            ragged.ice_options().collect::<Vec<_>>(),
            vec![ICE2, "trickle"]
        );
    }

    /// A foundation is `1*32ice-char`, and the bound is kept by the type rather than by whoever
    /// remembers to check it.
    #[test]
    fn a_foundation_keeps_its_bounds() {
        assert_eq!(
            Foundation::new("2").map(|f| f.as_str().to_owned()),
            Some("2".to_owned())
        );
        assert!(Foundation::new(&"a".repeat(32)).is_some());
        assert!(Foundation::new(&"a".repeat(33)).is_none());
        assert!(Foundation::new("").is_none());
        assert!(Foundation::new("a b").is_none());
        assert!(Foundation::new("+/").is_some(), "ice-char includes + and /");
    }
}
