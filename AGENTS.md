# sipx — working agreement

A SIP and VoIP stack in Rust. Read [docs/vision.md](docs/vision.md) once; it is the
tie-breaker when a design choice is unclear.

## Non-negotiables

1. **Never reference third-party prior-art projects in this repository.** All design
   rationale cites RFCs or our own specs in `docs/specs/`. This applies to code, comments,
   docs, story and design files, test fixture names, package metadata and commit messages.
   `scripts/check-provenance.sh` enforces it in CI and in the pre-commit hook; install the
   hook with `git config core.hooksPath .githooks`.

   **One exception, and it is a path list rather than a judgement call** (`X-71`): a
   *comparison subject* may be named in the comparison registry under `docs/comparison/`,
   in the `docs/comparison.md` generated from it, and on the public page generated from
   that. Nowhere else. The scope lives in `COMPARISON_SCOPE` in the check itself, so
   widening it is a reviewable diff and not a re-reading of this paragraph. A comparison
   whose subjects are anonymous cannot cite evidence anyone can check, which is why the
   exception exists at all — and it buys nothing outside that one artifact.

   Two things the exception does **not** cover. **Design rationale**, which still cites
   RFCs and our own specs: `docs/vision.md` principle 5 is unchanged, and a spec or design
   doc naming another implementation still fails. **Commit messages**, which stay denied
   absolutely — `--history` has no exception, so name a subject in the data and never in
   the message that lands it. That is what keeps the one failure with no cheap remedy,
   *"history must be rewritten before this repository is published"*, out of reach.
2. **The core does no I/O.** `sipx-sip` and `sipx-sdp` must not gain an async runtime, a
   socket, or a clock read. Time enters as a fired-timer input; bytes enter as data. If you
   find yourself wanting `tokio` in either crate, the logic belongs in `sipx-transport` or
   `sipx-media` instead.
3. **No panics on network input, no `unsafe`.** `unsafe_code` is forbidden workspace-wide.
   Parse failures are typed errors. `unwrap`, `expect`, `panic` and raw indexing are lint
   warnings — in library code, treat them as errors.
4. **Spec before code.** Non-trivial subsystems get a spec in `docs/specs/` first: normative
   RFC references, types, state tables, timers, and byte-level test vectors. Tests are
   derived from the spec's vectors.

   Test modules opt out of the no-panic lints — a test that cannot read its own fixtures
   should fail loudly. Annotate the module, not the crate:

   ```rust
   #[cfg(test)]
   #[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
   mod tests { … }
   ```
5. **Background work must be bounded and cancellation-safe.** Never create CPU busy loops or
   unbounded spinner processes to simulate load. Prefer a bounded, representative test workload.
   If a background process is genuinely necessary, give it a finite timeout, arrange cleanup with
   an `EXIT`/`INT`/`TERM` trap that terminates its entire process group, and wait for that cleanup
   before reporting the task complete. A trailing `kill $pid` is not sufficient: an interrupted
   shell leaves its children orphaned.
6. **Never commit without an explicit instruction from the user.**

## The gate

Before marking any story done, run the repository's complete local acceptance gate:

```sh
./scripts/gate.py          # --list shows the steps; --check verifies the gate against CI
```

Do not copy individual gate commands into this file or substitute a hand-picked subset. The script
reads the workspace MSRV and CI environment from their authoritative sources, verifies that every CI
job is accounted for, and runs the feature matrix as well as the all-features build. A missing MSRV
toolchain is a failure, not a skip.

A full run requires the MSRV toolchain, Node.js 20 or newer, network access, and enough free disk for
the build artifacts. The script checks disk space before starting. Some checks recover RFC fixtures
from the RFC editor; if those sources cannot be reached, the gate exits `2` to report that the run is
incomplete rather than claiming the tree passed or failed. Exit `1` means the tree has a real
finding.

## Before selecting a story

```sh
./scripts/check-story-closure.py
```

A story whose Acceptance is satisfied but whose `status` was never moved reads as available work and
is not. `A-16` was delivered on 2026-08-05 with seven of its eight rows ticked, left at `backlog`,
and dispatched to a second implementor three days later — over a contract six downstream stories
already cited. This reconciles a story's status against its own Acceptance, which nothing else does:
the gate checks that the board agrees with the frontmatter, not that the frontmatter agrees with
itself.

It is **a report and not a gate step**, deliberately, and the script's docstring carries the evidence
for that decision along with the exact rule it uses to stay quiet about a story somebody is still
implementing. The pre-commit hook runs it on every commit that touches `docs/stories/`.

## Keep derived artifacts synchronized

When a change affects one of these sources, update its derived artifact in the same change. The gate
checks each relationship; use the named update command rather than editing generated output by hand.

| Change | Required follow-up |
|---|---|
| Supported RFC behavior | Update `docs/rfc/registry.toml`; its schema and checks are documented in `docs/rfc/README.md` |
| `ConnectionKey` fields | Run `./scripts/check-pool-key.py --update`, then explain any new field in `docs/specs/sip-tls.md` §5 |
| Story status, priority, title, or epic | Regenerate `docs/stories/README.md` with `/track:board` |
| Release version or comparison evidence | Run `./scripts/comparison-report.py --check`; refresh expired observations from evidence, never by changing only `evaluated_at` |

Additional invariants enforced by the gate:

- `docs/maturity.md` is generated from roadmap predicates and story frontmatter. File a defect against
  a predicate with the story's `predicate:` field; an unknown predicate is an error.
- Comparison subjects may appear only in the comparison scope described in non-negotiable 1. Our own
  comparison cells are generated and tied to the workspace version; do not type their values.
- A fixed wall-clock duration may bound a failure or define silence, but may not substitute for a
  happens-before relation. If `check-fixed-sleep.py` reports a wait, wait for the event or classify
  the duration in an inline comment using one of the checker's accepted reasons.
- Every corpus under `crates/sipx-testkit/corpus/` is recovered from its RFC — the RFC 4475 and
  RFC 5118 message archives, and RFC 7714's published AES-GCM vectors. Never edit their fixture
  bytes by hand; each has an importer whose `--check` is a gate step.
- `--all-features` is not a replacement for `./scripts/check-features.sh`; optional configurations
  must also compile with features disabled.

<!-- BEGIN track:agents -->
## Start here (every session) — track backlog

This project tracks work with the **track** framework: every unit of work is a markdown story in
`docs/stories/`, and the board (`docs/stories/README.md`) is generated from story frontmatter.

1. **Orient** — read the latest user request, then run `git status --short --branch`. Treat
   uncommitted changes as user-owned unless you made them.
2. **What to work on** — if the user named work, do that. Otherwise open the
   [board](docs/stories/README.md) and take the top `ready` story by `priority` (lower = higher).
   `/track:next` reports it; `/track:next <area>` filters by optional story `areas`.
3. **The contract** — read the story's `## Goal` and `## Acceptance`; Acceptance defines "done". Read
   any linked `design:`.
4. **Do the work** — set the story `in-progress`; non-trivial design goes in `docs/designs/` first;
   implement; satisfy Acceptance with a **failing-first test**; keep the project's gate green.
5. **On done** — `/track:done <ID>`: set `status: done`, add a CHANGELOG entry, regenerate the board.
6. **New or unscoped work?** Create a story first (`/track:story`) so the next agent inherits the
   context.

The board's status lists are generated — after any change to a story's `status`/`priority`/`title`/
`epic`, run `/track:board`. Use optional `areas: [subsystem]` tags for query-only subsystem selection
without changing board rows. Story frontmatter is the single source of truth.
<!-- END track:agents -->
