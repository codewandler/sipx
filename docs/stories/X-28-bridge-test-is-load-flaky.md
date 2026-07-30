---
id: X-28
title: Make the bridge audio test deterministic under load
pillar: Build
status: done
design: docs/designs/media.md
epic: conformance
areas: [sipx-media, tests]
predicate: 3
note: found by M-25 — it races play against a fixed 400ms record on real sockets and records zero samples under load, so it will be blamed on innocent diffs
---

# Make the bridge audio test deterministic under load

## Goal
Stop `audio_played_into_one_call_is_heard_on_the_other` failing for reasons that have nothing to do
with the change under test, so a red gate means what it says.

## Acceptance
- [x] `crates/sipx-media/tests/bridge.rs::audio_played_into_one_call_is_heard_on_the_other` passes
      deterministically while the machine is loaded — concretely, while several other gates are
      compiling concurrently, which is the condition under which it was observed to fail.
- [x] The failure mode is understood and named before it is fixed. It records **zero of 3200
      samples**, not a degraded count, which is a different thing from "a bit slow" and the story
      should say which of the two it actually is.
- [x] The fix does not weaken what the test asserts. Loosening the sample threshold until it passes
      would leave a test that no longer proves audio crossed the bridge — the point of it.
- [x] Any sibling test racing a fixed wall-clock duration against real-socket work is found by the
      same sweep and named, fixed or explicitly left with a reason. `record_until_idle(400ms)`
      against `play` is a shape, not a one-off.
- [x] Failing-first evidence: the test failing under artificial load, quoted from a real run.

## Progress

### The cause

`MediaSession::record_until_idle(idle)` is one duration spent on two different jobs:

```rust
while let Ok(Some(frame)) = tokio::time::timeout(idle, self.recv()).await { … }
```

The *first* iteration's window is "how long to wait for the stream to **start**". Every later
one is "how long a gap means the stream has **ended**". Neither is a property of the audio; both
are properties of how fast the machine happens to be. The test then hands that duration the job
of deciding whether a bridge works.

Both windows are exceeded by a pipeline that is merely slow, and the first one is exceeded
first, because the first packet is the one the pipeline delays the most:

- `alice`'s send loop paces on a 20 ms `interval` (`session.rs:1645`), so a clip is not on the
  wire when `play` is called — `play` returns once the clip is *queued*.
- Each leg has its own jitter buffer, `jitter_depth: 3` growing adaptively to
  `jitter_max_depth: Some(12)` (`session.rs:311-312`). Twelve packets is 240 ms of deliberate
  delay, *per leg*, and a loaded machine is exactly the arrival jitter that makes it grow.
- A bridge is two legs, so both buffers fill before Bob hears anything, behind two 20 ms pacers.

Measured on this machine, time from `play` to Bob's first frame:

| condition | first frame | recorded |
|---|---|---|
| idle | 81 ms | 3200 of 3200 |
| one core contended 60× | 150–273 ms | 3200 of 3200 |
| one core contended 250× | never, or 87–230 ms | **0**, or 160–3200 |

The 400 ms window was never a large margin over 81 ms; it was about five times a number that
moves by four under load.

### Zero, or a degraded count?

The story asks which it is. **Zero — and zero is a structurally different failure from a short
recording, not a worse one.** Once the first frame lands, the rest follow at the packet rate, so
a 400 ms idle gap is never reached again until the clip genuinely ends: the recording is
all-or-nothing by construction. `0 of 3200` means *recording never began*.

The degraded counts are real too and they are a second, rarer mode: the recorder started, then
the scheduler stalled long enough to open a mid-stream gap wider than 400 ms and the recorder
concluded the far end had hung up. Twelve runs under load, the reported signature dominating:

| result | runs |
|---|---|
| `0 of 3200` — never started | 6 |
| `320` / `640` / `1280 of 3200` — started, cut short | 4 |
| passed | 2 |

Both modes are the same root cause. Neither is "a bit slow": the pipeline delivered every one of
the 3200 samples in every run — the recorder had stopped listening.

### The fix

A caller that already knows how much audio it played is not asking "has the far end gone quiet".
It is asking "did it all arrive", and that question has no wall-clock answer in it.
`MediaSession::record_at_least(samples, within)` (`session.rs:1227`) waits for a **count**, with
`within` as a bound on failure rather than a window to measure in — ten seconds against clips of
under half a second. A slow machine now takes longer and reaches the same verdict.

`record_until_idle` is untouched and stays public: it is correct for the callers whose question
genuinely is "has the far end stopped talking" — `sipx-cli`'s `dial` and `answer`, which record
a far end whose length nobody knows.

**No assertion was weakened.** `audio_played_into_one_call_is_heard_on_the_other` still asserts
`heard.len() > clip.len() / 2` and `loudest > 4000`, character for character. Only the wait
changed. Several assertions got *stronger* as a side effect of draining by count — noted below
where that happens.

### Falsification

`audio_played_into_one_call_is_heard_on_the_other`, 12 runs, pinned to one core against 250
spinners pinned to the same core:

```
=== run 12 FAILED ===
thread 'audio_played_into_one_call_is_heard_on_the_other' panicked at crates/sipx-media/tests/bridge.rs:67:5:
Bob should have heard most of what Alice played: 0 of 3200
FAILED 10 of 12
```

