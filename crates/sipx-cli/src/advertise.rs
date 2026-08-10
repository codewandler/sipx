//! Which of our addresses to put in a message.

use std::net::{IpAddr, SocketAddr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MediaAddresses {
    pub(crate) advertised: IpAddr,
    pub(crate) bind: IpAddr,
}

/// Choose the two media address roles for a command.
///
/// An explicit advertised address may be a NAT mapping the host cannot bind, so `local` remains
/// the bind choice. Without that override, the routing-table-selected address is locally owned:
/// binding it preserves host and server-reflexive ICE gathering on a wildcard signalling bind.
pub(crate) fn media_addresses(
    local: SocketAddr,
    peer: IpAddr,
    advertised: Option<IpAddr>,
) -> MediaAddresses {
    if let Some(advertised) = advertised {
        MediaAddresses {
            advertised,
            bind: local.ip(),
        }
    } else {
        let selected = reachable_ip(local, peer);
        MediaAddresses {
            advertised: selected,
            bind: selected,
        }
    }
}

/// The address this endpoint should advertise when talking to `peer`.
///
/// Everything that carries an address — the `Via` sent-by (RFC 3261 §18.1.1), the `Contact`
/// (RFC 3261 §8.1.1.8), an SDP connection line — promises a place *this* endpoint can be
/// reached. An explicit bind address is that promise; the unspecified one names every
/// interface and reaches none, so the routing table is asked which of our addresses faces
/// the peer instead.
pub(crate) fn reachable_ip(local: SocketAddr, peer: IpAddr) -> IpAddr {
    if !local.ip().is_unspecified() {
        return local.ip();
    }
    if peer.is_loopback() {
        return "127.0.0.1".parse().unwrap_or(peer);
    }
    local_address_towards(peer)
}

/// Which of our addresses faces a peer.
///
/// Asking the routing table by opening a UDP socket towards the peer — no packet is sent, but
/// the kernel picks the source address it would use, which is the one to advertise.
fn local_address_towards(peer: IpAddr) -> IpAddr {
    route_source(peer).unwrap_or_else(|| "127.0.0.1".parse().unwrap_or(peer))
}

/// Ask the operating system which local address would source traffic to `peer`.
///
/// Unlike [`reachable_ip`], this retains failure as `None`: a route diagnosis must not turn an
/// unavailable observation into a claim about an interface.
pub(crate) fn route_source(peer: IpAddr) -> Option<IpAddr> {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect(std::net::SocketAddr::new(peer, 9))?;
            socket.local_addr()
        })
        .ok()
        .map(|addr| addr.ip())
}

/// Return the two local sources when signalling and media select different interfaces.
pub(crate) fn split_route(signalling: IpAddr, media: IpAddr) -> Option<(IpAddr, IpAddr)> {
    (signalling != media).then_some((signalling, media))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_bind_address_is_advertised_as_given() {
        let local: SocketAddr = "192.0.2.10:5060".parse().unwrap();
        let peer: IpAddr = "203.0.113.1".parse().unwrap();
        assert_eq!(reachable_ip(local, peer), local.ip());
    }

    #[test]
    fn a_loopback_peer_is_faced_from_loopback() {
        let local: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        assert_eq!(reachable_ip(local, peer), peer);
    }

    /// RFC 3261 §18.1.1: sent-by is where the *sender* expects responses. Whatever the
    /// routing table answers, it must be one of our addresses — never the peer's copied
    /// back, and never the unspecified one, which no packet can be sent to.
    #[test]
    fn the_advertised_address_is_never_the_peers_or_unspecified() {
        let local: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let peer: IpAddr = "192.0.2.1".parse().unwrap();
        let advertised = reachable_ip(local, peer);
        assert_ne!(advertised, peer, "the peer's address is somebody else's");
        assert!(!advertised.is_unspecified());
    }

    #[test]
    fn an_implicit_wildcard_choice_binds_the_reachable_local_address_for_ice() {
        let local: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let peer: IpAddr = "192.0.2.1".parse().unwrap();
        let addresses = media_addresses(local, peer, None);
        assert_eq!(addresses.bind, addresses.advertised);
        assert_ne!(addresses.bind, peer);
        assert!(!addresses.bind.is_unspecified());
    }

    #[test]
    fn an_explicit_public_mapping_keeps_the_wildcard_bind_independent() {
        let local: SocketAddr = "0.0.0.0:0".parse().unwrap();
        let advertised: IpAddr = "198.51.100.44".parse().unwrap();
        let addresses = media_addresses(local, "192.0.2.1".parse().unwrap(), Some(advertised));
        assert_eq!(addresses.advertised, advertised);
        assert_eq!(addresses.bind, local.ip());
    }

    #[test]
    fn split_interfaces_are_a_distinct_diagnosis() {
        let signalling: IpAddr = "10.99.0.3".parse().unwrap();
        let media: IpAddr = "192.168.1.50".parse().unwrap();
        assert_eq!(split_route(signalling, media), Some((signalling, media)));
        assert_eq!(split_route(signalling, signalling), None);
    }
}
