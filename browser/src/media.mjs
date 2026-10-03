// Browser media ownership over the browser-sdk.md command/event boundary.
// SIP and SDP parsing remain inside the kernel. This adapter never reads browser globals.

export class SipxMediaError extends Error {
  constructor(kind) {
    super(`browser media failed: ${kind}`);
    this.name = "SipxMediaError";
    this.kind = kind;
  }
}
const CANCELLED = Symbol("cancelled");
const codecNames = new Set(["opus", "pcmu", "pcma", "cn", "telephone-event"]);
const clone = (value) => JSON.parse(JSON.stringify(value));
function codecValid(codec) {
  return (
    codec &&
    codecNames.has(codec.name) &&
    Number.isInteger(codec.clock_rate) &&
    codec.clock_rate > 0 &&
    Number.isInteger(codec.channels) &&
    codec.channels > 0 &&
    Number.isInteger(codec.payload_type) &&
    codec.payload_type >= 0 &&
    codec.payload_type <= 127
  );
}
function validFacts(facts) {
  return (
    facts?.rtcp_mux === true &&
    facts.audio_sections === 1 &&
    facts.fingerprint_algorithm === "sha-256" &&
    ["active", "passive"].includes(facts.answer_setup) &&
    ["active", "passive"].includes(facts.local_dtls_role) &&
    ["call_id", "local_tag", "remote_tag"].every(
      (key) =>
        typeof facts.dialog?.[key] === "string" && facts.dialog[key].length > 0,
    ) &&
    Array.isArray(facts.codecs) &&
    facts.codecs.length > 0 &&
    facts.codecs.every(codecValid) &&
    codecValid(facts.selected_codec) &&
    ["opus", "pcmu", "pcma"].includes(facts.selected_codec.name) &&
    facts.codecs.some((codec) =>
      ["name", "clock_rate", "channels", "payload_type"].every(
        (key) => codec[key] === facts.selected_codec[key],
      ),
    )
  );
}
function captureKind(error) {
  if (error?.name === "NotAllowedError" || error?.name === "SecurityError")
    return "permission";
  if (
    [
      "NotFoundError",
      "NotReadableError",
      "OverconstrainedError",
      "AbortError",
    ].includes(error?.name)
  )
    return "device";
  return "negotiation";
}
function stopStream(stream) {
  for (const track of stream?.getTracks?.() ?? []) {
    try {
      track.stop();
    } catch {
      /* Continue releasing independent tracks. */
    }
  }
}

/**
 * Media-only owner. `handle(event)` takes trusted kernel events; do not await it before delivering
 * cancellation/terminal events. `sendCommand({cmd,call,...})` receives no id/v: the lifecycle layer
 * allocates those and observes command outcomes. The sender MUST synchronously enqueue or throw;
 * its return value is ignored and an async sender is unsupported. Kernel outcome promises belong
 * to the lifecycle, never this callback. Hooks are internal, not public SDK callbacks.
 * `closeCall` and `close` revoke ownership synchronously even while a browser promise is pending.
 */
