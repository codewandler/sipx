---
id: X-45
title: Make `no_capture_flag_means_no_file` able to observe the thing it is named for
pillar: Build
status: ready
priority: 4
design: docs/roadmap.md
epic: conformance
areas: [sipx-cli]
note: found by X-40's implementor — the test kills the answerer immediately and never places a call, so it cannot detect a capture file being written *during* a call; the X-36 shape
---

# Make `no_capture_flag_means_no_file` able to observe the thing it is named for

## Goal
Make the test that proves capture stays off actually exercise the path that would write a capture file,
so it can fail when the flag stops being honoured.

## Acceptance
- [ ] **The test cannot currently detect the defect it guards.** `no_capture_flag_means_no_file` in
      `crates/sipx-cli/tests/` kills the answerer immediately and never places a call. Capture is written
      *during* a call, so the assertion passes whether or not the flag is honoured. Demonstrate that
      first: make `sipx answer` write a capture file unconditionally and show the test still green. That
      demonstration is the story's justification, in the same way `X-36` verified its reversal by doing
      it and reading the compiler error.
- [ ] **The rewritten test places a real call and then asserts no file exists.** Signalling has to cross
      the wire, because that is what a capture would record. Absent a call the assertion is vacuous.
- [ ] **The positive case is asserted too, and is what makes the negative meaningful.** A test that only
      proves "no file when off" passes trivially if capture is broken entirely. Pair it: with the flag on
      a file appears and contains captured signalling; with it off no file appears. `X-18` shipped the
      capture feature, so both directions are reachable.
- [ ] **The sweep question is answered, not assumed.** `X-40` established that
      `crates/sipx-cli/tests/` had one genuine instance of the wait-for-the-wrong-thing shape and two
      defensible ones. This is a *different* shape — a test whose subject is never exercised — so sweep
      for that one specifically: any test asserting the absence of a side effect without running the code
      that would produce it.
- [ ] Failing-first test: this story's failing-first evidence is the sabotage above (capture written
      unconditionally, test still green), since a test that cannot fail has no red state to show. Record
      it as such rather than manufacturing a conventional red.

## Progress
- Not started. Filed from `X-40`'s ADJACENT finding 3.

## Notes
- **This is the `X-36` shape**, and `X-36` is the precedent for how to close it: there, a test named
  `respond_returns_only_once_the_response_has_been_sent` could not observe the reversal it was named for,
  and the answer was to make the guarantee structural so reversing it became a compile error. Consider
  whether anything similar is available here, or whether an honest test is the whole of the fix.
- Reads with `X-44` (a mechanical guard for a related family of untrustworthy tests) and `X-18`, which
  built the capture feature this test is supposed to be guarding.
- The general lesson worth carrying: a green test proves nothing about a path it never enters, and the
  three instances found so far (`X-36`, `X-39`'s zero-guard test, this one) suggest the class is worth
  looking for deliberately rather than incidentally.
