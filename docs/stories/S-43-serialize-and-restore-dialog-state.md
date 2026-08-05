---
id: S-43
title: Serialize and restore dialog state
pillar: Signalling
status: in-progress
priority: 10
design: docs/designs/dialog-persistence.md
epic: dialog-persistence
areas: [sipx-call, sipx-sip, security, m13, parity-wave-1]
predicate:
announcement:
note: discovered by X-97 · versioned sans-I/O snapshot without serializing sockets, tasks or secrets
---

# Serialize and restore dialog state

## Goal

Let a host persist the protocol state required to continue a confirmed dialog after process-local
drivers are rebuilt, without treating sockets, tasks, clocks or authentication secrets as dialog
data.

## Acceptance

- [ ] A spec cites RFC 3261 §§12.1–12.2 and defines a versioned snapshot containing dialog IDs,
      local/remote parties and tags, route set, remote target, local and remote CSeq, secure flag and
      the minimum session/offer state needed to reject unsafe restoration.
- [ ] Snapshot encoding is deterministic, bounded and schema-versioned. Unknown versions, missing
      invariants, oversized values and contradictory route/target/security state return typed errors
      without allocating unboundedly or partially restoring a dialog.
- [ ] Runtime-only state is excluded: endpoint handles, sockets, transactions, timers, spawned work,
      credentials, private keys, media keys and entropy are never serialized.
- [ ] Restoration requires the host to inject a fresh endpoint/media policy and explicit current
      time inputs; expired session state is rejected or surfaced as an immediate fired-timer action,
      never revived silently.
- [ ] Byte-level round-trip and hostile-snapshot vectors prove CSeq monotonicity, route order,
      remote-target preservation, secure-dialog refusal on an insecure endpoint and cancellation
      with zero residual transactions/tasks.
- [ ] The public docs state that snapshot durability/distribution belongs to the host, and
      `./scripts/gate.py` is green.

## Progress

- 2026-08-05: normative snapshot and restore contract is being written before the codec or runtime
  attachment API. The integration branch owns the deferred full gate.
- 2026-08-05: the `SXD1` codec, atomic fresh-driver attachment, security/timer guards, hostile-byte
  matrix and a live post-restore re-INVITE proof are implemented. Focused verification is green;
  status remains in progress until the integration branch runs the deferred full gate.
- 2026-08-05: review hardening rejects RTP payload values outside the seven-bit wire range and makes
  the fresh media direction an explicit, pre-claim restore invariant. Adversarial tests prove both
  refusals are typed and leave the context reusable.
- 2026-08-05: final persistence review makes downtime an injected restore fact and subtracts it from
  session lifetime before attachment; due timers remain typed and do not consume the context. Context
  diagnostics now omit the complete signalling target so WSS paths and queries cannot leak.
