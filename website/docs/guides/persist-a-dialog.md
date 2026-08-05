---
title: Persist and restore a confirmed dialog
description: Capture bounded SIP dialog facts, then attach them to explicit fresh signalling and media drivers without serializing runtime resources or secrets.
---

# Persist and restore a confirmed dialog

`sipx-call` can capture the protocol facts needed to continue a confirmed, quiescent SIP dialog.
The result is deterministic, bounded, versioned bytes. It is deliberately not a serialized
`Call`: an endpoint handle, socket, transaction, timer task, credential, private key, media key,
entropy source, media frame, and process-local clock instant never enter the snapshot.

The surface is Experimental while sipx is pre-1.0. The version-one format accepts ordinary
non-ICE audio calls whose initial ACK and offer/answer exchange have settled and which have no live
transfer usage. A call with pending work returns a typed refusal instead of writing an incomplete
snapshot.

## Capture and store

Capture receives the current monotonic time explicitly. When a session timer is active the bytes
retain its remaining duration, not the `Instant` from this process:

```rust
let snapshot = call.dialog_snapshot(tokio::time::Instant::now())?;
let durable_bytes = snapshot.encode();
```

`DialogSnapshot::decode` checks the complete input ceiling, each field and route bound, the sum of
variable data, UTF-8 and URI grammar, CSeq headroom, security contradictions, reserved values, and
trailing bytes before it returns a value. Unknown schema versions are typed errors. Decoding proves
that the bytes are a valid snapshot; it does **not** prove who wrote them or authorize resuming the
call.

Your application owns the state store. That includes naming and locking records, access control,
encryption at rest, durability, replication, deletion, leader election, and split-brain prevention.
Even without credentials or keys, the parties and routing data are sensitive call metadata.

## Rebuild drivers, then attach

After a restart, first create a new transport endpoint and a new media session. Separately recover
or calculate `elapsed_since_capture` from your durable envelope or orchestration state; sipx does
not read a wall clock or assume that a process restart was instantaneous. Build the media
configuration from the snapshot's codec, payload, DTMF, RTCP, profile, direction and keying getters;
key material is supplied through your normal protected configuration path, never recovered from
the snapshot. Then describe the already-created resources in a `DialogRestoreContext`:

```rust
let snapshot = sipx_call::DialogSnapshot::decode(&durable_bytes)?;

let context = sipx_call::DialogRestoreContext::new(
    fresh_endpoint,
    resolved_first_hop,
    fresh_media_session,
    fresh_media_address,
    negotiated_remote_media_address,
    explicit_media_policy,
    snapshot.direction(),
    elapsed_since_capture,
    tokio::time::Instant::now(),
);

let call = sipx_call::Call::restore_dialog(&snapshot, &context)?;
```

Restoration is synchronous and performs no I/O. It validates all snapshot and context facts before
claiming the context. A secure dialog cannot attach to clear signalling; plain, SDES-SRTP and
DTLS-SRTP do not substitute for one another; codec and payload facts must match the running media
session. The fresh driver's direction is explicit too and must match the retained negotiated
direction. A context can successfully attach only once, so concurrent duplicate attempts produce
one call and one typed refusal.

The elapsed duration is deducted from the retained session remainder. If no lifetime remains,
restoration returns the exact refresh-or-expire action that is due before claiming the context. It
never turns downtime into a new lease. Only the residual lifetime is added to the explicit fresh
`now`, with checked arithmetic.

The restored event stream begins empty. It reports transitions that happen after attachment rather
than replaying historical `Answered` events. Snapshot distribution and authorization stay with the
host; sipx resumes one validated protocol owner and does not provide a failover coordinator.

## Current boundary

Version one refuses ended dialogs, an unacknowledged dialog-forming response, a pending offer or
answer, an active transfer usage, and every live ICE generation. ICE credentials and nominated-pair
state are runtime security facts, so silently dropping them would turn restoration into a media-path
change. A later schema may define a safe re-establishment contract; version one says no rather than
guessing.
