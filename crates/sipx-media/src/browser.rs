//! The security gate for one browser-audio ICE component.
//!
//! This module is the I/O-free boundary from `docs/specs/webrtc-audio.md` §7. The live socket
//! owner consults it before handing bytes to ICE, DTLS, SRTP or SRTCP; keeping it free of sockets
//! makes every hostile-input and ordering branch deterministic in a test.

use std::net::SocketAddr;

use std::future::Future;
#[cfg(feature = "dtls")]
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
#[cfg(feature = "dtls")]
use std::time::Duration;

use sipx_sdp::ice::CandidateType;
#[cfg(feature = "dtls")]
use tokio::net::UdpSocket;
#[cfg(feature = "dtls")]
use tokio::sync::{Mutex, mpsc};

#[cfg(all(test, feature = "dtls"))]
use std::sync::atomic::AtomicBool;
#[cfg(all(test, feature = "dtls"))]
use tokio::sync::Notify;

#[cfg(all(test, feature = "dtls"))]
static ACTIVE_SUPERVISORS: AtomicUsize = AtomicUsize::new(0);
#[cfg(all(test, feature = "dtls"))]
static DTLS_HANDSHAKING: AtomicBool = AtomicBool::new(false);
#[cfg(all(test, feature = "dtls"))]
static SUPERVISOR_CHANGED: Notify = Notify::const_new();
#[cfg(all(test, feature = "dtls"))]
static HANDSHAKE_STARTED: Notify = Notify::const_new();

/// Largest inbound datagram the browser-audio component admits to a protocol parser.
pub const MAX_DATAGRAM: usize = 2048;

/// Maximum browser-profile tasks alive at one instant (spec §7.2).
pub const MAX_PROFILE_TASKS: usize = 6;

/// Per-component accounting tied to the lifetime of every profile-owned task.
#[derive(Debug, Default)]
pub(crate) struct ProfileTasks {
    active: AtomicUsize,
    peak: AtomicUsize,
}

impl ProfileTasks {
    fn enter(self: &Arc<Self>) -> ProfileTaskPermit {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        ProfileTaskPermit {
            tasks: Arc::clone(self),
        }
    }

    #[cfg(all(test, feature = "dtls"))]
    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.active.load(Ordering::SeqCst),
            self.peak.load(Ordering::SeqCst),
        )
    }
}

struct ProfileTaskPermit {
    tasks: Arc<ProfileTasks>,
}

impl Drop for ProfileTaskPermit {
    fn drop(&mut self) {
        self.tasks.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) async fn profile_task<F: Future>(tasks: Arc<ProfileTasks>, future: F) -> F::Output {
    let _permit = tasks.enter();
    future.await
}

/// One protocol carried by the nominated component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressClass {
    /// An ICE/STUN datagram.
    Stun,
    /// A DTLS record.
    Dtls,
    /// A protected RTP packet.
    Srtp,
    /// A protected RTCP packet.
    Srtcp,
}

/// Why a datagram changed no browser-component protocol state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IngressDrop {
    /// The UDP payload was empty.
    Empty,
    /// An RTP/RTCP first byte arrived without the second byte needed by RFC 5761.
    TruncatedClassPrefix,
    /// The payload exceeded [`MAX_DATAGRAM`].
    Oversized,
    /// RFC 5764 assigns no protocol to the first-byte range.
    UnknownProtocol,
    /// Protected traffic or DTLS arrived before ICE selected a pair.
    BeforeNomination,
    /// Traffic came from an address other than the selected remote candidate.
    WrongPeer,
    /// Protected media arrived before all directional contexts were installed.
    KeysUnavailable,
    /// A new DTLS record arrived after the handshake boundary closed.
    UnexpectedDtls,
    /// The component no longer admits traffic.
    Closed,
}

/// Classify one bounded datagram by RFC 5764 §5.1.2 and RFC 5761 §4.
///
/// No protocol parser is attempted here. Length is checked before either classifier byte is read.
pub fn classify_datagram(datagram: &[u8]) -> Result<IngressClass, IngressDrop> {
    if datagram.len() > MAX_DATAGRAM {
        return Err(IngressDrop::Oversized);
    }
    let first = *datagram.first().ok_or(IngressDrop::Empty)?;
    match first {
        0..=1 => Ok(IngressClass::Stun),
        20..=63 => Ok(IngressClass::Dtls),
        128..=191 => {
            let second = *datagram.get(1).ok_or(IngressDrop::TruncatedClassPrefix)?;
            if (192..=223).contains(&second) {
                Ok(IngressClass::Srtcp)
            } else {
                Ok(IngressClass::Srtp)
            }
        }
        _ => Err(IngressDrop::UnknownProtocol),
    }
}

/// The exact ICE pair allowed to carry DTLS and protected media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedComponent {
    /// Bound local candidate address.
    pub local: SocketAddr,
    /// Nominated remote candidate address.
    pub remote: SocketAddr,
    /// ICE generation whose credentials authenticated the pair.
    pub ice_generation: u64,
    /// How the local candidate was obtained.
    pub local_kind: CandidateType,
    /// How the remote candidate was obtained.
    pub remote_kind: CandidateType,
}

impl SelectedComponent {
    /// A host-to-host selected pair.
    #[must_use]
    pub const fn new(local: SocketAddr, remote: SocketAddr, ice_generation: u64) -> Self {
        Self {
            local,
            remote,
            ice_generation,
            local_kind: CandidateType::Host,
            remote_kind: CandidateType::Host,
        }
    }

    /// Record the candidate types the ICE agent selected.
    #[must_use]
    pub const fn with_candidate_types(
        mut self,
        local_kind: CandidateType,
        remote_kind: CandidateType,
    ) -> Self {
        self.local_kind = local_kind;
        self.remote_kind = remote_kind;
        self
    }
}

/// Security phase of one component generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentState {
    /// ICE is checking and no media peer exists yet.
    IceChecking,
    /// ICE selected the immutable pair; DTLS may start.
    Nominated,
    /// DTLS records from the selected peer are admitted.
    DtlsHandshaking,
    /// All directional SRTP/SRTCP key material is installed, but delivery is not enabled.
    KeysInstalled,
    /// Protected RTP and RTCP are admitted.
    Running,
    /// Admissions and key material are closed.
    Closed,
}

