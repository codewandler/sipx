---
id: S-30
title: Refuse a valued flag that was given no value, instead of ignoring it
pillar: Signalling
status: in-progress
priority: 5
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-cli]
note: found by S-29's review — `Args::value` cannot tell "--flag was last" from "--flag was absent", so a trailing valued flag is accepted and dropped; CLI-wide, and it defeats the accepted-and-dropped rule S-29 asserted for its own six flags
---

# Refuse a valued flag that was given no value, instead of ignoring it

## Goal
Make the CLI refuse an argument list it cannot honour, so that no flag a user typed is silently
ignored. Today a valued flag in final position is indistinguishable from one that was never given,
and the command proceeds on a default the user did not ask for.

## Acceptance
- [ ] **A valued flag with no value is a usage error, not a default.**
      `sipx register sip:alice@example.com --outbound --instance` currently proceeds and generates a
      fresh instance URN; it must exit with a usage error naming the flag. `Args::value`
      (`crates/sipx-cli/src/main.rs:152-165`) returns `iter.next().map(String::as_str)`, so a flag in
      final position yields `None` — the same answer as absent — and every caller then takes its
      absent-branch.
- [ ] **The fix is in `Args`, not in each call site.** The defect is one function's return type
      conflating two outcomes; fixing it per-flag would leave the next flag to rediscover it. Whatever
      shape is chosen has to make "given but empty" unrepresentable-or-typed rather than relying on
      each caller to check twice.
- [ ] **Every valued flag is covered, not only the four `S-29` added.** The same hole applies to the
      pre-existing `--target` and `--expires`, and to every flag registered in `VALUED_FLAGS`. Derive
      the test from that list so a flag added later cannot escape it — the existing
      `every_valued_flag_in_the_help_text_is_registered` is the precedent for testing against the
      registry rather than against an enumeration.
- [ ] **`--flag=` with an empty right-hand side is decided explicitly.** `value` already handles
      `--flag=value` by prefix strip, so `--flag=` yields `Some("")`. State whether an empty value is
      a usage error or a legitimate value per flag, and assert it; today it is neither documented nor
      tested.
- [ ] **The refusals are documented** in `website/docs/reference/cli.md`'s usage-error paragraph,
      which is where the other refusals are enumerated.
- [ ] Failing-first test: a `--bin sipx` test that runs a valued flag in final position and requires a
      non-zero exit naming the flag. Name it.

## Notes
- **Found by the independent review of `S-29`**, not by the suite. `S-29`'s own Acceptance required
  that its six new flags never be "accepted-and-dropped", and it satisfied that for a *wrong value* —
  six refusal tests — while the *missing value* case fell through `Args::value` underneath it. So this
  is not a regression `S-29` introduced; it is the pre-existing floor its rule was standing on.
- **Why this is worth its own story rather than a line in `S-29`.** It is CLI-wide and predates that
  diff, it touches argument plumbing every subcommand shares rather than the registration path, and
  the fix belongs in one function whose blast radius is every flag. Merging it into `S-29` would have
  mixed a scoped feature with a shared-surface change and made the failing-first evidence for both
  harder to read.
- **It bears on alpha predicate 6** — "testable from a shell for everything the CLI exposes". A flag
  that is accepted and ignored is exposed but not honoured, which is the predicate's spirit failing
  even while its letter holds: the flag *is* testable, and the test would pass while the flag does
  nothing.
- The defect is symmetric with a shell habit that makes it easy to hit: trailing a command with the
  flag you are about to give a value to, then losing the value to shell quoting or a line edit. The
  silent path means the command still exits 0 with a registration the user did not describe.
