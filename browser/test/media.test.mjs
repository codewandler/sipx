import test from "node:test";
import assert from "node:assert/strict";
import { BrowserMediaAdapter } from "../src/media.mjs";
import {
  fixture,
  deferred,
  Stream,
  Peer,
  facts,
  local,
  remote,
  established,
  tick,
} from "./media-fakes.mjs";

function setup(options = {}) {
  const f = fixture();
  f.adapter = new BrowserMediaAdapter({ ...f.options, ...options });
  return f;
}
function clean(f) {
  assert.equal(f.timers.size, 0);
  for (const pc of f.peers) {
    assert.equal(pc.closed, true);
    assert.equal(pc.listenerCount, 0);
  }
  for (const stream of f.streams)
    for (const track of stream.getTracks()) {
      assert.equal(track.readyState, "ended");
      assert.equal(track.listenerCount, 0);
    }
}
test("construction acquires no media; complete offer preserves SDP and preferences", async () => {
  const f = setup();
  assert.equal(f.peers.length, 0);
  assert.equal(f.streams.length, 0);
  f.adapter.setMicrophone("mic-2");
  await f.adapter.handle(local());
  assert.deepEqual(f.platform.constraints, {
    audio: { deviceId: { exact: "mic-2" } },
    video: false,
  });
  assert.equal(f.commands[0].sdp, "opaque browser offer");
  assert.equal(f.commands[0].cmd, "local-media");
  assert.equal(f.peers[0].transceivers[0].codecs[0].mimeType, "audio/opus");
  f.adapter.mute(1, true);
  assert.equal(f.streams[0].tracks[0].enabled, false);
  f.adapter.close();
  clean(f);
  assert.equal(f.page.listenerCount, 0);
});
test("local SDP waits for complete ICE gathering", async () => {
  const f = setup();
  const pc = new Peer();
  pc.iceGatheringState = "gathering";
  f.platform.createPeerConnection = () => {
    f.peers.push(pc);
    return pc;
  };
  const pending = f.adapter.handle(local());
  await tick();
  assert.equal(f.commands.length, 0);
  pc.gather();
  await pending;
  assert.equal(f.commands[0].cmd, "local-media");
  f.adapter.close();
  clean(f);
});
test("inbound offer does not capture until answer and remote application precedes acknowledgment", async () => {
  const f = setup();
  await f.adapter.handle(remote(1, "offer"));
  assert.equal(f.streams.length, 0);
  assert.equal(f.commands.length, 0);
  await f.adapter.handle(local(1, "answer"));
  assert.equal(f.commands[0].sdp, "opaque browser answer");
  f.adapter.close();
  clean(f);
});
for (const omitted of ["kernel", "browser", "facts"])
  test(`readiness refuses missing ${omitted}`, async () => {
    const f = setup();
    await f.adapter.handle(local());
    if (omitted !== "facts") await f.adapter.handle(facts());
    if (omitted !== "kernel") await f.adapter.handle(established());
    if (omitted !== "browser") f.peers[0].connect();
    assert.equal(
      f.states.some((e) => e.state === "established"),
      false,
    );
    f.adapter.close();
    clean(f);
  });
test("readiness requires all facts, emits once and ignores transient disconnected", async () => {
  const f = setup();
  await f.adapter.handle(local());
  await f.adapter.handle(facts());
  await f.adapter.handle(established());
  f.peers[0].connect();
  f.peers[0].connect();
  assert.equal(f.states.filter((e) => e.state === "established").length, 1);
  f.peers[0].connect("disconnected");
  assert.equal(f.peers[0].closed, false);
  assert.equal(f.diagnostics.at(-1).kind, "disconnected");
  f.peers[0].connect("failed");
  assert.equal(f.errors.at(-1).kind, "negotiation");
  clean(f);
  f.adapter.close();
});
for (const [field, value] of [
  ["rtcp_mux", false],
  ["audio_sections", 2],
  ["fingerprint_algorithm", "sha-1"],
  ["selected_codec", null],
])
  test(`invalid kernel ${field} cannot establish`, async () => {
    const f = setup();
    await f.adapter.handle(local());
    const event = facts();
    event.kernel[field] = value;
    await f.adapter.handle(event);
    await f.adapter.handle(established());
    f.peers[0].connect();
    assert.equal(
      f.states.some((e) => e.state === "established"),
      false,
    );
    f.adapter.close();
    clean(f);
  });