/// Exact, monotonic drops decided by [`ComponentIngress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IngressCounts {
    /// Empty payloads.
    pub ingress_empty: u64,
    /// One-byte RTP/RTCP prefixes.
    pub ingress_truncated_prefix: u64,
    /// Oversized payloads.
    pub ingress_oversized: u64,
    /// Unassigned first-byte ranges.
    pub ingress_unknown_protocol: u64,
    /// DTLS before nomination.
    pub dtls_before_nomination: u64,
    /// SRTP before nomination.
    pub srtp_before_nomination: u64,
    /// SRTCP before nomination.
    pub srtcp_before_nomination: u64,
    /// DTLS from a non-nominated peer.
    pub dtls_wrong_peer: u64,
    /// SRTP from a non-nominated peer.
    pub srtp_wrong_peer: u64,
    /// SRTCP from a non-nominated peer.
    pub srtcp_wrong_peer: u64,
    /// SRTP received before key installation and media admission.
    pub srtp_keys_unavailable: u64,
    /// SRTCP received before key installation and media admission.
    pub srtcp_keys_unavailable: u64,
    /// DTLS records received after its admission phase.
    pub dtls_unexpected_records: u64,
    /// Classified traffic received after closure.
    pub ingress_closed: u64,
    /// STUN datagrams too short or malformed for ICE.
    pub stun_malformed: u64,
    /// DTLS records too short or malformed for the handshake.
    pub dtls_malformed: u64,
    /// SRTP packets too short or malformed for RTP.
    pub srtp_malformed: u64,
    /// SRTCP packets too short or malformed for RTCP.
    pub srtcp_malformed: u64,
    /// STUN handoffs refused by their bounded queue.
    pub stun_queue_refusals: u64,
    /// DTLS handoffs refused by their bounded queue.
    pub dtls_queue_refusals: u64,
    /// SRTP handoffs refused by their bounded queue.
    pub srtp_queue_refusals: u64,
    /// SRTCP handoffs refused by their bounded queue.
    pub srtcp_queue_refusals: u64,
    /// SRTP authentication failures excluding replay.
    pub srtp_authentication_failures: u64,
    /// SRTCP authentication failures excluding replay.
    pub srtcp_authentication_failures: u64,
    /// STUN messages whose short-term credential did not verify.
    pub stun_authentication_failures: u64,
    /// Replayed or too-old SRTP packets.
    pub srtp_replays: u64,
    /// Replayed or too-old SRTCP packets.
    pub srtcp_replays: u64,
    /// Authenticated RTCP compound packets applied by the media worker.
    pub srtcp_processed: u64,
}

impl IngressCounts {
    /// Sum every count, saturating rather than wrapping to a plausible low value.
    #[must_use]
    pub fn total(self) -> u64 {
        [
            self.ingress_empty,
            self.ingress_truncated_prefix,
            self.ingress_oversized,
            self.ingress_unknown_protocol,
            self.dtls_before_nomination,
            self.srtp_before_nomination,
            self.srtcp_before_nomination,
            self.dtls_wrong_peer,
            self.srtp_wrong_peer,
            self.srtcp_wrong_peer,
            self.srtp_keys_unavailable,
            self.srtcp_keys_unavailable,
            self.dtls_unexpected_records,
            self.ingress_closed,
            self.stun_malformed,
            self.dtls_malformed,
            self.srtp_malformed,
            self.srtcp_malformed,
            self.stun_queue_refusals,
            self.dtls_queue_refusals,
            self.srtp_queue_refusals,
            self.srtcp_queue_refusals,
            self.srtp_authentication_failures,
            self.srtcp_authentication_failures,
            self.stun_authentication_failures,
            self.srtp_replays,
            self.srtcp_replays,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

/// Result of applying both classification and the component security state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressDisposition {
    /// The named protocol may consume this datagram now.
    Accepted(IngressClass),
    /// The datagram was accounted for and must be released.
    Dropped(IngressDrop),
}

/// Read-only facts for diagnostics and the independent browser proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrowserComponentSnapshot {
    /// Current security phase.
    pub state: ComponentState,
    /// Selected pair, once ICE has nominated one.
    pub selected: Option<SelectedComponent>,
    /// Exact gate drops so far.
    pub counts: IngressCounts,
}

/// A rejected component transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ComponentError {
    /// ICE selected a pair from a generation this gate does not own.
    #[error("ICE nomination generation {got} does not match current generation {expected}")]
    Generation {
        /// Current generation.
        expected: u64,
        /// Generation attached to the nomination.
        got: u64,
    },
    /// An operation was attempted outside its one permitted phase.
    #[error("component operation {operation} is invalid in state {state:?}")]
    State {
        /// Operation being attempted.
        operation: &'static str,
        /// Current state.
        state: ComponentState,
    },
    /// DTLS was asked to start for a source other than the selected peer.
    #[error("DTLS peer {got} is not nominated peer {expected}")]
    WrongPeer {
        /// Selected peer.
        expected: SocketAddr,
        /// Requested peer.
        got: SocketAddr,
    },
}

/// I/O-free ingress and key-installation gate for one browser component.
#[derive(Debug)]
pub struct ComponentIngress {
    generation: u64,
    state: ComponentState,
    selected: Option<SelectedComponent>,
    keys: Option<crate::SrtpKeys>,
    counts: IngressCounts,
}

impl ComponentIngress {
    /// Begin checking one ICE generation.
    #[must_use]
    pub const fn new(generation: u64) -> Self {
        Self {
            generation,
            state: ComponentState::IceChecking,
            selected: None,
            keys: None,
            counts: IngressCounts {
                ingress_empty: 0,
                ingress_truncated_prefix: 0,
                ingress_oversized: 0,
                ingress_unknown_protocol: 0,
                dtls_before_nomination: 0,
                srtp_before_nomination: 0,
                srtcp_before_nomination: 0,
                dtls_wrong_peer: 0,
                srtp_wrong_peer: 0,
                srtcp_wrong_peer: 0,
                srtp_keys_unavailable: 0,
                srtcp_keys_unavailable: 0,
                dtls_unexpected_records: 0,
                ingress_closed: 0,
                stun_malformed: 0,
                dtls_malformed: 0,
                srtp_malformed: 0,
                srtcp_malformed: 0,
                stun_queue_refusals: 0,
                dtls_queue_refusals: 0,
                srtp_queue_refusals: 0,
                srtcp_queue_refusals: 0,
                srtp_authentication_failures: 0,
                srtcp_authentication_failures: 0,
                stun_authentication_failures: 0,
                srtp_replays: 0,
                srtcp_replays: 0,
                srtcp_processed: 0,
            },
        }
    }

