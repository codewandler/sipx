//! The session description AST.
//!
//! Every line is kept. SDP grows new attributes constantly, and an element that silently drops
//! what it does not understand breaks features it has never heard of — the typed fields here
//! are a view over the lines, not a replacement for them.

use std::fmt::{self, Write as _};
use std::net::IpAddr;

/// A unicast address as written on an `o=` or `c=` line.
///
/// RFC 8866 §5.2 and §5.7 allow a fully-qualified domain name here, not just a literal. A
/// name is kept as written: resolving it takes a resolver, which is I/O this crate does not
/// do, and re-emitting it verbatim is what keeps a round trip faithful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Address {
    /// An IP literal.
    Ip(IpAddr),
    /// A fully-qualified domain name, kept as written.
    Host(String),
}

impl Address {
    /// The IP, when the address is a literal. A name yields `None`; turning it into an
    /// address is the caller's job.
    #[must_use]
    pub fn ip(&self) -> Option<IpAddr> {
        match self {
            Self::Ip(ip) => Some(*ip),
            Self::Host(_) => None,
        }
    }

    fn address_type(&self) -> &'static str {
        match self {
            Self::Ip(IpAddr::V6(_)) => "IP6",
            // RFC 8866 §5.7 requires an addrtype even for a name, whose family the
            // description alone cannot reveal; IP4 is the one every implementation accepts.
            _ => "IP4",
        }
    }
}

impl From<IpAddr> for Address {
    fn from(ip: IpAddr) -> Self {
        Self::Ip(ip)
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ip(ip) => ip.fmt(f),
            Self::Host(host) => f.write_str(host),
        }
    }
}

/// Which way media flows, from the point of view of the description that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Both ways. The default when no direction attribute is present (RFC 4566 §6).
    #[default]
    SendRecv,
    /// This side sends only.
    SendOnly,
    /// This side receives only.
    RecvOnly,
    /// Neither, but the stream stays negotiated.
    Inactive,
}

impl Direction {
    /// The direction an answer must carry to match an offer.
    ///
    /// This is a mirror, not a copy. An offer of `sendonly` means "I will send, you will
    /// receive", so the answer says `recvonly`. Copying the offer's direction instead is a
    /// common bug and produces a call where both ends wait for audio.
    #[must_use]
    pub fn mirrored(self) -> Self {
        match self {
            Self::SendRecv => Self::SendRecv,
            Self::SendOnly => Self::RecvOnly,
            Self::RecvOnly => Self::SendOnly,
            Self::Inactive => Self::Inactive,
        }
    }

    /// Parse a direction attribute name.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "sendrecv" => Some(Self::SendRecv),
            "sendonly" => Some(Self::SendOnly),
            "recvonly" => Some(Self::RecvOnly),
            "inactive" => Some(Self::Inactive),
            _ => None,
        }
    }

    /// The attribute name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SendRecv => "sendrecv",
            Self::SendOnly => "sendonly",
            Self::RecvOnly => "recvonly",
            Self::Inactive => "inactive",
        }
    }

    /// Whether this side will send media.
    #[must_use]
    pub fn sends(self) -> bool {
        matches!(self, Self::SendRecv | Self::SendOnly)
    }

    /// Whether this side will receive media.
    #[must_use]
    pub fn receives(self) -> bool {
        matches!(self, Self::SendRecv | Self::RecvOnly)
    }
}

/// An `a=` line: either `a=name` or `a=name:value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    /// The attribute name.
    pub name: String,
    /// Its value, if it has one.
    pub value: Option<String>,
}

impl Attribute {
    /// A flag attribute, like `a=sendrecv`.
    #[must_use]
    pub fn flag(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
        }
    }

    /// A valued attribute, like `a=rtpmap:0 PCMU/8000`.
    #[must_use]
    pub fn valued(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
        }
    }

    fn write_to(&self, out: &mut String) {
        match &self.value {
            Some(value) => {
                let _ = writeln!(out, "a={}:{value}\r", self.name);
            }
            None => {
                let _ = writeln!(out, "a={}\r", self.name);
            }
        }
    }
}

/// An `o=` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    /// The originator's name, or `-`.
    pub username: String,
    /// A session identifier.
    pub session_id: u64,
    /// The version, which increases with each modified offer.
    pub session_version: u64,
    /// The address the session is described from.
    pub address: Address,
}

