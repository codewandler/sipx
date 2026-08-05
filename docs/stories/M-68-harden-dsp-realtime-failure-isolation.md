---
id: M-68
title: Harden DSP real-time and failure isolation
pillar: Media
status: backlog
priority: 40
design: docs/designs/custom-call-dsp.md
epic: custom-call-dsp
areas: [sipx-media, dsp, security, realtime, m18]
predicate:
announcement:
note: after M-63/M-64 · measured budgets and explicit fail-open/fail-closed policy
---

# Harden DSP real-time and failure isolation

## Goal

Prove and harden M-64's execution/failure policy so hostile audio and failed isolated processors
cannot panic, retain stack-owned audio or starve call media, while stating the trusted-native
boundary honestly.

## Acceptance

- [ ] The M-63/M-64 policy is hardened with measured frame CPU/allocation budgets, maximum
      consecutive misses, recovery/reset rules and stable fail-open/fail-closed events.
- [ ] Impulses, full-scale alternating samples, long silence, DC, discontinuity, rate/channel changes,
      invalid output length and processor error cannot panic or corrupt another call/direction.
- [ ] Deadline misses, bypasses, dropped/coalesced frames, resets and terminal failures have typed
      counters/events distinct from intentional glitch effects.
- [ ] Proven inline and supervised-isolated processors cannot retain stack-owned borrowed frames,
      grow stack-owned state past declaration, spawn unowned stack work or keep a call/graph alive
      after cancellation; isolated processes are terminated and reaped.
- [ ] Bounded stress tests prove RTP remains serviced through isolated worker hangs, crashes,
      malformed results and deadline misses, and all owned work drains through an event/barrier
      rather than a fixed sleep.
- [ ] Cooperative-native APIs/docs state that arbitrary trusted code cannot be preempted and may copy
      audio or spawn work; conformance never upgrades that profile to the containment claim.
- [ ] Strict lint/feature, fixed-sleep, adversarial corpus and the full gate are green.

## Progress

- Backlog. Hardening gate after M-63 and M-64.
