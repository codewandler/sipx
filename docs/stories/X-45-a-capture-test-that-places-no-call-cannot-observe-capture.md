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
- Filed from `X-40`'s ADJACENT finding 3.
- **Done.** `no_capture_flag_means_no_file` now places a real call and asserts over a directory.

### The justification, measured

The old test was blind for **two** independent reasons, not the one filed:

1. It killed the answerer the instant it announced its port, so no signalling ever flowed and there
   was nothing a capture could have recorded.
2. It watched `<scratch>/signalling.pcapng` — a path **never passed to the process**. A capture
   nobody asked for is given no path, so it can only fall back to a name compiled into the binary,
   which is never the test's scratch path. The assertion could not have fired for any defect.

Both were demonstrated rather than argued, with a two-part sabotage:

- **A** — `apply_capture` (`crates/sipx-cli/src/main.rs`) made unconditional, falling back to a bare
  relative `"signalling.pcapng"`: the flag is ignored and every run captures.
- **B** — `Capture::start` (`crates/sipx-transport/src/capture.rs`) made to create the file lazily on
  the first record, so a capture exists only *during* a call. This is what isolates reason 1.

Results, all in one worktree with its own target directory:

| test | sabotaged | restored |
|---|---|---|
| old `no_capture_flag_means_no_file` | **green** (0.00 s — nothing happened) | green |
| rewritten `no_capture_flag_means_no_file` | **red**: `wrote [".../unasked/signalling.pcapng"]` | green |
| `the_capture_flag_records_the_signalling_of_a_call` | green (sabotage did not break capture) | green |
| probe: directory assertion but *no call* | **green** | — |

The last row is the point of the story: the directory assertion alone does not observe the defect.
Placing the call is what makes it visible. And while the old test sat green, sabotage A left a real
`crates/sipx-cli/signalling.pcapng` on disk, written by the neighbouring test's caller — the defect
was live in the same test binary and this test still passed.

### The rewrite

- `place_a_call(dir, answerer_args)` is factored out of the positive test and shared by both, so the
  pair cannot drift into disagreeing about what a call is. It runs both processes in `dir`.
- The negative test runs the **positive control first** (same call, same machinery, `--capture` on,
  file must contain an `INVITE`), then the same call with the flag off, and asserts the directory the
  processes ran in is **empty** — not that one pre-chosen path is absent. Known edge, written at the
  call site: a compiled-in *absolute* default would still escape.
- Every wait is causal (port announced, caller exits, result line, answerer exits). No sleep was
  added (`X-28`/`X-29`).

### The sweep

`crates/sipx-cli/tests/` swept for the specific shape *asserting the absence of a side effect
without running the code that would produce it* — every negative assertion in all five test files
(`cli.rs`, `interop_call.rs`, `interop_srtp.rs`, `recording_bounds.rs`, `interop_media/mod.rs`) was
traced to whether its path is exercised. **Two** genuine instances, not one:

- `no_capture_flag_means_no_file` — this story, fixed.
- **`verbose_logging_stays_off_stdout` (`cli.rs:647`) — still open.** It runs
  `dial sip:bob@example.com --json -vv`, which is refused as a usage error in `dial.rs` *before any
  socket is bound*, and the CLI itself emits no log events (only the library crates do, and only
  after a socket opens). So not one log record is produced, and the assertion `stdout.is_empty()`
  would hold identically if `init_logging` wrote to stdout — the exact defect it is named for. It
  does not even assert the exit code, so nothing pins the path it silently depends on. **Worth its
  own story.**

Everything else is defensible: `an_unknown_command_is_a_usage_error_on_stderr` and its siblings
assert an empty stdout *after* the binary genuinely performed the refusal;
`a_valued_flag_before_the_uri_is_not_mistaken_for_it` backs its negative with an asserted exit 5;
`recording_bounds.rs` and `interop_srtp.rs` already carry prose about this very trap.

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
