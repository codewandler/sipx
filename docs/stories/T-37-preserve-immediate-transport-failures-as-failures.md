---
id: T-37
title: "Preserve immediate transport failures as failures"
pillar: "Transport"
status: ready
priority: 2
epic: phone-lifecycle
areas: [sipx-transport, sipx-cli]
design: docs/designs/phone-lifecycle.md
note: "external review finding 11 · a refused connection is currently reported as a SIP timeout"
---

# Preserve immediate transport failures as failures

## Goal

Carry definitive transport-establishment failures through call setup without flattening them into
"no final SIP response". Retry policy must be able to distinguish a closed connection endpoint
from a reachable peer that stayed silent.

## Acceptance

- [ ] The relevant transport/call spec records the typed setup outcomes and their command mapping:
      immediate connection failure is `failed`/exit 1, while an established or datagram attempt
      with no final SIP response may be `timeout`/exit 5.
- [ ] A failing-first loopback test selects a stream transport and a closed port, observes the
      connection refusal without a fixed delay, and proves the current timeout mapping before the
      implementation changes.
- [ ] The endpoint send/connect result retains its source error through transaction and call setup;
      it is not replaced by a later timer merely because no response object exists.
- [ ] The CLI JSON and text failures preserve an actionable transport cause, emit nothing on result
      stdout before the terminal record, and exit 1.
- [ ] A non-answering UDP control still follows the configured invitation timeout and exits 5, so
      the fix cannot classify all absence of a response as transport failure.
- [ ] TLS verification, WebSocket handshake and connection-refusal controls prove the mapping is by
      typed cause rather than elapsed-time heuristics or string matching.
- [ ] No automatic retry loop is added. Focused transport/call/CLI tests and the complete repository
      gate are green.

## Review evidence

Finding 11 reached a closed local TCP port in roughly 20 ms but received `status: timeout`, exit 5
and "no final response to the INVITE" instead of the connection cause.
