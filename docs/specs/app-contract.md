# Spec: The application contract — `sipx.app.v1`

**Status:** normative for the `sipx.app.v1` wire line — and **experimental**: the line may change
incompatibly until two dissimilar applications run against it (an inbound IVR and an outbound
notifier), after which changes require a new line · **Crate:** `sipx-app-protocol` (planned) ·
**Stories:** C-5 (and C-3, C-4, C-6, M-17, M-18 for the operations it names) ·
**Design:** [app-sdk](../designs/app-sdk.md)

> **Read this first.** There is no RFC for driving a call from an application over a wire; every
> choice below is **sipx's**, marked `[sipx]`. What SIP itself requires — transaction behaviour,
> dialog state, what a REFER means — is not restated here: this contract *names* those
> capabilities, and the SIP semantics behind them are RFC 3261 and the RFCs this section lists,
> implemented and specified elsewhere in this repository. The contract's versioning is its own
> (§4): `sipx.app.v1` is a wire line, not a crate version.

## 1. Normative references

- RFC 8259 — the wire format is JSON, UTF-8, no BOM.
- RFC 3339 — timestamps.
- RFC 3261 — the SIP semantics behind `answer`, `reject`, `dial`, `hangup`, and dialog identity.
- RFC 4733 — DTMF as telephone-events; what a "digit" is on the wire.
- RFC 3515, RFC 3891 — REFER and Replaces; the semantics behind the transfer verbs.
- RFC 2104, RFC 6234 — HMAC and SHA-256, for document-mode authentication (§9).
- RFC 6648 — why the signature header field is `Sipx-Signature`, not `X-…`.
- RFC 9110 — HTTP semantics for the document binding (§7).
- RFC 6455 — WebSocket, one carrier of the session binding (§8).
- [`sip-message.md`](sip-message.md), [`sip-transaction.md`](sip-transaction.md) — what the host
  is driving underneath.

## 2. Model and terms

- **Host** — the process that owns calls (built on `sipx-call`) and executes instructions.
- **App** — the customer's code: a webhook endpoint, a connected session peer, or a handler in
  an embedded runtime. The app never touches SIP; it consumes **events** and supplies
  **instructions**.
- **Program** — the host-side queue of not-yet-completed instructions for one call.
- **Bindings** — how events and instructions travel: **document mode** (§7, request/response),
  **session mode** (§8, full-duplex). An embedded runtime is a third carrier of the session
  binding's semantics with no wire; it changes nothing in this spec.

**[sipx] One call, one program, one app.** Every event belongs to exactly one call; instructions
act on the call whose event stream produced them (a `dial` creates a new leg *of the same call*).

**[sipx] Events are authoritative.** Every event carries a full call snapshot (§5.2). An app
that keeps its own state is expected to overwrite it from the snapshot on every event; the
contract never sends deltas, so a missed delivery cannot leave an app permanently wrong.

**[sipx] The interpreter is sans-IO.** The state machine defined by §§5–6 and the vectors in
§11 — events and documents in, effects out — contains no socket and no clock; time enters as
fired-timer inputs. Bindings are drivers over it.

## 3. Effects: what instructions map onto

Every instruction resolves to exactly one call-framework operation. The contract may not name a
verb that has no operation, which is why the epic's kernel stories exist:

| Verb | Operation | Story |
|---|---|---|
| `answer`, `ring`, `reject` | `answer`/`answer_ringing`/`ring` and a final refusal | shipped |
| `play`, `gather`, `record` | playback handle with stop and interrupt-on-digit | M-17 |
| `send_dtmf` | `Call::send_digits` | shipped |
| `dial` | `dial()` as a new leg | shipped + C-4 |
| `bridge`, `unbridge` | bridge two owned calls | C-6 |
| `hold`, `resume` | `reinvite(Direction)` | shipped |
| `mute`, `unmute` | local media gate | M-18 |
| `transfer`, `accept_transfer`, `refuse_transfer` | `refer`/`refer_attended`/`accept_referral`/`refuse_referral` | shipped |
| `hangup`, `pause`, `tag` | `hang_up`; interpreter-internal | shipped |

## 4. Versioning

**[sipx]** The wire line is the string `sipx.app.v1`, present in every envelope and every
document. Within a line, unknown *fields* must be ignored by both sides; an unknown *event
type* must be ignored by the app; an unknown *instruction verb* is an error (§6.4) — a host
that skipped a verb it does not know would run a different program than the app wrote. A
change that would break any of the vectors in §11 requires `sipx.app.v2`.

## 5. Events (host → app)

### 5.1 Envelope

```json
{
  "contract": "sipx.app.v1",
  "seq": 4,
  "at": "2026-07-28T09:15:04.221Z",
  "call": { … §5.2 … },
  "event": { "type": "call.dtmf", "digit": "5", "duration_ms": 160 }
}
```

- `seq` **[sipx]**: per-call, starts at 1, increments by 1 per event. Redelivery (document-mode
  retries, session reconnect replay) repeats `seq`; an app must treat a repeated `seq` as the
  same event (vector AC-4). Gaps must not occur; an app seeing one may resynchronise from the
  next snapshot.