impl Origin {
    /// An origin for an address, with the identifier and version supplied by the caller.
    ///
    /// The caller supplies them deliberately: a session version has to *increase* across
    /// re-offers, and only the caller knows what it used last.
    #[must_use]
    pub fn new(address: IpAddr, session_id: u64, session_version: u64) -> Self {
        Self {
            username: "-".to_owned(),
            session_id,
            session_version,
            address: Address::Ip(address),
        }
    }
}

/// A `c=` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    /// Where media should be sent.
    pub address: Address,
}

impl Connection {
    /// A connection line for an address.
    #[must_use]
    pub fn new(address: IpAddr) -> Self {
        Self {
            address: Address::Ip(address),
        }
    }
}

/// A `t=` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    /// Start time, 0 for unbounded.
    pub start: u64,
    /// Stop time, 0 for unbounded.
    pub stop: u64,
}

/// An `m=` line and everything under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDescription {
    /// `audio`, `video`, `application`…
    pub media: String,
    /// The port. Zero means the stream is rejected — and a rejected stream is still *present*,
    /// which is what keeps the answer's media lines aligned with the offer's.
    pub port: u16,
    /// The transport protocol, such as `RTP/AVP`.
    pub protocol: String,
    /// Payload type numbers, in preference order.
    pub formats: Vec<String>,
    /// A `c=` line for this stream, overriding the session's.
    pub connection: Option<Connection>,
    /// Attributes under this media line.
    pub attributes: Vec<Attribute>,
    /// Lines under this media line that this crate does not model, kept so they survive a
    /// round trip.
    pub other: Vec<(char, String)>,
}

impl MediaDescription {
    /// An audio stream offering these payload types.
    #[must_use]
    pub fn audio(port: u16, formats: Vec<String>) -> Self {
        Self {
            media: "audio".to_owned(),
            port,
            protocol: "RTP/AVP".to_owned(),
            formats,
            connection: None,
            attributes: Vec::new(),
            other: Vec::new(),
        }
    }

    /// Whether this stream is rejected.
    #[must_use]
    pub fn is_rejected(&self) -> bool {
        self.port == 0
    }

    /// The direction, defaulting to `sendrecv` when no attribute says otherwise.
    #[must_use]
    pub fn direction(&self) -> Direction {
        self.declared_direction().unwrap_or_default()
    }

    /// The direction attribute written under this `m=` line, if any.
    ///
    /// RFC 8866 §6.7: a stream without a direction of its own takes the session-level one,
    /// so an absent attribute is meaningful and not the same thing as `sendrecv`.
    #[must_use]
    pub fn declared_direction(&self) -> Option<Direction> {
        self.attributes.iter().find_map(|a| {
            a.value
                .is_none()
                .then(|| Direction::parse(&a.name))
                .flatten()
        })
    }

    /// Set the direction, replacing any existing one.
    pub fn set_direction(&mut self, direction: Direction) {
        self.attributes
            .retain(|a| !(a.value.is_none() && Direction::parse(&a.name).is_some()));
        self.attributes.push(Attribute::flag(direction.as_str()));
    }

