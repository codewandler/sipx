---
id: X-40
title: Stop asserting on recorded audio without waiting for it
pillar: Build
status: in-progress
priority: 3
design: docs/designs/media.md
epic: conformance
areas: [sipx-cli]
predicate: 3
note: alpha predicate 3, third instance after X-28 and X-29 — `dial_plays_a_file_and_records_the_far_end` waits for the call and then asserts on a real-time side effect, so under load it reads a valid WAV with zero samples; observed once, not reproducible in isolation
---

# Stop asserting on recorded audio without waiting for it

## Goal
Make `crates/sipx-cli/tests/cli.rs:296` fail only when media did not flow, rather than when the machine
was too busy to carry it in time.

## Acceptance
- [ ] **The recording is waited *for*, with a deadline, not slept past.** **Not done, and not doable
      from `crates/sipx-cli/tests/` — the premise is wrong.** There is nothing for the test to poll:
      `sipx answer` writes the recording exactly once, just before it reports, so by the time any
      assertion can see the file its contents are final. The window that decides whether there is
      anything in it belongs to the answerer, at `crates/sipx-cli/src/answer.rs:106`:
      `timeout(duration, media.record_until_idle(Duration::from_millis(500)))`. `record_until_idle`
      spends that one 500 ms window on two different questions — how long to wait for the stream to
      *start* and how long a gap means it has *ended* — which is exactly what
      `MediaSession::record_at_least` was added to fix in `X-28`, and its own documentation predicts
      this failure verbatim: "The observed result is a recording of **zero** samples — not a degraded
      one". So `--duration 10` never bounds the recording; the 500 ms does. See the Progress note for
      the measurement, and `tests/answer_records_late_media.rs` for the test that pins it.
      The remaining work is a change under `crates/sipx-cli/src/`, which this dispatch fenced off.
      The test currently waits for
      the *call* to complete — bounded, 40s/25s timeouts — and then asserts on a real-time side effect:
      that RTP audio accumulated during a 6-second call (`!heard.samples.is_empty()`, then
      `peak > 6000`). Call success does not imply media flowed. Under CPU starvation the media path can
      deliver nothing and the file on disk is a valid WAV with zero samples, which is exactly what was
      observed: `panicked at crates/sipx-cli/tests/cli.rs:296:5: the callee recorded nothing`. Poll for
      a non-empty frame under a deadline, as `X-29` did for the DNS cache — load can then only lengthen
      the wait, and "never arrived" becomes a failure that says so.
- [x] **The answerer's exit status is asserted, not discarded.** Done, at all four sites that
      discarded it, through `answerer_exits_cleanly` (`cli.rs:99`), which also carries the answerer's
      stderr into the failure message. **It does not catch this bug, and that is worth recording:** the
      answerer that records nothing reports `"status":"answered"` and exits **0**. The item is still
      right — it removes an ambiguity from every assertion after the wait — but it is a diagnosis fix
      and not a flake fix. `cli.rs:291` does
      `let _ = answerer.wait().await;`, so "the callee recorded nothing" cannot distinguish silent
      media from an answerer that crashed. This is a diagnosis defect in its own right: it makes the
      failure it does report ambiguous.
- [x] **The whole file is swept for the same shape, not just line 296.** Done, and over the whole
      directory rather than the one file. It found one genuine instance and fixed it:
      `interop_media/mod.rs`'s `echo_round_trip` gave the *first* echoed packet and the gaps between
      packets the same 600 ms window, so a peer that started echoing late left `payload` empty and
      `assert_echo` reported "no audio came back" on a call that carried it — the same
      one-window-for-two-questions defect as `answer.rs`, in the harness instead of the binary. The
      start now has its own ten-second deadline and the gap keeps 600 ms. Two further sites are left
      deliberately, with the reason at the site: `cli.rs:719`'s `elapsed() < 12s` is `X-29`'s third
      category (the clock *is* the measurement — which schedule fired — and it separates 3 s from
      32 s, so load can only fail it, never pass it wrongly), and
      `digits_sent_by_the_caller_are_reported_by_the_answerer` has the identical cliff to the
      recording one, in `collect_digits` (`sipx-media/src/session.rs:1117`) rather than in the test.
      `X-28` cleared the media path
      and `X-29` cleared `sipx-call` and `sipx-transport`; this is the third instance, so the pattern is
      established rather than incidental. Any other assertion in `crates/sipx-cli/tests/` that reads a
      real-time side effect after waiting on something else is the same defect.
