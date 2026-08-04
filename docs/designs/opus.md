# Design: Opus product support

**Status:** implemented and proven through calls, both CLI directions, normalized packages and an
independent peer; exact registry acceptance belongs to the beta release · **Pillar:** Media · **Epic:** `opus` ·
**Stories:** `M-13`, `M-30`, `M-39`

## Scope

This epic is the whole Opus adoption path, not another codec implementation. It connects RFC 6716
encoding and decoding, RFC 7587 payload negotiation, call-level choice, CLI reachability, positive
audio evidence, optional-feature packaging, and an independently implemented peer.

Optional RFC 7587 format parameters such as `maxaveragebitrate`, `useinbandfec`, `usedtx`, `cbr`, and
the stereo pair remain an explicitly documented extension point. They are not required for the
current mono codec path and must not be implied by the epic closing.

## Decisions already made

- `M-13` owns the stateful encoder/decoder and dynamic payload mapping. The native codec dependency
  stays behind an off-by-default `opus` feature.
- `M-30` owns call selection and feature propagation into `sipx-call`; the G.711 pair remains the
  default and an Opus request in a build without the feature fails before network I/O.
- `M-31` owns one parsed format identity for the SDP answer and media setup, so a dynamic payload's
  encoding name, 48 kHz clock and channel count cannot be accepted by one layer and rejected by the
  other.
- `P-9` maps the CLI's ordered codec policy to that call policy. `M-39` strengthens `P-13`'s original
  product check: two command processes exchange distinguishable 48 kHz signals in both directions,
  and the recordings assert clock, duration and far-end signal identity.
- `M-37` owns construction failure: a failed Opus encoder or decoder returns a typed setup error and
  never substitutes G.711 bytes under the negotiated dynamic payload type.

Those are completed constraints. The tracker links them and does not duplicate their tests.

## Product evidence

Both CLI roles read WAV structure before signalling, then validate its sample rate against the
codec the established media session actually negotiated before queuing a sample. Playback uses the
session's packet size—960 samples for the 20 ms Opus path—and recordings carry its 48 kHz clock.
The bidirectional process proof distinguishes the far end's tone from the local one and retains the
default G.711 path's 8 kHz/160-sample contract.

The independent-peer matrix now carries Opus-only calls in both offer/answer roles. Each role sends
a distinct 48 kHz signal through the peer's decoder and encoder, then requires a dynamic payload
type, the 48 kHz RTP clock and non-silent, signal-correlated samples from sipx's decoder. Exact codec
sets on both sides make G.711 unable to satisfy the case. The feature matrix covers Opus on
`sipx-audio`, `sipx-media`, `sipx-call`, and `sipx-cli`; the CLI is checked empty, Opus-only,
device-audio-only and with both optional features. The normalized-archive proof below now builds and
runs a clean local package consumer. Installing the exact published registry bytes remains the
distinct `A-12` release acceptance.

## Packaged feature proof

The package-boundary proof operates on Cargo's normalized archives, not on workspace manifests.
It creates the public workspace archives with verification disabled, safely extracts only regular
files below each package prefix into a temporary directory, and patches the registry names to those
extracted packages. This patch is only the local stand-in for publication: every dependency edge
and feature declaration being exercised came from the archive a registry consumer receives.

The proof has four assertions:

1. `sipx-audio`, `sipx-media`, `sipx-call`, and `sipx-cli` all build with defaults disabled and with
   their Opus feature selected; the CLI is also built with Opus and device audio selected together.
2. The normalized manifests retain the complete forwarding chain
   `sipx-cli/opus` → `sipx-call/opus` and `sipx-media/opus` → `sipx-audio/opus` → the optional
   native binding.
3. The default packaged CLI graph contains neither that binding nor its FFI package, while the
   Opus graph contains both. Off by default is therefore a resolved-graph fact, not only prose.
4. A clean temporary package consumer builds and runs `sipx --help` with `--features opus` and a
   freshly generated lockfile. The run is bounded and owns the Cargo process group so interruption
   cannot leave a compiler behind.

This does not stand in for `A-12`'s registry proof. Publication still has to build and install the
exact crates.io bytes. It closes the earlier gap where workspace inheritance and path dependencies
could make a feature work in-tree while its normalized package manifest was unusable.

The public library guide also carries the deployment boundary: the native dependency, the
permissive Cargo licence policy, the narrowly scoped advisory exception, and the fact that neither
the library nor a shipped application enables Opus by default. Optional RFC 7587 format parameters
remain absent regardless of package success.

## Exit

`M-39` closes the epic. Rate- and direction-correct CLI media, normalized feature-off and
Opus-enabled package consumers, and Opus-only independent-peer calls in both SIP roles are all
present and bounded. Exact crates.io installation remains `A-12`'s release-distribution proof; it
verifies the published instance of this completed feature rather than adding another Opus behavior.
