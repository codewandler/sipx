---
id: A-8
title: Declare what each published crate guarantees
pillar: Application
status: ready
priority: 3
design: docs/vision.md
epic: app-sdk
areas: [docs, sipx-app-protocol]
note: alpha predicate 5 — v1 freezes what "stable" means, so the line between stable and experimental has to exist before it can be frozen
---

# Declare what each published crate guarantees

## Goal
Give every published crate an explicit, written statement of which of its public surface is
**stable** and which is **experimental**, so cutting `1.0.0` freezes something that was decided
rather than something that merely happened to compile.

## Acceptance
- [ ] Each published crate states its guarantee in its crate-level documentation, where a reader on
      docs.rs meets it first — not only in a repository markdown file. Eleven crates publish;
      `sipx-testkit` is `publish = false` and needs no promise.
- [ ] **The unit of the promise is stated.** "Stable" must say what may still change: new enum
      variants behind `#[non_exhaustive]`, new struct fields, new trait methods with defaults. This
      project has already shipped three additive `sipx_call::Error` variants that were
      source-breaking for an exhaustive `match`, and said so in the changelog each time — that
      practice becomes the written rule.
- [ ] **Experimental surface is marked at the item, not only in prose.** `sipx-app-protocol` already
      describes itself as experimental; the rule is that a reader looking at one type can tell
      without going up a level.
- [ ] Anything that cannot honestly be called stable before 1.0 is named, with what would settle it.
      An empty list here is a claim, and by this project's standards claims get checked.
- [ ] The declaration is reachable from the README's crate table, so the question "can I depend on
      this" is answered where people ask it.
- [ ] Failing-first evidence: name a crate whose public surface a reader cannot today classify as
      stable or experimental from its own documentation — and the assertion, test or gate step that
      makes it classifiable.

## Findings from the read-only crate sweep (2026-07-29) — start here
A sweep read all 11 publishable crates' manifests, crate-level `//!` blocks and every module-level
`//!` against the corrected registry. What it settles, so this story does not re-derive it:

- **There is no coverage problem, only a content problem.** `missing_docs = "warn"` in
  `[workspace.lints.rust]` plus the gate's clippy `-D warnings` already guarantee every public item
  carries *a* doc comment.
- **`sipx-app-protocol` is the only crate that answers the question**, and it is the model to copy:
  `lib.rs:4-10` (`# Experimental`), `Cargo.toml:3`, a per-crate `README.md:6` — held together by a
  **test**, `tests/spec_tables.rs:337`
  `the_spec_and_the_crate_agree_that_this_is_experimental`, asserting spec ∧ README ∧
  `lib.contains("# Experimental")`. It is also the only crate getting `#[non_exhaustive]` right at the
  item, with a documented deliberate exception for `Output` (`interpreter.rs:208-210`) and a public
  module marked out-of-contract (`testing.rs:1-6`).
- **Acceptance item 2 is a decision to take, not a paragraph to write.** `sipx_call::Error` is **not**
  `#[non_exhaustive]` (`sipx-call/src/error.rs:6-7`) while the same crate's newer types are —
  `CallEvent` (`event.rs:57`), `EndCause` (`event.rs:132`), `Dispatched` (`dispatch.rs:65`),
  `DispatchCounts` (`dispatch.rs:295`). It went **13 variants at v0.8.0 → 16 at v0.9.0**, so each
  release broke any downstream exhaustive `match` — and `website/docs/guides/place-a-call.md:128-133`
  teaches readers to write one (it survives only because the sample happens to carry a `_` arm). The
  only written statement of the rule is `CHANGELOG.md:434-437`, which no docs.rs reader ever opens.
  Every public error enum outside `sipx-app-protocol` is exhaustive today; `sipx-media::Interrupt`
  (`session.rs:590`) is the lone exception. That is the sweep.
