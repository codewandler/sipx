//! Where a message goes, and how a response finds its way back.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use sipx_sip::headers::Via;
use sipx_sip::transaction::Reliability;

/// Which transport a message travels over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransportKind {
    /// UDP: retransmits, one message per datagram.
    Udp,
    /// TCP: reliable, `Content-Length` framing.
    Tcp,
    /// TLS over TCP.
    Tls,
    /// SIP over WebSocket (RFC 7118).
    Ws,
    /// SIP over secure WebSocket.
    Wss,
    /// SIP over QUIC using sipx's experimental mapping.
    Quic,
}

impl TransportKind {
    /// Whether the transport delivers reliably, which decides half the transaction timers.
    #[must_use]
    pub fn reliability(self) -> Reliability {
        match self {
            Self::Udp => Reliability::Unreliable,
            _ => Reliability::Reliable,
        }
    }

    /// The token this transport is spelled with in a `Via`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Udp => "UDP",
            Self::Tcp => "TCP",
            Self::Tls => "TLS",
            Self::Ws => "WS",
            Self::Wss => "WSS",
            Self::Quic => "QUIC",
        }
    }

    /// Whether this transport protects the signalling itself.
    ///
    /// The question SDES turns on. RFC 4568 §7.1 makes a secure signalling path a *condition* of
    /// carrying a key in SDP, because the key travels in the body — so this decides whether sipx
    /// may offer encrypted media at all.
    #[must_use]
    pub fn is_secure(self) -> bool {
        matches!(self, Self::Tls | Self::Wss | Self::Quic)
    }

    /// The default port, per RFC 3261 §19.1.2 and RFC 7118.
    #[must_use]
    pub fn default_port(self) -> u16 {
        match self {
            Self::Udp | Self::Tcp => 5060,
            Self::Tls | Self::Quic => 5061,
            Self::Ws => 80,
            Self::Wss => 443,
        }
    }

    /// Resolve a transport token from a `Via` or a URI parameter.
    #[must_use]
    pub fn parse(token: &[u8]) -> Option<Self> {
        match token.to_ascii_uppercase().as_slice() {
            b"UDP" => Some(Self::Udp),
            b"TCP" => Some(Self::Tcp),
            b"TLS" => Some(Self::Tls),
            b"WS" => Some(Self::Ws),
            b"WSS" => Some(Self::Wss),
            b"QUIC" => Some(Self::Quic),
            _ => None,
        }
    }
}

/// A destination.
///
/// Not `Copy`, because of `verify_as`. That field is the reason this type exists rather than a
/// bare `(SocketAddr, TransportKind)`: the address says where to send, while the original URI host
/// says which TLS identity to verify and which HTTP authority a WebSocket handshake must name.
/// Deriving either from the resolved address loses authority information before the connection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    /// Where to send.
    pub addr: SocketAddr,
    /// How to send.
    pub transport: TransportKind,
    /// Original URI host before resolution: TLS/WSS verification name and WS/WSS HTTP authority.
    /// For clear WS it is authority only; no certificate verification is implied.
    pub verify_as: Option<Arc<str>>,
    /// The resource the WebSocket handshake asks for. `None` means `/`, and `None` outside
    /// WebSocket, where there is no handshake to ask anything of.
    pub path: Option<Arc<str>>,
}

impl Target {
    /// A destination.
    #[must_use]
    pub fn new(addr: SocketAddr, transport: TransportKind) -> Self {
        Self {
            addr,
            transport,
            verify_as: None,
            path: None,
        }
    }

    /// A UDP destination.
    #[must_use]
    pub fn udp(addr: SocketAddr) -> Self {
        Self::new(addr, TransportKind::Udp)
    }

    /// The same destination, with its pre-resolution URI host.
    ///
    /// TLS/WSS verify certificates against it. WS/WSS also put it in the HTTP `Host` authority;
    /// clear WS uses that authority without implying authentication.
    #[must_use]
    pub fn verifying(mut self, name: impl AsRef<str>) -> Self {
        self.verify_as = Some(Arc::from(name.as_ref()));
        self
    }

