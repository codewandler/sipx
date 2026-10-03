export function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
export class Events {
  listeners = new Map();
  addEventListener(name, handler) {
    if (!this.listeners.has(name)) this.listeners.set(name, new Set());
    this.listeners.get(name).add(handler);
  }
  removeEventListener(name, handler) {
    this.listeners.get(name)?.delete(handler);
  }
  emit(name, extra = {}) {
    for (const handler of [...(this.listeners.get(name) ?? [])]) handler(extra);
  }
  get listenerCount() {
    return [...this.listeners.values()].reduce((n, set) => n + set.size, 0);
  }
}
export class Track extends Events {
  kind = "audio";
  enabled = true;
  readyState = "live";
  stopped = 0;
  stop() {
    this.stopped++;
    this.readyState = "ended";
  }
}
export class Stream {
  constructor(tracks = [new Track()]) {
    this.tracks = tracks;
  }
  getTracks() {
    return this.tracks;
  }
  getAudioTracks() {
    return this.tracks.filter((t) => t.kind === "audio");
  }
}
export class Peer extends Events {
  iceGatheringState = "complete";
  connectionState = "new";
  closed = false;
  localDescription = null;
  remoteDescription = null;
  transceivers = [];
  stats = new Map();
  offer = { type: "offer", sdp: "opaque browser offer" };
  answer = { type: "answer", sdp: "opaque browser answer" };
  async createOffer() {
    return this.offer;
  }
  async createAnswer() {
    return this.answer;
  }
  async setLocalDescription(description) {
    this.localDescription = description;
  }
  async setRemoteDescription(description) {
    this.remoteDescription = description;
  }
  addTrack(track) {
    const transceiver = {
      sender: { track },
      stopped: false,
      setCodecPreferences(codecs) {
        this.codecs = codecs;
      },
      stop() {
        this.stopped = true;
      },
    };
    this.transceivers.push(transceiver);
    return transceiver.sender;
  }
  getTransceivers() {
    return this.transceivers;
  }
  getReceivers() {
    return [];
  }
  async getStats() {
    return this.stats;
  }
  close() {
    this.closed = true;
    this.connectionState = "closed";
  }
  connect(state = "connected") {
    this.connectionState = state;
    this.emit("connectionstatechange");
  }
  gather() {
    this.iceGatheringState = "complete";
    this.emit("icegatheringstatechange");
  }
}
export function fixture() {
  const timers = new Map();
  let id = 0;
  const page = new Events();
  const peers = [];
  const streams = [];
  const playback = [];
  const commands = [],
    states = [],
    errors = [],
    diagnostics = [];
  const platform = {
    clock: {
      setTimer(fn, ms) {
        const key = ++id;
        timers.set(key, { fn, ms });
        return key;
      },
      clearTimer(key) {
        timers.delete(key);
      },
    },
    getUserMedia: async (constraints) => {
      platform.constraints = constraints;
      const stream = new Stream();
      streams.push(stream);
      return stream;
    },
    createPeerConnection: () => {
      const peer = new Peer();
      peers.push(peer);
      return peer;
    },
    codecs: () => [
      { mimeType: "audio/PCMU", clockRate: 8000 },
      { mimeType: "audio/opus", clockRate: 48000, channels: 2 },
    ],
    createPlayback: () => {
      const resource = {
        ready: Promise.resolve(),
        closed: false,
        close() {
          this.closed = true;
        },
      };
      playback.push(resource);
      return resource;
    },
    onPageHide: (handler) => {
      page.addEventListener("pagehide", handler);
      return () => page.removeEventListener("pagehide", handler);
    },
  };
  return {
    platform,
    peers,
    streams,
    playback,
    timers,
    page,
    commands,
    states,
    errors,
    diagnostics,
    options: {
      platform,
      sendCommand: (command) => commands.push(command),
      onState: (value) => states.push(value),
      onError: (value) => errors.push(value),
      onDiagnostic: (value) => diagnostics.push(value),
    },
    fireTimers() {
      for (const [key, timer] of [...timers]) {
        timers.delete(key);
        timer.fn();
      }
    },
  };
}
export function facts(call = 1) {
  const codec = {
    name: "opus",
    clock_rate: 48000,
    channels: 2,
    payload_type: 111,
  };
  return {
    v: 1,
    evt: "negotiated-media",
    call,
    kernel: {
      dialog: {
        call_id: "test-call",
        local_tag: "local",
        remote_tag: "remote",
      },
      codecs: [codec],
      selected_codec: codec,
      fingerprint_algorithm: "sha-256",
      answer_setup: "active",
      local_dtls_role: "passive",
      rtcp_mux: true,
      audio_sections: 1,
    },
  };
}
export const local = (call = 1, kind = "offer") => ({
  v: 1,
  evt: "need-local-media",
  call,
  kind,
  constraints: { audio: true, video: false },
});
export const remote = (call = 1, kind = "answer") => ({
  v: 1,
  evt: "remote-media",
  call,
  kind,
  sdp: "opaque remote description",
});
export const established = (call = 1) => ({
  v: 1,
  evt: "call",
  call,
  state: "sipEstablished",
});
export async function tick() {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}
