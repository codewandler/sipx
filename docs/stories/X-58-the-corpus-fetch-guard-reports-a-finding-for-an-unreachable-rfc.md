---
id: X-58
title: Make an unreachable RFC editor a non-result rather than a finding, and delete the false reason for the guard
pillar: Build
status: in-progress
priority: 3
epic: conformance
areas: [scripts, ci]
note: found by the independent review of X-56 — the fetch guard exits 1, so gate.py reports `1 of 25 steps failed` when the network is down, which is the exact confusion X-34 removed; and the rationale written into AGENTS.md for the guard is disproved by one command
---

# Make an unreachable RFC editor a non-result rather than a finding, and delete the false reason for the guard

## Goal
Make the two corpus steps say "this run is not a result" when they cannot reach the RFC, the way
`X-34` made the disk guard say it, and remove the false premise that was written into `AGENTS.md`
to justify them.

## Acceptance
- [x] **An unreachable RFC editor exits `2`, not `1`.** `scripts/gate.py` defines the contract at
      `:663-665` — `0` green, `1` the tree is wrong, `2` the run is not a result — and it exists
      because "a full disk and a broken diff used to leave the same exit code and print the same way,
      so nothing could tell a finding from a non-finding". Today a failed fetch exits 1, so
      `gate.py` appends `("rfc 5118 corpus", "exit 1")` and prints `gate: N of 25 steps failed`: a red
      gate that reads as a corpus that drifted. Either the importers exit with a code the gate reads
      as infrastructure, or `infrastructure_evidence` (`scripts/gate.py:758`) learns the fetch
      failure's shape — decide which and say why. The two mechanisms are not the same: a distinct
      exit code is the script's own claim, and a pattern in `infrastructure_evidence` is the gate's
      inference about a step it does not control.
- [x] **The failing-first test is a fetch that cannot succeed**, not a string match on the source.
      Point the importer at a host that cannot resolve — or otherwise force the fetch to fail —
      and assert what `gate.py` *reports*: the summary line and the exit code, not a sentence in the
      streamed output. That is where `X-34` put the property and it is the half the current test
      never reaches.
- [x] **Replace the shape assertion with a behavioural one.** `scripts/test-gate.py:1667-1677`
      requires the fetch line to `startswith("if ! curl")`, which pins spelling rather than
      behaviour. Review proved it both ways: a guard whose body is `then true; fi` — no message, no
      exit, falling through to base64-decode a file that does not exist — **passes**, while an
      equivalent `curl … || { echo …; exit 1; }` **fails**. Assert the two properties the docstring
      claims instead: the failure says which host could not be reached, and it never becomes a skip.
- [x] **Delete the false reason, everywhere it was copied.** *(`CHANGELOG.md:71-73` excepted — a
      fenced file the coordinator writes at integration; the wording it needs is in Progress.)* `AGENTS.md:128-129` says "`curl -f`
      prints nothing and a bare exit code reads as a corpus that changed". The flags in use are
      `-fsSL`, and `-S` is *show errors*: `curl -fsSL <bad-url> -o /tmp/x` prints
      `curl: (22) The requested URL returned error: 404` and exits 22. The guard may still be worth
      having — it turns curl's exit code into a sentence naming the corpus and the host — but it must
      be justified by what it actually does. The same premise is repeated in `scripts/gate.py:136-138`,
      both `scripts/import-rfc*-corpus.sh` guard comments, `CHANGELOG.md` and `X-56`'s Progress.
      **`AGENTS.md` is the file every future agent reads as the why**, and a why that one command
      disproves is the defect this project keeps filing stories about.
- [x] **Correct "the gate's only checks that reach the network".** *(same `CHANGELOG.md` exception.)* `AGENTS.md:118` claims it, and
      `scripts/build-docs.sh:111-113` runs `npm ci`/`npm install` whenever `website/node_modules` is
      absent — which is every fresh implementor worktree, since it is gitignored. The `docs site`
      step has always reached the network. Same overclaim in `CHANGELOG.md` and in `X-56`.
- [x] **An unrecognised flag must not take the write path.** `[[ "${1:-}" == "--check" ]] && check_only=1`
      (`import-rfc5118-corpus.sh:31`, `import-rfc4475-corpus.sh:25`) means `--check=1`, `-check` or a
      typo silently selects rewrite mode, which would overwrite a tampered fixture with the RFC's own
      bytes and exit 0 — a green step that erased the evidence. Pre-existing, but `X-56` added four
      invocation sites, which is what makes it worth closing now. Refuse an unknown argument.

## Progress
- Filed 2026-07-31 by the independent review of `X-56`, which reproduced every item above in scratch
  copies rather than reasoning about them: the no-op guard passing the suite, the `||` form failing
  it, curl's actual output at the actual flags, and `gate.py`'s classifier returning `None` for the
  fetch failure so the step lands in the red tally.
- **The decision the first item asked for: a distinct exit code, not `infrastructure_evidence`.**
  The importers are ours, so the disclaimer is a claim they are entitled to make about their own
  run; `infrastructure_evidence` would be `gate.py` inferring it from a step's *text*. Three things
  settle it. The pattern would have to match curl's wording for no route, no DNS, a timeout, a
  proxy refusal and a 404, in whatever locale and curl version the machine has — a regex over
  another program's prose, which is the spelling-not-behaviour mistake this story exists to remove.
  `INFRASTRUCTURE_SHAPES` is global, so any pattern loose enough to catch "could not fetch" would
  also excuse a `cargo` step that happened to print it. And `infrastructure_evidence` means "stop
  the run", which is right for a vanished `target/` and wrong here: an unreachable host says
  nothing about `cargo clippy`.