    /// Freeze the pair selected by ICE for this generation.
    pub fn nominate(&mut self, selected: SelectedComponent) -> Result<(), ComponentError> {
        if selected.ice_generation != self.generation {
            return Err(ComponentError::Generation {
                expected: self.generation,
                got: selected.ice_generation,
            });
        }
        if self.state != ComponentState::IceChecking {
            return Err(ComponentError::State {
                operation: "nominate",
                state: self.state,
            });
        }
        self.selected = Some(selected);
        self.state = ComponentState::Nominated;
        Ok(())
    }

    /// Admit DTLS records from the nominated peer.
    pub fn begin_dtls(&mut self, peer: SocketAddr) -> Result<(), ComponentError> {
        if self.state != ComponentState::Nominated {
            return Err(ComponentError::State {
                operation: "begin_dtls",
                state: self.state,
            });
        }
        let expected =
            self.selected
                .map(|selected| selected.remote)
                .ok_or(ComponentError::State {
                    operation: "begin_dtls",
                    state: self.state,
                })?;
        if peer != expected {
            return Err(ComponentError::WrongPeer {
                expected,
                got: peer,
            });
        }
        self.state = ComponentState::DtlsHandshaking;
        Ok(())
    }

    /// Atomically install both directions from a fingerprint-verified DTLS result.
    pub fn install_verified_keys(
        &mut self,
        keys: crate::dtls::VerifiedKeys,
    ) -> Result<(), ComponentError> {
        if self.state != ComponentState::DtlsHandshaking {
            return Err(ComponentError::State {
                operation: "install_verified_keys",
                state: self.state,
            });
        }
        self.keys = Some(keys.into_srtp_keys());
        self.state = ComponentState::KeysInstalled;
        Ok(())
    }

    /// Enable protected delivery and move the directional master material into the media session.
    pub fn start_media(&mut self) -> Result<crate::SrtpKeys, ComponentError> {
        if self.state != ComponentState::KeysInstalled {
            return Err(ComponentError::State {
                operation: "start_media",
                state: self.state,
            });
        }
        let keys = self.keys.take().ok_or(ComponentError::State {
            operation: "start_media",
            state: self.state,
        })?;
        self.state = ComponentState::Running;
        Ok(keys)
    }

    /// Classify and apply the nominated-peer/key-state boundary, counting one refusal on failure.
    pub fn admit(&mut self, source: SocketAddr, datagram: &[u8]) -> IngressDisposition {
        let class = match classify_datagram(datagram) {
            Ok(class) => class,
            Err(reason) => {
                self.note_drop(None, reason);
                return IngressDisposition::Dropped(reason);
            }
        };
        let reason = if self.state == ComponentState::Closed {
            Some(IngressDrop::Closed)
        } else if class == IngressClass::Stun {
            None
        } else if self.selected.is_none() {
            Some(IngressDrop::BeforeNomination)
        } else if self
            .selected
            .is_some_and(|selected| selected.remote != source)
        {
            Some(IngressDrop::WrongPeer)
        } else {
            match class {
                IngressClass::Stun => None,
                IngressClass::Dtls => (!matches!(
                    self.state,
                    ComponentState::Nominated | ComponentState::DtlsHandshaking
                ))
                .then_some(IngressDrop::UnexpectedDtls),
                IngressClass::Srtp | IngressClass::Srtcp => {
                    (self.state != ComponentState::Running).then_some(IngressDrop::KeysUnavailable)
                }
            }
        };
        if let Some(reason) = reason {
            self.note_drop(Some(class), reason);
            IngressDisposition::Dropped(reason)
        } else {
            IngressDisposition::Accepted(class)
        }
    }

    /// Refuse future admissions and erase any key material not moved into a session.
    pub fn close(&mut self) {
        self.keys = None;
        self.state = ComponentState::Closed;
    }

    /// Current low-cardinality diagnostic facts. No key material is exposed.
    #[must_use]
    pub const fn snapshot(&self) -> BrowserComponentSnapshot {
        BrowserComponentSnapshot {
            state: self.state,
            selected: self.selected,
            counts: self.counts,
        }
    }

    pub(crate) fn note_malformed(&mut self, class: IngressClass) {
        let counter = match class {
            IngressClass::Stun => &mut self.counts.stun_malformed,
            IngressClass::Dtls => &mut self.counts.dtls_malformed,
            IngressClass::Srtp => &mut self.counts.srtp_malformed,
            IngressClass::Srtcp => &mut self.counts.srtcp_malformed,
        };
        *counter = counter.saturating_add(1);
    }

