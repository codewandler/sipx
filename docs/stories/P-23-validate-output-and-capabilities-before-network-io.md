---
id: P-23
title: "Validate output and capabilities before network I/O"
pillar: "Phone"
status: in-progress
epic: diagnostic-automation
areas: [sipx-cli]
design: docs/designs/diagnostic-automation.md
note: "follow-up external review finding 9 · version ignores JSON and unavailable media features are reported only after a peer answers"
---

# Validate output and capabilities before network I/O

## Goal

Apply global output promises to every command and reject build-time capability gaps before binding
or signalling. The answer from an unreachable peer must not decide whether the local binary can
honor the requested codec, security mode or profile.

## Acceptance

- [x] Parser/output specs define the `version` text and JSON result and one validation phase that
      completes all build-capability checks before resolver, bind or peer I/O.
- [ ] Failing-first process tests prove `version --json` currently emits plain text and an
      unavailable codec against an unreachable target currently reports timeout instead of the
      local feature refusal.
- [ ] `version --json` emits one stable JSON object through the common report builder; plain
      `version` retains its existing one-line human output and both reject stray positionals.
- [ ] Codec, media-security, profile, ICE and device selections validate their compiled capability
      before destination resolution, transport bind, file/device open or datagram emission.
- [ ] The preflight is shared by `dial`, `answer`, `load`, `load-responder` and scenario wherever a
      selection is available; command-specific timing cannot reintroduce a peer-dependent verdict.
- [ ] Text/JSON failures retain usage exit 2 and name the missing feature. A local observer proves
      zero network bytes for every unavailable selection.
- [ ] Parser, feature-matrix, output-contract and process tests plus the complete repository gate
      are green.

## Review evidence

The follow-up review piped `version --json` into a JSON consumer and received plain text. On the
feature-minimal binary, an unavailable codec produced an excellent typed refusal only when a peer
answered; an unreachable peer hid the local incompatibility behind the invitation timeout.

## Progress

- The diagnostic-phone contract now fixes version's two result forms and defines one I/O-free
  capability phase shared across command selectors, including its feature matrix, error mapping and
  DPH-18/19 process vectors. Board regeneration and the complete gate remain deferred to push.
