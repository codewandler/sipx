# Design: User agent

**Status:** outline · **Pillar:** Signalling · **Epic:** `sip-ua` · **Stories:** _to be cut_

## Why

Applications should not assemble transactions by hand. The user agent layer is where SIP stops
being a message protocol and starts being calls, registrations and authentication — and it is
where a bad abstraction does the most damage, because everything above it inherits the shape.

## Approach

_To be written when the epic starts. In outline: a client that issues requests over the
transaction layer; a server that dispatches by method; dialogs (RFC 3261 §12) as typed state
machines rather than an integer state plus callbacks, so an illegal transition is a compile
error where possible and a typed error otherwise; digest authentication (RFC 7616, including
SHA-256) as a pure challenge-response function; registration with re-registration and
qualification loops._

## Alternatives considered

- _Pending._

## Risks & open questions

- Dialog state and media state must stay consistent across re-INVITE and UPDATE without the
  two layers sharing mutable state. The boundary needs deciding before `sipx-call` is built
  on top.
- Whether dialogs are owned by the application or held in a registry inside the UA changes the
  entire API surface.

## Acceptance / done

sipx registers with a third-party proxy, is called by it, answers, and tears down cleanly —
with digest authentication in both directions.
