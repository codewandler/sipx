# Design: media runtime safety

**Status:** accepted · **Pillar:** Media · **Epic:** `media-runtime-safety` · **Stories:** M-35,
M-36, M-37

## Why

Media setup crosses three boundaries where a public value becomes a long-lived worker: ownership,
timing and negotiated codec construction. The 2026-07-30 repository review found a failure in each.
Dropping a conference leaves participant collectors detached, zero timing values can terminate a
worker or spin it, and failed Opus construction substitutes PCMU state under the negotiated Opus
payload type. These paths are narrow, but each breaks a public contract silently or catastrophically.

## Approach

- A conference owns cancellation and join handles for the mixer and every participant collector.
  Explicit close and `Drop` use the same idempotent shutdown path.
- Public media and conference configurations are validated before sockets or workers start. Zero
  packet, report and mix intervals are typed errors; worker loops never need to defend against an
  impossible interval.
- Codec pipeline construction is fallible and preserves negotiation truth. Failure to construct the
  negotiated codec fails setup or disables that negotiated route explicitly; it never installs a
  different codec under the same payload type.
- Tests observe worker and strong-reference termination, not only functional audio on the happy path.

The normative startup, ownership, timing, and codec-failure contract is in
[`docs/specs/media-runtime.md`](../specs/media-runtime.md). Media packet formats and negotiation rules
remain in their existing specs.

## Alternatives considered

- Abort only the top-level mixer. Rejected because detached collectors retain their sessions
  independently.
- Replace zero timing values with defaults. Rejected because a caller-provided duration is a contract,
  and silent coercion makes diagnostics and capacity planning unreliable.
- Fall back to another codec when construction fails. Rejected because the negotiated payload type is
  a wire contract; relabeling a different codec produces invalid media rather than graceful
  degradation.

## Risks and open questions

- `Drop` cannot perform unbounded asynchronous waiting, so task shutdown needs an observable bounded
  mechanism that remains safe when no runtime is available to the destructor.
- Validation may make constructors fallible and therefore change public APIs; the story implementation
  must choose the narrowest compatible boundary and document migration.
- Codec failures should retain enough context for operators without logging key material or media.

## Acceptance / done

The epic is done when M-35 through M-37 are done and adversarial tests prove that invalid setup starts
no media workers, dropped conferences retain no sessions, and every active payload type uses exactly
the codec negotiated for it.
