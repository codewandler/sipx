# sipx — working agreement

A SIP and VoIP stack in Rust. Read [docs/vision.md](docs/vision.md) once; it is the
tie-breaker when a design choice is unclear.

## Non-negotiables

1. **Never reference third-party prior-art projects in this repository.** All design
   rationale cites RFCs or our own specs in `docs/specs/`. This applies to code, comments,
   docs, story and design files, test fixture names, package metadata and commit messages.
   `scripts/check-provenance.sh` enforces it in CI and in the pre-commit hook; install the
   hook with `git config core.hooksPath .githooks`.
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
5. **Never commit without an explicit instruction from the user.**

## The gate

Before marking any story done:

```sh
./scripts/gate.py          # --list to see the steps, --check to verify the gate itself
```

The gate is a script and not a list of commands here, because a list has to be transcribed
correctly and once was not. CI has always run an `msrv` job that the list never named, so an
implementor could run every documented command, see green, and tag a release that does not build
on the Rust version it advertises — which is what happened. That job was red from v0.4.0 through
v0.7.0, two published releases, and nobody was told for five days, because nothing anyone ran
locally covered it (fixed in `f761878`).

Two properties keep that from recurring, both enforced by `./scripts/gate.py --check`, which is
itself a gate step and a CI job:

- **The gate cannot omit a CI job.** The check reads `.github/workflows/ci.yml`: every command a
  job runs is either mirrored by a gate step or named in the script's `NOT_RUN_LOCALLY` list with
  a reason, and a flag CI passes that the local step drops counts as drift too — `cargo check`
  without `--all-targets` is a green gate and a red CI one argument down. Adding a job fails the
  check until somebody decides which it is.
- **This section cannot fall behind the gate.** The block above may invoke the script and say
  nothing else, so there is no second copy of the list here to drift.

The **MSRV step** is the one that was missing. It builds the workspace on the toolchain named by
the workspace `rust-version` in `Cargo.toml`, read at run time and written down nowhere else —
not here, not in the script. If that toolchain is not installed the step **fails** and prints the
`rustup toolchain install` line to fix it; skipping it would restore the exact hole it closes.
The steps run under `ci.yml`'s own `env:` block, so the gate builds with the flags CI builds with
rather than a friendlier set.

Beyond a Rust toolchain the gate needs three things: that MSRV toolchain, node >= 20 for
`build-docs.sh`, which builds `website/` and the API reference, and **free disk** — a run leaves
about 10.6 GiB of build artifacts, so the gate refuses to start below roughly 11.7 GiB rather than
reporting a result it cannot stand behind (`X-34`). The number is not written here twice: it is
measured, and `scripts/gate.py` prints the threshold and the actual free space when it refuses.

`rfc-report.py` regenerates `docs/compliance.md` from `docs/rfc/registry.toml` and checks that
its claims hold: a named header must be known to the parser, a cited file must exist, and an
entry claiming implementation or partial support must cite Rust source in a workspace crate —
prose alongside it is welcome, prose alone is not evidence. The rule was "cites something" until RFC
8996 satisfied it with a spec paragraph saying TLS 1.2 is the floor: a citation that cannot fail
(`X-43`). `docs/rfc/README.md` is the schema, and lists every way `--check` fails. **When a story
changes what sipx supports, update the registry in the same commit.** The table is linked from the
README as a measurement, and a measurement that lags the code is worse than no table at all.

`check-pool-key.py` holds `docs/specs/sip-transport.md` §8 against `ConnectionKey`, whose fields
the section lists in a generated region. The list used to be prose in three specs and was wrong
in one of them through two changes to the type — nobody was told, because nothing connected the
sentence to the field. **When a story changes `ConnectionKey`, run `./scripts/check-pool-key.py
--update` in the same commit**, and give any new field its paragraph in `sip-tls.md` §5: the
script generates *which* fields are in the key, never *why*.

`maturity.py` regenerates `docs/maturity.md`, and it reports the alpha predicates in
`docs/roadmap.md` from story frontmatter: a story says which predicate it bears on with
`predicate: 3` (or `predicate: [3, 7]`), and a predicate stays open until every story declaring it is
`done`. **File a defect against a predicate by setting that field in the story** — there is no list
anywhere else to update, deliberately, because the list that used to live in the script drifted:
three defects were filed against predicate 3 in one session, none of them was added to it, and the
report was one story away from calling the alpha complete (`X-42`). A `predicate:` naming a predicate
the roadmap does not have fails the gate rather than being quietly dropped.

`check-features.sh` is not optional garnish. `--all-features` hides a whole class of breakage:
an optional transport that does not compile with its feature turned off is invisible until a
downstream user turns it off, and that is exactly how `tls` came to be broken for a release.

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