    /// The first `a=crypto` line this stream carries that sipx can act on (RFC 4568).
    ///
    /// Several may be offered, in preference order. sipx takes the first it can perform rather
    /// than the first listed: an offer whose favourite suite is one sipx does not implement is
    /// still an offer worth answering.
    #[must_use]
    pub fn crypto(&self) -> Option<crate::crypto::Crypto> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "crypto")
            .filter_map(|attribute| attribute.value.as_deref())
            .find_map(crate::crypto::Crypto::parse)
    }

    /// The first `a=fingerprint` this stream carries that sipx may act on (RFC 8122 §5).
    ///
    /// §5.1 has an endpoint offer a fingerprint under *several* hash functions — "the 'SHA-256'
    /// hash function algorithm and the hash function used to generate the signature on the
    /// certificate" — so more than one line is normal and any of them identifies the same
    /// certificate. Taking the first sipx can compute is therefore correct rather than a shortcut;
    /// the ones it skips are `md5` and `md2`, which §5 forbids acting on.
    ///
    /// Looked for on the media description and not the session: a fingerprint may be given at
    /// either level, and the media-level one wins where both appear. A caller that wants the
    /// session-level fallback reads [`SessionDescription::fingerprint`].
    #[must_use]
    pub fn fingerprint(&self) -> Option<crate::fingerprint::Fingerprint> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "fingerprint")
            .filter_map(|attribute| attribute.value.as_deref())
            .find_map(crate::fingerprint::Fingerprint::parse)
    }

    /// The `a=setup` role this stream declares (RFC 4145 §4).
    #[must_use]
    pub fn setup(&self) -> Option<crate::fingerprint::Setup> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == "setup")
            .and_then(|attribute| attribute.value.as_deref())
            .and_then(crate::fingerprint::Setup::parse)
    }

    /// Every `a=candidate` under this stream that sipx can act on (RFC 8839 §5.1).
    ///
    /// Media-level, and only media-level: §5.1 defines the attribute there and nowhere else.
    ///
    /// Lines sipx cannot act on are **left out of the result rather than turned into an error** —
    /// an FQDN, an unsupported address family, a transport other than UDP, an unknown candidate
    /// type. §5.1 requires that a candidate be ignored, and ignoring it means ignoring the line:
    /// the attribute is still on the description and still round-trips, and the rest of the
    /// stream is still usable. A stack that refused the description instead would fail calls with
    /// peers doing nothing wrong.
    #[must_use]
    pub fn ice_candidates(&self) -> Vec<crate::ice::Candidate> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "candidate")
            .filter_map(|attribute| attribute.value.as_deref())
            .filter_map(crate::ice::Candidate::parse)
            .collect()
    }

    /// The `a=remote-candidates` this stream carries (RFC 8839 §5.2). Media-level.
    ///
    /// Present only in an offer from a controlling agent for a stream that is Completed, so an
    /// empty result is the normal case rather than a sign of anything.
    #[must_use]
    pub fn ice_remote_candidates(&self) -> Vec<crate::ice::RemoteCandidate> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "remote-candidates")
            .filter_map(|attribute| attribute.value.as_deref())
            .filter_map(crate::ice::RemoteCandidate::parse_list)
            .flatten()
            .collect()
    }

    /// This stream's own `a=ice-ufrag` (RFC 8839 §5.4), before the session-level default.
    ///
    /// Read [`SessionDescription::ice_credentials_for`] instead unless the distinction matters:
    /// a stream with no fragment of its own inherits the session's, and RFC 8839 §4.4.1.1.1
    /// makes the *pair* of values, not either alone, what an ICE restart changes.
    #[must_use]
    pub fn ice_ufrag(&self) -> Option<&str> {
        self.attribute_value("ice-ufrag")
    }

    /// This stream's own `a=ice-pwd` (RFC 8839 §5.4), before the session-level default.
    #[must_use]
    pub fn ice_pwd(&self) -> Option<&str> {
        self.attribute_value("ice-pwd")
    }

    /// The option tags this stream advertises (RFC 8839 §5.6).
    pub fn ice_options(&self) -> impl Iterator<Item = &str> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "ice-options")
            .filter_map(|attribute| attribute.value.as_deref())
            .flat_map(crate::ice::option_tags)
    }

    /// Whether this stream carries `a=ice-mismatch` (RFC 8839 §5.3). Media-level, in an answer.
    ///
    /// It means the offer's default destination for a component had no matching `candidate`
    /// attribute, and therefore that ICE MUST NOT be used for this stream — RFC 3264 procedures
    /// apply instead. Not a failure: it is the answerer saying an intermediary rewrote the
    /// addresses, which is what ICE was going to discover the hard way.
    #[must_use]
    pub fn ice_mismatch(&self) -> bool {
        self.has_flag("ice-mismatch")
    }

    fn attribute_value(&self, name: &str) -> Option<&str> {
        self.attribute(name)
            .and_then(|attribute| attribute.value.as_deref())
    }

    fn has_flag(&self, name: &str) -> bool {
        self.attributes
            .iter()
            .any(|attribute| attribute.name == name && attribute.value.is_none())
    }

    /// The `rtpmap` for a payload type, if the description gives one.
    #[must_use]
    pub fn rtpmap(&self, format: &str) -> Option<&str> {
        self.attributes.iter().find_map(|a| {
            if a.name != "rtpmap" {
                return None;
            }
            let value = a.value.as_deref()?;
            let (payload, rest) = value.split_once(' ')?;
            (payload == format).then_some(rest)
        })
    }

    /// The first attribute with this name.
    #[must_use]
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }

    fn write_to(&self, out: &mut String) {
        let _ = write!(out, "m={} {} {}", self.media, self.port, self.protocol);
        for format in &self.formats {
            let _ = write!(out, " {format}");
        }
        let _ = writeln!(out, "\r");
        // RFC 8866 §5.14 fixes the order inside a media description: `i=` before `c=`, then
        // `b=` and `k=`, then the attributes.
        write_other_lines(out, &self.other, |kind| kind == 'i');
        if let Some(connection) = &self.connection {
            let _ = writeln!(
                out,
                "c=IN {} {}\r",
                connection.address.address_type(),
                connection.address
            );
        }
        write_other_lines(out, &self.other, |kind| kind != 'i');
        for attribute in &self.attributes {
            attribute.write_to(out);
        }
    }
}

