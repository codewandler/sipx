// `T-33`'s acceptance, one case per property.
//
// Every case drives `WebSocketSignalling` through the fakes in `./fakes.mjs`: no socket, no
// clock, no entropy source and no network monitor is real, so each decision is made at the
// instant the case chooses and nothing waits on wall-clock time. `docs/specs/browser-signalling.md`
// is the contract; section references below are to it unless they name another document.

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { check, deepEqual, equal, test, throws } from "./assert.mjs";
import {
  FakeClock,
  FakeConnectivity,
  FakeEntropy,
  FakeKernel,
  FakeNetwork,
  Recorder,
  WSS_TRANSPORT,
  kernelEvent,
  timerCancel,
  timerSet,
  wire,
} from "./fakes.mjs";
import { decodeRecord } from "../src/records.mjs";
import {
  Reason,
  SIP_SUBPROTOCOL,
  SignallingConfigError,
  WebSocketSignalling,
  limits,
  signallingUrl,
} from "../src/transport.mjs";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** `BSDK-EVT-1` (`docs/specs/browser-sdk.md` §9.2), byte for byte. */
const BSDK_EVT_1 = '{"v":1,"evt":"need-entropy","min":64}';
/** `BSDK-EVT-2`, a registration state event — a kernel event with no obligation attached. */
const BSDK_EVT_2 = '{"v":1,"evt":"registration","state":"registered","expires":600}';
/** `BSDK-CMD-1`, the register command. */
const BSDK_CMD_1 = '{"v":1,"cmd":"register","id":1,"expires":600}';

/** One assembled transport plus every fake it was built from. */
function harness({ transport = WSS_TRANSPORT, insecure = "refuse", respond = () => [] } = {}) {
  const clock = new FakeClock();
  const network = new FakeNetwork();
  const kernel = new FakeKernel(respond);
  const entropy = new FakeEntropy();
  const connectivity = new FakeConnectivity();
  const recorder = new Recorder();
  const signalling = new WebSocketSignalling({
    transport,
    insecure,
    kernel,
    openSocket: network.factory,
    clock,
    entropy,
    connectivity,
    onEvent: recorder.sink,
  });
  return { signalling, clock, network, kernel, entropy, connectivity, recorder };
}

/** Bring a harness to an open socket with the `sip` subprotocol negotiated. */
function connected(options) {
  const parts = harness(options);
  parts.signalling.start();
  parts.clock.flush();
  parts.network.latest.acceptOpen(SIP_SUBPROTOCOL);
  parts.clock.flush();
  return parts;
}

const text = (bytes) => decoder.decode(bytes);

/**
 * Assert that a configuration is refused, typed, with the named reason.
 *
 * The reason is checked to be a string as well as to be equal: an assertion that only compared
 * `error.reason` to `Reason.Whatever` would hold when both are `undefined`, which is exactly what
 * an unimplemented binding looks like.
 */
function refuses(body, reason, description) {
  const error = throws(body, description);
  check(error instanceof SignallingConfigError, `${description}: a typed configuration error`);
  check(typeof error.reason === "string" && error.reason !== "", `${description}: names a reason`);
  equal(error.reason, reason, `${description}: the reason`);
  return error;
}

// ============================================================================================
// Acceptance 1 — opens the socket, selects the subprotocol, feeds bytes in, sends only bytes out
// ============================================================================================

test("the URL is built from the transport block, WSS by default", () => {
  equal(signallingUrl(WSS_TRANSPORT, "refuse"), "wss://edge.example.net/sip", "BSDK-CFG-1's URL");
});

test("start() opens one socket and asks for the sip subprotocol", () => {
  const { signalling, network, clock } = harness();
  signalling.start();
  clock.flush();
  equal(network.attempts, 1, "exactly one socket was constructed");
  equal(network.latest.url, "wss://edge.example.net/sip", "the socket's URL");
  deepEqual(network.latest.requestedProtocols, [SIP_SUBPROTOCOL], "RFC 7118 §4's subprotocol");
});