- [x] **The assertion still fails when media genuinely does not flow.** Done. `MediaSession::play` was
      temporarily stubbed to accept a clip, report it played and send nothing — a call that connects,
      negotiates and hangs up normally over a silent media path — and
      `dial_plays_a_file_and_records_the_far_end` failed, naming the cause: "the answerer reports it
      heard no audio at all during the call". The sabotage was reverted; `git diff` over
      `crates/sipx-media/` is empty. A deadline-polling test that
      cannot detect a silent media path is worse than a flaky one. Break the media path deliberately and
      show the test failing — `X-36` found a test that was green and could not detect the reversal of
      the invariant it was named for, and that is the failure mode to avoid here.
- [x] Failing-first test: **the substitute exists and is red, and it turned out not to need load at
      all.** `crates/sipx-cli/tests/answer_records_late_media.rs` places the call from the library
      instead of from a second `sipx` process, for the one reason that matters — the command line has
      no flag for *when* the audio starts, and that is the only variable load moves. The call connects,
      negotiates, carries 400 ms of tone and hangs up normally; only the start is delayed by 1.5 s. The
      answerer records **zero** samples, deterministically, every run. Its control case (audio starting
      at once) passes in the same file, so the two differ in timing and nothing else. It is `#[ignore]`d
      because it is red and the fix is out of scope, not because it needs anything to run: un-ignore it
      with the `answer.rs` change and it is that change's regression test.
      The original premise — "not reproducible in isolation" — was true of the *symptom* and false of
      the *defect*.

## Notes
- **Observed once and not reproduced**, which is recorded here deliberately rather than treated as a
  reason to wait. It failed during a gate run while three other worktrees were compiling and the disk
  was ~98% full. The reporting implementor could not reproduce it in isolation and said so plainly
  instead of re-running to green — that is why there is a story rather than a silent retry.
- **The structural argument does not depend on reproducing it.** The test asserts on a real-time
  side effect after waiting for a different event. That is unsound under load whether or not it has
  been caught, and it is the same shape `X-28` and `X-29` closed.
- **Why this is priority 3.** Alpha predicate 3 is "a red gate means a defect. No test in the workspace
  fails because the machine was busy", and it is documented as **load-bearing for the other six**,
  because every predicate is asserted by the gate. A gate that cries wolf invalidates all of them, and
  the pressure it creates — learning to re-run a red step — is the habit the predicate exists to
  prevent. `X-39` is the same predicate failing from the other direction, where a step is red for a
  reason that is not a defect at all.
- Reads with `X-28` (media path), `X-29` (the rest, and the deadline-polling shape to copy) and `X-36`
  (a green test that asserted nothing).

## Progress

**Blocked on `crates/sipx-cli/src/answer.rs`, which this dispatch fenced off. The diagnosis is
finished and measured; the cure is four lines someone else owns.**

### What this actually is

The story located the defect in the test and reasoned about it structurally. The reasoning was right
and the location was wrong. Measured, in this worktree, by placing the call from the library so that
the audio's start time is the only variable:

| audio starts | `samples_recorded` | `heard_audio` | `duration_ms` | answerer exit |
|---|---|---|---|---|
| immediately | 3200 | `true` | 964 | 0 |
| after 1.5 s | **0** | `false` | **801** | **0** |

