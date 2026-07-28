---
id: T-10
title: Verify TLS against a real server
pillar: Signalling
status: done
priority: 5
design: docs/designs/sip-transport.md
epic: depth
areas: [sipx-transport]
note: gap left explicitly by T-7
---

# Verify TLS against a real server

## Goal
Register over TLS against Kamailio, so the handshake is verified against an implementation that
did not learn it from sipx.

## Acceptance
- [x] `tests/interop` generates a certificate and configures Kamailio to serve TLS with it.
- [x] sipx registers over TLS and the interop suite asserts it.
- [x] The negative is asserted too: sipx refuses a Kamailio presenting a certificate for the
      wrong name, rather than connecting anyway.

## Progress
- Done. `tests/interop/run.sh` issues a CA and a server certificate for `sipx.test` from the
  same fixture authority the unit tests use (`cargo run -p sipx-testkit --example issue-certs`),
  mounts them into Kamailio, and exports the CA for the tests to trust.
- `registers_against_a_real_server_over_tls`, plus two negatives that a broken stack would not
  pass: a certificate for another name, and an issuer the client does not know. Both must fail
  **immediately** — accepting a timeout would let a hung stack, or a server that never started,
  pass as a refusal. Confirmed non-vacuous by handing each valid input and watching it fail.
- Went further than the story asked: `registers_against_a_real_server_over_websocket` proves
  `T-8` against Kamailio's own WebSocket module, on port 5060 — the same port as SIP-over-TCP,
  which is exactly the arrangement the new pool key exists for.
- Two harness bugs found and fixed, both of the "guard that fires wrongly" kind: Kamailio's
  default `tls_method` pins TLS 1.2 and refuses a ClientHello offering 1.3 (`openssl s_client`
  fails against it too), and under `set -o pipefail` a `docker logs | grep -q` guard reported
  *failure on a match*, because `grep -q` SIGPIPEs the producer.