    #[cfg(feature = "dtls")]
    pub(crate) fn note_queue_full(&mut self, class: IngressClass) {
        let counter = match class {
            IngressClass::Stun => &mut self.counts.stun_queue_refusals,
            IngressClass::Dtls => &mut self.counts.dtls_queue_refusals,
            IngressClass::Srtp => &mut self.counts.srtp_queue_refusals,
            IngressClass::Srtcp => &mut self.counts.srtcp_queue_refusals,
        };
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn note_authentication_failure(&mut self, class: IngressClass) {
        let counter = match class {
            IngressClass::Stun => &mut self.counts.stun_authentication_failures,
            IngressClass::Srtp => &mut self.counts.srtp_authentication_failures,
            IngressClass::Srtcp => &mut self.counts.srtcp_authentication_failures,
            IngressClass::Dtls => return,
        };
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn note_replay(&mut self, class: IngressClass) {
        let counter = match class {
            IngressClass::Srtp => &mut self.counts.srtp_replays,
            IngressClass::Srtcp => &mut self.counts.srtcp_replays,
            IngressClass::Stun | IngressClass::Dtls => return,
        };
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn note_srtcp_processed(&mut self) {
        self.counts.srtcp_processed = self.counts.srtcp_processed.saturating_add(1);
    }

    fn note_drop(&mut self, class: Option<IngressClass>, reason: IngressDrop) {
        let counter = match (class, reason) {
            (_, IngressDrop::Empty) => &mut self.counts.ingress_empty,
            (_, IngressDrop::TruncatedClassPrefix) => &mut self.counts.ingress_truncated_prefix,
            (_, IngressDrop::Oversized) => &mut self.counts.ingress_oversized,
            (_, IngressDrop::UnknownProtocol) => &mut self.counts.ingress_unknown_protocol,
            (Some(IngressClass::Dtls), IngressDrop::BeforeNomination) => {
                &mut self.counts.dtls_before_nomination
            }
            (Some(IngressClass::Srtp), IngressDrop::BeforeNomination) => {
                &mut self.counts.srtp_before_nomination
            }
            (Some(IngressClass::Srtcp), IngressDrop::BeforeNomination) => {
                &mut self.counts.srtcp_before_nomination
            }
            (Some(IngressClass::Dtls), IngressDrop::WrongPeer) => &mut self.counts.dtls_wrong_peer,
            (Some(IngressClass::Srtp), IngressDrop::WrongPeer) => &mut self.counts.srtp_wrong_peer,
            (Some(IngressClass::Srtcp), IngressDrop::WrongPeer) => {
                &mut self.counts.srtcp_wrong_peer
            }
            (Some(IngressClass::Srtp), IngressDrop::KeysUnavailable) => {
                &mut self.counts.srtp_keys_unavailable
            }
            (Some(IngressClass::Srtcp), IngressDrop::KeysUnavailable) => {
                &mut self.counts.srtcp_keys_unavailable
            }
            (Some(IngressClass::Dtls), IngressDrop::UnexpectedDtls) => {
                &mut self.counts.dtls_unexpected_records
            }
            (_, IngressDrop::Closed) => &mut self.counts.ingress_closed,
            // STUN is accepted by this gate; malformed/authentication and queue outcomes belong
            // to its parser/adapter. All other combinations are structurally unreachable.
            _ => return,
        };
        *counter = counter.saturating_add(1);
    }
}

/// Why the browser component could not advance from ICE through verified DTLS keying.
#[cfg(feature = "dtls")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BrowserStartError {
    /// Codec, pacing or mux configuration was invalid before the owner started.
    #[error("media setup: {0}")]
    Setup(#[from] crate::SetupError),
    /// The named browser path was started without negotiated RTP/RTCP multiplexing.
    #[error("browser audio requires RTP/RTCP multiplexing")]
    RtcpMuxRequired,
    /// ICE concluded component 1 without selecting a pair.
    #[error("ICE failed before nominating the browser-audio component")]
    IceFailed,
    /// ICE stopped before reporting selection or failure.
    #[error("ICE stopped before nominating the browser-audio component")]
    IceStopped,
    /// The bounded DTLS handshake did not complete.
    #[error("DTLS handshake exceeded its configured deadline")]
    DtlsTimeout,
    /// The component security gate rejected an internal transition.
    #[error("browser component transition: {0}")]
    Component(#[from] ComponentError),
    /// The blocking DTLS worker did not complete normally.
    #[error("DTLS worker: {0}")]
    Worker(String),
    /// DTLS, fingerprint verification or key derivation failed.
    #[error("DTLS: {0}")]
    Dtls(#[from] crate::dtls::Error),
    /// The DTLS adapter could not be constructed.
    #[error("DTLS adapter: {0}")]
    Adapter(String),
}

#[cfg(feature = "dtls")]
#[derive(Debug)]
pub(crate) struct Datagram {
    pub(crate) source: SocketAddr,
    pub(crate) bytes: Vec<u8>,
}

/// Receivers attached to the ordinary RTP/SRTCP workers after key installation.
#[cfg(feature = "dtls")]
#[derive(Debug)]
pub(crate) struct MediaIngress {
    pub(crate) srtp: mpsc::Receiver<Datagram>,
    pub(crate) srtcp: mpsc::Receiver<Datagram>,
}

/// The live, still-bound component and its one receive owner.
#[cfg(feature = "dtls")]
#[derive(Debug)]
pub(crate) struct Runtime {
    pub(crate) socket: Arc<UdpSocket>,
    pub(crate) media: MediaIngress,
    pub(crate) ice: crate::ice::driver::Handle,
    pub(crate) ingress: Arc<StdMutex<ComponentIngress>>,
    pub(crate) owner: tokio::task::JoinHandle<()>,
    pub(crate) ice_owner: tokio::task::JoinHandle<()>,
    pub(crate) stop: Arc<crate::session::Stop>,
    pub(crate) profile_tasks: Arc<ProfileTasks>,
}

#[cfg(feature = "dtls")]
const DTLS_QUEUE: usize = 64;
#[cfg(feature = "dtls")]
const SRTP_QUEUE: usize = 64;
#[cfg(feature = "dtls")]
const SRTCP_QUEUE: usize = 32;
#[cfg(feature = "dtls")]
const OUTBOUND_QUEUE: usize = 64;
#[cfg(feature = "dtls")]
const MAX_OUTBOUND_DATAGRAM: usize = 1200;

/// Start the sole receiver, await exact ICE nomination, run DTLS through it and install keys.
#[cfg(feature = "dtls")]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn prepare(
    socket: Arc<UdpSocket>,
    local: crate::ice::LocalDescription,
    ice_generation: u64,
    identity: crate::dtls::openssl::Identity,
    role: crate::dtls::Role,
    fingerprint: sipx_sdp::fingerprint::Fingerprint,
    timeout: Duration,
    stop: Arc<crate::session::Stop>,
    discards: Arc<crate::counters::DiscardMeters>,
) -> Result<(Runtime, crate::SrtpKeys), BrowserStartError> {
    let (finished, result) = tokio::sync::oneshot::channel();
    let profile_tasks = Arc::new(ProfileTasks::default());
    let mut cancellation = StopOnDrop {
        stop: Arc::clone(&stop),
        armed: true,
    };
    tokio::spawn(async move {
        let task_permit = profile_tasks.enter();
        #[cfg(all(test, feature = "dtls"))]
        let _activity = SupervisorActivity::new();
        let outcome = prepare_inner(
            socket,
            local,
            ice_generation,
            identity,
            role,
            fingerprint,
            timeout,
            stop,
            discards,
            Arc::clone(&profile_tasks),
        )
        .await;
        // The preparation supervisor is not a running-session task. End its counted lifetime
        // before waking the caller that will attach the four media workers.
        drop(task_permit);
        if let Err(returned) = finished.send(outcome)
            && let Ok((runtime, _keys)) = returned
        {
            cleanup_runtime(runtime).await;
        }
    });
    let outcome = result.await.map_err(|_| {
        BrowserStartError::Worker(
            "browser-component supervisor stopped without a result".to_owned(),
        )
    })?;
    cancellation.armed = false;
    outcome
}

#[cfg(feature = "dtls")]
struct StopOnDrop {
    stop: Arc<crate::session::Stop>,
    armed: bool,
}

#[cfg(feature = "dtls")]
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.stop.stop();
        }
    }
}

#[cfg(feature = "dtls")]
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn prepare_inner(
    socket: Arc<UdpSocket>,
    local: crate::ice::LocalDescription,
    ice_generation: u64,
    identity: crate::dtls::openssl::Identity,
    role: crate::dtls::Role,
    fingerprint: sipx_sdp::fingerprint::Fingerprint,
    timeout: Duration,
    stop: Arc<crate::session::Stop>,
    discards: Arc<crate::counters::DiscardMeters>,
    profile_tasks: Arc<ProfileTasks>,
) -> Result<(Runtime, crate::SrtpKeys), BrowserStartError> {
    let placeholder = SocketAddr::new(
        socket
            .local_addr()
            .map_err(|error| {
                BrowserStartError::Adapter(format!("component has no local address: {error}"))
            })?
            .ip(),
        0,
    );
    let destinations = crate::ice::driver::Destinations {
        rtp: Arc::new(Mutex::new(placeholder)),
        rtcp: Arc::new(Mutex::new(None)),
    };
    let (agent, pending) = local.into_driver_parts();
    let peering = agent.peering().cloned();
    let crate::ice::driver::OwnedDriver {
        handle: ice,
        task: ice_owner,
    } = crate::ice::driver::spawn_owned(
        agent,
        pending,
        vec![Arc::clone(&socket)],
        destinations,
        Arc::clone(&stop),
        discards,
        Arc::clone(&profile_tasks),
    );

    let ingress = Arc::new(StdMutex::new(ComponentIngress::new(ice_generation)));
    let (dtls_in, dtls_rx) = std::sync::mpsc::sync_channel(DTLS_QUEUE);
    let (dtls_out, dtls_writes) = mpsc::channel(OUTBOUND_QUEUE);
    let (audio_packets, srtp_rx) = mpsc::channel(SRTP_QUEUE);
    let (control_packets, srtcp_rx) = mpsc::channel(SRTCP_QUEUE);
    let owner = tokio::spawn(profile_task(
        Arc::clone(&profile_tasks),
        owner_loop(
            Arc::clone(&socket),
            ice.clone(),
            Arc::clone(&ingress),
            peering,
            dtls_in,
            dtls_writes,
            audio_packets,
            control_packets,
            Arc::clone(&stop),
        ),
    ));
    let mut tasks = PreparingTasks {
        stop: Arc::clone(&stop),
        owner: Some(owner),
        ice_owner: Some(ice_owner),
        dtls: None,
        armed: true,
    };

    let selected = match ice.wait_selected(ice_generation).await {
        Ok(selected) => selected,
        Err(crate::ice::driver::SelectionError::Failed) => {
            tasks.cleanup().await;
            return Err(BrowserStartError::IceFailed);
        }
        Err(crate::ice::driver::SelectionError::Stopped) => {
            tasks.cleanup().await;
            return Err(BrowserStartError::IceStopped);
        }
    };
    let transition = {
        let mut gate = lock_ingress(&ingress);
        gate.nominate(selected)
            .and_then(|()| gate.begin_dtls(selected.remote))
    };
    if let Err(error) = transition {
        tasks.cleanup().await;
        return Err(error.into());
    }
    #[cfg(all(test, feature = "dtls"))]
    {
        DTLS_HANDSHAKING.store(true, Ordering::SeqCst);
        HANDSHAKE_STARTED.notify_waiters();
    }

    let adapter = DtlsAdapter {
        inbound: dtls_rx,
        outbound: dtls_out,
        timeout: timeout.saturating_add(Duration::from_millis(250)),
    };
    let dtls_tasks = Arc::clone(&profile_tasks);
    tasks.dtls = Some(tokio::task::spawn_blocking(move || {
        let _permit = dtls_tasks.enter();
        let mut handshake = crate::dtls::openssl::Session::with_io(adapter, &identity)
            .map_err(|error| crate::dtls::Error::Dtls(error.to_string()))?;
        crate::dtls::establish_verified(&mut handshake, role, Some(&fingerprint))
    }));
    let handshake = if let Some(worker) = tasks.dtls.as_mut() {
        tokio::time::timeout(timeout, worker).await
    } else {
        tasks.cleanup().await;
        return Err(BrowserStartError::Worker(
            "DTLS worker was not retained".to_owned(),
        ));
    };
    let verified = match handshake {
        Err(_elapsed) => {
            tasks.cleanup().await;
            return Err(BrowserStartError::DtlsTimeout);
        }
        Ok(Err(error)) => {
            tasks.dtls.take();
            tasks.cleanup().await;
            return Err(BrowserStartError::Worker(error.to_string()));
        }
        Ok(Ok(Err(error))) => {
            tasks.dtls.take();
            tasks.cleanup().await;
            return Err(error.into());
        }
        Ok(Ok(Ok(verified))) => {
            tasks.dtls.take();
            verified
        }
    };

    let installation = {
        let mut gate = lock_ingress(&ingress);
        gate.install_verified_keys(verified)
            .and_then(|()| gate.start_media())
    };
    let keys = match installation {
        Ok(keys) => keys,
        Err(error) => {
            tasks.cleanup().await;
            return Err(error.into());
        }
    };
    let owner = tasks.owner.take().ok_or(BrowserStartError::IceStopped)?;
    let ice_owner = tasks
        .ice_owner
        .take()
        .ok_or(BrowserStartError::IceStopped)?;
    tasks.armed = false;
    Ok((
        Runtime {
            socket,
            media: MediaIngress {
                srtp: srtp_rx,
                srtcp: srtcp_rx,
            },
            ice,
            ingress,
            owner,
            ice_owner,
            stop,
            profile_tasks,
        },
        keys,
    ))
}

#[cfg(feature = "dtls")]
async fn cleanup_runtime(runtime: Runtime) {
    runtime.stop.stop();
    lock_ingress(&runtime.ingress).close();
    runtime.owner.abort();
    runtime.ice_owner.abort();
    // discard: cancellation is the requested terminal result; awaiting only proves both owned
    // tasks were reaped, so their expected cancelled JoinErrors carry no additional outcome.
    let _ = runtime.owner.await;
    let _ = runtime.ice_owner.await;
}

#[cfg(feature = "dtls")]
struct PreparingTasks {
    stop: Arc<crate::session::Stop>,
    owner: Option<tokio::task::JoinHandle<()>>,
    ice_owner: Option<tokio::task::JoinHandle<()>>,
    dtls: Option<tokio::task::JoinHandle<Result<crate::dtls::VerifiedKeys, crate::dtls::Error>>>,
    armed: bool,
}

#[cfg(feature = "dtls")]
impl PreparingTasks {
    async fn cleanup(&mut self) {
        self.stop.stop();
        if let Some(task) = &self.dtls {
            task.abort();
        }
        if let Some(task) = self.dtls.take() {
            // discard: the caller already retained the typed handshake outcome; this await only
            // reaps a worker that was either completed or explicitly aborted.
            let _ = task.await;
        }
        if let Some(task) = self.owner.take() {
            // discard: the preparation outcome is already terminal and stop is set; awaiting the
            // owner proves cleanup, while a cancellation JoinError changes no public result.
            let _ = task.await;
        }
        if let Some(task) = self.ice_owner.take() {
            // discard: the preparation outcome is already terminal and stop is set; awaiting the
            // ICE owner proves cleanup, while a cancellation JoinError changes no public result.
            let _ = task.await;
        }
        self.armed = false;
    }
}

#[cfg(feature = "dtls")]
impl Drop for PreparingTasks {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.stop.stop();
        if let Some(task) = &self.dtls {
            task.abort();
        }
        if let Some(task) = &self.owner {
            task.abort();
        }
        if let Some(task) = &self.ice_owner {
            task.abort();
        }
    }
}

