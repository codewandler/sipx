---
id: M-80
title: Decide non-exhaustive for the two relayed packet structs
pillar: Media
status: done
priority: 7
design:
epic: media
areas: [sipx-media, sipx-rtp]
predicate:
announcement:
note: two field additions in two stories, both breaking · the choice stops being reversible at 1.0
---

# Decide non-exhaustive for the two relayed packet structs

## Goal

Settle, while it is still reversible, whether `sipx_media::Encoded` and `sipx_rtp::Packet` carry
`#[non_exhaustive]` — and record the reason either way.

## Why

`M-75` added a field to `Packet`, `M-79` added one to `Encoded`, and each broke in-tree struct
literals. Both are public structs with public fields and a `new` constructor, so they already have
the shape `#[non_exhaustive]` would force, and `crates/sipx-media/src/lib.rs` currently reserves the
right to add fields in a minor release. That reservation and the missing attributes are two answers
to the same question, and `docs/roadmap.md`'s v1 predicate 4 is the point after which only one of
them can still be chosen.

The enum guard in `scripts/check-audio-claims.py` matches `pub enum` only, so nothing in the
repository has an opinion about these two. That is the gap.

## Acceptance

- [x] The decision is made **for both types together** — marking one and not the other is arbitrary
      — and recorded where a reader meets the type, not only in a story.
      → `crates/sipx-rtp/src/packet.rs:31` and `crates/sipx-media/src/session.rs:726`, each a
      `# Stability` section on the type's own doc comment naming the other.
- [x] `crates/sipx-media/src/lib.rs`'s stability sentence and the attributes agree. Whichever way it
      goes, the other side moves.
      → `crates/sipx-media/src/lib.rs:19` and the identical sentence at `crates/sipx-rtp/src/lib.rs:16`.
- [x] If `#[non_exhaustive]` is added, every in-tree construction site uses the constructor, the
      `CHANGELOG.md` entry says what a downstream literal should become, and a test proves the
      constructor reaches every field a literal could set.
      → construction sites below; the two tests are
      `crates/sipx-rtp/src/packet.rs` `the_constructor_reaches_every_field_a_literal_could_set` and
      the same name in `crates/sipx-media/src/session.rs`. **The CHANGELOG sentence is in this
      note, not in `CHANGELOG.md`** — that file is the coordinator's.
- [x] Whatever rule is chosen is **enforced**, so the next field addition cannot re-open this by
      accident: extend the audio-claims guard to public structs on the declared media surface, or
      state in the checker why structs are deliberately out of its scope.
      → `struct_problems` / `breakable_structs` / `MEDIA_SURFACE` in `scripts/check-audio-claims.py`,
      wired into `main` and covered by `TheStructExtensibilityRule` in `scripts/test-audio-claims.py`.
- [x] `./scripts/gate.py` green.
      → not run: one gate per wave is the coordinator's. See the pre-existing failure below.

## Progress

- 2026-08-08: filed from the independent review of `M-79`'s diff, which established the precedent
  chain and the deadline rather than merely raising the question.

- 2026-08-08: **decided — both types carry `#[non_exhaustive]`.**

  The argument is an asymmetry rather than a prediction about future fields. `#[non_exhaustive]`
  can be **removed** in any minor release without breaking a caller, and can only be **added** in a
  major one. Marking now therefore keeps both answers reachable, and shipping `1.0.0` unmarked
  spends the choice permanently. At `1.0.0-rc.10` only one of the two options is still
  reversible, and it is this one.

  The supporting evidence is a rate: `M-75` added `Packet::extension` and `M-79` added
  `Encoded::extension`, two additive changes in two consecutive stories, each of which broke in-tree
  struct literals. Both types mirror a wire format that grows. And the cost to a caller is close to
  nothing, because these are types you mostly *receive*: the fields stay `pub`, readable and
  assignable, and only the literal is withdrawn.

  `crates/sipx-media/src/lib.rs` already reserved the right to add fields in a minor release. That
  reservation and the missing attribute were the two answers the story named; the attribute is what
  makes the reservation something a downstream can act on rather than something it absorbs.

  **Construction sites changed** (the compiler enumerated them; there were fewer than expected
  because both types already had constructors most callers used):
  - `crates/sipx-media/src/session.rs` — the relay path's `Encoded { .. }` literal, now
    `Encoded::new` plus an `extension` assignment. In-crate, so it still compiled; converted anyway
    because it is the relay, and so the exact migration a downstream relay has to make.
  - `crates/sipx-media/src/session.rs` — four `DtmfEvent { .. }` literals in the test module, now
    `DtmfEvent::new` plus `end = true`. These were the only hard compile failures, all cross-crate.
  - No `Packet { .. }` literal existed outside `sipx-rtp`.

  **Scope note.** The rule below is not satisfiable by marking only two types. Any boundary that
  fired on exactly `Encoded` and `Packet` would be the suppression list this checker has always
  refused, so the four other types the rule selects are marked too, each with a pointer to the
  argument above: `sipx_media::session::Config`, `sipx_media::browser::SelectedComponent`,
  `sipx_media::ice::Gathering`, `sipx_rtp::dtmf::Event`. All four are protocol shapes that grow.

