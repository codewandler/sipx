//! The session kernel: one handle's worth of SIP, SDP policy, transaction and dialog state.
//!
//! Bytes in, fired timers in, monotonic time in, entropy in; bytes out, timer requests out, typed
//! events out. Nothing else crosses the boundary, and nothing inside reads a clock, touches a
//! socket or asks for randomness — see `docs/specs/browser-sdk.md` §3.1.
//!
//! The kernel never calls the host, so reentrancy is structurally impossible rather than merely
//! forbidden (§4.1). Every entry point completes all resulting work before it returns, appending
//! to the output queue as it goes; the host drains that queue afterwards.

mod call;
mod registration;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use bytes::Bytes;
use sipx_sip::transaction::{Dispatch, TuEvent};
use sipx_sip::{Message, Output, Reliability, Timer, Timers, TransactionKey, TransactionLayer};

use crate::bounds;
use crate::command::Command;
use crate::config::Config;
use crate::entropy::Pool;
use crate::error::{Error, Result};
use crate::event::{Event, Outcome, OutcomeError};
use crate::json::Writer;
use crate::output::{self, Record};

pub(crate) use call::Call;
pub(crate) use registration::Registration;

/// A timer the host is holding on the kernel's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scheduled {
    /// A `sipx-sip` transaction timer.
    Transaction { key: TransactionKey, timer: Timer },
    /// The registration refresh the kernel owes (§5.2: the kernel owns refreshes).
    RegistrationRefresh,
}

/// Monotonic counters, reported by the §4.11 snapshot.
#[derive(Debug, Default, Clone)]
pub(crate) struct Counters {
    pub(crate) parse_errors: u64,
    pub(crate) stale_timer_fires: u64,
    pub(crate) refused_incoming: u64,
    pub(crate) dropped_after_close: u64,
    /// One count per §4.10 code, indexed by that code's position in `Error::ALL`.
    pub(crate) rejections: BTreeMap<&'static str, u64>,
}

/// One kernel instance.
#[derive(Debug)]
pub(crate) struct Kernel {
    config: Config,
    /// The host's monotonic clock, as of the last state-advancing entry point.
    now_ms: u64,
    /// Whether any `now_ms` has been seen yet; the first call establishes the epoch.
    started: bool,
    entropy: Pool,
    outputs: output::Queue,
    transactions: TransactionLayer,
    /// Fresh timer ids, monotonically increasing and never reused within a handle (§4.5).
    next_timer_id: u64,
    timers: BTreeMap<u64, Scheduled>,
    registration: Registration,
    calls: BTreeMap<u32, Call>,
    next_call: u32,
    /// Which call owns which transaction. A short association list rather than a hash map:
    /// `TransactionKey` is `Hash` but not `Ord`, and §4.9 caps concurrent calls at eight, so the
    /// list never grows past a few dozen entries and a scan is cheaper than a hasher — and,
    /// unlike a `HashMap`, has no seed of its own on a target with no entropy source.
    transaction_calls: Vec<(TransactionKey, u32)>,
    /// Command ids the kernel has accepted but not yet completed (§5.1's uniqueness rule).
    unfinished: BTreeSet<u64>,
    counters: Counters,
    poison: Option<String>,
}

impl Kernel {
    /// Create a kernel from a parsed `BSDK-CFG` document.
    pub(crate) fn new(config: Config) -> Self {
        Self {
            config,
            now_ms: 0,
            started: false,
            entropy: Pool::default(),
            outputs: output::Queue::default(),
            // WSS and WS both deliver in order and without loss, so RFC 3261's retransmission
            // timers are off and its absorption timers fire at once (§17.1.1.2's reliable
            // branch). Getting this wrong would have the kernel retransmit into a stream that
            // already delivered the message.
            transactions: TransactionLayer::new(Timers::default()),
            next_timer_id: 1,
            timers: BTreeMap::new(),
            registration: Registration::default(),
            calls: BTreeMap::new(),
            next_call: 1,
            transaction_calls: Vec::new(),
            unfinished: BTreeSet::new(),
            counters: Counters::default(),
            poison: None,
        }
    }

