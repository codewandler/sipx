# Design: confirmed-dialog persistence

**Status:** accepted · **Pillar:** Signalling · **Epic:** `dialog-persistence` · **Stories:** S-43

## Why

A host that restarts a driver may still have a confirmed SIP dialog at the peer. Reconstructing that
dialog from a few remembered strings is unsafe: route order, target refreshes, independent sequence
numbers and session expiry are protocol state. Serializing a live `Call` is unsafe in the opposite
direction because it would turn sockets, tasks, clocks, media keys and credentials into durable
data.

## Approach

The persistent value is a bounded, versioned snapshot of confirmed protocol state. Encoding and
decoding are pure byte transformations. Capture receives the current monotonic time explicitly and
stores only remaining session lifetime, never a process-local instant. Capture refuses an ended
dialog, a pending offer/answer exchange or runtime-owned transfer work rather than claiming it can
resume an operation whose transaction was not serialized.

Restore is a validation and attachment operation. The host supplies a fresh endpoint, resolved
transport target, already-created media session and policy, explicit current time, and a declaration
of the security those new drivers actually provide. The operation validates every snapshot and
context invariant before publishing a `Call`; on failure it starts no transaction or task and leaves
caller-owned drivers untouched. Secure state may be restored only into an equally secure context,
whose key material is supplied out of band and never appears in the snapshot.

One context may attach successfully once. The one-owner claim is the only mutation in restore and
happens after every fallible validation; concurrent attempts therefore yield one `Call` and one
typed refusal without duplicating the dialog or sharing its media accidentally. The restored event
stream begins empty rather than replaying construction events from before the restart.

Snapshot durability, encryption at rest, replication, leader selection and distribution belong to
the host. sipx supplies bytes and validation, not a state store or failover coordinator.

## Exit

Deterministic vectors round-trip the complete confirmed-dialog contract, hostile bytes fail under a
fixed input/field/route bound, injected context cannot downgrade security or revive elapsed state,
and a restored loopback dialog can originate the next monotonically sequenced in-dialog request and
then shut down without residual transaction or task ownership.
