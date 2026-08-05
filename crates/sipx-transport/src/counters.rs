//! What a running endpoint will say about itself.
//!
//! `docs/specs/sip-transport.md` §12. Counters and nothing else: no metrics library, no exposition
//! format, no push. A [`Counters`] snapshot is read through [`crate::Handle::counters`] and what an
//! application does with it is the application's business — a stack that picks an exposition format
//! picks it for every user of the library, and that is the one observability decision here that
//! cannot be undone later.
//!
//! The atomics live behind an `Arc` shared with every handle rather than being answered by the
//! event loop, which is the same choice [`ShedCounts`] made for the same reason: the loop is busy in
//! precisely the situation these numbers describe. `Handle::shed` is synchronous; the `async`
//! `Handle::outstanding` beside it has to ask the loop and can fail because of it. A snapshot that
//! could return `Err(EndpointClosed)` under load would be unavailable exactly when an operator
//! reached for it.
//!
//! **What these numbers do not promise is in §12.2 and repeated on the types below.** The short
//! version: each field is individually exact, and the *relationship* between two fields of one
//! snapshot is not, because they are separate atomics read one after another.

use std::sync::atomic::{AtomicU64, Ordering};

use sipx_sip::Method;
use sipx_sip::transaction::Timer;

use crate::target::TransportKind;

/// How many transports are counted apart. One slot per [`TransportKind`] variant.
const TRANSPORTS: usize = 6;

/// Which slot a transport's counters live in.
///
/// A match rather than `as usize`, so adding a `TransportKind` variant is a compile error here
/// instead of a silent write into the wrong transport's numbers.
const fn slot(transport: TransportKind) -> usize {
    match transport {
        TransportKind::Udp => 0,
        TransportKind::Tcp => 1,
        TransportKind::Tls => 2,
        TransportKind::Ws => 3,
        TransportKind::Wss => 4,
        TransportKind::Quic => 5,
    }
}

/// What the endpoint has dropped because the application was not keeping up.
///
/// Kept as atomics behind an `Arc` rather than answered by the event loop, and that is the point:
/// the loop is busy in exactly the situation this counts, so a counter you could only read by
/// asking it would be unreadable when it mattered. [`crate::Handle::shed`] reads it without
/// touching the loop at all.
///
/// The three kinds are counted apart because their consequences differ by an order of magnitude,
/// and one number would hide that.
#[derive(Debug, Default)]
pub(crate) struct Shed {
    pub(crate) requests: AtomicU64,
    pub(crate) acks: AtomicU64,
    pub(crate) unmatched: AtomicU64,
}

/// A snapshot of what an endpoint has shed (see [`crate::Handle::shed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShedCounts {
    /// Requests that reached a server transaction and could not be handed to the application.
    ///
    /// Answered `503 Service Unavailable` with a `Retry-After`, so the peer is told something
    /// true rather than left to retransmit into a queue that is still full.
    pub requests: u64,
    /// **ACKs** that could not be handed over.
    ///
    /// The serious one, and the reason these are not one number. An ACK for a 2xx has no
    /// transaction to answer — RFC 3261 §17.1.1.3 makes it a new transaction of its own, and
    /// there is no response to an ACK in SIP at all — so there is no 503 to send and nothing
    /// retransmits it after Timer H. Both ends are then in a dialog that no timer will reap
    /// unless session timers (RFC 4028) happen to be in play. A non-zero count here means calls
    /// are leaking.
    pub acks: u64,
    /// Requests that matched no transaction and could not be handed over.
    ///
    /// The peer will retransmit an unmatched INVITE, so this is the most survivable of the three
    /// — but it is still loss, and it was previously invisible.
    pub unmatched: u64,
}

impl ShedCounts {
    /// Whether anything has been shed at all.
    #[must_use]
    pub fn any(self) -> bool {
        self.total() > 0
    }

    /// Everything shed, of every kind.
    #[must_use]
    pub fn total(self) -> u64 {
        self.requests
            .saturating_add(self.acks)
            .saturating_add(self.unmatched)
    }
}

/// Per-transport message counts, live.
#[derive(Debug, Default)]
struct TransportMeter {
    requests_in: AtomicU64,
    requests_out: AtomicU64,
    responses_in: AtomicU64,
    responses_out: AtomicU64,
    parse_failures: AtomicU64,
    source_refusals: AtomicU64,
}

