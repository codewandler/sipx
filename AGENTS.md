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
5. **Never commit without an explicit instruction from the user.**

## The gate

Before marking any story done:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
./scripts/check-provenance.sh
```

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
