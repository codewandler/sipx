//! Structured event sequences for the transaction layer, and the invariants they are checked
//! against.
//!
//! `sipx-sip` has four fuzz targets and all of them stop at the parser. That covers the half of
//! the north star about adversarial *input*; the half about adversarial *timing* — what happens
//! when messages, application calls and fired timers interleave in an order nobody wrote a test
//! for — had nothing. This module is that instrument.
//!
//! # Programs, not bytes
//!
//! The fuzzer's bytes are decoded into a [`Program`]: a sequence of [`Event`]s over a small
//! vocabulary, each of which the harness turns into a *well-formed* SIP message or a call on
//! [`TransactionLayer`]. Reinterpreting the bytes as SIP instead would spend the whole budget
//! producing messages that do not parse, which is `S-4`'s fuzz targets again with extra steps.
//! Nothing here parses: messages are **built**, so every event reaches a state machine.
//!
//! The encoding is four bytes per event — opcode, target, and two operands — because libFuzzer
//! mutates bytes and a fixed-width record keeps a byte flip to a single field instead of
//! desynchronising the rest of the program. Every opcode byte is valid (it is taken modulo the
//! opcode count), so no input is wasted on a decode failure.
//!
//! # The oracle
//!
//! A panic-only oracle finds almost nothing in a state machine: the machines are total, so
//! almost any sequence "succeeds". What can go wrong is silent, so it is asserted explicitly —
//! see [`Invariant`]. Violations are returned as data rather than panicked on, so the same code
//! serves the fuzz target (which panics) and the regression tests (which assert).
//!
//! # Sans-IO
//!
//! No clock, no socket, no runtime, in keeping with `AGENTS.md`'s second non-negotiable. Time
//! enters only as [`Event::FireTimer`], which is precisely what makes the timing half fuzzable.

// The harness builds its fixtures from constants in this file. A fixture it cannot build is a
// bug in the harness and must fail loudly rather than quietly drive nothing — the same reason
// `AGENTS.md` lets test modules opt out of these lints.
#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use bytes::Bytes;

use sipx_sip::build::{RequestBuilder, ResponseBuilder};
use sipx_sip::transaction::{
    ClientState, Dispatch, Output, Reliability, ServerState, Timer, Timers, TransactionKey,
    TransactionLayer, TuEvent,
};
use sipx_sip::{HeaderName, Host, HostName, Message, Method, Request, Response, StatusCode, Uri};

// ------------------------------------------------------------------------------------------
// The vocabulary
// ------------------------------------------------------------------------------------------

/// How many conversations the vocabulary can name.
///
/// Deliberately tiny. A large slot space would give almost every event a fresh transaction and
/// the interesting behaviour — retransmission absorption, matching, a timer arriving for the
/// transaction that has just gone — lives in collisions.
pub const SLOTS: u8 = 4;

/// Slots at or above this index use a branch with no RFC 3261 magic cookie, so they exercise
/// the RFC 2543 matching fallback of `TransactionKey::Legacy`.
pub const FIRST_LEGACY_SLOT: u8 = 2;

/// Status codes the vocabulary can produce: provisional, 2xx, and non-2xx finals across the
/// ranges the state tables branch on.
pub const STATUSES: [u16; 8] = [100, 180, 200, 302, 404, 486, 500, 603];

/// To-tags the vocabulary can produce. Three, because two are needed for a fork answering
/// twice and the third distinguishes a legacy key.
pub const TAGS: [&str; 3] = ["ta", "tb", "tc"];

/// Every timer of RFC 3261 §17 Table 4, plus the unlettered 200 ms one.
pub const TIMERS: [Timer; 13] = [
    Timer::A,
    Timer::B,
    Timer::D,
    Timer::E,
    Timer::F,
    Timer::G,
    Timer::H,
    Timer::I,
    Timer::J,
    Timer::K,
    Timer::L,
    Timer::M,
    Timer::Trying100,
];

/// The methods the vocabulary can produce.
///
/// `ACK` and `CANCEL` earn their place: §17.2.3 folds `ACK` onto the `INVITE` it acknowledges
/// and pointedly does not fold `CANCEL`, and getting either wrong is a call that never hangs up.
fn method(index: u8) -> Method {
    match index % METHOD_COUNT {
        0 => Method::Invite,
        1 => Method::Ack,
        2 => Method::Bye,
        3 => Method::Cancel,
        4 => Method::Register,
        _ => Method::Options,
    }
}

/// How many methods [`method`] can return.
const METHOD_COUNT: u8 = 6;

/// The table sizes, as the `u8` the decoder needs. Checked against the tables below, because a
/// count that drifts from its table silently narrows what the fuzzer can produce.
const STATUS_COUNT: u8 = 8;
const TAG_COUNT: u8 = 3;
const TIMER_COUNT: u8 = 13;

const _: () = {
    assert!(STATUS_COUNT as usize == STATUSES.len());
    assert!(TAG_COUNT as usize == TAGS.len());
    assert!(TIMER_COUNT as usize == TIMERS.len());
};

/// Distinct transaction keys the vocabulary can produce per branch space: `ACK` folds onto
/// `INVITE`, so six methods name five keys.
const FOLDED_METHODS: usize = 5;

/// The most transactions that can be in flight at once, whatever the program does.
///
/// Two branch spaces (client and server), [`SLOTS`] branches in each, and five keys per branch
/// — six methods, of which `ACK` folds onto `INVITE`. This is the bound behind
/// [`Invariant::StoreGrowth`]: it depends on the
/// *vocabulary* and not on the program's length, which is what "does not grow without bound
/// over a bounded sequence" has to mean if it is to mean anything.
pub const MAX_LIVE_TRANSACTIONS: usize = 2 * SLOTS as usize * FOLDED_METHODS;

/// Which branch space a message belongs to.
///
/// Requests the harness *receives* and responses it *sends* address server transactions;
/// requests it sends and responses it receives address client ones. Giving each role its own
/// branch keeps one key from naming a client and a server transaction at the same time, which
/// would make every oracle below ambiguous about which machine it was talking about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Space {
    Client,
    Server,
}

impl Space {
    fn letter(self) -> char {
        match self {
            Self::Client => 'c',
            Self::Server => 's',
        }
    }
}

