---
id: S-33
title: Decide what `sipx answer` should exit with when it heard no audio
pillar: Signalling
status: done
priority: 5
design: docs/roadmap.md
epic: sip-core
areas: [sipx-cli]
note: found by X-40's implementor — the answerer reports `heard_audio: false` and still exits 0, so a script cannot distinguish a silent call from a good one by exit code
---

# Decide what `sipx answer` should exit with when it heard no audio

## Goal
Make the `sipx` binary's exit code carry whether the call actually worked, so a script driving it can
tell a silent call from a successful one without parsing JSON.

## Acceptance
- [x] **The current behaviour is stated and reproduced.** `sipx answer` completes a call in which no
      audio arrived, reports `"heard_audio": false` in its JSON, and exits **0**. `X-40` reproduced the
      silent case deterministically (audio starting 1.5 s into the call yielded 0 samples,
      `"status":"answered"`, exit 0). A caller checking `$?` cannot tell that apart from a call that
      carried 3200 samples.
- [x] **The decision is made explicitly and written down, because it is a contract change, not a bug
      fix.** Silence is not obviously an error: a call can legitimately carry no audio, and a command
      that exits non-zero for a legitimate outcome is its own defect. State the chosen rule and the
      reasoning — candidates include exiting non-zero only when audio was *expected* (a `--record` or
      `--expect-audio` flag was given), introducing a distinct exit code for "call completed, no media",
      or leaving 0 and documenting that the JSON is the only source of truth. Rejecting the change with a
      recorded reason is an acceptable outcome for this story.
- [x] **Whatever is chosen applies to `dial` as well as `answer`, or says why not.** Both commands can
      complete a call having carried no media, and a rule that holds for one and not the other is the
      kind of per-command patch `S-32` was filed against.
- [x] **The behaviour is documented in `website/docs/reference/cli.md`** alongside the other exit-status
      and usage-error rules, so the contract is discoverable rather than folklore.
- [x] Failing-first test: a `--bin sipx` test that answers a call in which no audio flows and asserts the
      chosen exit status. Name it, and show it red before the change. If the decision is to keep exiting
      0, the test asserts *that*, and the failing-first evidence is instead the documentation check.

## Progress
- Filed from `X-40`'s ADJACENT finding 4.
- Decided 2026-08-03: keep exit 0 for a call that established and completed, even when it received
  no audio. Silence does not establish failure: a quiet caller and a one-way announcement are valid,
  and `--record` requests capture rather than declaring that non-empty input is required. Making
  received media decide the process status would therefore impose application policy the command
  was not given. Scripts that require inbound media use the existing `heard_audio` JSON field.
- The rule is shared without exception by `dial` and `answer`. The binary regression
  `a_completed_silent_call_is_success_for_dial_and_answer` establishes one silent loopback call,
  gives both processes `--record`, asserts exit 0 for both, holds both terminal reports to
  `samples_recorded: 0` and `heard_audio: false`, and reads both empty WAVs back. Its initial form
  passed before the documentation change, reproducing the current zero-exit contract.
- Failing-first evidence is
  `the_silent_call_exit_contract_is_documented_for_both_commands`: before the CLI reference was
  updated, it failed with “the CLI reference does not state the shared dial/answer exit rule for a
  silent call.” The reference now states the rule, its reasoning, and how a script asserts audio.
- Focused verification is green: both named silence tests, all 28 binary CLI tests, the sipx-cli
  all-features clippy target at `-D warnings`, the app-surface check, and the docs-link check.

## Notes
- **Deliberately filed as a decision rather than a fix.** `X-40` surfaced it while fixing a recording
  defect and correctly declined to take it as a rider — an exit-code contract deserves deciding on its
  own merits, not as a side effect of a test-hygiene change.
- Reads with `X-40`, which fixed the underlying cause of the silence (one window serving as both a start
  deadline and an end-of-stream gap), and with `S-32`, which is the precedent that a CLI rule must be
  fixed at the shared helper rather than in one command.
- The `heard_audio` field already exists and is honest, so this story is only about the exit code and its
  documented meaning — no new observability is needed.
