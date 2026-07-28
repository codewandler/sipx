# Design: Edge / B2BUA

**Status:** deferred — not scheduled · **Pillar:** Application · **Epic:** `edge` ·
**Stories:** _none yet_

## Why

A programmable SIP and media edge — transports, endpoints and routes, with dialog bridging and
selected session-border behaviour — is the natural product built on this stack. It is recorded
here so the layers beneath it are designed with it in mind, and deliberately **not** scheduled:
building an edge on an unproven core is how a stack acquires workarounds it can never remove.

## Approach

_Not designed. The shape, when it comes, is three concepts: transports (listeners), endpoints
(callers and destinations — carriers, PBXs, registered users) and routes (matching on endpoint,
dialled number and prefix), with a registrar and call recording._

## Alternatives considered

- _Not applicable yet._

## Risks & open questions

- Whether this belongs in this repository at all, or as a separate product consuming
  `sipx-call` as a library. Current inclination: separate.

## Acceptance / done

_Undefined. Revisit after M5._