/// One step of a decoded program.
///
/// The three kinds the story names — a message arriving, the application asking for something,
/// a timer firing — plus the two things a driver reports that no message can express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The application starts a client transaction.
    SendRequest {
        /// Which conversation, and therefore which branch.
        slot: u8,
        /// Index into the method vocabulary.
        method: u8,
        /// Whether the transport retransmits, which decides half the timer behaviour.
        reliable: bool,
    },
    /// A request arrives from the network.
    ReceiveRequest {
        /// Which conversation, and therefore which branch.
        slot: u8,
        /// Index into the method vocabulary.
        method: u8,
        /// Whether the transport retransmits.
        reliable: bool,
        /// Index into [`TAGS`], for the `To` tag.
        to_tag: u8,
    },
    /// A response arrives from the network, addressed at a client transaction.
    ReceiveResponse {
        /// Which conversation, and therefore which branch.
        slot: u8,
        /// Index into the method vocabulary, used for the `CSeq` method.
        method: u8,
        /// Index into [`STATUSES`].
        status: u8,
        /// Index into [`TAGS`], for the `To` tag.
        to_tag: u8,
    },
    /// The application answers a server transaction.
    SendResponse {
        /// Which conversation, and therefore which branch.
        slot: u8,
        /// Index into the method vocabulary.
        method: u8,
        /// Index into [`STATUSES`].
        status: u8,
        /// Index into [`TAGS`], for the `To` tag.
        to_tag: u8,
    },
    /// A timer the driver holds fires.
    ///
    /// The key is chosen from every key the layer has ever created, *including ones whose
    /// transaction is gone* — a driver's timer wheel cannot cancel atomically, so a timer
    /// firing into a retired machine is the normal case rather than the exotic one, and
    /// [`Invariant::TimerForRemovedKey`] is what says it must be harmless.
    FireTimer {
        /// Index into the keys the layer has created.
        key: u8,
        /// Index into the timers.
        timer: u8,
        /// Choose from all thirteen timers rather than the ones currently armed. Off by
        /// default so the budget mostly goes on timers a real driver would have set, and on
        /// so that stray and never-armed ones are reachable too.
        any: bool,
    },
    /// The transport reports it could not deliver for a transaction.
    TransportError {
        /// Index into the keys the layer has created.
        key: u8,
    },
    /// The driver gives up on a server transaction the application never answered.
    Abandon {
        /// Index into the keys the layer has created.
        key: u8,
    },
}

/// How many opcodes the decoder recognises.
const OPCODES: u8 = 7;

/// Bytes per encoded event.
const RECORD: usize = 4;

/// A decoded sequence of events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Program {
    /// The events, in the order they are driven.
    pub events: Vec<Event>,
}

impl Program {
    /// Decode a fuzzer input.
    ///
    /// Total: every byte string is a program. A trailing partial record is ignored, which is
    /// what lets libFuzzer shrink an input a byte at a time without the tail turning into
    /// noise.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Self {
        let events = bytes
            .chunks_exact(RECORD)
            .filter_map(|chunk| match chunk {
                &[op, target, a, b] => Some(decode_event(op, target, a, b)),
                _ => None,
            })
            .collect();
        Self { events }
    }

    /// Encode a program back to the bytes that decode to it.
    ///
    /// The inverse of [`Program::decode`] on canonical inputs, which is what lets a seed
    /// written as Rust be committed as a corpus file.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.events.len() * RECORD);
        for event in &self.events {
            out.extend_from_slice(&encode_event(*event));
        }
        out
    }
}

fn decode_event(op: u8, target: u8, a: u8, b: u8) -> Event {
    match op % OPCODES {
        0 => Event::SendRequest {
            slot: target % SLOTS,
            method: a % METHOD_COUNT,
            reliable: b & 1 != 0,
        },
        1 => Event::ReceiveRequest {
            slot: target % SLOTS,
            method: a % METHOD_COUNT,
            reliable: b & 1 != 0,
            to_tag: (b >> 4) % TAG_COUNT,
        },
        2 => Event::ReceiveResponse {
            slot: target % SLOTS,
            method: a % METHOD_COUNT,
            status: b % STATUS_COUNT,
            to_tag: (b >> 4) % TAG_COUNT,
        },
        3 => Event::SendResponse {
            slot: target % SLOTS,
            method: a % METHOD_COUNT,
            status: b % STATUS_COUNT,
            to_tag: (b >> 4) % TAG_COUNT,
        },
        4 => Event::FireTimer {
            key: target,
            timer: a % TIMER_COUNT,
            any: b & 1 != 0,
        },
        5 => Event::TransportError { key: target },
        _ => Event::Abandon { key: target },
    }
}

fn encode_event(event: Event) -> [u8; RECORD] {
    match event {
        Event::SendRequest {
            slot,
            method,
            reliable,
        } => [0, slot, method, u8::from(reliable)],
        Event::ReceiveRequest {
            slot,
            method,
            reliable,
            to_tag,
        } => [1, slot, method, u8::from(reliable) | (to_tag << 4)],
        Event::ReceiveResponse {
            slot,
            method,
            status,
            to_tag,
        } => [2, slot, method, status | (to_tag << 4)],
        Event::SendResponse {
            slot,
            method,
            status,
            to_tag,
        } => [3, slot, method, status | (to_tag << 4)],
        Event::FireTimer { key, timer, any } => [4, key, timer, u8::from(any)],
        Event::TransportError { key } => [5, key, 0, 0],
        Event::Abandon { key } => [6, key, 0, 0],
    }
}

// ------------------------------------------------------------------------------------------
// The oracle
// ------------------------------------------------------------------------------------------

/// The properties a transaction layer can break without panicking.
///
/// Each is something that would show up in production as a slow leak, a wedged call or a
/// response answering the wrong request — never as a crash, which is why a panic-only fuzz
/// target would run for a week and report nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invariant {
    /// No transaction outlives its terminal state.
    ///
    /// When a machine emits `Output::Terminated` the layer must have dropped it, and no
    /// observable state may ever read `Terminated` — a transaction that reports its own death
    /// and stays in the store is the leak this whole layer exists to avoid. Arming a timer in
    /// the same batch that retires the transaction is the same fault seen from the other side.
    OutlivedTermination,

    /// No timer fires for a key that has been removed.
    ///
    /// A driver's timer wheel cannot cancel atomically: `ClearTimer` and the fired callback
    /// race, and the transaction may be gone by the time the timer arrives. Firing into that
    /// gap must produce nothing and above all must not resurrect the transaction. This is the
    /// design record's "timer IDs must survive transaction termination without firing into a
    /// dead machine", stated as something a test can fail.
    TimerForRemovedKey,

    /// The store does not grow without bound over a bounded sequence.
    ///
    /// Two claims. In flight, no more transactions than the vocabulary has keys — bounded by
    /// [`MAX_LIVE_TRANSACTIONS`], not by how long the program is. And after every timer the
    /// layer asked for has been driven to quiescence, the only transactions left are the ones
    /// legitimately waiting on the application, because those are the only ones RFC 3261 gives
    /// no timer of their own.
    StoreGrowth,

    /// No state is reachable that the RFC 3261 §17 tables, as amended by RFC 6026, do not name.
    ///
    /// `ClientState` and `ServerState` each cover two machines, so the enum alone proves
    /// nothing: an INVITE client transaction reaching `Trying` — the *non*-INVITE waiting
    /// state — type-checks and is meaningless. The legal set is per machine, taken from
    /// `docs/specs/sip-transaction.md` §4.
    UnnamedState,

    /// A response to a request the layer sent must reach the transaction that sent it.
    ///
    /// Half of what the layer does is §17.1.3 matching, and a response that matches nothing is
    /// a call that hangs until Timer F rather than a crash. Checked only where the harness
    /// knows the answer: the response it builds carries the branch and `CSeq` method of the
    /// request that created the transaction, so it must match.
    UnroutableResponse,
}

