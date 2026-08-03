---
id: S-32
title: Refuse a numeric flag that was given something that is not a number
pillar: Signalling
status: done
priority: 5
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-cli]
note: found by S-30 — `Args::number` conflates "absent" with "not a number", so `sipx answer --wait notanumber` exits 0 after the default 60s; `dial::numeric` already fixed it for `dial` alone, which is the tell that it was patched per-command instead of at the source
---

# Refuse a numeric flag that was given something that is not a number

## Goal
Make the CLI refuse a numeric flag whose value is not a number, instead of falling back to a default
the user did not ask for.

## Acceptance
- [x] **A numeric flag given a non-number is a usage error.**
      `sipx answer --local 127.0.0.1:0 --wait notanumber` currently exits 0 having waited the default
      60 seconds. It must exit non-zero naming the flag and what it wanted. The cause is `Args::number`
      returning the same "nothing usable here" answer for an absent flag and for an unparseable one, so
      the caller takes its default branch either way.
- [x] **Fixed at the source, not per command.** `dial::numeric` already handles this correctly for
      `dial` and nowhere else, which is the evidence that the last person to hit it patched their own
      command rather than the shared helper. Whatever shape is chosen must make the mistake
      unavailable to the next command rather than fixed in one more place. `dial::numeric` should
      then have nothing left to do and should go.
- [x] **Every numeric flag is covered, derived from a list rather than enumerated.** Same standard as
      `S-30`: assert against the registry of flags so a numeric flag added later cannot escape the
      test. `every_valued_flag_in_the_help_text_is_registered` is the precedent.
- [x] **Range and overflow are decided, not left to `parse`.** State per flag what values are
      accepted — a negative `--wait`, a zero `--expires`, a value beyond the type's range — and assert
      the boundaries. A flag that accepts `0` and one that refuses it should differ because someone
      decided, not because of which integer type it happens to parse into.
- [x] **The refusals are documented** in `website/docs/reference/cli.md`'s usage-error paragraph,
      where the other refusals are enumerated.
- [x] Failing-first test: a `--bin sipx` test that passes a non-number to a numeric flag and requires
      a non-zero exit naming it. Name it, and show it red before the fix.

## Progress

- Implemented. `Args::new` now establishes both argument invariants before a command validates its
  URI, address, or opens a socket: every valued flag has a non-empty value, and every documented
  `<S>` flag is an integer in `0..=u32::MAX`. The shared `NUMERIC_FLAGS` registry is checked in both
  directions against the commands' help text, so adding a seconds flag without validation fails a
  unit test. `dial::numeric` is gone; `dial`, `answer`, and `register` all consume the constructor's
  guarantee through `Args::number`.
- The boundary rule is explicit rather than inherited from an integer type. Negative values,
  fractions, suffixed values and overflow are refused. Zero is retained deliberately for all four
  current flags, with a separate meaning recorded in the CLI reference: immediate call duration,
  transaction-layer timeout, immediate wait, or binding removal.
- Failing-first evidence: `a_non_number_is_refused_by_every_numeric_flag` initially failed because
  `dial --duration notanumber` continued to URI validation and reported `not a SIP URI: not-a-uri`.
  It now derives every `<S>` flag from each command's own `--help` and observes exit 2 naming the
  flag and its whole-number domain before command-specific validation.

## Notes
- **Found by `S-30`**, which fixed the neighbouring defect: a *valued* flag given no value at all. This
  is the same conflation one field over — `Args::value` could not distinguish "flag was last" from
  "flag was absent", and `Args::number` cannot distinguish "absent" from "unparseable". Both make a
  user's typed intent vanish into a default.
- **Filed separately from `S-30` deliberately**, and not because it is unrelated. `S-30`'s rework may
  well leave the fix trivially adjacent, and it was told to leave it alone anyway: one scoped change is
  reviewable, two braided ones are not, and the failing-first evidence for each stays legible.
- **It bears on alpha predicate 6** — "testable from a shell for everything the CLI exposes" — for the
  same reason `S-30` does. A flag that silently takes a default is exposed and not honoured: the
  predicate's letter holds while its spirit fails, because the flag is testable and the test would pass
  while the flag does nothing.
- The `--wait` case is the worst of the family found so far, because the fallback is a *60-second*
  default. A script that mistypes the value does not fail fast; it hangs for a minute and then reports
  success.