- `at`: RFC 3339, UTC, milliseconds.

### 5.2 The call snapshot

```json
{
  "id": "b7c1…", "leg": "a", "direction": "inbound", "state": "answered",
  "from": "sip:alice@example.com", "to": "sip:support@example.net",
  "headers": { "p-asserted-identity": "\"Alice\" <sip:alice@example.com>" },
  "media": { "encrypted": true, "on_hold": false, "muted": false },
  "legs": [ { "leg": "b", "state": "ringing", "to": "sip:bob@example.net" } ],
  "bridged": false,
  "tags": { "campaign": "renewal" }
}
```

`headers` **[sipx]** carries a *selected* set of inbound header fields (`From`, `To`,
`P-Asserted-Identity`, `Diversion`), lowercased keys, decoded values. It is not the raw message
and never carries fields the host uses to route (`Via`, `Route`, `CSeq`, …). `state` is one of
`incoming · ringing · answered · ended`.

### 5.3 Event types

| Type | Extra fields | Emitted when |
|---|---|---|
| `call.incoming` | — | a new INVITE reached the host and matched this app |
| `call.ringing` | `reliable` | a provisional was sent or received |
| `call.early_media.started` | — | a reliable provisional completed offer/answer and its media session is running |
| `call.answered` | — | the 2xx/ACK completed; media may flow |
| `call.dtmf` | `digit`, `duration_ms` | an RFC 4733 event ended |
| `call.playback.finished` | `instruction_id`, `completed` | a `play` ran out or was cut |
| `call.gather.finished` | `instruction_id`, `digits`, `reason` (`terminator · max · timeout`) | a `gather` resolved |
| `call.recording.finished` | `instruction_id`, `duration_ms` | a `record` resolved |
| `call.dial.finished` | `instruction_id`, `leg`, `outcome` (`answered · busy · rejected{status} · timeout`) | a `dial` resolved |
| `call.transfer.requested` | `target`, `attended` | an inbound REFER arrived; the app must decide (§6.3) |
| `call.transfer.progress` | `state` (`trying · ringing · succeeded · failed{status}`) | a NOTIFY moved the transfer |
| `call.bridged` / `call.unbridged` | `leg` | the media coupling changed |
| `call.hold` / `call.resumed` | — | the far end changed the media direction |
| `call.ended` | `cause` (`hangup · remote · rejected{status} · timeout · error`) | the call is over; always the last event, never dropped |

## 6. Instructions (app → host)

### 6.1 Document

```json
{
  "contract": "sipx.app.v1",
  "instructions": [
    { "id": "p1", "do": "play", "source": { "file": "welcome.wav" }, "interruptible": true },
    { "id": "g1", "do": "gather", "max": 4, "terminators": "#", "digit_timeout_ms": 4000, "timeout_ms": 10000 }
  ]
}
```

**[sipx]** `id` is client-assigned, unique within the call, and echoed as `instruction_id` on the
completion events of §5.3 — correlation is the app's, not positional. Instructions execute
strictly in order; a verb with a completion event blocks the queue until it resolves.

### 6.2 Verbs

| Verb | Fields | Completes with |
|---|---|---|
| `answer` | — | `call.answered` |
| `ring` | `reliable` | immediate |
| `reject` | `status`, `reason` | `call.ended` |
| `play` | `source` (`{file}` or `{inline}` — §6.5), `interruptible` | `call.playback.finished` |
| `gather` | `min`, `max`, `terminators`, `digit_timeout_ms`, `timeout_ms`, optional `prompt` (a `play` source, interruptible by definition) | `call.gather.finished` |
| `record` | `max_ms`, `idle_stop_ms` | `call.recording.finished` |
| `send_dtmf` | `digits`, `duration_ms` | immediate |
| `dial` | `target`, `from`, `timeout_ms`, `headers` (allowlisted — §6.5) | `call.dial.finished` |
| `bridge` | `leg`, `dtmf` (`passthrough · consume`) | `call.bridged`; a *state*, ended by `unbridge` or either leg ending |
| `unbridge` | — | `call.unbridged` |
| `hold` / `resume` | — | immediate (re-INVITE outcome surfaces as events) |
| `mute` / `unmute` | — | immediate |
| `transfer` | `target` **or** `via_leg` (attended) | `call.transfer.progress` |
| `accept_transfer` / `refuse_transfer` | — / `status` | transfer events / immediate |
| `pause` | `ms` | timer-driven |
| `tag` | `key`, `value` | immediate; lands in every later snapshot |
| `hangup` | `cause` | `call.ended` |

### 6.3 The continuation rule (normative, document mode)