/// What crossed one transport, in both directions.
///
/// Per transport because which transport is the first question a support case asks, and an
/// aggregate cannot answer it: "we are losing messages" has a different cause over UDP than over a
/// WebSocket, and the same total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransportCounts {
    /// Requests that arrived and parsed.
    pub requests_in: u64,
    /// Requests put on the wire, including retransmissions.
    pub requests_out: u64,
    /// Responses that arrived and parsed.
    pub responses_in: u64,
    /// Responses put on the wire, including retransmissions.
    pub responses_out: u64,
    /// Bytes that arrived and could not be parsed as a SIP message.
    ///
    /// **Not** also counted as a request or a response: which one it would have been is exactly
    /// what could not be determined (§12.2). So `requests_in + responses_in` omits these, and the
    /// number of messages that arrived at all is that sum *plus* this.
    pub parse_failures: u64,
    /// Packets or new connections rejected by live source admission before protocol work.
    pub source_refusals: u64,
}

/// Transactions abandoned by the timer that gave up on them.
///
/// Split by timer because the three mean different things to whoever is reading. A rise in `b` or
/// `f` is a peer that stopped answering; a rise in `h` is a peer that answered and then never
/// acknowledged our final response, which is a different fault with a different fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimeoutCounts {
    /// Timer B: an INVITE client transaction gave up (RFC 3261 §17.1.1.2).
    pub b: u64,
    /// Timer F: a non-INVITE client transaction gave up (§17.1.2.2).
    pub f: u64,
    /// Timer H: no ACK arrived for a final INVITE response (§17.2.1).
    pub h: u64,
}

impl TimeoutCounts {
    /// Every transaction any timer gave up on.
    #[must_use]
    pub fn total(self) -> u64 {
        self.b.saturating_add(self.f).saturating_add(self.h)
    }
}

/// How the capture is faring, if one is running (§13).
///
/// Zero on every field is the ordinary state, because capture is off by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureCounts {
    /// Records handed to the writer.
    pub records: u64,
    /// Records dropped because the writer was behind.
    ///
    /// The channel to the writer is bounded and an overrun drops rather than blocking the driver
    /// (§13.2): blocking would put the filesystem in the retransmission path. A capture with a gap
    /// that says so is usable; a stack that stalled to avoid the gap is not.
    pub dropped: u64,
    /// Writes that failed, after which the capture is disabled.
    ///
    /// A full disk is the usual reason. Counted rather than only logged because a capture that is
    /// silently not happening is the same failure as a silent discard, one level up.
    pub errors: u64,
    /// Records sent to the configured HEP3 collector.
    pub hep_records: u64,
    /// HEP3 records rejected by encoding or the non-blocking collector socket.
    ///
    /// A drop never disables the local pcapng capture or fails a call.
    pub hep_dropped: u64,
}

/// Places the endpoint throws something away that are not backpressure (§12.1).
///
/// Each of these was a `let _ = …` or a bare `tracing` line before `X-18`. None is necessarily a
/// fault — most are the correct handling of something unwanted — but every one of them used to be
/// invisible, and "how often" is not a question anyone should answer with `grep | wc -l`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiscardCounts {
    /// Events for a client transaction whose application receiver was full or gone.
    ///
    /// The serious one here. A dropped response event means an application that asked for a
    /// transaction's outcome does not learn it, and nothing retransmits an event.
    pub transaction_events: u64,
    /// Server transactions abandoned because the application never answered them.
    ///
    /// An application bug rather than a network one, which is exactly why it needs its own number:
    /// nothing on the wire will show it.
    pub unanswered: u64,
    /// Messages a transaction wanted sent that had no destination to send them to.
    pub no_destination: u64,
    /// Sends the transport refused.
    ///
    /// The transaction is given a transport error, so this is not silent loss — but the rate is
    /// worth having, because a peer that has become unreachable shows up here first.
    pub send_failures: u64,
    /// STUN replies that matched no keep-alive, and STUN messages that were not replies.
    ///
    /// RFC 5389 §6 wants the transaction ID unguessable precisely so a forged reply cannot be
    /// matched; a rising count here is either a broken peer or someone trying.
    pub stun_unmatched: u64,
}

