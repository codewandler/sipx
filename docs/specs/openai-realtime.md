# Spec: The OpenAI realtime bridge

**Status:** normative · **Epic:** `openai` · **Design:** [openai](../designs/openai.md) ·
**Stories:** A-19 (this spec) · A-20 (WSS client) · A-21 (stand-in peer) · A-22 (bridge) ·
A-23 (live proof)

> One sipx call leg, one realtime session, one WebSocket between them. The bridge is byte
> passthrough: the call's negotiated G.711 payload travels up as base64 inside JSON events and
> the agent's G.711 comes back the same way. This spec is the contract both sides of that seam
> are held to — the bridge (A-22) from ours, the stand-in peer (A-21) from the vendor's.

## 1. Stance and observation record

- **This spec is normative for this workspace.** The WSS client (A-20), the stand-in peer
  (A-21) and the bridge (A-22) are each held to its vectors independently, in the default CI
  matrix, with no credentials.
- **Toward the vendor it is observational.** The endpoint URL, event names, session fields and
  size limits in this document were read from OpenAI's published Realtime API documentation —
  the platform guides and API reference (`platform.openai.com/docs`, served from
  `developers.openai.com/api/docs` at observation time) and the vendor's published
  machine-readable event schemas (`github.com/openai/openai-python`,
  `src/openai/types/realtime`, `main`) — **observed 2026-08-05**. OpenAI is named here as the
  interop subject, a checkable fact like a comparison subject; every design rule in this spec
  cites RFCs or our own specs.
- **Drift is a spec update, not a silent fix.** The live proof (A-23) is the drift detector:
  when it disagrees with this document, the document changes with a new observation date and
  the stand-in peer follows it in the same story.

Normative references for our own rules: RFC 6455 (WebSocket), RFC 4648 §4 (base64), RFC 3551
§4.5.14 (PCMU/PCMA: 8000 Hz, one byte per sample), ITU-T G.711 (μ-law/A-law),
[sip-tls.md](sip-tls.md) §3 (certificate policy), [host-config.md](host-config.md) N7
(secrets by name), [session-binding.md](session-binding.md) §3 (bounded work, counted loss),
[media-runtime.md](media-runtime.md) (the media clock).

## 2. Endpoint and authentication

- **[sipx-app]** The bridge connects to
  `wss://api.openai.com/v1/realtime?model=<model>`. The model is selected by the `model` query
  parameter and nowhere else; the documented example at observation time is
  `gpt-realtime-2.1`. Both the base URL and the model are host configuration — the stand-in
  peer is reached by configuring its URL, not by a test hook in the bridge.
- **[sipx-app]** The upgrade request carries `Authorization: Bearer <key>`. No other
  authentication header is sent; in particular the retired beta header
  (`OpenAI-Beta: realtime=v1`) is **not** sent — the vendor's GA documentation says to remove
  it.
- **[sipx-app]** The key is resolved from a **named** secret per
  [host-config.md](host-config.md) N7. Configuration carries the name form only:

  ```toml
  api_key_secret = "openai-api-key"
  ```

  The name obeys §4.4's grammar (`[a-z][a-z0-9._-]{0,63}`), and the `sipx-host` process
  resolves it from `SIPX_SECRET_openai-api-key` at startup, before any call is admitted —
  the [webhook-binding.md](webhook-binding.md) §3 discipline exactly. The secret **value**
  never appears in configuration, logs, error messages or typed outcomes; a refused
  credential is reported by the secret's *name* (ORB-10).
- **[sipx-app]** TLS on the connection follows [sip-tls.md](sip-tls.md) §3 unchanged: one
  certificate discipline, not two. The WSS client (A-20) composes the RFC 6455 handshake over
  the same `ClientTls`.

## 3. Session configuration

- **[sipx-app]** After the server's `session.created` (§5.2), the bridge sends exactly one
  `session.update` pinning the session to the call's negotiated wire format. The call's codec
  is already fixed by the SDP answer when the bridge attaches; the bridge maps it:

  | Negotiated payload | Session audio format (`format.type`) |
  |---|---|
  | PCMU (payload type 0) | `audio/pcmu` |
  | PCMA (payload type 8) | `audio/pcma` |

  The **same** format is set for input and output — the bridge is byte passthrough via the
  call's relay mode (`recv_encoded`/`send_encoded`); it never transcodes and never resamples.
  A call whose negotiated codec is neither PCMU nor PCMA is not bridgeable and the bridge
  ends with `NotBridgeable` before any socket is opened (§6).
