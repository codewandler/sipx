// The browser's WebSocket, bound to the sans-I/O session kernel.
//
// `docs/specs/browser-signalling.md` is this module's contract and `docs/specs/browser-sdk.md` is
// its parent; section references below are to the parent unless they say otherwise. The division
// of labour is the epic's: the kernel parses, decides and serialises, and this file does every
// single thing that touches the outside world — opening the socket, holding the queues, owning
// the host timers, drawing entropy from the platform, and deciding when to stop trying.
//
// Three properties are structural rather than promised, and they are why the code is shaped the
// way it is:
//
//   1. **Only the kernel's bytes reach the socket.** `socket.send` is called from exactly one
//      place, `#write`, which is reachable only from a `WIRE` record. There is no path from a
//      caller's buffer to the wire.
//   2. **Timers re-enter only as fired-timer inputs.** A platform timer callback appends to the
//      inbox and returns; it never calls the kernel. The kernel is entered from one method,
//      `#drive`, running in a deferred task.
//   3. **Nothing here parses SIP.** A received WebSocket message is offered to the kernel exactly
//      as it arrived. A host that guessed at message boundaries would be a second parser on the
//      same hostile input (§8.1), which is the attack surface the kernel exists to concentrate.

/** RFC 7118 §4's subprotocol. A peer that does not select it is refused. */
export const SIP_SUBPROTOCOL = "sip";

/**
 * Every bound this binding enforces, with the reasoning that fixes each number.
 *
 * These are contract values rather than tuning knobs: `docs/specs/browser-signalling.md` §3 states
 * them, and a case in `browser/test/transport.test.mjs` holds the four derived from the kernel to
 * the kernel's own §4.9 constants so the two cannot drift apart silently.
 */
export const limits = Object.freeze({
  /** §4.9's inbound bound, enforced here so an oversize frame costs no linear memory. */
  maxSipMessage: 64 * 1024,

  /**
   * Outbound messages held while no socket is writable, and their total size.
   *
   * Both are §4.9's `MAX_QUEUED_RECORDS`/`MAX_QUEUED_BYTES`. That pair is the kernel's own
   * statement of how much in-flight signalling is a defect rather than a burst, and the host has
   * no better information than the kernel about where that line is. It is also exactly tight: one
   * drain can hand this binding no more than the kernel was willing to queue.
   */
  sendQueueRecords: 256,
  sendQueueBytes: 256 * 1024,

  /** The inbox, bounded by the same pair and for the same reason. */
  receiveQueueEntries: 256,
  receiveQueueBytes: 256 * 1024,

  /**
   * Connection attempts for the whole lifetime of one binding, initial attempt included.
   *
   * Deliberately a lifetime budget and not a per-episode one: replenishing it on a successful
   * open turns a peer that accepts and immediately drops into an unbounded retry loop, which is
   * the thing `T-33` forbids. Retry policy past the budget belongs to the application (§5.3), and
   * the application's move is to construct a new binding — a decision it takes deliberately, in
   * response to the typed `closed` event this one ends with.
   */
  connectAttempts: 7,

  /**
   * The delay before each reconnection, doubling from RFC 3261's T1.
   *
   * They sum to 31 500 ms — 63 × T1 — so the last attempt begins strictly inside RFC 3261's
   * 64 × T1 give-up horizon (Timer B and Timer F, `docs/specs/sip-transaction.md`). A transport
   * that reconnected later than that could not rescue any transaction the kernel still holds
   * open, so retrying past it buys nothing and costs a socket.
   */
  reconnectDelaysMs: Object.freeze([500, 1000, 2000, 4000, 8000, 16_000]),

  /**
   * How long one attempt may sit in `CONNECTING`. 64 × T1, for the reason above: a handshake that
   * has not completed by then is of no use to any transaction that was waiting for it. The
   * platform's `WebSocket` has no connect timeout of its own, so this is the only one.
   */
  connectTimeoutMs: 32_000,

  /**
   * Octets drawn from the platform CSPRNG for each refill.
   *
   * §4.7's capacity (1024) minus its low-water mark (64). The kernel asks only when the pool is
   * *below* the low-water mark, so a refill of exactly the headroom above that mark can never
   * overflow the pool — which means the binding never needs to read the pool level to size a
   * feed, and `E_BOUNDS` is unreachable from this path.
   */
  entropyRefill: 960,
});

/**
 * Why a connection was refused, dropped or abandoned.
 *
 * **Not exhaustive by design.** These are diagnostics for a transport, and a future kernel or
 * platform will end a connection in ways this list does not name yet; appending a reason is a
 * compatible change. Code that switches on a reason must have a default arm. What *is* stable is
 * that an existing value never changes meaning, matching the rule §4.10 sets for the ABI's codes.
 */
