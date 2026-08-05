# Design: Realtime phone understanding and actions

**Status:** proposed · **Pillar:** Application · **Epic:** `openai-realtime-phone` · **Stories:**
A-30, A-31, A-32, A-33, X-107, X-108

## Why

The delivered A-22 bridge lets one routed call exchange bounded G.711 audio with a configured
Realtime session. The next useful boundary is not another bridge: it is making the session's
transcripts, understanding and function requests visible without granting the model authority over
the phone. Applications need typed lifecycle events and a strict policy layer before any generated
request can become DTMF, mute, hold, transfer, hangup or another supported phone action.

This is an opt-in extension of the existing bridge, configuration and deterministic peer. It does
not create another audio path, credential source, WebSocket client or live-test authority. A-21
remains the default-suite counterparty; A-23 owns the one-call credentialed live-proof guard.

## Approach

`A-30` expands the existing finite call-owned session state machine to cover correlated speech,
transcript, response, rate-limit, error and function-item lifecycle. Session replacement is
deliberate: a new socket cannot pretend that old conversation state or a pending action survived.
A-22's bounded audio queues and interruption accounting remain the only media bridge.

`A-31` publishes typed understanding and transcript events through the application SDK. Events
carry call, session, item and utterance identity plus explicit external-model provenance. They carry
no authority bit, and their bounded delivery policy preserves final, cancellation and error state
without blocking media.

`A-33` is the policy gate. Closed schemas, a bounded idempotency store, queue and execution
deadlines, cancellation and explicit confirmation for consequential actions all run before phone
mutation. Model text cannot confirm its own request, and a late result cannot act on a replaced
session or ended call.

`A-32` converts a validated function item into an application-owned action request. Its tool schema
is generated only from actions the current phone exposes and policy permits. Every phone action is
classified as model-exposable or forbidden; new actions default to forbidden. The model receives no
`Handle`, raw SIP operation, shell/network executor, credential, address book or route authority.

`X-107` extends A-21's deterministic service double with the new events and action races, then reuses
A-23's bounded opt-in live proof for the corresponding live observations. `X-108` publishes the
runnable phone and honest latency, queue, rate, usage, cost and action-outcome evidence without
hard-coding a price or exposing caller content.

## Boundaries

- A-22's encoded-audio bridge, host configuration and credential resolution remain the single
  implementation path. This epic adds no PCM seam and does not duplicate local-speech providers.
- Transcript and understanding are untrusted application data. Only A-33 policy plus A-32's closed
  registry can authorize a phone action.
- The model never receives a transport or call handle. Accepted actions reuse supported SDK
  operations and preserve their typed terminal outcomes; dispatch alone is not success.
- Deterministic tests remain credential-free. A live run is explicit, bounded, redacted and owned by
  A-23's guard; absence is disclaimed rather than skipped or called success.
- Session, event, idempotency, action and confirmation state are per call and finite. Teardown joins
  every task and makes late results inert.

## Exit

A live far-end caller and configured session exchange bounded audio through A-22 while typed speech,
transcript, response and rate state reaches the SDK; only schema-valid, idempotent, policy-accepted
actions execute; consequential actions require independent confirmation; every request has a
correlated terminal event; deterministic CI covers malformed, reordered, replayed and overloaded
paths; and the packaged example records redacted, bounded evidence with zero residual work.
