---
id: M-34
title: Give `collect_digits` a start deadline separate from its inter-digit gap
pillar: Media
status: done
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
- [x] **The defect is the same one `X-40` proved in production, one layer down.**
      `crates/sipx-media/src/session.rs:1117` uses a single window as both "how long to wait for the
      first digit" and "how long a gap means the digits ended". `X-40` established the consequence
      concretely for the recording path: `sipx answer` produced a valid WAV with **zero samples**,
      `"status":"answered"` and exit **0** when audio started 1.5 s into the call, and `duration_ms: 801`
      against `--duration 10` showed the duration never bounded it. Establish the equivalent here:
      a first digit arriving after the window returns *no digits* rather than waiting for it.
- [x] **The two durations become two named things**, with the start deadline and the inter-digit gap
      independently settable and independently documented where a caller meets them. `X-28` built
      `record_at_least` for exactly this split on the audio path; say whether that is the shape to
      mirror or why this call needs a different one.
- [x] **`digits_sent_by_the_caller_are_reported_by_the_answerer` stops being a latent flake.** `X-40`
      identified it as the same cliff, deliberately not fixed there because the cause was in
      `collect_digits` rather than in the test. After this story it should be robust to a slow first
      digit — and the test must still fail if digits genuinely never arrive.
- [x] **The gap semantics are stated, not implied.** RFC 4733 carries events, not a completion signal,
      so "the digits ended" is a local inference from silence. Say what that inference is and what
      makes it safe: how many events, what timestamp, and what happens to a digit that arrives one
      millisecond after the gap expires.
- [x] Failing-first test: a `collect_digits` call whose first digit arrives after the current window,
      asserting the digit is collected. It must be red before the split and needs no network — the
      deterministic reproduction `X-40` used for the recording path is the model.

## Progress

Implemented on `impl/M-34`. `MediaSession::collect_digits` takes two durations instead of one.

### The defect, measured

`X-40`'s method, with the arrival time as the only variable, in `crates/sipx-media` over loopback
sockets — no second process and no load needed. At the merge base (`576f0dd`), against the same
1 s window:

| case | collected |
|---|---|
| digits sent at once (`a_sequence_of_keypresses_arrives_in_order`) | `"1234"` |
| the same digits sent 2 s later | **`""`** |

Empty, not short — the same all-or-nothing shape as the zero-sample WAV, and for the same reason:
the loop ends before its first iteration, so nothing that arrives afterwards is ever read.

### The split

`collect_digits(within, gap)`, mirroring `crate::record(call, within, idle)` — `X-40`'s cure, which
is the precedent this follows rather than a second answer to the same question:

- **`within`** bounds the wait for the first digit, and with it the whole collection. A bound on
  failure: `sipx answer` passes the call's own duration, so a caller cannot be slower than the call.
- **`gap`** keeps the only question a fixed window can answer — has the caller stopped dialling.
  `answer` keeps its 800 ms, which now does that job alone.
- The cap is enforced inside, so `answer`'s `timeout(duration, …).unwrap_or_default()` is gone with
  it. That was `X-40`'s second defect on the same line: every digit collected, discarded at the
  moment the cap fired.

**Not `record_at_least`'s count wait**, and the reasoning is in `docs/designs/media.md`: a counted
wait for five digits cannot see a sixth, so `assert_eq!(collected, "1234#")` would stop failing when
a keypress is reported twice — and the production caller has no count, because a keypad's length is
not known in advance. Where no count exists, `X-28`'s own remedy applies: keep the wall clock for
the question it can answer, set past any scheduling delay.

### The gap semantics

Stated in `collect_digits`' own documentation and in the design record. A digit is delivered once,
when the first packet carrying that tone's end bit arrives; the tone is identified by its RTP
timestamp, constant across its packets, so RFC 4733 §2.5.1.3's end retransmissions are absorbed and
`44` is told from one long `4`. An elapsed gap therefore means no keypress *completed* in it, never
that a packet was lost mid-tone. A digit arriving a millisecond late is not dropped — it stays
queued and opens the next collection — but it is in the wrong collection, and no window can fix
that, which is why an application that knows its digit count should stop at that count with
`recv_digit`.

### Landed

- `crates/sipx-media/src/session.rs` — the split, its documentation, and two tests:
  `a_first_digit_that_arrives_late_is_still_collected` (red at the merge base, `""` against
  `"1234"`) and `a_collection_with_no_digits_at_all_ends_empty`, which holds the other half: no
  digits still ends, still bounded, still empty.
- `crates/sipx-cli/src/answer.rs` and `main.rs` — `DIGIT_GAP` beside `RECORD_IDLE`, the call's
  duration as the first-digit bound, and the `unwrap_or_default` removed.
- `crates/sipx-call/tests/call.rs` — `a_call_carries_dtmf_digits` on the two named bounds.
- `docs/designs/media.md` — the digit case under the two-questions rule.

`digits_sent_by_the_caller_are_reported_by_the_answerer` is fixed from production, not from the
test: it drives `sipx answer`, whose first-digit window was the cliff, and `crates/sipx-cli/tests/`
is untouched by this diff.

## Notes
- **`X-40` is the reference implementation of the fix and of the proof.** Read its
  `audio_that_starts_late_is_recorded_too` and the table it produced (audio immediately → 3200 samples;
  audio after 1.5 s → 0 samples, both exit 0). The value of that story was showing the symptom is
  deterministic even though it was filed as load-dependent flake — expect the same here.
- **Do not fix this by widening the window.** A larger single window moves the cliff rather than
  removing it, and leaves the same defect for a slower caller. The split is the fix.
- Reads with `X-44`, which proposes a mechanical guard for this whole family, and with `M-33`, which
  settled the neighbouring question of who may close an RTCP reporting interval.
