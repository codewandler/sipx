# Design: real-time call-audio analysis

**Status:** proposed · **Pillar:** Media · **Epic:** `call-audio-analysis` · **Stories:** M-57,
M-58, M-59, M-60, M-61, X-106, A-29

## Why

Applications need small, predictable facts about live audio even when no speech model is enabled:
when voice starts and ends, whether the signal is silent or clipping, and what level reached the
call. These facts belong in deterministic frame processing, not in a model provider and not in the
RTCP network-quality snapshot. Keeping this epic separate lets a small CPU-only build use them
without loading the local-speech runtime.

## Approach

`M-57` specifies a sans-I/O processor: bounded PCM frames plus explicit sample rate, sequence and
discontinuity inputs produce typed observations. It reads no clock, socket or device and owns no
task. Time thresholds are sample counts derived from the declared rate, so a fixture produces the
same events on every machine. The processor consumes `M-54`'s shared call-media seam.

`M-58` adds voice activity with start, end and hangover transitions and carries those transitions
through `CallEvent` and the application SDK. `M-59` exposes level, energy, clipping and silence
windows without duplicating `M-10`'s RTP loss, jitter, round-trip and MOS statistics. `M-60` adds
bounded calibration and threshold adaptation whose state, reset rules and limits are observable and
deterministic.

`M-61` handles hostile audio and isolation: extreme amplitudes, impulses, DC, alternating samples,
format changes, discontinuities and long silence cannot panic, allocate without bound or starve the
call. Raw audio is not retained by default, and analysis state belongs to one call. `X-106` owns the
corpus and measurements for false positives, false negatives, start/end latency, CPU and memory.
`A-29` publishes a runnable live-call example using only the supported call and SDK surfaces.

## Boundaries

- `M-10` remains the network/media transport quality surface. This epic reports signal content, not
  packet delivery quality.
- RFC 4733 DTMF already has a typed call event, so this epic does not add a second DTMF detector.
  General tone classification, echo cancellation and answering-machine classification stay out
  until a separate requirement and measurement corpus justify them.
- The processor may inform synthesis ducking through typed activity state, but it cannot reach into
  a speech provider or mutate call media by itself.
- No microphone/device I/O enters `sipx-audio`, `sipx-media` or `sipx-call`; `P-10`'s leaf-driver
  boundary remains unchanged.

## Exit

A deterministic corpus produces stable voice-activity and signal-metric events through the
application SDK, adaptive thresholds remain bounded under adversarial input, measured CPU, memory,
latency and error rates meet the predeclared profile, cancellation leaves no processor state, and a
runnable example proves the behavior on a live call. This is a separate M16 epic from local speech
and remains outside M13.