- **[sipx-app]** Turn detection is server-side voice activity detection with response
  creation on and **response interruption off**:

  ```json
  {
    "type": "session.update",
    "session": {
      "type": "realtime",
      "output_modalities": ["audio"],
      "instructions": "<configured instructions>",
      "audio": {
        "input": {
          "format": { "type": "audio/pcmu" },
          "turn_detection": {
            "type": "server_vad",
            "create_response": true,
            "interrupt_response": false
          }
        },
        "output": {
          "format": { "type": "audio/pcmu" }
        }
      }
    }
  }
  ```

  `interrupt_response: false` is deliberate: cancellation has exactly one owner, the bridge's
  barge-in rule (§4), so the tests assert one causal chain rather than a race between two
  cancellers. `instructions` is host configuration; an optional configured voice may be added
  under `audio.output` and is not otherwise part of this contract.
- **[sipx-app]** Because turn detection is server-side, the bridge never commits or clears
  the input buffer: `input_audio_buffer.commit` and `input_audio_buffer.clear` are **not** in
  the client subset (§5.1). The server segments speech itself.
- **[sipx-app]** The server acknowledges with `session.updated`. Until it arrives the
  bridge writes no audio to the socket: uplink frames arriving in the window are admitted
  to the uplink queue (§5.4) and drain in order once `session.updated` lands, so a slow
  acknowledgement costs at most the queue's 640 ms of buffered audio plus counted overflow,
  never a protocol violation. `session.created` must arrive within **10 s** of the
  completed upgrade and `session.updated` within **10 s** of sending `session.update`;
  either missed bound fails setup typed (`SetupTimeout`, §6).

## 4. Audio framing and barge-in

### 4.1 Frames

- **[sipx-app]** The unit of audio is the call's 20 ms packet: **160 bytes** of G.711 at
  8000 Hz, one byte per sample (RFC 3551 §4.5.14). Uplink, each payload received from
  `recv_encoded` becomes exactly one `input_audio_buffer.append` whose `audio` member is the
  RFC 4648 §4 base64 of those bytes — 160 bytes encode to 216 base64 characters. The bridge
  never batches, splits or re-times uplink audio.
- **[sipx-app]** Downlink, `response.output_audio.delta` carries base64 audio in its `delta`
  member. A delta's decoded length is **not** guaranteed to be a multiple of 160: the bridge
  accumulates decoded bytes and slices them into 160-byte frames for `send_encoded`. A
  partial tail at response end (`response.output_audio.done`) is padded to a full frame with
  the format's silence byte — `0xFF` for μ-law, `0xD5` for A-law — so every frame handed to
  the media path is byte-exact and full-length.
- The vendor caps one `input_audio_buffer.append` at 15 MiB (observed). Our appends are 216
  base64 characters; the cap is unreachable from this spec and recorded only so a future
  batching story knows it exists.

### 4.2 Byte-level vectors

Frame **F-silence**: 160 bytes of `0xFF` (20 ms of μ-law digital silence). Its base64 is the
216-character string

```
/////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////////w==
```

Frame **F-ramp**: the 160 bytes `0x00, 0x01, … 0x9F`. Its base64 is

```
AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8gISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0BBQkNERUZHSElKS0xNTk9QUVJTVFVWV1hZWltcXV5fYGFiY2RlZmdoaWprbG1ub3BxcnN0dXZ3eHl6e3x9fn+AgYKDhIWGh4iJiouMjY6PkJGSk5SVlpeYmZqbnJ2enw==
```

These two frames are the literals behind ORB-3 and ORB-4: the expected base64 is a test literal,
never a value the assertion computes (the [webhook-binding.md](webhook-binding.md) WB-8
discipline).

### 4.3 Barge-in

- **[sipx-app]** On `input_audio_buffer.speech_started` while agent audio is in flight, the
  bridge, in order and without waiting on anything:
  1. sends `response.cancel` (no `response_id`: the in-progress response is the target);
  2. atomically empties the downlink queue **and** the re-framing accumulator (§4.1),
     counting every dropped **frame** in `bridge_barge_in_flushed`; the accumulator's
     sub-frame residue (< 160 bytes) is discarded with the flush and counts nothing — it
     never became a frame, and the `response.output_audio.done` padding rule does not apply
     to audio being thrown away;
  3. drops every further `response.output_audio.delta` until the next `response.done`,
     counting each dropped **event** in `bridge_cancelled_deltas`.
