---
id: M-34
title: Give `collect_digits` a start deadline separate from its inter-digit gap
pillar: Media
status: ready
priority: 4
design: docs/designs/media.md
epic: media
areas: [sipx-media]
note: found by X-40's implementor — `crates/sipx-media/src/session.rs:1117` has the identical one-window shape that made `sipx answer` record zero samples, so the DTMF test is the same flake waiting
---

# Give `collect_digits` a start deadline separate from its inter-digit gap

## Goal
Stop one duration in `collect_digits` from answering two different questions, so a caller that is slow
to send its first digit is not treated as a caller that finished sending.

## Acceptance
- [ ] **The defect is the same one `X-40` proved in production, one layer down.**
      `crates/sipx-media/src/session.rs:1117` uses a single window as both "how long to wait for the
      first digit" and "how long a gap means the digits ended". `X-40` established the consequence
      concretely for the recording path: `sipx answer` produced a valid WAV with **zero samples**,
      `"status":"answered"` and exit **0** when audio started 1.5 s into the call, and `duration_ms: 801`
      against `--duration 10` showed the duration never bounded it. Establish the equivalent here:
      a first digit arriving after the window returns *no digits* rather than waiting for it.
- [ ] **The two durations become two named things**, with the start deadline and the inter-digit gap
      independently settable and independently documented where a caller meets them. `X-28` built
      `record_at_least` for exactly this split on the audio path; say whether that is the shape to
      mirror or why this call needs a different one.
- [ ] **`digits_sent_by_the_caller_are_reported_by_the_answerer` stops being a latent flake.** `X-40`
      identified it as the same cliff, deliberately not fixed there because the cause was in
      `collect_digits` rather than in the test. After this story it should be robust to a slow first
      digit — and the test must still fail if digits genuinely never arrive.
- [ ] **The gap semantics are stated, not implied.** RFC 4733 carries events, not a completion signal,
      so "the digits ended" is a local inference from silence. Say what that inference is and what
      makes it safe: how many events, what timestamp, and what happens to a digit that arrives one
      millisecond after the gap expires.
- [ ] Failing-first test: a `collect_digits` call whose first digit arrives after the current window,
      asserting the digit is collected. It must be red before the split and needs no network — the
      deterministic reproduction `X-40` used for the recording path is the model.

## Progress
- Not started. Filed from `X-40`'s ADJACENT finding 2, which located the shape but was fenced from
  `crates/sipx-media/` because `M-33` held it.

## Notes
- **`X-40` is the reference implementation of the fix and of the proof.** Read its
  `audio_that_starts_late_is_recorded_too` and the table it produced (audio immediately → 3200 samples;
  audio after 1.5 s → 0 samples, both exit 0). The value of that story was showing the symptom is
  deterministic even though it was filed as load-dependent flake — expect the same here.
- **Do not fix this by widening the window.** A larger single window moves the cliff rather than
  removing it, and leaves the same defect for a slower caller. The split is the fix.
- Reads with `X-44`, which proposes a mechanical guard for this whole family, and with `M-33`, which
  settled the neighbouring question of who may close an RTCP reporting interval.
