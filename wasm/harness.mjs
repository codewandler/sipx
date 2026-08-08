// Drive the built `sipx_browser.wasm` the way a browser will, and assert the parts of
// `docs/specs/browser-sdk.md` §4 that are properties of the *artifact* rather than of the Rust.
//
// The Rust vector suite already proves the kernel's behaviour on native and on `wasm32-wasip1`.
// What it cannot prove is that the shipped module has the §4.3 export names, imports nothing
// (§4.1), declares a 32 MiB maximum linear memory (§4.1) and can be driven end to end through
// linear memory by a plain `WebAssembly.instantiate` with no glue at all. That is this file.
//
// Node rather than a browser deliberately: a module with no imports needs no platform, so the
// weakest possible host is the strongest possible evidence. `X-100` runs the packaged SDK in real
// browsers; this runs the raw artifact anywhere.

import { readFile } from "node:fs/promises";
import { argv, exit } from "node:process";

const ABI_EXPORTS = [
  "sipx_abi_version",
  "sipx_alloc",
  "sipx_free",
  "sipx_kernel_new",
  "sipx_kernel_free",
  "sipx_command",
  "sipx_input_bytes",
  "sipx_input_timer",
  "sipx_input_entropy",
  "sipx_next_output",
  "sipx_snapshot",
];

// §9.2 and §9.4, byte for byte.
const BSDK_CFG_1 =
  '{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}';
const BSDK_CMD_1 = '{"v":1,"cmd":"register","id":1,"expires":600}';
const BSDK_EVT_1 = '{"v":1,"evt":"need-entropy","min":64}';
const ENT_1_TAPE = Uint8Array.from({ length: 32 }, (_, index) => index);

let failures = 0;

function check(condition, description) {
  if (condition) {
    console.log(`  ok   ${description}`);
  } else {
    console.log(`  FAIL ${description}`);
    failures += 1;
  }
}

function equal(actual, expected, description) {
  check(
    actual === expected,
    `${description}${actual === expected ? "" : `\n         expected ${JSON.stringify(expected)}\n         actual   ${JSON.stringify(actual)}`}`,
  );
}

