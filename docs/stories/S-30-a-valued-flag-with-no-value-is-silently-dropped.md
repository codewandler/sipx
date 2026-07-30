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
- [x] **A valued flag with no value is a usage error, not a default.**
      `sipx register sip:alice@example.com --outbound --instance` currently proceeds and generates a
      fresh instance URN; it must exit with a usage error naming the flag. `Args::value`
      (`crates/sipx-cli/src/main.rs:152-165`) returns `iter.next().map(String::as_str)`, so a flag in
      final position yields `None` — the same answer as absent — and every caller then takes its
      absent-branch.
- [x] **The fix is in `Args`, not in each call site.** The defect is one function's return type
      conflating two outcomes; fixing it per-flag would leave the next flag to rediscover it. Whatever
      shape is chosen has to make "given but empty" unrepresentable-or-typed rather than relying on
      each caller to check twice.
- [x] **Every valued flag is covered, not only the four `S-29` added.** The same hole applies to the
      pre-existing `--target` and `--expires`, and to every flag registered in `VALUED_FLAGS`. Derive
      the test from that list so a flag added later cannot escape it — the existing
      `every_valued_flag_in_the_help_text_is_registered` is the precedent for testing against the
      registry rather than against an enumeration.
- [x] **`--flag=` with an empty right-hand side is decided explicitly.** `value` already handles
      `--flag=value` by prefix strip, so `--flag=` yields `Some("")`. State whether an empty value is
      a usage error or a legitimate value per flag, and assert it; today it is neither documented nor
      tested.
- [x] **The refusals are documented** in `website/docs/reference/cli.md`'s usage-error paragraph,
      which is where the other refusals are enumerated.
- [x] Failing-first test: a `--bin sipx` test that runs a valued flag in final position and requires a
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

## Progress
Implemented on `impl/S-30`. Written after the runs it describes, not before — see `X-39`'s
neighbours in the log for why that distinction is spelled out here.

**The shape.** `Args::new` is now fallible: `Result<Args, String>`. Holding an `Args` means every
valued flag on the line was given a non-empty value, so `Args::value` still returns
`Option<&str>` and its `None` now means *absent* and nothing else. That is what the second
Acceptance item asked for — the check exists once, in the constructor, keyed off `VALUED_FLAGS`,
and no call site re-checks anything. The 15-odd `value`/`number` call sites are untouched.

The four subcommands each changed in the same two ways, and neither is a per-flag check: the
`--help` branch moved ahead of construction (so `--help` still prints when another flag on the
line is malformed), and the constructor's `Err` is rendered through the existing
`fail(format, Exit::Usage, &message)`. That is a fallible constructor having its error turned into
an exit code at the boundary, identical in all four files.

The rejected alternative was `value() -> Result<Option<&str>, String>`, which pushes the decision
out to every caller and lets any of them write `.ok().flatten()` — the "check twice" outcome the
story rules out.

**Empty right-hand side: a usage error, for every flag, uniformly.** Nothing in `VALUED_FLAGS` has
a meaningful empty value — not a password, not a path (`--play`, `--record`, `--book`), not an
address (`--local`, `--target`, `--from`), not a count of seconds (`--timeout`, `--duration`,
`--wait`, `--expires`), not an identity (`--instance`, `--push-provider`, `--push-prid`,
`--push-param`), not `--dtmf`, whose empty string means "send no digits" — which is already what
omitting the flag means. Since omitting a flag is how a caller asks for the default, an empty value
can only be an accident, and it is the accident a shell produces: `--target "$ADDR"` with `ADDR`
unset. A per-flag exception list would be a second registry to hold in step with the first, for no
case that wants one. Both spellings are refused: `--flag=` and `--flag ""`.

**Failing-first evidence.** `e4da36c` carries the test and no production change (`git diff
52733e2 e4da36c -- crates/sipx-cli/src/` is empty). Against merge-base sources,
`a_valued_flag_given_no_value_is_refused_by_every_command` fails on its first case:

    `sipx register sip:alice@example.com --json --password` must name --password in its refusal:
    {"status":"usage","error":"cannot reach example.com: give --target host:port, ..."}

and the merge-base binary silently proceeded for the rest — `dial ... --json --play` exit 5 with
the clip dropped, `peers --json --book` exit 0 off the fallback book, `answer ... --json --record`
exit 5 having announced `listening`, `dial ... --dtmf=` exit 5. The story's headline case was
confirmed on the wire: with `--target` pointed at a local UDP socket,
`register sip:alice@example.com --outbound --instance` sent
`Contact: <sip:alice@127.0.0.1:56930>;reg-id=1;+sip.instance="<urn:uuid:574f679d-…>"` — an identity
generated because the flag was read as absent.

**Coverage.** `every_valued_flag_is_refused_when_it_is_given_no_value` (`main.rs`) iterates
`VALUED_FLAGS` itself, so a flag added to the registry later is covered with no new case;
`a_valued_flag_given_no_value_is_refused_by_every_command` (`tests/cli.rs`) derives the flags from
each command's own `--help` output, which reaches the exit code through the binary. Two guards
against over-reach: `a_valueless_flag_is_untouched_by_the_rule` and
`an_equals_in_the_positional_is_not_a_flag` (a URI parameter is spelled with `=`).

**Not done here.** A valued flag whose value is *another flag* — `--target --tcp` — still binds
`"--tcp"` as the value. Every such case is refused downstream today by the value's own validation,
and refusing it here would need a registry of valueless flags that does not exist, so it is left
alone rather than half-built.