    /// WSS and WS are both reliable, ordered transports.
    fn reliability() -> Reliability {
        Reliability::Reliable
    }

    /// Whether a prior internal fault has killed this instance.
    pub(crate) fn is_poisoned(&self) -> bool {
        self.poison.is_some()
    }

    /// Record an internal invariant failure.
    ///
    /// These are the situations that would panic natively, and in WebAssembly a panic is a trap
    /// (§8.1). Poisoning turns each of them into a value: the fatal `"error"` event is queued,
    /// every later entry point except `sipx_kernel_free` and `sipx_next_output` answers
    /// `E_POISONED`, and draining stays legal so the host can retrieve the event.
    fn poison(&mut self, reason: impl Into<String>) {
        let reason = reason.into();
        if self.poison.is_none() {
            self.poison = Some(reason.clone());
            // Straight onto the queue: `emit` would consult the poisoned flag that was just set.
            let record = Record::Event(
                Event::Fault {
                    fatal: true,
                    code: "internal",
                    reason,
                }
                .encode(),
            );
            let _ = self.outputs.push(record);
        }
    }

    /// Note that an entry point refused with a §4.10 code, for the snapshot's rejection counts.
    pub(crate) fn count_rejection(&mut self, error: Error) {
        *self.counters.rejections.entry(error.token()).or_insert(0) += 1;
    }

    /// Advance the clock, refusing a regression (§4.5).
    fn advance(&mut self, now_ms: u64) -> Result<()> {
        if self.started && now_ms < self.now_ms {
            return Err(Error::Time);
        }
        self.now_ms = now_ms;
        self.started = true;
        Ok(())
    }

    /// Queue one output record, poisoning the instance if the host has stopped draining.
    fn push(&mut self, record: Record) {
        if let Err(overflow) = self.outputs.push(record) {
            self.poison(overflow.reason());
        }
    }

    /// Queue one event, refusing to emit an oversize document (§4.9: truncation is forbidden,
    /// oversize is a defect).
    fn emit(&mut self, event: &Event) {
        let encoded = event.encode();
        if encoded.len() > bounds::MAX_EVENT {
            self.poison("an event document exceeded 32 KiB");
            return;
        }
        self.push(Record::Event(encoded));
    }

    /// Queue one SIP message as a `WIRE` record.
    fn wire(&mut self, message: &Message) {
        let mut bytes = Vec::new();
        message.write_to(&mut bytes);
        if bytes.len() > bounds::MAX_SIP_MESSAGE {
            self.poison("an outbound SIP message exceeded 64 KiB");
            return;
        }
        self.push(Record::Wire(bytes));
    }

    /// Take the next queued record.
    pub(crate) fn next_output(&mut self) -> Option<Record> {
        self.outputs.pop()
    }

    /// Ask the host for entropy when the pool has fallen below the low-water mark.
    fn ask_for_entropy_if_low(&mut self) {
        if self.entropy.below_low_water() {
            self.emit(&Event::NeedEntropy {
                min: bounds::ENTROPY_LOW_WATER as u64,
            });
        }
    }

    /// Complete a command with a successful outcome.
    fn succeed(&mut self, id: u64) {
        self.unfinished.remove(&id);
        self.emit(&Event::Outcome(Outcome { id, error: None }));
    }

    /// Complete a command with a typed refusal.
    fn refuse(&mut self, id: u64, code: &'static str, reason: impl Into<String>) {
        self.unfinished.remove(&id);
        self.emit(&Event::Outcome(Outcome {
            id,
            error: Some(OutcomeError::new(code, reason)),
        }));
    }

    // ---------------------------------------------------------------- entry points

    /// §4.3 `sipx_input_entropy`.
    pub(crate) fn input_entropy(&mut self, bytes: &[u8]) -> Result<()> {
        self.entropy.feed(bytes)?;
        self.ask_for_entropy_if_low();
        Ok(())
    }

