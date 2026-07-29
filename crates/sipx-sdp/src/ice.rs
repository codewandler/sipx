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
//! well-formed text, but §5.1 bounds the value at 2^31 − 1 and RFC 8445 §6.1.2.3's pair priority
//! — `2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)` — only fits in a `u64` because of that bound. The
//! check here is what makes that arithmetic safe later; see [`docs/specs/ice.md`] §4 and §6.2.
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
        token.eq_ignore_ascii_case(Self::Udp.as_str()).then_some(Self::Udp)
    }
}

/// A candidate priority: a positive integer up to 2^31 − 1 (RFC 8839 §5.1).
///
/// The bound is the type's whole reason for existing. RFC 8445 §6.1.2.3 combines two priorities
/// into a pair priority as `2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)`, whose largest value with
/// both operands at 2^31 − 1 is exactly `2^63 − 1`. Carry an unchecked `u32` from the wire into
/// that expression instead and it overflows — in a release build silently, reordering the
/// checklist that the arithmetic exists to order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Priority(u32);

impl Priority {
    /// The largest priority RFC 8839 §5.1 permits.
    pub const MAX: Self = Self(0x7fff_ffff);
    /// The smallest. §5.1 says "positive", so zero is not a priority.
    pub const MIN: Self = Self(1);

    /// The priority with this value, if it is in range.
    pub fn new(value: u32) -> Option<Self> {
        (Self::MIN.0..=Self::MAX.0).contains(&value).then_some(Self(value))
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
    /// has never heard of. `None` here makes [`Candidate::parse`] ignore the line: a candidate
    /// whose type is unknown cannot be given a foundation (RFC 8445 §5.1.1.3 keys foundations on
    /// the type) and so cannot be placed in a checklist correctly.
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
}
