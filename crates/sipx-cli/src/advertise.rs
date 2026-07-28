//! Which of our addresses to put in a message.

use std::net::{IpAddr, SocketAddr};

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
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| {
            socket.connect(std::net::SocketAddr::new(peer, 9))?;
            socket.local_addr()
        })
        .map_or_else(|_| "127.0.0.1".parse().unwrap_or(peer), |addr| addr.ip())
}

#[cfg(test)]
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
}
