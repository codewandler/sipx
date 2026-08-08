---
id: A-39
title: Build the speech provider contract skeleton
pillar: Application
status: done
priority:
design: docs/designs/local-speech.md
epic: local-speech
areas: [app-sdk, speech, m16]
predicate:
announcement:
note: A-25 specified it and nothing implements it · the registry, session types and selection order every later speech story needs
---

# Build the speech provider contract skeleton

## Goal

Turn `A-25`'s specification into compiling, testable types: the provider registry, the recognition
and synthesis session contracts, the discovery descriptor and the endpoint-default versus per-call
selection order — with no speech provider behind any of it. This is the thing `A-28`, `M-55` and
`M-56` were all written against and that does not exist.

## Acceptance

- [x] The recognition and synthesis session contracts from the specification exist as public types,
      with the lifecycle events kept disjoint from SIP failure types exactly as the spec requires.
- [x] A provider registry accepts registrations and resolves a provider by identity. The discovery
      descriptor carries identity, offline status, languages, voices, formats and devices.
- [x] Endpoint-default and per-call override precedence is implemented with the spec's typed refusal
      order. A failing-first test proves each refusal is distinguishable, and that an unknown or
      unavailable provider is refused before any call resource is taken.
- [x] The specification's conformance vectors run against a deliberately inert in-repo test provider
      that implements the contract and recognizes nothing — proving the contract is executable
      without implying a capability. **31 of §10's 34 vectors run here.** REC-7 and LIF-6 are
      obligations of §2's asynchronous driver, which this story deliberately does not build, and
      `A-40` owns them; the test file records the exclusion rather than leaving it to be noticed.
- [x] No speech recognition or synthesis implementation, model, accelerator dependency or audio
      retention ships in this story, and the public documentation does not present speech as an
      available capability.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from the rc.4 readiness audit. `A-25` shipped a specification and nothing else:
  there is no provider registry, session type or speech logging anywhere under `crates/*/src`. That
  left `A-28` unimplementable — three of its acceptance rows have nothing to run against — and the
  same gap sits under `M-55` and `M-56`. This story is the missing predecessor.
- 2026-08-08: the skeleton landed as `sipx_media::speech` — `docs/specs/speech-providers.md`'s §3
  descriptor and registry, §4 selection state machine, §5/§6 session contracts, §7 lifecycle and §8
  bounds, one module per section. It is marked `**Experimental** (`A-8`)` and no application reaches
  it; nothing in it recognises or synthesises anything.

  **Where it lives.** The spec's own crate plan (§ header) puts the contract types and the selection
  state machine in `sipx-media`, next to the seam that carries their audio, and that is where they
  are. `sipx-sip` and `sipx-sdp` are untouched, as §2 requires.

  **The seam.** `recognition_inputs(PcmFrame)` is the whole of the contract's reach into call media:
  it turns one frame from `M-54`'s tap into the ordered §5 inputs, break first. There is no second
  tap and no second queue — `SpeechBounds::input_frames` and `Processing::DEFAULT_QUEUE_CAPACITY`
  are asserted to be the same number, so the seam's drop-oldest policy *is* §5's input bound.

  **Refused before the call is touched.** `Selected::processing` is the only producer of a seam
  request, so an unknown, off-host or incompatible provider cannot reach `attach_processor`. The
  failing-first test walks all six evaluation steps against a live `MediaSession` and then takes all
  eight seam attachments; any refusal that had allocated a queue would make the eighth fail.

  **Vectors.** 31 of §10's 34 run in `crates/sipx-media/tests/speech.rs`, from outside the crate and
  against inert providers. `REC-7` and `LIF-6` do not, and that is recorded in the file rather than
  left to be noticed: both are obligations of §2's *driver*, the asynchronous shell this story
  deliberately does not build — the unconsumed-output bound and its coalescing, and the drain
  deadline's aborted `Stopped`. `REC-3`, the third of that family, *is* run, because the queue it
  bounds is the seam's and the seam exists.

  **One spec edit.** §3 said a voice carries "declared properties" and said nothing more about them,
  which is two readings and two different types. `docs/specs/speech-providers.md` §3 now states that
  they are stable lowercase tokens opaque to selection — consistent with §4, which has no step that
  reads them. §9's extensibility record was re-verified against the real public API, as `A-25`'s
  progress note asked the first implementing story to do: `Device` and `Resources` are built one
  capability at a time rather than through positional constructors, so a new field stays additive.

- 2026-08-08: closed against a green gate — `./scripts/gate.py` reported **40 steps, all green** on `main` at `1256b8e`. An earlier run on the same tree failed two `sipx-cli` audio tests; both pass in isolation and in the full 83-test `cli.rs` binary, and `M-59` independently hit the same class on a sibling test while five other checkouts were building on this shared box. `X-118` owns that flakiness.

## Notes

- Normative contract: `A-25`'s specification under `docs/specs/`. Do not restate or reinterpret it
  here; if the spec is ambiguous, fix the spec.
- `M-54` owns the bounded PCM attachment seam. This story must consume that seam rather than open a
  second tap into call media.
- Ordering: `A-39` → `A-28` (isolation and retention policy) → `M-55`/`M-56` (actual providers) →
  `A-26`/`A-27` (SDK lifecycle).
