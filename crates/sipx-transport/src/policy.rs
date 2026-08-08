//! Bounded endpoint observation and the two narrow policy seams.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use sipx_sip::{Header, Message, Request};
use tokio::sync::mpsc;

use crate::counters::Meters;
use crate::{ConnectionKey, Target, TransportKind};

/// One exact IP network used by live source admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SourcePrefix {
    network: IpAddr,
    bits: u8,
}

impl SourcePrefix {
    /// Build a prefix, rejecting a width outside its address family.
    #[must_use]
    pub fn new(network: IpAddr, bits: u8) -> Option<Self> {
        let max = if network.is_ipv4() { 32 } else { 128 };
        (bits <= max).then_some(Self { network, bits })
    }

    /// One address, expressed as its narrowest prefix.
    #[must_use]
    pub const fn address(address: IpAddr) -> Self {
        let bits = if address.is_ipv4() { 32 } else { 128 };
        Self {
            network: address,
            bits,
        }
    }

    /// Whether `candidate` belongs to this network.
    #[must_use]
    pub fn contains(self, candidate: IpAddr) -> bool {
        match (self.network, candidate) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => prefix_matches(
                u32::from(network).into(),
                u32::from(candidate).into(),
                self.bits,
                32,
            ),
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                prefix_matches(u128::from(network), u128::from(candidate), self.bits, 128)
            }
            _ => false,
        }
    }
}

fn prefix_matches(network: u128, candidate: u128, bits: u8, width: u8) -> bool {
    if bits == 0 {
        return true;
    }
    let shift = u32::from(width.saturating_sub(bits));
    (network >> shift) == (candidate >> shift)
}

#[derive(Debug, Clone)]
struct AdmissionGeneration {
    number: u64,
    prefixes: Option<Arc<[SourcePrefix]>>,
}

/// Atomic publication point for source-admission generations.
#[derive(Debug)]
pub(crate) struct SourceAdmission {
    current: Mutex<AdmissionGeneration>,
    limit: usize,
}

impl Default for SourceAdmission {
    fn default() -> Self {
        Self::new(1024)
    }
}

impl SourceAdmission {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            current: Mutex::new(AdmissionGeneration {
                number: 0,
                prefixes: None,
            }),
            limit,
        }
    }
    /// Admit an address and return the immutable generation that made the decision.
    pub(crate) fn admit(&self, address: IpAddr) -> Option<u64> {
        let current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let allowed = current
            .prefixes
            .as_ref()
            .is_none_or(|prefixes| prefixes.iter().any(|prefix| prefix.contains(address)));
        allowed.then_some(current.number)
    }

    pub(crate) fn replace(&self, prefixes: Vec<SourcePrefix>) -> crate::Result<u64> {
        if prefixes.len() > self.limit {
            return Err(crate::Error::SourceAdmissionCapacity {
                max: self.limit,
                attempted: prefixes.len(),
            });
        }
        Ok(self.publish(Some(prefixes.into())))
    }

    pub(crate) fn clear(&self) -> u64 {
        self.publish(None)
    }

    fn publish(&self, prefixes: Option<Arc<[SourcePrefix]>>) -> u64 {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        current.number = current.number.wrapping_add(1).max(1);
        current.prefixes = prefixes;
        current.number
    }
}

/// What a pre-transaction request policy decided.
#[derive(Debug)]
pub enum RequestPolicyDecision {
    /// Continue without adding fields.
    Allow,
    /// Refuse the send before a transaction exists.
    Reject(String),
    /// Continue after appending application-owned headers.
    AddHeaders(Vec<Header>),
}

/// An immutable pre-transaction request policy.
pub trait RequestPolicy: Send + Sync {
    /// Decide one send. The request and target cannot be mutated or replaced.
    fn decide(&self, request: &Request, target: &Target) -> RequestPolicyDecision;
}

/// Cloneable configured request policy.
#[derive(Clone)]
pub struct RequestPolicyRef(Arc<dyn RequestPolicy>);

impl RequestPolicyRef {
    /// Wrap a request policy for [`crate::Config`].
    #[must_use]
    pub fn new(policy: impl RequestPolicy + 'static) -> Self {
        Self(Arc::new(policy))
    }

    pub(crate) fn decide(&self, request: &Request, target: &Target) -> RequestPolicyDecision {
        self.0.decide(request, target)
    }
}

impl std::fmt::Debug for RequestPolicyRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RequestPolicyRef(..)")
    }
}

/// Which side of the endpoint boundary a message crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    /// Parsed network input.
    Inbound,
    /// Finalized network output.
    Outbound,
}

/// How the transaction layer classified an observed message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionClass {
    /// A new server transaction.
    ServerCreated,
    /// A message matched an existing transaction.
    Matched,
    /// No transaction matched.
    Unmatched,
    /// A new client transaction.
    ClientCreated,
    /// A request deliberately sent without a transaction.
    Direct,
}

/// A parsed inbound or finalized outbound SIP message.
#[derive(Debug, Clone)]
pub struct MessageObservation {
    /// Immutable message snapshot.
    pub message: Message,
    /// Endpoint address.
    pub local: std::net::SocketAddr,
    /// Remote address.
    pub peer: std::net::SocketAddr,
    /// Wire transport.
    pub transport: TransportKind,
    /// Boundary direction.
    pub direction: MessageDirection,
    /// Transaction-layer classification.
    pub transaction: TransactionClass,
}

/// Stable identity of one pooled connection incarnation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConnectionId {
    /// Pool key, including verified identity and WebSocket resource when present.
    pub key: ConnectionKey,
    /// Pool incarnation; replacement receives another value.
    pub generation: u64,
    /// Source-admission generation retained by an inbound connection.
    pub admission_generation: Option<u64>,
}

