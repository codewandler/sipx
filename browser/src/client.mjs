import { command, contract } from "../generated/abi.mjs";
import { loadKernel } from "./kernel.mjs";
import { WebSocketSignalling, signallingUrl } from "./transport.mjs";
import { browserPlatform } from "./platform.mjs";
import { browserMediaPlatform } from "./media-platform.mjs";
import { BrowserMediaAdapter } from "./media.mjs";
import {
  SipxAbiDefect,
  SipxStateError,
  SipxLimitError,
  SipxTransportError,
  SipxSipError,
  SipxCancelled,
  SipxCapabilityError,
  SipxMediaError,
  abiError,
} from "./errors.mjs";

const terminal = new Set(["ended", "failed"]);
const stateMap = {
  dialing: "dialing",
  inviteSent: "dialing",
  ringing: "ringing",
  incoming: "incoming",
  answerPending: "answering",
  answerSent: "answering",
  answerDelivered: "dialing",
};
function deferred() {
  let resolve, reject;
  const promise = new Promise((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
}
function outcomeError(event) {
  const code = event.error?.code;
  if (code === "call-limit") return new SipxLimitError("calls");
  if (code?.includes("media") || code?.includes("sdp"))
    return new SipxMediaError("negotiation");
  if (code === "cancelled" || code === "local") return new SipxCancelled();
  return new SipxSipError(event.error?.status);
}
function causeError(cause) {
  if (cause?.class === "media") return new SipxMediaError("negotiation");
  if (cause?.class === "local") return new SipxCancelled();
  return new SipxSipError(cause?.status);
}
function capabilities(scope) {
  const checks = [
    ["WebAssembly", () => typeof scope.WebAssembly === "object"],
    ["secure context", () => scope.isSecureContext === true],
    ["WebSocket", () => typeof scope.WebSocket === "function"],
    ["RTCPeerConnection", () => typeof scope.RTCPeerConnection === "function"],
    [
      "media capture with per-track stop",
      () =>
        typeof scope.navigator?.mediaDevices?.getUserMedia === "function" &&
        typeof scope.MediaStreamTrack?.prototype?.stop === "function",
    ],
    [
      "crypto.getRandomValues",
      () => typeof scope.crypto?.getRandomValues === "function",
    ],
    ["performance.now", () => typeof scope.performance?.now === "function"],
    ["queueMicrotask", () => typeof scope.queueMicrotask === "function"],
  ];
  for (const [name, present] of checks) {
    let ok = false;
    try {
      ok = present();
    } catch {}
    if (!ok) throw new SipxCapabilityError(name);
  }
}

/** A call's public handle; SIP identity and transitions belong to the Rust kernel. */
export class SipxCall {
  #client;
  #listeners = new Map();
  #state;
  #final = false;
  constructor(client, event) {
    this.#client = client;
    this.id = event.call;
    this.direction = event.dir;
    this.from = event.from;
    this.to = event.to;
    this.#state = stateMap[event.state] ?? "dialing";
  }
  get state() {
    return this.#state;
  }
  on(name, listener) {
    if (typeof listener !== "function")
      throw new TypeError("listener required");
    if (this.#final) return () => {};
    const set = this.#listeners.get(name) ?? new Set();
    set.add(listener);
    this.#listeners.set(name, set);
    return () => set.delete(listener);
  }
  _emit(name, value) {
    if (this.#final) return;
    this.#client._listeners(this.#listeners.get(name), value);
    if (name === "ended") {
      this.#final = true;
      this.#listeners.clear();
    }
  }
  _state(state) {
    if (this.#final || this.#state === state) return;
    this.#state = state;
    this._emit("state", state);
  }
  ring() {
    return this.#client._callCommand(this, "ring");
  }
  answer(options = {}) {
    return this.#client._callCommand(this, "answer", {}, options);
  }
  reject(status = 486) {
    return this.#client._callCommand(this, "reject", { status });
  }
  hangup() {
    return this.#client._callCommand(this, "hangup");
  }
  mute(muted = true) {
    this.#client._checkCall(this);
    return this.#client._media.mute(this.id, muted);
  }
  negotiatedMedia() {
    return this.#client._media.negotiatedMedia(this.id);
  }
  refreshStats() {
    this.#client._checkCall(this);
    return this.#client._media.refreshStats(this.id);
  }
}

export class SipxClient {
  #state = "starting";
  #clock;
  #kernel;
  #transport;
  _media;
  #listeners = new Map();
  #calls = new Map();
  #pending = new Map();
  #dialQueue = [];
  #next = 1;
  #queue = [];
  #queued = false;
  #stopped = false;
  #closing = false;
  #ready = deferred();
  #close;
  #closeTimer;
  #removePage;
  #registered = false;
  #registration = "unregistered";
  #diagnostic;
  #counts = { listener_errors: 0, dropped_after_close: 0, unknown_events: 0 };
  static async create(configuration, options = {}) {
    const scope = options.scope ?? globalThis;
    capabilities(scope);
    try {
      signallingUrl(configuration.transport, configuration.insecure);
    } catch (error) {
      throw new SipxTransportError(error.reason ?? "configuration");
    }
    const kernel = await loadKernel(
      { v: 1, insecure: "refuse", ...configuration },
      { scope, wasmURL: options.wasmURL },
    );
    try {
      return await createClientForTest(configuration, {
        kernel,
        ...browserPlatform(scope),
        mediaPlatform: browserMediaPlatform(scope),
        onDiagnostic: options.onDiagnostic,
        setupTimeoutMs: options.setupTimeoutMs,
        operationTimeoutMs: options.operationTimeoutMs,
        onPageHide: (fn) => {
          scope.addEventListener("pagehide", fn);
          return () => scope.removeEventListener("pagehide", fn);
        },
      });
    } catch (error) {
      kernel.free();
      if (error?.capability) throw new SipxCapabilityError(error.capability);
      throw error;
    }
  }
  constructor(configuration, deps) {
    this.#clock = deps.clock;
    this.#kernel = deps.kernel;
    this.#diagnostic = deps.onDiagnostic;
    this._media = (deps.makeMedia ?? ((opts) => new BrowserMediaAdapter(opts)))(
      {
        platform: deps.mediaPlatform,
        setupTimeoutMs: deps.setupTimeoutMs,
        operationTimeoutMs: deps.operationTimeoutMs,
        sendCommand: (doc) => this.#internal(doc),
        onState: (e) => this.#enqueue(() => this.#mediaState(e)),
        onError: (e) => this.#enqueue(() => this.#mediaError(e)),
        onDiagnostic: (e) => this.#enqueue(() => this.#diagnose(e)),
      },
    );
    this.#transport = (
      deps.makeTransport ?? ((opts) => new WebSocketSignalling(opts))
    )({
      ...deps,
      transport: configuration.transport,
      insecure: configuration.insecure,
      onEvent: (e) => this.#enqueue(() => this.#event(e)),
    });
    this.#removePage = deps.onPageHide?.(() =>
      this.#finish(new SipxCancelled(), false),
    );
  }
  _start() {
    try {
      this.#transport.start();
    } catch (error) {
      this.#finish(new SipxTransportError(error.reason ?? "start"), false);
    }
    return this.#ready.promise;
  }
  get state() {
    return this.#state;
  }
  get calls() {
    return Object.freeze([...this.#calls.values()].map((x) => x.call));
  }
  get diagnostics() {
    return Object.freeze({
      ...this.#counts,
      transport: this.#transport.counters,
    });
  }
  on(name, listener) {
    if (typeof listener !== "function")
      throw new TypeError("listener required");
    if (this.#stopped) return () => {};
    const set = this.#listeners.get(name) ?? new Set();
    set.add(listener);
    this.#listeners.set(name, set);
    return () => set.delete(listener);
  }
  _listeners(set, value) {
    for (const fn of [...(set ?? [])]) {
      if (this.#stopped) break;
      try {
        const result = fn(value);
        if (result && typeof result.then === "function")
          Promise.resolve(result).catch(() =>
            this.#enqueue(() => {
              this.#counts.listener_errors++;
              this.#diagnose({ kind: "listener-error" });
            }),
          );
      } catch {
        this.#counts.listener_errors++;
        this.#diagnose({ kind: "listener-error" });
      }
    }
  }
  #diagnose(event) {
    if (this.#stopped) return;
    try {
      this.#diagnostic?.(event);
    } catch {}
  }
  #emit(name, value) {
    this._listeners(this.#listeners.get(name), value);
  }
  #stateTo(state) {
    if (this.#state !== state) {
      this.#state = state;
      this.#emit("state", state);
    }
  }
  #enqueue(fn) {
    if (this.#stopped) {
      this.#counts.dropped_after_close++;
      return;
    }
    this.#queue.push(fn);
    if (this.#queued) return;
    this.#queued = true;
    this.#clock.defer(() => {
      this.#queued = false;
      while (this.#queue.length && !this.#stopped) {
        const next = this.#queue.shift();
        try {
          next();
        } catch {
          this.#finish(new SipxAbiDefect(), false);
        }
      }
    });
  }
  #check() {
    if (this.#closing || this.#stopped) throw new SipxStateError();
  }
  _checkCall(call) {
    this.#check();
    if (!this.#calls.has(call.id) || terminal.has(call.state))
      throw new SipxStateError();
  }
  register({ expires = 600, signal } = {}) {
    return this.#publicCommand("register", { expires }, { signal });
  }
  unregister() {
    return this.#publicCommand("unregister", {});
  }
  dial(target, { signal } = {}) {
    return this.#publicCommand("dial", { target }, { signal });
  }
  setMicrophone(deviceId) {
    this.#check();
    this._media.setMicrophone(deviceId);
  }
  _callCommand(call, verb, fields = {}, options = {}) {
    try {
      this._checkCall(call);
    } catch (error) {
      return Promise.reject(error);
    }
    return this.#publicCommand(verb, { call: call.id, ...fields }, options);
  }
  #publicCommand(verb, fields, options = {}) {
    try {
      this.#check();
      if (options.signal?.aborted) return Promise.reject(new SipxCancelled());
      if (
        verb === "register" &&
        (!Number.isInteger(fields.expires) ||
          fields.expires < 1 ||
          fields.expires > 0xffffffff)
      )
        throw new SipxStateError();
      if (
        verb === "dial" &&
        (typeof fields.target !== "string" ||
          fields.target.length > 8192 ||
          !/^sips?:[^\s\r\n]+$/.test(fields.target))
      )
        throw new SipxStateError();
      if (
        verb === "reject" &&
        (!Number.isInteger(fields.status) ||
          fields.status < 300 ||
          fields.status > 699)
      )
        throw new SipxStateError();
      return this.#command(verb, fields, options).promise;
    } catch (error) {
      return Promise.reject(error);
    }
  }
  #command(verb, fields, options = {}) {
    if (this.#pending.size >= 256) throw new SipxLimitError("commands");
    const item = {
      ...deferred(),
      id: this.#next++,
      verb,
      call: fields.call,
      done: false,
      cancelled: false,
    };
    this.#pending.set(item.id, item);
    if (verb === "dial") this.#dialQueue.push(item);
    if (options.signal) {
      const abort = () => this.#enqueue(() => this.#abort(item));
      options.signal.addEventListener("abort", abort, { once: true });
      item.removeAbort = () =>
        options.signal.removeEventListener("abort", abort);
    }
    try {
      this.#transport.submit(JSON.stringify(command(verb, item.id, fields)));
    } catch (error) {
      this.#settle(
        item,
        undefined,
        error instanceof SipxLimitError ? error : new SipxStateError(),
      );
    }
    if (verb === "hangup") {
      const record = this.#calls.get(fields.call);
      if (record) {
        record.ending = true;
        this._media.closeCall(fields.call);
        this.#enqueue(() => record.call._state("ending"));
      }
    }
    return item;
  }
  #internal({ cmd, ...fields }) {
    if (this.#stopped) return;
    const p = this.#command(cmd, fields);
    p.promise.catch((error) => {
      if (error instanceof SipxStateError) return;
      this.#enqueue(() => {
        const record = this.#calls.get(fields.call);
        if (record) {
          record.error ??= error;
          record.ending = true;
          record.call._state("ending");
          this._media.closeCall(fields.call);
          if (cmd !== "hangup")
            this.#internal({ cmd: "hangup", call: fields.call });
        }
      });
    });
  }
  #settle(item, value, error) {
    if (!this.#pending.delete(item.id)) return;
    item.removeAbort?.();
    this.#dialQueue = this.#dialQueue.filter((p) => p !== item);
    if (error) item.reject(error);
    else item.resolve(value);
  }
  #abort(item) {
    if (!this.#pending.has(item.id) || item.cancelled) return;
    item.cancelled = true;
    if (item.verb === "register") {
      // The actual kernel refuses unregister while REGISTER is in flight. Keep
      // cancellation ownership until that exchange reports, then run its inverse.
      this.#complete(item);
    } else if (item.call !== undefined) {
      const record = this.#calls.get(item.call);
      if (record) {
        record.ending = true;
        record.error = new SipxCancelled();
        this._media.closeCall(item.call);
        record.call._state("ending");
        this.#internal({ cmd: "hangup", call: item.call });
      }
    }
  }
  #complete(item) {
    if (!item.done) return;
    const record = this.#calls.get(item.call);
    if (item.cancelled) {
      if (item.verb === "register") {
        // A refused/failed registration created no registration to undo.
        if (item.error && !this.#registered)
          return this.#settle(item, undefined, new SipxCancelled());
        if (item.cleanupStarted) return;
        item.cleanupStarted = true;
        this.#command("unregister", {}).promise.then(
          () =>
            this.#enqueue(() =>
              this.#settle(item, undefined, new SipxCancelled()),
            ),
          // An inverse refusal is a truthful operation error, not a fatal client
          // fault: unrelated calls must retain their media and signalling.
          (error) => this.#enqueue(() => this.#settle(item, undefined, error)),
        );
        return;
      }
      if (record && !record.terminal) return;
      return this.#settle(item, undefined, new SipxCancelled());
    }
    if (item.error)
      return this.#settle(item, undefined, record?.error ?? item.error);
    if (item.verb === "dial" || item.verb === "answer") {
      if (!record?.established) return;
      this.#settle(item, record.call);
      return;
    }
    if (item.verb === "hangup" || item.verb === "reject") {
      if (record && !record.terminal) return;
    }
    this.#settle(item);
  }
  #event(event) {
    if (event.type === "open") {
      if (!this.#closing) {
        this.#stateTo("connected");
        this.#ready.resolve(this);
      }
      return;
    }
    if (event.type === "command-error") {
      const item = this.#pending.get(event.id);
      if (item) {
        item.done = true;
        item.error = abiError(event.code);
        this.#complete(item);
      }
      return;
    }
    if (
      event.type === "closed" ||
      event.type === "disconnected" ||
      event.type === "offline"
    ) {
      if (this.#closing) return this.#finish();
      return this.#finish(
        event.reason === "kernel-fault"
          ? (this.#kernel.lastError ?? new SipxAbiDefect())
          : new SipxTransportError(event.reason ?? event.type),
        event.reason !== "kernel-fault",
      );
    }
    if (event.type !== "kernel") return;
    const e = event.event;
    if (!Object.hasOwn(contract.events, e.evt)) {
      this.#counts.unknown_events++;
      return;
    }
    if (e.evt === "error" && e.fatal)
      return this.#finish(new SipxAbiDefect(), false);
    if (e.evt === "registration") {
      this.#registration = e.state;
      this.#registered = e.state === "registered";
      if (e.state === "failed")
        for (const item of this.#pending.values()) {
          if (item.verb === "register" || item.verb === "unregister")
            item.error ??= new SipxSipError(e.status);
        }
      if (!this.#closing)
        this.#stateTo(this.#registered ? "registered" : "connected");
      this.#maybeClose();
      return;
    }
    if (e.evt === "call") {
      let record = this.#calls.get(e.call);
      if (!record) {
        const call = new SipxCall(this, e);
        record = { call, terminal: false, established: false, ending: false };
        this.#calls.set(e.call, record);
        if (e.dir === "out") {
          const item = this.#dialQueue.shift();
          if (!item) return this.#finish(new SipxAbiDefect(), false);
          item.call = e.call;
          this.#emit("outgoing", call);
          if (item.cancelled) {
            record.ending = true;
            record.error = new SipxCancelled();
            this.#internal({ cmd: "hangup", call: e.call });
          }
        } else this.#emit("incoming", call);
        call._emit("state", call.state);
      }
      if (!record.ending && stateMap[e.state])
        record.call._state(stateMap[e.state]);
    }
    const record = this.#calls.get(e.call);
    if (e.evt === "call-ended") {
      if (record) {
        this._media.closeCall(e.call);
        record.terminal = true;
        record.error ??= causeError(e.cause);
        record.call._state(
          ["local", "remote"].includes(e.cause.class) && !record.error?.kind
            ? "ended"
            : "failed",
        );
        record.call._emit("ended", {
          cause: e.cause.class,
          ...(e.cause.status ? { status: e.cause.status } : {}),
        });
        for (const item of this.#pending.values())
          if (item.call === e.call) {
            if (item.verb === "dial" || item.verb === "answer")
              item.error ??= record.error;
            this.#complete(item);
          }
        this.#calls.delete(e.call);
      }
      this.#maybeClose();
      return;
    }
    if (e.evt === "outcome") {
      const item = this.#pending.get(e.id);
      if (item) {
        item.done = true;
        if (!e.ok) item.error ??= outcomeError(e);
        this.#complete(item);
      }
      this.#maybeClose();
      return;
    }
    if (e.call !== undefined && record && !record.ending) {
      Promise.resolve(this._media.handle(e)).catch(() =>
        this.#enqueue(() =>
          this.#mediaError({
            call: e.call,
            error: new SipxMediaError("negotiation"),
          }),
        ),
      );
    }
  }
  #mediaState(e) {
    const record = this.#calls.get(e.call);
    if (!record || record.ending || record.terminal) return;
    if (e.state === "established") {
      record.established = true;
      record.call._state("established");
      for (const item of this.#pending.values())
        if (item.call === e.call) this.#complete(item);
    }
  }
  #mediaError(e) {
    const record = this.#calls.get(e.call);
    if (!record || record.terminal) return;
    record.error = e.error ?? new SipxMediaError(e.kind ?? "negotiation");
    record.ending = true;
    record.call._state("ending");
    this._media.closeCall(e.call);
  }
  close({ timeoutMs = 3000 } = {}) {
    if (this.#close) return this.#close.promise;
    if (this.#stopped) return Promise.resolve();
    if (!Number.isFinite(timeoutMs) || timeoutMs < 0 || timeoutMs > 30000)
      return Promise.reject(new SipxStateError());
    this.#close = deferred();
    this.#closing = true;
    this.#enqueue(() => {
      this.#stateTo("closing");
      for (const id of this.#calls.keys())
        this.#internal({ cmd: "hangup", call: id });
      if (this.#registration !== "unregistered")
        this.#internal({ cmd: "unregister" });
      this.#closeTimer = this.#clock.setTimer(
        () => this.#enqueue(() => this.#finish()),
        timeoutMs,
      );
      this.#maybeClose();
    });
    return this.#close.promise;
  }
  #maybeClose() {
    if (
      this.#closing &&
      this.#calls.size === 0 &&
      (this.#registration === "unregistered" ||
        this.#registration === "failed") &&
      this.#pending.size === 0
    )
      this.#finish();
  }
  #finish(error = new SipxCancelled(), notify = true) {
    if (this.#stopped) return;
    this.#closing = true;
    if (this.#closeTimer !== undefined)
      this.#clock.clearTimer(this.#closeTimer);
    this.#transport.cancel();
    this._media.close();
    this.#removePage?.();
    this.#counts.dropped_after_close += this.#queue.length;
    this.#queue.length = 0;
    for (const item of [...this.#pending.values()])
      this.#settle(item, undefined, error);
    this.#ready.reject(error);
    if (notify)
      for (const record of this.#calls.values()) {
        const transportFailure = error instanceof SipxTransportError;
        record.call._state(transportFailure ? "failed" : "ended");
        record.call._emit("ended", {
          cause: transportFailure ? "transport" : "local",
        });
      }
    this.#calls.clear();
    if (notify) {
      this.#stateTo("closed");
      this.#emit("closed", undefined);
    } else this.#state = "closed";
    this.#stopped = true;
    this.#listeners.clear();
    this.#close?.resolve();
  }
}
/** Internal construction seam: not a package export. No alternative protocol model. */
export function createClientForTest(configuration, deps) {
  const client = new SipxClient(configuration, deps);
  return client._start();
}