#[cfg(all(test, feature = "dtls"))]
struct SupervisorActivity;

#[cfg(all(test, feature = "dtls"))]
impl SupervisorActivity {
    fn new() -> Self {
        ACTIVE_SUPERVISORS.fetch_add(1, Ordering::SeqCst);
        SUPERVISOR_CHANGED.notify_waiters();
        Self
    }
}

#[cfg(all(test, feature = "dtls"))]
impl Drop for SupervisorActivity {
    fn drop(&mut self) {
        ACTIVE_SUPERVISORS.fetch_sub(1, Ordering::SeqCst);
        DTLS_HANDSHAKING.store(false, Ordering::SeqCst);
        SUPERVISOR_CHANGED.notify_waiters();
    }
}

#[cfg(feature = "dtls")]
#[allow(clippy::too_many_arguments)]
async fn owner_loop(
    socket: Arc<UdpSocket>,
    ice: crate::ice::driver::Handle,
    ingress: Arc<StdMutex<ComponentIngress>>,
    peering: Option<crate::ice::stun::Peering>,
    dtls: std::sync::mpsc::SyncSender<Vec<u8>>,
    mut dtls_writes: mpsc::Receiver<Vec<u8>>,
    audio_packets: mpsc::Sender<Datagram>,
    control_packets: mpsc::Sender<Datagram>,
    stop: Arc<crate::session::Stop>,
) {
    let mut datagram = vec![0u8; MAX_DATAGRAM + 1];
    let mut dtls_writes_open = true;
    loop {
        let received = tokio::select! {
            () = stop.wait() => return,
            write = dtls_writes.recv(), if dtls_writes_open => {
                let Some(bytes) = write else {
                    dtls_writes_open = false;
                    continue;
                };
                let peer = lock_ingress(&ingress).snapshot().selected.map(|pair| pair.remote);
                if let Some(peer) = peer
                    && socket.send_to(&bytes, peer).await.is_err()
                {
                    return;
                }
                continue;
            }
            received = socket.recv_from(&mut datagram) => received,
        };
        let Ok((length, source)) = received else {
            return;
        };
        let bytes = datagram.get(..length).unwrap_or_default();
        let disposition = lock_ingress(&ingress).admit(source, bytes);
        let IngressDisposition::Accepted(class) = disposition else {
            continue;
        };
        if bytes.len() < minimum_length(class) {
            lock_ingress(&ingress).note_malformed(class);
            continue;
        }
        if class == IngressClass::Stun {
            let accepted = {
                let mut gate = lock_ingress(&ingress);
                account_stun(&mut gate, bytes, peering.as_ref())
            };
            if !accepted {
                continue;
            }
        }
        let admitted = match class {
            IngressClass::Stun => ice.datagram(source, crate::ice::LocalBase(0), bytes.to_vec()),
            IngressClass::Dtls => dtls.try_send(bytes.to_vec()).is_ok(),
            IngressClass::Srtp => audio_packets
                .try_send(Datagram {
                    source,
                    bytes: bytes.to_vec(),
                })
                .is_ok(),
            IngressClass::Srtcp => control_packets
                .try_send(Datagram {
                    source,
                    bytes: bytes.to_vec(),
                })
                .is_ok(),
        };
        if !admitted {
            lock_ingress(&ingress).note_queue_full(class);
        }
    }
}