    /// §4.3 `sipx_command`.
    pub(crate) fn command(&mut self, document: &[u8], now_ms: u64) -> Result<()> {
        // §9.5's `BSDK-NEG-7`: the bound is checked before JSON parsing, so an oversize document
        // never reaches the parser at all.
        if document.len() > bounds::MAX_COMMAND {
            return Err(Error::Bounds);
        }
        self.advance(now_ms)?;
        let command = Command::parse(document)?;
        if self.unfinished.contains(&command.id) {
            // §5.1: the id is "unique among that kernel's unfinished commands". Reusing a live
            // one would make the two `"outcome"` events indistinguishable.
            return Err(Error::State);
        }
        self.unfinished.insert(command.id);
        let result = self.dispatch(command.id, command.verb);
        if result.is_err() {
            self.unfinished.remove(&command.id);
        }
        result
    }

    /// §4.3 `sipx_input_bytes`: one received WebSocket message, which RFC 7118 §5 makes exactly
    /// one SIP message.
    pub(crate) fn input_bytes(&mut self, bytes: &[u8], now_ms: u64) -> Result<()> {
        if bytes.len() > bounds::MAX_SIP_MESSAGE {
            return Err(Error::Bounds);
        }
        self.advance(now_ms)?;

        let limits = sipx_sip::Limits::datagram();
        let Ok(message) = sipx_sip::parse_datagram(Bytes::copy_from_slice(bytes), &limits) else {
            // §4.10: hostile network input is a value, not a host-contract violation. It is
            // counted and dropped, and no event invents a call (`BSDK-NEG-13`).
            self.counters.parse_errors = self.counters.parse_errors.saturating_add(1);
            return Ok(());
        };

        let dispatch = self.transactions.receive(message, Self::reliability());
        match dispatch {
            Dispatch::Matched { key, outputs } | Dispatch::Created { key, outputs } => {
                self.drive(&key, outputs);
            }
            Dispatch::Unmatched(message) => self.unmatched(&message),
        }
        Ok(())
    }

    /// §4.3 `sipx_input_timer`.
    pub(crate) fn input_timer(&mut self, timer_id: u64, now_ms: u64) -> Result<()> {
        self.advance(now_ms)?;
        let Some(scheduled) = self.timers.remove(&timer_id) else {
            // §4.5: firing an unknown, cancelled or already fired id is not an error. Host races
            // are inevitable — a timer that fires while its cancellation is in flight is the
            // normal case, not a bug to report.
            self.counters.stale_timer_fires = self.counters.stale_timer_fires.saturating_add(1);
            return Ok(());
        };
        match scheduled {
            Scheduled::Transaction { key, timer } => {
                let outputs = self.transactions.on_timer(&key, timer);
                self.drive(&key, outputs);
            }
            Scheduled::RegistrationRefresh => self.refresh_registration(),
        }
        Ok(())
    }

