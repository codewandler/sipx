---
id: X-58
title: Make an unreachable RFC editor a non-result rather than a finding, and delete the false reason for the guard
pillar: Build
status: ready
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
- [ ] **An unreachable RFC editor exits `2`, not `1`.** `scripts/gate.py` defines the contract at
      `:663-665` — `0` green, `1` the tree is wrong, `2` the run is not a result — and it exists
      because "a full disk and a broken diff used to leave the same exit code and print the same way,
      so nothing could tell a finding from a non-finding". Today a failed fetch exits 1, so
      `gate.py` appends `("rfc 5118 corpus", "exit 1")` and prints `gate: N of 25 steps failed`: a red
      gate that reads as a corpus that drifted. Either the importers exit with a code the gate reads
      as infrastructure, or `infrastructure_evidence` (`scripts/gate.py:758`) learns the fetch
      failure's shape — decide which and say why. The two mechanisms are not the same: a distinct
      exit code is the script's own claim, and a pattern in `infrastructure_evidence` is the gate's
      inference about a step it does not control.
- [ ] **The failing-first test is a fetch that cannot succeed**, not a string match on the source.
      Point the importer at a host that cannot resolve — or otherwise force the fetch to fail —
      and assert what `gate.py` *reports*: the summary line and the exit code, not a sentence in the
      streamed output. That is where `X-34` put the property and it is the half the current test
      never reaches.
- [ ] **Replace the shape assertion with a behavioural one.** `scripts/test-gate.py:1667-1677`
      requires the fetch line to `startswith("if ! curl")`, which pins spelling rather than
      behaviour. Review proved it both ways: a guard whose body is `then true; fi` — no message, no
      exit, falling through to base64-decode a file that does not exist — **passes**, while an
      equivalent `curl … || { echo …; exit 1; }` **fails**. Assert the two properties the docstring
      claims instead: the failure says which host could not be reached, and it never becomes a skip.
- [ ] **Delete the false reason, everywhere it was copied.** `AGENTS.md:128-129` says "`curl -f`
      prints nothing and a bare exit code reads as a corpus that changed". The flags in use are
      `-fsSL`, and `-S` is *show errors*: `curl -fsSL <bad-url> -o /tmp/x` prints
      `curl: (22) The requested URL returned error: 404` and exits 22. The guard may still be worth
      having — it turns curl's exit code into a sentence naming the corpus and the host — but it must
      be justified by what it actually does. The same premise is repeated in `scripts/gate.py:136-138`,
      both `scripts/import-rfc*-corpus.sh` guard comments, `CHANGELOG.md` and `X-56`'s Progress.
      **`AGENTS.md` is the file every future agent reads as the why**, and a why that one command
      disproves is the defect this project keeps filing stories about.
- [ ] **Correct "the gate's only checks that reach the network".** `AGENTS.md:118` claims it, and
      `scripts/build-docs.sh:111-113` runs `npm ci`/`npm install` whenever `website/node_modules` is
      absent — which is every fresh implementor worktree, since it is gitignored. The `docs site`
      step has always reached the network. Same overclaim in `CHANGELOG.md` and in `X-56`.
- [ ] **An unrecognised flag must not take the write path.** `[[ "${1:-}" == "--check" ]] && check_only=1`
      (`import-rfc5118-corpus.sh:31`, `import-rfc4475-corpus.sh:25`) means `--check=1`, `-check` or a
      typo silently selects rewrite mode, which would overwrite a tampered fixture with the RFC's own
      bytes and exit 0 — a green step that erased the evidence. Pre-existing, but `X-56` added four
      invocation sites, which is what makes it worth closing now. Refuse an unknown argument.

## Progress
- Filed 2026-07-31 by the independent review of `X-56`, which reproduced every item above in scratch
  copies rather than reasoning about them: the no-op guard passing the suite, the `||` form failing
  it, curl's actual output at the actual flags, and `gate.py`'s classifier returning `None` for the
  fetch failure so the step lands in the red tally.

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