#[cfg(feature = "dtls")]
enum StunDrop {
    Malformed,
    AuthenticationFailed,
}

#[cfg(feature = "dtls")]
fn validate_stun(
    bytes: &[u8],
    peering: Option<&crate::ice::stun::Peering>,
) -> Result<(), StunDrop> {
    use crate::ice::stun::{Class, Message};

    let message = Message::decode(bytes).map_err(|_| StunDrop::Malformed)?;
    let peering = peering.ok_or(StunDrop::AuthenticationFailed)?;
    let authenticated = match message.class() {
        Class::Request => {
            message.username() == Some(peering.inbound_username().as_str())
                && message.verify_integrity(peering.inbound_key())
        }
        Class::Success | Class::Error => message.verify_integrity(peering.outbound_key()),
        Class::Indication => !message.has_integrity(),
    };
    if authenticated {
        Ok(())
    } else {
        Err(StunDrop::AuthenticationFailed)
    }
}

#[cfg(feature = "dtls")]
fn account_stun(
    ingress: &mut ComponentIngress,
    bytes: &[u8],
    peering: Option<&crate::ice::stun::Peering>,
) -> bool {
    match validate_stun(bytes, peering) {
        Ok(()) => true,
        Err(StunDrop::Malformed) => {
            ingress.note_malformed(IngressClass::Stun);
            false
        }
        Err(StunDrop::AuthenticationFailed) => {
            ingress.note_authentication_failure(IngressClass::Stun);
            false
        }
    }
}