**[sipx]** Per call, strictly alternating: the host delivers one event and waits; the app's
response document is the *entire* new program. **At most one callback is outstanding per call**,
and **a document accepted in response to event E replaces the pending program** — whatever was
still queued is discarded (running interruptible work is stopped; `bridge` state and `tag`s
persist). This is how program-level barge-in composes: respond to `call.dtmf` with a new
program, and the old one is gone (vector AC-3). An event that occurs while a callback is
outstanding queues and is delivered after the response is applied (AC-8) — except that a
snapshot always reflects *now*, not the queue's past.

An empty `instructions` array is valid and means "keep going". Anything the app cannot express
under alternation — acting on leg B while leg A's callback is out, unsolicited action at an
arbitrary time — is what session mode is for.

### 6.4 Errors

A document that fails to parse, names an unknown verb, an unknown `leg`, or an illegal field
value is rejected **whole** — no partial application — and the app's declared failure policy
(§9.2) applies as if the callback had failed with a 5xx.

### 6.5 Two deliberate limits

- `play.source` is a host-local `{file}` or `{inline}` base64 PCM only. Fetching by URL is a
  host capability behind an allowlist, outside this contract; a TTS verb is a non-goal.
- `dial.headers` may only set fields on a host-configured allowlist. The kernel's builders make
  header injection unrepresentable; a free header map here would hand that property away.

## 7. Document binding (HTTP)

`POST` per event; the envelope is the body; the response body is the document (or empty for
"keep going"). `2xx` is acceptance. Anything else, a timeout, or a connection failure invokes
§9.2. Redelivery on failure repeats `seq`. Requests carry `Sipx-Signature` (§9.1).

## 8. Session binding (WebSocket or a subprocess pipe)

Same envelope and document types as JSON text frames; the alternation rule of §6.3 does **not**
apply — the app may send a document at any time (it still *replaces* that call's program), and
one session multiplexes many calls. Additionally: an app may send
`{"do": "originate", "target": …, "from": …}` to place a new outbound call — the contract is
not purely reactive, and the host also exposes the same originate as a management-API request.
**[sipx] Backpressure is declared:** the per-session outbound queue is bounded; on overflow the
host closes the session (WebSocket close code 1013) and applies §9.2 to every call it carried.
Binary frames are reserved for a future media channel; in `v1` a binary frame is a protocol
error and closes the session.

## 9. Trust

### 9.1 Authenticating the host to the app

**[sipx]** Document-mode requests carry `Sipx-Signature: t=<unix-seconds>,
v1=<hex(HMAC-SHA-256(secret, t ∥ "." ∥ body))>` (RFC 2104, RFC 6234; field name per RFC 6648).
The app rejects a signature outside its replay window (recommended: 300 s) or one that does not
verify. Session mode authenticates at establishment (bearer or the same scheme on the upgrade
request) and needs no per-frame signature.

### 9.2 Declared failure semantics

**[sipx]** What happens when the app fails is **configuration declared per app, never code**:

| Knob | Values | Applied when |
|---|---|---|
| `timeout_ms` | duration | the callback does not return in time |
| `on_timeout`, `on_5xx`, `on_unreachable` | `continue` (keep program) · `hangup` · `reject{status}` | transient failures |
| `on_4xx` | same values | the app says the request itself is wrong |

A call with no program and no reachable app follows the same declaration (AC-1). Defaults:
`timeout_ms: 2000`, `on_timeout/on_5xx/on_unreachable: continue`, `on_4xx: reject{500}` — a
flapping app degrades a call it has already scripted, it does not kill it.

## 10. What this contract does not do

No routing between endpoints, no registration control, no raw SIP header access, no media
frames (reserved, §8), no conference verb yet (`M-12` exists; the verb waits for a consumer),
no application-server-model early session (RFC 3960 section 4), no record-to-URL, no TTS.

## 11. Vectors

Each row is a test in `sipx-app-protocol`; the JSON bodies live beside the tests. `→` is
host-to-app, `←` is app-to-host.

| # | Scenario | Assertion |
|---|---|---|
| AC-1 | `call.incoming` → app unreachable | after `timeout_ms`, the declared `on_unreachable` effect and nothing else; no panic, no hang |
| AC-2 | `call.incoming` → ← `answer, play(p1), gather(g1)` | effects in order; `call.gather.finished` carries `instruction_id: "g1"` |
| AC-3 | during AC-2's play: `call.dtmf` → ← `dial(d1)` | pending `gather` discarded, play stopped, dial effect issued — replacement, not append |
| AC-4 | redelivery of `seq: 3` answered differently | second response ignored; program unchanged |
| AC-5 | document names unknown verb `spindle` | rejected whole; §9.2 as 5xx; prior program still runs |
| AC-6 | `gather` with no digits until `timeout_ms` | `call.gather.finished{digits: "", reason: "timeout"}` |
| AC-7 | `dial` refused with 486 | `call.dial.finished{outcome: busy}`; snapshot's `legs` no longer lists the leg |
| AC-8 | `call.dtmf` fires while AC-2's callback outstanding | delivered after the response is applied, `seq` in order, snapshot current |
| AC-9 | `call.ended` under full event queue | still delivered; whatever the overflow policy drops, it is never `call.ended` |