export const Reason = Object.freeze({
  /** `ws:` without the explicit development policy. */
  InsecureScheme: "insecure-scheme",
  /** A scheme that is neither `wss` nor `ws`. */
  UnsupportedScheme: "unsupported-scheme",
  /** `ws:` to something that is not loopback, even under the development policy. */
  CleartextNotLoopback: "cleartext-not-loopback",
  /** Userinfo in the configured host. */
  CredentialsInUrl: "credentials-in-url",
  /** A query string or fragment in the configured resource. */
  ResourceCarriesQuery: "resource-carries-query",
  /** A resource that is neither empty nor rooted. */
  InvalidResource: "invalid-resource",
  /** A host the URL parser will not accept, or that it rewrote. */
  InvalidHost: "invalid-host",
  /** The handshake completed without RFC 7118 §4's subprotocol. */
  SubprotocolRefused: "subprotocol-refused",
  /** The socket could not be constructed. */
  ConnectFailed: "connect-failed",
  /** The handshake did not complete inside {@link limits.connectTimeoutMs}. */
  ConnectTimeout: "connect-timeout",
  /** The attempt budget is spent; this binding will not try again. */
  ConnectExhausted: "connect-exhausted",
  /** The far end closed, or the connection dropped. */
  RemoteClose: "remote-close",
  /** There was no socket to write to. */
  NoSocket: "no-socket",
  /** The browser reported itself offline. */
  Offline: "offline",
  /** The peer disagrees about where SIP messages end (`docs/specs/sip-tls.md` §4). */
  Framing: "framing",
  /** A received message exceeds {@link limits.maxSipMessage}. */
  MessageTooLarge: "message-too-large",
  /** A frame this binding will not read — a `Blob`, or anything with no synchronous bytes. */
  UnsupportedFrame: "unsupported-frame",
  /** The send queue, or the platform's own outbound buffer, is past its bound. */
  SendOverflow: "send-overflow",
  /** The inbox is past its bound. */
  ReceiveOverflow: "receive-overflow",
  /** The kernel reported a fatal fault, or behaved in a way its contract forbids. */
  KernelFault: "kernel-fault",
  /** {@link WebSocketSignalling#cancel} was called. */
  Cancelled: "cancelled",
});

/**
 * The typed events this binding emits.
 *
 * **Not exhaustive by design**, for the reason {@link Reason} gives. Every event carries a
 * `type` from this table and nothing else is guaranteed; §8.3 forbids credentials in any of them,
 * and no event ever carries SIP bytes, an authorization header or an entropy octet.
 */
export const EventType = Object.freeze({
  /** `{ attempt, url }` — a socket is about to be constructed. */
  Connecting: "connecting",
  /** `{ protocol, url }` — the handshake completed and the subprotocol is `sip`. */
  Open: "open",
  /** `{ reason, code?, wasClean?, dropped }` — a socket ended; a reconnection may follow. */
  Disconnected: "disconnected",
  /** `{ scope, reason, count }` — outbound messages were thrown away rather than replayed. */
  Discarded: "discarded",
  /** `{ attempt, delayMs }` — the next attempt, and when. */
  ReconnectScheduled: "reconnect-scheduled",
  /** `{}` — the browser is offline; nothing will be attempted until it is not. */
  Offline: "offline",
  /** `{}` — the browser is back; a suspended attempt resumes. */
  Online: "online",
  /** `{ queue, limit, unit }` — a bound in {@link limits} was reached. */
  Overflow: "overflow",
  /** `{ document, event }` — one §5.3 kernel event, forwarded whole. */
  Kernel: "kernel",
  /** `{ reason, attempts }` — terminal. Nothing follows it. */
  Closed: "closed",
});

/** A configuration this contract refuses. Its `message` never quotes the value it refused. */
export class SignallingConfigError extends Error {
  constructor(reason, message) {
    super(message);
    this.name = "SignallingConfigError";
    /** @type {string} one of {@link Reason} */
    this.reason = reason;
  }
}

/** An operation attempted in a state that does not allow it. */
export class SignallingStateError extends Error {
  constructor(message) {
    super(message);
    this.name = "SignallingStateError";
  }
}

const encoder = new TextEncoder();

/**
 * Loopback, and only loopback.
 *
 * RFC 5735 reserves all of `127.0.0.0/8`, and RFC 6761 §6.3 reserves `localhost`; `::1` is
 * IPv6's. Nothing else is local, whatever a hosts file says.
 */
const LOOPBACK = /^(?:localhost|127\.\d{1,3}\.\d{1,3}\.\d{1,3}|\[::1\])$/;

/**
 * Build the signalling URL from a `BSDK-CFG` `transport` block, or refuse it.
 *
 * WSS is the default and the contract (§8.6). Cleartext `ws:` needs two things at once: the
 * explicit `"allow-development"` policy in the configuration, *and* a loopback host. §8.6 hands
 * `T-33` the socket-side enforcement and says the flag exists "for local development against
 * loopback"; a development flag that also reaches a routable host is a production deployment with
 * a typo in it, and the kernel has already stopped answering digest challenges by then — so the
 * page would fail to register anyway, later, with a worse error.
 *
 * Credentials cannot enter the URL by any route: userinfo in the host is refused, and so is a
 * query string or fragment in the resource, which is where a bearer token would otherwise be
 * written. The refusal messages are fixed strings and never quote the configuration, because a
 * thrown error's text reaches logs and consoles (§8.3).
 *
 * The resource is RFC 7118's, and it is not fixed: `docs/specs/sip-tls.md` §4 says both `/` and
 * `/ws` are conformant, so a non-root path is normal rather than exotic. An empty resource means
 * the root; anything else must be rooted, because a relative resource has no defined meaning here.
 *
 * @param {{scheme: string, host: string, resource: string}} transport
 * @param {"refuse" | "allow-development"} insecure the configuration's `insecure` field
 * @returns {string} an absolute `wss:` or `ws:` URL
 * @throws {SignallingConfigError}
 */