#[cfg(all(test, feature = "dtls"))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ice::stun::{Peering, RoleAttribute, connectivity_check, new_transaction_id};
    use crate::ice::{Gathering, Negotiation, Timers};
    use crate::{Codec, Config, MediaPort};
    use sipx_sdp::RtcpMode;
    use sipx_sdp::ice::{Credentials, Priority};

    fn credentials(ufrag: &str, password: &str) -> Credentials {
        Credentials::new(ufrag, password).expect("valid ICE credentials")
    }

    fn gathering(ufrag: &str, offerer: bool) -> Gathering {
        let mut gathering =
            Gathering::new(credentials(ufrag, "browserPassword0123456789AB"), offerer);
        gathering.agent.timers = Timers {
            ta: Duration::from_millis(20),
            tn: Duration::from_millis(250),
            tr: Duration::from_millis(200),
            ..Timers::default()
        };
        gathering
    }

    fn peer(local: &crate::ice::LocalDescription) -> Negotiation {
        Negotiation::Ice {
            credentials: local.credentials().clone(),
            candidates: local.candidates().to_vec(),
            lite: false,
        }
    }

    fn config(remote: SocketAddr) -> Config {
        let mut config = Config::new(remote, Codec::Pcmu);
        config.rtcp_mode = RtcpMode::Mux;
        config.rtcp_interval = None;
        config
    }

    async fn wait_for_supervisors(expected: usize) {
        let wait = async {
            loop {
                let notified = SUPERVISOR_CHANGED.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if ACTIVE_SUPERVISORS.load(Ordering::SeqCst) == expected {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), wait)
            .await // bound on failure: supervisor cleanup has no timing semantics.
            .expect("supervisor count reaches the expected value");
    }

    async fn wait_for_handshake() {
        let wait = async {
            loop {
                let notified = HANDSHAKE_STARTED.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if DTLS_HANDSHAKING.load(Ordering::SeqCst) {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(Duration::from_secs(2), wait)
            .await // bound on failure: waits for the exact gate transition.
            .expect("DTLS handshaking begins");
    }

    #[test]
    fn structurally_sized_bad_stun_is_counted_exactly_once() {
        let local = credentials("local1", "localPassword0123456789AB");
        let remote = credentials("remote", "remotePassword0123456789A");
        let peering = Peering::new(local.clone(), remote.clone());
        let mut ingress = ComponentIngress::new(0);

        let bad_cookie = [0u8; 20];
        assert!(!account_stun(&mut ingress, &bad_cookie, Some(&peering)));
        assert_eq!(ingress.snapshot().counts.stun_malformed, 1);
        assert_eq!(ingress.snapshot().counts.total(), 1);

        let forged_remote = Peering::new(remote, credentials("local1", "wrongPassword0123456789A"));
        let forged = connectivity_check(
            new_transaction_id(),
            &forged_remote,
            Priority::new(100).expect("priority"),
            RoleAttribute::Controlled { tiebreaker: 7 },
        )
        .expect("encoded check");
        assert!(!account_stun(&mut ingress, &forged, Some(&peering)));
        let counts = ingress.snapshot().counts;
        assert_eq!(counts.stun_authentication_failures, 1);
        assert_eq!(counts.stun_malformed, 1);
        assert_eq!(counts.total(), 2);
    }

    #[test]
    fn every_bounded_ingress_queue_refusal_is_counted_once() {
        let mut ingress = ComponentIngress::new(0);
        for class in [
            IngressClass::Stun,
            IngressClass::Dtls,
            IngressClass::Srtp,
            IngressClass::Srtcp,
        ] {
            ingress.note_queue_full(class);
        }
        let counts = ingress.snapshot().counts;
        assert_eq!(counts.stun_queue_refusals, 1);
        assert_eq!(counts.dtls_queue_refusals, 1);
        assert_eq!(counts.srtp_queue_refusals, 1);
        assert_eq!(counts.srtcp_queue_refusals, 1);
        assert_eq!(counts.total(), 4);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_reaps_ice_and_dtls_preparation_without_a_detached_receiver() {
        use crate::dtls::Role;
        use crate::dtls::openssl::Identity;

        // Cancel while ICE is checking and no peer is running.
        let alice_port = MediaPort::bind("127.0.0.1:0".parse().expect("address"))
            .await
            .expect("Alice port");
        let bob_port = MediaPort::bind("127.0.0.1:0".parse().expect("address"))
            .await
            .expect("Bob fixture port");
        let alice_addr = alice_port.local_addr();
        let alice_gathering = gathering("alice4", true);
        let bob_gathering = gathering("bob004", false);
        let (mut alice_ice, bob_ice) = tokio::join!(
            alice_port.gather_with_rtcp_mode(&alice_gathering, RtcpMode::Mux),
            bob_port.gather_with_rtcp_mode(&bob_gathering, RtcpMode::Mux),
        );
        assert!(alice_ice.accept(&peer(&bob_ice)));
        let identity = Identity::generate().expect("identity");
        let fingerprint = Identity::generate()
            .expect("peer identity")
            .fingerprint()
            .expect("peer fingerprint");
        let checking = tokio::spawn(alice_port.start_browser_audio(
            config(bob_port.local_addr()),
            alice_ice,
            0,
            identity,
            Role::Client,
            fingerprint,
            Duration::from_secs(5),
        ));
        wait_for_supervisors(1).await;
        assert!(!DTLS_HANDSHAKING.load(Ordering::SeqCst));
        checking.abort();
        // discard: this test requested cancellation and asserts cleanup through the supervisor
        // census and immediate port rebind below, rather than through the expected JoinError.
        let _ = checking.await;
        wait_for_supervisors(0).await;
        drop(bob_port);
        drop(
            UdpSocket::bind(alice_addr)
                .await
                .expect("ICE cancellation released the port"),
        );

        // Cancel after exact nomination has opened the DTLS adapter.
        let alice_port = MediaPort::bind("127.0.0.1:0".parse().expect("address"))
            .await
            .expect("Alice port");
        let bob_port = MediaPort::bind("127.0.0.1:0".parse().expect("address"))
            .await
            .expect("Bob port");
        let (alice_addr, bob_addr) = (alice_port.local_addr(), bob_port.local_addr());
        let alice_gathering = gathering("alice5", true);
        let bob_gathering = gathering("bob005", false);
        let (mut alice_ice, mut bob_ice) = tokio::join!(
            alice_port.gather_with_rtcp_mode(&alice_gathering, RtcpMode::Mux),
            bob_port.gather_with_rtcp_mode(&bob_gathering, RtcpMode::Mux),
        );
        assert!(alice_ice.accept(&peer(&bob_ice)));
        assert!(bob_ice.accept(&peer(&alice_ice)));
        let bob = bob_port
            .start_with_ice(config(alice_addr), bob_ice)
            .expect("ordinary ICE peer");
        let identity = Identity::generate().expect("identity");
        let fingerprint = Identity::generate()
            .expect("peer identity")
            .fingerprint()
            .expect("peer fingerprint");
        let handshaking = tokio::spawn(alice_port.start_browser_audio(
            config(bob_addr),
            alice_ice,
            0,
            identity,
            Role::Client,
            fingerprint,
            Duration::from_secs(5),
        ));
        wait_for_handshake().await;
        handshaking.abort();
        // discard: this test requested cancellation and asserts cleanup through the supervisor
        // census and immediate port rebind below, rather than through the expected JoinError.
        let _ = handshaking.await;
        wait_for_supervisors(0).await;
        drop(bob);
        drop(
            UdpSocket::bind(alice_addr)
                .await
                .expect("DTLS cancellation released the port"),
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn actual_spawn_lifetimes_stay_inside_the_phase_task_bound() {
        use crate::dtls::Role;
        use crate::dtls::openssl::Identity;

        let alice_port = MediaPort::bind("127.0.0.1:0".parse().expect("address"))
            .await
            .expect("Alice port");
        let bob_port = MediaPort::bind("127.0.0.1:0".parse().expect("address"))
            .await
            .expect("Bob port");
        let (alice_addr, bob_addr) = (alice_port.local_addr(), bob_port.local_addr());
        let alice_gathering = gathering("alice6", true);
        let bob_gathering = gathering("bob006", false);
        let (mut alice_ice, mut bob_ice) = tokio::join!(
            alice_port.gather_with_rtcp_mode(&alice_gathering, RtcpMode::Mux),
            bob_port.gather_with_rtcp_mode(&bob_gathering, RtcpMode::Mux),
        );
        assert!(alice_ice.accept(&peer(&bob_ice)));
        assert!(bob_ice.accept(&peer(&alice_ice)));
        let alice_identity = Identity::generate().expect("Alice identity");
        let bob_identity = Identity::generate().expect("Bob identity");
        let alice_fingerprint = alice_identity.fingerprint().expect("Alice fingerprint");
        let bob_fingerprint = bob_identity.fingerprint().expect("Bob fingerprint");
        let mut alice_config = config(bob_addr);
        let mut bob_config = config(alice_addr);
        alice_config.rtcp_interval = Some(Duration::from_millis(50));
        bob_config.rtcp_interval = Some(Duration::from_millis(50));

        let (alice, bob) = tokio::time::timeout(Duration::from_secs(8), async {
            tokio::join!(
                alice_port.start_browser_audio(
                    alice_config,
                    alice_ice,
                    0,
                    alice_identity,
                    Role::Client,
                    bob_fingerprint,
                    Duration::from_secs(5),
                ),
                bob_port.start_browser_audio(
                    bob_config,
                    bob_ice,
                    0,
                    bob_identity,
                    Role::Server,
                    alice_fingerprint,
                    Duration::from_secs(5),
                ),
            )
        })
        .await // bound on failure: the task census requires a completed handshake.
        .expect("both components reach Running");
        let alice = alice.expect("Alice starts");
        let bob = bob.expect("Bob starts");

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let alice_peak = alice.browser_task_counts().map_or(0, |(_, _, peak)| peak);
                let bob_peak = bob.browser_task_counts().map_or(0, |(_, _, peak)| peak);
                if alice_peak == MAX_PROFILE_TASKS && bob_peak == MAX_PROFILE_TASKS {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await // bound on failure: every running worker must enter its counted future promptly.
        .expect("running workers enter the task census");
        for session in [&alice, &bob] {
            let (preparing_peak, active, peak) =
                session.browser_task_counts().expect("browser task census");
            assert_eq!(preparing_peak, 4);
            assert_eq!(peak, MAX_PROFILE_TASKS);
            assert!(active <= MAX_PROFILE_TASKS);
        }
        let alice_tasks = alice.browser_task_probe().expect("Alice task probe");
        let bob_tasks = bob.browser_task_probe().expect("Bob task probe");
        drop(alice);
        drop(bob);
        tokio::time::timeout(Duration::from_secs(1), async {
            while alice_tasks.counts().0 != 0 || bob_tasks.counts().0 != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await // bound on failure: Drop aborts and reaps every retained profile worker.
        .expect("running-session cancellation leaves no profile task");
        drop(
            UdpSocket::bind(alice_addr)
                .await
                .expect("Alice running cancellation released its port"),
        );
        drop(
            UdpSocket::bind(bob_addr)
                .await
                .expect("Bob running cancellation released its port"),
        );
    }
}

#[cfg(feature = "dtls")]
const fn minimum_length(class: IngressClass) -> usize {
    match class {
        IngressClass::Stun => 20,
        IngressClass::Dtls => 13,
        IngressClass::Srtp => 12,
        IngressClass::Srtcp => 8,
    }
}

pub(crate) fn lock_ingress(
    ingress: &Arc<StdMutex<ComponentIngress>>,
) -> std::sync::MutexGuard<'_, ComponentIngress> {
    ingress
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(feature = "dtls")]
struct DtlsAdapter {
    inbound: std::sync::mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<Vec<u8>>,
    timeout: Duration,
}

#[cfg(feature = "dtls")]
impl Read for DtlsAdapter {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let datagram = self.inbound.recv_timeout(self.timeout).map_err(|error| {
            let kind = match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => std::io::ErrorKind::TimedOut,
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    std::io::ErrorKind::UnexpectedEof
                }
            };
            std::io::Error::new(kind, error)
        })?;
        let destination = buffer.get_mut(..datagram.len()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "DTLS record exceeds the adapter read buffer",
            )
        })?;
        destination.copy_from_slice(&datagram);
        Ok(datagram.len())
    }
}

#[cfg(feature = "dtls")]
impl Write for DtlsAdapter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if buffer.len() > MAX_OUTBOUND_DATAGRAM {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "DTLS record exceeds the browser-component outbound bound",
            ));
        }
        self.outbound
            .blocking_send(buffer.to_vec())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::BrokenPipe, error))?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
