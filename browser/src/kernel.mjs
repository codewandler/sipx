import { bindABI } from "../generated/abi.mjs";
import { decodeRecord } from "./records.mjs";
import { SipxAbiDefect, abiError } from "./errors.mjs";
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", { fatal: true });

/** Production ownership wrapper over the generated ABI. Each entry drains before returning. */
export class Kernel {
  #exports;
  #handle = 0;
  #dead = false;
  lastError = null;
  static async instantiate(bytes, configuration) {
    try {
      const module =
        bytes instanceof WebAssembly.Module
          ? bytes
          : await WebAssembly.compile(bytes);
      if (WebAssembly.Module.imports(module).length) throw new SipxAbiDefect();
      const instance = await WebAssembly.instantiate(module, {});
      return new Kernel(instance.exports, configuration);
    } catch (error) {
      throw error instanceof SipxAbiDefect ? error : new SipxAbiDefect();
    }
  }
  constructor(exports, configuration) {
    try {
      this.#exports = bindABI(exports);
    } catch {
      throw new SipxAbiDefect();
    }
    this.#handle = this.#buffer(
      encoder.encode(
        typeof configuration === "string"
          ? configuration
          : JSON.stringify(configuration),
      ),
      (ptr, len) => this.#exports.sipx_kernel_new(ptr, len),
    );
    if (this.#handle <= 0) throw abiError(this.#handle);
  }
  #buffer(bytes, run) {
    const ptr = this.#exports.sipx_alloc(bytes.length);
    if (!ptr) throw abiError(-9);
    try {
      new Uint8Array(this.#exports.memory.buffer, ptr, bytes.length).set(bytes);
      return run(ptr, bytes.length);
    } catch (error) {
      if (error instanceof WebAssembly.RuntimeError) this.#dead = true;
      throw error;
    } finally {
      if (!this.#dead) this.#exports.sipx_free(ptr, bytes.length);
    }
  }
  #copy(packed) {
    const ptr = Number(packed >> 32n),
      len = Number(packed & 0xffffffffn);
    if (!ptr) {
      if (len) throw abiError(-len);
      return null;
    }
    if (len > 65536 + 8) throw new SipxAbiDefect();
    return new Uint8Array(this.#exports.memory.buffer, ptr, len).slice();
  }
  #entry(run) {
    if (this.#dead) throw new SipxAbiDefect();
    try {
      const code = run();
      if (code < 0) throw abiError(code);
      const records = [];
      let total = 0;
      for (;;) {
        const bytes = this.#copy(this.#exports.sipx_next_output(this.#handle));
        if (bytes === null) return records;
        total += bytes.length;
        if (records.length >= 256 || total > 256 * 1024 + 256 * 8)
          throw new SipxAbiDefect();
        records.push(decodeRecord(bytes));
      }
    } catch (cause) {
      const error = cause?.name?.startsWith("Sipx")
        ? cause
        : new SipxAbiDefect();
      this.lastError = error;
      if (error instanceof SipxAbiDefect) this.#dead = true;
      throw error;
    }
  }
  command(document, nowMs) {
    return this.#entry(() =>
      this.#buffer(document, (ptr, len) =>
        this.#exports.sipx_command(this.#handle, ptr, len, BigInt(nowMs)),
      ),
    );
  }
  inputBytes(bytes, nowMs) {
    return this.#entry(() =>
      this.#buffer(bytes, (ptr, len) =>
        this.#exports.sipx_input_bytes(this.#handle, ptr, len, BigInt(nowMs)),
      ),
    );
  }
  inputTimer(id, nowMs) {
    return this.#entry(() =>
      this.#exports.sipx_input_timer(this.#handle, id, BigInt(nowMs)),
    );
  }
  inputEntropy(bytes) {
    return this.#entry(() =>
      this.#buffer(bytes, (ptr, len) =>
        this.#exports.sipx_input_entropy(this.#handle, ptr, len),
      ),
    );
  }
  snapshot() {
    if (!this.#handle || this.#dead) return null;
    try {
      const data = this.#copy(this.#exports.sipx_snapshot(this.#handle));
      return data ? JSON.parse(decoder.decode(data)) : null;
    } catch {
      this.#dead = true;
      throw new SipxAbiDefect();
    }
  }
  free() {
    if (!this.#handle) return [];
    const handle = this.#handle;
    this.#handle = 0;
    // A trapped instance is never re-entered. Dropping this instance releases its linear memory.
    if (this.#dead) {
      this.#exports = null;
      return [];
    }
    const cancelled = [];
    try {
      this.#exports.sipx_kernel_free(handle);
      const count = this.#exports.sipx_teardown_timer_count();
      if (count > 128) throw new SipxAbiDefect();
      for (let i = 0; i < count; i++)
        cancelled.push(this.#exports.sipx_teardown_timer_id(i));
      return cancelled;
    } finally {
      this.#dead = true;
      this.#exports = null;
    }
  }
}

/** Fetch only a same-origin packaged asset; redirects cannot change that boundary. */
export async function loadKernel(
  configuration,
  {
    scope = globalThis,
    wasmURL = new URL("../sipx_browser.wasm", import.meta.url),
  } = {},
) {
  const url = new URL(wasmURL, scope.location.href);
  if (url.origin !== scope.location.origin) throw new SipxAbiDefect();
  try {
    const response = await scope.fetch(url, {
      credentials: "same-origin",
      redirect: "error",
    });
    if (
      !response.ok ||
      (response.url && new URL(response.url).origin !== scope.location.origin)
    )
      throw new SipxAbiDefect();
    return await Kernel.instantiate(
      await response.arrayBuffer(),
      configuration,
    );
  } catch {
    throw new SipxAbiDefect();
  }
}
