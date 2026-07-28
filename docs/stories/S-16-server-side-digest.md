---
id: S-16
title: Implement the server side of digest authentication
pillar: Signalling
status: done
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, sipx-ua]
note: M7 · RFC 7616 · sipx can answer a challenge but cannot issue one
---

# Implement the server side of digest authentication

## Goal
Let sipx *be* the party that authenticates: mint a nonce, emit a challenge, and verify the
credentials that come back — the mirror of the client-side digest that already ships.

## Acceptance
- [x] Nonce minting with an opaque, verifiable construction: a server can recognise its own
      nonce, and its expiry, without keeping a table of every nonce it ever issued.
- [x] `WWW-Authenticate` and `Proxy-Authenticate` challenge emission, and verification of the
      matching `Authorization` / `Proxy-Authorization`, for the MD5 and SHA-256 families
      (RFC 7616, RFC 8760), with `qop=auth`.
- [x] A bounded replay window over `nonce`/`nc`: a repeated nonce-count is rejected, and the
      window's memory does not grow with traffic. A retransmitted REGISTER — same nonce, same
      `nc` — is *not* a replay and must still authenticate.
- [x] `stale=true` is emitted when the nonce has expired but the credentials were otherwise
      correct, so a client re-challenges instead of prompting a human.
- [x] The hash formulas are shared with the client side rather than written a second time.
- [x] Failing-first test: `a_replayed_nonce_count_is_rejected_but_a_retransmission_is_not`.

## Progress
- Done. `Authenticator` mints, challenges and verifies; `Presented` parses what came back. The
  primitive/policy split the notes describe is expressed in the signature: `verify` takes the
  password as an argument, so the credential store never enters this crate.
- **Nonces are self-describing, so there is no table of issued nonces.** Each is
  `<issued-at>.<HMAC-SHA-256 over it and the realm>`: the MAC makes it unforgeable, the timestamp
  makes expiry checkable, and the realm is in the MAC so a nonce issued for one protection space is
  not accepted in another. HMAC rather than `H(secret ‖ message)`, which with a Merkle–Damgård hash
  is extensible by anyone who has seen one output.
- **The formula is not written twice.** Verification builds a client-side `Challenge` and calls
  `auth::respond` — the same function a sipx client uses to answer. A server with its own copy of
  the formula drifts from its own client and then rejects correct credentials.
- **The replay window tells a replay from a retransmission**, which is the story's failing-first
  test and the part that is easy to get wrong in both directions. Rejecting every repeated `nc`
  fails authentication whenever a UDP packet is duplicated; accepting every repeat is no replay
  protection at all. The response digest separates them: same count and same digest is one request
  seen twice, same count and a different digest is a captured credential aimed at a different
  request. Both directions are mutation-tested.
- The window is bounded at 4096 nonces, oldest evicted. A client whose nonce is evicted is
  challenged again with `stale=true` and retries by itself — a round trip. An unbounded window is
  an outage with a protocol in front of it.
- **The digest is checked before the clock**, so a wrong password on an expired nonce is a
  rejection rather than a `stale`. Answering `stale=true` there tells an attacker the only thing
  wrong with their guess was its timing.
- SHA-256 is the default rather than MD5. A *server* is the only party that can make that choice —
  a client can only answer what it is asked — and RFC 8760 exists because MD5 should not be the
  only thing on offer.
- `Reason::Mismatch` deliberately does not distinguish "no such user" from "wrong password". The
  caller has that distinction; the far end is not entitled to it.

## Notes
- Scope is the primitives only. *Which* credential a username maps to, and what policy applies to
  a failure, belong to whoever is authenticating — a registrar's credential store is not this
  crate's business.
- Requested by [sipx-clstr](https://github.com/codewandler/sipx-clstr)'s registrar (`RG-2`); see
  [its ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md). The
  primitive/policy split above is the boundary both repos agreed to.
- `S-14` adds the modern algorithm families on the client side; do these in either order, but
  share one formula table.
