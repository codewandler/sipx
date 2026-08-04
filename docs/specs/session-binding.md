# Spec: The session binding

**Status:** implemented by `A-4` · **Epic:** `app-host` ·
**Design:** [app-host](../designs/app-host.md)

> The wire is the contract's §8: envelopes and documents as JSON text frames, without webhook
> request/response alternation. This spec adds the host's side: establishment, multiplexing,
> liveness, bounded work, and what a dead session means for the calls it carried.

## 1. Establishment

- **[sipx-app]** A `protocol = "session"` listener accepts cleartext WebSocket upgrades. TLS is
  terminated by a reverse proxy; the listener should therefore be loopback or a protected private
  network. The request target is `/v1/apps/<app-name>`.
- The upgrade carries `Authorization: Bearer <secret>`. `<app-name>` must name a configured session
  app and the bearer must match the material resolved from that app's `bearer_secret`. A missing,
  unknown, or mismatched credential receives HTTP 401 and creates no session.
- One session serves one app. An app may hold at most 32 sessions and each session may carry at
  most 256 calls. A 33rd upgrade is refused with HTTP 503; a call that finds every live session at
  256 is unreachable.
- A new call is pinned to the live session for its app having the fewest calls; ties go to the
  oldest session. The pin lasts until the call ends or the session dies. It never moves to a
  reconnect and two sessions never own the same call.

### 1.1 Subprocess decision

The subprocess variant is deferred. WebSocket framing, authentication, liveness and close codes
are not reusable as safe process supervision: a subprocess additionally needs an executable
allowlist, a length-delimited stdio framing rule, stderr handling, restart limits, and child-process
group cancellation. Shipping an unspecified `lines()` loop would make both memory use and shutdown
unbounded. A later story may reuse the session registry and frames after specifying those process
properties; `A-4` does not spawn a child.

## 2. Frames

All frames are UTF-8 JSON text and carry `"contract":"sipx.app.v1"`. Unknown object members are
ignored. A binary frame closes the connection with 1003.

Host-to-app call events are the ordinary contract envelope. Its `call.id` names the pin, so no
second multiplexing wrapper exists.

An app replaces a call's program with:

```json
{"contract":"sipx.app.v1","request":"r1","call":"c1","instructions":[]}
```

`request` is a non-empty app-chosen correlation string, at most 128 UTF-8 bytes. `call` is required;
the rest of the object is an ordinary contract document and is rejected whole by the contract
parser. A valid document is acknowledged:

```json
{"contract":"sipx.app.v1","request":"r1","result":{"call":"c1"}}
```

Origination has no prior call:

```json
{"contract":"sipx.app.v1","request":"r2","do":"originate","target":"sip:bob@example.net","from":"sip:alerts@example.com"}
```

It requires the app's `grants.originate = true`. `target` and `from` are SIP URIs. The successful
result uses the same result shape and introduces the outbound call id. A result is sent only after
the call framework owns the call; subsequent envelopes carry that id and are pinned to the
requesting session.

Errors use one shape:

```json
{"contract":"sipx.app.v1","error":{"code":"unknown_call","message":"the call is not live on this session"},"request":"r1"}
```

The closed codes are `bad_frame`, `unknown_call`, `call_busy`, `originate_forbidden`, and
`originate_failed`. `request` is omitted only when the frame could not yield a valid correlation
string. An unknown or already-ended call is `unknown_call` and otherwise ignored: this is the
normal end/document race, not a session failure.

## 3. Liveness and bounded work

- The host sends an RFC 6455 Ping every 30 seconds. Any Pong proves liveness. If no Pong arrives
  within 10 seconds of a Ping, the session is dead. The host reads no fixed delay as ordering;
  timers only bound liveness failure.
- Each session has a 64-frame host-to-app queue and an 8-document queue per call. All sends use
  non-blocking admission. A full host-to-app queue atomically marks the session dead and closes its
  WebSocket with 1013 (`Try Again Later`). A full per-call document queue returns `call_busy` and
  leaves the session live.
- There are at most 128 live connection tasks per listener. All listener, connection, call, and
  originate tasks are owned by the host serving future and are cancelled and joined when it ends.
- A dead session applies each pinned call's declared `on_unreachable`, individually, by feeding
  `Failure::Unreachable` into the same contract interpreter that handles webhook delivery failure.
  Reconnection is a new session; calls that resolved failure do not migrate.

## 4. Multiplexing state table

| Input | State | Result |
|---|---|---|
| new call for app | at least one live session below 256 | pin least-loaded, oldest on tie |
| new call for app | no eligible session | apply that call's `on_unreachable` |
| document | call is pinned to this session | atomically replace its pending program; acknowledge |
| document | call is absent, ended, or pinned elsewhere | `unknown_call`; no call change |
| valid `originate` | granted | place call, pin it to this session, return its id |
| valid `originate` | denied or cannot be placed | typed error; session remains live |
| outbound queue full | live | mark dead, close 1013, fan out `on_unreachable` |
| ping grace expires / peer closes | live | mark dead and fan out `on_unreachable` |

## 5. Normative vectors

The vectors live beside the implementation in `sipx-app` and have these stable names:

| Vector | Claim |
|---|---|
| `SB-1 pinning` | least-loaded/oldest selection and a lifetime pin |
| `SB-2 dead fan-out` | one dead session independently applies every pinned call's policy |
| `SB-3 overflow` | the 65th queued event makes the session dead with close 1013 |
| `SB-4 unknown-call race` | a late document produces `unknown_call` and no mutation |
| `SB-5 originate` | deny-by-default and a granted request returns a newly pinned call id |
| `SB-6 coexistence` | webhook and session apps route through one host configuration |
| `SB-7 total binding` | every admitted app binding selects a driver or declared failure |