/// One invariant broken, and where.
#[derive(Debug, Clone)]
pub struct Violation {
    /// Which step of the program, or [`usize::MAX`] for the quiescence check that follows it.
    pub step: usize,
    /// Which invariant.
    pub invariant: Invariant,
    /// What was seen, in enough detail to name a defect from.
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "step {}: {:?}: {}",
            self.step, self.invariant, self.detail
        )
    }
}

/// What driving a program produced.
#[derive(Debug, Clone)]
pub struct Run {
    /// One line per step: the event, what the layer did with it, and the store's size.
    ///
    /// This is the artefact a crash report is read from, and the thing the replay test pins:
    /// a harness whose trace is not a function of its input reports crashes nobody can
    /// reproduce.
    pub trace: Vec<String>,
    /// Every invariant broken, in the order they were noticed.
    pub violations: Vec<Violation>,
}

// ------------------------------------------------------------------------------------------
// The driver
// ------------------------------------------------------------------------------------------

/// What the harness remembers about one key the layer has created.
#[derive(Debug)]
struct Tracked {
    key: TransactionKey,
    space: Space,
    /// Whether this is one of the INVITE machines, which decides the legal state set.
    invite: bool,
    /// The timers the layer has asked for and not cleared. A `Vec` rather than a set because
    /// `Timer` is not `Ord` and there are never more than three.
    armed: Vec<Timer>,
    /// `c0/INVITE`, for the trace.
    label: String,
}

/// How many rounds the quiescence check will drive timers for before giving up.
///
/// Generous: a Timer A that doubles forever is retired by Timer B in the same round, so a
/// transaction that survives this many rounds is not slow, it is stuck.
const DRAIN_ROUNDS: usize = 64;

struct Driver {
    layer: TransactionLayer,
    tracked: Vec<Tracked>,
    trace: Vec<String>,
    violations: Vec<Violation>,
    /// The defects this run steps over.
    ///
    /// Always empty while [`Known`] is uninhabited, hence unread — kept, with the plumbing in
    /// [`run_with`], because rebuilding it is what a campaign blocked behind a fresh defect
    /// cannot afford to stop and do.
    #[allow(dead_code)]
    suppressed: Vec<Known>,
}

/// A defect the campaign knows about and steps over.
///
/// A fuzzer that crashes on the same open bug every time reports that bug forever and nothing
/// behind it. Suppressing one is therefore necessary and also dangerous — a suppression nobody
/// removes hides its whole class permanently — so each is named here, documented on its variant,
/// and paired with an ignored regression test that fails until it is fixed. [`run_strict`]
/// suppresses nothing, which is how those tests see the defect.
///
/// There are none at the moment: `LegacyClientResponseMatching` — a response to an RFC 2543
/// client transaction matching nothing, because `TransactionKey::from_sent_request` derived the
/// client key by §17.2.3's server rules — was the first and so far only entry, and `S-26` fixed
/// it. The type stays because the next campaign will want it, and an empty enum is the honest
/// way to say the campaign is currently suppressing nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Known {}

/// Every defect [`run`] steps over.
pub const KNOWN_DEFECTS: [Known; 0] = [];

/// Drive a program and check it, stepping over [`KNOWN_DEFECTS`].
///
/// What the fuzz target calls. Never panics on a violation: it returns them, so the target can
/// panic with a report and a regression test can assert on the list.
#[must_use]
pub fn run(program: &Program) -> Run {
    run_with(program, &KNOWN_DEFECTS)
}

/// Drive a program and check it, suppressing nothing.
///
/// What the regression tests for the known defects call, so that "known" never quietly becomes
/// "invisible".
#[must_use]
pub fn run_strict(program: &Program) -> Run {
    run_with(program, &[])
}

/// Drive a program and check it, suppressing the named defects.
#[must_use]
pub fn run_with(program: &Program, suppressed: &[Known]) -> Run {
    let mut driver = Driver {
        layer: TransactionLayer::new(Timers::default()),
        tracked: Vec::new(),
        trace: Vec::new(),
        violations: Vec::new(),
        suppressed: suppressed.to_vec(),
    };

    for (step, event) in program.events.iter().enumerate() {
        driver.step(step, *event);
        driver.check_states(step);
        driver.check_bound(step);
    }
    driver.drive_to_quiescence();

    Run {
        trace: driver.trace,
        violations: driver.violations,
    }
}

impl Driver {
    fn step(&mut self, step: usize, event: Event) {
        match event {
            Event::SendRequest {
                slot,
                method: m,
                reliable,
            } => self.send_request(step, slot, m, reliable),
            Event::ReceiveRequest {
                slot,
                method: m,
                reliable,
                to_tag,
            } => self.receive_request(step, slot, m, reliable, to_tag),
            Event::ReceiveResponse {
                slot,
                method: m,
                status,
                to_tag,
            } => self.receive_response(step, slot, m, status, to_tag),
            Event::SendResponse {
                slot,
                method: m,
                status,
                to_tag,
            } => self.send_response(step, slot, m, status, to_tag),
            Event::FireTimer { key, timer, any } => self.fire_timer(step, key, timer, any),
            Event::TransportError { key } => self.transport_error(step, key),
            Event::Abandon { key } => self.abandon(step, key),
        }
    }

    // -- the events ------------------------------------------------------------------------

