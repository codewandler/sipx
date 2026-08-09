---
id: M-83
title: Extend the extensibility guard past the media path
pillar: Media
status: done
priority: 9
design:
epic: media
areas: [sipx-sip, sipx-ua, sipx-app, sipx-transport, sipx-testkit]
predicate:
announcement:
note: 98 reachable public enums outside the guarded five crates · the boundary names crates, so no enum is individually excused
---

# Extend the extensibility guard past the media path

## Goal

Bring the remaining reachable public enums under the rule `M-78` established, so the guarded set is
the whole published surface rather than the five crates the first pass could review in one diff.

## Why

`M-78` replaced a name-based selector with reachability from the crate root, and `M-74` resolved
what that surfaced on the media path: 46 enums, 25 now `#[non_exhaustive]` and 21 carrying a written
argument for being exhaustive. The measurement also killed the story's premise — this workspace
declares most modules `pub`, so reachability barely narrows the set. Workspace-wide it is 162.

The rest were left outside `GUARDED_SURFACE` deliberately, because resolving 144 enums across eleven
crates in one diff is unreviewable and would have collided with every other branch in flight. The
boundary names **crates and never enums**, so nothing is individually excused, and every run prints
the outstanding count. This story is what closes it.

Remaining: `sipx-sip` 33, `sipx-ua` 24, `sipx-app` 18, `sipx-transport` 13, `sipx-testkit` 10.
`sipx-cli` is a binary and has none; `sipx-app-protocol`'s 18 stay excluded under `A-9`; `sipx-wasm`
does not publish. The cross-crate breakage in the first pass was 14 sites in 6 files — far smaller
than it sounds, so this may be cheaper than the count suggests.

## Acceptance

- [x] Every reachable public enum in the five named crates is `#[non_exhaustive]` or carries an
      adjacent `/// Exhaustive by design:` argument, with the reason recorded **at the type**.
- [x] `GUARDED_SURFACE` is retired, or reduced to exactly the crates with a stated reason for being
      outside it. The checker's outstanding count reaches zero, or prints what remains and why.
- [x] Each enum that becomes `#[non_exhaustive]` is a breaking change for exhaustive `match` arms
      and gets one `CHANGELOG.md` statement covering the set, saying what a downstream arm must add
      and what it buys.
- [x] A failing-first proof: with the boundary widened and the enums untouched, the checker reports
      them; after, it is quiet.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-08: filed from `M-78`'s own measurement, which is the only reason the size of this is
  known rather than guessed.
