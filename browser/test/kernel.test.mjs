// Drive the signalling binding through the production generated ABI loader.
import { readFile } from "node:fs/promises";

import { check, equal, test } from "./assert.mjs";
import {
  FakeClock,
  FakeConnectivity,
  FakeEntropy,
  FakeNetwork,
  Recorder,
} from "./fakes.mjs";
import { Kernel } from "../src/kernel.mjs";
import { createClientForTest } from "../src/client.mjs";
import {
  Reason,
  SIP_SUBPROTOCOL,
  WebSocketSignalling,
} from "../src/transport.mjs";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** `BSDK-CFG-1` (`docs/specs/browser-sdk.md` §9.2), byte for byte. */
const BSDK_CFG_1 =
  '{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}';
/** `BSDK-CMD-1`. */
const BSDK_CMD_1 = '{"v":1,"cmd":"register","id":1,"expires":600}';

/**
 * Register the compiled-kernel cases against the module at `modulePath`.
 *
 * Separate from module scope so that `run.mjs` can omit them when no module was built, rather
 * than failing to import.
 */
export async function register(modulePath) {
  const bytes = new Uint8Array(await readFile(modulePath));
  const compiled = new WebAssembly.Module(bytes);

  /** A fresh instance per case: §4.9's create/free cycle is `S-41`'s claim, not this one's. */
  async function harness() {
    const instance = await WebAssembly.instantiate(compiled, {});
    const clock = new FakeClock();
    const network = new FakeNetwork();
    const recorder = new Recorder();
    const kernel = new Kernel(instance.exports, BSDK_CFG_1);
    const signalling = new WebSocketSignalling({
      transport: { scheme: "wss", host: "edge.example.net", resource: "/sip" },
      insecure: "refuse",
      kernel,
      openSocket: network.factory,
      clock,
      entropy: new FakeEntropy(),
      connectivity: new FakeConnectivity(),
      onEvent: recorder.sink,
    });
    signalling.start();
    clock.flush();
    network.latest.acceptOpen(SIP_SUBPROTOCOL);
    clock.flush();
    return { signalling, clock, network, kernel, recorder };
  }

  test("the compiled kernel's REGISTER is what reaches the socket, and nothing else", async () => {
    const { signalling, clock, network, recorder } = await harness();
    signalling.submit(BSDK_CMD_1);
    clock.flush();

    equal(network.latest.sent.length, 1, "exactly one WebSocket message");
    const register = decoder.decode(network.latest.sent[0]);
    check(
      register.startsWith("REGISTER sip:example.net SIP/2.0\r\n"),
      `the request line the kernel serialised (got ${JSON.stringify(register.slice(0, 40))})`,
    );
    check(
      register.includes("Via: SIP/2.0/WSS"),
      "over the WebSocket transport (RFC 7118 §5.2)",
    );
    check(
      !register.includes("secret"),
      "§8.3 the credential is not on the wire",
    );
    check(
      !network.latest.url.includes("secret"),
      "§8.6 nor in the URL the socket was opened with",
    );
    check(
      !JSON.stringify(recorder.events).includes("secret"),
      "§8.3 nor in any event the binding emitted",
    );
  });

  test("a REGISTER makes the compiled kernel ask for a host timer", async () => {
    const { signalling, clock } = await harness();
    const before = clock.pending;
    signalling.submit(BSDK_CMD_1);
    clock.flush();
    check(
      clock.pending > before,
      "the transaction's timer became a host timer",
    );
  });

  test("the compiled parser's verdict on a fragmented frame closes the connection", async () => {
    const { clock, network, recorder, kernel } = await harness();
    equal(
      kernel.snapshot().counters.parse_errors,
      0,
      "nothing has failed to parse yet",
    );
    // Half a request: RFC 7118 §5 makes the frame boundary the message boundary, so a peer that
    // split one message across two frames has sent this.
    network.latest.deliver(
      "REGISTER sip:example.net SIP/2.0\r\nVia: SIP/2.0/WSS ",
    );
    clock.flush();
    // Reaching `framing` is itself the evidence: the binding gets there only by reading a raised
    // `parse_errors` out of the §4.11 snapshot, and the count came from the compiled parser. The
    // kernel cannot be re-read afterwards — teardown freed the handle, and §4.9 makes a freed
    // handle answer nothing.
    equal(
      recorder.ofType("closed")[0]?.reason,
      Reason.Framing,
      "the compiled parser refused the fragment and the binding closed (docs/specs/sip-tls.md §4)",
    );
    equal(kernel.snapshot(), null, "and the handle is gone");
  });

  /** Two complete responses, differing only where they must, for one frame to hold both. */
  function response(tag) {
    return (
      "SIP/2.0 200 OK\r\n" +
      `Via: SIP/2.0/WSS df7jal23ls0d.invalid;branch=z9hG4bK${tag}\r\n` +
      "To: <sip:alice@example.net>;tag=remote\r\n" +
      `From: <sip:alice@example.net>;tag=${tag}\r\n` +
      `Call-ID: ${tag}@example.net\r\n` +
      "CSeq: 1 REGISTER\r\n" +
      "Content-Length: 0\r\n\r\n"
    );
  }

  test("the compiled parser's verdict on a coalesced frame closes the connection", async () => {
    const { clock, network, recorder, kernel } = await harness();
    equal(
      kernel.snapshot().counters.parse_errors,
      0,
      "nothing has failed to parse yet",
    );
    // The other half of RFC 7118 §5, and the half that used to be silent: the kernel parsed the
    // first message, acted on it, and dropped the rest without counting it. The binding offers the
    // frame whole and once — it holds no boundary search to split one with — so reaching `framing`
    // is again the compiled parser's verdict rather than this test's.
    network.latest.deliver(response("first") + response("second"));
    clock.flush();
    equal(
      recorder.ofType("closed")[0]?.reason,
      Reason.Framing,
      "the compiled kernel refused the coalesced frame and the binding closed (docs/specs/sip-tls.md §4)",
    );
    equal(network.latest.sent.length, 0, "no message was invented in response");
    equal(kernel.snapshot(), null, "and the handle is gone");
  });

  test("a frame that omits Content-Length is one message, body and all", async () => {
    const { clock, network, recorder, kernel } = await harness();
    // This frame reads as two messages and is one. With no `Content-Length` on the `100 Trying`,
    // `docs/specs/sip-tls.md` §4 runs its body to the end of the frame (RFC 3261 §20.14), so those
    // octets are already spent — refusing it would reject traffic this transport frames perfectly
    // well. Pinned because the boundary between this and the case above is the whole of what
    // `docs/specs/browser-sdk.md` §4.3.1 has to get right.
    network.latest.deliver("SIP/2.0 100 Trying\r\n\r\nSIP/2.0 200 OK\r\n\r\n");
    clock.flush();
    equal(
      kernel.snapshot().counters.parse_errors,
      0,
      "the frame said where the message ended",
    );
    equal(recorder.ofType("closed").length, 0, "so the connection is still up");
    equal(
      network.latest.sent.length,
      0,
      "and nothing was invented in response",
    );
  });

  test("cancellation frees the compiled kernel's handle", async () => {
    const { signalling, clock, network } = await harness();
    signalling.cancel();
    clock.flush();
    check(network.latest.closed !== null, "the socket was closed");
    equal(signalling.state, "closed", "terminal");
  });
  async function clientHarness() {
    const instance = await WebAssembly.instantiate(compiled, {});
    const clock = new FakeClock(),
      network = new FakeNetwork();
    const kernel = new Kernel(instance.exports, BSDK_CFG_1);
    const ready = createClientForTest(JSON.parse(BSDK_CFG_1), {
      kernel,
      clock,
      openSocket: network.factory,
      entropy: new FakeEntropy(),
      connectivity: new FakeConnectivity(),
      makeMedia: () => ({ handle() {}, closeCall() {}, close() {} }),
    });
    clock.flush();
    network.latest.acceptOpen(SIP_SUBPROTOCOL);
    clock.flush();
    return { client: await ready, clock, network, kernel };
  }
  async function settleTasks(h) {
    for (let i = 0; i < 8; i++) {
      h.clock.flush();
      await Promise.resolve();
    }
  }
  function registrationResponse(request, status = 200) {
    const header = (name) =>
      request.match(new RegExp(`^${name}: (.*)$`, "mi"))[1].trim();
    return (
      `SIP/2.0 ${status} ${status === 200 ? "OK" : "Forbidden"}\r\n` +
      `Via: ${header("Via")}\r\nFrom: ${header("From")}\r\n` +
      `To: ${header("To")};tag=registrar\r\nCall-ID: ${header("Call-ID")}\r\n` +
      `CSeq: ${header("CSeq")}\r\nExpires: ${header("Expires")}\r\nContent-Length: 0\r\n\r\n`
    );
  }
  for (const finalStatus of [200, 403]) {
    test(`aborted pending registration waits for real kernel ${finalStatus} then cleans only its registration`, async () => {
      const h = await clientHarness();
      try {
        const unrelated = h.client.dial("sip:bob@example.net").catch((e) => e);
        await settleTasks(h);
        const call = h.client.calls[0];
        check(call, "an unrelated call has a real kernel identity");
        const abort = new AbortController();
        let outcome;
        const pending = h.client
          .register({ signal: abort.signal })
          .catch((e) => {
            outcome = e;
            return e;
          });
        await settleTasks(h);
        const socket = h.network.latest;
        const request = decoder.decode(socket.sent.at(-1));
        check(request.startsWith("REGISTER "), "real REGISTER is on the wire");
        abort.abort();
        abort.abort();
        await settleTasks(h);
        equal(
          h.client.state,
          "connected",
          "aborting pending REGISTER does not close the client",
        );
        equal(
          outcome,
          undefined,
          "cancellation waits for the original exchange",
        );
        equal(
          socket.sent.length,
          1,
          "no premature deregistration while kernel is busy",
        );
        socket.deliver(registrationResponse(request, finalStatus));
        await settleTasks(h);
        if (finalStatus === 200) {
          equal(
            socket.sent.length,
            2,
            "accepted registration is deregistered exactly once",
          );
          const inverse = decoder.decode(socket.sent.at(-1));
          check(
            inverse.includes("Expires: 0\r\n"),
            "inverse is actual Expires0 REGISTER",
          );
          equal(
            outcome,
            undefined,
            "promise still waits for deregistration response",
          );
          socket.deliver(registrationResponse(inverse));
          await settleTasks(h);
        }
        equal(
          (await pending).name,
          "SipxCancelled",
          "cancel settles typed only after kernel reports",
        );
        equal(h.client.calls[0], call, "unrelated call survives cancellation");
        equal(call.state, "dialing", "unrelated call is not terminalized");
        equal(h.client.state, "connected", "client remains usable");
        const closed = h.client.close({ timeoutMs: 1 });
        h.clock.flush();
        h.clock.advance(1);
        await closed;
        await unrelated;
        equal(h.clock.pending, 0, "teardown frees all owned timers");
      } finally {
        const closed = h.client.close({ timeoutMs: 1 });
        h.clock.flush();
        h.clock.advance(1);
        await closed;
      }
    });
  }
}