for (const [name, kind] of [
  ["NotAllowedError", "permission"],
  ["NotFoundError", "device"],
  ["NotReadableError", "device"],
  ["OverconstrainedError", "device"],
])
  test(`capture ${name} remains ${kind}`, async () => {
    const f = setup();
    f.platform.getUserMedia = async () => {
      throw Object.assign(new Error("SECRET"), { name });
    };
    await f.adapter.handle(local());
    assert.equal(f.errors[0].kind, kind);
    assert.equal(JSON.stringify(f.errors).includes("SECRET"), false);
    clean(f);
    f.adapter.close();
  });
test("late permission cannot revive a canceled call and its stream is stopped", async () => {
  const f = setup();
  const gate = deferred();
  f.platform.getUserMedia = () => gate.promise;
  const pending = f.adapter.handle(local());
  await tick();
  f.adapter.closeCall(1);
  await pending;
  const stream = new Stream();
  f.streams.push(stream);
  gate.resolve(stream);
  await tick();
  assert.equal(f.commands.length, 0);
  clean(f);
  await f.adapter.handle(local());
  assert.equal(f.commands.length, 0);
  f.adapter.close();
});
for (const operation of [
  "createOffer",
  "setLocalDescription",
  "setRemoteDescription",
  "getStats",
])
  test(`late ${operation} is inert after cancellation`, async () => {
    const f = setup();
    const gate = deferred();
    const pc = new Peer();
    pc[operation] = () => gate.promise;
    f.platform.createPeerConnection = () => {
      f.peers.push(pc);
      return pc;
    };
    let pending;
    if (operation === "setRemoteDescription")
      pending = f.adapter.handle(remote());
    else if (operation === "getStats") {
      await f.adapter.handle(local());
      pending = f.adapter.refreshStats(1);
    } else pending = f.adapter.handle(local());
    await tick();
    const before = f.commands.length;
    f.adapter.closeCall(1);
    await pending;
    gate.resolve(operation === "getStats" ? new Map() : pc.offer);
    await tick();
    assert.equal(f.commands.length, before);
    clean(f);
    f.adapter.close();
  });
test("ICE cancellation removes listener immediately; timeout cleans without fixed wait", async () => {
  const f = setup();
  const pc = new Peer();
  pc.iceGatheringState = "gathering";
  f.platform.createPeerConnection = () => {
    f.peers.push(pc);
    return pc;
  };
  const pending = f.adapter.handle(local());
  await tick();
  f.fireTimers();
  await pending;
  assert.equal(f.errors[0].kind, "negotiation");
  clean(f);
  f.adapter.close();
});
test("remote description refusal emits no media-applied", async () => {
  const f = setup();
  const pc = new Peer();
  pc.setRemoteDescription = async () => {
    throw new Error("SECRET SDP");
  };
  f.platform.createPeerConnection = () => {
    f.peers.push(pc);
    return pc;
  };
  await f.adapter.handle(remote());
  assert.equal(
    f.commands.some((c) => c.cmd === "media-applied"),
    false,
  );
  assert.equal(f.errors[0].kind, "negotiation");
  clean(f);
  f.adapter.close();
});
test("track ended and playback refusal remain distinct", async () => {
  const f = setup();
  await f.adapter.handle(local());
  f.streams[0].tracks[0].emit("ended");
  assert.equal(f.errors[0].kind, "track-ended");
  clean(f);
  f.adapter.close();
  const p = setup();
  p.platform.createPlayback = () => ({
    ready: Promise.reject(new Error("SECRET")),
    close() {},
  });
  await p.adapter.handle(local());
  p.peers[0].emit("track", {
    track: new Stream().tracks[0],
    streams: [new Stream()],
  });
  await tick();
  assert.equal(p.errors[0].kind, "autoplay");
  clean(p);
  p.adapter.close();
});
test("stats preserve origins, omit missing fields and exclude candidate addresses", async () => {
  const f = setup();
  await f.adapter.handle(local());
  await f.adapter.handle(facts());
  f.peers[0].stats = new Map([
    [
      "t",
      {
        id: "t",
        type: "transport",
        dtlsState: "connected",
        selectedCandidatePairId: "p",
      },
    ],
    [
      "p",
      {
        id: "p",
        type: "candidate-pair",
        state: "succeeded",
        nominated: true,
        localCandidateId: "l",
        remoteCandidateId: "r",
        address: "SECRET",
      },
    ],
    [
      "in",
      {
        type: "inbound-rtp",
        kind: "audio",
        packetsReceived: 4,
        bytesReceived: 8,
      },
    ],
  ]);
  await f.adapter.refreshStats(1);
  const report = f.adapter.negotiatedMedia(1);
  assert.equal(report.kernel.rtcp_mux, true);
  assert.equal(report.browser.dtls_state, "connected");
  assert.equal(report.browser.inbound.packets, 4);
  assert.equal("srtp_cipher" in report.browser, false);
  assert.equal(JSON.stringify(report).includes("SECRET"), false);
  report.kernel.rtcp_mux = false;
  assert.equal(f.adapter.negotiatedMedia(1).kernel.rtcp_mux, true);
  f.adapter.close();
  clean(f);
});
test("pagehide and repeated close release all owned handles and silence callbacks", async () => {
  const f = setup();
  await f.adapter.handle(local());
  f.page.emit("pagehide");
  f.adapter.close();
  clean(f);
  assert.equal(f.page.listenerCount, 0);
  const count = f.states.length;
  await f.adapter.handle(local(2));
  f.peers[0].connect();
  assert.equal(f.states.length, count);
});