    fn send_request(&mut self, step: usize, slot: u8, method_index: u8, reliable: bool) {
        let m = method(method_index);
        let request = build_request(Space::Client, slot, &m, None);
        let reliability = reliability(reliable);
        let Some((key, outputs)) = self.layer.send_request(request, reliability) else {
            self.record(
                step,
                &format!("SendRequest({}) no key", label(slot, &m)),
                "",
            );
            return;
        };
        let index = self.track(key, Space::Client, slot, &m, m == Method::Invite);
        let outcome = format!(
            "created client {} {}",
            self.tracked
                .get(index)
                .map_or_else(String::new, |t| t.label.clone()),
            self.apply(index, &outputs)
        );
        self.record(
            step,
            &format!(
                "SendRequest({} {})",
                label(slot, &m),
                transport_name(reliable)
            ),
            &outcome,
        );
        self.check_termination(step, index, &outputs);
    }

    fn receive_request(
        &mut self,
        step: usize,
        slot: u8,
        method_index: u8,
        reliable: bool,
        to_tag: u8,
    ) {
        let m = method(method_index);
        let request = build_request(Space::Server, slot, &m, Some(to_tag));
        let dispatch = self
            .layer
            .receive(Message::Request(request), reliability(reliable));
        let outcome = self.absorb(step, dispatch, Space::Server, slot, &m);
        self.record(
            step,
            &format!(
                "ReceiveRequest({} {})",
                label(slot, &m),
                transport_name(reliable)
            ),
            &outcome,
        );
    }

    fn receive_response(
        &mut self,
        step: usize,
        slot: u8,
        method_index: u8,
        status_index: u8,
        to_tag: u8,
    ) {
        let m = method(method_index);
        let status = status(status_index);
        // The response carries the branch and CSeq of the request a `SendRequest` for this slot
        // and method would have produced, which is what makes the matching claim below checkable.
        let request = build_request(Space::Client, slot, &m, None);
        let expected = TransactionKey::from_sent_request(&request);
        let live = expected
            .as_ref()
            .and_then(|key| self.layer.client_state(key))
            .is_some();

        let response = build_response(&request, status, to_tag);
        let dispatch = self
            .layer
            .receive(Message::Response(response), Reliability::Unreliable);
        let matched = matches!(dispatch, Dispatch::Matched { .. });
        let outcome = self.absorb(step, dispatch, Space::Client, slot, &m);

        // The property holds on every slot, legacy ones included: §17.1.3 keys a client
        // transaction on the branch and the `CSeq` method, which a response carries whether or
        // not the branch has a magic cookie. Legacy slots used to be excluded here, under
        // `Known::LegacyClientResponseMatching`, until `S-26` gave the client key its own
        // derivation.
        //
        // A new suppression is consulted here: `if live && !matched && !self.suppressed.contains(…)`.
        if live && !matched {
            self.violate(
                step,
                Invariant::UnroutableResponse,
                format!(
                    "a {} response for {} matched nothing, but its client transaction is live",
                    status.code(),
                    label(slot, &m)
                ),
            );
        }

        self.record(
            step,
            &format!("ReceiveResponse({} {})", label(slot, &m), status.code()),
            &outcome,
        );
    }

    fn send_response(
        &mut self,
        step: usize,
        slot: u8,
        method_index: u8,
        status_index: u8,
        to_tag: u8,
    ) {
        let m = method(method_index);
        let status = status(status_index);
        let request = build_request(Space::Server, slot, &m, Some(to_tag));
        let Some(key) = TransactionKey::from_request(&request) else {
            self.record(
                step,
                &format!("SendResponse({}) no key", label(slot, &m)),
                "",
            );
            return;
        };
        let response = build_response(&request, status, to_tag);
        let outputs = self.layer.send_response(&key, response);
        let outcome = match self.index_of(&key) {
            Some(index) => {
                let label = self
                    .tracked
                    .get(index)
                    .map_or_else(String::new, |t| t.label.clone());
                format!("{label} {}", self.apply(index, &outputs))
            }
            None => format!("no transaction store={}", store(&self.layer)),
        };
        self.record(
            step,
            &format!("SendResponse({} {})", label(slot, &m), status.code()),
            &outcome,
        );
        if let Some(index) = self.index_of(&key) {
            self.check_termination(step, index, &outputs);
        }
    }

    fn fire_timer(&mut self, step: usize, key_index: u8, timer_index: u8, any: bool) {
        if self.tracked.is_empty() {
            self.record(step, "FireTimer(no keys)", "");
            return;
        }
        let index = key_index as usize % self.tracked.len();
        let Some(entry) = self.tracked.get(index) else {
            return;
        };
        let armed = entry.armed.clone();
        let key = entry.key.clone();
        let label = entry.label.clone();
        let space = entry.space;

        let timer = if any || armed.is_empty() {
            *TIMERS
                .get(timer_index as usize % TIMERS.len())
                .expect("the timer index is taken modulo the table")
        } else {
            *armed
                .get(timer_index as usize % armed.len())
                .expect("the armed index is taken modulo a non-empty list")
        };

        let live = self.is_live(space, &key);
        let before = self.layer.len();
        // A timer that fires is spent. Whatever the machine wants next it re-arms in its outputs.
        if let Some(entry) = self.tracked.get_mut(index) {
            entry.armed.retain(|t| *t != timer);
        }
        let outputs = self.layer.on_timer(&key, timer);
        let after = self.layer.len();

        if !live && (!outputs.is_empty() || before != after) {
            self.violate(
                step,
                Invariant::TimerForRemovedKey,
                format!(
                    "timer {timer:?} for {label}, whose transaction is gone, produced \
                     {} output(s) and left the store at {after:?} (was {before:?})",
                    outputs.len()
                ),
            );
        }

        let outcome = format!("{label} {}", self.apply(index, &outputs));
        self.record(
            step,
            &format!(
                "FireTimer({label} {timer:?}{})",
                if live { "" } else { " stale" }
            ),
            &outcome,
        );
        self.check_termination(step, index, &outputs);
    }

    fn transport_error(&mut self, step: usize, key_index: u8) {
        if self.tracked.is_empty() {
            self.record(step, "TransportError(no keys)", "");
            return;
        }
        let index = key_index as usize % self.tracked.len();
        let Some(entry) = self.tracked.get(index) else {
            return;
        };
        let key = entry.key.clone();
        let label = entry.label.clone();
        let outputs = self.layer.on_transport_error(&key);
        let outcome = format!("{label} {}", self.apply(index, &outputs));
        self.record(step, &format!("TransportError({label})"), &outcome);
        self.check_termination(step, index, &outputs);
    }

