---
id: P-13
title: Prove the complete diagnostic phone from a shell
pillar: Phone
status: done
priority: 11
design: docs/designs/phone.md
epic: phone
areas: [sipx-cli, interop, docs]
announcement: [2, 3, 5]
note: executable proof complete; combined full gate green
---

# Prove the complete diagnostic phone from a shell

## Goal

Make the phone epic's exit criterion one reproducible matrix rather than a collection of lower-layer
claims.

## Acceptance

- [x] One bounded runner executes `DPH-1` … `DPH-12` and emits a checked matrix with requested and
      negotiated paths.
- [x] Real-network cases cover all five signalling transports, G.711 and Opus, plain RTP, SDES,
      DTLS-SRTP, early media, authenticated INVITE and an ICE NAT case.
- [x] Two independently implemented peers cover every signalling transport the public README claims.
- [x] Device evidence uses a virtual loopback on Linux; no test requires a human or a fixed sleep.
- [x] The public CLI reference is generated/checked against `--help` and the JSON schema.
- [x] The full gate is green with default, no-default and all feature sets.

## Progress

- `scripts/diagnostic-phone-proof.py --run` now executes all twelve command-process vectors under a
  finite failure bound and prints the requested and observed path. The latest complete local run
  passed every vector, including the scenario header/action cases and deterministic Linux virtual
  device loopback.
- The two independent peer profiles exercise UDP, TCP, TLS, WebSocket and secure WebSocket from the
  same shared test list. WSS has its own verified TLS-plus-upgrade registration test; a complete
  container-backed run passed both profiles on all five transports.
- Positive Opus selection now enters through `sipx --codec opus`, reports Opus on both processes and
  carries audio using the negotiated packet size. `sipx dial --early-media` consumes and PRACKs a
  reliable provisional answer, records its media before the fixture permits the final answer, and
  reports the measured early samples. Both are separate product-path cases in the runner.
- `scripts/check-cli-reference.py` now builds the default command and executes root plus all seven
  subcommand help paths. It holds their commands and long options against the public page, discovers
  all three versioned CLI schemas/envelopes and their literal structural fields from the Rust
  producers, and checks the public contract table. Six reversed fixtures prove an undocumented or
  over-documented flag, field or contract is red. The checker is a diagnostic-proof product path and
  has matching local-gate and CI steps.
- The proof runner's fixture tests and structural `--check` are local-gate and CI steps. The root
  full gate passed all 30 steps on the combined release tree, including clippy, default/no-default
  feature configurations, the MSRV build and the public documentation build.
