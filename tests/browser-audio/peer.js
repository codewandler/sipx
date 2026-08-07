"use strict";

(() => {
  const CONTRACT = "sipx.browser-audio.v1";
  const MAX_SIP_BYTES = 65535;
  let proofState = null;

  function fail(message) {
    throw new Error(message);
  }

  function randomToken() {
    return crypto.randomUUID().replaceAll("-", "");
  }

  function withDeadline(label, milliseconds, start) {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`${label} exceeded ${milliseconds} ms`)), milliseconds);
      Promise.resolve()
        .then(start)
        .then((value) => {
          clearTimeout(timer);
          resolve(value);
        }, (error) => {
          clearTimeout(timer);
          reject(error);
        });
    });
  }

  function waitEvent(target, name, predicate, milliseconds) {
    return withDeadline(name, milliseconds, () => new Promise((resolve) => {
      const listener = (event) => {
        if (!predicate || predicate(event)) {
          target.removeEventListener(name, listener);
          resolve(event);
        }
      };
      target.addEventListener(name, listener);
    }));
  }

  function parseSip(text) {
    if (new TextEncoder().encode(text).length > MAX_SIP_BYTES) fail("SIP WebSocket message exceeds 65535 bytes");
    const split = text.indexOf("\r\n\r\n");
    if (split < 0) fail("SIP message has no header terminator");
    const lines = text.slice(0, split).split("\r\n");
    const start = lines.shift();
    const headers = new Map();
    for (const line of lines) {
      const colon = line.indexOf(":");
      if (colon <= 0) fail(`malformed SIP header: ${line}`);
      const name = line.slice(0, colon).trim().toLowerCase();
      const value = line.slice(colon + 1).trim();
      const values = headers.get(name) || [];
      values.push(value);
      headers.set(name, values);
    }
    const body = text.slice(split + 4);
    const declared = Number.parseInt((headers.get("content-length") || [String(body.length)])[0], 10);
    if (!Number.isSafeInteger(declared) || declared !== new TextEncoder().encode(body).length) {
      fail("SIP Content-Length disagrees with the WebSocket message");
    }
    return {start, headers, body};
  }

  function header(message, name) {
    const value = message.headers.get(name.toLowerCase());
    if (!value || !value[0]) fail(`SIP message has no ${name}`);
    return value[0];
  }

  function sipMessage(start, headers, body = "") {
    const encoded = new TextEncoder().encode(body);
    return `${start}\r\n${headers.join("\r\n")}\r\nContent-Length: ${encoded.length}\r\n\r\n${body}`;
  }

  class MessageQueue {
    constructor(socket) {
      this.messages = [];
      this.waiters = [];
      this.error = null;
      socket.addEventListener("message", (event) => {
        try {
          if (typeof event.data !== "string") fail("SIP arrived in a non-text WebSocket message");
          const message = parseSip(event.data);
          const waiter = this.waiters.shift();
          if (waiter) waiter.resolve(message); else this.messages.push(message);
        } catch (error) {
          this.error = error;
          for (const waiter of this.waiters.splice(0)) waiter.reject(error);
        }
      });
    }

    next(milliseconds) {
      if (this.error) return Promise.reject(this.error);
      if (this.messages.length) return Promise.resolve(this.messages.shift());
      return withDeadline("SIP message", milliseconds, () => new Promise((resolve, reject) => {
        this.waiters.push({resolve, reject});
      }));
    }
  }

  function waitIceGathering(pc, milliseconds) {
    if (pc.iceGatheringState === "complete") return Promise.resolve();
    return waitEvent(pc, "icegatheringstatechange", () => pc.iceGatheringState === "complete", milliseconds);
  }

  function waitConnected(pc, milliseconds) {
    if (["connected", "completed"].includes(pc.iceConnectionState)) return Promise.resolve();
    return withDeadline("ICE connection", milliseconds, () => new Promise((resolve, reject) => {
      const listener = () => {
        if (["connected", "completed"].includes(pc.iceConnectionState)) {
          pc.removeEventListener("iceconnectionstatechange", listener);
          resolve();
        } else if (["failed", "closed"].includes(pc.iceConnectionState)) {
          pc.removeEventListener("iceconnectionstatechange", listener);
          reject(new Error(`ICE became ${pc.iceConnectionState}`));
        }
      };
      pc.addEventListener("iceconnectionstatechange", listener);
    }));
  }

  function setupFrom(sdp) {
    return (sdp.match(/^a=setup:(active|passive|actpass|holdconn)$/m) || [])[1] || "";
  }

  async function digest(text) {
    const value = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
    return [...new Uint8Array(value)].map((byte) => byte.toString(16).padStart(2, "0")).join("");
  }

  async function sdpEvidence(local, remote) {
    const describe = async (sdp) => {
      const lines = sdp.split(/\r?\n/).filter(Boolean);
      const media = lines.find((line) => line.startsWith("m=audio ")) || "";
      const candidates = lines.filter((line) => line.startsWith("a=candidate:"));
      return {
        raw: sdp,
        sha256: await digest(sdp),
        bytes: new TextEncoder().encode(sdp).length,
        audio_protocol: media.split(/\s+/)[2] || "",
        payloads: media.split(/\s+/).slice(3),
        mappings: lines.filter((line) => line.startsWith("a=rtpmap:")),
        candidate_components: [...new Set(candidates.map((line) => line.split(/\s+/)[1]))],
        ice_options: lines.filter((line) => line.startsWith("a=ice-options:")),
        rtcp_mux: lines.includes("a=rtcp-mux"),
        rtcp_fallback: lines.find((line) => line.startsWith("a=rtcp:")) || "",
        setup: setupFrom(sdp),
        fingerprint: lines.find((line) => line.startsWith("a=fingerprint:")) || "",
      };
    };
    return {local: await describe(local), remote: await describe(remote)};
  }

  function buildInvite(config, dialog, sdp) {
    return sipMessage(`INVITE ${config.requestUri} SIP/2.0`, [
      `Via: SIP/2.0/WSS browser.invalid;branch=z9hG4bK${dialog.branch};rport`,
      `Max-Forwards: 70`,
      `To: <${config.requestUri}>`,
      `From: <${config.fromUri}>;tag=${dialog.localTag}`,
      `Call-ID: ${dialog.callId}`,
      `CSeq: 1 INVITE`,
      `Contact: <${config.contactUri}>`,
      `Content-Type: application/sdp`,
    ], sdp);
  }

  function buildResponse(invite, dialog, sdp) {
    const to = header(invite, "to");
    return sipMessage("SIP/2.0 200 OK", [
      `Via: ${header(invite, "via")}`,
      `To: ${to.includes(";tag=") ? to : `${to};tag=${dialog.localTag}`}`,
      `From: ${header(invite, "from")}`,
      `Call-ID: ${header(invite, "call-id")}`,
      `CSeq: ${header(invite, "cseq")}`,
      `Contact: <${dialog.contactUri}>`,
      `Content-Type: application/sdp`,
    ], sdp);
  }

  function buildEmptyResponse(request) {
    return sipMessage("SIP/2.0 200 OK", [
      `Via: ${header(request, "via")}`,
      `To: ${header(request, "to")}`,
      `From: ${header(request, "from")}`,
      `Call-ID: ${header(request, "call-id")}`,
      `CSeq: ${header(request, "cseq")}`,
    ]);
  }

  function buildReadiness(config, dialog) {
    return sipMessage(`OPTIONS ${config.requestUri} SIP/2.0`, [
      `Via: SIP/2.0/WSS browser.invalid;branch=z9hG4bK${dialog.branch};rport`,
      "Max-Forwards: 70",
      `To: <${config.requestUri}>`,
      `From: <${config.fromUri}>;tag=${dialog.localTag}`,
      `Call-ID: ready-${dialog.callId}`,
      "CSeq: 1 OPTIONS",
      `Contact: <${config.contactUri}>`,
    ]);
  }

  function mutateSdp(sdp, mutation) {
    if (!mutation) return sdp;
    if (mutation === "UnusedRtcpCandidate") {
      const candidate = sdp.split(/\r?\n/).find((line) => /^a=candidate:\S+ 1 /i.test(line));
      if (!candidate) fail("browser SDP had no component-one candidate to extend");
      const fields = candidate.split(/\s+/);
      const priority = Number.parseInt(fields[3], 10);
      const port = Number.parseInt(fields[5], 10);
      if (!Number.isSafeInteger(priority) || priority <= 1 || !Number.isSafeInteger(port)) {
        fail("browser component-one candidate cannot form a bounded fallback");
      }
      fields[1] = "2";
      fields[3] = String(priority - 1);
      fields[5] = String(port === 65535 ? port - 1 : port + 1);
      return sdp.replace(candidate, `${candidate}\r\n${fields.join(" ")}`);
    }
    if (mutation === "FingerprintMismatch") {
      const mutated = sdp.replace(/^(a=fingerprint:[^\r\n]*:)([0-9A-F]{2})$/mi, (_line, prefix, octet) => {
        return `${prefix}${octet === "00" ? "01" : "00"}`;
      });
      if (mutated === sdp) fail("browser SDP had no fingerprint to mutate");
      return mutated;
    }
    if (mutation === "NoNominatedPair") {
      return sdp;
    }
    if (mutation === "WeakerMedia") {
      return sdp
        .replace("UDP/TLS/RTP/SAVPF", "RTP/AVP")
        .split(/(?<=\r\n)/)
        .filter((line) => !/^a=(fingerprint|setup|ice-ufrag|ice-pwd|candidate|rtcp-mux):?/.test(line))
        .join("");
    }
    fail(`unknown SDP mutation ${mutation}`);
  }

  function buildDialogRequest(method, config, dialog, cseq) {
    return sipMessage(`${method} ${config.requestUri} SIP/2.0`, [
      `Via: SIP/2.0/WSS browser.invalid;branch=z9hG4bK${randomToken()};rport`,
      `Max-Forwards: 70`,
      `To: ${dialog.remoteTo}`,
      `From: ${dialog.localFrom}`,
      `Call-ID: ${dialog.callId}`,
      `CSeq: ${cseq} ${method}`,
      `Contact: <${config.contactUri}>`,
    ]);
  }

  async function selectedStats(pc) {
    const report = await pc.getStats();
    const entries = new Map();
    report.forEach((value) => entries.set(value.id, value));
    const transport = [...entries.values()].find((value) => value.type === "transport");
    const pair = transport && entries.get(transport.selectedCandidatePairId)
      || [...entries.values()].find((value) => value.type === "candidate-pair" && value.nominated && value.state === "succeeded");
    if (!transport || !pair) return null;
    const local = entries.get(pair.localCandidateId);
    const remote = entries.get(pair.remoteCandidateId);
    if (!local || !remote) return null;
    const inbound = [...entries.values()].find((value) => value.type === "inbound-rtp" && value.kind === "audio");
    const outbound = [...entries.values()].find((value) => value.type === "outbound-rtp" && value.kind === "audio");
    if (!inbound || !outbound) return {transport, pair, local, remote, inbound: null, outbound: null, codec: null};
    const codec = entries.get(inbound.codecId) || entries.get(outbound.codecId);
    if (!codec) fail("statistics expose no selected codec");
    return {transport, pair, local, remote, inbound, outbound, codec};
  }

  async function run(config) {
    if (!["browser-offerer", "browser-answerer"].includes(config.role)) fail("unknown browser role");
    const status = document.getElementById("status");
    status.textContent = `running ${config.role}`;
    const events = [];
    const observe = (name) => events.push(name);
    const context = new AudioContext({sampleRate: 48000});
    await context.resume();
    const oscillator = new OscillatorNode(context, {frequency: config.toneHz || 697});
    const gain = new GainNode(context, {gain: 0.15});
    const destination = new MediaStreamAudioDestinationNode(context);
    oscillator.connect(gain).connect(destination);
    oscillator.start();

    const pc = new RTCPeerConnection({iceServers: config.iceServers || [], rtcpMuxPolicy: "require"});
    proofState = {pc, events, config};
    pc.addTrack(destination.stream.getAudioTracks()[0], destination.stream);
    pc.addEventListener("track", (event) => {
      const remote = document.getElementById("remote");
      remote.srcObject = event.streams[0] || new MediaStream([event.track]);
      void remote.play();
    });

    const socket = new WebSocket(config.wssUrl, "sip");
    await waitEvent(socket, "open", null, config.signallingTimeoutMs || 10000);
    if (socket.protocol !== "sip") fail(`WSS selected ${socket.protocol || "no subprotocol"}, not sip`);
    const queue = new MessageQueue(socket);
    const dialog = {
      branch: randomToken(), localTag: randomToken(), callId: `${randomToken()}@browser.invalid`,
      contactUri: config.contactUri,
    };
    let localSdp;
    let remoteSdp;

    if (config.role === "browser-offerer") {
      await pc.setLocalDescription(await pc.createOffer());
      await waitIceGathering(pc, config.iceTimeoutMs || 15000);
      localSdp = mutateSdp(pc.localDescription.sdp, config.mutation);
      socket.send(buildInvite(config, dialog, localSdp));
      observe("invite");
      let response = await queue.next(config.signallingTimeoutMs || 10000);
      while (!response.start.startsWith("SIP/2.0 2")) response = await queue.next(config.signallingTimeoutMs || 10000);
      observe("final");
      remoteSdp = response.body;
      dialog.remoteTo = header(response, "to");
      dialog.localFrom = header(response, "from");
      dialog.callId = header(response, "call-id");
      await pc.setRemoteDescription({type: "answer", sdp: remoteSdp});
      socket.send(buildDialogRequest("ACK", config, dialog, 1));
      observe("ack");
    } else {
      socket.send(buildReadiness(config, dialog));
      const ready = await queue.next(config.signallingTimeoutMs || 10000);
      if (!ready.start.startsWith("SIP/2.0 2")) fail(`OPTIONS readiness got ${ready.start}`);
      const invite = await queue.next(config.signallingTimeoutMs || 10000);
      if (!invite.start.startsWith("INVITE ")) fail(`expected INVITE, got ${invite.start}`);
      observe("invite");
      remoteSdp = invite.body;
      await pc.setRemoteDescription({type: "offer", sdp: remoteSdp});
      const answer = await pc.createAnswer();
      if (config.mutation === "WeakerMedia") {
        // Deliberately do not apply the weaker answer locally: the refusal must precede browser
        // ICE/DTLS, and createAnswer supplies native browser SDP without starting those layers.
        localSdp = mutateSdp(answer.sdp, config.mutation);
      } else {
        await pc.setLocalDescription(answer);
        await waitIceGathering(pc, config.iceTimeoutMs || 15000);
        localSdp = mutateSdp(pc.localDescription.sdp, config.mutation);
      }
      dialog.callId = header(invite, "call-id");
      dialog.localFrom = `${header(invite, "to")};tag=${dialog.localTag}`;
      dialog.remoteTo = header(invite, "from");
      socket.send(buildResponse(invite, dialog, localSdp));
      observe("final");
      const ack = await queue.next(config.signallingTimeoutMs || 10000);
      if (!ack.start.startsWith("ACK ")) fail(`expected ACK, got ${ack.start}`);
      observe("ack");
      if (config.mutation === "NoNominatedPair") {
        pc.close();
        fail("browser closed the complete answer before any ICE pair could be nominated");
      }
      if (config.mutation === "WeakerMedia") {
        const bye = await queue.next(config.signallingTimeoutMs || 10000);
        if (!bye.start.startsWith("BYE ")) fail(`expected teardown BYE, got ${bye.start}`);
        observe("bye");
        socket.send(buildEmptyResponse(bye));
        observe("bye-final");
        fail("weaker media answer was refused before local ICE/DTLS");
      }
    }

    await waitConnected(pc, config.iceTimeoutMs || 15000);
    if (config.mutation === "FingerprintMismatch") {
      proofState.negativeFacts = await withDeadline("selected pair evidence", 1000, async () => {
        while (true) {
          const selected = await selectedStats(pc);
          if (selected) {
            return {
              selected_pair: true,
              nominated: selected.pair.nominated === true,
              dtls_state: selected.transport.dtlsState || "not-started",
            };
          }
          await waitEvent(pc, "connectionstatechange", null, 50).catch(() => undefined);
        }
      });
    }
    const stats = await withDeadline("two-way RTP evidence", config.mediaTimeoutMs || 20000, async () => {
      while (true) {
        const current = await selectedStats(pc);
        if (!current || !current.inbound || !current.outbound) {
          await waitEvent(pc, "connectionstatechange", null, 250).catch(() => undefined);
          continue;
        }
        const energy = current.inbound.totalAudioEnergy || current.inbound.audioLevel || 0;
        if (current.inbound.packetsReceived > 0 && current.outbound.packetsSent > 0 && energy > 0) return current;
        // Sampling cadence: getStats(), not this duration, decides whether media evidence exists.
        await waitEvent(pc, "connectionstatechange", null, 250).catch(() => undefined);
      }
    });

    if (config.role === "browser-offerer") {
      socket.send(buildDialogRequest("BYE", config, dialog, 2));
      observe("bye");
      let byeResponse = await queue.next(config.signallingTimeoutMs || 10000);
      while (!byeResponse.start.startsWith("SIP/2.0 2")) {
        byeResponse = await queue.next(config.signallingTimeoutMs || 10000);
      }
      observe("bye-final");
    } else {
      const bye = await queue.next(config.signallingTimeoutMs || 10000);
      if (!bye.start.startsWith("BYE ")) fail(`expected BYE, got ${bye.start}`);
      observe("bye");
      socket.send(buildEmptyResponse(bye));
      observe("bye-final");
    }
    const answerSetup = setupFrom(config.role === "browser-offerer" ? remoteSdp : localSdp);
    const evidence = {
      contract: CONTRACT,
      type: "proof.result",
      role: config.role,
      codec: {
        mime_type: stats.codec.mimeType,
        payload_type: stats.codec.payloadType,
        clock_rate: stats.codec.clockRate,
      },
      security: {
        wss_spki_sha256: config.wssSpkiSha256,
        dtls_state: stats.transport.dtlsState,
        setup_role: answerSetup,
        dtls_cipher: stats.transport.dtlsCipher || "",
        srtp_profile: stats.transport.srtpCipher || "",
      },
      candidate_pair: {
        id: stats.pair.id,
        selected: stats.transport.selectedCandidatePairId === stats.pair.id,
        nominated: stats.pair.nominated === true,
        state: stats.pair.state,
        component: 1,
        local: {candidate_type: stats.local.candidateType, address: stats.local.address, port: stats.local.port},
        remote: {candidate_type: stats.remote.candidateType, address: stats.remote.address, port: stats.remote.port},
      },
      media: {
        inbound_packets: stats.inbound.packetsReceived,
        outbound_packets: stats.outbound.packetsSent,
        inbound_bytes: stats.inbound.bytesReceived,
        outbound_bytes: stats.outbound.bytesSent,
        received_audio_energy: stats.inbound.totalAudioEnergy || stats.inbound.audioLevel || 0,
        oscillator_frames: stats.outbound.totalSamplesSent || stats.outbound.packetsSent,
      },
      sip: {order: events},
      sdp: await sdpEvidence(localSdp, remoteSdp),
    };
    oscillator.stop();
    pc.close();
    socket.close();
    await context.close();
    status.textContent = "proof complete";
    return evidence;
  }

  async function failure(config, error) {
    const pc = proofState && proofState.pc;
    const values = [];
    if (pc) {
      try {
        (await pc.getStats()).forEach((value) => values.push(value));
      } catch (_statsError) {
        // A deliberately closed peer connection can make its final stats unavailable. The
        // independently observed terminal states below still fail closed in the validator.
      }
    }
    const transport = values.find((value) => value.type === "transport");
    const selectedId = transport && transport.selectedCandidatePairId;
    const pair = values.find((value) => value.id === selectedId)
      || values.find((value) => value.type === "candidate-pair" && value.nominated);
    const inbound = values.filter((value) => value.type === "inbound-rtp")
      .reduce((sum, value) => sum + (value.packetsReceived || 0), 0);
    const outbound = values.filter((value) => value.type === "outbound-rtp")
      .reduce((sum, value) => sum + (value.packetsSent || 0), 0);
    const negativeFacts = proofState && proofState.negativeFacts || {};
    return {
      contract: CONTRACT,
      type: "proof.negative-browser",
      role: config.role,
      mutation: config.mutation,
      error: String(error && error.stack || error),
      facts: {
        ice_started: Boolean(pc && (pc.iceGatheringState !== "new" || pc.iceConnectionState !== "new")),
        ice_state: pc ? pc.iceConnectionState : "not-started",
        selected_pair: negativeFacts.selected_pair || Boolean(selectedId),
        nominated: negativeFacts.nominated || Boolean(pair && pair.nominated),
        dtls_state: negativeFacts.dtls_state || transport && transport.dtlsState || "not-started",
        rtp_packets: inbound,
        outbound_rtp_attempts: outbound,
        fallback_attempted: false,
      },
      sip: {order: proofState ? proofState.events : []},
    };
  }

  window.sipxBrowserAudio = {run, failure};
})();
