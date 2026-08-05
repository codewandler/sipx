# Confirmed-dialog snapshot and restoration

**Status:** normative · **Story:** S-43 · **RFCs:** 3261, 3264, 3311, 4028

## 1. Scope

RFC 3261 §§12.1 and 12.2 define the state a user agent retains for a dialog and the rules for
constructing later requests. This specification makes the confirmed subset durable without making
runtime resources durable. It covers a call whose initial offer/answer exchange is complete and
whose dialog usage is neither ended nor in the middle of an in-dialog transaction.

It does not promise transparent process failover. A snapshot contains no transaction, retransmission
timer, socket, task, resolver result, authentication credential, identity key, SRTP/DTLS key, ICE
password, entropy, media frame, event receiver or process-local clock value. A host must reconstruct
and inject those resources before it can attach the restored protocol state.

## 2. Public operations and atomicity

The public surface has three operations:

1. `Call::dialog_snapshot(now)` validates the live call at the caller-supplied monotonic `now` and
   produces an immutable `DialogSnapshot` or a typed `SnapshotError`.
2. `DialogSnapshot::encode()` produces the canonical bytes in §4; `DialogSnapshot::decode(bytes)`
   applies the total and per-field bounds before returning a value.
3. `Call::restore_dialog(snapshot, context)` validates the complete snapshot and fresh context before
   returning a call. The context includes the host-measured elapsed time since capture. Restoration
   starts no signalling transaction. Any media runtime in `context` remains caller-owned until
   success, and a failed restore does not stop or mutate it.

Validation precedes publication. No public collection, dispatcher route or task sees a partially
decoded or partially restored dialog. After validation, restore atomically claims the context;
only one concurrent or repeated attachment can succeed. The claim starts no work. A restored call's
event receiver begins empty: later transitions are emitted normally, while historical `Ringing` or
`Answered` events are not replayed as though they happened after restoration.

## 3. Snapshot state

The version-one snapshot contains exactly these non-secret facts:

| Field | Meaning and invariant |
|---|---|
| role | caller or callee, preserving RFC 3261 local/remote orientation |
| Call-ID, local tag, remote tag | all non-empty; together form the dialog identifier |
| local and remote party | complete validated `From`/`To` address values without runtime handles |
| remote target | one absolute SIP or SIPS URI from the latest target refresh |
| route set | zero or more validated route values, already in send order |
| local CSeq | last locally used value; MUST be below `u32::MAX` so the next request is monotonic |
| remote CSeq | optional greatest accepted remote value |
| signalling security | clear or protected; a SIPS target is never marked clear |
| media security | plain, SDES-SRTP or DTLS-SRTP; keys are excluded |
| media contract | profile, negotiated codec/wire payload, optional DTMF payload and RTCP mode |
| hold direction | the most recently negotiated RFC 3264 direction |
| peer UPDATE support | retained RFC 3311 capability used for later refresh selection |
| session timer | absent, or interval/refresher role and remaining lifetime at capture |
| offer state | version one admits only `idle`; any pending local or remote offer refuses capture |

An ended call, pending REFER/NOTIFY transfer usage, unacknowledged dialog-forming response, any live
ICE generation, or non-idle offer exchange returns a named `NotQuiescent` reason. Version one
deliberately refuses those states because their missing transaction, credentials, nominated-pair
state or timer cannot be recreated from dialog facts alone. It refuses all ICE rather than trying to
infer whether a generation happens to be idle at the capture instant.

The initial response, diversion history, application tags and diagnostic counters are not required
to construct the next in-dialog request and are not serialized.

## 4. Canonical byte encoding and bounds

The encoding is network-byte-order binary, independent of Rust layout:

```text
magic       4 octets   "SXD1"
version     u16        1
flags       u16        role/security/session bits; every unassigned bit is zero
fields      ordered, length-prefixed values from §3
checksum    none       integrity/authentication belongs to host storage policy
```

Unsigned integers are big-endian. Optional values have a one-octet presence marker (`0` or `1`).
Byte strings use a big-endian `u32` length followed by exactly that many octets. Route count is a
big-endian `u16`, followed by that many strings. Encoders emit one spelling; decoders reject trailing
bytes, non-zero reserved bits, unknown enum values and non-canonical presence markers.

The decoder refuses an input larger than 262,144 octets before reading a field. Call-ID and each tag
are limited to 1,024 octets; each party, target and route value to 8,192; the route count to 64; and
the sum of all variable fields to 131,072. It checks a length against remaining input and the field
limit before allocating. Unknown versions return `UnsupportedVersion`; truncation, oversized values,
invalid UTF-8 where text is required, invalid URI/address syntax, repeated or contradictory state,
and trailing bytes have distinct typed errors. Audio and DTMF payload values are limited to the RTP
header's seven-bit payload type field (`0..=127`); values `128..=255` are typed refusals rather than
values that can later alias after masking.

