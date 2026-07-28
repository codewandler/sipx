//! Transaction timers (RFC 3261 §17, Table 4).

use std::time::Duration;

use crate::transaction::Reliability;

/// The timers of RFC 3261 §17.
///
/// Named by letter because that is what the RFC calls them and what every packet capture and
/// every mailing-list thread will call them. Renaming them to something friendlier would only
/// make the code harder to check against the specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Timer {
    /// INVITE request retransmission. Unreliable transports only.
    A,
    /// INVITE client transaction timeout.
    B,
    /// Wait for response retransmissions after a non-2xx final response.
    D,
    /// Non-INVITE request retransmission. Unreliable transports only.
    E,
    /// Non-INVITE client transaction timeout.
    F,
    /// INVITE response retransmission. Unreliable transports only.
    G,
    /// Wait for an ACK.
    H,
    /// Wait for ACK retransmissions.
    I,
    /// Wait for non-INVITE request retransmissions.
    J,
    /// Wait for response retransmissions, non-INVITE client.
    K,
    /// RFC 6026: wait for an ACK to a 2xx.
    L,
    /// RFC 6026: wait for retransmissions of a 2xx.
    M,
    /// The 200 ms after which a server transaction sends 100 Trying by itself.
    ///
    /// Not lettered in the RFC — §17.2.1 states it as a plain duration — but it is a timer and
    /// the machine needs a name for it.
    Trying100,
}

/// The three constants everything else is derived from.
#[derive(Debug, Clone, Copy)]
pub struct Timers {
    /// Round-trip estimate. The base of every backoff.
    pub t1: Duration,
    /// Ceiling for retransmission intervals.
    pub t2: Duration,
    /// Longest a message can linger in the network.
    pub t4: Duration,
}

impl Default for Timers {
    fn default() -> Self {
        Self {
            t1: Duration::from_millis(500),
            t2: Duration::from_secs(4),
            t4: Duration::from_secs(5),
        }
    }
}

impl Timers {
    /// 64·T1 — how long a transaction waits before giving up.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.t1 * 64
    }

    /// Timer D: at least 32 s on an unreliable transport, nothing on a reliable one.
    ///
    /// The RFC gives 32 s rather than a multiple of T1 because the purpose is to outlast
    /// response retransmissions from the *other* end, whose T1 we do not know.
    #[must_use]
    pub fn timer_d(&self, reliability: Reliability) -> Duration {
        if reliability.is_reliable() {
            Duration::ZERO
        } else {
            Duration::from_secs(32).max(self.timeout())
        }
    }

    /// Timer I and Timer K: T4 unreliable, zero reliable.
    #[must_use]
    pub fn absorb(&self, reliability: Reliability) -> Duration {
        if reliability.is_reliable() {
            Duration::ZERO
        } else {
            self.t4
        }
    }

    /// Timer J: 64·T1 unreliable, zero reliable.
    #[must_use]
    pub fn timer_j(&self, reliability: Reliability) -> Duration {
        if reliability.is_reliable() {
            Duration::ZERO
        } else {
            self.timeout()
        }
    }

    /// The next retransmission interval, doubling without a ceiling — Timer A.
    #[must_use]
    pub fn double(&self, current: Duration) -> Duration {
        current.saturating_mul(2)
    }

    /// The next retransmission interval, doubling but capped at T2 — Timers E and G.
    #[must_use]
    pub fn double_capped(&self, current: Duration) -> Duration {
        current.saturating_mul(2).min(self.t2)
    }

    /// How long a server transaction waits before sending 100 Trying on its own initiative.
    #[must_use]
    pub fn trying_100(&self) -> Duration {
        Duration::from_millis(200)
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

    #[test]
    fn defaults_match_rfc3261_table_4() {
        let t = Timers::default();
        assert_eq!(t.t1, Duration::from_millis(500));
        assert_eq!(t.t2, Duration::from_secs(4));
        assert_eq!(t.t4, Duration::from_secs(5));
        assert_eq!(t.timeout(), Duration::from_secs(32));
    }

    #[test]
    fn backoff_doubles_and_timer_e_stops_at_t2() {
        let t = Timers::default();
        // Timer A doubles without a ceiling: 500ms, 1s, 2s, 4s, 8s…
        assert_eq!(t.double(t.t1), Duration::from_secs(1));
        assert_eq!(t.double(Duration::from_secs(4)), Duration::from_secs(8));
        // Timer E is capped at T2.
        assert_eq!(t.double_capped(Duration::from_secs(4)), t.t2);
        assert_eq!(t.double_capped(Duration::from_secs(8)), t.t2);
    }

    #[test]
    fn reliable_transports_collapse_the_absorption_timers() {
        let t = Timers::default();
        assert_eq!(t.absorb(Reliability::Reliable), Duration::ZERO);
        assert_eq!(t.timer_j(Reliability::Reliable), Duration::ZERO);
        assert_eq!(t.timer_d(Reliability::Reliable), Duration::ZERO);

        assert_eq!(t.absorb(Reliability::Unreliable), t.t4);
        assert_eq!(t.timer_j(Reliability::Unreliable), t.timeout());
        assert_eq!(t.timer_d(Reliability::Unreliable), Duration::from_secs(32));
    }

    /// With a large T1, Timer D must still outlast the transaction, so it is the larger of
    /// 32 s and 64·T1 rather than a flat 32 s.
    #[test]
    fn timer_d_outlasts_the_transaction_even_with_a_large_t1() {
        let t = Timers {
            t1: Duration::from_secs(2),
            ..Timers::default()
        };
        assert_eq!(t.timeout(), Duration::from_secs(128));
        assert_eq!(t.timer_d(Reliability::Unreliable), Duration::from_secs(128));
    }
}