    fn abandon(&mut self, step: usize, key_index: u8) {
        if self.tracked.is_empty() {
            self.record(step, "Abandon(no keys)", "");
            return;
        }
        let index = key_index as usize % self.tracked.len();
        let Some(entry) = self.tracked.get(index) else {
            return;
        };
        let key = entry.key.clone();
        let label = entry.label.clone();
        let gone = self.layer.abandon(&key);
        if let Some(entry) = self.tracked.get_mut(index).filter(|_| gone) {
            entry.armed.clear();
        }
        self.record(
            step,
            &format!("Abandon({label})"),
            &format!(
                "{} store={}",
                if gone { "dropped" } else { "absent" },
                store(&self.layer)
            ),
        );
    }

    // -- bookkeeping -----------------------------------------------------------------------

    /// Fold a dispatch into the model, returning its summary for the trace.
    fn absorb(
        &mut self,
        step: usize,
        dispatch: Dispatch,
        space: Space,
        slot: u8,
        m: &Method,
    ) -> String {
        match dispatch {
            Dispatch::Created { key, outputs } => {
                let index = self.track(key, space, slot, m, m == &Method::Invite);
                let label = self
                    .tracked
                    .get(index)
                    .map_or_else(String::new, |t| t.label.clone());
                let summary = format!("created server {label} {}", self.apply(index, &outputs));
                self.check_termination(step, index, &outputs);
                summary
            }
            Dispatch::Matched { key, outputs } => match self.index_of(&key) {
                Some(index) => {
                    let label = self
                        .tracked
                        .get(index)
                        .map_or_else(String::new, |t| t.label.clone());
                    let summary = format!("matched {label} {}", self.apply(index, &outputs));
                    self.check_termination(step, index, &outputs);
                    summary
                }
                None => format!("matched untracked {}", summarise(&outputs)),
            },
            Dispatch::Unmatched(_) => format!("unmatched store={}", store(&self.layer)),
        }
    }

    /// Record a key the layer has created, returning its index.
    ///
    /// A key can be created more than once — the layer's stores are maps, so a second
    /// transaction under the same key replaces the first — in which case the entry is reset
    /// rather than duplicated.
    fn track(
        &mut self,
        key: TransactionKey,
        space: Space,
        slot: u8,
        m: &Method,
        invite: bool,
    ) -> usize {
        if let Some(index) = self.index_of(&key) {
            if let Some(entry) = self.tracked.get_mut(index) {
                entry.invite = invite;
                entry.armed.clear();
            }
            return index;
        }
        self.tracked.push(Tracked {
            key,
            space,
            invite,
            armed: Vec::new(),
            label: format!("{}{slot}/{}", space.letter(), method_name(m)),
        });
        self.tracked.len() - 1
    }

    fn index_of(&self, key: &TransactionKey) -> Option<usize> {
        self.tracked.iter().position(|t| &t.key == key)
    }

    /// Fold a batch of outputs into the armed-timer model and summarise it for the trace.
    fn apply(&mut self, index: usize, outputs: &[Output]) -> String {
        if let Some(entry) = self.tracked.get_mut(index) {
            for output in outputs {
                match output {
                    Output::SetTimer { timer, .. } => {
                        if !entry.armed.contains(timer) {
                            entry.armed.push(*timer);
                        }
                    }
                    Output::ClearTimer(timer) => entry.armed.retain(|t| t != timer),
                    Output::Terminated(_) => entry.armed.clear(),
                    Output::Send(_) | Output::ToTu(_) => {}
                }
            }
        }
        let state =
            self.tracked
                .get(index)
                .map_or_else(String::new, |entry| match self.state_of(entry) {
                    Some(state) => format!(" state={state}"),
                    None => String::new(),
                });
        format!("{}{state} store={}", summarise(outputs), store(&self.layer))
    }

    // -- the invariants --------------------------------------------------------------------

    /// No transaction outlives its terminal state.
    fn check_termination(&mut self, step: usize, index: usize, outputs: &[Output]) {
        let Some(entry) = self.tracked.get(index) else {
            return;
        };
        if !outputs.iter().any(|o| matches!(o, Output::Terminated(_))) {
            return;
        }
        let label = entry.label.clone();
        let still_there = self.state_of(entry).is_some();
        if still_there {
            self.violate(
                step,
                Invariant::OutlivedTermination,
                format!("{label} reported Terminated and is still in the store"),
            );
        }
        if let Some(timer) = outputs.iter().find_map(|o| match o {
            Output::SetTimer { timer, .. } => Some(*timer),
            _ => None,
        }) {
            self.violate(
                step,
                Invariant::OutlivedTermination,
                format!("{label} armed timer {timer:?} in the batch that retired it"),
            );
        }
    }

    /// No state is reachable that the §17 tables do not name for that machine.
    ///
    /// The two non-INVITE arms name the same three states today and are kept apart anyway: each
    /// is a different table in `docs/specs/sip-transaction.md`, and collapsing them would mean
    /// the next amendment to one of them silently changed the other.
    #[allow(clippy::match_same_arms)]
    fn check_states(&mut self, step: usize) {
        let mut found = Vec::new();
        for entry in &self.tracked {
            let Some(state) = self.state_of(entry) else {
                continue;
            };
            let legal = match (entry.space, entry.invite) {
                // §4.1: the INVITE client machine never waits in Trying — that is the
                // non-INVITE machine's state — and never reaches Terminated observably.
                (Space::Client, true) => {
                    ["Calling", "Proceeding", "Completed", "Accepted"].as_slice()
                }
                // §4.2: no Calling, and no Accepted — RFC 6026 adds Accepted to the INVITE
                // machines only, because only a 2xx to an INVITE can arrive twice.
                (Space::Client, false) => ["Trying", "Proceeding", "Completed"].as_slice(),
                // §4.3: an INVITE server transaction starts in Proceeding, never Trying.
                (Space::Server, true) => {
                    ["Proceeding", "Completed", "Confirmed", "Accepted"].as_slice()
                }
                // §4.4: no Confirmed and no Accepted; there is no ACK to wait for.
                (Space::Server, false) => ["Trying", "Proceeding", "Completed"].as_slice(),
            };
            if !legal.contains(&state.as_str()) {
                found.push(format!(
                    "{} is in {state}, which §17 does not name for {} machine",
                    entry.label,
                    machine_name(entry.space, entry.invite)
                ));
            }
        }
        for detail in found {
            self.violate(step, Invariant::UnnamedState, detail);
        }
    }