impl DiscardCounts {
    /// Everything discarded outside the backpressure path.
    #[must_use]
    pub fn total(self) -> u64 {
        self.transaction_events
            .saturating_add(self.unanswered)
            .saturating_add(self.no_destination)
            .saturating_add(self.send_failures)
            .saturating_add(self.stun_unmatched)
    }
}

/// Live counts of requests the endpoint was asked to send and did not.
#[derive(Debug, Default)]
struct Unsent {
    invite: AtomicU64,
    ack: AtomicU64,
    bye: AtomicU64,
    cancel: AtomicU64,
    other: AtomicU64,
}

/// Requests the endpoint tried to put on the wire and could not, by method (§12.3).
///
/// **Split by method because the consequence is.** A CANCEL that does not go out leaves the far end
/// ringing; an ACK that does not go out leaves a 2xx retransmitting for thirty-two seconds and then
/// a peer streaming at a port this side has closed; a BYE that does not go out leaves a dialog up
/// at the far end that no timer will reap. One number would hide which of those happened, and they
/// are the three questions an operator asking "why did that call linger" is choosing between.
///
/// # Where this is counted, and why not at the hand-off
///
/// **At the transmit, in the driver** — the two places the socket is actually written: a
/// transaction's `Output::Send`, and the direct send that carries an ACK for a 2xx.
///
/// It was counted inside [`crate::Handle::send`] and [`crate::Handle::send_directly`] when `X-54`
/// first wrote it, and that was **wrong in a way the type's own documentation concealed**.
/// `Handle::send` returns as soon as the driver has created the transaction and handed back its
/// key; the transmit happens afterwards. So a counter at that hand-off could only ever fire when
/// the endpoint refused the request outright — a closed endpoint, or a request with no usable
/// `Via` — and **never** on a refused connection, an unreachable peer or an over-MTU datagram,
/// which is the whole of the question it claims to answer. `send_directly` did await the transmit,
/// so `ack` behaved one way and `bye` and `cancel` another, with nothing saying so. Counting where
/// the wire is missed makes all four mean the same thing.
///
/// # What this does and does not promise (§12.2)
///
/// - **Requests only.** A response that fails to transmit is counted by
///   [`DiscardCounts::send_failures`] and not here, because these fields are methods and a response
///   has none.
/// - **This overlaps [`DiscardCounts::send_failures`] on purpose, and the two are views rather
///   than tallies.** A *request* that fails on the transaction path increments both: that field is
///   the transaction path's aggregate over requests and responses alike, this is the per-method
///   breakdown over requests on any path. Adding them together is meaningless, and so is
///   subtracting them — §12.2's rule against arithmetic across fields applies here in particular.
/// - **An endpoint that is shutting down does not inflate this.** A send that loses the race with
///   `shutdown` fails at the hand-off, before any transmit is attempted, and is not counted — the
///   earlier design counted it, which made an ordinary teardown look like lost signalling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnsentCounts {
    /// INVITEs the endpoint could not put on the wire.
    pub invite: u64,
    /// ACKs the endpoint could not put on the wire — RFC 3261 §13.2.2.4's ACK for a 2xx, which has
    /// no transaction to retry it.
    pub ack: u64,
    /// BYEs the endpoint could not put on the wire. The one that leaves a call up at the far end.
    pub bye: u64,
    /// CANCELs the endpoint could not put on the wire. The one that leaves a phone ringing.
    pub cancel: u64,
    /// Every other method the endpoint could not put on the wire.
    pub other: u64,
}

impl UnsentCounts {
    /// Every request that did not reach the wire, of every method.
    #[must_use]
    pub fn total(self) -> u64 {
        self.invite
            .saturating_add(self.ack)
            .saturating_add(self.bye)
            .saturating_add(self.cancel)
            .saturating_add(self.other)
    }
}