test("permission failure requests hangup rather than invalid media-failed command", async () => {
  const f = setup();
  f.platform.getUserMedia = async () => {
    throw Object.assign(new Error(), { name: "NotAllowedError" });
  };
  await f.adapter.handle(local());
  assert.equal(f.commands[0].cmd, "hangup");
  f.adapter.close();
  clean(f);
});
test("outbound answer success acknowledges only after application; failure uses media-failed", async () => {
  const f = setup();
  await f.adapter.handle(local());
  await f.adapter.handle(remote());
  assert.equal(f.commands.at(-1).cmd, "media-applied");
  f.adapter.close();
  const p = setup();
  const pc = new Peer();
  pc.setRemoteDescription = async () => {
    throw new Error();
  };
  p.platform.createPeerConnection = () => {
    p.peers.push(pc);
    return pc;
  };
  await p.adapter.handle(remote());
  assert.equal(p.commands.at(-1).cmd, "media-failed");
  p.adapter.close();
});
test("playback canceled before play resolves is released immediately", async () => {
  const f = setup();
  const gate = deferred();
  const output = {
    ready: gate.promise,
    closed: false,
    close() {
      this.closed = true;
    },
  };
  f.platform.createPlayback = () => output;
  await f.adapter.handle(local());
  f.peers[0].emit("track", {
    track: new Stream().tracks[0],
    streams: [new Stream()],
  });
  await tick();
  f.adapter.close();
  assert.equal(output.closed, true);
  clean(f);
  gate.resolve();
  await tick();
  clean(f);
});
test("incoming description failure rejects with hangup, not outbound media-failed", async () => {
  const f = setup();
  const pc = new Peer();
  pc.setRemoteDescription = async () => {
    throw new Error();
  };
  f.platform.createPeerConnection = () => {
    f.peers.push(pc);
    return pc;
  };
  await f.adapter.handle(remote(1, "offer"));
  assert.equal(f.commands[0].cmd, "hangup");
  f.adapter.close();
  clean(f);
});
test("late answer and ICE events cannot submit commands after terminal event", async () => {
  const f = setup();
  const pc = new Peer();
  pc.iceGatheringState = "gathering";
  f.platform.createPeerConnection = () => {
    f.peers.push(pc);
    return pc;
  };
  const pending = f.adapter.handle(local());
  await tick();
  await f.adapter.handle({ evt: "call-ended", call: 1 });
  await pending;
  pc.gather();
  await f.adapter.handle(remote());
  assert.equal(f.commands.length, 0);
  clean(f);
  f.adapter.close();
});
test("listener exception cannot retain resources or prevent diagnostics", async () => {
  const f = setup({
    onState() {
      throw new Error("SECRET");
    },
  });
  await f.adapter.handle(local());
  await f.adapter.handle(facts());
  await f.adapter.handle(established());
  f.peers[0].connect();
  assert.equal(f.diagnostics.at(-1).kind, "listener-error");
  f.adapter.close();
  clean(f);
});
test("stats timeout stays diagnostic, leaves media alive and clears its timer", async () => {
  const f = setup();
  await f.adapter.handle(local());
  await f.adapter.handle(facts());
  await f.adapter.handle(established());
  f.peers[0].connect();
  f.peers[0].getStats = () => new Promise(() => {});
  const pending = f.adapter.refreshStats(1);
  await tick();
  f.fireTimers();
  await pending;
  assert.equal(f.peers[0].closed, false);
  assert.equal(f.diagnostics.at(-1).kind, "stats-unavailable");
  assert.equal(f.timers.size, 0);
  f.adapter.close();
  clean(f);
});
test("media setup deadline fails a silent transport and clears every resource", async () => {
  const f = setup();
  await f.adapter.handle(local());
  f.fireTimers();
  assert.equal(f.errors[0].kind, "negotiation");
  assert.equal(f.commands.at(-1).cmd, "hangup");
  clean(f);
  f.adapter.close();
});
test("platform capability checks fail before constructing browser resources", async () => {
  const { browserMediaPlatform, MediaCapabilityError } =
    await import("../src/media-platform.mjs");
  assert.throws(
    () => browserMediaPlatform({ isSecureContext: false }),
    (error) =>
      error instanceof MediaCapabilityError &&
      error.capability === "secure context",
  );
  assert.throws(
    () => browserMediaPlatform({ isSecureContext: true }),
    (error) => error.capability === "RTCPeerConnection",
  );
});
test("browser platform owns audio playback synchronously even with unresolved play", async () => {
  const { browserMediaPlatform } = await import("../src/media-platform.mjs");
  const gate = deferred();
  const audio = {
    play: () => gate.promise,
    pause() {
      this.paused = true;
    },
    remove() {
      this.removed = true;
    },
  };
  const scope = {
    isSecureContext: true,
    RTCPeerConnection: Peer,
    navigator: { mediaDevices: { getUserMedia: async () => new Stream() } },
    document: { createElement: () => audio },
    addEventListener() {},
    removeEventListener() {},
  };
  const platform = browserMediaPlatform(scope);
  const stream = new Stream();
  const output = platform.createPlayback({ streams: [stream] });
  assert.equal(audio.srcObject, stream);
  output.close();
  assert.equal(audio.srcObject, null);
  assert.equal(audio.paused, true);
  assert.equal(audio.removed, true);
  gate.resolve();
  await output.ready;
});