- **[sipx-app]** Two counters, because only one of them has a bound. `bridge_barge_in_flushed`
  counts frames that were in the queue, so per barge-in it is **≤ 2048**, the queue bound
  (§5.4) — that and the residual below are the numbers A-22's test asserts.
  `bridge_cancelled_deltas` counts events the peer chose to send after the cancel; how many
  arrive before `response.done` is the peer's, so this spec claims no bound on it — it is
  observability, not a limit.
- **[sipx-app]** The residual bound: after step 2, at most **one** frame of agent audio is
  still ahead of the flush locally — the frame already committed to the media path — so the
  locally-sourced residual is **≤ 20 ms**. (What the bound cannot cover, the design records
  as a risk: audio the far end of the *call* has already been sent is beyond recall.)
- **[sipx-app]** `speech_started` with no response in flight performs steps 2–3 vacuously
  and sends nothing: `response.cancel` is only sent when a response is in flight.
- **[sipx-app]** The cancel/done race is normal, not a failure: the response may complete
  (`response.done`, any `status`) before the cancel lands, and the server may then report the
  cancel as an error. An `error` event received between sending `response.cancel` and the
  next `response.done`-or-10-seconds is classified as the race, ignored, and counted in
  `bridge_cancel_race`; the session stays live. Every other `error` event is session-fatal
  (§6). The 10-second window bounds a failure (a peer that never sends `response.done`), it
  does not order anything.
- Non-goal, recorded: the vendor recommends `conversation.item.truncate` after an
  interruption so the model's context matches what the caller actually heard. This epic does
  not send it; after a barge-in the far end may believe more was heard than was. A later
  story that adds it extends §5.1 and this paragraph.

## 5. The event subset

The subset is exhaustive **for the bridge**: every event the bridge may send and every event
it consumes is named below with a vector. Everything else has one defined disposition each
(§5.3). Nothing is "whatever the implementation does".

Unknown *members* inside a known event are ignored everywhere — the vendor adds fields; the
bridge reads only the members this section names.

### 5.1 Client events (the bridge sends exactly these three)

**`session.update`** — once, after `session.created` (full vector in §3):

```json
{"type": "session.update", "session": {"type": "realtime", "…": "…"}}
```

**`input_audio_buffer.append`** — one per uplink frame:

```json
{"type": "input_audio_buffer.append", "audio": "<216 base64 chars>"}
```

**`response.cancel`** — on barge-in only:

```json
{"type": "response.cancel"}
```

`event_id` is optional on all client events and the bridge does not send it. The bridge sends
no other client event; a code path that would emit one is a defect against ORB-5.

### 5.2 Server events (the bridge consumes exactly these seven)

| Event | Members the bridge reads | Bridge action |
|---|---|---|
| `session.created` | `type` | ready: send `session.update` |
| `session.updated` | `type` | configured: start uplink audio |
| `input_audio_buffer.speech_started` | `type` | barge-in (§4.3) |
| `response.output_audio.delta` | `type`, `response_id`, `delta` | decode, slice, enqueue downlink |
| `response.output_audio.done` | `type`, `response_id` | flush the partial-frame tail (§4.1) |
| `response.done` | `type` | close the response; end any cancel-race window — whatever `response.status` says |
| `error` | `type`, `error.type`, `error.code`, `error.message`, `error.param` | cancel-race → count; otherwise session-fatal (§6) |

JSON vectors, one per event, members beyond the read set elided as `…` (the peer sends them;
the bridge must not require them):

```json
{"type": "session.created", "event_id": "event_001", "session": {"…": "…"}}
{"type": "session.updated", "event_id": "event_002", "session": {"…": "…"}}
{"type": "input_audio_buffer.speech_started", "event_id": "event_003", "audio_start_ms": 460, "item_id": "item_001"}
{"type": "response.output_audio.delta", "event_id": "event_004", "response_id": "resp_001", "item_id": "item_002", "output_index": 0, "content_index": 0, "delta": "<base64>"}
{"type": "response.output_audio.done", "event_id": "event_005", "response_id": "resp_001", "item_id": "item_002", "output_index": 0, "content_index": 0}
{"type": "response.done", "event_id": "event_006", "response": {"id": "resp_001", "status": "completed", "…": "…"}}
{"type": "error", "event_id": "event_007", "error": {"type": "invalid_request_error", "code": "…", "message": "…", "param": null}}
```

### 5.3 Everything else

- **[sipx-app]** A text frame that parses as JSON with a string `type` **outside** §5.2 —
  the vendor emits many (`conversation.*`, `input_audio_buffer.speech_stopped`,
  `input_audio_buffer.committed`, `rate_limits.updated`, `response.created`,
  `response.output_audio_transcript.delta`, …) — is **ignored with a counter**:
  `bridge_ignored_events` increments, the session stays live. This is what keeps a vendor
  *addition* from being an outage.
