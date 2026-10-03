// Proof-only fault injection surrounds the exact installed package. No SIP client here.
import { SipxClient } from "@sipx/browser";
const real = {
  setTimeout: globalThis.setTimeout.bind(globalThis),
  clearTimeout: globalThis.clearTimeout.bind(globalThis),
  WebSocket: globalThis.WebSocket,
  RTCPeerConnection: globalThis.RTCPeerConnection,
};
const delay = (ms) => new Promise((resolve) => real.setTimeout(resolve, ms));
const hash = async (text) =>
  Array.from(
    new Uint8Array(
      await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)),
    ),
    (b) => b.toString(16).padStart(2, "0"),
  ).join("");
window.runProof = async (config) => {
  const timerStacks = new Map();
  const live = {
    timers: new Set(),
    sockets: new Set(),
    pcs: new Set(),
    tracks: new Set(),
  };
  const evidence = {
    case: config.case,
    role: config.role,
    established: 0,
    states: [],
    fault: null,
  };
  let ctx = new AudioContext(),
    client,
    call;
  const originalStop = MediaStreamTrack.prototype.stop;
  MediaStreamTrack.prototype.stop = function () {
    live.tracks.delete(this);
    return originalStop.call(this);
  };
  globalThis.setTimeout = (fn, ms, ...args) => {
    let id;
    id = real.setTimeout(() => {
      live.timers.delete(id);
      fn(...args);
    }, ms);
    const stack = new Error().stack;
    if (stack.includes("/node_modules/@sipx/browser/")) live.timers.add(id);
    timerStacks.set(id, stack);
    return id;
  };
  globalThis.clearTimeout = (id) => {
    live.timers.delete(id);
    real.clearTimeout(id);
  };
  let observedPc,
    remotePeak = 0;
  let capturePending = 0;
  globalThis.RTCPeerConnection = class extends real.RTCPeerConnection {
    constructor(options) {
      super(options);
      live.pcs.add(this);
      observedPc = this;
      evidence.connections = [];
      for (const name of [
        "iceconnectionstatechange",
        "connectionstatechange",
        "icegatheringstatechange",
      ])
        this.addEventListener(name, () =>
          evidence.connections.push({
            event: name,
            ice: this.iceConnectionState,
            connection: this.connectionState,
            gathering: this.iceGatheringState,
          }),
        );
      this.addEventListener("track", (e) => {
        live.tracks.add(e.track);
        if (!ctx) return;
        const stream = e.streams[0] ?? new MediaStream([e.track]);
        const input = ctx.createMediaStreamSource(stream),
          analyser = ctx.createAnalyser();
        input.connect(analyser);
        const samples = new Float32Array(analyser.fftSize);
        const sample = () => {
          if (!live.pcs.has(this)) return;
          analyser.getFloatTimeDomainData(samples);
          remotePeak = Math.max(remotePeak, ...samples.map(Math.abs));
          real.setTimeout(sample, 20);
        };
        sample();
      });
    }
    async setRemoteDescription(description) {
      evidence.remoteSdp = description.sdp;
      try {
        return await super.setRemoteDescription(description);
      } catch (error) {
        evidence.remoteDescriptionError = {
          name: error.name,
          message: error.message,
        };
        throw error;
      }
    }
    async setLocalDescription(description) {
      await super.setLocalDescription(description);
      evidence.localSdp = this.localDescription?.sdp;
      evidence.codecs = RTCRtpSender.getCapabilities("audio").codecs;
    }
    close() {
      evidence.localSdp = this.localDescription?.sdp ?? evidence.localSdp;
      live.pcs.delete(this);
      super.close();
    }
  };
  function mutation(input) {
    const original =
      typeof input === "string" ? input : new TextDecoder().decode(input);
    evidence.received ??= [];
    evidence.received.push({
      line: original.split("\r\n")[0],
      cseq: original.match(/^CSeq:.*$/im)?.[0],
    });
    if (original.startsWith("SIP/2.0 200") && /CSeq:.*INVITE/i.test(original))
      evidence.receivedAnswer = original.split("\r\n\r\n")[1];
    if (
      evidence.fault ||
      !/^SIP\/2.0 200 /m.test(original) ||
      !/^CSeq:\s*\d+ INVITE\r?$/im.test(original) ||
      !original.includes("m=audio")
    )
      return input;
    let changed = original;
    if (config.case === "wrong-fingerprint")
      changed = changed.replace(
        /a=fingerprint:sha-256 [^\r\n]+/g,
        "a=fingerprint:sha-256 " + Array(32).fill("00").join(":"),
      );
    if (config.case === "missing-ice") {
      if (
        !original.includes(
          `${config.mediaAddress} ${config.blackholePort} typ host`,
        )
      )
        throw Error("native blackhole was not advertised");
      evidence.fault = {
        kind: config.case,
        nativeBlackhole: true,
        port: config.blackholePort,
      };
      return input;
    }
    if (config.case === "weaker-media")
      changed = changed
        .replaceAll("UDP/TLS/RTP/SAVPF", "RTP/AVP")
        .replaceAll("UDP/TLS/RTP/SAVP", "RTP/AVP");
    if (config.case === "oversized-signalling") changed = "X".repeat(65537);
    if (changed === original) return input;
    if (config.case !== "oversized-signalling") {
      const body = changed.split("\r\n\r\n")[1];
      changed = changed.replace(
        /Content-Length:\s*\d+/i,
        "Content-Length: " + new TextEncoder().encode(body).length,
      );
    }
    evidence.fault = {
      kind: config.case,
      beforeBytes: new TextEncoder().encode(original).length,
      afterBytes: new TextEncoder().encode(changed).length,
      changed: true,
    };
    evidence.hashes = Promise.all([hash(original), hash(changed)]);
    return typeof input === "string"
      ? changed
      : new TextEncoder().encode(changed).buffer;
  }
  globalThis.WebSocket = class extends real.WebSocket {
    #listeners = new Map();
    send(data) {
      const text =
        typeof data === "string" ? data : new TextDecoder().decode(data);
      evidence.sent ??= [];
      evidence.sent.push({
        line: text.split("\r\n")[0],
        cseq: text.match(/^CSeq:.*$/im)?.[0],
      });
      return super.send(data);
    }
    constructor(...args) {
      super(...args);
      live.sockets.add(this);
      super.addEventListener("close", (event) => {
        live.sockets.delete(this);
        evidence.socketClose = { code: event.code, clean: event.wasClean };
      });
      super.addEventListener("error", () => {
        evidence.socketError = true;
      });
    }
    addEventListener(name, fn, options) {
      if (name !== "message") return super.addEventListener(name, fn, options);
      const wrapped = (event) =>
        fn(new MessageEvent("message", { data: mutation(event.data) }));
      this.#listeners.set(fn, wrapped);
      return super.addEventListener(name, wrapped, options);
    }
    removeEventListener(name, fn, options) {
      return super.removeEventListener(
        name,
        this.#listeners.get(fn) ?? fn,
        options,
      );
    }
    close(...args) {
      super.close(...args);
    }
  };
  const originalCapture = navigator.mediaDevices.getUserMedia.bind(
    navigator.mediaDevices,
  );
  navigator.mediaDevices.getUserMedia = async (constraints) => {
    capturePending++;
    try {
      const permission = await originalCapture(constraints);
      for (const track of permission.getTracks()) track.stop();
      if (config.case === "cancel-during-setup") {
        evidence.fault = { kind: config.case, capturePending: true };
        await delay(250);
      }
      ctx ??= new AudioContext();
      await ctx.resume();
      const oscillator = ctx.createOscillator(),
        destination = ctx.createMediaStreamDestination();
      oscillator.frequency.value = 440;
      oscillator.connect(destination);
      oscillator.start();
      for (const track of destination.stream.getTracks())
        live.tracks.add(track);
      return destination.stream;
    } finally {
      capturePending--;
    }
  };
  let resultError;
  try {
    const transport = {
      scheme: config.case === "insecure-signalling" ? "ws" : "wss",
      host: `localhost:${config.port}`,
      resource: "/sip",
    };
    if (config.case === "insecure-signalling")
      evidence.fault = { kind: config.case, scheme: "ws", policy: "refuse" };
    client = await SipxClient.create(
      {
        v: 1,
        aor: "sip:browser@localhost",
        auth: { username: "browser", password: "fixture-password" },
        transport,
        insecure: "refuse",
      },
      {
        setupTimeoutMs: config.case === "positive" ? 30000 : 6000,
        operationTimeoutMs: 10000,
      },
    );
    client.on("outgoing", (c) => {
      c.on("state", (state) => {
        evidence.states.push(state);
        if (state === "established") evidence.established++;
      });
    });
    const incoming = new Promise((resolve, reject) =>
      client.on("incoming", (c) => {
        c.on("state", (state) => {
          evidence.states.push(state);
          if (state === "established") evidence.established++;
        });
        c.answer().then(resolve, reject);
      }),
    );
    await client.register();
    if (config.role === "browser-offerer") {
      const abort = new AbortController();
      const pending = client.dial("sip:native@localhost", {
        signal: abort.signal,
      });
      if (config.case === "cancel-during-setup") {
        while (!evidence.fault) await delay(10);
        abort.abort();
      }
      call = await pending;
    } else call = await incoming;
    if (config.case !== "positive")
      throw Error("negative established unexpectedly");
    const mediaDeadline = performance.now() + 15000;
    for (;;) {
      const native = await (
        await fetch("/native-status", { cache: "no-store" })
      ).json();
      if (native.error) throw Error("native media failed");
      if (native.status === "media-ready" && remotePeak > 0.001) break;
      if (performance.now() > mediaDeadline)
        throw Error("media observation deadline");
      await delay(50);
    }
    evidence.remotePeak = remotePeak;
    evidence.media = await call.refreshStats();
    if (!(remotePeak > 0.001)) throw Error("browser audio absent or silent");
    await call.hangup();
  } catch (error) {
    resultError = { name: error.name, kind: error.kind, reason: error.reason };
  } finally {
    await client?.close({ timeoutMs: 500 });
    const cleanupDeadline = performance.now() + 3000;
    while (
      capturePending ||
      Object.values(live).some((values) => values.size)
    ) {
      if (performance.now() > cleanupDeadline) break;
      await delay(10);
    }
    evidence.capturePending = capturePending;
    await ctx?.close();
    if (evidence.hashes) {
      const [before, after] = await evidence.hashes;
      delete evidence.hashes;
      evidence.fault.sha256 = { before, after };
    }
    evidence.timerStacks = [...live.timers].map((id) => timerStacks.get(id));
    evidence.resources = Object.fromEntries(
      Object.entries(live).map(([name, values]) => [name, values.size]),
    );
    evidence.error = resultError ?? null;
    evidence.pcState = observedPc?.connectionState;
    globalThis.WebSocket = real.WebSocket;
    globalThis.RTCPeerConnection = real.RTCPeerConnection;
    globalThis.setTimeout = real.setTimeout;
    globalThis.clearTimeout = real.clearTimeout;
    navigator.mediaDevices.getUserMedia = originalCapture;
    MediaStreamTrack.prototype.stop = originalStop;
  }
  return evidence;
};
