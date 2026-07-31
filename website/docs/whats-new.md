---
title: What's new
description: Release highlights for sipx 1.0.0-alpha.4 and guidance on the newer main-branch documentation.
---

# What's new

<!-- BEGIN generated:release-heading -->
## 1.0.0-alpha.4 — 2026-08-01
<!-- END generated:release-heading -->

This is the current tagged release. It establishes a measured alpha baseline for the SIP,
transport, call, and media stack; it is not an API-stability promise. Breaking API changes are
still possible before 1.0.

Install this exact release with:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --tag v1.0.0-alpha.4 --locked sipx-cli
```

### Release highlights

- **No breaking changes.** This release is additive: new counters, a new CLI flag behaviour that
  was documented but inert, and four measurement defects closed.
- **Two milestones are delivered, on evidence rather than on mechanism.** The previous release
  checked M10 and M12 against their evidence and claimed neither. **M10 — Reachable** is now
  delivered: a call placed at one instance's GRUU is answered by that instance with audio both
  ways while the other registration, equally current, never sees the INVITE. **M12 — Provable**
  is delivered: its last clause needed every discard counted and exportable, which reached only
  one crate and was false outside the process.
- **Every discard in the signalling path is counted, and the numbers come out.** Widening the
  enumeration beyond one crate exposed **sixteen** unexplained discard sites where a hand census
  had found seven. `UnsentCounts` counts, by method, every request the endpoint tried to put on
  the wire and could not — so a failed BYE on a teardown path is finally a number an operator
  asking "why did that call linger" can read. `sipx --counters <FILE>` writes on every path out
  of the command, not only the successful one, because the run that fails is the run the bug
  report is about.
- **`-vv` reaches DEBUG.** It was documented, accepted and inert: verbosity counted arguments
  beginning with `-v`, so `-vv` counted as one and yielded INFO — and nothing on a call's path
  logged at INFO, so the documented flag produced no output at all.
- **The fixed-sleep rule is enforced rather than swept for.** A wall-clock duration may bound a
  failure or define silence; it may not stand in for a happens-before. That had been swept for
  twice and enforced by nothing, so two fresh violations landed after the second sweep. The
  first enforced run found 30 clock-decided assertions and 2 that said which; two were real
  defects and are now causal waits. There is no suppression list under any name.
- **Both RFC torture corpora are tamper-evident, from the gate and from CI.** Each is recovered
  from its RFC's own appendix rather than transcribed, and the check re-recovers and diffs it —
  the only thing that can tell a fixture edited by hand from the RFC's own bytes, since the
  suites read whatever is in the directory and pass. One corpus was checked only inside a job
  that never runs locally; the other was checked nowhere at all.
- **A gate that could not reach the network says so instead of reporting a defect.** An
  unreachable RFC editor used to print `1 of 25 steps failed` naming a corpus — a step that never
  read the archive claiming the committed messages had drifted. It now disclaims its own run, and
  a disclaimer never outranks a genuine finding beside it.

#### Still current from earlier alphas

- **`MediaSession::collect_digits` takes two durations** (breaking in `1.0.0-alpha.3`). It took
  one, and spent it on two different questions — how long to wait for the first keypress, and how
  long a silence means the caller has stopped. Pass the old value as both arguments to keep the
  old behaviour, including the defect. `sipx answer` consequently holds a call for its full
  `--duration` when nobody dials, which is what `--duration` is documented to mean.
- **`sip-tls.md` no longer advertises a minimum-TLS-version setting.** There was no such setting;
  the specification was corrected rather than the setting invented, because the absence of a
  version-selecting API is what currently evidences the TLS floor.
- **A call can use ICE.** An application selects host gathering or a configured STUN server through
  one call-level media policy; the default selects no ICE and is unchanged. A call between two
  endpoints whose advertised addresses do not reach each other completes over a checked candidate
  pair.
- **An ICE session survives a restart.** A re-offer whose credentials have both changed begins a new
  session, and audio keeps flowing on the previously selected path until the new one is chosen.
  Every later offer and answer on a call using ICE now restates its ICE attributes, so a hold or a
  session refresh no longer reads to the far end as ICE having been switched off.
- **The diagnostic phone can select every released signalling transport.** `dial`, `answer` and
  `register` choose UDP, TCP, TLS, WS or WSS through one fail-closed policy; a secure URI scheme
  cannot be served over cleartext, and a certificate failure is reported rather than downgraded.
- **The published compliance and maturity reports are trustworthy again.** Two continuous-integration
  jobs had been reporting a discrepancy in the maturity report that did not exist, because they
  measured a repository history they had not fetched.
- **Media startup is transactional.** A media session or conference is returned only after its
  configuration and codecs have been validated. Startup failures are typed errors, and no worker
  or socket is left behind.
- **Transport resource limits cover live work.** Connection eviction now terminates the evicted
  connection, and unauthenticated TLS and WebSocket handshakes share a finite per-endpoint budget
  and deadline.
- **Invalid runtime settings fail before binding.** Zero channel capacities, connection or
  handshake limits, handshake deadlines, WebSocket keepalives, and media worker intervals are
  rejected as configuration errors rather than reaching a panic or dead worker.
- **Conference shutdown owns its workers.** Removing a participant, closing a conference, or
  dropping it initiates cleanup of participant collectors, media sessions, and sockets.
- **Codec construction cannot change the negotiated codec.** An Opus setup failure is reported
  instead of substituting G.711 bytes under the negotiated Opus payload type.
- **Media statistics are observational.** Reading current statistics no longer resets the RTCP
  reporting interval; sending a report is the operation that closes that interval.
- **CLI recordings retain received audio.** `sipx answer` and `sipx dial` wait separately for the
  first frame and for later idle, and a duration limit preserves samples recorded before it fires.
- **RFC support claims require implementation evidence.** Implemented and partial entries in the
  generated compliance table cite workspace source, so prose alone cannot make a support claim.

The alpha also includes the shipped user-agent surface described throughout this site: calls,
registration, G.711 audio, optional Opus, RTP/RTCP, secure library transports, SDES-keyed SRTP,
and the scriptable WAV-based CLI.

### Not complete in this release

ICE cannot use a relay: host and server-reflexive candidates are gathered, TURN is not implemented,
so the NAT pairs that need a relayed path are not served. A DTLS-keyed call path is still not
available, and the CLI cannot yet select codecs, media security or ICE. The CLI uses WAV files
instead of sound devices.
The experimental `sipx-host` process can bind and
answer calls, but application callback bindings are not implemented.

## Changes on `main`

This website is built from `main`, so a page or API link may describe work newer than the tagged
alpha. Use the tag above when reproducibility matters, and consult the
[complete changelog](https://github.com/codewandler/sipx/blob/main/CHANGELOG.md) before updating a
Git revision. Unreleased behavior is not part of `1.0.0-alpha.4` merely because it appears on this
site.