`duration_ms: 801` is the whole story. The answerer was given `--duration 10` and stopped recording
after 0.8 s — so the recording is not bounded by the duration the test asks for, it is bounded by
`record_until_idle`'s 500 ms, which is also the only window it has for waiting for the *first* frame.
Miss it and the loop exits before its first iteration: a valid WAV with zero samples,
`"status":"answered"`, exit 0. That is the observed failure exactly, and no flag the test can pass
widens that window.

`sipx-media/src/session.rs`'s own "Why this exists (`X-28`)" already describes it — "the first packet
is the one that waits out both jitter buffers filling, and a stalled scheduler opens mid-stream gaps
wider than any packet interval… The observed result is a recording of **zero** samples — not a
degraded one" — and `record_at_least` is the cure it was written for. `X-28` fixed the library and left
its only production caller using the unsound primitive.

### The fix, for whoever picks this up

In `crates/sipx-cli/src/answer.rs:106` and `crates/sipx-cli/src/dial.rs:193`, separate the two
questions: wait for the *first* frame under a bound that is a bound on failure (the call's `duration`
is the natural one), then use the 500 ms idle gap only to decide the stream has ended.
`record_until_idle` cannot express that, so either give `MediaSession` a
`record_until_idle_starting_within(start, idle)` or inline the two-phase loop. Note the second defect
while there: both call sites do `recorded.unwrap_or_default()`, so a fired `timeout` throws away
**everything recorded so far** rather than keeping a short recording — a partial recording is turned
into no recording at all. `collect_digits` (`session.rs:1117`) has the identical one-window shape and
`digits_sent_by_the_caller_are_reported_by_the_answerer` rides on it.

Then un-ignore `audio_that_starts_late_is_recorded_too` in
`crates/sipx-cli/tests/answer_records_late_media.rs`; it is that change's regression test and is red
today.

### Landed here (all inside `crates/sipx-cli/tests/`)

- `answerer_exits_cleanly` replaces `let _ = answerer.wait().await` at all four sites, with stderr in
  the message and a bound on the wait.
- The recording assertion now reads the answerer's own `heard_audio` first, so the failure says
  whether the media path delivered nothing or the file was written wrong. The old message,
  "the callee recorded nothing", could not tell those apart.
- `interop_media/mod.rs`'s `echo_round_trip` no longer gives the first echoed packet and the
  inter-packet gaps the same 600 ms window.
- `cli.rs:719`'s clock assertion is left, with the reason at the site.

### The symptom reproduced, in a real gate run

The story records it as observed once and not reproducible. It reproduced here, on the second of two
full `./scripts/gate.py` runs — in the `test` step, where `cargo test --workspace` has many test
binaries running at once, which is the load the first report described. The first gate run was green in
that step, so the rate is roughly one in two on this machine rather than the 15/15 the original report
measured.

```
test dial_plays_a_file_and_records_the_far_end ... FAILED
panicked at crates/sipx-cli/tests/cli.rs:511:5:
the answerer reports it heard no audio at all during the call, so the recording has nothing in it to
assert on: {"status":"answered",...,"duration_ms":801,"samples_recorded":0,"heard_audio":false,...}
```

`duration_ms: 801` — the same 801 the deliberate 1.5 s delay produces, against `--duration 10`. The
answerer gave up at 0.8 s in both cases, because 0.8 s is where its window closes no matter how late
the audio is. That is the diagnosis confirmed against the real failure and not only against the
reproduction.

Two things follow. The new message is doing its job: it named the cause and attached the answerer's own
report, where the old one said "the callee recorded nothing" and left the reader to guess. And the
flake is **not fixed** — this whole diff is inside `crates/sipx-cli/tests/`, and the test still fails,
which is the same conclusion the first Acceptance item reaches by reading the code.

### Unrelated, and red at the merge base

`./scripts/maturity.py --check` fails at `36d0b3f` with my work stashed — `docs/maturity.md` row
`| 2026-07-30 | 15 | 15 | +0 |` regenerates as `| 2026-07-30 | 15 | 16 | +1 |`, from the stories filed
in `60daa1e`. Left alone: it is a generated shared ledger and not this story's to touch.
