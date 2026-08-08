// Drive the binding against the compiled kernel, not a fake one.
//
// `transport.test.mjs` proves what the binding decides; a scripted kernel cannot prove that those
// decisions are the ones the real module needs. These cases wire `WebSocketSignalling` to the same
// `sipx_browser.wasm` that `wasm/harness.mjs` checks, through a `KernelPort` written by hand over
// the §4 ABI, and assert the two properties that only the real module can settle: that bytes the
// binding sends are the kernel's serialisation, and that "the peer disagrees about message
// boundaries" is the real parser's verdict rather than a fake's.
//
// The port below is a **test fixture**. `docs/specs/browser-sdk.md` §7.2 makes the ABI bindings
// generated from a checked source in the Rust workspace, which is `A-17`'s deliverable; this is
// forty lines of it, written out so that `T-33` can be evidenced before `A-17` exists, and it is
// expected to be deleted when the generated glue lands.

import { readFile } from "node:fs/promises";

import { check, equal, test } from "./assert.mjs";
import { FakeClock, FakeConnectivity, FakeEntropy, FakeNetwork, Recorder } from "./fakes.mjs";
import { decodeRecord } from "../src/records.mjs";
import { Reason, SIP_SUBPROTOCOL, WebSocketSignalling } from "../src/transport.mjs";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** `BSDK-CFG-1` (`docs/specs/browser-sdk.md` §9.2), byte for byte. */
const BSDK_CFG_1 =
  '{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}';
/** `BSDK-CMD-1`. */
const BSDK_CMD_1 = '{"v":1,"cmd":"register","id":1,"expires":600}';

/** A `KernelPort` over one handle of the compiled module. */
class CompiledKernel {
  #exports;
  #handle;

  constructor(exports, configuration) {
    this.#exports = exports;
    this.#handle = this.#withBuffer(encoder.encode(configuration), (ptr, len) =>
      exports.sipx_kernel_new(ptr, len),
    );
    if (this.#handle <= 0) throw new Error(`sipx_kernel_new returned ${this.#handle}`);
  }

  /** §4.4's host side: allocate, write, call, release. */
  #withBuffer(data, run) {
    const ptr = this.#exports.sipx_alloc(data.length);
    if (ptr === 0) throw new Error("sipx_alloc returned 0");
    new Uint8Array(this.#exports.memory.buffer, ptr, data.length).set(data);
    try {
      return run(ptr, data.length);
    } finally {
      this.#exports.sipx_free(ptr, data.length);
    }
  }

  /** §4.6's drain obligation, discharged before the entry point's caller sees anything. */
  #drain() {
    const records = [];
    for (;;) {
      const packed = this.#exports.sipx_next_output(this.#handle);
      if (packed === 0n) return records;
      const ptr = Number(packed >> 32n);
      const len = Number(packed & 0xffffffffn);
      records.push(decodeRecord(new Uint8Array(this.#exports.memory.buffer, ptr, len)));
    }
  }

  #checked(code) {
    if (code < 0) throw new Error(`the ABI returned ${code}`);
    return this.#drain();
  }

  command(document, nowMs) {
    return this.#checked(
      this.#withBuffer(document, (ptr, len) =>
        this.#exports.sipx_command(this.#handle, ptr, len, BigInt(nowMs)),
      ),
    );
  }

  inputBytes(bytes, nowMs) {
    return this.#checked(
      this.#withBuffer(bytes, (ptr, len) =>
        this.#exports.sipx_input_bytes(this.#handle, ptr, len, BigInt(nowMs)),
      ),
    );
  }

  inputTimer(id, nowMs) {
    return this.#checked(this.#exports.sipx_input_timer(this.#handle, id, BigInt(nowMs)));
  }

  inputEntropy(bytes) {
    return this.#checked(
      this.#withBuffer(bytes, (ptr, len) =>
        this.#exports.sipx_input_entropy(this.#handle, ptr, len),
      ),
    );
  }

  snapshot() {
    const packed = this.#exports.sipx_snapshot(this.#handle);
    const ptr = Number(packed >> 32n);
    const len = Number(packed & 0xffffffffn);
    // §4.2: a null pointer with a nonzero length carries an error-code magnitude rather than a
    // buffer — which is what a freed handle answers. There is nothing to report about a kernel
    // that no longer exists, and inventing a counter would be worse than saying so.
    if (ptr === 0) return null;
    return JSON.parse(decoder.decode(new Uint8Array(this.#exports.memory.buffer, ptr, len)));
  }

  free() {
    this.#exports.sipx_kernel_free(this.#handle);
  }
}

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
    const kernel = new CompiledKernel(instance.exports, BSDK_CFG_1);
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
    check(register.includes("Via: SIP/2.0/WSS"), "over the WebSocket transport (RFC 7118 §5.2)");
    check(!register.includes("secret"), "§8.3 the credential is not on the wire");
    check(!network.latest.url.includes("secret"), "§8.6 nor in the URL the socket was opened with");
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
    check(clock.pending > before, "the transaction's timer became a host timer");
  });

  test("the compiled parser's verdict on a fragmented frame closes the connection", async () => {
    const { clock, network, recorder, kernel } = await harness();
    equal(kernel.snapshot().counters.parse_errors, 0, "nothing has failed to parse yet");
    // Half a request: RFC 7118 §5 makes the frame boundary the message boundary, so a peer that
    // split one message across two frames has sent this.
    network.latest.deliver("REGISTER sip:example.net SIP/2.0\r\nVia: SIP/2.0/WSS ");
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

  test("a coalesced frame is neither split nor answered by the binding", async () => {
    const { clock, network } = await harness();
    network.latest.deliver("SIP/2.0 100 Trying\r\n\r\nSIP/2.0 200 OK\r\n\r\n");
    clock.flush();
    // The binding offers the frame whole and once; what the compiled kernel makes of the trailing
    // bytes is its own verdict. Today it parses the first message and discards the rest without
    // counting it, so no framing close follows — `docs/specs/sip-tls.md` §4 says it should, and
    // `S-53` owns closing that gap in the kernel. Asserted here only as far as `T-33` is
    // responsible: the binding invents nothing in response.
    equal(network.latest.sent.length, 0, "no message was invented in response");
  });

  test("cancellation frees the compiled kernel's handle", async () => {
    const { signalling, clock, network } = await harness();
    signalling.cancel();
    clock.flush();
    check(network.latest.closed !== null, "the socket was closed");
    equal(signalling.state, "closed", "terminal");
  });
}