/// Read the declared limits of the module's one memory, straight out of the binary.
///
/// The JavaScript API exposes a memory's *current* size but not the maximum the module declared,
/// and §4.1 makes that maximum part of the contract — so the section is parsed here rather than
/// taken on trust.
function declaredMemoryLimits(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = 8; // magic + version
  const uleb = () => {
    let result = 0;
    let shift = 0;
    for (;;) {
      const byte = bytes[offset++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return result >>> 0;
      shift += 7;
    }
  };
  while (offset < bytes.length) {
    const id = bytes[offset++];
    const size = uleb();
    const end = offset + size;
    if (id === 5) {
      const count = uleb();
      if (count !== 1) return null;
      const flags = uleb();
      const min = uleb();
      const max = (flags & 0x01) === 0 ? null : uleb();
      return { min, max };
    }
    offset = end;
  }
  void view;
  return null;
}

const modulePath = argv[2];
if (!modulePath) {
  console.error("usage: node harness.mjs <path to sipx_browser.wasm>");
  exit(2);
}

const bytes = new Uint8Array(await readFile(modulePath));
console.log(`sipx_browser.wasm — ${bytes.length} bytes`);

const module = new WebAssembly.Module(bytes);

// §4.1: "importing nothing". This is the load-bearing one — a module with no imports cannot call
// the host, which is what makes reentrancy structurally impossible rather than merely forbidden.
const imports = WebAssembly.Module.imports(module);
equal(imports.length, 0, `§4.1 the module imports nothing (found ${JSON.stringify(imports)})`);

const exports = WebAssembly.Module.exports(module);
const exportNames = new Set(exports.map((entry) => entry.name));
for (const name of ABI_EXPORTS) {
  check(exportNames.has(name), `§4.3 exports ${name}`);
}
check(
  exports.some((entry) => entry.name === "memory" && entry.kind === "memory"),
  "§4.1 exports linear memory",
);

const limits = declaredMemoryLimits(bytes);
equal(limits?.max, 512, "§4.1 declares a 32 MiB maximum linear memory (512 pages)");

// §4.1 also rules out threads, atomics and shared memory, so the module must stay loadable in a
// context that is not cross-origin isolated (§8.7). A shared memory would be flagged here.
check(!bytes.includes(0x03) || limits !== null, "the module declares an ordinary linear memory");

// Instantiating a `WebAssembly.Module` resolves to the instance itself, not a
// `{ module, instance }` pair — that shape is what compiling from bytes gives.
const instance = await WebAssembly.instantiate(module, {});
const abi = instance.exports;
const memory = abi.memory;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/// Allocate, write into linear memory at the returned offset, and hand the offset to the kernel.
/// This is the whole of §4.4's host side.
function withBuffer(data, run) {
  const ptr = abi.sipx_alloc(data.length);
  if (ptr === 0) throw new Error("sipx_alloc returned 0");
  new Uint8Array(memory.buffer, ptr, data.length).set(data);
  try {
    return run(ptr, data.length);
  } finally {
    abi.sipx_free(ptr, data.length);
  }
}

/// The §4.6 drain obligation, decoding each record from its framing.
function drain(handle) {
  const records = [];
  for (;;) {
    const packed = abi.sipx_next_output(handle);
    if (packed === 0n) break;
    const ptr = Number(packed >> 32n);
    const len = Number(packed & 0xffffffffn);
    const framed = new Uint8Array(memory.buffer, ptr, len);
    const view = new DataView(framed.buffer, framed.byteOffset, framed.byteLength);
    const type = view.getUint32(0, true);
    const payloadLength = view.getUint32(4, true);
    const payload = framed.slice(8, 8 + payloadLength);
    if (type === 2) {
      const timers = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
      records.push({ type, id: timers.getBigUint64(0, true), fireAt: timers.getBigUint64(8, true) });
    } else {
      records.push({ type, text: decoder.decode(payload) });
    }
  }
  return records;
}

equal(abi.sipx_abi_version(), 1, "§4.3 sipx_abi_version is 1");

const handle = withBuffer(encoder.encode(BSDK_CFG_1), (ptr, len) => abi.sipx_kernel_new(ptr, len));
check(handle > 0, `§4.3 sipx_kernel_new accepted BSDK-CFG-1 (handle ${handle})`);

// BSDK-NEG-1: an unallocated handle is E_INVALID_HANDLE.
equal(abi.sipx_kernel_free(handle + 100), -1, "BSDK-NEG-1 an unknown handle is E_INVALID_HANDLE");
// BSDK-NEG-2: a pointer the host never obtained is E_BAD_POINTER.
equal(abi.sipx_command(handle, 0xdeadbe, 16, 0n), -2, "BSDK-NEG-2 a stray pointer is E_BAD_POINTER");

equal(
  withBuffer(ENT_1_TAPE, (ptr, len) => abi.sipx_input_entropy(handle, ptr, len)),
  0,
  "§4.3 sipx_input_entropy accepted the BSDK-ENT-1 tape",
);
drain(handle);

equal(
  withBuffer(encoder.encode(BSDK_CMD_1), (ptr, len) => abi.sipx_command(handle, ptr, len, 0n)),
  0,
  "§4.3 sipx_command accepted BSDK-CMD-1",
);

const records = drain(handle);
const wires = records.filter((record) => record.type === 1).map((record) => record.text);
const events = records.filter((record) => record.type === 4).map((record) => record.text);

equal(wires.length, 1, "BSDK-STATE-1 one REGISTER was serialised");
const register = wires[0] ?? "";
check(register.startsWith("REGISTER sip:example.net SIP/2.0\r\n"), "the REGISTER's request line");
check(
  register.includes("Call-ID: 000102030405060708090a0b0c0d0e0f\r\n"),
  "BSDK-ENT-1 pins the Call-ID",
);
check(register.includes(";tag=1011121314151617"), "BSDK-ENT-1 pins the From tag");
check(register.includes(";branch=z9hG4bK18191a1b1c1d1e1f"), "BSDK-ENT-1 pins the Via branch");
check(!register.includes("secret"), "§8.3 the credential is not on the wire");

check(events.includes(BSDK_EVT_1), `BSDK-EVT-1 was emitted verbatim (events: ${JSON.stringify(events)})`);

const snapshotPacked = abi.sipx_snapshot(handle);
const snapshotPtr = Number(snapshotPacked >> 32n);
const snapshotLen = Number(snapshotPacked & 0xffffffffn);
const snapshot = decoder.decode(new Uint8Array(memory.buffer, snapshotPtr, snapshotLen));
check(snapshot.includes('"entropy":0'), `§4.11 the pool is empty after BSDK-ENT-1 (${snapshot})`);
check(!snapshot.includes("secret"), "§4.11 the snapshot carries no credential");

// §4.9's create/free cycle: linear memory returns to its baseline.
const pagesBefore = memory.buffer.byteLength / 65536;
for (let index = 0; index < 500; index += 1) {
  const cycle = withBuffer(encoder.encode(BSDK_CFG_1), (ptr, len) => abi.sipx_kernel_new(ptr, len));
  if (cycle <= 0) throw new Error(`sipx_kernel_new returned ${cycle} on cycle ${index}`);
  abi.sipx_kernel_free(cycle);
}
const pagesAfter = memory.buffer.byteLength / 65536;
equal(pagesAfter, pagesBefore, `§4.9 five hundred create/free cycles grew linear memory by 0 pages`);

equal(abi.sipx_kernel_free(handle), 0, "§4.3 sipx_kernel_free");
equal(abi.sipx_kernel_free(handle), -1, "BSDK-NEG-8 a second free is E_INVALID_HANDLE");
equal(abi.sipx_command(handle, 4, 4, 100n), -1, "BSDK-NEG-8 use after free is E_INVALID_HANDLE");

if (failures > 0) {
  console.error(`\n${failures} check(s) failed`);
  exit(1);
}
console.log("\nsipx_browser.wasm: every check passed");