export class BrowserMediaAdapter {
  #platform;
  #send;
  #hooks;
  #calls = new Map();
  #highWater = 0;
  #closed = false;
  #unsubscribe;
  #setupMs;
  #operationMs;
  #microphone;
  constructor({
    platform,
    sendCommand,
    onState,
    onError,
    onDiagnostic,
    setupTimeoutMs = 30000,
    operationTimeoutMs = 10000,
  }) {
    if (!platform || typeof sendCommand !== "function")
      throw new TypeError("media platform and command sender are required");
    for (const bound of [setupTimeoutMs, operationTimeoutMs])
      if (!Number.isFinite(bound) || bound <= 0 || bound > 300000)
        throw new RangeError("media deadline is outside its bound");
    this.#platform = platform;
    this.#send = sendCommand;
    this.#hooks = { onState, onError, onDiagnostic };
    this.#setupMs = setupTimeoutMs;
    this.#operationMs = operationTimeoutMs;
    this.#unsubscribe = platform.onPageHide(() => this.close());
  }
  setMicrophone(deviceId) {
    if (
      deviceId !== undefined &&
      (typeof deviceId !== "string" || deviceId.length === 0)
    )
      throw new TypeError("device id must be a nonempty string");
    this.#microphone = deviceId;
  }
  #hook(name, event) {
    if (this.#closed) return;
    try {
      this.#hooks[name]?.(event);
    } catch {
      if (name !== "onDiagnostic") {
        try {
          this.#hooks.onDiagnostic?.({ kind: "listener-error" });
        } catch {
          /* Diagnostics cannot prevent cleanup. */
        }
      }
    }
  }
  #call(number) {
    if (this.#closed || !Number.isSafeInteger(number) || number < 1) return;
    const existing = this.#calls.get(number);
    if (existing) return existing;
    // Kernel call numbers are monotonically allocated. Retired events cannot recreate an owner.
    if (number <= this.#highWater || this.#calls.size >= 8) return;
    this.#highWater = number;
    const call = {
      number,
      live: true,
      pc: null,
      stream: null,
      tracks: new Set(),
      playback: new Set(),
      listeners: [],
      timers: new Set(),
      cancel: new Set(),
      queue: Promise.resolve(),
      kernel: null,
      browser: {},
      sip: false,
      ready: false,
      answerPending: false,
    };
    this.#calls.set(number, call);
    call.setup = this.#timer(
      call,
      () => this.#fail(call, "negotiation"),
      this.#setupMs,
    );
    return call;
  }
  #timer(call, fn, ms) {
    const id = this.#platform.clock.setTimer(() => {
      call.timers.delete(id);
      if (call.live) fn();
    }, ms);
    call.timers.add(id);
    return id;
  }
  #clear(call, id) {
    this.#platform.clock.clearTimer(id);
    call.timers.delete(id);
  }
  #listen(call, target, name, fn) {
    target.addEventListener(name, fn);
    const remove = () => target.removeEventListener(name, fn);
    call.listeners.push(remove);
    return remove;
  }
  #command(call, command) {
    if (!call.live || this.#closed) return;
    this.#send({ ...command, call: call.number });
  }
  #fail(call, kind) {
    if (!call.live || this.#closed) return;
    const error = new SipxMediaError(kind);
    try {
      this.#command(
        call,
        call.answerPending
          ? { cmd: "media-failed", reason: kind }
          : { cmd: "hangup" },
      );
    } catch {
      this.#hook("onDiagnostic", { kind: "command-error" });
    }
    this.closeCall(call.number);
    this.#hook("onError", { call: call.number, kind, error });
  }
  #operation(call, start, late) {
    if (!call.live) return Promise.reject(CANCELLED);
    return new Promise((resolve, reject) => {
      let settled = false;
      const finish = (ok, value) => {
        if (settled) {
          if (ok) late?.(value);
          return;
        }
        settled = true;
        this.#clear(call, timer);
        call.cancel.delete(cancel);
        if (ok) resolve(value);
        else reject(value);
      };
      const cancel = () => finish(false, CANCELLED);
      const timer = this.#timer(
        call,
        () => finish(false, new SipxMediaError("negotiation")),
        this.#operationMs,
      );
      call.cancel.add(cancel);
      try {
        Promise.resolve(start()).then(
          (value) => finish(true, value),
          (error) => finish(false, error),
        );
      } catch (error) {
        finish(false, error);
      }
    });
  }
  #peer(call) {
    if (call.pc) return call.pc;
    const pc = this.#platform.createPeerConnection({
      rtcpMuxPolicy: "require",
      bundlePolicy: "balanced",
    });
    call.pc = pc;
    this.#listen(call, pc, "connectionstatechange", () => {
      if (!call.live) return;
      if (pc.connectionState === "failed" || pc.connectionState === "closed")
        this.#fail(call, "negotiation");
      else if (pc.connectionState === "disconnected")
        this.#hook("onDiagnostic", { call: call.number, kind: "disconnected" });
      else this.#readiness(call);
    });
    this.#listen(call, pc, "track", (event) => {
      if (!call.live) {
        event.track?.stop();
        return;
      }
      if (event.track) {
        call.tracks.add(event.track);
        this.#listen(call, event.track, "ended", () =>
          this.#fail(call, "track-ended"),
        );
      }
      const play = async () => {
        try {
          const output = this.#platform.createPlayback(event);
          call.playback.add(output);
          await this.#operation(call, () => output.ready);
        } catch (error) {
          if (error !== CANCELLED) this.#fail(call, "autoplay");
        }
      };
      void play();
    });
    return pc;
  }
  async #capture(call) {
    if (call.stream) return;
    let stream;
    try {
      stream = await this.#operation(
        call,
        () =>
          this.#platform.getUserMedia({
            audio: this.#microphone
              ? { deviceId: { exact: this.#microphone } }
              : true,
            video: false,
          }),
        stopStream,
      );
    } catch (error) {
      if (error === CANCELLED || error instanceof SipxMediaError) throw error;
      throw new SipxMediaError(captureKind(error));
    }
    if (!call.live) {
      stopStream(stream);
      throw CANCELLED;
    }
    call.stream = stream;
    const tracks = stream.getTracks();
    if (
      tracks.length === 0 ||
      tracks.some(
        (track) => track.kind !== "audio" || track.readyState === "ended",
      )
    )
      throw new SipxMediaError("device");
    for (const track of tracks) {
      call.tracks.add(track);
      this.#listen(call, track, "ended", () => this.#fail(call, "track-ended"));
      this.#peer(call).addTrack(track, stream);
    }
    const codecs = this.#platform.codecs?.() ?? [];
    const preferred = codecs
      .filter((codec) =>
        codecNames.has(codec.mimeType?.toLowerCase().replace("audio/", "")),
      )
      .sort(
        (a, b) =>
          Number(b.mimeType?.toLowerCase() === "audio/opus") -
          Number(a.mimeType?.toLowerCase() === "audio/opus"),
      );
    if (preferred.length)
      for (const transceiver of call.pc.getTransceivers())
        transceiver.setCodecPreferences?.(preferred);
  }
  async #gather(call) {
    if (call.pc.iceGatheringState === "complete") return;
    let remove;
    try {
      await this.#operation(
        call,
        () =>
          new Promise((resolve) => {
            remove = this.#listen(
              call,
              call.pc,
              "icegatheringstatechange",
              () => {
                if (call.pc.iceGatheringState === "complete") resolve();
              },
            );
            if (call.pc.iceGatheringState === "complete") resolve();
          }),
      );
    } finally {
      remove?.();
    }
  }
  async #local(call, kind) {
    if (kind !== "offer" && kind !== "answer")
      throw new SipxMediaError("negotiation");
    const pc = this.#peer(call);
    await this.#capture(call);
    const description = await this.#operation(call, () =>
      kind === "offer" ? pc.createOffer() : pc.createAnswer(),
    );
    await this.#operation(call, () => pc.setLocalDescription(description));
    await this.#gather(call);
    if (!call.live) return;
    if (
      typeof pc.localDescription?.sdp !== "string" ||
      pc.localDescription.type !== kind
    )
      throw new SipxMediaError("negotiation");
    this.#command(call, {
      cmd: "local-media",
      kind,
      sdp: pc.localDescription.sdp,
    });
  }
  async #remote(call, event) {
    if (
      !["offer", "answer"].includes(event.kind) ||
      typeof event.sdp !== "string"
    )
      throw new SipxMediaError("negotiation");
    call.answerPending = event.kind === "answer";
    await this.#operation(call, () =>
      this.#peer(call).setRemoteDescription({
        type: event.kind,
        sdp: event.sdp,
      }),
    );
    // Only an outbound answer releases a held ACK. Applying an incoming offer needs no command.
    if (event.kind === "answer") {
      this.#command(call, { cmd: "media-applied" });
      call.answerPending = false;
    }
  }
  #readiness(call) {
    if (
      !call.live ||
      call.ready ||
      !call.sip ||
      call.pc?.connectionState !== "connected" ||
      !validFacts(call.kernel)
    )
      return;
    call.ready = true;
    this.#clear(call, call.setup);
    this.#hook("onState", { call: call.number, state: "established" });
  }
  handle(event) {
    if (
      this.#closed ||
      !event ||
      ![
        "call",
        "need-local-media",
        "remote-media",
        "negotiated-media",
        "call-ended",
      ].includes(event.evt)
    )
      return Promise.resolve();
    if (event.evt === "call-ended") {
      this.closeCall(event.call);
      return Promise.resolve();
    }
    const call = this.#call(event.call);
    if (!call) return Promise.resolve();
    if (event.evt === "call") {
      if (event.state === "sipEstablished") {
        call.sip = true;
        this.#readiness(call);
      }
      return Promise.resolve();
    }
    if (event.evt === "negotiated-media") {
      if (!validFacts(event.kernel)) this.#fail(call, "negotiation");
      else {
        call.kernel = clone(event.kernel);
        this.#readiness(call);
      }
      return Promise.resolve();
    }
    call.queue = call.queue.then(async () => {
      if (!call.live) return;
      try {
        if (event.evt === "need-local-media")
          await this.#local(call, event.kind);
        else await this.#remote(call, event);
      } catch (error) {
        if (error !== CANCELLED)
          this.#fail(
            call,
            error instanceof SipxMediaError ? error.kind : "negotiation",
          );
      }
    });
    return call.queue;
  }
  mute(number, muted) {
    const call = this.#calls.get(number);
    if (call?.live)
      for (const track of call.stream?.getAudioTracks() ?? [])
        track.enabled = !muted;
  }
  negotiatedMedia(number) {
    const call = this.#calls.get(number);
    return call?.live
      ? clone({
          ...(call.kernel ? { kernel: call.kernel } : {}),
          browser: call.browser,
        })
      : undefined;
  }
  async refreshStats(number) {
    const call = this.#calls.get(number);
    if (!call?.live || !call.pc) return;
    try {
      const stats = await this.#operation(call, () => call.pc.getStats());
      if (!call.live) return;
      const entries = [...stats.values()];
      const transport = entries.find((entry) => entry.type === "transport");
      const browser = {};
      if (typeof transport?.dtlsState === "string")
        browser.dtls_state = transport.dtlsState;
      if (typeof transport?.srtpCipher === "string")
        browser.srtp_cipher = transport.srtpCipher;
      const pair = stats.get(transport?.selectedCandidatePairId);
      if (pair?.type === "candidate-pair" && pair.state === "succeeded")
        browser.selected_candidate_pair = {
          id: pair.id,
          local_candidate_id: pair.localCandidateId,
          remote_candidate_id: pair.remoteCandidateId,
          ...(typeof pair.nominated === "boolean"
            ? { nominated: pair.nominated }
            : {}),
        };
      for (const [direction, type, packetField, byteField] of [
        ["inbound", "inbound-rtp", "packetsReceived", "bytesReceived"],
        ["outbound", "outbound-rtp", "packetsSent", "bytesSent"],
      ]) {
        const entry = entries.find(
          (value) =>
            value.type === type &&
            (value.kind === "audio" || value.mediaType === "audio") &&
            !value.isRemote,
        );
        const value = {};
        if (Number.isFinite(entry?.[packetField]))
          value.packets = entry[packetField];
        if (Number.isFinite(entry?.[byteField])) value.bytes = entry[byteField];
        if (Object.keys(value).length) browser[direction] = value;
      }
      call.browser = browser;
      return this.negotiatedMedia(number);
    } catch (error) {
      if (error !== CANCELLED && call.live)
        this.#hook("onDiagnostic", { call: number, kind: "stats-unavailable" });
    }
  }
  closeCall(number) {
    const call = this.#calls.get(number);
    if (!call) return;
    call.live = false;
    this.#calls.delete(number);
    for (const cancel of [...call.cancel]) cancel();
    for (const timer of [...call.timers]) this.#clear(call, timer);
    for (const remove of call.listeners) {
      try {
        remove();
      } catch {
        /* Release the remaining resources. */
      }
    }
    for (const output of call.playback) {
      try {
        output.close();
      } catch {
        /* Continue teardown. */
      }
    }
    for (const transceiver of call.pc?.getTransceivers?.() ?? []) {
      try {
        transceiver.stop();
      } catch {
        /* Continue teardown. */
      }
    }
    for (const track of call.tracks) {
      try {
        track.stop();
      } catch {
        /* Continue teardown. */
      }
    }
    if (call.stream)
      for (const track of call.stream.getTracks())
        if (!call.tracks.has(track)) {
          try {
            track.stop();
          } catch {
            /* Continue teardown. */
          }
        }
    try {
      call.pc?.close();
    } catch {
      /* Ownership has already been revoked. */
    }
  }
  close() {
    if (this.#closed) return;
    this.#closed = true;
    for (const number of [...this.#calls.keys()]) this.closeCall(number);
    this.#unsubscribe?.();
    this.#unsubscribe = undefined;
  }
}