/// Everything an endpoint will tell you about itself, at one moment (§12).
///
/// # What this is not
///
/// **A snapshot is not an instant.** The fields are separate atomics read one after another, so a
/// snapshot taken while traffic flows can show [`TransportCounts::requests_in`] from a later moment
/// than [`TransportCounts::responses_in`]. Every field is individually monotonic and none is ever
/// lost, so *differences between successive snapshots* are sound. Arithmetic identities across
/// fields of a single snapshot are not, unless the endpoint is quiet.
///
/// See [`TransportCounts::parse_failures`] for the one place that surprises people: in and out do
/// not balance, by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// What was dropped because the application was not keeping up (§10).
    ///
    /// The same value [`crate::Handle::shed`] returns, embedded rather than recounted: two tallies
    /// of one event would eventually disagree, and then neither could be trusted.
    pub shed: ShedCounts,
    /// Locally rejected outbound requests under RFC 7339/RFC 7415 control.
    pub overload_rejections: u64,
    /// Responses that matched no client transaction (RFC 3261 §16.7).
    ///
    /// Counted whether or not anyone is watching for them through
    /// [`crate::Handle::watch_unmatched`]. A user agent is right to ignore these; a forwarding
    /// element is required to act on them, and either way the rate is worth knowing.
    pub unmatched_responses: u64,
    /// Retransmissions put on the wire by Timer A, E or G.
    ///
    /// Counted **where the timer fires**, so a retransmission the socket then refuses is still
    /// counted as sent (§12.2). Counting after the socket call would mean a peer that stopped
    /// hearing us produced a *falling* count, inverting the signal this exists to give.
    ///
    /// A rise with no matching growth in traffic is a peer that is not hearing us, which is the
    /// difference between a network problem and an application one.
    pub retransmissions_sent: u64,
    /// Oversized UDP requests for which RFC 3261 §18.1.1 selected TCP instead.
    ///
    /// Counted at selection, including a selection whose connection attempt then fails. Consult
    /// [`Self::unsent`] and [`Self::discards`] for whether the selected send reached the wire.
    pub oversized_request_tcp_fallbacks: u64,
    /// Transactions a timer gave up on.
    pub timeouts: TimeoutCounts,
    /// Discards that are not backpressure (§12.1).
    pub discards: DiscardCounts,
    /// Requests the endpoint tried to put on the wire and could not, by method (§12.3).
    ///
    /// Overlaps [`DiscardCounts::send_failures`] for requests on the transaction path — see
    /// [`UnsentCounts`], which states what that does and does not let you conclude.
    pub unsent: UnsentCounts,
    /// How the capture is faring, if one is running (§13).
    pub capture: CaptureCounts,
    /// Observation events dropped because the optional bounded consumer was behind.
    pub observation_dropped: u64,
    /// Per-transport message counts, read through [`Counters::transport`].
    per_transport: [TransportCounts; TRANSPORTS],
}

impl Counters {
    /// What crossed one transport.
    #[must_use]
    pub fn transport(&self, transport: TransportKind) -> TransportCounts {
        // `slot` is total over the enum and `per_transport` is sized to match, so this cannot be
        // out of range — but `get` says so without a panicking index (`AGENTS.md` §3).
        self.per_transport
            .get(slot(transport))
            .copied()
            .unwrap_or_default()
    }

    /// Messages that arrived and parsed, over every transport.
    #[must_use]
    pub fn messages_in(&self) -> u64 {
        self.per_transport.iter().fold(0, |total, counts| {
            total
                .saturating_add(counts.requests_in)
                .saturating_add(counts.responses_in)
        })
    }

    /// Messages put on the wire, over every transport.
    #[must_use]
    pub fn messages_out(&self) -> u64 {
        self.per_transport.iter().fold(0, |total, counts| {
            total
                .saturating_add(counts.requests_out)
                .saturating_add(counts.responses_out)
        })
    }

    /// Everything that arrived and could not be parsed, over every transport.
    #[must_use]
    pub fn parse_failures(&self) -> u64 {
        self.per_transport.iter().fold(0, |total, counts| {
            total.saturating_add(counts.parse_failures)
        })
    }

    /// Whether anything at all has been lost: shed, discarded, dropped from a capture, or never
    /// put on the wire.
    ///
    /// A single question for a health check to ask. Deliberately does **not** include
    /// [`Self::parse_failures`] or [`Self::timeouts`]: a malformed datagram from a stranger and a
    /// peer that stopped answering are things that happened *to* the endpoint, not things it threw
    /// away, and folding them in here would make the number impossible to act on.
    #[must_use]
    pub fn any_loss(&self) -> bool {
        self.shed.any()
            || self.overload_rejections > 0
            || self.discards.total() > 0
            || self.capture.dropped > 0
            || self.capture.hep_dropped > 0
            || self.observation_dropped > 0
            || self.unsent.total() > 0
    }
}

