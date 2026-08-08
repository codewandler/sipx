// Deterministic stand-ins for the four host facilities the binding is written against.
//
// The binding takes its socket, clock, entropy source and connectivity monitor as constructor
// arguments precisely so that this file can exist: every case below is a decision the binding
// makes, driven at the exact instant the case wants it, with no wall-clock wait anywhere. The
// real browser bindings are in `../src/platform.mjs` and are the only place a global is read.

const encoder = new TextEncoder();

// ------------------------------------------------------------------------------------------
// Clock
// ------------------------------------------------------------------------------------------

/**
 * A monotonic clock, a timer scheduler and a task deferral, all under the test's control.
 *
 * `fireEveryRetainedCallback` is the point of it: a real `clearTimeout` cannot unring a callback
 * the platform has already taken off its queue, so a cancelled timer firing anyway is a race the
 * binding must survive rather than a state it can assume away.
 */
export class FakeClock {
  #now = 0;
  #timers = new Map();
  #retained = [];
  #deferred = [];
  #nextId = 1;

  /** Every delay the binding asked for, in order. Reconnect backoff is read from here. */
  requested = [];

  now() {
    return this.#now;
  }

  setTimer(callback, delayMs) {
    const id = this.#nextId;
    this.#nextId += 1;
    this.#timers.set(id, { callback, at: this.#now + delayMs, delayMs });
    this.#retained.push(callback);
    this.requested.push(delayMs);
    return id;
  }

  clearTimer(id) {
    this.#timers.delete(id);
  }

  defer(task) {
    this.#deferred.push(task);
  }

  /** How many timers the binding currently owns. Zero after cancellation is the assertion. */
  get pending() {
    return this.#timers.size;
  }

  /** Run every deferred task, including tasks those tasks defer. */
  flush() {
    let guard = 0;
    while (this.#deferred.length > 0) {
      guard += 1;
      if (guard > 10_000) throw new Error("deferred tasks did not settle");
      const task = this.#deferred.shift();
      task();
    }
  }

