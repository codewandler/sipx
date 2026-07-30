---
id: P-7
title: Make `sipx dial --password` authenticate, or reject the flag
pillar: Application
status: done
design: docs/designs/phone.md
epic: cli
areas: [sipx-cli]
note: main.rs:168 accepts --password on dial and dial.rs never reads it, so a 407-challenged call fails while the user who supplied credentials is told nothing
---

# Make `sipx dial --password` authenticate, or reject the flag

## Goal
Stop `sipx dial --password …` from silently doing nothing with the password. Either the dial path
answers a challenge with it, or the flag is rejected on `dial` so the user learns immediately.

## Acceptance
- [ ] **A 407 or 401 on an outbound INVITE is answered when a password was supplied.** Today it is
      not: `crates/sipx-cli/src/main.rs:168` lists `--password` among the valued flags — globally, so
      `dial` accepts it, and `main.rs:193` asserts that `["dial", "--password", "secret", "sip:a@b"]`
      parses — while `crates/sipx-cli/src/dial.rs` contains no reference to it. Only
      `register.rs:53` reads a password.
- [x] **`register` is the working model**: `register.rs:94-95` builds
      `Credentials::new(user, password)` into the UA config. Whether the same shape fits `dial` is the
      substance of this story — a call's challenge is answered per-transaction, not per-registration —
      so check it rather than assume it transfers.
- [x] **Rejecting the flag on `dial` is an acceptable outcome** if wiring authentication is larger
      than it looks. What is not acceptable is accepting it and discarding it. If it is rejected, the
      help text at `dial.rs:32`'s block must not advertise it either.
- [x] The existing security note is preserved: `register.rs:22` and `:51-53` say a password in argv is
      world-readable and prefer `SIPX_PASSWORD`. Whatever `dial` does, it makes the same point and
      reads the same environment variable.
- [x] Failing-first test: a challenged INVITE with `--password` supplied fails today. Name the test.

## Progress
- **Done by refusing the flag, which the Acceptance named as acceptable — and it turned out to be the
  only honest option.** The first item is `[~]`, not `[x]`: a challenged call still does not
  authenticate. What changed is that it no longer pretends to.
- **The reason it could not be wired here.** `grep -rn "Credentials" crates/sipx-call/src` returns
  **nothing**, and so does a grep for `407`, `Unauthorized` or `ProxyAuth` in `call.rs`. There was no
  half-built path to finish; the concept does not exist in that crate, and `sipx-call` does not even
  depend on `sipx-ua`, where the digest machinery lives. That is a feature, filed as `S-28`, not a CLI
  fix.
- `dial` now fails with `Exit::Usage` and a message that says a call cannot answer a challenge yet,
  names `register` as the command that does authenticate, and points at `S-28`. The flag was never in
  `dial`'s help text, so nothing advertised it.
- **Mutation-checked rather than assumed**: with the refusal disabled the test fails `left: 5,
  right: 2` — `Timeout` instead of `Usage` — and takes 22 s, because without it the command really does
  place the call and discard the password. That wall-clock is the defect.
- **The refusal is a placeholder and says so at the site.** `S-28`'s Acceptance names the test to delete
  when the feature lands, so this cannot quietly become the permanent answer. Found by `X-33`'s implementor while reading `sipx-cli`; the flag/handler asymmetry was
  verified at integration by grepping both files.

## Notes
- The shape is the same as the day's other findings — **an interface that promises something no code
  behind it delivers** — but one layer further out than `X-35`'s: not a capability table claiming a
  feature, an actual accepted command-line flag. `X-35`'s new front-door guard cannot see this,
  because a flag is not a capability word in a doc string.
- Reads with `S-27`, found in the same pass over the same crate and probably worth doing together:
  both are `sipx dial` accepting input whose meaning it then drops. `S-27` is priority 1 because a
  silent security downgrade is worse than a silent authentication failure — this one at least ends in
  a visible call failure.
- `crates/sipx-cli/tests/cli.rs` is where the CLI's surface is asserted. Note that `X-33` found one
  design claim citing `cli.rs:116` as exercising digest authentication when that line is
  `register_advertises_this_client_in_via_and_contact`, and the tree contains no
  `password`/`401`/`407`/`Authorization` test at all — so there is no existing coverage to extend
  here, only coverage to write.