- **Acceptance item 5 cannot be satisfied against today's table.** `README.md:107-118` lists ten
  crates: it *includes* `sipx-testkit` (`publish = false`, `Cargo.toml:12`) and *omits* `sipx-app` and
  `sipx-app-protocol` (both publish). `website/docs/guides/as-a-library.md:99-106` lists eight and
  omits the same two plus the CLI. Membership is inverted from "published" before a guarantee is
  written. Fix membership first, mechanically — `X-35` owns the front-door check that would pin it.
- **Acceptance item 6's best failing-first target is `sipx-media`**, not a marginal crate. Its crate
  doc (`src/lib.rs:1-12`) discusses symmetric RTP and the pacing clock and mentions **none** of
  DTLS-SRTP, ICE, bridging or conferencing — four public module trees, two of which the registry marks
  unreachable and two of which are unreachable from `Call`. A reader cannot classify any of it.
- **Two manifest keys are prerequisites, and belong in this story.** No crate sets `readme` and
  `[workspace.package]` does not either, so ten of eleven will have **no README on crates.io** — the
  one-line `description` is the entire landing page, which is why the over-claims `X-35` fixes live in
  descriptions. And no crate sets `[package.metadata.docs.rs]`, while `scripts/build-docs.sh:78` builds
  with `--all-features` — so docs.rs builds defaults and `sipx-media` (`default = []`) and
  `sipx-app-protocol` (`default = []`) will publish pages missing `dtls::openssl`, Opus and
  `event_from_call`. Without `readme` the guarantee is unreachable from crates.io; without
  `all-features = true` docs.rs shows a different API than the guarantee describes.
- **Do not let item 4's list come out empty.** Named, with what settles each: `sipx-media::dtls` and
  `::ice` → `M-27`/`M-28` (a call that selects them); `sipx-call`'s multi-party surface → `C-6`;
  `sipx-app` → the host process; `sipx-app-protocol` → two dissimilar applications, per its own spec;
  `sipx_call::Error` → the `#[non_exhaustive]` decision above.
- **Under-described, not false** (so item 1 has work beyond stability wording): `sipx-ua`'s crate doc
  never names presence, PUBLISH, subscriptions or event packages — a third of its public surface — and
  its `default-features = false` list (`lib.rs:15-16`) is stale by three ungated modules (`packages`,
  `presence`, `subscribe` at `:29,30,33`); `sipx-rtp`'s never mentions `srtp`, `dtmf`, `quality` or
  `rtcp`; `sipx-transport`'s never says TLS/WS/DNS are switchable off.
- **`sipx-cli` needs a decision on where its promise lives.** Bin-only, no `src/lib.rs`; locally
  `cargo doc -p sipx-cli` emits `target/doc/sipx/index.html` under the *binary* name, so a reader
  following a `sipx_cli` link finds nothing.
- Capability over-claims in descriptions — `sipx-call`'s "bridging", `sipx-sip`'s and `sipx-ua`'s
  "dialogs", `sipx-app`'s crate-doc summary — are **`X-35`'s**, not this story's. Do not fix them here;
  they need the front-door check or they drift back.

## Progress
- Not started.

## Notes
- **Alpha predicate 5** (`docs/roadmap.md`, "The v1 gate"). It is the one alpha item that is pure
  decision rather than correction, and the reason the alpha exists at all: **cutting `1.0.0` freezes
  the public API, and this API has not yet been used by anyone outside this repository.** An alpha
  release is how that gets exercised before the freeze.
- Related: the `sipx.app.v1` contract already carries a version in its name and is marked
  experimental, matching its spec's status. That is the model — the promise is written down and the
  version says which promise.
- **Do not turn this into a semver policy essay.** One paragraph per crate that a person deciding
  whether to depend on it can act on. The vision's non-goals discipline applies to documentation as
  much as to features.
- Worth checking against `docs/rfc/registry.toml` while writing: a crate that publishes a capability
  the registry marks `partial` should not describe that surface as stable without saying which half
  works — the same honesty rule `X-30` and `M-28` applied to the table.