fn write_other_lines(out: &mut String, lines: &[(char, String)], take: impl Fn(char) -> bool) {
    for (kind, value) in lines {
        if take(*kind) {
            let _ = writeln!(out, "{kind}={value}\r");
        }
    }
}

/// A whole session description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDescription {
    /// The `o=` line.
    pub origin: Origin,
    /// The `s=` line.
    pub session_name: String,
    /// The session-level `c=` line.
    pub connection: Option<Connection>,
    /// The `t=` lines.
    pub timing: Vec<Timing>,
    /// Session-level attributes.
    pub attributes: Vec<Attribute>,
    /// The media streams, in order. The order is load-bearing: an answer's streams correspond
    /// to the offer's by position.
    pub media: Vec<MediaDescription>,
    /// Lines this crate does not model, kept so they survive a round trip.
    pub other: Vec<(char, String)>,
}

impl SessionDescription {
    /// A session description for an address.
    #[must_use]
    pub fn new(address: IpAddr, session_id: u64, session_version: u64) -> Self {
        Self {
            origin: Origin::new(address, session_id, session_version),
            session_name: "-".to_owned(),
            connection: Some(Connection::new(address)),
            timing: vec![Timing::default()],
            attributes: Vec::new(),
            media: Vec::new(),
            other: Vec::new(),
        }
    }

    /// The connection address for a stream: its own `c=` if it has one, else the session's.
    ///
    /// A domain-name address yields `None` here — the name is preserved in the description,
    /// but only a resolver can turn it into somewhere to send media.
    #[must_use]
    pub fn address_for(&self, media: &MediaDescription) -> Option<IpAddr> {
        media
            .connection
            .as_ref()
            .or(self.connection.as_ref())
            .and_then(|connection| connection.address.ip())
    }

    /// The session-level direction, defaulting to `sendrecv`.
    #[must_use]
    pub fn direction(&self) -> Direction {
        self.attributes
            .iter()
            .find_map(|a| {
                a.value
                    .is_none()
                    .then(|| Direction::parse(&a.name))
                    .flatten()
            })
            .unwrap_or_default()
    }

