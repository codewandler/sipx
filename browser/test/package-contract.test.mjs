import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const config = {
  v: 1,
  aor: "sip:alice@example.net",
  auth: { username: "alice", password: "secret" },
  transport: { scheme: "wss", host: "edge.example.net", resource: "/sip" },
  insecure: "refuse",
};

test("production loader drives the shipped ABI and releases its handle and teardown timers", async () => {
  const { Kernel } = await import("../src/kernel.mjs");
  const bytes = await readFile(
    new URL(
      "../../wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm",
      import.meta.url,
    ),
  );
  const kernel = await Kernel.instantiate(bytes, config);
  kernel.inputEntropy(new Uint8Array(960));
  const records = kernel.command(
    new TextEncoder().encode('{"v":1,"id":1,"cmd":"register","expires":600}'),
    1,
  );
  assert.ok(records.some((r) => r.kind === "wire"));
  assert.equal(kernel.snapshot().registration, "registering");
  const cancelled = kernel.free();
  assert.ok(cancelled.length > 0);
  assert.equal(kernel.snapshot(), null);
  assert.deepEqual(kernel.free(), []);
});

test("production loader rejects a foreign ABI before allocating a kernel", async () => {
  const { Kernel } = await import("../src/kernel.mjs");
  let allocated = false;
  assert.throws(
    () =>
      new Kernel(
        {
          sipx_abi_version: () => 99,
          sipx_kernel_new: () => {
            allocated = true;
          },
        },
        config,
      ),
    (error) => error.name === "SipxAbiDefect",
  );
  assert.equal(allocated, false);
});

test("a trapped ABI is never re-entered, including allocation cleanup and later free", async () => {
  const { Kernel } = await import("../src/kernel.mjs");
  const bytes = await readFile(
    new URL(
      "../../wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm",
      import.meta.url,
    ),
  );
  const { instance } = await WebAssembly.instantiate(bytes, {});
  let frees = 0,
    kernelFrees = 0;
  const kernel = new Kernel(
    {
      ...instance.exports,
      sipx_free: function (ptr, len) {
        frees++;
        instance.exports.sipx_free(ptr, len);
      },
      sipx_kernel_free: function (handle) {
        kernelFrees++;
        return instance.exports.sipx_kernel_free(handle);
      },
      sipx_command: function (_handle, _ptr, _len, _now) {
        throw new WebAssembly.RuntimeError("private trap detail");
      },
    },
    config,
  );
  const before = frees;
  assert.throws(
    () => kernel.command(new TextEncoder().encode("{}"), 1),
    (e) => e.name === "SipxAbiDefect" && !e.message.includes("private"),
  );
  assert.equal(frees, before);
  kernel.free();
  assert.equal(kernelFrees, 0);
  assert.equal(kernel.snapshot(), null);
});
test("the production loader retains a usable instance after a state refusal", async () => {
  const { Kernel } = await import("../src/kernel.mjs");
  const bytes = await readFile(
    new URL(
      "../../wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm",
      import.meta.url,
    ),
  );
  const kernel = await Kernel.instantiate(bytes, config);
  kernel.inputEntropy(new Uint8Array(960));
  assert.throws(
    () =>
      kernel.command(
        new TextEncoder().encode('{"v":1,"id":1,"cmd":"hangup","call":999}'),
        1,
      ),
    (e) => e.name === "SipxStateError",
  );
  assert.ok(kernel.snapshot());
  kernel.free();
});


