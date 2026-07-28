---
id: S-16
title: Implement the server side of digest authentication
pillar: Signalling
status: ready
priority: 6
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, sipx-ua]
note: track: auth · RFC 7616 · sipx can answer a challenge but cannot issue one
---

# Implement the server side of digest authentication

## Goal
Let sipx *be* the party that authenticates: mint a nonce, emit a challenge, and verify the
credentials that come back — the mirror of the client-side digest that already ships.

## Acceptance
- [ ] Nonce minting with an opaque, verifiable construction: a server can recognise its own
      nonce, and its expiry, without keeping a table of every nonce it ever issued.
- [ ] `WWW-Authenticate` and `Proxy-Authenticate` challenge emission, and verification of the
      matching `Authorization` / `Proxy-Authorization`, for the MD5 and SHA-256 families
      (RFC 7616, RFC 8760), with `qop=auth`.
- [ ] A bounded replay window over `nonce`/`nc`: a repeated nonce-count is rejected, and the
      window's memory does not grow with traffic. A retransmitted REGISTER — same nonce, same
      `nc` — is *not* a replay and must still authenticate.
- [ ] `stale=true` is emitted when the nonce has expired but the credentials were otherwise
      correct, so a client re-challenges instead of prompting a human.
- [ ] The hash formulas are shared with the client side rather than written a second time.
- [ ] Failing-first test: `a_replayed_nonce_count_is_rejected_but_a_retransmission_is_not`.

## Progress
- Not started. `crates/sipx-ua/src/auth.rs` has the hash formulas and challenge *parsing*;
  nothing mints, emits or verifies.

## Notes
- Scope is the primitives only. *Which* credential a username maps to, and what policy applies to
  a failure, belong to whoever is authenticating — a registrar's credential store is not this
  crate's business.
- Requested by [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s registrar (`RG-2`); see
  [its ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md). The
  primitive/policy split above is the boundary both repos agreed to.
- `S-14` adds the modern algorithm families on the client side; do these in either order, but
  share one formula table.