export function signallingUrl({ scheme, host, resource }, insecure) {
  if (scheme !== "wss" && scheme !== "ws") {
    throw new SignallingConfigError(
      Reason.UnsupportedScheme,
      "the signalling scheme must be wss, or ws under the development policy",
    );
  }
  if (scheme === "ws" && insecure !== "allow-development") {
    throw new SignallingConfigError(
      Reason.InsecureScheme,
      'cleartext signalling requires "insecure":"allow-development"',
    );
  }
  if (typeof host !== "string" || host === "" || host.includes("/")) {
    throw new SignallingConfigError(
      Reason.InvalidHost,
      "the signalling host is not a host",
    );
  }
  if (host.includes("@")) {
    throw new SignallingConfigError(
      Reason.CredentialsInUrl,
      "the signalling host carries userinfo; credentials never enter a URL",
    );
  }
  if (typeof resource !== "string") {
    throw new SignallingConfigError(
      Reason.InvalidResource,
      "the signalling resource is not a string",
    );
  }
  if (resource !== "" && !resource.startsWith("/")) {
    throw new SignallingConfigError(
      Reason.InvalidResource,
      "the signalling resource must be empty or begin with /",
    );
  }
  if (resource.includes("?") || resource.includes("#")) {
    throw new SignallingConfigError(
      Reason.ResourceCarriesQuery,
      "the signalling resource carries a query or fragment; credentials never enter a URL",
    );
  }

  let url;
  try {
    url = new URL(`${scheme}://${host}${resource === "" ? "/" : resource}`);
  } catch {
    throw new SignallingConfigError(
      Reason.InvalidHost,
      "the signalling host is not a host",
    );
  }
  // Parse, then verify: the checks above read the strings, and these read what the URL parser
  // made of them. A host that smuggled a scheme, userinfo or query past a substring test cannot
  // also survive being reparsed and compared.
  if (url.protocol !== `${scheme}:`) {
    throw new SignallingConfigError(
      Reason.UnsupportedScheme,
      "the signalling scheme was rewritten",
    );
  }
  if (url.username !== "" || url.password !== "") {
    throw new SignallingConfigError(
      Reason.CredentialsInUrl,
      "the signalling URL carries userinfo; credentials never enter a URL",
    );
  }
  if (url.search !== "" || url.hash !== "") {
    throw new SignallingConfigError(
      Reason.ResourceCarriesQuery,
      "the signalling URL carries a query or fragment; credentials never enter a URL",
    );
  }
  if (url.host !== host.toLowerCase()) {
    throw new SignallingConfigError(
      Reason.InvalidHost,
      "the signalling host was rewritten",
    );
  }
  if (scheme === "ws" && !LOOPBACK.test(url.hostname)) {
    throw new SignallingConfigError(
      Reason.CleartextNotLoopback,
      "the development policy permits cleartext to loopback only",
    );
  }
  return url.toString();
}

/**
 * The kernel, as this binding needs it.
 *
 * Every method takes the §4.6 drain obligation with it: an entry point returns the records that
 * call produced, so there is no way to enter the kernel and forget to drain it. `A-17` generates
 * the implementation over the real ABI; `browser/test/kernel.test.mjs` writes one by hand to
 * drive the compiled module.
 *
 * @typedef {object} KernelPort
 * @property {(document: Uint8Array, nowMs: number) => object[]} command submit one §5.2 command
 * @property {(bytes: Uint8Array, nowMs: number) => object[]} inputBytes one received message
 * @property {(id: bigint, nowMs: number) => object[]} inputTimer a requested timer fired
 * @property {(bytes: Uint8Array) => object[]} inputEntropy append §4.7 entropy
 * @property {() => {counters: {parse_errors: number}}} snapshot the §4.11 diagnostic read
 * @property {() => void} free §6.5 step 4
 */

/**
 * Drive the session kernel over the browser's WebSocket API.
 *
 * The binding owns one socket at a time, one bounded inbox, one bounded send queue and every host
 * timer the kernel asked for. It reaches the platform only through the four facilities passed to
 * the constructor, which is what makes every case in the suite deterministic and what lets a
 * page substitute its own for a diagnostic build. `browser/src/platform.mjs` supplies the real
 * ones.
 *
 * What it does **not** do: parse SIP, invent a retransmission, hold protocol state, or interpret
 * any kernel event other than the two it is obliged to act on (§4.7's entropy demand and §5.3's
 * fatal error). Everything else is forwarded whole.
 */
export class WebSocketSignalling {
  #url;
  #kernel;
  #openSocket;
  #clock;
  #entropy;
  #connectivity;
  #onEvent;

  #state = "idle";
  #socket = null;
  #handlers = null;
  #attempts = 0;

  #inbox = [];
  #inboxBytes = 0;
  #pumpScheduled = false;

  #sendQueue = [];
  #sendQueueBytes = 0;

  #timers = new Map();
  #connectTimer = null;
  #reconnectTimer = null;
  #unsubscribe = null;

  #pending = [];
  #deliveryScheduled = false;

  #entropyQueued = false;
  #drivingEntropy = false;
  #offlineAnnounced = false;
  #parseErrors = 0;
  #lastNow = 0;