- **[sipx-app]** A text frame that does not parse as JSON, or parses without a string
  `type`, is **session-fatal**: `MalformedEvent` (§6). A binary frame is the same outcome —
  every event in this contract is a JSON text frame.
- **[sipx-app]** Exhaustiveness covers the members too: a §5.2 event that arrives without a
  read member, or with one the bridge cannot interpret, is the same `MalformedEvent`.
  Concretely: a `response.output_audio.delta` whose `delta` is absent, not a string, or not
  valid RFC 4648 §4 base64; a `response.output_audio.delta` or `response.output_audio.done`
  without a string `response_id`. One stated exception: an `error` event is consumed
  whatever its members — a missing or malformed `error` object changes nothing about its
  disposition (§4.3's race rule or `SessionError`), it only leaves the outcome without a
  code. The remaining §5.2 events read only `type`, so they have no invalid-member case.
- **[sipx-app]** An inbound WebSocket message larger than **1 MiB** is session-fatal:
  `OversizeFrame` (§6). The bound is enforced by the WSS client (A-20) before any JSON
  parsing, so an oversize frame cannot cost an allocation proportional to the peer's claim.

### 5.4 Buffering and backpressure

Both queues carry the bounded-work discipline of [session-binding.md](session-binding.md)
§3 — bounded, non-blocking admission, never a blocking send. The full-queue policy is this
spec's own, not that one's: a full control queue there goes dead or refuses (`1013`,
`call_busy`) because control loss is corruption; a media stream tolerates loss, so here a
full queue **drops the offered frame, counts it, and the session stays live**.

| Queue | Direction | Bound | Full ⇒ | Counter |
|---|---|---|---|---|
| uplink | call → session | **32 frames** (640 ms) | drop the offered frame, session lives | `bridge_uplink_dropped` |
| downlink | session → call | **2048 frames** (40.96 s) | drop the offered frame, session lives | `bridge_downlink_dropped` |

- The uplink queue absorbs write jitter only — the call side produces one frame per 20 ms,
  so sustained fullness means the socket has stalled and liveness (§6) will end the bridge;
  the queue never masks a dead peer.
- The downlink queue is sized for the vendor's shape: a response's audio arrives as a burst
  far ahead of real time while the media path drains at the RTP clock. 2048 frames is
  320 KiB — an agent turn longer than 40.96 s loses its tail, counted.
- **No rule in this spec requires a fixed wall-clock wait to stand in for a happens-before**
  ([media.md](../designs/media.md) normative rule). Ordering is always by event
  (`session.created` → `session.update` → `session.updated` → audio; `speech_started` →
  cancel → flush). The only durations in this contract bound failures (§6) or belong to the
  media clock, which paces frames and is [media-runtime.md](media-runtime.md)'s, not the
  bridge's.

## 6. Connection lifecycle and failure taxonomy

- **[sipx-app]** The bridge owns exactly one socket for exactly one call leg. When the
  socket closes or fails — any close code, clean or not — the bridge **ends with a typed
  outcome. There is no reconnect**, silent or otherwise: a reconnect would invent
  conversation state the far end no longer has. What follows a dead bridge is the
  application's declared failure policy, the same place a dead app session lands
  ([session-binding.md](session-binding.md) §3).
- **[sipx-app]** Liveness is RFC 6455 Ping/Pong, the session-binding numbers: a Ping every
  30 s, dead if no Pong within 10 s. Timers bound failure; they never order.
- **[sipx-app]** When the call ends first, the bridge sends a normal close (1000) and ends
  `CallEnded`. When the host shuts down, tasks are cancelled and joined per the host's
  ownership rule; the outcome is `Cancelled`.

Every way the bridge ends, with its trigger and bound:

| Outcome | Trigger | Bound |
|---|---|---|
| `CallEnded` | the call leg ended | — (normal) |
| `NotBridgeable` | negotiated codec is neither PCMU nor PCMA | before connecting |
| `AuthRefused` | the upgrade is refused (observed: HTTP 4xx before 101) | one attempt; reports the secret **name** only |
| `SetupTimeout` | no `session.created`, or no `session.updated` after ours | 10 s from the completed upgrade (the 101); 10 s from sending `session.update` |
| `PeerClosed` | server close frame or EOF, carrying the close code if any | — |
| `PeerStalled` | no Pong within grace | 30 s + 10 s |
| `MalformedEvent` | §5.3: unparseable text, `type` missing, a binary frame, or a §5.2 event failing its read set | first occurrence |
| `OversizeFrame` | §5.3: inbound message > 1 MiB | first occurrence |
| `SessionError` | an `error` event outside the cancel-race window, carrying `error.code` | first occurrence |
| `Cancelled` | host shutdown | join, no orphan tasks |

No outcome, log line or error string carries the bearer value or any fragment of the
`Authorization` header.

## 7. Vectors

Machine-consumable the way [webhook-binding.md](webhook-binding.md) WB-1…WB-9 are: the test
names are the vector identifiers, and each vector names the story that owns its enforcement.
A-21's peer supplies every scripted behaviour (including the negative modes) so every vector
except ORB-17 runs in the default CI matrix with no credentials; A-23 alone (ORB-17) touches
the network.

| ID | Script | Required result | Owner |
|---|---|---|---|
| ORB-1 | connect with secret name `openai-api-key` resolved to a known value | upgrade to the configured URL with `?model=`, `Authorization: Bearer` carries the resolved bytes, no `OpenAI-Beta` header; peer's `session.created` yields a live session | A-20 |
| ORB-2 | peer sends `session.created` | bridge sends exactly one `session.update`, byte-shape of §3, formats matching the negotiated codec; audio starts only after `session.updated` | A-22 |
| ORB-3 | uplink frame F-silence from `recv_encoded` | one `input_audio_buffer.append`, `audio` equal to F-silence's 216-char literal (§4.2) | A-22 |
| ORB-4 | peer sends one delta with F-ramp's base64 | `send_encoded` receives exactly the bytes `0x00…0x9F` as one 160-byte frame | A-22 |
| ORB-5 | a full scripted conversation | the peer observes **only** the three client events of §5.1 | A-21 |
| ORB-6 | delta of 400 bytes, then `response.output_audio.done` | two 160-byte frames, then one frame of 80 audio bytes padded with 80 silence bytes (`0xFF` μ-law / `0xD5` A-law) | A-22 |
| ORB-7 | A-law call | `session.update` says `audio/pcma` both directions; ORB-3/ORB-4/ORB-6 shapes unchanged | A-22 |
| ORB-8 | 16 frames queued downlink plus 80 bytes in the accumulator, response in flight; peer sends `speech_started`, then two more deltas, then `response.done` | `response.cancel` sent; queue and accumulator empty; `bridge_barge_in_flushed` = 16 (the residue counts nothing); `bridge_cancelled_deltas` = 2; ≤ 1 further frame reaches the media path | A-22 |
| ORB-9 | `response.done` then a scripted `error` inside the race window after a cancel | `bridge_cancel_race` = 1, session lives; the same `error` outside the window ends the bridge `SessionError` | A-22 |
| ORB-10 | peer refuses the upgrade (401) | `AuthRefused`; no session, no audio ever; the outcome names `openai-api-key` and provably not the value | A-22 |
| ORB-11 | peer sends a 2 MiB text frame | `OversizeFrame` before JSON parsing | A-20 |
| ORB-12 | peer sends `rate_limits.updated` and one unknown future event mid-call | both ignored, `bridge_ignored_events` = 2, audio unaffected | A-22 |
| ORB-13 | peer sends `not json{` / a frame without `type` / a binary frame | each is `MalformedEvent`, session over on first occurrence | A-22 |
| ORB-14 | peer answers the upgrade then goes silent | `PeerStalled` within 30 s + 10 s; nothing waits on it but the liveness timer | A-20 |
| ORB-15 | peer never sends `session.created`; separately, never `session.updated` | `SetupTimeout` at 10 s in each case | A-22 |
| ORB-16 | peer closes 1000 mid-call; separately, TCP reset | `PeerClosed` with the code / without; the peer observes **no second upgrade attempt** | A-22 |
| ORB-17 | one real call against the live endpoint | the session establishes per ORB-1/ORB-2, the agent's reply is non-silent, and every fact this spec observes about the vendor (URL, headers, event names, formats) held; evidence recorded in A-23's Progress | A-23 |
| ORB-18 | peer sends a delta whose `delta` is `not base64!!`; separately a delta with no `delta` member; separately a `response.output_audio.done` with no `response_id` | each is `MalformedEvent` on its first occurrence (§5.3's read-set rule) | A-22 |

ORB-5's exhaustiveness and ORB-12's forward-compatibility are two halves of one claim: the
client subset is closed, the server subset is open-with-a-counter. ORB-13 and ORB-18 are the
closed half of the server side: what cannot be interpreted is fatal, whether the frame or a
required member.