    /// §4.3 `sipx_snapshot` (§4.11).
    ///
    /// Never contains credentials, entropy bytes or SIP message bodies.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        let mut writer = Writer::object();
        writer
            .number("v", 1)
            .string("registration", self.registration.state().as_str())
            .object_field("calls", |calls| {
                for (number, call) in &self.calls {
                    calls.string(&number.to_string(), call.state().as_str());
                }
            })
            .number("entropy", self.entropy.level() as u64)
            .number("pendingTimers", self.timers.len() as u64)
            .number("queuedOutputs", self.outputs.len() as u64)
            .object_field("counters", |counters| {
                counters
                    .number("parse_errors", self.counters.parse_errors)
                    .number("stale_timer_fires", self.counters.stale_timer_fires)
                    .number("refused_incoming", self.counters.refused_incoming)
                    .number("dropped_after_close", self.counters.dropped_after_close);
                for code in Error::ALL {
                    counters.number(
                        code.token(),
                        self.counters
                            .rejections
                            .get(code.token())
                            .copied()
                            .unwrap_or(0),
                    );
                }
            })
            .boolean("poisoned", self.poison.is_some());
        writer.finish().into_bytes()
    }

    /// Tear the kernel down (§6.5 step 4, §9.6's `BSDK-STATE-6`).
    ///
    /// Every pending timer is cancelled so the glue can clear the host-side handle it holds, and
    /// every queued record is discarded: nothing survives the free.
    pub(crate) fn shutdown(&mut self) -> Vec<Record> {
        let mut cancellations = Vec::new();
        for id in self.timers.keys().copied() {
            cancellations.push(Record::TimerCancel(id));
        }
        self.timers.clear();
        let dropped = self.outputs.drain_count();
        self.counters.dropped_after_close = self
            .counters
            .dropped_after_close
            .saturating_add(dropped as u64);
        self.calls.clear();
        self.entropy.zeroise();
        // The credential is zeroised for the same reason and with the same caveat: hygiene, and
        // documented in §8.3 as *not* a confidentiality boundary.
        self.config.password.clear();
        cancellations
    }

    // ---------------------------------------------------------------- timers

    /// Turn a transaction's relative `SetTimer` into the absolute `TIMER_SET` record §4.5 defines.
    fn set_timer(&mut self, scheduled: Scheduled, after: Duration) {
        if self.timers.len() >= bounds::MAX_PENDING_TIMERS {
            self.poison("pending timers exceeded 128");
            return;
        }
        let id = self.next_timer_id;
        self.next_timer_id = self.next_timer_id.saturating_add(1);
        let fire_at_ms = self
            .now_ms
            .saturating_add(after.as_millis().try_into().unwrap_or(u64::MAX));
        self.timers.insert(id, scheduled);
        self.push(Record::TimerSet { id, fire_at_ms });
    }

    /// Clear every host timer standing for `scheduled`.
    fn clear_timer(&mut self, scheduled: &Scheduled) {
        let ids: Vec<u64> = self
            .timers
            .iter()
            .filter(|(_, value)| *value == scheduled)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.timers.remove(&id);
            self.push(Record::TimerCancel(id));
        }
    }

    /// Clear every timer belonging to a transaction, which is what ending a call owes the host.
    fn clear_transaction_timers(&mut self, key: &TransactionKey) {
        let ids: Vec<u64> = self
            .timers
            .iter()
            .filter(|(_, value)| matches!(value, Scheduled::Transaction { key: owner, .. } if owner == key))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.timers.remove(&id);
            self.push(Record::TimerCancel(id));
        }
    }

    // ---------------------------------------------------------------- transaction plumbing

    /// Turn one transaction's outputs into records and TU work.
    ///
    /// `sipx-sip` guarantees the order — a `Send` always precedes the `SetTimer` that would
    /// retransmit it — and this preserves it, because §4.6's records are strictly FIFO and the
    /// host replays them in the order it drains them.
    fn drive(&mut self, key: &TransactionKey, outputs: Vec<Output>) {
        for output in outputs {
            match output {
                Output::Send(message) => self.wire(&message),
                Output::SetTimer { timer, after } => self.set_timer(
                    Scheduled::Transaction {
                        key: key.clone(),
                        timer,
                    },
                    after,
                ),
                Output::ClearTimer(timer) => self.clear_timer(&Scheduled::Transaction {
                    key: key.clone(),
                    timer,
                }),
                Output::ToTu(event) => self.deliver_to_tu(key, *event),
                Output::Terminated(_) => self.clear_transaction_timers(key),
            }
        }
    }

    /// Hand a transaction user event to whichever of registration or calls owns it.
    fn deliver_to_tu(&mut self, key: &TransactionKey, event: TuEvent) {
        match event {
            TuEvent::Response(response) => self.on_response(key, &response),
            TuEvent::Request(request) => self.on_request(key, &request),
            TuEvent::Ack(request) => self.on_ack(&request),
            // A transaction that ran out of time and one whose transport the host reported
            // failed end the same work the same way; only the diagnostic differs, and §5.3 has
            // no field for it.
            TuEvent::Timeout | TuEvent::TransportError => self.on_timeout(key),
        }
    }
}
