import test from "node:test";
import assert from "node:assert/strict";
import { FakeClock } from "./fakes.mjs";
async function harness() {
  const { createClientForTest } = await import("../src/client.mjs");
  const clock = new FakeClock(),
    commands = [],
    resources = { closed: false, calls: [] };
  let transportEvent, mediaHooks;
  const clientPromise = createClientForTest(
    {},
    {
      clock,
      kernel: {},
      makeTransport: (opts) => {
        transportEvent = opts.onEvent;
        return {
          start() {
            transportEvent({ type: "open" });
          },
          submit(s) {
            commands.push(JSON.parse(s));
          },
          cancel() {
            resources.closed = true;
          },
          counters: {},
        };
      },
      makeMedia: (opts) => {
        mediaHooks = opts;
        return {
          handle(e) {
            resources.calls.push(e);
          },
          closeCall(n) {
            resources.calls.push({ closed: n });
          },
          close() {
            resources.mediaClosed = true;
          },
          negotiatedMedia() {
            return {};
          },
        };
      },
    },
  );
  clock.flush();
  const client = await clientPromise;
  return {
    client,
    clock,
    commands,
    resources,
    emit: (e) => {
      transportEvent({ type: "kernel", event: e });
      clock.flush();
    },
    media: (e) => {
      mediaHooks.onState(e);
      clock.flush();
    },
    error: (e) => {
      mediaHooks.onError(e);
      clock.flush();
    },
    transport: (e) => {
      transportEvent(e);
      clock.flush();
    },
  };
}
test("registration callbacks precede settlement and throwing listeners are isolated", async () => {
  const h = await harness(),
    order = [];
  h.client.on("state", () => {
    throw Error("private");
  });
  h.client.on("state", (s) => order.push(s));
  const p = h.client.register().then(() => order.push("settled"));
  h.clock.flush();
  const id = h.commands[0].id;
  h.emit({ evt: "registration", state: "registered" });
  h.emit({ evt: "outcome", id, ok: true });
  await p;
  assert.deepEqual(order, ["registered", "settled"]);
  assert.equal(h.client.diagnostics.listener_errors, 1);
  await finish(h);
});
test("dial cannot settle from SIP establishment alone; media gate and ordered state complete it", async () => {
  const h = await harness();
  let settled = false;
  const p = h.client.dial("sip:bob@example.net").then((c) => {
    settled = true;
    return c;
  });
  h.clock.flush();
  const id = h.commands[0].id;
  h.emit({ evt: "call", call: 1, dir: "out", state: "dialing" });
  h.emit({ evt: "call", call: 1, dir: "out", state: "sipEstablished" });
  h.emit({ evt: "outcome", id, ok: true });
  await Promise.resolve();
  assert.equal(settled, false);
  h.media({ call: 1, state: "established" });
  const call = await p;
  assert.equal(call.state, "established");
  await finish(h);
});
test("abort before permission skips capture and settles only after kernel terminal cleanup", async () => {
  const h = await harness(),
    abort = new AbortController();
  let done = false;
  const p = h.client
    .dial("sip:bob@example.net", { signal: abort.signal })
    .catch((e) => {
      done = true;
      return e;
    });
  abort.abort();
  h.clock.flush();
  const id = h.commands[0].id;
  h.emit({ evt: "call", call: 1, dir: "out", state: "dialing" });
  h.emit({ evt: "need-local-media", call: 1, kind: "offer" });
  assert.equal(
    h.resources.calls.some((e) => e.evt === "need-local-media"),
    false,
  );
  assert.ok(h.commands.some((c) => c.cmd === "hangup"));
  await Promise.resolve();
  assert.equal(done, false);
  h.emit({ evt: "call-ended", call: 1, cause: { class: "local" } });
  h.emit({ evt: "outcome", id, ok: false, error: { code: "cancelled" } });
  assert.equal((await p).name, "SipxCancelled");
  assert.ok(h.resources.calls.some((e) => e.closed === 1));
  await finish(h);
});
test("close deadline frees all resources and delivers closed before resolving with no later callbacks", async () => {
  const h = await harness(),
    order = [];
  h.client.on("state", (s) => order.push(s));
  h.client.on("closed", () => order.push("closed-event"));
  const p = h.client.close({ timeoutMs: 5 }).then(() => order.push("resolved"));
  h.clock.flush();
  h.clock.advance(5);
  h.clock.flush();
  await p;
  const previous = [...order];
  h.emit({ evt: "registration", state: "registered" });
  assert.deepEqual(order, previous);
  assert.deepEqual(order.slice(-3), ["closed", "closed-event", "resolved"]);
  assert.equal(h.resources.closed, true);
  assert.equal(h.resources.mediaClosed, true);
  assert.equal(h.clock.pending, 0);
});
test("nonfatal command state rejection leaves the client usable", async () => {
  const h = await harness();
  const p = h.client.register().catch((e) => e);
  h.clock.flush();
  h.transport({ type: "command-error", id: h.commands[0].id, code: -6 });
  assert.equal((await p).name, "SipxStateError");
  assert.equal(h.client.state, "connected");
  await finish(h);
});
async function finish(h) {
  const p = h.client.close({ timeoutMs: 1 });
  h.clock.flush();
  h.clock.advance(1);
  h.clock.flush();
  await p;
}
test("outgoing handle is delivered before readiness and can hang up during INVITE", async () => {
  const h = await harness();
  let call, hangup;
  h.client.on("outgoing", (c) => {
    call = c;
  });
  const dial = h.client.dial("sip:bob@example.net").catch((e) => e);
  h.clock.flush();
  const dialId = h.commands[0].id;
  h.emit({ evt: "call", call: 1, dir: "out", state: "dialing" });
  assert.equal(call.state, "dialing");
  h.emit({ evt: "call", call: 1, dir: "out", state: "inviteSent" });
  hangup = call.hangup();
  h.clock.flush();
  const hangupId = h.commands.at(-1).id;
  h.emit({ evt: "outcome", id: hangupId, ok: true });
  h.emit({ evt: "call-ended", call: 1, cause: { class: "local" } });
  h.emit({
    evt: "outcome",
    id: dialId,
    ok: false,
    error: { code: "cancelled" },
  });
  await hangup;
  assert.equal((await dial).name, "SipxCancelled");
  await finish(h);
});
test("close while registration is pending sends inverse and rejects the original after cleanup", async () => {
  const h = await harness();
  const registration = h.client.register().catch((e) => e);
  h.clock.flush();
  h.emit({ evt: "registration", state: "registering" });
  const closing = h.client.close({ timeoutMs: 5 });
  h.clock.flush();
  assert.ok(h.commands.some((c) => c.cmd === "unregister"));
  h.clock.advance(5);
  h.clock.flush();
  await closing;
  assert.equal((await registration).name, "SipxCancelled");
  assert.equal(h.resources.closed, true);
});
test("close during INVITE waits boundedly then releases resources before rejecting dial", async () => {
  const h = await harness();
  const dial = h.client.dial("sip:bob@example.net").catch((e) => {
    assert.equal(h.resources.closed, true);
    assert.equal(h.resources.mediaClosed, true);
    return e;
  });
  h.clock.flush();
  h.emit({ evt: "call", call: 1, dir: "out", state: "inviteSent" });
  const close = h.client.close({ timeoutMs: 5 });
  h.clock.flush();
  assert.ok(h.commands.some((c) => c.cmd === "hangup"));
  h.clock.advance(5);
  h.clock.flush();
  await close;
  assert.equal((await dial).name, "SipxCancelled");
});
test("fatal defects reject pending operations and suppress all queued and subsequent callbacks", async () => {
  const h = await harness(),
    events = [];
  h.client.on("closed", () => events.push("closed"));
  h.client.on("state", (s) => events.push(s));
  const pending = h.client.register().catch((e) => e);
  h.clock.flush();
  h.transport({ type: "closed", reason: "kernel-fault" });
  assert.equal((await pending).name, "SipxAbiDefect");
  h.emit({ evt: "registration", state: "registered" });
  assert.deepEqual(events, []);
  assert.equal(h.resources.closed, true);
  assert.equal(h.resources.mediaClosed, true);
  assert.equal(h.clock.pending, 0);
});