- **The exit code is `75`** (`EX_TEMPFAIL`, sysexits(3)), not `2`: under `set -e`, `tar`, `diff` and
  `grep` all exit `2` for real trouble and would hand the gate a disclaimer no script meant to make.
  Steps opt in with `Step.not_a_result`, so the number is only read that way from scripts this
  repository owns. `gate.py` still exits `2` — the doctrine's number — when a step disclaims.
- **A disclaimed step does not end the run, and does not outrank a real finding.** With something
  genuinely red alongside it the gate exits `1` and prints `1 of 2 steps failed` naming only the
  red one, with the disclaimer above under its own heading. Exiting `2` there would tell an
  implementor to re-run instead of to look, which is the disease `X-34` named rather than the cure.
  This is the one place the design departs from the disk guard's shape, and deliberately.
- **Failing-first, at the merge base `f5c18a1`:** `scripts/test-gate.py` with a `curl` on `PATH`
  that answers `curl: (6) Could not resolve host: www.rfc-editor.org` and exits 6 — 20 failures and
  1 error, the load-bearing one being `gate: 1 of 1 steps failed / rfc 5118 corpus: exit 1` at
  exit code `1`. After: `gate: NOT A RESULT — these steps could not reach what they check, and did
  not fail`, exit code `2`. The stub is why the suite still runs offline and costs no DNS timeout.
- The replaced assertion is gone: `startswith("if ! curl")` is now five behavioural cases — the
  failure names the host, it names the corpus, it never exits `0`, it exits `EX_TEMPFAIL`, and
  `gate.py`'s own summary and exit code are asserted. A body of `then true; fi` fails all of them.
- **`CHANGELOG.md:71-73` carried both false sentences** and is fenced from implementors; the
  coordinator corrected it at integration (`2e25bf9`). `X-56`'s Progress note carries the same
  correction in full.
- **Reworked after review (round 1), and the review was right.** One of the eight new cases
  repeated the exact mistake this story exists to remove. `test_an_unreachable_rfc_editor_names_
  the_host_it_could_not_reach` asserted the host and the corpus number appeared in
  `stdout + stderr` — but the importer prints `fetching <url>` unconditionally *before* the fetch,
  on stdout, and that one line already contains both. A guard whose entire body is `exit 75` — no
  sentence, no host, no corpus — passed. Measured both ways in a scratch copy:

  | guard body | exit | stderr under a quiet curl | old assertion | new |
  | --- | --- | --- | --- | --- |
  | `exit 75` (silent) | 75 | `''` | **passes** | fails |
  | `true` (no-op) | 1 | `awk: fatal: cannot open file …` | **passes** | fails |
  | the real guard | 75 | names the host and the corpus | passes | passes |

  The fix is to assert on **`stderr` only, under a curl that says nothing**: the `fetching` line is
  on stdout and curl is mute, so the guard's own three messages are the only thing there. Proved
  discriminating by swapping the silent variant into the worktree and running the suite against it
  — `AssertionError: 'www.rfc-editor.org' not found in ''`, then restored and checksum-verified.
  This mattered more than a normally weak test: `AGENTS.md` now names that sentence as the guard's
  entire justification, having just deleted the false one.
- Two smaller things the same review found, both fixed here: an **empty-string argument** still took
  the write path (`case "${1:-}" in "")` cannot tell "no argument" from "an empty one", so
  `./import-…sh ""` rewrote a tampered fixture and exited 0 — the argument dispatch is on `$#` now;
  and the **precedence divergence had no test**, so `test_a_disclaimed_step_does_not_hide_a_step_
  that_really_failed` drives `gate.run()` with synthetic steps and pins exit `1` with `1 of 2 steps
  failed` naming only the red one.
- A machine with **no `curl` at all** exits 127 into the guard and is reported as unreachable. Kept
  deliberately — it is equally true that the RFC could not be read and equally false that the corpus
  drifted — and the message now names the tool as well as the network, so the sentence is accurate
  either way.
- **My earlier report got one number wrong.** I wrote that a `then true; fi` guard exits 2 from
  `awk`; it exits **1**, because under `pipefail` the pipeline takes `grep`'s 1, not `awk`'s 2.
  Immaterial to the design — still not 75, so still caught — but the reasoning was wrong.

## Notes
- **The wiring `X-56` shipped is sound and is not reopened.** Both corpus checks run from a gate step
  and a CI job, `gate.py --check` accounts for them, and a hand-edited fixture is caught — the review
  reproduced the tampered-fixture proof independently, including that `--check` never writes and that
  a second run does not repair the corpus. Everything here is the *fetch guard*, which was a
  coordinator addition on top of that story's Acceptance rather than part of it.
- **This is `X-34`'s doctrine in a third place.** The disk guard refuses to report rather than
  reporting something it cannot stand behind; `gate.py --check` refuses when the gate no longer
  matches CI. A corpus step that cannot reach the RFC knows nothing about the corpus, and saying
  "red" is claiming otherwise.
- Two things only an observed CI run can settle, both raised by the same review and neither blocking:
  whether the `corpus` job executes green on GitHub's runners, and whether `rfc-editor.org` objects to
  two fetches per push from shared runner egress.