- 2026-08-09: implemented. The count had grown from the note's 98 to **100** between filing and
  pickup, and it is now **0**.

  **Failing-first.** `GUARDED_SURFACE` was retired first, with every enum untouched.
  `./scripts/check-audio-claims.py --check` then exited 1 naming all hundred, one line each:

  ```
  the crate front doors advertise what the crates do not implement, or do not say what they guarantee:
    crates/sipx-app/src/config/mod.rs:222 `Protocol` is reachable from the crate root and exhaustive; add `#[non_exhaustive]` or an adjacent `/// Exhaustive by design:` rationale
    …
    crates/sipx-ua/src/subscribe.rs:197 `Answer` is reachable from the crate root and exhaustive; add `#[non_exhaustive]` or an adjacent `/// Exhaustive by design:` rationale
  ```

  Afterwards it exits 0 and prints
  `11 crates hold every reachable public enum non-exhaustive or argued; 18 reachable enums in
  sipx-app-protocol stay exhaustive, which A-9 decided because that crate's vocabulary is closed
  and versioned`.

  **The boundary is gone rather than widened.** `guarded` is now derived from `published` the way
  the rest of the check is, so a crate added later joins the enum rule by existing. What is left
  outside is `CLOSED_VOCABULARY` alone, which is a stated decision about one crate and not a
  rollout's convenience — and its 18 are still counted and printed, because an exclusion nobody
  prints is a suppression list. `MEDIA_SURFACE` is now the file's only rollout boundary and its
  comment says so.

  **The split: 61 marked, 39 argued.** The argued ones are the enums closed by something outside
  this workspace — a normative grammar (`ContactValue`, `Host`, `OcParameter`), a normative table
  (`session::Answer` is RFC 4028 §9 Table 2; `Timer` is §17 Table 4), a two-valued RFC element
  (`Basic`, `Refresher`, `packages::Direction`, `Keepalive`), or a complete partition of the
  inputs (`Publish` is the case analysis of `Publish::read`; `rel::Received` is three integer
  comparisons; `Verdict` is right/wrong crossed with fresh/stale). Everything whose variant set is
  a registry, a taxonomy of failures, or a driver instruction vocabulary is marked.

  **`Timer` is argued, and it is the one worth re-reading.** It was marked first and reverted: the
  type publishes `Timer::ALL: [Self; 13]` as its own complete set, so `#[non_exhaustive]` would
  promise growth while `ALL` promises none, and `sipx-testkit`'s `timer_row` exists specifically
  so a fourteenth variant is a compile error. The attribute would have turned that into a
  wildcard returning a bogus table row.

  **Cross-crate breakage: 21 sites in 15 files**, against `M-78`'s 14 in 6. The expensive ones
  were the three transport enums (`UriTransport`, `TransportKind`, `event_client::Transport`),
  because the mirror mappings between them stop being total: `sipx-call`'s `from_transport` /
  `to_transport` / `transport`, and `sipx-cli`'s `event_peer`, now return `Option` and their
  callers decline to send rather than approximate a transport. That is deliberate — every total
  fallback available downgrades a protected flow — and it is the one behavioural change in the
  diff. `sipx-transport::counters::unsent` lost a documented compile-time guarantee to
  `Method`'s attribute; its doc comment now says so and points at `slot`, which keeps the
  guarantee because `TransportKind` is that crate's own type.

  **CHANGELOG sentence owed** (the coordinator writes it; this story may not):

  > **Breaking:** every reachable public enum in `sipx-sip`, `sipx-transport`, `sipx-ua`,
  > `sipx-app` and `sipx-testkit` is now `#[non_exhaustive]` or carries a written argument for
  > being exhaustive, which completes the rollout `M-74` began on the media path and retires the
  > checker's `GUARDED_SURFACE` boundary. A downstream `match` on one of the 61 marked types —
  > `Method`, `Scheme`, `UriTransport`, `TransportKind`, the transaction `Output` and `TuEvent`,
  > and the event and publication client vocabularies among them — needs a `_` arm it did not
  > need before. What it buys is that sipx can add a method, a transport or a driver instruction
  > in a minor release instead of a major one, which after `1.0.0` freezes the API is a choice
  > that can no longer be made.

  **Filed `M-97` on the way past.** The guard tests `"#[non_exhaustive]" in above` against the
  whole preamble, so a doc comment that *mentions* the attribute satisfies the rule. Three
  arguments written here explained themselves by naming it and were silently reclassified from
  argued to marked; they were caught only by reconciling the 61/39 split against the list of 100,
  which nothing in the gate does. One type in the workspace passes on prose alone today —
  `sipx-media`'s `ProviderKind`, whose argument is sound and is written in the wrong form —
  and `sipx-app-protocol`'s `Output` has the same shape outside the rule. Not fixed here: it is a
  defect in the checker's reader rather than in this story's surface.

- 2026-08-09: **open** — the `gate` row is unticked. Per the throughput contract this implementor
  did not run `./scripts/gate.py`; the wave gate covers it. Verified here instead:
  `python3 -m unittest scripts/test-audio-claims.py` (104 tests), `check-audio-claims.py --check`,
  `check-provenance.sh`, `cargo check`/`clippy --workspace --all-targets --all-features
  -D warnings`, `cargo fmt --all`, `cargo test` for all eight changed crates, and `cargo doc`.

- 2026-08-09: closed at the `1.0.0-rc.14` boundary, against the wave gate run on this tree.
