---
id: X-65
title: Assert the branch and tag RNG is cryptographic
pillar: Build
status: done
design: docs/designs/input-hardening.md
epic: input-hardening
areas: [sipx-sip, sipx-transport, beta4]
predicate:
announcement: 2
note: spec says cryptographic because a guessable branch is a response-injection primitive · nothing fails if it stops being · beta-1
---

# Assert the branch and tag RNG is cryptographic

## Goal

Make the Via branch and tag generator's cryptographic property fail a test when it stops holding,
rather than depending on review of the call site.

## Acceptance

- [x] A test asserts the generated Via branch carries the RFC 3261 §8.1.1.7 `z9hG4bK` magic cookie
      and the full documented entropy width, over a sample large enough that a truncated or
      counter-derived generator fails it.
- [x] The property is pinned by construction as well as statistically: swapping the generator for a
      non-cryptographic source fails to compile or fails a test, and the story's Progress log records
      that demonstration.
- [x] Dialog tags (RFC 3261 §19.3) are covered by the same assertions.
- [x] The statistical bound states its arithmetic in a comment on the line — the chosen threshold and
      the resulting false-failure rate — so the test cannot become a retry.
- [x] `./scripts/gate.py` green, including `check-fixed-sleep.py`.

## Progress

- The original generators already drew one `u64` from `rand::rng()`, so the failing-first question
  was whether either source could silently be replaced by a merely deterministic `RngCore`. Both
  generation seams now require `rand::CryptoRng`; the public branch and private dialog-token paths
  call only through those seams.
- Scratch-replacing each production `rand::rng()` with
  `rand::rngs::mock::StepRng::new(0, 1)` made `cargo check -p sipx-transport --lib` and
  `cargo check -p sipx-call --lib` fail with `E0277: the trait bound StepRng: CryptoRng is not
  satisfied`, at `branch_with_rng` and `token_with_rng` respectively. The scratch edits were then
  removed.
- The branch and dialog-tag suites each draw 4,096 identifiers, require the branch's `z9hG4bK`
  cookie, require exactly sixteen lowercase hexadecimal digits, and count ones independently in
  every one of the 64 positions. The accepted interval is 1,664 through 2,432. Hoeffding's bound
  gives `2 exp(-2 * 384^2 / 4096)` per position; a union bound over both generators' 128 positions
  is below `1.4e-29`. The arithmetic is repeated on the assertion line so changing the threshold
  cannot leave this explanation behind.
- The permanent counter-negative test feeds values 0 through 4,095 to the same width guard and
  proves it rejects the 52 fixed high bits. In a second scratch mutation, both generators drew only
  `next_u32` and widened that value to the advertised `u64`; both width tests failed at bit 32 with
  zero ones in 4,096 samples (exit 101), then passed again after restoring `next_u64`.
- `cargo test -p sipx-transport --lib via_branch -- --nocapture` passed both branch tests;
  `cargo test -p sipx-transport --lib the_width_guard_rejects_a_counter -- --nocapture` passed the
  mutation guard; and `cargo test -p sipx-call --lib dialog_tag -- --nocapture` passed both tag
  tests. `cargo clippy -p sipx-transport -p sipx-call --all-targets --all-features -- -D warnings`,
  `cargo fmt --check`, and `./scripts/check-fixed-sleep.py --check` are also green. The complete
  workspace gate remains the coordinator's combined-wave check, so this story stays in progress.

## Notes
- `docs/specs/sip-transport.md:110` states the requirement and the reason: a guessable branch lets an
  off-path attacker inject responses. The generator satisfies it today; nothing detects a change.
- Small story, deliberately separate from `X-64`: a different property, a different failure, and it
  should not ride green on another story's result.
- Do not weaken the assertion to make it fast. If a large sample is slow, sample in one test rather
  than spreading a weak assertion across several.