test("the socket is put in arraybuffer mode before the handshake completes", () => {
  const { signalling, network, clock } = harness();
  signalling.start();
  clock.flush();
  equal(network.latest.binaryType, "arraybuffer", "binaryType, set before any message can arrive");
});

test("a peer that selected no subprotocol is refused and not retried", () => {
  const { signalling, network, clock, recorder } = harness();
  signalling.start();
  clock.flush();
  network.latest.acceptOpen("");
  clock.flush();
  clock.advance(60_000);
  equal(recorder.ofType("closed")[0]?.reason, Reason.SubprotocolRefused, "typed refusal");
  equal(network.attempts, 1, "a deterministic refusal is not retried");
  equal(signalling.state, "closed", "terminal");
});

test("a received message is handed to the kernel verbatim, exactly once", () => {
  const { network, kernel, clock } = connected();
  network.latest.deliver("SIP/2.0 200 OK\r\n\r\n");
  clock.flush();
  const delivered = kernel.of("bytes");
  equal(delivered.length, 1, "one inputBytes call");
  equal(text(delivered[0].bytes), "SIP/2.0 200 OK\r\n\r\n", "the bytes, unchanged");
});

test("the socket is written only from WIRE records", () => {
  const register = "REGISTER sip:example.net SIP/2.0\r\n\r\n";
  const { signalling, network, kernel, clock } = connected({
    respond: (call) => (call.kind === "command" ? [wire(register)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(network.latest.sent.length, 1, "one WebSocket message");
  equal(text(network.latest.sent[0]), register, "exactly the kernel's WIRE payload");
  check(
    !network.latest.sent.some((sent) => text(sent).includes('"cmd"')),
    "the submitted command document never reached the socket",
  );
  equal(kernel.of("command").length, 1, "the command reached the kernel");
});

// ============================================================================================
// Acceptance 2 — WSS is the default; cleartext needs the policy; credentials never leak
// ============================================================================================

test("plain ws without the development policy is refused", () => {
  refuses(
    () => signallingUrl({ scheme: "ws", host: "localhost", resource: "/sip" }, "refuse"),
    Reason.InsecureScheme,
    "cleartext under the default policy",
  );
});

test("plain ws with the development policy is allowed against loopback", () => {
  equal(
    signallingUrl({ scheme: "ws", host: "localhost:8080", resource: "/sip" }, "allow-development"),
    "ws://localhost:8080/sip",
    "the development opt-in",
  );
});

test("the development policy does not extend cleartext beyond loopback", () => {
  refuses(
    () =>
      signallingUrl({ scheme: "ws", host: "edge.example.net", resource: "/sip" }, "allow-development"),
    Reason.CleartextNotLoopback,
    "cleartext to a routable host",
  );
});

test("an unknown scheme is refused", () => {
  refuses(
    () => signallingUrl({ scheme: "https", host: "edge.example.net", resource: "/" }, "refuse"),
    Reason.UnsupportedScheme,
    "a scheme that is not ws or wss",
  );
});

test("userinfo in the host is refused — credentials never enter a URL", () => {
  const error = refuses(
    () => signallingUrl({ ...WSS_TRANSPORT, host: "alice:secret@edge.example.net" }, "refuse"),
    Reason.CredentialsInUrl,
    "userinfo",
  );
  check(!`${error.message}`.includes("secret"), "and the refusal does not quote the credential");
});

test("a query string or fragment in the resource is refused", () => {
  for (const resource of ["/sip?token=abc", "/sip#alice"]) {
    refuses(
      () => signallingUrl({ ...WSS_TRANSPORT, resource }, "refuse"),
      Reason.ResourceCarriesQuery,
      `resource ${resource}`,
    );
  }
});

test("no typed event carries the configured credential", () => {
  const challenged =
    'REGISTER sip:example.net SIP/2.0\r\nAuthorization: Digest response="0123456789abcdef"\r\n\r\n';
  const { signalling, network, clock, recorder } = connected({
    respond: (call) => (call.kind === "command" ? [wire(challenged)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  network.latest.remoteClose({ code: 1006 });
  clock.flush();
  const rendered = JSON.stringify(recorder.events);
  check(!rendered.includes("secret"), "no password in any event");
  check(!rendered.includes("0123456789abcdef"), "no digest response in any event");
  check(!rendered.includes("Authorization"), "no authorization header in any event");
});

// ============================================================================================
// Acceptance 3 — bounded queues, typed events, and no unbounded retry
// ============================================================================================

test("the derived queue bounds still match the kernel's §4.9 constants", async () => {
  const bounds = await readFile(
    fileURLToPath(new URL("../../crates/sipx-wasm/src/bounds.rs", import.meta.url)),
    "utf8",
  );
  const constant = (name) => {
    const match = bounds.match(new RegExp(`pub const ${name}: usize = ([^;]+);`));
    if (!match) throw new Error(`${name} is not declared in bounds.rs`);
    // The declarations are arithmetic over literals — `256 * 1024` and friends.
    return match[1]
      .split("*")
      .map((part) => Number(part.trim()))
      .reduce((left, right) => left * right, 1);
  };
  equal(limits.sendQueueRecords, constant("MAX_QUEUED_RECORDS"), "send queue records");
  equal(limits.sendQueueBytes, constant("MAX_QUEUED_BYTES"), "send queue bytes");
  equal(limits.receiveQueueEntries, constant("MAX_QUEUED_RECORDS"), "receive queue entries");
  equal(limits.receiveQueueBytes, constant("MAX_QUEUED_BYTES"), "receive queue bytes");
  equal(limits.maxSipMessage, constant("MAX_SIP_MESSAGE"), "one SIP message");
  equal(
    limits.entropyRefill,
    constant("ENTROPY_CAPACITY") - constant("ENTROPY_LOW_WATER"),
    "a refill that cannot overflow the pool",
  );
});

test("the send queue is bounded and its overflow closes the transport typed", () => {
  const { signalling, network, clock, recorder } = harness({
    respond: (call) => (call.kind === "command" ? [wire("MESSAGE sip:b SIP/2.0\r\n\r\n")] : []),
  });
  signalling.start();
  clock.flush();
  // The socket has not opened, so every WIRE record queues.
  for (let index = 0; index <= limits.sendQueueRecords; index += 1) {
    signalling.submit(BSDK_CMD_1);
    clock.flush();
  }
  const overflow = recorder.ofType("overflow")[0];
  equal(overflow?.queue, "send", "the send queue overflowed");
  equal(overflow?.limit, limits.sendQueueRecords, "at its stated limit");
  equal(recorder.ofType("closed")[0]?.reason, Reason.SendOverflow, "and the transport failed closed");
  equal(network.latest.closed !== null, true, "the socket was closed");
});

test("backpressure past the buffered-amount bound fails closed rather than growing", () => {
  const { signalling, network, clock, recorder } = connected({
    respond: (call) => (call.kind === "command" ? [wire("MESSAGE sip:b SIP/2.0\r\n\r\n")] : []),
  });
  network.latest.bufferedAmount = limits.sendQueueBytes;
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(recorder.ofType("overflow")[0]?.queue, "send", "backpressure is a send-queue overflow");
  equal(recorder.ofType("closed")[0]?.reason, Reason.SendOverflow, "typed close");
  equal(network.latest.sent.length, 0, "and nothing was pushed into the buffer");
});

test("the receive queue is bounded and its overflow closes the transport typed", () => {
  const { network, clock, recorder, kernel } = connected();
  const message = "SIP/2.0 200 OK\r\n\r\n";
  for (let index = 0; index <= limits.receiveQueueEntries; index += 1) {
    network.latest.deliver(message);
  }
  clock.flush();
  const overflow = recorder.ofType("overflow")[0];
  equal(overflow?.queue, "receive", "the receive queue overflowed");
  equal(overflow?.limit, limits.receiveQueueEntries, "at its stated limit");
  equal(recorder.ofType("closed")[0]?.reason, Reason.ReceiveOverflow, "typed close");
  check(kernel.of("bytes").length <= limits.receiveQueueEntries, "the kernel saw no more than the bound");
});

test("an inbound message over the §4.9 bound never reaches the kernel", () => {
  const { network, clock, recorder, kernel } = connected();
  network.latest.deliver("A".repeat(limits.maxSipMessage + 1));
  clock.flush();
  equal(kernel.of("bytes").length, 0, "not offered to the kernel");
  equal(recorder.ofType("closed")[0]?.reason, Reason.MessageTooLarge, "typed close");
});

test("reconnection is bounded by a stated attempt budget and ends typed", () => {
  const { signalling, network, clock, recorder } = harness();
  signalling.start();
  clock.flush();
  for (let index = 0; index < limits.connectAttempts * 2; index += 1) {
    if (network.latest.closed === null && network.latest.readyState !== 3) {
      network.latest.remoteClose({ code: 1006 });
    }
    clock.flush();
    clock.advance(limits.connectTimeoutMs);
  }
  equal(network.attempts, limits.connectAttempts, "never more sockets than the budget allows");
  equal(recorder.ofType("closed")[0]?.reason, Reason.ConnectExhausted, "typed exhaustion");
  equal(recorder.ofType("closed")[0]?.attempts, limits.connectAttempts, "reported with the count");
  equal(signalling.state, "closed", "terminal — the loop stopped");
});

test("each reconnect decision is a typed event carrying its attempt and delay", () => {
  const { signalling, network, clock, recorder } = harness();
  signalling.start();
  clock.flush();
  for (let index = 0; index < limits.connectAttempts * 2; index += 1) {
    if (network.latest.closed === null && network.latest.readyState !== 3) {
      network.latest.remoteClose({ code: 1006 });
    }
    clock.flush();
    clock.advance(limits.connectTimeoutMs);
  }
  const scheduled = recorder.ofType("reconnect-scheduled");
  deepEqual(
    scheduled.map((event) => event.delayMs),
    [...limits.reconnectDelaysMs],
    "the backoff schedule, in order",
  );
  deepEqual(
    scheduled.map((event) => event.attempt),
    limits.reconnectDelaysMs.map((_, index) => index + 2),
    "each names the attempt it will make",
  );
});

test("a connection that opens and drops repeatedly cannot outrun the budget", () => {
  const { signalling, network, clock } = harness();
  signalling.start();
  clock.flush();
  for (let index = 0; index < limits.connectAttempts * 3; index += 1) {
    const socket = network.latest;
    if (socket.readyState !== 3) {
      socket.acceptOpen(SIP_SUBPROTOCOL);
      clock.flush();
      socket.remoteClose({ code: 1006 });
    }
    clock.flush();
    clock.advance(limits.connectTimeoutMs);
  }
  equal(network.attempts, limits.connectAttempts, "a stable open does not replenish the budget");
});

test("going offline suspends reconnection and coming back does not replenish the budget", () => {
  const { signalling, network, clock, recorder, connectivity } = harness();
  signalling.start();
  clock.flush();
  network.latest.remoteClose({ code: 1006 });
  clock.flush();
  equal(recorder.ofType("reconnect-scheduled").length, 1, "the first failure scheduled a retry");
  clock.advance(limits.reconnectDelaysMs[0]);
  equal(network.attempts, 2, "which happened");

  connectivity.goOffline();
  network.latest.remoteClose({ code: 1006 });
  clock.flush();
  clock.advance(300_000);
  equal(recorder.ofType("offline").length, 1, "offline is a typed event");
  equal(network.attempts, 2, "and nothing is attempted while the browser is offline");

  connectivity.goOnline();
  clock.flush();
  clock.advance(limits.reconnectDelaysMs[1]);
  equal(recorder.ofType("online").length, 1, "online is a typed event");
  equal(network.attempts, 3, "the suspended attempt resumes");

  for (let index = 0; index < limits.connectAttempts * 3; index += 1) {
    if (network.latest.readyState !== 3) network.latest.remoteClose({ code: 1006 });
    clock.flush();
    clock.advance(limits.connectTimeoutMs);
  }
  equal(network.attempts, limits.connectAttempts, "the offline period bought no extra attempts");
});

// ============================================================================================
// Acceptance 4 — timers re-enter only as fired-timer inputs; cancellation clears everything
// ============================================================================================

test("a TIMER_SET is scheduled on the host clock at its absolute deadline", () => {
  const { signalling, clock, kernel } = connected({
    respond: (call) => (call.kind === "command" ? [timerSet(7, 500)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(clock.pending, 1, "one host timer is owned");
  deepEqual(clock.requested.slice(-1), [500], "the delay is fire_at_ms minus now");
  equal(kernel.of("timer").length, 0, "and nothing has re-entered the kernel yet");
});

test("a fired timer re-enters only as a fired-timer input, never from the callback", () => {
  const { signalling, clock, kernel } = connected({
    respond: (call) => (call.kind === "command" ? [timerSet(7, 500)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();

  clock.advanceWithoutFlush(500);
  equal(kernel.of("timer").length, 0, "the platform callback did not call the kernel");

  clock.flush();
  const fired = kernel.of("timer");
  equal(fired.length, 1, "the deferred task did");
  equal(fired[0].id, 7n, "carrying the timer id the kernel asked for");
});

test("a TIMER_CANCEL clears the host timer it names", () => {
  const script = [[timerSet(7, 500), timerSet(8, 900)], [timerCancel(7)]];
  const { signalling, clock } = connected({
    respond: (call) => (call.kind === "command" ? (script.shift() ?? []) : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(clock.pending, 2, "two host timers");
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(clock.pending, 1, "the cancelled one is gone");
});

test("cancellation closes the socket and releases every timer and callback it owns", () => {
  const { signalling, network, clock, kernel, connectivity } = connected({
    respond: (call) => (call.kind === "command" ? [timerSet(7, 500), timerSet(8, 900)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  const socket = network.latest;
  equal(clock.pending, 2, "timers are owned before cancellation");

  signalling.cancel();

  equal(clock.pending, 0, "every host timer was cleared");
  check(socket.closed !== null, "the socket was closed");
  equal(socket.attachedHandlers, 0, "every socket handler was detached");
  equal(connectivity.subscribers, 0, "the connectivity subscription was released");
  equal(kernel.freed, true, "the kernel was freed (§6.5 step 4, before the socket closes)");
  equal(signalling.state, "closed", "terminal");
});

test("a stale timer callback after cancellation reaches neither the kernel nor a listener", () => {
  const { signalling, clock, kernel, recorder } = connected({
    respond: (call) => (call.kind === "command" ? [timerSet(7, 500)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  signalling.cancel();
  clock.flush(); // the final `closed` event is delivered; nothing may follow it (§6.4 rule 6)
  const kernelCalls = kernel.calls.length;
  const events = recorder.events.length;
  equal(recorder.events[recorder.events.length - 1]?.type, "closed", "closed was the last event");

  clock.fireEveryRetainedCallback();

  equal(kernel.calls.length, kernelCalls, "the kernel was not entered");
  equal(recorder.events.length, events, "no listener ran");
  check(signalling.counters.staleCallbacks > 0, "and the race was counted rather than ignored");
});

test("a stale socket callback after cancellation reaches neither the kernel nor a listener", () => {
  const { signalling, network, clock, kernel, recorder } = connected();
  const socket = network.latest;
  signalling.cancel();
  clock.flush(); // as above: the final `closed` event has been delivered before we snapshot
  const kernelCalls = kernel.calls.length;
  const events = recorder.events.length;

  socket.deliverRegardless("message", { data: "SIP/2.0 200 OK\r\n\r\n" });
  socket.deliverRegardless("close", { code: 1006, reason: "", wasClean: false });
  clock.flush();

  equal(kernel.calls.length, kernelCalls, "the kernel was not entered");
  equal(recorder.events.length, events, "no listener ran");
});

// ============================================================================================
// Acceptance 5 — fragmentation, coalescing, close during authentication, non-root paths
// ============================================================================================

/** A kernel that reports what the real one reports for unparseable bytes: nothing, and a count. */
const countsAParseError = (call, kernel) => {
  if (call.kind === "bytes") kernel.counters.parse_errors += 1;
  return [];
};

test("a SIP message split across two WebSocket messages is refused, never joined", () => {
  const { network, clock, kernel, recorder } = connected({ respond: countsAParseError });
  network.latest.deliver("REGISTER sip:example.net SIP/2.0\r\nVia: SIP/2.0/WSS ");
  clock.flush();
  const delivered = kernel.of("bytes");
  equal(delivered.length, 1, "the fragment was offered once");
  equal(
    text(delivered[0].bytes),
    "REGISTER sip:example.net SIP/2.0\r\nVia: SIP/2.0/WSS ",
    "verbatim — the binding holds no cross-message buffer to join it with",
  );
  equal(recorder.ofType("closed")[0]?.reason, Reason.Framing, "sip-tls.md §4: the peer is refused");
  check(network.latest.closed !== null, "the socket was closed");
});

test("two SIP messages in one WebSocket message are offered as one, never split", () => {
  const coalesced = "SIP/2.0 100 Trying\r\n\r\nSIP/2.0 200 OK\r\n\r\n";
  const { network, clock, kernel, recorder } = connected({ respond: countsAParseError });
  network.latest.deliver(coalesced);
  clock.flush();
  const delivered = kernel.of("bytes");
  equal(delivered.length, 1, "one frame is one offer");
  equal(text(delivered[0].bytes), coalesced, "verbatim — the binding does not look for a boundary");
  // Splitting the frame would mean parsing SIP here, which §8.1 concentrates in the kernel on
  // purpose. Whether the trailing bytes are refused is therefore the kernel's verdict, and the
  // binding acts on it: this fake reports the parse error a fragment earns, and the connection
  // closes. `S-53` covers the case where the kernel does not report one.
  equal(recorder.ofType("closed")[0]?.reason, Reason.Framing, "sip-tls.md §4: the peer is refused");
});

test("a well-formed message after a framing violation is never processed", () => {
  const { network, clock, kernel } = connected({ respond: countsAParseError });
  const socket = network.latest;
  socket.deliver("garbage");
  clock.flush();
  socket.deliverRegardless("message", { data: "SIP/2.0 200 OK\r\n\r\n" });
  clock.flush();
  equal(kernel.of("bytes").length, 1, "the connection ended at the violation");
});

test("a Blob message is refused rather than read asynchronously out of order", () => {
  const { network, clock, kernel, recorder } = connected();
  network.latest.deliver({ size: 12, type: "" }); // a Blob-shaped value: no bytes, only a promise
  clock.flush();
  equal(kernel.of("bytes").length, 0, "never offered — reading it would reorder delivery");
  equal(recorder.ofType("closed")[0]?.reason, Reason.UnsupportedFrame, "typed close");
});

/** A challenge answer: the one message that must never be replayed onto a connection nobody asked
 * for. The digest response is a recognisable constant so a case can prove it went nowhere. */
const CHALLENGE_ANSWER =
  'REGISTER sip:example.net SIP/2.0\r\nAuthorization: Digest response="0123456789abcdef"\r\n\r\n';

test("a close during authentication discards the challenge answer instead of replaying it", () => {
  const { network, clock, recorder } = connected({
    respond: (call) => (call.kind === "bytes" ? [wire(CHALLENGE_ANSWER)] : []),
  });
  const first = network.latest;
  // The 401 arrives and the socket dies in the same turn, so the kernel's answer to the challenge
  // is produced when there is no longer a connection that asked for it.
  first.deliver("SIP/2.0 401 Unauthorized\r\n\r\n");
  first.remoteClose({ code: 1006 });
  clock.flush();

  const discarded = recorder.ofType("discarded");
  check(discarded.length >= 1, "the answer to the challenge was discarded, typed");
  equal(discarded[0]?.reason, Reason.NoSocket, "because no socket owned it");

  clock.advance(limits.reconnectDelaysMs[0]);
  network.latest.acceptOpen(SIP_SUBPROTOCOL);
  clock.flush();
  equal(network.latest.sent.length, 0, "nothing was replayed onto the new socket");
  check(
    !JSON.stringify(recorder.events).includes("0123456789abcdef"),
    "and the credential is in no event",
  );
});

test("a message queued for a socket that never opened dies with that socket", () => {
  const { signalling, network, clock, recorder } = harness({
    respond: (call) => (call.kind === "command" ? [wire(CHALLENGE_ANSWER)] : []),
  });
  signalling.start();
  clock.flush();
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  network.latest.remoteClose({ code: 1006 });
  clock.flush();

  const disconnected = recorder.ofType("disconnected")[0];
  equal(disconnected?.reason, Reason.RemoteClose, "the drop is typed");
  check(disconnected?.dropped >= 1, "and says how many queued messages it discarded");

  clock.advance(limits.reconnectDelaysMs[0]);
  network.latest.acceptOpen(SIP_SUBPROTOCOL);
  clock.flush();
  equal(network.latest.sent.length, 0, "nothing was replayed onto the new socket");
});

test("a non-root resource reaches the socket exactly as configured", () => {
  const transport = { ...WSS_TRANSPORT, resource: "/sip/edge/v1" };
  equal(signallingUrl(transport, "refuse"), "wss://edge.example.net/sip/edge/v1", "the URL");
  const { signalling, network, clock } = harness({ transport });
  signalling.start();
  clock.flush();
  equal(network.latest.url, "wss://edge.example.net/sip/edge/v1", "as the socket saw it");
});

test("an empty resource means the root, and a relative one is refused", () => {
  equal(signallingUrl({ ...WSS_TRANSPORT, resource: "" }, "refuse"), "wss://edge.example.net/", "root");
  const error = throws(
    () => signallingUrl({ ...WSS_TRANSPORT, resource: "sip" }, "refuse"),
    "a resource with no leading slash",
  );
  equal(error.reason, Reason.InvalidResource, "reason");
});

// ============================================================================================
// §4.6 framing and §4.7 entropy
// ============================================================================================

test("BSDK-OUT-2 decodes to the TIMER_SET it frames", () => {
  const octets = Uint8Array.from([
    0x02, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xf4, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  ]);
  equal(octets.length, 24, "BSDK-OUT-2 is 24 octets");
  const record = decodeRecord(octets);
  equal(record.kind, "timer-set", "type 2");
  equal(record.id, 1n, "timer id");
  equal(record.fireAtMs, 500, "fire_at_ms");
});

test("BSDK-OUT-1 decodes to the need-entropy event it frames", () => {
  const payload = encoder.encode(BSDK_EVT_1);
  equal(payload.length, 37, "BSDK-EVT-1 is 37 octets");
  const octets = new Uint8Array(8 + payload.length);
  new DataView(octets.buffer).setUint32(0, 4, true);
  new DataView(octets.buffer).setUint32(4, payload.length, true);
  octets.set(payload, 8);
  equal(octets.length, 45, "BSDK-OUT-1 is 45 octets");
  const record = decodeRecord(octets);
  equal(record.kind, "event", "type 4");
  equal(record.document, BSDK_EVT_1, "the event document, verbatim");
});

test("the pool is seeded from the platform CSPRNG before the first command, unasked", () => {
  const { kernel, entropy } = connected();
  deepEqual(entropy.requests, [limits.entropyRefill], "one refill, drawn at start()");
  const fed = kernel.of("entropy");
  equal(fed.length, 1, "fed to the kernel once");
  equal(fed[0].bytes.length, limits.entropyRefill, "with the whole refill");
  equal(kernel.calls[0]?.kind, "entropy", "and before anything else reached the kernel");
});

test("need-entropy is answered with a refill that cannot overflow the pool", () => {
  const { signalling, clock, kernel, entropy } = connected({
    respond: (call) => (call.kind === "command" ? [kernelEvent(BSDK_EVT_1)] : []),
  });
  const seeded = entropy.requests.length;
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  deepEqual(
    entropy.requests.slice(seeded),
    [limits.entropyRefill],
    "one further refill, sized to the headroom above the low-water mark",
  );
  equal(kernel.of("entropy").length, seeded + 1, "fed once more");
});

test("a kernel that asks for entropy again immediately is a fault, not a refill loop", () => {
  // The seed is answered normally; the refill that answers the *demand* asks again, which a
  // kernel keeping §4.7 cannot do — the pool is far above the low-water mark by then.
  let fedAfterCommand = false;
  const { signalling, clock, entropy, recorder } = connected({
    respond: (call) => {
      if (call.kind === "command") {
        fedAfterCommand = true;
        return [kernelEvent(BSDK_EVT_1)];
      }
      return call.kind === "entropy" && fedAfterCommand ? [kernelEvent(BSDK_EVT_1)] : [];
    },
  });
  const seeded = entropy.requests.length;
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(entropy.requests.length, seeded + 1, "exactly one further refill was ever drawn");
  equal(recorder.ofType("closed")[0]?.reason, Reason.KernelFault, "and the loop was refused typed");
});

test("a fatal kernel error tears the transport down", () => {
  const fatal = '{"v":1,"evt":"error","fatal":true,"code":"E_POISONED","reason":"internal"}';
  const { signalling, network, clock, recorder } = connected({
    respond: (call) => (call.kind === "command" ? [kernelEvent(fatal)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  equal(recorder.ofType("closed")[0]?.reason, Reason.KernelFault, "typed close");
  check(network.latest.closed !== null, "the socket was closed");
  equal(signalling.state, "closed", "terminal");
});

test("kernel events reach the application unaltered", () => {
  const { signalling, clock, recorder } = connected({
    respond: (call) => (call.kind === "command" ? [kernelEvent(BSDK_EVT_1)] : []),
  });
  signalling.submit(BSDK_CMD_1);
  clock.flush();
  const forwarded = recorder.ofType("kernel");
  equal(forwarded.length, 1, "one kernel event");
  equal(forwarded[0].document, BSDK_EVT_1, "the document, byte for byte");
});

test("no listener runs synchronously inside a socket handler or an SDK call", () => {
  const { signalling, network, clock, recorder } = connected({
    respond: (call) => (call.kind === "entropy" ? [] : [kernelEvent(BSDK_EVT_2)]),
  });
  const before = recorder.events.length;
  network.latest.deliver("SIP/2.0 200 OK\r\n\r\n");
  equal(recorder.events.length, before, "the message handler dispatched nothing (§6.4 rule 2)");
  signalling.submit(BSDK_CMD_1);
  equal(recorder.events.length, before, "nor did submit()");
  clock.flush();
  check(recorder.events.length > before, "the deferred task did");
});
