---
id: X-40
title: Stop asserting on recorded audio without waiting for it
pillar: Build
status: done
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
- [x] **The recording is waited *for*, with a deadline, not slept past.** Done — **in production, not
      in the test, because that is where the wait lives.** There was never anything for the test to
      poll: `sipx answer` writes the recording exactly once, just before it reports, so by the time an
      assertion can see the file its contents are final. The window that decided whether there was
      anything in it belonged to the answerer, at `crates/sipx-cli/src/answer.rs:106`:
      `timeout(duration, media.record_until_idle(Duration::from_millis(500)))`. `record_until_idle`
      spends that one 500 ms window on two different questions — how long to wait for the stream to
      *start* and how long a gap means it has *ended* — which is exactly what
      `MediaSession::record_at_least` was added to fix in `X-28`, and its own documentation predicts
      this failure verbatim: "The observed result is a recording of **zero** samples — not a degraded
      one". `--duration 10` never bounded the recording; the 500 ms did.
      The cure is `crate::record` (`main.rs:190`), used by both `answer` and `dial`: the first frame is
      bounded by the call's own duration — a bound on failure, so load can only lengthen the wait — and
      the short window is spent only on the gap it can actually measure. "Never arrived" is now an empty
      recording from a call that really carried nothing. Pinned by
      `tests/recording_bounds.rs::audio_that_starts_late_is_recorded_too`, red at the merge base and
      green with the fix.
      The original text follows, and its reading of the *shape* was right even though it put the fix in
      the wrong file. The test waits for
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
      start now has its own ten-second deadline and the gap keeps 600 ms.
      The sweep also reached the *same line* of production code by a second path, and that is fixed
      here too: `recorded.unwrap_or_default()` (`answer.rs:113`, `dial.rs:199`) replaced a partial
      recording with silence whenever the duration cap fired mid-stream — losing the whole recording to
      save none of it. It is an independent defect, so it has its own test,
      `a_recording_cut_short_by_the_cap_is_kept`: six seconds of tone into an answerer that hangs up
      after two, with no 500 ms gap anywhere in the clip, so the cap fires every time. At the merge base
      it recorded 0 samples with `duration_ms: 2006` — audio flowing for the full two seconds, every
      sample discarded.
      Two sites are left deliberately, with the reason at the site or below: `cli.rs:719`'s
      `elapsed() < 12s` is `X-29`'s third category (the clock *is* the measurement — which schedule
      fired — and it separates 3 s from 32 s, so load can only fail it, never pass it wrongly), and
      `digits_sent_by_the_caller_are_reported_by_the_answerer` has the identical cliff to the recording
      one, in `collect_digits` (`sipx-media/src/session.rs:1117`) rather than in the test —
      **left as code, deliberately: `crates/sipx-media/` is held by `M-33`, and the coordinator is
      filing it as its own story.** Two further findings are also getting their own stories rather than
      riding along here: `no_capture_flag_means_no_file` never places a call, so it cannot detect a
      capture written *during* one; and an answerer that reports `heard_audio: false` still exits 0, so
      a script cannot tell a silent call from a good one by exit code.
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
- [x] Failing-first test: **a conventional one turned out to be possible after all, and no load was
      needed.** `crates/sipx-cli/tests/recording_bounds.rs` places its calls from the library instead of
      from a second `sipx` process, for the one reason that matters — the command line has no flag for
      *when* the audio starts, and timing is the only variable load moves. The calls connect, negotiate,
      carry a tone and hang up normally.
      Both defect cases were red at the merge base and are green with the fix, 3/3 runs each, with a
      control case (audio starting at once) passing throughout so the cases differ in timing and nothing
      else:
      `audio_that_starts_late_is_recorded_too` — 0 samples, `duration_ms: 801` against `--duration 10`;
      `a_recording_cut_short_by_the_cap_is_kept` — 0 samples, `duration_ms: 2006` against a 6 s clip.
      The original premise — "not reproducible in isolation" — was true of the *symptom* and false of
      the *defect*, which is the lesson worth carrying: the symptom needed a loaded machine, the defect
      needed a 1.5-second sleep.

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

**Done. Read the next paragraph before anything else in this file, because the story above was filed
against the wrong layer and every heading here depends on that.**

### This was filed as a test-hygiene story. The defect was in production.

`X-40` was written as one of the `X-28`/`X-29` family: a test that asserts on a real-time side effect
after waiting for a different event, to be fixed by making the test wait properly. The *shape* was
diagnosed correctly and the *location* was not. There was nothing in the test to fix — `sipx answer`
writes the recording once, just before it reports, so the file is final before any assertion can see
it. The window that decided whether there was anything in it was in the binary, and two separate
defects sat on one line of it:

```rust
// crates/sipx-cli/src/answer.rs:106, and dial.rs:193, before
timeout(duration, media.record_until_idle(Duration::from_millis(500))).await.unwrap_or_default()
```