  #fireDue() {
    let guard = 0;
    for (;;) {
      guard += 1;
      if (guard > 10_000) throw new Error("timers did not settle");
      const due = [...this.#timers.entries()]
        .filter(([, timer]) => timer.at <= this.#now)
        .sort((left, right) => left[1].at - right[1].at);
      if (due.length === 0) return;
      const [id, timer] = due[0];
      this.#timers.delete(id);
      timer.callback();
    }
  }

  /**
   * Move time forward and fire what comes due, **without** running deferred tasks.
   *
   * This is how "a timer re-enters the kernel only as a fired-timer input" is checkable: the
   * platform callback has run and the kernel must still not have been called.
   */
  advanceWithoutFlush(ms) {
    this.#now += ms;
    this.#fireDue();
  }

  /** Move time forward, firing every timer that comes due, earliest first. */
  advance(ms) {
    this.#now += ms;
    this.#fireDue();
    this.flush();
    this.#fireDue();
    this.flush();
  }

  /**
   * Invoke every callback ever scheduled, cleared or not, in registration order.
   *
   * This is the stale-callback race made deterministic.
   */
  fireEveryRetainedCallback() {
    for (const callback of [...this.#retained]) callback();
    this.flush();
  }
}

// ------------------------------------------------------------------------------------------
// Socket
// ------------------------------------------------------------------------------------------

export const CONNECTING = 0;
export const OPEN = 1;
export const CLOSING = 2;
export const CLOSED = 3;

/** Enough of the browser's `WebSocket` for the binding, and nothing it does not use. */
export class FakeSocket {
  constructor(url, protocols) {
    this.url = url;
    this.requestedProtocols = protocols;
    this.readyState = CONNECTING;
    this.bufferedAmount = 0;
    this.protocol = "";
    this.binaryType = "blob";
    this.sent = [];
    this.closed = null;
    this.listeners = new Map();
  }

  addEventListener(type, handler) {
    const set = this.listeners.get(type) ?? new Set();
    set.add(handler);
    this.listeners.set(type, set);
  }

  removeEventListener(type, handler) {
    this.listeners.get(type)?.delete(handler);
  }

  send(data) {
    if (this.readyState !== OPEN) throw new Error("send() on a socket that is not open");
    this.sent.push(data);
    this.bufferedAmount += data.byteLength ?? data.length ?? 0;
  }

  close(code, reason) {
    this.closed = { code, reason };
    this.readyState = CLOSED;
  }

  /** Total attached handlers. Zero after cancellation is the assertion. */
  get attachedHandlers() {
    let total = 0;
    for (const set of this.listeners.values()) total += set.size;
    return total;
  }

  #emit(type, event) {
    for (const handler of [...(this.listeners.get(type) ?? [])]) handler({ type, ...event });
  }

  /** The handshake completed, with the subprotocol the far end selected. */
  acceptOpen(protocol = "sip") {
    this.readyState = OPEN;
    this.protocol = protocol;
    this.#emit("open", {});
  }

  /** One inbound WebSocket message. `data` is delivered exactly as the platform would. */
  deliver(data) {
    this.#emit("message", { data });
  }

  remoteClose({ code = 1006, reason = "", wasClean = false } = {}) {
    this.readyState = CLOSED;
    this.#emit("close", { code, reason, wasClean });
  }

  fail() {
    this.#emit("error", {});
  }

  /** Deliver to handlers the binding believes it removed. */
  deliverRegardless(type, event) {
    this.#emit(type, event);
  }
}

/** A socket factory that records every socket the binding constructs. */
export class FakeNetwork {
  constructor() {
    this.sockets = [];
    this.factory = (url, protocols) => {
      const socket = new FakeSocket(url, protocols);
      this.sockets.push(socket);
      return socket;
    };
  }

  get latest() {
    return this.sockets[this.sockets.length - 1];
  }

  get attempts() {
    return this.sockets.length;
  }
}

// ------------------------------------------------------------------------------------------
// Kernel
// ------------------------------------------------------------------------------------------

/** A WIRE record (§4.6 type 1), from text for readability. */
export const wire = (text) => ({ kind: "wire", bytes: encoder.encode(text) });
/** A TIMER_SET record (§4.6 type 2). */
export const timerSet = (id, fireAtMs) => ({ kind: "timer-set", id: BigInt(id), fireAtMs });
/** A TIMER_CANCEL record (§4.6 type 3). */
export const timerCancel = (id) => ({ kind: "timer-cancel", id: BigInt(id) });
/** An EVENT record (§4.6 type 4), carrying one §5.3 document. */
export const kernelEvent = (document) => ({ kind: "event", document });

/**
 * A scriptable kernel.
 *
 * `respond` receives every call the binding makes, in order, and returns the records that call
 * produced. Everything the binding is supposed to send, schedule or surface arrives this way,
 * which is how "sends only bytes emitted by the kernel" becomes checkable: the case controls the
 * complete set of bytes the kernel ever emitted.
 */
export class FakeKernel {
  constructor(respond = () => []) {
    this.respond = respond;
    this.calls = [];
    this.counters = { parse_errors: 0 };
    this.snapshots = 0;
    this.freed = false;
  }

  #call(kind, detail) {
    const call = { kind, ...detail };
    this.calls.push(call);
    return this.respond(call, this) ?? [];
  }

  command(document, nowMs) {
    return this.#call("command", { document, nowMs });
  }

  inputBytes(bytes, nowMs) {
    return this.#call("bytes", { bytes, nowMs });
  }

  inputTimer(id, nowMs) {
    return this.#call("timer", { id, nowMs });
  }

  inputEntropy(bytes) {
    return this.#call("entropy", { bytes });
  }

  snapshot() {
    this.snapshots += 1;
    return {
      v: 1,
      registration: "unregistered",
      calls: {},
      entropy: 0,
      pendingTimers: 0,
      queuedOutputs: 0,
      counters: { ...this.counters },
      poisoned: false,
    };
  }

  free() {
    this.freed = true;
  }

  /** The calls of one kind, for a case that only cares about one channel. */
  of(kind) {
    return this.calls.filter((call) => call.kind === kind);
  }
}

// ------------------------------------------------------------------------------------------
// Connectivity and entropy
// ------------------------------------------------------------------------------------------

export class FakeConnectivity {
  constructor(online = true) {
    this.online = online;
    this.handlers = new Set();
  }

  isOnline() {
    return this.online;
  }

  subscribe(handler) {
    this.handlers.add(handler);
    return () => this.handlers.delete(handler);
  }

  get subscribers() {
    return this.handlers.size;
  }

  goOffline() {
    this.online = false;
    for (const handler of [...this.handlers]) handler(false);
  }

  goOnline() {
    this.online = true;
    for (const handler of [...this.handlers]) handler(true);
  }
}

/** A recording entropy source. Fills with a counting pattern so a test can recognise it. */
export class FakeEntropy {
  constructor() {
    this.requests = [];
  }

  fill(length) {
    this.requests.push(length);
    return Uint8Array.from({ length }, (_, index) => index & 0xff);
  }
}

/** Collect the binding's typed events. */
export class Recorder {
  constructor() {
    this.events = [];
  }

  get sink() {
    return (event) => this.events.push(event);
  }

  ofType(type) {
    return this.events.filter((event) => event.type === type);
  }

  get types() {
    return this.events.map((event) => event.type);
  }
}

/** The `transport` block of `BSDK-CFG-1` (`docs/specs/browser-sdk.md` §9.2). */
export const WSS_TRANSPORT = Object.freeze({
  scheme: "wss",
  host: "edge.example.net",
  resource: "/sip",
});