    /// The same destination, with the resource its WebSocket handshake asks for.
    ///
    /// RFC 7118 §5 registers a subprotocol and a set of framing rules; it says nothing at all
    /// about where on a server SIP lives. A server is therefore entitled to serve it from `/`,
    /// from `/ws`, or from its own HTTP server on another port, and a client that can only ask
    /// for `/` reaches the first kind and none of the others.
    ///
    /// A leading `/` is supplied when it is missing, because a resource name that lacks one is
    /// not a relative path in a request-target — it runs into the authority and silently sends
    /// the upgrade somewhere nobody meant. Anything after the path is kept as given: a server
    /// that wants a query string gets the one it was handed.
    #[must_use]
    pub fn at_path(mut self, path: impl AsRef<str>) -> Self {
        let path = path.as_ref();
        self.path = Some(if path.starts_with('/') {
            Arc::from(path)
        } else {
            Arc::from(format!("/{path}"))
        });
        self
    }

    /// Which pooled connection carries traffic for this destination.
    #[must_use]
    pub fn connection(&self) -> ConnectionKey {
        ConnectionKey {
            peer: self.addr,
            transport: self.transport,
            identity: self.verify_as.clone(),
            path: if matches!(self.transport, TransportKind::Ws | TransportKind::Wss) {
                self.path.clone()
            } else {
                None
            },
        }
    }
}

/// What makes two connections the same connection.
///
/// Not the address alone, and each of the other two fields earns its place.
///
/// **The transport**, because TCP, TLS and QUIC to one address are not interchangeable: a `sips:`
/// request riding a cleartext socket has silently become what it asked not to be. With
/// WebSocket in the mix the case stops being theoretical — WS and TCP can and do share a port.
///
/// **The URI authority/verified identity**, because two names resolving to one address are still
/// distinct TLS identities and distinct WebSocket HTTP authorities. Reusing one for another either
/// throws away certificate verification or sends traffic on an upgrade granted to a different
/// virtual host. `None` on a connection a peer opened: sipx selected no outbound authority.
///
/// **The WebSocket resource**, for the same reason one step down: a socket upgraded at `/ws` was
/// accepted by whatever serves `/ws`, and handing it traffic that asked for `/other` ignores the
/// only thing the target said about where it wanted to go. `None` everywhere the question does
/// not arise — every other transport, and every connection a peer opened.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionKey {
    /// The far end.
    pub peer: SocketAddr,
    /// Which transport it speaks.
    pub transport: TransportKind,
    /// Original URI host: verified for TLS/WSS and used as authority for WS/WSS.
    pub identity: Option<Arc<str>>,
    /// The resource the upgrade asked for, for WebSocket connections sipx opened.
    pub path: Option<Arc<str>>,
}

impl ConnectionKey {
    /// A connection with nothing verified about it and no resource named — anything a peer
    /// opened, and every cleartext transport.
    #[must_use]
    pub fn new(peer: SocketAddr, transport: TransportKind) -> Self {
        Self {
            peer,
            transport,
            identity: None,
            path: None,
        }
    }

    /// The resource a WebSocket handshake for this connection asks for.
    ///
    /// `/` when the target named none, which is what RFC 6455 §3 requires of a request-target
    /// that would otherwise be empty and what every server serving SIP at its root expects.
    #[must_use]
    pub fn ws_path(&self) -> &str {
        self.path.as_deref().unwrap_or("/")
    }
}

/// Where a response to this request must be sent (RFC 3261 §18.2.2).
///
/// The order is the RFC's and each step exists for a reason: `maddr` is an explicit override,
/// `received` is where the request actually came from as opposed to where the sender believed
/// it was, and the sent-by is what the sender claims. Behind a NAT only `received` is true,
/// which is why the fallback order matters more than it looks.
#[must_use]
pub fn response_destination(via: &Via, source: SocketAddr, transport: TransportKind) -> Target {
    // 1. An explicit maddr wins.
    if let Some(maddr) = via.maddr()
        && let Some(addr) = parse_host(maddr)
    {
        let port = via.port.unwrap_or_else(|| transport.default_port());
        return Target::new(SocketAddr::new(addr, port), transport);
    }

    // RFC 3581 §4: an observed `rport` names the port the response has to go to, whichever
    // address the steps below settle on. It is not tied to `received` — a client whose
    // sent-by host is right but whose port was rewritten, or which simply sent from an
    // ephemeral socket, has its pinhole open here and nothing listening on the claimed port.
    let observed_port = via
        .rport()
        .flatten()
        .and_then(|v| std::str::from_utf8(v).ok())
        .and_then(|v| v.parse::<u16>().ok());

    // 2. received, at the rport if the sender asked us to observe one.
    if let Some(received) = via.received()
        && let Some(addr) = parse_host(received)
    {
        let port = observed_port
            .or(via.port)
            .unwrap_or_else(|| transport.default_port());
        return Target::new(SocketAddr::new(addr, port), transport);
    }

    // 3. The sent-by, if it is an address we can use directly.
    if let sipx_sip::Host::Ip(ip) = &via.host {
        let port = observed_port
            .or(via.port)
            .unwrap_or_else(|| transport.default_port());
        return Target::new(SocketAddr::new(*ip, port), transport);
    }

    // A hostname sent-by needs resolution, which the caller does. Falling back to the source
    // address is both the safest answer and, behind a NAT, the only one that works.
    Target::new(source, transport)
}