## 5. Restore context and security

The host supplies one `DialogRestoreContext` containing:

- a fresh endpoint handle and explicit resolved `Target` for the current first hop;
- an already-created call-owned media session plus the media policy and negotiated non-secret wire
  facts it implements, including the negotiated media direction;
- explicit `now`, elapsed time since capture, media bind/advertised addresses and session-expiry
  policy; and
- observed signalling protection plus the injected media keying class.

The context is validated against the snapshot before attachment. A protected snapshot requires TLS,
WSS or QUIC target protection. SDES/DTLS state requires a context declaring the same keying class and
an encrypted media session; plain state refuses an implicitly encrypted/different policy rather than
silently changing the negotiated contract. Codec, wire payload, DTMF payload, RTCP mode, profile and
direction must agree before the context's one-owner claim. A SIPS remote target or route can never
be restored through clear signalling.
The media session's own codec, wire payload, DTMF payload, RTCP mode, encryption fact, ICE fact and
bound address are compared with the declarations; policy text alone is not treated as runtime proof.

Security declarations are facts about resources the host already created; they do not contain or
derive keys. Debug output for snapshot, context and every error omits party values and secret-bearing
runtime internals. In particular, context diagnostics include only the target socket address,
transport and whether a WebSocket path exists; the complete target, certificate name and path are
not formatted because a path or query can contain credentials.

## 6. Time and session restoration

Capture never serializes an `Instant`. Given caller-supplied `now`, it stores the negotiated interval,
which side refreshes, and the remaining duration until the live action deadline. If the deadline is
at or before `now`, capture returns `SessionActionDue` instead of moving the deadline into the future.

Restore receives both fresh monotonic `now` and a host-measured `elapsed_since_capture`. The core
does not read a wall clock or infer downtime: the host derives elapsed time from its durable envelope
or orchestration state. Restore subtracts that elapsed duration from the captured remainder with
checked arithmetic before adding the residual to `now`. An elapsed duration equal to or greater
than the remainder returns a typed `SessionActionDue` containing whether the next action is refresh
or expiry, before the context is claimed; the host must feed that action through the normal call
timer path. It MUST NOT produce a live call with a silently renewed interval. Values above the
negotiated interval, below the RFC 4028 floor, or overflowing the runtime clock are contradictory
and refused.

## 7. Required vectors

| Vector | Input | Expected result |
|---|---|---|
| DP-1 | caller dialog, two routes, refreshed remote target, local CSeq 41, remote 9 | canonical encode/decode/encode bytes identical; next local request is 42; route order and target unchanged |
| DP-2 | valid DP-1 with version 2 | `UnsupportedVersion(2)` before any variable-field allocation |
| DP-3 | declared field length one beyond its limit and separately beyond remaining bytes | typed `FieldTooLarge` / `Truncated`; no partial value |
| DP-4 | duplicate/empty tag, malformed target, 65 routes, payload type 128 or 255, non-zero reserved flag or trailing byte | named invariant error; no restore side effect |
| DP-5 | protected/SIPS snapshot with clear UDP context | `SecurityDowngrade`; endpoint counters and task/transaction counts unchanged |
| DP-6 | SDES or DTLS snapshot with plain/mismatched injected media policy, or a mismatched injected direction | typed security/contract mismatch; no serialized key bytes, runtime mutation or consumed context claim |
| DP-7 | elapsed time below, equal to and greater than the captured remainder; separately, a remainder greater than the interval | checked `now + (remaining - elapsed)`, reusable pre-claim `SessionActionDue`, and contradiction refusal |
| DP-8 | snapshot/capture while offer, ACK, transfer or ICE work is pending | typed `NotQuiescent`; no bytes produced |
| DP-9 | restored loopback dialog sends one re-INVITE, receives its response, then shuts down the fresh endpoint | dialog identifiers, route order, target and CSeq are preserved; the endpoint shutdown barrier completes without orphaned work |

Adversarial tests enumerate every prefix of a canonical snapshot, mutate each byte position, and
exercise bounded hostile payloads. Decoding never panics, allocates beyond the declared limits or
accepts two encodings for one snapshot value.

## 8. Host responsibility

sipx does not open, name, encrypt, lock, replicate or delete snapshot files. Hosts choose storage,
access control, encryption at rest, durability, distribution and split-brain policy. A snapshot is
sensitive call metadata even though it contains no credential or key. Public examples keep bytes in
memory, use explicit redaction and say that successful decode proves format validity, not ownership
or authorization to resume a call.