/// A connection lifecycle transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// An inbound socket was accepted into the pool.
    Accepted,
    /// An outbound or inbound stream became usable.
    Opened,
    /// A secure transport authenticated its peer.
    Authenticated,
    /// The connection joined the bounded pool.
    Pooled,
    /// Existing pooled transport carried another send.
    Reused,
    /// Opening, authentication or framing failed.
    Failed,
    /// The incarnation closed.
    Closed,
}

/// One connection transition.
#[derive(Debug, Clone)]
pub struct ConnectionObservation {
    /// Stable connection incarnation.
    pub connection: ConnectionId,
    /// Transition.
    pub state: ConnectionState,
}

/// One item on the bounded endpoint observation stream.
#[derive(Debug, Clone)]
pub enum EndpointObservation {
    /// A parsed or finalized SIP message.
    Message(Box<MessageObservation>),
    /// A connection lifecycle transition.
    Connection(ConnectionObservation),
}

/// Shared non-blocking fan-in for every endpoint task.
#[derive(Debug)]
pub(crate) struct ObservationHub {
    sink: Mutex<Option<mpsc::Sender<EndpointObservation>>>,
    meters: Arc<Meters>,
}

impl ObservationHub {
    pub(crate) fn new(meters: Arc<Meters>) -> Self {
        Self {
            sink: Mutex::new(None),
            meters,
        }
    }

    pub(crate) fn subscribe(&self, capacity: usize) -> mpsc::Receiver<EndpointObservation> {
        let (sender, receiver) = mpsc::channel(capacity.max(1));
        *self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        receiver
    }

    pub(crate) fn emit(&self, event: EndpointObservation) {
        let mut sink = self
            .sink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(sender) = sink.as_ref() else {
            return;
        };
        match sender.try_send(event) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => self.meters.observation_drop(),
            Err(mpsc::error::TrySendError::Closed(_)) => *sink = None,
        }
    }
}

pub(crate) fn connection_event(
    key: ConnectionKey,
    generation: u64,
    admission_generation: Option<u64>,
    state: ConnectionState,
) -> EndpointObservation {
    EndpointObservation::Connection(ConnectionObservation {
        connection: ConnectionId {
            key,
            generation,
            admission_generation,
        },
        state,
    })
}

/// Resolve deliberate `Other` construction before deciding whether policy may append the field.
pub(crate) fn policy_header(name: &sipx_sip::HeaderName) -> (sipx_sip::HeaderName, bool) {
    use sipx_sip::HeaderName;
    let semantic = HeaderName::parse(&bytes::Bytes::copy_from_slice(name.canonical()));
    let allowed = matches!(
        semantic,
        HeaderName::AlertInfo
            | HeaderName::CallInfo
            | HeaderName::Organization
            | HeaderName::Priority
            | HeaderName::Subject
            | HeaderName::UserAgent
            | HeaderName::Other(_)
    );
    (semantic, allowed)
}

pub(crate) fn duplicate_policy_header(request: &Request, semantic: &sipx_sip::HeaderName) -> bool {
    !matches!(semantic, sipx_sip::HeaderName::Other(_)) && request.headers.get(semantic).is_some()
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
    use bytes::Bytes;
    use sipx_sip::HeaderName;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn prefixes_match_only_their_network_and_family() {
        let v4 = SourcePrefix::new(Ipv4Addr::new(192, 0, 2, 0).into(), 24).unwrap();
        assert!(v4.contains(Ipv4Addr::new(192, 0, 2, 42).into()));
        assert!(!v4.contains(Ipv4Addr::new(192, 0, 3, 1).into()));
        assert!(!v4.contains(Ipv6Addr::LOCALHOST.into()));
    }

    #[test]
    fn replacement_publishes_complete_generations() {
        let admission = SourceAdmission::default();
        assert_eq!(admission.admit(IpAddr::V4(Ipv4Addr::LOCALHOST)), Some(0));
        let one = admission
            .replace(vec![SourcePrefix::address(IpAddr::V4(Ipv4Addr::LOCALHOST))])
            .unwrap();
        assert_eq!(admission.admit(IpAddr::V4(Ipv4Addr::LOCALHOST)), Some(one));
        assert_eq!(admission.admit(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), None);
        let two = admission.clear();
        assert!(two > one);
        assert_eq!(
            admission.admit(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            Some(two)
        );
    }

    #[test]
    fn oversized_replacement_preserves_the_old_generation() {
        let admission = SourceAdmission::new(1);
        let first = admission
            .replace(vec![SourcePrefix::address(IpAddr::V4(Ipv4Addr::LOCALHOST))])
            .unwrap();
        let error = admission
            .replace(vec![
                SourcePrefix::address(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                SourcePrefix::address(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            ])
            .unwrap_err();
        assert!(matches!(
            error,
            crate::Error::SourceAdmissionCapacity { .. }
        ));
        assert_eq!(
            admission.admit(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Some(first)
        );
        assert_eq!(admission.admit(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), None);
    }

    #[test]
    fn request_policy_allows_only_application_fields_and_unknown_extensions() {
        for name in [HeaderName::Subject, HeaderName::Organization] {
            assert!(policy_header(&name).1);
        }
        assert!(policy_header(&HeaderName::Other(Bytes::from_static(b"X-Trace"))).1);
        for name in [
            HeaderName::Contact,
            HeaderName::ContentType,
            HeaderName::Event,
        ] {
            assert!(!policy_header(&name).1);
        }
        for raw in [b"vIa".as_slice(), b"v".as_slice()] {
            let (semantic, allowed) =
                policy_header(&HeaderName::Other(Bytes::copy_from_slice(raw)));
            assert_eq!(semantic, HeaderName::Via);
            assert!(!allowed);
        }
    }
}