test("media setup failure stays a media error through the kernel local-cancellation outcome", async () => {
  const h = await harness();
  const pending = h.client.dial("sip:bob@example.net").catch((e) => e);
  h.clock.flush();
  const id = h.commands[0].id;
  h.emit({ evt: "call", call: 1, dir: "out", state: "dialing" });
  h.error({ call: 1, kind: "negotiation" });
  h.emit({ evt: "call-ended", call: 1, cause: { class: "local" } });
  h.emit({ evt: "outcome", id, ok: false, error: { code: "cancelled" } });
  assert.equal((await pending).name, "SipxMediaError");
  await finish(h);
});

for (const type of ["disconnected", "offline", "closed"]) {
  test(`ordinary ${type} terminalizes an established handle and delivers final notifications once`, async () => {
    const h = await harness();
    const pending = h.client.dial("sip:bob@example.net");
    h.clock.flush();
    h.emit({ evt: "call", call: 1, dir: "out", state: "sipEstablished" });
    h.emit({ evt: "outcome", id: h.commands[0].id, ok: true });
    h.media({ call: 1, state: "established" });
    const call = await pending;
    const order = [];
    call.on("state", (state) => order.push(state));
    call.on("ended", (event) => order.push(`ended:${event.cause}`));
    h.client.on("state", (state) => order.push(`client:${state}`));
    h.client.on("closed", () => order.push("closed"));
    h.transport({ type, reason: "remote-close" });
    assert.equal(call.state, "failed");
    assert.deepEqual(order, [
      "failed",
      "ended:transport",
      "client:closed",
      "closed",
    ]);
    assert.equal(h.resources.closed, true);
    assert.equal(h.resources.mediaClosed, true);
    assert.equal(h.clock.pending, 0);
    assert.equal(h.client.calls.length, 0);
    h.transport({ type, reason: "remote-close" });
    h.emit({ evt: "call-ended", call: 1, cause: { class: "transport" } });
    await h.client.close();
    assert.equal(order.length, 4);
  });
}