/// The live counters, shared between the driver and every handle.
///
/// Every counter below is incremented from exactly one place in the crate — the methods on this
/// type — which is what makes §12.2's promise checkable: there is no path on which one event
/// increments a counter twice, and none on which an increment is lost. `Relaxed` throughout,
/// because nothing here guards data and no reader draws a conclusion from the order of two
/// increments.
#[derive(Debug, Default)]
pub(crate) struct Meters {
    pub(crate) shed: Shed,
    overload_rejections: AtomicU64,
    per_transport: [TransportMeter; TRANSPORTS],
    unmatched_responses: AtomicU64,
    retransmissions: AtomicU64,
    oversized_request_tcp_fallbacks: AtomicU64,
    timeout_b: AtomicU64,
    timeout_f: AtomicU64,
    timeout_h: AtomicU64,
    discard_transaction_events: AtomicU64,
    discard_unanswered: AtomicU64,
    discard_no_destination: AtomicU64,
    discard_send_failures: AtomicU64,
    discard_stun_unmatched: AtomicU64,
    capture_records: AtomicU64,
    capture_dropped: AtomicU64,
    capture_errors: AtomicU64,
    capture_hep_records: AtomicU64,
    capture_hep_dropped: AtomicU64,
    observation_dropped: AtomicU64,
    unsent: Unsent,
}