// Independent lifecycle cases drive the shipped WASM through the actual signalling adapter.
import { createClientForTest } from "../src/client.mjs";
import { FakeClock, FakeNetwork, FakeEntropy, FakeConnectivity } from "./fakes.mjs";
async function adversaryProductionClient() {
  const { Kernel } = await import("../src/kernel.mjs");
  const bytes = await readFile(new URL("../../wasm/target/wasm32-unknown-unknown/release/sipx_browser_wasm.wasm", import.meta.url));
  const clock = new FakeClock(), network = new FakeNetwork();
  const kernel = await Kernel.instantiate(bytes, config);
  const owned = { mediaClosed: false };
  const pending = createClientForTest(config, {
    kernel, clock, openSocket: network.factory, entropy: new FakeEntropy(),
    connectivity: new FakeConnectivity(),
    makeMedia: () => ({ handle() {}, closeCall() {}, close() { owned.mediaClosed = true; } }),
  });
  clock.flush(); network.latest.acceptOpen("sip"); clock.flush();
  return { client: await pending, clock, network, kernel, owned };
}
async function adversaryDrain(h) {
  for (let i = 0; i < 8; i++) { h.clock.flush(); await Promise.resolve(); }
}
function adversaryRegisterResponse(request, status, challenge = false) {
  const header = name => request.match(new RegExp(`^${name}: (.*)$`, "mi"))[1].trim();
  return `SIP/2.0 ${status} ${status === 200 ? "OK" : "Unauthorized"}\r\n`
    + `Via: ${header("Via")}\r\nFrom: ${header("From")}\r\nTo: ${header("To")};tag=adversary\r\n`
    + `Call-ID: ${header("Call-ID")}\r\nCSeq: ${header("CSeq")}\r\nExpires: ${header("Expires")}\r\n`
    + (challenge ? 'WWW-Authenticate: Digest realm="example.net", nonce="review-nonce", algorithm=SHA-256, qop="auth"\r\n' : '')
    + 'Content-Length: 0\r\n\r\n';
}
test("adversary cancellation owns a challenged REGISTER through its inverse response", async () => {
  const h = await adversaryProductionClient();
  try {
    const abort = new AbortController(); let outcome;
    const pending = h.client.register({ signal: abort.signal }).catch(e => { outcome = e; return e; });
    await adversaryDrain(h);
    const socket = h.network.latest;
    const first = new TextDecoder().decode(socket.sent[0]);
    assert.match(first, /^REGISTER sip:example.net SIP\/2.0/);
    abort.abort(); await adversaryDrain(h);
    socket.deliver(adversaryRegisterResponse(first, 401, true));
    await adversaryDrain(h);
    assert.equal(outcome, undefined);
    assert.equal(h.client.state, "connected");
    assert.equal(socket.sent.length, 2);
    const authenticated = new TextDecoder().decode(socket.sent[1]);
    assert.match(authenticated, /Authorization: Digest /);
    assert.match(authenticated, /Expires: 600\r\n/);
    socket.deliver(adversaryRegisterResponse(authenticated, 200));
    await adversaryDrain(h);
    assert.equal(socket.sent.length, 3);
    const inverse = new TextDecoder().decode(socket.sent[2]);
    assert.match(inverse, /Expires: 0\r\n/);
    assert.equal(outcome, undefined);
    socket.deliver(adversaryRegisterResponse(inverse, 200));
    await adversaryDrain(h);
    assert.equal((await pending).name, "SipxCancelled");
    assert.equal(h.client.state, "connected");
    assert.equal(socket.sent.length, 3);
  } finally {
    const closed = h.client.close({ timeoutMs: 1 });
    h.clock.flush(); h.clock.advance(1); await closed;
    assert.equal(h.clock.pending, 0);
    assert.equal(h.network.latest.attachedHandlers, 0);
    assert.equal(h.kernel.snapshot(), null);
  }
});
test("adversary real socket loss terminalizes the public handle before rejecting pending dial", async () => {
  const h = await adversaryProductionClient();
  const order = []; let call;
  h.client.on("outgoing", c => {
    call = c;
    c.on("state", state => order.push(state));
    c.on("ended", e => order.push(`ended:${e.cause}`));
  });
  h.client.on("closed", () => order.push("closed"));
  const pending = h.client.dial("sip:bob@example.net").catch(e => {
    order.push("rejected");
    assert.equal(call.state, "failed");
    assert.equal(h.owned.mediaClosed, true);
    return e;
  });
  await adversaryDrain(h);
  assert.equal(call.state, "dialing");
  h.network.latest.remoteClose();
  await adversaryDrain(h);
  assert.equal((await pending).name, "SipxTransportError");
  assert.deepEqual(order, ["dialing", "failed", "ended:transport", "closed", "rejected"]);
  assert.equal(h.clock.pending, 0);
  assert.equal(h.kernel.snapshot(), null);
  assert.equal(h.network.latest.attachedHandlers, 0);
  assert.equal(h.network.attempts, 1);
  h.clock.fireEveryRetainedCallback(); await adversaryDrain(h);
  assert.equal(h.network.attempts, 1);
  assert.equal(order.length, 5);
  await h.client.close();
});
