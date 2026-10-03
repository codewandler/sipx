import test from "node:test";
import assert from "node:assert/strict";
import { WebSocketSignalling } from "../src/transport.mjs";
import { abiError } from "../src/errors.mjs";
import {
  FakeClock,
  FakeNetwork,
  FakeConnectivity,
  FakeEntropy,
  FakeKernel,
  WSS_TRANSPORT,
} from "./fakes.mjs";
for (const code of [-6, -7, -9, -10, -12])
  test(`ABI ${code} is ${code === -12 ? "fatal" : "a correlated command refusal"}`, () => {
    const clock = new FakeClock(),
      network = new FakeNetwork(),
      events = [];
    const kernel = new FakeKernel((entry) => {
      if (entry.kind === "command") throw abiError(code);
      return [];
    });
    const binding = new WebSocketSignalling({
      transport: WSS_TRANSPORT,
      kernel,
      clock,
      openSocket: network.factory,
      connectivity: new FakeConnectivity(),
      entropy: new FakeEntropy(),
      onEvent: (e) => events.push(e),
    });
    binding.start();
    clock.flush();
    network.latest.acceptOpen("sip");
    clock.flush();
    binding.submit('{"v":1,"id":41,"cmd":"register","expires":600}');
    clock.flush();
    if (code === -12) {
      assert.equal(binding.state, "closed");
      assert.ok(
        events.some((e) => e.type === "closed" && e.reason === "kernel-fault"),
      );
    } else {
      assert.equal(binding.state, "open");
      assert.deepEqual(
        events.find((e) => e.type === "command-error"),
        { type: "command-error", id: 41, code },
      );
    }
    binding.cancel();
    clock.flush();
    assert.equal(clock.pending, 0);
  });
