# Spec: Scenario automation stream

**Status:** normative for `sipx scenario` · **Story:** P-19 · **Design:**
[diagnostic automation](../designs/diagnostic-automation.md) · **Envelope:**
[`sipx.app.v1`](app-contract.md)

This specification defines the subprocess stream owned by `sipx scenario`. It does not add a new
application-contract version: command results and call events use the existing `sipx.app.v1`
envelope, while the process exit reports whether the complete input stream was handled.

## 1. References and terms

- RFC 8259 defines JSON syntax and UTF-8 encoding.
- RFC 3339 defines the `at` timestamp in each output envelope.
- RFC 3261 defines the SIP operations behind dial, accept, reject and hangup.
- RFC 3515 defines the transfer request behind the `transfer` command.
- [`app-contract.md`](app-contract.md) defines the output envelope, call snapshot and instruction
  semantics shared with the other application bindings.
- [`diagnostic-phone.md`](diagnostic-phone.md) defines command-line setup, media policy and bounded
  cleanup.

A **frame** is one UTF-8 JSON object followed by a line feed. A **correlation** is the
caller-supplied `id`. A **command outcome** is exactly one `scenario.command.completed` or
`scenario.command.refused` event carrying that correlation. A **stream outcome** is the final
`scenario.stream.completed` or `scenario.stream.failed` event emitted after cleanup.

## 2. Input frame

The canonical frame is flat:

```json
{"id":"dial-1","command":"dial","uri":"sip:echo@127.0.0.1:5060","timeout_ms":5000}
```

`id` MUST be a non-empty string of at most 128 UTF-8 bytes. It MUST be unique for the lifetime of
the process. Once an output event has used a recovered or parsed correlation, a later frame cannot
reuse it. A duplicate is refused and is never executed.

`command` MUST be a non-empty string and is the canonical selector. The non-empty string field `do`
is retained as a compatibility alias only when `command` is absent. A frame containing both
selectors is refused, even when their values agree. A one-key-per-command object such as
`{"id":"dial-1","dial":{"uri":"sip:echo@example.net"}}` is not a command frame and is refused.

Unknown object members are ignored, following the `sipx.app.v1` within-line compatibility rule.
A known optional member with the wrong JSON type is not unknown and MUST be refused rather than
silently replaced by its default.

## 3. Commands

All fields listed as required are non-empty strings unless another type is shown. `timeout_ms`
values are unsigned integer milliseconds and therefore finite. A zero deadline is valid and fires
immediately. `headers` is an array of strings and each string passes the diagnostic phone's normal
application-owned header validation.

| Command | Required fields | Optional fields and defaults | Operation |
|---|---|---|---|
| `dial` | `uri` | `target` is a compatibility alias for `uri`; `from`; `timeout_ms` (CLI `--timeout`); `headers` (empty) | Place one outbound call and adopt it as the active call. Supplying both `uri` and `target` is refused. |
| `accept` | — | — | Accept the pending inbound invitation. |
| `reject` | — | `status` unsigned integer 300–699 (603); `reason` string (`Decline`) | Send the selected final refusal to the pending invitation. A successfully sent refusal completes this command. |
| `play` | `path` | — | Start bounded WAV playback on the active call. |
| `stop_playback` | — | — | Stop the owned playback. |
| `start_recording` | `path` | — | Start the bounded recording owned by this process. |
| `stop_recording` | — | — | Stop, join and write the owned recording. |
| `send_dtmf` | `digits` | — | Send the requested negotiated telephone events. |
| `hold` | — | — | Change the local media direction to send-only. |
| `resume` | — | — | Restore send-and-receive media. |
| `transfer` | `target` | — | Send a transfer request to the active call. |
| `hangup` | — | — | Stop owned media work and end the active call. |
| `wait_for` | `event`, `timeout_ms` | — | Complete when the named emitted event exists or refuse when the deadline fires. A missing deadline is always a refusal. |
| `shutdown` | — | — | Complete the command, perform orderly cleanup, emit the stream outcome and exit. |

`dial.uri`, its `target` alias, and `transfer.target` MUST be SIP URIs accepted by the corresponding
call operation. State-dependent commands are refused when their required pending invitation,
active call, playback or recording does not exist. An unknown command is refused.

## 4. Output and correlation

The first output is `scenario.ready`. Every output line is one `sipx.app.v1` event envelope with a
strictly increasing process-local `seq`. Runtime call events may occur between a command frame and
its outcome. They carry the call snapshot but no command correlation unless the event definition
requires one.

Every parsed frame with a valid new `id` receives exactly one command outcome:

```json
{"contract":"sipx.app.v1","seq":2,"at":"2026-08-06T00:00:00.000Z","call":{},"event":{"type":"scenario.command.completed","id":"dial-1","command":"dial"}}
{"contract":"sipx.app.v1","seq":3,"at":"2026-08-06T00:00:00.001Z","call":{},"event":{"type":"scenario.command.refused","id":"wait-1","message":"wait_for requires a finite timeout_ms"}}
```

The examples abbreviate `call`; the real envelope always carries the complete snapshot from
`app-contract.md`. A malformed JSON line receives `scenario.command.refused`. Its `id` is echoed
only when the prefix contains one unescaped, non-empty string correlation of at most 128 bytes;
otherwise the refusal is uncorrelated.

After orderly cleanup, the process emits exactly one uncorrelated terminal event:

- `scenario.stream.completed` when no frame or operation was refused;
- `scenario.stream.failed` when input, parsing, validation, execution or cleanup failed.

When cleanup itself fails, the failed terminal event includes its actionable `message`.
The terminal event is the output join barrier. No call, invitation, playback, recording or endpoint
work owned by the actor remains after it is printed.

## 5. Recovery, ordering and termination

Each input line is an independent recovery boundary. Invalid JSON, a missing or duplicate `id`, an
invalid selector, an unknown command or an operation refusal emits its refusal and processing
continues with the next line. A later successful command cannot erase the remembered stream
failure. Input order, runtime event order and command outcomes are preserved by the single actor.

Clean EOF and a successful `shutdown` command both request orderly cleanup. The `shutdown` command
outcome precedes cleanup and the terminal stream outcome. An empty stream therefore emits
`scenario.ready`, then `scenario.stream.completed`, and exits successfully without opening a call.
An stdin read error is a stream failure and still requests bounded cleanup.

## 6. Process exit

After the terminal stream event has been written:

| Exit | Meaning |
|---|---|
| 0 | Every received frame and requested operation completed, or the stream was empty. |
| 1 | At least one frame, command, operation, input read or cleanup step failed. |
| 2 | Command-line validation failed before `scenario.ready`; no scenario stream was established. |

A successfully executed `reject` command is a completed operation even though the SIP response it
sends is a refusal. Its intent was satisfied, so it does not make the stream fail. All deadlines,
media ownership and shutdown cleanup remain bounded by `diagnostic-phone.md`.

## 7. Executable transcript

Against an answering peer on `127.0.0.1:5060`, this finite stream dials, waits causally for the
answer, hangs up and shuts down:

```sh
printf '%s\n' \
  '{"id":"dial-1","command":"dial","uri":"sip:echo@127.0.0.1:5060","timeout_ms":5000}' \
  '{"id":"wait-1","command":"wait_for","event":"call.answered","timeout_ms":5000}' \
  '{"id":"hangup-1","command":"hangup"}' \
  '{"id":"shutdown-1","command":"shutdown"}' \
  | sipx scenario --local 127.0.0.1:0
```

The wait is an event predicate with a finite deadline. No fixed sleep substitutes for readiness.