After the fix, same load, same 12 runs: `FAILED 0 of 12`. Raised to 600 spinners: `FAILED 0 of
10`. The `sipx-media` session unit tests under the same 250× load went from 9, 7 and 3 failures
in three consecutive runs to `31 passed; 0 failed` three times.

### The sweep

`record_until_idle` had 46 call sites. Classified by whether a slow machine can make the
assertion fail:

**Converted to `record_at_least` — 30 sites.** Every one already knew its expected count.
`sipx-media`: `session.rs` ×16, `bridge.rs` ×4, `srtp.rs` ×2, `opus.rs` ×1. `sipx-call`:
`call.rs` ×5, `mute.rs` ×5, `playback.rs` ×1, `secure_media.rs` ×2, `wss.rs` ×1. Three of those
were `let _ =` drains whose *side effect* was load-bearing — `packets_are_counted_on_both_sides`
asserts `packets_received() == 5` and the discarded recording is what gives the five packets
time to land.

**Two drains were doing worse than nothing.** `unmuting_a_session_restores_the_audio`
(`session.rs`) and its `sipx-call` twin drain a muted stretch, then play again and compare
against the source. A short drain leaves silence in the channel for the *second* recording to
pick up, so the failure reads "unmuting did not restore the audio" — a lie about the code under
test. Both now drain by count.

**Left on `record_until_idle`, deliberately — 7 sites.** Each asserts its recording is *empty*:
`bridge.rs:270`, `session.rs` ×3, `call.rs:214`, `mute.rs` ×2. A fixed window is a window to
look in rather than a deadline to beat, and load can only make them pass. Waiting by count for
samples that must never arrive would be a ten-second sleep apiece.

**Left, with the window widened instead — 2 sites**, both because no count exists:

- `playback.rs::a_clip_queued_while_another_is_stopping_starts_within_the_bound`. How much the
  far end hears *is the measurement* — bounded below by the reply and above by the stop bound —
  so there is nothing to count to. "The far end stopped talking" is the only end this recording
  has, which is what `record_until_idle` is for. Its gap went from 400 ms to 2 s: a hundred
  consecutive missed 20 ms packet intervals, past any scheduling delay.
- `events.rs::a_recording_reports_the_duration_of_what_it_captured`. The idle window is the
  *subject* — it asserts the timeout is not counted as recorded audio — so it cannot move to a
  counted wait. Same 500 ms → 2 s, and both assertions survive: a duration that wrongly counted
  the window would be `spoken + idle`, still over it.

**The two production sites are correct as they are** — `sipx-cli`'s `dial.rs:168` and
`answer.rs:105` record a far end whose length is unknown. That is the question
`record_until_idle` answers.

**`conference.rs` had the same shape under a different name** and the sweep would have missed it
by grep: `record_for(session, 600ms)`, a fixed wall-clock recording window against a real mixer,
with `peak(&heard) > 3000` on the other side. Under load it returns few frames or none, `peak`
falls to zero, and the test reports that a participant cannot hear the conference. Now
`record_mixed(session, count)`. A conference sends continuously, which is why waiting for a gap
was never an option there — and is exactly why waiting for a count terminates for *every*
participant, the ones asserted to hear silence included.

### Found by the sweep and NOT fixed

The sweep turned up a second, larger family that this story did not touch: **a fixed
`tokio::time::sleep`, then an assertion that a SIP message or a socket read has arrived.** It is
the same disease — wall clock standing in for a happens-before — but the cure is different
(poll-until-condition, not wait-for-count), and it is spread across `sipx-call` and
`sipx-transport` rather than the media path this story owns. Worth its own story:

- `call.rs:382` (300 ms → ACK and BYE), `:769` and `:823` (400 ms → CANCEL), `:996` (300 ms →
  INVITE), `:568/:591/:601` (200 ms → re-INVITE applied), `:683` and `:1240` (150 ms → remote
  sequence advanced, which the *next* assertion depends on being stale).
- `events.rs:233` (100 ms ordering a raw 180 before a raw 200).
- `transport/tests/udp.rs:473` — a **50 ms** bound on a positive socket read, the tightest in
  the workspace.
- `transport/tests/backpressure.rs:91` (300 ms → one request through).
- `media/tests/quality.rs:69/100/226` and `session.rs:3162/3439` — hand-sent packets with a
  100–300 ms drain before asserting exact `cumulative_lost` / `extended_highest_sequence`.
- `playback.rs:184/225` — `packets_received() == settled` inside a 600 ms sleep.
- `playback.rs:100` `hearing()` — a 2 s deadline loop on `packets_received() != 0`; generous,
  but a positive-arrival deadline rather than a bound on failure.

## Notes
- Found by `M-25` during a gate run with several worktrees compiling concurrently. It passed 3/3
  standalone immediately afterwards and stayed green in every subsequent run, which is what makes
  it dangerous: **it will be blamed on whichever diff happens to be in flight.** `M-25` had to
  prove it was not its own change by showing `bridge.rs` contains no reference to `srtp`, `dtls` or
  `Srtp` at all.
- This is a real-socket, wall-clock test: `play` racing `record_until_idle(400ms)`. Under load the
  recorder's idle window elapses before any audio arrives.
- **Priority 4 because a flaky gate step is worse than a missing one.** A test that fails at random
  trains everyone to re-run the gate instead of reading it, which is how a genuine regression gets
  waved through — and this project has already paid once for a CI signal nobody was watching (see
  `AGENTS.md` on the MSRV job that was red through two releases).