test("late permission after operation timeout releases tracks without a second command", async () => {
  const f = fixture();
  const pending = deferred();
  f.platform.getUserMedia = () => pending.promise;
  const adapter = new BrowserMediaAdapter(f.options);
  const work = adapter.handle(local());
  await tick();
  const timer = [...f.timers.entries()].find(([, item]) => item.ms === 10000);
  assert.ok(timer);
  f.timers.delete(timer[0]);
  timer[1].fn();
  await work;
  assert.equal(f.errors.length, 1);
  assert.equal(f.errors[0].kind, "negotiation");
  assert.deepEqual(f.commands, [{ cmd: "hangup", call: 1 }]);
  const stream = new Stream();
  pending.resolve(stream);
  await tick();
  assert.equal(stream.getTracks()[0].readyState, "ended");
  assert.equal(f.commands.length, 1);
  assert.equal(f.timers.size, 0);
  assert.equal(f.peers[0].closed, true);
  adapter.close();
});

test("synchronous command enqueue refusal releases acquired resources", async () => {
  const f = fixture();
  const attempted = [];
  const adapter = new BrowserMediaAdapter({
    ...f.options,
    sendCommand(command) {
      attempted.push(command.cmd);
      throw new Error("sender unavailable");
    },
  });
  await adapter.handle(local());
  assert.deepEqual(attempted, ["local-media", "hangup"]);
  assert.equal(f.errors[0].kind, "negotiation");
  assert.equal(f.streams[0].getTracks()[0].readyState, "ended");
  assert.equal(f.peers[0].closed, true);
  assert.equal(f.peers[0].listenerCount, 0);
  assert.equal(f.timers.size, 0);
  adapter.close();
});

test("codec preferences preserve the profile-required comfort noise alongside speech and telephone events", async () => {
  const f = setup();
  f.platform.codecs = () =>
    ["PCMU", "CN", "telephone-event", "opus", "PCMA", "G722"].map((name) => ({
      mimeType: `audio/${name}`,
    }));
  await f.adapter.handle(local());
  const names = f.peers[0].transceivers[0].codecs.map((codec) =>
    codec.mimeType.toLowerCase(),
  );
  assert.ok(names.includes("audio/cn"));
  assert.ok(names.includes("audio/telephone-event"));
  assert.equal(names[0], "audio/opus");
  assert.equal(names.includes("audio/g722"), false);
  f.adapter.close();
  clean(f);
});

test("readiness admits profile comfort noise facts without selecting comfort noise as speech", async () => {
  const f = setup();
  await f.adapter.handle(local());
  const event = facts();
  event.kernel.codecs.push({
    name: "cn",
    clock_rate: 8000,
    channels: 1,
    payload_type: 13,
  });
  await f.adapter.handle(event);
  await f.adapter.handle(established());
  f.peers[0].connect();
  assert.equal(f.states.filter((e) => e.state === "established").length, 1);
  f.adapter.close();
  clean(f);
});
test("comfort noise cannot be the selected speech codec", async () => {
  const f = setup();
  await f.adapter.handle(local());
  const event = facts();
  const cn = { name: "cn", clock_rate: 8000, channels: 1, payload_type: 13 };
  event.kernel.codecs.push(cn);
  event.kernel.selected_codec = cn;
  await f.adapter.handle(event);
  assert.equal(f.errors.at(-1).kind, "negotiation");
  clean(f);
  f.adapter.close();
});