- 2026-08-08: **enforcement.** `scripts/check-audio-claims.py` gained a struct half to its
  extensibility rule. The general rule: a reachable public struct with a public field is as
  breakable by an additive change as a public enum, and the file's own comment claiming otherwise
  ("a struct can add a private field without breaking a caller") was true only of structs whose
  fields are private. The corrected selector reports around 190 types workspace-wide, so the rule
  runs behind two boundaries, both stated where they are defined:

  - `MEDIA_SURFACE = ("sipx-media", "sipx-rtp")` — a crate boundary, the same shape as the enum
    rule's `GUARDED_SURFACE`.
  - `breakable_structs` — a *type* boundary, and a property rather than a list: the struct must
    already publish a `pub fn new`. A crate that ships `T::new(..)` and leaves every field `pub` has
    published two construction paths and can evolve only one, which is precisely how `M-75` and
    `M-79` turned additive changes into breaking ones. A public-field struct with no constructor is
    different work rather than the same work deferred — marking one leaves a caller no way to build
    it — and this boundary widens by itself when such a type gains a `new`.

  The remainder is printed on every run next to the enum rule's, never suppressed: **155 reachable
  public-field structs, 29 of them inside the two guarded crates**. Filed as `M-92`.

- 2026-08-08: **failing-first**, run at merge base `1c7d1c4` with the Rust tree untouched and only
  the two scripts in place:

  ```
  $ ./scripts/check-audio-claims.py --check          # EXIT=1
  crates/sipx-media/src/browser.rs:146 `SelectedComponent` is reachable from the crate root, has
    public fields and publishes a constructor; add `#[non_exhaustive]` or an adjacent
    `/// Complete by design:` rationale
  crates/sipx-media/src/ice/gather.rs:39 `Gathering` … (same)
  crates/sipx-media/src/session.rs:381 `Config` … (same)
  crates/sipx-media/src/session.rs:728 `Encoded` … (same)
  crates/sipx-rtp/src/dtmf.rs:113 `Event` … (same)
  crates/sipx-rtp/src/packet.rs:33 `Packet` … (same)
  ```

  and the same at the unit-test level:

  ```
  $ python3 -m unittest \
      scripts.test-audio-claims.TheRepositoryItself\
  .test_every_breakable_public_struct_is_non_exhaustive_or_argued_at_the_type
  AssertionError: Lists differ: [] != [… 6 additional elements …]
  FAILED (failures=1)
  ```

- 2026-08-08: **owed CHANGELOG sentence** (not written here — `CHANGELOG.md` is fenced):

  > **Breaking.** `sipx_media::Encoded`, `sipx_media::Config`, `sipx_media::browser::SelectedComponent`,
  > `sipx_media::ice::Gathering`, `sipx_rtp::Packet` and `sipx_rtp::dtmf::Event` are now
  > `#[non_exhaustive]`, so a struct literal naming their fields no longer compiles outside these
  > crates. Replace `Packet { payload_type, sequence, .. }` with `Packet::new(payload_type,
  > sequence, timestamp, ssrc, payload)` and assign the remaining fields, which stay `pub`; the same
  > for the other five and their `new`. Added before `1.0.0` deliberately: the attribute can be
  > removed in a minor release and only added in a major one, so this is the last release in which
  > the choice is reversible (`M-80`).

- 2026-08-08: **pre-existing failure, not from this diff.**
  `scripts/test-audio-claims.py`'s `TheModuleReader.test_it_reads_the_modules_the_crate_declares`
  asserts a hardcoded module list for `sipx-audio` that no longer matches the crate — it has gained
  `dsp`, `dsp::conformance` and `dsp::contract`. Reproduced at merge base `1c7d1c4` in a clean
  worktree with no changes applied: `Ran 73 tests … FAILED (failures=1)`. Left alone rather than
  fixed: the assertion belongs to the DSP epic that added those modules, and updating it would be
  asserting a crate shape this story never examined.

- 2026-08-08: closed at the `1.0.0-rc.11` boundary, against the wave gate run on this tree.