    /// The store is bounded by the vocabulary, not by the program.
    fn check_bound(&mut self, step: usize) {
        let (client, server) = self.layer.len();
        let live = client + server;
        if live > MAX_LIVE_TRANSACTIONS {
            self.violate(
                step,
                Invariant::StoreGrowth,
                format!(
                    "{live} transactions in flight, but the vocabulary names at most \
                     {MAX_LIVE_TRANSACTIONS} keys"
                ),
            );
        }
        if live > self.tracked.len() {
            self.violate(
                step,
                Invariant::StoreGrowth,
                format!(
                    "{live} transactions in flight, but the layer has only reported \
                     {} keys",
                    self.tracked.len()
                ),
            );
        }
    }

    /// Drive every timer the layer asked for until nothing is left to fire, then say what
    /// survived.
    ///
    /// The only transactions RFC 3261 leaves without a timer of their own are the ones waiting
    /// on the application: a server transaction the TU has not answered (§17.2.2, and §17.2.1
    /// once the 100 has gone out), and an INVITE client transaction in Proceeding, whose
    /// Timer B is cancelled by the first provisional on purpose (§17.1.1.2). Anything else
    /// still in the store after quiescence is a transaction nothing will ever retire, which is
    /// the slow quiet outage this invariant is about.
    fn drive_to_quiescence(&mut self) {
        for _ in 0..DRAIN_ROUNDS {
            let mut fired = false;
            for index in 0..self.tracked.len() {
                let Some(entry) = self.tracked.get(index) else {
                    continue;
                };
                if self.state_of(entry).is_none() {
                    continue;
                }
                let key = entry.key.clone();
                let armed = entry.armed.clone();
                for timer in armed {
                    if let Some(entry) = self.tracked.get_mut(index) {
                        entry.armed.retain(|t| *t != timer);
                    }
                    let outputs = self.layer.on_timer(&key, timer);
                    let _ = self.apply(index, &outputs);
                    fired = true;
                }
            }
            if !fired {
                break;
            }
        }

        let mut found = Vec::new();
        for entry in &self.tracked {
            let Some(state) = self.state_of(entry) else {
                continue;
            };
            let waiting_on_the_application = match entry.space {
                Space::Server => state == "Trying" || state == "Proceeding",
                Space::Client => entry.invite && state == "Proceeding",
            };
            if !waiting_on_the_application {
                found.push(format!(
                    "{} is still in {state} with every timer fired; nothing will retire it",
                    entry.label
                ));
            }
        }
        for detail in found {
            self.violate(usize::MAX, Invariant::StoreGrowth, detail);
        }
        self.trace.push(format!(
            "quiescent store={} live={:?}",
            store(&self.layer),
            self.tracked
                .iter()
                .filter_map(|entry| self.state_of(entry).map(|s| format!("{}={s}", entry.label)))
                .collect::<Vec<_>>()
        ));
    }

    // -- helpers ---------------------------------------------------------------------------

    fn state_of(&self, entry: &Tracked) -> Option<String> {
        match entry.space {
            Space::Client => self.layer.client_state(&entry.key).map(client_state_name),
            Space::Server => self.layer.server_state(&entry.key).map(server_state_name),
        }
    }

    fn is_live(&self, space: Space, key: &TransactionKey) -> bool {
        match space {
            Space::Client => self.layer.client_state(key).is_some(),
            Space::Server => self.layer.server_state(key).is_some(),
        }
    }

    fn record(&mut self, step: usize, event: &str, outcome: &str) {
        let mut line = String::new();
        let _ = write!(line, "{step:03} {event}");
        if !outcome.is_empty() {
            let _ = write!(line, " | {outcome}");
        }
        self.trace.push(line);
    }

    fn violate(&mut self, step: usize, invariant: Invariant, detail: String) {
        self.violations.push(Violation {
            step,
            invariant,
            detail,
        });
    }
}

// ------------------------------------------------------------------------------------------
// Building the messages
// ------------------------------------------------------------------------------------------

fn reliability(reliable: bool) -> Reliability {
    if reliable {
        Reliability::Reliable
    } else {
        Reliability::Unreliable
    }
}

fn transport_name(reliable: bool) -> &'static str {
    if reliable { "tcp" } else { "udp" }
}

fn status(index: u8) -> StatusCode {
    let code = *STATUSES
        .get(index as usize % STATUSES.len())
        .expect("the status index is taken modulo the table");
    StatusCode::new(code).expect("the status table holds valid codes")
}

/// The branch a slot's messages carry.
///
/// Slots below [`FIRST_LEGACY_SLOT`] carry the RFC 3261 magic cookie; the rest deliberately do
/// not, so the RFC 2543 fallback in `TransactionKey::Legacy` is reachable — those senders are
/// still on the public internet, and the RFC 4475 corpus has one.
fn branch(space: Space, slot: u8) -> String {
    if slot < FIRST_LEGACY_SLOT {
        format!("z9hG4bK-fz{}{slot}", space.letter())
    } else {
        format!("fz{}{slot}-rfc2543", space.letter())
    }
}

fn build_request(space: Space, slot: u8, m: &Method, to_tag: Option<u8>) -> Request {
    let uri = Uri::sip(Host::Name(
        HostName::new("example.com").expect("a valid host"),
    ));
    let to = match to_tag.and_then(|index| TAGS.get(index as usize % TAGS.len())) {
        Some(tag) => format!("<sip:callee@example.com>;tag={tag}"),
        None => "<sip:callee@example.com>".to_owned(),
    };
    RequestBuilder::new(m.clone(), uri)
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/UDP host.example.net;branch={}",
                branch(space, slot)
            )),
        )
        .expect("a valid Via")
        .header(
            HeaderName::From,
            Bytes::from(format!("<sip:caller@example.net>;tag=f{slot}")),
        )
        .expect("a valid From")
        .header(HeaderName::To, Bytes::from(to))
        .expect("a valid To")
        .header(
            HeaderName::CallId,
            Bytes::from(format!("fuzz-{slot}@example.net")),
        )
        .expect("a valid Call-ID")
        .cseq(1, m)
        .expect("a valid CSeq")
        .max_forwards(70)
        .build()
}

fn build_response(request: &Request, status: StatusCode, to_tag: u8) -> Response {
    let mut builder = ResponseBuilder::to_request(request, status, "Sequence")
        .expect("a response can be built for any request the harness makes");
    if !status.is_provisional() {
        // A UAS tags To on its first final response — replacing the header, not adding a
        // second one, which would make the response invalid.
        let tag = TAGS
            .get(to_tag as usize % TAGS.len())
            .expect("the tag index is taken modulo the table");
        builder = builder
            .set_header(
                &HeaderName::To,
                Bytes::from(format!("<sip:callee@example.com>;tag={tag}")),
            )
            .expect("a valid To");
    }
    builder.build()
}