  #counters = {
    connectAttempts: 0,
    sent: 0,
    received: 0,
    discarded: 0,
    staleCallbacks: 0,
    droppedAfterClose: 0,
    listenerFaults: 0,
  };

  /**
   * @param {object} options
   * @param {{scheme: string, host: string, resource: string}} options.transport the `BSDK-CFG`
   *   transport block
   * @param {"refuse" | "allow-development"} [options.insecure] the `BSDK-CFG` `insecure` field
   * @param {KernelPort} options.kernel
   * @param {(url: string, protocols: string[]) => object} options.openSocket
   * @param {{now: () => number, setTimer: (fn: () => void, ms: number) => unknown,
   *   clearTimer: (handle: unknown) => void, defer: (fn: () => void) => void}} options.clock
   * @param {{fill: (length: number) => Uint8Array}} options.entropy the platform CSPRNG, and
   *   nothing else (§8.4)
   * @param {{isOnline: () => boolean, subscribe: (fn: (online: boolean) => void) => () => void}}
   *   options.connectivity
   * @param {(event: object) => void} options.onEvent
   * @throws {SignallingConfigError} before anything is allocated, so there is no half-built
   *   binding for a caller to hold (§6.2's "no half-started client")
   */
  constructor({
    transport,
    insecure = "refuse",
    kernel,
    openSocket,
    clock,
    entropy,
    connectivity,
    onEvent,
  }) {
    this.#url = signallingUrl(transport, insecure);
    this.#kernel = kernel;
    this.#openSocket = openSocket;
    this.#clock = clock;
    this.#entropy = entropy;
    this.#connectivity = connectivity;
    this.#onEvent = onEvent ?? (() => {});
  }

  /** `idle`, `connecting`, `open`, `disconnected`, `waiting`, `suspended` or `closed`. */
  get state() {
    return this.#state;
  }

  /** The signalling URL. Never carries credentials — {@link signallingUrl} refuses one. */
  get url() {
    return this.#url;
  }

  /** A snapshot of the binding's own counters. Diagnostics only; nothing gates on them. */
  get counters() {
    return Object.freeze({ ...this.#counters });
  }

  /**
   * Seed the entropy pool, subscribe to connectivity and begin connecting.
   *
   * The pool is seeded before anything else and without being asked. A fresh kernel's pool is
   * empty, and §4.7 makes an identifier derived from an insufficient pool fail whole with
   * `E_ENTROPY` — so a binding that waited for `"need-entropy"` would earn that failure on the
   * first command rather than on the second. Seeding is queued ahead of the connect, so the pool
   * is full before a socket exists to carry anything.
   *
   * The first connection attempt is deferred rather than made inline, so that no listener runs
   * inside this call (§6.4 rule 2). Calling it twice is a no-op; calling it after {@link cancel}
   * throws.
   */
  start() {
    if (this.#state === "closed") {
      throw new SignallingStateError(
        "this transport has closed and does not restart",
      );
    }
    if (this.#state !== "idle") return;
    this.#state = "connecting";
    // The framing check in `#framingViolation` compares against this baseline. Read rather than
    // assumed to be zero: a kernel handed over with a parse error already counted would otherwise
    // make the first message that produced no records look like a framing violation.
    this.#parseErrors = this.#readParseErrors() ?? 0;
    if (!this.#refillEntropy()) return;
    this.#unsubscribe = this.#connectivity.subscribe((online) =>
      this.#onConnectivity(online),
    );
    this.#clock.defer(() => this.#connect());
  }

  /**
   * Submit one §5.2 command document.
   *
   * The document reaches the kernel and never the socket: the only bytes that reach the socket
   * are the ones the kernel emits as `WIRE` records in response. Completion is the kernel's
   * `"outcome"` event, forwarded like any other; promisifying it is the lifecycle layer's job
   * (§7.2), not this one's.
   *
   * @param {string | Uint8Array} document canonical §5.1 JSON
   */
  submit(document) {
    if (this.#state === "closed") {
      throw new SignallingStateError(
        "this transport has closed and accepts no commands",
      );
    }
    const bytes =
      typeof document === "string" ? encoder.encode(document) : document;
    this.#enqueue({ kind: "command", document: bytes, size: bytes.length });
  }

  /**
   * Tear everything down, synchronously and idempotently.
   *
   * In §6.5's order: the kernel is freed first (step 4) and the socket closed after (step 5), so
   * the kernel has cancelled its own state before the transport it was speaking over disappears.
   * Every host timer is cleared, every socket handler detached, the connectivity subscription
   * released and both queues dropped. A callback that fires anyway — a platform timer already
   * taken off its queue when `clearTimeout` ran, or a socket event dispatched before
   * `removeEventListener` took effect — reaches neither the kernel nor a listener, and is counted
   * in `counters.staleCallbacks` rather than ignored.
   *
   * Synchronous by necessity: `pagehide` gives a page no task in which to finish anything, and
   * §8.8 will not have a microphone track outlive its page.
   *
   * @param {string} [reason] one of {@link Reason}
   */
  cancel(reason = Reason.Cancelled) {
    this.#finish(reason);
  }

  // ----------------------------------------------------------------------------------------
  // Connecting
  // ----------------------------------------------------------------------------------------

  #connect() {
    if (this.#state === "closed") return;
    if (!this.#connectivity.isOnline()) {
      this.#suspend();
      return;
    }
    if (this.#attempts >= limits.connectAttempts) {
      this.#finish(Reason.ConnectExhausted);
      return;
    }
    this.#attempts += 1;
    this.#counters.connectAttempts = this.#attempts;
    this.#state = "connecting";
    this.#emit({
      type: EventType.Connecting,
      attempt: this.#attempts,
      url: this.#url,
    });

    let socket;
    try {
      socket = this.#openSocket(this.#url, [SIP_SUBPROTOCOL]);
    } catch {
      // The platform refused to construct the socket at all. Treated as this attempt failing, so
      // the budget applies to it exactly as it applies to a handshake that never completes.
      this.#endSocket(Reason.ConnectFailed, {});
      return;
    }
    // Before any message can arrive. The default `blob` mode makes a message's bytes reachable
    // only through a promise, and awaiting one per message reorders delivery against a peer that
    // sends two in the same turn — a SIP stack cannot survive its inputs being permuted.
    try {
      socket.binaryType = "arraybuffer";
    } catch {
      // A socket that will not accept the mode is one this binding cannot read safely.
      this.#endSocket(Reason.UnsupportedFrame, {});
      return;
    }
    this.#socket = socket;
    this.#attach(socket);
    this.#connectTimer = this.#clock.setTimer(
      () => this.#onConnectTimeout(socket),
      limits.connectTimeoutMs,
    );
  }

  #attach(socket) {
    const on = {
      open: () => this.#onOpen(socket),
      message: (event) => this.#onMessage(socket, event),
      close: (event) => this.#onClose(socket, event),
      // A platform fires `error` and then `close`; the close carries the detail, so the error
      // needs only its stale guard. Acting on both would end one socket twice.
      error: () => this.#guard(socket),
    };
    for (const [type, handler] of Object.entries(on))
      socket.addEventListener(type, handler);
    this.#handlers = { socket, on };
  }

  #detach(socket) {
    if (this.#handlers === null || this.#handlers.socket !== socket) return;
    for (const [type, handler] of Object.entries(this.#handlers.on)) {
      socket.removeEventListener(type, handler);
    }
    this.#handlers = null;
  }

  /** True when a callback belongs to the current socket and the binding is still live. */
  #guard(socket) {
    if (socket === this.#socket && this.#state !== "closed") return true;
    this.#counters.staleCallbacks += 1;
    return false;
  }

  #onOpen(socket) {
    if (!this.#guard(socket)) return;
    this.#clearConnectTimer();
    if (socket.protocol !== SIP_SUBPROTOCOL) {
      // RFC 7118 §4, as `docs/specs/sip-tls.md` §4 states it: without the subprotocol there is no
      // agreement about what the frames mean. Deterministic, so it is not retried — the same peer
      // will refuse the same subprotocol on the next socket, and six more attempts prove nothing.
      this.#finish(Reason.SubprotocolRefused);
      return;
    }
    this.#state = "open";
    this.#emit({
      type: EventType.Open,
      protocol: socket.protocol,
      url: this.#url,
    });
    this.#flushSendQueue();
  }

  #onConnectTimeout(socket) {
    if (!this.#guard(socket)) return;
    this.#endSocket(Reason.ConnectTimeout, {});
  }

  #onClose(socket, event) {
    if (!this.#guard(socket)) return;
    this.#endSocket(Reason.RemoteClose, {
      code: event?.code,
      wasClean: event?.wasClean === true,
    });
  }

  /**
   * One socket ended. Drop what belonged to it and decide whether to try again.
   *
   * The send queue dies with the socket. A SIP message serialised for a connection that no longer
   * exists is precisely the message the kernel's transaction layer will produce again if it still
   * wants it, and replaying one onto a fresh socket would put a credential-bearing request on the
   * wire that nothing asked for — the close-during-authentication case (§8.3, §8.6).
   */
  #endSocket(reason, detail) {
    if (this.#state === "closed") return;
    this.#clearConnectTimer();
    const socket = this.#socket;
    this.#socket = null;
    if (socket) {
      this.#detach(socket);
      if (socket.readyState !== 3) {
        try {
          socket.close(1000, "");
        } catch {
          // A socket that will not close is already gone.
        }
      }
    }
    const dropped = this.#sendQueue.length;
    this.#sendQueue = [];
    this.#sendQueueBytes = 0;
    this.#counters.discarded += dropped;
    this.#state = "disconnected";
    this.#emit({ type: EventType.Disconnected, reason, dropped, ...detail });
    if (dropped > 0) {
      this.#emit({
        type: EventType.Discarded,
        scope: "send",
        reason,
        count: dropped,
      });
    }
    this.#scheduleReconnect();
  }

  #scheduleReconnect() {
    if (this.#state === "closed") return;
    if (!this.#connectivity.isOnline()) {
      this.#suspend();
      return;
    }
    const delayMs = limits.reconnectDelaysMs[this.#attempts - 1];
    if (delayMs === undefined) {
      this.#finish(Reason.ConnectExhausted);
      return;
    }
    this.#state = "waiting";
    this.#emit({
      type: EventType.ReconnectScheduled,
      attempt: this.#attempts + 1,
      delayMs,
    });
    this.#reconnectTimer = this.#clock.setTimer(
      () => this.#onReconnect(),
      delayMs,
    );
  }

  #onReconnect() {
    if (this.#state !== "waiting") {
      this.#counters.staleCallbacks += 1;
      return;
    }
    this.#reconnectTimer = null;
    this.#connect();
  }

  #suspend() {
    this.#state = "suspended";
    if (this.#offlineAnnounced) return;
    this.#offlineAnnounced = true;
    this.#emit({ type: EventType.Offline });
  }

  #onConnectivity(online) {
    if (this.#state === "closed") {
      this.#counters.staleCallbacks += 1;
      return;
    }
    if (online) {
      if (!this.#offlineAnnounced) return;
      this.#offlineAnnounced = false;
      this.#emit({ type: EventType.Online });
      // The budget is not replenished: an offline period buys patience, never attempts.
      if (this.#state === "suspended") this.#scheduleReconnect();
      return;
    }
    if (this.#state === "waiting") this.#clearReconnectTimer();
    if (this.#state === "waiting" || this.#state === "disconnected") {
      this.#suspend();
      return;
    }
    if (!this.#offlineAnnounced) {
      this.#offlineAnnounced = true;
      this.#emit({ type: EventType.Offline });
    }
  }

  // ----------------------------------------------------------------------------------------
  // Receiving
  // ----------------------------------------------------------------------------------------

  #onMessage(socket, event) {
    if (!this.#guard(socket)) return;
    const bytes = synchronousBytes(event?.data);
    if (bytes === null) {
      this.#finish(Reason.UnsupportedFrame);
      return;
    }
    if (bytes.length > limits.maxSipMessage) {
      // §4.9's inbound bound, and `docs/specs/sip-tls.md` §4's decoder bound, enforced before the
      // bytes are copied anywhere: an oversize frame must not buy an allocation.
      this.#finish(Reason.MessageTooLarge);
      return;
    }
    this.#counters.received += 1;
    this.#enqueue({ kind: "bytes", bytes, size: bytes.length });
  }

  /**
   * Append to the inbox — the binding's single ordered queue of everything that enters the kernel.
   *
   * One queue rather than three, because the kernel's output order is only reproducible if its
   * input order is total: a received message, a fired timer and a submitted command that arrive in
   * the same turn have to reach the kernel in a defined sequence, and "the order they arrived" is
   * the only one that needs no arbitration. Fired timers are additionally bounded by §4.9's 128
   * pending timers, so the bound below is reached by inbound traffic or by commands.
   */
  #enqueue(entry) {
    if (this.#state === "closed") {
      this.#counters.droppedAfterClose += 1;
      return;
    }
    const size = entry.size ?? 0;
    if (this.#inbox.length >= limits.receiveQueueEntries) {
      this.#emit({
        type: EventType.Overflow,
        queue: "receive",
        limit: limits.receiveQueueEntries,
        unit: "entries",
      });
      this.#finish(Reason.ReceiveOverflow);
      return;
    }
    if (this.#inboxBytes + size > limits.receiveQueueBytes) {
      this.#emit({
        type: EventType.Overflow,
        queue: "receive",
        limit: limits.receiveQueueBytes,
        unit: "bytes",
      });
      this.#finish(Reason.ReceiveOverflow);
      return;
    }
    this.#inbox.push(entry);
    this.#inboxBytes += size;
    this.#schedulePump();
  }

  #schedulePump() {
    if (this.#pumpScheduled || this.#state === "closed") return;
    this.#pumpScheduled = true;
    this.#clock.defer(() => this.#pump());
  }

  #pump() {
    this.#pumpScheduled = false;
    while (this.#inbox.length > 0 && this.#state !== "closed") {
      const entry = this.#inbox.shift();
      this.#inboxBytes -= entry.size ?? 0;
      this.#drive(entry);
    }
  }

  /**
   * The one place the kernel is entered.
   *
   * `now` is read here rather than when the input arrived, which is what makes §4.5's
   * non-decreasing requirement hold by construction: the inbox is FIFO and the clock is
   * monotonic, so a later entry cannot carry an earlier reading. It is truncated per §4.2 and
   * clamped against the last value handed over, because a platform clock is monotonic in its own
   * terms and this is cheaper than trusting that.
   */
  #drive(entry) {
    const nowMs = this.#monotonic();
    let records;
    try {
      switch (entry.kind) {
        case "bytes":
          records = this.#kernel.inputBytes(entry.bytes, nowMs);
          break;
        case "command":
          records = this.#kernel.command(entry.document, nowMs);
          break;
        case "timer":
          records = this.#kernel.inputTimer(entry.id, nowMs);
          break;
        case "entropy":
          this.#entropyQueued = false;
          this.#drivingEntropy = true;
          records = this.#kernel.inputEntropy(entry.bytes);
          break;
        default:
          records = [];
      }
    } catch (error) {
      // Legal host commands can be refused without poisoning the instance. Keep their
      // correlation id, but never expose document bytes or an exception message.
      if (
        entry.kind === "command" &&
        ["SipxStateError", "SipxLimitError"].includes(error?.name)
      ) {
        let id;
        try {
          id = JSON.parse(
            new TextDecoder("utf-8", { fatal: true }).decode(entry.document),
          ).id;
        } catch {}
        if (Number.isSafeInteger(id) && id > 0) {
          this.#emit({ type: "command-error", id, code: error.code });
          return;
        }
      }
      this.#drivingEntropy = false;
      this.#finish(Reason.KernelFault);
      return;
    }
    const produced = records ?? [];
    if (
      entry.kind === "bytes" &&
      produced.length === 0 &&
      this.#framingViolation()
    ) {
      this.#drivingEntropy = false;
      this.#finish(Reason.Framing);
      return;
    }
    this.#dispatch(produced);
    this.#drivingEntropy = false;
  }

  #monotonic() {
    const reading = Math.trunc(this.#clock.now());
    this.#lastNow = Math.max(
      this.#lastNow,
      Number.isFinite(reading) ? reading : this.#lastNow,
    );
    return this.#lastNow;
  }

  /**
   * Did the message the kernel just refused mean the peer disagrees about message boundaries?
   *
   * `docs/specs/sip-tls.md` §4 is normative and unambiguous: over WebSocket the frame boundary is
   * the message boundary, a message split across frames is malformed, two messages in one frame
   * likewise, and **both close the connection rather than being patched up**. This binding cannot
   * tell those two apart from any other unparseable input, and does not try — it asks the kernel
   * whether the message parsed, and applies the same rule the native transport applies. Joining
   * fragments or splitting a coalesced frame would require parsing SIP here, which would put a
   * second parser on the same hostile bytes (§8.1).
   *
   * Only consulted when the input produced no records at all, which a parse failure always does
   * (the kernel counts it and returns), so a healthy connection pays for this once per message it
   * had nothing to say about rather than once per message.
   */
  #framingViolation() {
    const seen = this.#readParseErrors();
    if (seen === null || seen <= this.#parseErrors) return false;
    this.#parseErrors = seen;
    return true;
  }

  /** The §4.11 snapshot's `parse_errors`, or `null` if the kernel cannot be asked. */
  #readParseErrors() {
    try {
      const seen = Number(this.#kernel.snapshot()?.counters?.parse_errors ?? 0);
      return Number.isFinite(seen) ? seen : null;
    } catch {
      return null;
    }
  }

  // ----------------------------------------------------------------------------------------
  // Dispatching what the kernel produced
  // ----------------------------------------------------------------------------------------

  #dispatch(records) {
    for (const record of records) {
      if (this.#state === "closed") return;
      switch (record.kind) {
        case "wire":
          this.#write(record.bytes);
          break;
        case "timer-set":
          this.#setTimer(record.id, record.fireAtMs);
          break;
        case "timer-cancel":
          this.#clearKernelTimer(record.id);
          break;
        case "event":
          this.#onKernelEvent(record.document);
          break;
        default:
          // §5.1: an unknown record from a newer kernel is ignored and counted as drift, never
          // guessed at. It cannot be a corrupt one — the kernel produced it.
          this.#counters.discarded += 1;
      }
    }
  }

  /** The only call site of `socket.send` in this package. */
  #write(bytes) {
    if (bytes.length > limits.maxSipMessage) {
      // §4.9: an outbound message over the bound is a kernel defect, not a peer's doing.
      this.#finish(Reason.KernelFault);
      return;
    }
    const socket = this.#socket;
    if (this.#state === "open" && socket) {
      const buffered = Number(socket.bufferedAmount ?? 0);
      if (buffered + bytes.length > limits.sendQueueBytes) {
        // Backpressure. The platform's outbound buffer is not draining, and the only two options
        // are to grow it without limit or to stop; §4.9's bound says which.
        this.#emit({
          type: EventType.Overflow,
          queue: "send",
          limit: limits.sendQueueBytes,
          unit: "bytes",
        });
        this.#finish(Reason.SendOverflow);
        return;
      }
      try {
        socket.send(bytes);
      } catch {
        this.#endSocket(Reason.RemoteClose, {});
        return;
      }
      this.#counters.sent += 1;
      return;
    }
    if (socket === null) {
      // No socket owns this message, so nothing may. Held bytes would be replayed onto whatever
      // connects next, which is the one thing `#endSocket` exists to prevent.
      this.#counters.discarded += 1;
      this.#emit({
        type: EventType.Discarded,
        scope: "send",
        reason: Reason.NoSocket,
        count: 1,
      });
      return;
    }
    if (this.#sendQueue.length >= limits.sendQueueRecords) {
      this.#emit({
        type: EventType.Overflow,
        queue: "send",
        limit: limits.sendQueueRecords,
        unit: "records",
      });
      this.#finish(Reason.SendOverflow);
      return;
    }
    if (this.#sendQueueBytes + bytes.length > limits.sendQueueBytes) {
      this.#emit({
        type: EventType.Overflow,
        queue: "send",
        limit: limits.sendQueueBytes,
        unit: "bytes",
      });
      this.#finish(Reason.SendOverflow);
      return;
    }
    this.#sendQueue.push(bytes);
    this.#sendQueueBytes += bytes.length;
  }

  #flushSendQueue() {
    const queued = this.#sendQueue;
    this.#sendQueue = [];
    this.#sendQueueBytes = 0;
    for (const bytes of queued) {
      if (this.#state !== "open") return;
      this.#write(bytes);
    }
  }

  #setTimer(id, fireAtMs) {
    const previous = this.#timers.get(id);
    if (previous !== undefined) this.#clock.clearTimer(previous);
    const delay = Math.max(0, fireAtMs - this.#lastNow);
    this.#timers.set(
      id,
      this.#clock.setTimer(() => this.#onTimer(id), delay),
    );
  }

  #clearKernelTimer(id) {
    const handle = this.#timers.get(id);
    if (handle === undefined) return;
    this.#clock.clearTimer(handle);
    this.#timers.delete(id);
  }

  /**
   * A host timer came due.
   *
   * It appends to the inbox and returns. The kernel is not called from here and cannot be: the
   * answer travels back through `sipx_input_timer` in a deferred task, which is what §4.5 means by
   * a timer re-entering as a fired-timer input. A callback for a timer this binding no longer owns
   * — cleared, already fired, or belonging to a cancelled binding — is counted and dropped, so the
   * platform race that `clearTimeout` cannot win is survivable rather than assumed away.
   */
  #onTimer(id) {
    if (this.#state === "closed" || !this.#timers.has(id)) {
      this.#counters.staleCallbacks += 1;
      return;
    }
    this.#timers.delete(id);
    this.#enqueue({ kind: "timer", id, size: 0 });
  }

  #onKernelEvent(document) {
    let event = null;
    try {
      event = JSON.parse(document);
    } catch {
      // The kernel promises canonical JSON (§5.1). One that does not keep that promise has a
      // defect, and the event is still forwarded verbatim so a diagnostic can see it.
    }
    if (event?.evt === "error" && event.fatal === true) {
      this.#emit({ type: EventType.Kernel, document, event });
      this.#finish(Reason.KernelFault);
      return;
    }
    if (event?.evt === "need-entropy" && !this.#refillEntropy()) return;
    this.#emit({ type: EventType.Kernel, document, event });
  }

  /**
   * Answer §4.7's entropy demand from the platform CSPRNG, once.
   *
   * Returns false when the binding has been torn down by the attempt. Two guards keep this from
   * becoming the unbounded loop `T-33` forbids: a demand raised while a refill is already queued
   * is the same demand, and a demand raised *by the refill itself* is impossible for a kernel
   * keeping §4.7 — after {@link limits.entropyRefill} octets the pool is far above the low-water
   * mark — so it is treated as the kernel defect it is instead of being answered again.
   */
  #refillEntropy() {
    if (this.#entropyQueued) return true;
    if (this.#drivingEntropy) {
      this.#finish(Reason.KernelFault);
      return false;
    }
    let bytes;
    try {
      bytes = this.#entropy.fill(limits.entropyRefill);
    } catch {
      // §8.4: there is no fallback generator. A platform that cannot produce entropy cannot run
      // this endpoint.
      this.#finish(Reason.KernelFault);
      return false;
    }
    this.#entropyQueued = true;
    this.#enqueue({ kind: "entropy", bytes, size: bytes.length });
    return this.#state !== "closed";
  }

  // ----------------------------------------------------------------------------------------
  // Events and teardown
  // ----------------------------------------------------------------------------------------

  /**
   * Queue one typed event for delivery in a later task.
   *
   * Never delivered inline: §6.4 rule 2 forbids a listener running inside an SDK method call or
   * inside the socket or timer handler that drove the kernel, and the cheapest way to keep that
   * rule is to have exactly one delivery path and defer all of it.
   */
  #emit(event) {
    if (this.#state === "closed") {
      this.#counters.droppedAfterClose += 1;
      return;
    }
    this.#queueForDelivery(event);
  }

  #queueForDelivery(event) {
    this.#pending.push(event);
    if (this.#deliveryScheduled) return;
    this.#deliveryScheduled = true;
    this.#clock.defer(() => this.#deliver());
  }

  #deliver() {
    this.#deliveryScheduled = false;
    while (this.#pending.length > 0) {
      const event = this.#pending.shift();
      try {
        this.#onEvent(event);
      } catch {
        // §6.4 rule 5: a throwing listener stops neither this event's siblings nor the next one.
        this.#counters.listenerFaults += 1;
      }
    }
  }

  /** §6.5's teardown, reached by every ending: cancellation, exhaustion, fault and refusal. */
  #finish(reason, detail = {}) {
    if (this.#state === "closed") return;
    this.#clearConnectTimer();
    this.#clearReconnectTimer();
    for (const handle of this.#timers.values()) this.#clock.clearTimer(handle);
    this.#timers.clear();
    this.#inbox = [];
    this.#inboxBytes = 0;
    this.#pumpScheduled = false;
    this.#sendQueue = [];
    this.#sendQueueBytes = 0;
    if (this.#unsubscribe) {
      try {
        this.#unsubscribe();
      } catch {
        // A monitor that will not unsubscribe is one this binding stops listening to anyway.
      }
      this.#unsubscribe = null;
    }
    // §6.5 step 4 before step 5: the kernel cancels its state, then the socket goes.
    try {
      this.#kernel.free();
    } catch {
      // Freeing twice, or freeing a kernel that already trapped, is not a reason to leak a socket.
    }
    const socket = this.#socket;
    this.#socket = null;
    if (socket) {
      this.#detach(socket);
      if (socket.readyState !== 3) {
        try {
          socket.close(1000, "");
        } catch {
          // Already gone.
        }
      }
    }
    this.#state = "closed";
    // Past the `#emit` guard deliberately: `closed` is the one event that must survive closing,
    // and it is the last one this binding will ever deliver.
    this.#queueForDelivery({
      type: EventType.Closed,
      reason,
      attempts: this.#attempts,
      ...detail,
    });
  }

  #clearConnectTimer() {
    if (this.#connectTimer === null) return;
    this.#clock.clearTimer(this.#connectTimer);
    this.#connectTimer = null;
  }

  #clearReconnectTimer() {
    if (this.#reconnectTimer === null) return;
    this.#clock.clearTimer(this.#reconnectTimer);
    this.#reconnectTimer = null;
  }
}

/**
 * The bytes of one received WebSocket message, or `null` if they are not available synchronously.
 *
 * `null` is returned rather than a promise on purpose. A `Blob` yields its bytes only through an
 * asynchronous read, and awaiting one per message lets a peer that sends two in the same turn have
 * them delivered in either order. Ordered delivery is not negotiable for a transaction state
 * machine, so a binding that cannot guarantee it refuses the frame instead — which is unreachable
 * in practice, because the socket is put in `arraybuffer` mode before it can carry anything.
 */
function synchronousBytes(data) {
  if (typeof data === "string") return encoder.encode(data);
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data))
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return null;
}
