---
id: phone-lifecycle
---

# Phone call lifecycle closure

**Status:** proposed · **Pillar:** Phone/Transport · **Epic:** `phone-lifecycle` ·
**Review:** [external functionality and usability review](../reviews/extern-2026-08-06T01-18-47+02-00-full-sweep.md)
findings 1, 8 and 11 · **Stories:** `P-16`, `P-17`, `T-37`

## Problem

The diagnostic phone can establish a call, but its process lifecycle is not yet the dialog's
lifecycle. A confirmed answerer can wait out its own duration after the peer's BYE; interrupting a
caller can omit both orderly teardown and a terminal record; invitation timeout cleanup adds a
fixed, undocumented tail; and an immediate connection refusal is flattened into a SIP timeout.
Those are one boundary problem: the command driver is not consuming and reporting every terminal
cause from transport through dialog cleanup.

## Direction

- A confirmed call is driven by one cancellation-safe serving loop. Remote in-dialog requests,
  media progress, the local duration, process interruption and transport failure race as typed
  inputs; exactly one cause wins and owns teardown.
- RFC 3261 §§12.2.2 and 15 govern BYE handling. A received BYE is answered before the local process
  reports completion; a deliberate local stop sends BYE when a dialog exists. A pending INVITE uses
  RFC 3261 §9 cancellation instead.
- Timeout means an observable end-to-end budget, not only the first phase of an operation. If SIP
  cancellation needs a distinct cleanup allowance, it is named, bounded and reported rather than
  hidden in a fixed sleep.
- Transport establishment errors stay typed until the command maps them to its stable result and
  exit vocabulary. A definitive connection refusal is a failure; silence from a reachable
  datagram target can still be a timeout.
- Teardown closes admission, cancels owned work, joins it, and only then emits the terminal record.
  No detached cleanup task may outlive the command.

## Boundaries

This epic does not redesign the sans-I/O dialog or transaction state machines. It fixes the async
drivers and command adapters that must keep feeding those machines while a call is live. It also
does not add automatic retry policy: callers receive the correct cause and decide whether retry is
appropriate.

## Exit

The epic is complete when remote BYE, local duration, interruption, invitation timeout and
transport failure each have a deterministic process-level proof, terminal causes cannot be emitted
twice, and no path leaves dialog, media or transport work running after the process result.
