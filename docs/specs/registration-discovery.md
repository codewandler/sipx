# Registration discovery contract

**Status:** normative · **Owner:** `sipx-ua` package consumer, `sipx-call` event runtime and
`sipx-cli peers` application surface · **RFCs:** RFC 3680, RFC 6665

## 1. Boundary and source of truth

Registration discovery is a package consumer of the generic event client in
[`event-client.md`](event-client.md). It does not copy SUBSCRIBE authentication, dialog, timer,
refresh, NOTIFY validation or cancellation state. The consumer receives only a bounded media type
and body and returns a typed current registrar snapshot.

The package core performs no I/O and reads no clock. The endpoint runtime records when it receives a
snapshot; `sipx peers` derives `age` from that observation and labels every entry `registrar`.
Registration contacts are discovery facts, not local bindings, routes or authority to accept calls.

## 2. Public package model and bounds

`RegistrationConsumer` is constructed with a positive contact limit and the exact registrar source
URI. It implements `PackageConsumer` with Event `reg`, Accept `application/reginfo+xml`, no neutral
value and an empty-body refusal while the subscription is live.

Its value is a complete `RegistrationSnapshot`:

| Field | Meaning |
|---|---|
| `version` | RFC 3680 document version last applied |
| `peers` | active contacts after applying this document |
| peer `name` | user part of the registration AOR, or the complete AOR when it has no user |
| peer `aor` | registration AOR which owns the contact |
| peer `uri` | exact active contact URI |
| peer `registration_id`, `contact_id` | stable document keys used for partial updates |

The generic `notify_body_limit` bounds bytes before parsing. The consumer additionally bounds active
contacts by its configured limit and rejects duplicate registration/contact keys, missing required
attributes, malformed XML, non-SIP contact URIs and entity expansion. It never returns a truncated
snapshot. Unknown elements and attributes are ignored only when their nesting is well formed; text
outside the RFC elements has no meaning.

## 3. Full and partial state

The first accepted document MUST have `state="full"`. A full document replaces all prior contacts
atomically. A partial document requires the next version and applies changes to a copy before
committing it:

| Registration/contact state and event | Current-set effect |
|---|---|
| active + registered/created/refreshed/shortened | insert or replace this contact |
| terminated + expired/deactivated/probation/unregistered/rejected | remove this contact |
| registration terminated | remove every contact under that registration id |
| contradictory state/event, unknown event, missing URI for an active contact | reject document |

Versions start at zero on the first full document and then increase by exactly one without
wrapping. A repeated, missing, regressed or gapped version is rejected and leaves the prior snapshot
unchanged. This fail-closed rule prevents a partial list from being presented as complete after one
lost NOTIFY. A later full document may re-establish authority.

## 4. CLI lifecycle and refusal semantics

Without `--registrar`, `sipx peers` retains the existing file-only behavior byte for byte. With
`--registrar <AOR>`, it attaches `EventSubscriptions` to the same dispatcher used by other live
endpoint services and begins one `reg` subscription. `--target`, signalling/TLS options,
`--password`/`SIPX_PASSWORD`, `--local`, `--expires` and `--watch` select that finite operation.
An explicit `--book` is read and merged; registrar-only discovery does not require a local book.

The command waits for a complete snapshot. `--watch N` keeps the subscription for N additional
seconds after the first snapshot, applies every subsequent NOTIFY, and prints only the final current
set. That duration is a user-selected observation window, not a protocol ordering delay. Each
registrar entry reports `status=peer`, `name`, `uri`, `source=registrar`, and integer `age` seconds
since the last applied snapshot. Book entries continue to omit age because they have no observation
time.

No snapshot is printed before authority exists. Initial 403 is `unauthorized`, initial 489 is
`rejected`, and Timer N / local expiry / transaction failure before the first snapshot is an
explicit timeout or failure. The diagnostic names the registrar status. A refusal never falls back
to a book-only success, because that would present a partial merged result as complete.

## 5. Conformance vectors

- **S24-V1 — full then registration.** A full version 0 document contains no active contacts. A
  partial version 1 NOTIFY registers `sip:alice@192.0.2.10`; the current snapshot contains that URI
  with registrar source and an age measured from the second delivery.
- **S24-V2 — removal.** Partial versions 2 and 3 expire one contact and terminate a registration;
  both are absent from the next complete snapshot.
- **S24-V3 — atomic failure.** A duplicate key, capacity overflow, malformed/non-SIP URI or version
  gap receives 400/413 and leaves the previously delivered snapshot unchanged.
- **S24-V4 — explicit refusal.** Initial 403, initial 489 and no initial NOTIFY each emit their typed
  terminal state and no peer delivery; the CLI exits non-zero and names the refusal.
- **S24-V5 — generic lifecycle and cleanup.** A real endpoint test observes SUBSCRIBE Event `reg`,
  sends full and partial NOTIFY through the generic S-38 runtime, sees the newly registered contact,
  then unsubscribes and observes zero lifecycle, timer and transaction work.