fn parse_host(raw: &[u8]) -> Option<IpAddr> {
    std::str::from_utf8(raw).ok()?.parse().ok()
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

    fn via(text: &str) -> Via {
        Via::parse_one(text.as_bytes()).expect("a valid Via")
    }

    fn source() -> SocketAddr {
        "203.0.113.9:41234".parse().expect("a valid address")
    }

    #[test]
    fn a_plain_via_goes_to_its_sent_by() {
        let target = response_destination(
            &via("SIP/2.0/UDP 192.0.2.1:5060;branch=z9hG4bKx"),
            source(),
            TransportKind::Udp,
        );
        assert_eq!(target.addr.to_string(), "192.0.2.1:5060");
    }

    #[test]
    fn a_sent_by_without_a_port_uses_the_transport_default() {
        assert_eq!(
            response_destination(
                &via("SIP/2.0/UDP 192.0.2.1;branch=z9hG4bKx"),
                source(),
                TransportKind::Udp
            )
            .addr
            .port(),
            5060
        );
        assert_eq!(
            response_destination(
                &via("SIP/2.0/TLS 192.0.2.1;branch=z9hG4bKx"),
                source(),
                TransportKind::Tls
            )
            .addr
            .port(),
            5061
        );
    }

    /// RFC 3581 §4: when the topmost `Via` carries an `rport`, the response goes to the source
    /// IP address *and port* the request came from. The port matters on its own — a client
    /// whose sent-by names the right host but the wrong port (an ephemeral socket, or a NAT
    /// that rewrote only the port) has a pinhole open on the observed port and nothing
    /// listening on the claimed one.
    #[test]
    fn an_observed_rport_is_used_even_without_a_received() {
        let target = response_destination(
            &via("SIP/2.0/UDP 203.0.113.9:5060;rport=41234;branch=z9hG4bKx"),
            source(),
            TransportKind::Udp,
        );
        assert_eq!(target.addr.to_string(), "203.0.113.9:41234");
    }

    /// The NAT case, and the reason this function is not one line. The sender believes it is
    /// at 10.0.0.5:5060; it is actually behind a NAT and reachable only at the observed
    /// address and port.
    #[test]
    fn received_and_rport_override_the_sent_by() {
        let target = response_destination(
            &via("SIP/2.0/UDP 10.0.0.5:5060;received=203.0.113.9;rport=41234;branch=z9hG4bKx"),
            source(),
            TransportKind::Udp,
        );
        assert_eq!(target.addr.to_string(), "203.0.113.9:41234");
    }

    #[test]
    fn received_without_rport_uses_the_sent_by_port() {
        let target = response_destination(
            &via("SIP/2.0/UDP 10.0.0.5:5070;received=203.0.113.9;branch=z9hG4bKx"),
            source(),
            TransportKind::Udp,
        );
        assert_eq!(target.addr.to_string(), "203.0.113.9:5070");
    }

    #[test]
    fn maddr_wins_over_everything() {
        let target = response_destination(
            &via("SIP/2.0/UDP 10.0.0.5:5060;maddr=192.0.2.99;received=203.0.113.9;branch=z9hG4bKx"),
            source(),
            TransportKind::Udp,
        );
        assert_eq!(target.addr.ip().to_string(), "192.0.2.99");
    }

    /// A hostname sent-by cannot be used without resolving it, and the source address is both
    /// the safest fallback and the only one that works behind a NAT.
    #[test]
    fn a_hostname_sent_by_falls_back_to_the_source() {
        let target = response_destination(
            &via("SIP/2.0/UDP client.example.com;branch=z9hG4bKx"),
            source(),
            TransportKind::Udp,
        );
        assert_eq!(target.addr, source());
    }

    #[test]
    fn transports_have_their_rfc_default_ports() {
        assert_eq!(TransportKind::Udp.default_port(), 5060);
        assert_eq!(TransportKind::Tcp.default_port(), 5060);
        assert_eq!(TransportKind::Tls.default_port(), 5061);
        assert_eq!(TransportKind::Ws.default_port(), 80);
        assert_eq!(TransportKind::Wss.default_port(), 443);
        assert_eq!(TransportKind::Quic.default_port(), 5061);
    }

    fn peer() -> SocketAddr {
        "127.0.0.1:8088".parse().expect("a valid address")
    }

    /// Nothing that works today changes: a target that names no resource asks for the root,
    /// which is where every server sipx has ever reached serves SIP.
    #[test]
    fn a_target_asks_for_the_root_unless_it_says_otherwise() {
        let target = Target::new(peer(), TransportKind::Ws);
        assert_eq!(target.path, None);
        assert_eq!(target.connection().ws_path(), "/");
    }

    #[test]
    fn a_target_can_name_the_resource_it_wants() {
        let target = Target::new(peer(), TransportKind::Ws).at_path("/ws");
        assert_eq!(target.path.as_deref(), Some("/ws"));
        assert_eq!(target.connection().ws_path(), "/ws");
    }

    /// A resource name without a leading slash is not a relative path in a request-target — it
    /// runs into the authority, and `ws://127.0.0.1:8088ws` is a request to somewhere nobody
    /// meant. Supplying the slash is the difference between a typo and a silent misdirection.
    #[test]
    fn a_resource_name_missing_its_leading_slash_gets_one() {
        for named in ["ws", "/ws"] {
            assert_eq!(
                Target::new(peer(), TransportKind::Ws)
                    .at_path(named)
                    .connection()
                    .ws_path(),
                "/ws"
            );
        }
        assert_eq!(
            Target::new(peer(), TransportKind::Ws)
                .at_path("")
                .connection()
                .ws_path(),
            "/",
            "naming nothing is naming the root"
        );
    }

    /// The same argument the verified identity makes, one step down: a socket upgraded at `/ws`
    /// was accepted by whatever serves `/ws`, so handing it traffic that asked for somewhere
    /// else throws away the only thing the target said about where it was going.
    #[test]
    fn two_resources_on_one_address_are_two_connections() {
        let one = Target::new(peer(), TransportKind::Ws).at_path("/ws");
        let other = Target::new(peer(), TransportKind::Ws).at_path("/sip");
        assert_ne!(one.connection(), other.connection());
        assert_ne!(
            one.connection(),
            Target::new(peer(), TransportKind::Ws).connection(),
            "the root is a resource like any other"
        );
    }

    #[test]
    fn only_udp_is_unreliable() {
        assert_eq!(TransportKind::Udp.reliability(), Reliability::Unreliable);
        for t in [
            TransportKind::Tcp,
            TransportKind::Tls,
            TransportKind::Ws,
            TransportKind::Wss,
            TransportKind::Quic,
        ] {
            assert_eq!(t.reliability(), Reliability::Reliable);
        }
    }

    #[test]
    fn quic_is_a_secure_reliable_via_transport() {
        assert_eq!(TransportKind::parse(b"QUIC"), Some(TransportKind::Quic));
        assert_eq!(TransportKind::Quic.as_str(), "QUIC");
        assert!(TransportKind::Quic.is_secure());
    }

    #[test]
    fn quic_pool_keys_keep_verified_names_and_transports_separate() {
        let one = Target::new(peer(), TransportKind::Quic)
            .verifying("one.example")
            .connection();
        let two = Target::new(peer(), TransportKind::Quic)
            .verifying("two.example")
            .connection();
        let tls = Target::new(peer(), TransportKind::Tls)
            .verifying("one.example")
            .connection();
        assert_ne!(one, two, "Q15: two authenticated names are two connections");
        assert_ne!(one, tls, "Q16: QUIC and TLS cannot share a connection");
    }

    #[test]
    fn quic_pool_keys_never_include_a_websocket_resource() {
        let plain = Target::new(peer(), TransportKind::Quic)
            .verifying("one.example")
            .connection();
        let with_irrelevant_path = Target::new(peer(), TransportKind::Quic)
            .verifying("one.example")
            .at_path("/ws")
            .connection();
        assert_eq!(plain, with_irrelevant_path);
        assert_eq!(plain.path, None);
    }
}