// ------------------------------------------------------------------------------------------
// Naming things, for the trace
// ------------------------------------------------------------------------------------------

fn method_name(m: &Method) -> String {
    String::from_utf8_lossy(m.as_bytes()).into_owned()
}

fn label(slot: u8, m: &Method) -> String {
    format!("{slot}/{}", method_name(m))
}

fn machine_name(space: Space, invite: bool) -> &'static str {
    match (space, invite) {
        (Space::Client, true) => "the INVITE client",
        (Space::Client, false) => "the non-INVITE client",
        (Space::Server, true) => "the INVITE server",
        (Space::Server, false) => "the non-INVITE server",
    }
}

fn client_state_name(state: ClientState) -> String {
    match state {
        ClientState::Calling => "Calling",
        ClientState::Trying => "Trying",
        ClientState::Proceeding => "Proceeding",
        ClientState::Completed => "Completed",
        ClientState::Accepted => "Accepted",
        ClientState::Terminated => "Terminated",
    }
    .to_owned()
}

fn server_state_name(state: ServerState) -> String {
    match state {
        ServerState::Trying => "Trying",
        ServerState::Proceeding => "Proceeding",
        ServerState::Completed => "Completed",
        ServerState::Confirmed => "Confirmed",
        ServerState::Accepted => "Accepted",
        ServerState::Terminated => "Terminated",
    }
    .to_owned()
}

fn store(layer: &TransactionLayer) -> String {
    let (client, server) = layer.len();
    format!("{client}/{server}")
}

fn summarise(outputs: &[Output]) -> String {
    let mut parts = Vec::new();
    let sends = outputs
        .iter()
        .filter(|o| matches!(o, Output::Send(_)))
        .count();
    if sends > 0 {
        parts.push(format!("send={sends}"));
    }
    let events: Vec<&str> = outputs
        .iter()
        .filter_map(|o| match o {
            Output::ToTu(event) => Some(match event.as_ref() {
                TuEvent::Request(_) => "request",
                TuEvent::Response(_) => "response",
                TuEvent::Ack(_) => "ack",
                TuEvent::Timeout => "timeout",
                TuEvent::TransportError => "transport-error",
            }),
            _ => None,
        })
        .collect();
    if !events.is_empty() {
        parts.push(format!("tu=[{}]", events.join(",")));
    }
    let set: Vec<String> = outputs
        .iter()
        .filter_map(|o| match o {
            Output::SetTimer { timer, .. } => Some(format!("{timer:?}")),
            _ => None,
        })
        .collect();
    if !set.is_empty() {
        parts.push(format!("set=[{}]", set.join(",")));
    }
    let cleared: Vec<String> = outputs
        .iter()
        .filter_map(|o| match o {
            Output::ClearTimer(timer) => Some(format!("{timer:?}")),
            _ => None,
        })
        .collect();
    if !cleared.is_empty() {
        parts.push(format!("clear=[{}]", cleared.join(",")));
    }
    if let Some(reason) = outputs.iter().find_map(|o| match o {
        Output::Terminated(reason) => Some(*reason),
        _ => None,
    }) {
        parts.push(format!("terminated={reason:?}"));
    }
    if parts.is_empty() {
        "absorbed".to_owned()
    } else {
        parts.join(" ")
    }
}

// ------------------------------------------------------------------------------------------
// The seed corpus
// ------------------------------------------------------------------------------------------

/// A named seed program, committed to the corpus as the bytes it encodes to.
#[derive(Debug, Clone)]
pub struct Seed {
    /// The corpus file's name.
    pub name: &'static str,
    /// The program.
    pub program: Program,
}

/// Where the committed seed corpus lives, relative to the repository root.
pub const CORPUS_PATH: &str = "crates/sipx-testkit/corpus/transaction-sequences";

/// The committed seed corpus directory.
#[must_use]
pub fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .join("transaction-sequences")
}

/// Write [`seeds`] to [`corpus_dir`], one file per seed.
///
/// The corpus is generated rather than hand-written so its provenance is reproducible, the way
/// `scripts/import-rfc4475-corpus.sh` makes the parser corpus reproducible from the RFC. What
/// makes that worth anything is the test on the other side:
/// `the_committed_corpus_is_exactly_the_seed_programs` fails if the two ever disagree.
pub fn write_corpus() -> std::io::Result<usize> {
    let dir = corpus_dir();
    std::fs::create_dir_all(&dir)?;
    let seeds = seeds();
    for seed in &seeds {
        std::fs::write(dir.join(seed.name), seed.program.encode())?;
    }
    Ok(seeds.len())
}

