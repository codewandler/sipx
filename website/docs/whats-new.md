---
title: What's new
description: Release highlights and adoption notes for the sipx 1.0.0-beta.3 prerelease.
---

# What's new

<!-- BEGIN generated:release-heading -->
## 1.0.0-beta.3 — 2026-08-04
<!-- END generated:release-heading -->

The first public beta is published and remains immutable. Beta.3 is
published as exact crates.io packages from a new immutable tag without moving or overwriting
beta.2. Its runtime library and CLI
surface is unchanged from beta.2; the release adds the checked public stack comparison, a
demand-led capability roadmap, and checksum-bound recovery for interrupted registry publication.
See [How sipx is built](reference/development-process.md) for the measured process and
[Diagnostic-phone proof](reference/diagnostic-phone-proof.md) for the executable product matrix.

Install the exact CLI release with:

```bash
cargo install --locked --version =1.0.0-beta.3 sipx-cli
```

The adoption surface leads with the modular Rust crates: applications select the protocol, transport,
user-agent, media, call, or host layer they need rather than taking a facade crate. The `sipx` CLI is
the shell-testable proof of those layers. This beta covers all five released signalling
transports, selectable codec/media-security/ICE policy, optional live devices, interactive scenarios,
bounded load, and reliable early media. The application host now serves real calls through
document-mode webhooks and authenticated full-duplex sessions; the latter can originate calls when
granted.

Public APIs are not frozen before 1.0. Supported APIs receive a changelog entry and migration
guidance when they break. Experimental APIs may change shape or be removed without a migration
note; that includes the language-neutral `sipx.app.v1` wire contract and SIP over QUIC.

The release intentionally does not provide a proxy, registrar, PBX, routing product, dial plan,
TURN relay, graphical desktop phone, headset mixer, video, arbitrary application codecs, complete
browser-media stack, automatic live-state presence, or SIP instant-message behavior. It also does
not ship an embedded application runtime, subprocess binding, or packaged TypeScript SDK. The
two-dialog coupling primitive is public, but the truly off-media relay role is not complete. The
[fit guide](guides/does-this-fit.md) is the maintained deployment boundary.

## 1.0.0-beta.2 — 2026-08-04

Beta.2 was the first published public beta. It established the same endpoint, library, transport,
media and application-host surface described above, backed by exact registry packages, the
installed diagnostic CLI, independent transport peers and release-commit documentation.

```bash
cargo install --locked --version =1.0.0-beta.2 sipx-cli
```

Use beta.3 for new installations; beta.2 remains immutable for reproducible existing consumers.

## 1.0.0-alpha.5 — 2026-08-03

This previous tagged release established a measured alpha baseline for the SIP,
transport, call, and media stack; it is not an API-stability promise. Breaking API changes are
still possible before 1.0.

Install this exact release with:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --tag v1.0.0-alpha.5 --locked sipx-cli
```

### Release highlights

- **Breaking: public error enums are now `#[non_exhaustive]`.** Downstream exhaustive matches need
  a wildcard arm. This is the one-time compatibility cost that lets future diagnostic variants be
  additive instead of breaking every caller. The sole exhaustive exception is a closed set of host
  boundaries, with that reason maintained beside the type.
- **Every published crate has its own landing page.** All eleven packages now ship a README that
  says what the crate is, points to its crate-level stability contract, and names the layer or
  responsibility it deliberately leaves elsewhere.
- **The release surface is measured from five directions per crate.** The guard now compares each
  package README's lead paragraph with the manifest description, crate documentation, and both
  public crate tables: 55 front doors in total. Packaging tests also prove Cargo ships every
  README, rather than merely finding the file in a checkout.

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

## Changes after the beta

This website is built from `main`, so a page or API link may describe work newer than the tagged
beta. Use the exact crates.io version when reproducibility matters, and consult the
[complete changelog](https://github.com/codewandler/sipx/blob/main/CHANGELOG.md) before updating a
Git revision. Unreleased behavior is not part of `1.0.0-beta.3` merely because it appears on this
site.