    /// The session-level `a=fingerprint`, if the description carries one (RFC 8122 §5).
    ///
    /// §5 allows the attribute at either level, and one given here applies to every stream that
    /// does not override it. A browser puts it here and on no `m=` line at all, so a stack that
    /// reads only the media level finds nothing and refuses a perfectly good offer.
    #[must_use]
    pub fn fingerprint(&self) -> Option<crate::fingerprint::Fingerprint> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "fingerprint")
            .filter_map(|attribute| attribute.value.as_deref())
            .find_map(crate::fingerprint::Fingerprint::parse)
    }

    /// The session-level `a=ice-ufrag` (RFC 8839 §5.4), which is a default for every stream.
    #[must_use]
    pub fn ice_ufrag(&self) -> Option<&str> {
        self.attribute_value("ice-ufrag")
    }

    /// The session-level `a=ice-pwd` (RFC 8839 §5.4), which is a default for every stream.
    #[must_use]
    pub fn ice_pwd(&self) -> Option<&str> {
        self.attribute_value("ice-pwd")
    }

    /// The short-term credentials that apply to one stream (RFC 8839 §5.4).
    ///
    /// **Media level wins.** §5.4 allows the attributes at either level and makes the session
    /// level a default, so a stream with its own `ice-ufrag` uses it and a stream without it
    /// inherits — and the two must not be mixed: taking the fragment from the media line and the
    /// password from the session line produces a credential neither end can authenticate with,
    /// and it looks exactly like a network fault. The pair is therefore resolved together, from
    /// whichever level supplied the fragment.
    ///
    /// `None` when the description gives no usable pair at either level, which per §5.4 means
    /// the stream is not doing ICE. Values up to 256 characters are accepted, as §5.4 requires,
    /// even though sipx will not send one longer than 32.
    #[must_use]
    pub fn ice_credentials_for(&self, media: &MediaDescription) -> Option<crate::ice::Credentials> {
        let level = |ufrag: Option<&str>, pwd: Option<&str>| match (ufrag, pwd) {
            (Some(ufrag), Some(pwd)) => crate::ice::Credentials::received(ufrag, pwd),
            _ => None,
        };
        level(media.ice_ufrag(), media.ice_pwd())
            .or_else(|| level(self.ice_ufrag(), self.ice_pwd()))
    }

    /// The session-level option tags (RFC 8839 §5.6).
    pub fn ice_options(&self) -> impl Iterator<Item = &str> {
        self.attributes
            .iter()
            .filter(|attribute| attribute.name == "ice-options")
            .filter_map(|attribute| attribute.value.as_deref())
            .flat_map(crate::ice::option_tags)
    }

    /// The option tags that apply to one stream: the session's and the stream's together.
    ///
    /// A union and not an override, which is where this differs from the credentials above.
    /// §5.6 makes the attribute a statement that "a certain extension is supported by the agent",
    /// and an agent does not stop supporting an extension because a particular `m=` line named a
    /// different one. Tags may repeat if both levels name the same one.
    pub fn ice_options_for<'a>(
        &'a self,
        media: &'a MediaDescription,
    ) -> impl Iterator<Item = &'a str> {
        self.ice_options().chain(media.ice_options())
    }

    /// Whether the description carries `a=ice-lite` (RFC 8839 §5.3). Session-level only.
    ///
    /// A lite peer never gathers, never sends a check and never nominates, so sipx takes the
    /// controlling role unconditionally against one (RFC 8445 §6.1.1) and must not wait for
    /// checks that will never arrive. sipx itself is always a full agent and never sends this.
    #[must_use]
    pub fn is_ice_lite(&self) -> bool {
        self.has_flag("ice-lite")
    }

    /// The `a=ice-pacing` the description asks for (RFC 8839 §5.5). Session-level only.
    ///
    /// [`Pacing::DEFAULT`] when the attribute is absent or unreadable, because §5.5 gives the
    /// absent case a value — 50 ms — rather than leaving it undefined.
    ///
    /// [`Pacing::DEFAULT`]: crate::ice::Pacing::DEFAULT
    #[must_use]
    pub fn ice_pacing(&self) -> crate::ice::Pacing {
        self.attribute_value("ice-pacing")
            .and_then(crate::ice::Pacing::parse)
            .unwrap_or(crate::ice::Pacing::DEFAULT)
    }

    fn attribute_value(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|attribute| attribute.name == name)
            .and_then(|attribute| attribute.value.as_deref())
    }

    fn has_flag(&self, name: &str) -> bool {
        self.attributes
            .iter()
            .any(|attribute| attribute.name == name && attribute.value.is_none())
    }

    /// Serialize to the wire format.
    ///
    /// Line order follows RFC 8866 §5, which is not a style preference: the grammar fixes the
    /// order, and receivers do reject descriptions that get it wrong.
    #[must_use]
    pub fn to_string_sdp(&self) -> String {
        let mut out = String::with_capacity(256);
        let _ = writeln!(out, "v=0\r");
        let _ = writeln!(
            out,
            "o={} {} {} IN {} {}\r",
            self.origin.username,
            self.origin.session_id,
            self.origin.session_version,
            self.origin.address.address_type(),
            self.origin.address
        );
        let _ = writeln!(out, "s={}\r", self.session_name);
        // RFC 8866 §5 gives every line type a fixed slot: `i=`, `u=`, `e=` and `p=` before
        // `c=`, `b=` between `c=` and the timing lines, everything else after them. Kept
        // lines go into their slot, not wherever is convenient, because receivers enforce
        // the grammar's order.
        write_other_lines(&mut out, &self.other, |kind| {
            matches!(kind, 'i' | 'u' | 'e' | 'p')
        });
        if let Some(connection) = &self.connection {
            let _ = writeln!(
                out,
                "c=IN {} {}\r",
                connection.address.address_type(),
                connection.address
            );
        }
        write_other_lines(&mut out, &self.other, |kind| kind == 'b');
        if self.timing.is_empty() {
            let _ = writeln!(out, "t=0 0\r");
        }
        for timing in &self.timing {
            let _ = writeln!(out, "t={} {}\r", timing.start, timing.stop);
        }
        write_other_lines(&mut out, &self.other, |kind| {
            !matches!(kind, 'i' | 'u' | 'e' | 'p' | 'b')
        });
        for attribute in &self.attributes {
            attribute.write_to(&mut out);
        }
        for media in &self.media {
            media.write_to(&mut out);
        }
        out
    }
}