/// The programs the corpus is seeded from.
///
/// Seeding from nothing means the first minutes of every campaign are spent rediscovering that
/// a response has to follow a request. These are the scenarios of
/// `docs/specs/sip-transaction.md` §7 — the rows the FSM table tests already walk — rewritten
/// as event programs, which is the same trick CI plays on the parser targets by seeding them
/// with the RFC 4475 corpus. The fuzzer starts from behaviour that reaches every machine and
/// mutates outwards from there.
///
/// One long function on purpose: it is a table of scenarios, and a table reads best as a table.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn seeds() -> Vec<Seed> {
    // Slots 0 and 1 carry the magic cookie; 2 and 3 do not.
    const INVITE: u8 = 0;
    const ACK: u8 = 1;
    const BYE: u8 = 2;
    const CANCEL: u8 = 3;
    const REGISTER: u8 = 4;
    const OPTIONS: u8 = 5;

    // Indices into STATUSES.
    const S100: u8 = 0;
    const S180: u8 = 1;
    const S200: u8 = 2;
    const S486: u8 = 5;
    const S500: u8 = 6;

    /// Fire a named timer for the key at `key`, whether or not the model thinks it is armed.
    fn timer(key: u8, which: Timer) -> Event {
        let index = u8::try_from(
            TIMERS
                .iter()
                .position(|t| *t == which)
                .expect("every timer is in the table"),
        )
        .expect("the timer table is shorter than 256 entries");
        Event::FireTimer {
            key,
            timer: index,
            any: true,
        }
    }
    fn send(slot: u8, method: u8) -> Event {
        Event::SendRequest {
            slot,
            method,
            reliable: false,
        }
    }
    fn recv(slot: u8, method: u8) -> Event {
        Event::ReceiveRequest {
            slot,
            method,
            reliable: false,
            to_tag: 0,
        }
    }
    fn answer(slot: u8, method: u8, status: u8) -> Event {
        Event::SendResponse {
            slot,
            method,
            status,
            to_tag: 0,
        }
    }
    fn reply(slot: u8, method: u8, status: u8, to_tag: u8) -> Event {
        Event::ReceiveResponse {
            slot,
            method,
            status,
            to_tag,
        }
    }

    let mut seeds = Vec::new();
    let mut seed = |name: &'static str, events: Vec<Event>| {
        seeds.push(Seed {
            name,
            program: Program { events },
        });
    };

    // §7 T1: retransmission with doubling intervals, then the timeout.
    seed(
        "t1-invite-client-retransmits-then-times-out",
        vec![
            send(0, INVITE),
            timer(0, Timer::A),
            timer(0, Timer::A),
            timer(0, Timer::A),
            timer(0, Timer::B),
        ],
    );
    // §7 T2: a non-2xx is acknowledged by the transaction, which then waits out Timer D.
    seed(
        "t2-invite-client-acks-a-non-2xx",
        vec![
            send(0, INVITE),
            reply(0, INVITE, S486, 0),
            reply(0, INVITE, S486, 0),
            timer(0, Timer::D),
        ],
    );
    // §7 T3 and T4: a 2xx is not acknowledged here, and a fork's second 2xx still finds a
    // transaction to arrive at (RFC 6026).
    seed(
        "t3-invite-client-2xx-is-not-acked-and-a-fork-answers-twice",
        vec![
            send(0, INVITE),
            reply(0, INVITE, S200, 0),
            reply(0, INVITE, S200, 1),
            timer(0, Timer::M),
        ],
    );
    // A provisional cancels Timer B on purpose: the callee may ring for longer than 64·T1.
    seed(
        "invite-client-waits-in-proceeding-with-no-timeout",
        vec![
            send(0, INVITE),
            reply(0, INVITE, S180, 0),
            timer(0, Timer::B),
            reply(0, INVITE, S200, 0),
            timer(0, Timer::M),
        ],
    );
    // §4.2: Timer E backs off to T2 and Timer F retires the machine from Trying.
    seed(
        "non-invite-client-backs-off-then-times-out",
        vec![
            send(1, OPTIONS),
            timer(0, Timer::E),
            timer(0, Timer::E),
            timer(0, Timer::E),
            timer(0, Timer::F),
        ],
    );
    seed(
        "non-invite-client-completes-on-a-final-response",
        vec![
            send(1, BYE),
            reply(1, BYE, S200, 0),
            reply(1, BYE, S200, 0),
            timer(0, Timer::K),
        ],
    );
    // §7 T5 and T6: retransmissions are absorbed, and answered from the last response.
    seed(
        "t5-server-absorbs-request-retransmissions",
        vec![
            recv(0, REGISTER),
            recv(0, REGISTER),
            answer(0, REGISTER, S200),
            recv(0, REGISTER),
            timer(0, Timer::J),
        ],
    );
    // §7 T7 and T11: the transaction answers 100 itself, then a non-2xx, then absorbs the ACK.
    seed(
        "t7-invite-server-sends-100-then-absorbs-the-ack",
        vec![
            recv(0, INVITE),
            timer(0, Timer::Trying100),
            answer(0, INVITE, S486),
            timer(0, Timer::G),
            recv(0, ACK),
            timer(0, Timer::I),
        ],
    );
    // §7 T8 and T12: the TU answers first, so no 100 goes out, and the ACK for the 2xx is the
    // TU's business (RFC 6026).
    seed(
        "t8-invite-server-2xx-hands-the-ack-to-the-tu",
        vec![
            recv(0, INVITE),
            answer(0, INVITE, S180),
            timer(0, Timer::Trying100),
            answer(0, INVITE, S200),
            recv(0, ACK),
            timer(0, Timer::L),
        ],
    );
    // Timer H: no ACK ever comes.
    seed(
        "invite-server-times-out-waiting-for-an-ack",
        vec![
            recv(0, INVITE),
            answer(0, INVITE, S500),
            timer(0, Timer::G),
            timer(0, Timer::H),
        ],
    );
    // §7 T9: a reliable transport sets no retransmission timers and its absorption timers fire
    // at once.
    seed(
        "t9-reliable-transport-sets-no-retransmission-timers",
        vec![
            Event::SendRequest {
                slot: 0,
                method: INVITE,
                reliable: true,
            },
            reply(0, INVITE, S486, 0),
            timer(0, Timer::D),
            Event::ReceiveRequest {
                slot: 1,
                method: OPTIONS,
                reliable: true,
                to_tag: 0,
            },
            answer(1, OPTIONS, S200),
            timer(1, Timer::J),
        ],
    );
    // §7 T13: a sender from before the magic cookie meant anything is matched by the legacy key.
    seed(
        "t13-legacy-branch-matching",
        vec![
            recv(2, OPTIONS),
            recv(2, OPTIONS),
            answer(2, OPTIONS, S200),
            recv(2, OPTIONS),
            timer(0, Timer::J),
        ],
    );
    // §7 T14: a CANCEL names the INVITE's branch but runs in a transaction of its own.
    seed(
        "t14-cancel-runs-in-its-own-transaction",
        vec![
            recv(0, INVITE),
            recv(0, CANCEL),
            answer(0, CANCEL, S200),
            answer(0, INVITE, S486),
            recv(0, ACK),
            timer(0, Timer::I),
            timer(1, Timer::J),
        ],
    );
    // The transport failing, which no message can express.
    seed(
        "transport-error-terminates-and-tells-the-tu",
        vec![send(0, OPTIONS), Event::TransportError { key: 0 }],
    );
    // A server transaction the application never answered, dropped by the driver — §17.2.2
    // gives it no timer, so nothing else would.
    seed(
        "abandon-a-server-transaction-the-application-never-answered",
        vec![recv(1, REGISTER), Event::Abandon { key: 0 }],
    );
    // Timers arriving after the transaction has gone: the race a driver's timer wheel cannot
    // close, and what `Invariant::TimerForRemovedKey` is about.
    seed(
        "stale-timers-fire-after-the-transaction-is-gone",
        vec![
            send(1, OPTIONS),
            reply(1, OPTIONS, S200, 0),
            timer(0, Timer::K),
            timer(0, Timer::K),
            timer(0, Timer::F),
            timer(0, Timer::E),
        ],
    );
    // A provisional to a non-INVITE client, which keeps Timer F running — the asymmetry with
    // Timer B that §17.1.2.2 states outright.
    seed(
        "non-invite-client-times-out-from-proceeding-too",
        vec![
            send(1, REGISTER),
            reply(1, REGISTER, S100, 0),
            timer(0, Timer::E),
            timer(0, Timer::F),
        ],
    );

    seeds
}