/// One increment, in the one ordering this module uses.
fn bump(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

fn read(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

impl Meters {
    /// An outbound request was rejected by active overload control.
    pub(crate) fn overload_rejection(&self) {
        bump(&self.overload_rejections);
    }

    /// The meter for one transport.
    fn meter(&self, transport: TransportKind) -> Option<&TransportMeter> {
        self.per_transport.get(slot(transport))
    }

    /// A message arrived and parsed.
    pub(crate) fn message_in(&self, transport: TransportKind, is_response: bool) {
        if let Some(meter) = self.meter(transport) {
            if is_response {
                bump(&meter.responses_in);
            } else {
                bump(&meter.requests_in);
            }
        }
    }

    /// A message went out.
    pub(crate) fn message_out(&self, transport: TransportKind, is_response: bool) {
        if let Some(meter) = self.meter(transport) {
            if is_response {
                bump(&meter.responses_out);
            } else {
                bump(&meter.requests_out);
            }
        }
    }

    /// Bytes arrived that were not a SIP message.
    pub(crate) fn parse_failure(&self, transport: TransportKind) {
        if let Some(meter) = self.meter(transport) {
            bump(&meter.parse_failures);
        }
    }

    /// A source was refused before parsing or handshaking.
    pub(crate) fn source_refusal(&self, transport: TransportKind) {
        if let Some(meter) = self.meter(transport) {
            bump(&meter.source_refusals);
        }
    }

    /// A bounded observation receiver was full.
    pub(crate) fn observation_drop(&self) {
        bump(&self.observation_dropped);
    }

    /// A response matched no client transaction.
    pub(crate) fn unmatched_response(&self) {
        bump(&self.unmatched_responses);
    }

    /// A timer fired and produced a retransmission.
    ///
    /// Only A, E and G retransmit; every other timer is a deadline or an absorption window, and
    /// counting those here would make the number mean "timers fired" instead.
    pub(crate) fn on_timer(&self, timer: Timer) {
        match timer {
            Timer::A | Timer::E | Timer::G => bump(&self.retransmissions),
            Timer::B => bump(&self.timeout_b),
            Timer::F => bump(&self.timeout_f),
            Timer::H => bump(&self.timeout_h),
            Timer::D | Timer::I | Timer::J | Timer::K | Timer::L | Timer::M | Timer::Trying100 => {}
        }
    }

    /// An oversized UDP request selected TCP under RFC 3261 §18.1.1.
    pub(crate) fn oversized_request_tcp_fallback(&self) {
        bump(&self.oversized_request_tcp_fallbacks);
    }

    /// An event for a client transaction could not be handed over.
    pub(crate) fn discard_transaction_event(&self) {
        bump(&self.discard_transaction_events);
    }

    /// A server transaction the application never answered was abandoned.
    pub(crate) fn discard_unanswered(&self) {
        bump(&self.discard_unanswered);
    }

    /// A message a transaction wanted sent had nowhere to go.
    pub(crate) fn discard_no_destination(&self) {
        bump(&self.discard_no_destination);
    }

    /// The transport refused a send.
    pub(crate) fn discard_send_failure(&self) {
        bump(&self.discard_send_failures);
    }

    /// A STUN message matched no keep-alive, or was not a reply.
    pub(crate) fn discard_stun_unmatched(&self) {
        bump(&self.discard_stun_unmatched);
    }

    /// A record was handed to the capture writer.
    pub(crate) fn capture_record(&self) {
        bump(&self.capture_records);
    }

    /// A record was dropped because the capture writer was behind.
    pub(crate) fn capture_drop(&self) {
        bump(&self.capture_dropped);
    }

    /// A capture write failed.
    pub(crate) fn capture_error(&self) {
        bump(&self.capture_errors);
    }

    /// A redacted HEP3 datagram reached the collector socket.
    pub(crate) fn capture_hep_record(&self) {
        bump(&self.capture_hep_records);
    }

    /// A HEP3 datagram could not be encoded or sent without blocking.
    pub(crate) fn capture_hep_drop(&self) {
        bump(&self.capture_hep_dropped);
    }

    /// A request the endpoint tried to put on the wire and could not.
    ///
    /// Called from the driver, at the two places the socket is written — never from a `Handle`
    /// method, which returns before the transmit happens (see [`UnsentCounts`]).
    ///
    /// A match rather than an index, so a new [`Method`] variant is a compile error here instead of
    /// silently landing in `other` — the same reason [`slot`] is a match.
    pub(crate) fn unsent(&self, method: &Method) {
        bump(match method {
            Method::Invite => &self.unsent.invite,
            Method::Ack => &self.unsent.ack,
            Method::Bye => &self.unsent.bye,
            Method::Cancel => &self.unsent.cancel,
            Method::Register
            | Method::Options
            | Method::Info
            | Method::Prack
            | Method::Update
            | Method::Subscribe
            | Method::Notify
            | Method::Refer
            | Method::Message
            | Method::Publish
            | Method::Other(_) => &self.unsent.other,
        });
    }

    /// Read everything, field by field.
    ///
    /// Not a consistent instant, and [`Counters`] says so: taking a lock to make it one would put
    /// the reader in the driver's way, which is the thing §12 refuses to do.
    pub(crate) fn snapshot(&self) -> Counters {
        let mut per_transport = [TransportCounts::default(); TRANSPORTS];
        for (counts, meter) in per_transport.iter_mut().zip(self.per_transport.iter()) {
            *counts = TransportCounts {
                requests_in: read(&meter.requests_in),
                requests_out: read(&meter.requests_out),
                responses_in: read(&meter.responses_in),
                responses_out: read(&meter.responses_out),
                parse_failures: read(&meter.parse_failures),
                source_refusals: read(&meter.source_refusals),
            };
        }
        Counters {
            shed: ShedCounts {
                requests: read(&self.shed.requests),
                acks: read(&self.shed.acks),
                unmatched: read(&self.shed.unmatched),
            },
            overload_rejections: read(&self.overload_rejections),
            unmatched_responses: read(&self.unmatched_responses),
            retransmissions_sent: read(&self.retransmissions),
            oversized_request_tcp_fallbacks: read(&self.oversized_request_tcp_fallbacks),
            timeouts: TimeoutCounts {
                b: read(&self.timeout_b),
                f: read(&self.timeout_f),
                h: read(&self.timeout_h),
            },
            discards: DiscardCounts {
                transaction_events: read(&self.discard_transaction_events),
                unanswered: read(&self.discard_unanswered),
                no_destination: read(&self.discard_no_destination),
                send_failures: read(&self.discard_send_failures),
                stun_unmatched: read(&self.discard_stun_unmatched),
            },
            capture: CaptureCounts {
                records: read(&self.capture_records),
                dropped: read(&self.capture_dropped),
                errors: read(&self.capture_errors),
                hep_records: read(&self.capture_hep_records),
                hep_dropped: read(&self.capture_hep_dropped),
            },
            observation_dropped: read(&self.observation_dropped),
            unsent: UnsentCounts {
                invite: read(&self.unsent.invite),
                ack: read(&self.unsent.ack),
                bye: read(&self.unsent.bye),
                cancel: read(&self.unsent.cancel),
                other: read(&self.unsent.other),
            },
            per_transport,
        }
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
    fn every_transport_has_its_own_slot() {
        let kinds = [
            TransportKind::Udp,
            TransportKind::Tcp,
            TransportKind::Tls,
            TransportKind::Ws,
            TransportKind::Wss,
            TransportKind::Quic,
        ];
        let mut slots: Vec<usize> = kinds.iter().copied().map(slot).collect();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(
            slots.len(),
            TRANSPORTS,
            "two transports share a slot, so their counts are being added together"
        );
        assert_eq!(
            slots.last().copied(),
            Some(TRANSPORTS - 1),
            "a slot is out of range of the array it indexes"
        );
    }

    #[test]
    fn a_message_is_counted_against_its_own_transport_only() {
        let meters = Meters::default();
        meters.message_in(TransportKind::Tcp, false);
        meters.message_out(TransportKind::Tcp, true);

        let counters = meters.snapshot();
        assert_eq!(counters.transport(TransportKind::Tcp).requests_in, 1);
        assert_eq!(counters.transport(TransportKind::Tcp).responses_out, 1);
        assert_eq!(
            counters.transport(TransportKind::Udp),
            TransportCounts::default(),
            "a TCP message must not appear in UDP's counts"
        );
        assert_eq!(counters.messages_in(), 1);
        assert_eq!(counters.messages_out(), 1);
    }

    /// §12.2's second limit: a parse failure is not also a message.
    #[test]
    fn a_parse_failure_is_not_counted_as_a_message() {
        let meters = Meters::default();
        meters.parse_failure(TransportKind::Udp);

        let counters = meters.snapshot();
        assert_eq!(counters.parse_failures(), 1);
        assert_eq!(
            counters.messages_in(),
            0,
            "which it would have been is exactly what could not be determined"
        );
    }

    /// Only the three retransmission timers count as retransmissions, and only the three deadline
    /// timers as timeouts. Without this the absorption timers — D, I, J, K, L, M — would inflate
    /// both numbers on every ordinary transaction.
    #[test]
    fn only_the_retransmission_timers_count_as_retransmissions() {
        let meters = Meters::default();
        for timer in [Timer::A, Timer::E, Timer::G] {
            meters.on_timer(timer);
        }
        for timer in [
            Timer::D,
            Timer::I,
            Timer::J,
            Timer::K,
            Timer::L,
            Timer::M,
            Timer::Trying100,
        ] {
            meters.on_timer(timer);
        }

        let counters = meters.snapshot();
        assert_eq!(counters.retransmissions_sent, 3);
        assert_eq!(
            counters.timeouts.total(),
            0,
            "an absorption window closing is not a transaction timing out"
        );
    }

    #[test]
    fn each_deadline_timer_is_counted_apart() {
        let meters = Meters::default();
        meters.on_timer(Timer::B);
        meters.on_timer(Timer::H);
        meters.on_timer(Timer::H);

        let counters = meters.snapshot();
        assert_eq!(counters.timeouts.b, 1);
        assert_eq!(counters.timeouts.f, 0);
        assert_eq!(counters.timeouts.h, 2);
        assert_eq!(counters.timeouts.total(), 3);
    }

    /// `any_loss` is what a health check asks. It must answer "yes" to a discard and "no" to a
    /// malformed datagram, which happened *to* the endpoint rather than being thrown away by it.
    #[test]
    fn any_loss_covers_discards_and_not_arrivals() {
        let meters = Meters::default();
        assert!(!meters.snapshot().any_loss());

        meters.parse_failure(TransportKind::Udp);
        meters.on_timer(Timer::B);
        assert!(
            !meters.snapshot().any_loss(),
            "a stranger's malformed datagram is not this endpoint losing something"
        );

        meters.discard_transaction_event();
        assert!(meters.snapshot().any_loss());
    }

    #[test]
    fn a_fresh_endpoint_reports_zero_everywhere() {
        let counters = Meters::default().snapshot();
        assert_eq!(counters, Counters::default());
        assert!(!counters.any_loss());
        assert_eq!(counters.capture, CaptureCounts::default());
    }
}