Both turn a call that carried audio into a WAV with zero samples, and each is reachable without the
other, so each has its own test. Measured in this worktree, from the library, with timing as the only
variable:

| case | `samples_recorded` | `heard_audio` | `duration_ms` | answerer exit |
|---|---|---|---|---|
| audio at once, `--duration 10` | 3200 | `true` | 964 | 0 |
| audio after 1.5 s, `--duration 10` | **0** | `false` | **801** | **0** |
| 6 s clip, `--duration 2` (cap fires) | **0** | `false` | **2006** | **0** |

**The generalisable lesson, and the reason this correction is worth reading:** the story's own evidence
was what misdirected it. "Observed once under load, not reproducible in isolation, 15/15 alone" reads
like a scheduling race in a test, so the search went to the test. It was a deterministic production bug
the whole time — a 1.5-second sleep reproduces it 3/3 — and the load was only what made a real call's
first frame late. *Not reproducible in isolation* described the symptom, never the defect. A flake
whose trigger is timing is worth one attempt at reproducing it deterministically before it is filed as
test hygiene.

The two remaining rows above are also why the exit status could not have caught this: the answerer
reports `"status":"answered"` and exits **0** having recorded nothing.

### Defect 1 — one window for two questions

`duration_ms: 801` against `--duration 10` is the whole of it. The recording was never bounded by the
duration the caller asked for; it was bounded by `record_until_idle`'s 500 ms, which is *also* the only
window it has for waiting for the **first** frame. Miss it and the loop exits before its first
iteration.

`sipx-media/src/session.rs`'s own "Why this exists (`X-28`)" already describes it — "the first packet
is the one that waits out both jitter buffers filling, and a stalled scheduler opens mid-stream gaps
wider than any packet interval… The observed result is a recording of **zero** samples — not a
degraded one" — and `record_at_least` is the cure it was written for. **`X-28` fixed the library and
left both of its production callers on the old primitive**, which is the part of `X-28` that was never
finished and the reason this recurred as a third instance.

### Defect 2 — `unwrap_or_default` on the cap

Independent of the first, and reached whenever the far end is still talking when the call's time is up:
the outer `timeout` fires and `unwrap_or_default` replaces everything recorded so far with silence.
`duration_ms: 2006` with `samples_recorded: 0` is audio flowing for a full two seconds and every sample
of it discarded — the whole recording lost in order to save none of it.

### The cure

`crate::record` (`main.rs:190`), used by `answer` and `dial` with one shared `RECORD_IDLE`:

- The **first frame** is bounded by the call's own duration — a bound on failure, so load can only
  lengthen the wait, which is `X-29`'s doctrine applied where it belonged all along.
- The **gap** keeps the short window, which is the only question it can answer.
- The cap is enforced *inside*, so there is no timed-out future left for a caller to unwrap, and
  whatever arrived is returned including a recording the cap cut short.

It lives in `main.rs` rather than in either command because both need it and it is exactly the kind of
arithmetic that drifts once written twice — it already was written twice.

One behaviour change falls out, and it is a fix rather than a regression: a call whose far end stays
connected and silent now holds the recording open for its `--duration` instead of giving up after
500 ms, which is what the flag asked for. A far end that hangs up still ends it immediately, because a
closed session is not a gap. `cli.rs` went from 5.0 s to 12.2 s for this reason.

### Landed here

Production (`crates/sipx-cli/src/`):

- `crate::record` and `RECORD_IDLE` in `main.rs`; `answer.rs` and `dial.rs` both use them, and
  `record_until_idle` plus both `unwrap_or_default`s are gone.

Tests (`crates/sipx-cli/tests/`):

- `recording_bounds.rs` — the control, and one test per defect. Both defect cases red at the merge base,
  green with the fix.
- `answerer_exits_cleanly` replaces `let _ = answerer.wait().await` at all four sites, with stderr in
  the message and a bound on the wait.
- The recording assertion in `cli.rs` reads the answerer's own `heard_audio` first, so the failure says
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

This happened while the fix was still out of scope, and it was the evidence that settled the argument:
the diff at that point was entirely inside `crates/sipx-cli/tests/` and the test failed anyway, which is
what a test-side fix can never do anything about. The new message also earned its keep — it named the
cause and attached the answerer's own report, where the old one said "the callee recorded nothing" and
left the reader to guess.

### Unrelated, and red at the merge base

`./scripts/maturity.py --check` and `python3 scripts/test-maturity.py` both fail at `36d0b3f` with this
work stashed — `docs/maturity.md` row `| 2026-07-30 | 15 | 15 | +0 |` regenerates as
`| 2026-07-30 | 15 | 16 | +1 |`, from the stories filed in `60daa1e`. Left alone: it is a generated
shared ledger and not this story's to touch. `X-39` fixes it and is being merged first.
